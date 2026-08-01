//! Everything the in-game state keeps *for drawing*, and nothing else.
//!
//! [`SceneCache`] owns every GPU resource the session has uploaded — chunk
//! meshes, animated player and mob models, the crack overlay, the selection
//! outline, drops and arrows — plus the camera parameters and the animation
//! clocks that feed them.
//!
//! Pulling it out of `InGameState` does two things. It gives the render state
//! one owner instead of scattering nineteen mesh fields through a struct that
//! also holds the world and the player. And it confines `RenderContext` — the
//! Vulkan device handle — to this module, so chunk streaming, mob simulation
//! and block interaction became plain logic with no GPU dependency.
//!
//! The simulation produces [`CpuMesh`] data; this module is the only thing that
//! turns it into a [`GpuMesh`].

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;

use glam::Vec3;

use super::mobs::{RemoteMob, mob_mesh};
use super::{OUTLINE_COLOR, REMOTE_MAX_SPEED, THIRD_PERSON_DISTANCE};
use crate::core::{Aabb, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle};
use crate::entity::{AnimationState, Arrow, DroppedItem, HumanoidModel, Mob, Perspective, Player};
use crate::inventory::{ARMOR_SIZE, Inventory, ItemId, ItemRegistry};
use crate::net::{PlayerId, RemotePlayer};
use crate::render::{
    Camera, CpuMesh, GpuLines, GpuMesh, LightParams, PreviewFrame, RenderContext, SceneFrame,
    SkyParams, debug, tiles,
};
use crate::world::World;
use crate::world::block::{BlockRegistry, FaceTextures};
use crate::world::meshing::{mesh_block_overlay, mesh_chunk, push_item_cube};

/// Animation state for a remote player plus the position used to derive their
/// speed (no extra protocol data needed — movement is inferred from the change
/// in rendered position each frame).
struct RemoteAnim {
    anim: AnimationState,
    last_pos: Vec3,
}

/// How the player model is posed for the inventory preview. Set by the UI,
/// read when the preview mesh is rebuilt.
#[derive(Default, Clone, Copy)]
pub(super) struct PreviewPose {
    /// Yaw the model is turned to by dragging.
    pub yaw: f32,
    /// Where the head looks — (yaw, pitch) toward the cursor. Purely cosmetic.
    pub look: (f32, f32),
}

/// All GPU state for the in-game scene.
pub(super) struct SceneCache {
    /// Opaque GPU meshes for loaded chunks, keyed by chunk position.
    meshes: HashMap<ChunkPos, GpuMesh>,
    /// Transparent (water/glass) GPU meshes, drawn in a second blended pass.
    transparent_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Pending mesh rebuilds (budgeted across frames), with a dedup set.
    mesh_queue: VecDeque<ChunkPos>,
    queued: HashSet<ChunkPos>,

    /// Local player model + its GPU mesh (only built in third person).
    player_model: HumanoidModel,
    player_mesh: Option<GpuMesh>,
    /// Procedural animation state for the local player's model.
    player_anim: AnimationState,
    /// Player-model mesh for the inventory preview (built only while the
    /// inventory is open), and how it is posed.
    preview_mesh: Option<GpuMesh>,
    pub preview: PreviewPose,

    remote_meshes: Vec<GpuMesh>,
    /// Per-remote-player animation, keyed by id.
    remote_anims: HashMap<PlayerId, RemoteAnim>,
    /// One GPU mesh per visible mob, rebuilt each frame like remote players.
    mob_meshes: Vec<GpuMesh>,

    /// Crack overlay drawn on the block being mined (rebuilt as progress grows).
    break_mesh: Option<GpuMesh>,
    /// Selection outline on the targeted block, cached until the target changes.
    outline_block: Option<BlockPos>,
    outline_mesh: Option<GpuLines>,
    /// Combined mesh for all arrows (rebuilt per frame, like drops).
    arrows_mesh: Option<GpuMesh>,
    /// Combined GPU meshes for all drops, split by render pass.
    drops_mesh: Option<GpuMesh>,
    drops_mesh_transparent: Option<GpuMesh>,

    pub fov_degrees: f32,
    /// Fraction `[0,1)` through the current physics step, for camera
    /// interpolation between fixed steps.
    pub render_alpha: f32,
    /// Seconds since entering the state; drives shader animation (water frames).
    pub elapsed: f32,
}

impl SceneCache {
    pub fn new() -> Self {
        Self {
            meshes: HashMap::new(),
            transparent_meshes: HashMap::new(),
            mesh_queue: VecDeque::new(),
            queued: HashSet::new(),
            player_model: HumanoidModel::player(),
            player_mesh: None,
            player_anim: AnimationState::new(),
            preview_mesh: None,
            preview: PreviewPose {
                yaw: std::f32::consts::PI,
                look: (0.0, 0.0),
            },
            remote_meshes: Vec::new(),
            remote_anims: HashMap::new(),
            mob_meshes: Vec::new(),
            break_mesh: None,
            outline_block: None,
            outline_mesh: None,
            arrows_mesh: None,
            drops_mesh: None,
            drops_mesh_transparent: None,
            fov_degrees: 70.0,
            render_alpha: 0.0,
            elapsed: 0.0,
        }
    }

    // --- Chunk meshes ---------------------------------------------------------------

    /// Chunk meshes currently uploaded (debug HUD).
    pub fn loaded_mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Chunk meshes waiting to be rebuilt (debug HUD).
    pub fn queued_mesh_count(&self) -> usize {
        self.mesh_queue.len()
    }

    /// Drop a chunk's meshes when it unloads.
    pub fn forget_chunk(&mut self, pos: ChunkPos) {
        self.meshes.remove(&pos);
        self.transparent_meshes.remove(&pos);
    }

    /// Move freshly-dirtied chunks into the mesh queue (deduped).
    pub fn enqueue_dirty(&mut self, dirty: impl IntoIterator<Item = ChunkPos>) {
        for pos in dirty {
            if self.queued.insert(pos) {
                self.mesh_queue.push_back(pos);
            }
        }
    }

    /// Rebuild up to `budget` chunk meshes this frame.
    pub fn process_mesh_budget(
        &mut self,
        ctx: &Arc<RenderContext>,
        world: &World,
        blocks: &BlockRegistry,
        budget: usize,
    ) {
        for _ in 0..budget {
            let Some(pos) = self.mesh_queue.pop_front() else {
                break;
            };
            self.queued.remove(&pos);

            let output = world
                .chunk(pos)
                .map(|chunk| mesh_chunk(chunk, blocks, |p| world.block_at(p)));
            match output {
                Some(output) => {
                    match GpuMesh::upload(&ctx.memory_allocator, &output.opaque) {
                        Ok(Some(mesh)) => {
                            self.meshes.insert(pos, mesh);
                        }
                        Ok(None) => {
                            self.meshes.remove(&pos);
                        }
                        Err(err) => log::error!("opaque mesh upload failed at {pos:?}: {err:?}"),
                    }
                    match GpuMesh::upload(&ctx.memory_allocator, &output.transparent) {
                        Ok(Some(mesh)) => {
                            self.transparent_meshes.insert(pos, mesh);
                        }
                        Ok(None) => {
                            self.transparent_meshes.remove(&pos);
                        }
                        Err(err) => {
                            log::error!("transparent mesh upload failed at {pos:?}: {err:?}")
                        }
                    }
                }
                // Chunk was unloaded before we got to it.
                None => self.forget_chunk(pos),
            }
        }
    }

    // --- Animated models ------------------------------------------------------------

    /// Advance the local player's animation clock.
    pub fn advance_player_anim(&mut self, speed: f32, dt: f32) {
        self.player_anim.advance(speed, dt);
    }

    /// Trigger the main-hand swing on the local player's model.
    pub fn trigger_swing(&mut self) {
        self.player_anim.trigger_swing();
    }

    /// Rebuild the player model mesh in third person; drop it in first person.
    pub fn update_player_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        inventory: &Inventory,
        items: &ItemRegistry,
    ) {
        if player.perspective.is_first_person() {
            self.player_mesh = None;
            return;
        }
        let pose = self.player_anim.pose(player.pitch);
        let armor = inventory.equipped_armor();
        let mesh =
            self.player_model
                .build_mesh_armored(player.position, player.yaw, &pose, &armor, items);
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    /// Rebuild the inventory-preview player mesh: the model at the origin,
    /// turned to the preview yaw, wearing the currently equipped armor. Only
    /// built while the inventory is open; cleared otherwise so the offscreen
    /// pass is skipped entirely.
    pub fn update_preview_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        inventory_open: bool,
        player: &Player,
        inventory: &Inventory,
        items: &ItemRegistry,
    ) {
        if !inventory_open {
            self.preview_mesh = None;
            return;
        }
        // The preview head tracks the cursor (yaw + pitch), independent of the
        // world player's facing; limbs still idle-animate from the anim state.
        let mut pose = self.player_anim.pose(player.pitch);
        pose.head_yaw = self.preview.look.0;
        pose.head_pitch = self.preview.look.1;
        let armor = inventory.equipped_armor();
        let mesh = self.player_model.build_mesh_armored(
            Vec3::ZERO,
            self.preview.yaw,
            &pose,
            &armor,
            items,
        );
        self.preview_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    /// Rebuild GPU meshes for remote players, advancing each one's animation
    /// from the movement observed since the previous frame.
    pub fn update_remote_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        remote_players: &HashMap<PlayerId, RemotePlayer>,
        items: &ItemRegistry,
        dt: f32,
    ) {
        self.remote_meshes.clear();
        // Snapshot the render-relevant fields first so we can mutate
        // `remote_anims` and read `player_model` without holding a borrow.
        type Snapshot = (PlayerId, Vec3, f32, f32, [Option<u16>; ARMOR_SIZE]);
        let snapshots: Vec<Snapshot> = remote_players
            .values()
            .map(|rp| (rp.id, rp.position(), rp.yaw, rp.pitch, rp.armor))
            .collect();
        for (id, pos, yaw, pitch, armor_ids) in snapshots {
            let state = self.remote_anims.entry(id).or_insert_with(|| RemoteAnim {
                anim: AnimationState::new(),
                last_pos: pos,
            });
            let delta = pos - state.last_pos;
            let speed =
                (Vec3::new(delta.x, 0.0, delta.z).length() / dt.max(1e-4)).min(REMOTE_MAX_SPEED);
            state.anim.advance(speed, dt);
            state.last_pos = pos;
            let pose = state.anim.pose(pitch);

            let armor = armor_item_ids(armor_ids, items);
            let mesh = self
                .player_model
                .build_mesh_armored(pos, yaw, &pose, &armor, items);
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.remote_meshes.push(gpu);
            }
        }
        // Drop animation state for players that have left.
        self.remote_anims
            .retain(|id, _| remote_players.contains_key(id));
    }

    /// Rebuild one mesh per visible mob — the authority's own simulated mobs
    /// plus, on a client, the host's replicas (whose animation is driven from
    /// their rendered movement, like remote players).
    pub fn update_mob_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        mobs: &[Mob],
        remote_mobs: &mut HashMap<u64, RemoteMob>,
        dt: f32,
    ) {
        self.mob_meshes.clear();
        for mob in mobs {
            if let Some(mesh) = mob_mesh(&mob.visual, mob.position, mob.yaw, &mob.anim.pose(0.0))
                && let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh)
            {
                self.mob_meshes.push(gpu);
            }
        }
        for rm in remote_mobs.values_mut() {
            let (visual, position, yaw, pose) = rm.animate(dt);
            if let Some(mesh) = mob_mesh(visual, position, yaw, &pose)
                && let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh)
            {
                self.mob_meshes.push(gpu);
            }
        }
    }

    /// Rebuild the combined arrow mesh (small cubes, like the drops pass).
    pub fn update_arrows_mesh(&mut self, ctx: &Arc<RenderContext>, arrows: &[Arrow]) {
        let mut mesh = CpuMesh::new();
        let shaft = FaceTextures::uniform(tiles::WOOD_BARK);
        for arrow in arrows {
            push_item_cube(&mut mesh, arrow.position, 0.15, arrow.yaw(), &shaft);
        }
        self.arrows_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    // --- World overlays -------------------------------------------------------------

    /// Rebuild the combined drop meshes (opaque + transparent passes). Drops are
    /// few and tiny, so a per-frame rebuild stays cheap, like remote players.
    pub fn update_drops_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        drops: &[DroppedItem],
        textures: impl Fn(ItemId) -> (FaceTextures, bool),
    ) {
        let mut opaque = CpuMesh::new();
        let mut transparent = CpuMesh::new();
        for item in drops {
            let (faces, is_transparent) = textures(item.stack.item);
            let target = if is_transparent {
                &mut transparent
            } else {
                &mut opaque
            };
            push_item_cube(
                target,
                item.render_center(),
                item.size(),
                item.spin_yaw(),
                &faces,
            );
        }
        self.drops_mesh = GpuMesh::upload(&ctx.memory_allocator, &opaque)
            .ok()
            .flatten();
        self.drops_mesh_transparent = GpuMesh::upload(&ctx.memory_allocator, &transparent)
            .ok()
            .flatten();
    }

    /// (Re)build the crack overlay for the block being mined; drop it when idle.
    /// Cheap enough to rebuild every frame (six quads).
    pub fn update_break_overlay(
        &mut self,
        ctx: &Arc<RenderContext>,
        breaking: Option<(BlockPos, f32)>,
    ) {
        self.break_mesh = breaking.and_then(|(block, progress)| {
            let overlay = mesh_block_overlay(block, tiles::crack_tile(progress));
            match GpuMesh::upload(&ctx.memory_allocator, &overlay) {
                Ok(mesh) => mesh,
                Err(err) => {
                    log::error!("break overlay upload failed at {block:?}: {err:?}");
                    None
                }
            }
        });
    }

    /// (Re)build the selection outline on the targeted block. The geometry only
    /// depends on the block position, so it's cached until the target changes.
    pub fn update_target_outline(&mut self, ctx: &Arc<RenderContext>, target: Option<BlockPos>) {
        if target == self.outline_block {
            return;
        }
        self.outline_block = target;
        self.outline_mesh = target.and_then(|block| {
            let mut vertices = Vec::new();
            debug::push_block_outline(&mut vertices, block, OUTLINE_COLOR);
            match GpuLines::upload(&ctx.memory_allocator, &vertices) {
                Ok(lines) => lines,
                Err(err) => {
                    log::error!("selection outline upload failed at {block:?}: {err:?}");
                    None
                }
            }
        });
    }

    // --- Frames ---------------------------------------------------------------------

    /// The camera for this frame, placed by the player's perspective. Physics
    /// ticks at a fixed rate, so the eye is blended between steps to stay smooth
    /// when the display runs faster than the simulation.
    pub fn camera(&self, player: &Player, aspect: f32) -> Camera {
        let mut camera = Camera::new(self.fov_degrees, aspect);
        let eye = player.interpolated_eye_position(self.render_alpha);
        let look = player.look_direction();
        match player.perspective {
            Perspective::First => {
                camera.position = eye;
                camera.forward = look;
            }
            Perspective::ThirdBack => {
                camera.position = eye - look * THIRD_PERSON_DISTANCE;
                camera.forward = look;
            }
            Perspective::ThirdFront => {
                camera.position = eye + look * THIRD_PERSON_DISTANCE;
                camera.forward = -look;
            }
        }
        camera
    }

    /// Collect this frame's visible geometry: frustum-culled chunk meshes plus
    /// every entity and overlay mesh, split by render pass.
    pub fn scene_frame(
        &self,
        player: &Player,
        day_cycle: &DayCycle,
        aspect: f32,
    ) -> SceneFrame<'_> {
        let camera = self.camera(player, aspect);
        let frustum = camera.frustum();
        let in_view = |pos: &ChunkPos| {
            let origin = pos.origin();
            let aabb = Aabb::new(
                Vec3::new(origin.x as f32, 0.0, origin.z as f32),
                Vec3::new(
                    (origin.x + CHUNK_SIZE) as f32,
                    CHUNK_HEIGHT as f32,
                    (origin.z + CHUNK_SIZE) as f32,
                ),
            );
            frustum.intersects_aabb(aabb)
        };

        // Frustum-cull chunk meshes by their column AABB.
        let mut opaque: Vec<&GpuMesh> = self
            .meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();
        let mut transparent: Vec<&GpuMesh> = self
            .transparent_meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();

        // Crack overlay on the block being mined, blended over everything else.
        // No frustum check: the target is a single nearby block within reach.
        if let Some(mesh) = &self.break_mesh {
            transparent.push(mesh);
        }

        // The local player model (third person only) + remote players + mobs.
        if let Some(mesh) = &self.player_mesh {
            opaque.push(mesh);
        }
        opaque.extend(&self.remote_meshes);
        opaque.extend(&self.mob_meshes);
        if let Some(mesh) = &self.arrows_mesh {
            opaque.push(mesh);
        }

        // Dropped items, split by pass like the blocks they represent.
        if let Some(mesh) = &self.drops_mesh {
            opaque.push(mesh);
        }
        if let Some(mesh) = &self.drops_mesh_transparent {
            transparent.push(mesh);
        }

        let atmo = day_cycle.atmosphere();
        SceneFrame {
            view_proj: camera.view_projection(),
            sky: SkyParams {
                inv_view_proj: camera.sky_inv_view_proj(),
                sun_dir: atmo.sun_dir,
                zenith_color: atmo.zenith_color,
                horizon_color: atmo.horizon_color,
                sun_color: atmo.sun_color,
                star_intensity: atmo.star_intensity,
                moon_intensity: atmo.moon_intensity,
            },
            light: LightParams {
                light_dir: atmo.light_dir,
                light_color: atmo.light_color,
                ambient: atmo.ambient,
            },
            time: self.elapsed,
            opaque,
            transparent,
            lines: self.outline_mesh.as_ref(),
        }
    }

    /// The offscreen player-model preview, when the inventory is open.
    pub fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        let model = self.preview_mesh.as_ref()?;
        // Fixed head-height orbit looking at the model built at the origin.
        // The preview image is 0.48:1 (see PREVIEW_SIZE in app.rs).
        let target = Vec3::new(0.0, 1.05, 0.0);
        let mut camera = Camera::new(32.0, 0.48);
        camera.position = Vec3::new(0.0, 1.05, 3.9);
        camera.forward = (target - camera.position).normalize();
        // Neutral, mostly-ambient light so the model reads clearly against the
        // dark preview backdrop, independent of the world's time of day.
        Some(PreviewFrame {
            view_proj: camera.view_projection(),
            light: LightParams {
                light_dir: Vec3::new(0.3, 0.8, 0.5).normalize(),
                light_color: Vec3::splat(0.9),
                ambient: 0.55,
            },
            model,
        })
    }
}

/// Resolve wire armor ids to `ItemId`s the local registry knows, dropping any
/// out-of-range id (a peer running divergent content is already refused, but be
/// safe: raw ids index the registry directly).
fn armor_item_ids(
    ids: [Option<u16>; ARMOR_SIZE],
    items: &ItemRegistry,
) -> [Option<ItemId>; ARMOR_SIZE] {
    ids.map(|id| id.and_then(|raw| ((raw as usize) < items.len()).then_some(ItemId(raw))))
}

impl super::InGameState {
    /// Bring every GPU resource in line with the simulation state this frame.
    ///
    /// This is the single seam where rendering meets simulation: it is the only
    /// method in the in-game state that touches a [`RenderContext`]. Everything
    /// above it — streaming, mobs, fluids, interaction — is plain logic that
    /// runs without a GPU, which is what makes it testable.
    pub(super) fn refresh_view(&mut self, ctx: &Arc<RenderContext>, dt: f32) {
        // Overlays on the block under the crosshair.
        let breaking = self.breaking.as_ref().map(|b| (b.block, b.progress));
        self.view.update_break_overlay(ctx, breaking);
        let target = if self.dead {
            None
        } else {
            self.targeted_block().map(|hit| hit.block)
        };
        self.view.update_target_outline(ctx, target);

        // Chunk meshes: queue what the world dirtied, then spend the budget.
        let dirty = self.world.take_dirty();
        self.view.enqueue_dirty(dirty);
        self.view
            .process_mesh_budget(ctx, &self.world, &self.blocks, super::MESH_BUDGET);

        // Loose entities.
        let (items, blocks) = (&self.items, &self.blocks);
        self.view.update_drops_mesh(ctx, &self.drops, |item| {
            let is_transparent = items
                .get(item)
                .place_block
                .is_some_and(|b| blocks.get(b).is_transparent());
            (
                super::interaction::drop_textures(item, items, blocks),
                is_transparent,
            )
        });
        self.view
            .update_mob_meshes(ctx, &self.mobs, &mut self.remote_mobs, dt);
        self.view.update_arrows_mesh(ctx, &self.arrows);

        // Animated humanoids. The local player settles to idle while the
        // inventory is open (movement is frozen).
        let local_speed = if self.inventory_open {
            0.0
        } else {
            let v = self.player.velocity;
            Vec3::new(v.x, 0.0, v.z).length()
        };
        self.view.advance_player_anim(local_speed, dt);
        self.view
            .update_player_mesh(ctx, &self.player, &self.inventory, &self.items);
        self.view.update_preview_mesh(
            ctx,
            self.inventory_open,
            &self.player,
            &self.inventory,
            &self.items,
        );
        self.view
            .update_remote_meshes(ctx, &self.peers.players, &self.items, dt);
    }
}

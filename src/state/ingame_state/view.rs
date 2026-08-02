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
use crate::content::ItemModel;
use crate::core::{Aabb, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle};
use crate::entity::{
    AnimationState, Arrow, DroppedItem, HandAnchor, HumanoidModel, Mob, Perspective, Player,
};
use crate::inventory::{ARMOR_SIZE, Inventory, ItemId, ItemRegistry};
use crate::model::mesh as model_mesh;
use crate::model::{ModelId, ModelRegistry};
use crate::net::{PlayerId, RemotePlayer};
use crate::render::{
    Camera, CpuMesh, GpuLines, GpuMesh, LightParams, PreviewFrame, RenderContext, SceneFrame,
    SkyParams, Texture, TexturedMesh, debug, tiles,
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

/// Everything the view needs to draw geometry loaded from model files: the
/// parsed models, and which one each item uses. The two always travel together,
/// so they are passed as one borrow rather than threaded separately through
/// every mesh builder.
#[derive(Clone, Copy)]
pub(super) struct ModelContent<'a> {
    pub models: &'a ModelRegistry,
    pub item_models: &'a [Option<ItemModel>],
}

impl ModelContent<'_> {
    /// The model of whatever is in the selected hotbar slot.
    fn held(&self, inventory: &Inventory) -> Option<ItemModel> {
        self.of(inventory.selected_stack()?.item)
    }

    /// The model an item is drawn as, if it declares one.
    fn of(&self, item: ItemId) -> Option<ItemModel> {
        *self.item_models.get(item.0 as usize)?
    }
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
    /// Box-model mobs sample the block atlas (`None`); file-loaded models carry
    /// the id of the texture they need bound.
    mob_meshes: Vec<(GpuMesh, Option<ModelId>)>,
    /// The item model in the local player's hand, drawn in third person.
    held_mesh: Option<(GpuMesh, ModelId)>,
    /// The same model in the inventory preview, posed for the preview camera.
    preview_held_mesh: Option<(GpuMesh, ModelId)>,
    /// Model geometry for dropped stacks, one mesh per distinct model.
    drops_model_meshes: Vec<(GpuMesh, ModelId)>,
    /// GPU textures for loaded models, indexed by [`ModelId`] and uploaded on
    /// first use — states are constructed before the `Renderer` exists, so the
    /// atlas's startup upload is not an option here.
    model_textures: Vec<Option<Texture>>,

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
            held_mesh: None,
            preview_held_mesh: None,
            drops_model_meshes: Vec::new(),
            model_textures: Vec::new(),
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
        content: ModelContent<'_>,
    ) {
        if player.perspective.is_first_person() {
            self.player_mesh = None;
            self.held_mesh = None;
            return;
        }
        let pose = self.player_anim.pose(player.pitch);
        let armor = inventory.equipped_armor();
        let mesh =
            self.player_model
                .build_mesh_armored(player.position, player.yaw, &pose, &armor, items);
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();

        let anchor = self
            .player_model
            .hand_anchor(player.position, player.yaw, &pose);
        self.held_mesh = self.bake_held(ctx, content, inventory, anchor);
    }

    /// Bake the model of the item in `anchor`'s hand, if it has one.
    fn bake_held(
        &mut self,
        ctx: &Arc<RenderContext>,
        content: ModelContent<'_>,
        inventory: &Inventory,
        anchor: HandAnchor,
    ) -> Option<(GpuMesh, ModelId)> {
        let held = content.held(inventory)?;
        let transform = model_mesh::placement(
            anchor.position,
            anchor.yaw,
            anchor.pitch,
            held.scale,
            held.offset,
        );
        self.bake_model(ctx, content.models, held.id, transform)
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
        content: ModelContent<'_>,
    ) {
        if !inventory_open {
            self.preview_mesh = None;
            self.preview_held_mesh = None;
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

        let anchor = self
            .player_model
            .hand_anchor(Vec3::ZERO, self.preview.yaw, &pose);
        self.preview_held_mesh = self.bake_held(ctx, content, inventory, anchor);
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
        models: &ModelRegistry,
        dt: f32,
    ) {
        self.mob_meshes.clear();
        let mut visuals = Vec::new();
        for mob in mobs {
            if let Some(visual) = mob_mesh(
                &mob.visual,
                mob.position,
                mob.yaw,
                &mob.anim.pose(0.0),
                models,
            ) {
                visuals.push(visual);
            }
        }
        for rm in remote_mobs.values_mut() {
            let (visual, position, yaw, pose) = rm.animate(dt);
            if let Some(visual) = mob_mesh(visual, position, yaw, &pose, models) {
                visuals.push(visual);
            }
        }
        for visual in visuals {
            if let Some(id) = visual.model {
                self.ensure_model_texture(ctx, models, id);
            }
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &visual.mesh) {
                self.mob_meshes.push((gpu, visual.model));
            }
        }
    }

    /// Upload a model's texture the first time something asks to draw it.
    ///
    /// Model textures cannot be built with the block atlas at startup: game
    /// states are constructed before the `Renderer` (and its device) exists, so
    /// the first frame that needs one is the earliest point this can happen.
    fn ensure_model_texture(
        &mut self,
        ctx: &Arc<RenderContext>,
        models: &ModelRegistry,
        id: ModelId,
    ) {
        let index = id.0 as usize;
        if self.model_textures.len() <= index {
            self.model_textures.resize_with(index + 1, || None);
        }
        if self.model_textures[index].is_some() {
            return;
        }
        let Some(model) = models.get(id) else {
            return;
        };
        match Texture::create(ctx, &model.texture) {
            Ok(texture) => self.model_textures[index] = Some(texture),
            // Without a texture the mesh would sample whatever was bound last,
            // so it is simply not drawn (see `textured_mesh`).
            Err(err) => log::warn!("could not upload model texture: {err}"),
        }
    }

    /// Pair a mesh with its model texture, or `None` if the texture is missing.
    fn textured_mesh<'a>(&'a self, mesh: &'a GpuMesh, id: ModelId) -> Option<TexturedMesh<'a>> {
        let texture = self.model_textures.get(id.0 as usize)?.as_ref()?;
        Some(TexturedMesh { mesh, texture })
    }

    /// Bake a model under `transform` and upload it, keeping its id alongside so
    /// the draw can bind the right texture.
    fn bake_model(
        &mut self,
        ctx: &Arc<RenderContext>,
        models: &ModelRegistry,
        id: ModelId,
        transform: glam::Mat4,
    ) -> Option<(GpuMesh, ModelId)> {
        self.ensure_model_texture(ctx, models, id);
        let mesh = models.get(id)?.mesh.bake(transform);
        let gpu = GpuMesh::upload(&ctx.memory_allocator, &mesh)
            .ok()
            .flatten()?;
        Some((gpu, id))
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
        content: ModelContent<'_>,
    ) {
        let mut opaque = CpuMesh::new();
        let mut transparent = CpuMesh::new();
        // Drops whose item declares a model are drawn as that model instead of
        // the default spinning cube. Those cannot join the merged cube meshes —
        // each needs its own texture bound — so they are grouped by model, one
        // mesh per model however many drops share it.
        let mut by_model: HashMap<ModelId, CpuMesh> = HashMap::new();
        for item in drops {
            if let Some(model) = content.of(item.stack.item)
                && let Some(loaded) = content.models.get(model.id)
            {
                let transform = model_mesh::placement(
                    item.render_center(),
                    item.spin_yaw(),
                    0.0,
                    model.scale,
                    model.offset,
                );
                let mesh = loaded.mesh.bake(transform);
                let entry = by_model.entry(model.id).or_default();
                entry.push_indexed(mesh.vertices, mesh.indices);
                continue;
            }
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

        self.drops_model_meshes.clear();
        for (id, mesh) in by_model {
            self.ensure_model_texture(ctx, content.models, id);
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.drops_model_meshes.push((gpu, id));
            }
        }
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
        opaque.extend(
            self.mob_meshes
                .iter()
                .filter_map(|(mesh, model)| model.is_none().then_some(mesh)),
        );
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

        // Everything drawn from a model file, each with its own texture.
        let textured: Vec<TexturedMesh<'_>> = self
            .mob_meshes
            .iter()
            .filter_map(|(mesh, model)| self.textured_mesh(mesh, (*model)?))
            .chain(
                self.drops_model_meshes
                    .iter()
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            .chain(
                self.held_mesh
                    .iter()
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            .collect();

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
            textured,
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
            held: self
                .preview_held_mesh
                .as_ref()
                .and_then(|(mesh, id)| self.textured_mesh(mesh, *id)),
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
        let models = self.models.clone();
        let item_models = self.item_models.clone();
        let content = ModelContent {
            models: &models,
            item_models: &item_models,
        };
        self.view.update_drops_mesh(
            ctx,
            &self.drops,
            |item| {
                let is_transparent = items
                    .get(item)
                    .place_block
                    .is_some_and(|b| blocks.get(b).is_transparent());
                (
                    super::interaction::drop_textures(item, items, blocks),
                    is_transparent,
                )
            },
            content,
        );
        self.view
            .update_mob_meshes(ctx, &self.mobs, &mut self.remote_mobs, &models, dt);
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
            .update_player_mesh(ctx, &self.player, &self.inventory, &self.items, content);
        self.view.update_preview_mesh(
            ctx,
            self.inventory_open,
            &self.player,
            &self.inventory,
            &self.items,
            content,
        );
        self.view
            .update_remote_meshes(ctx, &self.peers.players, &self.items, dt);
    }
}

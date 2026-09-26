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

use glam::{Mat4, Vec3};

use super::mobs::mob_mesh;
use super::{INSPECT_MODEL_FROM, OUTLINE_COLOR, THIRD_PERSON_DISTANCE};
use crate::application::ecs::components::{
    Animation, Body, ItemDrop, Kind, MobId, Projectile, RemotePlayer, Transform, Velocity,
};
use crate::domain::core::{Aabb, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle};
use crate::domain::entity::camera::Shot;
use crate::domain::entity::kind::{EntityRegistry, VisualSpec};
use crate::domain::entity::{AnimationState, Player, camera};
use crate::domain::inventory::{Inventory, ItemId, Placeable};
use crate::domain::world::World;
use crate::domain::world::meshing::{
    ItemSprite, mesh_block_overlay, mesh_chunk, push_item_cube, push_item_sprite,
};
use crate::presentation::art::{cracks, mobskin, skin};
use crate::presentation::content::BlockAppearance;
use crate::presentation::content::{ItemModel, ItemShape};
use crate::presentation::editor::{PlacementKey, PlacementSource};
use crate::presentation::render::camera::ShotCamera;
use crate::presentation::render::viewmodel::{self, HandPose};
use crate::presentation::render::{Character, HeadLook, HumanoidRig};
use wyven_model::mesh as model_mesh;
use wyven_model::{DisplayContext, ModelId, ModelRegistry};
use wyven_render::{
    Camera, CpuMesh, ForegroundFrame, GpuLines, GpuMesh, LightParams, RenderContext, SceneFrame,
    SkyParams, Texture, TexturedMesh, TileRegistry, debug,
};
use wyven_voxel::FaceTextures;

/// Animation state for a remote player plus the position used to derive their
/// speed (no extra protocol data needed — movement is inferred from the change
/// in rendered position each frame).
/// Another player, as the view needs them: where their body is drawn (the
/// interpolated position their nameplate follows), where they look, how they
/// are posed, and what is in their hand.
pub(super) struct PeerSprite {
    pub position: Vec3,
    pub pitch: f32,
    pub anim: AnimationState,
    pub held: Option<ItemId>,
}

/// A mob, simulated or replicated, as the view needs it.
pub(super) struct MobSprite<'a> {
    pub visual: &'a VisualSpec,
    /// Feet.
    pub position: Vec3,
    /// The torso's yaw, which lags the look yaw.
    pub yaw: f32,
    pub pose: crate::domain::entity::Pose,
}

/// A dropped item, as the view needs it: what it is and where to draw it.
#[derive(Debug, Clone, Copy)]
pub(super) struct DropSprite {
    pub item: ItemId,
    /// Centre of the drawn box, bob and ground lift included.
    pub center: Vec3,
    /// Edge length of the drawn box.
    pub size: f32,
    /// Spin about Y.
    pub yaw: f32,
}

/// An arrow in flight, as the view needs it.
#[derive(Debug, Clone, Copy)]
pub(super) struct ArrowSprite {
    pub position: Vec3,
    pub yaw: f32,
    pub size: f32,
}

/// Everything the view needs to draw an item: the parsed models and which one
/// each item uses, plus — for the items that have no model at all — the atlas
/// and the shape they fall back to. These always travel together, so they are
/// passed as one borrow rather than threaded separately through every mesh
/// builder.
#[derive(Clone, Copy)]
pub(super) struct ModelContent<'a> {
    pub models: &'a ModelRegistry,
    pub item_models: &'a [Option<ItemModel>],
    /// The shared atlas, for the cube and sprite fallbacks.
    pub tiles: &'a TileRegistry,
    /// What an item with no model is drawn as, and whether it belongs in the
    /// blended pass. A closure because the answer needs the block registry,
    /// which the view deliberately cannot reach.
    pub shape: &'a dyn Fn(ItemId) -> (ItemShape, bool),
    /// Where a held item sits, when something wants to say. Normally nothing
    /// does and every placement is the shipped one; while the in-game editor is
    /// moving a value, this is how it reaches the screen. Deliberately a port:
    /// none of the five seams below learns that an editor exists.
    pub placement: &'a dyn PlacementSource,
    /// Where a held block sits — one value shared by every block item, loaded
    /// from `assets/models/items/block.json`.
    pub block_display: &'a wyven_model::DisplayTransforms,
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

    /// Where a model-backed item sits in this context.
    fn local(&self, item: ItemId, model: ItemModel, context: DisplayContext) -> Mat4 {
        self.placement
            .local(PlacementKey::Item(item), context)
            .unwrap_or_else(|| model.local(self.models, context))
    }

    /// The same for an item with no model file, which is placed as the cube or
    /// sprite it falls back to.
    ///
    /// A cube is keyed by [`PlacementKey::BlockItem`], not by the item: every
    /// block item is the same cube and shares one placement, so an edit to it
    /// has to reach all of them at once.
    fn atlas_local(&self, item: ItemId, shape: ItemShape, context: DisplayContext) -> Mat4 {
        let key = match shape {
            ItemShape::Cube(_) => PlacementKey::BlockItem,
            ItemShape::Sprite(_) => PlacementKey::Item(item),
        };
        self.placement
            .local(key, context)
            .unwrap_or_else(|| held_placement(shape, context, self.block_display).matrix())
    }
}

/// The player's rigged model as the entity data describes it, with its bones
/// and clips resolved once.
///
/// Held by id rather than by reference because the registry lives behind an
/// `Arc` on the state, and the view is handed a fresh borrow of it each frame.
struct PlayerRig {
    model: ModelId,
    scale: f32,
    /// Atlas tile origin of the 64×64 sheet the model samples.
    sheet: [u32; 2],
    clips: HumanoidRig,
}

/// All GPU state for the in-game scene.
pub(super) struct SceneCache {
    /// Opaque GPU meshes for loaded chunks, keyed by chunk position.
    meshes: HashMap<ChunkPos, GpuMesh>,
    /// Transparent (water/glass) GPU meshes, drawn in a second blended pass.
    transparent_meshes: HashMap<ChunkPos, GpuMesh>,
    /// The same two passes for Blockbench-authored blocks, which sample the
    /// block texture array instead of the atlas. One mesh per chunk either way:
    /// the array layer rides on the vertex, so every block type in a chunk
    /// batches into one draw. These become the only chunk meshes once the last
    /// block has been re-authored as a model.
    array_meshes: HashMap<ChunkPos, GpuMesh>,
    array_transparent_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Model-backed blocks (plants, mushrooms) in each chunk, one mesh per
    /// distinct model because each binds its own texture.
    model_meshes: HashMap<ChunkPos, Vec<(GpuMesh, ModelId)>>,
    /// Pending mesh rebuilds (budgeted across frames), with a dedup set.
    mesh_queue: VecDeque<ChunkPos>,
    queued: HashSet<ChunkPos>,

    /// The player's rigged model, bound the first time content is in reach —
    /// the model registry does not exist when this cache is built.
    player_rig: Option<PlayerRig>,
    /// The local player's GPU mesh (only built in third person).
    player_mesh: Option<GpuMesh>,
    remote_meshes: Vec<GpuMesh>,
    /// What each remote player is holding, in the same two flavours the local
    /// body's hand takes: a model file, or the cube/sprite fallback.
    remote_held: Vec<(GpuMesh, ModelId)>,
    remote_held_atlas: Vec<(GpuMesh, bool)>,
    /// Per-remote-player animation, keyed by id.
    /// One GPU mesh per visible mob, rebuilt each frame like remote players.
    /// Box-model mobs sample the block atlas (`None`); file-loaded models carry
    /// the id of the texture they need bound.
    mob_meshes: Vec<(GpuMesh, Option<ModelId>)>,
    /// The item model in the local player's hand, drawn in third person.
    held_mesh: Option<(GpuMesh, ModelId)>,
    /// The view model: the player's own arm, and the item in it. Built only in
    /// first person, where `player_mesh` and `held_mesh` are not.
    hand_mesh: Option<GpuMesh>,
    hand_held_mesh: Option<(GpuMesh, ModelId)>,

    /// The held item when it has **no model file** — a block cube or a flat
    /// sprite, sampling the shared atlas rather than a texture of its own.
    ///
    /// Two fields rather than one because the two views reach the renderer by
    /// two different routes: the foreground's atlas list and the world's
    /// opaque/transparent lists (hence the `bool`). Each is `Some` only when its
    /// `*_mesh` counterpart above is `None` — a model always wins, and the two
    /// can never draw at once.
    hand_held_atlas: Option<GpuMesh>,
    held_atlas: Option<(GpuMesh, bool)>,
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
    /// Extruded silhouettes for flat item icons, keyed by atlas tile. Tracing
    /// one walks the whole tile's alpha, so it is done once and kept rather than
    /// repeated for every drop on every frame.
    item_sprites: HashMap<u32, ItemSprite>,

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
            array_meshes: HashMap::new(),
            array_transparent_meshes: HashMap::new(),
            model_meshes: HashMap::new(),
            mesh_queue: VecDeque::new(),
            queued: HashSet::new(),
            player_rig: None,
            player_mesh: None,
            remote_meshes: Vec::new(),
            remote_held: Vec::new(),
            remote_held_atlas: Vec::new(),
            mob_meshes: Vec::new(),
            held_mesh: None,
            hand_mesh: None,
            hand_held_mesh: None,
            hand_held_atlas: None,
            held_atlas: None,
            drops_model_meshes: Vec::new(),
            model_textures: Vec::new(),
            break_mesh: None,
            outline_block: None,
            outline_mesh: None,
            arrows_mesh: None,
            drops_mesh: None,
            drops_mesh_transparent: None,
            item_sprites: HashMap::new(),
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
        self.array_meshes.remove(&pos);
        self.array_transparent_meshes.remove(&pos);
        self.model_meshes.remove(&pos);
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
        blocks: BlockAppearance<'_>,
        budget: usize,
    ) {
        for _ in 0..budget {
            let Some(pos) = self.mesh_queue.pop_front() else {
                break;
            };
            self.queued.remove(&pos);

            let generator = world.generator();
            let output = world.chunk(pos).map(|chunk| {
                mesh_chunk(
                    chunk,
                    &blocks,
                    |p| world.block_at(p),
                    |x, z, index| generator.biome_tint(x, z, index),
                )
            });
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
                    // Blockbench-authored blocks: one mesh per chunk however
                    // many block types and textures it holds, because the layer
                    // index rides on the vertex.
                    match GpuMesh::upload(&ctx.memory_allocator, &output.array_opaque) {
                        Ok(Some(mesh)) => {
                            self.array_meshes.insert(pos, mesh);
                        }
                        Ok(None) => {
                            self.array_meshes.remove(&pos);
                        }
                        Err(err) => log::error!("block mesh upload failed at {pos:?}: {err:?}"),
                    }
                    match GpuMesh::upload(&ctx.memory_allocator, &output.array_transparent) {
                        Ok(Some(mesh)) => {
                            self.array_transparent_meshes.insert(pos, mesh);
                        }
                        Ok(None) => {
                            self.array_transparent_meshes.remove(&pos);
                        }
                        Err(err) => {
                            log::error!("blended block mesh upload failed at {pos:?}: {err:?}")
                        }
                    }
                    // Model-backed blocks: one mesh per model in this chunk,
                    // each needing its texture resident before it can be drawn.
                    let mut baked = Vec::new();
                    for (id, mesh) in &output.models {
                        self.ensure_model_texture(ctx, blocks.models, *id);
                        match GpuMesh::upload(&ctx.memory_allocator, mesh) {
                            Ok(Some(gpu)) => baked.push((gpu, *id)),
                            Ok(None) => {}
                            Err(err) => {
                                log::error!("model mesh upload failed at {pos:?}: {err:?}")
                            }
                        }
                    }
                    if baked.is_empty() {
                        self.model_meshes.remove(&pos);
                    } else {
                        self.model_meshes.insert(pos, baked);
                    }
                }
                // Chunk was unloaded before we got to it.
                None => self.forget_chunk(pos),
            }
        }
    }

    // --- Animated models ------------------------------------------------------------

    /// Resolve the player's rigged model, its clips and the bones that matter.
    ///
    /// Idempotent and cheap after the first call, which is why it can sit on the
    /// per-frame path: content is only reachable from there, and a model that
    /// fails to load simply leaves the player undrawn rather than the frame
    /// panicking — the same fail-soft the rest of the model pipeline takes.
    pub fn bind_player_rig(&mut self, entities: &EntityRegistry, models: &ModelRegistry) {
        if self.player_rig.is_some() {
            return;
        }
        let Some(kind) = entities.find("player") else {
            return;
        };
        let VisualSpec::Rigged(visual) = &kind.visual else {
            return;
        };
        let Some(movement) = kind.movement.as_ref() else {
            return;
        };
        let Some(id) = models.find(&visual.path) else {
            return;
        };
        let Some(rig) = models.get(id).and_then(|model| model.rig.as_ref()) else {
            log::warn!(
                "{} carries no rig; the player will not be drawn",
                visual.path
            );
            return;
        };
        self.player_rig = Some(PlayerRig {
            model: id,
            scale: visual.scale,
            sheet: visual
                .skin
                .as_deref()
                .and_then(mobskin::origin_for)
                .unwrap_or(skin::SKIN_ORIGIN),
            clips: HumanoidRig::bind(rig, movement),
        });
    }

    /// Drop the third-person body and whatever it was holding. One helper
    /// because the two must always go together: a held item left behind after
    /// the body it hung off is gone would float in the world on its own.
    fn clear_player_meshes(&mut self) {
        self.player_mesh = None;
        self.held_mesh = None;
        self.held_atlas = None;
    }

    /// The player as something drawable, or `None` before the rig is bound.
    fn character<'a>(&'a self, models: &'a ModelRegistry) -> Option<Character<'a>> {
        let rig = self.player_rig.as_ref()?;
        Some(Character {
            model: models.get(rig.model)?,
            clips: &rig.clips,
            scale: rig.scale,
            sheet: rig.sheet,
        })
    }

    /// Rebuild the player model mesh in third person, or the view model in
    /// first — never both, since in first person the body is the camera.
    ///
    /// `inspect` is how far through the inventory's camera pan we are. Past
    /// [`INSPECT_MODEL_FROM`] it forces the world model on even in first
    /// person, which would otherwise swing the camera out to frame an empty
    /// stage, and suppresses the view-model arm at the same instant, since the
    /// two are alternatives. It also levels the model's head: the pose inherits
    /// the player's pitch, so a player who opened the inventory while looking
    /// up would be shown a model staring at the ceiling.
    pub fn update_player_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        anim: &AnimationState,
        inventory: &Inventory,
        inspect: f32,
        content: ModelContent<'_>,
    ) {
        let show_body = inspect >= INSPECT_MODEL_FROM;
        if player.perspective.is_first_person() && !show_body {
            self.player_mesh = None;
            self.held_mesh = None;
            self.held_atlas = None;
            self.update_hand_meshes(ctx, player, anim, inventory, content);
            return;
        }
        self.hand_mesh = None;
        self.hand_held_mesh = None;
        self.hand_held_atlas = None;

        // The body is drawn at the torso yaw, which lags the look yaw the camera
        // uses; the head bone's own turn is what puts the face back where the
        // player looks. The held item hangs off the hand *bone* under the same
        // pose, so it cannot drift out of the fist however the elbow bends.
        let body_yaw = anim.body_yaw();
        // Drawn at the interpolated position, not the raw one: physics steps at
        // a fixed rate while this runs every frame, and the camera is built from
        // the *same* interpolation. Baking the body at `player.position` instead
        // makes it lurch one tick's worth against a camera that glides — which
        // reads as the whole player juddering, most of all in a jump, where a
        // tick is 0.15 blocks straight up.
        let render_position = player.interpolated_position(self.render_alpha);
        let baked = {
            let Some(character) = self.character(content.models) else {
                self.clear_player_meshes();
                return;
            };
            let look = HeadLook {
                yaw: anim.head_offset(),
                pitch: player.pitch,
            };
            character.pose(anim, look).map(|pose| {
                (
                    character.bake(&pose, render_position, body_yaw),
                    character.hand_anchor(&pose, render_position, body_yaw),
                )
            })
        };
        let Some((mesh, anchor)) = baked else {
            self.clear_player_meshes();
            return;
        };
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();

        let held = inventory.selected_stack().map(|stack| stack.item);
        self.held_mesh = anchor.and_then(|a| self.bake_held(ctx, content, held, a));
        self.held_atlas = anchor.and_then(|a| self.bake_held_atlas(ctx, content, held, a));
    }

    /// Build the held item for something with **no model file**: the same cube
    /// or sprite a dropped stack of it would be, placed by `transform`.
    ///
    /// This is what keeps a block from being invisible in the hand. The geometry
    /// is built in `0..1` model space — the space a Blockbench export occupies —
    /// so the caller's placement matrix positions it by exactly the path an
    /// authored model takes.
    ///
    /// `normal_basis` is passed separately because the two hands want different
    /// answers: in third person the item really is out in the world and should
    /// light like the body carrying it, while in first person the transform
    /// carries the camera's own rotation and lighting taken from it would pulse
    /// as the player turns. See [`CpuMesh::transformed`].
    fn shaped_item_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        shape: ItemShape,
        tiles: &TileRegistry,
        transform: glam::Mat4,
        normal_basis: glam::Mat4,
    ) -> Option<GpuMesh> {
        let mut mesh = CpuMesh::new();
        match shape {
            ItemShape::Cube(faces) => {
                push_item_cube(&mut mesh, Vec3::splat(0.5), 1.0, 0.0, &faces);
            }
            ItemShape::Sprite(tile) => {
                let sprite = self
                    .item_sprites
                    .entry(tile)
                    .or_insert_with(|| ItemSprite::new(tile, tiles.art(tile)));
                push_item_sprite(&mut mesh, sprite, Vec3::splat(0.5), 1.0, 0.0);
            }
        }
        let placed = mesh.transformed(transform, normal_basis);
        GpuMesh::upload(&ctx.memory_allocator, &placed)
            .ok()
            .flatten()
    }

    /// The third-person counterpart of [`SceneCache::bake_held`], for an
    /// item with no model. Returns the mesh and whether it belongs in the
    /// blended pass, so a held glass block reads like the block it places.
    fn bake_held_atlas(
        &mut self,
        ctx: &Arc<RenderContext>,
        content: ModelContent<'_>,
        item: Option<ItemId>,
        anchor: Mat4,
    ) -> Option<(GpuMesh, bool)> {
        let item = item?;
        // A model always wins; this is only the fallback for items without one.
        if content.of(item).is_some() {
            return None;
        }
        let (shape, is_transparent) = (content.shape)(item);
        let local = content.atlas_local(item, shape, DisplayContext::ThirdPersonRightHand);
        let transform = anchor * local;
        let mesh = self.shaped_item_mesh(ctx, shape, content.tiles, transform, transform)?;
        Some((mesh, is_transparent))
    }

    /// Bake the model of the item in `anchor`'s hand, if it has one.
    fn bake_held(
        &mut self,
        ctx: &Arc<RenderContext>,
        content: ModelContent<'_>,
        item: Option<ItemId>,
        anchor: Mat4,
    ) -> Option<(GpuMesh, ModelId)> {
        let item = item?;
        let held = content.of(item)?;
        let local = content.local(item, held, DisplayContext::ThirdPersonRightHand);
        self.bake_model(ctx, content.models, held.id, anchor * local)
    }

    /// Rebuild the first-person view model: the player's own arm, and whatever
    /// it holds.
    ///
    /// The arm always draws; the item only when it has a model of its own, the
    /// same rule third person follows. Both hang off one [`HandPose::frame`], so
    /// the item cannot drift out of the fist.
    fn update_hand_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        anim: &AnimationState,
        inventory: &Inventory,
        content: ModelContent<'_>,
    ) {
        let pose = HandPose {
            eye: player.interpolated_eye_position(self.render_alpha),
            yaw: player.yaw,
            pitch: player.pitch,
            swing: anim.swing_progress(),
            walk_phase: anim.walk_phase(),
            walk_amount: anim.walk_amount(),
        };
        let frame = pose.frame();

        // The arm is posed entirely by `frame` — bob and swing — and takes
        // nothing from the body's clips or its head look. See `arm_mesh`.
        let arm = self
            .character(content.models)
            .map(|character| viewmodel::arm_mesh(&character, frame));
        self.hand_mesh =
            arm.and_then(|arm| GpuMesh::upload(&ctx.memory_allocator, &arm).ok().flatten());

        let selected = inventory.selected_stack().map(|stack| stack.item);
        let held = content.held(inventory);
        self.hand_held_mesh = held.zip(selected).and_then(|(held, item)| {
            let local = content.local(item, held, DisplayContext::FirstPersonRightHand);
            let transform = viewmodel::item_anchor(frame) * local;
            self.bake_model(ctx, content.models, held.id, transform)
        });

        // No model file: draw the cube or sprite the ground would draw, rather
        // than an empty fist. Keyed off `held` rather than off the mesh above,
        // so a model that merely failed to upload does not fall back to the
        // magenta placeholder `item_shape` returns for model-backed items.
        self.hand_held_atlas = match (held, inventory.selected_stack()) {
            (None, Some(stack)) => {
                let (shape, _) = (content.shape)(stack.item);
                let local =
                    content.atlas_local(stack.item, shape, DisplayContext::FirstPersonRightHand);
                let transform = viewmodel::item_anchor(frame) * local;
                // Lit by the placement alone: `transform` carries the camera's
                // rotation, and using it would make the block pulse as you spin.
                self.shaped_item_mesh(ctx, shape, content.tiles, transform, local)
            }
            _ => None,
        };
    }

    /// Rebuild GPU meshes for remote players, advancing each one's animation
    /// from the movement observed since the previous frame.
    pub fn update_remote_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        peers: impl IntoIterator<Item = PeerSprite>,
        content: ModelContent<'_>,
    ) {
        self.remote_meshes.clear();
        self.remote_held.clear();
        self.remote_held_atlas.clear();
        let snapshots: Vec<PeerSprite> = peers.into_iter().collect();
        let mut baked: Vec<CpuMesh> = Vec::with_capacity(snapshots.len());
        // The fist and what is in it, gathered here and baked below: `character`
        // borrows `self` for as long as the pose does, and baking an item needs
        // `self` mutably for the sprite cache.
        let mut hands: Vec<(Mat4, Option<ItemId>)> = Vec::with_capacity(snapshots.len());
        for PeerSprite {
            position: pos,
            pitch,
            anim,
            held,
        } in snapshots
        {
            let Some(character) = self.character(content.models) else {
                break;
            };
            let look = HeadLook {
                yaw: anim.head_offset(),
                pitch,
            };
            // Derived here rather than sent: only the look yaw crosses the wire,
            // and a torso that follows it is cosmetic, so every peer can work it
            // out for itself.
            if let Some(pose) = character.pose(&anim, look) {
                let body_yaw = anim.body_yaw();
                baked.push(character.bake(&pose, pos, body_yaw));
                // The same pose and yaw the body was baked at, so a peer's item
                // rides its fist exactly the way the local player's does.
                if let Some(anchor) = character.hand_anchor(&pose, pos, body_yaw) {
                    hands.push((anchor, held));
                }
            }
        }
        for mesh in baked {
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.remote_meshes.push(gpu);
            }
        }
        for (anchor, held) in hands {
            if let Some(mesh) = self.bake_held(ctx, content, held, anchor) {
                self.remote_held.push(mesh);
            }
            if let Some(mesh) = self.bake_held_atlas(ctx, content, held, anchor) {
                self.remote_held_atlas.push(mesh);
            }
        }
    }

    /// Rebuild one mesh per visible mob — the authority's own simulated mobs
    /// plus, on a client, the host's replicas (whose animation is driven from
    /// their rendered movement, like remote players).
    pub fn update_mob_meshes<'a>(
        &mut self,
        ctx: &Arc<RenderContext>,
        mobs: impl IntoIterator<Item = MobSprite<'a>>,
        models: &ModelRegistry,
    ) {
        self.mob_meshes.clear();
        let visuals: Vec<_> = mobs
            .into_iter()
            .filter_map(|mob| mob_mesh(mob.visual, mob.position, mob.yaw, &mob.pose, models))
            .collect();
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
    ///
    /// `shaft` is the arrow item's own faces, resolved by the caller — an arrow
    /// in flight is not an inventory stack, so it cannot look itself up.
    pub fn update_arrows_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        arrows: impl IntoIterator<Item = ArrowSprite>,
        shaft: FaceTextures,
    ) {
        let mut mesh = CpuMesh::new();
        for arrow in arrows {
            push_item_cube(&mut mesh, arrow.position, arrow.size, arrow.yaw, &shaft);
        }
        self.arrows_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    // --- World overlays -------------------------------------------------------------

    /// Rebuild the combined drop meshes (opaque + transparent passes). Drops are
    /// few and tiny, so a per-frame rebuild stays cheap, like remote players.
    ///
    /// Takes an iterator of plain [`DropSprite`]s rather than entities, so the
    /// view never learns where drops are stored — and so the item placement
    /// editor can chain a still preview drop onto the real ones, down this exact
    /// path rather than an approximation of it.
    pub fn update_drops_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        drops: impl IntoIterator<Item = DropSprite>,
        content: ModelContent<'_>,
    ) {
        let (shape, tiles) = (content.shape, content.tiles);
        let mut opaque = CpuMesh::new();
        let mut transparent = CpuMesh::new();
        // Drops whose item declares a model are drawn as that model instead of
        // the default spinning cube. Those cannot join the merged cube meshes —
        // each needs its own texture bound — so they are grouped by model, one
        // mesh per model however many drops share it.
        let mut by_model: HashMap<ModelId, CpuMesh> = HashMap::new();
        for item in drops {
            if let Some(model) = content.of(item.item)
                && let Some(loaded) = content.models.get(model.id)
            {
                let transform = model_mesh::anchor(item.center, item.yaw, 0.0)
                    * content.local(item.item, model, DisplayContext::Ground);
                let mesh = loaded.mesh.bake(transform);
                let entry = by_model.entry(model.id).or_default();
                entry.push_indexed(mesh.vertices, mesh.indices);
                continue;
            }
            let (shape, is_transparent) = shape(item.item);
            let target = if is_transparent {
                &mut transparent
            } else {
                &mut opaque
            };
            match shape {
                ItemShape::Cube(faces) => {
                    push_item_cube(target, item.center, item.size, item.yaw, &faces)
                }
                ItemShape::Sprite(tile) => {
                    let sprite = self
                        .item_sprites
                        .entry(tile)
                        .or_insert_with(|| ItemSprite::new(tile, tiles.art(tile)));
                    push_item_sprite(target, sprite, item.center, item.size, item.yaw);
                }
            }
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
        breaking: Option<(BlockPos, Aabb, f32)>,
    ) {
        self.break_mesh = breaking.and_then(|(block, box_, progress)| {
            // No crack art on disk means no overlay at all: it is drawn *over*
            // the block being mined, so a missing-texture marker would hide the
            // thing you are looking at rather than read as art that is absent.
            let overlay = mesh_block_overlay(box_, cracks::tile(progress)?);
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
    pub fn update_target_outline(
        &mut self,
        ctx: &Arc<RenderContext>,
        target: Option<(BlockPos, Aabb)>,
    ) {
        let block = target.map(|(block, _)| block);
        if block == self.outline_block {
            return;
        }
        self.outline_block = block;
        self.outline_mesh = target.and_then(|(block, box_)| {
            let mut vertices = Vec::new();
            debug::push_block_outline(&mut vertices, box_, OUTLINE_COLOR);
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

    /// Collect this frame's visible geometry: frustum-culled chunk meshes plus
    /// every entity and overlay mesh, split by render pass.
    ///
    /// Takes the finished camera rather than building one: it is derived once,
    /// by [`InGameState::world_camera`], and shared with the nameplate pass so
    /// the two cannot disagree about where the viewer is.
    pub fn scene_frame(
        &self,
        player: &Player,
        day_cycle: &DayCycle,
        camera: Camera,
    ) -> SceneFrame<'_> {
        let aspect = camera.aspect;
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

        // Blockbench-authored blocks, culled by the same chunk column AABB.
        // They sample the block texture array, so they are a separate list even
        // though they are the same geometry kind as `opaque`.
        let array_opaque: Vec<&GpuMesh> = self
            .array_meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();
        let array_transparent: Vec<&GpuMesh> = self
            .array_transparent_meshes
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

        // A held item with no model of its own, split by pass for the same
        // reason drops are: a glass block in the fist must blend like glass.
        // Every fist in the world, not just this one's — a peer holding glass
        // has to blend too.
        for (mesh, is_transparent) in self.held_atlas.iter().chain(&self.remote_held_atlas) {
            if *is_transparent {
                transparent.push(mesh);
            } else {
                opaque.push(mesh);
            }
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
                    .chain(&self.remote_held)
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            // Model-backed blocks, culled by the same chunk column AABB as the
            // atlas meshes above.
            .chain(
                self.model_meshes
                    .iter()
                    .filter(|(pos, _)| in_view(pos))
                    .flat_map(|(_, chunk)| chunk.iter())
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
            array_opaque,
            array_transparent,
            textured,
            lines: self.outline_mesh.as_ref(),
            foreground: self.foreground_frame(player, aspect),
        }
    }

    /// The view model, framed by its own camera.
    ///
    /// Its own, because the field of view a player picks for the world should
    /// not distort their own hand — and because the renderer clears depth before
    /// drawing it, so it needs no relationship to the world's near plane.
    fn foreground_frame(&self, player: &Player, aspect: f32) -> Option<ForegroundFrame<'_>> {
        let arm = self.hand_mesh.as_ref()?;
        let mut camera = Camera::new(viewmodel::HAND_FOV_DEGREES, aspect);
        // The hand is baked in world space against the same eye the world pass
        // uses, so the foreground camera has to sit exactly there too — only its
        // field of view differs.
        camera.position = player.interpolated_eye_position(self.render_alpha);
        camera.forward = player.look_direction();
        Some(ForegroundFrame {
            view_proj: camera.view_projection(),
            atlas: std::iter::once(arm)
                .chain(self.hand_held_atlas.as_ref())
                .collect(),
            textured: self
                .hand_held_mesh
                .iter()
                .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id))
                .collect(),
        })
    }
}

/// Which display table places a held item that has no model file of its own.
///
/// A cube takes Minecraft's `block/block` numbers; a flat sprite is precisely
/// the geometry `item/generated` describes, so it takes that model's standard
/// placement — the same one every extruded 2D item already uses, which is why
/// an apple in the fist and a lump of coal in the fist agree.
fn held_placement(
    shape: ItemShape,
    context: DisplayContext,
    block_display: &wyven_model::DisplayTransforms,
) -> wyven_model::display::ItemTransform {
    match shape {
        ItemShape::Cube(_) => block_display.get(context).unwrap_or_default(),
        ItemShape::Sprite(_) => wyven_model::generated::default_display()
            .get(context)
            .unwrap_or_default(),
    }
}

impl super::InGameState {
    /// How far the third-person camera sits from the eye this frame: the desired
    /// [`THIRD_PERSON_DISTANCE`], pulled in so nothing solid ends up between the
    /// camera and the player.
    ///
    /// Lives here rather than on [`SceneCache`] because it needs the world, which
    /// the view deliberately cannot reach. Recomputed per call rather than
    /// cached: it is a pure function of the world and the player, so the
    /// nameplate camera and the world camera work out the same answer within a
    /// frame without having to share state to do it.
    ///
    /// The predicate is `is_solid_for_collision`, the one player physics uses:
    /// it counts an unloaded chunk as solid, so at the streaming edge the camera
    /// pulls in rather than drifting into terrain that has not arrived yet.
    pub(super) fn world_camera(&self, aspect: f32) -> Camera {
        let shot = self.camera_shot();
        let yaw = self.framing_yaw();
        let eye = self
            .sim
            .player
            .interpolated_eye_position(self.view.render_alpha);

        let distance = if shot.distance <= 0.0 {
            // First person, or a sweep that has not left the eye yet: there is
            // no gap between camera and player for anything to get into.
            0.0
        } else {
            let clearance = Camera::new(self.view.fov_degrees, aspect).near_radius();
            camera::clear_distance(eye, shot.offset(yaw), shot.distance, clearance, |p| {
                self.sim.world.is_solid_for_collision(p)
            })
        };

        shot.camera(eye, yaw, distance, self.view.fov_degrees, aspect)
    }

    /// The yaw the shot is framed on.
    ///
    /// The *body* yaw while the inventory is up, not the look yaw: the model is
    /// drawn at `AnimationState::body_yaw`, which lags the look yaw and can sit
    /// a good way off it when the player is standing still. Framing on the look
    /// yaw would show a model visibly turned away from the camera.
    fn framing_yaw(&self) -> f32 {
        if self.inventory_anim.active() {
            self.sim.player_anim.body_yaw()
        } else {
            self.sim.player.yaw
        }
    }

    /// This frame's shot: the player's chosen perspective, blended toward the
    /// inventory's framing shot by however far through the sweep we are.
    ///
    /// Blending happens in [`Shot`]'s polar form, so a swing from behind the
    /// player to in front of them orbits around them instead of passing through
    /// their head at the halfway point.
    fn camera_shot(&self) -> Shot {
        let gameplay = self
            .sim
            .player
            .perspective
            .shot(self.sim.player.pitch, THIRD_PERSON_DISTANCE);
        let t = self.inventory_anim.progress();
        if t <= 0.0 {
            return gameplay;
        }
        // `layout` reasons in points — it subtracts a panel some hundreds of
        // points wide from the screen — so it has to be handed the *real*
        // screen rect. Handing it a normalised one collapses the whole stage to
        // zero width and slams the model into the left edge.
        let stage = crate::presentation::ui::inventory::layout(
            self.screen,
            self.sim.player.mode.is_creative(),
        )
        .stage_center_x;
        gameplay.blend(Shot::inspect(self.view.fov_degrees.to_radians(), stage), t)
    }

    /// Bring every GPU resource in line with the simulation state this frame.
    ///
    /// This is the single seam where rendering meets simulation: it is the only
    /// method in the in-game state that touches a [`RenderContext`]. Everything
    /// above it — streaming, mobs, fluids, interaction — is plain logic that
    /// runs without a GPU, which is what makes it testable.
    pub(super) fn refresh_view(&mut self, ctx: &Arc<RenderContext>) {
        // Overlays on the block under the crosshair. Both are drawn around the
        // block's targeting box, so cracks and outline hug a mushroom the same
        // way the crosshair does.
        let breaking = self
            .sim
            .breaking
            .as_ref()
            .map(|b| (b.block, self.hitbox_at(b.block), b.progress));
        self.view.update_break_overlay(ctx, breaking);
        let target = if self.sim.dead {
            None
        } else {
            self.targeted_block()
                .map(|hit| (hit.block, self.hitbox_at(hit.block)))
        };
        self.view.update_target_outline(ctx, target);

        // Chunk meshes: queue what the world dirtied, then spend the budget.
        let dirty = self.sim.world.take_dirty();
        self.view.enqueue_dirty(dirty);
        self.view.process_mesh_budget(
            ctx,
            &self.sim.world,
            BlockAppearance {
                blocks: &self.content.rules.blocks,
                face_tiles: &self.content.visuals.block_face_tiles,
                models: &self.content.visuals.models,
                placed: &self.content.visuals.block_models,
                baked: &self.content.visuals.baked_models,
                fluids: &self.content.visuals.fluid_textures,
            },
            super::MESH_BUDGET,
        );

        // Loose entities. One `Arc` clone releases the borrow on `self` for
        // the closure below, where three deep `Vec` clones used to.
        let loaded = self.content.clone();
        let (items, blocks) = (&loaded.rules.items, &loaded.rules.blocks);
        let models = &loaded.visuals.models;
        // What shape an item is, and which pass it belongs in. Shared by the
        // drops and by every hand that has to fall back to a cube or a sprite.
        let shape = |item| {
            let is_transparent = items
                .get(item)
                .get::<Placeable>()
                .is_some_and(|p| blocks.get(p.block).is_transparent());
            (loaded.item_shape(item), is_transparent)
        };
        let content = ModelContent {
            models,
            item_models: &loaded.visuals.item_models,
            tiles: &loaded.visuals.tiles,
            shape: &shape,
            placement: &self.editor,
            block_display: &loaded.visuals.block_item_display,
        };
        // The editor's ground preview, when it has one, rides along with the
        // real drops so it is drawn by exactly the same code.
        let preview = self.editor_ground_preview();
        let drops = self.sim.ecs.query::<(&ItemDrop, &Transform, &Body)>().map(
            |(_, (drop, transform, body))| DropSprite {
                item: drop.stack.item,
                center: drop.render_center(transform.position, &body.physics),
                size: drop.render_size(&body.physics),
                yaw: drop.spin_yaw(),
            },
        );
        self.view
            .update_drops_mesh(ctx, drops.chain(preview), content);
        // Simulated and replicated mobs alike: a kind, a place and an
        // animation. Replicas were animated in `update`, so this only reads.
        let mobs = self
            .sim
            .ecs
            .query::<(&Kind, &Transform, &Animation, &MobId)>()
            .map(|(_, (kind, transform, anim, _))| MobSprite {
                visual: &kind.visual,
                position: transform.position,
                yaw: anim.0.body_yaw(),
                pose: anim.0.pose(0.0),
            });
        self.view.update_mob_meshes(ctx, mobs, models);
        let arrows = self
            .sim
            .ecs
            .query::<(&Projectile, &Transform, &Velocity)>()
            .map(|(_, (_, transform, velocity))| ArrowSprite {
                position: transform.position,
                yaw: Projectile::yaw(velocity.0),
                size: Projectile::SIZE,
            });
        self.view
            .update_arrows_mesh(ctx, arrows, loaded.visuals.arrow_faces);

        self.view.update_player_mesh(
            ctx,
            &self.sim.player,
            &self.sim.player_anim,
            &self.sim.inventory,
            self.inventory_anim.progress(),
            content,
        );
        // Cheap after the first call, and this is the only place the entity
        // registry and the model registry are both in reach.
        self.view.bind_player_rig(&loaded.rules.entities, models);
        let alpha = self.view.render_alpha;
        let peers = self
            .sim
            .ecs
            .query::<(&RemotePlayer, &Animation)>()
            .map(|(_, (rp, anim))| PeerSprite {
                position: rp.interpolated_position(alpha),
                pitch: rp.pitch,
                anim: anim.0,
                held: rp.equipment.held.map(ItemId),
            });
        self.view.update_remote_meshes(ctx, peers, content);
    }
}

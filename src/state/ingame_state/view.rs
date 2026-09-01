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

use egui::{Rect, pos2, vec2};
use glam::Vec3;

use super::mobs::{RemoteMob, mob_mesh};
use super::{INSPECT_MODEL_FROM, OUTLINE_COLOR, REMOTE_MAX_SPEED, THIRD_PERSON_DISTANCE};
use crate::art::cracks;
use crate::content::BlockAppearance;
use crate::content::{ItemModel, ItemShape};
use crate::core::{Aabb, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle};
use crate::entity::camera::Shot;
use crate::entity::viewmodel::{self, HandPose};
use crate::entity::{
    AnimationState, Arrow, DroppedItem, HandAnchor, HumanoidModel, Mob, Player, camera,
};
use crate::inventory::{ARMOR_SIZE, Inventory, ItemId, ItemRegistry};
use crate::net::{PlayerId, RemotePlayer};
use crate::world::World;
use crate::world::meshing::{
    ItemSprite, mesh_block_overlay, mesh_chunk, push_item_cube, push_item_sprite,
};
use wyven_model::mesh as model_mesh;
use wyven_model::{DisplayContext, ModelId, ModelRegistry};
use wyven_render::{
    Camera, CpuMesh, ForegroundFrame, GpuLines, GpuMesh, LightParams, RenderContext,
    SceneFrame, SkyParams, Texture, TexturedMesh, TileRegistry, debug,
};
use wyven_voxel::FaceTextures;

/// Animation state for a remote player plus the position used to derive their
/// speed (no extra protocol data needed — movement is inferred from the change
/// in rendered position each frame).
struct RemoteAnim {
    anim: AnimationState,
    last_pos: Vec3,
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

    /// Local player model + its GPU mesh (only built in third person).
    player_model: HumanoidModel,
    player_mesh: Option<GpuMesh>,
    /// Procedural animation state for the local player's model.
    player_anim: AnimationState,
    remote_meshes: Vec<GpuMesh>,
    /// Per-remote-player animation, keyed by id.
    remote_anims: HashMap<PlayerId, RemoteAnim>,
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
            player_model: HumanoidModel::player(),
            player_mesh: None,
            player_anim: AnimationState::new(),
            remote_meshes: Vec::new(),
            remote_anims: HashMap::new(),
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

    /// Advance the local player's animation clock.
    pub fn advance_player_anim(&mut self, speed: f32, look_yaw: f32, dt: f32) {
        self.player_anim.advance(speed, look_yaw, dt);
    }

    /// Trigger the main-hand swing on the local player's model.
    pub fn trigger_swing(&mut self) {
        self.player_anim.trigger_swing();
    }

    /// Keep the main-hand swing looping while a held action continues.
    pub fn keep_swinging(&mut self) {
        self.player_anim.keep_swinging();
    }

    /// Rebuild the player model mesh in third person, or the view model in
    /// first — never both, since in first person the body is the camera.
    ///
    /// `show_body` forces the world model on even in first person, for the
    /// inventory's camera pan — which would otherwise swing out to frame an
    /// empty stage. It suppresses the view-model arm at the same instant, since
    /// the two are alternatives.
    pub fn update_player_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        inventory: &Inventory,
        items: &ItemRegistry,
        show_body: bool,
        content: ModelContent<'_>,
    ) {
        if player.perspective.is_first_person() && !show_body {
            self.player_mesh = None;
            self.held_mesh = None;
            self.held_atlas = None;
            self.update_hand_meshes(ctx, player, inventory, content);
            return;
        }
        self.hand_mesh = None;
        self.hand_held_mesh = None;
        self.hand_held_atlas = None;
        let pose = self.player_anim.pose(player.pitch);
        // The body is drawn at the torso yaw, which lags the look yaw the camera
        // uses; `pose.head_yaw` is what puts the face back where the player looks.
        // The hand anchor must take the *same* yaw, or the held item leaves the fist.
        let body_yaw = self.player_anim.body_yaw();
        let armor = inventory.equipped_armor();
        let mesh =
            self.player_model
                .build_mesh_armored(player.position, body_yaw, &pose, &armor, items);
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();

        let anchor = self
            .player_model
            .hand_anchor(player.position, body_yaw, &pose);
        self.held_mesh = self.bake_held(ctx, content, inventory, anchor);
        self.held_atlas = self.bake_held_atlas(ctx, content, inventory, anchor);
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
        inventory: &Inventory,
        anchor: HandAnchor,
    ) -> Option<(GpuMesh, bool)> {
        // A model always wins; this is only the fallback for items without one.
        if content.held(inventory).is_some() {
            return None;
        }
        let item = inventory.selected_stack()?.item;
        let (shape, is_transparent) = (content.shape)(item);
        let local = held_placement(shape, DisplayContext::ThirdPersonRightHand).matrix();
        let transform = model_mesh::anchor(anchor.position, anchor.yaw, anchor.pitch) * local;
        let mesh = self.shaped_item_mesh(ctx, shape, content.tiles, transform, transform)?;
        Some((mesh, is_transparent))
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
        let local = held.local(content.models, DisplayContext::ThirdPersonRightHand);
        let transform = model_mesh::anchor(anchor.position, anchor.yaw, anchor.pitch) * local;
        self.bake_model(ctx, content.models, held.id, transform)
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
        inventory: &Inventory,
        content: ModelContent<'_>,
    ) {
        let pose = HandPose {
            eye: player.interpolated_eye_position(self.render_alpha),
            yaw: player.yaw,
            pitch: player.pitch,
            swing: self.player_anim.swing_progress(),
            walk_phase: self.player_anim.walk_phase(),
            walk_amount: self.player_anim.walk_amount(),
        };
        let frame = pose.frame();

        let arm = viewmodel::arm_mesh(&self.player_model, frame);
        self.hand_mesh = GpuMesh::upload(&ctx.memory_allocator, &arm).ok().flatten();

        let held = content.held(inventory);
        self.hand_held_mesh = held.and_then(|held| {
            let local = held.local(content.models, DisplayContext::FirstPersonRightHand);
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
                let local = held_placement(shape, DisplayContext::FirstPersonRightHand).matrix();
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
            state.anim.advance(speed, yaw, dt);
            state.last_pos = pos;
            let pose = state.anim.pose(pitch);
            // Derived here rather than sent: only the look yaw crosses the wire, and
            // a torso that follows it is cosmetic, so every peer can work it out.
            let body_yaw = state.anim.body_yaw();

            let armor = armor_item_ids(armor_ids, items);
            let mesh = self
                .player_model
                .build_mesh_armored(pos, body_yaw, &pose, &armor, items);
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
                mob.anim.body_yaw(),
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
    ///
    /// `shaft` is the arrow item's own faces, resolved by the caller — an arrow
    /// in flight is not an inventory stack, so it cannot look itself up.
    pub fn update_arrows_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        arrows: &[Arrow],
        shaft: FaceTextures,
    ) {
        let mut mesh = CpuMesh::new();
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
            if let Some(model) = content.of(item.stack.item)
                && let Some(loaded) = content.models.get(model.id)
            {
                let transform = model_mesh::anchor(item.render_center(), item.spin_yaw(), 0.0)
                    * model.local(content.models, DisplayContext::Ground);
                let mesh = loaded.mesh.bake(transform);
                let entry = by_model.entry(model.id).or_default();
                entry.push_indexed(mesh.vertices, mesh.indices);
                continue;
            }
            let (shape, is_transparent) = shape(item.stack.item);
            let target = if is_transparent {
                &mut transparent
            } else {
                &mut opaque
            };
            match shape {
                ItemShape::Cube(faces) => push_item_cube(
                    target,
                    item.render_center(),
                    item.render_size(),
                    item.spin_yaw(),
                    &faces,
                ),
                ItemShape::Sprite(tile) => {
                    let sprite = self
                        .item_sprites
                        .entry(tile)
                        .or_insert_with(|| ItemSprite::new(tile, tiles.art(tile)));
                    push_item_sprite(
                        target,
                        sprite,
                        item.render_center(),
                        item.render_size(),
                        item.spin_yaw(),
                    );
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

    /// The camera for this frame, placed by the player's perspective. Physics
    /// ticks at a fixed rate, so the eye is blended between steps to stay smooth
    /// when the display runs faster than the simulation.
    ///
    /// The yaw the local player's model is *drawn* at — the torso, which eases
    /// after the look direction rather than tracking it.
    pub fn player_body_yaw(&self) -> f32 {
        self.player_anim.body_yaw()
    }

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
        if let Some((mesh, is_transparent)) = &self.held_atlas {
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
) -> wyven_model::display::ItemTransform {
    match shape {
        ItemShape::Cube(_) => viewmodel::block_placement(context),
        ItemShape::Sprite(_) => wyven_model::generated::default_display()
            .get(context)
            .unwrap_or_default(),
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
        let shot = self.camera_shot(aspect);
        let yaw = self.framing_yaw();
        let eye = self
            .player
            .interpolated_eye_position(self.view.render_alpha);

        let distance = if shot.distance <= 0.0 {
            // First person, or a sweep that has not left the eye yet: there is
            // no gap between camera and player for anything to get into.
            0.0
        } else {
            let clearance = Camera::new(self.view.fov_degrees, aspect).near_radius();
            camera::clear_distance(eye, shot.offset(yaw), shot.distance, clearance, |p| {
                self.world.is_solid_for_collision(p)
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
            self.view.player_body_yaw()
        } else {
            self.player.yaw
        }
    }

    /// This frame's shot: the player's chosen perspective, blended toward the
    /// inventory's framing shot by however far through the sweep we are.
    ///
    /// Blending happens in [`Shot`]'s polar form, so a swing from behind the
    /// player to in front of them orbits around them instead of passing through
    /// their head at the halfway point.
    fn camera_shot(&self, aspect: f32) -> Shot {
        let gameplay = self
            .player
            .perspective
            .shot(self.player.pitch, THIRD_PERSON_DISTANCE);
        let t = self.inventory_anim.progress();
        if t <= 0.0 {
            return gameplay;
        }
        let screen = Rect::from_min_size(pos2(0.0, 0.0), vec2(aspect, 1.0));
        let stage = crate::ui::inventory::layout(screen, self.player.mode.is_creative())
            .stage_center_x;
        gameplay.blend(
            Shot::inspect(self.view.fov_degrees.to_radians(), stage),
            t,
        )
    }

    /// Bring every GPU resource in line with the simulation state this frame.
    ///
    /// This is the single seam where rendering meets simulation: it is the only
    /// method in the in-game state that touches a [`RenderContext`]. Everything
    /// above it — streaming, mobs, fluids, interaction — is plain logic that
    /// runs without a GPU, which is what makes it testable.
    pub(super) fn refresh_view(&mut self, ctx: &Arc<RenderContext>, dt: f32) {
        // Overlays on the block under the crosshair. Both are drawn around the
        // block's targeting box, so cracks and outline hug a mushroom the same
        // way the crosshair does.
        let breaking = self
            .breaking
            .as_ref()
            .map(|b| (b.block, self.hitbox_at(b.block), b.progress));
        self.view.update_break_overlay(ctx, breaking);
        let target = if self.dead {
            None
        } else {
            self.targeted_block()
                .map(|hit| (hit.block, self.hitbox_at(hit.block)))
        };
        self.view.update_target_outline(ctx, target);

        // Chunk meshes: queue what the world dirtied, then spend the budget.
        let dirty = self.world.take_dirty();
        self.view.enqueue_dirty(dirty);
        self.view.process_mesh_budget(
            ctx,
            &self.world,
            BlockAppearance {
                blocks: &self.content.blocks,
                face_tiles: &self.content.block_face_tiles,
                models: &self.content.models,
                placed: &self.content.block_models,
                baked: &self.content.baked_models,
                fluids: &self.content.fluid_textures,
            },
            super::MESH_BUDGET,
        );

        // Loose entities. One `Arc` clone releases the borrow on `self` for
        // the closure below, where three deep `Vec` clones used to.
        let loaded = self.content.clone();
        let (items, blocks) = (&loaded.items, &loaded.blocks);
        let models = &loaded.models;
        // What shape an item is, and which pass it belongs in. Shared by the
        // drops and by every hand that has to fall back to a cube or a sprite.
        let shape = |item| {
            let is_transparent = items
                .get(item)
                .place_block
                .is_some_and(|b| blocks.get(b).is_transparent());
            (loaded.item_shape(item), is_transparent)
        };
        let content = ModelContent {
            models,
            item_models: &loaded.item_models,
            tiles: &loaded.tiles,
            shape: &shape,
        };
        self.view.update_drops_mesh(ctx, &self.drops, content);
        self.view
            .update_mob_meshes(ctx, &self.mobs.live, &mut self.mobs.remote, models, dt);
        self.view
            .update_arrows_mesh(ctx, &self.mobs.arrows, loaded.arrow_faces);

        // Animated humanoids. The local player settles to idle while the
        // inventory is open (movement is frozen).
        let local_speed = if self.inventory_open {
            0.0
        } else {
            let v = self.player.velocity;
            Vec3::new(v.x, 0.0, v.z).length()
        };
        self.view
            .advance_player_anim(local_speed, self.player.yaw, dt);
        // Geometry is drawn with culling off, so a camera inside the head sees
        // the back of its own faces. The pan starts on the eye and only clears
        // the head a little way in, which is where the body appears — the same
        // instant the panel starts fading in, so the arm-to-body cut lands on
        // one beat rather than two. It cannot be cross-faded: the shader is an
        // alpha-test `discard` with no per-draw opacity.
        let show_body = self.inventory_anim.progress() >= INSPECT_MODEL_FROM;
        self.view.update_player_mesh(
            ctx,
            &self.player,
            &self.inventory,
            &self.content.items,
            show_body,
            content,
        );
        self.view
            .update_remote_meshes(ctx, &self.peers.players, &self.content.items, dt);
    }
}

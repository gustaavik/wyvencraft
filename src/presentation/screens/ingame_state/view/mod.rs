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
}

mod chunks;
mod entities;
mod frame;
mod framing;
mod overlays;
mod player;
mod refresh;

use frame::held_placement;

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
        }
    }
}

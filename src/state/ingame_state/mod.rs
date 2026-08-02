//! The playing state: owns the world, the local player, the inventory, and the
//! background chunk loader, and ties together input → simulation → rendering data.
//!
//! The implementation is split across sibling modules by concern; each adds
//! `impl` blocks to [`InGameState`] defined here:
//! - [`setup`] — construction from new/saved/host/client sessions.
//! - [`chat`] — chat relay, command authorization, and what a command does.
//! - [`net`] — the per-frame network pump and wire (de)serialization.
//! - [`streaming`] — chunk request/insert/unload and mesh budgeting.
//! - [`interaction`] — block break/place, mining, drops, target outline.
//! - [`mobs`] — mob spawning, AI perception/updates, and their attacks.
//! - [`view`] — every GPU resource, the camera, and the animation clocks.
//! - [`inventory`] — the inventory-screen click/craft handlers.
//! - [`persistence`] — world save + restore.
//! - [`frame`] — the [`GameState`] impl (update/ui/scene_frame/preview_frame).

mod chat;
mod frame;
mod interaction;
mod inventory;
mod mobs;
mod net;
mod peers;
mod persistence;
mod setup;
mod streaming;
mod view;

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;

use crate::chat::{ChatState, OpsList};
use crate::content::ItemModel;
use crate::core::{BlockPos, DayCycle};
use crate::entity::{Arrow, DroppedItem, EntityRegistry, Mob, Player, SpawnConfig, Spawner};
use crate::inventory::{Inventory, ItemRegistry, ItemStack, RecipeBook};
use crate::model::ModelRegistry;
use crate::state::session::Session;
use crate::world::{BlockRegistry, ChunkLoader, FluidSim, World};
use peers::Peers;
use persistence::Persistence;
use view::SceneCache;

/// The host's own player always has this id; clients are numbered from 1.
pub(crate) use crate::state::session::HOST_PLAYER_ID;

/// Chunks generated synchronously at startup so the player has ground to stand on.
const SPAWN_RADIUS: i32 = 1;
/// Keep chunks loaded this many chunks beyond the render distance before unloading.
const UNLOAD_MARGIN: i32 = 2;
/// Max new generation requests issued per frame (nearest-first).
const REQUEST_BUDGET: usize = 64;
/// Max chunk meshes (re)built per frame — bounds per-frame CPU + upload cost.
const MESH_BUDGET: usize = 8;
/// Camera distance behind/in front of the player in third-person view.
const THIRD_PERSON_DISTANCE: f32 = 4.0;
/// Radians of preview rotation per pixel dragged across the model preview.
const PREVIEW_DRAG_SENSITIVITY: f32 = 0.01;
/// Upper bound on a remote player's derived speed, so a teleport or first snapshot
/// can't drive an absurd walk cadence.
const REMOTE_MAX_SPEED: f32 = 12.0;
/// Max gap (s) between two jump presses to count as a double-tap (creative fly).
const DOUBLE_TAP_WINDOW: f32 = 0.3;
/// How often (s) a client reports its survival stats to the host.
const STATS_INTERVAL: f32 = 0.25;
/// Max edits per `WorldEdits` batch when replaying world state to a joining client.
/// ~4096 edits ≈ ~60 KB/message, well under the reliable channel's 5 MB budget.
const WORLD_SYNC_BATCH: usize = 4096;
/// Seconds between periodic autosaves (persistent worlds only).
const AUTOSAVE_INTERVAL: f32 = 60.0;
/// How often (s) a client checks whether to report its inventory to the host.
const INVENTORY_SYNC_INTERVAL: f32 = 1.0;
/// Colour of the selection outline on the targeted block (near-black).
const OUTLINE_COLOR: [f32; 3] = [0.05, 0.05, 0.05];

/// Progressive break state for survival timed mining.
struct BreakState {
    block: BlockPos,
    /// Accumulated progress in `[0, 1)`; the block breaks at `>= 1.0`.
    progress: f32,
}

pub struct InGameState {
    pub world: World,
    pub player: Player,
    pub blocks: Arc<BlockRegistry>,
    pub items: Arc<ItemRegistry>,
    pub entities: Arc<EntityRegistry>,
    /// Geometry loaded from model files, referenced by entity visuals and items.
    pub models: Arc<ModelRegistry>,
    /// Which model each item is drawn as, indexed by `ItemId`. Visual-only, so
    /// it lives beside the registries rather than inside `ItemRegistry`.
    pub item_models: Arc<Vec<Option<ItemModel>>>,
    /// Fingerprint of the loaded content; hosts send it in `Welcome` so
    /// mismatched clients refuse the session (raw ids cross the wire).
    content_hash: u64,
    pub inventory: Inventory,
    /// Crafting recipes, loaded from `assets/recipes.toml` at world start.
    pub recipes: RecipeBook,
    pub show_debug: bool,
    /// Every GPU resource this session has uploaded, plus the camera
    /// parameters and animation clocks that feed them.
    view: SceneCache,
    /// Background terrain generation.
    loader: ChunkLoader,
    /// Time-of-day clock driving the sky and world lighting.
    day_cycle: DayCycle,
    /// Inventory screen state.
    inventory_open: bool,
    /// The chat history this peer has seen and the line it is typing. Purely
    /// local: only the messages travel, never this.
    chat: ChatState,
    /// Who may run op-only commands, by stable client identity. Loaded from
    /// `ops.toml` on the authority; always empty on a client, which never
    /// decides anything.
    ops: OpsList,
    /// Stack currently "held" by the cursor in the inventory screen.
    held: Option<ItemStack>,
    /// Where the player (re)spawns on death.
    spawn: Vec3,
    /// Water flow simulation. Only singleplayer/host sessions tick it (the
    /// authority); clients receive the resulting edits over the network.
    fluids: FluidSim,
    /// Progressive block-break state for survival timed mining.
    breaking: Option<BreakState>,
    /// Live mobs, simulated by the authority (singleplayer/host) only.
    /// Clients keep interpolated replicas in `remote_mobs` instead.
    mobs: Vec<Mob>,
    /// Next value for a host-allocated [`crate::entity::MobId`].
    next_mob_id: u64,
    /// Mob spawn rules from `assets/spawning.toml` (part of the content hash).
    spawning: Arc<SpawnConfig>,
    /// The periodic spawn scheduler (authority only; idle on clients).
    spawner: Spawner,
    /// Client: replicas of the host's mobs, keyed by wire id.
    remote_mobs: HashMap<u64, mobs::RemoteMob>,
    /// Arrows in flight. Every peer simulates the ones it knows about; only
    /// the authority applies their damage.
    arrows: Vec<Arrow>,
    /// Item drops lying in the world. Local-only: not synced over the network.
    drops: Vec<DroppedItem>,
    /// True while the player is dead and awaiting respawn (control frozen).
    dead: bool,
    /// Time (s) since the last jump press, for creative double-tap-to-fly.
    jump_tap_timer: f32,
    /// Unspent frame time owed to the fixed-rate player physics step. Keeping
    /// player physics off the variable frame delta is what makes jump height
    /// identical at every framerate.
    physics_accum: f32,
    /// This session's networking role: who has authority, and how messages
    /// reach the other peers (a no-op transport in singleplayer).
    session: Box<dyn Session>,
    /// The other peers in this session and what we still owe them.
    peers: Peers,
    /// Where this session's world is persisted, and what it still owes the
    /// next save.
    save: Persistence,
}

impl InGameState {
    /// Reset the player at the world spawn after death.
    fn respawn(&mut self) {
        self.player.respawn_at(self.spawn);
        self.dead = false;
        self.breaking = None;
    }
}

#[cfg(test)]
mod tests {
    use super::net::{recipes_from_wire, recipes_to_wire};
    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::net::RecipeData;

    #[test]
    fn recipe_book_survives_the_wire_roundtrip() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let book = RecipeBook::load(&items);
        assert!(!book.recipes().is_empty());

        let wire = recipes_to_wire(&book, &items);
        let back = recipes_from_wire(&wire, &items);

        assert_eq!(back.recipes().len(), book.recipes().len());
        for (a, b) in book.recipes().iter().zip(back.recipes()) {
            assert_eq!(a.output, b.output);
            assert_eq!(a.count, b.count);
            assert_eq!(a.ingredients, b.ingredients);
        }
    }

    #[test]
    fn unknown_wire_recipes_are_skipped_not_fatal() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let wire = vec![
            RecipeData {
                output: "modded item this build lacks".to_string(),
                count: 1,
                ingredients: vec![("wood".to_string(), 1)],
            },
            RecipeData {
                output: "glass".to_string(),
                count: 1,
                ingredients: vec![("sand".to_string(), 1)],
            },
        ];
        let book = recipes_from_wire(&wire, &items);
        assert_eq!(book.recipes().len(), 1);
        assert_eq!(book.recipes()[0].output, items.find("glass").unwrap());
    }

    /// End-to-end combat: spawn a cow next to the player, punch it to death,
    /// and confirm its raw-beef loot pops as dropped items.
    #[test]
    fn killing_a_cow_drops_raw_beef() {
        let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        let cow_kind = state.entities.find("cow").expect("cow kind");
        let max_health = cow_kind.mob.as_ref().unwrap().max_health;

        // Stand the cow on the ground right in front of the player.
        let look = state.player.look_direction();
        let pos = state.player.position + Vec3::new(look.x, 0.0, look.z).normalize() * 2.0;
        let ground = state
            .find_ground(pos.x, pos.z, crate::core::CHUNK_HEIGHT - 2)
            .expect("ground near spawn");
        state
            .spawn_mob("cow", Vec3::new(pos.x, ground, pos.z))
            .expect("cow spawns");

        // The crosshair ray finds it (it may need to be exactly ahead: aim by
        // construction, the player looks along `look` from the eye).
        let Some(mobs::MobTargetRef::Local(index)) = state.targeted_mob() else {
            panic!("cow should be under the crosshair as a local mob");
        };

        // Punch until dead; the next tick reaps it and rolls the drop table.
        let hits = (max_health / 2.0).ceil() as usize; // PLAYER_ATTACK_DAMAGE
        for _ in 0..hits {
            state.attack_mob(mobs::MobTargetRef::Local(index));
        }
        state.update_mobs(1.0 / 60.0);
        assert!(state.mobs.is_empty(), "cow should be dead and reaped");
        assert!(!state.drops.is_empty(), "death should drop loot");
        let beef = state.items.find("raw beef").unwrap();
        let dropped: u32 = state
            .drops
            .iter()
            .filter(|d| d.stack.item == beef)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert!(
            (1..=3).contains(&dropped),
            "cow drops 1..=3 raw beef, got {dropped}"
        );
    }

    /// End-to-end persistence: create a world, play (edit terrain, move, change
    /// the inventory), save, and reload it through the real `InGameState` path.
    #[test]
    fn saved_world_roundtrips_through_ingame_state() {
        use crate::save::WorldSave;
        use crate::world::block::blocks;

        let root = std::env::temp_dir().join(format!("wyven-ingame-save-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);

        let game = WorldSave::create(&root, "Roundtrip", 42, GameMode::Survival)
            .unwrap()
            .load()
            .unwrap();
        let mut state = InGameState::new_saved(GameContent::builtin(), game);

        // "Play": place a block above ground, move, rearrange the inventory,
        // and share the world with a slightly hurt zombie.
        let edit_pos = BlockPos::new(3, 200, 5);
        assert!(state.world.set_block(edit_pos, blocks::STONE).is_some());
        state.player.position = Vec3::new(10.0, 90.0, -4.0);
        state.player.health = 13.5;
        state.inventory.set_slot(
            8,
            Some(ItemStack::new(state.items.find("bread").unwrap(), 2)),
        );
        state.inventory.set_selected(8);
        state
            .spawn_mob("zombie", Vec3::new(6.0, 80.0, 6.0))
            .expect("zombie spawns");
        state.mobs[0].health = 11.0;
        state.mobs[0].night_spawned = true;
        state.save_world();
        drop(state);

        let game = WorldSave::open(&root, "roundtrip").unwrap().load().unwrap();
        let state = InGameState::new_saved(GameContent::builtin(), game);
        assert_eq!(
            state.world.block_at(edit_pos),
            blocks::STONE,
            "terrain edit persists"
        );
        assert_eq!(state.player.position, Vec3::new(10.0, 90.0, -4.0));
        assert_eq!(state.player.health, 13.5);
        assert_eq!(
            state.inventory.slot(8),
            Some(ItemStack::new(state.items.find("bread").unwrap(), 2))
        );
        assert_eq!(state.inventory.selected_index(), 8);
        assert_eq!(state.mobs.len(), 1, "the zombie survives the reload");
        assert_eq!(state.mobs[0].kind_name, "zombie");
        assert_eq!(state.mobs[0].position, Vec3::new(6.0, 80.0, 6.0));
        assert_eq!(state.mobs[0].health, 11.0);
        assert!(state.mobs[0].night_spawned, "daylight rule survives");

        let _ = std::fs::remove_dir_all(&root);
    }
}

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
use crate::content::GameContent;
use crate::core::{BlockPos, DayCycle};
use crate::entity::{Arrow, DroppedItem, Mob, Player, Spawner};
use crate::inventory::{HeldLabel, Inventory, ItemStack, RecipeBook};
use crate::state::session::Session;
use crate::world::{ChunkLoader, FluidSim, World};
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

/// The living population of a session.
struct MobWorld {
    /// Mobs this peer simulates (the authority) or renders from snapshots.
    live: Vec<Mob>,
    next_id: u64,
    /// Seeded spawn planner — deterministic in (seed, tick), so a host and its
    /// clients agree without exchanging the decision.
    spawner: Spawner,
    /// Mobs a client knows about only from the host's snapshots.
    remote: HashMap<u64, mobs::RemoteMob>,
    arrows: Vec<Arrow>,
}

impl MobWorld {
    /// Empty, with a spawn planner seeded from the world.
    fn new(seed: u64) -> Self {
        Self {
            live: Vec::new(),
            next_id: 0,
            spawner: Spawner::new(seed),
            remote: HashMap::new(),
            arrows: Vec::new(),
        }
    }
}

pub struct InGameState {
    pub world: World,
    pub player: Player,
    /// Everything loaded from `assets/*.toml`, shared by every session.
    ///
    /// Held whole rather than destructured into a field per registry: eleven of
    /// those were just this `Arc` taken apart, and putting them back means the
    /// systems below can each borrow the one table they need without the state
    /// growing a field every time content does.
    pub content: Arc<GameContent>,
    pub inventory: Inventory,
    /// The fading name of the item in hand, shown above the hotbar. Ephemeral
    /// presentation state, so it is never saved and never crosses the wire.
    held_label: HeldLabel,
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
    /// Everything alive that is not a player: the mobs this peer simulates,
    /// the ones a host told it about, and the arrows in flight.
    ///
    /// Grouped because they are one concern with one lifetime — a mob spawns,
    /// shoots, dies and drops together, and nothing outside `mobs` and `view`
    /// touches any of them. The *methods* stay on `InGameState`: mob AI
    /// perceives the world and attacks the player, so moving them here would
    /// replace one honest `&mut self` with six borrows threaded through every
    /// call — not less coupling, only less visible coupling.
    mobs: MobWorld,
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
    use crate::inventory::ItemRegistry;
    use crate::net::RecipeData;
    use crate::world::BlockRegistry;

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
        let cow_kind = state.content.entities.find("cow").expect("cow kind");
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
        assert!(state.mobs.live.is_empty(), "cow should be dead and reaped");
        assert!(!state.drops.is_empty(), "death should drop loot");
        let beef = state.content.items.find("raw_beef").unwrap();
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

    /// End-to-end: place a plant in front of the player, break it, and confirm
    /// its own item pops out. Ground cover is `solid = false`, so this also
    /// pins the split between "collides" and "can be put in the crosshair" —
    /// before that split a walk-through block was simply unbreakable.
    #[test]
    fn breaking_ground_cover_drops_its_own_item() {
        use crate::world::block::blocks;

        let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        // Two blocks ahead at eye level, well inside reach.
        let look = state.player.look_direction();
        let at = BlockPos::from_world(state.player.eye_position() + look * 2.0);
        state.world.set_block(at, blocks::RED_MUSHROOM);

        assert!(
            !state.world.is_solid(at),
            "ground cover must not collide with the player"
        );
        let hit = state.targeted_block().expect("plant is in the crosshair");
        assert_eq!(hit.block, at, "the crosshair stops at the plant");

        assert!(state.break_block_at(at));
        assert!(state.world.block_at(at).is_air(), "the plant is gone");

        let expected = state
            .content
            .items
            .find("red_mushroom")
            .expect("shipped item");
        let dropped: u32 = state
            .drops
            .iter()
            .filter(|d| d.stack.item == expected)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert_eq!(
            dropped, 1,
            "breaking a plant yields exactly one of its item"
        );
    }

    /// The crosshair must hit ground cover on the plant, not on the cell around
    /// it: a ray through the top corner of a mushroom's block passes over the
    /// mushroom, while one through its middle stops on it.
    #[test]
    fn the_crosshair_hits_the_plant_not_the_cell_around_it() {
        use crate::world::Target;
        use crate::world::block::blocks;

        let mut state = InGameState::new(GameContent::load(), 7, GameMode::Survival);
        // High above the terrain, inside the chunks loaded around spawn, so the
        // rays below travel through nothing but air and the two test blocks.
        let at = BlockPos::new(2, 200, 2);
        assert!(state.world.set_block(at, blocks::RED_MUSHROOM).is_some());

        let Some(Target::Box(box_)) = state.target_at(at) else {
            panic!("ground cover must offer a box, not a whole cell");
        };
        let size = box_.max - box_.min;
        assert!(
            size.x < 0.5 && size.y < 0.7,
            "hitbox {size:?} is cell-sized"
        );
        assert_eq!(state.hitbox_at(at), box_, "the outline uses the same box");

        // A plain block behind it still fills its cell.
        let stone = BlockPos::new(6, 200, 2);
        assert!(state.world.set_block(stone, blocks::STONE).is_some());
        assert!(matches!(state.target_at(stone), Some(Target::Cell)));

        // Fire along +X through the middle of the mushroom: hits it.
        let mid = Vec3::new(-2.0, 200.3, 2.5);
        let hit = crate::world::raycast(mid, Vec3::X, 20.0, |p| state.target_at(p));
        assert_eq!(hit.expect("hit").block, at, "aimed at the cap");

        // The same ray nudged sideways still crosses the mushroom's *cell*, but
        // misses the mushroom — so it carries on to the stone behind.
        let corner = Vec3::new(-2.0, 200.3, 2.95);
        let past = crate::world::raycast(corner, Vec3::X, 20.0, |p| state.target_at(p));
        assert_eq!(
            past.expect("hit").block,
            stone,
            "the corner of the cell is not the mushroom"
        );
    }

    /// The crosshair reaches *through* water but stops on ground cover: both
    /// are `solid = false`, and only the fluid check separates them.
    #[test]
    fn water_stays_untargetable_while_ground_cover_does_not() {
        use crate::world::block::blocks;

        let state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        let probe = BlockPos::new(0, 200, 0); // empty sky, nothing generated
        assert!(!state.world.is_targetable(probe), "air");

        let registry = &state.content.blocks;
        assert!(registry.get(blocks::STONE).solid);
        for (id, name) in [
            (blocks::WATER, "water"),
            (blocks::WATER_FLOW_1, "flowing water"),
        ] {
            let block = registry.get(id);
            assert!(
                !block.solid && !block.is_replaceable(),
                "{name} must stay out of the crosshair"
            );
        }
        for id in [blocks::BLUE_BELLS, blocks::RED_MUSHROOM] {
            let block = registry.get(id);
            assert!(!block.solid, "{} walks through", block.id);
            assert!(block.is_replaceable(), "{} is built over", block.id);
        }
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
            Some(ItemStack::new(
                state.content.items.find("bread").unwrap(),
                2,
            )),
        );
        state.inventory.set_selected(8);
        state
            .spawn_mob("zombie", Vec3::new(6.0, 80.0, 6.0))
            .expect("zombie spawns");
        state.mobs.live[0].health = 11.0;
        state.mobs.live[0].night_spawned = true;
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
            Some(ItemStack::new(
                state.content.items.find("bread").unwrap(),
                2
            ))
        );
        assert_eq!(state.inventory.selected_index(), 8);
        assert_eq!(state.mobs.live.len(), 1, "the zombie survives the reload");
        assert_eq!(state.mobs.live[0].kind_name, "zombie");
        assert_eq!(state.mobs.live[0].position, Vec3::new(6.0, 80.0, 6.0));
        assert_eq!(state.mobs.live[0].health, 11.0);
        assert!(state.mobs.live[0].night_spawned, "daylight rule survives");

        let _ = std::fs::remove_dir_all(&root);
    }
}

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
//! - [`inventory`] — the inventory-screen click handlers.
//! - [`crafting`] — recipe discovery, stations in reach, and crafting.
//! - [`persistence`] — world save + restore.
//! - [`frame`] — the [`GameState`] impl (update/ui/scene_frame).

mod block_use;
mod bosses;
mod chat;
mod crafting;
mod editor;
mod frame;
mod interaction;
mod inventory;
mod mobs;
mod net;
mod peers;
mod persistence;
mod progression_net;
mod setup;
mod streaming;
mod view;
mod wayfinding;

use std::sync::Arc;

use glam::Vec3;

use crate::application::ecs::Ecs;
use crate::application::session::Session;
use crate::domain::chat::{ChatState, OpsList};
use crate::domain::core::{BlockPos, DayCycle};
use crate::domain::entity::{AnimationState, Player, Spawner};
use crate::domain::inventory::{HeldLabel, Inventory, ItemStack, RecipeBook};
use crate::domain::progression::WorldProgression;
use crate::domain::world::structure::Structures;
use crate::domain::world::{ChunkLoader, FluidSim, World};
use crate::presentation::content::GameContent;
use crate::presentation::editor::EditorSession;
use peers::Peers;
use persistence::Persistence;
use view::SceneCache;

/// The host's own player always has this id; clients are numbered from 1.
pub(crate) use crate::application::session::HOST_PLAYER_ID;

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
/// How far into the inventory sweep the player's own body appears.
///
/// Not zero: the sweep starts with the camera on the eye, and geometry is drawn
/// with culling off, so a body meshed at progress 0 would show the inside of
/// its own head. By this point the camera has pulled clear of it.
const INSPECT_MODEL_FROM: f32 = 0.25;
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
    next_id: u64,
    /// Seeded spawn planner — deterministic in (seed, tick), so a host and its
    /// clients agree without exchanging the decision.
    spawner: Spawner,
    /// The boss fight in progress, if one is (authority only).
    fight: Option<bosses::BossFight>,
    /// The boss attack being wound up, shown on the boss bar.
    telegraph: Option<bosses::Telegraph>,
}

impl MobWorld {
    /// Empty, with a spawn planner seeded from the world.
    fn new(seed: u64) -> Self {
        Self {
            next_id: 0,
            spawner: Spawner::new(seed),
            fight: None,
            telegraph: None,
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
    /// What this player has discovered, the stations in reach, and the
    /// crafting panel's selection.
    crafting: crafting::CraftingState,
    pub show_debug: bool,
    /// The item placement editor. Inert unless `WYVEN_EDITOR=1` asked for it,
    /// and even then it costs one hash lookup per held item per frame until
    /// something is actually moved.
    editor: EditorSession,
    /// Every GPU resource this session has uploaded, plus the camera
    /// parameters and animation clocks that feed them.
    view: SceneCache,
    /// Background terrain generation.
    loader: ChunkLoader,
    /// Time-of-day clock driving the sky and world lighting.
    day_cycle: DayCycle,
    /// Whether the player has asked for the inventory. The *rendered* state is
    /// `inventory_anim`, which lags this while the panel and camera move.
    inventory_open: bool,
    /// How far through the open/close sweep the panel and camera are.
    inventory_anim: inventory::OpenAnim,
    /// The egui screen rect, in points, as of this frame's `ui` pass.
    ///
    /// The camera needs it: the inventory's framing shot is derived from where
    /// `ui::inventory::layout` puts the panel, and that reasons in points — a
    /// fixed-size panel against the real screen. `scene_frame` is handed only an
    /// aspect ratio, so the size has to be carried across from `ui`, which the
    /// runner always runs first within the same frame.
    screen: egui::Rect,
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
    /// The last block the player was told is too hard for their tool, so the
    /// hint is said once per block rather than every frame of digging.
    tier_hint: Option<BlockPos>,
    /// Everything alive that is not a player: the mobs this peer simulates
    /// and the ones a host told it about.
    ///
    /// Grouped because they are one concern with one lifetime — a mob spawns,
    /// shoots, dies and drops together, and nothing outside `mobs` and `view`
    /// touches any of them. The *methods* stay on `InGameState`: mob AI
    /// perceives the world and attacks the player, so moving them here would
    /// replace one honest `&mut self` with six borrows threaded through every
    /// call — not less coupling, only less visible coupling.
    mobs: MobWorld,
    /// The session's entities: item drops lying in the world (local-only, never
    /// synced) and arrows in flight. See [`crate::application::ecs`].
    ecs: Ecs,
    /// True while the player is dead and awaiting respawn (control frozen).
    dead: bool,
    /// Time (s) since the last jump press, for creative double-tap-to-fly.
    jump_tap_timer: f32,
    /// Unspent frame time owed to the fixed-rate player physics step. Keeping
    /// player physics off the variable frame delta is what makes jump height
    /// identical at every framerate.
    physics_accum: f32,
    /// The local player's walk/idle/swing animation. Simulation state like
    /// the player it poses — advanced in `update`, only read by the view.
    player_anim: AnimationState,
    /// This session's networking role: who has authority, and how messages
    /// reach the other peers (a no-op transport in singleplayer).
    session: Box<dyn Session>,
    /// The other peers in this session and what we still owe them.
    peers: Peers,
    /// Where this session's world is persisted, and what it still owes the
    /// next save.
    save: Persistence,
    /// This world's shrines and altars, and the terrain sampler they were
    /// placed with — the generator's own, so the game locates exactly what
    /// the chunks contain.
    structures: Arc<Structures>,
    /// How far through the biome/boss loop this world is. Authoritative on the
    /// host and in singleplayer; a mirror of the host's on a client.
    progression: WorldProgression,
}

impl InGameState {
    /// Reset the player at the world spawn after death.
    fn respawn(&mut self) {
        self.player.respawn_at(self.spawn);
        self.dead = false;
        self.breaking = None;
    }
}

/// One simulated mob, read out of the ECS for a test to assert on.
#[cfg(test)]
#[derive(Debug, Clone)]
struct MobSnapshot {
    id: crate::domain::entity::MobId,
    entity: crate::application::ecs::Entity,
    kind: String,
    position: Vec3,
    health: f32,
    night_spawned: bool,
    boss: bool,
}

#[cfg(test)]
impl InGameState {
    /// Every mob this peer simulates, in storage order.
    fn simulated_mobs(&self) -> Vec<MobSnapshot> {
        use crate::application::ecs::components::{Boss, Health, Kind, Mob, MobId, Transform};
        self.ecs
            .query::<(&MobId, &Kind, &Transform, &Health, &Mob, Option<&Boss>)>()
            .map(|(entity, (id, kind, t, health, mob, boss))| MobSnapshot {
                id: *id,
                entity,
                kind: kind.name.clone(),
                position: t.position,
                health: health.current,
                night_spawned: mob.night_spawned,
                boss: boss.is_some(),
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::net::{recipes_from_wire, recipes_to_wire};
    use super::*;
    use crate::domain::core::GameMode;
    use crate::domain::inventory::ItemRegistry;
    use crate::domain::inventory::crafting::station_ids;
    use crate::domain::world::BlockRegistry;
    use crate::infrastructure::net::RecipeData;
    use crate::presentation::content::GameContent;

    #[test]
    fn recipe_book_survives_the_wire_roundtrip() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let stations = station_ids(&blocks);
        let book = crate::infrastructure::recipes::load_recipe_book(&items, &stations);
        assert!(!book.recipes().is_empty());

        let wire = recipes_to_wire(&book, &items);
        let back = recipes_from_wire(&wire, &items, &stations);

        assert_eq!(back.recipes(), book.recipes(), "stations included");
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
                station: None,
            },
            RecipeData {
                output: "glass".to_string(),
                count: 1,
                ingredients: vec![("sand".to_string(), 1)],
                station: None,
            },
        ];
        let book = recipes_from_wire(&wire, &items, &station_ids(&blocks));
        assert_eq!(book.recipes().len(), 1);
        assert_eq!(book.recipes()[0].output, items.find("glass").unwrap());
    }

    /// Clear a 9×9 pad of open air around the player, floored with stone
    /// under their feet, so a test's line of sight never depends on terrain.
    fn flatten_around_player(state: &mut InGameState) {
        use crate::domain::world::block::blocks;
        let feet = BlockPos::from_world(state.player.position);
        for dx in -4..=4 {
            for dz in -4..=4 {
                let floor = BlockPos::new(feet.x + dx, feet.y - 1, feet.z + dz);
                state.world.set_block(floor, blocks::STONE);
                for dy in 0..4 {
                    let air = BlockPos::new(feet.x + dx, feet.y + dy, feet.z + dz);
                    state
                        .world
                        .set_block(air, crate::domain::core::BlockId::AIR);
                }
            }
        }
    }

    /// End-to-end combat: spawn a cow next to the player, punch it to death,
    /// and confirm its raw-raw_beef loot pops as dropped items.
    #[test]
    fn killing_a_cow_drops_beef() {
        let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        let cow_kind = state.content.rules.entities.find("cow").expect("cow kind");
        let max_health = cow_kind.mob.as_ref().unwrap().max_health;

        // Stand the cow on the ground right in front of the player, on a pad
        // carved flat so whatever the seed grew at spawn cannot hide it.
        flatten_around_player(&mut state);
        let look = state.player.look_direction();
        let pos = state.player.position + Vec3::new(look.x, 0.0, look.z).normalize() * 2.0;
        let ground = state
            .find_ground(pos.x, pos.z, crate::domain::core::CHUNK_HEIGHT - 2)
            .expect("ground near spawn");
        state
            .spawn_mob("cow", Vec3::new(pos.x, ground, pos.z))
            .expect("cow spawns");

        // The crosshair ray finds it: aim from the eye down at its body, which
        // on flat ground sits below eye level.
        let body = Vec3::new(pos.x, ground + 0.6, pos.z) - state.player.eye_position();
        state.player.pitch = body.y.atan2(Vec3::new(body.x, 0.0, body.z).length());
        let Some(mobs::MobTargetRef::Local(index)) = state.targeted_mob() else {
            panic!("cow should be under the crosshair as a local mob");
        };

        // Punch until dead; the next tick reaps it and rolls the drop table.
        let hits = (max_health / 2.0).ceil() as usize; // PLAYER_ATTACK_DAMAGE
        for _ in 0..hits {
            state.attack_mob(mobs::MobTargetRef::Local(index));
        }
        state.update_mobs(1.0 / 60.0);
        assert!(
            state.simulated_mobs().is_empty(),
            "cow should be dead and reaped"
        );
        assert!(state.drops().next().is_some(), "death should drop loot");
        let raw_beef = state.content.rules.items.find("raw_beef").unwrap();
        let dropped: u32 = state
            .drops()
            .map(|(d, _)| d)
            .filter(|d| d.stack.item == raw_beef)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert!(
            (1..=3).contains(&dropped),
            "cow drops 1..=3 raw raw_beef, got {dropped}"
        );
    }

    /// End-to-end: place a plant in front of the player, break it, and confirm
    /// its own item pops out. Ground cover is `solid = false`, so this also
    /// pins the split between "collides" and "can be put in the crosshair" —
    /// before that split a walk-through block was simply unbreakable.
    #[test]
    fn breaking_ground_cover_drops_its_own_item() {
        use crate::domain::world::block::blocks;

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
            .rules
            .items
            .find("red_mushroom")
            .expect("shipped item");
        let dropped: u32 = state
            .drops()
            .map(|(d, _)| d)
            .filter(|d| d.stack.item == expected)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert_eq!(
            dropped, 1,
            "breaking a plant yields exactly one of its item"
        );
    }

    /// End-to-end for the block's own tool gate: oak leaves declare
    /// `[block.harvest] tool = "shears", required = true`, so they yield their
    /// block only for a shears-shaped tool. The rule lives entirely in
    /// `blocks.toml` — an axe is a perfectly good tool and still gets nothing,
    /// and no item anywhere had to be told that leaves exist.
    #[test]
    fn oak_leaves_drop_only_for_the_tool_they_ask_for() {
        use crate::domain::world::block::blocks;

        let leaves_dropped = |held: Option<&str>| {
            let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
            let hotbar = 0;
            state.inventory.set_selected(hotbar);
            state.inventory.set_slot(
                hotbar,
                held.map(|name| {
                    let id = state.content.rules.items.find(name).expect("shipped item");
                    state.content.rules.items.full_stack(id)
                }),
            );

            let look = state.player.look_direction();
            let at = BlockPos::from_world(state.player.eye_position() + look * 2.0);
            state.world.set_block(at, blocks::OAK_LEAVES);
            assert!(state.break_block_at(at));

            let leaves = state
                .content
                .rules
                .items
                .find("oak_leaves")
                .expect("shipped item");
            state
                .drops()
                .map(|(d, _)| d)
                .filter(|d| d.stack.item == leaves)
                .map(|d| u32::from(d.stack.count))
                .sum::<u32>()
        };

        assert_eq!(leaves_dropped(Some("shears")), 1, "the tool it asks for");
        assert_eq!(
            leaves_dropped(Some("iron_axe")),
            0,
            "any tool is not enough"
        );
        assert_eq!(leaves_dropped(None), 0, "a bare hand gets nothing");
    }

    /// The crosshair must hit ground cover on the plant, not on the cell around
    /// it: a ray through the top corner of a mushroom's block passes over the
    /// mushroom, while one through its middle stops on it.
    #[test]
    fn the_crosshair_hits_the_plant_not_the_cell_around_it() {
        use crate::domain::world::Target;
        use crate::domain::world::block::blocks;

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
        let hit = crate::domain::world::raycast(mid, Vec3::X, 20.0, |p| state.target_at(p));
        assert_eq!(hit.expect("hit").block, at, "aimed at the cap");

        // The same ray nudged sideways still crosses the mushroom's *cell*, but
        // misses the mushroom — so it carries on to the stone behind.
        let corner = Vec3::new(-2.0, 200.3, 2.95);
        let past = crate::domain::world::raycast(corner, Vec3::X, 20.0, |p| state.target_at(p));
        assert_eq!(
            past.expect("hit").block,
            stone,
            "the corner of the cell is not the mushroom"
        );
    }

    /// The bug this guards: a third-person camera placed a flat
    /// `THIRD_PERSON_DISTANCE` behind the eye walks straight into whatever is
    /// behind the player. It must stop short of a wall, and first person must
    /// stay exactly on the eye.
    #[test]
    fn the_third_person_camera_stops_short_of_a_wall_behind_the_player() {
        use crate::domain::entity::Perspective;
        use crate::domain::world::block::blocks;

        let mut state = InGameState::new(GameContent::load(), 7, GameMode::Survival);
        // High above the terrain, inside the chunks loaded around spawn, so the
        // only thing the camera can meet is the wall placed below.
        state.player.position = Vec3::new(2.5, 200.0, 2.5);
        // Fully caught up to the position just set, rather than interpolating
        // from wherever the player spawned.
        state.view.render_alpha = 1.0;
        // Looking down +X, so the third-person camera swings out along -X.
        state.player.yaw = std::f32::consts::FRAC_PI_2;
        state.player.pitch = 0.0;

        let aspect = 16.0 / 9.0;
        let eye = state.player.eye_position();

        state.player.perspective = Perspective::First;
        let first = state.world_camera(aspect);
        assert!(
            (first.position - eye).length() < 1.0e-5,
            "first person sits on the eye, got {:?}",
            first.position
        );

        // Nothing behind: the camera takes the whole distance.
        state.player.perspective = Perspective::ThirdBack;
        let open = state.world_camera(aspect);
        assert!(
            ((open.position - eye).length() - THIRD_PERSON_DISTANCE).abs() < 1.0e-3,
            "open sky should give the full pullback, got {:?}",
            open.position
        );

        // A wall two cells behind must push the camera in front of it. It goes
        // at the *eye's* height, not the feet's — the trace is horizontal.
        let wall = BlockPos::from_world(eye - Vec3::X * 2.0);
        assert!(state.world.set_block(wall, blocks::STONE).is_some());
        let blocked = state.world_camera(aspect);
        // The wall's near face is at x = 1.0. The camera must sit just outside
        // it — clear of the block, but not thrown all the way to the player.
        let gap = blocked.position.x - 1.0;
        assert!(
            gap > 0.0,
            "the camera entered the wall at x = 0..1, ending up at {:?}",
            blocked.position
        );
        assert!(
            gap < 0.5,
            "it stopped {gap} short of the wall, far more than the near plane needs"
        );
        assert!(
            blocked.position.x < eye.x,
            "it should still be behind the player, got {:?}",
            blocked.position
        );
    }

    /// The crosshair reaches *through* water but stops on ground cover: both
    /// are `solid = false`, and only the fluid check separates them.
    #[test]
    fn water_stays_untargetable_while_ground_cover_does_not() {
        use crate::domain::world::block::blocks;

        let state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        let probe = BlockPos::new(0, 200, 0); // empty sky, nothing generated
        assert!(!state.world.is_targetable(probe), "air");

        let registry = &state.content.rules.blocks;
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
        use crate::domain::world::block::blocks;
        use crate::infrastructure::save::WorldSave;

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
                state.content.rules.items.find("bread").unwrap(),
                2,
            )),
        );
        state.inventory.set_selected(8);
        state
            .spawn_mob("zombie", Vec3::new(6.0, 80.0, 6.0))
            .expect("zombie spawns");
        let zombie = state.simulated_mobs()[0].id;
        state.restore_mob(zombie, 11.0, true);
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
                state.content.rules.items.find("bread").unwrap(),
                2
            ))
        );
        assert_eq!(state.inventory.selected_index(), 8);
        let mobs = state.simulated_mobs();
        assert_eq!(mobs.len(), 1, "the zombie survives the reload");
        assert_eq!(mobs[0].kind, "zombie");
        assert_eq!(mobs[0].position, Vec3::new(6.0, 80.0, 6.0));
        assert_eq!(mobs[0].health, 11.0);
        assert!(mobs[0].night_spawned, "daylight rule survives");

        let _ = std::fs::remove_dir_all(&root);
    }
}

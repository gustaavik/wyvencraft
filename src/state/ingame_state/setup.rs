//! Construction of [`InGameState`] for each kind of session: fresh
//! singleplayer/host, a world loaded from disk, or a client joining a host.

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;

use super::net::recipes_from_wire;
use super::peers::Peers;
use super::persistence::Persistence;
use super::view::SceneCache;
use super::{DOUBLE_TAP_WINDOW, InGameState, SPAWN_RADIUS};
use crate::chat::{ChatState, OpsList};
use crate::content::GameContent;
use crate::core::{BlockPos, CHUNK_HEIGHT, ChunkPos, DayCycle, GameMode};
use crate::entity::{Player, Spawner};
use crate::inventory::{Inventory, RecipeBook};
use crate::net::{Client, Host, NetVec3, PlayerId, PlayerRestore, RecipeData};
use crate::save::{FileWorldRepository, SavedGame};
use crate::state::session::{ClientSession, HostSession, Session, SingleplayerSession};
use crate::world::{ChunkLoader, FluidSim, NoiseGenerator, World, WorldGenerator};

impl InGameState {
    /// Singleplayer world.
    pub fn new(content: Arc<GameContent>, seed: u64, mode: GameMode) -> Self {
        Self::build(
            content,
            seed,
            Box::new(SingleplayerSession),
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Host a multiplayer session (the host also plays locally).
    pub fn new_host(content: Arc<GameContent>, seed: u64, host: Host, mode: GameMode) -> Self {
        Self::build(
            content,
            seed,
            Box::new(HostSession::new(host)),
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Singleplayer session of a world loaded from (or just created on) disk.
    pub fn new_saved(content: Arc<GameContent>, game: SavedGame) -> Self {
        Self::from_save(content, game, Box::new(SingleplayerSession))
    }

    /// Host a multiplayer session of a world loaded from (or created on) disk.
    pub fn new_host_saved(content: Arc<GameContent>, game: SavedGame, host: Host) -> Self {
        Self::from_save(content, game, Box::new(HostSession::new(host)))
    }

    /// Build from a saved world: regenerate terrain from the saved seed, replay
    /// the edit overlay, and restore the player/inventory/clock. This runs
    /// before the first network pump, so restored edits are already in
    /// `World::edits` before any client can request world state.
    fn from_save(content: Arc<GameContent>, game: SavedGame, session: Box<dyn Session>) -> Self {
        let SavedGame {
            save,
            world,
            player,
            players,
            mobs,
        } = game;
        // Anchor spawn-area generation at the saved player position (or the
        // world's recorded spawn) so there's ground under a restored player.
        let anchor = player
            .as_ref()
            .map(|p| Vec3::from_array(p.position))
            .or_else(|| world.as_ref().map(|_| Vec3::from_array(save.meta.spawn)));
        let mut state = Self::build(
            content,
            save.meta.seed,
            session,
            anchor,
            DayCycle::new(save.meta.time_of_day),
            save.meta.game_mode,
            None,
        );
        if let Some(world) = &world {
            let resolved = world.resolve(&state.blocks);
            let count = resolved.len();
            for (pos, block) in resolved {
                state.world.apply_edit(pos, block);
            }
            // `build` conflates the generation anchor with the respawn point;
            // a saved world keeps its recorded spawn instead.
            state.spawn = Vec3::from_array(save.meta.spawn);
            log::info!("restored {count} world edits");
        }
        if let Some(player) = &player {
            player.apply(&mut state.player, &mut state.inventory, &state.items);
        }
        // Respawn the saved mob population. Fresh ids and brains (both are
        // session-scoped); unknown kinds fail soft like unknown blocks/items.
        let saved_mobs = mobs.mobs.len();
        for data in mobs.mobs {
            let position = Vec3::from_array(data.position);
            match state.spawn_mob(&data.kind, position) {
                Some(_) => {
                    if let Some(mob) = state.mobs.last_mut() {
                        mob.health = data.health.min(mob.params.max_health);
                        mob.night_spawned = data.night_spawned;
                    }
                }
                None => log::warn!(
                    "save references unknown mob kind '{}'; dropping it",
                    data.kind
                ),
            }
        }
        if saved_mobs > 0 {
            log::info!("restored {} of {saved_mobs} saved mobs", state.mobs.len());
        }
        log::info!(
            "loaded world '{}' (seed {}, time {:.3}, player {})",
            save.meta.name,
            save.meta.seed,
            save.meta.time_of_day,
            if player.is_some() {
                "restored"
            } else {
                "fresh"
            },
        );
        state.save.records = players;
        state.save.repository = Box::new(FileWorldRepository::new(save));
        state
    }

    /// Join a multiplayer session as a client (world built from the host's seed).
    /// `spawn` is the position the host assigned us in its `Welcome`; `time_of_day`
    /// seeds our day/night clock to the host's so skies match on join; `mode` is the
    /// session's game mode as told by the host; `recipes` are the host's crafting
    /// recipes (authoritative — the client's own recipe file is ignored);
    /// `restored` is our saved state if the host's world remembers us.
    #[allow(clippy::too_many_arguments)]
    pub fn new_client(
        content: Arc<GameContent>,
        seed: u64,
        client: Client,
        local_id: PlayerId,
        spawn: NetVec3,
        time_of_day: f32,
        mode: GameMode,
        recipes: Vec<RecipeData>,
        restored: Option<PlayerRestore>,
    ) -> Self {
        let mut state = Self::build(
            content,
            seed,
            Box::new(ClientSession::new(client, local_id)),
            Some(Vec3::from_array(spawn)),
            DayCycle::new(time_of_day),
            mode,
            Some(recipes),
        );
        if let Some(restored) = &restored {
            state.apply_restore(restored);
        }
        state
    }

    /// Build the in-game state. `spawn_override` (clients) places the player at the
    /// host-provided position and anchors synchronous generation there; otherwise the
    /// spawn is found over the origin column. `recipe_data` (clients) is the host's
    /// recipe book from the `Welcome`; hosts and singleplayer load the local file.
    fn build(
        content: Arc<GameContent>,
        seed: u64,
        session: Box<dyn Session>,
        spawn_override: Option<Vec3>,
        day_cycle: DayCycle,
        mode: GameMode,
        recipe_data: Option<Vec<RecipeData>>,
    ) -> Self {
        let blocks = content.blocks.clone();
        let items = content.items.clone();
        let entities = content.entities.clone();
        let models = content.models.clone();
        let item_models = Arc::new(content.item_models.clone());
        let block_models = Arc::new(content.block_models.clone());
        let recipes = match recipe_data {
            Some(data) => {
                let book = recipes_from_wire(&data, &items);
                log::info!("using {} crafting recipes from host", book.recipes().len());
                book
            }
            None => RecipeBook::load(&items),
        };

        let generator: Arc<dyn WorldGenerator> =
            Arc::new(NoiseGenerator::with_config(seed, content.worldgen.clone()));
        let mut world = World::new(generator.clone(), blocks.clone());

        // Worker pool sized to leave headroom for the main + render threads.
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2).max(1))
            .unwrap_or(4);
        let loader = ChunkLoader::new(generator, workers);

        // Synchronously generate the immediate spawn area so the player lands on
        // solid ground; the rest streams in via the loader. Anchor on the override
        // (the host-assigned spawn for clients) so there's ground under the player.
        let center = spawn_override
            .map(|p| BlockPos::from_world(p).chunk())
            .unwrap_or_else(|| ChunkPos::new(0, 0));
        for dx in -SPAWN_RADIUS..=SPAWN_RADIUS {
            for dz in -SPAWN_RADIUS..=SPAWN_RADIUS {
                world.ensure_chunk(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
        let spawn = spawn_override.unwrap_or_else(|| find_spawn(&world));

        // Creative starts empty (items come from the palette); survival gets the
        // starter kit declared in assets/items.toml so mining, durability, and
        // eating are usable without crafting.
        let mut inventory = Inventory::new();
        if !mode.is_creative() {
            for (slot, stack) in items.starter_kit_survival().iter().enumerate() {
                inventory.set_slot(slot, Some(*stack));
            }
        }

        // Read before `session` is moved into the struct below.
        let session_is_authority = session.is_authority();

        let mut state = Self {
            world,
            player: Player::new(spawn, mode, entities.player()),
            blocks,
            items,
            entities,
            models,
            item_models,
            block_models,
            content_hash: content.hash,
            inventory,
            recipes,
            show_debug: false,
            view: SceneCache::new(),
            loader,
            day_cycle,
            inventory_open: false,
            chat: ChatState::default(),
            // A client never authorizes anything, so it never reads the file.
            ops: if session_is_authority {
                OpsList::load()
            } else {
                OpsList::default()
            },
            held: None,
            spawn,
            fluids: FluidSim::new(),
            breaking: None,
            mobs: Vec::new(),
            next_mob_id: 0,
            spawning: content.spawning.clone(),
            spawner: Spawner::new(seed ^ 0x5EED_0F5B_A3B1_E5B0),
            remote_mobs: HashMap::new(),
            arrows: Vec::new(),
            drops: Vec::new(),
            dead: false,
            jump_tap_timer: DOUBLE_TAP_WINDOW * 2.0,
            physics_accum: 0.0,
            session,
            peers: Peers::default(),
            save: Persistence::none(),
        };
        if state.session.is_authority() {
            state.debug_spawn_from_env();
        }
        state
    }
}

/// Find a safe spawn (top solid block at the origin column + 1).
fn find_spawn(world: &World) -> Vec3 {
    for y in (0..CHUNK_HEIGHT).rev() {
        if world.is_solid(BlockPos::new(0, y, 0)) {
            return Vec3::new(0.5, (y + 1) as f32, 0.5);
        }
    }
    Vec3::new(0.5, 80.0, 0.5)
}

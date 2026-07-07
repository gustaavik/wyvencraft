//! The playing state: owns the world, the local player, the inventory, and the
//! background chunk loader, and ties together input → simulation → rendering data.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use winit::event::MouseButton;

use super::{GameState, PauseMenuState, StateContext, Transition};
use crate::core::{
    Aabb, BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle, GameMode,
};
use crate::entity::{AnimationState, DROP_SIZE, DroppedItem, HumanoidModel, Perspective, Player};
use crate::inventory::{INVENTORY_SIZE, Inventory, ItemId, ItemRegistry, ItemStack, RecipeBook};
use crate::net::{
    Channel, Client, ClientMessage, Host, NetItemStack, NetVec3, PlayerId, PlayerRestore,
    RecipeData, RemotePlayer, ServerMessage,
};
use crate::render::{
    Camera, CpuMesh, GpuLines, GpuMesh, LightParams, RenderContext, SceneFrame, SkyParams, debug,
    tiles,
};
use crate::save::{ItemStackData, PlayerData, PlayerRecords, SavedGame, WorldData, WorldSave};
use crate::ui::hud;
use crate::world::block::FaceTextures;
use crate::world::meshing::{mesh_block_overlay, mesh_chunk, push_item_cube};
use crate::world::{BlockRegistry, ChunkLoader, FluidSim, NoiseGenerator, World, WorldGenerator};

/// The networking role of this in-game session. Host/client drivers are boxed
/// to keep the enum near the size of its `Singleplayer` variant.
enum NetRole {
    Singleplayer,
    Host(Box<Host>),
    Client {
        client: Box<Client>,
        local_id: PlayerId,
    },
}

/// The host's own player always has this id; clients are numbered from 1.
const HOST_PLAYER_ID: PlayerId = PlayerId(0);

/// Reach distance for breaking/placing blocks.
const REACH: f32 = 5.0;
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
/// How far beyond the player's collision box dropped items are collected.
const PICKUP_RANGE: f32 = 1.0;
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
    pub items: ItemRegistry,
    pub inventory: Inventory,
    /// Crafting recipes, loaded from `assets/recipes.toml` at world start.
    pub recipes: RecipeBook,
    pub show_debug: bool,
    /// Background terrain generation.
    loader: ChunkLoader,
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
    fov_degrees: f32,
    /// Time-of-day clock driving the sky and world lighting.
    day_cycle: DayCycle,
    /// Inventory screen state.
    inventory_open: bool,
    /// Stack currently "held" by the cursor in the inventory screen.
    held: Option<ItemStack>,
    /// Where the player (re)spawns on death.
    spawn: Vec3,
    /// Water flow simulation. Only singleplayer/host sessions tick it (the
    /// authority); clients receive the resulting edits over the network.
    fluids: FluidSim,
    /// Progressive block-break state for survival timed mining.
    breaking: Option<BreakState>,
    /// Crack overlay drawn on the block being mined (rebuilt as progress grows).
    break_mesh: Option<GpuMesh>,
    /// Selection outline on the targeted block, cached until the target changes.
    outline_block: Option<BlockPos>,
    outline_mesh: Option<GpuLines>,
    /// Item drops lying in the world. Local-only: not synced over the network.
    drops: Vec<DroppedItem>,
    /// Combined GPU meshes for all drops, split by render pass (rebuilt per frame).
    drops_mesh: Option<GpuMesh>,
    drops_mesh_transparent: Option<GpuMesh>,
    /// Seconds since entering the state; drives shader animation (water frames).
    elapsed: f32,
    /// True while the player is dead and awaiting respawn (control frozen).
    dead: bool,
    /// Time (s) since the last jump press, for creative double-tap-to-fly.
    jump_tap_timer: f32,
    /// Throttle accumulator for sending stats over the network.
    stats_timer: f32,
    /// Networking role + remote players.
    net: NetRole,
    /// Persistence handle: `Some` when playing a named world as singleplayer or
    /// host; `None` for clients and ephemeral dev-boot worlds (never saved).
    save: Option<WorldSave>,
    /// Seconds accumulated toward the next periodic autosave.
    autosave_timer: f32,
    /// Host: saved per-identity player records for this world; handed back to
    /// returning clients and written to `players.dat`.
    player_records: PlayerRecords,
    /// Host: stable identity (netcode client id) of each connected player.
    remote_identities: HashMap<PlayerId, u64>,
    /// Host: latest inventory each client reported (kept in wire form; converted
    /// to the name-based disk form only when a record is written).
    remote_inventories: HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    /// Client: throttle + change detection for inventory reports to the host.
    inventory_sync_timer: f32,
    last_synced_inventory: Option<Inventory>,
    /// Client-only: whether we've asked the host for the initial world state yet.
    world_state_requested: bool,
    remote_players: HashMap<PlayerId, RemotePlayer>,
    remote_meshes: Vec<GpuMesh>,
    /// Per-remote-player animation, keyed by id. Speed is derived from the change in
    /// their rendered position each frame (no extra protocol data needed).
    remote_anims: HashMap<PlayerId, RemoteAnim>,
}

/// Animation state for a remote player plus the position used to derive their speed.
struct RemoteAnim {
    anim: AnimationState,
    last_pos: Vec3,
}

impl InGameState {
    /// Singleplayer world.
    pub fn new(seed: u64, mode: GameMode) -> Self {
        Self::build(
            seed,
            NetRole::Singleplayer,
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Host a multiplayer session (the host also plays locally).
    pub fn new_host(seed: u64, host: Host, mode: GameMode) -> Self {
        Self::build(
            seed,
            NetRole::Host(Box::new(host)),
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Singleplayer session of a world loaded from (or just created on) disk.
    pub fn new_saved(game: SavedGame) -> Self {
        Self::from_save(game, NetRole::Singleplayer)
    }

    /// Host a multiplayer session of a world loaded from (or created on) disk.
    pub fn new_host_saved(game: SavedGame, host: Host) -> Self {
        Self::from_save(game, NetRole::Host(Box::new(host)))
    }

    /// Build from a saved world: regenerate terrain from the saved seed, replay
    /// the edit overlay, and restore the player/inventory/clock. This runs
    /// before the first network pump, so restored edits are already in
    /// `World::edits` before any client can request world state.
    fn from_save(game: SavedGame, net: NetRole) -> Self {
        let SavedGame {
            save,
            world,
            player,
            players,
        } = game;
        // Anchor spawn-area generation at the saved player position (or the
        // world's recorded spawn) so there's ground under a restored player.
        let anchor = player
            .as_ref()
            .map(|p| Vec3::from_array(p.position))
            .or_else(|| world.as_ref().map(|_| Vec3::from_array(save.meta.spawn)));
        let mut state = Self::build(
            save.meta.seed,
            net,
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
        state.player_records = players;
        state.save = Some(save);
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
            seed,
            NetRole::Client {
                client: Box::new(client),
                local_id,
            },
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
        seed: u64,
        net: NetRole,
        spawn_override: Option<Vec3>,
        day_cycle: DayCycle,
        mode: GameMode,
        recipe_data: Option<Vec<RecipeData>>,
    ) -> Self {
        let blocks = Arc::new(BlockRegistry::with_builtins());
        let items = ItemRegistry::from_blocks(&blocks);
        let recipes = match recipe_data {
            Some(data) => {
                let book = recipes_from_wire(&data, &items);
                log::info!("using {} crafting recipes from host", book.recipes().len());
                book
            }
            None => RecipeBook::load(&items),
        };

        let generator: Arc<dyn WorldGenerator> = Arc::new(NoiseGenerator::new(seed));
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

        // Creative starts empty (items come from the palette); survival gets a small
        // starter kit so mining, durability, and eating are usable without crafting.
        let mut inventory = Inventory::new();
        if !mode.is_creative() {
            inventory.set_slot(0, Some(items.full_stack(items.wooden_pickaxe)));
            inventory.set_slot(1, Some(items.full_stack(items.wooden_axe)));
            inventory.set_slot(2, Some(items.full_stack(items.wooden_shovel)));
            inventory.set_slot(3, Some(ItemStack::new(items.apple, 5)));
            inventory.set_slot(4, Some(ItemStack::new(items.bread, 3)));
        }

        Self {
            world,
            player: Player::new(spawn, mode),
            blocks,
            items,
            inventory,
            recipes,
            show_debug: false,
            loader,
            meshes: HashMap::new(),
            transparent_meshes: HashMap::new(),
            mesh_queue: VecDeque::new(),
            queued: HashSet::new(),
            player_model: HumanoidModel::player(),
            player_mesh: None,
            player_anim: AnimationState::new(),
            fov_degrees: 70.0,
            day_cycle,
            inventory_open: false,
            held: None,
            spawn,
            fluids: FluidSim::new(),
            breaking: None,
            break_mesh: None,
            outline_block: None,
            outline_mesh: None,
            drops: Vec::new(),
            drops_mesh: None,
            drops_mesh_transparent: None,
            elapsed: 0.0,
            dead: false,
            jump_tap_timer: DOUBLE_TAP_WINDOW * 2.0,
            stats_timer: 0.0,
            net,
            save: None,
            autosave_timer: 0.0,
            player_records: PlayerRecords::default(),
            remote_identities: HashMap::new(),
            remote_inventories: HashMap::new(),
            inventory_sync_timer: 0.0,
            last_synced_inventory: None,
            world_state_requested: false,
            remote_players: HashMap::new(),
            remote_meshes: Vec::new(),
            remote_anims: HashMap::new(),
        }
    }

    /// Reset the player at the world spawn after death.
    fn respawn(&mut self) {
        self.player.respawn_at(self.spawn);
        self.dead = false;
        self.breaking = None;
    }

    /// Open/close the inventory screen; returns a held stack to storage on close.
    fn toggle_inventory(&mut self) {
        self.inventory_open = !self.inventory_open;
        if !self.inventory_open
            && let Some(held) = self.held.take()
        {
            self.inventory.add(held, &self.items);
        }
    }

    /// Click-to-move logic for an inventory slot (pick up / place / merge / swap).
    fn handle_slot_click(&mut self, index: usize) {
        match (self.held, self.inventory.slot(index)) {
            (None, Some(stack)) => {
                self.held = Some(stack);
                self.inventory.set_slot(index, None);
            }
            (Some(held), None) => {
                self.inventory.set_slot(index, Some(held));
                self.held = None;
            }
            (Some(mut held), Some(mut stack)) => {
                if held.item == stack.item {
                    let max = self.items.max_stack(stack.item);
                    let leftover = stack.merge(held, max);
                    self.inventory.set_slot(index, Some(stack));
                    self.held = if leftover == 0 {
                        None
                    } else {
                        held.count = leftover;
                        Some(held)
                    };
                } else {
                    // Swap held and slot.
                    self.inventory.set_slot(index, Some(held));
                    self.held = Some(stack);
                }
            }
            (None, None) => {}
        }
    }

    /// Craft the recipe at `index`: consume its ingredients and store the
    /// output; whatever doesn't fit is tossed out in front of the player.
    fn handle_craft(&mut self, index: usize) {
        let Some(recipe) = self.recipes.get(index) else {
            return;
        };
        let Some(stack) = recipe.craft(&mut self.inventory, &self.items) else {
            return;
        };
        let leftover = self.inventory.add(stack, &self.items);
        if leftover > 0 {
            self.drops.push(DroppedItem::thrown(
                ItemStack {
                    count: leftover,
                    ..stack
                },
                self.player.eye_position(),
                self.player.look_direction(),
            ));
        }
    }

    /// Rebuild the player model mesh in third person; drop it in first person.
    fn update_player_mesh(&mut self, ctx: &Arc<RenderContext>) {
        if self.player.perspective.is_first_person() {
            self.player_mesh = None;
            return;
        }
        let pose = self.player_anim.pose(self.player.pitch);
        let mesh = self
            .player_model
            .build_mesh(self.player.position, self.player.yaw, &pose);
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    /// Request/insert/unload chunks around the player using the worker pool.
    fn update_streaming(&mut self, radius: i32) {
        let center = BlockPos::from_world(self.player.position).chunk();

        // 1. Insert finished chunks (discard any that drifted out of range).
        let mut inserted = 0;
        for chunk in self.loader.drain_ready() {
            if center.chebyshev_distance(chunk.pos) <= radius + UNLOAD_MARGIN {
                self.world.insert_chunk(chunk);
                inserted += 1;
            }
        }
        if inserted > 0 {
            log::debug!(
                "streamed +{inserted} chunks (loaded={}, pending={})",
                self.world.loaded_count(),
                self.loader.pending_count()
            );
        }

        // 2. Request missing chunks within the radius, nearest first.
        let mut wanted: Vec<ChunkPos> = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let pos = ChunkPos::new(center.x + dx, center.z + dz);
                if !self.world.is_loaded(pos) && !self.loader.is_pending(pos) {
                    wanted.push(pos);
                }
            }
        }
        wanted.sort_by_key(|p| center.chebyshev_distance(*p));
        for pos in wanted.into_iter().take(REQUEST_BUDGET) {
            self.loader.request(pos);
        }

        // 3. Unload distant chunks and their meshes.
        let to_unload: Vec<ChunkPos> = self
            .world
            .loaded_positions()
            .filter(|p| center.chebyshev_distance(*p) > radius + UNLOAD_MARGIN)
            .collect();
        for pos in to_unload {
            self.world.unload_chunk(pos);
            self.meshes.remove(&pos);
            self.transparent_meshes.remove(&pos);
        }
    }

    /// Move freshly-dirtied chunks into the mesh queue (deduped).
    fn enqueue_dirty(&mut self) {
        for pos in self.world.take_dirty() {
            if self.queued.insert(pos) {
                self.mesh_queue.push_back(pos);
            }
        }
    }

    /// Rebuild up to [`MESH_BUDGET`] chunk meshes this frame.
    fn process_mesh_budget(&mut self, ctx: &Arc<RenderContext>) {
        for _ in 0..MESH_BUDGET {
            let Some(pos) = self.mesh_queue.pop_front() else {
                break;
            };
            self.queued.remove(&pos);

            let output = self
                .world
                .chunk(pos)
                .map(|chunk| mesh_chunk(chunk, &self.blocks, |p| self.world.block_at(p)));
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
                None => {
                    self.meshes.remove(&pos);
                    self.transparent_meshes.remove(&pos);
                }
            }
        }
    }

    /// Drive networking for one frame: process incoming, broadcast local state.
    fn pump_network(&mut self, dt: f32) {
        let duration = Duration::from_secs_f32(dt.max(1.0e-4));
        let position = self.player.position.to_array();
        let yaw = self.player.yaw;
        let pitch = self.player.pitch;
        let mode = self.player.mode;
        let health = self.player.health;
        let hunger = self.player.hunger;
        let saturation = self.player.saturation;

        // Survival stats are low-frequency; throttle them to keep the wire quiet.
        self.stats_timer += dt;
        let send_stats = self.stats_timer >= STATS_INTERVAL;
        if send_stats {
            self.stats_timer = 0.0;
        }

        // Clients report their inventory (throttled, only on change) so the
        // host can persist it in the world save.
        let mut inventory_sync = None;
        if matches!(self.net, NetRole::Client { .. }) {
            self.inventory_sync_timer += dt;
            if self.inventory_sync_timer >= INVENTORY_SYNC_INTERVAL {
                self.inventory_sync_timer = 0.0;
                let changed = self.last_synced_inventory.as_ref().is_none_or(|last| {
                    last.slots() != self.inventory.slots()
                        || last.selected_index() != self.inventory.selected_index()
                });
                if changed {
                    inventory_sync = Some(inventory_to_wire(&self.inventory));
                    self.last_synced_inventory = Some(self.inventory.clone());
                }
            }
        }

        // One-shot initial world-state request (client only). Captured here and
        // written back after the match to avoid borrowing `self` while `self.net` is.
        let need_world_request = !self.world_state_requested;
        let mut requested_world_state_now = false;

        match &mut self.net {
            NetRole::Singleplayer => {}
            NetRole::Host(host) => {
                host.pump(duration);
                let seed = self.world.seed();
                let time_of_day = self.day_cycle.time_of_day();

                for cid in host.take_joined() {
                    if let Some(pid) = host.player_id(cid) {
                        // The netcode client id doubles as the player's stable
                        // identity: returning players get their saved state back.
                        let identity: u64 = cid;
                        let restored = self
                            .player_records
                            .0
                            .get(&identity)
                            .map(|record| record_to_restore(record, &self.items));
                        let spawn = restored.as_ref().map(|r| r.position).unwrap_or(position);
                        if restored.is_some() {
                            log::info!("player {} rejoined; restoring saved state", pid.0);
                        }
                        let name = format!("Player {}", pid.0);
                        host.send(
                            cid,
                            &ServerMessage::Welcome {
                                seed,
                                your_id: pid,
                                spawn,
                                time_of_day,
                                game_mode: mode,
                                recipes: recipes_to_wire(&self.recipes, &self.items),
                                restored,
                            },
                            Channel::Reliable,
                        );
                        host.broadcast(
                            &ServerMessage::PlayerJoined {
                                id: pid,
                                name: name.clone(),
                            },
                            Channel::Reliable,
                        );
                        self.remote_identities.insert(pid, identity);
                        self.remote_players
                            .insert(pid, RemotePlayer::new(pid, name, Vec3::from_array(spawn)));
                    }
                }
                for pid in host.take_left() {
                    // Snapshot the leaving player so their state survives a rejoin.
                    record_remote(
                        &mut self.player_records,
                        &self.remote_identities,
                        &self.remote_players,
                        &self.remote_inventories,
                        &self.items,
                        pid,
                    );
                    self.remote_players.remove(&pid);
                    self.remote_identities.remove(&pid);
                    self.remote_inventories.remove(&pid);
                    host.broadcast(&ServerMessage::PlayerLeft { id: pid }, Channel::Reliable);
                }

                for (pid, msg) in host.receive() {
                    match msg {
                        ClientMessage::Move {
                            position,
                            yaw,
                            pitch,
                        } => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.push_snapshot(Vec3::from_array(position), yaw, pitch);
                            }
                        }
                        ClientMessage::Break { pos } => {
                            if self.world.set_block(pos, BlockId::AIR).is_some() {
                                self.fluids.block_changed(pos);
                                host.broadcast(
                                    &ServerMessage::BlockChanged {
                                        pos,
                                        block: BlockId::AIR,
                                    },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Place { pos, block } => {
                            if self.world.set_block(pos, block).is_some() {
                                self.fluids.block_changed(pos);
                                host.broadcast(
                                    &ServerMessage::BlockChanged { pos, block },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Stats {
                            health,
                            hunger,
                            saturation,
                        } => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.health = health;
                                rp.hunger = hunger;
                                rp.saturation = saturation;
                            }
                        }
                        ClientMessage::SetMode(m) => {
                            if let Some(rp) = self.remote_players.get_mut(&pid) {
                                rp.mode = m;
                            }
                        }
                        ClientMessage::SyncInventory { slots, selected } => {
                            self.remote_inventories.insert(pid, (slots, selected));
                        }
                        ClientMessage::Chat(_) => {}
                        ClientMessage::RequestWorldState => {
                            let edits = self.world.collect_edits();
                            log::debug!(
                                "replaying {} world edits to player {}",
                                edits.len(),
                                pid.0
                            );
                            for batch in edits.chunks(WORLD_SYNC_BATCH) {
                                host.send_to_player(
                                    pid,
                                    &ServerMessage::WorldEdits {
                                        edits: batch.to_vec(),
                                    },
                                    Channel::Chunk,
                                );
                            }
                        }
                    }
                }

                // Broadcast authoritative player snapshots.
                host.broadcast(
                    &ServerMessage::PlayerState {
                        id: HOST_PLAYER_ID,
                        position,
                        yaw,
                        pitch,
                    },
                    Channel::Unreliable,
                );
                let snapshots: Vec<_> = self
                    .remote_players
                    .iter()
                    .map(|(pid, rp)| (*pid, rp.position().to_array(), rp.yaw, rp.pitch))
                    .collect();
                for (id, position, yaw, pitch) in snapshots {
                    host.broadcast(
                        &ServerMessage::PlayerState {
                            id,
                            position,
                            yaw,
                            pitch,
                        },
                        Channel::Unreliable,
                    );
                }

                // Periodic authoritative vitals for the host and every remote player.
                if send_stats {
                    host.broadcast(
                        &ServerMessage::PlayerStats {
                            id: HOST_PLAYER_ID,
                            health,
                            hunger,
                            mode,
                        },
                        Channel::Reliable,
                    );
                    let stats: Vec<_> = self
                        .remote_players
                        .values()
                        .map(|rp| (rp.id, rp.health, rp.hunger, rp.mode))
                        .collect();
                    for (id, health, hunger, mode) in stats {
                        host.broadcast(
                            &ServerMessage::PlayerStats {
                                id,
                                health,
                                hunger,
                                mode,
                            },
                            Channel::Reliable,
                        );
                    }
                }
                host.flush();
            }
            NetRole::Client { client, local_id } => {
                let local_id = *local_id;
                if let Err(err) = client.pump(duration) {
                    log::warn!("client pump error: {err}");
                }
                // Ask the host to replay the world's existing edits, once connected.
                if need_world_request && client.is_connected() {
                    client.send(&ClientMessage::RequestWorldState, Channel::Reliable);
                    requested_world_state_now = true;
                }
                client.send(
                    &ClientMessage::Move {
                        position,
                        yaw,
                        pitch,
                    },
                    Channel::Unreliable,
                );
                if send_stats {
                    client.send(
                        &ClientMessage::Stats {
                            health,
                            hunger,
                            saturation,
                        },
                        Channel::Reliable,
                    );
                }
                if let Some((slots, selected)) = inventory_sync.take() {
                    client.send(
                        &ClientMessage::SyncInventory { slots, selected },
                        Channel::Reliable,
                    );
                }
                for msg in client.receive() {
                    match msg {
                        ServerMessage::Welcome { .. } => {}
                        ServerMessage::PlayerJoined { id, name } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| RemotePlayer::new(id, name, Vec3::ZERO));
                        }
                        ServerMessage::PlayerLeft { id } => {
                            self.remote_players.remove(&id);
                        }
                        ServerMessage::PlayerState {
                            id,
                            position,
                            yaw,
                            pitch,
                        } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| {
                                    RemotePlayer::new(
                                        id,
                                        format!("Player {}", id.0),
                                        Vec3::from_array(position),
                                    )
                                })
                                .push_snapshot(Vec3::from_array(position), yaw, pitch);
                        }
                        ServerMessage::BlockChanged { pos, block } => {
                            // apply_edit (not set_block) so an edit whose chunk hasn't
                            // streamed in yet is buffered and applied when it loads.
                            self.world.apply_edit(pos, block);
                        }
                        ServerMessage::WorldEdits { edits } => {
                            let count = edits.len();
                            for (pos, block) in edits {
                                self.world.apply_edit(pos, block);
                            }
                            log::debug!("applied {count} world-state edits on join");
                        }
                        ServerMessage::PlayerStats {
                            id,
                            health,
                            hunger,
                            mode,
                        } if id != local_id => {
                            self.remote_players
                                .entry(id)
                                .or_insert_with(|| {
                                    RemotePlayer::new(id, format!("Player {}", id.0), Vec3::ZERO)
                                })
                                .set_stats(health, hunger, mode);
                        }
                        _ => {}
                    }
                }
                let _ = client.flush();
            }
        }

        if requested_world_state_now {
            self.world_state_requested = true;
        }
    }

    /// Propagate a local block edit to the network (host broadcasts, client requests).
    fn broadcast_local_edit(&mut self, pos: BlockPos, block: BlockId) {
        match &mut self.net {
            NetRole::Singleplayer => {}
            NetRole::Host(host) => {
                host.broadcast(
                    &ServerMessage::BlockChanged { pos, block },
                    Channel::Reliable,
                );
            }
            NetRole::Client { client, .. } => {
                let msg = if block.is_air() {
                    ClientMessage::Break { pos }
                } else {
                    ClientMessage::Place { pos, block }
                };
                client.send(&msg, Channel::Reliable);
            }
        }
    }

    /// Tell the host the local player's game mode changed (no-op for host /
    /// singleplayer — the host advertises its mode via `PlayerStats`/`Welcome`).
    fn broadcast_mode_change(&mut self) {
        let mode = self.player.mode;
        if let NetRole::Client { client, .. } = &mut self.net {
            client.send(&ClientMessage::SetMode(mode), Channel::Reliable);
        }
    }

    /// Rebuild GPU meshes for remote players, advancing each one's animation from the
    /// movement observed since the previous frame.
    fn update_remote_meshes(&mut self, ctx: &Arc<RenderContext>, dt: f32) {
        self.remote_meshes.clear();
        // Snapshot the render-relevant fields first so we can mutate `remote_anims`
        // and read `player_model` without holding a borrow on `remote_players`.
        let snapshots: Vec<(PlayerId, Vec3, f32, f32)> = self
            .remote_players
            .values()
            .map(|rp| (rp.id, rp.position(), rp.yaw, rp.pitch))
            .collect();
        for (id, pos, yaw, pitch) in snapshots {
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

            let mesh = self.player_model.build_mesh(pos, yaw, &pose);
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.remote_meshes.push(gpu);
            }
        }
        // Drop animation state for players that have left.
        self.remote_anims
            .retain(|id, _| self.remote_players.contains_key(id));
    }

    fn net_status(&self) -> String {
        match &self.net {
            NetRole::Singleplayer => "singleplayer".to_string(),
            NetRole::Host(host) => format!("host ({} players)", host.player_count()),
            NetRole::Client { .. } => format!("client ({} remote)", self.remote_players.len()),
        }
    }

    /// The block the player is currently looking at within reach, if any.
    fn targeted_block(&self) -> Option<crate::world::RaycastHit> {
        crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            REACH,
            |p| self.world.is_solid(p),
        )
    }

    /// Remove the block at `pos`. In survival the block pops out as a dropped
    /// item; in creative it just disappears. Broadcasts the edit. Returns `true`
    /// on a hit.
    fn break_block_at(&mut self, pos: BlockPos) -> bool {
        let Some(prev) = self.world.set_block(pos, BlockId::AIR) else {
            return false;
        };
        if prev.is_air() {
            return false;
        }
        self.fluids.block_changed(pos);
        if self.player.mode.consumes_blocks()
            && let Some(item) = self.items.item_for_block(prev)
        {
            // Scatter direction varies with the animation clock — cheap pseudo-random.
            let angle = self.elapsed * 9.73;
            self.drops
                .push(DroppedItem::block_drop(ItemStack::single(item), pos, angle));
        }
        self.broadcast_local_edit(pos, BlockId::AIR);
        true
    }

    /// Toss one item from the selected hotbar slot out in front of the player.
    fn drop_selected_item(&mut self) {
        let Some(stack) = self.inventory.take_one_selected() else {
            return;
        };
        self.drops.push(DroppedItem::thrown(
            stack,
            self.player.eye_position(),
            self.player.look_direction(),
        ));
    }

    /// Advance drop physics, collect drops the player walks over, cull expired ones.
    fn update_drops(&mut self, dt: f32) {
        for item in &mut self.drops {
            item.update(dt, |p| self.world.is_solid_for_collision(p));
        }
        let reach = self.player.aabb().expand(Vec3::splat(PICKUP_RANGE));
        let dead = self.dead;
        self.drops.retain_mut(|item| {
            if item.expired() {
                return false;
            }
            if dead || !item.can_pickup() || !reach.intersects(item.aabb()) {
                return true;
            }
            let leftover = self.inventory.add(item.stack, &self.items);
            if leftover == 0 {
                false
            } else {
                // Inventory full: whatever didn't fit stays on the ground.
                item.stack.count = leftover;
                true
            }
        });
    }

    /// Atlas tiles for a dropped item's cube: the block's own faces for block
    /// items; simple stand-in tiles for tools and food (no dedicated item art yet).
    fn drop_textures(&self, item: ItemId) -> FaceTextures {
        let def = self.items.get(item);
        match def.place_block {
            Some(block) => self.blocks.get(block).textures,
            None if def.tool.is_some() => FaceTextures::uniform(tiles::WOOD_BARK),
            None => FaceTextures::uniform(tiles::LEAVES),
        }
    }

    /// Rebuild the combined drop meshes (opaque + transparent passes). Drops are
    /// few and tiny, so a per-frame rebuild stays cheap, like remote players.
    fn update_drops_mesh(&mut self, ctx: &Arc<RenderContext>) {
        let mut opaque = CpuMesh::new();
        let mut transparent = CpuMesh::new();
        for item in &self.drops {
            let textures = self.drop_textures(item.stack.item);
            let is_transparent = self
                .items
                .get(item.stack.item)
                .place_block
                .is_some_and(|b| self.blocks.get(b).is_transparent());
            let target = if is_transparent {
                &mut transparent
            } else {
                &mut opaque
            };
            push_item_cube(
                target,
                item.render_center(),
                DROP_SIZE,
                item.spin_yaw(),
                &textures,
            );
        }
        self.drops_mesh = GpuMesh::upload(&ctx.memory_allocator, &opaque)
            .ok()
            .flatten();
        self.drops_mesh_transparent = GpuMesh::upload(&ctx.memory_allocator, &transparent)
            .ok()
            .flatten();
    }

    /// Survival timed mining: accumulate break progress on the targeted block
    /// while the dig button is held, breaking it once progress reaches 1.0.
    fn update_mining(&mut self, digging: bool, dt: f32) {
        if !digging {
            self.breaking = None;
            return;
        }
        let Some(hit) = self.targeted_block() else {
            self.breaking = None;
            return;
        };
        let block = self.blocks.get(self.world.block_at(hit.block));
        if !block.is_breakable() {
            self.breaking = None;
            return;
        }
        // Effective tool: the held item, if it's a tool.
        let tool = self
            .inventory
            .item_in_selected()
            .and_then(|id| self.items.tool(id).map(|k| (k, self.items.dig_speed(id))));
        let seconds = crate::inventory::break_seconds(block.hardness, block.material, tool);

        // Reset progress when the targeted block changes.
        let prior = match &self.breaking {
            Some(b) if b.block == hit.block => b.progress,
            _ => 0.0,
        };
        let progress = prior + dt / seconds.max(1.0e-3);
        if progress >= 1.0 {
            self.player_anim.trigger_swing();
            if self.break_block_at(hit.block) {
                self.inventory.damage_selected_tool();
            }
            self.breaking = None;
        } else {
            self.breaking = Some(BreakState {
                block: hit.block,
                progress,
            });
        }
    }

    /// (Re)build the crack overlay for the block being mined; drop it when idle.
    /// Cheap enough to rebuild every frame (six quads).
    fn update_break_overlay(&mut self, ctx: &Arc<RenderContext>) {
        self.break_mesh = self.breaking.as_ref().and_then(|b| {
            let overlay = mesh_block_overlay(b.block, tiles::crack_tile(b.progress));
            match GpuMesh::upload(&ctx.memory_allocator, &overlay) {
                Ok(mesh) => mesh,
                Err(err) => {
                    log::error!("break overlay upload failed at {:?}: {err:?}", b.block);
                    None
                }
            }
        });
    }

    /// (Re)build the selection outline on the targeted block. The geometry only
    /// depends on the block position, so it's cached until the target changes.
    fn update_target_outline(&mut self, ctx: &Arc<RenderContext>) {
        let target = if self.dead {
            None
        } else {
            self.targeted_block().map(|hit| hit.block)
        };
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

    /// Right-click: eat the held food when hungry, otherwise place its block.
    fn use_selected(&mut self) {
        let Some(item_id) = self.inventory.item_in_selected() else {
            return;
        };
        if let Some(food) = self.items.food(item_id)
            && self.player.mode.takes_damage()
            && self.player.is_hungry()
        {
            self.player.feed(food.hunger, food.saturation);
            self.inventory.consume_selected(1);
            self.player_anim.trigger_swing();
            return;
        }
        self.place_block(item_id);
    }

    /// Place the selected item's block against the targeted face. Consumes from
    /// the inventory only in survival (creative has infinite blocks).
    fn place_block(&mut self, item_id: ItemId) {
        let Some(block) = self.items.get(item_id).place_block else {
            return;
        };
        let Some(hit) = self.targeted_block() else {
            return;
        };
        let target = hit.place_position();
        // Don't place inside the player.
        if Aabb::block(Vec3::new(target.x as f32, target.y as f32, target.z as f32))
            .intersects(self.player.aabb())
        {
            return;
        }
        if self.world.set_block(target, block).is_some() {
            self.fluids.block_changed(target);
            if self.player.mode.consumes_blocks() {
                self.inventory.consume_selected(1);
            }
            self.broadcast_local_edit(target, block);
            self.player_anim.trigger_swing();
        }
    }

    /// Apply the saved state the host handed back in its `Welcome` (this client
    /// played this world before). Replaces the starter kit wholesale.
    fn apply_restore(&mut self, restore: &PlayerRestore) {
        self.player.position = Vec3::from_array(restore.position);
        self.player.yaw = restore.yaw;
        self.player.pitch = restore.pitch;
        self.player.health = restore.health;
        self.player.hunger = restore.hunger;
        self.player.saturation = restore.saturation;
        for index in 0..INVENTORY_SIZE {
            let stack = restore.slots.get(index).and_then(|slot| {
                slot.and_then(|s| {
                    ((s.item as usize) < self.items.len()).then_some(ItemStack {
                        item: ItemId(s.item),
                        count: s.count,
                        durability: s.durability,
                    })
                })
            });
            self.inventory.set_slot(index, stack);
        }
        self.inventory.set_selected(restore.selected as usize);
        // Don't immediately echo the restored inventory back to the host.
        self.last_synced_inventory = Some(self.inventory.clone());
        log::info!("restored player state from host at {:?}", restore.position);
    }

    /// Persist the world if this session owns one (singleplayer or host of a
    /// named world). Clients and ephemeral worlds are no-ops by construction.
    fn save_world(&mut self) {
        if self.save.is_none() {
            return;
        }
        debug_assert!(
            !matches!(self.net, NetRole::Client { .. }),
            "clients never hold a save handle"
        );
        // Fold currently connected players into the persistent records first.
        let connected: Vec<PlayerId> = self.remote_players.keys().copied().collect();
        for pid in connected {
            record_remote(
                &mut self.player_records,
                &self.remote_identities,
                &self.remote_players,
                &self.remote_inventories,
                &self.items,
                pid,
            );
        }
        let world = WorldData::from_world(&self.world);
        let player = PlayerData::capture(&self.player, &self.inventory, &self.items);
        let save = self.save.as_mut().expect("checked above");
        save.meta.game_mode = self.player.mode;
        save.meta.spawn = self.spawn.to_array();
        save.meta.time_of_day = self.day_cycle.time_of_day();
        match save.write(&world, &player, &self.player_records) {
            Ok(()) => log::info!(
                "saved world '{}' ({} edits, {} player records)",
                save.meta.name,
                world.edits.len(),
                self.player_records.0.len()
            ),
            Err(err) => log::error!("failed to save world '{}': {err}", save.meta.name),
        }
    }
}

/// Snapshot one connected player into the host's persistent per-identity
/// records. A free function over the individual fields so it can be called from
/// inside `pump_network`'s borrow of `self.net`.
fn record_remote(
    records: &mut PlayerRecords,
    identities: &HashMap<PlayerId, u64>,
    remote_players: &HashMap<PlayerId, RemotePlayer>,
    remote_inventories: &HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    items: &ItemRegistry,
    pid: PlayerId,
) {
    let Some(&identity) = identities.get(&pid) else {
        return;
    };
    let Some(rp) = remote_players.get(&pid) else {
        return;
    };
    // A client that never reported an inventory keeps its previous record's.
    let (slots, selected) = match remote_inventories.get(&pid) {
        Some((slots, selected)) => (wire_slots_to_names(slots, items), *selected),
        None => match records.0.get(&identity) {
            Some(prev) => (prev.slots.clone(), prev.selected_slot),
            // Never reported an inventory and no history: don't record at all,
            // so a rejoin starts fresh (starter kit) instead of empty-handed.
            None => return,
        },
    };
    records.0.insert(
        identity,
        PlayerData {
            position: rp.position().to_array(),
            yaw: rp.yaw,
            pitch: rp.pitch,
            flying: false,
            health: rp.health,
            hunger: rp.hunger,
            saturation: rp.saturation,
            selected_slot: selected,
            slots,
        },
    );
}

impl GameState for InGameState {
    fn name(&self) -> &'static str {
        "InGame"
    }

    fn on_exit(&mut self, _ctx: &mut StateContext) {
        // Fires when pausing (Push), quitting to the menu (ReplaceAll), and on
        // app shutdown (Quit / window close) — every path that leaves the world.
        self.save_world();
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        let kb = ctx.settings.controls.keybinds.clone();

        if !self.dead && ctx.input.just_pressed(kb.inventory) {
            self.toggle_inventory();
        }
        // Esc closes the inventory if open, otherwise opens the pause overlay.
        if ctx.input.just_pressed(kb.pause) {
            if self.inventory_open {
                self.toggle_inventory();
            } else {
                return Transition::Push(Box::new(PauseMenuState::new()));
            }
        }

        if self.inventory_open || self.dead {
            // Inventory screen / death screen: free cursor, freeze player control,
            // and abandon any in-progress mining.
            ctx.grab_cursor = false;
            self.breaking = None;
        } else {
            ctx.grab_cursor = true;

            if ctx.input.just_pressed(kb.toggle_perspective) {
                self.player.toggle_perspective();
            }
            if ctx.input.just_pressed(kb.toggle_debug) {
                self.show_debug = !self.show_debug;
            }

            // Live game-mode toggle (F4).
            if ctx.input.just_pressed(kb.toggle_gamemode) {
                self.player.set_mode(self.player.mode.toggled());
                self.breaking = None;
                self.broadcast_mode_change();
            }

            // Creative flight: double-tap the jump key within the window.
            self.jump_tap_timer += ctx.dt;
            if ctx.input.just_pressed(kb.jump) {
                if self.player.mode.can_fly() && self.jump_tap_timer < DOUBLE_TAP_WINDOW {
                    self.player.flying = !self.player.flying;
                }
                self.jump_tap_timer = 0.0;
            }

            // Mouse look.
            let sens = ctx.settings.controls.mouse_sensitivity * 0.0025;
            let pitch_sign = if ctx.settings.controls.invert_y {
                1.0
            } else {
                -1.0
            };
            let delta = ctx.input.mouse_delta();
            self.player
                .rotate(delta.x * sens, pitch_sign * delta.y * sens);

            // Hotbar selection via scroll.
            let scroll = ctx.input.scroll_delta();
            if scroll != 0.0 {
                self.inventory.scroll_selected(-scroll.signum() as i32);
            }

            // Toss one item from the selected slot onto the ground.
            if ctx.input.just_pressed(kb.drop_item) {
                self.drop_selected_item();
            }

            // Movement + physics.
            let movement = ctx.input.movement(&kb);
            let dt = ctx.dt.min(0.05);
            self.player
                .update(movement, dt, |p| self.world.is_solid_for_collision(p));

            // Survival vitals: hunger drain, regen, starvation.
            if self.player.mode.takes_damage() {
                self.player.tick_survival(dt, movement.sprint);
                if self.player.is_dead() {
                    self.dead = true;
                    self.breaking = None;
                }
            }

            // Block interaction. The main-hand swing fires on every left click,
            // even when punching air (no block hit).
            if ctx.input.mouse_just_pressed(MouseButton::Left) {
                self.player_anim.trigger_swing();
            }
            if self.player.mode.instant_break() {
                // Creative: instant break on click.
                self.breaking = None;
                if ctx.input.mouse_just_pressed(MouseButton::Left)
                    && let Some(hit) = self.targeted_block()
                {
                    self.break_block_at(hit.block);
                }
            } else {
                // Survival: progressive mining while the dig button is held.
                let digging = ctx.input.mouse_held(MouseButton::Left);
                self.update_mining(digging, dt);
            }
            if ctx.input.mouse_just_pressed(MouseButton::Right) {
                self.use_selected();
            }
        }

        self.fov_degrees = ctx.settings.render.fov_degrees;
        // Wrap the animation clock so f32 precision never degrades over long
        // sessions. The period must stay a whole multiple of the water loop
        // (WATER_FRAMES / WATER_FPS = 0.8 s in voxel.frag) to wrap seamlessly.
        self.elapsed = (self.elapsed + ctx.dt) % 3600.0;
        self.day_cycle.advance(ctx.dt);
        // Periodic autosave for persistent worlds (also fires on pause/exit).
        if self.save.is_some() {
            self.autosave_timer += ctx.dt;
            if self.autosave_timer >= AUTOSAVE_INTERVAL {
                self.autosave_timer = 0.0;
                self.save_world();
            }
        }
        self.update_break_overlay(ctx.render);
        self.update_target_outline(ctx.render);
        self.pump_network(ctx.dt);
        // Water flow: singleplayer/host simulate authoritatively and broadcast
        // each change; clients receive them as ordinary BlockChanged edits.
        if !matches!(self.net, NetRole::Client { .. }) {
            for (pos, block) in self.fluids.tick(&mut self.world, ctx.dt) {
                self.broadcast_local_edit(pos, block);
            }
        }
        self.update_streaming(ctx.settings.render.render_distance);
        self.enqueue_dirty();
        self.process_mesh_budget(ctx.render);
        // Drops keep simulating even with the inventory or death screen open.
        self.update_drops(ctx.dt.min(0.05));
        self.update_drops_mesh(ctx.render);

        // Advance + rebuild animated player models. The local player settles to idle
        // while the inventory is open (movement is frozen).
        let anim_dt = ctx.dt.min(0.05);
        let local_speed = if self.inventory_open {
            0.0
        } else {
            let v = self.player.velocity;
            Vec3::new(v.x, 0.0, v.z).length()
        };
        self.player_anim.advance(local_speed, anim_dt);
        self.update_player_mesh(ctx.render);
        self.update_remote_meshes(ctx.render, anim_dt);
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        use crate::ui::inventory::InvAction;

        // Death screen takes over everything else.
        if self.dead {
            let mut respawn = false;
            egui::Area::new(egui::Id::new("death_screen"))
                .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
                .show(egui_ctx, |ui| {
                    ui.vertical_centered(|ui| {
                        ui.label(
                            egui::RichText::new("You died")
                                .size(40.0)
                                .color(egui::Color32::from_rgb(220, 40, 40)),
                        );
                        ui.add_space(12.0);
                        if ui
                            .add_sized([180.0, 40.0], egui::Button::new("Respawn"))
                            .clicked()
                        {
                            respawn = true;
                        }
                    });
                });
            if respawn {
                self.respawn();
            }
            return Transition::None;
        }

        if self.inventory_open {
            if let Some(action) = crate::ui::inventory::draw_inventory(
                egui_ctx,
                &self.inventory,
                &self.items,
                &self.recipes,
                self.held,
                self.player.mode,
            ) {
                match action {
                    InvAction::Slot(index) => self.handle_slot_click(index),
                    InvAction::Pick(id) => self.held = Some(self.items.full_stack(id)),
                    InvAction::Craft(index) => self.handle_craft(index),
                }
            }
            return Transition::None;
        }

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(egui_ctx, &self.inventory, &self.items);
        hud::draw_mode_indicator(egui_ctx, self.player.mode.label());

        // Survival HUD: vitals and break progress.
        if self.player.mode.takes_damage() {
            hud::draw_vitals(
                egui_ctx,
                self.player.health,
                crate::entity::player::MAX_HEALTH,
                self.player.hunger,
                crate::entity::player::MAX_HUNGER,
            );
        }
        if let Some(breaking) = &self.breaking {
            hud::draw_break_progress(egui_ctx, breaking.progress);
        }

        if self.show_debug {
            let fps = if ctx.dt > 0.0 { 1.0 / ctx.dt } else { 0.0 };
            let p = self.player.position;
            let facing = self.player.look_direction();
            let lines = vec![
                format!("Wyvencraft — {fps:.0} fps"),
                format!("xyz: {:.2} {:.2} {:.2}", p.x, p.y, p.z),
                format!("facing: {:.2} {:.2} {:.2}", facing.x, facing.y, facing.z),
                format!(
                    "chunks: {} loaded / {} meshes / {} queued / {} pending",
                    self.world.loaded_count(),
                    self.meshes.len(),
                    self.mesh_queue.len(),
                    self.loader.pending_count()
                ),
                format!("on_ground: {}", self.player.on_ground),
                format!("net: {}", self.net_status()),
                format!("time: {}", format_time_of_day(self.day_cycle.time_of_day())),
                format!(
                    "world: {}",
                    self.save
                        .as_ref()
                        .map(|s| s.meta.name.as_str())
                        .unwrap_or("(unsaved)")
                ),
            ];
            hud::draw_debug(egui_ctx, &lines);
        }

        Transition::None
    }

    fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        let mut camera = Camera::new(self.fov_degrees, aspect);
        let eye = self.player.eye_position();
        let look = self.player.look_direction();
        match self.player.perspective {
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

        // The local player model (third person only) + remote players.
        if let Some(mesh) = &self.player_mesh {
            opaque.push(mesh);
        }
        for mesh in &self.remote_meshes {
            opaque.push(mesh);
        }

        // Dropped items, split by pass like the blocks they represent.
        if let Some(mesh) = &self.drops_mesh {
            opaque.push(mesh);
        }
        if let Some(mesh) = &self.drops_mesh_transparent {
            transparent.push(mesh);
        }

        let atmo = self.day_cycle.atmosphere();
        let sky = SkyParams {
            inv_view_proj: camera.sky_inv_view_proj(),
            sun_dir: atmo.sun_dir,
            zenith_color: atmo.zenith_color,
            horizon_color: atmo.horizon_color,
            sun_color: atmo.sun_color,
            star_intensity: atmo.star_intensity,
            moon_intensity: atmo.moon_intensity,
        };
        let light = LightParams {
            light_dir: atmo.light_dir,
            light_color: atmo.light_color,
            ambient: atmo.ambient,
        };

        Some(SceneFrame {
            view_proj: camera.view_projection(),
            sky,
            light,
            time: self.elapsed,
            opaque,
            transparent,
            lines: self.outline_mesh.as_ref(),
        })
    }
}

/// Serialize the recipe book back to item names for the `Welcome` message.
fn recipes_to_wire(book: &RecipeBook, items: &ItemRegistry) -> Vec<RecipeData> {
    book.recipes()
        .iter()
        .map(|recipe| RecipeData {
            output: items.get(recipe.output).name.clone(),
            count: recipe.count as u32,
            ingredients: recipe
                .ingredients
                .iter()
                .map(|&(item, n)| (items.get(item).name.clone(), n))
                .collect(),
        })
        .collect()
}

/// Convert the local inventory to its wire form for `SyncInventory`.
fn inventory_to_wire(inventory: &Inventory) -> (Vec<Option<NetItemStack>>, u32) {
    let slots = inventory
        .slots()
        .iter()
        .map(|slot| {
            slot.map(|stack| NetItemStack {
                item: stack.item.0,
                count: stack.count,
                durability: stack.durability,
            })
        })
        .collect();
    (slots, inventory.selected_index() as u32)
}

/// Convert wire inventory slots to the name-based on-disk form. Ids out of this
/// build's registry range (mismatched peer) become empty slots.
fn wire_slots_to_names(
    slots: &[Option<NetItemStack>],
    items: &ItemRegistry,
) -> Vec<Option<ItemStackData>> {
    slots
        .iter()
        .map(|slot| {
            slot.and_then(|s| {
                ((s.item as usize) < items.len()).then(|| ItemStackData {
                    name: items.get(ItemId(s.item)).name.clone(),
                    count: s.count,
                    durability: s.durability,
                })
            })
        })
        .collect()
}

/// Convert a saved record back to wire form for a returning client's `Welcome`.
/// Item names this build no longer knows are dropped.
fn record_to_restore(record: &PlayerData, items: &ItemRegistry) -> PlayerRestore {
    PlayerRestore {
        position: record.position,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        hunger: record.hunger,
        saturation: record.saturation,
        slots: record
            .slots
            .iter()
            .map(|slot| {
                slot.as_ref().and_then(|s| {
                    items.find(&s.name).map(|id| NetItemStack {
                        item: id.0,
                        count: s.count,
                        durability: s.durability,
                    })
                })
            })
            .collect(),
        selected: record.selected_slot,
    }
}

/// Rebuild a recipe book from a host's wire data. Recipes naming items this
/// build doesn't know are skipped with a warning (mismatched versions).
fn recipes_from_wire(data: &[RecipeData], items: &ItemRegistry) -> RecipeBook {
    let resolved = data
        .iter()
        .filter_map(|r| {
            crate::inventory::crafting::resolve_named(&r.output, r.count, &r.ingredients, items)
        })
        .collect();
    RecipeBook::from_recipes(resolved)
}

/// Format a normalized time-of-day `[0,1)` (0.0 = midnight) as a 24-hour clock.
fn format_time_of_day(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
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

#[cfg(test)]
mod tests {
    use super::*;

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
        let mut state = InGameState::new_saved(game);

        // "Play": place a block above ground, move, and rearrange the inventory.
        let edit_pos = BlockPos::new(3, 200, 5);
        assert!(state.world.set_block(edit_pos, blocks::STONE).is_some());
        state.player.position = Vec3::new(10.0, 90.0, -4.0);
        state.player.health = 13.5;
        state
            .inventory
            .set_slot(8, Some(ItemStack::new(state.items.bread, 2)));
        state.inventory.set_selected(8);
        state.save_world();
        drop(state);

        let game = WorldSave::open(&root, "roundtrip").unwrap().load().unwrap();
        let state = InGameState::new_saved(game);
        assert_eq!(
            state.world.block_at(edit_pos),
            blocks::STONE,
            "terrain edit persists"
        );
        assert_eq!(state.player.position, Vec3::new(10.0, 90.0, -4.0));
        assert_eq!(state.player.health, 13.5);
        assert_eq!(
            state.inventory.slot(8),
            Some(ItemStack::new(state.items.bread, 2))
        );
        assert_eq!(state.inventory.selected_index(), 8);

        let _ = std::fs::remove_dir_all(&root);
    }
}

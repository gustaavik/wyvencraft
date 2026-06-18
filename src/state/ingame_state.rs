//! The playing state: owns the world, the local player, the inventory, and the
//! background chunk loader, and ties together input → simulation → rendering data.

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::Arc;
use std::time::Duration;

use glam::Vec3;
use winit::event::MouseButton;

use super::{GameState, PauseMenuState, StateContext, Transition};
use crate::core::{Aabb, BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, DayCycle};
use crate::entity::{AnimationState, HumanoidModel, Perspective, Player};
use crate::inventory::{Inventory, ItemRegistry, ItemStack};
use crate::net::{
    Channel, Client, ClientMessage, Host, NetVec3, PlayerId, RemotePlayer, ServerMessage,
};
use crate::render::{Camera, GpuMesh, LightParams, RenderContext, SceneFrame, SkyParams};
use crate::ui::hud;
use crate::world::block::blocks;
use crate::world::meshing::mesh_chunk;
use crate::world::{BlockRegistry, ChunkLoader, NoiseGenerator, World, WorldGenerator};

/// The networking role of this in-game session.
enum NetRole {
    Singleplayer,
    Host(Host),
    Client { client: Client, local_id: PlayerId },
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

pub struct InGameState {
    pub world: World,
    pub player: Player,
    pub blocks: Arc<BlockRegistry>,
    pub items: ItemRegistry,
    pub inventory: Inventory,
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
    /// Networking role + remote players.
    net: NetRole,
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
    pub fn new(seed: u64) -> Self {
        Self::build(seed, NetRole::Singleplayer, None, DayCycle::default())
    }

    /// Host a multiplayer session (the host also plays locally).
    pub fn new_host(seed: u64, host: Host) -> Self {
        Self::build(seed, NetRole::Host(host), None, DayCycle::default())
    }

    /// Join a multiplayer session as a client (world built from the host's seed).
    /// `spawn` is the position the host assigned us in its `Welcome`; `time_of_day`
    /// seeds our day/night clock to the host's so skies match on join.
    pub fn new_client(
        seed: u64,
        client: Client,
        local_id: PlayerId,
        spawn: NetVec3,
        time_of_day: f32,
    ) -> Self {
        Self::build(
            seed,
            NetRole::Client { client, local_id },
            Some(Vec3::from_array(spawn)),
            DayCycle::new(time_of_day),
        )
    }

    /// Build the in-game state. `spawn_override` (clients) places the player at the
    /// host-provided position and anchors synchronous generation there; otherwise the
    /// spawn is found over the origin column.
    fn build(seed: u64, net: NetRole, spawn_override: Option<Vec3>, day_cycle: DayCycle) -> Self {
        let blocks = Arc::new(BlockRegistry::with_builtins());
        let items = ItemRegistry::from_blocks(&blocks);

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

        // Creative-style starter hotbar so placing blocks works immediately.
        let mut inventory = Inventory::new();
        let starter = [
            blocks::STONE,
            blocks::DIRT,
            blocks::GRASS,
            blocks::SAND,
            blocks::WOOD,
            blocks::LEAVES,
            blocks::GLASS,
            blocks::SNOW,
            blocks::BEDROCK,
        ];
        for (slot, block) in starter.iter().enumerate() {
            if let Some(item) = items.item_for_block(*block) {
                inventory.set_slot(slot, Some(ItemStack::new(item, 64)));
            }
        }

        Self {
            world,
            player: Player::new(spawn),
            blocks,
            items,
            inventory,
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
            net,
            remote_players: HashMap::new(),
            remote_meshes: Vec::new(),
            remote_anims: HashMap::new(),
        }
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

        match &mut self.net {
            NetRole::Singleplayer => {}
            NetRole::Host(host) => {
                host.pump(duration);
                let seed = self.world.seed();
                let time_of_day = self.day_cycle.time_of_day();

                for cid in host.take_joined() {
                    if let Some(pid) = host.player_id(cid) {
                        let name = format!("Player {}", pid.0);
                        host.send(
                            cid,
                            &ServerMessage::Welcome {
                                seed,
                                your_id: pid,
                                spawn: position,
                                time_of_day,
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
                        self.remote_players.insert(
                            pid,
                            RemotePlayer::new(pid, name, Vec3::from_array(position)),
                        );
                    }
                }
                for pid in host.take_left() {
                    self.remote_players.remove(&pid);
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
                                host.broadcast(
                                    &ServerMessage::BlockChanged { pos, block },
                                    Channel::Reliable,
                                );
                            }
                        }
                        ClientMessage::Chat(_) => {}
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
                host.flush();
            }
            NetRole::Client { client, local_id } => {
                let local_id = *local_id;
                if let Err(err) = client.pump(duration) {
                    log::warn!("client pump error: {err}");
                }
                client.send(
                    &ClientMessage::Move {
                        position,
                        yaw,
                        pitch,
                    },
                    Channel::Unreliable,
                );
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
                            self.world.set_block(pos, block);
                        }
                        _ => {}
                    }
                }
                let _ = client.flush();
            }
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

    /// Break the block the player is looking at; drop it into the inventory.
    fn try_break_block(&mut self) {
        let hit = crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            REACH,
            |p| self.world.is_solid(p),
        );
        if let Some(hit) = hit
            && let Some(prev) = self.world.set_block(hit.block, BlockId::AIR)
            && !prev.is_air()
        {
            if let Some(item) = self.items.item_for_block(prev) {
                self.inventory.add(ItemStack::single(item), &self.items);
            }
            self.broadcast_local_edit(hit.block, BlockId::AIR);
        }
    }

    /// Place the currently selected block against the targeted face.
    fn try_place_block(&mut self) {
        let hit = crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            REACH,
            |p| self.world.is_solid(p),
        );
        let Some(hit) = hit else { return };
        let Some(item_id) = self.inventory.item_in_selected() else {
            return;
        };
        let Some(block) = self.items.get(item_id).place_block else {
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
            self.inventory.consume_selected(1);
            self.broadcast_local_edit(target, block);
            self.player_anim.trigger_swing();
        }
    }
}

impl GameState for InGameState {
    fn name(&self) -> &'static str {
        "InGame"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        let kb = ctx.settings.controls.keybinds.clone();

        if ctx.input.just_pressed(kb.inventory) {
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

        if self.inventory_open {
            // Inventory screen: free cursor, freeze player control.
            ctx.grab_cursor = false;
        } else {
            ctx.grab_cursor = true;

            if ctx.input.just_pressed(kb.toggle_perspective) {
                self.player.toggle_perspective();
            }
            if ctx.input.just_pressed(kb.toggle_debug) {
                self.show_debug = !self.show_debug;
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

            // Movement + physics.
            let movement = ctx.input.movement(&kb);
            let dt = ctx.dt.min(0.05);
            self.player
                .update(movement, dt, |p| self.world.is_solid_for_collision(p));

            // Block interaction. The main-hand swing fires on every left click,
            // even when punching air (no block hit).
            if ctx.input.mouse_just_pressed(MouseButton::Left) {
                self.player_anim.trigger_swing();
                self.try_break_block();
            }
            if ctx.input.mouse_just_pressed(MouseButton::Right) {
                self.try_place_block();
            }
        }

        self.fov_degrees = ctx.settings.render.fov_degrees;
        self.day_cycle.advance(ctx.dt);
        self.pump_network(ctx.dt);
        self.update_streaming(ctx.settings.render.render_distance);
        self.enqueue_dirty();
        self.process_mesh_budget(ctx.render);

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
        if self.inventory_open {
            if let Some(index) = crate::ui::inventory::draw_inventory(
                egui_ctx,
                &self.inventory,
                &self.items,
                self.held,
            ) {
                self.handle_slot_click(index);
            }
            return Transition::None;
        }

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(egui_ctx, &self.inventory, &self.items);

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
        let transparent: Vec<&GpuMesh> = self
            .transparent_meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();

        // The local player model (third person only) + remote players.
        if let Some(mesh) = &self.player_mesh {
            opaque.push(mesh);
        }
        for mesh in &self.remote_meshes {
            opaque.push(mesh);
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
            opaque,
            transparent,
        })
    }
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

//! The game world as one value: everything a session simulates, apart from
//! how it is drawn and how it reaches other peers.
//!
//! [`Simulation`] is what used to be most of the in-game screen's fields. It
//! holds the world and its entities, the local player and what they carry,
//! and the clocks and planners that move them. It names no renderer, no egui
//! and no transport, which is what lets the use cases on it be tested with
//! nothing but a seed.

mod drops;
mod mobs;
mod player;

use std::sync::Arc;

use glam::Vec3;

use crate::application::ecs::Ecs;
pub use mobs::{
    BossBeat, KNOCKBACK_LIFT, KNOCKBACK_PUSH, MobTarget, MobTick, PLAYER_ATTACK_DAMAGE, ground_at,
};

use crate::domain::content::Registries;
use crate::domain::core::{BlockPos, CHUNK_HEIGHT, ChunkPos, DayCycle, GameMode};
use crate::domain::entity::{AnimationState, MobId, Player, Spawner};
use crate::domain::inventory::{Inventory, ItemStack, RecipeBook};
use crate::domain::progression::WorldProgression;
use crate::domain::world::structure::Structures;
use crate::domain::world::{ChunkLoader, FluidSim, NoiseGenerator, World, WorldGenerator};

/// Progressive break state for survival timed mining.
#[derive(Debug, Clone, Copy)]
pub struct BreakState {
    pub block: BlockPos,
    /// Accumulated progress in `[0, 1)`; the block breaks at `>= 1.0`.
    pub progress: f32,
}

/// The one boss fight a world may have going at a time.
#[derive(Debug, Clone, Copy)]
pub struct BossFight {
    pub mob: MobId,
    /// The altar it was summoned at — the centre of its arena.
    pub altar: BlockPos,
    /// Seconds the arena has stood empty.
    pub empty_for: f32,
}

/// An attack being wound up, as every peer shows it.
#[derive(Debug, Clone)]
pub struct Telegraph {
    pub mob: u64,
    /// The attack's id, title-cased for display.
    pub name: String,
    pub remaining: f32,
}

/// Who and what spawns: the id counter, the spawn planner and the boss fight.
pub struct MobDirector {
    pub next_id: u64,
    /// Seeded spawn planner — deterministic in (seed, tick), so a host and its
    /// clients agree without exchanging the decision.
    pub spawner: Spawner,
    /// The boss fight in progress, if one is (authority only).
    pub fight: Option<BossFight>,
    /// The boss attack being wound up, shown on the boss bar.
    pub telegraph: Option<Telegraph>,
}

impl MobDirector {
    /// Empty, with a spawn planner seeded from the world.
    pub fn new(seed: u64) -> Self {
        Self {
            next_id: 0,
            spawner: Spawner::new(seed),
            fight: None,
            telegraph: None,
        }
    }
}

/// Chunks generated synchronously at startup so the player has ground to stand on.
const SPAWN_RADIUS: i32 = 1;

/// How a session starts, beyond its rules and seed.
pub struct SimulationStart {
    /// Where to put the player. `None` finds a safe spot at the origin; a
    /// client is told where by the host.
    pub spawn: Option<Vec3>,
    pub day_cycle: DayCycle,
    pub mode: GameMode,
    /// The recipe book to craft from — the host's, on a client.
    pub recipes: RecipeBook,
    /// Whether this peer decides (singleplayer, host) or mirrors (client).
    pub authoritative: bool,
}

/// The period [`Simulation::clock`] wraps at.
pub const CLOCK_PERIOD: f32 = 3600.0;

/// Everything the session simulates.
pub struct Simulation {
    /// The rules everything here obeys — the hashed half of the content.
    pub rules: Registries,
    /// Whether this peer decides: simulates mobs, fluids and damage. False on
    /// a client, which renders what the host decided. Fixed for a session.
    pub authoritative: bool,
    /// Seconds of play, wrapped hourly so f32 precision never degrades over a
    /// long session. The animated textures' clock, and the cheap source of
    /// variety a drop's scatter angle draws on.
    ///
    /// The period must stay a whole multiple of every animated texture's
    /// loop or the wrap skips a frame: `voxel_array.frag` steps a layer at
    /// `fps`, so what has to divide 3600 * fps is the frame count (water's
    /// `[block.fluid.texture]` is 64 frames at 8 fps, and the blocks loader
    /// refuses a pairing that does not divide evenly).
    pub clock: f32,
    /// What the simulation decided that other peers must hear about — spawns,
    /// hurts, deaths, arrows, boss beats. Drained by the network pump each
    /// frame: broadcast by a host, discarded where nobody is listening.
    pub outbox: Vec<crate::application::protocol::ServerMessage>,
    pub world: World,
    /// Background terrain generation.
    pub loader: ChunkLoader,
    /// Water flow. Only the authority ticks it; clients receive the edits.
    pub fluids: FluidSim,
    /// This world's shrines and altars, and the terrain sampler they were
    /// placed with — the generator's own, so the game locates exactly what
    /// the chunks contain.
    pub structures: Arc<Structures>,
    /// How far through the biome/boss loop this world is. Authoritative on the
    /// host and in singleplayer; a mirror of the host's on a client.
    pub progression: WorldProgression,
    /// Time-of-day clock driving the sky and world lighting.
    pub day_cycle: DayCycle,
    /// Every entity but the local player: drops, arrows, mobs, replicas and
    /// other players. See [`crate::application::ecs`].
    pub ecs: Ecs,
    pub mobs: MobDirector,
    /// The local player. A singleton rather than an entity — see
    /// [`crate::application::ecs::systems::players`].
    pub player: Player,
    /// The local player's walk/idle/swing animation.
    pub player_anim: AnimationState,
    /// Unspent frame time owed to the fixed-rate player physics step. Keeping
    /// player physics off the variable frame delta is what makes jump height
    /// identical at every framerate.
    pub physics_accum: f32,
    pub inventory: Inventory,
    /// Stack currently "held" by the cursor in the inventory screen.
    pub held: Option<ItemStack>,
    /// Crafting recipes this session crafts from.
    pub recipes: RecipeBook,
    /// Where the player (re)spawns on death.
    pub spawn: Vec3,
    /// True while the player is dead and awaiting respawn (control frozen).
    pub dead: bool,
    /// Progressive block-break state for survival timed mining.
    pub breaking: Option<BreakState>,
    /// The last block the player was told is too hard for their tool, so the
    /// hint is said once per block rather than every frame of digging.
    pub tier_hint: Option<BlockPos>,
}

impl Simulation {
    /// Generate the world from `seed` and put a player in it.
    ///
    /// The chunks around the spawn are generated before this returns, so the
    /// player lands on solid ground; the rest streams in through the loader.
    /// Survival starts with the starter kit `items.toml` declares; creative
    /// starts empty, since its items come from the palette.
    pub fn new(rules: Registries, seed: u64, start: SimulationStart) -> Self {
        let noise = Arc::new(NoiseGenerator::with_config(
            seed,
            rules.worldgen.clone(),
            rules.structures.clone(),
        ));
        let structures = noise.structures().clone();
        let generator: Arc<dyn WorldGenerator> = noise;
        let mut world = World::new(generator.clone(), rules.blocks.clone());

        // Worker pool sized to leave headroom for the main + render threads.
        let workers = std::thread::available_parallelism()
            .map(|n| n.get().saturating_sub(2).max(1))
            .unwrap_or(4);
        let loader = ChunkLoader::new(generator, workers);

        // Anchor on the requested spawn (the host-assigned one for clients) so
        // there is ground under the player.
        let center = start
            .spawn
            .map(|p| BlockPos::from_world(p).chunk())
            .unwrap_or_else(|| ChunkPos::new(0, 0));
        for dx in -SPAWN_RADIUS..=SPAWN_RADIUS {
            for dz in -SPAWN_RADIUS..=SPAWN_RADIUS {
                world.ensure_chunk(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
        let spawn = start.spawn.unwrap_or_else(|| find_spawn(&world));

        let mut inventory = Inventory::new();
        if !start.mode.is_creative() {
            for (slot, stack) in rules.items.starter_kit_survival().iter().enumerate() {
                inventory.set_slot(slot, Some(*stack));
            }
        }
        let player = Player::new(spawn, start.mode, rules.entities.player());

        Self {
            authoritative: start.authoritative,
            clock: 0.0,
            outbox: Vec::new(),
            world,
            loader,
            fluids: FluidSim::new(),
            structures,
            progression: WorldProgression::default(),
            day_cycle: start.day_cycle,
            ecs: Ecs::new(),
            mobs: MobDirector::new(seed ^ 0x5EED_0F5B_A3B1_E5B0),
            player,
            player_anim: AnimationState::new(),
            physics_accum: 0.0,
            inventory,
            held: None,
            recipes: start.recipes,
            spawn,
            dead: false,
            breaking: None,
            tier_hint: None,
            rules,
        }
    }

    /// Advance the play clock by `dt`.
    pub fn tick_clock(&mut self, dt: f32) {
        self.clock = (self.clock + dt) % CLOCK_PERIOD;
    }

    /// The entity passes every peer runs each frame, in order: drops fall and
    /// are collected, arrows fly, then every animation that is not stepped by
    /// its own simulation — a client's mob replicas and other players, from
    /// what their snapshots show (at the interpolation `alpha` their bodies
    /// are drawn at), and the local body from how it actually moved.
    pub fn tick_entities(&mut self, dt: f32, alpha: f32, max_speed: f32) {
        self.update_drops(dt);
        self.fly_arrows(dt);
        crate::application::ecs::systems::mobs::animate_replicas(&mut self.ecs, dt, max_speed);
        crate::application::ecs::systems::players::animate(&mut self.ecs, dt, alpha, max_speed);
        self.animate_player(dt);
    }

    /// Tell other peers about something this peer decided.
    pub fn emit(&mut self, msg: crate::application::protocol::ServerMessage) {
        self.outbox.push(msg);
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

#[cfg(test)]
mod tests;

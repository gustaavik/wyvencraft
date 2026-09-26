//! The game world as one value: everything a session simulates, apart from
//! how it is drawn and how it reaches other peers.
//!
//! [`Simulation`] is what used to be most of the in-game screen's fields. It
//! holds the world and its entities, the local player and what they carry,
//! and the clocks and planners that move them. It names no renderer, no egui
//! and no transport, which is what lets the use cases on it be tested with
//! nothing but a seed.

use std::sync::Arc;

use glam::Vec3;

use crate::application::ecs::Ecs;
use crate::domain::content::Registries;
use crate::domain::core::{BlockPos, DayCycle};
use crate::domain::entity::{AnimationState, MobId, Player, Spawner};
use crate::domain::inventory::{Inventory, ItemStack, RecipeBook};
use crate::domain::progression::WorldProgression;
use crate::domain::world::structure::Structures;
use crate::domain::world::{ChunkLoader, FluidSim, World};

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

/// Everything the session simulates.
pub struct Simulation {
    /// The rules everything here obeys — the hashed half of the content.
    pub rules: Registries,
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

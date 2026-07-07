//! Cellular water-flow simulation.
//!
//! Water is levelled like classic voxel games: a source block is level 8 and
//! flowing water decays from 7 down to 1 as it spreads horizontally, so a
//! breached shoreline fills nearby air without flooding the world. Water
//! prefers to fall: a column drops straight down and only spreads sideways
//! where it can't go deeper (sources always spread).
//!
//! The sim is event-driven: cells are only (re)evaluated after a nearby block
//! change, at a fixed tick rate, and each change wakes its neighbours — so
//! flow ripples outward and *recedes* the same way when its source is cut.
//! Levels are encoded as distinct block ids (the auto-registered flow blocks
//! of each fluid, see [`crate::world::block::FluidInfo`]), so flow state
//! travels through the ordinary edit-overlay, save, and network paths with no
//! extra metadata.
//!
//! The sim itself is fluid-agnostic: it reads the `fluid` component off the
//! block registry, so any block declared as a fluid source in
//! `assets/blocks.toml` flows with these rules.

use std::collections::HashSet;

use crate::core::{BlockId, BlockPos, Direction};
use crate::world::World;

/// Seconds between simulation steps (one "water tick").
const TICK_INTERVAL: f32 = 0.25;
/// Cap on cells evaluated per tick; the rest stay scheduled for the next one.
const CELL_BUDGET: usize = 2048;

const HORIZONTAL: [Direction; 4] = [
    Direction::NegX,
    Direction::PosX,
    Direction::NegZ,
    Direction::PosZ,
];

/// Event-driven water spread/recede simulation over a [`World`].
#[derive(Default)]
pub struct FluidSim {
    /// Cells to (re)evaluate on the next tick.
    scheduled: HashSet<BlockPos>,
    timer: f32,
}

impl FluidSim {
    pub fn new() -> Self {
        Self::default()
    }

    /// Wake the cell at `pos` and its neighbours after a block edit there.
    pub fn block_changed(&mut self, pos: BlockPos) {
        self.scheduled.insert(pos);
        for dir in Direction::ALL {
            self.scheduled.insert(pos.offset(dir));
        }
    }

    /// Advance the simulation clock and run one step when due. Returns the
    /// block changes applied to `world` so the caller can broadcast them.
    pub fn tick(&mut self, world: &mut World, dt: f32) -> Vec<(BlockPos, BlockId)> {
        self.timer += dt;
        if self.timer < TICK_INTERVAL {
            return Vec::new();
        }
        self.timer = 0.0;
        if self.scheduled.is_empty() {
            return Vec::new();
        }

        let cells: Vec<BlockPos> = self.scheduled.iter().copied().take(CELL_BUDGET).collect();
        for pos in &cells {
            self.scheduled.remove(pos);
        }

        let mut changes = Vec::new();
        for pos in cells {
            let Some(next) = evaluate(world, pos) else {
                continue;
            };
            if world.set_block(pos, next).is_some() {
                changes.push((pos, next));
                // Ripple outward: neighbours may now flow or recede in turn.
                self.block_changed(pos);
            }
        }
        changes
    }
}

/// What the cell at `pos` should become, or `None` if it is stable or not a
/// fluid cell. Only air and flowing fluid re-evaluate; sources are permanent
/// until a block replaces them.
fn evaluate(world: &World, pos: BlockPos) -> Option<BlockId> {
    let reg = world.registry();
    let current = world.block_at(pos);
    match reg.fluid(current) {
        Some(f) if f.is_source() => return None,
        None if !current.is_air() => return None,
        _ => {}
    }
    let next = match target(world, pos) {
        Some((group, level)) => reg.flowing(group, level),
        None => BlockId::AIR,
    };
    (next != current).then_some(next)
}

/// The strongest `(group, level)` claim on `pos` from its neighbours: a full
/// falling stream under any fluid, otherwise one less than the strongest
/// horizontal neighbour that spreads sideways. `None` when nothing feeds it.
fn target(world: &World, pos: BlockPos) -> Option<(u16, u8)> {
    let reg = world.registry();
    if let Some(above) = reg.fluid(world.block_at(pos.offset(Direction::PosY))) {
        return Some((above.group, above.max_level - 1));
    }
    let mut best: Option<(u16, u8)> = None;
    for dir in HORIZONTAL {
        let np = pos.offset(dir);
        let Some(f) = reg.fluid(world.block_at(np)) else {
            continue;
        };
        // Fluid that can still fall (air below) only falls; anything resting
        // on solid ground feeds sideways. Over fluid, only a source above
        // another source spreads (an ocean surface fills a breach) — never
        // fluid above a falling column, so streams stay one block wide.
        let below = world.block_at(np.offset(Direction::NegY));
        let spreads = !below.is_air()
            && match reg.fluid(below) {
                None => true,
                Some(bf) => f.is_source() && bf.is_source() && bf.group == f.group,
            };
        if spreads && f.level > 1 && best.is_none_or(|(_, level)| f.level - 1 > level) {
            best = Some((f.group, f.level - 1));
        }
    }
    best
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use super::*;
    use crate::core::ChunkPos;
    use crate::world::block::{BlockRegistry, blocks};
    use crate::world::generation::NoiseGenerator;
    use crate::world::generation::WorldGenerator;

    /// Basin floor height — far above generated terrain, so the surroundings
    /// are guaranteed air.
    const FLOOR_Y: i32 = 199;
    /// Water sits on the floor at this height.
    const WATER_Y: i32 = 200;
    const CENTER: BlockPos = BlockPos::new(8, WATER_Y, 8);

    /// A world with a walled 5x5 stone basin (interior x/z in 6..=10) high in
    /// the air, so tests fully contain the flow.
    fn basin_world() -> World {
        let generator: Arc<dyn WorldGenerator> = Arc::new(NoiseGenerator::new(42));
        let registry = Arc::new(BlockRegistry::with_builtins());
        let mut world = World::new(generator, registry);
        world.ensure_chunk(ChunkPos::new(0, 0));
        for x in 5..=11 {
            for z in 5..=11 {
                world.set_block(BlockPos::new(x, FLOOR_Y, z), blocks::STONE);
                let rim = x == 5 || x == 11 || z == 5 || z == 11;
                if rim {
                    world.set_block(BlockPos::new(x, WATER_Y, z), blocks::STONE);
                }
            }
        }
        world
    }

    /// Run ticks until a full step applies no changes (or panic — diverged).
    fn settle(sim: &mut FluidSim, world: &mut World) {
        for _ in 0..64 {
            if sim.tick(world, TICK_INTERVAL).is_empty() {
                return;
            }
        }
        panic!("fluid simulation did not settle");
    }

    #[test]
    fn water_spreads_from_a_source_with_decaying_levels() {
        let mut world = basin_world();
        let mut sim = FluidSim::new();
        world.set_block(CENTER, blocks::WATER);
        sim.block_changed(CENTER);
        settle(&mut sim, &mut world);

        // One level weaker per horizontal step; the source is untouched.
        assert_eq!(world.block_at(CENTER), blocks::WATER);
        assert_eq!(
            world.block_at(BlockPos::new(9, WATER_Y, 8)),
            world.registry().flowing(0, 7)
        );
        assert_eq!(
            world.block_at(BlockPos::new(10, WATER_Y, 8)),
            world.registry().flowing(0, 6)
        );
        // Nothing climbs the rim or floats above the surface.
        assert!(world.block_at(BlockPos::new(11, WATER_Y + 1, 8)).is_air());
        assert!(world.block_at(BlockPos::new(8, WATER_Y + 1, 8)).is_air());
    }

    #[test]
    fn flow_recedes_when_the_source_is_removed() {
        let mut world = basin_world();
        let mut sim = FluidSim::new();
        world.set_block(CENTER, blocks::WATER);
        sim.block_changed(CENTER);
        settle(&mut sim, &mut world);

        world.set_block(CENTER, BlockId::AIR);
        sim.block_changed(CENTER);
        settle(&mut sim, &mut world);

        for x in 6..=10 {
            for z in 6..=10 {
                let pos = BlockPos::new(x, WATER_Y, z);
                assert!(world.block_at(pos).is_air(), "water left behind at {pos:?}");
            }
        }
    }

    #[test]
    fn water_falls_before_spreading_sideways() {
        let mut world = basin_world();
        let mut sim = FluidSim::new();
        // A source hovering above the basin: the stream must drop straight
        // down, then spread only from the cell that lands on the floor.
        let hover = BlockPos::new(8, WATER_Y + 4, 8);
        world.set_block(hover, blocks::WATER);
        sim.block_changed(hover);
        settle(&mut sim, &mut world);

        for y in WATER_Y..hover.y {
            let pos = BlockPos::new(8, y, 8);
            assert!(
                world.registry().is_fluid(world.block_at(pos)),
                "column gap at {pos:?}"
            );
            if y > WATER_Y {
                // Mid-air column cells never spread sideways.
                assert!(world.block_at(BlockPos::new(9, y, 8)).is_air());
            }
        }
        // The landing cell spreads across the floor.
        assert!(
            world
                .registry()
                .is_fluid(world.block_at(BlockPos::new(9, WATER_Y, 8)))
        );
    }
}

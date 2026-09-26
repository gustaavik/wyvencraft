//! Writing a structure's blocks into one chunk.
//!
//! Each chunk stamps only the cells of an instance that fall inside it, so a
//! structure straddling a border is assembled from the pieces its chunks write
//! independently — the same way trees are. Nothing here reads a neighbouring
//! chunk: heights come from [`Terrain`], which is a pure function of position.

use crate::domain::core::{BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, LocalPos};
use crate::domain::world::chunk::Chunk;
use crate::domain::world::generation::Terrain;

use super::config::{Arena, StructureDef};
use super::placement::Instance;
use super::template::Cell;

/// Deepest a foundation or arena fill reaches below its structure, so a site
/// on a cliff edge builds a plinth rather than a tower to bedrock.
const MAX_FILL: i32 = 24;

/// Stamp `instance` of `def` into `chunk`, overwriting whatever terrain and
/// features were there.
pub fn stamp(chunk: &mut Chunk, terrain: &Terrain, def: &StructureDef, instance: &Instance) {
    let origin = chunk.pos.origin();
    if let Some(arena) = def.arena {
        level_arena(chunk, origin, terrain, instance, arena);
    }
    let template = &def.template;
    let reach = template.reach();
    let anchor = instance.anchor;
    for dx in -reach..=reach {
        for dz in -reach..=reach {
            let (x, z) = (anchor.x + dx, anchor.z + dz);
            if !in_chunk(origin, x, z) {
                continue;
            }
            for dy in -template.below()..=template.above() {
                if let Cell::Block(block) = template.cell(instance.rot, dx, dy, dz) {
                    put(chunk, origin, x, anchor.y + dy, z, block);
                }
            }
            if let Some(foundation) = template.foundation
                && template.rests_on_ground(instance.rot, dx, dz)
            {
                let top = anchor.y - template.below() - 1;
                let ground = terrain.height(x, z).max(top - MAX_FILL);
                for y in ground + 1..=top {
                    put(chunk, origin, x, y, z, foundation);
                }
            }
        }
    }
}

/// Clear and level the disc around a boss altar: raise low ground to the
/// arena floor, cut hills and trees away above it.
fn level_arena(
    chunk: &mut Chunk,
    origin: BlockPos,
    terrain: &Terrain,
    instance: &Instance,
    arena: Arena,
) {
    let anchor = instance.anchor;
    let r = arena.radius;
    for dx in -r..=r {
        for dz in -r..=r {
            if dx * dx + dz * dz > r * r {
                continue;
            }
            let (x, z) = (anchor.x + dx, anchor.z + dz);
            if !in_chunk(origin, x, z) {
                continue;
            }
            for y in anchor.y + 1..=anchor.y + arena.clearance {
                put(chunk, origin, x, y, z, BlockId::AIR);
            }
            put(chunk, origin, x, anchor.y, z, arena.floor);
            let ground = terrain.height(x, z).max(anchor.y - MAX_FILL);
            for y in ground + 1..anchor.y {
                put(chunk, origin, x, y, z, arena.fill);
            }
        }
    }
}

fn in_chunk(origin: BlockPos, x: i32, z: i32) -> bool {
    (0..CHUNK_SIZE).contains(&(x - origin.x)) && (0..CHUNK_SIZE).contains(&(z - origin.z))
}

fn put(chunk: &mut Chunk, origin: BlockPos, x: i32, y: i32, z: i32, block: BlockId) {
    if !(1..CHUNK_HEIGHT).contains(&y) || !in_chunk(origin, x, z) {
        return;
    }
    let local = LocalPos {
        x: (x - origin.x) as u8,
        y: y as u16,
        z: (z - origin.z) as u8,
    };
    chunk.set_generated(local, block);
}

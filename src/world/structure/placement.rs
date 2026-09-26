//! Where structures stand: a pure function of the seed and the terrain.
//!
//! The world is cut into a `grid × grid` lattice per structure type; each cell
//! rolls at most one candidate site, jittered inside the cell with a margin so
//! the whole structure — arena included — stays in its own cell. Any chunk can
//! therefore list every structure touching it by visiting the few cells its
//! bounds overlap, with no knowledge of any other chunk, and a host can find
//! the nearest altar for a shrine without generating a block.

use crate::core::BlockPos;
use crate::world::generation::Terrain;
use crate::world::generation::features::feature_hash;

use super::config::StructureDef;

/// Candidate sites tried by the spawn guarantee before giving up.
const GUARANTEE_TRIES: i32 = 128;
/// Keeps the guarantee's hash stream apart from the grid's.
const GUARANTEE_SALT: u64 = 0x4755_4152; // "GUAR"
/// Innermost fraction of the guarantee radius a guaranteed site may use —
/// not right on top of spawn.
const GUARANTEE_MIN_FRACTION: f32 = 0.3;

/// One placed structure.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct Instance {
    /// Index into the structure config.
    pub structure: usize,
    /// The footprint's centre column; `y` is the ground layer's height.
    pub anchor: BlockPos,
    /// Quarter turns applied to the template.
    pub rot: u8,
}

impl Instance {
    /// Horizontal distance from the anchor to `(x, z)`.
    pub fn distance_to(&self, x: i32, z: i32) -> f32 {
        ((self.anchor.x - x) as f32).hypot((self.anchor.z - z) as f32)
    }

    /// Whether any block of the instance can land in the inclusive column box.
    pub fn touches(&self, reach: i32, min: (i32, i32), max: (i32, i32)) -> bool {
        self.anchor.x + reach >= min.0
            && self.anchor.x - reach <= max.0
            && self.anchor.z + reach >= min.1
            && self.anchor.z - reach <= max.1
    }
}

/// Whether `(x, z)` is a fit site for `def`, and the instance there if so: the
/// right biome, flat enough across the footprint, and dry if it must be.
pub fn site(
    terrain: &Terrain,
    def: &StructureDef,
    index: usize,
    x: i32,
    z: i32,
    rot: u8,
) -> Option<Instance> {
    if terrain.biome_at(x, z) != def.biome {
        return None;
    }
    let r = def.template.reach();
    let centre = terrain.height(x, z);
    let mut lo = centre;
    let mut hi = centre;
    for (dx, dz) in [(-r, -r), (r, -r), (-r, r), (r, r)] {
        let h = terrain.height(x + dx, z + dz);
        lo = lo.min(h);
        hi = hi.max(h);
    }
    if hi - lo > def.max_slope {
        return None;
    }
    if def.above_sea && lo <= terrain.config().sea_level {
        return None;
    }
    Some(Instance {
        structure: index,
        anchor: BlockPos::new(x, centre, z),
        rot,
    })
}

/// The instance grid cell `(cx, cz)` holds, if any.
pub fn in_cell(
    terrain: &Terrain,
    seed: u64,
    def: &StructureDef,
    index: usize,
    cx: i32,
    cz: i32,
) -> Option<Instance> {
    let h = feature_hash(seed, cx, cz, def.salt);
    if h % 1000 >= def.chance_per_mille {
        return None;
    }
    let margin = def.reach() + 1;
    let span = (def.grid - 2 * margin).max(1) as u64;
    let x = cx * def.grid + margin + ((h >> 8) % span) as i32;
    let z = cz * def.grid + margin + ((h >> 24) % span) as i32;
    site(terrain, def, index, x, z, ((h >> 40) & 3) as u8)
}

/// The grid cells whose instances could reach the inclusive column box.
pub fn cells_touching(
    def: &StructureDef,
    min: (i32, i32),
    max: (i32, i32),
) -> impl Iterator<Item = (i32, i32)> {
    let reach = def.reach();
    let grid = def.grid;
    let cell = |v: i32| v.div_euclid(grid);
    let (x0, x1) = (cell(min.0 - reach), cell(max.0 + reach));
    let (z0, z1) = (cell(min.1 - reach), cell(max.1 + reach));
    (x0..=x1).flat_map(move |cx| (z0..=z1).map(move |cz| (cx, cz)))
}

/// The instance placed by `def`'s spawn guarantee, if it has one and needs it.
///
/// Needed only when the grid put nothing within the radius. Then candidate
/// sites are drawn from the seed in a ring around spawn, and the first that
/// fits — and keeps clear of every grid instance — is used. Deterministic, so
/// every peer finds the same one; expensive enough (a few hundred column
/// samples) that callers cache it.
pub fn guaranteed(
    terrain: &Terrain,
    seed: u64,
    def: &StructureDef,
    index: usize,
) -> Option<Instance> {
    let within = def.guarantee_within?;
    let bound = within.ceil() as i32 + def.grid;
    let nearby: Vec<Instance> = cells_touching(def, (-bound, -bound), (bound, bound))
        .filter_map(|(cx, cz)| in_cell(terrain, seed, def, index, cx, cz))
        .collect();
    if nearby.iter().any(|i| i.distance_to(0, 0) <= within) {
        return None;
    }
    let clearance = (2 * def.reach() + 4) as f32;
    for attempt in 0..GUARANTEE_TRIES {
        let h = feature_hash(seed, attempt, -1, def.salt ^ GUARANTEE_SALT);
        let angle = (h & 0xFFFF) as f32 / 65536.0 * std::f32::consts::TAU;
        let fraction = ((h >> 16) & 0xFFFF) as f32 / 65536.0;
        let radius = within * (GUARANTEE_MIN_FRACTION + (1.0 - GUARANTEE_MIN_FRACTION) * fraction);
        let x = (angle.cos() * radius).round() as i32;
        let z = (angle.sin() * radius).round() as i32;
        let Some(candidate) = site(terrain, def, index, x, z, ((h >> 40) & 3) as u8) else {
            continue;
        };
        if nearby.iter().all(|i| i.distance_to(x, z) > clearance) {
            return Some(candidate);
        }
    }
    log::warn!(
        "no site for guaranteed structure {:?} within {within} blocks of spawn",
        def.id
    );
    None
}

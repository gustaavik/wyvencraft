//! Procedural pixel art for the texture atlas.
//!
//! Every tile is painted deterministically at startup — no image assets on
//! disk. The tile indices defined here are the single source of truth shared
//! by the block registry ([`crate::world::block`]), the player model
//! ([`crate::entity::model`]), and the block-break overlay.

use super::texture::TILE_SIZE;

// Row 0: terrain blocks.
pub const STONE: u32 = 1;
pub const DIRT: u32 = 2;
pub const GRASS_TOP: u32 = 3;
pub const GRASS_SIDE: u32 = 4;
pub const SAND: u32 = 5;
pub const WOOD_BARK: u32 = 7;
pub const WOOD_RINGS: u32 = 8;
pub const LEAVES: u32 = 9;
pub const GLASS: u32 = 10;
pub const BEDROCK: u32 = 11;
pub const SNOW: u32 = 12;
pub const GRAVEL: u32 = 13;
pub const CLAY: u32 = 14;

// Row 4: ores (stone with mineral specks).
pub const COAL_ORE: u32 = 64;
pub const IRON_ORE: u32 = 65;
pub const GOLD_ORE: u32 = 66;
pub const DIAMOND_ORE: u32 = 67;

// Row 1: player skin.
pub const HEAD_FRONT: u32 = 16;
pub const HEAD_SIDE: u32 = 17;
pub const HEAD_TOP: u32 = 18;
pub const HEAD_BACK: u32 = 19;
/// Plain skin (neck, underside of hands).
pub const SKIN: u32 = 20;
/// Shirt torso (front/back).
pub const BODY: u32 = 21;
/// Shirt, shaded darker (torso sides, shoulder tops).
pub const BODY_SIDE: u32 = 22;
/// Sleeve over hand.
pub const ARM: u32 = 23;
/// Trouser leg over boot.
pub const LEG: u32 = 24;
/// Boot sole (underside of legs).
pub const SOLE: u32 = 25;

// Row 2: water animation. The mesher flags water faces and the fragment
// shader steps `WATER_0 + frame` horizontally over time.
pub const WATER_0: u32 = 32;
pub const WATER_FRAMES: u32 = 4;

// Row 3: block-break crack overlay, in order of growing damage.
pub const CRACK_0: u32 = 48;
pub const CRACK_STAGES: u32 = 8;

/// Whether `tile` is one of the animated water frames.
pub fn is_water(tile: u32) -> bool {
    (WATER_0..WATER_0 + WATER_FRAMES).contains(&tile)
}

/// Paint the builtin pixel art for a texture *name* (as referenced by
/// `assets/blocks.toml`). The painters keep the legacy tile constants as their
/// noise seeds, so the art is identical regardless of which atlas slot the
/// name is assigned. Animated names (water) are engine tiles, pre-registered
/// by the tile registry, and are not painted through here.
pub fn paint_named(name: &str) -> Option<TileRgba> {
    Some(match name {
        "stone" => stone(),
        "dirt" => fill(dirt_pixel),
        "grass_top" => fill(grass_pixel),
        "grass_side" => grass_side(),
        "sand" => sand(),
        "wood_bark" => wood_bark(),
        "wood_rings" => wood_rings(),
        "leaves" => leaves(),
        "glass" => glass(),
        "bedrock" => bedrock(),
        "snow" => snow(),
        "gravel" => gravel(),
        "clay" => clay(),
        "coal_ore" => ore(COAL_ORE, [44, 44, 48]),
        "iron_ore" => ore(IRON_ORE, [214, 158, 110]),
        "gold_ore" => ore(GOLD_ORE, [250, 212, 76]),
        "diamond_ore" => ore(DIAMOND_ORE, [104, 224, 220]),
        _ => return None,
    })
}

/// Crack-overlay tile for a break progress in `[0, 1]`.
pub fn crack_tile(progress: f32) -> u32 {
    let stage = (progress.clamp(0.0, 1.0) * CRACK_STAGES as f32) as u32;
    CRACK_0 + stage.min(CRACK_STAGES - 1)
}

const N: usize = TILE_SIZE as usize;

/// One tile of RGBA pixels, indexed `[y][x]` with y = 0 at the top.
pub type TileRgba = [[[u8; 4]; N]; N];

/// Paint the pixel art for `tile`; `None` for unassigned tiles (the atlas
/// builder fills those with the magenta "missing texture" marker).
pub fn paint(tile: u32) -> Option<TileRgba> {
    Some(match tile {
        STONE => stone(),
        DIRT => fill(dirt_pixel),
        GRASS_TOP => fill(grass_pixel),
        GRASS_SIDE => grass_side(),
        SAND => sand(),
        WOOD_BARK => wood_bark(),
        WOOD_RINGS => wood_rings(),
        LEAVES => leaves(),
        GLASS => glass(),
        BEDROCK => bedrock(),
        SNOW => snow(),
        GRAVEL => gravel(),
        CLAY => clay(),
        COAL_ORE => ore(COAL_ORE, [44, 44, 48]),
        IRON_ORE => ore(IRON_ORE, [214, 158, 110]),
        GOLD_ORE => ore(GOLD_ORE, [250, 212, 76]),
        DIAMOND_ORE => ore(DIAMOND_ORE, [104, 224, 220]),
        HEAD_FRONT => head_front(),
        HEAD_SIDE => head_side(),
        HEAD_TOP => fill(|x, y| rgb(HAIR, noise(HEAD_TOP, x, y, 8))),
        HEAD_BACK => fill(|x, y| rgb(HAIR, noise(HEAD_BACK, x, y, 8) - 6)),
        SKIN => fill(|x, y| rgb(SKIN_TONE, noise(SKIN, x, y, 5) - 10)),
        BODY => body(),
        BODY_SIDE => fill(|x, y| rgb(SHIRT, noise(BODY_SIDE, x, y, 6) - 18)),
        ARM => arm(),
        LEG => leg(),
        SOLE => fill(|x, y| rgb(BOOT, noise(SOLE, x, y, 5) - 12)),
        t if is_water(t) => water(t - WATER_0),
        t if (CRACK_0..CRACK_0 + CRACK_STAGES).contains(&t) => cracks(t - CRACK_0),
        _ => return None,
    })
}

// ---- palette ---------------------------------------------------------------

const SKIN_TONE: [u8; 3] = [224, 172, 138];
const HAIR: [u8; 3] = [74, 52, 32];
const SHIRT: [u8; 3] = [66, 118, 186];
const PANTS: [u8; 3] = [58, 70, 118];
const BOOT: [u8; 3] = [58, 52, 46];
const CRACK: [u8; 4] = [26, 22, 18, 205];

// ---- deterministic noise helpers -------------------------------------------

/// Cheap 2D integer hash; `seed` separates layers that share coordinates.
fn hash(seed: u32, x: u32, y: u32) -> u32 {
    let mut h =
        seed.wrapping_mul(0x9E37_79B9) ^ x.wrapping_mul(0x85EB_CA6B) ^ y.wrapping_mul(0xC2B2_AE35);
    h ^= h >> 13;
    h = h.wrapping_mul(0x27D4_EB2F);
    h ^ (h >> 15)
}

/// Uniform noise in `[-amp, amp]`.
fn noise(seed: u32, x: u32, y: u32, amp: i32) -> i32 {
    (hash(seed, x, y) % (2 * amp as u32 + 1)) as i32 - amp
}

/// Opaque pixel: `base` brightened/darkened by `delta`.
fn rgb(base: [u8; 3], delta: i32) -> [u8; 4] {
    rgba(base, delta, 255)
}

fn rgba(base: [u8; 3], delta: i32, alpha: u8) -> [u8; 4] {
    let c = |v: u8| (v as i32 + delta).clamp(0, 255) as u8;
    [c(base[0]), c(base[1]), c(base[2]), alpha]
}

/// Build a tile from a per-pixel function.
fn fill(f: impl Fn(u32, u32) -> [u8; 4]) -> TileRgba {
    std::array::from_fn(|y| std::array::from_fn(|x| f(x as u32, y as u32)))
}

// ---- terrain ----------------------------------------------------------------

fn stone() -> TileRgba {
    fill(|x, y| {
        let blob = noise(STONE, x / 3, y / 3, 10);
        let grain = noise(STONE + 100, x, y, 6);
        let pock = if hash(STONE + 200, x, y) % 100 < 7 {
            -22
        } else {
            0
        };
        rgb([127, 127, 127], blob + grain + pock)
    })
}

fn dirt_pixel(x: u32, y: u32) -> [u8; 4] {
    let clump = noise(DIRT, x / 2, y / 2, 12);
    let grain = noise(DIRT + 100, x, y, 8);
    let fleck = match hash(DIRT + 200, x, y) % 100 {
        0..8 => -26, // dark clods
        8..14 => 18, // lighter grit
        _ => 0,
    };
    rgb([134, 96, 67], clump + grain + fleck)
}

fn grass_pixel(x: u32, y: u32) -> [u8; 4] {
    let base = noise(GRASS_TOP, x, y, 10);
    let blade = if hash(GRASS_TOP + 100, x, y / 2) % 100 < 18 {
        14
    } else {
        0
    };
    rgb([88, 152, 56], base + blade)
}

fn grass_side() -> TileRgba {
    fill(|x, y| {
        // Grass overhang of ragged, per-column depth over the dirt body.
        let fringe = 3 + hash(GRASS_SIDE, x, 0) % 3;
        if y < fringe {
            grass_pixel(x, y)
        } else if y == fringe {
            rgb([70, 120, 45], noise(GRASS_SIDE + 100, x, y, 8))
        } else {
            dirt_pixel(x, y)
        }
    })
}

fn sand() -> TileRgba {
    fill(|x, y| {
        // Soft horizontal ripples with jittered phase, plus fine grain.
        let phase = hash(SAND, x / 4, y / 4) % 3;
        let ripple = if (y + phase) % 6 < 3 { 7 } else { -7 };
        rgb([219, 205, 158], ripple + noise(SAND + 100, x, y, 7))
    })
}

fn wood_bark() -> TileRgba {
    fill(|x, y| {
        let column = (hash(WOOD_BARK, x, 0) % 3) as i32 * 10 - 10;
        let streak = noise(WOOD_BARK + 100, x, y / 3, 8);
        let seam = if hash(WOOD_BARK + 200, x, 0) % 100 < 22 {
            -20
        } else {
            0
        };
        rgb([104, 82, 50], column + streak + seam)
    })
}

fn wood_rings() -> TileRgba {
    fill(|x, y| {
        // Concentric square growth rings around the tile centre.
        let dx = (2 * x as i32 - 15).abs();
        let dy = (2 * y as i32 - 15).abs();
        let ring = dx.max(dy) / 4;
        let base = if ring % 2 == 0 {
            [168, 136, 84]
        } else {
            [136, 104, 60]
        };
        rgb(base, noise(WOOD_RINGS, x, y, 6))
    })
}

fn leaves() -> TileRgba {
    fill(|x, y| {
        if hash(LEAVES, x, y) % 100 < 9 {
            return [0, 0, 0, 0]; // see-through gaps between leaves
        }
        let clump = noise(LEAVES + 100, x / 2, y / 2, 12);
        let grain = noise(LEAVES + 200, x, y, 10);
        rgb([58, 118, 40], clump + grain)
    })
}

fn glass() -> TileRgba {
    fill(|x, y| {
        let edge = TILE_SIZE - 1;
        if x == 0 || y == 0 || x == edge || y == edge {
            return [225, 235, 240, 210]; // pane frame
        }
        match (x + y) % TILE_SIZE {
            4 | 5 | 12 => [235, 245, 250, 96], // diagonal sheen
            _ => [205, 225, 235, 28],
        }
    })
}

fn bedrock() -> TileRgba {
    fill(|x, y| {
        let base = if hash(BEDROCK, x / 3, y / 3) % 100 < 45 {
            [38, 38, 42]
        } else {
            [72, 72, 78]
        };
        rgb(base, noise(BEDROCK + 100, x, y, 8))
    })
}

fn snow() -> TileRgba {
    fill(|x, y| {
        let dimple = if hash(SNOW, x, y) % 100 < 8 { -14 } else { 0 };
        rgb([242, 246, 251], dimple + noise(SNOW + 100, x, y, 4))
    })
}

fn gravel() -> TileRgba {
    fill(|x, y| {
        // Rounded pebbles: one tone per cell, with rows sheared per column of
        // cells so the packing doesn't read as a square grid.
        let sy = y + (x / 3) * 2;
        let cell = hash(GRAVEL, x / 3, sy / 3);
        let tone = (cell % 5) as i32 * 11 - 22;
        let base = if cell.is_multiple_of(7) {
            [94, 88, 82] // occasional dark stone
        } else {
            [131, 123, 114]
        };
        let seam = if x % 3 == 0 || sy % 3 == 0 { -16 } else { 0 };
        rgb(base, tone + seam + noise(GRAVEL + 100, x, y, 5))
    })
}

fn clay() -> TileRgba {
    fill(|x, y| {
        // Smooth blue-grey with soft, broad clumps.
        let clump = noise(CLAY, x / 4, y / 4, 9);
        rgb([155, 162, 176], clump + noise(CLAY + 100, x, y, 4))
    })
}

/// Stone with embedded mineral specks in the ore's signature colour.
fn ore(tile: u32, color: [u8; 3]) -> TileRgba {
    let mut art = stone();
    for i in 0..5u32 {
        let h = hash(tile, i, 0);
        let cx = 2 + (h % 12) as i32;
        let cy = 2 + ((h >> 8) % 12) as i32;
        let chunky = h.is_multiple_of(3);
        for dy in -1i32..=1 {
            for dx in -1i32..=1 {
                if dx.abs() + dy.abs() == 2 && !chunky {
                    continue; // small specks are plus-shaped, big ones 3x3
                }
                let (px, py) = ((cx + dx) as usize, (cy + dy) as usize);
                let glint = if dx == 0 && dy == 0 { 26 } else { 0 };
                art[py][px] = rgb(color, glint + noise(tile + 100, px as u32, py as u32, 10));
            }
        }
    }
    art
}

/// One water animation frame: two diagonal wave layers whose phase advances
/// with `frame`, so cycling the frames reads as rolling water.
fn water(frame: u32) -> TileRgba {
    fill(|x, y| {
        let crest = (x + 2 * y + frame * 4) % TILE_SIZE;
        let swell = (2 * x + y + TILE_SIZE - frame * 4 % TILE_SIZE) % TILE_SIZE;
        if crest < 2 {
            rgba([110, 160, 228], noise(WATER_0, x, y, 6), 185)
        } else if swell < 2 {
            rgba([70, 122, 208], noise(WATER_0 + 1, x, y, 6), 175)
        } else {
            rgba([40, 92, 190], noise(WATER_0 + 2, x, y, 6), 168)
        }
    })
}

// ---- player skin ------------------------------------------------------------

/// Ragged hairline depth (rows of hair) for a given column of a head face.
fn hairline(seed: u32, x: u32) -> u32 {
    4 + hash(seed, x, 0) % 2
}

fn head_front() -> TileRgba {
    fill(|x, y| {
        if y < hairline(HEAD_FRONT, x) {
            return rgb(HAIR, noise(HEAD_TOP, x, y, 8));
        }
        // Eyes: whites outboard, pupils inboard.
        if (8..10).contains(&y) {
            if (3..5).contains(&x) || (11..13).contains(&x) {
                return [244, 244, 244, 255];
            }
            if (5..7).contains(&x) || (9..11).contains(&x) {
                return [58, 66, 120, 255];
            }
        }
        // Mouth.
        if (12..14).contains(&y) && (6..10).contains(&x) {
            return rgb([176, 128, 100], 0);
        }
        rgb(SKIN_TONE, noise(HEAD_FRONT + 100, x, y, 5))
    })
}

fn head_side() -> TileRgba {
    fill(|x, y| {
        if y < hairline(HEAD_SIDE, x) {
            rgb(HAIR, noise(HEAD_TOP, x, y, 8))
        } else if (8..11).contains(&y) && (6..9).contains(&x) {
            rgb([206, 152, 120], 0) // ear
        } else {
            rgb(SKIN_TONE, noise(HEAD_SIDE + 100, x, y, 5))
        }
    })
}

fn body() -> TileRgba {
    fill(|x, y| {
        if y < 2 {
            return rgb(SHIRT, -24); // collar
        }
        if y >= 14 {
            return rgb([52, 62, 96], 0); // belt
        }
        let side = if (2..14).contains(&x) { 0 } else { -14 };
        rgb(SHIRT, side + noise(BODY, x, y, 6))
    })
}

fn arm() -> TileRgba {
    fill(|x, y| {
        if y < 9 {
            let cuff = if y == 8 { -20 } else { 0 };
            rgb(SHIRT, cuff + noise(ARM, x, y, 6))
        } else {
            rgb(SKIN_TONE, noise(ARM + 100, x, y, 5)) // hand
        }
    })
}

fn leg() -> TileRgba {
    fill(|x, y| {
        if y < 12 {
            let seam = if x == 7 || x == 8 { -10 } else { 0 };
            rgb(PANTS, seam + noise(LEG, x, y, 6))
        } else {
            rgb(BOOT, noise(LEG + 100, x, y, 5)) // boot
        }
    })
}

// ---- break cracks -----------------------------------------------------------

/// Minimal LCG so crack paths are deterministic and stage-independent.
struct Lcg(u32);

impl Lcg {
    fn new(seed: u32) -> Self {
        Self(seed | 1)
    }

    /// Next value in `[0, m)`.
    fn next(&mut self, m: u32) -> u32 {
        self.0 = self.0.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (self.0 >> 16) % m
    }
}

/// Crack overlay for damage `stage` in `[0, CRACK_STAGES)`: transparent except
/// for dark crack pixels. Each stage reveals a longer prefix of the same fixed
/// set of random-walk paths, so the cracks grow instead of jumping around.
fn cracks(stage: u32) -> TileRgba {
    const PATHS: u32 = 8;
    const FULL_LENGTH: u32 = 12;
    // Compass bias per path, spreading the cracks around the centre.
    const DIRS: [(i32, i32); 8] = [
        (1, 0),
        (1, 1),
        (0, 1),
        (-1, 1),
        (-1, 0),
        (-1, -1),
        (0, -1),
        (1, -1),
    ];

    let mut tile: TileRgba = [[[0; 4]; N]; N];
    let mut set = |x: i32, y: i32| {
        if (0..N as i32).contains(&x) && (0..N as i32).contains(&y) {
            tile[y as usize][x as usize] = CRACK;
        }
    };

    for i in 0..PATHS {
        let mut rng = Lcg::new(0x9E37_79B9 ^ i.wrapping_mul(0x85EB_CA6B));
        let (bx, by) = DIRS[i as usize % DIRS.len()];
        let mut x = 6 + rng.next(3) as i32;
        let mut y = 6 + rng.next(3) as i32;
        let reveal = (FULL_LENGTH * (stage + 1) / CRACK_STAGES).max(1);
        for step in 0..reveal {
            set(x, y);
            if stage >= 4 && step % 2 == 0 {
                set(x + 1, y); // thicken mature cracks
            }
            // Mostly follow the compass bias, with some jitter.
            if rng.next(4) < 3 {
                x += bx;
                y += by;
            } else {
                x += rng.next(3) as i32 - 1;
                y += rng.next(3) as i32 - 1;
            }
        }
    }

    // A chipped-out centre from mid-damage onward.
    if stage >= 3 {
        for (dx, dy) in [(0, 0), (1, 0), (0, 1), (1, 1)] {
            set(7 + dx, 7 + dy);
        }
    }

    tile
}

#[cfg(test)]
mod tests {
    use super::*;

    fn crack_pixels(stage: u32) -> usize {
        let tile = paint(CRACK_0 + stage).expect("crack tile");
        tile.iter().flatten().filter(|p| p[3] > 0).count()
    }

    #[test]
    fn water_frames_differ_so_animation_is_visible() {
        for frame in 1..WATER_FRAMES {
            assert_ne!(paint(WATER_0), paint(WATER_0 + frame), "frame {frame}");
        }
    }

    #[test]
    fn water_is_translucent() {
        let tile = paint(WATER_0).expect("water tile");
        for p in tile.iter().flatten() {
            assert!(p[3] > 0 && p[3] < 255, "alpha {}", p[3]);
        }
    }

    #[test]
    fn opaque_block_tiles_are_fully_opaque() {
        for tile in [
            STONE,
            DIRT,
            GRASS_TOP,
            GRASS_SIDE,
            SAND,
            WOOD_BARK,
            WOOD_RINGS,
            BEDROCK,
            SNOW,
            GRAVEL,
            CLAY,
            COAL_ORE,
            IRON_ORE,
            GOLD_ORE,
            DIAMOND_ORE,
        ] {
            let art = paint(tile).expect("block tile");
            assert!(art.iter().flatten().all(|p| p[3] == 255), "tile {tile}");
        }
    }

    #[test]
    fn ore_tiles_carry_visible_specks() {
        let stone = paint(STONE).expect("stone");
        for tile in [COAL_ORE, IRON_ORE, GOLD_ORE, DIAMOND_ORE] {
            let art = paint(tile).expect("ore tile");
            let specks = art
                .iter()
                .flatten()
                .zip(stone.iter().flatten())
                .filter(|(a, b)| a != b)
                .count();
            assert!(
                specks >= 12,
                "tile {tile} has too few speck pixels: {specks}"
            );
        }
    }

    #[test]
    fn leaves_have_gaps_but_are_mostly_solid() {
        let art = paint(LEAVES).expect("leaves");
        let gaps = art.iter().flatten().filter(|p| p[3] == 0).count();
        assert!(gaps > 0, "leaves should have see-through gaps");
        assert!(gaps < N * N / 4, "leaves too sparse: {gaps} gaps");
    }

    #[test]
    fn crack_stages_grow() {
        for stage in 1..CRACK_STAGES {
            assert!(
                crack_pixels(stage) >= crack_pixels(stage - 1),
                "stage {stage} shrank"
            );
        }
        assert!(crack_pixels(CRACK_STAGES - 1) > crack_pixels(0));
    }

    #[test]
    fn crack_tile_maps_progress_to_stages() {
        assert_eq!(crack_tile(0.0), CRACK_0);
        assert_eq!(crack_tile(0.5), CRACK_0 + CRACK_STAGES / 2);
        assert_eq!(crack_tile(1.0), CRACK_0 + CRACK_STAGES - 1);
        assert_eq!(crack_tile(2.0), CRACK_0 + CRACK_STAGES - 1);
    }

    #[test]
    fn head_front_has_eyes() {
        let art = paint(HEAD_FRONT).expect("head front");
        assert_eq!(art[8][3], [244, 244, 244, 255], "left eye white");
        assert_eq!(art[8][5], [58, 66, 120, 255], "left pupil");
        assert_eq!(art[8][11], [244, 244, 244, 255], "right eye white");
    }

    #[test]
    fn unassigned_tiles_are_missing() {
        assert!(paint(0).is_none());
        assert!(paint(WATER_0 + WATER_FRAMES).is_none());
        assert!(paint(255).is_none());
    }
}

//! Procedural pixel art for the texture atlas.
//!
//! Every tile is painted deterministically at startup — no image assets on
//! disk. The tile indices defined here are the single source of truth shared
//! by the block registry ([`crate::world::block`]) and the block-break overlay.
//!
//! Blocks are painted with noise; the item icons at the bottom of the file are
//! drawn as ASCII sprites instead, because a pickaxe silhouette is easier to
//! read (and edit) as a picture than as arithmetic.

use wyven_render::texture::TILE_SIZE;

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

// Row 3: block-break crack overlay, in order of growing damage.
pub const CRACK_0: u32 = 48;
pub const CRACK_STAGES: u32 = 8;

/// Paint the builtin pixel art for a texture *name* (as referenced by
/// `assets/blocks.toml`). The painters keep the legacy tile constants as their
/// noise seeds, so the art is identical regardless of which atlas slot the
/// name is assigned.
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
        // Item icons. These are resolved by their in-game item name (spaces and
        // all), so the inventory can ask the tile registry for "wooden pickaxe".
        "wooden pickaxe" => tool_icon(PICKAXE, WOOD_HANDLE, STONE_HEAD),
        "wooden axe" => tool_icon(AXE, WOOD_HANDLE, STONE_HEAD),
        "wooden shovel" => tool_icon(SHOVEL, WOOD_HANDLE, STONE_HEAD),
        "shears" => sprite(SHEARS, SHEARS_PALETTE),
        "vine sword" => sprite(VINE_SWORD, VINE_SWORD_PALETTE),
        // The tiered tools draw from their .bbmodel files instead, so they need
        // no painter here — only `stick`, which has no model, does.
        "stick" => sprite(STICK, STICK_PALETTE),
        "apple" => sprite(APPLE, APPLE_PALETTE),
        "bread" => sprite(BREAD, BREAD_PALETTE),
        "raw beef" => sprite(RAW_BEEF, RAW_BEEF_PALETTE),
        "raw mutton" => sprite(RAW_MUTTON, RAW_MUTTON_PALETTE),
        "helmet" => sprite(HELMET, IRON_ARMOR_PALETTE),
        "chestplate" => sprite(CHESTPLATE, IRON_ARMOR_PALETTE),
        "leggings" => sprite(LEGGINGS, IRON_ARMOR_PALETTE),
        "boots" => sprite(BOOTS, IRON_ARMOR_PALETTE),
        "glove" => sprite(GLOVE, CLOTH_ARMOR_PALETTE),
        "cape" => sprite(CAPE, CLOTH_ARMOR_PALETTE),
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
        t if (CRACK_0..CRACK_0 + CRACK_STAGES).contains(&t) => cracks(t - CRACK_0),
        _ => return None,
    })
}

// ---- palette ---------------------------------------------------------------

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

// ---- item icons -------------------------------------------------------------
//
// Icons are 16×16 ASCII sprites: each of the 16 rows is a 16-char string, and
// every non-space char is looked up in a palette to get its RGBA. A space is
// transparent. This keeps a recognisable silhouette editable by hand, unlike
// the noise-based block art above.

/// A 16-row × 16-col sprite. Rows shorter than 16 chars pad transparent.
type Sprite = [&'static str; N];

/// `(char, rgba)` colour table for a sprite.
type Palette = [(char, [u8; 4])];

/// Rasterise an ASCII sprite through its palette; unmapped chars (and spaces)
/// are transparent.
fn sprite(art: Sprite, palette: &Palette) -> TileRgba {
    let lookup = |c: char| palette.iter().find(|(k, _)| *k == c).map(|(_, v)| *v);
    fill(|x, y| {
        art[y as usize]
            .chars()
            .nth(x as usize)
            .and_then(lookup)
            .unwrap_or([0, 0, 0, 0])
    })
}

/// Compose a tool icon: the shared wooden handle sprite tinted `handle`, then
/// the head sprite tinted `head` painted on top.
fn tool_icon(head: Sprite, handle: [u8; 4], head_color: [u8; 4]) -> TileRgba {
    let handle_px = sprite(HANDLE, &[('H', handle)]);
    let head_px = sprite(head, &[('#', head_color), ('.', shade(head_color, -34))]);
    fill(|x, y| {
        let (hx, hy) = (x as usize, y as usize);
        let h = head_px[hy][hx];
        if h[3] > 0 { h } else { handle_px[hy][hx] }
    })
}

/// Darken/lighten an rgba by a flat delta (keeps alpha).
fn shade(c: [u8; 4], delta: i32) -> [u8; 4] {
    rgba([c[0], c[1], c[2]], delta, c[3])
}

const WOOD_HANDLE: [u8; 4] = [140, 100, 58, 255];
const STONE_HEAD: [u8; 4] = [122, 122, 128, 255];

// A diagonal handle from the bottom-left to the upper-right, shared by all tools.
const HANDLE: Sprite = [
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "            H   ",
    "           H    ",
    "          H     ",
    "         H      ",
    "        H       ",
    "       H        ",
    "      H         ",
    "     H          ",
    "    H           ",
    "   H            ",
    "                ",
];

const PICKAXE: Sprite = [
    "                ",
    "  ..        ..  ",
    " .##.      .##. ",
    " .###.    .###. ",
    "  .####..####.  ",
    "    .########.  ",
    "      .####.    ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const AXE: Sprite = [
    "                ",
    "        ....    ",
    "       .####.   ",
    "      .######.  ",
    "      .######.  ",
    "      .#####.   ",
    "       .###.    ",
    "        ..      ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const SHOVEL: Sprite = [
    "                ",
    "         ...    ",
    "        .###.   ",
    "        .###.   ",
    "        .###.   ",
    "         .#.    ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const SHEARS: Sprite = [
    "                ",
    "  bb        bb  ",
    " b..b      b..b ",
    " b..b      b..b ",
    "  b..b    b..b  ",
    "   b..b  b..b   ",
    "    b..bb..b    ",
    "     b.mm.b     ",
    "      bmmb      ",
    "     bm..mb     ",
    "     m....m     ",
    "     m....m     ",
    "      m..m      ",
    "       mm       ",
    "                ",
    "                ",
];

const SHEARS_PALETTE: &Palette = &[
    ('b', [176, 182, 190, 255]),
    ('.', [214, 220, 228, 255]),
    ('m', [90, 96, 104, 255]),
];

/// The 2D icon for the vine sword. Items drawn from a model file still need a
/// flat icon for the hotbar and inventory grid — the 3D model is what the world
/// shows, not what the UI does.
const VINE_SWORD: Sprite = [
    "              b ",
    "             b.b",
    "            b..b",
    "           b..b ",
    "          b..b  ",
    "     v   b..b   ",
    "      v b..b    ",
    "     vvb..b     ",
    "    v gbb       ",
    "   ggggg  v     ",
    "  gg  gg v      ",
    " hh    ggg      ",
    "hh              ",
    "h               ",
    "                ",
    "                ",
];

const VINE_SWORD_PALETTE: &Palette = &[
    ('b', [122, 132, 146, 255]),
    ('.', [206, 216, 228, 255]),
    ('g', [84, 132, 62, 255]),
    ('v', [122, 172, 82, 255]),
    ('h', [96, 70, 44, 255]),
];

/// A bare stick: the same bottom-left-to-top-right diagonal the tool handles
/// run along, drawn two pixels thick so it reads on its own.
const STICK: Sprite = [
    "                ",
    "                ",
    "            ww  ",
    "           ww.  ",
    "          ww.   ",
    "         ww.    ",
    "        ww.     ",
    "       ww.      ",
    "      ww.       ",
    "     ww.        ",
    "    ww.         ",
    "   ww.          ",
    "   w.           ",
    "                ",
    "                ",
    "                ",
];

const STICK_PALETTE: &Palette = &[('w', [140, 100, 58, 255]), ('.', [104, 74, 42, 255])];

const APPLE: Sprite = [
    "                ",
    "        s       ",
    "        s       ",
    "       l s      ",
    "      ll        ",
    "    hh..hhh     ",
    "   h..#...hh    ",
    "  h..#....hh    ",
    "  h.......hh    ",
    "  h.......hh    ",
    "  hh.....hh     ",
    "   hh...hh      ",
    "    hh.hh       ",
    "     hhh        ",
    "                ",
    "                ",
];

const APPLE_PALETTE: &Palette = &[
    ('h', [176, 34, 40, 255]),
    ('.', [214, 52, 58, 255]),
    ('#', [244, 150, 150, 255]),
    ('s', [96, 66, 40, 255]),
    ('l', [96, 158, 60, 255]),
];

const BREAD: Sprite = [
    "                ",
    "                ",
    "     ccccc      ",
    "   cc##c##cc    ",
    "  c#######c#c   ",
    " c##cc###c##c   ",
    " c#c##c#####c   ",
    " c##########c   ",
    " c##cc###c##c   ",
    "  c########c    ",
    "   cc####cc     ",
    "     cccc       ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const BREAD_PALETTE: &Palette = &[('c', [150, 96, 44, 255]), ('#', [206, 152, 78, 255])];

// A steak: dark seared rim around red flesh with a pale marbling streak.
const RAW_BEEF: Sprite = [
    "                ",
    "                ",
    "                ",
    "    rrrrrrr     ",
    "   r#######rr   ",
    "  r####m####rr  ",
    "  r###m#####rr  ",
    " r####m######r  ",
    " r#####m#####r  ",
    " r######m###rr  ",
    "  r########rr   ",
    "   rrr####rr    ",
    "     rrrrrr     ",
    "                ",
    "                ",
    "                ",
];

const RAW_BEEF_PALETTE: &Palette = &[
    ('r', [130, 32, 34, 255]),
    ('#', [198, 60, 62, 255]),
    ('m', [232, 176, 168, 255]),
];

// A chop on the bone: pink meat with a pale bone stub at the lower left.
const RAW_MUTTON: Sprite = [
    "                ",
    "                ",
    "       ppppp    ",
    "      p#####p   ",
    "     p#######p  ",
    "     p#######p  ",
    "    p########p  ",
    "    p#######p   ",
    "   bp######p    ",
    "  bb p####p     ",
    " bbb  pppp      ",
    " bb             ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const RAW_MUTTON_PALETTE: &Palette = &[
    ('p', [168, 62, 70, 255]),
    ('#', [224, 108, 116, 255]),
    ('b', [230, 224, 210, 255]),
];

const IRON_ARMOR_PALETTE: &Palette = &[
    ('#', [200, 205, 212, 255]),
    ('.', [150, 156, 166, 255]),
    ('o', [108, 114, 124, 255]),
];

const CLOTH_ARMOR_PALETTE: &Palette = &[
    ('#', [150, 84, 62, 255]),
    ('.', [116, 62, 44, 255]),
    ('o', [88, 46, 32, 255]),
];

const HELMET: Sprite = [
    "                ",
    "                ",
    "    o######o    ",
    "   o########o   ",
    "  o##########o  ",
    "  o##########o  ",
    "  o#........#o  ",
    "  o#.oooooo.#o  ",
    "  o##o....o##o  ",
    "  o##o....o##o  ",
    "   oo o..o oo   ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const CHESTPLATE: Sprite = [
    "                ",
    "   o#o    o#o   ",
    "  o###o..o###o  ",
    "  o##########o  ",
    "  o##########o  ",
    "  o#.######.#o  ",
    "  o#.######.#o  ",
    "  o##########o  ",
    "  o##########o  ",
    "  o##########o  ",
    "  o##########o  ",
    "   oo######oo   ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const LEGGINGS: Sprite = [
    "                ",
    "                ",
    "  o##########o  ",
    "  o##########o  ",
    "  o##########o  ",
    "  o####oo####o  ",
    "  o###o..o###o  ",
    "  o###o  o###o  ",
    "  o##.o  o.##o  ",
    "  o##.o  o.##o  ",
    "  o##o    o##o  ",
    "   oo      oo   ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const BOOTS: Sprite = [
    "                ",
    "                ",
    "                ",
    "  o##o    o##o  ",
    "  o##o    o##o  ",
    "  o##o    o##o  ",
    "  o##oo  oo##o  ",
    "  o########.#o  ",
    "  o##########o  ",
    "  o##########o  ",
    "  oo########oo  ",
    "                ",
    "                ",
    "                ",
    "                ",
    "                ",
];

const GLOVE: Sprite = [
    "                ",
    "                ",
    "    o#o o#o     ",
    "    o#o o#o o#o ",
    "    o#o o#o o#o ",
    "    o#####o o#o ",
    "   o########o   ",
    "  o#########o   ",
    "  o#.#######o   ",
    "  o#########o   ",
    "  o########o    ",
    "   oo#####oo    ",
    "     ooooo      ",
    "                ",
    "                ",
    "                ",
];

const CAPE: Sprite = [
    "                ",
    "   oo######oo   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#......#o   ",
    "   o#.####.#o   ",
    "    o#....#o    ",
    "     oo##oo     ",
    "                ",
];

#[cfg(test)]
mod tests {
    use super::*;

    fn crack_pixels(stage: u32) -> usize {
        let tile = paint(CRACK_0 + stage).expect("crack tile");
        tile.iter().flatten().filter(|p| p[3] > 0).count()
    }

    #[test]
    fn item_icons_paint_a_recognisable_silhouette() {
        for name in [
            "wooden pickaxe",
            "wooden axe",
            "wooden shovel",
            "shears",
            "vine sword",
            "stick",
            "apple",
            "bread",
            "raw beef",
            "raw mutton",
            "helmet",
            "chestplate",
            "leggings",
            "boots",
            "glove",
            "cape",
        ] {
            let art = paint_named(name).unwrap_or_else(|| panic!("no icon for {name}"));
            let opaque = art.iter().flatten().filter(|p| p[3] > 0).count();
            // Enough pixels to read as a shape, but not a full opaque square.
            assert!(opaque > 20, "{name} icon nearly empty: {opaque} px");
            assert!(opaque < N * N, "{name} icon has no transparent margin");
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
    fn unassigned_tiles_are_missing() {
        assert!(paint(0).is_none());
        assert!(paint(32).is_none());
        assert!(paint(255).is_none());
    }
}

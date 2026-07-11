//! Procedural armor sheets blitted into the block atlas.
//!
//! Each of the six armor pieces has its own 64×64 sheet, laid out with the same
//! Minecraft unwrap as the player skin ([`super::skin`]) so the entity model can
//! address it with the same [`SkinPart`] rects — only the sheet's atlas origin
//! differs. Sheets are painted procedurally (transparent except the covered face
//! rects) and overridable by `assets/textures/armor_<piece>.png`. They live in a
//! high band of the atlas (rows 4–11), clear of the dynamically-allocated block
//! tiles, the skin block, and the water/crack engine rows.
//!
//! This module is render-local (no dependency on `inventory`); the entity model
//! maps `inventory::ArmorSlot` onto [`ArmorKind`].

use crate::core::Direction;

use super::skin::{self, BODY, CAPE, HEAD, LEFT_ARM, LEFT_LEG, RIGHT_ARM, RIGHT_LEG, SkinPart};
use super::tiles::TileRgba;

/// The six armor pieces, in [`inventory::ArmorSlot`](crate::inventory::ArmorSlot)
/// order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ArmorKind {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
    Glove,
    Cape,
}

pub const ALL: [ArmorKind; 6] = [
    ArmorKind::Helmet,
    ArmorKind::Chestplate,
    ArmorKind::Leggings,
    ArmorKind::Boots,
    ArmorKind::Glove,
    ArmorKind::Cape,
];

/// One covered box: a body part's unwrap plus the vertical band of it the piece
/// paints (`0.0` = top, `1.0` = bottom of the part). Bands let boots and gloves
/// cover the lower limb while leggings/chestplate keep the upper, so both remain
/// visible on the shared box.
type Cover = (SkinPart, f32, f32);

impl ArmorKind {
    /// The atlas tile `[col, row]` of this piece's 64×64 sheet.
    pub fn origin(self) -> [u32; 2] {
        match self {
            ArmorKind::Helmet => [0, 4],
            ArmorKind::Chestplate => [4, 4],
            ArmorKind::Leggings => [8, 4],
            ArmorKind::Boots => [0, 8],
            ArmorKind::Glove => [4, 8],
            ArmorKind::Cape => [8, 8],
        }
    }

    /// File-name stem for the PNG override (`assets/textures/armor_<name>.png`).
    fn name(self) -> &'static str {
        match self {
            ArmorKind::Helmet => "helmet",
            ArmorKind::Chestplate => "chestplate",
            ArmorKind::Leggings => "leggings",
            ArmorKind::Boots => "boots",
            ArmorKind::Glove => "glove",
            ArmorKind::Cape => "cape",
        }
    }

    fn coverage(self) -> &'static [Cover] {
        match self {
            ArmorKind::Helmet => &[(HEAD, 0.0, 1.0)],
            ArmorKind::Chestplate => &[
                (BODY, 0.0, 1.0),
                (LEFT_ARM, 0.0, 0.55),
                (RIGHT_ARM, 0.0, 0.55),
            ],
            ArmorKind::Leggings => &[
                (BODY, 0.5, 1.0),
                (LEFT_LEG, 0.0, 0.7),
                (RIGHT_LEG, 0.0, 0.7),
            ],
            ArmorKind::Boots => &[(LEFT_LEG, 0.65, 1.0), (RIGHT_LEG, 0.65, 1.0)],
            ArmorKind::Glove => &[(LEFT_ARM, 0.55, 1.0), (RIGHT_ARM, 0.55, 1.0)],
            ArmorKind::Cape => &[(CAPE, 0.0, 1.0)],
        }
    }

    /// Base colour: steel for the plate pieces, cloth brown for glove and cape.
    fn base_color(self) -> [u8; 3] {
        match self {
            ArmorKind::Glove | ArmorKind::Cape => [150, 84, 62],
            _ => [200, 205, 212],
        }
    }
}

/// This piece's sheet as atlas `(tile index, pixels)` pairs, ready to blit.
pub fn atlas_tiles(kind: ArmorKind) -> Vec<(u32, TileRgba)> {
    let sheet = load(kind);
    skin::atlas_tiles_at(&sheet, kind.origin()).collect()
}

/// Whether `tile` belongs to any armor sheet's reserved block.
pub fn is_armor_tile(tile: u32) -> bool {
    ALL.iter()
        .flat_map(|k| skin::tile_indices_at(k.origin()))
        .any(|t| t == tile)
}

/// Load a piece's sheet: the PNG override if present and valid, else painted.
fn load(kind: ArmorKind) -> Box<skin::SkinRgba> {
    let path = format!("assets/textures/armor_{}.png", kind.name());
    match std::fs::read(&path) {
        Ok(bytes) => match skin::decode(&bytes) {
            Ok(sheet) => {
                log::info!("armor {}: using {path}", kind.name());
                sheet
            }
            Err(err) => {
                log::warn!(
                    "ignoring {path}: {err}; painting {} procedurally",
                    kind.name()
                );
                paint(kind)
            }
        },
        Err(_) => paint(kind),
    }
}

/// Paint a piece's sheet: transparent everywhere except the covered face rects,
/// which are filled with a shaded, edge-darkened base colour.
fn paint(kind: ArmorKind) -> Box<skin::SkinRgba> {
    let n = skin::SKIN_SIZE as usize;
    let mut sheet = Box::new([[[0u8; 4]; skin::SKIN_SIZE as usize]; skin::SKIN_SIZE as usize]);
    let base = kind.base_color();
    for &(part, v0, v1) in kind.coverage() {
        for dir in Direction::ALL {
            let [x, y, w, h] = part.face_rect(dir);
            let Some((row0, row1)) = band(dir, y, h, v0, v1) else {
                continue;
            };
            let fill = shade(base, dir);
            for yy in row0..row1 {
                for xx in x..x + w {
                    if (xx as usize) >= n || (yy as usize) >= n {
                        continue;
                    }
                    let edge = xx == x || xx + 1 == x + w || yy == row0 || yy + 1 == row1;
                    let px = if edge { delta(fill, -30) } else { fill };
                    sheet[yy as usize][xx as usize] = px;
                }
            }
        }
    }
    sheet
}

/// The pixel rows of a face to paint for a `[v0, v1]` vertical band. Side faces
/// take the band directly; caps paint only when the band reaches their edge.
fn band(dir: Direction, y: u32, h: u32, v0: f32, v1: f32) -> Option<(u32, u32)> {
    match dir {
        Direction::PosY => (v0 == 0.0).then_some((y, y + h)), // top cap
        Direction::NegY => (v1 >= 1.0).then_some((y, y + h)), // bottom cap
        _ => {
            let r0 = y + (h as f32 * v0) as u32;
            let r1 = y + (h as f32 * v1).round() as u32;
            (r1 > r0).then_some((r0, r1))
        }
    }
}

/// Directional shading matching the model's face shading (top lit, sides dim).
fn shade(base: [u8; 3], dir: Direction) -> [u8; 4] {
    let d = match dir {
        Direction::PosY => 20,
        Direction::NegY => -34,
        Direction::PosX | Direction::NegX => -10,
        Direction::PosZ | Direction::NegZ => -18,
    };
    delta_rgb(base, d)
}

fn delta_rgb(base: [u8; 3], d: i32) -> [u8; 4] {
    let c = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
    [c(base[0]), c(base[1]), c(base[2]), 255]
}

fn delta(px: [u8; 4], d: i32) -> [u8; 4] {
    let c = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
    [c(px[0]), c(px[1]), c(px[2]), px[3]]
}

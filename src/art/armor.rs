//! Worn-armor sheets blitted into the block atlas.
//!
//! Each of the six armor pieces has its own 64×64 sheet, laid out with the same
//! Minecraft unwrap as the player skin ([`super::skin`]) so the entity model can
//! address it with the same `SkinPart` rects — only the sheet's atlas origin
//! differs. Sheets are read from `assets/textures/equipment/armor_<piece>.png`; a piece
//! with no file gets a magenta sheet, so unmade art is visible rather than
//! invisible. They live in a band below the crack overlay, clear of the
//! dynamically-allocated block tiles and the skin block.
//!
//! This module is render-local (no dependency on `inventory`); the entity model
//! maps `inventory::ArmorSlot` onto [`ArmorKind`].

use super::skin;
use wyven_render::TileRgba;

/// First atlas row of the armor band. Row 3 is the crack overlay, and the skin
/// sits in the top-right corner, so the band starts below both.
const BAND_ROW: u32 = 4;

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

/// How many atlas rows the whole armor band takes, so the mob band below it
/// knows where it may start.
pub fn band_rows() -> u32 {
    skin::band_rows(ALL.len() as u32)
}

/// The first atlas row *after* the armor band.
pub fn next_row() -> u32 {
    BAND_ROW + band_rows()
}

impl ArmorKind {
    /// The atlas tile `[col, row]` of this piece's 64×64 sheet.
    pub fn origin(self) -> [u32; 2] {
        let index = ALL.iter().position(|&k| k == self).unwrap_or(0) as u32;
        skin::sheet_origin(BAND_ROW, index)
    }

    /// File-name stem for the PNG (`assets/textures/equipment/armor_<name>.png`).
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

/// Load a piece's sheet: the PNG if present and valid, else a magenta sheet.
fn load(kind: ArmorKind) -> Box<skin::SkinRgba> {
    let path = format!("assets/textures/equipment/armor_{}.png", kind.name());
    match std::fs::read(&path) {
        Ok(bytes) => match skin::decode(&bytes) {
            Ok(sheet) => {
                log::info!("armor {}: using {path}", kind.name());
                sheet
            }
            Err(err) => {
                log::warn!("ignoring {path}: {err}; {} has no art", kind.name());
                skin::missing_sheet()
            }
        },
        Err(_) => {
            log::warn!("no {path}; {} has no art", kind.name());
            skin::missing_sheet()
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashSet;

    /// Every piece gets its own block of tiles — the layout helper must not
    /// hand two pieces the same slot at any [`TILE_SIZE`](wyven_render::texture::TILE_SIZE).
    #[test]
    fn armor_sheets_never_overlap() {
        let mut seen = HashSet::new();
        for kind in ALL {
            for tile in skin::tile_indices_at(kind.origin()) {
                assert!(seen.insert(tile), "{kind:?} reuses tile {tile}");
            }
        }
    }

    /// The band sits below the crack overlay and clear of the skin block.
    #[test]
    fn armor_band_is_clear_of_the_other_reserved_art() {
        for kind in ALL {
            for tile in skin::tile_indices_at(kind.origin()) {
                assert!(
                    !(super::super::cracks::CRACK_0
                        ..super::super::cracks::CRACK_0 + super::super::cracks::CRACK_STAGES)
                        .contains(&tile),
                    "{kind:?} lands on the crack overlay"
                );
                assert!(
                    !skin::atlas_tile_indices().any(|t| t == tile),
                    "{kind:?} lands on the player skin"
                );
            }
        }
    }
}

//! Mob skin sheets blitted into the block atlas.
//!
//! Each mob skin is a 64×64 sheet like the player skin and the armor sheets
//! ([`super::skin`], [`super::armor`]). Humanoid mobs (zombie, skeleton) use
//! the standard Minecraft unwrap so [`crate::entity::HumanoidModel`] renders
//! them by only switching the sheet's atlas origin; quadrupeds (cow, sheep,
//! chicken) use this module's own unwrap ([`Q_HEAD`]/[`Q_BODY`]/[`Q_LEG`])
//! sized for a four-legged body. Sheets are read from
//! `assets/textures/mob_<name>.png`; a mob with no file gets a magenta sheet,
//! so unmade art is visible rather than invisible. They occupy the band below
//! the armor sheets, clear of the dynamically allocated content tiles.
//!
//! Entity kinds reference skins by name (`[entity.visual] skin = "cow"`);
//! [`origin_for`] is the lookup the state layer uses when building meshes.

use super::armor;
use super::skin::{self, SkinPart};
use wyven_render::TileRgba;

/// The quadruped unwrap. A cow-sized body (12×10×18 px) unfolds 60 px wide,
/// so it gets its own row instead of reusing the humanoid layout. All four
/// legs share one unwrap. Data-driven part sizes that differ from these
/// canonical boxes simply stretch the sampled rects — fine for flat fills.
pub const Q_HEAD: SkinPart = SkinPart::new([0, 0], [8, 8, 6]);
pub const Q_LEG: SkinPart = SkinPart::new([28, 0], [4, 11, 4]);
pub const Q_BODY: SkinPart = SkinPart::new([0, 20], [12, 10, 18]);

/// The shipped mob skins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobSkin {
    Zombie,
    Skeleton,
    Cow,
    Sheep,
    Chicken,
}

pub const ALL: [MobSkin; 5] = [
    MobSkin::Zombie,
    MobSkin::Skeleton,
    MobSkin::Cow,
    MobSkin::Sheep,
    MobSkin::Chicken,
];

impl MobSkin {
    /// The atlas tile `[col, row]` of this skin's 64×64 sheet.
    pub fn origin(self) -> [u32; 2] {
        let index = ALL.iter().position(|&k| k == self).unwrap_or(0) as u32;
        skin::sheet_origin(armor::next_row(), index)
    }

    /// The name entity kinds reference (and the PNG file stem).
    fn name(self) -> &'static str {
        match self {
            MobSkin::Zombie => "zombie",
            MobSkin::Skeleton => "skeleton",
            MobSkin::Cow => "cow",
            MobSkin::Sheep => "sheep",
            MobSkin::Chicken => "chicken",
        }
    }
}

/// Atlas origin of the named skin, as referenced from `entities.toml`.
pub fn origin_for(name: &str) -> Option<[u32; 2]> {
    ALL.iter().find(|s| s.name() == name).map(|s| s.origin())
}

/// This skin's sheet as atlas `(tile index, pixels)` pairs, ready to blit.
pub fn atlas_tiles(kind: MobSkin) -> Vec<(u32, TileRgba)> {
    let sheet = load(kind);
    skin::atlas_tiles_at(&sheet, kind.origin()).collect()
}

/// Whether `tile` belongs to any mob skin's reserved block.
pub fn is_mob_tile(tile: u32) -> bool {
    ALL.iter()
        .flat_map(|k| skin::tile_indices_at(k.origin()))
        .any(|t| t == tile)
}

/// Load a skin's sheet: the PNG if present and valid, else a magenta sheet.
fn load(kind: MobSkin) -> Box<skin::SkinRgba> {
    let path = format!("assets/textures/mob_{}.png", kind.name());
    match std::fs::read(&path) {
        Ok(bytes) => match skin::decode(&bytes) {
            Ok(sheet) => {
                log::info!("mob skin {}: using {path}", kind.name());
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

    #[test]
    fn skin_names_resolve_to_reserved_origins() {
        for kind in ALL {
            assert_eq!(origin_for(kind.name()), Some(kind.origin()));
        }
        assert_eq!(origin_for("dragon"), None);
    }

    #[test]
    fn quadruped_unwrap_fits_the_sheet() {
        for part in [Q_HEAD, Q_BODY, Q_LEG] {
            for dir in crate::core::Direction::ALL {
                let [x, y, w, h] = part.face_rect(dir);
                assert!(x + w <= skin::SKIN_SIZE, "{part:?} {dir:?} overflows x");
                assert!(y + h <= skin::SKIN_SIZE, "{part:?} {dir:?} overflows y");
            }
        }
    }

    #[test]
    fn mob_sheets_stay_clear_of_skin_and_armor_blocks() {
        let mut seen = HashSet::new();
        for kind in ALL {
            for tile in skin::tile_indices_at(kind.origin()) {
                assert!(seen.insert(tile), "{kind:?} reuses tile {tile}");
                assert!(!skin::atlas_tile_indices().any(|t| t == tile));
                assert!(!armor::is_armor_tile(tile), "{kind:?} overlaps armor");
            }
        }
    }
}

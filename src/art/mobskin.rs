//! Mob skin sheets blitted into the block atlas.
//!
//! Each mob skin is a 64×64 sheet like the player skin and the armor sheets
//! ([`super::skin`], [`super::armor`]); a 64×32 half-sheet is padded out to one.
//! Humanoid mobs (zombie, skeleton) use the standard Minecraft unwrap so
//! [`crate::entity::HumanoidModel`] renders them by only switching the sheet's
//! atlas origin. Quadrupeds (cow, sheep, pig, chicken) carry their unwrap as
//! data — `head_uv`/`body_uv`/`leg_uv` in `assets/entities.toml` — because no
//! two of them share one: a cow, a pig and a sheep all sit at different offsets.
//! Sheets are read from `assets/textures/entity/<name>/<name>.png`; a mob with
//! no file gets a magenta sheet, so unmade art is visible rather than invisible.
//! They occupy the band below the armor sheets, clear of the dynamically
//! allocated content tiles.
//!
//! Entity kinds reference skins by name (`[entity.visual] skin = "cow"`);
//! [`origin_for`] is the lookup the state layer uses when building meshes.

use super::armor;
use super::skin;
use wyven_render::TileRgba;

/// The shipped mob skins.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MobSkin {
    Zombie,
    Skeleton,
    Cow,
    Sheep,
    Pig,
    Chicken,
}

pub const ALL: [MobSkin; 6] = [
    MobSkin::Zombie,
    MobSkin::Skeleton,
    MobSkin::Cow,
    MobSkin::Sheep,
    MobSkin::Pig,
    MobSkin::Chicken,
];

impl MobSkin {
    /// The atlas tile `[col, row]` of this skin's 64×64 sheet.
    pub fn origin(self) -> [u32; 2] {
        let index = ALL.iter().position(|&k| k == self).unwrap_or(0) as u32;
        skin::sheet_origin(armor::next_row(), index)
    }

    /// Whether this mob wears the player unwrap, and so needs a legacy sheet's
    /// missing left limbs filled in. Quadrupeds have their own unwrap and no
    /// such notion.
    fn is_humanoid(self) -> bool {
        matches!(self, MobSkin::Zombie | MobSkin::Skeleton)
    }

    /// The name entity kinds reference (and the PNG file stem).
    fn name(self) -> &'static str {
        match self {
            MobSkin::Zombie => "zombie",
            MobSkin::Skeleton => "skeleton",
            MobSkin::Cow => "cow",
            MobSkin::Sheep => "sheep",
            MobSkin::Pig => "pig",
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
///
/// Mob art lives one directory per mob (`entity/cow/cow.png`) so a mob that
/// grows a second sheet — a sheep's wool, a variant's recolour — has somewhere
/// obvious to put it.
fn load(kind: MobSkin) -> Box<skin::SkinRgba> {
    let name = kind.name();
    let path = format!("assets/textures/entity/{name}/{name}.png");
    let decode = if kind.is_humanoid() {
        skin::decode_humanoid
    } else {
        skin::decode
    };
    match std::fs::read(&path) {
        Ok(bytes) => match decode(&bytes) {
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

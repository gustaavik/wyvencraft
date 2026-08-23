//! Wyvencraft's art, and how the renderer gets at it.
//!
//! None of this is engine work. `grass_block_side`, a zombie's face and the
//! eight stages of a block breaking are this game's assets; they lived inside
//! `render` only because that is where the atlas is assembled. The renderer now
//! asks for art through [`wyven_render::TileSource`], and [`WyvencraftArt`] is
//! the answer — which is why a second game built on these crates would not
//! inherit a pickaxe sprite.
//!
//! Every tile is a PNG under `assets/textures/`; nothing here paints. What the
//! modules below hold is the *layout* — which sheet lands on which atlas tiles,
//! and how a body part's box unwraps onto it:
//!
//! - [`cracks`] — the eight break-overlay stages and their reserved band.
//! - [`skin`] — the 64×64 Minecraft-format player skin sheet and its part rects.
//! - [`armor`] — worn-armor sheets, sharing the skin's unwrap.
//! - [`mobskin`] — per-mob sheets, humanoid and quadruped.

pub mod armor;
pub mod cracks;
pub mod mobskin;
pub mod skin;

use wyven_render::{ReservedTiles, TileRegistry, TileRgba, TileSource, decode_tile};

/// Where Wyvencraft looks for a named tile: `assets/textures/<name>.png`.
///
/// A name with no file resolves to the magenta missing-texture marker and one
/// warning in the log, which is exactly what art that has not been drawn yet
/// should look like.
pub struct WyvencraftArt;

impl TileSource for WyvencraftArt {
    fn tile(&self, name: &str) -> Option<TileRgba> {
        load_png(name)
    }
}

/// Decode `assets/textures/<name>.png`, or `None` if it is absent or unusable.
///
/// A malformed or wrong-sized PNG warns and resolves to the missing marker
/// rather than failing the load — bad art should look wrong, not stop the game
/// booting.
fn load_png(name: &str) -> Option<TileRgba> {
    let path = format!("assets/textures/{name}.png");
    let bytes = std::fs::read(&path).ok()?;
    match decode_tile(&bytes) {
        Ok(art) => {
            log::info!("texture {name:?}: using {path}");
            Some(art)
        }
        Err(err) => {
            log::warn!("ignoring {path}: {err}");
            None
        }
    }
}

/// The atlas slots Wyvencraft addresses by constant rather than by name, and so
/// must pin: the break-crack overlay, the player skin, the armor sheets and the
/// mob sheets. Everything else is allocated on demand by name.
fn reserved() -> ReservedTiles {
    let sheet = skin::load_default();
    let armor = armor::ALL.iter().flat_map(|&kind| armor::atlas_tiles(kind));
    let mobs = mobskin::ALL
        .iter()
        .flat_map(|&kind| mobskin::atlas_tiles(kind));

    ReservedTiles::new()
        .extend(cracks::atlas_tiles())
        .extend(skin::atlas_tiles(&sheet))
        .extend(armor)
        .extend(mobs)
}

/// A tile registry stocked with Wyvencraft's art: the reserved sheets pinned,
/// and [`WyvencraftArt`] standing by for everything the content files name.
pub fn tile_registry() -> TileRegistry {
    TileRegistry::new(Box::new(WyvencraftArt), &reserved())
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_render::{TileEntry, texture::TILE_SIZE};

    /// Assert `tile` belongs to no reserved band, naming the one it hit.
    fn assert_unreserved(tile: u32, who: &str) {
        assert_ne!(tile, 0, "{who} got the missing marker");
        assert!(
            !skin::atlas_tile_indices().any(|s| s == tile),
            "{who} hit the skin"
        );
        assert!(!armor::is_armor_tile(tile), "{who} landed in the armor band");
        assert!(!mobskin::is_mob_tile(tile), "{who} landed in a mob sheet");
        assert!(
            !(cracks::CRACK_0..cracks::CRACK_0 + cracks::CRACK_STAGES).contains(&tile),
            "{who} landed on the crack overlay"
        );
    }

    /// The reserved bands exist so art addressed by constant keeps its index.
    /// Content allocated by name must never be handed one of those slots.
    #[test]
    fn content_tiles_never_land_on_reserved_art() {
        let mut reg = tile_registry();
        for name in ["apple", "stick", "coal", "flint"] {
            assert_unreserved(reg.resolve(name).tile, name);
        }
    }

    /// The same invariant, but for *every* slot the registry ever hands out
    /// rather than the handful of names a game happens to resolve first.
    ///
    /// Those first names land at tiles 1-4, far below any reserved band, so
    /// the test above cannot see a band that was never claimed in the first
    /// place. That is precisely how the crack overlay's slots went missing:
    /// with no art on disk `cracks::atlas_tiles` yielded nothing, tiles 48-55
    /// were never marked used, and content quietly moved in.
    #[test]
    fn allocation_never_reaches_a_reserved_band() {
        let mut reg = tile_registry();
        let art: TileRgba = [[[255; 4]; TILE_SIZE as usize]; TILE_SIZE as usize];
        // Ask for far more tiles than any band starts at, so allocation is
        // forced to walk past all of them.
        for i in 0..(mobskin::ALL.len() as u32 + 1) * 64 {
            let name = format!("filler_{i}");
            let entry = reg.insert(&name, art);
            if entry == TileEntry::MISSING {
                break; // atlas exhausted; every slot before it was checked
            }
            assert_unreserved(entry.tile, &name);
        }
    }

    #[test]
    fn content_tiles_allocate_and_stay_stable() {
        let mut reg = tile_registry();
        let apple = reg.resolve("apple");
        assert_ne!(apple, TileEntry::MISSING);
        assert_eq!(reg.resolve("apple"), apple);
        assert_ne!(reg.resolve("stick").tile, apple.tile);
    }

    #[test]
    fn unknown_names_resolve_to_missing() {
        let mut reg = tile_registry();
        assert_eq!(reg.resolve("no such texture"), TileEntry::MISSING);
    }
}

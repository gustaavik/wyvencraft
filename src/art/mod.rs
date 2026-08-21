//! Wyvencraft's own pixel art, and how the renderer gets at it.
//!
//! None of this is engine work. `grass_block_side`, a zombie's face and the
//! eight stages of a block breaking are this game's assets; they lived inside
//! `render` only because that is where the atlas is assembled. The renderer now
//! asks for art through [`crate::render::TileSource`], and [`WyvencraftArt`] is
//! the answer — which is why a second game built on these crates would not
//! inherit a pickaxe sprite.
//!
//! - [`tiles`] — procedural 16×16 art for blocks, plus ASCII-sprite item icons.
//! - [`skin`] — the 64×64 Minecraft-format player skin sheet and its part rects.
//! - [`armor`] — worn-armor sheets, sharing the skin's unwrap.
//! - [`mobskin`] — per-mob sheets, humanoid and quadruped.

pub mod armor;
pub mod mobskin;
pub mod skin;
pub mod tiles;

use crate::render::{ReservedTiles, TileRegistry, TileRgba, TileSource, decode_tile};

/// Where Wyvencraft looks for a named tile: a PNG under `assets/textures/`
/// first, then the procedural painter of the same name.
///
/// The PNG wins so any painted tile can be overridden by dropping a file in
/// beside it, which is how art gets replaced without touching Rust.
pub struct WyvencraftArt;

impl TileSource for WyvencraftArt {
    fn tile(&self, name: &str) -> Option<TileRgba> {
        load_png(name).or_else(|| tiles::paint_named(name))
    }
}

/// Decode `assets/textures/<name>.png`, or `None` if it is absent or unusable.
///
/// A malformed or wrong-sized PNG warns and falls through to the procedural
/// painter rather than failing the load — a bad override should look wrong, not
/// stop the game booting.
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
    let cracks = (tiles::CRACK_0..tiles::CRACK_0 + tiles::CRACK_STAGES)
        .filter_map(|tile| tiles::paint(tile).map(|art| (tile, art)));

    let sheet = skin::load_default();
    let armor = armor::ALL.iter().flat_map(|&kind| armor::atlas_tiles(kind));
    let mobs = mobskin::ALL
        .iter()
        .flat_map(|&kind| mobskin::atlas_tiles(kind));

    ReservedTiles::new()
        .extend(cracks)
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
    use crate::render::TileEntry;

    /// The reserved bands exist so art addressed by constant keeps its index.
    /// Content allocated by name must never be handed one of those slots.
    #[test]
    fn content_tiles_never_land_on_reserved_art() {
        let mut reg = tile_registry();
        for name in ["stone", "dirt", "leaves", "glass"] {
            let t = reg.resolve(name).tile;
            assert_ne!(t, 0, "{name} got the missing marker");
            assert!(
                !skin::atlas_tile_indices().any(|s| s == t),
                "{name} hit the skin"
            );
            assert!(!armor::is_armor_tile(t), "{name} landed in the armor band");
            assert!(!mobskin::is_mob_tile(t), "{name} landed in a mob sheet");
            assert!(
                !(tiles::CRACK_0..tiles::CRACK_0 + tiles::CRACK_STAGES).contains(&t),
                "{name} landed on the crack overlay"
            );
        }
    }

    #[test]
    fn content_tiles_allocate_and_stay_stable() {
        let mut reg = tile_registry();
        let stone = reg.resolve("stone");
        assert_ne!(stone, TileEntry::MISSING);
        assert_eq!(reg.resolve("stone"), stone);
        assert_ne!(reg.resolve("dirt").tile, stone.tile);
    }

    #[test]
    fn unknown_names_resolve_to_missing() {
        let mut reg = tile_registry();
        assert_eq!(reg.resolve("no such texture"), TileEntry::MISSING);
    }
}

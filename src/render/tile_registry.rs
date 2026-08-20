//! Name-keyed atlas tile registry.
//!
//! Content (blocks.toml) references textures by *name*. Each name resolves,
//! in priority order, to: a PNG on disk (`assets/textures/<name>.png`, 16×16),
//! the procedural painter of the same name ([`super::tiles::paint_named`]),
//! or the magenta "missing texture" marker. Tile indices are assigned
//! dynamically at load; nothing outside the registry should assume a name's
//! index.
//!
//! *Engine tiles* (the player skin sheet, the animated water frames, the
//! break-crack overlay) are pre-registered at fixed indices: the skin block is
//! blitted from [`super::skin`]; water and cracks are painted from the
//! constants in [`super::tiles`], since the entity model and the fragment
//! shader's water animation address them by constant. Animated textures occupy
//! contiguous same-row slots (`<name>_<frame>.png` overrides each frame).

use std::collections::HashMap;

use super::texture::{ATLAS_COLUMNS, ATLAS_SIZE, TILE_SIZE};
use super::tiles::{self, TileRgba};
use super::{armor, mobskin, skin};

/// Total atlas capacity; growth requires touching `ATLAS_COLUMNS` in the
/// fragment shader too, so it is fixed for now.
const MAX_TILES: usize = (ATLAS_COLUMNS * ATLAS_COLUMNS) as usize;

/// The magenta marker painted into any atlas tile without assigned art, so a
/// bad tile index is immediately visible in-game.
const MISSING_TEXTURE: [u8; 4] = [255, 0, 255, 255];

/// A resolved texture: first atlas tile + frame count (1 = static).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    pub tile: u32,
    pub frames: u32,
}

impl TileEntry {
    /// The missing-texture marker (tile 0 is reserved for it).
    pub const MISSING: TileEntry = TileEntry { tile: 0, frames: 1 };

    #[inline]
    pub fn is_animated(&self) -> bool {
        self.frames > 1
    }
}

/// Name → tile assignment plus the CPU-side atlas pixels.
pub struct TileRegistry {
    by_name: HashMap<String, TileEntry>,
    pixels: Vec<Option<TileRgba>>,
    used: [bool; MAX_TILES],
}

impl TileRegistry {
    /// A registry holding only the engine tiles, at their fixed indices.
    pub fn with_engine_tiles() -> Self {
        let mut reg = Self {
            by_name: HashMap::new(),
            pixels: vec![None; MAX_TILES],
            used: [false; MAX_TILES],
        };
        // Tile 0 is the reserved missing-texture marker.
        reg.used[0] = true;

        // Unnamed engine art addressed by constant: the break-crack overlay.
        let cracks = tiles::CRACK_0..tiles::CRACK_0 + tiles::CRACK_STAGES;
        for tile in cracks {
            reg.claim_fixed(tile);
        }

        // Player skin: blit the 64×64 Minecraft-format sheet into its reserved
        // atlas block. Each tile carries its own art (sliced from the sheet), so
        // it is set directly rather than through `claim_fixed`/`tiles::paint`.
        let sheet = skin::load_default();
        for (tile, art) in skin::atlas_tiles(&sheet) {
            reg.used[tile as usize] = true;
            reg.pixels[tile as usize] = Some(art);
        }

        // Armor sheets: reserved high (rows 4–11), each addressed by its origin
        // like the skin, so worn pieces sample them with the same face rects.
        for kind in armor::ALL {
            for (tile, art) in armor::atlas_tiles(kind) {
                reg.used[tile as usize] = true;
                reg.pixels[tile as usize] = Some(art);
            }
        }

        // Mob skin sheets: same scheme, filling out the high band (rows 4–15).
        for kind in mobskin::ALL {
            for (tile, art) in mobskin::atlas_tiles(kind) {
                reg.used[tile as usize] = true;
                reg.pixels[tile as usize] = Some(art);
            }
        }

        // Water animation: the shader steps `tile + frame` horizontally from
        // WATER_0, so the frames keep their fixed contiguous slots. The name
        // is resolvable so blocks.toml can reference it.
        for frame in 0..tiles::WATER_FRAMES {
            reg.claim_fixed(tiles::WATER_0 + frame);
        }
        reg.by_name.insert(
            "water".into(),
            TileEntry {
                tile: tiles::WATER_0,
                frames: tiles::WATER_FRAMES,
            },
        );
        reg.apply_png_overrides(
            "water",
            TileEntry {
                tile: tiles::WATER_0,
                frames: tiles::WATER_FRAMES,
            },
        );
        reg
    }

    fn claim_fixed(&mut self, tile: u32) {
        self.used[tile as usize] = true;
        self.pixels[tile as usize] = tiles::paint(tile);
    }

    /// Resolve `name` to its tile entry, assigning a slot and loading pixels
    /// on first use. Never fails: unknown names resolve to the missing marker
    /// (with a warning), as does an exhausted atlas.
    pub fn resolve(&mut self, name: &str) -> TileEntry {
        if let Some(&entry) = self.by_name.get(name) {
            return entry;
        }
        let art = load_png(name, None).or_else(|| tiles::paint_named(name));
        let entry = match art {
            Some(art) => match self.allocate() {
                Some(tile) => {
                    self.pixels[tile as usize] = Some(art);
                    TileEntry { tile, frames: 1 }
                }
                None => {
                    log::warn!(
                        "texture atlas full ({MAX_TILES} tiles); {name:?} gets the missing marker"
                    );
                    TileEntry::MISSING
                }
            },
            None => {
                log::warn!(
                    "unknown texture {name:?} (no assets/textures/{name}.png and no builtin art)"
                );
                TileEntry::MISSING
            }
        };
        self.by_name.insert(name.to_string(), entry);
        entry
    }

    /// Register `art` under `name`, allocating a slot on first use.
    ///
    /// For art with no PNG or painter of its own: the small stand-ins derived
    /// from a Blockbench block model's 256-pixel textures, which the inventory
    /// icon and the dropped-item cube still sample. Keyed by name exactly like
    /// [`TileRegistry::resolve`], so faces sharing a texture share a tile.
    pub fn insert(&mut self, name: &str, art: TileRgba) -> TileEntry {
        if let Some(&entry) = self.by_name.get(name) {
            return entry;
        }
        let entry = match self.allocate() {
            Some(tile) => {
                self.pixels[tile as usize] = Some(art);
                TileEntry { tile, frames: 1 }
            }
            None => {
                log::warn!(
                    "texture atlas full ({MAX_TILES} tiles); {name:?} gets the missing marker"
                );
                TileEntry::MISSING
            }
        };
        self.by_name.insert(name.to_string(), entry);
        entry
    }

    /// Replace a pre-registered animated entry's frames with PNGs from disk
    /// where present (`<name>_<frame>.png`). Static names load their PNG in
    /// [`TileRegistry::resolve`] directly.
    fn apply_png_overrides(&mut self, name: &str, entry: TileEntry) {
        for frame in 0..entry.frames {
            if let Some(art) = load_png(name, Some(frame)) {
                self.pixels[(entry.tile + frame) as usize] = Some(art);
            }
        }
    }

    fn allocate(&mut self) -> Option<u32> {
        let free = self.used.iter().position(|used| !used)?;
        self.used[free] = true;
        Some(free as u32)
    }

    /// Generate the atlas as tightly packed RGBA8 pixels
    /// (`ATLAS_SIZE^2 * 4` bytes) for the GPU upload.
    pub fn atlas_rgba(&self) -> Vec<u8> {
        let size = ATLAS_SIZE as usize;
        let mut out = vec![0u8; size * size * 4];
        for (tile, art) in self.pixels.iter().enumerate() {
            let tx = tile as u32 % ATLAS_COLUMNS;
            let ty = tile as u32 / ATLAS_COLUMNS;
            for py in 0..TILE_SIZE {
                for px in 0..TILE_SIZE {
                    let rgba = match art {
                        Some(t) => t[py as usize][px as usize],
                        None => MISSING_TEXTURE,
                    };
                    let ax = (tx * TILE_SIZE + px) as usize;
                    let ay = (ty * TILE_SIZE + py) as usize;
                    out[(ay * size + ax) * 4..][..4].copy_from_slice(&rgba);
                }
            }
        }
        out
    }
}

/// Path of a texture override on disk (CWD-relative, like all assets).
fn png_path(name: &str, frame: Option<u32>) -> String {
    match frame {
        Some(frame) => format!("assets/textures/{name}_{frame}.png"),
        None => format!("assets/textures/{name}.png"),
    }
}

/// Decode a 16×16 texture PNG (RGBA or RGB, 8-bit). Anything else warns and
/// falls through to the procedural art.
fn load_png(name: &str, frame: Option<u32>) -> Option<TileRgba> {
    let path = png_path(name, frame);
    let bytes = std::fs::read(&path).ok()?;
    let decode = || -> Result<TileRgba, String> {
        let image = super::texture::decode_png(&bytes)?;
        if image.size != [TILE_SIZE; 2] {
            return Err(format!(
                "must be {TILE_SIZE}x{TILE_SIZE}, got {}x{}",
                image.width(),
                image.height()
            ));
        }
        let mut art: TileRgba = [[[0; 4]; TILE_SIZE as usize]; TILE_SIZE as usize];
        for (y, row) in art.iter_mut().enumerate() {
            for (x, px) in row.iter_mut().enumerate() {
                let i = (y * TILE_SIZE as usize + x) * 4;
                *px = [
                    image.pixels[i],
                    image.pixels[i + 1],
                    image.pixels[i + 2],
                    image.pixels[i + 3],
                ];
            }
        }
        Ok(art)
    };
    match decode() {
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn engine_tiles_keep_fixed_indices() {
        let mut reg = TileRegistry::with_engine_tiles();
        let water = reg.resolve("water");
        assert_eq!(water.tile, tiles::WATER_0);
        assert_eq!(water.frames, tiles::WATER_FRAMES);
        assert!(water.is_animated());
    }

    #[test]
    fn content_tiles_allocate_and_stay_stable() {
        let mut reg = TileRegistry::with_engine_tiles();
        let stone = reg.resolve("stone");
        assert!(!stone.is_animated());
        assert_ne!(stone, TileEntry::MISSING);
        // Same name resolves to the same slot.
        assert_eq!(reg.resolve("stone"), stone);
        // Different names get different slots.
        assert_ne!(reg.resolve("dirt").tile, stone.tile);
        // Content never lands on an engine slot.
        for name in ["stone", "dirt", "leaves", "glass"] {
            let t = reg.resolve(name).tile;
            assert!(!skin::atlas_tile_indices().any(|s| s == t));
            assert!(!armor::is_armor_tile(t), "{name} landed in the armor band");
            assert!(!mobskin::is_mob_tile(t), "{name} landed in a mob sheet");
            assert!(!(tiles::WATER_0..tiles::WATER_0 + tiles::WATER_FRAMES).contains(&t));
            assert!(!(tiles::CRACK_0..tiles::CRACK_0 + tiles::CRACK_STAGES).contains(&t));
            assert_ne!(t, 0);
        }
    }

    #[test]
    fn unknown_names_resolve_to_missing() {
        let mut reg = TileRegistry::with_engine_tiles();
        assert_eq!(reg.resolve("no such texture"), TileEntry::MISSING);
    }

    #[test]
    fn atlas_has_expected_size() {
        let reg = TileRegistry::with_engine_tiles();
        assert_eq!(
            reg.atlas_rgba().len(),
            (ATLAS_SIZE * ATLAS_SIZE * 4) as usize
        );
    }
}

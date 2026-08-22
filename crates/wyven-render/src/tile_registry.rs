//! Name-keyed atlas tile registry.
//!
//! The registry does two jobs: hand each texture *name* a slot in the atlas, and
//! assemble the finished pixels for upload. It deliberately does **not** know
//! what any name means or where its art comes from — that is a [`TileSource`],
//! injected at construction. A voxel game's grass and a flight sim's instrument
//! panel are the same problem to this file.
//!
//! Two kinds of art share the atlas:
//!
//! - **Named** art, allocated on demand through [`TileRegistry::resolve`]. The
//!   caller references it by name and never learns the index.
//! - **Reserved** art, pinned to a fixed index by the caller
//!   ([`ReservedTiles`]). Anything addressed by *constant* rather than by name
//!   needs this — a sprite sheet whose parts are read at known offsets, or an
//!   overlay a shader indexes arithmetically. Reserved slots are claimed before
//!   any name is resolved, so named art can never land on one.

use std::collections::HashMap;

use super::block_textures::upscale;
use super::texture::{ATLAS_COLUMNS, ATLAS_SIZE, TILE_SIZE};
use wyven_assets::decode_png;

/// Total atlas capacity; growth requires touching `ATLAS_COLUMNS` in the
/// fragment shader too, so it is fixed for now.
const MAX_TILES: usize = (ATLAS_COLUMNS * ATLAS_COLUMNS) as usize;

/// The magenta marker painted into any atlas tile without assigned art, so a
/// bad tile index is immediately visible in-game.
///
/// Public so a caller assembling *reserved* art — which never goes through
/// [`TileSource`] and so can't fall back to the registry's own marker — can
/// paint the same "nothing here yet" colour.
pub const MISSING_TEXTURE: [u8; 4] = [255, 0, 255, 255];

const N: usize = TILE_SIZE as usize;

/// One tile of RGBA pixels, indexed `[y][x]` with y = 0 at the top.
pub type TileRgba = [[[u8; 4]; N]; N];

/// A resolved texture: the atlas tile it was assigned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TileEntry {
    pub tile: u32,
}

impl TileEntry {
    /// The missing-texture marker (tile 0 is reserved for it).
    pub const MISSING: TileEntry = TileEntry { tile: 0 };
}

/// Where a named tile's art comes from.
///
/// This is the seam that keeps the renderer free of any particular game's
/// artwork. The registry asks for a name; what it gets back — a PNG off disk,
/// procedurally painted pixels, or nothing — is entirely the implementor's
/// business.
pub trait TileSource: Send + Sync {
    /// Art for `name`, or `None` if this source has none (the registry then
    /// logs and hands out [`TileEntry::MISSING`]).
    fn tile(&self, name: &str) -> Option<TileRgba>;
}

/// A source with no art at all: every name resolves to the missing marker.
/// Useful for tests, and for any atlas built purely from reserved art.
pub struct NoTiles;

impl TileSource for NoTiles {
    fn tile(&self, _name: &str) -> Option<TileRgba> {
        None
    }
}

/// Art pinned to fixed atlas indices, claimed before any name is allocated.
#[derive(Default)]
pub struct ReservedTiles {
    entries: Vec<(u32, TileRgba)>,
}

impl ReservedTiles {
    pub fn new() -> Self {
        Self::default()
    }

    /// Pin `art` at `tile`.
    pub fn with(mut self, tile: u32, art: TileRgba) -> Self {
        self.entries.push((tile, art));
        self
    }

    /// Pin a whole sheet's worth of `(index, art)` pairs.
    pub fn extend(mut self, art: impl IntoIterator<Item = (u32, TileRgba)>) -> Self {
        self.entries.extend(art);
        self
    }
}

/// Decode a tile-sized texture PNG (RGBA or RGB, 8-bit) into atlas art.
///
/// Exposed because a [`TileSource`] reading PNGs off disk should not have to
/// re-derive "which PNG flavours do we accept, and at what size".
///
/// The art must be square, and either exactly [`TILE_SIZE`] or an exact integer
/// fraction of it — smaller art is replicated up, which is pixel-identical on
/// screen under the atlas's nearest sampling. The same rule the block texture
/// array applies at 256px, so one PNG can serve either store.
pub fn decode_tile(bytes: &[u8]) -> Result<TileRgba, String> {
    let decoded = decode_png(bytes)?;
    let [width, height] = decoded.size;
    if width != height {
        return Err(format!("must be square, got {width}x{height}"));
    }
    let image = if width == TILE_SIZE {
        decoded
    } else if width == 0 || !TILE_SIZE.is_multiple_of(width) {
        return Err(format!(
            "must be {TILE_SIZE}x{TILE_SIZE} or an exact fraction of it, got {width}x{height}"
        ));
    } else {
        upscale(&decoded, TILE_SIZE / width)
    };
    let mut art: TileRgba = [[[0; 4]; N]; N];
    for (y, row) in art.iter_mut().enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (y * N + x) * 4;
            *px = [
                image.pixels[i],
                image.pixels[i + 1],
                image.pixels[i + 2],
                image.pixels[i + 3],
            ];
        }
    }
    Ok(art)
}

/// Name → tile assignment plus the CPU-side atlas pixels.
pub struct TileRegistry {
    by_name: HashMap<String, TileEntry>,
    pixels: Vec<Option<TileRgba>>,
    used: [bool; MAX_TILES],
    source: Box<dyn TileSource>,
}

impl TileRegistry {
    /// A registry that draws named art from `source`, with `reserved` already
    /// claimed at its fixed indices.
    pub fn new(source: Box<dyn TileSource>, reserved: &ReservedTiles) -> Self {
        let mut reg = Self {
            by_name: HashMap::new(),
            pixels: vec![None; MAX_TILES],
            used: [false; MAX_TILES],
            source,
        };
        // Tile 0 is the reserved missing-texture marker.
        reg.used[0] = true;
        for &(tile, art) in &reserved.entries {
            reg.used[tile as usize] = true;
            reg.pixels[tile as usize] = Some(art);
        }
        reg
    }

    /// A registry with no art at all — every name resolves to the missing
    /// marker. For tests that only care about slot allocation.
    pub fn empty() -> Self {
        Self::new(Box::new(NoTiles), &ReservedTiles::new())
    }

    /// Resolve `name` to its tile entry, assigning a slot and loading pixels
    /// on first use. Never fails: unknown names resolve to the missing marker
    /// (with a warning), as does an exhausted atlas.
    pub fn resolve(&mut self, name: &str) -> TileEntry {
        if let Some(&entry) = self.by_name.get(name) {
            return entry;
        }
        let entry = match self.source.tile(name) {
            Some(art) => self.claim(name, art),
            None => {
                log::warn!("unknown texture {name:?}: no art from the tile source");
                TileEntry::MISSING
            }
        };
        self.by_name.insert(name.to_string(), entry);
        entry
    }

    /// Register `art` under `name`, allocating a slot on first use.
    ///
    /// For art the source has no answer for because it was *derived* at load
    /// time rather than authored — a small stand-in downsampled from a larger
    /// texture, say. Keyed by name exactly like [`TileRegistry::resolve`], so
    /// two callers naming the same art share a tile.
    pub fn insert(&mut self, name: &str, art: TileRgba) -> TileEntry {
        if let Some(&entry) = self.by_name.get(name) {
            return entry;
        }
        let entry = self.claim(name, art);
        self.by_name.insert(name.to_string(), entry);
        entry
    }

    /// Put `art` in the next free slot, or warn and fall back to the marker.
    fn claim(&mut self, name: &str, art: TileRgba) -> TileEntry {
        match self.allocate() {
            Some(tile) => {
                self.pixels[tile as usize] = Some(art);
                TileEntry { tile }
            }
            None => {
                log::warn!(
                    "texture atlas full ({MAX_TILES} tiles); {name:?} gets the missing marker"
                );
                TileEntry::MISSING
            }
        }
    }

    fn allocate(&mut self) -> Option<u32> {
        let free = self.used.iter().position(|used| !used)?;
        self.used[free] = true;
        Some(free as u32)
    }

    /// The pixels assigned to `tile`, or `None` for a slot with no art (which
    /// the atlas fills with the magenta marker).
    ///
    /// Exposed for geometry derived from the art itself — extruding a flat item
    /// icon needs to know where the drawn shape ends.
    pub fn art(&self, tile: u32) -> Option<&TileRgba> {
        self.pixels.get(tile as usize)?.as_ref()
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Serves a flat colour for any name starting with `known`.
    struct Swatch;

    impl TileSource for Swatch {
        fn tile(&self, name: &str) -> Option<TileRgba> {
            if !name.starts_with("known") {
                return None;
            }
            // Colour-code by name length so two names get distinguishable art.
            Some([[[name.len() as u8, 0, 0, 255]; N]; N])
        }
    }

    fn registry(reserved: ReservedTiles) -> TileRegistry {
        TileRegistry::new(Box::new(Swatch), &reserved)
    }

    /// A `w`×`h` PNG whose pixel at `(x, y)` encodes its own coordinates, so a
    /// scaling bug shows up as the wrong texel rather than the wrong colour.
    fn png(w: u32, h: u32) -> Vec<u8> {
        let mut data = Vec::with_capacity((w * h * 4) as usize);
        for y in 0..h {
            for x in 0..w {
                data.extend_from_slice(&[x as u8, y as u8, 0, 255]);
            }
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, w, h);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&data)
            .unwrap();
        out
    }

    #[test]
    fn tile_sized_art_is_taken_verbatim() {
        let art = decode_tile(&png(TILE_SIZE, TILE_SIZE)).expect("exact size");
        assert_eq!(art[0][0], [0, 0, 0, 255]);
        assert_eq!(art[N - 1][N - 1], [(N - 1) as u8, (N - 1) as u8, 0, 255]);
    }

    /// Smaller art is replicated up by an integer factor, so every source texel
    /// becomes a whole square block and nearest sampling shows it unchanged.
    #[test]
    fn smaller_art_upscales_into_whole_blocks() {
        let half = TILE_SIZE / 2;
        let art = decode_tile(&png(half, half)).expect("exact fraction");
        for (y, row) in art.iter().enumerate() {
            for (x, px) in row.iter().enumerate() {
                assert_eq!(
                    *px,
                    [(x / 2) as u8, (y / 2) as u8, 0, 255],
                    "texel ({x}, {y})"
                );
            }
        }
    }

    #[test]
    fn non_square_and_non_dividing_art_is_rejected() {
        let err = decode_tile(&png(TILE_SIZE, TILE_SIZE / 2)).unwrap_err();
        assert!(err.contains("square"), "{err}");
        let odd = TILE_SIZE + TILE_SIZE / 2;
        let err = decode_tile(&png(odd, odd)).unwrap_err();
        assert!(err.contains("fraction"), "{err}");
    }

    #[test]
    fn names_allocate_once_and_stay_stable() {
        let mut reg = registry(ReservedTiles::new());
        let a = reg.resolve("known a");
        assert_ne!(a, TileEntry::MISSING);
        assert_eq!(reg.resolve("known a"), a, "same name, same slot");
        assert_ne!(
            reg.resolve("known bb").tile,
            a.tile,
            "distinct names differ"
        );
    }

    #[test]
    fn unknown_names_resolve_to_missing() {
        let mut reg = registry(ReservedTiles::new());
        assert_eq!(reg.resolve("no such texture"), TileEntry::MISSING);
    }

    /// The point of reserving: art addressed by constant must keep its index,
    /// and no name may ever be handed that slot.
    #[test]
    fn named_art_never_lands_on_a_reserved_slot() {
        let taken = [3u32, 4, 5, 200];
        let reserved = taken
            .iter()
            .fold(ReservedTiles::new(), |r, &t| r.with(t, [[[1; 4]; N]; N]));
        let mut reg = registry(reserved);
        for i in 0..8 {
            let tile = reg.resolve(&format!("known {i}")).tile;
            assert!(!taken.contains(&tile), "name {i} landed on a reserved slot");
            assert_ne!(tile, 0, "name {i} landed on the missing marker's slot");
        }
    }

    #[test]
    fn insert_registers_art_the_source_has_no_answer_for() {
        let mut reg = registry(ReservedTiles::new());
        let derived = reg.insert("derived", [[[7; 4]; N]; N]);
        assert_ne!(derived, TileEntry::MISSING);
        assert_eq!(reg.insert("derived", [[[9; 4]; N]; N]), derived, "memoised");
    }

    #[test]
    fn atlas_has_expected_size() {
        let reg = registry(ReservedTiles::new());
        assert_eq!(
            reg.atlas_rgba().len(),
            (ATLAS_SIZE * ATLAS_SIZE * 4) as usize
        );
    }

    #[test]
    fn a_registry_with_no_source_resolves_everything_to_missing() {
        let mut reg = TileRegistry::empty();
        assert_eq!(reg.resolve("known a"), TileEntry::MISSING);
    }
}

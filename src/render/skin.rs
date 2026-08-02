//! Minecraft-format player skin (64×64 sheet) blitted into the block atlas.
//!
//! The sheet occupies a fixed 4×4-tile square block of the atlas so the entity
//! model can address it by pixel rect instead of whole tiles. Layout follows
//! the standard Minecraft skin unwrap: per part, a horizontal strip of
//! right|front|left|back side faces with top/bottom above it, plus a second
//! overlay layer (hat, jacket, sleeves, pants). The base and overlay regions are
//! both left in the atlas with their alpha intact; the model draws the overlay
//! as a slightly inflated second shell (its transparent pixels are alpha-tested
//! away in the shader) for a 3D layered look.
//!
//! `assets/textures/defaultskin.png` overrides the embedded copy; an invalid
//! or missing file degrades fail-soft with a logged warning.

use crate::core::Direction;

use super::texture::{ATLAS_COLUMNS, ATLAS_SIZE, TILE_SIZE};
use super::tiles::TileRgba;

/// Sheet edge length in pixels.
pub const SKIN_SIZE: u32 = 64;
/// Sheet edge length in atlas tiles.
const SKIN_TILES: u32 = SKIN_SIZE / TILE_SIZE;
/// Atlas tile column/row of the sheet's top-left tile (top-right corner of the
/// atlas, clear of the water and crack rows).
pub const SKIN_COL: u32 = ATLAS_COLUMNS - SKIN_TILES;
pub const SKIN_ROW: u32 = 0;

/// The player skin's atlas origin as an `[col, row]` tile pair.
pub const SKIN_ORIGIN: [u32; 2] = [SKIN_COL, SKIN_ROW];

const SKIN_PATH: &str = "assets/textures/defaultskin.png";
const EMBEDDED_SKIN: &[u8] = include_bytes!("../../assets/textures/defaultskin.png");

/// The whole skin sheet as RGBA pixels, indexed `[y][x]` with y = 0 at the top.
pub type SkinRgba = [[[u8; 4]; SKIN_SIZE as usize]; SKIN_SIZE as usize];

/// One body part's box texture: the unwrap origin on the sheet plus the box
/// pixel dimensions (width, height, depth). These match the model boxes in
/// [`crate::entity::model::HumanoidModel::player`].
#[derive(Debug, Clone, Copy)]
pub struct SkinPart {
    uv: [u32; 2],
    size: [u32; 3],
}

pub const HEAD: SkinPart = SkinPart {
    uv: [0, 0],
    size: [8, 8, 8],
};
pub const BODY: SkinPart = SkinPart {
    uv: [16, 16],
    size: [8, 12, 4],
};
pub const RIGHT_ARM: SkinPart = SkinPart {
    uv: [40, 16],
    size: [4, 12, 4],
};
pub const LEFT_ARM: SkinPart = SkinPart {
    uv: [32, 48],
    size: [4, 12, 4],
};
pub const RIGHT_LEG: SkinPart = SkinPart {
    uv: [0, 16],
    size: [4, 12, 4],
};
pub const LEFT_LEG: SkinPart = SkinPart {
    uv: [16, 48],
    size: [4, 12, 4],
};

// Overlay layer: the second, inflated shell. Same unwrap as each base part at a
// different sheet origin, so `face_rect`/`face_uv` apply unchanged.
pub const HAT: SkinPart = SkinPart {
    uv: [32, 0],
    size: [8, 8, 8],
};
pub const JACKET: SkinPart = SkinPart {
    uv: [16, 32],
    size: [8, 12, 4],
};
pub const RIGHT_SLEEVE: SkinPart = SkinPart {
    uv: [40, 32],
    size: [4, 12, 4],
};
pub const LEFT_SLEEVE: SkinPart = SkinPart {
    uv: [48, 48],
    size: [4, 12, 4],
};
pub const RIGHT_PANTS: SkinPart = SkinPart {
    uv: [0, 32],
    size: [4, 12, 4],
};
pub const LEFT_PANTS: SkinPart = SkinPart {
    uv: [0, 48],
    size: [4, 12, 4],
};

const PARTS: [SkinPart; 6] = [HEAD, BODY, RIGHT_ARM, LEFT_ARM, RIGHT_LEG, LEFT_LEG];

/// The cape: a thin, tall box hung off the shoulders. Not part of the standard
/// skin unwrap — it's used only by the cape armor sheet and its own model box.
pub const CAPE: SkinPart = SkinPart {
    uv: [0, 0],
    size: [10, 16, 1],
};

impl SkinPart {
    /// A part unwrap at `uv` for a box of `size` pixels (width, height,
    /// depth). Lets other sheets (mob skins) define their own layouts.
    pub const fn new(uv: [u32; 2], size: [u32; 3]) -> Self {
        Self { uv, size }
    }

    /// Pixel rect `(x, y, w, h)` on the sheet for the face pointing `dir`, in
    /// the model's frame (front = -Z, character's right = +X — see
    /// [`crate::entity::model`]).
    pub fn face_rect(&self, dir: Direction) -> [u32; 4] {
        let [u, v] = self.uv;
        let [w, h, d] = self.size;
        match dir {
            Direction::PosX => [u, v + d, d, h],         // character's right
            Direction::NegX => [u + d + w, v + d, d, h], // character's left
            Direction::PosY => [u + d, v, w, d],         // top
            Direction::NegY => [u + d + w, v, w, d],     // bottom
            Direction::NegZ => [u + d, v + d, w, h],     // front
            Direction::PosZ => [u + 2 * d + w, v + d, w, h], // back
        }
    }
}

/// Edge length of a 64×64 sheet in atlas tiles (skin and armor share it).
pub const SHEET_TILES: u32 = SKIN_SIZE / TILE_SIZE;

/// Map a face rect + local image uv (u → right, v → down) into atlas texture
/// coordinates for a sheet whose top-left tile is `origin_tile` (`[col, row]`).
/// Generic over the sheet position so armor sheets reuse it; the skin-space
/// counterpart of [`super::texture::atlas_uv`].
pub fn sheet_uv(origin_tile: [u32; 2], rect: [u32; 4], local: [f32; 2]) -> [f32; 2] {
    let size = ATLAS_SIZE as f32;
    let origin = [origin_tile[0] * TILE_SIZE, origin_tile[1] * TILE_SIZE];
    [
        (origin[0] as f32 + rect[0] as f32 + local[0] * rect[2] as f32) / size,
        (origin[1] as f32 + rect[1] as f32 + local[1] * rect[3] as f32) / size,
    ]
}

/// [`sheet_uv`] for the player skin's fixed atlas block.
pub fn face_uv(rect: [u32; 4], local: [f32; 2]) -> [f32; 2] {
    sheet_uv([SKIN_COL, SKIN_ROW], rect, local)
}

/// The atlas tile indices reserved for the player skin (a `SHEET_TILES`² block).
pub fn atlas_tile_indices() -> impl Iterator<Item = u32> {
    tile_indices_at([SKIN_COL, SKIN_ROW])
}

/// The atlas tile indices of a `SHEET_TILES`² block at `origin_tile`.
pub fn tile_indices_at(origin_tile: [u32; 2]) -> impl Iterator<Item = u32> {
    let [col, row] = origin_tile;
    (0..SHEET_TILES * SHEET_TILES)
        .map(move |i| (row + i / SHEET_TILES) * ATLAS_COLUMNS + col + i % SHEET_TILES)
}

/// A 64×64 sheet sliced into the atlas tiles of the block at `origin_tile`, as
/// `(tile index, tile pixels)` pairs.
pub fn atlas_tiles_at(
    sheet: &SkinRgba,
    origin_tile: [u32; 2],
) -> impl Iterator<Item = (u32, TileRgba)> + '_ {
    tile_indices_at(origin_tile).enumerate().map(|(i, tile)| {
        let (tx, ty) = (i as u32 % SHEET_TILES, i as u32 / SHEET_TILES);
        let art = std::array::from_fn(|y| {
            std::array::from_fn(|x| {
                sheet[(ty * TILE_SIZE) as usize + y][(tx * TILE_SIZE) as usize + x]
            })
        });
        (tile, art)
    })
}

/// The player skin sliced into its reserved atlas tiles.
pub fn atlas_tiles(skin: &SkinRgba) -> impl Iterator<Item = (u32, TileRgba)> + '_ {
    atlas_tiles_at(skin, [SKIN_COL, SKIN_ROW])
}

/// Load the default player skin: the PNG on disk if present and valid, else
/// the embedded copy. The base face rects are forced opaque (matching how
/// Minecraft treats the base layer); overlay regions keep their alpha for the
/// separate inflated overlay shell the model draws.
pub fn load_default() -> Box<SkinRgba> {
    let mut skin = match std::fs::read(SKIN_PATH) {
        Ok(bytes) => match decode(&bytes) {
            Ok(skin) => {
                log::info!("player skin: using {SKIN_PATH}");
                skin
            }
            Err(err) => {
                log::warn!("ignoring {SKIN_PATH}: {err}; using the embedded default skin");
                embedded()
            }
        },
        Err(_) => embedded(),
    };
    force_base_opaque(&mut skin);
    skin
}

fn embedded() -> Box<SkinRgba> {
    decode(EMBEDDED_SKIN).expect("embedded default skin decodes")
}

/// Decode a 64×64 skin/armor PNG. The size is the only extra constraint on top
/// of [`super::texture::decode_png`]: the part rects below index into a sheet of
/// exactly this shape.
pub fn decode(bytes: &[u8]) -> Result<Box<SkinRgba>, String> {
    let image = super::texture::decode_png(bytes)?;
    if image.size != [SKIN_SIZE; 2] {
        return Err(format!(
            "must be {SKIN_SIZE}x{SKIN_SIZE}, got {}x{}",
            image.width(),
            image.height()
        ));
    }
    let mut skin = Box::new([[[0u8; 4]; SKIN_SIZE as usize]; SKIN_SIZE as usize]);
    for (y, row) in skin.iter_mut().enumerate() {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (y * SKIN_SIZE as usize + x) * 4;
            *px = [
                image.pixels[i],
                image.pixels[i + 1],
                image.pixels[i + 2],
                image.pixels[i + 3],
            ];
        }
    }
    Ok(skin)
}

/// Force the base layer's face rects fully opaque, so the inner body never
/// renders see-through (matching how Minecraft treats the base layer). The
/// overlay regions are left untouched — their alpha drives the 3D overlay shell.
fn force_base_opaque(skin: &mut SkinRgba) {
    for part in PARTS {
        for dir in Direction::ALL {
            let [x, y, w, h] = part.face_rect(dir);
            for yy in y..y + h {
                for xx in x..x + w {
                    skin[yy as usize][xx as usize][3] = 255;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn face_rects_follow_the_minecraft_unwrap() {
        assert_eq!(HEAD.face_rect(Direction::NegZ), [8, 8, 8, 8]); // front
        assert_eq!(HEAD.face_rect(Direction::PosY), [8, 0, 8, 8]); // top
        assert_eq!(HEAD.face_rect(Direction::PosX), [0, 8, 8, 8]); // character's right
        assert_eq!(BODY.face_rect(Direction::PosZ), [32, 20, 8, 12]); // back
        assert_eq!(RIGHT_ARM.face_rect(Direction::PosY), [44, 16, 4, 4]);
        assert_eq!(LEFT_LEG.face_rect(Direction::NegX), [24, 52, 4, 12]);
    }

    #[test]
    fn face_uv_lands_in_the_skin_atlas_block() {
        let rect = HEAD.face_rect(Direction::NegZ);
        // Sheet origin is (192, 0); the head front rect starts 8px in.
        assert_eq!(face_uv(rect, [0.0, 0.0]), [200.0 / 256.0, 8.0 / 256.0]);
        assert_eq!(face_uv(rect, [1.0, 1.0]), [208.0 / 256.0, 16.0 / 256.0]);
    }

    #[test]
    fn atlas_tiles_cover_a_square_block_off_the_engine_rows() {
        let skin = load_default();
        let tiles: Vec<u32> = atlas_tiles(&skin).map(|(t, _)| t).collect();
        assert_eq!(tiles.len(), (SKIN_TILES * SKIN_TILES) as usize);
        for t in &tiles {
            assert!(t % ATLAS_COLUMNS >= SKIN_COL);
            assert!(t / ATLAS_COLUMNS < SKIN_ROW + SKIN_TILES);
        }
        // Tile pixels round-trip to sheet pixels: first tile's first pixel.
        let (_, art) = atlas_tiles(&skin).next().unwrap();
        assert_eq!(art[0][0], skin[0][0]);
    }

    #[test]
    fn default_skin_base_faces_are_opaque() {
        let skin = load_default();
        for part in PARTS {
            for dir in Direction::ALL {
                let [x, y, w, h] = part.face_rect(dir);
                for yy in y..y + h {
                    for xx in x..x + w {
                        assert_eq!(skin[yy as usize][xx as usize][3], 255, "({xx},{yy})");
                    }
                }
            }
        }
    }

    #[test]
    fn embedded_skin_decodes() {
        let skin = embedded();
        assert_eq!(skin.len(), SKIN_SIZE as usize);
    }
}

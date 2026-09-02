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
//! `assets/textures/entity/defaultskin.png` overrides the embedded copy; an invalid
//! or missing file degrades fail-soft with a logged warning.

use crate::core::Direction;

use wyven_render::TileRgba;
use wyven_render::texture::{ATLAS_COLUMNS, ATLAS_SIZE, TILE_SIZE};

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

const SKIN_PATH: &str = "assets/textures/entity/defaultskin.png";
const EMBEDDED_SKIN: &[u8] = include_bytes!("../../assets/textures/entity/defaultskin.png");

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

/// How many whole sheets fit across one atlas row-band.
pub const SHEETS_PER_ROW: u32 = ATLAS_COLUMNS / SHEET_TILES;

/// The atlas origin of the `index`th sheet of a band starting at `base_row`,
/// laid out left to right and wrapping onto the next `SHEET_TILES` rows.
///
/// Armor and mob sheets both address the atlas this way rather than with a
/// hand-written table, so a change to [`TILE_SIZE`] — which changes how many
/// tiles a 64×64 sheet spans — cannot silently make two sheets overlap.
pub fn sheet_origin(base_row: u32, index: u32) -> [u32; 2] {
    [
        (index % SHEETS_PER_ROW) * SHEET_TILES,
        base_row + (index / SHEETS_PER_ROW) * SHEET_TILES,
    ]
}

/// How many atlas rows a band of `count` sheets occupies, so the next band
/// knows where it may start.
pub fn band_rows(count: u32) -> u32 {
    count.div_ceil(SHEETS_PER_ROW) * SHEET_TILES
}

/// A sheet with no art: every texel the magenta missing-texture marker.
///
/// Reserved sheets never reach the registry's own marker — they are pinned
/// pixels, not a name it failed to resolve — so a piece whose PNG has not been
/// drawn yet paints this instead, and reads the same way in-game.
pub fn missing_sheet() -> Box<SkinRgba> {
    Box::new([[wyven_render::MISSING_TEXTURE; SKIN_SIZE as usize]; SKIN_SIZE as usize])
}

/// Map a face rect + local image uv (u → right, v → down) into atlas texture
/// coordinates for a sheet whose top-left tile is `origin_tile` (`[col, row]`).
/// Generic over the sheet position so armor sheets reuse it; the skin-space
/// counterpart of [`wyven_render::texture::atlas_uv`].
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
    match std::fs::read(SKIN_PATH) {
        Ok(bytes) => match decode_humanoid(&bytes) {
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
    }
}

fn embedded() -> Box<SkinRgba> {
    decode_humanoid(EMBEDDED_SKIN).expect("embedded default skin decodes")
}

/// Decode a skin/armor/mob PNG into a 64×64 sheet.
///
/// Two shapes are accepted, because that is what mob art comes in: the modern
/// 64×64 sheet, and the pre-1.8 64×32 half-sheet, which is padded out with
/// transparency. A half-height sheet has no separate left limbs and no overlay
/// layer — see [`mirror_legacy_limbs`], which the humanoid loader applies so the
/// model never has to know which it got.
pub fn decode(bytes: &[u8]) -> Result<Box<SkinRgba>, String> {
    let image = wyven_render::texture::decode_png(bytes)?;
    let [width, height] = image.size;
    if width != SKIN_SIZE || (height != SKIN_SIZE && height != SKIN_SIZE / 2) {
        return Err(format!(
            "must be {SKIN_SIZE}x{SKIN_SIZE} or {SKIN_SIZE}x{}, got {width}x{height}",
            SKIN_SIZE / 2
        ));
    }
    let mut skin = Box::new([[[0u8; 4]; SKIN_SIZE as usize]; SKIN_SIZE as usize]);
    for (y, row) in skin.iter_mut().enumerate().take(height as usize) {
        for (x, px) in row.iter_mut().enumerate() {
            let i = (y * width as usize + x) * 4;
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

/// Whether `sheet` uses the pre-1.8 layout: everything in the top half, with no
/// separate left arm or leg and no overlay.
///
/// Detected rather than declared, because a 64×32 skin padded out to 64×64 is
/// indistinguishable from one that was authored that way — and both need the
/// same fixing up.
pub fn is_legacy(sheet: &SkinRgba) -> bool {
    sheet[SKIN_SIZE as usize / 2..]
        .iter()
        .all(|row| row.iter().all(|px| px[3] == 0))
}

/// Fill a legacy sheet's empty left arm and leg from its right ones, mirrored.
///
/// Pre-1.8 skins drew both sides from one unwrap; read with the modern part
/// rects those slots are empty, which [`force_base_opaque`] would then turn into
/// solid black limbs. Mirroring is what the old renderer did implicitly, so this
/// reproduces the intended look rather than inventing one.
fn mirror_legacy_limbs(sheet: &mut SkinRgba) {
    for (right, left) in [(RIGHT_ARM, LEFT_ARM), (RIGHT_LEG, LEFT_LEG)] {
        for dir in Direction::ALL {
            // Mirroring in X swaps the two side faces and flips every face's
            // horizontal run; the rest of the unwrap maps straight across.
            let source = match dir {
                Direction::PosX => Direction::NegX,
                Direction::NegX => Direction::PosX,
                other => other,
            };
            let [sx, sy, w, h] = right.face_rect(source);
            let [dx, dy, dw, dh] = left.face_rect(dir);
            debug_assert_eq!([w, h], [dw, dh], "mirrored faces must match in size");
            for y in 0..h {
                for x in 0..w {
                    let px = sheet[(sy + y) as usize][(sx + w - 1 - x) as usize];
                    sheet[(dy + y) as usize][(dx + x) as usize] = px;
                }
            }
        }
    }
}

/// [`decode`], then whatever fixing up the sheet's own layout calls for.
pub fn decode_humanoid(bytes: &[u8]) -> Result<Box<SkinRgba>, String> {
    let mut sheet = decode(bytes)?;
    if is_legacy(&sheet) {
        mirror_legacy_limbs(&mut sheet);
    }
    force_base_opaque(&mut sheet);
    Ok(sheet)
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

    /// A pre-1.8 sheet has nothing in its lower half, so the modern left-limb
    /// rects read as empty — which `force_base_opaque` would then turn into
    /// solid black limbs. They must come from the right ones instead.
    #[test]
    fn a_legacy_sheet_gets_its_left_limbs_from_its_right_ones() {
        let mut sheet = Box::new([[[0u8; 4]; SKIN_SIZE as usize]; SKIN_SIZE as usize]);
        // Paint the right arm's unwrap with a per-column marker, top half only.
        let [ax, ay, aw, ah] = RIGHT_ARM.face_rect(Direction::PosX);
        for y in ay..ay + ah {
            for x in ax..ax + aw {
                sheet[y as usize][x as usize] = [x as u8, 0, 0, 255];
            }
        }
        assert!(is_legacy(&sheet), "nothing below the halfway line");

        mirror_legacy_limbs(&mut sheet);

        // The right arm's outward face lands on the left arm's *inward* one —
        // that is what mirroring means — and reversed along its run.
        let [bx, by, bw, _] = LEFT_ARM.face_rect(Direction::NegX);
        for i in 0..bw {
            let got = sheet[by as usize][(bx + i) as usize];
            assert_eq!(got, [(ax + aw - 1 - i) as u8, 0, 0, 255], "column {i}");
        }
        assert!(!is_legacy(&sheet), "the lower half is now populated");
    }

    /// The half-height sheets mob art ships as are padded, not rejected.
    #[test]
    fn a_half_height_sheet_is_accepted() {
        let mut png = Vec::new();
        {
            let mut enc = png::Encoder::new(&mut png, SKIN_SIZE, SKIN_SIZE / 2);
            enc.set_color(png::ColorType::Rgba);
            enc.set_depth(png::BitDepth::Eight);
            enc.write_header()
                .unwrap()
                .write_image_data(&vec![255u8; (SKIN_SIZE * SKIN_SIZE / 2 * 4) as usize])
                .unwrap();
        }
        let sheet = decode(&png).expect("a 64x32 sheet is valid mob art");
        assert_eq!(sheet[0][0][3], 255, "the drawn half survives");
        assert_eq!(
            sheet[SKIN_SIZE as usize - 1][0][3],
            0,
            "the rest is padding"
        );
    }

    #[test]
    fn face_uv_lands_in_the_skin_atlas_block() {
        let rect = HEAD.face_rect(Direction::NegZ);
        // The head's front rect starts 8px into the sheet and is 8px square,
        // wherever in the atlas the sheet itself happens to sit.
        let origin = (SKIN_COL * TILE_SIZE) as f32;
        let atlas = ATLAS_SIZE as f32;
        assert_eq!(
            face_uv(rect, [0.0, 0.0]),
            [(origin + 8.0) / atlas, 8.0 / atlas]
        );
        assert_eq!(
            face_uv(rect, [1.0, 1.0]),
            [(origin + 16.0) / atlas, 16.0 / atlas]
        );
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

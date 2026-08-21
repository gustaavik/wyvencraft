//! Procedural mob skin sheets blitted into the block atlas.
//!
//! Each mob skin is a 64×64 sheet like the player skin and the armor sheets
//! ([`super::skin`], [`super::armor`]). Humanoid mobs (zombie, skeleton) use
//! the standard Minecraft unwrap so [`crate::entity::HumanoidModel`] renders
//! them by only switching the sheet's atlas origin; quadrupeds (cow, sheep)
//! use this module's own unwrap ([`Q_HEAD`]/[`Q_BODY`]/[`Q_LEG`]) sized for a
//! four-legged body. Sheets are painted procedurally and overridable by
//! `assets/textures/mob_<name>.png`. They occupy atlas rows 4–15 alongside
//! the armor band, clear of the dynamically allocated content tiles.
//!
//! Entity kinds reference skins by name (`[entity.visual] skin = "cow"`);
//! [`origin_for`] is the lookup the state layer uses when building meshes.

use crate::core::{Direction, Rng64};

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
}

pub const ALL: [MobSkin; 4] = [
    MobSkin::Zombie,
    MobSkin::Skeleton,
    MobSkin::Cow,
    MobSkin::Sheep,
];

impl MobSkin {
    /// The atlas tile `[col, row]` of this skin's 64×64 sheet.
    pub fn origin(self) -> [u32; 2] {
        match self {
            MobSkin::Zombie => [12, 4],
            MobSkin::Skeleton => [12, 8],
            MobSkin::Cow => [0, 12],
            MobSkin::Sheep => [4, 12],
        }
    }

    /// The name entity kinds reference (and the PNG override stem).
    fn name(self) -> &'static str {
        match self {
            MobSkin::Zombie => "zombie",
            MobSkin::Skeleton => "skeleton",
            MobSkin::Cow => "cow",
            MobSkin::Sheep => "sheep",
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

/// Load a skin's sheet: the PNG override if present and valid, else painted.
fn load(kind: MobSkin) -> Box<skin::SkinRgba> {
    let path = format!("assets/textures/mob_{}.png", kind.name());
    match std::fs::read(&path) {
        Ok(bytes) => match skin::decode(&bytes) {
            Ok(sheet) => {
                log::info!("mob skin {}: using {path}", kind.name());
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

fn paint(kind: MobSkin) -> Box<skin::SkinRgba> {
    match kind {
        MobSkin::Zombie => paint_humanoid(
            [92, 140, 80], // skin green
            [58, 92, 108], // ragged shirt
            [64, 62, 88],  // trousers
            [24, 32, 22],  // sunken eyes
            None,
        ),
        MobSkin::Skeleton => paint_humanoid(
            [224, 222, 210], // bone
            [224, 222, 210],
            [224, 222, 210],
            [38, 38, 40],          // eye sockets
            Some([180, 178, 166]), // rib shading stripes
        ),
        MobSkin::Cow => paint_quadruped(
            [96, 64, 46],    // hide brown
            [230, 226, 218], // patch white
            true,
        ),
        MobSkin::Sheep => paint_quadruped(
            [232, 230, 224], // wool
            [178, 174, 168], // face/leg gray
            false,
        ),
    }
}

/// Paint a humanoid sheet on the standard skin unwrap: `skin` colours the
/// head, hands, and lower legs; `shirt` the torso and upper arms; `pants` the
/// upper legs. `eye` dots the head's front face; `ribs` (skeleton) stripes
/// the torso.
fn paint_humanoid(
    skin_c: [u8; 3],
    shirt: [u8; 3],
    pants: [u8; 3],
    eye: [u8; 3],
    ribs: Option<[u8; 3]>,
) -> Box<skin::SkinRgba> {
    let mut sheet = blank();
    fill_part(&mut sheet, skin::HEAD, skin_c, 0.0, 1.0);
    fill_part(&mut sheet, skin::BODY, shirt, 0.0, 1.0);
    for arm in [skin::LEFT_ARM, skin::RIGHT_ARM] {
        fill_part(&mut sheet, arm, shirt, 0.0, 0.6);
        fill_part(&mut sheet, arm, skin_c, 0.6, 1.0);
    }
    for leg in [skin::LEFT_LEG, skin::RIGHT_LEG] {
        fill_part(&mut sheet, leg, pants, 0.0, 0.7);
        fill_part(&mut sheet, leg, skin_c, 0.7, 1.0);
    }

    // Eyes on the head's front face (an 8×8 rect): two 2×1 dots at brow level.
    let [fx, fy, ..] = skin::HEAD.face_rect(Direction::NegZ);
    for (dx, dy) in [(1, 4), (2, 4), (5, 4), (6, 4)] {
        put(&mut sheet, fx + dx, fy + dy, [eye[0], eye[1], eye[2], 255]);
    }

    // Rib stripes across the torso front (skeleton).
    if let Some(rib) = ribs {
        let [bx, by, bw, bh] = skin::BODY.face_rect(Direction::NegZ);
        for row in (2..bh - 2).step_by(3) {
            for xx in bx + 1..bx + bw - 1 {
                put(&mut sheet, xx, by + row, [rib[0], rib[1], rib[2], 255]);
            }
        }
    }
    sheet
}

/// Paint a quadruped sheet: base hide with an `accent` muzzle and lower legs;
/// `patches` scatters seeded accent blotches over the body (the cow's spots).
fn paint_quadruped(hide: [u8; 3], accent: [u8; 3], patches: bool) -> Box<skin::SkinRgba> {
    let mut sheet = blank();
    fill_part(&mut sheet, Q_BODY, hide, 0.0, 1.0);
    fill_part(&mut sheet, Q_HEAD, hide, 0.0, 0.7);
    fill_part(&mut sheet, Q_HEAD, accent, 0.7, 1.0); // muzzle band
    fill_part(&mut sheet, Q_LEG, hide, 0.0, 0.6);
    fill_part(&mut sheet, Q_LEG, accent, 0.6, 1.0); // socks

    // Eyes on the head's front face (8×8): dots above the muzzle band.
    let [fx, fy, ..] = Q_HEAD.face_rect(Direction::NegZ);
    for (dx, dy) in [(1, 3), (6, 3)] {
        put(&mut sheet, fx + dx, fy + dy, [30, 26, 24, 255]);
    }

    if patches {
        // Seeded blotches over every body face, deterministic across runs.
        let mut rng = Rng64::new(0xC0B0_51DE);
        for dir in Direction::ALL {
            let [x, y, w, h] = Q_BODY.face_rect(dir);
            for _ in 0..3 {
                let px = x + rng.range_u32(0, w.saturating_sub(4));
                let py = y + rng.range_u32(0, h.saturating_sub(3));
                for yy in py..(py + 3).min(y + h) {
                    for xx in px..(px + rng.range_u32(3, 5)).min(x + w) {
                        put(&mut sheet, xx, yy, [accent[0], accent[1], accent[2], 255]);
                    }
                }
            }
        }
    }
    sheet
}

fn blank() -> Box<skin::SkinRgba> {
    Box::new([[[0u8; 4]; skin::SKIN_SIZE as usize]; skin::SKIN_SIZE as usize])
}

fn put(sheet: &mut skin::SkinRgba, x: u32, y: u32, px: [u8; 4]) {
    if (x as usize) < skin::SKIN_SIZE as usize && (y as usize) < skin::SKIN_SIZE as usize {
        sheet[y as usize][x as usize] = px;
    }
}

/// Fill the `[v0, v1)` vertical band of every face of `part` with a shaded,
/// edge-darkened colour (same look as the armor painter). Top/bottom caps
/// paint when the band reaches their edge.
fn fill_part(sheet: &mut skin::SkinRgba, part: SkinPart, base: [u8; 3], v0: f32, v1: f32) {
    for dir in Direction::ALL {
        let [x, y, w, h] = part.face_rect(dir);
        let (row0, row1) = match dir {
            Direction::PosY => {
                if v0 > 0.0 {
                    continue;
                }
                (y, y + h)
            }
            Direction::NegY => {
                if v1 < 1.0 {
                    continue;
                }
                (y, y + h)
            }
            _ => {
                let r0 = y + (h as f32 * v0) as u32;
                let r1 = y + (h as f32 * v1).round() as u32;
                if r1 <= r0 {
                    continue;
                }
                (r0, r1)
            }
        };
        let fill = shade(base, dir);
        for yy in row0..row1 {
            for xx in x..x + w {
                let edge = xx == x || xx + 1 == x + w || yy == row0 || yy + 1 == row1;
                let px = if edge { darker(fill, 24) } else { fill };
                put(sheet, xx, yy, px);
            }
        }
    }
}

/// Directional shading matching the model's face shading (top lit, sides dim).
fn shade(base: [u8; 3], dir: Direction) -> [u8; 4] {
    let d: i32 = match dir {
        Direction::PosY => 16,
        Direction::NegY => -30,
        Direction::PosX | Direction::NegX => -8,
        Direction::PosZ | Direction::NegZ => -14,
    };
    let c = |v: u8| (v as i32 + d).clamp(0, 255) as u8;
    [c(base[0]), c(base[1]), c(base[2]), 255]
}

fn darker(px: [u8; 4], d: i32) -> [u8; 4] {
    let c = |v: u8| (v as i32 - d).clamp(0, 255) as u8;
    [c(px[0]), c(px[1]), c(px[2]), px[3]]
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn skin_names_resolve_to_reserved_origins() {
        assert_eq!(origin_for("zombie"), Some([12, 4]));
        assert_eq!(origin_for("skeleton"), Some([12, 8]));
        assert_eq!(origin_for("cow"), Some([0, 12]));
        assert_eq!(origin_for("sheep"), Some([4, 12]));
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
    fn painted_sheets_cover_their_unwraps() {
        for kind in ALL {
            let sheet = paint(kind);
            let parts: &[SkinPart] = match kind {
                MobSkin::Cow | MobSkin::Sheep => &[Q_HEAD, Q_BODY, Q_LEG],
                _ => &[
                    skin::HEAD,
                    skin::BODY,
                    skin::LEFT_ARM,
                    skin::RIGHT_ARM,
                    skin::LEFT_LEG,
                    skin::RIGHT_LEG,
                ],
            };
            for part in parts {
                for dir in crate::core::Direction::ALL {
                    let [x, y, w, h] = part.face_rect(dir);
                    // Sample the face's centre pixel: painted and opaque.
                    let px = sheet[(y + h / 2) as usize][(x + w / 2) as usize];
                    assert_eq!(px[3], 255, "{kind:?} {dir:?} face unpainted");
                }
            }
        }
    }

    #[test]
    fn mob_sheets_stay_clear_of_skin_and_armor_blocks() {
        use super::super::armor;
        for kind in ALL {
            for tile in skin::tile_indices_at(kind.origin()) {
                assert!(!skin::atlas_tile_indices().any(|t| t == tile));
                assert!(!armor::is_armor_tile(tile), "{kind:?} overlaps armor");
            }
        }
    }
}

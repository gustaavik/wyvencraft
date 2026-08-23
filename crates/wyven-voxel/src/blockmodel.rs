//! Block models, baked for the chunk mesher.
//!
//! [`wyven_model::blockjson`] turns a Blockbench export into quads in
//! block-local space; this turns those quads into something the mesher can emit
//! per cell without doing any work per block. Two things are resolved here that
//! the parser has no business knowing:
//!
//! - **Texture layers.** Each quad's texture becomes an index into the shared
//!   [`BlockTextureSet`], so every block in a chunk lands in one vertex buffer
//!   and one draw call however many distinct textures they use between them.
//! - **Occlusion.** Whether the model covers a cell face completely with an
//!   opaque texture, which is what lets the *neighbour* drop its own face.
//!   Derived by measuring the geometry, never authored — a slab declaring
//!   itself a full cube would punch holes in the terrain around it.
//!
//! Everything here is in block-local `0..1` coordinates, so the mesher's job is
//! a translation per cell.

use glam::Vec3;

use crate::appearance::model_hitbox;
use wyven_core::{Aabb, Direction};
use wyven_model::blockjson::BlockJsonModel;
use wyven_render::block_textures::BlockTextureSet;

/// How far a corner may sit from a cell boundary and still count as on it.
const COVERAGE_EPSILON: f32 = 1e-3;

/// One quad of a baked model, ready to be translated into a cell.
#[derive(Debug, Clone, PartialEq)]
pub struct BakedQuad {
    pub positions: [Vec3; 4],
    pub normal: [f32; 3],
    pub uvs: [[f32; 2]; 4],
    /// Layer of [`wyven_render::block_textures`] this quad samples.
    pub layer: u32,
    /// The neighbour that hides this quad when that neighbour is solid.
    pub cull: Option<Direction>,
    /// Which biome colour to multiply in, from `tintindex` — `0` grass, `1`
    /// foliage. `None` draws the texture's own colour.
    pub tint: Option<u8>,
    pub shade: f32,
}

/// A block's geometry, resolved against the block texture array.
#[derive(Debug, Clone)]
pub struct BakedBlockModel {
    pub quads: Vec<BakedQuad>,
    /// Per-[`Direction`] "this model fills that cell face with an opaque
    /// texture", so a neighbouring block may drop the face it shares with this
    /// one. Measured from the geometry, not declared.
    pub occludes: [bool; 6],
    pub bounds: (Vec3, Vec3),
    /// What the crosshair hits, in block-local `0..1` coordinates, or `None`
    /// for a model that fills its cell — an ordinary cube, which the raycast
    /// marches through without a box test at all.
    ///
    /// Measured from `bounds` by the same [`model_hitbox`] the `.bbmodel` path
    /// uses, so a flower is only targetable where it actually is.
    pub hitbox: Option<Aabb>,
    /// Turn each instance by a hash of its position. Declared on the `[[block]]`
    /// table, not in the model file — it is placement, not geometry.
    pub random_yaw: bool,
}

impl BakedBlockModel {
    /// Resolve `model`'s textures into `textures` and measure its coverage.
    pub fn bake(model: &BlockJsonModel, textures: &mut BlockTextureSet, random_yaw: bool) -> Self {
        let quads: Vec<BakedQuad> = model
            .quads
            .iter()
            .map(|q| BakedQuad {
                positions: q.positions,
                normal: q.normal.to_array(),
                uvs: q.uvs,
                layer: textures
                    .resolve(&model.texture_paths[q.texture], &model.textures[q.texture]),
                cull: q.cull,
                tint: q.tint,
                shade: q.shade,
            })
            .collect();

        let occludes = std::array::from_fn(|i| {
            let dir = Direction::ALL[i];
            quads
                .iter()
                .any(|q| covers_cell_face(q, dir) && textures.is_opaque(q.layer))
        });

        let bounds = quads
            .iter()
            .flat_map(|q| q.positions)
            .fold(None, |acc: Option<(Vec3, Vec3)>, p| {
                Some(match acc {
                    Some((lo, hi)) => (lo.min(p), hi.max(p)),
                    None => (p, p),
                })
            })
            .unwrap_or((Vec3::ZERO, Vec3::ZERO));

        // Whether the geometry reaches every side of the cell, regardless of
        // what its texture lets through. Bounds alone cannot answer this: two
        // crossed planes span the whole cell without filling any of it.
        let fills_cell = Direction::ALL
            .iter()
            .all(|&dir| quads.iter().any(|q| covers_cell_face(q, dir)));

        Self {
            quads,
            occludes,
            bounds,
            hitbox: (!fills_cell).then(|| model_hitbox(bounds)),
            random_yaw,
        }
    }

    /// The texture layer filling each cell face, ordered by [`Direction`].
    ///
    /// Dropped items and inventory icons still draw six-sided cubes off the
    /// 16-pixel atlas; this is what lets those stand-ins be derived from the
    /// model's own art instead of being authored a second time.
    pub fn face_layers(&self) -> [Option<u32>; 6] {
        std::array::from_fn(|i| {
            let dir = Direction::ALL[i];
            self.quads
                .iter()
                .find(|q| covers_cell_face(q, dir))
                .map(|q| q.layer)
        })
    }
}

/// Whether `quad` lies flat against the cell face `dir` and covers all of it.
///
/// Three conditions, all necessary: it faces the right way, it sits on that
/// boundary plane, and it spans the whole unit square across the other two
/// axes. A 4×4 nub centred in the cell passes the first two and fails the last.
fn covers_cell_face(quad: &BakedQuad, dir: Direction) -> bool {
    let normal = Vec3::from(quad.normal);
    if normal.dot(dir.normal()) < 1.0 - COVERAGE_EPSILON {
        return false;
    }
    let axis = match dir {
        Direction::NegX | Direction::PosX => 0,
        Direction::NegY | Direction::PosY => 1,
        Direction::NegZ | Direction::PosZ => 2,
    };
    let plane = if matches!(dir, Direction::PosX | Direction::PosY | Direction::PosZ) {
        1.0
    } else {
        0.0
    };
    if quad
        .positions
        .iter()
        .any(|p| (p[axis] - plane).abs() > COVERAGE_EPSILON)
    {
        return false;
    }
    // The two axes the face spans must run the full width of the cell.
    (0..3).filter(|&a| a != axis).all(|a| {
        let lo = quad
            .positions
            .iter()
            .fold(f32::INFINITY, |m, p| m.min(p[a]));
        let hi = quad
            .positions
            .iter()
            .fold(f32::NEG_INFINITY, |m, p| m.max(p[a]));
        lo <= COVERAGE_EPSILON && hi >= 1.0 - COVERAGE_EPSILON
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_assets::MapSource;
    use wyven_render::block_textures::BLOCK_TEXTURE_SIZE;

    /// A block-sized PNG, since the texture array demands one exact extent.
    fn png(alpha: u8) -> Vec<u8> {
        let size = BLOCK_TEXTURE_SIZE;
        let mut data = Vec::with_capacity((size * size * 4) as usize);
        for _ in 0..size * size {
            data.extend_from_slice(&[40, 80, 120, alpha]);
        }
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&data)
            .unwrap();
        out
    }

    fn bake(json: &str) -> (BakedBlockModel, BlockTextureSet) {
        let source = MapSource::new()
            .with_bytes("assets/textures/blocks/solid.png", png(255))
            .with_bytes("assets/textures/blocks/cutout.png", png(0));
        let model = wyven_model::blockjson::load(json.as_bytes(), "assets/models/blocks", &source)
            .expect("model loads");
        let mut textures = BlockTextureSet::new();
        let baked = BakedBlockModel::bake(&model, &mut textures, false);
        (baked, textures)
    }

    fn full_cube(texture: &str) -> String {
        format!(
            r##"{{
            "textures": {{ "0": "../../textures/blocks/{texture}" }},
            "elements": [{{
                "from": [0,0,0], "to": [16,16,16],
                "faces": {{
                    "north": {{"uv": [0,0,16,16], "texture": "#0", "cullface": "north"}},
                    "south": {{"uv": [0,0,16,16], "texture": "#0", "cullface": "south"}},
                    "east":  {{"uv": [0,0,16,16], "texture": "#0", "cullface": "east"}},
                    "west":  {{"uv": [0,0,16,16], "texture": "#0", "cullface": "west"}},
                    "up":    {{"uv": [0,0,16,16], "texture": "#0", "cullface": "up"}},
                    "down":  {{"uv": [0,0,16,16], "texture": "#0", "cullface": "down"}}
                }}
            }}]
        }}"##
        )
    }

    #[test]
    fn a_full_opaque_cube_occludes_every_neighbour() {
        let (baked, _) = bake(&full_cube("solid"));
        assert_eq!(baked.quads.len(), 6);
        assert_eq!(baked.occludes, [true; 6]);
        assert_eq!(baked.bounds, (Vec3::ZERO, Vec3::ONE));
    }

    /// The neighbour behind a see-through face still has to draw, so coverage
    /// alone is not enough — the texture must be opaque too.
    #[test]
    fn a_cutout_texture_occludes_nothing_even_at_full_size() {
        let (baked, _) = bake(&full_cube("cutout"));
        assert_eq!(baked.quads.len(), 6);
        assert_eq!(baked.occludes, [false; 6]);
    }

    /// Blockbench's default new cube, left unresized. It must not claim to fill
    /// the cell, or the terrain around it would develop holes.
    #[test]
    fn a_partial_cube_occludes_nothing() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/solid" },
            "elements": [{
                "from": [6,6,6], "to": [10,10,10],
                "faces": {
                    "north": {"uv": [0,0,16,16], "texture": "#0", "cullface": "north"},
                    "up":    {"uv": [0,0,16,16], "texture": "#0", "cullface": "up"}
                }
            }]
        }"##;
        let (baked, _) = bake(json);
        assert_eq!(baked.occludes, [false; 6]);
    }

    #[test]
    fn every_quad_gets_a_real_texture_layer() {
        let (baked, textures) = bake(&full_cube("solid"));
        assert_eq!(textures.len(), 2, "the missing marker plus one texture");
        for quad in &baked.quads {
            assert_ne!(quad.layer, 0, "content must not land on the missing marker");
        }
        // One texture used six times must be one layer.
        let layers: std::collections::HashSet<u32> = baked.quads.iter().map(|q| q.layer).collect();
        assert_eq!(layers.len(), 1);
    }

    #[test]
    fn cullfaces_survive_the_bake() {
        let (baked, _) = bake(&full_cube("solid"));
        for dir in Direction::ALL {
            assert!(baked.quads.iter().any(|q| q.cull == Some(dir)));
        }
    }

    #[test]
    fn face_layers_names_the_texture_covering_each_side() {
        let (baked, _) = bake(&full_cube("solid"));
        let layers = baked.face_layers();
        assert!(layers.iter().all(|l| l.is_some_and(|l| l != 0)));
    }

    #[test]
    fn face_layers_is_empty_where_nothing_covers_the_cell_face() {
        let json = r##"{
            "textures": { "0": "../../textures/blocks/solid" },
            "elements": [{
                "from": [0,0,0], "to": [16,16,16],
                "faces": { "up": {"uv": [0,0,16,16], "texture": "#0"} }
            }]
        }"##;
        let (baked, _) = bake(json);
        let layers = baked.face_layers();
        assert!(layers[Direction::PosY as usize].is_some());
        assert!(layers[Direction::NegY as usize].is_none());
    }
}

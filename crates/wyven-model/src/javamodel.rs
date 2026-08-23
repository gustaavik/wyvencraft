//! Blockbench "Java Block/Item" models (`.json`) as loadable [`Model`]s.
//!
//! [`super::blockjson`] already reads this format for blocks, and reads it
//! completely. What it produces, though, is shaped for the chunk mesher: many
//! textures, plus the `cullface` and `tintindex` a block needs to take part in
//! face culling and biome colouring. A [`Model`] carries one texture and none of
//! that, because the things it draws — a mob, a held sword, a dropped item — have
//! no neighbours to cull against and no biome to sample.
//!
//! So this is an adapter rather than a second parser: it calls `blockjson`, then
//! flattens the result. Three things are dropped on the way, all of them
//! meaningless here:
//!
//! - `cullface` — nothing neighbours an item in a hand;
//! - `tintindex` — an item is never biome-coloured;
//! - `BlockQuad::shade` — [`ModelMesh::bake`] recomputes the face shade from the
//!   transformed normal, so `"shade": false` reads as shaded through this path.
//!
//! What it *keeps*, and what no other format can express, is the `display`
//! block: where the model sits in the hand, on the ground and in the inventory
//! slot, separately. That is the reason this loader exists.
//!
//! Boundaries: pure, like the rest of the crate.

use wyven_assets::AssetSource as ContentSource;
use wyven_assets::Rgba8;
use wyven_render::block_textures::upscale;

use super::mesh::ModelMesh;
use super::{Model, ModelLoader, blockjson};

/// Reads `.json` — see the module docs.
pub struct JavaModelLoader;

impl ModelLoader for JavaModelLoader {
    fn extensions(&self) -> &'static [&'static str] {
        &["json"]
    }

    fn load(&self, bytes: &[u8], dir: &str, source: &dyn ContentSource) -> Result<Model, String> {
        let parsed = blockjson::load(bytes, dir, source)?;
        let (texture, remap) = pack_strip(&parsed.textures)?;

        let mut mesh = ModelMesh::default();
        for quad in &parsed.quads {
            let (v_scale, v_offset) = remap.get(quad.texture).copied().ok_or_else(|| {
                format!("quad names texture {} which was not packed", quad.texture)
            })?;
            let base = mesh.positions.len() as u32;
            for i in 0..4 {
                mesh.positions.push(quad.positions[i]);
                mesh.normals.push(quad.normal);
                mesh.uvs
                    .push([quad.uvs[i][0], quad.uvs[i][1] * v_scale + v_offset]);
            }
            // The same two-triangle split, in the same winding, that every other
            // loader uses — `CpuMesh::push_quad`'s, spelled with indices.
            mesh.indices
                .extend_from_slice(&[base, base + 1, base + 2, base + 2, base + 3, base]);
        }
        mesh.validate()?;

        Ok(Model::new(mesh, texture)?.with_display(parsed.display))
    }
}

/// Pack `textures` into the single image a [`Model`] carries, stacked top to
/// bottom, and return the `(scale, offset)` each one's `v` coordinate needs
/// afterwards.
///
/// A Java model may name several textures — a grass block names four — where a
/// `Model` has room for one. One texture, which is every item model in
/// practice, short-circuits to itself with no copy and no UV change.
///
/// Differing sizes are brought up to the widest with the same rule the atlas and
/// the block texture array apply: nearest, at a whole-number factor, so a 16px
/// texture stays pixel-identical beside a 32px one. Anything that is not a whole
/// multiple is an error rather than a silent stretch, because a stretched
/// texture is a bug you find by squinting.
///
/// A vertical strip is safe under this renderer's sampler (nearest, clamped, no
/// mips): nothing can bleed across the seam between two stacked textures.
fn pack_strip(textures: &[Rgba8]) -> Result<(Rgba8, Vec<(f32, f32)>), String> {
    let [first, ..] = textures else {
        return Err("model has no textures".into());
    };
    if textures.len() == 1 {
        return Ok((first.clone(), vec![(1.0, 0.0)]));
    }

    let width = textures.iter().map(Rgba8::width).max().unwrap_or(0);
    let height = textures.iter().map(Rgba8::height).max().unwrap_or(0);
    if width == 0 || height == 0 {
        return Err("model texture is empty".into());
    }

    let mut scaled = Vec::with_capacity(textures.len());
    for texture in textures {
        let [w, h] = texture.size;
        if w == 0 || h == 0 {
            return Err("model texture is empty".into());
        }
        if !width.is_multiple_of(w) || !height.is_multiple_of(h) || width / w != height / h {
            return Err(format!(
                "model texture {w}x{h} is not a whole fraction of {width}x{height}; \
                 every texture a model names must scale up to the largest by a whole factor"
            ));
        }
        scaled.push(match width / w {
            1 => texture.clone(),
            factor => upscale(texture, factor),
        });
    }

    let count = scaled.len() as u32;
    let mut pixels = Vec::with_capacity((width * height * count * 4) as usize);
    for texture in &scaled {
        pixels.extend_from_slice(&texture.pixels);
    }
    let strip = Rgba8 {
        pixels,
        size: [width, height * count],
    };
    let step = 1.0 / count as f32;
    let remap = (0..count).map(|i| (step, i as f32 * step)).collect();
    Ok((strip, remap))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::display::DisplayContext;
    use wyven_assets::MapSource;

    /// A `size`×`size` PNG of one colour, the way `blockjson`'s tests build one.
    fn png(size: u32, colour: [u8; 4]) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, size, size);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let data: Vec<u8> = colour
            .iter()
            .copied()
            .cycle()
            .take((size * size * 4) as usize)
            .collect();
        encoder
            .write_header()
            .unwrap()
            .write_image_data(&data)
            .unwrap();
        out
    }

    fn image(size: u32, colour: [u8; 4]) -> Rgba8 {
        wyven_assets::decode_png(&png(size, colour)).unwrap()
    }

    #[test]
    fn one_texture_is_passed_through_untouched() {
        let only = image(2, [1, 2, 3, 4]);
        let (packed, remap) = pack_strip(std::slice::from_ref(&only)).unwrap();
        assert_eq!(packed.size, only.size);
        assert_eq!(packed.pixels, only.pixels);
        assert_eq!(remap, vec![(1.0, 0.0)]);
    }

    #[test]
    fn two_textures_stack_and_halve_the_v_range() {
        let a = image(2, [10, 0, 0, 255]);
        let b = image(2, [0, 20, 0, 255]);
        let (packed, remap) = pack_strip(&[a, b]).unwrap();
        assert_eq!(packed.size, [2, 4]);
        assert_eq!(remap, vec![(0.5, 0.0), (0.5, 0.5)]);
        // The second texture's pixels start halfway down the strip.
        assert_eq!(packed.pixels[..4], [10, 0, 0, 255]);
        assert_eq!(packed.pixels[2 * 2 * 4..2 * 2 * 4 + 4], [0, 20, 0, 255]);
    }

    #[test]
    fn a_smaller_texture_is_upscaled_to_the_widest() {
        let small = image(2, [1, 1, 1, 255]);
        let large = image(4, [2, 2, 2, 255]);
        let (packed, remap) = pack_strip(&[small, large]).unwrap();
        assert_eq!(packed.size, [4, 8]);
        assert_eq!(remap.len(), 2);
    }

    #[test]
    fn a_texture_that_is_not_a_whole_fraction_is_rejected() {
        let odd = image(3, [1, 1, 1, 255]);
        let even = image(4, [2, 2, 2, 255]);
        let err = pack_strip(&[odd, even]).unwrap_err();
        assert!(err.contains("whole"), "{err}");
    }

    /// A two-element model: one solid cube and one flat plane, the shapes a
    /// Blockbench item export is made of.
    const MODEL: &str = r##"{
        "texture_size": [16, 16],
        "textures": {"0": "../textures/a"},
        "elements": [
            {"from": [0, 0, 0], "to": [16, 16, 16],
             "faces": {
                "north": {"uv": [0, 0, 16, 16], "texture": "#0"},
                "south": {"uv": [0, 0, 16, 16], "texture": "#0"},
                "east":  {"uv": [0, 0, 16, 16], "texture": "#0"},
                "west":  {"uv": [0, 0, 16, 16], "texture": "#0"},
                "up":    {"uv": [0, 0, 16, 16], "texture": "#0"},
                "down":  {"uv": [0, 0, 16, 16], "texture": "#0"}}}
        ],
        "display": {
            "firstperson_righthand": {
                "rotation": [-99.9, 87.78, 95.45],
                "translation": [0, 1, 1],
                "scale": [0.79883, 0.79883, 0.79883]
            },
            "gui": {"rotation": [-180, -1.25, 136]}
        }
    }"##;

    fn load(json: &str) -> Result<Model, String> {
        let source = MapSource::new()
            .with_bytes("assets/models/t.json", json.as_bytes().to_vec())
            .with_bytes("assets/textures/a.png", png(16, [9, 9, 9, 255]));
        JavaModelLoader.load(json.as_bytes(), "assets/models", &source)
    }

    #[test]
    fn a_cube_loads_as_six_quads() {
        let model = load(MODEL).unwrap();
        assert_eq!(model.vertex_count(), 24, "6 faces x 4 corners");
        assert_eq!(model.triangle_count(), 12);
        assert_eq!(model.texture.size, [16, 16]);
        assert!(
            model
                .mesh
                .uvs
                .iter()
                .all(|[u, v]| { (0.0..=1.0).contains(u) && (0.0..=1.0).contains(v) })
        );
    }

    #[test]
    fn the_display_block_survives_the_load() {
        let model = load(MODEL).unwrap();
        let first = model
            .placement_for(DisplayContext::FirstPersonRightHand)
            .expect("firstperson_righthand");
        assert_eq!(first.translation, [0.0, 1.0, 1.0]);
        assert!((first.scale[0] - 0.79883).abs() < 1e-6);
        assert!(model.placement_for(DisplayContext::Gui).is_some());
        // Undeclared contexts stay absent, so the caller falls back to its spec.
        assert!(model.placement_for(DisplayContext::Ground).is_none());
    }

    #[test]
    fn the_loader_claims_json() {
        assert_eq!(JavaModelLoader.extensions(), &["json"]);
    }
}

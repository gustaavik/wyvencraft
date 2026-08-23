//! Minecraft's `item/generated`: a model extruded from a 2D sprite.
//!
//! Most items in a voxel game are flat art — an apple, a stick, a lump of coal.
//! They have no geometry anyone authored, and authoring some in Blockbench is
//! the wrong answer: a modelling tool cannot see where the drawn shape ends, so
//! a hand-made slab shows square corners wherever the PNG is transparent.
//!
//! Minecraft's answer, which this follows, is to declare the *absence* of
//! geometry and let the loader derive it:
//!
//! ```json
//! { "parent": "item/generated",
//!   "textures": { "layer0": "../../textures/items/apple" } }
//! ```
//!
//! The sprite becomes two quads back to back, one texel apart, plus a rim
//! wherever its alpha ends — so the item has a real edge when it turns side-on
//! in the hand, and its silhouette is exactly the art's.
//!
//! It works by *synthesising elements*, not by emitting geometry directly. Every
//! quad still goes through the same [`super::blockjson`] path an authored model
//! does, so face-corner order, UV rotation, the co-planar nudge and the shade
//! curve cannot drift between a generated item and a modelled one — there is
//! only one implementation of each.
//!
//! Boundaries: pure. The outline comes from [`super::silhouette`], shared with
//! the voxel crate's sprite mesher so a dropped item and its model agree.

use wyven_assets::Rgba8;

use super::blockjson::{Element, Face};
use super::display::{DisplayTransforms, ItemTransform};
use super::silhouette;

/// The parent names that mean "extrude my texture", with or without a namespace.
const GENERATED_PARENTS: [&str; 3] = ["item/generated", "builtin/generated", "item/handheld"];

/// The texture key a generated model draws its shape from.
pub const LAYER: &str = "layer0";

/// How thick the extruded sprite is, in Minecraft's sixteenths — one texel of a
/// 16px sprite, centred on the middle of the block, exactly as Minecraft does.
const FRONT: f32 = 8.5;
const BACK: f32 = 7.5;

/// Minecraft's UV space, which is always `0..16` whatever the sprite's size.
const UV_SPAN: f32 = 16.0;

/// Whether `parent` asks for the geometry to be generated.
///
/// `item/handheld` is included because it differs from `item/generated` only in
/// its display transforms, which nothing here reads — the geometry is identical.
pub fn claims(parent: &str) -> bool {
    let bare = parent.rsplit(':').next().unwrap_or(parent);
    GENERATED_PARENTS.contains(&bare)
}

/// Build the elements a generated model is made of: the two flat faces, then one
/// rim quad per exposed texel edge.
///
/// `sprite` must be square — the UV grid and the model grid are the same grid,
/// and a non-square sprite has no sensible reading as a 1:1 block face.
pub fn elements(sprite: &Rgba8) -> Result<Vec<Element>, String> {
    let [width, height] = sprite.size;
    if width == 0 || height == 0 {
        return Err("generated model: the sprite is empty".into());
    }
    if width != height {
        return Err(format!(
            "generated model: the sprite is {width}x{height}; it must be square, \
             because its pixels are also its geometry"
        ));
    }

    // One texel is this many sixteenths, in both model space and UV space —
    // they are the same grid, which is the whole idea of `item/generated`.
    let step = UV_SPAN / width as f32;
    let (w, h) = (width as usize, height as usize);

    // The two flat faces. Alpha-testing cuts the silhouette out of each, so
    // they stay one quad apiece however complicated the drawn shape is.
    let mut out = vec![
        // Front: +Z, seen the same way round as the inventory icon.
        plane(BACK, FRONT, "south", [0.0, 0.0, UV_SPAN, UV_SPAN]),
        // Back: -Z, mirrored in u so the art reads correctly from behind.
        plane(BACK, FRONT, "north", [UV_SPAN, 0.0, 0.0, UV_SPAN]),
    ];

    let opaque = |x: usize, y: usize| silhouette::is_opaque(&sprite.pixels, w, x, y);
    for (tx, ty, dir) in silhouette::trace(w, h, opaque) {
        out.push(rim(tx, ty, dir, step));
    }
    Ok(out)
}

/// Where a generated item sits in each context, when its file says nothing.
///
/// These are Minecraft's own `item/generated` numbers, and they are the reason a
/// stub can be three lines: the placement of a flat sprite in a fist, on the
/// ground and in a slot is the same for every flat sprite, so nobody should have
/// to author it per item. A stub that *does* declare a `display` block keeps it —
/// this is a default, not an override.
///
/// The `gui` entry matters more than it looks. Without one the icon renderer
/// falls back to fitting the model by its bounds under the shared three-quarter
/// view, which turns a one-texel-thick card almost edge-on. An identity
/// placement is what keeps a 2D item's icon face-on, the way it is drawn.
pub fn default_display() -> DisplayTransforms {
    fn at(rotation: [f32; 3], translation: [f32; 3], scale: f32) -> Option<ItemTransform> {
        Some(ItemTransform {
            rotation,
            translation,
            scale: [scale; 3],
        })
    }
    DisplayTransforms {
        firstperson_righthand: at([0.0, -90.0, 25.0], [1.13, 3.2, 1.13], 0.68),
        thirdperson_righthand: at([0.0, 0.0, 0.0], [0.0, 3.0, 1.0], 0.55),
        gui: at([0.0; 3], [0.0; 3], 1.0),
        ground: at([0.0, 0.0, 0.0], [0.0, 2.0, 0.0], 0.5),
        fixed: at([0.0, 180.0, 0.0], [0.0; 3], 1.0),
        head: at([0.0, 180.0, 0.0], [0.0, 13.0, 7.0], 1.0),
    }
}

/// One of the two flat faces: the full sprite, spanning the whole block.
fn plane(z_lo: f32, z_hi: f32, face: &str, uv: [f32; 4]) -> Element {
    Element::synthetic(
        [0.0, 0.0, z_lo],
        [UV_SPAN, UV_SPAN, z_hi],
        face,
        Face::synthetic(uv),
    )
}

/// One rim quad: a single texel's edge, spanning the sprite's full thickness.
///
/// The box is flat in the direction the edge faces, so only the one face named
/// has any area — the other five collapse and are dropped downstream.
fn rim(tx: u16, ty: u16, dir: wyven_core::Direction, step: f32) -> Element {
    use wyven_core::Direction;

    // Image rows count down from the top; model space counts up. A texel on row
    // `ty` therefore sits `ty + 1` rows below the top edge of the sprite.
    let (x0, x1) = (tx as f32 * step, (tx + 1) as f32 * step);
    let (y1, y0) = (UV_SPAN - ty as f32 * step, UV_SPAN - (ty + 1) as f32 * step);
    let uv = [x0, ty as f32 * step, x1, (ty + 1) as f32 * step];

    let (from, to, face) = match dir {
        Direction::NegX => ([x0, y0, BACK], [x0, y1, FRONT], "west"),
        Direction::PosX => ([x1, y0, BACK], [x1, y1, FRONT], "east"),
        Direction::PosY => ([x0, y1, BACK], [x1, y1, FRONT], "up"),
        Direction::NegY => ([x0, y0, BACK], [x1, y0, FRONT], "down"),
        // `silhouette::trace` only ever reports the four in-plane directions.
        other => unreachable!("a sprite rim cannot face {other:?}"),
    };
    Element::synthetic(from, to, face, Face::synthetic(uv))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A square of `n` opaque texels a side, as an `Rgba8`.
    fn solid(n: u32) -> Rgba8 {
        Rgba8 {
            size: [n, n],
            pixels: vec![255; (n * n * 4) as usize],
        }
    }

    #[test]
    fn the_generated_parents_are_recognised_with_or_without_a_namespace() {
        assert!(claims("item/generated"));
        assert!(claims("minecraft:item/generated"));
        assert!(claims("builtin/generated"));
        assert!(claims("item/handheld"));
        assert!(!claims("block/cube_all"));
        assert!(!claims(""));
    }

    /// Two flat faces however big the sprite, plus its perimeter in texels: a
    /// solid n x n square has 4n exposed edges.
    #[test]
    fn a_solid_square_gets_two_planes_and_a_rim_of_its_perimeter() {
        for n in [4u32, 16] {
            let els = elements(&solid(n)).expect("builds");
            assert_eq!(els.len(), 2 + 4 * n as usize, "n = {n}");
        }
    }

    /// Nothing drawn means no rim at all — the two planes remain, and alpha
    /// testing takes care of the rest.
    #[test]
    fn a_fully_transparent_sprite_has_no_rim() {
        let blank = Rgba8 {
            size: [8, 8],
            pixels: vec![0; 8 * 8 * 4],
        };
        assert_eq!(elements(&blank).expect("builds").len(), 2);
    }

    #[test]
    fn a_non_square_sprite_is_rejected() {
        let wide = Rgba8 {
            size: [16, 8],
            pixels: vec![255; 16 * 8 * 4],
        };
        let err = elements(&wide).expect_err("must be refused");
        assert!(err.contains("square"), "{err}");
    }

    #[test]
    fn an_empty_sprite_is_rejected() {
        let empty = Rgba8 {
            size: [0, 0],
            pixels: Vec::new(),
        };
        assert!(elements(&empty).is_err());
    }

    /// The sprite is one texel thick and centred on the block, whatever its
    /// resolution — a 256px item is no fatter than a 16px one.
    #[test]
    fn thickness_is_one_sixteenth_regardless_of_resolution() {
        assert!((FRONT - BACK - 1.0).abs() < f32::EPSILON);
        assert!(((FRONT + BACK) / 2.0 - 8.0).abs() < f32::EPSILON);
    }
}

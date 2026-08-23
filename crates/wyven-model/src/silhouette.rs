//! Tracing the outline of a sprite's drawn shape.
//!
//! A flat item has no authored geometry — its shape is whatever its PNG's alpha
//! says it is. Two things need that outline and must agree on it exactly:
//!
//! - [`super::generated`], which extrudes a `parent: "item/generated"` stub into
//!   a real model with a rim around its edge;
//! - the voxel crate's item-sprite mesher, which does the same for a loose stack
//!   lying on the ground that has no model file at all.
//!
//! They used to walk the pixels separately, each with its own copy of the alpha
//! cutoff. If those two numbers ever drifted apart, the rim traced for the
//! dropped item and the rim baked into the model would disagree — a bug that
//! looks like nothing until you stand at the right angle. So the walk lives here
//! once, and both callers ask this.
//!
//! Boundaries: pure. No image type of its own, no filesystem: callers pass the
//! dimensions and a predicate, because one of them holds a fixed-size atlas tile
//! and the other a dynamically sized PNG.

use wyven_core::Direction;

/// Alpha at or above which a texel counts as part of the drawn shape.
///
/// The same cutoff `voxel.frag` discards below, so a traced rim lands exactly on
/// the silhouette that ends up on screen rather than a texel inside or outside
/// it.
pub const ALPHA_CUTOFF: u8 = 26; // 0.1 * 255, rounded up

/// One exposed texel edge: where in the image, and which way it faces.
///
/// `y` counts *down* from the top of the image, matching how both a PNG and an
/// atlas tile are indexed. Only the four in-plane directions ever appear — the
/// two flat faces of a sprite are implicit.
pub type Edge = (u16, u16, Direction);

/// Every texel edge of the drawn shape with nothing beside it.
///
/// `solid(x, y)` reports whether a texel is part of the shape; see
/// [`is_opaque`] for the usual alpha-based answer. The walk is row-major, so
/// the result is stable for a given image and can be compared directly between
/// callers.
pub fn trace(width: usize, height: usize, solid: impl Fn(usize, usize) -> bool) -> Vec<Edge> {
    let mut rim = Vec::new();
    for y in 0..height {
        for x in 0..width {
            if !solid(x, y) {
                continue;
            }
            // `y` counts down the image, so the *previous* row is up.
            let exposed = [
                (Direction::NegX, x == 0 || !solid(x - 1, y)),
                (Direction::PosX, x + 1 == width || !solid(x + 1, y)),
                (Direction::PosY, y == 0 || !solid(x, y - 1)),
                (Direction::NegY, y + 1 == height || !solid(x, y + 1)),
            ];
            for (dir, is_exposed) in exposed {
                if is_exposed {
                    rim.push((x as u16, y as u16, dir));
                }
            }
        }
    }
    rim
}

/// Whether the texel at `(x, y)` of an RGBA image clears [`ALPHA_CUTOFF`].
///
/// Out-of-bounds reads as empty, so a caller need not bounds-check first.
pub fn is_opaque(pixels: &[u8], width: usize, x: usize, y: usize) -> bool {
    let index = (y * width + x) * 4 + 3;
    pixels.get(index).is_some_and(|&a| a >= ALPHA_CUTOFF)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A lone texel is exposed on all four sides.
    #[test]
    fn a_single_texel_has_four_edges() {
        let rim = trace(3, 3, |x, y| (x, y) == (1, 1));
        assert_eq!(rim.len(), 4);
        for dir in [
            Direction::NegX,
            Direction::PosX,
            Direction::PosY,
            Direction::NegY,
        ] {
            assert!(rim.contains(&(1, 1, dir)), "missing {dir:?}");
        }
    }

    /// A fully solid image is exposed only around its border, never inside it —
    /// this is what stops the rim costing a quad per texel.
    #[test]
    fn a_full_square_is_exposed_only_at_its_border() {
        let rim = trace(4, 4, |_, _| true);
        assert_eq!(rim.len(), 4 * 4, "one edge per border texel per open side");
        assert!(
            !rim.iter()
                .any(|&(x, y, _)| (1..3).contains(&x) && (1..3).contains(&y)),
            "an interior texel must contribute no rim"
        );
    }

    /// Two neighbours hide the edge they share, and only that one.
    #[test]
    fn touching_texels_hide_the_edge_between_them() {
        let rim = trace(3, 1, |x, _| x < 2);
        assert!(
            !rim.contains(&(0, 0, Direction::PosX)),
            "left texel's right edge"
        );
        assert!(
            !rim.contains(&(1, 0, Direction::NegX)),
            "right texel's left edge"
        );
        assert!(rim.contains(&(0, 0, Direction::NegX)));
        assert!(rim.contains(&(1, 0, Direction::PosX)));
    }

    /// Nothing drawn means nothing to outline.
    #[test]
    fn an_empty_image_has_no_rim() {
        assert!(trace(8, 8, |_, _| false).is_empty());
    }

    /// The walk is row-major, so two callers tracing the same shape get the
    /// same list in the same order.
    #[test]
    fn the_walk_is_deterministic() {
        let shape = |x: usize, y: usize| !(x + y).is_multiple_of(3);
        assert_eq!(trace(6, 6, shape), trace(6, 6, shape));
    }

    #[test]
    fn opacity_is_read_off_the_alpha_channel() {
        // 2x1: an opaque texel then a transparent one.
        let pixels = [255, 0, 0, 255, 0, 255, 0, ALPHA_CUTOFF - 1];
        assert!(is_opaque(&pixels, 2, 0, 0));
        assert!(!is_opaque(&pixels, 2, 1, 0));
        assert!(!is_opaque(&pixels, 2, 9, 9), "out of bounds reads as empty");
    }
}

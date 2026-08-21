//! The 3D item-icon sheet.
//!
//! Items whose model comes from a file can't be drawn as a flat atlas tile or a
//! shaded cube — they have arbitrary geometry and their own texture. Instead
//! each one is rendered once, at startup, into a cell of an offscreen sheet;
//! the inventory and hotbar then draw that cell exactly like any other icon.
//!
//! Rendering once rather than per frame is the whole point: an icon never
//! changes, and the alternative is an offscreen pass per visible slot per frame.
//!
//! The camera is a fixed orthographic three-quarter view, shared by every cell,
//! which is what lets the sheet be filled in a single render pass with nothing
//! but a viewport change between cells.

use glam::{Mat4, Vec3};

/// Edge of one icon cell, in pixels. Generous enough that a slot drawn at any
/// reasonable UI scale is downsampling rather than magnifying.
pub const CELL: u32 = 96;
/// Cells per row in the sheet.
pub const COLUMNS: u32 = 8;

/// Sheet dimensions needed to hold `count` icons.
pub fn sheet_size(count: u32) -> [u32; 2] {
    let rows = count.div_ceil(COLUMNS).max(1);
    [COLUMNS * CELL, rows * CELL]
}

/// Pixel rect `[x, y, w, h]` of cell `index` within the sheet.
pub fn cell_rect(index: u32) -> [u32; 4] {
    [
        (index % COLUMNS) * CELL,
        (index / COLUMNS) * CELL,
        CELL,
        CELL,
    ]
}

/// Normalised UVs `[u0, v0, u1, v1]` of cell `index` in a sheet built for
/// `count` icons. Used by the UI to sample one cell out of the sheet.
pub fn cell_uv(index: u32, count: u32) -> [f32; 4] {
    let [w, h] = sheet_size(count);
    let [x, y, cw, ch] = cell_rect(index);
    [
        x as f32 / w as f32,
        y as f32 / h as f32,
        (x + cw) as f32 / w as f32,
        (y + ch) as f32 / h as f32,
    ]
}

/// Fraction of the cell the model's widest on-screen axis is scaled to, leaving
/// a margin so nothing touches the slot border.
const FILL: f32 = 0.92;
/// The three-quarter view every icon is drawn from: turn, then tilt down.
const ICON_YAW: f32 = 35.0;
const ICON_PITCH: f32 = 20.0;
/// Lay the model corner-to-corner in its cell. A tool or weapon is long and
/// thin, so upright it is a sliver a few pixels wide once the cell is scaled
/// into a hotbar slot; on the diagonal it spans both axes and stays legible.
const ICON_ROLL: f32 = -45.0;

/// Model→world transform that frames geometry with the given bounds for an
/// icon: centre it on the origin, turn it to the shared three-quarter view,
/// then scale it to fill the cell.
///
/// Fitting measures the bounds *after* rotating — the view angle is fixed and
/// known, so the exact on-screen extent is too. Fitting the bounding sphere
/// instead would be simpler but badly conservative for the common case: a sword
/// laid across the diagonal is mostly empty space, and sizing it by its length
/// leaves it a sliver in the middle of the slot.
///
/// Rotating a box centred on the origin leaves its axis-aligned extent centred
/// there too, so scaling afterwards keeps the model centred.
///
/// Note this fits the model's *bounds*, not its silhouette: geometry that
/// doesn't reach the corners of its own bounding box renders somewhat smaller
/// than [`FILL`] implies. Raise `FILL` if icons want to sit tighter in the slot.
pub fn frame(bounds: (Vec3, Vec3)) -> Mat4 {
    let (lo, hi) = bounds;
    let orient = Mat4::from_rotation_z(ICON_ROLL.to_radians())
        * Mat4::from_rotation_x(ICON_PITCH.to_radians())
        * Mat4::from_rotation_y(ICON_YAW.to_radians());
    let centre = (lo + hi) * 0.5;

    // Extent of the rotated bounds on the two axes the camera shows.
    let mut half = Vec3::ZERO;
    for i in 0..8 {
        let corner = Vec3::new(
            if i & 1 == 0 { lo.x } else { hi.x },
            if i & 2 == 0 { lo.y } else { hi.y },
            if i & 4 == 0 { lo.z } else { hi.z },
        );
        half = half.max(orient.transform_point3(corner - centre).abs());
    }
    let widest = half.x.max(half.y);
    let scale = if widest > f32::EPSILON {
        FILL * 0.5 / widest
    } else {
        1.0
    };

    Mat4::from_scale(Vec3::splat(scale)) * orient * Mat4::from_translation(-centre)
}

/// The orthographic camera every icon cell is rendered with. Looks down −Z at
/// the unit box [`frame`] places geometry in, with the Y flip the rest of the
/// renderer uses for Vulkan clip space.
///
/// The depth range is deliberately loose: fitting only considers the two axes
/// the camera shows, so a deep model must not be clipped front-to-back for it.
pub fn view_projection() -> Mat4 {
    let mut proj = Mat4::orthographic_rh(-0.5, 0.5, -0.5, 0.5, -4.0, 4.0);
    proj.y_axis.y *= -1.0;
    proj
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cells_tile_the_sheet_without_overlap() {
        let count = COLUMNS * 2 + 3;
        let [w, h] = sheet_size(count);
        assert_eq!(w, COLUMNS * CELL);
        assert_eq!(h, 3 * CELL, "three rows for two full ones plus a partial");
        for i in 0..count {
            let [x, y, cw, ch] = cell_rect(i);
            assert!(x + cw <= w && y + ch <= h, "cell {i} escapes the sheet");
        }
        // Neighbours in a row are exactly one cell apart, and do not overlap.
        assert_eq!(cell_rect(1)[0] - cell_rect(0)[0], CELL);
        assert_eq!(cell_rect(COLUMNS)[1], CELL, "the row wraps");
    }

    #[test]
    fn cell_uvs_span_zero_to_one_across_a_full_sheet() {
        let uv = cell_uv(0, COLUMNS);
        assert_eq!(uv[0], 0.0);
        assert_eq!(uv[1], 0.0);
        assert_eq!(uv[3], 1.0, "a single row is the full height");
        let last = cell_uv(COLUMNS - 1, COLUMNS);
        assert_eq!(last[2], 1.0, "the last column reaches the right edge");
    }

    /// Representative model shapes: a block, the tall thin sword (offset well
    /// away from the origin), and something wide and flat.
    const SHAPES: [(Vec3, Vec3); 3] = [
        (Vec3::ZERO, Vec3::ONE),
        (
            Vec3::new(0.467, -0.889, 0.113),
            Vec3::new(0.533, 1.471, 0.887),
        ),
        (Vec3::new(-8.0, 0.0, -8.0), Vec3::new(8.0, 0.2, 8.0)),
    ];

    /// Clip-space bounds of `bounds` once framed and projected.
    fn projected(bounds: (Vec3, Vec3)) -> (Vec3, Vec3) {
        let (lo, hi) = bounds;
        let transform = view_projection() * frame(bounds);
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            let clip = transform.transform_point3(corner);
            min = min.min(clip);
            max = max.max(clip);
        }
        (min, max)
    }

    /// Whatever the model's size or where it sits in its own space, framing must
    /// land every corner inside the cell — including front-to-back, which the
    /// fit deliberately ignores and the loose depth range has to absorb.
    #[test]
    fn framing_fits_any_model_inside_the_cell() {
        for bounds in SHAPES {
            let (min, max) = projected(bounds);
            assert!(
                min.x >= -1.0 && max.x <= 1.0 && min.y >= -1.0 && max.y <= 1.0,
                "{bounds:?} projects to {min}..{max}, outside the cell"
            );
            assert!(
                min.z >= -1.0 && max.z <= 1.0,
                "{bounds:?} projects to depth {}..{}, outside the clip range",
                min.z,
                max.z
            );
        }
    }

    /// ...and it must actually *fill* the cell: the widest on-screen axis reaches
    /// the margin. Guards against the fit going conservative again and leaving
    /// models as slivers adrift in their slots.
    #[test]
    fn framing_fills_the_cell_on_the_widest_axis() {
        for bounds in SHAPES {
            let (min, max) = projected(bounds);
            let widest = (max.x - min.x).max(max.y - min.y);
            // Clip space spans 2.0 across the cell, so a full cell is 2 * FILL.
            assert!(
                (widest - 2.0 * FILL).abs() < 1e-4,
                "{bounds:?} spans {widest}, expected {}",
                2.0 * FILL
            );
        }
    }

    /// Framing must also *centre* the model, not just contain it — an icon
    /// hugging one edge of its slot reads as a layout bug.
    #[test]
    fn framing_centres_the_model_in_the_cell() {
        // Bounds well away from the origin, so a missing re-centre shows up.
        let (lo, hi) = (
            Vec3::new(0.467, -0.889, 0.113),
            Vec3::new(0.533, 1.471, 0.887),
        );
        let transform = view_projection() * frame((lo, hi));
        let (mut min, mut max) = (Vec3::splat(f32::MAX), Vec3::splat(f32::MIN));
        for i in 0..8 {
            let corner = Vec3::new(
                if i & 1 == 0 { lo.x } else { hi.x },
                if i & 2 == 0 { lo.y } else { hi.y },
                if i & 4 == 0 { lo.z } else { hi.z },
            );
            let clip = transform.transform_point3(corner);
            min = min.min(clip);
            max = max.max(clip);
        }
        let centre = (min + max) * 0.5;
        assert!(
            centre.x.abs() < 1e-4 && centre.y.abs() < 1e-4,
            "model centred at {centre}, not on the cell centre"
        );
    }

    #[test]
    fn framing_a_degenerate_model_does_not_divide_by_zero() {
        let m = frame((Vec3::ZERO, Vec3::ZERO));
        assert!(m.is_finite());
    }
}

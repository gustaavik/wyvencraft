//! Nine-slice blitting for the inventory's UI sheet.
//!
//! A panel has to be drawn at whatever size its contents need, but its border
//! must not stretch with it — a 2px bevel scaled to a 600px-wide panel is a
//! 25px smear. So each region of `assets/textures/gui/panel.png` is cut into a
//! 3x3: the four corners are drawn at their authored size, the four edges
//! stretch along one axis only, and the centre stretches along both.
//!
//! One `Mesh` of nine quads per call, exactly as [`crate::ui::icon`] builds its
//! item icons, so a whole panel is a single draw against a single texture.
//!
//! The offsets below mirror `scripts/gen-gui-textures.py`, which draws the
//! sheet. Change them together.

use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Painter, Pos2, Rect, Shape, TextureId, pos2};

/// Side of the square UI sheet, in texels.
const SHEET: f32 = 128.0;

/// One nine-sliced region of the sheet.
#[derive(Clone, Copy)]
pub struct NinePatch {
    /// Top-left of the region on the sheet, in texels.
    origin: [f32; 2],
    /// The region is square; this is its side, in texels.
    size: f32,
    /// Corner size, in texels. Everything inside it stretches.
    inset: f32,
}

/// The dark carcass the whole inventory sits in.
pub const PANEL: NinePatch = NinePatch {
    origin: [0.0, 0.0],
    size: 48.0,
    inset: 16.0,
};

/// The mid-brown bed behind the slot grid.
pub const GRID: NinePatch = NinePatch {
    origin: [48.0, 0.0],
    size: 48.0,
    inset: 16.0,
};

/// One empty cell.
pub const SLOT: NinePatch = NinePatch {
    origin: [0.0, 48.0],
    size: 24.0,
    inset: 8.0,
};

/// A cell with the hotbar's selection surround.
pub const SLOT_SELECTED: NinePatch = NinePatch {
    origin: [24.0, 48.0],
    size: 24.0,
    inset: 8.0,
};

/// The hover tooltip's backing.
pub const TOOLTIP: NinePatch = NinePatch {
    origin: [48.0, 48.0],
    size: 32.0,
    inset: 12.0,
};

/// Draw `patch` stretched across `rect`, tinted by `tint`.
///
/// The borders are never scaled up: if `rect` is too small to hold both corners
/// on an axis, they are scaled *down* to fit rather than overlapping, so a panel
/// mid-animation collapses cleanly instead of drawing its right border left of
/// its left one.
pub fn draw_nine(painter: &Painter, rect: Rect, patch: NinePatch, tint: Color32, tex: TextureId) {
    let mut mesh = Mesh::with_texture(tex);
    push_nine(&mut mesh, rect, patch, tint);
    painter.add(Shape::mesh(mesh));
}

/// The geometry behind [`draw_nine`], separated so it can be checked without a
/// `Painter` (and therefore without a GPU).
pub fn push_nine(mesh: &mut Mesh, rect: Rect, patch: NinePatch, tint: Color32) {
    // Shrink the corners rather than let them overlap on a too-small rect.
    let border = patch
        .inset
        .min(rect.width() * 0.5)
        .min(rect.height() * 0.5)
        .max(0.0);

    // Screen-space column and row boundaries.
    let xs = [
        rect.left(),
        rect.left() + border,
        rect.right() - border,
        rect.right(),
    ];
    let ys = [
        rect.top(),
        rect.top() + border,
        rect.bottom() - border,
        rect.bottom(),
    ];

    // The matching cuts on the sheet, normalized. Half a texel is *not* inset
    // here: the interior cuts are shared edges between adjacent quads, and
    // pulling them apart would drop a texel column out of the middle of the
    // border. Only the outer edge of the whole region needs protecting, and
    // the generator leaves a 1px outline there for exactly that reason.
    let [ox, oy] = patch.origin;
    let us = [
        ox / SHEET,
        (ox + patch.inset) / SHEET,
        (ox + patch.size - patch.inset) / SHEET,
        (ox + patch.size) / SHEET,
    ];
    let vs = [
        oy / SHEET,
        (oy + patch.inset) / SHEET,
        (oy + patch.size - patch.inset) / SHEET,
        (oy + patch.size) / SHEET,
    ];

    for row in 0..3 {
        for col in 0..3 {
            let cell = Rect::from_min_max(pos2(xs[col], ys[row]), pos2(xs[col + 1], ys[row + 1]));
            // A zero-width or zero-height slice contributes nothing; skipping it
            // keeps a collapsed panel from emitting degenerate triangles.
            if cell.width() <= 0.0 || cell.height() <= 0.0 {
                continue;
            }
            let uv = Rect::from_min_max(pos2(us[col], vs[row]), pos2(us[col + 1], vs[row + 1]));
            quad(mesh, cell, uv, tint);
        }
    }
}

/// Push one textured quad (two triangles) into `mesh`.
fn quad(mesh: &mut Mesh, rect: Rect, uv: Rect, color: Color32) {
    let base = mesh.vertices.len() as u32;
    let pts: [Pos2; 4] = [
        rect.left_top(),
        rect.right_top(),
        rect.right_bottom(),
        rect.left_bottom(),
    ];
    let uvs: [Pos2; 4] = [
        uv.left_top(),
        uv.right_top(),
        uv.right_bottom(),
        uv.left_bottom(),
    ];
    for i in 0..4 {
        mesh.vertices.push(Vertex {
            pos: pts[i],
            uv: uvs[i],
            color,
        });
    }
    mesh.indices
        .extend_from_slice(&[base, base + 1, base + 2, base, base + 2, base + 3]);
}

#[cfg(test)]
mod tests {
    use super::*;
    use egui::vec2;

    fn mesh_for(rect: Rect, patch: NinePatch) -> Mesh {
        let mut mesh = Mesh::default();
        push_nine(&mut mesh, rect, patch, Color32::WHITE);
        mesh
    }

    #[test]
    fn a_patch_is_nine_quads() {
        let rect = Rect::from_min_size(pos2(10.0, 20.0), vec2(300.0, 200.0));
        let mesh = mesh_for(rect, PANEL);
        assert_eq!(mesh.vertices.len(), 9 * 4);
        assert_eq!(mesh.indices.len(), 9 * 6);
    }

    /// The whole point of a nine-slice: the border keeps its authored thickness
    /// however far the middle is stretched. Checked as the corner quad's size,
    /// so it still means something after the sheet is re-cut.
    #[test]
    fn the_corners_keep_their_size_at_any_target_size() {
        for size in [vec2(120.0, 90.0), vec2(600.0, 400.0), vec2(1200.0, 130.0)] {
            let rect = Rect::from_min_size(pos2(0.0, 0.0), size);
            let mesh = mesh_for(rect, PANEL);
            // Quad 0 is the top-left corner: vertices 0..4.
            let corner = mesh.vertices[2].pos - mesh.vertices[0].pos;
            assert_eq!(
                (corner.x, corner.y),
                (PANEL.inset, PANEL.inset),
                "top-left corner stretched at {size:?}"
            );
        }
    }

    /// A panel animating out of the hotbar passes through sizes narrower than
    /// its own border. The corners must give way rather than cross over.
    #[test]
    fn a_rect_smaller_than_the_border_does_not_invert() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(6.0, 400.0));
        let mesh = mesh_for(rect, PANEL);
        for v in &mesh.vertices {
            assert!(
                v.pos.x >= rect.left() - 1e-3 && v.pos.x <= rect.right() + 1e-3,
                "vertex {:?} escaped a rect narrower than the border",
                v.pos
            );
        }
    }

    /// Every slice must sample inside its own region, or a panel bleeds the
    /// neighbouring art on the shared sheet.
    #[test]
    fn a_patch_samples_only_its_own_region() {
        let rect = Rect::from_min_size(pos2(0.0, 0.0), vec2(500.0, 300.0));
        for patch in [PANEL, GRID, SLOT, SLOT_SELECTED, TOOLTIP] {
            let mesh = mesh_for(rect, patch);
            let u0 = patch.origin[0] / SHEET;
            let u1 = (patch.origin[0] + patch.size) / SHEET;
            let v0 = patch.origin[1] / SHEET;
            let v1 = (patch.origin[1] + patch.size) / SHEET;
            for vert in &mesh.vertices {
                assert!(
                    vert.uv.x >= u0 - 1e-6 && vert.uv.x <= u1 + 1e-6,
                    "u {} outside [{u0}, {u1}]",
                    vert.uv.x
                );
                assert!(
                    vert.uv.y >= v0 - 1e-6 && vert.uv.y <= v1 + 1e-6,
                    "v {} outside [{v0}, {v1}]",
                    vert.uv.y
                );
            }
        }
    }

    /// `SHEET` is what turns texel offsets into UVs, so if the committed PNG is
    /// ever regenerated at another size every region silently samples the wrong
    /// part of it. Decoded through the same path the game loads it by.
    #[test]
    fn the_committed_sheet_is_the_size_the_uvs_assume() {
        let bytes = include_bytes!("../../assets/textures/gui/panel.png");
        let image = wyven_assets::decode_png(bytes).expect("the UI sheet decodes");
        assert_eq!(
            image.size,
            [SHEET as u32, SHEET as u32],
            "assets/textures/gui/panel.png disagrees with SHEET; \
             re-run scripts/gen-gui-textures.py or fix the constant"
        );
    }

    /// The regions are cut out of one sheet by hand, in two places
    /// (`scripts/gen-gui-textures.py` draws them, this module addresses them).
    /// Overlapping rectangles would show as one patch wearing another's bevel.
    #[test]
    fn the_regions_do_not_overlap_on_the_sheet() {
        let all = [
            ("panel", PANEL),
            ("grid", GRID),
            ("slot", SLOT),
            ("slot_selected", SLOT_SELECTED),
            ("tooltip", TOOLTIP),
        ];
        for (i, (name, a)) in all.iter().enumerate() {
            let ar = Rect::from_min_size(pos2(a.origin[0], a.origin[1]), vec2(a.size, a.size));
            assert!(
                ar.max.x <= SHEET && ar.max.y <= SHEET,
                "{name} runs off the sheet"
            );
            assert!(a.inset * 2.0 <= a.size, "{name}'s corners overlap");
            for (other, b) in &all[i + 1..] {
                let br = Rect::from_min_size(pos2(b.origin[0], b.origin[1]), vec2(b.size, b.size));
                // Positive-area overlap, not mere contact: the regions are
                // packed edge to edge on purpose, and `Rect::intersects` counts
                // a shared border as an intersection.
                let shared = ar.intersect(br);
                assert!(
                    shared.width() <= 0.0 || shared.height() <= 0.0,
                    "{name} overlaps {other}"
                );
            }
        }
    }
}

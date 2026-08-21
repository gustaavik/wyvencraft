//! Drawing item icons into egui.
//!
//! The textures are registered with egui once (see [`crate::app`]) to get the
//! [`egui::TextureId`]s these helpers sample. A [`crate::content::ItemIcon`] is
//! one of three things — a flat atlas tile (tools, food, armor, fluids), an
//! isometric cube built from a block's own face tiles, or a cell of the
//! pre-rendered 3D sheet (items with a model file) — and each emits a single
//! [`egui::Mesh`]. Consumed by the inventory screen and the HUD hotbar so they
//! render items identically.

use egui::epaint::{Mesh, Vertex};
use egui::{Color32, Painter, Pos2, Rect, Shape, pos2};

use crate::content::ItemIcon;
use crate::state::UiTextures;
use wyven_render::icons;
use wyven_render::texture::{ATLAS_COLUMNS, ATLAS_SIZE};

/// Isometric cube face tints (gamma space — egui linearizes them). The lit top
/// reads brightest, the two visible sides progressively darker, giving a small
/// tile enough shading to read as a 3D block.
const TOP_TINT: Color32 = Color32::from_gray(255);
const LEFT_TINT: Color32 = Color32::from_gray(204);
const RIGHT_TINT: Color32 = Color32::from_gray(153);

/// Draw `icon` filling `rect`.
///
/// Which texture is sampled depends on the icon: tiles and cubes come from the
/// block atlas, a model comes from its cell of the pre-rendered icon sheet.
/// Taking the whole [`UiTextures`] rather than one id keeps that choice here,
/// where the icon variant is already being matched on, instead of at all seven
/// call sites.
pub fn draw_item_icon(painter: &Painter, rect: Rect, icon: ItemIcon, tex: UiTextures) {
    let mut mesh = Mesh::with_texture(tex.atlas);
    match icon {
        ItemIcon::Flat(tile) => {
            let [u0, v0, u1, v1] = tile_uv(tile);
            quad(
                &mut mesh,
                [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ],
                [pos2(u0, v0), pos2(u1, v0), pos2(u1, v1), pos2(u0, v1)],
                Color32::WHITE,
            );
        }
        ItemIcon::Cube { top, left, right } => cube(&mut mesh, rect, top, left, right),
        ItemIcon::Model(id) => {
            mesh = Mesh::with_texture(tex.model_icons);
            // The cell is already square and pre-shaded; draw it 1:1 into the
            // slot. Sampling is linear, so the render is downscaled smoothly
            // rather than aliased into the (usually smaller) slot.
            let [u0, v0, u1, v1] = icons::cell_uv(id.0, tex.model_count);
            quad(
                &mut mesh,
                [
                    rect.left_top(),
                    rect.right_top(),
                    rect.right_bottom(),
                    rect.left_bottom(),
                ],
                [pos2(u0, v0), pos2(u1, v0), pos2(u1, v1), pos2(u0, v1)],
                Color32::WHITE,
            );
        }
    }
    painter.add(Shape::mesh(mesh));
}

/// Emit the three visible faces of an isometric cube inscribed in `rect`.
fn cube(mesh: &mut Mesh, rect: Rect, top: u32, left: u32, right: u32) {
    let c = rect.center();
    let s = 0.5 * rect.width().min(rect.height());
    // Hexagon silhouette of a cube seen from a top corner.
    let top_v = pos2(c.x, c.y - s);
    let upper_l = pos2(c.x - s, c.y - s * 0.5);
    let upper_r = pos2(c.x + s, c.y - s * 0.5);
    let mid = pos2(c.x, c.y);
    let lower_l = pos2(c.x - s, c.y + s * 0.5);
    let lower_r = pos2(c.x + s, c.y + s * 0.5);
    let bottom = pos2(c.x, c.y + s);

    let [tu0, tv0, tu1, tv1] = tile_uv(top);
    quad(
        mesh,
        [top_v, upper_r, mid, upper_l],
        [
            pos2(tu0, tv0),
            pos2(tu1, tv0),
            pos2(tu1, tv1),
            pos2(tu0, tv1),
        ],
        TOP_TINT,
    );

    let [lu0, lv0, lu1, lv1] = tile_uv(left);
    quad(
        mesh,
        [upper_l, mid, bottom, lower_l],
        [
            pos2(lu0, lv0),
            pos2(lu1, lv0),
            pos2(lu1, lv1),
            pos2(lu0, lv1),
        ],
        LEFT_TINT,
    );

    let [ru0, rv0, ru1, rv1] = tile_uv(right);
    quad(
        mesh,
        [upper_r, lower_r, bottom, mid],
        [
            pos2(ru0, rv0),
            pos2(ru0, rv1),
            pos2(ru1, rv1),
            pos2(ru1, rv0),
        ],
        RIGHT_TINT,
    );
}

/// Push one textured quad (two triangles) into `mesh`.
fn quad(mesh: &mut Mesh, pts: [Pos2; 4], uvs: [Pos2; 4], color: Color32) {
    let base = mesh.vertices.len() as u32;
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

/// Normalized atlas UVs `[u0, v0, u1, v1]` for `tile`, inset half a texel so
/// nearest sampling can't bleed into the neighbouring tile.
fn tile_uv(tile: u32) -> [f32; 4] {
    let cols = ATLAS_COLUMNS as f32;
    let inset = 0.5 / ATLAS_SIZE as f32;
    let tx = (tile % ATLAS_COLUMNS) as f32;
    let ty = (tile / ATLAS_COLUMNS) as f32;
    [
        tx / cols + inset,
        ty / cols + inset,
        (tx + 1.0) / cols - inset,
        (ty + 1.0) / cols - inset,
    ]
}

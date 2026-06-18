//! In-world HUD overlays drawn with egui: crosshair, hotbar, and the F3 debug
//! panel. These are pure draw helpers; gameplay state is passed in by reference.

use egui::{Align2, Color32, Context, Stroke};

use crate::inventory::{HOTBAR_SIZE, Inventory, ItemRegistry};

/// Draw a simple crosshair at the screen centre.
pub fn draw_crosshair(ctx: &Context) {
    let screen = ctx.screen_rect();
    let center = screen.center();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let len = 8.0;
    let stroke = Stroke::new(2.0, Color32::from_white_alpha(180));
    painter.line_segment(
        [center - egui::vec2(len, 0.0), center + egui::vec2(len, 0.0)],
        stroke,
    );
    painter.line_segment(
        [center - egui::vec2(0.0, len), center + egui::vec2(0.0, len)],
        stroke,
    );
}

/// Draw the hotbar (9 slots) anchored to the bottom-centre, highlighting the
/// selected slot and labelling each with its item + count.
pub fn draw_hotbar(ctx: &Context, inventory: &Inventory, items: &ItemRegistry) {
    let slot = 48.0;
    let pad = 4.0;
    let width = HOTBAR_SIZE as f32 * (slot + pad) + pad;

    egui::Area::new(egui::Id::new("hotbar"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -8.0))
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(width, slot + 2.0 * pad), egui::Sense::hover());
            let painter = ui.painter();
            painter.rect_filled(rect, 4.0, Color32::from_black_alpha(120));

            for i in 0..HOTBAR_SIZE {
                let x = rect.left() + pad + i as f32 * (slot + pad);
                let cell = egui::Rect::from_min_size(
                    egui::pos2(x, rect.top() + pad),
                    egui::vec2(slot, slot),
                );
                let selected = i == inventory.selected_index();
                painter.rect_filled(cell, 3.0, Color32::from_white_alpha(20));
                if selected {
                    painter.rect_stroke(
                        cell,
                        3.0,
                        Stroke::new(2.5, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }

                if let Some(stack) = inventory.slot(i) {
                    let name = &items.get(stack.item).name;
                    let label: String = name.chars().take(3).collect();
                    painter.text(
                        cell.center(),
                        Align2::CENTER_CENTER,
                        label,
                        egui::FontId::monospace(13.0),
                        Color32::WHITE,
                    );
                    if stack.count > 1 {
                        painter.text(
                            cell.right_bottom() - egui::vec2(2.0, 1.0),
                            Align2::RIGHT_BOTTOM,
                            stack.count.to_string(),
                            egui::FontId::proportional(12.0),
                            Color32::YELLOW,
                        );
                    }
                    // Tool durability bar along the bottom of the cell.
                    if let (Some(dur), Some(max)) =
                        (stack.durability, items.max_durability(stack.item))
                        && max > 0
                        && dur < max
                    {
                        let ratio = dur as f32 / max as f32;
                        let bar_w = slot - 8.0;
                        let track = egui::Rect::from_min_size(
                            egui::pos2(cell.left() + 4.0, cell.bottom() - 7.0),
                            egui::vec2(bar_w, 4.0),
                        );
                        painter.rect_filled(track, 1.0, Color32::from_black_alpha(160));
                        let fill =
                            egui::Rect::from_min_size(track.min, egui::vec2(bar_w * ratio, 4.0));
                        painter.rect_filled(fill, 1.0, durability_color(ratio));
                    }
                }
            }
        });
}

/// Green→red colour for a tool's remaining-durability ratio.
fn durability_color(ratio: f32) -> Color32 {
    let r = ((1.0 - ratio) * 255.0) as u8;
    let g = (ratio * 220.0) as u8;
    Color32::from_rgb(r.max(40), g, 40)
}

/// Draw a heart icon (two lobes + a point) centred at `c`, sized `s`.
fn heart(painter: &egui::Painter, c: egui::Pos2, s: f32, color: Color32) {
    let r = s * 0.27;
    let lobe = r * 0.45;
    painter.circle_filled(c + egui::vec2(-r * 0.55, -lobe), r * 0.62, color);
    painter.circle_filled(c + egui::vec2(r * 0.55, -lobe), r * 0.62, color);
    let left = c + egui::vec2(-r * 1.1, -lobe * 0.2);
    let right = c + egui::vec2(r * 1.1, -lobe * 0.2);
    let tip = c + egui::vec2(0.0, s * 0.5);
    painter.add(egui::Shape::convex_polygon(
        vec![left, right, tip],
        color,
        egui::Stroke::NONE,
    ));
}

/// Draw a row of `count` value-pips (e.g. hearts/food), each worth 2 units, with
/// half-resolution by over-painting the right half of a half-full icon.
fn draw_pips(
    painter: &egui::Painter,
    origin: egui::Pos2,
    rtl: bool,
    count: usize,
    value: f32,
    full: Color32,
    empty: Color32,
) {
    let size = 16.0;
    let step = size + 2.0;
    for i in 0..count {
        let dx = if rtl {
            -(i as f32) * step
        } else {
            i as f32 * step
        };
        let center = origin + egui::vec2(dx, 0.0);
        let remaining = value - i as f32 * 2.0;
        heart(painter, center, size, empty);
        if remaining >= 0.5 {
            heart(painter, center, size, full);
            if remaining < 1.5 {
                // Over-paint the right half to render a half-pip.
                let half = egui::Rect::from_min_size(
                    egui::pos2(center.x, center.y - size * 0.6),
                    egui::vec2(size * 0.7, size * 1.2),
                );
                painter.rect_filled(half, 0.0, empty);
            }
        }
    }
}

/// Draw the survival vitals: a hearts row (health) and a food row (hunger),
/// stacked just above the hotbar.
pub fn draw_vitals(ctx: &Context, health: f32, max_health: f32, hunger: f32, max_hunger: f32) {
    let hearts = (max_health / 2.0).round() as usize;
    let shanks = (max_hunger / 2.0).round() as usize;
    egui::Area::new(egui::Id::new("vitals"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, -70.0))
        .show(ctx, |ui| {
            let width = 240.0;
            let (rect, _) = ui.allocate_exact_size(egui::vec2(width, 22.0), egui::Sense::hover());
            let painter = ui.painter();
            let row_y = rect.center().y;
            // Health hearts grow left→right from the left edge.
            draw_pips(
                painter,
                egui::pos2(rect.left() + 10.0, row_y),
                false,
                hearts,
                health,
                Color32::from_rgb(220, 40, 40),
                Color32::from_rgb(60, 20, 20),
            );
            // Hunger pips grow right→left from the right edge.
            draw_pips(
                painter,
                egui::pos2(rect.right() - 10.0, row_y),
                true,
                shanks,
                hunger,
                Color32::from_rgb(190, 130, 50),
                Color32::from_rgb(55, 40, 20),
            );
        });
}

/// Draw a small break-progress bar just below the crosshair.
pub fn draw_break_progress(ctx: &Context, progress: f32) {
    let screen = ctx.screen_rect();
    let center = screen.center();
    let w = 40.0;
    let h = 5.0;
    let painter = ctx.layer_painter(egui::LayerId::background());
    let track = egui::Rect::from_min_size(
        egui::pos2(center.x - w * 0.5, center.y + 16.0),
        egui::vec2(w, h),
    );
    painter.rect_filled(track, 1.0, Color32::from_black_alpha(160));
    let fill = egui::Rect::from_min_size(track.min, egui::vec2(w * progress.clamp(0.0, 1.0), h));
    painter.rect_filled(fill, 1.0, Color32::from_white_alpha(220));
}

/// Draw the current game-mode label in the top-right corner.
pub fn draw_mode_indicator(ctx: &Context, label: &str) {
    egui::Area::new(egui::Id::new("mode_indicator"))
        .anchor(Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .show(ctx, |ui| {
            ui.label(
                egui::RichText::new(label)
                    .monospace()
                    .color(Color32::WHITE)
                    .background_color(Color32::from_black_alpha(120)),
            );
        });
}

/// Draw the F3-style debug overlay with the provided lines of text.
pub fn draw_debug(ctx: &Context, lines: &[String]) {
    egui::Area::new(egui::Id::new("debug_overlay"))
        .anchor(Align2::LEFT_TOP, egui::vec2(6.0, 6.0))
        .show(ctx, |ui| {
            for line in lines {
                ui.label(
                    egui::RichText::new(line)
                        .monospace()
                        .color(Color32::WHITE)
                        .background_color(Color32::from_black_alpha(140)),
                );
            }
        });
}

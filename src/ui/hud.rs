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
                }
            }
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

//! In-world HUD overlays drawn with egui: crosshair, hotbar, and the F3 debug
//! panel. These are pure draw helpers; gameplay state is passed in by reference.

use egui::{Align2, Color32, Context, Stroke};

use crate::content::ItemIcon;
use crate::inventory::{HOTBAR_SIZE, Inventory, ItemRegistry};
use crate::state::UiTextures;
use crate::ui::icon::draw_item_icon;

/// How far the vitals row floats above the bottom edge, and how tall it is.
/// The held-item label clears both, so the three rows stack rather than overlap.
const VITALS_Y: f32 = -70.0;
const VITALS_HEIGHT: f32 = 22.0;
/// Where the held-item label sits with and without a vitals row beneath it.
const LABEL_Y_ABOVE_VITALS: f32 = VITALS_Y - VITALS_HEIGHT - 4.0;
const LABEL_Y_ABOVE_HOTBAR: f32 = VITALS_Y;

/// Draw a simple crosshair at the screen centre.
pub fn draw_crosshair(ctx: &Context) {
    let screen = ctx.screen_rect();
    let center = screen.center();
    let painter = ctx.layer_painter(egui::LayerId::background());
    let len = 8.0;
    let stroke = Stroke::new(2.0_f32, Color32::from_white_alpha(180));
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
/// selected slot and drawing each item's icon + count.
pub fn draw_hotbar(
    ctx: &Context,
    inventory: &Inventory,
    items: &ItemRegistry,
    icons: &[ItemIcon],
    tex: UiTextures,
) {
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
                        Stroke::new(2.5_f32, Color32::WHITE),
                        egui::StrokeKind::Inside,
                    );
                }

                if let Some(stack) = inventory.slot(i) {
                    draw_item_icon(painter, cell.shrink(6.0), icons[stack.item.0 as usize], tex);
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

/// Name the item the player is holding, centred above the hotbar and fading
/// out — the timing is [`HeldLabel`](crate::inventory::HeldLabel)'s, this only
/// paints the result at the opacity it reports.
///
/// `above_vitals` must match whether [`draw_vitals`] is being drawn this frame,
/// or the label lands on top of the hearts.
pub fn draw_held_label(ctx: &Context, name: &str, alpha: f32, above_vitals: bool) {
    if alpha <= 0.0 || name.is_empty() {
        return;
    }
    let y = if above_vitals {
        LABEL_Y_ABOVE_VITALS
    } else {
        LABEL_Y_ABOVE_HOTBAR
    };
    let fade = |c: Color32| {
        Color32::from_rgba_unmultiplied(c.r(), c.g(), c.b(), (alpha * c.a() as f32) as u8)
    };
    egui::Area::new(egui::Id::new("hotbar_label"))
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, y))
        .interactable(false)
        .show(ctx, |ui| {
            let font = egui::FontId::proportional(17.0);
            // Measure first: the label is centred on the screen, not on a box
            // whose width we had to guess.
            let galley =
                ui.painter()
                    .layout_no_wrap(name.to_string(), font.clone(), fade(Color32::WHITE));
            let size = galley.size();
            let (rect, _) = ui.allocate_exact_size(size, egui::Sense::hover());
            let painter = ui.painter();
            // A shadow rather than a panel: the name has to stay readable over
            // bright terrain without boxing in the middle of the screen.
            painter.galley(
                rect.min + egui::vec2(1.0, 1.0),
                painter.layout_no_wrap(
                    name.to_string(),
                    font,
                    fade(Color32::from_black_alpha(200)),
                ),
                Color32::PLACEHOLDER,
            );
            painter.galley(rect.min, galley, Color32::PLACEHOLDER);
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
        .anchor(Align2::CENTER_BOTTOM, egui::vec2(0.0, VITALS_Y))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Run one headless egui frame and report how many shapes it emitted.
    /// Enough to tell "painted the label" from "painted nothing" without a GPU.
    fn shapes_from(draw: impl Fn(&Context)) -> usize {
        let ctx = Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        // Two frames: an anchored `Area` is placed from the size it had last
        // frame, so the first pass is the one that measures it.
        let _ = ctx.run(input(), |ctx| draw(ctx));
        let output = ctx.run(input(), |ctx| draw(ctx));
        output
            .shapes
            .iter()
            .filter(|s| !matches!(s.shape, egui::Shape::Noop))
            .count()
    }

    #[test]
    fn a_visible_held_label_paints_something() {
        let painted = shapes_from(|ctx| draw_held_label(ctx, "Wooden Pickaxe", 1.0, true));
        assert!(painted > 0, "nothing was painted");
    }

    /// The fade must actually stop drawing, not just draw at alpha 0 — an
    /// invisible label still costs a layer and can still catch a hover.
    #[test]
    fn a_faded_out_label_paints_nothing() {
        assert_eq!(
            shapes_from(|ctx| draw_held_label(ctx, "Bread", 0.0, true)),
            0
        );
        assert_eq!(shapes_from(|ctx| draw_held_label(ctx, "", 1.0, true)), 0);
    }

    /// With a vitals row beneath it the label has to clear the hearts; without
    /// one it drops into the space they would have taken. Compared as painted
    /// rectangles rather than as constants, so the check still means something
    /// after someone re-tunes the offsets.
    #[test]
    fn the_label_clears_the_vitals_row() {
        let bounds = |above_vitals: bool| {
            let ctx = Context::default();
            let input = || egui::RawInput {
                screen_rect: Some(egui::Rect::from_min_size(
                    egui::Pos2::ZERO,
                    egui::vec2(1280.0, 720.0),
                )),
                ..Default::default()
            };
            let run = |ctx: &Context| {
                draw_held_label(ctx, "Wooden Pickaxe", 1.0, above_vitals);
                draw_vitals(ctx, 20.0, 20.0, 20.0, 20.0);
            };
            let _ = ctx.run(input(), run);
            let _ = ctx.run(input(), run);
            (
                ctx.memory(|m| m.area_rect(egui::Id::new("hotbar_label"))),
                ctx.memory(|m| m.area_rect(egui::Id::new("vitals"))),
            )
        };

        let (label, vitals) = bounds(true);
        let (label, vitals) = (label.expect("label area"), vitals.expect("vitals area"));
        assert!(
            label.bottom() <= vitals.top(),
            "label {label:?} overlaps the hearts {vitals:?}"
        );

        let lower = bounds(false).0.expect("label area");
        assert!(
            lower.top() > label.top(),
            "with no vitals the label should sit lower, got {lower:?} vs {label:?}"
        );
    }
}

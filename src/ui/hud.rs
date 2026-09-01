//! In-world HUD overlays drawn with egui: crosshair, hotbar, and the F3 debug
//! panel. These are pure draw helpers; gameplay state is passed in by reference.

use egui::{Align2, Color32, Context, Stroke};

use crate::content::ItemIcon;
use crate::inventory::{HOTBAR_SIZE, Inventory, ItemRegistry};
use crate::state::UiTextures;
use crate::ui::slot;

/// How far the vitals row floats above the bottom edge, and how tall it is.
/// The held-item label clears both, so the three rows stack rather than overlap.
const VITALS_Y: f32 = -70.0;
const VITALS_HEIGHT: f32 = 22.0;
/// Where the held-item label sits with and without a vitals row beneath it.
const LABEL_Y_ABOVE_VITALS: f32 = VITALS_Y - VITALS_HEIGHT - 4.0;
const LABEL_Y_ABOVE_HOTBAR: f32 = VITALS_Y;
/// How far the hotbar floats above the bottom edge.
const HOTBAR_MARGIN: f32 = -8.0;

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

/// Where the hotbar sits, without drawing it.
///
/// Computed rather than read back from egui: the inventory panel animates out
/// of this rect, and it needs it on the very frame the player presses E — when
/// `ctx.memory().area_rect(..)` would still be `None`, because an anchored
/// `Area` is placed from the size it had *last* frame. [`draw_hotbar`] places
/// itself from this same function, so the two cannot drift.
pub fn hotbar_rect(screen: egui::Rect) -> egui::Rect {
    let size = egui::vec2(
        HOTBAR_SIZE as f32 * slot::PITCH + slot::GAP,
        slot::SIZE + 2.0 * slot::GAP,
    );
    Align2::CENTER_BOTTOM
        .align_size_within_rect(size, screen)
        .translate(egui::vec2(0.0, HOTBAR_MARGIN))
}

/// The cell rect of hotbar slot `index` within a hotbar occupying `rect`.
pub fn hotbar_cell(rect: egui::Rect, index: usize) -> egui::Rect {
    egui::Rect::from_min_size(
        rect.min + egui::vec2(slot::GAP + index as f32 * slot::PITCH, slot::GAP),
        egui::vec2(slot::SIZE, slot::SIZE),
    )
}

/// Draw the hotbar (9 slots) anchored to the bottom-centre, highlighting the
/// selected slot and drawing each item's icon + count.
///
/// Painted through [`slot::paint_slot`], the same painter the inventory grid's
/// bottom row uses — they are the same nine slots, and on the frame the panel
/// takes over they are drawn in the same place.
pub fn draw_hotbar(
    ctx: &Context,
    inventory: &Inventory,
    items: &ItemRegistry,
    icons: &[ItemIcon],
    tex: UiTextures,
) {
    let rect = hotbar_rect(ctx.screen_rect());
    let painter = ctx.layer_painter(egui::LayerId::background());
    draw_hotbar_row(&painter, rect, inventory, items, icons, Color32::WHITE, tex);
}

/// The hotbar's nine cells and their backing, drawn into `rect`.
///
/// Shared with the inventory panel's bottom row, which is why it takes a
/// painter and a rect rather than reaching for the screen itself.
pub fn draw_hotbar_row(
    painter: &egui::Painter,
    rect: egui::Rect,
    inventory: &Inventory,
    items: &ItemRegistry,
    icons: &[ItemIcon],
    tint: Color32,
    tex: UiTextures,
) {
    for i in 0..HOTBAR_SIZE {
        let cell = hotbar_cell(rect, i);
        let contents = inventory.slot(i).map(|stack| slot::SlotContents {
            stack,
            icon: icons[stack.item.0 as usize],
            items,
        });
        let selected = i == inventory.selected_index();
        slot::paint_slot(painter, cell, contents, None, selected, tint, tex);
    }
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

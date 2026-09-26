//! The boss bar: title, health, phase and the attack being wound up, across
//! the top of the screen while a boss is near.

use egui::{Align2, Color32, Context, FontId, Stroke};

/// Everything the bar shows, worked out by the in-game state.
#[derive(Debug, Clone, PartialEq)]
pub struct BossBarView {
    pub title: String,
    /// Health over maximum, `0..=1`.
    pub fraction: f32,
    /// 0 is the opening phase; each later one is angrier.
    pub phase: u8,
    /// The attack being wound up right now, if any — the telegraph.
    pub telegraph: Option<String>,
}

const WIDTH: f32 = 420.0;
const BAR_HEIGHT: f32 = 12.0;
/// Below the compass strip.
const TOP: f32 = 58.0;

const FILL: Color32 = Color32::from_rgb(178, 34, 34);
const ENRAGED: Color32 = Color32::from_rgb(214, 88, 20);
const TROUGH: Color32 = Color32::from_rgb(40, 12, 12);
const WARNING: Color32 = Color32::from_rgb(255, 196, 64);

/// Roman numerals for the phase label; a boss with more phases than this reads
/// its number instead.
const NUMERALS: [&str; 5] = ["I", "II", "III", "IV", "V"];

pub fn phase_label(phase: u8) -> String {
    NUMERALS
        .get(usize::from(phase))
        .map_or_else(|| (phase + 1).to_string(), |n| (*n).to_string())
}

pub fn draw_boss_bar(ctx: &Context, view: &BossBarView) {
    egui::Area::new(egui::Id::new("boss_bar"))
        .anchor(Align2::CENTER_TOP, egui::vec2(0.0, TOP))
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) =
                ui.allocate_exact_size(egui::vec2(WIDTH, BAR_HEIGHT + 40.0), egui::Sense::hover());
            let painter = ui.painter();
            let title = format!("{} — Phase {}", view.title, phase_label(view.phase));
            painter.text(
                egui::pos2(rect.center().x, rect.top()),
                Align2::CENTER_TOP,
                title,
                FontId::proportional(16.0),
                Color32::WHITE,
            );
            let bar = egui::Rect::from_min_size(
                egui::pos2(rect.left(), rect.top() + 20.0),
                egui::vec2(WIDTH, BAR_HEIGHT),
            );
            painter.rect_filled(bar, 2.0, TROUGH);
            let mut filled = bar;
            filled.set_width(WIDTH * view.fraction.clamp(0.0, 1.0));
            let colour = if view.phase > 0 { ENRAGED } else { FILL };
            painter.rect_filled(filled, 2.0, colour);
            painter.rect_stroke(
                bar,
                2.0,
                Stroke::new(1.0_f32, Color32::BLACK),
                egui::StrokeKind::Outside,
            );
            if let Some(attack) = &view.telegraph {
                painter.text(
                    egui::pos2(rect.center().x, bar.bottom() + 4.0),
                    Align2::CENTER_TOP,
                    format!("⚠ {attack}!"),
                    FontId::proportional(15.0),
                    WARNING,
                );
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn phases_read_as_numerals() {
        assert_eq!(phase_label(0), "I");
        assert_eq!(phase_label(1), "II");
        assert_eq!(phase_label(7), "8");
    }

    #[test]
    fn the_bar_paints() {
        let ctx = Context::default();
        let view = BossBarView {
            title: "The Elder Stag".into(),
            fraction: 0.4,
            phase: 1,
            telegraph: Some("Stomp".into()),
        };
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| draw_boss_bar(ctx, &view));
        let output = ctx.run(input(), |ctx| draw_boss_bar(ctx, &view));
        assert!(output.shapes.len() >= 4, "title, trough, fill, telegraph");
    }
}

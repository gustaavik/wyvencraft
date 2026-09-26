//! The compass strip across the top of the screen: the cardinal points and a
//! marker for every structure a shrine has revealed.
//!
//! It shows the half of the horizon in front of the player (±90°). A marker
//! for something behind is pinned to the nearer edge with an arrow, so a
//! revealed altar is never simply missing from the screen.

use egui::{Align2, Color32, Context, FontId, Stroke};

/// One revealed place, as seen from the player.
#[derive(Debug, Clone, PartialEq)]
pub struct Waypoint {
    pub label: String,
    /// Radians to turn to face it: positive clockwise (the way yaw grows).
    pub angle: f32,
    pub distance: f32,
}

const HALF_WIDTH: f32 = 220.0;
const HEIGHT: f32 = 22.0;
/// The field of view the strip spans either side of centre.
const HALF_SPAN: f32 = std::f32::consts::FRAC_PI_2;

const CARDINALS: [(&str, f32); 8] = [
    ("N", 0.0),
    ("NE", std::f32::consts::FRAC_PI_4),
    ("E", std::f32::consts::FRAC_PI_2),
    ("SE", 3.0 * std::f32::consts::FRAC_PI_4),
    ("S", std::f32::consts::PI),
    ("SW", -3.0 * std::f32::consts::FRAC_PI_4),
    ("W", -std::f32::consts::FRAC_PI_2),
    ("NW", -std::f32::consts::FRAC_PI_4),
];

/// How far in from each end a cardinal label must sit to be drawn at all.
const LABEL_MARGIN: f32 = 10.0;

const MARKER: Color32 = Color32::from_rgb(110, 235, 255);

/// Where on the strip an angle lands, as an offset from its centre, and
/// whether it is actually in view (off-view angles are pinned to an edge).
pub fn strip_offset(angle: f32) -> (f32, bool) {
    let visible = angle.abs() <= HALF_SPAN;
    let clamped = angle.clamp(-HALF_SPAN, HALF_SPAN);
    (clamped / HALF_SPAN * HALF_WIDTH, visible)
}

/// Wrap an angle into `(-π, π]`.
fn wrap(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let a = (angle + PI).rem_euclid(TAU) - PI;
    if a <= -PI { a + TAU } else { a }
}

/// Draw the strip for a player facing `yaw`, with `waypoints` marked.
pub fn draw_compass(ctx: &Context, yaw: f32, waypoints: &[Waypoint]) {
    egui::Area::new(egui::Id::new("compass"))
        .anchor(Align2::CENTER_TOP, egui::vec2(0.0, 8.0))
        .interactable(false)
        .show(ctx, |ui| {
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(HALF_WIDTH * 2.0, HEIGHT + 18.0),
                egui::Sense::hover(),
            );
            let painter = ui.painter();
            let strip = egui::Rect::from_min_size(rect.min, egui::vec2(HALF_WIDTH * 2.0, HEIGHT));
            painter.rect_filled(strip, 3.0, Color32::from_black_alpha(110));
            let centre = strip.center();
            for (label, heading) in CARDINALS {
                let (dx, visible) = strip_offset(wrap(heading - yaw));
                // Keep whole labels inside the strip rather than half-drawn
                // over its rounded end.
                if !visible || dx.abs() > HALF_WIDTH - LABEL_MARGIN {
                    continue;
                }
                let major = label.len() == 1;
                painter.text(
                    egui::pos2(centre.x + dx, centre.y),
                    Align2::CENTER_CENTER,
                    label,
                    FontId::monospace(if major { 14.0 } else { 10.0 }),
                    if label == "N" {
                        Color32::from_rgb(230, 80, 70)
                    } else {
                        Color32::from_gray(if major { 235 } else { 160 })
                    },
                );
            }
            // Centre notch: where the player is facing.
            painter.line_segment(
                [
                    egui::pos2(centre.x, strip.bottom() - 4.0),
                    egui::pos2(centre.x, strip.bottom()),
                ],
                Stroke::new(2.0_f32, Color32::WHITE),
            );
            for waypoint in waypoints {
                let (dx, visible) = strip_offset(waypoint.angle);
                let at = egui::pos2(centre.x + dx, strip.top() + 4.0);
                let marker = if visible {
                    "◆"
                } else if dx < 0.0 {
                    "◀"
                } else {
                    "▶"
                };
                painter.text(
                    at,
                    Align2::CENTER_TOP,
                    marker,
                    FontId::proportional(13.0),
                    MARKER,
                );
                if visible && dx.abs() < HALF_WIDTH * 0.5 {
                    painter.text(
                        egui::pos2(at.x, strip.bottom() + 2.0),
                        Align2::CENTER_TOP,
                        format!("{} · {:.0} m", waypoint.label, waypoint.distance),
                        FontId::proportional(12.0),
                        MARKER,
                    );
                }
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn straight_ahead_is_the_centre_and_the_edges_are_a_quarter_turn() {
        assert_eq!(strip_offset(0.0), (0.0, true));
        assert_eq!(strip_offset(HALF_SPAN), (HALF_WIDTH, true));
        assert_eq!(strip_offset(-HALF_SPAN), (-HALF_WIDTH, true));
    }

    #[test]
    fn something_behind_is_pinned_to_the_nearer_edge() {
        assert_eq!(strip_offset(2.5), (HALF_WIDTH, false));
        assert_eq!(strip_offset(-3.0), (-HALF_WIDTH, false));
    }

    #[test]
    fn north_is_ahead_at_yaw_zero_and_east_to_the_right() {
        assert_eq!(strip_offset(wrap(0.0 - 0.0)).0, 0.0);
        let (east, _) = strip_offset(wrap(std::f32::consts::FRAC_PI_2 - 0.0));
        assert!(east > 0.0);
    }
}

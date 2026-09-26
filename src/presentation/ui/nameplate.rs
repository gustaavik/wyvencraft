//! Usernames floating above other players.
//!
//! Painted as 2D egui text at a projected screen position, rather than as
//! world-space geometry. egui composites over the finished world image every
//! frame (`GuiConfig { is_overlay: true }`), so this costs no GPU work, no
//! texture upload and no new pipeline — and the text stays crisp at any
//! distance instead of being a stretched quad.
//!
//! What that costs is depth: egui knows nothing about the depth buffer, so a
//! name would show through a mountain. [`Nameplate::occluded`] is the caller's
//! answer to that — see `InGameState::nameplates`.

use egui::{Color32, FontId, Stroke, Vec2};
use glam::Vec3;

use wyven_render::Camera;

/// Beyond this many blocks a name is not drawn at all.
///
/// Roughly Minecraft's 64-block cap. Far enough to find someone across a
/// clearing, short enough that a busy server is not a wall of text.
pub const MAX_DISTANCE: f32 = 48.0;

/// Distance at which names begin to fade, so they thin out rather than
/// vanishing mid-step.
const FADE_START: f32 = 36.0;

/// Font size at the player's feet and at [`MAX_DISTANCE`].
///
/// Clamped rather than scaled by true perspective: a name that shrinks with
/// distance the way geometry does becomes unreadable long before it becomes
/// uninteresting.
const NEAR_FONT: f32 = 15.0;
const FAR_FONT: f32 = 11.0;

/// How far above a player's feet the plate sits.
///
/// The rigged player model is scaled in `assets/entities.toml` so its head tops
/// out at the collision height, 1.8; the extra clearance keeps the text off the
/// hair.
pub const ANCHOR_HEIGHT: f32 = 2.05;

/// One player's plate, ready to draw.
pub struct Nameplate<'a> {
    pub name: &'a str,
    /// Feet position, interpolated for this frame.
    pub position: Vec3,
    /// Whether the world is between the camera and this player. Computed by the
    /// caller, which is the only place that has the world to raycast against.
    pub occluded: bool,
}

/// Draw a nameplate for each visible player.
///
/// Painted on the background layer so it sits under the HUD and any open panel —
/// a name should never cover the inventory.
pub fn draw_nameplates<'a>(
    egui_ctx: &egui::Context,
    camera: &Camera,
    plates: impl IntoIterator<Item = Nameplate<'a>>,
) {
    let screen = egui_ctx.screen_rect();
    let painter = egui_ctx.layer_painter(egui::LayerId::background());

    for plate in plates {
        if plate.occluded {
            continue;
        }

        let distance = camera.position.distance(plate.position);
        if distance > MAX_DISTANCE {
            continue;
        }

        let anchor = plate.position + Vec3::Y * ANCHOR_HEIGHT;
        let Some(normalized) = camera.project(anchor) else {
            continue;
        };

        // Off-screen plates are dropped rather than clamped to an edge: a name
        // pinned to the border reads as a marker for something *there*, which is
        // exactly wrong.
        if !(0.0..=1.0).contains(&normalized.x) || !(0.0..=1.0).contains(&normalized.y) {
            continue;
        }

        let pos = egui::pos2(
            screen.left() + normalized.x * screen.width(),
            screen.top() + normalized.y * screen.height(),
        );

        let alpha = fade_alpha(distance);
        let font = FontId::proportional(font_size(distance));
        let text_color = Color32::WHITE.gamma_multiply(alpha);

        // Measured first so the backing plate can be sized to the text. Without
        // it, a pale name over snow or sand is unreadable.
        let galley = painter.layout_no_wrap(plate.name.to_owned(), font, text_color);
        let size = galley.size();
        let padding = Vec2::new(5.0, 2.0);
        let rect = egui::Rect::from_center_size(
            egui::pos2(pos.x, pos.y - size.y * 0.5),
            size + padding * 2.0,
        );

        painter.rect(
            rect,
            4.0,
            Color32::from_black_alpha(140).gamma_multiply(alpha),
            Stroke::NONE,
            egui::StrokeKind::Middle,
        );
        painter.galley(
            egui::pos2(
                rect.center().x - size.x * 0.5,
                rect.center().y - size.y * 0.5,
            ),
            galley,
            text_color,
        );
    }
}

/// Opacity for a plate at `distance`, fading out over the last stretch.
fn fade_alpha(distance: f32) -> f32 {
    if distance <= FADE_START {
        return 1.0;
    }
    let span = MAX_DISTANCE - FADE_START;
    (1.0 - (distance - FADE_START) / span).clamp(0.0, 1.0)
}

/// Font size for a plate at `distance`, interpolated between the two bounds.
fn font_size(distance: f32) -> f32 {
    let t = (distance / MAX_DISTANCE).clamp(0.0, 1.0);
    NEAR_FONT + (FAR_FONT - NEAR_FONT) * t
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn nearby_plates_are_fully_opaque() {
        assert_eq!(fade_alpha(0.0), 1.0);
        assert_eq!(fade_alpha(FADE_START), 1.0);
    }

    #[test]
    fn plates_fade_out_over_the_last_stretch() {
        let midpoint = fade_alpha((FADE_START + MAX_DISTANCE) * 0.5);
        assert!(
            (midpoint - 0.5).abs() < 1.0e-5,
            "expected a linear fade, got {midpoint}"
        );
        assert_eq!(fade_alpha(MAX_DISTANCE), 0.0);
    }

    /// Alpha must never leave `[0,1]`; egui would silently misrender it.
    #[test]
    fn fade_stays_in_range_beyond_the_cutoff() {
        for distance in [MAX_DISTANCE + 1.0, 1_000.0, f32::MAX] {
            let alpha = fade_alpha(distance);
            assert!((0.0..=1.0).contains(&alpha), "alpha was {alpha}");
        }
    }

    #[test]
    fn font_shrinks_with_distance_but_stays_readable() {
        assert_eq!(font_size(0.0), NEAR_FONT);
        assert_eq!(font_size(MAX_DISTANCE), FAR_FONT);
        assert!(
            font_size(MAX_DISTANCE * 2.0) >= FAR_FONT,
            "must not keep shrinking"
        );

        let near = font_size(1.0);
        let far = font_size(MAX_DISTANCE - 1.0);
        assert!(near > far, "closer names should be larger");
    }

    /// The plate sits above the model, not inside it.
    #[test]
    fn the_anchor_clears_the_head() {
        // The player model is fitted to its 1.8-block collision box, so that is
        // where the top of the head is.
        const { assert!(ANCHOR_HEIGHT > 1.8) };
    }
}

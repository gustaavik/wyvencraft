//! The chat overlay: a message log above the hotbar and, when open, the input
//! line beneath it.
//!
//! Like every other view here it draws and reports — it never touches game
//! state. The one exception is `draft`, which a `TextEdit` has to own; the
//! caller reads it back when the returned action says to.

use std::path::PathBuf;

use egui::{Align2, Color32, Context, CursorIcon, Key, RichText, Sense};

use crate::chat::{ChatLog, log::FADE_SECONDS};
use crate::net::ChatKind;

/// Vertical gap from the bottom of the screen, clearing the hotbar.
const BOTTOM_MARGIN: f32 = 68.0;
/// Width of the log and the input line.
const WIDTH: f32 = 520.0;
/// Lines shown on the HUD with the chat closed.
const HUD_LINES: usize = 10;
/// Lines shown in the scrollback with the chat open.
const OPEN_LINES: usize = 20;
/// Fraction of [`FADE_SECONDS`] a line stays fully opaque before fading out.
const OPAQUE_FRACTION: f32 = 0.75;

/// Colour of a clickable file name. Distinct from every [`ChatKind`] colour so
/// a link never reads as an ordinary word in a system message.
const LINK_COLOR: Color32 = Color32::from_rgb(120, 190, 255);

/// Everything the chat overlay reports back after a frame.
///
/// Two independent fields rather than one enum, because both can happen at once:
/// clicking a link also takes focus off the composer, which closes it. Folding
/// them together would make one of the two silently lose.
#[derive(Default)]
pub struct ChatOutcome {
    /// What the player did on the input line, if anything.
    pub action: Option<ChatAction>,
    /// A line's file name was clicked; the caller shows this file.
    pub open: Option<PathBuf>,
}

/// What the player did on the chat line this frame.
pub enum ChatAction {
    /// Enter — the caller reads `draft` and sends it.
    Submit,
    /// Escape or focus lost — discard the draft.
    Cancel,
    /// Up — recall an earlier message.
    HistoryPrev,
    /// Down — walk back toward a blank line.
    HistoryNext,
}

/// Draw the chat log and, when `open`, the input line.
///
/// `focus` requests keyboard focus for this frame; the caller sets it once, on
/// the frame the line opens. Focus is what makes typing safe: egui consumes the
/// key events, so they never reach gameplay input.
pub fn draw_chat(
    ctx: &Context,
    log: &ChatLog,
    open: bool,
    draft: &mut String,
    focus: bool,
) -> ChatOutcome {
    let mut outcome = ChatOutcome::default();

    egui::Area::new(egui::Id::new("chat"))
        .anchor(Align2::LEFT_BOTTOM, egui::vec2(8.0, -BOTTOM_MARGIN))
        .show(ctx, |ui| {
            ui.set_width(WIDTH);
            ui.vertical(|ui| {
                outcome.open = draw_log(ui, log, open);
                if open {
                    outcome.action = draw_input(ui, draft, focus);
                }
            });
        });

    outcome
}

/// The message list. Closed, it shows only lines still inside the fade window;
/// open, it shows the recent scrollback at full opacity so you can read back.
///
/// Returns the path of a file name that was clicked, if any. Clicking is only
/// reachable with the composer open — the cursor is grabbed for mouse-look the
/// rest of the time — which is the same bargain Minecraft makes with its links.
fn draw_log(ui: &mut egui::Ui, log: &ChatLog, open: bool) -> Option<PathBuf> {
    // Collect from the back so the newest `n` are kept, then draw oldest-first.
    let mut lines: Vec<_> = if open {
        log.lines().rev().take(OPEN_LINES).collect()
    } else {
        log.recent().rev().take(HUD_LINES).collect()
    };
    lines.reverse();

    let mut clicked = None;
    for line in lines {
        let alpha = if open { 1.0 } else { fade_alpha(line.age) };
        let body = |text: &str, color: Color32| {
            RichText::new(text)
                .monospace()
                .color(with_alpha(color, alpha))
                .background_color(Color32::from_black_alpha((140.0 * alpha) as u8))
        };

        let Some(path) = line.link.as_ref() else {
            ui.label(body(&line.text, kind_color(line.kind)));
            continue;
        };

        // The name is read off the path, never stored twice, so the label and
        // the file it opens cannot drift apart.
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();

        ui.horizontal(|ui| {
            // The prose and the name are one continuous band; default spacing
            // would cut a gap in the background between them.
            ui.spacing_mut().item_spacing.x = 0.0;
            ui.label(body(&line.text, kind_color(line.kind)));
            let response = ui
                .add(egui::Label::new(body(&name, LINK_COLOR).underline()).sense(Sense::click()))
                .on_hover_cursor(CursorIcon::PointingHand)
                .on_hover_text(path.display().to_string());
            if response.clicked() {
                clicked = Some(path.clone());
            }
        });
    }
    clicked
}

/// The input line. Returns the action the player took on it.
fn draw_input(ui: &mut egui::Ui, draft: &mut String, focus: bool) -> Option<ChatAction> {
    let response = ui.add(
        egui::TextEdit::singleline(draft)
            .desired_width(WIDTH)
            .font(egui::TextStyle::Monospace)
            .hint_text("say something, or /help"),
    );
    if focus {
        response.request_focus();
    }

    // Escape and the arrows are read from egui rather than from `InputState`:
    // with the widget focused, egui consumes them and gameplay never sees them.
    if ui.input(|i| i.key_pressed(Key::Escape)) {
        return Some(ChatAction::Cancel);
    }
    if ui.input(|i| i.key_pressed(Key::ArrowUp)) {
        return Some(ChatAction::HistoryPrev);
    }
    if ui.input(|i| i.key_pressed(Key::ArrowDown)) {
        return Some(ChatAction::HistoryNext);
    }
    if response.lost_focus() {
        // Enter submits; losing focus any other way (a click elsewhere) closes.
        return Some(if ui.input(|i| i.key_pressed(Key::Enter)) {
            ChatAction::Submit
        } else {
            ChatAction::Cancel
        });
    }
    None
}

fn kind_color(kind: ChatKind) -> Color32 {
    match kind {
        ChatKind::Player => Color32::WHITE,
        ChatKind::System => Color32::from_rgb(240, 210, 90),
        ChatKind::Error => Color32::from_rgb(235, 90, 80),
    }
}

/// Opacity for a line of this age: fully opaque for most of the window, then a
/// linear fade so lines leave quietly instead of blinking out.
fn fade_alpha(age: f32) -> f32 {
    let opaque_until = FADE_SECONDS * OPAQUE_FRACTION;
    if age <= opaque_until {
        return 1.0;
    }
    ((FADE_SECONDS - age) / (FADE_SECONDS - opaque_until)).clamp(0.0, 1.0)
}

fn with_alpha(color: Color32, alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(
        color.r(),
        color.g(),
        color.b(),
        (color.a() as f32 * alpha) as u8,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A line is readable for most of its life and only fades at the end — the
    /// alternative (fading from the moment it arrives) makes chat hard to read.
    #[test]
    fn a_line_stays_opaque_before_it_fades_out() {
        assert_eq!(fade_alpha(0.0), 1.0);
        assert_eq!(fade_alpha(FADE_SECONDS * OPAQUE_FRACTION), 1.0);
        let midway = fade_alpha(FADE_SECONDS * (1.0 + OPAQUE_FRACTION) / 2.0);
        assert!((0.0..1.0).contains(&midway), "fading, got {midway}");
        assert_eq!(fade_alpha(FADE_SECONDS), 0.0);
        assert_eq!(fade_alpha(FADE_SECONDS * 10.0), 0.0, "and stays gone");
    }
}

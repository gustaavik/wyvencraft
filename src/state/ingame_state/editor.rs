//! Wiring the item placement editor into the frame.
//!
//! The session itself is pure state and the panel is pure drawing; what is left
//! is the three things only the in-game state can do — hand the session a clock
//! and a filesystem, point the camera at the context being edited, and put a
//! still copy of the item on the ground to look at.

use glam::Vec3;

use super::InGameState;
use crate::editor::{EditorAction, FsStamps};
use crate::entity::{DroppedItem, Perspective};
use crate::net::ChatKind;
use wyven_model::display::DisplayContext;

/// How far in front of the eye the ground preview is put, in blocks. Close
/// enough to see the placement, far enough not to sit inside the near plane.
const PREVIEW_DISTANCE: f32 = 1.6;
/// How far below eye level, so it reads as lying on the ground in front of you
/// rather than floating at your face.
const PREVIEW_DROP: f32 = 0.9;

impl InGameState {
    /// F6. Does nothing at all unless `WYVEN_EDITOR=1` asked for the editor.
    pub(super) fn toggle_editor(&mut self) {
        self.editor.toggle(&FsStamps);
    }

    /// Whether the editor is holding the mouse. The player is not driving while
    /// it is: the panel needs a cursor, and mouse-look would fight every drag.
    pub(super) fn editor_open(&self) -> bool {
        self.editor.is_open()
    }

    /// Poll the watched files and follow the hand.
    pub(super) fn update_editor(&mut self, dt: f32) {
        if !self.editor.is_open() {
            return;
        }
        self.editor.tick(dt, &FsStamps);
        let held = self.inventory.selected_stack().map(|stack| stack.item);
        self.editor.follow(held, &FsStamps);
    }

    pub(super) fn draw_editor_panel(&mut self, egui_ctx: &egui::Context) {
        if !self.editor.is_open() {
            return;
        }
        if let Some(action) = crate::ui::editor::draw_editor(egui_ctx, &self.editor) {
            self.apply_editor_action(action);
        }
    }

    /// A still copy of the selected item, lying in front of the player.
    ///
    /// Only while the `ground` tab is up: that placement is the one context you
    /// cannot otherwise see without tossing the item and chasing it. It goes
    /// down the *real* drops path in `refresh_view`, so what it shows is what a
    /// dropped item will look like, not an approximation of one.
    pub(super) fn editor_ground_preview(&self) -> Option<DroppedItem> {
        if !self.editor.is_open() || self.editor.context() != DisplayContext::Ground {
            return None;
        }
        let target = self.editor.current()?;
        let eye = self.player.eye_position();
        let look = self.player.look_direction();
        let at = eye + Vec3::new(look.x, 0.0, look.z).normalize_or_zero() * PREVIEW_DISTANCE
            - Vec3::Y * PREVIEW_DROP;
        Some(DroppedItem::preview(
            self.content.items.full_stack(target.item),
            at,
            self.content.entities.dropped_item(),
        ))
    }

    /// Point the camera at whatever is being edited.
    ///
    /// Switching to the third-person tab and still being inside your own head
    /// would leave the developer dragging numbers with nothing to watch, and F5
    /// is one more thing to remember. The first-person tab switches back for the
    /// same reason.
    fn apply_editor_action(&mut self, action: EditorAction) {
        if let EditorAction::Context(context) = action {
            match context {
                DisplayContext::FirstPersonRightHand => {
                    self.player.perspective = Perspective::First;
                }
                DisplayContext::ThirdPersonRightHand => {
                    self.player.perspective = Perspective::ThirdBack;
                }
                // The icon sheet is rendered once, inside `Game::start`, which
                // is the only moment a `&mut Gui` exists. A `gui` edit is saved
                // for real, but there is nothing on screen it can change until
                // the next launch — better said plainly, once, than left to be
                // discovered as a number that appears to do nothing.
                DisplayContext::Gui => self.chat.log.push(
                    ChatKind::System,
                    "editor: the inventory icon sheet is built at startup, so a saved                      GUI placement shows on the next launch",
                ),
                _ => {}
            }
        }
        self.editor.apply(action, &FsStamps);
    }
}

//! The screen stack: one active screen at a time, with overlays.
//!
//! Classic state pattern, and the reason the runner in [`crate::run`] never
//! learns what a menu or a playing session is. It drives whatever is on top and
//! applies whatever transition comes back.
//!
//! Every screen is parameterised by the [`Game`](crate::Game) it belongs to, so
//! two games' screens cannot be pushed onto the same stack, and each game gets
//! its own `Shared` payload without the engine knowing a field of it.

use wyven_input::InputState;
use wyven_render::{PreviewFrame, SceneFrame};

use crate::Game;

/// What the runner gives a screen each frame.
///
/// Four scalars and the game's own bundle — deliberately small. Everything a
/// screen actually works with (content, GPU handles, settings, whoever is signed
/// in) lives in [`Game::Shared`], so *this* interface stays the same however
/// elaborate a game gets, and a menu is never handed a Vulkan device just
/// because the in-game screen needs one.
pub struct Frame<'a, G: Game> {
    pub input: &'a InputState,
    /// Variable frame delta, in seconds.
    pub dt: f32,
    /// Total elapsed time, in seconds.
    pub elapsed: f32,
    /// Current framebuffer aspect ratio.
    pub aspect: f32,
    /// Set by a screen to ask that the OS cursor be locked and hidden
    /// (gameplay) or freed (menus).
    pub grab_cursor: bool,
    /// Whatever this game hands its screens.
    pub shared: &'a mut G::Shared,
}

/// What a screen asks the stack to do after an update or UI pass.
pub enum Transition<G: Game> {
    /// Stay where we are.
    None,
    /// Push on top. The screen beneath keeps its state, and keeps rendering if
    /// the pushed one is an overlay.
    Push(Box<dyn Screen<G>>),
    /// Replace the current screen.
    Replace(Box<dyn Screen<G>>),
    /// Pop back to the screen beneath.
    Pop,
    /// Clear the stack and start again with this one (quit to menu).
    ReplaceAll(Box<dyn Screen<G>>),
    /// Exit the application, running every exit hook on the way out.
    Quit,
}

/// One screen or mode of a game.
pub trait Screen<G: Game> {
    /// Per-frame logic update.
    fn update(&mut self, frame: &mut Frame<G>) -> Transition<G>;

    /// Per-frame egui UI. Default: none.
    fn ui(&mut self, _egui: &egui::Context, _frame: &mut Frame<G>) -> Transition<G> {
        Transition::None
    }

    /// Called when this screen becomes (or returns to being) the active top.
    fn on_enter(&mut self, _frame: &mut Frame<G>) {}
    /// Called when it is removed or covered.
    fn on_exit(&mut self, _frame: &mut Frame<G>) {}

    /// If true, the screen beneath keeps rendering under this one (pause menu).
    fn is_overlay(&self) -> bool {
        false
    }

    /// The 3D scene to render this frame, if any. Menus return `None`.
    fn scene_frame(&self, _aspect: f32) -> Option<SceneFrame<'_>> {
        None
    }

    /// A model to render into the game's offscreen preview image this frame, if
    /// any. Returning `None` skips the extra pass entirely.
    fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        None
    }

    /// Human-readable name, for logs.
    fn name(&self) -> &'static str;
}

/// LIFO stack of active screens.
pub struct ScreenStack<G: Game> {
    screens: Vec<Box<dyn Screen<G>>>,
}

impl<G: Game> ScreenStack<G> {
    pub fn new(initial: Box<dyn Screen<G>>) -> Self {
        Self {
            screens: vec![initial],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.screens.is_empty()
    }

    pub fn top_name(&self) -> &'static str {
        self.screens.last().map(|s| s.name()).unwrap_or("<empty>")
    }

    /// Scene to render this frame. Searched from the top down, so an overlay
    /// still shows the world rendered by the screen beneath it.
    pub fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        self.screens
            .iter()
            .rev()
            .find_map(|s| s.scene_frame(aspect))
    }

    /// The preview to render this frame, searched top-down like
    /// [`ScreenStack::scene_frame`].
    pub fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        self.screens.iter().rev().find_map(|s| s.preview_frame())
    }

    /// Update the top screen and apply its transition. `false` means the app
    /// should quit — the stack emptied, or `Quit` was requested.
    pub fn update(&mut self, frame: &mut Frame<G>) -> bool {
        let transition = match self.screens.last_mut() {
            Some(screen) => screen.update(frame),
            None => return false,
        };
        self.apply(transition, frame)
    }

    /// Run egui for the top screen, applying any resulting transition.
    pub fn ui(&mut self, egui: &egui::Context, frame: &mut Frame<G>) -> bool {
        let transition = match self.screens.last_mut() {
            Some(screen) => screen.ui(egui, frame),
            None => return false,
        };
        self.apply(transition, frame)
    }

    fn apply(&mut self, transition: Transition<G>, frame: &mut Frame<G>) -> bool {
        match transition {
            Transition::None => {}
            Transition::Push(mut screen) => {
                if let Some(top) = self.screens.last_mut() {
                    top.on_exit(frame);
                }
                screen.on_enter(frame);
                self.screens.push(screen);
            }
            Transition::Replace(mut screen) => {
                if let Some(mut top) = self.screens.pop() {
                    top.on_exit(frame);
                }
                screen.on_enter(frame);
                self.screens.push(screen);
            }
            Transition::Pop => {
                if let Some(mut top) = self.screens.pop() {
                    top.on_exit(frame);
                }
                if let Some(top) = self.screens.last_mut() {
                    top.on_enter(frame);
                }
            }
            Transition::ReplaceAll(mut screen) => {
                while let Some(mut top) = self.screens.pop() {
                    top.on_exit(frame);
                }
                screen.on_enter(frame);
                self.screens.push(screen);
            }
            Transition::Quit => {
                // Pop with exit hooks rather than clearing, so a screen holding
                // unsaved work gets its chance before the app goes away.
                self.shutdown(frame);
                return false;
            }
        }
        !self.screens.is_empty()
    }

    /// Tear every screen down, running its exit hook. Used for `Quit` and by the
    /// runner when the window is closed directly, which bypasses transitions.
    pub fn shutdown(&mut self, frame: &mut Frame<G>) {
        while let Some(mut top) = self.screens.pop() {
            top.on_exit(frame);
        }
    }
}

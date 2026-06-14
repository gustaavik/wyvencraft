//! Game state machine (State pattern).
//!
//! The app drives a [`StateStack`] of [`GameState`] trait objects: Main Menu →
//! Multiplayer Menu → Loading → In-Game, with the Pause menu pushed as an
//! overlay. Each state handles its own update + egui UI and requests transitions;
//! the app stays agnostic of which screen is active.

pub mod connecting_state;
pub mod ingame_state;
pub mod loading_state;
pub mod menu_state;
pub mod multiplayer_menu;
pub mod pause_menu;

pub use connecting_state::ConnectingState;
pub use ingame_state::InGameState;
pub use loading_state::LoadingState;
pub use menu_state::MainMenuState;
pub use multiplayer_menu::MultiplayerMenuState;
pub use pause_menu::PauseMenuState;

use std::sync::Arc;

use crate::config::Settings;
use crate::input::InputState;
use crate::render::{RenderContext, SceneFrame};

/// Shared, per-frame context handed to states.
pub struct StateContext<'a> {
    pub settings: &'a mut Settings,
    pub input: &'a InputState,
    /// GPU device + allocators, for states that upload meshes/textures.
    pub render: &'a Arc<RenderContext>,
    /// Variable frame delta (seconds).
    pub dt: f32,
    /// Total elapsed time (seconds).
    pub elapsed: f32,
    /// Set by a state to request the OS cursor be locked/hidden (gameplay) or
    /// freed (menus).
    pub grab_cursor: bool,
}

/// What a state asks the stack to do after an update/UI pass.
pub enum Transition {
    /// Stay on the current state.
    None,
    /// Push a new state on top (current keeps its data; may keep rendering if the
    /// pushed state is an overlay).
    Push(Box<dyn GameState>),
    /// Replace the current state.
    Replace(Box<dyn GameState>),
    /// Pop the current state, returning to the one beneath.
    Pop,
    /// Clear the entire stack and start fresh with this state (e.g. quit to menu).
    ReplaceAll(Box<dyn GameState>),
    /// Exit the application.
    Quit,
}

/// A single screen/mode of the game.
pub trait GameState {
    /// Per-frame logic update.
    fn update(&mut self, ctx: &mut StateContext) -> Transition;

    /// Per-frame egui UI. Default: no UI. Menus/HUD override this.
    fn ui(&mut self, _egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        Transition::None
    }

    /// Called when the state becomes (or returns to being) the active top.
    fn on_enter(&mut self, _ctx: &mut StateContext) {}
    /// Called when the state is removed or covered.
    fn on_exit(&mut self, _ctx: &mut StateContext) {}

    /// If true, the state beneath continues to render under this one (pause menu).
    fn is_overlay(&self) -> bool {
        false
    }

    /// Provide the 3D scene to render this frame, if any (menus return `None`).
    /// `aspect` is the current framebuffer aspect ratio.
    fn scene_frame(&self, _aspect: f32) -> Option<SceneFrame<'_>> {
        None
    }

    /// Human-readable name for debugging.
    fn name(&self) -> &'static str;
}

/// LIFO stack of active states.
pub struct StateStack {
    states: Vec<Box<dyn GameState>>,
}

impl StateStack {
    pub fn new(initial: Box<dyn GameState>) -> Self {
        Self {
            states: vec![initial],
        }
    }

    pub fn is_empty(&self) -> bool {
        self.states.is_empty()
    }

    pub fn top_name(&self) -> &'static str {
        self.states.last().map(|s| s.name()).unwrap_or("<empty>")
    }

    /// Scene to render this frame. Searches from the top down so an overlay
    /// (e.g. the pause menu) still shows the world rendered by the state beneath.
    pub fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        self.states.iter().rev().find_map(|s| s.scene_frame(aspect))
    }

    /// Update the top state and apply its transition. Returns `false` if the app
    /// should quit (stack emptied or `Quit` requested).
    pub fn update(&mut self, ctx: &mut StateContext) -> bool {
        let transition = match self.states.last_mut() {
            Some(state) => state.update(ctx),
            None => return false,
        };
        self.apply(transition, ctx)
    }

    /// Run egui for the top state, applying any resulting transition.
    pub fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> bool {
        let transition = match self.states.last_mut() {
            Some(state) => state.ui(egui_ctx, ctx),
            None => return false,
        };
        self.apply(transition, ctx)
    }

    fn apply(&mut self, transition: Transition, ctx: &mut StateContext) -> bool {
        match transition {
            Transition::None => {}
            Transition::Push(mut state) => {
                if let Some(top) = self.states.last_mut() {
                    top.on_exit(ctx);
                }
                state.on_enter(ctx);
                self.states.push(state);
            }
            Transition::Replace(mut state) => {
                if let Some(mut top) = self.states.pop() {
                    top.on_exit(ctx);
                }
                state.on_enter(ctx);
                self.states.push(state);
            }
            Transition::Pop => {
                if let Some(mut top) = self.states.pop() {
                    top.on_exit(ctx);
                }
                if let Some(top) = self.states.last_mut() {
                    top.on_enter(ctx);
                }
            }
            Transition::ReplaceAll(mut state) => {
                while let Some(mut top) = self.states.pop() {
                    top.on_exit(ctx);
                }
                state.on_enter(ctx);
                self.states.push(state);
            }
            Transition::Quit => {
                self.states.clear();
                return false;
            }
        }
        !self.states.is_empty()
    }
}

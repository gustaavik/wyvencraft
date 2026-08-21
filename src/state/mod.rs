//! Game state machine (State pattern).
//!
//! The app drives a [`StateStack`] of [`GameState`] trait objects: Main Menu →
//! Multiplayer Menu → Loading → In-Game, with the Pause menu pushed as an
//! overlay. Each state handles its own update + egui UI and requests transitions;
//! the app stays agnostic of which screen is active.

pub mod connecting_state;
pub mod ingame_state;
pub mod loading_state;
pub mod login_state;
pub mod menu_state;
pub mod multiplayer_menu;
pub mod pause_menu;
pub mod session;
pub mod singleplayer_menu;

pub use connecting_state::ConnectingState;
pub use ingame_state::InGameState;
pub use loading_state::LoadingState;
pub use login_state::LoginState;
pub use menu_state::MainMenuState;
pub use multiplayer_menu::MultiplayerMenuState;
pub use pause_menu::PauseMenuState;
pub use singleplayer_menu::SingleplayerMenuState;

use std::sync::Arc;

use crate::config::Settings;
use crate::content::GameContent;
use crate::input::InputState;
use wyven_render::{PreviewFrame, RenderContext, SceneFrame};

/// egui texture handles the app registers once and hands to the UI each frame:
/// the block atlas (for tile-based item icons), the sheet of pre-rendered 3D
/// icons (for items with a model), and the offscreen player-model preview.
#[derive(Clone, Copy)]
pub struct UiTextures {
    pub atlas: egui::TextureId,
    /// One cell per loaded model, indexed by `ModelId` — see
    /// [`wyven_render::icons`]. `model_count` is how many cells it holds,
    /// which the UI needs to turn a cell index into UVs.
    pub model_icons: egui::TextureId,
    pub model_count: u32,
    pub preview: egui::TextureId,
}

/// The app-owned resources a state may draw from: the GPU, the loaded content,
/// and the egui texture handles.
///
/// Grouped because they travel together and almost nothing needs them — the
/// menus reach for `content` at most, and since the in-game state's GPU work
/// moved behind a single seam, `render` has exactly one consumer. Keeping them
/// off the flat context makes it obvious that a state touching `resources` is
/// doing something with assets, not just reading a frame delta.
#[derive(Clone, Copy)]
pub struct Resources<'a> {
    /// GPU device + allocators, for states that upload meshes/textures.
    pub render: &'a Arc<RenderContext>,
    /// Loaded game content (block/item registries), shared by every session.
    pub content: &'a Arc<GameContent>,
    /// egui handles for the block atlas and the player-model preview image.
    pub ui_tex: UiTextures,
    /// Who this client is signed in as. Read by the menus (to decide whether
    /// multiplayer is available) and by `ConnectingState` (to fetch a join
    /// ticket). Passed rather than global, like everything else here.
    pub account: &'a wyven_auth::AccountState,
}

/// Shared, per-frame context handed to states.
pub struct StateContext<'a> {
    pub settings: &'a mut Settings,
    pub input: &'a InputState,
    /// Variable frame delta (seconds).
    pub dt: f32,
    /// Total elapsed time (seconds).
    pub elapsed: f32,
    /// Set by a state to request the OS cursor be locked/hidden (gameplay) or
    /// freed (menus).
    pub grab_cursor: bool,
    /// App-owned assets and GPU handles.
    pub resources: Resources<'a>,
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

    /// Provide the player-model preview to render into the inventory's offscreen
    /// image this frame, if any. Only the in-game state, with its inventory open,
    /// returns `Some`; every other frame skips the extra pass entirely.
    fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
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

    /// The player-model preview to render this frame, searched top-down like
    /// [`StateStack::scene_frame`].
    pub fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        self.states.iter().rev().find_map(|s| s.preview_frame())
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
                // Pop with exit hooks (not a bare clear) so e.g. the in-game
                // state saves the world before the app quits.
                self.shutdown(ctx);
                return false;
            }
        }
        !self.states.is_empty()
    }

    /// Tear down every state, running its exit hook. Called for `Quit` and by
    /// the app when the window is closed directly (which bypasses transitions).
    pub fn shutdown(&mut self, ctx: &mut StateContext) {
        while let Some(mut top) = self.states.pop() {
            top.on_exit(ctx);
        }
    }
}

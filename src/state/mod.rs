//! Wyvencraft's screens: login → menus → loading → in-game, with the pause
//! menu pushed as an overlay.
//!
//! The stack itself is [`wyven_app`]; what is on it is here. Each screen
//! implements [`wyven_app::Screen<Wyvencraft>`], which the aliases below make
//! readable — [`Transition`] and [`Frame`] are the engine's types with this
//! game's payload already filled in, so no screen has to spell the parameter.

pub mod connecting_state;
pub mod ingame_state;
pub mod loading_state;
pub mod login_state;
pub mod menu_state;
pub mod multiplayer_menu;
pub mod pause_menu;
pub mod session;
pub mod shared;
pub mod singleplayer_menu;

pub use connecting_state::ConnectingState;
pub use ingame_state::InGameState;
pub use loading_state::LoadingState;
pub use login_state::LoginState;
pub use menu_state::MainMenuState;
pub use multiplayer_menu::MultiplayerMenuState;
pub use pause_menu::PauseMenuState;
pub use shared::{Shared, UiTextures, Wyvencraft};
pub use singleplayer_menu::SingleplayerMenuState;

/// A screen of this game. The trait is `wyven_app`'s; this alias just fills in
/// the parameter, so nothing below has to spell `Screen<Wyvencraft>`.
pub use wyven_app::Screen as GameState;
/// What a screen asks the stack to do next.
pub type Transition = wyven_app::Transition<Wyvencraft>;
/// The per-frame context a screen is handed.
pub type StateContext<'a> = wyven_app::Frame<'a, Wyvencraft>;

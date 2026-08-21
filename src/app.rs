//! Application entry point.
//!
//! Everything that used to be here — the window, the Vulkan device, the winit
//! event loop, the frame pump, the screen stack — is [`wyven_app`], which knows
//! nothing about Wyvencraft. What is left is naming this game and handing it
//! over.
//!
//! The startup work that *is* Wyvencraft's (loading content, baking the item
//! icon sheet, creating the inventory's preview target, deciding which screen to
//! open on) lives in `state::shared` and `boot::start`, reached through the
//! [`wyven_app::Game`] impl.

pub use wyven_app::AppError;

use crate::state::Wyvencraft;

/// Entry point invoked from `main`.
pub fn run() -> Result<(), AppError> {
    wyven_app::run(Wyvencraft::new())
}

//! Application entry point.
//!
//! Everything that used to be here — the window, the Vulkan device, the winit
//! event loop, the frame pump, the screen stack — is [`wyven_app`], which knows
//! nothing about Wyvencraft. What is left is naming this game and handing it
//! over.
//!
//! The startup work that *is* Wyvencraft's (loading content, baking the item
//! icon sheet, creating the inventory's preview target, deciding which screen to
//! open on) lives in `presentation::screens::shared` and `boot::start`, reached
//! through the [`wyven_app::Game`] impl. This file wires the two together: the
//! boot plan is read here and handed to the game as its first-screen factory.

pub use wyven_app::AppError;

use crate::boot::{self, BootPlan, SystemEnv};
use crate::presentation::screens::Wyvencraft;

/// Entry point invoked from `main`.
pub fn run() -> Result<(), AppError> {
    // Dev convenience env vars skip the menus — see `application::boot_plan`
    // for the rules: WYVEN_BOOT_INGAME / WYVEN_HOST / WYVEN_JOIN / WYVEN_MODE /
    // WYVEN_WORLD / WYVEN_SEED.
    let plan = BootPlan::from_env(&SystemEnv);
    wyven_app::run(Wyvencraft::new(Box::new(move |content, account| {
        boot::initial_screen(plan, content, account)
    })))
}

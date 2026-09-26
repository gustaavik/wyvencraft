//! Startup: what the environment asks for, and how that becomes a screen.
//!
//! Two halves, deliberately separated. The plan
//! ([`crate::application::boot_plan`]) is pure — it reads `WYVEN_*` and decides,
//! with no window, GPU or socket anywhere in reach, which is what makes every
//! boot path testable. [`start`] performs the decision: opens saves, binds
//! sockets, signs in.
//!
//! This module is the **composition root**: the one place that names concrete
//! adapters and wires them into the first screen. Nothing imports it back —
//! `app` hands [`start::initial_screen`] to the game as a factory.

pub mod start;

pub use crate::application::boot_plan::{
    BootPlan, Environment, MapEnv, SystemEnv, WorldChoice, screenshot_at,
};
pub use start::initial_screen;

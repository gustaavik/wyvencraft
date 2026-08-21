//! Startup: what the environment asks for, and how that becomes a screen.
//!
//! Two halves, deliberately separated. [`plan`] is pure — it reads `WYVEN_*` and
//! decides, with no window, GPU or socket anywhere in reach, which is what makes
//! every boot path testable. [`start`] performs the decision: opens saves, binds
//! sockets, signs in.

pub mod plan;
pub mod start;

pub use plan::{BootPlan, Environment, MapEnv, SystemEnv, WorldChoice};
pub use start::initial_screen;

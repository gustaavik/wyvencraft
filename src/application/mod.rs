//! Use cases and the ports they need: what a session *does* with the domain,
//! independent of how bytes reach a socket or pixels reach the screen.
//!
//! May depend on [`crate::domain`] and engine primitives; never on
//! `infrastructure` or `presentation` (`tests/architecture.rs`).

pub mod boot_plan;
pub mod protocol;
pub mod session;
pub mod sync;

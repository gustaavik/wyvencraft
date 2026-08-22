//! Foundational vocabulary, plus the two pieces of it that are Wyvencraft rules
//! rather than engine primitives.
//!
//! The coordinate types, math, RNG and clock live in [`wyven_core`] and are
//! re-exported here, so `crate::core::BlockPos` still resolves and the engine
//! crate stays reachable under one name. What does *not* live there:
//!
//! - [`GameMode`] — survival vs. creative is a rulebook (`consumes_blocks`,
//!   `instant_break`, `can_fly`), not a primitive.
//! - [`DayCycle`] — a 20-minute day and an `is_night` that gates mob spawning
//!   are likewise Wyvencraft's numbers, not the engine's.
//! - [`ident`] — how content is named: what makes a legal block/item id, and
//!   the display name derived from one.

pub mod day_cycle;
pub mod gamemode;
pub mod ident;

pub use day_cycle::{Atmosphere, DayCycle};
pub use gamemode::GameMode;

pub use wyven_core::*;

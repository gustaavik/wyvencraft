//! World progression: how far a world's players have come through the
//! Valheim-style loop — which shrines they have read, which boss altars those
//! revealed, which bosses have fallen.
//!
//! Pure data and rules, owned by the authority (singleplayer or host), saved
//! with the world in `progression.dat`, and mirrored to clients whole via
//! `ServerMessage::Progression` whenever it changes. Deliberately *not* part of
//! the content hash: it is world state, like block edits, not a definition.
//!
//! - [`state`] — [`WorldProgression`] and its idempotent updates.
//! - [`waypoint`] — the bearing from a player to a revealed altar, for the
//!   compass.

pub mod state;
pub mod waypoint;

pub use state::WorldProgression;
pub use waypoint::{Bearing, bearing};

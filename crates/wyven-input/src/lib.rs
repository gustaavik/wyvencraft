//! Frame-coherent input state derived from winit events.
//!
//! Accumulates key and mouse state during a frame; callers query it and
//! [`InputState::end_frame`] clears the per-frame deltas and edge flags.
//!
//! Deliberately free of any notion of what a key *does*. There is no `forward`
//! and no `jump` here — a binding table and the intent it produces are the
//! game's, and keeping them out is what makes this reusable by a game with
//! entirely different verbs.

pub mod controls;

pub use controls::InputState;

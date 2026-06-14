//! Input handling: turns raw winit events into frame-coherent, queryable state
//! and high-level movement intent.

pub mod controls;

pub use controls::InputState;

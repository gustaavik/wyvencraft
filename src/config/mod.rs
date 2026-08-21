//! User-facing settings, and the one place raw key state becomes Wyvencraft's
//! verbs.
//!
//! [`wyven_input::InputState`] deliberately knows nothing about `forward` or
//! `jump` — a binding table and the intent it produces belong to whoever
//! defines the verbs. [`movement`] is that translation, and it is a free
//! function over the input state rather than a method on it precisely so the
//! engine crate need never hear about either.

pub mod settings;

pub use settings::{ControlSettings, Keybinds, RenderSettings, Settings, WindowSettings};

use winit::keyboard::KeyCode;
use wyven_input::InputState;

use crate::entity::MovementInput;

/// Build the movement intent for the player from current key state.
pub fn movement(input: &InputState, binds: &Keybinds) -> MovementInput {
    let axis =
        |pos: KeyCode, neg: KeyCode| (input.is_held(pos) as i32 - input.is_held(neg) as i32) as f32;
    MovementInput {
        forward: axis(binds.forward, binds.back),
        strafe: axis(binds.right, binds.left),
        jump: input.is_held(binds.jump),
        sneak: input.is_held(binds.sneak),
        sprint: input.is_held(binds.sprint),
    }
}

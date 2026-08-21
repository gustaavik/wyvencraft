//! Frame-coherent input state derived from winit events.
//!
//! Accumulates key/mouse state during a frame; gameplay code queries it and then
//! [`InputState::end_frame`] clears the per-frame deltas and edge flags.

use std::collections::HashSet;

use glam::Vec2;
use winit::event::{ElementState, MouseButton};
use winit::keyboard::KeyCode;

#[derive(Default)]
pub struct InputState {
    held: HashSet<KeyCode>,
    pressed_this_frame: HashSet<KeyCode>,
    mouse_held: HashSet<MouseButton>,
    mouse_pressed_this_frame: HashSet<MouseButton>,
    mouse_delta: Vec2,
    scroll_delta: f32,
    /// When false, gameplay ignores mouse-look (e.g. a menu is open).
    pub cursor_grabbed: bool,
}

impl InputState {
    pub fn new() -> Self {
        Self::default()
    }

    // --- Event intake (called from the winit event handler) ---

    pub fn on_key(&mut self, key: KeyCode, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.held.insert(key) {
                    self.pressed_this_frame.insert(key);
                }
            }
            ElementState::Released => {
                self.held.remove(&key);
            }
        }
    }

    pub fn on_mouse_button(&mut self, button: MouseButton, state: ElementState) {
        match state {
            ElementState::Pressed => {
                if self.mouse_held.insert(button) {
                    self.mouse_pressed_this_frame.insert(button);
                }
            }
            ElementState::Released => {
                self.mouse_held.remove(&button);
            }
        }
    }

    pub fn on_mouse_motion(&mut self, dx: f64, dy: f64) {
        self.mouse_delta += Vec2::new(dx as f32, dy as f32);
    }

    pub fn on_scroll(&mut self, delta: f32) {
        self.scroll_delta += delta;
    }

    /// Drop all held state (used when focus is lost so keys don't "stick").
    pub fn clear_all(&mut self) {
        self.held.clear();
        self.mouse_held.clear();
        self.end_frame();
    }

    // --- Queries ---

    pub fn is_held(&self, key: KeyCode) -> bool {
        self.held.contains(&key)
    }

    pub fn just_pressed(&self, key: KeyCode) -> bool {
        self.pressed_this_frame.contains(&key)
    }

    pub fn mouse_held(&self, button: MouseButton) -> bool {
        self.mouse_held.contains(&button)
    }

    pub fn mouse_just_pressed(&self, button: MouseButton) -> bool {
        self.mouse_pressed_this_frame.contains(&button)
    }

    pub fn mouse_delta(&self) -> Vec2 {
        self.mouse_delta
    }

    pub fn scroll_delta(&self) -> f32 {
        self.scroll_delta
    }

    /// Clear per-frame deltas and edge-triggered sets. Call at end of each frame.
    pub fn end_frame(&mut self) {
        self.pressed_this_frame.clear();
        self.mouse_pressed_this_frame.clear();
        self.mouse_delta = Vec2::ZERO;
        self.scroll_delta = 0.0;
    }
}

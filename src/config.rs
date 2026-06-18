//! User-facing settings: rendering, controls, and window options.
//!
//! Kept as plain data with sane [`Default`]s. A future milestone can
//! (de)serialize this to `config.toml`; the `serde` derives are already here.

use winit::keyboard::KeyCode;

/// All tunable game settings.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct Settings {
    pub window: WindowSettings,
    pub render: RenderSettings,
    pub controls: ControlSettings,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct WindowSettings {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub vsync: bool,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1280,
            height: 720,
            title: "Wyvencraft".to_string(),
            vsync: true,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct RenderSettings {
    /// Field of view in degrees (vertical).
    pub fov_degrees: f32,
    /// How many chunks out from the player to load/draw.
    pub render_distance: i32,
    pub max_fps: Option<u32>,
}

impl Default for RenderSettings {
    fn default() -> Self {
        Self {
            fov_degrees: 70.0,
            render_distance: 8,
            max_fps: None,
        }
    }
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct ControlSettings {
    pub mouse_sensitivity: f32,
    pub invert_y: bool,
    pub keybinds: Keybinds,
}

impl Default for ControlSettings {
    fn default() -> Self {
        Self {
            mouse_sensitivity: 0.15,
            invert_y: false,
            keybinds: Keybinds::default(),
        }
    }
}

/// Remappable key assignments. Serialized via `KeyCode`'s name.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct Keybinds {
    pub forward: KeyCode,
    pub back: KeyCode,
    pub left: KeyCode,
    pub right: KeyCode,
    pub jump: KeyCode,
    pub sneak: KeyCode,
    pub sprint: KeyCode,
    pub inventory: KeyCode,
    pub toggle_perspective: KeyCode,
    pub toggle_debug: KeyCode,
    pub toggle_gamemode: KeyCode,
    pub pause: KeyCode,
}

impl Default for Keybinds {
    fn default() -> Self {
        Self {
            forward: KeyCode::KeyW,
            back: KeyCode::KeyS,
            left: KeyCode::KeyA,
            right: KeyCode::KeyD,
            jump: KeyCode::Space,
            sneak: KeyCode::ShiftLeft,
            sprint: KeyCode::ControlLeft,
            inventory: KeyCode::KeyE,
            toggle_perspective: KeyCode::F5,
            toggle_debug: KeyCode::F3,
            toggle_gamemode: KeyCode::F4,
            pause: KeyCode::Escape,
        }
    }
}

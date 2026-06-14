//! egui-based user interface: menus, HUD, and the inventory screen.
//!
//! The egui ↔ winit ↔ vulkano integration (a `Gui` from `egui_winit_vulkano`) is
//! created by the app in milestone M5. Individual screens are drawn either here
//! (HUD) or directly in their owning [`crate::state`] (menus), and migrate into
//! dedicated view modules (`main_menu`, `multiplayer_menu`, `inventory`,
//! `pause_menu`) as those milestones land.

pub mod hud;

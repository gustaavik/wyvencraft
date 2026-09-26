//! Pause overlay shown over the (still-rendered, frozen) world.

use winit::keyboard::KeyCode;

use super::{GameState, MainMenuState, StateContext, Transition, Wyvencraft};

#[derive(Default)]
pub struct PauseMenuState;

impl PauseMenuState {
    pub fn new() -> Self {
        Self
    }
}

impl GameState<Wyvencraft> for PauseMenuState {
    fn name(&self) -> &'static str {
        "Pause"
    }

    fn is_overlay(&self) -> bool {
        true
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        // Free the cursor while paused.
        ctx.grab_cursor = false;
        // Esc resumes.
        if ctx.input.just_pressed(KeyCode::Escape) {
            return Transition::Pop;
        }
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;

        // Dim the world behind the menu.
        egui::Area::new(egui::Id::new("pause_dim"))
            .fixed_pos(egui::pos2(0.0, 0.0))
            .show(egui_ctx, |ui| {
                let screen = ui.ctx().screen_rect();
                ui.painter()
                    .rect_filled(screen, 0.0, egui::Color32::from_black_alpha(140));
            });

        egui::Window::new("Paused")
            .collapsible(false)
            .resizable(false)
            .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
            .show(egui_ctx, |ui| {
                ui.vertical_centered(|ui| {
                    let button = |ui: &mut egui::Ui, label: &str| {
                        ui.add_sized([200.0, 32.0], egui::Button::new(label))
                            .clicked()
                    };
                    if button(ui, "Resume") {
                        transition = Transition::Pop;
                    }
                    ui.add_space(6.0);
                    if button(ui, "Main Menu") {
                        transition = Transition::ReplaceAll(Box::new(MainMenuState::new()));
                    }
                    ui.add_space(6.0);
                    if button(ui, "Quit") {
                        transition = Transition::Quit;
                    }
                });
            });

        transition
    }
}

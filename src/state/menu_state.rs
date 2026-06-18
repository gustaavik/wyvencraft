//! Main menu: choose singleplayer / multiplayer / quit.

use super::{GameState, MultiplayerMenuState, SingleplayerMenuState, StateContext, Transition};

#[derive(Default)]
pub struct MainMenuState;

impl MainMenuState {
    pub fn new() -> Self {
        Self
    }
}

impl GameState for MainMenuState {
    fn name(&self) -> &'static str {
        "MainMenu"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        // Menus run with a free cursor.
        ctx.grab_cursor = false;
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(80.0);
                ui.heading(egui::RichText::new("Wyvencraft").size(48.0));
                ui.add_space(48.0);

                let button = |ui: &mut egui::Ui, label: &str| {
                    ui.add_sized([220.0, 36.0], egui::Button::new(label))
                        .clicked()
                };

                if button(ui, "Singleplayer") {
                    transition = Transition::Replace(Box::new(SingleplayerMenuState::new()));
                }
                ui.add_space(8.0);
                if button(ui, "Multiplayer") {
                    transition = Transition::Replace(Box::new(MultiplayerMenuState::new()));
                }
                ui.add_space(8.0);
                if button(ui, "Quit") {
                    transition = Transition::Quit;
                }
            });
        });
        transition
    }
}

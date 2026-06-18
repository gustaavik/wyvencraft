//! Singleplayer setup: choose a game mode, then create the world.

use super::{GameState, LoadingState, MainMenuState, StateContext, Transition};
use crate::core::GameMode;

pub struct SingleplayerMenuState {
    mode: GameMode,
}

impl Default for SingleplayerMenuState {
    fn default() -> Self {
        Self {
            mode: GameMode::Survival,
        }
    }
}

impl SingleplayerMenuState {
    pub fn new() -> Self {
        Self::default()
    }
}

impl GameState for SingleplayerMenuState {
    fn name(&self) -> &'static str {
        "SingleplayerMenu"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.heading("New World");
                ui.add_space(24.0);

                ui.label("Game mode:");
                ui.add_space(6.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 240.0).max(0.0) * 0.5);
                    ui.selectable_value(&mut self.mode, GameMode::Survival, "Survival");
                    ui.selectable_value(&mut self.mode, GameMode::Creative, "Creative");
                });
                ui.add_space(6.0);
                ui.label(
                    egui::RichText::new(mode_blurb(self.mode))
                        .small()
                        .color(egui::Color32::GRAY),
                );

                ui.add_space(20.0);
                if ui
                    .add_sized([220.0, 36.0], egui::Button::new("Create World"))
                    .clicked()
                {
                    transition =
                        Transition::Replace(Box::new(LoadingState::singleplayer(self.mode)));
                }
                ui.add_space(8.0);
                if ui
                    .add_sized([220.0, 30.0], egui::Button::new("Back"))
                    .clicked()
                {
                    transition = Transition::Replace(Box::new(MainMenuState::new()));
                }
            });
        });
        transition
    }
}

fn mode_blurb(mode: GameMode) -> &'static str {
    match mode {
        GameMode::Survival => "Health, hunger, fall damage, timed mining. Mine to gather blocks.",
        GameMode::Creative => {
            "Fly (double-tap jump), invulnerable, instant break, infinite blocks."
        }
    }
}

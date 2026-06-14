//! Transient world-loading screen. In M3 it waits for the initial ring of chunks
//! to finish generating; for now it hands straight off to the in-game state.

use super::{GameState, InGameState, StateContext, Transition};

pub struct LoadingState {
    seed: u64,
}

impl LoadingState {
    pub fn with_seed(seed: u64) -> Self {
        Self { seed }
    }

    /// A fresh singleplayer world with a time-derived seed.
    pub fn singleplayer() -> Self {
        Self::with_seed(random_seed())
    }
}

impl GameState for LoadingState {
    fn name(&self) -> &'static str {
        "Loading"
    }

    fn update(&mut self, _ctx: &mut StateContext) -> Transition {
        // M3: poll the chunk loader and only transition once spawn-area chunks
        // are ready. For now, enter the world immediately.
        Transition::Replace(Box::new(InGameState::new(self.seed)))
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                ui.heading("Generating world…");
                ui.spinner();
            });
        });
        Transition::None
    }
}

fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

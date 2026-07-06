//! Transient world-loading screen. In M3 it waits for the initial ring of chunks
//! to finish generating; for now it hands straight off to the in-game state.
//!
//! All fallible disk I/O happens *before* this state (in the menus / boot code),
//! so entering the world is infallible here.

use super::{GameState, InGameState, StateContext, Transition};
use crate::core::GameMode;
use crate::save::{self, SavedGame};

enum LoadKind {
    /// Seed-only world, never written to disk (menu-less dev boots).
    Ephemeral { seed: u64, mode: GameMode },
    /// A named world from `saves/` (fresh or previously played). Boxed: a
    /// `SavedGame` dwarfs the seed variant.
    Saved(Box<SavedGame>),
}

pub struct LoadingState {
    /// Taken on the first update; `None` afterwards.
    kind: Option<LoadKind>,
}

impl LoadingState {
    pub fn with_seed(seed: u64, mode: GameMode) -> Self {
        Self {
            kind: Some(LoadKind::Ephemeral { seed, mode }),
        }
    }

    /// A fresh singleplayer world with a time-derived seed.
    pub fn singleplayer(mode: GameMode) -> Self {
        Self::with_seed(save::random_seed(), mode)
    }

    /// Enter a world loaded from (or just created on) disk.
    pub fn saved(game: SavedGame) -> Self {
        Self {
            kind: Some(LoadKind::Saved(Box::new(game))),
        }
    }
}

impl GameState for LoadingState {
    fn name(&self) -> &'static str {
        "Loading"
    }

    fn update(&mut self, _ctx: &mut StateContext) -> Transition {
        // M3: poll the chunk loader and only transition once spawn-area chunks
        // are ready. For now, enter the world immediately.
        match self.kind.take() {
            Some(LoadKind::Ephemeral { seed, mode }) => {
                Transition::Replace(Box::new(InGameState::new(seed, mode)))
            }
            Some(LoadKind::Saved(game)) => {
                Transition::Replace(Box::new(InGameState::new_saved(*game)))
            }
            None => Transition::None,
        }
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

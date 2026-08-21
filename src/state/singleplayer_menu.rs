//! Singleplayer setup: pick an existing world (play/delete) or create a new one
//! with a name, optional seed, and game mode.

use std::time::{SystemTime, UNIX_EPOCH};

use super::{GameState, LoadingState, MainMenuState, StateContext, Transition, Wyvencraft};
use crate::core::GameMode;
use crate::save::{self, WorldEntry, WorldSave};

pub struct SingleplayerMenuState {
    worlds: Vec<WorldEntry>,
    name: String,
    seed_text: String,
    mode: GameMode,
    error: Option<String>,
    /// Slug awaiting the second (confirming) Delete click.
    confirm_delete: Option<String>,
}

impl Default for SingleplayerMenuState {
    fn default() -> Self {
        Self {
            worlds: save::list_worlds(&save::saves_root()),
            name: String::new(),
            seed_text: String::new(),
            mode: GameMode::Survival,
            error: None,
            confirm_delete: None,
        }
    }
}

impl SingleplayerMenuState {
    pub fn new() -> Self {
        Self::default()
    }

    fn rescan(&mut self) {
        self.worlds = save::list_worlds(&save::saves_root());
    }

    /// Open + load a world, transitioning into it (errors go to the menu label).
    fn play(&mut self, slug: &str) -> Transition {
        match WorldSave::open(&save::saves_root(), slug).and_then(WorldSave::load) {
            Ok(game) => Transition::Replace(Box::new(LoadingState::saved(game))),
            Err(err) => {
                self.error = Some(format!("Failed to load: {err}"));
                Transition::None
            }
        }
    }

    fn create(&mut self) -> Transition {
        let name = self.name.trim().to_string();
        if name.is_empty() {
            self.error = Some("Enter a world name".to_string());
            return Transition::None;
        }
        let seed = save::parse_seed(&self.seed_text);
        match WorldSave::create(&save::saves_root(), &name, seed, self.mode)
            .and_then(WorldSave::load)
        {
            Ok(game) => Transition::Replace(Box::new(LoadingState::saved(game))),
            Err(err) => {
                self.error = Some(err.to_string());
                Transition::None
            }
        }
    }
}

impl GameState<Wyvencraft> for SingleplayerMenuState {
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
                ui.add_space(40.0);
                ui.heading("Singleplayer");
                ui.add_space(16.0);

                // --- Existing worlds ---
                if self.worlds.is_empty() {
                    ui.label(
                        egui::RichText::new("No worlds yet — create one below.")
                            .color(egui::Color32::GRAY),
                    );
                } else {
                    let mut play: Option<String> = None;
                    let mut delete: Option<String> = None;
                    egui::ScrollArea::vertical()
                        .max_height(220.0)
                        .show(ui, |ui| {
                            ui.set_width(420.0);
                            for world in &self.worlds {
                                ui.horizontal(|ui| {
                                    ui.vertical(|ui| {
                                        ui.label(egui::RichText::new(&world.meta.name).strong());
                                        ui.label(
                                            egui::RichText::new(format!(
                                                "{} · seed {} · {}",
                                                world.meta.game_mode.label(),
                                                world.meta.seed,
                                                time_ago(world.meta.last_played_unix),
                                            ))
                                            .small()
                                            .color(egui::Color32::GRAY),
                                        );
                                    });
                                    ui.with_layout(
                                        egui::Layout::right_to_left(egui::Align::Center),
                                        |ui| {
                                            let confirming = self.confirm_delete.as_deref()
                                                == Some(world.slug.as_str());
                                            let delete_label =
                                                if confirming { "Confirm?" } else { "Delete" };
                                            if ui
                                                .add_sized(
                                                    [70.0, 28.0],
                                                    egui::Button::new(delete_label),
                                                )
                                                .clicked()
                                            {
                                                if confirming {
                                                    delete = Some(world.slug.clone());
                                                } else {
                                                    self.confirm_delete = Some(world.slug.clone());
                                                }
                                            }
                                            if ui
                                                .add_sized([70.0, 28.0], egui::Button::new("Play"))
                                                .clicked()
                                            {
                                                play = Some(world.slug.clone());
                                            }
                                        },
                                    );
                                });
                                ui.separator();
                            }
                        });
                    if let Some(slug) = delete {
                        self.confirm_delete = None;
                        match save::delete_world(&save::saves_root(), &slug) {
                            Ok(()) => log::info!("deleted world '{slug}'"),
                            Err(err) => self.error = Some(format!("Delete failed: {err}")),
                        }
                        self.rescan();
                    }
                    if let Some(slug) = play {
                        self.confirm_delete = None;
                        transition = self.play(&slug);
                    }
                }

                // --- Create a new world ---
                ui.add_space(18.0);
                ui.label(egui::RichText::new("Create New World").strong());
                ui.add_space(8.0);
                ui.add_sized(
                    [260.0, 24.0],
                    egui::TextEdit::singleline(&mut self.name).hint_text("World name"),
                );
                ui.add_space(4.0);
                ui.add_sized(
                    [260.0, 24.0],
                    egui::TextEdit::singleline(&mut self.seed_text)
                        .hint_text("Seed (blank = random)"),
                );
                ui.add_space(8.0);
                ui.horizontal(|ui| {
                    ui.add_space((ui.available_width() - 240.0).max(0.0) * 0.5);
                    ui.selectable_value(&mut self.mode, GameMode::Survival, "Survival");
                    ui.selectable_value(&mut self.mode, GameMode::Creative, "Creative");
                });
                ui.add_space(4.0);
                ui.label(
                    egui::RichText::new(mode_blurb(self.mode))
                        .small()
                        .color(egui::Color32::GRAY),
                );

                ui.add_space(12.0);
                if ui
                    .add_sized([220.0, 36.0], egui::Button::new("Create World"))
                    .clicked()
                {
                    transition = self.create();
                }
                ui.add_space(8.0);
                if ui
                    .add_sized([220.0, 30.0], egui::Button::new("Back"))
                    .clicked()
                {
                    transition = Transition::Replace(Box::new(MainMenuState::new()));
                }

                if let Some(err) = &self.error {
                    ui.add_space(10.0);
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
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

/// Compact "played X ago" formatting without a date/time dependency.
fn time_ago(unix: u64) -> String {
    let now = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let secs = now.saturating_sub(unix);
    if secs < 60 {
        "played just now".to_string()
    } else if secs < 3600 {
        format!("played {}m ago", secs / 60)
    } else if secs < 86_400 {
        format!("played {}h ago", secs / 3600)
    } else {
        format!("played {}d ago", secs / 86_400)
    }
}

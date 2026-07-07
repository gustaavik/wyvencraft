//! Multiplayer menu: host a session (backed by a persistent world save) or join
//! one by IP address.

use std::net::{SocketAddr, ToSocketAddrs};
use std::sync::Arc;

use super::connecting_state::ConnectingState;
use super::{GameState, InGameState, MainMenuState, StateContext, Transition};
use crate::content::GameContent;
use crate::core::GameMode;
use crate::net::{DEFAULT_PORT, Host};
use crate::save::{self, WorldEntry, WorldSave};

pub struct MultiplayerMenuState {
    address: String,
    error: Option<String>,
    /// Game mode for a newly created hosted world.
    mode: GameMode,
    /// Saved worlds available to host.
    worlds: Vec<WorldEntry>,
    /// Index into `worlds`; `None` hosts a brand-new world.
    selected_world: Option<usize>,
    /// Name for a newly created hosted world.
    world_name: String,
}

impl Default for MultiplayerMenuState {
    fn default() -> Self {
        Self {
            address: format!("127.0.0.1:{DEFAULT_PORT}"),
            error: None,
            mode: GameMode::Survival,
            worlds: save::list_worlds(&save::saves_root()),
            selected_world: None,
            world_name: "Server World".to_string(),
        }
    }
}

impl MultiplayerMenuState {
    pub fn new() -> Self {
        Self::default()
    }

    /// Load or create the world to host, bind the server on its seed, and enter.
    fn host(&mut self, content: Arc<GameContent>) -> Transition {
        let game = match self.selected_world {
            Some(index) => {
                let Some(world) = self.worlds.get(index) else {
                    self.error = Some("Select a world".to_string());
                    return Transition::None;
                };
                WorldSave::open(&save::saves_root(), &world.slug).and_then(WorldSave::load)
            }
            None => {
                let name = self.world_name.trim().to_string();
                if name.is_empty() {
                    self.error = Some("Enter a world name".to_string());
                    return Transition::None;
                }
                WorldSave::create(&save::saves_root(), &name, save::random_seed(), self.mode)
                    .and_then(WorldSave::load)
            }
        };
        let game = match game {
            Ok(game) => game,
            Err(err) => {
                self.error = Some(err.to_string());
                return Transition::None;
            }
        };
        // Bind with the world's own seed so `Host::seed()` matches the world.
        match Host::bind(DEFAULT_PORT, game.save.meta.seed) {
            Ok(host) => {
                Transition::Replace(Box::new(InGameState::new_host_saved(content, game, host)))
            }
            Err(err) => {
                self.error = Some(format!("Host failed: {err}"));
                Transition::None
            }
        }
    }
}

impl GameState for MultiplayerMenuState {
    fn name(&self) -> &'static str {
        "MultiplayerMenu"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.heading("Multiplayer");
                ui.add_space(24.0);

                ui.label("World to host:");
                ui.add_space(4.0);
                let selected_label = match self.selected_world {
                    Some(index) => self
                        .worlds
                        .get(index)
                        .map(|w| w.meta.name.as_str())
                        .unwrap_or("New world"),
                    None => "New world",
                };
                egui::ComboBox::from_id_salt("host_world")
                    .width(220.0)
                    .selected_text(selected_label)
                    .show_ui(ui, |ui| {
                        ui.selectable_value(&mut self.selected_world, None, "New world");
                        for (index, world) in self.worlds.iter().enumerate() {
                            ui.selectable_value(
                                &mut self.selected_world,
                                Some(index),
                                &world.meta.name,
                            );
                        }
                    });
                ui.add_space(8.0);

                if self.selected_world.is_none() {
                    ui.add_sized(
                        [220.0, 24.0],
                        egui::TextEdit::singleline(&mut self.world_name).hint_text("World name"),
                    );
                    ui.add_space(8.0);
                    ui.label("Session mode:");
                    ui.horizontal(|ui| {
                        ui.add_space((ui.available_width() - 240.0).max(0.0) * 0.5);
                        ui.selectable_value(&mut self.mode, GameMode::Survival, "Survival");
                        ui.selectable_value(&mut self.mode, GameMode::Creative, "Creative");
                    });
                    ui.add_space(8.0);
                }

                if ui
                    .add_sized([220.0, 36.0], egui::Button::new("Host Game"))
                    .clicked()
                {
                    transition = self.host(ctx.content.clone());
                }

                ui.add_space(16.0);
                ui.label("Join by address:");
                ui.add_sized([220.0, 24.0], egui::TextEdit::singleline(&mut self.address));
                if ui
                    .add_sized([220.0, 36.0], egui::Button::new("Join"))
                    .clicked()
                {
                    match resolve_addr(&self.address) {
                        Some(addr) => {
                            transition = Transition::Replace(Box::new(ConnectingState::new(addr)));
                        }
                        None => self.error = Some("Invalid address".to_string()),
                    }
                }

                ui.add_space(16.0);
                if ui
                    .add_sized([220.0, 30.0], egui::Button::new("Back"))
                    .clicked()
                {
                    transition = Transition::Replace(Box::new(MainMenuState::new()));
                }

                if let Some(err) = &self.error {
                    ui.add_space(12.0);
                    ui.colored_label(egui::Color32::LIGHT_RED, err);
                }
            });
        });
        transition
    }
}

/// Parse `host:port` (or bare `host`, defaulting the port) into a socket address.
fn resolve_addr(input: &str) -> Option<SocketAddr> {
    let trimmed = input.trim();
    let with_port = if trimmed.contains(':') {
        trimmed.to_string()
    } else {
        format!("{trimmed}:{DEFAULT_PORT}")
    };
    with_port.to_socket_addrs().ok()?.next()
}

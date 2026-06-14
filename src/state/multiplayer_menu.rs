//! Multiplayer menu: host a session or join one by IP address.

use std::net::{SocketAddr, ToSocketAddrs};

use super::connecting_state::ConnectingState;
use super::{GameState, InGameState, MainMenuState, StateContext, Transition};
use crate::net::{DEFAULT_PORT, Host};

pub struct MultiplayerMenuState {
    address: String,
    error: Option<String>,
}

impl Default for MultiplayerMenuState {
    fn default() -> Self {
        Self {
            address: format!("127.0.0.1:{DEFAULT_PORT}"),
            error: None,
        }
    }
}

impl MultiplayerMenuState {
    pub fn new() -> Self {
        Self::default()
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

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.heading("Multiplayer");
                ui.add_space(24.0);

                if ui
                    .add_sized([220.0, 36.0], egui::Button::new("Host Game"))
                    .clicked()
                {
                    let seed = random_seed();
                    match Host::bind(DEFAULT_PORT, seed) {
                        Ok(host) => {
                            transition =
                                Transition::Replace(Box::new(InGameState::new_host(seed, host)));
                        }
                        Err(err) => self.error = Some(format!("Host failed: {err}")),
                    }
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

fn random_seed() -> u64 {
    use std::time::{SystemTime, UNIX_EPOCH};
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

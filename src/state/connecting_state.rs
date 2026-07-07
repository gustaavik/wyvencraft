//! Transient state while connecting to a host: pumps the client until it
//! connects and receives the `Welcome` (which carries the world seed), then
//! enters the world as a client.

use std::net::SocketAddr;
use std::time::Duration;

use super::{GameState, InGameState, MultiplayerMenuState, StateContext, Transition};
use crate::net::{Client, ServerMessage};

const TIMEOUT_SECS: f32 = 12.0;

pub struct ConnectingState {
    address: SocketAddr,
    client: Option<Client>,
    elapsed: f32,
    status: String,
}

impl ConnectingState {
    pub fn new(address: SocketAddr) -> Self {
        // Connect with the persisted profile identity so the host can recognise
        // a returning player and hand back their saved inventory/position.
        let identity = crate::save::client_identity();
        let (client, status) = match Client::connect(address, identity) {
            Ok(c) => (Some(c), format!("Connecting to {address}…")),
            Err(e) => (None, format!("Connection failed: {e}")),
        };
        Self {
            address,
            client,
            elapsed: 0.0,
            status,
        }
    }
}

impl GameState for ConnectingState {
    fn name(&self) -> &'static str {
        "Connecting"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        self.elapsed += ctx.dt;

        let Some(client) = self.client.as_mut() else {
            // Connect() failed outright; bail back to the menu after a moment.
            if self.elapsed > 2.0 {
                return Transition::Replace(Box::new(MultiplayerMenuState::new()));
            }
            return Transition::None;
        };

        if let Err(err) = client.pump(Duration::from_secs_f32(ctx.dt.max(1.0e-4))) {
            log::warn!("connection error: {err}");
            return Transition::Replace(Box::new(MultiplayerMenuState::new()));
        }

        // Wait for the Welcome message carrying the world seed + our id + mode
        // + the host's crafting recipes + any saved state it remembers for us.
        let mut welcome = None;
        for msg in client.receive() {
            if let ServerMessage::Welcome {
                seed,
                your_id,
                spawn,
                time_of_day,
                game_mode,
                content_hash,
                recipes,
                restored,
            } = msg
            {
                // Raw block/item ids cross the wire, so divergent content
                // definitions would silently corrupt the session. Refuse.
                if content_hash != ctx.content.hash {
                    log::warn!(
                        "content mismatch: host {content_hash:#018x} vs ours {:#018x}; refusing to join",
                        ctx.content.hash
                    );
                    return Transition::Replace(Box::new(MultiplayerMenuState::new()));
                }
                welcome = Some((
                    seed,
                    your_id,
                    spawn,
                    time_of_day,
                    game_mode,
                    recipes,
                    restored,
                ));
            }
        }
        let _ = client.flush();

        if let Some((seed, your_id, spawn, time_of_day, game_mode, recipes, restored)) = welcome {
            log::info!(
                "connected; world seed {seed}, player id {}, spawn {spawn:?}, time {time_of_day:.3}, game_mode {}, saved state: {}",
                your_id.0,
                game_mode.label(),
                if restored.is_some() {
                    "restored"
                } else {
                    "none"
                },
            );
            let client = self.client.take().expect("client present");
            return Transition::Replace(Box::new(InGameState::new_client(
                ctx.content.clone(),
                seed,
                client,
                your_id,
                spawn,
                time_of_day,
                game_mode,
                recipes,
                restored,
            )));
        }

        if self.elapsed > TIMEOUT_SECS {
            self.status = "Timed out".to_string();
            return Transition::Replace(Box::new(MultiplayerMenuState::new()));
        }
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                ui.heading(&self.status);
                ui.spinner();
            });
        });
        Transition::None
    }
}

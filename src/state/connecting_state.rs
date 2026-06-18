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
        let (client, status) = match Client::connect(address) {
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

        // Wait for the Welcome message carrying the world seed + our id.
        let mut welcome = None;
        for msg in client.receive() {
            if let ServerMessage::Welcome {
                seed,
                your_id,
                spawn,
                time_of_day,
            } = msg
            {
                welcome = Some((seed, your_id, spawn, time_of_day));
            }
        }
        let _ = client.flush();

        if let Some((seed, your_id, spawn, time_of_day)) = welcome {
            log::info!(
                "connected; world seed {seed}, player id {}, spawn {spawn:?}, time {time_of_day:.3}",
                your_id.0
            );
            let client = self.client.take().expect("client present");
            return Transition::Replace(Box::new(InGameState::new_client(
                seed,
                client,
                your_id,
                spawn,
                time_of_day,
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

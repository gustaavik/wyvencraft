//! Transient state while connecting to a host: fetches a join ticket, pumps the
//! client until it connects and receives the `Welcome` (which carries the world
//! seed), then enters the world as a client.
//!
//! The ticket is fetched here rather than at login because it lives about two
//! minutes — one obtained at sign-in would be long stale by the time anyone
//! clicked Join. The fetch is blocking, so it happens on a worker while this
//! screen spins.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use super::{GameState, InGameState, MultiplayerMenuState, StateContext, Transition};
use crate::net::{Client, ServerMessage};
use wyven_auth::{AccountState, AuthClient, AuthError, HttpAuthClient, JoinTicket};

const TIMEOUT_SECS: f32 = 12.0;

/// What the ticket worker reports back.
type TicketResult = Result<JoinTicket, AuthError>;

pub struct ConnectingState {
    address: SocketAddr,
    /// The netcode identity to present — derived from the account.
    identity: u64,
    /// In flight until the ticket lands.
    pending_ticket: Option<Receiver<TicketResult>>,
    client: Option<Client>,
    elapsed: f32,
    status: String,
    /// Set once we have given up, so `update` stops trying and drops back.
    failed: bool,
}

impl ConnectingState {
    /// Start connecting. The account is used to obtain the join ticket.
    pub fn new(address: SocketAddr, account: &AccountState) -> Self {
        Self::with_client(address, account, Arc::new(HttpAuthClient::from_env()))
    }

    /// With an injected auth client, for tests.
    pub fn with_client(
        address: SocketAddr,
        account: &AccountState,
        auth: Arc<dyn AuthClient>,
    ) -> Self {
        // Checked here as well as in the menu, because this is the path that
        // matters: the menu's greyed-out button is a courtesy, this is the gate.
        if !account.can_play_multiplayer() {
            return Self {
                address,
                identity: 0,
                pending_ticket: None,
                client: None,
                elapsed: 0.0,
                status: "Sign in to play with other people.".to_string(),
                failed: true,
            };
        }

        // Derived from the account, so the host matches a returning player to
        // their saved inventory and position across machines. The host checks
        // that this agrees with the ticket before letting anyone in.
        // Offline there is no account to derive one from, so it falls back to
        // the local profile id. Harmless: an offline client has no ticket to
        // present and every verifying host will turn it away regardless.
        let identity = account
            .netcode_id()
            .unwrap_or_else(crate::save::local_identity);

        let (tx, rx) = channel();
        let account = account.clone();
        std::thread::spawn(move || {
            let now = unix_now();
            let _ = tx.send(account.issue_ticket(auth.as_ref(), now));
        });

        Self {
            address,
            identity,
            pending_ticket: Some(rx),
            client: None,
            elapsed: 0.0,
            status: "Getting a join ticket...".to_string(),
            failed: false,
        }
    }

    /// Once the ticket lands, open the connection with it attached.
    fn poll_ticket(&mut self) -> Transition {
        let Some(rx) = self.pending_ticket.as_ref() else {
            return Transition::None;
        };

        match rx.try_recv() {
            Ok(Ok(ticket)) => {
                self.pending_ticket = None;
                match Client::connect(
                    self.address,
                    self.identity,
                    crate::net::PROTOCOL_ID,
                    Some(ticket.slot),
                ) {
                    Ok(client) => {
                        self.client = Some(client);
                        self.status = format!("Connecting to {}...", self.address);
                    }
                    Err(err) => {
                        self.status = format!("Connection failed: {err}");
                        self.failed = true;
                    }
                }
                Transition::None
            }
            Ok(Err(err)) => {
                self.pending_ticket = None;
                log::warn!("could not get a join ticket: {err}");
                self.status = if err.is_offline() {
                    "The account server is unreachable — cannot join.".to_string()
                } else {
                    format!("Could not join: {err}")
                };
                self.failed = true;
                Transition::None
            }
            Err(TryRecvError::Empty) => Transition::None,
            Err(TryRecvError::Disconnected) => {
                self.pending_ticket = None;
                self.status = "Could not get a join ticket.".to_string();
                self.failed = true;
                Transition::None
            }
        }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl GameState for ConnectingState {
    fn name(&self) -> &'static str {
        "Connecting"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        self.elapsed += ctx.dt;

        // Long enough to read why, short enough not to feel stuck.
        if self.failed {
            if self.elapsed > 3.0 {
                return Transition::Replace(Box::new(MultiplayerMenuState::new()));
            }
            return Transition::None;
        }

        let transition = self.poll_ticket();
        if !matches!(transition, Transition::None) {
            return transition;
        }

        let Some(client) = self.client.as_mut() else {
            // Still waiting on the ticket, or it failed and `failed` is set.
            if self.elapsed > TIMEOUT_SECS {
                self.status = "Timed out".to_string();
                self.failed = true;
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
                if content_hash != ctx.resources.content.hash {
                    log::warn!(
                        "content mismatch: host {content_hash:#018x} vs ours {:#018x}; refusing to join",
                        ctx.resources.content.hash
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
                ctx.resources.content.clone(),
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

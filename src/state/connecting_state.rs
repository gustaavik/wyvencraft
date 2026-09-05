//! Transient state while connecting to a host: resolves the address, fetches a
//! join ticket, pumps the client until it connects and receives the `Welcome`
//! (which carries the world seed), then enters the world as a client.
//!
//! The ticket is fetched here rather than at login because it lives about two
//! minutes — one obtained at sign-in would be long stale by the time anyone
//! clicked Join. Both the fetch and the name lookup block, so both happen on a
//! worker while this screen spins. That is also why the target is carried as
//! *text*: a saved server is a hostname the player typed, and resolving it on
//! the frame that drew the Join button would freeze the menu for as long as the
//! resolver takes to give up.
//!
//! Every one of those waits is a wait the player did not ask for, so the screen
//! is escapable at any point in it: [`ConnectingState::cancel`] says goodbye if
//! we got as far as speaking to a host, abandons the worker, and drops back to
//! the server list.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use winit::keyboard::KeyCode;

use super::{GameState, InGameState, MultiplayerMenuState, StateContext, Transition, Wyvencraft};
use crate::net::{Client, ServerMessage, address};
use wyven_auth::{AccountState, AuthClient, AuthError, HttpAuthClient, JoinTicket};

const TIMEOUT_SECS: f32 = 12.0;

/// What the worker reports back: where to connect, and what to present there.
type ConnectPrep = (Result<SocketAddr, String>, Result<JoinTicket, AuthError>);

pub struct ConnectingState {
    /// Where the player asked to go, as typed. Shown while connecting.
    target: String,
    /// The netcode identity to present — derived from the account.
    identity: u64,
    /// In flight until the address and ticket land.
    pending: Option<Receiver<ConnectPrep>>,
    client: Option<Client>,
    elapsed: f32,
    status: String,
    /// Set once we have given up, so `update` stops trying and drops back.
    failed: bool,
}

impl ConnectingState {
    /// Start connecting to `target` — `host`, `host:port` or an address, exactly
    /// as the player typed or saved it. The account is used to obtain the join
    /// ticket.
    pub fn new(target: impl Into<String>, account: &AccountState) -> Self {
        Self::with_client(target, account, Arc::new(HttpAuthClient::from_env()))
    }

    /// With an injected auth client, for tests.
    pub fn with_client(
        target: impl Into<String>,
        account: &AccountState,
        auth: Arc<dyn AuthClient>,
    ) -> Self {
        let target = target.into();
        // Checked here as well as in the menu, because this is the path that
        // matters: the menu's greyed-out button is a courtesy, this is the gate.
        if !account.can_play_multiplayer() {
            return Self {
                target,
                identity: 0,
                pending: None,
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
        let lookup = target.clone();
        std::thread::spawn(move || {
            let now = unix_now();
            // Both blocking halves on one worker: the DNS lookup and the ticket
            // request. The ticket is issued through `net::ticket` rather than
            // the account directly, so a refresh token rotated on the way is
            // written to disk before it can be lost.
            let resolved = address::resolve(&lookup);
            let ticket = crate::net::ticket::issue(&account, auth.as_ref(), now);
            let _ = tx.send((resolved, ticket));
        });

        Self {
            target,
            identity,
            pending: Some(rx),
            client: None,
            elapsed: 0.0,
            status: "Getting a join ticket...".to_string(),
            failed: false,
        }
    }

    /// Once the address and ticket land, open the connection with it attached.
    fn poll_prep(&mut self) -> Transition {
        let Some(rx) = self.pending.as_ref() else {
            return Transition::None;
        };

        let (resolved, ticket) = match rx.try_recv() {
            Ok(prep) => prep,
            Err(TryRecvError::Empty) => return Transition::None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.status = "Could not get a join ticket.".to_string();
                self.failed = true;
                return Transition::None;
            }
        };
        self.pending = None;

        let ticket = match ticket {
            Ok(ticket) => ticket,
            Err(err) => {
                log::warn!("could not get a join ticket: {err}");
                self.status = if err.is_offline() {
                    "The account server is unreachable — cannot join.".to_string()
                } else {
                    format!("Could not join: {err}")
                };
                self.failed = true;
                return Transition::None;
            }
        };
        let address = match resolved {
            Ok(address) => address,
            Err(reason) => {
                self.status = reason;
                self.failed = true;
                return Transition::None;
            }
        };

        match Client::connect(
            address,
            self.identity,
            crate::net::PROTOCOL_ID,
            Some(ticket.slot),
        ) {
            Ok(client) => {
                self.client = Some(client);
                self.status = format!("Connecting to {}...", self.target);
            }
            Err(err) => {
                self.status = format!("Connection failed: {err}");
                self.failed = true;
            }
        }
        Transition::None
    }

    /// Give up on this connection: stop everything in flight, and tell a host
    /// we may already be speaking to.
    ///
    /// Dropping the client alone would only close its socket, which a host
    /// cannot tell apart from a crashed peer — it would hold the slot for the
    /// whole of the connection timeout. The worker cannot be interrupted, but
    /// dropping the receiver makes its `send` fail, so it finishes and exits on
    /// its own instead of being waited on here.
    fn abandon(&mut self) {
        if let Some(mut client) = self.client.take() {
            client.disconnect();
        }
        self.pending = None;
    }

    /// What the player asked to leave. Also the timeout's own way out, which is
    /// the same act with nobody pressing anything.
    fn cancel(&mut self, ctx: &mut StateContext) -> Transition {
        self.abandon();
        Transition::Replace(Box::new(MultiplayerMenuState::new(ctx)))
    }

    /// The button under the status line. Once there is nothing left to wait
    /// for, cancelling is just going back, and saying so avoids implying the
    /// attempt is still running.
    fn dismiss_label(&self) -> &'static str {
        if self.failed { "Back" } else { "Cancel" }
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

impl GameState<Wyvencraft> for ConnectingState {
    fn name(&self) -> &'static str {
        "Connecting"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        self.elapsed += ctx.dt;

        // Checked before anything else, including the failure hold below: Esc
        // means leave *now*, whatever this screen is currently waiting on.
        if ctx.input.just_pressed(KeyCode::Escape) {
            log::info!("connection to {} cancelled", self.target);
            return self.cancel(ctx);
        }

        // Long enough to read why, short enough not to feel stuck.
        if self.failed {
            if self.elapsed > 3.0 {
                return Transition::Replace(Box::new(MultiplayerMenuState::new(ctx)));
            }
            return Transition::None;
        }

        let transition = self.poll_prep();
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
            return self.cancel(ctx);
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
                if content_hash != ctx.shared.content.hash {
                    log::warn!(
                        "content mismatch: host {content_hash:#018x} vs ours {:#018x}; refusing to join",
                        ctx.shared.content.hash
                    );
                    return Transition::Replace(Box::new(MultiplayerMenuState::new(ctx)));
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
                ctx.shared.content.clone(),
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
            return self.cancel(ctx);
        }
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        // Collected rather than acted on inside the closure: `cancel` needs
        // `&mut self`, which the panel is still holding for the status text.
        let mut dismissed = false;
        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(120.0);
                ui.heading(&self.status);
                // Nothing is in flight once we have failed, and a spinner that
                // keeps turning under an error reads as one.
                if !self.failed {
                    ui.spinner();
                }
                ui.add_space(24.0);
                dismissed = ui
                    .add_sized([160.0, 32.0], egui::Button::new(self.dismiss_label()))
                    .clicked();
                ui.add_space(4.0);
                ui.label(egui::RichText::new("or press Esc").weak().small());
            });
        });

        if dismissed {
            log::info!("connection to {} cancelled", self.target);
            return self.cancel(ctx);
        }
        Transition::None
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_auth::FakeAuthClient;

    fn signed_in(auth: &FakeAuthClient) -> AccountState {
        let account = AccountState::new();
        account.sign_in(auth.login("gustav", "hunter2").expect("signs in"));
        account
    }

    /// The whole point of the button: pressing it while the ticket is still in
    /// flight must leave nothing that can quietly finish the job afterwards.
    /// The worker cannot be stopped, so what is asserted is that its answer can
    /// no longer be collected — a cancelled screen never opens a socket.
    #[test]
    fn cancelling_mid_flight_leaves_nothing_that_could_still_connect() {
        let auth = FakeAuthClient::new().with_account("gustav", "hunter2");
        let account = signed_in(&auth);
        let mut state = ConnectingState::with_client("127.0.0.1:6091", &account, Arc::new(auth));
        assert!(state.pending.is_some(), "the worker is in flight");

        state.abandon();

        assert!(
            state.pending.is_none(),
            "the worker's answer is unreachable"
        );
        assert!(state.client.is_none());
        // Even once the worker has answered, there is nothing left to answer to.
        assert!(matches!(state.poll_prep(), Transition::None));
        assert!(state.client.is_none(), "a cancelled screen never connects");
    }

    /// Nothing is being waited on once the attempt has failed, so the button
    /// stops offering to stop it.
    #[test]
    fn the_button_says_back_once_there_is_nothing_left_to_wait_for() {
        let auth = FakeAuthClient::new().with_account("gustav", "hunter2");
        let account = signed_in(&auth);
        let mut state = ConnectingState::with_client("127.0.0.1:6091", &account, Arc::new(auth));
        assert_eq!(state.dismiss_label(), "Cancel");

        state.failed = true;
        assert_eq!(state.dismiss_label(), "Back");
    }

    /// A screen that never started a worker still tears down cleanly — the
    /// signed-out path builds one of these, and its button is a plain Back.
    #[test]
    fn a_screen_that_never_started_still_cancels() {
        let auth = FakeAuthClient::new();
        let mut state =
            ConnectingState::with_client("127.0.0.1:6091", &AccountState::new(), Arc::new(auth));
        assert!(state.failed, "signed out cannot join");
        assert_eq!(state.dismiss_label(), "Back");

        state.abandon();

        assert!(state.pending.is_none());
        assert!(state.client.is_none());
    }
}

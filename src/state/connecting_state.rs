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
//!
//! **Every way this can go wrong ends here rather than back at the server
//! list.** A failure that drops the player onto the previous screen tells them
//! only that nothing happened; [`ConnectingState::fail`] keeps them where they
//! were, with what went wrong on it, until they dismiss it themselves. That is
//! the whole reason this screen outlives its own failure.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use winit::keyboard::KeyCode;

use super::{GameState, InGameState, MultiplayerMenuState, StateContext, Transition, Wyvencraft};
use crate::net::{Client, ServerMessage, address};
use wyven_auth::{AccountState, AuthClient, AuthError, HttpAuthClient, JoinTicket};

const TIMEOUT_SECS: f32 = 12.0;

/// How wide the detail line under the headline is allowed to get before it
/// wraps. Narrower than any window it will be drawn in, so it always wraps at a
/// readable measure rather than at the window's edge.
const DETAIL_WIDTH: f32 = 420.0;

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
    /// The headline: what is happening, or what went wrong. Written for the
    /// player, not the log.
    status: String,
    /// The technical reason under it, when there is one worth reading — a
    /// transport error, a pair of content hashes. Kept apart from `status` so
    /// the headline stays a sentence a player can act on.
    detail: Option<String>,
    /// Set once we have given up, so `update` stops trying and the screen waits
    /// to be dismissed instead.
    failed: bool,
    /// Whether the handshake ever completed. What separates "could not reach
    /// it" from "reached it and then lost it", which are different problems
    /// with different fixes.
    reached: bool,
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
                // The same sentence the main menu shows, because it is the same
                // dead end: there is no login screen to send anyone to.
                detail: Some("Sign in from the Wyvencraft launcher, then try again.".to_string()),
                failed: true,
                reached: false,
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
            detail: None,
            failed: false,
            reached: false,
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
                self.fail(
                    "Could not get a join ticket.",
                    Some("The request ended without an answer.".to_string()),
                );
                return Transition::None;
            }
        };
        self.pending = None;

        let ticket = match ticket {
            Ok(ticket) => ticket,
            Err(err) => {
                log::warn!("could not get a join ticket: {err}");
                if err.is_offline() {
                    self.fail(
                        "The account server is unreachable — cannot join.",
                        Some(err.to_string()),
                    );
                } else {
                    self.fail("Could not get a join ticket.", Some(err.to_string()));
                }
                return Transition::None;
            }
        };
        let address = match resolved {
            Ok(address) => address,
            // Already phrased for a player by `address::resolve` — it names the
            // host it could not find, which is the one thing worth saying.
            Err(reason) => {
                self.fail(reason, None);
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
                self.fail(
                    format!("Could not open a connection to {}", self.target),
                    Some(err.to_string()),
                );
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

    /// Stop, and stay here saying why.
    ///
    /// Deliberately *not* a transition. Dropping back to the server list is
    /// what a failure used to do, and it left the player looking at the screen
    /// they started from with nothing to explain why they were back on it —
    /// indistinguishable from a click that did not register. So the screen
    /// keeps itself, shows `message` (with `detail` under it when there is
    /// something more precise to say), and waits for Back or Esc.
    fn fail(&mut self, message: impl Into<String>, detail: Option<String>) {
        self.abandon();
        self.status = message.into();
        self.detail = detail;
        self.failed = true;
    }

    /// What the player asked to leave.
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

        // Held until dismissed rather than timed out. A message that clears
        // itself is one the player may never have been looking at, and the
        // three seconds this used to wait were three seconds of an explanation
        // nobody had asked to have taken away.
        if self.failed {
            return Transition::None;
        }

        let transition = self.poll_prep();
        if !matches!(transition, Transition::None) {
            return transition;
        }

        let Some(client) = self.client.as_mut() else {
            // Still waiting on the ticket, or it failed and `failed` is set.
            if self.elapsed > TIMEOUT_SECS {
                self.fail(
                    "Timed out getting a join ticket.",
                    Some("The account server did not answer.".to_string()),
                );
            }
            return Transition::None;
        };

        if let Err(err) = client.pump(Duration::from_secs_f32(ctx.dt.max(1.0e-4))) {
            log::warn!("connection error: {err}");
            // The transport's own words as the detail. A refused join reads
            // "disconnected: server denied connection" here, which is the
            // closest thing to a reason a host can give: netcode turns an
            // unverifiable ticket away before either side says anything at the
            // application level, so there is no message to phrase it better
            // with.
            let headline = if self.reached {
                format!("Lost the connection to {}", self.target)
            } else {
                format!("Could not join {}", self.target)
            };
            self.fail(headline, Some(err.to_string()));
            return Transition::None;
        }

        // First frame the handshake is complete. Worth saying: from here the
        // wait is the host's to answer, not the network's to establish.
        if !self.reached && client.is_connected() {
            self.reached = true;
            self.status = "Entering the world...".to_string();
        }

        // Wait for the Welcome message carrying the world seed + our id + mode
        // + the host's crafting recipes + any saved state it remembers for us.
        let mut welcome = None;
        // Recorded rather than acted on: `fail` needs `&mut self`, and `client`
        // is borrowed out of it for as long as this loop runs.
        let mut mismatch = None;
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
                    mismatch = Some((content_hash, ctx.shared.content.hash));
                    break;
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

        if let Some((theirs, ours)) = mismatch {
            self.fail(
                "This server is running different content.",
                Some(format!(
                    "Its blocks and items do not match yours (server {theirs:#018x}, you {ours:#018x}). Both sides have to be on the same version."
                )),
            );
            return Transition::None;
        }

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
            if self.reached {
                self.fail(
                    format!("{} never sent the world.", self.target),
                    Some(
                        "The connection is open but the server did not finish letting you in."
                            .to_string(),
                    ),
                );
            } else {
                self.fail(
                    format!("Could not reach {}", self.target),
                    Some(format!(
                        "No answer after {TIMEOUT_SECS:.0} seconds. Check the address, and that the server is running."
                    )),
                );
            }
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
                if let Some(detail) = &self.detail {
                    ui.add_space(8.0);
                    // Wrapped inside a column narrower than the window: a
                    // transport error is one long line, and centred text that
                    // runs the full width of a maximised window is unreadable.
                    ui.allocate_ui_with_layout(
                        egui::vec2(DETAIL_WIDTH, 0.0),
                        egui::Layout::top_down(egui::Align::Center),
                        |ui| {
                            ui.label(egui::RichText::new(detail).weak());
                        },
                    );
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

    /// The complaint this answers: a failure used to drop the player back on
    /// the server list, which looks exactly like a click that never registered.
    /// Failing has to leave them here, holding the reason.
    #[test]
    fn a_failure_stays_on_this_screen_and_keeps_the_reason() {
        let auth = FakeAuthClient::new().with_account("gustav", "hunter2");
        let account = signed_in(&auth);
        let mut state = ConnectingState::with_client("127.0.0.1:6091", &account, Arc::new(auth));

        state.fail(
            "Could not join 127.0.0.1:6091",
            Some("disconnected: server denied connection".to_string()),
        );

        assert!(state.failed);
        assert_eq!(state.status, "Could not join 127.0.0.1:6091");
        assert_eq!(
            state.detail.as_deref(),
            Some("disconnected: server denied connection"),
            "the transport's own words are what say which failure this was",
        );
        // Failing is also a teardown: nothing may still be trying underneath a
        // screen that has already told the player it gave up.
        assert!(state.pending.is_none());
        assert!(state.client.is_none());
        assert_eq!(state.dismiss_label(), "Back");
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
        assert!(
            !state.status.is_empty(),
            "even the earliest refusal says something"
        );
        assert_eq!(state.dismiss_label(), "Back");

        state.abandon();

        assert!(state.pending.is_none());
        assert!(state.client.is_none());
    }
}

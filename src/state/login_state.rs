//! The sign-in screen: the first thing the game shows.
//!
//! Every auth call is blocking, and this runs on the frame thread, so the work
//! goes to a one-shot worker and the UI polls a channel. That keeps the window
//! responsive while a request is in flight — and, more to the point, means a
//! dead auth server stalls a spinner rather than the whole process.

use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};

use super::{GameState, MainMenuState, StateContext, Transition, Wyvencraft};
use crate::save::AccountProfile;
use wyven_auth::{AccountState, AuthClient, AuthError, AuthSession, HttpAuthClient, KeyCache};

/// Which form is showing.
#[derive(PartialEq, Eq, Clone, Copy)]
enum Mode {
    SignIn,
    Register,
}

/// What a worker sends back.
enum Outcome {
    Session(Box<AuthSession>),
    Failed(AuthError),
}

pub struct LoginState {
    account: AccountState,
    client: Arc<dyn AuthClient>,

    mode: Mode,
    username: String,
    password: String,
    email: String,

    /// Set while a request is in flight; taken when it lands.
    pending: Option<Receiver<Outcome>>,
    /// Shown under the form.
    message: Option<String>,
    /// Whether the last failure was "server unreachable", which is what offers
    /// the offline path rather than just reporting an error.
    offline_offered: bool,
    /// True while restoring a saved session at startup, so the screen shows a
    /// spinner instead of flashing a login form at a player who is signed in.
    restoring: bool,
}

impl LoginState {
    /// The ordinary entry point: real HTTP client, restoring any saved session.
    pub fn new(account: AccountState) -> Self {
        Self::with_client(account, Arc::new(HttpAuthClient::from_env()))
    }

    /// With an injected client, for tests.
    pub fn with_client(account: AccountState, client: Arc<dyn AuthClient>) -> Self {
        let mut state = Self {
            account,
            client,
            mode: Mode::SignIn,
            username: String::new(),
            password: String::new(),
            email: String::new(),
            pending: None,
            message: None,
            offline_offered: false,
            restoring: false,
        };

        // A player who signed in last time should not be asked again. The saved
        // refresh token is spent for a fresh session in the background; if that
        // fails the form appears as usual.
        if let Some(saved) = crate::save::stored_account() {
            state.username = saved.username.clone();
            state.restoring = true;
            state.spawn(move |client| client.refresh(&saved.refresh_token));
        }

        state
    }

    /// Run `task` on a worker and deliver the result to [`Self::poll`].
    fn spawn<F>(&mut self, task: F)
    where
        F: FnOnce(&dyn AuthClient) -> Result<AuthSession, AuthError> + Send + 'static,
    {
        let (tx, rx) = channel();
        let client = self.client.clone();
        self.pending = Some(rx);
        self.message = None;

        std::thread::spawn(move || {
            let outcome = match task(client.as_ref()) {
                Ok(session) => Outcome::Session(Box::new(session)),
                Err(err) => Outcome::Failed(err),
            };
            // The receiver is gone if the player left the screen; nothing to do.
            let _ = tx.send(outcome);
        });
    }

    /// Check for a finished request. Returns the transition to apply.
    fn poll(&mut self) -> Transition {
        let Some(rx) = self.pending.as_ref() else {
            return Transition::None;
        };

        match rx.try_recv() {
            Ok(Outcome::Session(session)) => {
                self.pending = None;
                self.restoring = false;
                self.sign_in(*session);
                Transition::Replace(Box::new(MainMenuState::new()))
            }
            Ok(Outcome::Failed(err)) => {
                self.pending = None;
                self.offline_offered = err.is_offline();

                if self.restoring {
                    // A saved session that would not restore is not worth
                    // reporting as an error — the player simply signs in again.
                    self.restoring = false;
                    if !self.offline_offered {
                        // Only clear the stored token when the server actively
                        // rejected it. A network blip must not log the player
                        // out of a session that is still perfectly good.
                        let _ = crate::save::store_account(None);
                        self.message = Some("Your session expired — please sign in.".to_string());
                    } else {
                        self.message = Some(format!("{err}"));
                    }
                } else {
                    self.message = Some(format!("{err}"));
                }
                Transition::None
            }
            Err(TryRecvError::Empty) => Transition::None,
            Err(TryRecvError::Disconnected) => {
                self.pending = None;
                self.restoring = false;
                self.message = Some("The sign-in attempt failed unexpectedly.".to_string());
                Transition::None
            }
        }
    }

    /// Record a successful sign-in: in memory, on disk, and — for a future host
    /// — the verification keys.
    fn sign_in(&mut self, session: AuthSession) {
        let _ = crate::save::store_account(Some(AccountProfile {
            account_id: session.identity.account_id.to_string(),
            username: session.identity.username.clone(),
            refresh_token: session.refresh_token.clone(),
        }));

        log::info!("signed in as {}", session.identity);
        self.account.sign_in(session);

        // Refresh the cached ticket keys while there is definitely a working
        // connection. This client may host later, and a host with no keys turns
        // everyone away — so the moment to fetch them is the moment we know the
        // server is reachable, not the moment someone tries to join.
        self.refresh_keys();
    }

    fn refresh_keys(&self) {
        let client = self.client.clone();
        std::thread::spawn(move || match client.public_keys() {
            Ok(keys) if !keys.is_empty() => match KeyCache::new().store(&keys) {
                Ok(()) => log::info!("cached {} auth key(s) for hosting", keys.len()),
                Err(err) => log::warn!("could not cache auth keys: {err}"),
            },
            Ok(_) => log::warn!("the auth server published no keys"),
            Err(err) => log::warn!("could not fetch auth keys: {err}"),
        });
    }

    fn submit(&mut self) {
        let username = self.username.trim().to_string();
        let password = std::mem::take(&mut self.password);
        let email = self.email.trim().to_string();

        if username.is_empty() || password.is_empty() {
            self.message = Some("Enter a username and password.".to_string());
            return;
        }

        match self.mode {
            Mode::SignIn => self.spawn(move |client| client.login(&username, &password)),
            Mode::Register => {
                if email.is_empty() {
                    self.message = Some("Enter an email address.".to_string());
                    return;
                }
                self.spawn(move |client| client.register(&username, &email, &password));
            }
        }
    }

    /// Continue without an account. Singleplayer only.
    fn play_offline(&mut self) -> Transition {
        log::info!("continuing offline; multiplayer will be unavailable");
        self.account.set_offline();
        Transition::Replace(Box::new(MainMenuState::new()))
    }

    fn busy(&self) -> bool {
        self.pending.is_some()
    }
}

impl GameState<Wyvencraft> for LoginState {
    fn name(&self) -> &'static str {
        "Login"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        self.poll()
    }

    fn ui(&mut self, egui_ctx: &egui::Context, _ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        let busy = self.busy();

        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.heading(egui::RichText::new("Wyvencraft").size(48.0));
                ui.add_space(24.0);

                if self.restoring {
                    ui.label("Signing you in...");
                    ui.spinner();
                    return;
                }

                ui.label(match self.mode {
                    Mode::SignIn => "Sign in to play",
                    Mode::Register => "Create an account",
                });
                ui.add_space(16.0);

                let field_width = 260.0;
                ui.add_enabled_ui(!busy, |ui| {
                    ui.add_sized(
                        [field_width, 24.0],
                        egui::TextEdit::singleline(&mut self.username).hint_text("Username"),
                    );
                    ui.add_space(6.0);

                    if self.mode == Mode::Register {
                        ui.add_sized(
                            [field_width, 24.0],
                            egui::TextEdit::singleline(&mut self.email).hint_text("Email"),
                        );
                        ui.add_space(6.0);
                    }

                    let password = ui.add_sized(
                        [field_width, 24.0],
                        egui::TextEdit::singleline(&mut self.password)
                            .hint_text("Password")
                            .password(true),
                    );
                    // Enter submits, which is what anyone typing a password
                    // expects.
                    if password.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                        self.submit();
                    }
                });

                ui.add_space(12.0);

                if busy {
                    ui.spinner();
                } else {
                    let label = match self.mode {
                        Mode::SignIn => "Sign in",
                        Mode::Register => "Create account",
                    };
                    if ui
                        .add_sized([field_width, 32.0], egui::Button::new(label))
                        .clicked()
                    {
                        self.submit();
                    }

                    ui.add_space(6.0);
                    let toggle = match self.mode {
                        Mode::SignIn => "Create an account instead",
                        Mode::Register => "I already have an account",
                    };
                    if ui.link(toggle).clicked() {
                        self.mode = match self.mode {
                            Mode::SignIn => Mode::Register,
                            Mode::Register => Mode::SignIn,
                        };
                        self.message = None;
                    }
                }

                if let Some(message) = &self.message {
                    ui.add_space(12.0);
                    ui.colored_label(egui::Color32::from_rgb(220, 120, 120), message);
                }

                // Offered once the server has actually been shown to be
                // unreachable, rather than as a standing "skip login" button.
                if self.offline_offered && !busy {
                    ui.add_space(16.0);
                    ui.separator();
                    ui.add_space(8.0);
                    ui.label("The account server is unreachable.");
                    if ui
                        .add_sized([field_width, 30.0], egui::Button::new("Play offline"))
                        .clicked()
                    {
                        transition = self.play_offline();
                    }
                    ui.small("Singleplayer only — multiplayer needs an account.");
                }

                ui.add_space(20.0);
                if ui.link("Quit").clicked() {
                    transition = Transition::Quit;
                }
            });
        });

        transition
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_auth::FakeAuthClient;

    /// Drive `poll` until the worker thread lands, so a test does not depend on
    /// scheduling. Fails rather than hanging if nothing ever arrives.
    fn settle(state: &mut LoginState) -> Transition {
        for _ in 0..2_000 {
            match state.poll() {
                Transition::None if state.busy() => {
                    std::thread::sleep(std::time::Duration::from_millis(1))
                }
                other => return other,
            }
        }
        panic!("the auth request never completed");
    }

    fn login_state(client: FakeAuthClient) -> (LoginState, AccountState) {
        let account = AccountState::new();
        let state = LoginState {
            account: account.clone(),
            client: Arc::new(client),
            mode: Mode::SignIn,
            username: String::new(),
            password: String::new(),
            email: String::new(),
            pending: None,
            message: None,
            offline_offered: false,
            restoring: false,
        };
        (state, account)
    }

    #[test]
    fn signing_in_with_good_credentials_reaches_the_main_menu() {
        let (mut state, account) =
            login_state(FakeAuthClient::new().with_account("gustav", "hunter2hunter2"));
        state.username = "gustav".to_string();
        state.password = "hunter2hunter2".to_string();

        state.submit();
        let transition = settle(&mut state);

        assert!(matches!(transition, Transition::Replace(_)));
        assert_eq!(account.username().as_deref(), Some("gustav"));
        assert!(account.can_play_multiplayer());
    }

    #[test]
    fn a_wrong_password_stays_on_the_screen_and_explains() {
        let (mut state, account) =
            login_state(FakeAuthClient::new().with_account("gustav", "hunter2hunter2"));
        state.username = "gustav".to_string();
        state.password = "wrong password".to_string();

        state.submit();
        let transition = settle(&mut state);

        assert!(matches!(transition, Transition::None));
        assert!(state.message.is_some());
        assert!(!account.can_play_multiplayer());
        assert!(
            !state.offline_offered,
            "a refusal must not offer offline play"
        );
    }

    /// The offline path is offered only when the server is actually unreachable
    /// — never as a way around a rejected password.
    #[test]
    fn an_unreachable_server_offers_offline_play() {
        let (mut state, account) = login_state(
            FakeAuthClient::new()
                .with_account("gustav", "hunter2hunter2")
                .offline(),
        );
        state.username = "gustav".to_string();
        state.password = "hunter2hunter2".to_string();

        state.submit();
        settle(&mut state);

        assert!(state.offline_offered);

        let transition = state.play_offline();
        assert!(matches!(transition, Transition::Replace(_)));
        assert!(
            !account.can_play_multiplayer(),
            "offline play must not unlock multiplayer"
        );
    }

    #[test]
    fn an_empty_form_is_refused_without_a_request() {
        let (mut state, _) = login_state(FakeAuthClient::new());

        state.submit();

        assert!(!state.busy(), "no request should have been sent");
        assert!(state.message.is_some());
    }

    #[test]
    fn registering_requires_an_email() {
        let (mut state, _) = login_state(FakeAuthClient::new());
        state.mode = Mode::Register;
        state.username = "gustav".to_string();
        state.password = "hunter2hunter2".to_string();

        state.submit();

        assert!(!state.busy());
        assert!(state.message.is_some());
    }

    #[test]
    fn registering_signs_the_new_player_straight_in() {
        let (mut state, account) = login_state(FakeAuthClient::new());
        state.mode = Mode::Register;
        state.username = "newplayer".to_string();
        state.email = "new@example.test".to_string();
        state.password = "hunter2hunter2".to_string();

        state.submit();
        let transition = settle(&mut state);

        assert!(matches!(transition, Transition::Replace(_)));
        assert_eq!(account.username().as_deref(), Some("newplayer"));
    }

    /// The password must not survive in the form after being submitted.
    #[test]
    fn the_password_field_is_cleared_on_submit() {
        let (mut state, _) =
            login_state(FakeAuthClient::new().with_account("gustav", "pw-correct"));
        state.username = "gustav".to_string();
        state.password = "pw-correct".to_string();

        state.submit();

        assert!(state.password.is_empty());
        settle(&mut state);
    }
}

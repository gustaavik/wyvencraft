//! Who this client is signed in as, for the lifetime of the process.
//!
//! One shared value the whole game reads: the menus to decide whether
//! multiplayer is available, `ConnectingState` to fetch a join ticket, and
//! `save` to key the world's player records. It lives behind an `RwLock` and is
//! handed around as an `Arc` in `StateContext::resources`, rather than being a
//! global — the same reason every other shared thing in this codebase is passed
//! explicitly.

use std::sync::Arc;

use parking_lot::RwLock;

use crate::client::{AuthClient, AuthError, JoinTicket};
use crate::session::{AccountIdentity, AuthSession};

/// How this client is signed in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AccountStatus {
    /// Not signed in. Only reachable before login, or after logging out.
    SignedOut,
    /// Signed in and able to reach the auth server.
    SignedIn(AccountIdentity),
    /// Playing without an account.
    ///
    /// Singleplayer works; multiplayer does not, because a host has no way to
    /// know who this is. Entered when no session could be restored — see the
    /// game's `boot::start::boot_account`.
    Offline,
}

impl AccountStatus {
    /// The name to show, if there is one.
    pub fn username(&self) -> Option<&str> {
        match self {
            Self::SignedIn(identity) => Some(identity.username.as_str()),
            Self::SignedOut | Self::Offline => None,
        }
    }

    /// Whether this client may join or host a multiplayer session.
    ///
    /// The single place that decides it, so the menu's greyed-out button and the
    /// connect path cannot disagree.
    pub fn can_play_multiplayer(&self) -> bool {
        matches!(self, Self::SignedIn(_))
    }
}

/// Shared handle on the current account.
#[derive(Clone)]
pub struct AccountState {
    inner: Arc<RwLock<Inner>>,
}

struct Inner {
    status: AccountStatus,
    session: Option<AuthSession>,
}

impl AccountState {
    /// Signed out.
    pub fn new() -> Self {
        Self {
            inner: Arc::new(RwLock::new(Inner {
                status: AccountStatus::SignedOut,
                session: None,
            })),
        }
    }

    /// Already signed in — restoring a persisted session at boot.
    pub fn signed_in(session: AuthSession) -> Self {
        let state = Self::new();
        state.sign_in(session);
        state
    }

    /// Playing without an account.
    pub fn offline() -> Self {
        let state = Self::new();
        state.set_offline();
        state
    }

    pub fn status(&self) -> AccountStatus {
        self.inner.read().status.clone()
    }

    pub fn identity(&self) -> Option<AccountIdentity> {
        match self.inner.read().status {
            AccountStatus::SignedIn(ref identity) => Some(identity.clone()),
            _ => None,
        }
    }

    pub fn username(&self) -> Option<String> {
        self.identity().map(|identity| identity.username)
    }

    pub fn can_play_multiplayer(&self) -> bool {
        self.inner.read().status.can_play_multiplayer()
    }

    /// A snapshot of the current session, if signed in.
    pub fn session(&self) -> Option<AuthSession> {
        self.inner.read().session.clone()
    }

    pub fn sign_in(&self, session: AuthSession) {
        let mut inner = self.inner.write();
        inner.status = AccountStatus::SignedIn(session.identity.clone());
        inner.session = Some(session);
    }

    pub fn sign_out(&self) {
        let mut inner = self.inner.write();
        inner.status = AccountStatus::SignedOut;
        inner.session = None;
    }

    pub fn set_offline(&self) {
        let mut inner = self.inner.write();
        inner.status = AccountStatus::Offline;
        inner.session = None;
    }

    /// The `u64` this client presents in the netcode handshake.
    ///
    /// Derived from the account, so a world save follows the player rather than
    /// the machine. `None` while offline — what to fall back to then is the
    /// caller's policy, not this crate's, and an offline player never reaches a
    /// multiplayer session anyway.
    pub fn netcode_id(&self) -> Option<u64> {
        self.identity().map(|identity| identity.netcode_id())
    }

    /// Get a join ticket, refreshing the access token first if it is close to
    /// expiring.
    ///
    /// Blocking — call it from a worker thread. `ConnectingState` does.
    pub fn issue_ticket(
        &self,
        client: &dyn AuthClient,
        now_unix: u64,
    ) -> Result<JoinTicket, AuthError> {
        let Some(session) = self.session() else {
            return Err(AuthError::Refused {
                code: "not_signed_in".to_string(),
                message: "you are not signed in".to_string(),
            });
        };

        let session = if session.access_token_usable(now_unix) {
            session
        } else {
            // The refresh token is single-use, so the rotated pair must be
            // stored even if the ticket request then fails — otherwise the next
            // attempt spends a token the server has already retired, which it
            // correctly reads as theft and revokes the whole family.
            let refreshed = client.refresh(&session.refresh_token)?;
            self.sign_in(refreshed.clone());
            refreshed
        };

        client.issue_ticket(&session.access_token)
    }
}

impl Default for AccountState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::client::FakeAuthClient;

    fn session(username: &str) -> AuthSession {
        AuthSession {
            identity: AccountIdentity {
                account_id: uuid::Uuid::from_u128(9),
                username: username.to_string(),
            },
            access_token: format!("access-for-{username}"),
            access_expires_at: u64::MAX / 2,
            refresh_token: format!("refresh-for-{username}"),
        }
    }

    #[test]
    fn a_fresh_state_is_signed_out_and_cannot_play_multiplayer() {
        let state = AccountState::new();

        assert_eq!(state.status(), AccountStatus::SignedOut);
        assert!(!state.can_play_multiplayer());
        assert_eq!(state.username(), None);
    }

    #[test]
    fn signing_in_reports_the_account() {
        let state = AccountState::new();
        state.sign_in(session("gustav"));

        assert!(state.can_play_multiplayer());
        assert_eq!(state.username().as_deref(), Some("gustav"));
    }

    /// The requirement the whole login gate exists for.
    #[test]
    fn an_offline_player_cannot_play_multiplayer() {
        let state = AccountState::offline();

        assert_eq!(state.status(), AccountStatus::Offline);
        assert!(!state.can_play_multiplayer());
    }

    #[test]
    fn signing_out_clears_the_session() {
        let state = AccountState::signed_in(session("gustav"));
        state.sign_out();

        assert_eq!(state.status(), AccountStatus::SignedOut);
        assert!(state.session().is_none());
        assert!(!state.can_play_multiplayer());
    }

    #[test]
    fn a_signed_in_client_derives_its_netcode_id_from_the_account() {
        let state = AccountState::signed_in(session("gustav"));
        let expected = state.identity().unwrap().netcode_id();

        assert_eq!(state.netcode_id(), Some(expected));
    }

    #[test]
    fn issues_a_ticket_for_a_signed_in_account() {
        let client = FakeAuthClient::new().with_account("gustav", "hunter2hunter2");
        let signed_in = client.login("gustav", "hunter2hunter2").unwrap();
        let state = AccountState::signed_in(signed_in);

        let ticket = state.issue_ticket(&client, 1_800_000_000).unwrap();

        let mut verifier = crate::TicketVerifier::new(client.key_set());
        assert_eq!(
            verifier
                .verify(Some(&ticket.slot), 1_800_000_000)
                .unwrap()
                .username,
            "gustav"
        );
    }

    #[test]
    fn refuses_to_issue_a_ticket_when_signed_out() {
        let client = FakeAuthClient::new();
        let state = AccountState::new();

        assert!(matches!(
            state.issue_ticket(&client, 1_800_000_000),
            Err(AuthError::Refused { .. })
        ));
    }

    /// A rotated refresh token must be kept even when the *next* call fails, or
    /// the following attempt replays a spent token and the auth server revokes
    /// the whole family.
    #[test]
    fn a_stale_access_token_is_refreshed_and_the_rotation_is_kept() {
        let client = FakeAuthClient::new().with_account("gustav", "hunter2hunter2");
        let mut stale = client.login("gustav", "hunter2hunter2").unwrap();
        stale.access_expires_at = 0;
        stale.access_token = "expired-and-useless".to_string();

        let state = AccountState::signed_in(stale);
        assert!(state.issue_ticket(&client, 1_800_000_000).is_ok());

        let stored = state.session().expect("still signed in");
        assert_eq!(
            stored.access_token, "access-for-gustav",
            "the refreshed token should have been stored"
        );
    }

    #[test]
    fn an_unreachable_server_surfaces_as_offline_rather_than_a_refusal() {
        let client = FakeAuthClient::new().with_account("gustav", "pw").offline();
        let state = AccountState::signed_in(session("gustav"));

        let err = state.issue_ticket(&client, u64::MAX / 2).unwrap_err();
        assert!(err.is_offline());
    }
}

//! Accounts: proving who this player is, and checking who a joining player is.
//!
//! Two halves that barely touch:
//!
//! * **As a client** — [`AuthClient`] talks to the auth server to log in, keep a
//!   session alive, and fetch a join ticket. [`AuthSession`] is what a
//!   logged-in player carries around; `save::profile` persists it.
//! * **As a host** — [`TicketVerifier`] checks the ticket a joining player
//!   presents, using a public key set cached on disk. No network call, so a host
//!   admits legitimate players even when the auth server is unreachable.
//!
//! The two never meet: a host verifying a guest does not need to be logged in
//! itself, and a client logging in does not need any host.
//!
//! Depends only on `crate::core` (plus `ureq` for HTTP), so it sits at the same
//! level as `net` in the module graph and can be tested with no window, GPU or
//! socket.

pub mod account;
pub mod client;
pub mod keys;
pub mod session;
pub mod verifier;

pub use account::{AccountState, AccountStatus};
pub use client::{AuthClient, AuthError, FakeAuthClient, HttpAuthClient, JoinTicket};
pub use keys::KeyCache;
pub use session::{AccountIdentity, AuthSession};
pub use verifier::{TicketVerifier, VerifyFailure};

/// Where the game looks for the auth server when nothing overrides it.
///
/// `WYVEN_AUTH_URL` takes precedence, which is what lets a developer point at a
/// local `docker compose up` without editing anything.
pub const DEFAULT_AUTH_URL: &str = "http://llzdmervhd2eyewlrapa8jhi.100.94.237.98.sslip.io";

/// The auth server this build should talk to.
pub fn auth_url() -> String {
    std::env::var("WYVEN_AUTH_URL")
        .ok()
        .map(|value| value.trim_end_matches('/').to_string())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| DEFAULT_AUTH_URL.to_string())
}

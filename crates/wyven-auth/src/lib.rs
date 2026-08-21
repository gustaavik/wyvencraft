//! Accounts: proving who this player is, and checking who a joining player is.
//!
//! Two halves that barely touch:
//!
//! * **As a client** — [`AuthClient`] talks to the auth server to log in, keep a
//!   session alive, and fetch a join ticket. [`AuthSession`] is what a
//!   logged-in player carries around; `save::profile` persists it. [`username`]
//!   knows what the server will accept as a name, so a login form can refuse a
//!   bad one without asking.
//! * **As a host** — [`TicketVerifier`] checks the ticket a joining player
//!   presents, using a public key set cached on disk. No network call, so a host
//!   admits legitimate players even when the auth server is unreachable.
//!
//! The two never meet: a host verifying a guest does not need to be logged in
//! itself, and a client logging in does not need any host.
//!
//! Depends on nothing but `wcauth-ticket` and `ureq`, so it can be tested with
//! no window, GPU or socket — and, more usefully, so it is the *only* crate that
//! names the private ticket repository. Every other engine crate builds with no
//! GitHub credential at all.

pub mod account;
pub mod client;
pub mod keys;
pub mod session;
pub mod username;
pub mod verifier;

pub use account::{AccountState, AccountStatus};
pub use client::{AuthClient, AuthError, FakeAuthClient, HttpAuthClient, JoinTicket};
pub use keys::KeyCache;
pub use session::{AccountIdentity, AuthSession};
pub use username::UsernameError;
pub use verifier::{TicketVerifier, VerifyFailure};

/// Where the game looks for the auth server when nothing overrides it.
///
/// Baked in at build time from `WYVEN_AUTH_URL` — that is how a release build
/// ships pointing at production while the source keeps a local default. At run
/// time the same variable takes precedence again, which is what lets a shipped
/// binary be pointed at a local `docker compose up` without a rebuild.
pub const DEFAULT_AUTH_URL: &str = match option_env!("WYVEN_AUTH_URL") {
    Some(url) => url,
    None => "http://127.0.0.1:8080",
};

/// The auth server this build should talk to.
pub fn auth_url() -> String {
    let url = std::env::var("WYVEN_AUTH_URL")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| DEFAULT_AUTH_URL.to_string());
    normalize(&url)
}

/// Strip a trailing `/` so callers can always concatenate a path onto this.
fn normalize(url: &str) -> String {
    url.trim_end_matches('/').to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The baked-in default is what an unconfigured build talks to, so an empty
    /// one would mean silently unreachable auth rather than a build failure.
    #[test]
    fn default_url_is_not_empty() {
        assert!(!DEFAULT_AUTH_URL.trim().is_empty());
    }

    /// Callers build request paths as `format!("{base}/v1/…")`, so a trailing
    /// slash from either source would produce a double slash.
    #[test]
    fn normalize_strips_trailing_slashes() {
        assert_eq!(normalize("http://example.test/"), "http://example.test");
        assert_eq!(normalize("http://example.test///"), "http://example.test");
    }

    #[test]
    fn normalize_leaves_a_bare_url_alone() {
        assert_eq!(normalize("http://example.test"), "http://example.test");
        assert_eq!(
            normalize("http://example.test/base"),
            "http://example.test/base"
        );
    }
}

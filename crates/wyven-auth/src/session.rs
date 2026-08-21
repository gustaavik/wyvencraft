//! What a signed-in player carries.

use std::fmt;

/// The identity a host learns about a connected player, from their verified
/// ticket.
///
/// This replaces the invented `format!("Player {}", pid.0)` that used to stand
/// in for a name, and — more importantly — replaces the self-asserted `u64` that
/// `ops.toml` used to trust. A [`AccountIdentity`] can only be constructed by
/// verifying a signature, so a value of this type *is* proof.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccountIdentity {
    /// Stable account id. What saves and `ops.toml` are keyed on.
    pub account_id: uuid::Uuid,
    /// The name shown in chat and above the player's head.
    pub username: String,
}

impl AccountIdentity {
    /// The `u64` this account presents in the netcode handshake.
    ///
    /// Must agree exactly with the auth server's `AccountId::to_netcode_id`, and
    /// with the `netcode_id` it reports over HTTP — a mismatch would mean a
    /// player's saved inventory stopped being found. There is a test pinning the
    /// formula against a known account.
    pub fn netcode_id(&self) -> u64 {
        let (high, low) = self.account_id.as_u64_pair();
        (high ^ low).max(1)
    }
}

impl fmt::Display for AccountIdentity {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{} ({})", self.username, self.account_id)
    }
}

/// A signed-in session, as persisted in `profile.toml`.
#[derive(Debug, Clone)]
pub struct AuthSession {
    pub identity: AccountIdentity,
    /// Short-lived bearer token for the auth server's own API.
    pub access_token: String,
    /// Unix seconds. Past this, `access_token` is refused and the refresh token
    /// must be spent.
    pub access_expires_at: u64,
    /// Long-lived, single-use. Rotated on every refresh.
    pub refresh_token: String,
}

impl AuthSession {
    /// Whether the access token is worth trying.
    ///
    /// Deliberately pessimistic by [`REFRESH_MARGIN_SECS`]: a token that expires
    /// in two seconds will have expired by the time the request lands, and a
    /// player watching a loading screen should not pay for that round trip.
    pub fn access_token_usable(&self, now_unix: u64) -> bool {
        now_unix + REFRESH_MARGIN_SECS < self.access_expires_at
    }
}

/// How far ahead of real expiry an access token is treated as spent.
pub const REFRESH_MARGIN_SECS: u64 = 60;

#[cfg(test)]
mod tests {
    use super::*;

    fn session(expires_at: u64) -> AuthSession {
        AuthSession {
            identity: AccountIdentity {
                account_id: uuid::Uuid::from_u128(1),
                username: "gustav".to_string(),
            },
            access_token: "token".to_string(),
            access_expires_at: expires_at,
            refresh_token: "refresh".to_string(),
        }
    }

    #[test]
    fn an_access_token_well_inside_its_life_is_usable() {
        assert!(session(1_000).access_token_usable(500));
    }

    #[test]
    fn an_access_token_close_to_expiry_is_refreshed_early() {
        // Technically still valid, but not for long enough to be worth using.
        assert!(!session(1_000).access_token_usable(1_000 - REFRESH_MARGIN_SECS));
        assert!(!session(1_000).access_token_usable(999));
    }

    #[test]
    fn an_expired_access_token_is_not_usable() {
        assert!(!session(1_000).access_token_usable(1_001));
    }

    /// Pinned against the auth server's own derivation. If these ever disagree,
    /// returning players silently lose their saved inventory and position.
    #[test]
    fn netcode_id_matches_the_auth_servers_derivation() {
        let identity = AccountIdentity {
            account_id: uuid::Uuid::from_u64_pair(0x0123_4567_89AB_CDEF, 0xFEDC_BA98_7654_3210),
            username: "gustav".to_string(),
        };
        assert_eq!(
            identity.netcode_id(),
            0x0123_4567_89AB_CDEF ^ 0xFEDC_BA98_7654_3210
        );
    }

    /// `PlayerId(0)` is the host's own local player, so no account may derive it.
    #[test]
    fn netcode_id_never_collides_with_the_hosts_reserved_zero() {
        let nil = AccountIdentity {
            account_id: uuid::Uuid::nil(),
            username: "x".to_string(),
        };
        assert_eq!(nil.netcode_id(), 1);

        let mirrored = AccountIdentity {
            account_id: uuid::Uuid::from_u64_pair(7, 7),
            username: "x".to_string(),
        };
        assert_eq!(mirrored.netcode_id(), 1);
    }
}

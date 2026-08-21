//! Who may join a Wyvencraft host, and what its peers say to each other.
//!
//! Two small declarations that turn the generic transport in [`wyven_net`] into
//! this game's multiplayer.

use wyven_auth::{AccountIdentity, KeyCache, TicketVerifier, keys};
use wyven_net::{HostConfig, JoinVerifier, Protocol, UserData};

use crate::net::protocol::{ClientMessage, ServerMessage};

/// Application/protocol id — clients must match to connect.
pub const PROTOCOL_ID: u64 = 0x5759_564E_0001; // "WYVN" v1

/// How many peers a host admits.
const MAX_CLIENTS: usize = 16;

/// Wyvencraft's message pair.
pub struct WyvenProtocol;

impl Protocol for WyvenProtocol {
    type ToClient = ServerMessage;
    type ToServer = ClientMessage;
}

pub fn host_config() -> HostConfig {
    HostConfig {
        protocol_id: PROTOCOL_ID,
        max_clients: MAX_CLIENTS,
    }
}

/// The join gate: an Ed25519 ticket, checked offline against the public keys
/// cached in `authkeys.toml`.
///
/// Offline on purpose — a host admits legitimate players even when the auth
/// server is unreachable. The corollary is that a host which has *never* reached
/// it has no keys and turns everyone away. That is the safe direction, and the
/// one that keeps "I could not check you" from silently meaning "you're in".
pub struct TicketJoin {
    /// `None` when this host has no cached keys, and so cannot verify anyone.
    verifier: Option<TicketVerifier>,
}

impl TicketJoin {
    /// Load the cached keys. Call once, at bind.
    pub fn from_cache() -> Self {
        let cached = KeyCache::new().load();
        if cached.is_empty() {
            log::warn!(
                "no {} — this host cannot verify players and will refuse every join. \
                 Sign in once to fetch the auth keys.",
                keys::KEYS_FILE
            );
            return Self { verifier: None };
        }
        log::info!("verifying joins against {} auth key(s)", cached.len());
        Self {
            verifier: Some(TicketVerifier::new(cached)),
        }
    }
}

impl JoinVerifier for TicketJoin {
    type Identity = AccountIdentity;

    fn verify(
        &mut self,
        user_data: Option<&UserData>,
        client_id: u64,
        now_unix: u64,
    ) -> Result<AccountIdentity, String> {
        let Some(verifier) = self.verifier.as_mut() else {
            return Err("this host has no auth keys and cannot verify anyone".to_string());
        };
        let identity = verifier
            .verify(user_data, now_unix)
            .map_err(|err| err.to_string())?;

        // The netcode id is derived from the account, so a client claiming one
        // id while holding a ticket for another is trying something. Refusing
        // keeps `identity -> save record` a function rather than a suggestion.
        if identity.netcode_id() != client_id {
            return Err(format!(
                "ticket is for {} but the client connected as {client_id}",
                identity.netcode_id()
            ));
        }
        Ok(identity)
    }

    fn is_ready(&self) -> bool {
        self.verifier.is_some()
    }
}

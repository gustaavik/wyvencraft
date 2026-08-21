//! The two things a transport has to be told before it can carry a game.
//!
//! `renet` moves bytes; deciding *which* bytes and *who* may send them is the
//! game's business. [`Protocol`] names the two message types, [`JoinVerifier`]
//! decides who gets in, and between them this crate needs to know nothing about
//! blocks, inventories or accounts.

use serde::Serialize;
use serde::de::DeserializeOwned;

/// The pair of message types a host and its clients exchange.
///
/// One trait rather than two type parameters, so a `Host` and a `Client` are
/// spelled `Host<MyProtocol>` and cannot be wired to half a protocol each.
pub trait Protocol {
    /// Server → client.
    type ToClient: Serialize + DeserializeOwned;
    /// Client → server.
    type ToServer: Serialize + DeserializeOwned;
}

/// Bytes a connecting peer put in netcode's `user_data` — the only thing it can
/// say before the host decides whether to keep it.
pub type UserData = [u8; renet_netcode::NETCODE_USER_DATA_BYTES];

/// Decides whether a connecting peer may join, and who it is.
///
/// Called *before* a [`PlayerId`](crate::PlayerId) is assigned, so a rejected
/// peer never reaches the game layer: no welcome, no world state, no entry to
/// clean up. Refusing is the safe direction — a verifier that cannot check
/// should refuse, never admit.
pub trait JoinVerifier {
    /// What a verified peer turns out to be. The host hands this back with
    /// every join so the game can attach permissions or a display name to it.
    ///
    /// `Display` because the host logs who joined, and "player 3 joined" is a
    /// materially worse operator experience than naming the account — which is
    /// the whole point of having verified it.
    type Identity: std::fmt::Display;

    /// Check `user_data` from a peer that connected as `client_id`.
    ///
    /// `now_unix` is passed rather than read so a verifier stays testable and
    /// deterministic. The `Err` string is logged, not sent — a refused peer
    /// learns only that it was refused.
    fn verify(
        &mut self,
        user_data: Option<&UserData>,
        client_id: u64,
        now_unix: u64,
    ) -> Result<Self::Identity, String>;

    /// Whether this verifier can check anyone at all. A host that answers
    /// `false` will refuse every join, and should say so at bind time.
    fn is_ready(&self) -> bool {
        true
    }
}

/// Admits every peer with no identity at all.
///
/// For a transport that is not the security boundary — a LAN session, a test.
/// Anything facing a network wants a real verifier.
pub struct OpenJoin;

impl JoinVerifier for OpenJoin {
    type Identity = Anonymous;

    fn verify(&mut self, _: Option<&UserData>, _: u64, _: u64) -> Result<Anonymous, String> {
        Ok(Anonymous)
    }
}

/// The identity of a peer nobody checked.
pub struct Anonymous;

impl std::fmt::Display for Anonymous {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("an unverified peer")
    }
}

//! The in-game session's networking role, behind one trait.
//!
//! This used to be a `NetRole` enum that the state layer matched on in nine
//! places — `matches!(self.net, NetRole::Client { .. })` scattered through
//! mob simulation, fluid ticking, persistence and the frame loop. Adding a role
//! meant finding every site, and none of the host/client logic could be tested
//! without a real UDP socket.
//!
//! [`Session`] splits that into two questions the state layer actually asks:
//!
//! - **Who decides?** [`Session::authority`] — the authority simulates mobs,
//!   fluids and damage; everyone else renders replicas. Singleplayer and host
//!   are both [`Authority::Local`], which is why so many call sites only ever
//!   needed "am I the client?".
//! - **What arrived, and what do I send?** [`Session::poll`] drains the
//!   transport into [`Inbound`] values; the state layer applies them and replies
//!   through [`Session::broadcast`] / [`send_to`](Session::send_to) /
//!   [`request`](Session::request).
//!
//! Transport lives in the implementations; *interpreting* a message stays in
//! the state layer, which is the only thing that owns the world, the player and
//! the mob list. [`FakeSession`] makes that half testable offline.

mod client;
mod fake;
mod host;
mod singleplayer;

use std::time::Duration;

use crate::net::{Channel, ClientMessage, PlayerId, ServerMessage};

pub use client::ClientSession;
pub use fake::{FakeHandle, FakeSession, FakeState, Sent};
pub use host::{HOST_PLAYER_ID, HostSession};
pub use singleplayer::SingleplayerSession;

/// Who owns the simulation for this session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Authority {
    /// This peer decides: it ticks mobs and fluids and applies damage.
    /// Singleplayer and the host both sit here.
    Local,
    /// The host decides; this peer renders what it is told.
    Remote,
}

/// One thing that arrived from the network this frame.
pub enum Inbound {
    /// Host: a client finished connecting. `identity` is their stable id, used
    /// to hand back a returning player's saved record.
    ///
    /// `account` is the verified account behind them — present for anyone who
    /// arrived over a real network, because the host refuses joins it cannot
    /// verify. It is `None` only where there is nobody to verify: the
    /// singleplayer session, and `FakeSession` in tests.
    Joined {
        player: PlayerId,
        identity: u64,
        account: Option<crate::auth::AccountIdentity>,
    },
    /// Host: a player disconnected.
    Left { player: PlayerId },
    /// Host: a request from a client, to be validated before it takes effect.
    Request {
        player: PlayerId,
        msg: ClientMessage,
    },
    /// Client: an authoritative update from the host.
    Update(ServerMessage),
}

/// The networking role of one in-game session.
///
/// Every method is meaningful for every role; the ones that don't apply are
/// no-ops rather than errors (a client never broadcasts, a host never sends a
/// request), which is what lets the state layer call them unconditionally.
pub trait Session {
    fn authority(&self) -> Authority;

    /// This peer's own player id. The host and singleplayer are always player 0;
    /// a client uses the id the host assigned it in the `Welcome`.
    fn local_id(&self) -> PlayerId;

    /// Drive the transport and drain everything that arrived.
    fn poll(&mut self, dt: Duration) -> Vec<Inbound>;

    /// Host: send to every connected client. Otherwise a no-op.
    fn broadcast(&mut self, msg: &ServerMessage, channel: Channel);

    /// Host: send to one player. Otherwise a no-op.
    fn send_to(&mut self, player: PlayerId, msg: &ServerMessage, channel: Channel);

    /// Client: ask the host to do something. Otherwise a no-op.
    fn request(&mut self, msg: &ClientMessage, channel: Channel);

    /// Push queued messages onto the wire. Called once per frame, last.
    fn flush(&mut self);

    /// Whether the transport is up. Always true off the network.
    fn is_connected(&self) -> bool;

    /// Debug-HUD description. `remote_players` is the state layer's count,
    /// which the session itself doesn't track.
    fn status(&self, remote_players: usize) -> String;

    /// Convenience: this peer owns the simulation.
    fn is_authority(&self) -> bool {
        self.authority() == Authority::Local
    }

    /// Whether this session publishes authoritative events to other peers —
    /// true only for a host. Singleplayer has no listeners and a client has
    /// nothing authoritative to say, so both skip building those messages.
    fn serves_peers(&self) -> bool {
        false
    }
}

//! Asking a server what it is, before deciding to join it.
//!
//! The server browser shows three things per row — the world's name, who is on
//! it, and how far away it feels. None of them can be known without asking, and
//! the game has exactly one way to ask: its own connection. So a probe *is* a
//! client. It presents a real join ticket, sends
//! [`ClientMessage::RequestStatus`](crate::net::ClientMessage::RequestStatus),
//! reads the reply and leaves.
//!
//! The host answers that one message without announcing the peer or recording
//! it (see `state::ingame_state::net`), which is what keeps a Refresh from
//! showing up as a join and a leave to everyone already playing.
//!
//! A port, because the browser's logic must be testable without a socket, an
//! auth server or a host to talk to.

mod fake;
mod probe;

use std::time::Duration;

pub use fake::FakeStatusProbe;
pub use probe::NetStatusProbe;

/// What a server said about itself.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerStatus {
    /// The hosted world's name — the host's, not the one the player filed it
    /// under.
    pub name: String,
    /// Players in the world, the host included.
    pub online: u32,
    /// The most it holds.
    pub max: u32,
    /// Application-level round trip: our question to its answer.
    pub ping_ms: u32,
    /// The host's content fingerprint. Compared, not judged, here — deciding
    /// what a mismatch *means* is the browser's business.
    pub content_hash: u64,
}

/// How one query turned out.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StatusOutcome {
    Online(Box<ServerStatus>),
    /// Unreachable, refused, or too old to answer — with something to show the
    /// player instead of a number.
    Offline(String),
}

/// Asks servers what they are, without the caller learning how.
pub trait StatusProbe {
    /// Start querying `targets` (addresses as the player typed them),
    /// abandoning anything already in flight.
    fn begin(&mut self, targets: Vec<String>);

    /// Advance every query by `dt` and take whatever resolved, keyed by the
    /// target string it was asked about.
    fn poll(&mut self, dt: Duration) -> Vec<(String, StatusOutcome)>;

    /// Whether anything is still outstanding — what the Refresh spinner reads.
    fn is_busy(&self) -> bool;
}

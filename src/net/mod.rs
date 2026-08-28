//! Wyvencraft's multiplayer: what peers say to each other, and who is allowed
//! to say it.
//!
//! The transport is [`wyven_net`] — sockets, channels, connection events, the
//! join gate. It carries bytes and knows nothing about blocks or inventories.
//! Two declarations wire this game into it:
//!
//! * [`WyvenProtocol`] names the message pair, so `Host` and `Client` below are
//!   plain aliases rather than types anyone has to spell.
//! * [`TicketJoin`] is the join gate: an Ed25519 ticket checked against
//!   `authkeys.toml`, refusing anyone it cannot verify. **No keys means no
//!   joins**, never "everyone joins".
//!
//! The wire format itself lives in [`protocol`]; remote-player smoothing in
//! [`sync`]. The player's saved servers and how they are asked what they are
//! live in [`serverlist`] and [`status`].

pub mod address;
pub mod join;
pub mod protocol;
pub mod serverlist;
pub mod status;
pub mod sync;
pub mod ticket;

pub use join::{MAX_CLIENTS, PROTOCOL_ID, TicketJoin, WyvenProtocol, host_config};
pub use protocol::{
    ChatKind, ClientMessage, NetItemStack, NetVec3, PlayerRestore, RecipeData, ServerMessage,
};
pub use serverlist::{FileServerStore, ServerEntry, ServerList, ServerStore};
pub use status::{NetStatusProbe, ServerStatus, StatusOutcome, StatusProbe};
pub use sync::RemotePlayer;
pub use wyven_net::{Channel, DEFAULT_PORT, PlayerId};

/// The host driver, speaking Wyvencraft's protocol behind its join gate.
pub type Host = wyven_net::Host<WyvenProtocol, TicketJoin>;
/// The client driver, speaking the same protocol.
pub type Client = wyven_net::Client<WyvenProtocol>;

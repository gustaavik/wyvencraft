//! Peer-to-peer (host-authoritative) networking.
//!
//! One peer runs a [`server::GameServer`]; others connect with a
//! [`client::GameClient`]. Singleplayer runs the server in-process. The wire
//! format lives in [`protocol`]; remote-player smoothing in [`sync`].

pub mod client;
pub mod protocol;
pub mod server;
pub mod sync;

pub use client::Client;
pub use protocol::{
    Channel, ChatKind, ClientMessage, NetItemStack, NetVec3, PlayerId, PlayerRestore, RecipeData,
    ServerMessage,
};
pub use server::{DEFAULT_PORT, Host};
pub use sync::RemotePlayer;

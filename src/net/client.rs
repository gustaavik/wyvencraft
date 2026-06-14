//! Client connection to a host: sends local input/edits, receives authoritative
//! world and player updates.
//!
//! Implemented in milestone M7 on top of `renet::RenetClient` +
//! `renet_netcode::NetcodeClientTransport`.

use std::collections::HashMap;

use crate::net::protocol::PlayerId;
use crate::net::sync::RemotePlayer;

/// Client-side networking driver and the remote players it tracks. Fleshed out
/// in M7.
#[derive(Default)]
pub struct GameClient {
    pub remote_players: HashMap<PlayerId, RemotePlayer>,
    pub local_id: Option<PlayerId>,
}

impl GameClient {
    pub fn new() -> Self {
        Self::default()
    }
}

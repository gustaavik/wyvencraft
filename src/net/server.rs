//! Authoritative host: owns world truth, validates client requests, and
//! broadcasts state to peers.
//!
//! Runs both for dedicated "host" sessions and, in-process, for singleplayer
//! (the local client talks to this same server over a loopback transport) so
//! there is a single code path for game logic.
//!
//! Implemented in milestone M7 on top of `renet::RenetServer` +
//! `renet_netcode::NetcodeServerTransport`.

use crate::net::protocol::PlayerId;

/// Default UDP port the host listens on.
pub const DEFAULT_PORT: u16 = 25_565;

/// Per-connection state the host tracks for each player.
pub struct ConnectedPlayer {
    pub id: PlayerId,
    pub name: String,
}

/// The host-side networking driver. Fleshed out in M7.
pub struct GameServer {
    next_id: u64,
    players: Vec<ConnectedPlayer>,
}

impl GameServer {
    pub fn new() -> Self {
        Self {
            next_id: 0,
            players: Vec::new(),
        }
    }

    /// Allocate the next unique player id.
    pub fn allocate_id(&mut self) -> PlayerId {
        let id = PlayerId(self.next_id);
        self.next_id += 1;
        id
    }

    pub fn players(&self) -> &[ConnectedPlayer] {
        &self.players
    }
}

impl Default for GameServer {
    fn default() -> Self {
        Self::new()
    }
}

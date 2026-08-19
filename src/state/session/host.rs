//! Host session: authoritative, serving connected clients.

use std::time::Duration;

use super::{Authority, Inbound, Session};
use crate::net::{Channel, ClientMessage, Host, PlayerId, ServerMessage};

/// The host's own player always has this id; clients are numbered from 1.
pub const HOST_PLAYER_ID: PlayerId = PlayerId(0);

/// Wraps the [`Host`] driver, turning its per-frame event drain into
/// [`Inbound`] values.
pub struct HostSession {
    host: Box<Host>,
}

impl HostSession {
    pub fn new(host: Host) -> Self {
        Self {
            host: Box::new(host),
        }
    }

    pub fn seed(&self) -> u64 {
        self.host.seed()
    }
}

impl Session for HostSession {
    fn authority(&self) -> Authority {
        Authority::Local
    }

    fn local_id(&self) -> PlayerId {
        HOST_PLAYER_ID
    }

    fn poll(&mut self, dt: Duration) -> Vec<Inbound> {
        self.host.pump(dt);
        let mut inbound = Vec::new();

        // Joins first, so a request arriving in the same frame as the join is
        // applied to a player the state layer already knows about.
        for cid in self.host.take_joined() {
            if let Some(player) = self.host.player_id(cid) {
                // The netcode client id doubles as the player's stable identity:
                // returning players get their saved state back. It is derived
                // from the account, and the host checked that the ticket agrees
                // with it before ever reporting the join.
                inbound.push(Inbound::Joined {
                    player,
                    identity: cid,
                    account: self.host.account(player).cloned(),
                });
            }
        }
        for player in self.host.take_left() {
            inbound.push(Inbound::Left { player });
        }
        for (player, msg) in self.host.receive() {
            inbound.push(Inbound::Request { player, msg });
        }
        inbound
    }

    fn broadcast(&mut self, msg: &ServerMessage, channel: Channel) {
        self.host.broadcast(msg, channel);
    }

    fn send_to(&mut self, player: PlayerId, msg: &ServerMessage, channel: Channel) {
        self.host.send_to_player(player, msg, channel);
    }

    fn request(&mut self, _msg: &ClientMessage, _channel: Channel) {
        // The host *is* the authority; it has no one to ask.
    }

    fn flush(&mut self) {
        self.host.flush();
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn status(&self, _remote_players: usize) -> String {
        format!("host ({} players)", self.host.player_count())
    }

    fn serves_peers(&self) -> bool {
        true
    }
}

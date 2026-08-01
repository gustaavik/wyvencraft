//! Offline session: full authority, no transport.

use std::time::Duration;

use super::{Authority, Inbound, Session};
use crate::net::{Channel, ClientMessage, PlayerId, ServerMessage};

/// A session with no peers. Owns the simulation; every send is discarded.
pub struct SingleplayerSession;

impl Session for SingleplayerSession {
    fn authority(&self) -> Authority {
        Authority::Local
    }

    fn local_id(&self) -> PlayerId {
        super::HOST_PLAYER_ID
    }

    fn poll(&mut self, _dt: Duration) -> Vec<Inbound> {
        Vec::new()
    }

    fn broadcast(&mut self, _msg: &ServerMessage, _channel: Channel) {}

    fn send_to(&mut self, _player: PlayerId, _msg: &ServerMessage, _channel: Channel) {}

    fn request(&mut self, _msg: &ClientMessage, _channel: Channel) {}

    fn flush(&mut self) {}

    fn is_connected(&self) -> bool {
        true
    }

    fn status(&self, _remote_players: usize) -> String {
        "singleplayer".to_string()
    }
}

//! Client session: renders the host's authoritative world.

use std::time::Duration;

use super::{Authority, Inbound, Session};
use crate::net::{Channel, Client, ClientMessage, PlayerId, ServerMessage};

/// Wraps the [`Client`] driver. Everything it receives is authoritative.
pub struct ClientSession {
    client: Box<Client>,
    /// The player id the host assigned us in its `Welcome`.
    local_id: PlayerId,
}

impl ClientSession {
    pub fn new(client: Client, local_id: PlayerId) -> Self {
        Self {
            client: Box::new(client),
            local_id,
        }
    }
}

impl Session for ClientSession {
    fn authority(&self) -> Authority {
        Authority::Remote
    }

    fn local_id(&self) -> PlayerId {
        self.local_id
    }

    fn poll(&mut self, dt: Duration) -> Vec<Inbound> {
        if let Err(err) = self.client.pump(dt) {
            log::warn!("client pump error: {err}");
        }
        self.client
            .receive()
            .into_iter()
            .map(Inbound::Update)
            .collect()
    }

    fn broadcast(&mut self, _msg: &ServerMessage, _channel: Channel) {
        // Only the host is authoritative; a client has nothing to assert.
    }

    fn send_to(&mut self, _player: PlayerId, _msg: &ServerMessage, _channel: Channel) {}

    fn request(&mut self, msg: &ClientMessage, channel: Channel) {
        self.client.send(msg, channel);
    }

    fn flush(&mut self) {
        let _ = self.client.flush();
    }

    fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    fn status(&self, remote_players: usize) -> String {
        format!("client ({remote_players} remote)")
    }
}

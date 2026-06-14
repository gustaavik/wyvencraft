//! Client connection to a host: sends local input/edits, receives authoritative
//! world and player updates.
//!
//! Built on `renet::RenetClient` + `renet_netcode::NetcodeClientTransport`.

use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use renet::{ConnectionConfig, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};

use crate::net::protocol::{self, Channel, ClientMessage, ServerMessage};
use crate::net::server::PROTOCOL_ID;

/// Client-side networking driver.
pub struct Client {
    client: RenetClient,
    transport: NetcodeClientTransport,
}

impl Client {
    /// Open a connection to `server_addr` (does not block; poll [`Client::is_connected`]).
    pub fn connect(server_addr: SocketAddr) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let current_time = unix_now();
        let client_id = current_time.as_millis() as u64;
        let authentication = ClientAuthentication::Unsecure {
            protocol_id: PROTOCOL_ID,
            client_id,
            server_addr,
            user_data: None,
        };
        let transport = NetcodeClientTransport::new(current_time, authentication, socket)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        log::info!("connecting to {server_addr}");
        Ok(Self {
            client: RenetClient::new(ConnectionConfig::default()),
            transport,
        })
    }

    /// Receive packets. Returns `Err` if the transport disconnected.
    pub fn pump(&mut self, dt: Duration) -> Result<(), String> {
        self.client.update(dt);
        self.transport
            .update(dt, &mut self.client)
            .map_err(|e| e.to_string())
    }

    pub fn is_connected(&self) -> bool {
        self.client.is_connected()
    }

    pub fn is_connecting(&self) -> bool {
        self.client.is_connecting()
    }

    /// Drain all incoming server messages across channels.
    pub fn receive(&mut self) -> Vec<ServerMessage> {
        let mut out = Vec::new();
        for channel in [Channel::Unreliable, Channel::Reliable, Channel::Chunk] {
            while let Some(bytes) = self.client.receive_message(channel.id()) {
                if let Some(msg) = protocol::decode::<ServerMessage>(&bytes) {
                    out.push(msg);
                }
            }
        }
        out
    }

    pub fn send(&mut self, msg: &ClientMessage, channel: Channel) {
        if self.client.is_connected() {
            self.client
                .send_message(channel.id(), protocol::encode(msg));
        }
    }

    /// Flush queued messages. Call once per frame, last.
    pub fn flush(&mut self) -> Result<(), String> {
        self.transport
            .send_packets(&mut self.client)
            .map_err(|e| e.to_string())
    }
}

fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

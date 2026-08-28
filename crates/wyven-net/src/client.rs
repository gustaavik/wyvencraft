//! Client connection to a host: sends local input/edits, receives authoritative
//! world and player updates.
//!
//! Built on `renet::RenetClient` + `renet_netcode::NetcodeClientTransport`.

use std::marker::PhantomData;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use renet::{ConnectionConfig, RenetClient};
use renet_netcode::{ClientAuthentication, NetcodeClientTransport};

use crate::session::{Protocol, UserData};
use crate::wire::{self, Channel};

/// Client-side networking driver.
///
/// Generic over the protocol it speaks, fixed at construction, so it cannot be
/// pointed at a host expecting different messages without saying so.
pub struct Client<P: Protocol> {
    client: RenetClient,
    transport: NetcodeClientTransport,
    protocol: PhantomData<fn() -> P>,
}

impl<P: Protocol> Client<P> {
    /// Open a connection to `server_addr` (does not block; poll [`Client::is_connected`]).
    ///
    /// `client_id` is the netcode identity we present — derived from the signed-in
    /// account, so a host recognises a returning player across sessions and
    /// machines.
    ///
    /// `credentials` is whatever this game's [`JoinVerifier`] expects to see —
    /// a signed ticket, typically. It rides in netcode's `user_data`, the only
    /// thing a client can say before the host decides whether to keep it, so a
    /// host can reject an unauthenticated peer without ever having spoken to it
    /// at the application level. `None` connects, but a host that verifies will
    /// drop it immediately.
    ///
    /// [`JoinVerifier`]: crate::JoinVerifier
    pub fn connect(
        server_addr: SocketAddr,
        client_id: u64,
        protocol_id: u64,
        credentials: Option<UserData>,
    ) -> std::io::Result<Self> {
        let socket = UdpSocket::bind("0.0.0.0:0")?;
        let current_time = unix_now();
        let authentication = ClientAuthentication::Unsecure {
            protocol_id,
            client_id,
            server_addr,
            user_data: credentials,
        };
        let transport = NetcodeClientTransport::new(current_time, authentication, socket)
            .map_err(|e| std::io::Error::other(e.to_string()))?;
        log::info!("connecting to {server_addr}");
        Ok(Self {
            client: RenetClient::new(ConnectionConfig::default()),
            transport,
            protocol: PhantomData,
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
    pub fn receive(&mut self) -> Vec<P::ToClient> {
        let mut out = Vec::new();
        for channel in [Channel::Unreliable, Channel::Reliable, Channel::Chunk] {
            while let Some(bytes) = self.client.receive_message(channel.id()) {
                if let Some(msg) = wire::decode::<P::ToClient>(&bytes) {
                    out.push(msg);
                }
            }
        }
        out
    }

    pub fn send(&mut self, msg: &P::ToServer, channel: Channel) {
        if self.client.is_connected() {
            self.client.send_message(channel.id(), wire::encode(msg));
        }
    }

    /// Leave now, telling the host so rather than falling silent.
    ///
    /// Dropping a `Client` only closes its socket, which the host cannot tell
    /// apart from a crashed peer until the connection times out — and it holds
    /// a slot for the whole of that. Anything that connects deliberately
    /// briefly (a server-list status query) should say goodbye instead.
    pub fn disconnect(&mut self) {
        self.transport.disconnect();
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

//! Authoritative host: accepts peers, checks who they are, and carries messages
//! both ways. Runs in-process alongside the host's own player.
//!
//! Built on `renet::RenetServer` + `renet_netcode::NetcodeServerTransport` over
//! UDP. netcode's own "unsecure" auth is *not* the security boundary — the
//! [`JoinVerifier`] is, and it runs before a peer is assigned a
//! [`PlayerId`].

use std::collections::HashMap;
use std::marker::PhantomData;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use renet::{ClientId, ConnectionConfig, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};

use crate::session::{JoinVerifier, Protocol};
use crate::wire::{self, Channel, PlayerId};

/// Default UDP port the host listens on.
pub const DEFAULT_PORT: u16 = 25_565;

/// What a host needs settling before it binds.
#[derive(Debug, Clone, Copy)]
pub struct HostConfig {
    /// Application/protocol id — clients must present the same one. The game
    /// picks it; two games on one LAN must not share a value.
    pub protocol_id: u64,
    pub max_clients: usize,
}

/// The host-side networking driver.
///
/// Generic over the protocol it speaks and the verifier that guards it, both
/// fixed at construction: a `Host` cannot be half-wired to another game's
/// messages, and cannot be built without deciding who may join.
pub struct Host<P: Protocol, V: JoinVerifier> {
    server: RenetServer,
    transport: NetcodeServerTransport,
    seed: u64,
    next_player_id: u64,
    /// Map of transport client id -> assigned player id.
    players: HashMap<ClientId, PlayerId>,
    /// The verified identity behind each connected player.
    ///
    /// Populated only from a successful [`JoinVerifier::verify`], so an entry
    /// here *is* proof — which is what lets a game treat it as a permission
    /// rather than a suggestion, and give a nameplate a name nobody chose for
    /// themselves.
    accounts: HashMap<PlayerId, V::Identity>,
    verifier: V,
    /// Detected this frame (drained by the caller).
    joined: Vec<ClientId>,
    left: Vec<PlayerId>,
    protocol: PhantomData<fn() -> P>,
}

impl<P: Protocol, V: JoinVerifier> Host<P, V> {
    /// Bind a host on `port` serving a world with `seed`, guarded by `verifier`.
    pub fn bind(port: u16, seed: u64, config: HostConfig, verifier: V) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        let current_time = unix_now();
        let public_addr: SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .expect("valid loopback addr");

        let server_config = ServerConfig {
            current_time,
            max_clients: config.max_clients,
            protocol_id: config.protocol_id,
            public_addresses: vec![public_addr],
            authentication: ServerAuthentication::Unsecure,
        };
        let transport = NetcodeServerTransport::new(server_config, socket)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // Checked once, at bind, so a host that cannot admit anybody says so
        // now rather than at the first silent refusal.
        if !verifier.is_ready() {
            log::warn!("this host cannot verify players and will refuse every join");
        }

        log::info!("hosting on port {port} (seed {seed})");
        Ok(Self {
            server: RenetServer::new(ConnectionConfig::default()),
            transport,
            seed,
            next_player_id: 1, // 0 is reserved for the host's local player
            players: HashMap::new(),
            accounts: HashMap::new(),
            verifier,
            joined: Vec::new(),
            left: Vec::new(),
            protocol: PhantomData,
        })
    }

    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Receive packets and detect joins/leaves. Call once per frame before
    /// reading messages.
    pub fn pump(&mut self, dt: Duration) {
        self.server.update(dt);
        if let Err(err) = self.transport.update(dt, &mut self.server) {
            log::warn!("host transport error: {err}");
        }

        // Drain connection events. Events (not id polling) are required: the
        // netcode transport removes a disconnected client from the server's map
        // immediately, so `disconnections_id()` never reports it — the stale
        // `players` entry would then block the same client id from rejoining.
        while let Some(event) = self.server.get_event() {
            match event {
                ServerEvent::ClientConnected { client_id } => {
                    if self.players.contains_key(&client_id) {
                        continue;
                    }
                    // Checked *before* a PlayerId is assigned or a join is
                    // reported, so a peer that fails never reaches the game
                    // layer at all — there is no `Welcome`, no world state, no
                    // entry in the player list to clean up.
                    let identity = match self.verify_join(client_id) {
                        Ok(identity) => identity,
                        Err(reason) => {
                            log::warn!("refused client {client_id}: {reason}");
                            self.server.disconnect(client_id);
                            continue;
                        }
                    };

                    let pid = PlayerId(self.next_player_id);
                    self.next_player_id += 1;
                    log::info!("player {} joined", pid.0);
                    self.players.insert(client_id, pid);
                    self.accounts.insert(pid, identity);
                    self.joined.push(client_id);
                }
                ServerEvent::ClientDisconnected { client_id, reason } => {
                    if let Some(pid) = self.players.remove(&client_id) {
                        log::info!("player {} disconnected: {reason}", pid.0);
                        self.accounts.remove(&pid);
                        self.left.push(pid);
                    }
                }
            }
        }
    }

    /// Ask the verifier about a connecting peer.
    fn verify_join(&mut self, client_id: ClientId) -> Result<V::Identity, String> {
        let user_data = self.transport.user_data(client_id);
        self.verifier
            .verify(user_data.as_ref(), client_id, unix_now().as_secs())
    }

    /// The verified identity behind a player, if they are still connected.
    pub fn account(&self, pid: PlayerId) -> Option<&V::Identity> {
        self.accounts.get(&pid)
    }

    /// Whether this host is able to verify joining players at all.
    pub fn can_verify(&self) -> bool {
        self.verifier.is_ready()
    }

    /// Newly connected clients this frame (transport ids).
    pub fn take_joined(&mut self) -> Vec<ClientId> {
        std::mem::take(&mut self.joined)
    }

    /// Players that disconnected this frame.
    pub fn take_left(&mut self) -> Vec<PlayerId> {
        std::mem::take(&mut self.left)
    }

    pub fn player_id(&self, client: ClientId) -> Option<PlayerId> {
        self.players.get(&client).copied()
    }

    /// Drain all incoming client messages across channels.
    pub fn receive(&mut self) -> Vec<(PlayerId, P::ToServer)> {
        let mut out = Vec::new();
        let channels = [Channel::Unreliable, Channel::Reliable, Channel::Chunk];
        for client_id in self.server.clients_id() {
            let Some(pid) = self.players.get(&client_id).copied() else {
                continue;
            };
            for channel in channels {
                while let Some(bytes) = self.server.receive_message(client_id, channel.id()) {
                    if let Some(msg) = wire::decode::<P::ToServer>(&bytes) {
                        out.push((pid, msg));
                    }
                }
            }
        }
        out
    }

    pub fn send(&mut self, client: ClientId, msg: &P::ToClient, channel: Channel) {
        self.server
            .send_message(client, channel.id(), wire::encode(msg));
    }

    /// Send a message to the single client owning `pid` (no-op if they've left).
    pub fn send_to_player(&mut self, pid: PlayerId, msg: &P::ToClient, channel: Channel) {
        let mut target = None;
        for (&cid, &p) in &self.players {
            if p == pid {
                target = Some(cid);
                break;
            }
        }
        if let Some(cid) = target {
            self.server
                .send_message(cid, channel.id(), wire::encode(msg));
        }
    }

    pub fn broadcast(&mut self, msg: &P::ToClient, channel: Channel) {
        self.server
            .broadcast_message(channel.id(), wire::encode(msg));
    }

    /// Flush queued messages to the network. Call once per frame, last.
    pub fn flush(&mut self) {
        self.transport.send_packets(&mut self.server);
    }

    pub fn player_count(&self) -> usize {
        self.players.len() + 1 // + host
    }
}

fn unix_now() -> Duration {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
}

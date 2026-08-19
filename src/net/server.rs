//! Authoritative host: owns world truth, validates client requests, and
//! broadcasts state to peers. Runs in-process alongside the host's own player.
//!
//! Built on `renet::RenetServer` + `renet_netcode::NetcodeServerTransport` over
//! UDP, using unsecure auth (LAN / direct-connect).

use std::collections::HashMap;
use std::net::{SocketAddr, UdpSocket};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use renet::{ClientId, ConnectionConfig, RenetServer, ServerEvent};
use renet_netcode::{NetcodeServerTransport, ServerAuthentication, ServerConfig};

use crate::auth::{AccountIdentity, TicketVerifier};
use crate::net::protocol::{self, Channel, ClientMessage, PlayerId, ServerMessage};

/// Default UDP port the host listens on.
pub const DEFAULT_PORT: u16 = 25_565;
/// Application/protocol id — clients must match to connect.
pub const PROTOCOL_ID: u64 = 0x5759_564E_0001; // "WYVN" v1

/// The host-side networking driver.
pub struct Host {
    server: RenetServer,
    transport: NetcodeServerTransport,
    seed: u64,
    next_player_id: u64,
    /// Map of transport client id -> assigned player id.
    players: HashMap<ClientId, PlayerId>,
    /// The verified account behind each connected player.
    ///
    /// Populated only from a checked signature, so an entry here *is* proof of
    /// identity — which is what makes `ops.toml` a permission rather than a
    /// suggestion, and what gives a nameplate a name nobody chose for themselves.
    accounts: HashMap<PlayerId, AccountIdentity>,
    /// Checks join tickets. `None` when this host has no auth keys cached, which
    /// means it cannot verify anyone.
    verifier: Option<TicketVerifier>,
    /// Detected this frame (drained by the caller).
    joined: Vec<ClientId>,
    left: Vec<PlayerId>,
}

impl Host {
    /// Bind a host on `port` serving a world with `seed`.
    pub fn bind(port: u16, seed: u64) -> std::io::Result<Self> {
        let socket = UdpSocket::bind(("0.0.0.0", port))?;
        let current_time = unix_now();
        let public_addr: SocketAddr = format!("127.0.0.1:{port}")
            .parse()
            .expect("valid loopback addr");

        let server_config = ServerConfig {
            current_time,
            max_clients: 16,
            protocol_id: PROTOCOL_ID,
            public_addresses: vec![public_addr],
            authentication: ServerAuthentication::Unsecure,
        };
        let transport = NetcodeServerTransport::new(server_config, socket)
            .map_err(|e| std::io::Error::other(e.to_string()))?;

        // Loaded once, at bind. A host that has never reached the auth server
        // has no keys and turns everyone away — the safe direction, and the one
        // that keeps "I could not check you" from silently meaning "you're in".
        let keys = crate::auth::KeyCache::new().load();
        let verifier = if keys.is_empty() {
            log::warn!(
                "no {} — this host cannot verify players and will refuse every join. \
                 Sign in once to fetch the auth keys.",
                crate::auth::keys::KEYS_FILE
            );
            None
        } else {
            log::info!("verifying joins against {} auth key(s)", keys.len());
            Some(TicketVerifier::new(keys))
        };

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
                    log::info!("player {} joined as {identity}", pid.0);
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

    /// Check the ticket a connecting client presented.
    fn verify_join(&mut self, client_id: ClientId) -> Result<AccountIdentity, String> {
        let Some(verifier) = self.verifier.as_mut() else {
            return Err("this host has no auth keys and cannot verify anyone".to_string());
        };

        let user_data = self.transport.user_data(client_id);
        let identity = verifier
            .verify(user_data.as_ref(), unix_now().as_secs())
            .map_err(|err| err.to_string())?;

        // The netcode id is derived from the account, so a client claiming one
        // id while holding a ticket for another is trying something. Refusing
        // keeps `identity -> save record` a function rather than a suggestion.
        if identity.netcode_id() != client_id {
            return Err(format!(
                "ticket is for {} but the client connected as {client_id}",
                identity.netcode_id()
            ));
        }

        Ok(identity)
    }

    /// The verified account behind a player, if they are still connected.
    pub fn account(&self, pid: PlayerId) -> Option<&AccountIdentity> {
        self.accounts.get(&pid)
    }

    /// Whether this host is able to verify joining players at all.
    pub fn can_verify(&self) -> bool {
        self.verifier.is_some()
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
    pub fn receive(&mut self) -> Vec<(PlayerId, ClientMessage)> {
        let mut out = Vec::new();
        let channels = [Channel::Unreliable, Channel::Reliable, Channel::Chunk];
        for client_id in self.server.clients_id() {
            let Some(pid) = self.players.get(&client_id).copied() else {
                continue;
            };
            for channel in channels {
                while let Some(bytes) = self.server.receive_message(client_id, channel.id()) {
                    if let Some(msg) = protocol::decode::<ClientMessage>(&bytes) {
                        out.push((pid, msg));
                    }
                }
            }
        }
        out
    }

    pub fn send(&mut self, client: ClientId, msg: &ServerMessage, channel: Channel) {
        self.server
            .send_message(client, channel.id(), protocol::encode(msg));
    }

    /// Send a message to the single client owning `pid` (no-op if they've left).
    pub fn send_to_player(&mut self, pid: PlayerId, msg: &ServerMessage, channel: Channel) {
        let mut target = None;
        for (&cid, &p) in &self.players {
            if p == pid {
                target = Some(cid);
                break;
            }
        }
        if let Some(cid) = target {
            self.server
                .send_message(cid, channel.id(), protocol::encode(msg));
        }
    }

    pub fn broadcast(&mut self, msg: &ServerMessage, channel: Channel) {
        self.server
            .broadcast_message(channel.id(), protocol::encode(msg));
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

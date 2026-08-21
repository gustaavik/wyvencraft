//! Who else is in this session, and what we still owe them.
//!
//! The [`Session`](crate::state::session::Session) trait covers *transport* —
//! authority, send, receive. This is the bookkeeping that hangs off it: the
//! replicas of everyone else, the host's per-identity records of them, and the
//! throttles governing what a client reports and when.
//!
//! It exists because those eight fields were previously loose members of
//! `InGameState`, sitting between the world and the inventory with nothing
//! marking them as one concern.

use std::collections::HashMap;

use crate::inventory::{ARMOR_SIZE, Inventory};
use crate::net::{NetItemStack, PlayerId, RemotePlayer, ServerMessage};

/// Everything the session knows about the other peers.
#[derive(Default)]
pub(super) struct Peers {
    /// Replicas of every other player, interpolated between snapshots.
    pub players: HashMap<PlayerId, RemotePlayer>,
    /// Host: stable identity (netcode client id) of each connected player,
    /// used to match a returning player to their saved record.
    pub identities: HashMap<PlayerId, u64>,
    /// Host: the verified account behind each connected player.
    ///
    /// Only ever written from a checked ticket signature, which is what lets
    /// `ops.toml` key on it. Empty in singleplayer and in tests, where there is
    /// no remote peer to verify.
    pub accounts: HashMap<PlayerId, wyven_auth::AccountIdentity>,
    /// Host: latest inventory each client reported, kept in wire form and
    /// converted to the name-based disk form only when a record is written.
    pub inventories: HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    /// Host: last equipment broadcast for each player, so `PlayerEquipment` is
    /// only re-sent on change and a joiner can be brought up to date.
    pub equipment: HashMap<PlayerId, [Option<u16>; ARMOR_SIZE]>,

    /// Host: reliable mob events queued by this frame's simulation, drained
    /// into the broadcast by the network pump. A field rather than a return
    /// value so mob code never has to reach the session.
    pub mob_events: Vec<ServerMessage>,

    /// Throttle for outgoing survival stats.
    pub stats_timer: f32,
    /// Client: throttle + change detection for inventory reports to the host.
    pub inventory_sync_timer: f32,
    pub last_synced_inventory: Option<Inventory>,
    /// Client: whether we've asked the host for the initial world state yet.
    pub world_state_requested: bool,
}

impl Peers {
    /// Drop every trace of a player who left.
    pub fn remove(&mut self, pid: PlayerId) {
        self.players.remove(&pid);
        self.identities.remove(&pid);
        self.accounts.remove(&pid);
        self.inventories.remove(&pid);
        self.equipment.remove(&pid);
    }

    /// The replica for `id`, created at `fallback` if this is the first we've
    /// heard of them (an unreliable snapshot can outrun the reliable join).
    pub fn entry(&mut self, id: PlayerId, fallback: glam::Vec3) -> &mut RemotePlayer {
        self.players
            .entry(id)
            .or_insert_with(|| RemotePlayer::new(id, format!("Player {}", id.0), fallback))
    }

    pub fn count(&self) -> usize {
        self.players.len()
    }
}

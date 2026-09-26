//! Networking for [`InGameState`]: applying what arrived, deciding what to say.
//!
//! Transport itself lives behind [`Session`](crate::application::session::Session) —
//! this module never touches a socket. It splits into three parts:
//!
//! - [`InGameState::pump_network`] drives one frame: drain, apply, speak, flush.
//! - `apply_*` interpret one [`Inbound`] against the world, the players and the
//!   mobs. They are ordinary `&mut self` methods, testable against a
//!   [`FakeSession`](crate::application::session::FakeSession).
//! - [`host`] admits and forgets peers and judges their requests; [`client`]
//!   applies the host's word; [`outgoing`] is what this peer says.
//! - Converting between the in-memory model and the wire is
//!   [`crate::application::protocol::mapping`]; a host's player records are
//!   [`crate::infrastructure::save::records`].

use std::time::Duration;

use glam::Vec3;

use super::mobs;
use super::{
    HOST_PLAYER_ID, INVENTORY_SYNC_INTERVAL, InGameState, STATS_INTERVAL, WORLD_SYNC_BATCH,
};
use crate::application::ecs::components::{Health, Kind, Mob, RemotePlayer, Transform};
use crate::application::ecs::systems::mobs as mob_systems;
use crate::application::ecs::systems::players;
use crate::application::ecs::{With, spawn};
use crate::application::protocol::mapping::{
    equipment_from_slots, equipment_of, inventory_to_wire, recipes_to_wire,
};
use crate::application::session::Inbound;
use crate::domain::core::{BlockId, BlockPos};
use crate::domain::entity::MobId;
use crate::domain::inventory::crafting::KnownItems;
use crate::domain::inventory::{ItemId, Tool};
use crate::infrastructure::net::{Channel, ClientMessage, Equipment, PlayerId, ServerMessage};
use crate::infrastructure::save::records::{record_remote, record_to_restore};

mod client;
mod host;
mod outgoing;

impl InGameState {
    /// Drive networking for one frame: drain the transport and apply what
    /// arrived, then say this frame's piece and flush.
    pub(super) fn pump_network(&mut self, dt: f32) {
        let duration = Duration::from_secs_f32(dt.max(1.0e-4));

        // Survival stats are low-frequency; throttle them to keep the wire quiet.
        self.net.peers.stats_timer += dt;
        let send_stats = self.net.peers.stats_timer >= STATS_INTERVAL;
        if send_stats {
            self.net.peers.stats_timer = 0.0;
        }

        for msg in self.net.session.poll(duration) {
            self.apply_inbound(msg);
        }

        if self.net.session.is_authority() {
            self.broadcast_authority_state(send_stats);
        } else {
            // A client decides nothing, so it has nothing to tell anyone.
            self.sim.outbox.clear();
            self.report_to_host(dt, send_stats);
        }
        self.net.session.flush();
    }

    /// Apply one message from the network. Requests arrive on the authority and
    /// are validated here; updates arrive on a client and are already truth.
    fn apply_inbound(&mut self, inbound: Inbound) {
        match inbound {
            Inbound::Joined {
                player,
                identity,
                account,
            } => self.welcome_player(player, identity, account),
            Inbound::Left { player } => self.forget_player(player),
            Inbound::Request { player, msg } => self.apply_request(player, msg),
            Inbound::Update(msg) => self.apply_update(msg),
        }
    }
}

#[cfg(test)]
mod tests;

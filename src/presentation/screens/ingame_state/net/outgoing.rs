//! What this peer tells the others each frame, and on the spot.

use super::*;

impl InGameState {
    /// Host: publish this frame's authoritative state.
    pub(super) fn broadcast_authority_state(&mut self, send_stats: bool) {
        self.broadcast_player_states();
        // Periodic authoritative vitals for the host and every remote player.
        if send_stats {
            self.broadcast_player_stats();
        }
        self.broadcast_equipment_changes();
        self.broadcast_mob_state();
    }

    /// Player snapshots: the host's own, then every remote's.
    fn broadcast_player_states(&mut self) {
        let mut snapshots = vec![(
            HOST_PLAYER_ID,
            self.sim.player.position.to_array(),
            self.sim.player.yaw,
            self.sim.player.pitch,
        )];
        snapshots.extend(
            players::all(&self.sim.ecs)
                .map(|rp| (rp.id, rp.position().to_array(), rp.yaw, rp.pitch)),
        );
        for (id, position, yaw, pitch) in snapshots {
            self.net.session.broadcast(
                &ServerMessage::PlayerState {
                    id,
                    position,
                    yaw,
                    pitch,
                },
                Channel::Unreliable,
            );
        }
    }

    /// Every player's vitals and mode, host first.
    fn broadcast_player_stats(&mut self) {
        let mut stats = vec![(
            HOST_PLAYER_ID,
            self.sim.player.health,
            self.sim.player.hunger,
            self.sim.player.mode,
        )];
        stats.extend(players::all(&self.sim.ecs).map(|rp| (rp.id, rp.health, rp.hunger, rp.mode)));
        for (id, health, hunger, mode) in stats {
            self.net.session.broadcast(
                &ServerMessage::PlayerStats {
                    id,
                    health,
                    hunger,
                    mode,
                },
                Channel::Reliable,
            );
        }
    }

    /// What everyone is wearing and holding, only when it changes (the host's
    /// own from its inventory, each remote's from its last inventory sync).
    fn broadcast_equipment_changes(&mut self) {
        let mut equip: Vec<(PlayerId, Equipment)> =
            vec![(HOST_PLAYER_ID, equipment_of(&self.sim.inventory))];
        equip.extend(players::all(&self.sim.ecs).map(|rp| (rp.id, rp.equipment)));
        for (id, equipment) in equip {
            if self.net.peers.equipment.get(&id) != Some(&equipment) {
                self.net.peers.equipment.insert(id, equipment);
                self.net.session.broadcast(
                    &ServerMessage::PlayerEquipment { id, equipment },
                    Channel::Reliable,
                );
            }
        }
    }

    /// Mob lifecycle events queued by this frame's simulation (spawns, hurts,
    /// deaths, arrows, remote-player damage), then one batched unreliable
    /// movement snapshot for all live mobs.
    fn broadcast_mob_state(&mut self) {
        // Only a host has anyone to tell; elsewhere the events are dropped
        // here, exactly where they used to be dropped at the moment of emission.
        let events = std::mem::take(&mut self.sim.outbox);
        if self.net.session.serves_peers() {
            for msg in events {
                self.net.session.broadcast(&msg, Channel::Reliable);
            }
        }
        if self.sim.ecs.count::<Mob>() > 0 {
            let states = ServerMessage::MobStates {
                mobs: self
                    .sim
                    .ecs
                    .query::<(&MobId, &Transform, With<Mob>)>()
                    .map(|(_, (id, t, ()))| (id.0, t.position.to_array(), t.yaw))
                    .collect(),
            };
            self.net.session.broadcast(&states, Channel::Unreliable);
        }
    }

    /// Client: report our own state to the host.
    pub(super) fn report_to_host(&mut self, dt: f32, send_stats: bool) {
        // Ask the host to replay the world's existing edits, once connected.
        if !self.net.peers.world_state_requested && self.net.session.is_connected() {
            self.net
                .session
                .request(&ClientMessage::RequestWorldState, Channel::Reliable);
            self.net.peers.world_state_requested = true;
        }

        self.net.session.request(
            &ClientMessage::Move {
                position: self.sim.player.position.to_array(),
                yaw: self.sim.player.yaw,
                pitch: self.sim.player.pitch,
            },
            Channel::Unreliable,
        );
        if send_stats {
            self.net.session.request(
                &ClientMessage::Stats {
                    health: self.sim.player.health,
                    hunger: self.sim.player.hunger,
                    saturation: self.sim.player.saturation,
                },
                Channel::Reliable,
            );
        }

        // Report the inventory (throttled, only on change) so the host can
        // persist it in the world save.
        self.net.peers.inventory_sync_timer += dt;
        if self.net.peers.inventory_sync_timer >= INVENTORY_SYNC_INTERVAL {
            self.net.peers.inventory_sync_timer = 0.0;
            let changed = self
                .net
                .peers
                .last_synced_inventory
                .as_ref()
                .is_none_or(|last| {
                    last.slots() != self.sim.inventory.slots()
                        || last.selected_index() != self.sim.inventory.selected_index()
                });
            if changed {
                let (slots, selected) = inventory_to_wire(&self.sim.inventory);
                self.net.session.request(
                    &ClientMessage::SyncInventory { slots, selected },
                    Channel::Reliable,
                );
                self.net.peers.last_synced_inventory = Some(self.sim.inventory.clone());
            }
        }
    }

    /// Propagate a local block edit: the authority asserts it, a client asks.
    pub(in super::super) fn broadcast_local_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.net.session.is_authority() {
            self.net.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        } else {
            let msg = if block.is_air() {
                ClientMessage::Break { pos }
            } else {
                ClientMessage::Place { pos, block }
            };
            self.net.session.request(&msg, Channel::Reliable);
        }
    }

    /// Tell the host the local player's game mode changed (a no-op on the
    /// authority — the host advertises its mode via `PlayerStats`/`Welcome`).
    pub(in super::super) fn broadcast_mode_change(&mut self) {
        let mode = self.sim.player.mode;
        if !self.net.session.is_authority() {
            self.net
                .session
                .request(&ClientMessage::SetMode(mode), Channel::Reliable);
        }
    }

    pub(in super::super) fn net_status(&self) -> String {
        self.net
            .session
            .status(self.sim.ecs.count::<RemotePlayer>())
    }

    /// Swap in a different networking role. Tests use this to drive host and
    /// client logic through a fake transport; production wiring sets it in
    /// `setup`.
    #[cfg(test)]
    pub(in super::super) fn set_session(
        &mut self,
        session: Box<dyn crate::application::session::Session>,
    ) {
        self.sim.authoritative = session.is_authority();
        self.net.session = session;
    }
}

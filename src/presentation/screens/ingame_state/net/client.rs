//! A client's side: applying what the host says is so.

use super::*;

impl InGameState {
    /// Apply one authoritative update from the host.
    pub(super) fn apply_update(&mut self, msg: ServerMessage) {
        let local_id = self.net.session.local_id();
        // The biome/boss loop's messages are handled beside the code that
        // sends them; whatever they decline comes back to be matched here.
        let Some(msg) = self.apply_progression_update(msg) else {
            return;
        };
        let Some(msg) = self.apply_boss_update(msg) else {
            return;
        };
        match msg {
            // The welcome is consumed during construction, not here.
            ServerMessage::Welcome { .. } => {}
            // Only the server browser's probe ever asks for one, and it is not
            // an `InGameState` — a playing client seeing this is the host
            // answering a question nobody in the world asked.
            ServerMessage::Status { .. } => {}
            ServerMessage::PlayerJoined { id, name } if id != local_id => {
                if players::find(&self.sim.ecs, id).is_none() {
                    spawn::remote_player(
                        &mut self.sim.ecs,
                        RemotePlayer::new(id, name, Vec3::ZERO),
                    );
                }
            }
            ServerMessage::PlayerLeft { id } => {
                players::remove(&mut self.sim.ecs, id);
            }
            ServerMessage::PlayerState {
                id,
                position,
                yaw,
                pitch,
            } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::from_array(position)).push_snapshot(
                    Vec3::from_array(position),
                    yaw,
                    pitch,
                );
            }
            ServerMessage::BlockChanged { pos, block } => {
                // apply_edit (not set_block) so an edit whose chunk hasn't
                // streamed in yet is buffered and applied when it loads.
                self.sim.world.apply_edit(pos, block);
            }
            ServerMessage::WorldEdits { edits } => {
                let count = edits.len();
                for (pos, block) in edits {
                    self.sim.world.apply_edit(pos, block);
                }
                log::debug!("applied {count} world-state edits on join");
            }
            ServerMessage::PlayerStats {
                id,
                health,
                hunger,
                mode,
            } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::ZERO).set_stats(health, hunger, mode);
            }
            ServerMessage::PlayerEquipment { id, equipment } if id != local_id => {
                players::entry(&mut self.sim.ecs, id, Vec3::ZERO).equipment = equipment;
            }
            ServerMessage::MobSpawned { id, kind, position } => {
                match self.content.rules.entities.find(&kind) {
                    Some(k) if k.mob.is_some() => {
                        log::debug!("replicating mob {id} ({kind}) from host");
                        spawn::replica(&mut self.sim.ecs, k, MobId(id), Vec3::from_array(position));
                    }
                    // Shouldn't happen (the content hash gates divergent builds),
                    // but degrade gracefully.
                    _ => log::warn!("host spawned unknown mob kind {kind:?}; ignoring"),
                }
            }
            ServerMessage::MobStates { mobs } => {
                for (id, position, yaw) in mobs {
                    // Unknown ids are fine: an unreliable snapshot can outrun
                    // its reliable MobSpawned.
                    if let Some(entity) = mob_systems::find(&self.sim.ecs, MobId(id))
                        && let Some(transform) = self.sim.ecs.get_mut::<Transform>(entity)
                    {
                        transform.position = Vec3::from_array(position);
                        transform.yaw = yaw;
                    }
                }
            }
            ServerMessage::MobHurt { id, health } => {
                // Mirrored for the boss bar; the authoritative outcome of a
                // killing blow still arrives as MobDespawned.
                if let Some(entity) = mob_systems::find(&self.sim.ecs, MobId(id))
                    && let Some(mirror) = self.sim.ecs.get_mut::<Health>(entity)
                {
                    mirror.current = health;
                }
            }
            ServerMessage::MobDespawned { id, killed_by } => {
                let gone = mob_systems::find(&self.sim.ecs, MobId(id)).and_then(|entity| {
                    let kind = self.sim.ecs.get::<Kind>(entity)?.name.clone();
                    let position = self.sim.ecs.get::<Transform>(entity)?.position;
                    self.sim.ecs.despawn(entity);
                    Some((kind, position))
                });
                if let Some((kind, position)) = gone
                    && killed_by == Some(local_id)
                {
                    // This player made the kill: roll the loot locally.
                    self.sim.pop_loot(&kind, id, position);
                }
            }
            ServerMessage::ArrowSpawned {
                position,
                velocity,
                gravity,
                lifetime,
            } => {
                // Visual-only on clients: damage is host-side, so the local
                // copy carries none.
                crate::application::ecs::spawn::arrow(
                    &mut self.sim.ecs,
                    Vec3::from_array(position),
                    Vec3::from_array(velocity),
                    0.0,
                    gravity,
                    lifetime,
                );
            }
            ServerMessage::PlayerDamaged { id, amount } if id == local_id => {
                self.sim.damage_local_player(amount);
            }
            ServerMessage::Chat { from, kind, text } => self.show_remote_chat(from, kind, text),
            ServerMessage::GrantItems { to, stacks } if to == local_id => {
                self.apply_granted_items(&stacks);
            }
            ServerMessage::Teleport { to, position } if to == local_id => {
                self.apply_teleport(position);
            }
            _ => {}
        }
    }
}

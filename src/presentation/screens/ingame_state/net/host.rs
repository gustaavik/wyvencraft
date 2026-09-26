//! The host's side: who joins and leaves, and what their requests may do.

use super::*;

impl InGameState {
    /// Admit a joining peer: hand back their saved state if this world
    /// remembers them, and give them a replica to be tracked by.
    ///
    /// It does *not* announce them — see [`InGameState::announce_player`].
    pub(super) fn welcome_player(
        &mut self,
        pid: PlayerId,
        identity: u64,
        account: Option<wyven_auth::AccountIdentity>,
    ) {
        let restored = self.save.records.0.get(&identity).map(|record| {
            let known = self.save.discovery.players.get(&identity);
            record_to_restore(
                record,
                known.map_or(&[], Vec::as_slice),
                &self.content.rules.items,
            )
        });
        let spawn = restored
            .as_ref()
            .map(|r| r.position)
            .unwrap_or_else(|| self.sim.player.position.to_array());
        if restored.is_some() {
            log::info!("player {} rejoined; restoring saved state", pid.0);
        }

        let welcome = ServerMessage::Welcome {
            seed: self.sim.world.seed(),
            your_id: pid,
            spawn,
            time_of_day: self.sim.day_cycle.time_of_day(),
            game_mode: self.sim.player.mode,
            content_hash: self.content.rules.hash(),
            recipes: recipes_to_wire(&self.sim.recipes, &self.content.rules.items),
            restored,
        };
        self.net.session.send_to(pid, &welcome, Channel::Reliable);

        // The name comes from the player's verified ticket, not from anything
        // they typed or the host made up. `PlayerJoined` already carried a name
        // field — it was just never given a real one to carry.
        //
        // The fallback only fires where there is nobody to verify (the
        // singleplayer session, or a test): a host on a real socket refuses
        // joins it cannot verify, so a connected peer always has an account.
        let name = account
            .as_ref()
            .map(|account| account.username.clone())
            .unwrap_or_else(|| format!("Player {}", pid.0));

        self.net.peers.identities.insert(pid, identity);
        if let Some(account) = account {
            self.net.peers.accounts.insert(pid, account);
        }
        players::remove(&mut self.sim.ecs, pid);
        spawn::remote_player(
            &mut self.sim.ecs,
            RemotePlayer::new(pid, name, Vec3::from_array(spawn)),
        );
    }

    /// Drop everything this session knows about a peer that has gone: their
    /// entity, and what the host still owed them.
    pub(super) fn forget_peer(&mut self, pid: PlayerId) {
        players::remove(&mut self.sim.ecs, pid);
        self.net.peers.remove(pid);
    }

    /// Tell everyone a peer is really here, and bring it up to date on what
    /// everyone is wearing.
    ///
    /// Deliberately *not* part of admitting a peer. A status probe from the
    /// server browser connects with a valid ticket and gets a `Welcome` like
    /// anyone else, but it never asks for the world — so it never reaches here,
    /// and nobody playing ever sees it come and go. See `Peers::announced`.
    pub(super) fn announce_player(&mut self, pid: PlayerId) {
        if !self.net.peers.announced.insert(pid) {
            return;
        }
        let Some(name) = players::get(&self.sim.ecs, pid).map(|rp| rp.name.clone()) else {
            return;
        };

        self.net.session.broadcast(
            &ServerMessage::PlayerJoined { id: pid, name },
            Channel::Reliable,
        );

        let equipment: Vec<(PlayerId, Equipment)> = self
            .net
            .peers
            .equipment
            .iter()
            .map(|(&id, &equipment)| (id, equipment))
            .collect();
        for (id, equipment) in equipment {
            self.net.session.send_to(
                pid,
                &ServerMessage::PlayerEquipment { id, equipment },
                Channel::Reliable,
            );
        }
    }

    /// Answer a status query: what the server browser puts in a row.
    ///
    /// Sent only to the peer that asked. Nothing else happens — no announcement,
    /// no world state, no record — which is the whole reason a probe can ask.
    pub(super) fn report_status(&mut self, pid: PlayerId) {
        // The host counts as one of the players, and as one of the slots: it is
        // in the world and it is occupying capacity, so a row reading "1/17" on
        // an empty server is the truth rather than an off-by-one.
        let status = ServerMessage::Status {
            name: self.save.world_name().to_string(),
            // Announced players, not connected peers: a probe gives itself a
            // replica in `peers` like anyone else, and must not count itself
            // (nor the other probes refreshing their lists at the same moment).
            online: (self.net.peers.announced.len() + 1) as u32,
            max: (crate::infrastructure::net::MAX_CLIENTS + 1) as u32,
            content_hash: self.content.rules.hash(),
        };
        self.net.session.send_to(pid, &status, Channel::Reliable);
    }

    /// Keep what a client has discovered, so its next `Welcome` hands it back.
    ///
    /// Merged rather than replaced: the set only ever grows, and merging means
    /// two reports arriving out of order (`Channel::Reliable` is unordered)
    /// cannot lose anything. Kept by stable identity, like the player records.
    pub(super) fn record_discovery(&mut self, pid: PlayerId, wire: &[u16]) {
        let Some(&identity) = self.net.peers.identities.get(&pid) else {
            return;
        };
        let items = &self.content.rules.items;
        let entry = self.save.discovery.players.entry(identity).or_default();
        let mut known = KnownItems::from_ids(entry, items);
        if known.merge(&KnownItems::from_wire(wire, items)) {
            *entry = known.to_ids(items);
        }
    }

    /// Snapshot a leaving player so their state survives a rejoin, then drop
    /// every trace of them from this session.
    pub(super) fn forget_player(&mut self, pid: PlayerId) {
        // A peer nobody was told about is a peer nobody has to be told left —
        // and, more importantly, one whose account must not be written over. A
        // status probe never plays, so recording it would replace a real
        // player's saved position and vitals with the spawn values it was
        // handed a moment earlier.
        if !self.net.peers.announced.contains(&pid) {
            self.forget_peer(pid);
            return;
        }

        record_remote(
            &mut self.save.records,
            &self.net.peers.identities,
            &self.sim.ecs,
            &self.net.peers.inventories,
            &self.content.rules.items,
            pid,
        );
        self.forget_peer(pid);
        self.net
            .session
            .broadcast(&ServerMessage::PlayerLeft { id: pid }, Channel::Reliable);
    }

    /// Validate and apply one client request. Everything a client can ask for
    /// is checked here — this is the authority's only inbound surface.
    pub(super) fn apply_request(&mut self, pid: PlayerId, msg: ClientMessage) {
        match msg {
            ClientMessage::Move {
                position,
                yaw,
                pitch,
            } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.push_snapshot(Vec3::from_array(position), yaw, pitch);
                }
            }
            ClientMessage::Break { pos } => {
                if self.client_may_break(pid, pos) {
                    self.apply_client_edit(pos, BlockId::AIR);
                }
            }
            ClientMessage::Place { pos, block } => self.apply_client_edit(pos, block),
            ClientMessage::Stats {
                health,
                hunger,
                saturation,
            } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.health = health;
                    rp.hunger = hunger;
                    rp.saturation = saturation;
                }
            }
            ClientMessage::SetMode(m) => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.mode = m;
                }
            }
            ClientMessage::SyncInventory { slots, selected } => {
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.equipment = equipment_from_slots(&slots, selected);
                }
                self.net.peers.inventories.insert(pid, (slots, selected));
            }
            ClientMessage::SyncKnown { items } => self.record_discovery(pid, &items),
            // The only place a command is ever parsed and run: the host knows
            // who is authorized, so the host decides.
            ClientMessage::Chat(text) => self.dispatch_chat(pid, text),
            ClientMessage::RequestWorldState => {
                // The first thing a real client asks for, and the thing a status
                // probe never asks for — so this is where a connected peer
                // becomes a player everyone else can see.
                self.announce_player(pid);
                self.replay_world_state(pid);
            }
            ClientMessage::RequestStatus => self.report_status(pid),
            ClientMessage::Attack { id } => self.apply_client_attack(pid, id),
            ClientMessage::UseBlock {
                pos,
                slots,
                selected,
            } => {
                // Adopt the snapshot first, so the use is judged against
                // exactly what the client held when it clicked.
                if let Some(rp) = players::get_mut(&mut self.sim.ecs, pid) {
                    rp.equipment = equipment_from_slots(&slots, selected);
                }
                self.net.peers.inventories.insert(pid, (slots, selected));
                self.use_block(pid, pos);
            }
        }
    }

    /// Whether a client's tool meets the tier of the block it broke — the
    /// progression gate, checked here too so a modified client cannot mine a
    /// later biome's ore on day one. Read off the inventory it last reported,
    /// the same trust `client_melee_damage` extends. A refused break is undone
    /// on the client by echoing the block back, since it already applied the
    /// edit optimistically.
    pub(super) fn client_may_break(&mut self, pid: PlayerId, pos: BlockPos) -> bool {
        let existing = self.sim.world.block_at(pos);
        let harvest = self.content.rules.blocks.get(existing).harvest.as_ref();
        let tool = self
            .net
            .peers
            .inventories
            .get(&pid)
            .and_then(|(slots, selected)| slots.get(*selected as usize)?.as_ref())
            .and_then(|stack| {
                self.content
                    .rules
                    .items
                    .component::<Tool>(ItemId(stack.item))
            });
        if crate::domain::inventory::meets_tier(harvest, tool) {
            return true;
        }
        log::info!("player {} broke {pos:?} below its tier; undoing", pid.0);
        self.net.session.send_to(
            pid,
            &ServerMessage::BlockChanged {
                pos,
                block: existing,
            },
            Channel::Reliable,
        );
        false
    }

    /// Apply a client's block edit and echo the result to everyone.
    pub(super) fn apply_client_edit(&mut self, pos: BlockPos, block: BlockId) {
        if self.sim.world.set_block(pos, block).is_some() {
            self.sim.fluids.block_changed(pos);
            self.net.session.broadcast(
                &ServerMessage::BlockChanged { pos, block },
                Channel::Reliable,
            );
        }
    }

    /// Replay the world's existing edits and current mob population to a
    /// joining player, so they see what's already there.
    pub(super) fn replay_world_state(&mut self, pid: PlayerId) {
        let edits = self.sim.world.collect_edits();
        log::debug!("replaying {} world edits to player {}", edits.len(), pid.0);
        for batch in edits.chunks(WORLD_SYNC_BATCH) {
            self.net.session.send_to(
                pid,
                &ServerMessage::WorldEdits {
                    edits: batch.to_vec(),
                },
                Channel::Chunk,
            );
        }
        let spawned: Vec<ServerMessage> = self
            .sim
            .ecs
            .query::<(&MobId, &Kind, &Transform, With<Mob>)>()
            .map(|(_, (id, kind, transform, ()))| ServerMessage::MobSpawned {
                id: id.0,
                kind: kind.name.clone(),
                position: transform.position.to_array(),
            })
            .collect();
        for msg in spawned {
            self.net.session.send_to(pid, &msg, Channel::Reliable);
        }
        self.net.session.send_to(
            pid,
            &ServerMessage::Progression(self.sim.progression.clone()),
            Channel::Reliable,
        );
    }

    /// Damage a client's swing lands, from the item in the hotbar slot they
    /// last reported selected.
    ///
    /// `SyncInventory` is throttled and client-reported, so this can lag a
    /// weapon swap by a beat and a dishonest client could claim a better sword.
    /// That is the same trust the host already extends to `ClientMessage::Stats`
    /// for health and hunger; an unknown or empty slot falls back to the fist.
    pub(super) fn client_melee_damage(&self, pid: PlayerId) -> f32 {
        self.net
            .peers
            .inventories
            .get(&pid)
            .and_then(|(slots, selected)| slots.get(*selected as usize)?.as_ref())
            .and_then(|stack| {
                self.content
                    .rules
                    .items
                    .component::<Tool>(ItemId(stack.item))
            })
            .and_then(|tool| tool.damage)
            .unwrap_or(mobs::PLAYER_ATTACK_DAMAGE)
    }

    /// Validate a client's melee swing against their last known position, then
    /// apply it with kill credit. The outcome reaches clients via `mob_events`.
    pub(super) fn apply_client_attack(&mut self, pid: PlayerId, mob_id: u64) {
        let Some(attacker) = players::get(&self.sim.ecs, pid).map(|rp| rp.position()) else {
            return;
        };
        let damage = self.client_melee_damage(pid);
        let target = mob_systems::find(&self.sim.ecs, MobId(mob_id)).and_then(|entity| {
            let at = self.sim.ecs.get::<Transform>(entity)?.position;
            Some((entity, at))
        });
        if let Some((entity, at)) = target
            && mobs::attack_in_range(attacker, at)
        {
            let to_mob = at - attacker;
            let push = Vec3::new(to_mob.x, 0.0, to_mob.z).normalize_or_zero()
                * mobs::KNOCKBACK_PUSH
                + Vec3::Y * mobs::KNOCKBACK_LIFT;
            let Some(health) = mob_systems::hit(&mut self.sim.ecs, entity, damage, push, pid.0)
            else {
                return;
            };
            self.sim.emit(ServerMessage::MobHurt { id: mob_id, health });
        }
    }
}

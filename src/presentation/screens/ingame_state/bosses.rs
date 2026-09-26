//! Boss fights from the state layer: summoning at an altar, landing the
//! attacks a boss's brain commits to, the leash, the kill, and what the HUD
//! shows about it all.
//!
//! The authority runs the fight — the brain lives on the simulated
//! [`Mob`](crate::domain::entity::Mob) — and tells clients the rest through
//! `BossTelegraph`, `BossPhase` and `BossDefeated`. Kept out of `mobs.rs`,
//! which already carries every other mob.

use glam::Vec3;

use super::InGameState;
use super::block_use::UseAt;
use crate::application::ecs::With;
use crate::application::ecs::components::{Boss, Health, Kind, Mob, Replica, Transform};
use crate::application::ecs::systems::mobs::{self as mob_systems, Reaped};
use crate::domain::core::Rng64;
use crate::domain::entity::MobId;
use crate::domain::entity::boss::{AttackEffect, BossParams};
use crate::domain::entity::mob::eye_position;
use crate::domain::inventory::ItemStack;
use crate::infrastructure::net::{ChatKind, PlayerId, ServerMessage};
use crate::presentation::ui::boss_bar::BossBarView;

/// How far from its altar (blocks) a summoned boss appears.
const SUMMON_DISTANCE: f32 = 7.0;
/// Players within this multiple of a boss's arena see its bar.
const BAR_RANGE: f32 = 1.5;
/// Horizontal speed a slam flings the local player at, per point of knockback.
const SLAM_LIFT: f32 = 6.0;
/// Where volley projectiles leave the boss, ahead of its face.
const MUZZLE: f32 = 0.8;

pub(super) use crate::application::simulation::{BossFight, Telegraph};

pub(super) use crate::application::simulation::BossBeat;

impl InGameState {
    /// Authority: `at.actor` offers at an altar that summons `boss`.
    pub(super) fn offer_at_altar(&mut self, at: &UseAt, boss: &str) {
        let Some(params) = self
            .content
            .rules
            .entities
            .find(boss)
            .and_then(|kind| kind.boss.clone())
        else {
            log::warn!("altar at {:?} names {boss:?}, which is no boss", at.pos);
            return;
        };
        if self.sim.mobs.fight.is_some() {
            let text = "The altar is cold — a battle already rages.".to_string();
            self.reply(at.actor, ChatKind::System, text);
            return;
        }
        let Some(item) = self.content.rules.items.find(&params.offering.item) else {
            log::warn!(
                "boss {boss:?} asks for unknown item {:?}",
                params.offering.item
            );
            return;
        };
        let need = u32::from(params.offering.count);
        if self.count_items(at.actor, item) < need {
            let name = self.content.item_display_name(item).to_string();
            let text = format!("The altar hungers for {need} × {name}.");
            self.reply(at.actor, ChatKind::System, text);
            return;
        }
        self.take_items(at.actor, &[ItemStack::new(item, params.offering.count)]);
        let at_altar = Vec3::new(
            at.pos.x as f32 + 0.5,
            at.pos.y as f32,
            at.pos.z as f32 + 0.5,
        );
        let spot = at_altar + Vec3::Z * SUMMON_DISTANCE;
        let ground = self
            .sim
            .find_ground(spot.x, spot.z, at.pos.y + 8)
            .unwrap_or(spot.y);
        let Some(mob) = self.sim.spawn_mob(boss, Vec3::new(spot.x, ground, spot.z)) else {
            return;
        };
        self.sim.mobs.fight = Some(BossFight {
            mob,
            altar: at.pos,
            empty_for: 0.0,
        });
        self.announce(format!("{} awakens!", params.title));
        log::info!("player {} summoned {boss:?} at {:?}", at.actor.0, at.pos);
    }

    /// Authority: resolve the beats the mob tick collected.
    pub(super) fn land_boss_beats(&mut self, beats: Vec<BossBeat>) {
        for beat in beats {
            match beat {
                BossBeat::Windup {
                    mob,
                    attack,
                    seconds,
                } => self.telegraph(mob, attack, seconds),
                BossBeat::Release {
                    mob,
                    attack,
                    target,
                } => self.land_attack(mob, attack, target),
                BossBeat::Phase { mob, phase } => self.enter_phase(mob, phase),
            }
        }
    }

    /// A live boss by id: where it stands, where it sees from, and its move set.
    fn boss_mob(&self, id: MobId) -> Option<(Vec3, Vec3, &BossParams)> {
        let entity = mob_systems::find(&self.sim.ecs, id)?;
        let transform = self.sim.ecs.get::<Transform>(entity)?;
        let body = self
            .sim
            .ecs
            .get::<crate::application::ecs::components::Body>(entity)?;
        let boss = self.sim.ecs.get::<Boss>(entity)?;
        Some((
            transform.position,
            eye_position(transform.position, &body.physics),
            &boss.params,
        ))
    }

    fn telegraph(&mut self, id: MobId, attack: usize, seconds: f32) {
        let Some((_, _, params)) = self.boss_mob(id) else {
            return;
        };
        let attack_id = params.attacks[attack].id.clone();
        self.sim.mobs.telegraph = Some(Telegraph {
            mob: id.0,
            name: crate::domain::core::ident::title_case(&attack_id),
            remaining: seconds,
        });
        self.sim.emit(ServerMessage::BossTelegraph {
            id: id.0,
            attack: attack_id,
            windup: seconds,
        });
    }

    fn enter_phase(&mut self, id: MobId, phase: u8) {
        let Some((_, _, params)) = self.boss_mob(id) else {
            return;
        };
        let title = params.title.clone();
        self.sim.emit(ServerMessage::BossPhase { id: id.0, phase });
        if phase > 0 {
            self.announce(format!("{title} is enraged!"));
        }
    }

    /// Apply one attack's effect as it lands.
    fn land_attack(&mut self, id: MobId, attack: usize, target: Option<(Option<PlayerId>, Vec3)>) {
        let Some((position, eye, params)) = self.boss_mob(id) else {
            return;
        };
        let spec = params.attacks[attack].clone();
        if self
            .sim
            .mobs
            .telegraph
            .as_ref()
            .is_some_and(|t| t.mob == id.0)
        {
            self.sim.mobs.telegraph = None;
        }
        match spec.effect {
            AttackEffect::Melee { damage } => {
                if let Some((player, target_eye)) = target
                    && target_eye.distance(eye) <= spec.range + 1.0
                {
                    self.hit_player(player, damage, Vec3::ZERO);
                }
            }
            AttackEffect::Slam {
                damage,
                radius,
                knockback,
            } => {
                for t in self.sim.mob_targets() {
                    let offset = t.eye - position;
                    if Vec3::new(offset.x, 0.0, offset.z).length() <= radius {
                        let away = Vec3::new(offset.x, 0.0, offset.z).normalize_or_zero();
                        let push = away * knockback + Vec3::Y * (knockback / SLAM_LIFT).min(8.0);
                        self.hit_player(t.player, damage, push);
                    }
                }
            }
            AttackEffect::Volley {
                damage,
                count,
                spread_deg,
                speed,
                gravity,
                lifetime,
            } => {
                let Some((_, target_eye)) = target else {
                    return;
                };
                let to = target_eye - eye;
                let lift = 0.5 * gravity * to.length() / speed.max(0.001);
                let dir = to.normalize_or_zero();
                for i in 0..count {
                    let t = if count > 1 {
                        f32::from(i) / f32::from(count - 1) - 0.5
                    } else {
                        0.0
                    };
                    let turned = glam::Quat::from_rotation_y((spread_deg * t).to_radians()) * dir;
                    let velocity = turned * speed + Vec3::Y * lift;
                    let origin = eye + turned * MUZZLE;
                    self.sim.emit(ServerMessage::ArrowSpawned {
                        position: origin.to_array(),
                        velocity: velocity.to_array(),
                        gravity,
                        lifetime,
                    });
                    crate::application::ecs::spawn::arrow(
                        &mut self.sim.ecs,
                        origin,
                        velocity,
                        damage,
                        gravity,
                        lifetime,
                    );
                }
            }
            AttackEffect::Summon { entity, count, cap } => {
                let alive = self
                    .sim
                    .ecs
                    .query::<(&Kind, &Transform, With<Mob>)>()
                    .filter(|(_, (kind, t, ()))| {
                        kind.name == entity && t.position.distance(position) < 48.0
                    })
                    .count() as u32;
                let mut rng = Rng64::new(self.sim.world.seed() ^ id.0 ^ self.sim.mobs.next_id);
                let want = rng.range_u32(u32::from(count[0]), u32::from(count[1]));
                for i in 0..want.min(cap.saturating_sub(alive)) {
                    let angle = i as f32 * 2.1 + rng.range_f32(0.0, 1.0);
                    let spot = position + Vec3::new(angle.cos(), 0.0, angle.sin()) * 3.0;
                    let ground = self
                        .sim
                        .find_ground(spot.x, spot.z, position.y as i32 + 6)
                        .unwrap_or(position.y);
                    self.sim
                        .spawn_mob(&entity, Vec3::new(spot.x, ground, spot.z));
                }
            }
        }
    }

    /// Damage a player — the local one directly (knocked back by `push`),
    /// a remote one through the wire (clients own their bodies).
    fn hit_player(&mut self, player: Option<PlayerId>, damage: f32, push: Vec3) {
        match player {
            None => {
                self.sim.damage_local_player(damage);
                self.sim.player.velocity += push;
            }
            Some(id) => self
                .sim
                .emit(ServerMessage::PlayerDamaged { id, amount: damage }),
        }
    }

    /// Authority, every frame: the leash, and the telegraph clock.
    pub(super) fn update_boss_fight(&mut self, dt: f32) {
        self.tick_telegraph(dt);
        let Some(fight) = &self.sim.mobs.fight else {
            return;
        };
        let (id, altar) = (fight.mob, fight.altar);
        let Some((_, _, params)) = self.boss_mob(id) else {
            // Killed (the defeat already cleared the fight) or gone some other
            // way: either way nothing is left to leash.
            self.sim.mobs.fight = None;
            return;
        };
        let radius = params.arena_radius;
        let leash = params.leash_seconds;
        let title = params.title.clone();
        let centre = Vec3::new(altar.x as f32 + 0.5, altar.y as f32, altar.z as f32 + 0.5);
        let occupied = self.fighters_near(centre, radius).next().is_some();
        let Some(fight) = &mut self.sim.mobs.fight else {
            return;
        };
        fight.empty_for = if occupied { 0.0 } else { fight.empty_for + dt };
        if fight.empty_for < leash {
            return;
        }
        // Nobody stayed to fight: the boss returns whence it came, and takes
        // the offering with it.
        self.sim.mobs.fight = None;
        if let Some(entity) = mob_systems::find(&self.sim.ecs, id) {
            self.sim.ecs.despawn(entity);
        }
        self.sim.emit(ServerMessage::MobDespawned {
            id: id.0,
            killed_by: None,
        });
        self.announce(format!("{title} returns to the wilds."));
    }

    /// Players (local as `None`) within `radius` of `centre`, alive ones only.
    fn fighters_near(&self, centre: Vec3, radius: f32) -> impl Iterator<Item = Option<PlayerId>> {
        let flat = |p: Vec3| Vec3::new(p.x - centre.x, 0.0, p.z - centre.z).length();
        let local = (!self.sim.dead && flat(self.sim.player.position) <= radius).then_some(None);
        let remote: Vec<Option<PlayerId>> =
            crate::application::ecs::systems::players::all(&self.sim.ecs)
                .filter(|p| flat(p.position()) <= radius)
                .map(|p| Some(p.id))
                .collect();
        local.into_iter().chain(remote)
    }

    fn tick_telegraph(&mut self, dt: f32) {
        if let Some(t) = &mut self.sim.mobs.telegraph {
            t.remaining -= dt;
            if t.remaining <= 0.0 {
                self.sim.mobs.telegraph = None;
            }
        }
    }

    /// Authority: a boss died. Record it, tell everyone, and hand out the
    /// loot to every player who was in the arena — not just the killer.
    pub(super) fn on_boss_defeated(&mut self, mob: &Reaped) {
        let Some(params) = &mob.boss else {
            return;
        };
        let participants: Vec<Option<PlayerId>> = self
            .fighters_near(mob.position, params.arena_radius)
            .collect();
        let local_id = self.net.session.local_id();
        let wire: Vec<PlayerId> = participants.iter().map(|p| p.unwrap_or(local_id)).collect();
        self.sim.emit(ServerMessage::BossDefeated {
            id: mob.id.0,
            kind: mob.kind.clone(),
            position: mob.position.to_array(),
            participants: wire,
        });
        if participants.contains(&None) {
            self.sim
                .pop_loot(&mob.kind, loot_seed(mob.id.0, local_id), mob.position);
        }
        let first = self.sim.progression.defeat(&mob.kind);
        if self
            .sim
            .mobs
            .fight
            .as_ref()
            .is_some_and(|f| f.mob == mob.id)
        {
            self.sim.mobs.fight = None;
        }
        if self
            .sim
            .mobs
            .telegraph
            .as_ref()
            .is_some_and(|t| t.mob == mob.id.0)
        {
            self.sim.mobs.telegraph = None;
        }
        self.broadcast_progression();
        let title = params.title.clone();
        let text = if first {
            format!("{title} has fallen! Its power stirs in the land beyond.")
        } else {
            format!("{title} has fallen again!")
        };
        self.announce(text);
    }

    /// Client: apply a boss update from the host. Returns the message back if
    /// it was not one of ours.
    pub(super) fn apply_boss_update(&mut self, msg: ServerMessage) -> Option<ServerMessage> {
        let local_id = self.net.session.local_id();
        match msg {
            ServerMessage::BossPhase { id, phase } => {
                if let Some(entity) = mob_systems::find(&self.sim.ecs, MobId(id))
                    && let Some(replica) = self.sim.ecs.get_mut::<Replica>(entity)
                {
                    replica.phase = phase;
                }
            }
            ServerMessage::BossTelegraph { id, attack, windup } => {
                self.sim.mobs.telegraph = Some(Telegraph {
                    mob: id,
                    name: crate::domain::core::ident::title_case(&attack),
                    remaining: windup,
                });
            }
            ServerMessage::BossDefeated {
                id,
                kind,
                position,
                participants,
            } => {
                if participants.contains(&local_id) {
                    let at = Vec3::from_array(position);
                    self.sim.pop_loot(&kind, loot_seed(id, local_id), at);
                }
                if self
                    .sim
                    .mobs
                    .telegraph
                    .as_ref()
                    .is_some_and(|t| t.mob == id)
                {
                    self.sim.mobs.telegraph = None;
                }
            }
            other => return Some(other),
        }
        None
    }

    /// Client: the telegraph clock (the authority ticks its own in
    /// [`Self::update_boss_fight`]).
    pub(super) fn tick_remote_telegraph(&mut self, dt: f32) {
        self.tick_telegraph(dt);
    }

    /// The boss bar for the nearest boss this player is close to, simulated
    /// here or replicated from the host.
    pub(super) fn boss_bar(&self) -> Option<BossBarView> {
        let here = self.sim.player.position;
        // Simulated here or replicated from the host, a boss is the same
        // entity shape: a kind, a place, a health, and a phase — from its
        // brain when simulated, from the host's last word when replicated.
        let bosses = self
            .sim
            .ecs
            .query::<(
                &MobId,
                &Kind,
                &Transform,
                &Health,
                Option<&Boss>,
                Option<&Replica>,
            )>()
            .filter_map(|(_, (id, kind, t, health, boss, replica))| {
                let (params, phase) = match (boss, replica) {
                    (Some(boss), _) => (&boss.params, boss.phase()),
                    (None, Some(replica)) => {
                        let params = self
                            .content
                            .rules
                            .entities
                            .find(&kind.name)?
                            .boss
                            .as_ref()?;
                        (params, replica.phase)
                    }
                    (None, None) => return None,
                };
                Some((id.0, t.position, params, health.fraction(), phase))
            });
        let (id, _, params, fraction, phase) = bosses
            .filter(|(_, pos, params, _, _)| pos.distance(here) <= params.arena_radius * BAR_RANGE)
            .min_by(|a, b| a.1.distance(here).total_cmp(&b.1.distance(here)))?;
        Some(BossBarView {
            title: params.title.clone(),
            fraction: fraction.clamp(0.0, 1.0),
            phase,
            telegraph: self
                .sim
                .mobs
                .telegraph
                .as_ref()
                .filter(|t| t.mob == id)
                .map(|t| t.name.clone()),
        })
    }
}

/// Each participant rolls a boss's loot table with their own seed, so two
/// players in the same fight do not get identical drops.
fn loot_seed(mob: u64, player: PlayerId) -> u64 {
    mob ^ player.0.wrapping_mul(0x2545_F491_4F6C_DD1D).rotate_left(29)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::session::FakeSession;
    use crate::domain::core::BlockPos;
    use crate::domain::core::GameMode;
    use crate::domain::world::block::blocks;
    use crate::domain::world::structure::Cell;
    use crate::infrastructure::net::NetItemStack;
    use crate::presentation::content::GameContent;

    /// A survival world with the altar nearest spawn, and that altar block.
    fn at_the_altar() -> (InGameState, BlockPos) {
        let state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let config = state.sim.structures.config();
        let index = config.find("meadows_altar").unwrap();
        let altar = state
            .sim
            .structures
            .nearest(index, BlockPos::new(0, 0, 0), 4)
            .expect("an altar near spawn");
        let template = &config.get(index).template;
        let mut block = None;
        for dx in -4..=4 {
            for dz in -4..=4 {
                if template.cell(altar.rot, dx, 1, dz) == Cell::Block(blocks::ELDER_ALTAR) {
                    let a = altar.anchor;
                    block = Some(BlockPos::new(a.x + dx, a.y + 1, a.z + dz));
                }
            }
        }
        (state, block.expect("the template holds the altar block"))
    }

    fn give_local(state: &mut InGameState, item: &str, count: u8) {
        let id = state.content.rules.items.find(item).unwrap();
        state
            .sim
            .inventory
            .set_slot(0, Some(ItemStack::new(id, count)));
    }

    fn stand_beside(state: &mut InGameState, pos: BlockPos) {
        state.sim.player.teleport(Vec3::new(
            pos.x as f32 + 2.5,
            pos.y as f32,
            pos.z as f32 + 0.5,
        ));
    }

    fn use_altar(state: &mut InGameState, pos: BlockPos) {
        let local = state.net.session.local_id();
        state.use_block(local, pos);
    }

    fn bosses(state: &InGameState) -> usize {
        state.simulated_mobs().iter().filter(|m| m.boss).count()
    }

    #[test]
    fn an_empty_handed_offering_summons_nothing() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 0);
        assert!(state.sim.mobs.fight.is_none());
    }

    #[test]
    fn the_offering_is_taken_and_one_boss_answers() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        give_local(&mut state, "stag_effigy", 2);
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 1);
        let effigy = state.content.rules.items.find("stag_effigy").unwrap();
        assert_eq!(
            state.sim.inventory.count_of(effigy),
            1,
            "exactly one offered"
        );

        // A second offering while it lives is refused and costs nothing.
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 1);
        assert_eq!(state.sim.inventory.count_of(effigy), 1);
    }

    /// A client's offering comes out of the copy of its inventory the host
    /// holds, and the client is told to remove it from its own.
    #[test]
    fn a_clients_offering_is_taken_through_the_wire() {
        let (mut state, altar) = at_the_altar();
        let session = FakeSession::host();
        let handle = session.handle();
        state.set_session(Box::new(session));
        let pid = PlayerId(1);
        let beside = Vec3::new(altar.x as f32 + 2.5, altar.y as f32, altar.z as f32 + 0.5);
        crate::application::ecs::systems::players::entry(&mut state.sim.ecs, pid, beside);
        let effigy = state.content.rules.items.find("stag_effigy").unwrap();
        let mut slots = vec![None; 4];
        slots[3] = Some(NetItemStack {
            item: effigy.0,
            count: 1,
            durability: None,
        });
        state.net.peers.inventories.insert(pid, (slots, 0));

        state.use_block(pid, altar);
        assert_eq!(bosses(&state), 1);
        assert_eq!(
            state.count_items(pid, effigy),
            0,
            "the host's copy is spent"
        );
        let guard = handle.lock();
        assert!(guard.messages_to(pid).iter().any(|m| matches!(
            m,
            ServerMessage::ConsumeItems { to, stacks } if *to == pid && stacks[0].item == effigy.0
        )));
    }

    #[test]
    fn felling_the_boss_records_it_and_shares_the_loot() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        give_local(&mut state, "stag_effigy", 1);
        use_altar(&mut state, altar);
        let boss = state.simulated_mobs().into_iter().find(|m| m.boss).unwrap();
        let at = boss.position;
        crate::application::ecs::systems::mobs::hit(
            &mut state.sim.ecs,
            boss.entity,
            10_000.0,
            Vec3::ZERO,
            0,
        );
        state.sim.player.teleport(at + Vec3::X * 3.0);
        state.update_mobs(1.0 / 60.0);

        assert_eq!(bosses(&state), 0);
        assert!(state.sim.progression.is_defeated("elder stag"));
        assert!(state.sim.mobs.fight.is_none());
        let antler = state.content.rules.items.find("elder_antler").unwrap();
        let dropped: u32 = state
            .sim
            .drops()
            .map(|(d, _)| d)
            .filter(|d| d.stack.item == antler)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert_eq!(dropped, 3, "the participant got the elder antlers");
    }

    /// Walk away and the boss leaves, taking the offering with it.
    #[test]
    fn an_abandoned_arena_leashes_the_boss() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        give_local(&mut state, "stag_effigy", 1);
        use_altar(&mut state, altar);
        state.sim.player.teleport(Vec3::new(5000.0, 120.0, 5000.0));
        for _ in 0..40 {
            state.update_boss_fight(1.0);
        }
        assert_eq!(bosses(&state), 0);
        assert!(state.sim.mobs.fight.is_none());
        assert!(!state.sim.progression.is_defeated("elder stag"));
    }

    #[test]
    fn the_bar_shows_near_the_boss_only() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        give_local(&mut state, "stag_effigy", 1);
        use_altar(&mut state, altar);
        let bar = state.boss_bar().expect("a bar beside the boss");
        assert_eq!(bar.title, "The Elder Stag");
        assert!((bar.fraction - 1.0).abs() < 1e-6);
        state.sim.player.teleport(Vec3::new(5000.0, 120.0, 5000.0));
        assert!(state.boss_bar().is_none());
    }

    #[test]
    fn loot_seeds_differ_per_participant() {
        assert_ne!(loot_seed(7, PlayerId(0)), loot_seed(7, PlayerId(1)));
    }
}

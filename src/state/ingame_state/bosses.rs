//! Boss fights from the state layer: summoning at an altar, landing the
//! attacks a boss's brain commits to, the leash, the kill, and what the HUD
//! shows about it all.
//!
//! The authority runs the fight — the brain lives on the simulated
//! [`Mob`](crate::entity::Mob) — and tells clients the rest through
//! `BossTelegraph`, `BossPhase` and `BossDefeated`. Kept out of `mobs.rs`,
//! which already carries every other mob.

use glam::Vec3;

use super::InGameState;
use super::block_use::UseAt;
use crate::core::{BlockPos, Rng64};
use crate::entity::boss::{AttackEffect, BossParams};
use crate::entity::{Arrow, Mob, MobId};
use crate::inventory::ItemStack;
use crate::net::{ChatKind, PlayerId, ServerMessage};
use crate::ui::boss_bar::BossBarView;

/// How far from its altar (blocks) a summoned boss appears.
const SUMMON_DISTANCE: f32 = 7.0;
/// Players within this multiple of a boss's arena see its bar.
const BAR_RANGE: f32 = 1.5;
/// Horizontal speed a slam flings the local player at, per point of knockback.
const SLAM_LIFT: f32 = 6.0;
/// Where volley projectiles leave the boss, ahead of its face.
const MUZZLE: f32 = 0.8;

/// The one fight a world may have going at a time.
pub(super) struct BossFight {
    pub mob: MobId,
    /// The altar it was summoned at — the centre of its arena.
    pub altar: BlockPos,
    /// Seconds the arena has stood empty.
    pub empty_for: f32,
}

/// An attack being wound up, as every peer shows it.
#[derive(Debug, Clone)]
pub(super) struct Telegraph {
    pub mob: u64,
    /// The attack's id, title-cased for display.
    pub name: String,
    pub remaining: f32,
}

/// One beat of a boss fight, collected during the mob tick and resolved once
/// the mobs are no longer borrowed.
pub(super) enum BossBeat {
    Windup {
        mob: MobId,
        attack: usize,
        seconds: f32,
    },
    Release {
        mob: MobId,
        attack: usize,
        /// Who it was fighting: `None` inside is the local player.
        target: Option<(Option<PlayerId>, Vec3)>,
    },
    Phase {
        mob: MobId,
        phase: u8,
    },
}

impl InGameState {
    /// Authority: `at.actor` offers at an altar that summons `boss`.
    pub(super) fn offer_at_altar(&mut self, at: &UseAt, boss: &str) {
        let Some(params) = self
            .content
            .entities
            .find(boss)
            .and_then(|kind| kind.boss.clone())
        else {
            log::warn!("altar at {:?} names {boss:?}, which is no boss", at.pos);
            return;
        };
        if self.mobs.fight.is_some() {
            let text = "The altar is cold — a battle already rages.".to_string();
            self.reply(at.actor, ChatKind::System, text);
            return;
        }
        let Some(item) = self.content.items.find(&params.offering.item) else {
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
            .find_ground(spot.x, spot.z, at.pos.y + 8)
            .unwrap_or(spot.y);
        let Some(mob) = self.spawn_mob(boss, Vec3::new(spot.x, ground, spot.z)) else {
            return;
        };
        self.mobs.fight = Some(BossFight {
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

    fn boss_mob(&self, id: MobId) -> Option<(&Mob, &BossParams)> {
        let mob = self.mobs.live.iter().find(|m| m.id == id)?;
        Some((mob, mob.boss()?))
    }

    fn telegraph(&mut self, id: MobId, attack: usize, seconds: f32) {
        let Some((_, params)) = self.boss_mob(id) else {
            return;
        };
        let attack_id = params.attacks[attack].id.clone();
        self.mobs.telegraph = Some(Telegraph {
            mob: id.0,
            name: crate::core::ident::title_case(&attack_id),
            remaining: seconds,
        });
        self.emit_mob_event(ServerMessage::BossTelegraph {
            id: id.0,
            attack: attack_id,
            windup: seconds,
        });
    }

    fn enter_phase(&mut self, id: MobId, phase: u8) {
        let Some((_, params)) = self.boss_mob(id) else {
            return;
        };
        let title = params.title.clone();
        self.emit_mob_event(ServerMessage::BossPhase { id: id.0, phase });
        if phase > 0 {
            self.announce(format!("{title} is enraged!"));
        }
    }

    /// Apply one attack's effect as it lands.
    fn land_attack(&mut self, id: MobId, attack: usize, target: Option<(Option<PlayerId>, Vec3)>) {
        let Some((mob, params)) = self.boss_mob(id) else {
            return;
        };
        let spec = params.attacks[attack].clone();
        let (position, eye) = (mob.position, mob.eye_position());
        if self.mobs.telegraph.as_ref().is_some_and(|t| t.mob == id.0) {
            self.mobs.telegraph = None;
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
                for t in self.mob_targets() {
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
                    self.emit_mob_event(ServerMessage::ArrowSpawned {
                        position: origin.to_array(),
                        velocity: velocity.to_array(),
                        gravity,
                        lifetime,
                    });
                    self.mobs
                        .arrows
                        .push(Arrow::new(origin, velocity, damage, gravity, lifetime));
                }
            }
            AttackEffect::Summon { entity, count, cap } => {
                let alive = self
                    .mobs
                    .live
                    .iter()
                    .filter(|m| m.kind_name == entity && m.position.distance(position) < 48.0)
                    .count() as u32;
                let mut rng = Rng64::new(self.world.seed() ^ id.0 ^ self.mobs.next_id);
                let want = rng.range_u32(u32::from(count[0]), u32::from(count[1]));
                for i in 0..want.min(cap.saturating_sub(alive)) {
                    let angle = i as f32 * 2.1 + rng.range_f32(0.0, 1.0);
                    let spot = position + Vec3::new(angle.cos(), 0.0, angle.sin()) * 3.0;
                    let ground = self
                        .find_ground(spot.x, spot.z, position.y as i32 + 6)
                        .unwrap_or(position.y);
                    self.spawn_mob(&entity, Vec3::new(spot.x, ground, spot.z));
                }
            }
        }
    }

    /// Damage a player — the local one directly (knocked back by `push`),
    /// a remote one through the wire (clients own their bodies).
    fn hit_player(&mut self, player: Option<PlayerId>, damage: f32, push: Vec3) {
        match player {
            None => {
                self.damage_local_player(damage);
                self.player.velocity += push;
            }
            Some(id) => self.emit_mob_event(ServerMessage::PlayerDamaged { id, amount: damage }),
        }
    }

    /// Authority, every frame: the leash, and the telegraph clock.
    pub(super) fn update_boss_fight(&mut self, dt: f32) {
        self.tick_telegraph(dt);
        let Some(fight) = &self.mobs.fight else {
            return;
        };
        let (id, altar) = (fight.mob, fight.altar);
        let Some((_, params)) = self.boss_mob(id) else {
            // Killed (the defeat already cleared the fight) or gone some other
            // way: either way nothing is left to leash.
            self.mobs.fight = None;
            return;
        };
        let radius = params.arena_radius;
        let leash = params.leash_seconds;
        let title = params.title.clone();
        let centre = Vec3::new(altar.x as f32 + 0.5, altar.y as f32, altar.z as f32 + 0.5);
        let occupied = self.fighters_near(centre, radius).next().is_some();
        let Some(fight) = &mut self.mobs.fight else {
            return;
        };
        fight.empty_for = if occupied { 0.0 } else { fight.empty_for + dt };
        if fight.empty_for < leash {
            return;
        }
        // Nobody stayed to fight: the boss returns whence it came, and takes
        // the offering with it.
        self.mobs.fight = None;
        self.mobs.live.retain(|m| m.id != id);
        self.emit_mob_event(ServerMessage::MobDespawned {
            id: id.0,
            killed_by: None,
        });
        self.announce(format!("{title} returns to the wilds."));
    }

    /// Players (local as `None`) within `radius` of `centre`, alive ones only.
    fn fighters_near(&self, centre: Vec3, radius: f32) -> impl Iterator<Item = Option<PlayerId>> {
        let flat = |p: Vec3| Vec3::new(p.x - centre.x, 0.0, p.z - centre.z).length();
        let local = (!self.dead && flat(self.player.position) <= radius).then_some(None);
        let remote: Vec<Option<PlayerId>> = self
            .peers
            .players
            .iter()
            .filter(|(_, p)| flat(p.position()) <= radius)
            .map(|(id, _)| Some(*id))
            .collect();
        local.into_iter().chain(remote)
    }

    fn tick_telegraph(&mut self, dt: f32) {
        if let Some(t) = &mut self.mobs.telegraph {
            t.remaining -= dt;
            if t.remaining <= 0.0 {
                self.mobs.telegraph = None;
            }
        }
    }

    /// Authority: a boss died. Record it, tell everyone, and hand out the
    /// loot to every player who was in the arena — not just the killer.
    pub(super) fn on_boss_defeated(&mut self, mob: &Mob) {
        let Some(params) = mob.boss() else {
            return;
        };
        let participants: Vec<Option<PlayerId>> = self
            .fighters_near(mob.position, params.arena_radius)
            .collect();
        let local_id = self.session.local_id();
        let wire: Vec<PlayerId> = participants.iter().map(|p| p.unwrap_or(local_id)).collect();
        self.emit_mob_event(ServerMessage::BossDefeated {
            id: mob.id.0,
            kind: mob.kind_name.clone(),
            position: mob.position.to_array(),
            participants: wire,
        });
        if participants.contains(&None) {
            self.pop_drops_for(&mob.kind_name, loot_seed(mob.id.0, local_id), mob.position);
        }
        let first = self.progression.defeat(&mob.kind_name);
        if self.mobs.fight.as_ref().is_some_and(|f| f.mob == mob.id) {
            self.mobs.fight = None;
        }
        if self
            .mobs
            .telegraph
            .as_ref()
            .is_some_and(|t| t.mob == mob.id.0)
        {
            self.mobs.telegraph = None;
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
        let local_id = self.session.local_id();
        match msg {
            ServerMessage::BossPhase { id, phase } => {
                if let Some(mob) = self.mobs.remote.get_mut(&id) {
                    mob.phase = phase;
                }
            }
            ServerMessage::BossTelegraph { id, attack, windup } => {
                self.mobs.telegraph = Some(Telegraph {
                    mob: id,
                    name: crate::core::ident::title_case(&attack),
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
                    self.pop_drops_for(&kind, loot_seed(id, local_id), at);
                }
                if self.mobs.telegraph.as_ref().is_some_and(|t| t.mob == id) {
                    self.mobs.telegraph = None;
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
        let here = self.player.position;
        let local = self.mobs.live.iter().filter_map(|m| {
            let params = m.boss()?;
            Some((
                m.id.0,
                m.position,
                params,
                m.health / m.params.max_health,
                m.boss_phase(),
            ))
        });
        let remote = self.mobs.remote.iter().filter_map(|(&id, m)| {
            let kind = self.content.entities.find(m.kind_name())?;
            let params = kind.boss.as_ref()?;
            let max = kind.mob.as_ref()?.max_health;
            Some((id, m.position(), params, m.health / max, m.phase))
        });
        let (id, _, params, fraction, phase) = local
            .chain(remote)
            .filter(|(_, pos, params, _, _)| pos.distance(here) <= params.arena_radius * BAR_RANGE)
            .min_by(|a, b| a.1.distance(here).total_cmp(&b.1.distance(here)))?;
        Some(BossBarView {
            title: params.title.clone(),
            fraction: fraction.clamp(0.0, 1.0),
            phase,
            telegraph: self
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
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::net::NetItemStack;
    use crate::state::session::FakeSession;
    use crate::world::block::blocks;
    use crate::world::structure::Cell;

    /// A survival world with the altar nearest spawn, and that altar block.
    fn at_the_altar() -> (InGameState, BlockPos) {
        let state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let config = state.structures.config();
        let index = config.find("meadows_altar").unwrap();
        let altar = state
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
        let id = state.content.items.find(item).unwrap();
        state.inventory.set_slot(0, Some(ItemStack::new(id, count)));
    }

    fn stand_beside(state: &mut InGameState, pos: BlockPos) {
        state.player.teleport(Vec3::new(
            pos.x as f32 + 2.5,
            pos.y as f32,
            pos.z as f32 + 0.5,
        ));
    }

    fn use_altar(state: &mut InGameState, pos: BlockPos) {
        let local = state.session.local_id();
        state.use_block(local, pos);
    }

    fn bosses(state: &InGameState) -> usize {
        state
            .mobs
            .live
            .iter()
            .filter(|m| m.boss().is_some())
            .count()
    }

    #[test]
    fn an_empty_handed_offering_summons_nothing() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 0);
        assert!(state.mobs.fight.is_none());
    }

    #[test]
    fn the_offering_is_taken_and_one_boss_answers() {
        let (mut state, altar) = at_the_altar();
        stand_beside(&mut state, altar);
        give_local(&mut state, "stag_effigy", 2);
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 1);
        let effigy = state.content.items.find("stag_effigy").unwrap();
        assert_eq!(state.inventory.count_of(effigy), 1, "exactly one offered");

        // A second offering while it lives is refused and costs nothing.
        use_altar(&mut state, altar);
        assert_eq!(bosses(&state), 1);
        assert_eq!(state.inventory.count_of(effigy), 1);
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
        state.peers.entry(pid, beside);
        let effigy = state.content.items.find("stag_effigy").unwrap();
        let mut slots = vec![None; 4];
        slots[3] = Some(NetItemStack {
            item: effigy.0,
            count: 1,
            durability: None,
        });
        state.peers.inventories.insert(pid, (slots, 0));

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
        let boss = state
            .mobs
            .live
            .iter_mut()
            .find(|m| m.boss().is_some())
            .unwrap();
        let at = boss.position;
        boss.damage(10_000.0, Vec3::ZERO);
        state.player.teleport(at + Vec3::X * 3.0);
        state.update_mobs(1.0 / 60.0);

        assert_eq!(bosses(&state), 0);
        assert!(state.progression.is_defeated("elder stag"));
        assert!(state.mobs.fight.is_none());
        let antler = state.content.items.find("elder_antler").unwrap();
        let dropped: u32 = state
            .drops
            .iter()
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
        state.player.teleport(Vec3::new(5000.0, 120.0, 5000.0));
        for _ in 0..40 {
            state.update_boss_fight(1.0);
        }
        assert_eq!(bosses(&state), 0);
        assert!(state.mobs.fight.is_none());
        assert!(!state.progression.is_defeated("elder stag"));
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
        state.player.teleport(Vec3::new(5000.0, 120.0, 5000.0));
        assert!(state.boss_bar().is_none());
    }

    #[test]
    fn loot_seeds_differ_per_participant() {
        assert_ne!(loot_seed(7, PlayerId(0)), loot_seed(7, PlayerId(1)));
    }
}

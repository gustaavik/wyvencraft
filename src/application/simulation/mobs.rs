//! Mob use cases: spawning, the per-frame tick, loot, arrows and the standing
//! spawn and despawn rules. Authority-side except where noted.

use glam::Vec3;
use wyven_net::PlayerId;

use super::Simulation;
use crate::application::ecs::components::{Kind, Mob, Transform};
use crate::application::ecs::systems::mobs::{self as mob_systems, MobStep, Reaped};
use crate::application::ecs::systems::{players, projectiles};
use crate::application::ecs::{Entity, With, spawn};
use crate::application::protocol::ServerMessage;
use crate::application::session::HOST_PLAYER_ID;
use crate::domain::core::{Aabb, BlockPos, Rng64};
use crate::domain::entity::{Health, ItemDrop, MobAction, MobId, PlayerSighting};
use crate::domain::inventory::{ItemStack, Tool};
use crate::domain::world::World;

/// Damage a bare-fisted player melee swing lands. A held tool overrides it with
/// its `[item.tool] damage` component; this is the floor everything falls back
/// to (see [`Simulation::melee_damage`]).
pub const PLAYER_ATTACK_DAMAGE: f32 = 2.0;
/// Knockback impulse a player hit imparts: horizontal shove + a small pop.
pub const KNOCKBACK_PUSH: f32 = 6.0;
pub const KNOCKBACK_LIFT: f32 = 3.0;

/// A player a mob could target, from the authority's point of view.
/// `player` is `None` for the authority's own (local) player.
pub struct MobTarget {
    pub player: Option<PlayerId>,
    pub eye: Vec3,
}

/// One beat of a boss fight, collected during the mob tick and resolved by
/// whoever runs the fight.
pub enum BossBeat {
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

/// What one mob tick decided, for [`Simulation::resolve_mob_attacks`] once
/// the boss beats have landed.
#[derive(Default)]
pub struct MobTick {
    pub beats: Vec<BossBeat>,
    /// Arrows loosed: (origin, velocity, damage, gravity, lifetime).
    pub fired: Vec<(Vec3, Vec3, f32, f32, f32)>,
    /// Melee blows: (whom, damage), `None` being the local player.
    pub melee: Vec<(Option<PlayerId>, f32)>,
}

impl Simulation {
    /// Spawn a mob of the named kind at `position` (feet). `None` when the
    /// kind is unknown or isn't a mob. The brain's random stream is seeded
    /// from the world seed and the mob id, so runs are reproducible.
    pub fn spawn_mob(&mut self, kind_name: &str, position: Vec3) -> Option<MobId> {
        let kind = self.rules.entities.find(kind_name)?;
        let id = MobId(self.mobs.next_id);
        let seed = self.world.seed() ^ id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        spawn::mob(&mut self.ecs, kind, id, position, seed)?;
        self.mobs.next_id += 1;
        self.emit(ServerMessage::MobSpawned {
            id: id.0,
            kind: kind_name.to_string(),
            position: position.to_array(),
        });
        Some(id)
    }

    /// Put back what a save remembered about mob `id`: its health (never
    /// above what its kind allows) and whether dawn should reap it.
    pub fn restore_mob(&mut self, id: MobId, health: f32, night_spawned: bool) {
        let Some(entity) = mob_systems::find(&self.ecs, id) else {
            return;
        };
        if let Some(current) = self.ecs.get_mut::<Health>(entity) {
            current.current = health.min(current.max);
        }
        if let Some(mob) = self.ecs.get_mut::<Mob>(entity) {
            mob.night_spawned = night_spawned;
        }
    }

    /// Every player a mob may attack right now: the local player (unless dead
    /// or in a protected mode) plus survival-mode remote players.
    pub fn mob_targets(&self) -> Vec<MobTarget> {
        let mut targets = Vec::new();
        if !self.dead && self.player.mode.takes_damage() {
            targets.push(MobTarget {
                player: None,
                eye: self.player.eye_position(),
            });
        }
        let eye_height = self
            .rules
            .entities
            .player()
            .movement
            .map(|m| m.eye_height)
            .unwrap_or(1.62);
        for remote in players::all(&self.ecs) {
            if remote.mode.takes_damage() {
                targets.push(MobTarget {
                    player: Some(remote.id),
                    eye: remote.position() + Vec3::new(0.0, eye_height, 0.0),
                });
            }
        }
        targets
    }

    /// Advance every mob one step (authority only): build each mob's
    /// perception of the nearest attackable player, run brain + physics, and
    /// collect what they commit to. Nothing is resolved yet — see
    /// [`MobTick`].
    pub fn tick_mobs(&mut self, dt: f32) -> MobTick {
        let targets = self.mob_targets();
        let world = &self.world;
        let steps = mob_systems::simulate(
            &mut self.ecs,
            dt,
            // Mobs straddling the streaming edge freeze until their chunk is
            // back (unloaded chunks read as solid, which would trap them).
            |feet| world.is_loaded(BlockPos::from_world(feet).chunk()),
            |eye| nearest_sighting(world, &targets, eye),
            |p| world.is_solid_for_collision(p),
        );

        let mut tick = MobTick::default();
        for MobStep {
            entity,
            id,
            action,
            phase_change,
            target,
            eye,
        } in steps
        {
            let target = target.map(|index| &targets[index]);
            if let Some(phase) = phase_change {
                tick.beats.push(BossBeat::Phase { mob: id, phase });
            }
            match action {
                MobAction::None => {}
                MobAction::Melee { damage } => {
                    tick.melee.push((target.and_then(|t| t.player), damage));
                }
                MobAction::Fire { velocity, damage } => {
                    let ranged = self
                        .ecs
                        .get::<Mob>(entity)
                        .and_then(|mob| mob.params.ranged)
                        .expect("ranged kinds fire");
                    tick.fired.push((
                        // Nock ahead of the face so the arrow clears the model.
                        eye + velocity.normalize_or_zero() * 0.4,
                        velocity,
                        damage,
                        ranged.projectile_gravity,
                        ranged.lifetime,
                    ));
                }
                MobAction::BossWindup { attack, seconds } => {
                    tick.beats.push(BossBeat::Windup {
                        mob: id,
                        attack,
                        seconds,
                    });
                }
                MobAction::BossRelease { attack } => {
                    tick.beats.push(BossBeat::Release {
                        mob: id,
                        attack,
                        target: target.map(|t| (t.player, t.eye)),
                    });
                }
            }
        }
        tick
    }

    /// Loose the arrows and land the blows a mob tick committed to.
    pub fn resolve_mob_attacks(
        &mut self,
        fired: Vec<(Vec3, Vec3, f32, f32, f32)>,
        melee: Vec<(Option<PlayerId>, f32)>,
    ) {
        for (origin, velocity, damage, gravity, lifetime) in fired {
            self.emit(ServerMessage::ArrowSpawned {
                position: origin.to_array(),
                velocity: velocity.to_array(),
                gravity,
                lifetime,
            });
            spawn::arrow(&mut self.ecs, origin, velocity, damage, gravity, lifetime);
        }
        for (target, damage) in melee {
            match target {
                None => self.damage_local_player(damage),
                Some(id) => self.emit(ServerMessage::PlayerDamaged { id, amount: damage }),
            }
        }
    }

    /// Settle an ordinary mob's death: tell everyone, and — if this peer made
    /// the kill — roll its loot. Drops are per-peer local, like block drops: the
    /// host rolls for its own kills; a client killer learns via `MobDespawned {
    /// killed_by }` and rolls the identical table itself.
    pub fn settle_kill(&mut self, mob: Reaped) {
        let Reaped {
            id,
            kind,
            position,
            last_attacker,
            ..
        } = mob;
        let killed_by = last_attacker.map(PlayerId);
        self.emit(ServerMessage::MobDespawned {
            id: id.0,
            killed_by,
        });
        if killed_by.is_none_or(|pid| pid == HOST_PLAYER_ID) {
            self.pop_loot(&kind, id.0, position);
        }
    }

    /// Roll a kind's drop table and pop the loot as dropped items at
    /// `position` (a dead mob's feet). Deterministic per (world seed, `seed`)
    /// — the host and the killing client roll identical loot without it ever
    /// crossing the wire. Unknown item names are skipped with a warning.
    pub fn pop_loot(&mut self, kind_name: &str, seed: u64, position: Vec3) {
        let Some(kind) = self.rules.entities.find(kind_name) else {
            return;
        };
        let Some(params) = kind.mob.clone() else {
            return;
        };
        let mut rng = Rng64::new(self.world.seed() ^ seed.wrapping_mul(0xA24B_AED4_963E_E407));
        let block = BlockPos::from_world(position + Vec3::Y * 0.25);
        for drop in &params.drops {
            let Some(item) = self.rules.items.find(&drop.item) else {
                log::warn!(
                    "mob {kind_name:?} drop references unknown item {:?}; skipping",
                    drop.item
                );
                continue;
            };
            let count = rng.range_u32(drop.min.into(), drop.max.into());
            if count == 0 {
                continue;
            }
            let stack = ItemStack::new(item, (count as u8).min(self.rules.items.max_stack(item)));
            let angle = self.clock * 9.73 + rng.range_f32(0.0, std::f32::consts::TAU);
            let drop =
                ItemDrop::block_drop(stack, block, angle, self.rules.entities.dropped_item());
            self.spawn_drop(drop);
        }
    }

    /// Damage the local player's swing lands, from whatever is in the selected
    /// hotbar slot. A tool without a `damage` component — a pickaxe, a shovel —
    /// hits exactly as hard as a bare fist.
    pub fn melee_damage(&self) -> f32 {
        self.inventory
            .item_in_selected()
            .and_then(|id| self.rules.items.component::<Tool>(id))
            .and_then(|tool| tool.damage)
            .unwrap_or(PLAYER_ATTACK_DAMAGE)
    }

    /// Advance arrows on every peer (they're visual off the authority); the
    /// authority alone hit-tests players and applies damage.
    pub fn fly_arrows(&mut self, dt: f32) {
        let authority = self.authoritative;
        let local_box = (authority && !self.dead && self.player.mode.takes_damage())
            .then(|| self.player.aabb());
        let player_kind = self.rules.entities.player().physics;
        let remote_boxes: Vec<(PlayerId, Aabb)> = if authority {
            players::all(&self.ecs)
                .filter(|rp| rp.mode.takes_damage())
                .map(|rp| {
                    let half = player_kind.width * 0.5;
                    let feet = rp.position();
                    (
                        rp.id,
                        Aabb::new(
                            feet - Vec3::new(half, 0.0, half),
                            feet + Vec3::new(half, player_kind.height, half),
                        ),
                    )
                })
                .collect()
        } else {
            Vec::new()
        };

        // The local player first: where boxes overlap, it takes the hit.
        let targets: Vec<(Option<PlayerId>, Aabb)> = local_box
            .map(|b| (None, b))
            .into_iter()
            .chain(remote_boxes.into_iter().map(|(id, b)| (Some(id), b)))
            .collect();
        let world = &self.world;
        let hits = projectiles::fly(
            &mut self.ecs,
            dt,
            |p| world.is_solid_for_collision(p),
            &targets,
        );
        for (target, damage) in hits {
            match target {
                None => self.damage_local_player(damage),
                Some(_id) => {
                    // Remote players: damaged via the net layer (sync phase).
                }
            }
        }
    }

    /// The walkable surface at `(x, z)` (see [`ground_at`]).
    pub fn find_ground(&self, x: f32, z: f32, top: i32) -> Option<f32> {
        ground_at(&self.world, x, z, top)
    }

    /// Periodic mob spawning + the standing despawn rules (authority only).
    /// The planner is pure ([`crate::domain::entity::Spawner::tick`]); this
    /// method feeds it the live world and applies its plan.
    pub fn update_spawning(&mut self, dt: f32) {
        let cfg = self.rules.spawning.clone();
        let mut anchors = vec![self.player.position];
        anchors.extend(players::all(&self.ecs).map(|r| r.position()));
        let is_night = self.day_cycle.is_night();
        // Cap the surface search near player height: caves far below the
        // surface aren't valid spawn floors for surface mobs (and there's no
        // light level to gate on yet).
        let top = (self.player.position.y + 24.0) as i32;

        let world = &self.world;
        let terrain = self.structures.terrain();
        let ecs = &self.ecs;
        let requests = self.mobs.spawner.tick(
            &cfg,
            dt,
            is_night,
            &anchors,
            ecs.count::<Mob>(),
            |name| {
                ecs.query::<(&Kind, With<Mob>)>()
                    .filter(|(_, (kind, ()))| kind.name == name)
                    .count()
            },
            |x, z| ground_at(world, x, z, top),
            |x, z| terrain.biome_at(x.floor() as i32, z.floor() as i32),
        );
        for request in requests {
            if let Some(id) = self.spawn_mob(&request.entity, request.position) {
                let night_rule = cfg
                    .entry(&request.entity)
                    .is_some_and(|e| e.despawn_in_daylight);
                if night_rule
                    && let Some(entity) = mob_systems::find(&self.ecs, id)
                    && let Some(mob) = self.ecs.get_mut::<Mob>(entity)
                {
                    mob.night_spawned = true;
                }
                log::debug!(
                    "spawned {:?} at {:.0?}",
                    request.entity,
                    request.position.to_array()
                );
            }
        }

        // Standing despawn rules: strays beyond the despawn ring, and
        // night-rule mobs caught out in daylight. Two passes so each removal
        // also tells clients (`MobDespawned` without a killer = no loot).
        let day = !is_night;
        let despawn_sq = cfg.limits.despawn_distance * cfg.limits.despawn_distance;
        let leaving: Vec<(Entity, MobId)> = self
            .ecs
            .query::<(&Mob, &Transform, &MobId)>()
            .filter(|(_, (mob, transform, _))| {
                let stray = anchors
                    .iter()
                    .all(|a| (transform.position - *a).length_squared() > despawn_sq);
                stray || (day && mob.night_spawned)
            })
            .map(|(entity, (_, _, id))| (entity, *id))
            .collect();
        for (entity, id) in leaving {
            self.ecs.despawn(entity);
            self.emit(ServerMessage::MobDespawned {
                id: id.0,
                killed_by: None,
            });
        }
    }
}

/// The nearest of `targets` to a mob's `eye`, as an index into `targets` and
/// what the mob sees of it.
///
/// Line of sight is solid blocks only, and always the whole cell: sight is
/// about what blocks it, so ground cover you can walk through must not hide a
/// player from a mob.
fn nearest_sighting(
    world: &World,
    targets: &[MobTarget],
    eye: Vec3,
) -> Option<(usize, PlayerSighting)> {
    let (index, distance) = targets
        .iter()
        .enumerate()
        .map(|(i, t)| (i, (t.eye - eye).length()))
        .min_by(|a, b| a.1.total_cmp(&b.1))?;
    let offset = targets[index].eye - eye;
    let visible = distance < 1.0e-3
        || crate::domain::world::raycast(eye, offset / distance, distance, |p| {
            world
                .is_solid(p)
                .then_some(crate::domain::world::Target::Cell)
        })
        .is_none();
    Some((
        index,
        PlayerSighting {
            offset,
            distance,
            visible,
        },
    ))
}

/// The walkable *surface* at `(x, z)`: the Y of the first space with solid
/// ground below, two air blocks above, and open sky the rest of the way up,
/// scanning down from `top`. `None` when the column's chunk is unloaded or
/// has no such spot. The rules this encodes:
/// - non-air, non-solid blocks (water) are not clear → no spawning in fluids;
/// - the sky requirement rejects caves and overhangs, which the plain
///   top-down scan would otherwise fall into under oceans and mountains
///   (there is no per-block light to gate on, so sky access is the proxy).
pub fn ground_at(world: &World, x: f32, z: f32, top: i32) -> Option<f32> {
    let column = BlockPos::new(x.floor() as i32, 0, z.floor() as i32);
    if !world.is_loaded(column.chunk()) {
        return None;
    }
    let clear = |y: i32| {
        world
            .block_at(BlockPos::new(column.x, y, column.z))
            .is_air()
    };
    let y = (1..=top.min(crate::domain::core::CHUNK_HEIGHT - 2))
        .rev()
        .find(|&y| {
            world.is_solid(BlockPos::new(column.x, y - 1, column.z)) && clear(y) && clear(y + 1)
        })?;
    let open_sky = (y + 2..crate::domain::core::CHUNK_HEIGHT)
        .all(|yy| !world.is_solid(BlockPos::new(column.x, yy, column.z)));
    open_sky.then_some(y as f32)
}

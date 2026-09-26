//! Mob simulation from the state layer: spawning, per-frame perception +
//! updates, and applying the attacks mobs commit to.
//!
//! Only the authority (singleplayer/host) runs this — mirroring the fluid
//! sim, `frame.rs` gates the tick on the session's authority. Clients hold
//! interpolated replicas fed by the host instead of simulating.

use glam::Vec3;

use super::{HOST_PLAYER_ID, InGameState};
use crate::application::ecs::components::{Body, Kind, Mob, Replica, Transform};
use crate::application::ecs::systems::mobs::{self as mob_systems, MobStep, Reaped};
use crate::application::ecs::systems::players;
use crate::application::ecs::{Entity, With, spawn};
use crate::domain::core::{Aabb, BlockPos, Rng64};
use crate::domain::entity::kind::VisualSpec;
use crate::domain::entity::{MobAction, MobId, PlayerSighting};
use crate::domain::inventory::{ItemStack, Tool};
use crate::infrastructure::net::{Channel, ClientMessage, PlayerId, ServerMessage};
use crate::presentation::art::{mobskin, skin};
use crate::presentation::render::rigged;
use crate::presentation::render::{HumanoidModel, QuadrupedModel};
use wyven_model::{ModelId, ModelRegistry};
use wyven_render::CpuMesh;

/// Zombie shamble: both arms held straight out (≈ 80° forward of hanging).
///
/// Positive is *forward* — the arm pivots at the shoulder, so a positive angle
/// carries the fist toward the model's front (-Z). See `rot_x` in
/// [`crate::presentation::render::model`], which every arm pose is written against.
const ARMS_FORWARD_ANGLE: f32 = 1.4;
/// Damage a bare-fisted player melee swing lands. A held tool overrides it with
/// its `[item.tool] damage` component; this is the floor everything falls back
/// to (see `InGameState::melee_damage`).
pub(super) const PLAYER_ATTACK_DAMAGE: f32 = 2.0;
/// Knockback impulse a player hit imparts: horizontal shove + a small pop.
pub(super) const KNOCKBACK_PUSH: f32 = 6.0;
pub(super) const KNOCKBACK_LIFT: f32 = 3.0;
/// Reach the host accepts for a client's `Attack` (their reach plus lag slack).
const ATTACK_VALIDATE_RANGE: f32 = 7.0;

/// A player a mob could target, from the authority's point of view.
/// `player` is `None` for the authority's own (local) player.
pub(super) struct MobTarget {
    pub player: Option<PlayerId>,
    pub eye: Vec3,
}

/// Renderable geometry for one entity, and which texture draws it.
pub(super) struct VisualMesh {
    pub mesh: CpuMesh,
    /// `None` means the shared block atlas (box models sampling skin sheets);
    /// `Some` means the model brings its own texture.
    pub model: Option<ModelId>,
}

/// Build the render mesh for a mob visual at `position` facing `yaw` (`None`
/// for visuals with no model). Shared by simulated mobs and client replicas.
pub(super) fn mob_mesh(
    visual: &VisualSpec,
    position: Vec3,
    yaw: f32,
    pose: &crate::domain::entity::Pose,
    models: &ModelRegistry,
) -> Option<VisualMesh> {
    let atlas = |mesh| Some(VisualMesh { mesh, model: None });
    match visual {
        VisualSpec::Humanoid(v) => {
            let origin = v
                .skin
                .as_deref()
                .and_then(mobskin::origin_for)
                .unwrap_or(skin::SKIN_ORIGIN);
            let mut pose = *pose;
            if v.arms_forward {
                pose.left_arm = ARMS_FORWARD_ANGLE;
                pose.right_arm = ARMS_FORWARD_ANGLE;
            }
            atlas(HumanoidModel::player().build_mesh_sheet(position, yaw, &pose, origin))
        }
        VisualSpec::Quadruped(v) => {
            let origin = mobskin::origin_for(&v.skin).unwrap_or(skin::SKIN_ORIGIN);
            atlas(QuadrupedModel::new(v).build_mesh(position, yaw, pose, origin))
        }
        VisualSpec::Model(spec) => {
            // A model that failed to load leaves the entity invisible rather
            // than crashing the frame; `ModelRegistry::load` already warned.
            let id = models.find(&spec.path)?;
            let model = models.get(id)?;
            Some(VisualMesh {
                mesh: model.mesh.to_cpu_mesh(
                    position,
                    yaw,
                    spec.scale,
                    spec.rotation(),
                    spec.offset(),
                ),
                model: Some(id),
            })
        }
        VisualSpec::Rigged(v) => {
            // A model that failed to load leaves the entity invisible rather
            // than crashing the frame; `ModelRegistry::load` already warned.
            let model = models.get(models.find(&v.path)?)?;
            let origin = v
                .skin
                .as_deref()
                .and_then(mobskin::origin_for)
                .unwrap_or(skin::SKIN_ORIGIN);
            // Mobs are drawn in the rest pose: the clip layer lives with the
            // player, which is the only rigged entity so far. Giving a mob its
            // clips means threading its `AnimationState` in here and binding a
            // `HumanoidRig` per model — the geometry path below is already the
            // one the player uses.
            atlas(rigged::bake_rest(model, v.scale, origin, position, yaw)?)
        }
        VisualSpec::ItemCube(_) => None,
    }
}

/// Whether an attacker at `attacker` plausibly reaches a mob at `mob` (the
/// host's lag-tolerant validation of a client `Attack`).
pub(super) fn attack_in_range(attacker: Vec3, mob: Vec3) -> bool {
    (mob - attacker).length() <= ATTACK_VALIDATE_RANGE
}

/// What the crosshair is on: one of the authority's own mobs, or a replica's
/// wire id on a client.
pub(super) enum MobTargetRef {
    Local(Entity),
    Remote(u64),
}

impl InGameState {
    /// Queue a mob event for the host broadcast (dropped outside hosting; a
    /// singleplayer session has no listeners and clients never emit).
    pub(super) fn emit_mob_event(&mut self, msg: ServerMessage) {
        if self.net.session.serves_peers() {
            self.net.peers.mob_events.push(msg);
        }
    }

    /// Spawn a mob of the named kind at `position` (feet). `None` when the
    /// kind is unknown or isn't a mob. The brain's random stream is seeded
    /// from the world seed and the mob id, so runs are reproducible.
    pub(super) fn spawn_mob(&mut self, kind_name: &str, position: Vec3) -> Option<MobId> {
        let kind = self.content.rules.entities.find(kind_name)?;
        let id = MobId(self.sim.mobs.next_id);
        let seed = self.sim.world.seed() ^ id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        spawn::mob(&mut self.sim.ecs, kind, id, position, seed)?;
        self.sim.mobs.next_id += 1;
        self.emit_mob_event(ServerMessage::MobSpawned {
            id: id.0,
            kind: kind_name.to_string(),
            position: position.to_array(),
        });
        Some(id)
    }

    /// Put back what a save remembered about mob `id`: its health (never
    /// above what its kind allows) and whether dawn should reap it.
    pub(super) fn restore_mob(&mut self, id: MobId, health: f32, night_spawned: bool) {
        let Some(entity) = mob_systems::find(&self.sim.ecs, id) else {
            return;
        };
        if let Some(current) = self
            .sim
            .ecs
            .get_mut::<crate::domain::entity::Health>(entity)
        {
            current.current = health.min(current.max);
        }
        if let Some(mob) = self.sim.ecs.get_mut::<Mob>(entity) {
            mob.night_spawned = night_spawned;
        }
    }

    /// Every player a mob may attack right now: the local player (unless dead
    /// or in a protected mode) plus survival-mode remote players.
    pub(super) fn mob_targets(&self) -> Vec<MobTarget> {
        let mut targets = Vec::new();
        if !self.sim.dead && self.sim.player.mode.takes_damage() {
            targets.push(MobTarget {
                player: None,
                eye: self.sim.player.eye_position(),
            });
        }
        let eye_height = self
            .content
            .rules
            .entities
            .player()
            .movement
            .map(|m| m.eye_height)
            .unwrap_or(1.62);
        for remote in players::all(&self.sim.ecs) {
            let id = &remote.id;
            if remote.mode.takes_damage() {
                targets.push(MobTarget {
                    player: Some(*id),
                    eye: remote.position() + Vec3::new(0.0, eye_height, 0.0),
                });
            }
        }
        targets
    }

    /// Advance every mob one step (authority only): build each mob's
    /// perception of the nearest attackable player, run brain + physics, and
    /// resolve the attacks they commit to.
    pub(super) fn update_mobs(&mut self, dt: f32) {
        let targets = self.mob_targets();
        let world = &self.sim.world;
        let steps = mob_systems::simulate(
            &mut self.sim.ecs,
            dt,
            // Mobs straddling the streaming edge freeze until their chunk is
            // back (unloaded chunks read as solid, which would trap them).
            |feet| world.is_loaded(BlockPos::from_world(feet).chunk()),
            |eye| nearest_sighting(world, &targets, eye),
            |p| world.is_solid_for_collision(p),
        );

        let mut melee_hits: Vec<(Option<PlayerId>, f32)> = Vec::new();
        // Arrows launched this tick: (origin eye, velocity, damage, gravity, lifetime).
        let mut fired: Vec<(Vec3, Vec3, f32, f32, f32)> = Vec::new();
        let mut beats: Vec<super::bosses::BossBeat> = Vec::new();
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
                beats.push(super::bosses::BossBeat::Phase { mob: id, phase });
            }
            match action {
                MobAction::None => {}
                MobAction::Melee { damage } => {
                    melee_hits.push((target.and_then(|t| t.player), damage));
                }
                MobAction::Fire { velocity, damage } => {
                    let ranged = self
                        .sim
                        .ecs
                        .get::<Mob>(entity)
                        .and_then(|mob| mob.params.ranged)
                        .expect("ranged kinds fire");
                    fired.push((
                        // Nock ahead of the face so the arrow clears the model.
                        eye + velocity.normalize_or_zero() * 0.4,
                        velocity,
                        damage,
                        ranged.projectile_gravity,
                        ranged.lifetime,
                    ));
                }
                MobAction::BossWindup { attack, seconds } => {
                    beats.push(super::bosses::BossBeat::Windup {
                        mob: id,
                        attack,
                        seconds,
                    });
                }
                MobAction::BossRelease { attack } => {
                    beats.push(super::bosses::BossBeat::Release {
                        mob: id,
                        attack,
                        target: target.map(|t| (t.player, t.eye)),
                    });
                }
            }
        }
        self.land_boss_beats(beats);

        for (origin, velocity, damage, gravity, lifetime) in fired {
            self.emit_mob_event(ServerMessage::ArrowSpawned {
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

        for (target, damage) in melee_hits {
            match target {
                None => self.damage_local_player(damage),
                Some(id) => {
                    self.emit_mob_event(ServerMessage::PlayerDamaged { id, amount: damage })
                }
            }
        }

        self.reap_dead_mobs();
    }

    /// Remove dead mobs, credit their killer, and pop loot. The killing
    /// peer's side spawns the drops (drops are per-peer local, like block
    /// drops): the host rolls for its own kills; a client killer learns via
    /// `MobDespawned { killed_by }` and rolls the identical table itself.
    fn reap_dead_mobs(&mut self) {
        for mob in mob_systems::reap(&mut self.sim.ecs) {
            if mob.boss.is_some() {
                // Everyone in the arena shares a boss's loot, so it is handed
                // out by the defeat rather than by kill credit. The defeat
                // carries the position, so it does not matter which of the
                // two messages a client's unordered channel delivers first.
                self.on_boss_defeated(&mob);
                self.emit_mob_event(ServerMessage::MobDespawned {
                    id: mob.id.0,
                    killed_by: None,
                });
                continue;
            }
            let Reaped {
                id,
                kind,
                position,
                last_attacker,
                ..
            } = mob;
            let killed_by = last_attacker.map(PlayerId);
            self.emit_mob_event(ServerMessage::MobDespawned {
                id: id.0,
                killed_by,
            });
            if killed_by.is_none_or(|pid| pid == HOST_PLAYER_ID) {
                self.pop_drops_for(&kind, id.0, position);
            }
        }
    }

    /// Roll a kind's drop table and pop the loot as dropped items at
    /// `position` (a dead mob's feet). Deterministic per (world seed, mob id)
    /// — the host and the killing client roll identical loot without it ever
    /// crossing the wire. Unknown item names are skipped with a warning.
    pub(super) fn pop_drops_for(&mut self, kind_name: &str, mob_id: u64, position: Vec3) {
        let Some(kind) = self.content.rules.entities.find(kind_name) else {
            return;
        };
        let Some(params) = kind.mob.clone() else {
            return;
        };
        let mut rng =
            Rng64::new(self.sim.world.seed() ^ mob_id.wrapping_mul(0xA24B_AED4_963E_E407));
        let block = BlockPos::from_world(position + Vec3::Y * 0.25);
        for drop in &params.drops {
            let Some(item) = self.content.rules.items.find(&drop.item) else {
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
            let stack = ItemStack::new(
                item,
                (count as u8).min(self.content.rules.items.max_stack(item)),
            );
            let angle = self.view.elapsed * 9.73 + rng.range_f32(0.0, std::f32::consts::TAU);
            let drop = crate::domain::entity::ItemDrop::block_drop(
                stack,
                block,
                angle,
                self.content.rules.entities.dropped_item(),
            );
            self.spawn_drop(drop);
        }
    }

    /// Damage the local player's swing lands, from whatever is in the selected
    /// hotbar slot. A tool without a `damage` component — a pickaxe, a shovel —
    /// hits exactly as hard as a bare fist.
    pub(super) fn melee_damage(&self) -> f32 {
        self.sim
            .inventory
            .item_in_selected()
            .and_then(|id| self.content.rules.items.component::<Tool>(id))
            .and_then(|tool| tool.damage)
            .unwrap_or(PLAYER_ATTACK_DAMAGE)
    }

    /// The mob under the crosshair within melee reach, if any — and only if
    /// no solid block is closer (no punching mobs through walls). The
    /// authority scans its own mobs; a client scans its replicas.
    pub(super) fn targeted_mob(&self) -> Option<MobTargetRef> {
        let eye = self.sim.player.eye_position();
        let look = self.sim.player.look_direction();
        let reach = self.sim.player.movement().reach;
        // Cap the ray at the targeted block, so the block face wins ties.
        let max_t = self
            .targeted_block()
            .and_then(|hit| {
                Aabb::block(Vec3::new(
                    hit.block.x as f32,
                    hit.block.y as f32,
                    hit.block.z as f32,
                ))
                .ray_hit(eye, look, reach)
            })
            .unwrap_or(reach);
        let under = |(entity, (transform, body, id)): (Entity, (&Transform, &Body, &MobId))| {
            Some((
                entity,
                *id,
                body.aabb(transform.position).ray_hit(eye, look, max_t)?,
            ))
        };
        let nearest = |a: &(Entity, MobId, f32), b: &(Entity, MobId, f32)| a.2.total_cmp(&b.2);
        if !self.net.session.is_authority() {
            self.sim
                .ecs
                .query::<(&Transform, &Body, &MobId, With<Replica>)>()
                .map(|(e, (t, b, id, ()))| (e, (t, b, id)))
                .filter_map(under)
                .min_by(nearest)
                .map(|(_, id, _)| MobTargetRef::Remote(id.0))
        } else {
            self.sim
                .ecs
                .query::<(&Transform, &Body, &MobId, With<Mob>)>()
                .map(|(e, (t, b, id, ()))| (e, (t, b, id)))
                .filter_map(under)
                .min_by(nearest)
                .map(|(entity, _, _)| MobTargetRef::Local(entity))
        }
    }

    /// Land a player melee swing on the targeted mob. The authority applies
    /// it directly (with kill credit to itself); a client sends `Attack` and
    /// the host validates and applies.
    pub(super) fn attack_mob(&mut self, target: MobTargetRef) {
        match target {
            MobTargetRef::Local(entity) => {
                let look = self.sim.player.look_direction();
                let push = Vec3::new(look.x, 0.0, look.z).normalize_or_zero() * KNOCKBACK_PUSH
                    + Vec3::Y * KNOCKBACK_LIFT;
                let damage = self.melee_damage();
                let id = self.sim.ecs.get::<MobId>(entity).copied();
                if let Some(id) = id
                    && let Some(health) =
                        mob_systems::hit(&mut self.sim.ecs, entity, damage, push, HOST_PLAYER_ID.0)
                {
                    self.emit_mob_event(ServerMessage::MobHurt { id: id.0, health });
                }
            }
            MobTargetRef::Remote(id) => {
                // Only the host may apply damage; ask it to.
                self.net
                    .session
                    .request(&ClientMessage::Attack { id }, Channel::Reliable);
            }
        }
    }

    /// Advance arrows on every peer (they're visual off the authority); the
    /// authority alone hit-tests players and applies damage.
    pub(super) fn update_arrows(&mut self, dt: f32) {
        let authority = self.net.session.is_authority();
        let local_box = (authority && !self.sim.dead && self.sim.player.mode.takes_damage())
            .then(|| self.sim.player.aabb());
        let player_kind = self.content.rules.entities.player().physics;
        let remote_boxes: Vec<(PlayerId, Aabb)> = if authority {
            players::all(&self.sim.ecs)
                .filter(|rp| rp.mode.takes_damage())
                .map(|rp| {
                    let id = &rp.id;
                    let half = player_kind.width * 0.5;
                    let feet = rp.position();
                    (
                        *id,
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
        let world = &self.sim.world;
        let hits = crate::application::ecs::systems::projectiles::fly(
            &mut self.sim.ecs,
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
    pub(super) fn find_ground(&self, x: f32, z: f32, top: i32) -> Option<f32> {
        ground_at(&self.sim.world, x, z, top)
    }

    /// Periodic mob spawning + the standing despawn rules (authority only).
    /// The planner is pure ([`crate::domain::entity::Spawner::tick`]); this method
    /// feeds it the live world and applies its plan.
    pub(super) fn update_spawning(&mut self, dt: f32) {
        let cfg = self.content.rules.spawning.clone();
        let mut anchors = vec![self.sim.player.position];
        anchors.extend(players::all(&self.sim.ecs).map(|r| r.position()));
        let is_night = self.sim.day_cycle.is_night();
        // Cap the surface search near player height: caves far below the
        // surface aren't valid spawn floors for surface mobs (and there's no
        // light level to gate on yet).
        let top = (self.sim.player.position.y + 24.0) as i32;

        let world = &self.sim.world;
        let terrain = self.sim.structures.terrain();
        let ecs = &self.sim.ecs;
        let requests = self.sim.mobs.spawner.tick(
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
                    && let Some(entity) = mob_systems::find(&self.sim.ecs, id)
                    && let Some(mob) = self.sim.ecs.get_mut::<Mob>(entity)
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
            .sim
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
            self.sim.ecs.despawn(entity);
            self.emit_mob_event(ServerMessage::MobDespawned {
                id: id.0,
                killed_by: None,
            });
        }
    }

    /// Dev hook (`WYVEN_DEBUG_SPAWN=cow,zombie,...`): spawn the named kinds in a
    /// line near the player right after entering the world, for visual checks
    /// without waiting on the spawner. Replaced by real spawning rules.
    pub(super) fn debug_spawn_from_env(&mut self) {
        let Ok(kinds) = std::env::var("WYVEN_DEBUG_SPAWN") else {
            return;
        };
        // Beside the player, not the spawn, so it composes with
        // `WYVEN_DEBUG_GOTO` (which may have moved them to a far structure).
        let near = self.sim.player.position;
        for (i, kind) in kinds.split(',').map(str::trim).enumerate() {
            let x = near.x + 3.0 + 2.0 * i as f32;
            let z = near.z + 3.0;
            let y = self
                .find_ground(x, z, crate::domain::core::CHUNK_HEIGHT - 2)
                .unwrap_or(near.y);
            match self.spawn_mob(kind, Vec3::new(x, y, z)) {
                Some(id) => log::info!("debug-spawned {kind:?} as {id:?} at ({x}, {y}, {z})"),
                None => log::warn!("WYVEN_DEBUG_SPAWN: unknown mob kind {kind:?}"),
            }
        }
    }

    /// Route damage to the authority's own player: armor mitigates inside
    /// `Player::damage`, worn pieces take wear, and death freezes control.
    pub(super) fn damage_local_player(&mut self, amount: f32) {
        let before = self.sim.player.health;
        self.sim.player.damage(amount);
        if self.sim.player.health < before {
            self.sim.inventory.wear_armor(1);
        }
        if self.sim.player.is_dead() && !self.sim.dead {
            self.sim.dead = true;
            self.sim.breaking = None;
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
    world: &crate::domain::world::World,
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
fn ground_at(world: &crate::domain::world::World, x: f32, z: f32, top: i32) -> Option<f32> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entity::Pose;

    /// The shamble has to reach *at* you. The sign is easy to get backwards and
    /// nothing else would catch it — a zombie with its arms behind it still
    /// walks, chases and hits exactly the same.
    ///
    /// Read off the built mesh rather than off a hand anchor: the player's hand
    /// moved to the rigged model, and a box-model mob has no anchor of its own.
    #[test]
    fn the_shamble_holds_the_arms_out_in_front() {
        let model = HumanoidModel::player();
        let sheet = crate::presentation::art::skin::SKIN_ORIGIN;
        let forward = |pose: &Pose| {
            // The model faces -Z, so "reaching" is the most negative z the arm
            // geometry gets to. Arms are the third and fourth parts, pushed as
            // base+overlay boxes of 24 vertices each after head and body.
            model
                .build_mesh_sheet(Vec3::ZERO, 0.0, pose, sheet)
                .vertices
                .iter()
                .skip(4 * 24)
                .take(4 * 24)
                .map(|v| v.position[2])
                .fold(f32::MAX, f32::min)
        };
        let rest = forward(&Pose::default());
        let shamble = forward(&Pose {
            left_arm: ARMS_FORWARD_ANGLE,
            right_arm: ARMS_FORWARD_ANGLE,
            ..Default::default()
        });
        assert!(
            shamble < rest - 0.2,
            "the shamble should reach forward (-Z): rest {rest}, shamble {shamble}"
        );
    }
}

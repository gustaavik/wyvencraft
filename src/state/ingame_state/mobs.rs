//! Mob simulation from the state layer: spawning, per-frame perception +
//! updates, and applying the attacks mobs commit to.
//!
//! Only the authority (singleplayer/host) runs this — mirroring the fluid
//! sim, `frame.rs` gates the tick on the session's authority. Clients hold
//! interpolated replicas fed by the host instead of simulating.

use glam::Vec3;

use super::{HOST_PLAYER_ID, InGameState};
use crate::art::{mobskin, skin};
use crate::core::{Aabb, BlockPos, Rng64};
use crate::entity::kind::VisualSpec;
use crate::entity::rigged;
use crate::entity::{
    AnimationState, Arrow, HumanoidModel, Mob, MobAction, MobId, Perception, PlayerSighting,
    QuadrupedModel,
};
use crate::inventory::ItemStack;
use crate::net::{Channel, ClientMessage, PlayerId, ServerMessage};
use crate::world::Target;
use wyven_model::{ModelId, ModelRegistry};
use wyven_render::CpuMesh;

/// Zombie shamble: both arms held straight out (≈ 80° forward of hanging).
///
/// Positive is *forward* — the arm pivots at the shoulder, so a positive angle
/// carries the fist toward the model's front (-Z). See `rot_x` in
/// [`crate::entity::model`], which every arm pose is written against.
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
struct MobTarget {
    player: Option<PlayerId>,
    eye: Vec3,
}

/// A client's replica of a host-simulated mob: two position snapshots (the
/// `RemotePlayer` pattern), the visual to render, and enough of the kind to
/// target it and roll kill loot. Created from `MobSpawned`, moved by
/// `MobStates`, removed by `MobDespawned`.
pub(super) struct RemoteMob {
    kind_name: String,
    visual: VisualSpec,
    /// Collision-box extents (for crosshair targeting).
    width: f32,
    height: f32,
    position: Vec3,
    yaw: f32,
    /// Walk-animation state; speed is derived from the position delta each
    /// frame, like remote players.
    anim: AnimationState,
    last_pos: Vec3,
}

impl RemoteMob {
    /// Build a replica from the kind named in `MobSpawned`.
    pub(super) fn new(kind: &crate::entity::EntityKind, position: Vec3) -> Self {
        Self {
            kind_name: kind.name.clone(),
            visual: kind.visual.clone(),
            width: kind.physics.width,
            height: kind.physics.height,
            position,
            yaw: 0.0,
            anim: AnimationState::new(),
            last_pos: position,
        }
    }

    /// Advance the walk animation from the movement observed since the last
    /// frame (the remote-player trick), clamped so a teleport can't drive an
    /// absurd cadence. Returns the mesh inputs for this frame.
    pub(super) fn animate(&mut self, dt: f32) -> (&VisualSpec, Vec3, f32, crate::entity::Pose) {
        let speed = if dt > 0.0 {
            (Vec3::new(
                self.position.x - self.last_pos.x,
                0.0,
                self.position.z - self.last_pos.z,
            )
            .length()
                / dt)
                .min(super::REMOTE_MAX_SPEED)
        } else {
            0.0
        };
        self.anim.advance(speed, self.yaw, dt);
        self.last_pos = self.position;
        // Drawn at the torso yaw, which follows the snapshot yaw the head keeps.
        (
            &self.visual,
            self.position,
            self.anim.body_yaw(),
            self.anim.pose(0.0),
        )
    }

    pub(super) fn push_snapshot(&mut self, position: Vec3, yaw: f32) {
        self.position = position;
        self.yaw = yaw;
    }

    pub(super) fn kind_name(&self) -> &str {
        &self.kind_name
    }

    pub(super) fn position(&self) -> Vec3 {
        self.position
    }

    fn aabb(&self) -> Aabb {
        let half = self.width * 0.5;
        Aabb::new(
            self.position - Vec3::new(half, 0.0, half),
            self.position + Vec3::new(half, self.height, half),
        )
    }
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
    pose: &crate::entity::Pose,
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

/// What the crosshair is on: an index into the authority's own mob list, or
/// a replica's wire id on a client.
pub(super) enum MobTargetRef {
    Local(usize),
    Remote(u64),
}

impl InGameState {
    /// Queue a mob event for the host broadcast (dropped outside hosting; a
    /// singleplayer session has no listeners and clients never emit).
    fn emit_mob_event(&mut self, msg: ServerMessage) {
        if self.session.serves_peers() {
            self.peers.mob_events.push(msg);
        }
    }

    /// Spawn a mob of the named kind at `position` (feet). `None` when the
    /// kind is unknown or isn't a mob. The brain's random stream is seeded
    /// from the world seed and the mob id, so runs are reproducible.
    pub(super) fn spawn_mob(&mut self, kind_name: &str, position: Vec3) -> Option<MobId> {
        let kind = self.content.entities.find(kind_name)?;
        let id = MobId(self.mobs.next_id);
        let seed = self.world.seed() ^ id.0.wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mob = Mob::spawn(kind, id, position, seed)?;
        self.mobs.next_id += 1;
        self.mobs.live.push(mob);
        self.emit_mob_event(ServerMessage::MobSpawned {
            id: id.0,
            kind: kind_name.to_string(),
            position: position.to_array(),
        });
        Some(id)
    }

    /// Every player a mob may attack right now: the local player (unless dead
    /// or in a protected mode) plus survival-mode remote players.
    fn mob_targets(&self) -> Vec<MobTarget> {
        let mut targets = Vec::new();
        if !self.dead && self.player.mode.takes_damage() {
            targets.push(MobTarget {
                player: None,
                eye: self.player.eye_position(),
            });
        }
        let eye_height = self
            .content
            .entities
            .player()
            .movement
            .map(|m| m.eye_height)
            .unwrap_or(1.62);
        for (id, remote) in &self.peers.players {
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
        let mut melee_hits: Vec<(Option<PlayerId>, f32)> = Vec::new();
        // Arrows launched this tick: (origin eye, velocity, damage, gravity, lifetime).
        let mut fired: Vec<(Vec3, Vec3, f32, f32, f32)> = Vec::new();

        for mob in &mut self.mobs.live {
            // Mobs straddling the streaming edge freeze until their chunk is
            // back (unloaded chunks read as solid, which would trap them).
            if !self
                .world
                .is_loaded(BlockPos::from_world(mob.position).chunk())
            {
                continue;
            }

            let eye = mob.eye_position();
            let nearest = targets
                .iter()
                .map(|t| (t, (t.eye - eye).length()))
                .min_by(|a, b| a.1.total_cmp(&b.1));
            let sighting = nearest.map(|(t, distance)| {
                let offset = t.eye - eye;
                // Line of sight: no solid block between the two eye points.
                // Solid blocks only, and always the whole cell: sight is about
                // what blocks it, so ground cover you can walk through must not
                // hide a player from a mob.
                let visible = distance < 1.0e-3
                    || crate::world::raycast(eye, offset / distance, distance, |p| {
                        self.world.is_solid(p).then_some(Target::Cell)
                    })
                    .is_none();
                PlayerSighting {
                    offset,
                    distance,
                    visible,
                }
            });
            let perception = Perception {
                on_ground: mob.on_ground,
                target: sighting,
                hurt: mob.take_hurt(),
            };

            let action = mob.update(dt, perception, |p| self.world.is_solid_for_collision(p));
            match action {
                MobAction::None => {}
                MobAction::Melee { damage } => {
                    melee_hits.push((nearest.map(|(t, _)| t.player).unwrap_or(None), damage));
                }
                MobAction::Fire { velocity, damage } => {
                    let ranged = mob.params.ranged.as_ref().expect("ranged kinds fire");
                    fired.push((
                        // Nock ahead of the face so the arrow clears the model.
                        mob.eye_position() + velocity.normalize_or_zero() * 0.4,
                        velocity,
                        damage,
                        ranged.projectile_gravity,
                        ranged.lifetime,
                    ));
                }
            }
        }

        for (origin, velocity, damage, gravity, lifetime) in fired {
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
        let mut i = 0;
        while i < self.mobs.live.len() {
            if !self.mobs.live[i].dead() {
                i += 1;
                continue;
            }
            let mob = self.mobs.live.swap_remove(i);
            let killed_by = mob.last_attacker.map(PlayerId);
            self.emit_mob_event(ServerMessage::MobDespawned {
                id: mob.id.0,
                killed_by,
            });
            if killed_by.is_none_or(|pid| pid == HOST_PLAYER_ID) {
                self.pop_drops_for(&mob.kind_name, mob.id.0, mob.position);
            }
        }
    }

    /// Roll a kind's drop table and pop the loot as dropped items at
    /// `position` (a dead mob's feet). Deterministic per (world seed, mob id)
    /// — the host and the killing client roll identical loot without it ever
    /// crossing the wire. Unknown item names are skipped with a warning.
    pub(super) fn pop_drops_for(&mut self, kind_name: &str, mob_id: u64, position: Vec3) {
        let Some(kind) = self.content.entities.find(kind_name) else {
            return;
        };
        let Some(params) = kind.mob.clone() else {
            return;
        };
        let mut rng = Rng64::new(self.world.seed() ^ mob_id.wrapping_mul(0xA24B_AED4_963E_E407));
        let block = BlockPos::from_world(position + Vec3::Y * 0.25);
        for drop in &params.drops {
            let Some(item) = self.content.items.find(&drop.item) else {
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
            let stack = ItemStack::new(item, (count as u8).min(self.content.items.max_stack(item)));
            let angle = self.view.elapsed * 9.73 + rng.range_f32(0.0, std::f32::consts::TAU);
            self.drops.push(crate::entity::DroppedItem::block_drop(
                stack,
                block,
                angle,
                self.content.entities.dropped_item(),
            ));
        }
    }

    /// Damage the local player's swing lands, from whatever is in the selected
    /// hotbar slot. A tool without a `damage` component — a pickaxe, a shovel —
    /// hits exactly as hard as a bare fist.
    pub(super) fn melee_damage(&self) -> f32 {
        self.inventory
            .item_in_selected()
            .and_then(|id| self.content.items.tool(id))
            .and_then(|tool| tool.damage)
            .unwrap_or(PLAYER_ATTACK_DAMAGE)
    }

    /// The mob under the crosshair within melee reach, if any — and only if
    /// no solid block is closer (no punching mobs through walls). The
    /// authority scans its own mobs; a client scans its replicas.
    pub(super) fn targeted_mob(&self) -> Option<MobTargetRef> {
        let eye = self.player.eye_position();
        let look = self.player.look_direction();
        let reach = self.player.movement().reach;
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
        if !self.session.is_authority() {
            self.mobs
                .remote
                .iter()
                .filter_map(|(id, mob)| Some((*id, mob.aabb().ray_hit(eye, look, max_t)?)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(id, _)| MobTargetRef::Remote(id))
        } else {
            self.mobs
                .live
                .iter()
                .enumerate()
                .filter_map(|(i, mob)| Some((i, mob.aabb().ray_hit(eye, look, max_t)?)))
                .min_by(|a, b| a.1.total_cmp(&b.1))
                .map(|(i, _)| MobTargetRef::Local(i))
        }
    }

    /// Land a player melee swing on the targeted mob. The authority applies
    /// it directly (with kill credit to itself); a client sends `Attack` and
    /// the host validates and applies.
    pub(super) fn attack_mob(&mut self, target: MobTargetRef) {
        match target {
            MobTargetRef::Local(index) => {
                let look = self.player.look_direction();
                let push = Vec3::new(look.x, 0.0, look.z).normalize_or_zero() * KNOCKBACK_PUSH
                    + Vec3::Y * KNOCKBACK_LIFT;
                let damage = self.melee_damage();
                if let Some(mob) = self.mobs.live.get_mut(index) {
                    mob.damage(damage, push);
                    mob.last_attacker = Some(HOST_PLAYER_ID.0);
                    let hurt = ServerMessage::MobHurt {
                        id: mob.id.0,
                        health: mob.health,
                    };
                    self.emit_mob_event(hurt);
                }
            }
            MobTargetRef::Remote(id) => {
                // Only the host may apply damage; ask it to.
                self.session
                    .request(&ClientMessage::Attack { id }, Channel::Reliable);
            }
        }
    }

    /// Advance arrows on every peer (they're visual off the authority); the
    /// authority alone hit-tests players and applies damage.
    pub(super) fn update_arrows(&mut self, dt: f32) {
        let authority = self.session.is_authority();
        let local_box = (authority && !self.dead && self.player.mode.takes_damage())
            .then(|| self.player.aabb());
        let player_kind = self.content.entities.player().physics;
        let remote_boxes: Vec<(PlayerId, Aabb)> = if authority {
            self.peers
                .players
                .iter()
                .filter(|(_, rp)| rp.mode.takes_damage())
                .map(|(id, rp)| {
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

        let mut hits: Vec<(Option<PlayerId>, f32)> = Vec::new();
        let world = &self.world;
        self.mobs.arrows.retain_mut(|arrow| {
            if !arrow.update(dt, |p| world.is_solid_for_collision(p)) {
                return false;
            }
            let hitbox = arrow.aabb();
            if local_box.is_some_and(|b| b.intersects(hitbox)) {
                hits.push((None, arrow.damage));
                return false;
            }
            if let Some((id, _)) = remote_boxes.iter().find(|(_, b)| b.intersects(hitbox)) {
                hits.push((Some(*id), arrow.damage));
                return false;
            }
            true
        });
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
        ground_at(&self.world, x, z, top)
    }

    /// Periodic mob spawning + the standing despawn rules (authority only).
    /// The planner is pure ([`crate::entity::Spawner::tick`]); this method
    /// feeds it the live world and applies its plan.
    pub(super) fn update_spawning(&mut self, dt: f32) {
        let cfg = self.content.spawning.clone();
        let mut anchors = vec![self.player.position];
        anchors.extend(self.peers.players.values().map(|r| r.position()));
        let is_night = self.day_cycle.is_night();
        // Cap the surface search near player height: caves far below the
        // surface aren't valid spawn floors for surface mobs (and there's no
        // light level to gate on yet).
        let top = (self.player.position.y + 24.0) as i32;

        let world = &self.world;
        let mobs = &self.mobs.live;
        let requests = self.mobs.spawner.tick(
            &cfg,
            dt,
            is_night,
            &anchors,
            mobs.len(),
            |name| mobs.iter().filter(|m| m.kind_name == name).count(),
            |x, z| ground_at(world, x, z, top),
        );
        for request in requests {
            if self.spawn_mob(&request.entity, request.position).is_some() {
                let night_rule = cfg
                    .entry(&request.entity)
                    .is_some_and(|e| e.despawn_in_daylight);
                if night_rule && let Some(mob) = self.mobs.live.last_mut() {
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
        let mut index = 0;
        while index < self.mobs.live.len() {
            let mob = &self.mobs.live[index];
            let stray = anchors
                .iter()
                .all(|a| (mob.position - *a).length_squared() > despawn_sq);
            if stray || (day && mob.night_spawned) {
                let id = self.mobs.live.swap_remove(index).id.0;
                self.emit_mob_event(ServerMessage::MobDespawned {
                    id,
                    killed_by: None,
                });
            } else {
                index += 1;
            }
        }
    }

    /// Dev hook (`WYVEN_DEBUG_SPAWN=cow,zombie,...`): spawn the named kinds in a
    /// line near the player right after entering the world, for visual checks
    /// without waiting on the spawner. Replaced by real spawning rules.
    pub(super) fn debug_spawn_from_env(&mut self) {
        let Ok(kinds) = std::env::var("WYVEN_DEBUG_SPAWN") else {
            return;
        };
        for (i, kind) in kinds.split(',').map(str::trim).enumerate() {
            let x = self.spawn.x + 3.0 + 2.0 * i as f32;
            let z = self.spawn.z + 3.0;
            let y = self
                .find_ground(x, z, crate::core::CHUNK_HEIGHT - 2)
                .unwrap_or(self.spawn.y);
            match self.spawn_mob(kind, Vec3::new(x, y, z)) {
                Some(id) => log::info!("debug-spawned {kind:?} as {id:?} at ({x}, {y}, {z})"),
                None => log::warn!("WYVEN_DEBUG_SPAWN: unknown mob kind {kind:?}"),
            }
        }
    }

    /// Route damage to the authority's own player: armor mitigates inside
    /// `Player::damage`, worn pieces take wear, and death freezes control.
    pub(super) fn damage_local_player(&mut self, amount: f32) {
        let before = self.player.health;
        self.player.damage(amount);
        if self.player.health < before {
            self.inventory.wear_armor(1);
        }
        if self.player.is_dead() && !self.dead {
            self.dead = true;
            self.breaking = None;
        }
    }
}

/// The walkable *surface* at `(x, z)`: the Y of the first space with solid
/// ground below, two air blocks above, and open sky the rest of the way up,
/// scanning down from `top`. `None` when the column's chunk is unloaded or
/// has no such spot. The rules this encodes:
/// - non-air, non-solid blocks (water) are not clear → no spawning in fluids;
/// - the sky requirement rejects caves and overhangs, which the plain
///   top-down scan would otherwise fall into under oceans and mountains
///   (there is no per-block light to gate on, so sky access is the proxy).
fn ground_at(world: &crate::world::World, x: f32, z: f32, top: i32) -> Option<f32> {
    let column = BlockPos::new(x.floor() as i32, 0, z.floor() as i32);
    if !world.is_loaded(column.chunk()) {
        return None;
    }
    let clear = |y: i32| {
        world
            .block_at(BlockPos::new(column.x, y, column.z))
            .is_air()
    };
    let y = (1..=top.min(crate::core::CHUNK_HEIGHT - 2))
        .rev()
        .find(|&y| {
            world.is_solid(BlockPos::new(column.x, y - 1, column.z)) && clear(y) && clear(y + 1)
        })?;
    let open_sky = (y + 2..crate::core::CHUNK_HEIGHT)
        .all(|yy| !world.is_solid(BlockPos::new(column.x, yy, column.z)));
    open_sky.then_some(y as f32)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::Pose;

    /// The shamble has to reach *at* you. The sign is easy to get backwards and
    /// nothing else would catch it — a zombie with its arms behind it still
    /// walks, chases and hits exactly the same.
    ///
    /// Read off the built mesh rather than off a hand anchor: the player's hand
    /// moved to the rigged model, and a box-model mob has no anchor of its own.
    #[test]
    fn the_shamble_holds_the_arms_out_in_front() {
        let model = HumanoidModel::player();
        let sheet = crate::art::skin::SKIN_ORIGIN;
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

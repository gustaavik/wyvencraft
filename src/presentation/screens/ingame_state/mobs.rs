//! Mob simulation from the state layer: spawning, per-frame perception +
//! updates, and applying the attacks mobs commit to.
//!
//! Only the authority (singleplayer/host) runs this — mirroring the fluid
//! sim, `frame.rs` gates the tick on the session's authority. Clients hold
//! interpolated replicas fed by the host instead of simulating.

use glam::Vec3;

use super::{HOST_PLAYER_ID, InGameState};
use crate::application::ecs::components::{Body, Mob, Replica, Transform};
use crate::application::ecs::systems::mobs::{self as mob_systems};
use crate::application::ecs::{Entity, With};
use crate::domain::core::Aabb;
use crate::domain::entity::MobId;
use crate::domain::entity::kind::VisualSpec;
use crate::infrastructure::net::{Channel, ClientMessage, ServerMessage};
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
pub(super) use crate::application::simulation::{
    KNOCKBACK_LIFT, KNOCKBACK_PUSH, PLAYER_ATTACK_DAMAGE,
};
/// Reach the host accepts for a client's `Attack` (their reach plus lag slack).
const ATTACK_VALIDATE_RANGE: f32 = 7.0;

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
    /// Advance every mob one step (authority only), land the boss beats it
    /// produced, resolve the attacks, and reap the dead — in that order, so a
    /// boss killed this frame still lands the blow it committed to.
    pub(super) fn update_mobs(&mut self, dt: f32) {
        let tick = self.sim.tick_mobs(dt);
        self.land_boss_beats(tick.beats);
        self.sim.resolve_mob_attacks(tick.fired, tick.melee);
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
                self.sim.emit(ServerMessage::MobDespawned {
                    id: mob.id.0,
                    killed_by: None,
                });
                continue;
            }
            self.sim.settle_kill(mob);
        }
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
                let damage = self.sim.melee_damage();
                let id = self.sim.ecs.get::<MobId>(entity).copied();
                if let Some(id) = id
                    && let Some(health) =
                        mob_systems::hit(&mut self.sim.ecs, entity, damage, push, HOST_PLAYER_ID.0)
                {
                    self.sim.emit(ServerMessage::MobHurt { id: id.0, health });
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
                .sim
                .find_ground(x, z, crate::domain::core::CHUNK_HEIGHT - 2)
                .unwrap_or(near.y);
            match self.sim.spawn_mob(kind, Vec3::new(x, y, z)) {
                Some(id) => log::info!("debug-spawned {kind:?} as {id:?} at ({x}, {y}, {z})"),
                None => log::warn!("WYVEN_DEBUG_SPAWN: unknown mob kind {kind:?}"),
            }
        }
    }
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

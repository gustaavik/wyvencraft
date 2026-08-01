//! A simulated mob: the body around a [`MobBrain`]. Owns position, velocity,
//! health, and the animation clock; each update turns the brain's [`Intent`]
//! into real movement through the shared swept-AABB physics, and reports any
//! attack for the owner (the in-game state) to resolve.
//!
//! Like every entity, all tuning is copied from the kind (`entities.toml`)
//! at spawn; there is no per-species code here. Mobs are simulated only by
//! the authority (singleplayer/host) — clients render replicas.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};
use crate::entity::animation::AnimationState;
use crate::entity::brain::{Gait, Intent, MobBrain, Perception};
use crate::entity::kind::{EntityKind, MobParams, PhysicsParams, VisualSpec};
use crate::entity::physics;

/// Session-scoped mob identifier, allocated by the authority. Crosses the
/// wire so clients can track replicas; never persisted (saves re-number).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct MobId(pub u64);

/// An attack the mob committed to this frame; the owner applies its effect.
#[derive(Debug, Clone, Copy)]
pub enum MobAction {
    None,
    /// Hit the perceived target for `damage`.
    Melee {
        damage: f32,
    },
    /// Launch a projectile with this initial velocity.
    Fire {
        velocity: Vec3,
        damage: f32,
    },
}

/// Fraction of the eye height up the collision box the mob "sees" from.
const EYE_FRACTION: f32 = 0.85;
/// A step is "blocked" when it achieves under this fraction of its intent.
const BLOCKED_FRACTION: f32 = 0.2;

pub struct Mob {
    pub id: MobId,
    /// Registry name of the kind (the save/wire identity).
    pub kind_name: String,
    physics: PhysicsParams,
    pub params: MobParams,
    pub visual: VisualSpec,
    /// Feet position (like the player; models build from the feet).
    pub position: Vec3,
    pub velocity: Vec3,
    /// Facing (radians about Y, 0 = -Z).
    pub yaw: f32,
    pub health: f32,
    pub on_ground: bool,
    brain: MobBrain,
    /// Procedural walk/idle animation, advanced from actual speed.
    pub anim: AnimationState,
    /// Seconds left until the next attack is allowed.
    attack_timer: f32,
    /// Set by [`Mob::damage`]; drained into the next perception.
    hurt: bool,
    /// Spawned under the night-only rule; reaped at dawn.
    pub night_spawned: bool,
    /// Raw player id (`net::PlayerId.0`) of the last attacker, for kill
    /// credit. A bare `u64` keeps this module free of net-layer types.
    pub last_attacker: Option<u64>,
}

impl Mob {
    /// Instantiate a kind at `position` (feet). Returns `None` when the kind
    /// carries no `[entity.mob]` component. `seed` fixes the brain's random
    /// stream, making the mob's decisions reproducible.
    pub fn spawn(kind: &EntityKind, id: MobId, position: Vec3, seed: u64) -> Option<Self> {
        let params = kind.mob.clone()?;
        Some(Self {
            id,
            kind_name: kind.name.clone(),
            physics: kind.physics,
            health: params.max_health,
            params,
            visual: kind.visual.clone(),
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            on_ground: false,
            brain: MobBrain::new(seed),
            anim: AnimationState::new(),
            attack_timer: 0.0,
            hurt: false,
            night_spawned: false,
            last_attacker: None,
        })
    }

    /// Collision box in world space.
    pub fn aabb(&self) -> Aabb {
        let half = self.physics.width * 0.5;
        Aabb::new(
            self.position - Vec3::new(half, 0.0, half),
            self.position + Vec3::new(half, self.physics.height, half),
        )
    }

    /// Where the mob sees (and shoots) from.
    pub fn eye_position(&self) -> Vec3 {
        self.position + Vec3::new(0.0, self.physics.height * EYE_FRACTION, 0.0)
    }

    pub fn height(&self) -> f32 {
        self.physics.height
    }

    /// Take a hit: no armor, straight off the health, plus knockback. The
    /// hurt flag feeds the next perception so the brain can react.
    pub fn damage(&mut self, amount: f32, knockback: Vec3) {
        self.health = (self.health - amount).max(0.0);
        self.velocity += knockback;
        self.hurt = true;
    }

    pub fn dead(&self) -> bool {
        self.health <= 0.0
    }

    /// Drain the since-last-decision hurt flag (used to build perception).
    pub fn take_hurt(&mut self) -> bool {
        std::mem::take(&mut self.hurt)
    }

    /// Advance one step: decide, steer, integrate, animate. Returns the attack
    /// the mob commits to this frame (already cooldown-gated), if any.
    pub fn update(
        &mut self,
        dt: f32,
        perception: Perception,
        is_solid: impl Fn(BlockPos) -> bool,
    ) -> MobAction {
        self.attack_timer = (self.attack_timer - dt).max(0.0);
        let intent = self.brain.think(&perception, &self.params, dt);

        if let Some(yaw) = intent.yaw {
            self.yaw = yaw;
        }
        let speed = match intent.gait {
            Gait::Stand => 0.0,
            Gait::Walk => self.params.walk_speed,
            Gait::Run => self.params.run_speed,
        };
        let (sy, cy) = self.yaw.sin_cos();
        let forward = Vec3::new(sy, 0.0, -cy);
        let sign = if intent.backward { -1.0 } else { 1.0 };
        self.velocity.x = forward.x * speed * sign;
        self.velocity.z = forward.z * speed * sign;
        self.velocity.y =
            (self.velocity.y - self.physics.gravity * dt).max(self.physics.terminal_velocity);

        let step = self.velocity * dt;
        let result = physics::move_and_collide(self.aabb(), step, &is_solid);
        self.position += result.delta;
        self.on_ground = result.on_ground;
        if result.on_ground && self.velocity.y < 0.0 {
            self.velocity.y = 0.0;
        }
        // A hop under a low ceiling would otherwise keep pushing upward and pin
        // the mob to the block for the rest of its ascent.
        if result.hit_ceiling {
            self.velocity.y = 0.0;
        }

        // No step-up in the physics: when a walk is stopped by a ledge, hop.
        let intended = Vec3::new(step.x, 0.0, step.z).length();
        let achieved = Vec3::new(result.delta.x, 0.0, result.delta.z).length();
        if self.on_ground && intended > 1.0e-4 && achieved < intended * BLOCKED_FRACTION {
            self.velocity.y = self.params.jump_speed;
        }

        let horizontal = Vec3::new(self.velocity.x, 0.0, self.velocity.z).length();
        self.anim.advance(horizontal, dt);

        self.resolve_attack(intent, &perception)
    }

    /// Turn an attacking intent into a committed action, respecting the
    /// cooldown. Ranged kinds loft a projectile at the target's eye; everyone
    /// else swings.
    fn resolve_attack(&mut self, intent: Intent, perception: &Perception) -> MobAction {
        if !intent.attack || self.attack_timer > 0.0 {
            return MobAction::None;
        }
        let Some(target) = perception.target else {
            return MobAction::None;
        };
        self.attack_timer = self.params.attack_cooldown;
        self.anim.trigger_swing();
        match &self.params.ranged {
            Some(ranged) => {
                // Aim at the eye, lofted to cancel gravity drop over the
                // flight time (t ≈ distance / speed → lift = g·t / 2).
                let dir = target.offset.normalize_or_zero();
                let lift = 0.5 * ranged.projectile_gravity * target.distance
                    / ranged.projectile_speed.max(0.001);
                MobAction::Fire {
                    velocity: dir * ranged.projectile_speed + Vec3::Y * lift,
                    damage: ranged.projectile_damage,
                }
            }
            None => MobAction::Melee {
                damage: self.params.attack_damage,
            },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityRegistry;
    use crate::entity::brain::PlayerSighting;

    fn spawn(kind: &str, pos: Vec3) -> Mob {
        let kinds = EntityRegistry::builtin();
        Mob::spawn(kinds.find(kind).unwrap(), MobId(1), pos, 42).expect("mob kind")
    }

    fn seeing(offset: Vec3) -> Perception {
        Perception {
            on_ground: true,
            target: Some(PlayerSighting {
                offset,
                distance: offset.length(),
                visible: true,
            }),
            hurt: false,
        }
    }

    #[test]
    fn kinds_without_a_mob_component_do_not_spawn() {
        let kinds = EntityRegistry::builtin();
        assert!(Mob::spawn(kinds.player(), MobId(0), Vec3::ZERO, 0).is_none());
    }

    #[test]
    fn mob_settles_on_flat_ground() {
        // Solid floor fills y < 64; the cow starts falling from above it.
        let solid = |p: BlockPos| p.y < 64;
        let mut cow = spawn("cow", Vec3::new(0.5, 70.0, 0.5));
        let calm = Perception {
            on_ground: false,
            target: None,
            hurt: false,
        };
        for _ in 0..600 {
            cow.update(1.0 / 60.0, calm, solid);
        }
        assert!(
            (cow.position.y - 64.0).abs() < 0.05,
            "feet should rest on the floor; y = {}",
            cow.position.y
        );
        assert!(cow.on_ground);
    }

    #[test]
    fn chasing_mob_jumps_a_one_block_ledge() {
        // Floor at y < 64, with a one-block step (y = 64) beyond x >= 3.
        let solid = |p: BlockPos| p.y < 64 || (p.x >= 3 && p.y == 64);
        let mut zombie = spawn("zombie", Vec3::new(0.5, 64.0, 0.5));
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            // A visible player 10 blocks along +X (eyes above the step's
            // walking surface) keeps the chase pinned.
            let offset = Vec3::new(10.5, 65.0 + 1.62, 0.5) - zombie.eye_position();
            zombie.update(dt, seeing(offset), solid);
        }
        assert!(
            zombie.position.x > 3.5,
            "zombie should clear the ledge; x = {}",
            zombie.position.x
        );
        assert!(
            (zombie.position.y - 65.0).abs() < 0.05,
            "zombie should stand on the step; y = {}",
            zombie.position.y
        );
    }

    #[test]
    fn melee_attacks_are_cooldown_gated() {
        let solid = |p: BlockPos| p.y < 64;
        let mut zombie = spawn("zombie", Vec3::new(0.5, 64.0, 0.5));
        let close = seeing(Vec3::new(1.0, 0.0, 0.0));
        let dt = 0.05;
        let mut hits = 0;
        let steps = (3.0 / dt) as usize; // three seconds toe-to-toe
        for _ in 0..steps {
            if let MobAction::Melee { damage } = zombie.update(dt, close, solid) {
                assert_eq!(damage, 3.0);
                hits += 1;
            }
        }
        // 1 s cooldown over 3 s: the swing count is bounded, not per-frame.
        assert!((3..=4).contains(&hits), "expected ~3 swings, got {hits}");
    }

    #[test]
    fn skeleton_fires_lofted_projectiles() {
        let solid = |p: BlockPos| p.y < 64;
        let mut skeleton = spawn("skeleton", Vec3::new(0.5, 64.0, 0.5));
        let target = seeing(Vec3::new(10.0, 0.0, 0.0));
        let mut fired = None;
        for _ in 0..100 {
            if let MobAction::Fire { velocity, damage } = skeleton.update(0.05, target, solid) {
                fired = Some((velocity, damage));
                break;
            }
        }
        let (velocity, damage) = fired.expect("skeleton should fire");
        assert_eq!(damage, 3.0);
        assert!(velocity.x > 15.0, "arrow flies at the target");
        assert!(velocity.y > 0.0, "arrow is lofted against gravity");
    }

    #[test]
    fn damage_applies_knockback_and_marks_hurt() {
        let mut cow = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
        cow.damage(4.0, Vec3::new(3.0, 2.0, 0.0));
        assert_eq!(cow.health, 6.0);
        assert!(cow.velocity.x > 0.0 && cow.velocity.y > 0.0);
        assert!(cow.take_hurt());
        assert!(!cow.take_hurt(), "hurt flag drains");
        cow.damage(100.0, Vec3::ZERO);
        assert!(cow.dead());
    }
}

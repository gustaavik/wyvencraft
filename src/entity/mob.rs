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
    /// Horizontal velocity from external shoves (knockback), kept apart from
    /// the steering velocity because [`Mob::update`] *overwrites* `velocity.x/z`
    /// from the gait every frame — an impulse folded straight into `velocity`
    /// would be erased on the very next tick. This bleeds off through ground
    /// friction instead, the same way a [`crate::entity::DroppedItem`] settles.
    impulse: Vec3,
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
            impulse: Vec3::ZERO,
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
        // Weight is a spectrum and is independent of disposition: a fixture can
        // be bolted down (resistance 1) or sent flying (0), and so can a mob.
        let taken = 1.0 - self.params.knockback_resistance.clamp(0.0, 1.0);
        let shove = knockback * taken;
        // The vertical pop goes straight into the velocity gravity integrates;
        // the horizontal shove has to survive the gait overwriting `velocity`.
        self.velocity.y += shove.y;
        self.impulse.x += shove.x;
        self.impulse.z += shove.z;
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
        // Where it wants to go, plus whatever shove is still bleeding off.
        self.velocity.x = forward.x * speed * sign + self.impulse.x;
        self.velocity.z = forward.z * speed * sign + self.impulse.z;
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

        // A shove only bleeds off against the ground; in mid-air the mob keeps
        // carrying it, so a hit launches it on an arc instead of stalling.
        if self.on_ground {
            let damp = (1.0 - self.physics.ground_friction * dt).max(0.0);
            self.impulse.x *= damp;
            self.impulse.z *= damp;
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
        // The pop goes into the velocity gravity integrates; the horizontal
        // shove is held apart so the gait can't overwrite it next tick.
        assert!(cow.velocity.y > 0.0, "vertical pop");
        assert!(cow.impulse.x > 0.0, "horizontal shove");
        assert!(cow.take_hurt());
        assert!(!cow.take_hurt(), "hurt flag drains");
        cow.damage(100.0, Vec3::ZERO);
        assert!(cow.dead());
    }

    /// Knockback resistance is a dial, not a switch: the same impulse moves a
    /// mob in proportion to what it does *not* resist.
    #[test]
    fn knockback_scales_with_resistance() {
        let push = Vec3::new(6.0, 0.0, 0.0);
        let speed_after = |resistance: f32| {
            let mut mob = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
            mob.params.knockback_resistance = resistance;
            mob.damage(1.0, push);
            mob.impulse.x
        };
        assert_eq!(
            speed_after(0.0),
            push.x,
            "no resistance takes the full shove"
        );
        assert_eq!(speed_after(1.0), 0.0, "full resistance is immovable");
        assert_eq!(speed_after(0.5), push.x * 0.5, "and it scales in between");
        // Out-of-range values clamp rather than inverting the shove.
        assert_eq!(speed_after(2.0), 0.0);
        assert_eq!(speed_after(-1.0), push.x);
    }

    /// Regression guard: `update` rewrites `velocity.x/z` from the gait every
    /// frame, so an impulse folded straight into `velocity` is erased on the
    /// next tick and knockback silently does nothing. It has to actually carry
    /// the mob somewhere.
    #[test]
    fn knockback_survives_the_gait_overwriting_velocity() {
        let solid = |p: BlockPos| p.y < 64;
        let mut cow = spawn("cow", Vec3::new(0.5, 64.0, 0.5));
        let start = cow.position;
        let calm = Perception {
            on_ground: true,
            target: None,
            hurt: false,
        };
        cow.damage(1.0, Vec3::new(6.0, 3.0, 0.0));
        for _ in 0..40 {
            cow.update(1.0 / 60.0, calm, solid);
        }
        assert!(
            cow.position.x - start.x > 0.5,
            "the shove should carry it: {} -> {}",
            start,
            cow.position
        );
        // …and then bleed off against the ground. Measured on the impulse
        // rather than on position: a passive cow wanders off under its own
        // power, which is movement but not the shove still acting.
        for _ in 0..120 {
            cow.update(1.0 / 60.0, calm, solid);
        }
        assert!(
            cow.impulse.length() < 0.01,
            "the shove should decay, not persist: {}",
            cow.impulse
        );
    }

    /// The case the two axes exist for: a fixture that decides nothing but is
    /// *not* bolted down still gets sent flying by a hit — and still never
    /// turns to face whoever threw it.
    #[test]
    fn an_inert_prop_can_still_be_knocked_back() {
        let solid = |p: BlockPos| p.y < 64;
        let mut prop = spawn("vine sword", Vec3::new(0.5, 64.0, 0.5));
        prop.params.knockback_resistance = 0.0;
        let start = prop.position;

        prop.damage(1.0, Vec3::new(6.0, 3.0, 0.0));
        for _ in 0..30 {
            prop.update(
                1.0 / 60.0,
                Perception {
                    on_ground: true,
                    target: Some(PlayerSighting {
                        offset: Vec3::new(-1.5, 0.0, 0.0),
                        distance: 1.5,
                        visible: true,
                    }),
                    hurt: true,
                },
                solid,
            );
        }
        assert!(
            prop.position.x - start.x > 0.5,
            "an unresisting prop should be shoved: {} -> {}",
            start,
            prop.position
        );
        assert_eq!(prop.yaw, 0.0, "but it still must not turn");
    }

    /// A fixture that *is* bolted down can be broken, but not shoved or turned.
    /// This is the whole-body version of the brain test: over a long run with a
    /// player right next to it and repeated hits, it must not have budged.
    #[test]
    fn a_bolted_down_prop_is_never_moved_or_turned_by_being_hit() {
        let solid = |p: BlockPos| p.y < 64;
        let mut prop = spawn("vine sword", Vec3::new(0.5, 64.0, 0.5));
        assert_eq!(prop.params.knockback_resistance, 1.0, "as shipped");
        let start = prop.position;
        let dt = 1.0 / 60.0;

        for step in 0..600 {
            // A player standing right beside it, hitting it every half second.
            let mut p = Perception {
                on_ground: true,
                target: Some(PlayerSighting {
                    offset: Vec3::new(1.5, 0.0, 0.5),
                    distance: 1.6,
                    visible: true,
                }),
                hurt: false,
            };
            if step % 30 == 0 {
                prop.damage(0.01, Vec3::new(6.0, 3.0, 0.0));
                p.hurt = true;
            }
            prop.update(dt, p, solid);
        }

        assert_eq!(prop.yaw, 0.0, "a prop must not rotate");
        assert!(
            (prop.position.x - start.x).abs() < 1e-3 && (prop.position.z - start.z).abs() < 1e-3,
            "a prop must not be pushed: {} -> {}",
            start,
            prop.position
        );
        assert!(
            prop.health < prop.params.max_health,
            "but it can be damaged"
        );
    }
}

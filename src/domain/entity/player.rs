//! Player state: transform, movement intent, and physics integration.
//!
//! All tuning numbers (speeds, gravity, vitals model) come from the "player"
//! entity kind in `assets/entities.toml`; the formulas live here.

use glam::Vec3;

use crate::domain::core::{Aabb, BlockPos, FIXED_DT, GameMode};
use crate::domain::entity::camera::Shot;
use crate::domain::entity::kind::{EntityKind, MovementParams, PhysicsParams, VitalsParams};
use crate::domain::entity::physics::{self};

/// Max physics steps simulated in one frame, so a long stall (chunk load,
/// alt-tab) can't spiral into a huge catch-up burst.
const MAX_PHYSICS_STEPS: u32 = 5;
/// Defense points beyond which armor stops helping (an 80% reduction).
const MAX_DEFENSE: f32 = 20.0;
/// Defense points that would absorb a hit entirely, were `MAX_DEFENSE` not lower.
const DEFENSE_PER_FULL_ABSORB: f32 = 25.0;
/// Horizontal speed below which a coasting entity is simply stopped.
const STOP_EPSILON: f32 = 0.05;

/// Which camera the player is using.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Perspective {
    First,
    ThirdBack,
    ThirdFront,
}

impl Perspective {
    /// Cycle F5: first -> third-back -> third-front -> first.
    pub fn next(self) -> Perspective {
        match self {
            Perspective::First => Perspective::ThirdBack,
            Perspective::ThirdBack => Perspective::ThirdFront,
            Perspective::ThirdFront => Perspective::First,
        }
    }

    pub fn is_first_person(self) -> bool {
        matches!(self, Perspective::First)
    }

    /// Where the camera sits relative to the eye, as a [`Shot`].
    ///
    /// The one place that knows this, so the camera's placement and the
    /// collision trace that decides how far along the offset it may go cannot
    /// drift apart.
    ///
    /// First person is not a special case: it is `ThirdBack` at zero distance,
    /// which puts the camera on the eye looking along the look direction. That
    /// is what the old `None` return and its `unwrap_or((Vec3::ZERO, look))`
    /// spelled out longhand — and collapsing it is what lets the inventory's
    /// framing shot be blended in from first person like any other.
    pub fn shot(self, pitch: f32, distance: f32) -> Shot {
        let (azimuth, elevation) = match self {
            Perspective::First | Perspective::ThirdBack => (std::f32::consts::PI, -pitch),
            Perspective::ThirdFront => (0.0, pitch),
        };
        Shot {
            azimuth,
            elevation,
            distance: if self.is_first_person() {
                0.0
            } else {
                distance
            },
            aim: 0.0,
            shift: 0.0,
        }
    }
}

/// Desired movement for one simulation tick, in player-local terms.
#[derive(Debug, Clone, Copy, Default)]
pub struct MovementInput {
    /// Forward(+)/back(-) along look direction (horizontal).
    pub forward: f32,
    /// Right(+)/left(-) strafe.
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    pub sprint: bool,
}

pub struct Player {
    /// Feet position (centre of the box on X/Z, bottom on Y).
    pub position: Vec3,
    pub velocity: Vec3,
    /// Yaw (around Y) and pitch (around X) in radians.
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub flying: bool,
    pub perspective: Perspective,
    /// Which gameplay rules apply (survival vs. creative).
    pub mode: GameMode,
    /// Survival vitals (ignored in creative, where the player is invulnerable).
    pub health: f32,
    pub hunger: f32,
    pub saturation: f32,
    /// Defense points from worn armor, mixed into [`Player::damage`]. Kept as a
    /// field rather than a `damage` argument because fall damage fires from
    /// inside [`Player::update`], which cannot see the inventory; the owner
    /// (`InGameState`) refreshes it each frame from `Inventory::total_defense`.
    pub defense: f32,
    /// Highest Y reached since last leaving the ground; drives fall-damage.
    fall_peak_y: f32,
    /// Feet Y at the moment the current jump was launched; the variable-height
    /// jump measures its guaranteed rise from here.
    jump_origin_y: f32,
    /// Feet position before the last fixed step, for render interpolation.
    prev_position: Vec3,
    // Static tuning, copied from the "player" entity kind at construction.
    physics: PhysicsParams,
    movement: MovementParams,
    vitals: VitalsParams,
}

impl Player {
    /// `kind` is the "player" entity kind from the registry (its movement and
    /// vitals components are validated present at content load).
    pub fn new(position: Vec3, mode: GameMode, kind: &EntityKind) -> Self {
        let vitals = kind.vitals.expect("player kind has vitals");
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: false,
            perspective: Perspective::First,
            mode,
            health: vitals.max_health,
            hunger: vitals.max_hunger,
            saturation: vitals.max_hunger,
            defense: 0.0,
            fall_peak_y: position.y,
            jump_origin_y: position.y,
            prev_position: position,
            physics: kind.physics,
            movement: kind.movement.expect("player kind has movement"),
            vitals,
        }
    }

    /// The vitals tuning (max health/hunger etc.), for the HUD and callers.
    pub fn vitals(&self) -> &VitalsParams {
        &self.vitals
    }

    /// The movement tuning (reach etc.).
    pub fn movement(&self) -> &MovementParams {
        &self.movement
    }

    /// Eye position used for the camera and raycasting.
    pub fn eye_position(&self) -> Vec3 {
        self.position + Vec3::new(0.0, self.movement.eye_height, 0.0)
    }

    /// Feet position blended `alpha` of the way from the previous fixed step to
    /// the current one — the render-time counterpart of [`Player::position`].
    ///
    /// Physics ticks at a fixed rate, so anything drawn above that rate must
    /// interpolate or it visibly steps. During a jump one tick is `jump_speed /
    /// TICKS_PER_SECOND` of vertical travel, which is a pop you cannot miss.
    pub fn interpolated_position(&self, alpha: f32) -> Vec3 {
        self.prev_position
            .lerp(self.position, alpha.clamp(0.0, 1.0))
    }

    /// Eye position blended the same way, for the camera and everything hung
    /// off it.
    ///
    /// Defined in terms of [`Player::interpolated_position`] rather than
    /// repeating the lerp: the body mesh is drawn at that position and the
    /// camera at this one, and the two drifting apart *is* the jitter this
    /// interpolation exists to remove.
    pub fn interpolated_eye_position(&self, alpha: f32) -> Vec3 {
        self.interpolated_position(alpha) + Vec3::new(0.0, self.movement.eye_height, 0.0)
    }

    /// Collision box in world space.
    pub fn aabb(&self) -> Aabb {
        let half = self.physics.width * 0.5;
        Aabb::new(
            self.position - Vec3::new(half, 0.0, half),
            self.position + Vec3::new(half, self.physics.height, half),
        )
    }

    /// Normalized forward look direction (includes pitch).
    pub fn look_direction(&self) -> Vec3 {
        let (sy, cy) = self.yaw.sin_cos();
        let (sp, cp) = self.pitch.sin_cos();
        Vec3::new(cp * sy, sp, -cp * cy).normalize()
    }

    /// Apply mouse look, clamping pitch to just under straight up/down.
    pub fn rotate(&mut self, delta_yaw: f32, delta_pitch: f32) {
        use std::f32::consts::FRAC_PI_2;
        self.yaw += delta_yaw;
        self.pitch = (self.pitch + delta_pitch).clamp(-FRAC_PI_2 + 0.001, FRAC_PI_2 - 0.001);
    }

    /// Advance the player at the fixed simulation rate, consuming `frame_dt` of
    /// wall-clock time. `accum` is the caller's carry-over between frames.
    /// Returns the fraction `[0,1)` through the next step, for interpolating the
    /// camera in [`Player::interpolated_eye_position`].
    ///
    /// Physics must not run on the variable frame delta: with semi-implicit
    /// Euler the jump apex is `v0²/2g + v0·dt/2`, so jump height would otherwise
    /// change with framerate. Input is sampled once and replayed into each step.
    pub fn step_fixed(
        &mut self,
        input: MovementInput,
        frame_dt: f32,
        accum: &mut f32,
        is_solid: impl Fn(BlockPos) -> bool,
    ) -> f32 {
        *accum = (*accum + frame_dt).min(MAX_PHYSICS_STEPS as f32 * FIXED_DT);
        while *accum >= FIXED_DT {
            *accum -= FIXED_DT;
            self.update(input, FIXED_DT, &is_solid);
        }
        *accum / FIXED_DT
    }

    /// Advance one fixed simulation step: steer, apply gravity or flight,
    /// move through the world, then settle falls and blocked motion.
    pub fn update(&mut self, input: MovementInput, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
        self.prev_position = self.position;
        // Flight only takes effect in a mode that permits it.
        let flying = self.flying && self.mode.can_fly();
        self.steer(input, flying, dt);
        self.apply_vertical(input, flying, dt);

        let was_on_ground = self.on_ground;
        let result = physics::move_and_collide(self.aabb(), self.velocity * dt, is_solid);
        self.position += result.delta;
        self.on_ground = result.on_ground;
        self.track_fall(flying, was_on_ground);
        self.stop_blocked(&result, flying);
    }

    /// Horizontal velocity toward what the input asks for.
    fn steer(&mut self, input: MovementInput, flying: bool, dt: f32) {
        // Horizontal wish-direction relative to yaw (ignore pitch for walking).
        let (sy, cy) = self.yaw.sin_cos();
        let forward = Vec3::new(sy, 0.0, -cy);
        let right = Vec3::new(cy, 0.0, sy);
        let mut wish = forward * input.forward + right * input.strafe;
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }

        let speed = if flying {
            self.movement.fly_speed
        } else if input.sprint {
            self.movement.sprint_speed
        } else {
            self.movement.walk_speed
        };

        // On the ground (and in flight) steering is instant; in the air the
        // velocity eases toward the wish so a mid-flight reversal ramps instead
        // of snapping, and momentum carries through the arc.
        //
        // The one exception is stopping *on foot*. Asking for nothing decays the
        // velocity over `stop_rate` rather than zeroing it, so releasing the
        // controls means coast rather than halt — which is what lets the
        // inventory keep stepping physics while it is open without the player
        // stopping dead in mid-stride under the camera that just panned onto
        // them. Only this branch ramps: starting and turning are as instant as
        // they ever were, so ordinary movement does not feel loose.
        //
        // Flight is deliberately left instant. At `fly_speed` a coast of the
        // same length would overshoot by better than half a block, and flight
        // is the mode where the player is placing blocks precisely.
        let target = wish * speed;
        let asking_to_move = wish.length_squared() > 0.0;
        if flying || (self.on_ground && asking_to_move) {
            self.velocity.x = target.x;
            self.velocity.z = target.z;
        } else if self.on_ground {
            let t = (self.movement.stop_rate * dt).clamp(0.0, 1.0);
            self.velocity.x -= self.velocity.x * t;
            self.velocity.z -= self.velocity.z * t;
            // An exponential decay never quite reaches zero; snap the last
            // sliver so a released player actually comes to rest instead of
            // creeping, which would keep the walk animation twitching.
            if Vec3::new(self.velocity.x, 0.0, self.velocity.z).length() < STOP_EPSILON {
                self.velocity.x = 0.0;
                self.velocity.z = 0.0;
            }
        } else {
            let t = (self.movement.air_control * dt).clamp(0.0, 1.0);
            self.velocity.x += (target.x - self.velocity.x) * t;
            self.velocity.z += (target.z - self.velocity.z) * t;
        }
    }

    /// Vertical velocity: flight's up and down, or gravity, the jump, and the
    /// variable-height cut when the jump is released early.
    fn apply_vertical(&mut self, input: MovementInput, flying: bool, dt: f32) {
        if flying {
            let vertical = (input.jump as i32 - input.sneak as i32) as f32;
            self.velocity.y = vertical * self.movement.fly_speed;
        } else {
            self.velocity.y =
                (self.velocity.y - self.physics.gravity * dt).max(self.physics.terminal_velocity);
            if input.jump && self.on_ground {
                self.velocity.y = self.movement.jump_speed;
                self.jump_origin_y = self.position.y;
            } else if !input.jump && !self.on_ground && self.velocity.y > 0.0 {
                // Variable-height jump: releasing early cuts the ascent, but
                // never below the speed still needed to reach `min_jump_height`
                // above the launch point — a tap must always clear one block.
                let risen = self.position.y - self.jump_origin_y;
                let remaining = (self.movement.min_jump_height - risen).max(0.0);
                let floor = (2.0 * self.physics.gravity * remaining).sqrt();
                self.velocity.y = self.velocity.y.min(floor);
            }
        }
    }

    /// Fall-damage bookkeeping: track the peak height of an airborne arc and,
    /// on landing, hurt the player for the distance fallen beyond the safe
    /// margin.
    fn track_fall(&mut self, flying: bool, was_on_ground: bool) {
        if flying {
            self.fall_peak_y = self.position.y;
        } else if self.on_ground {
            if !was_on_ground {
                let dist = self.fall_peak_y - self.position.y;
                if self.mode.takes_damage() && dist > self.vitals.safe_fall {
                    self.damage((dist - self.vitals.safe_fall) * self.vitals.fall_damage_per_block);
                }
            }
            self.fall_peak_y = self.position.y;
        } else {
            self.fall_peak_y = self.fall_peak_y.max(self.position.y);
        }
    }

    /// Zero out velocity components that were blocked.
    fn stop_blocked(&mut self, result: &physics::CollisionResult, flying: bool) {
        if !flying {
            if result.on_ground && self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
            // Without this the jump keeps pushing into the block overhead and
            // the player hangs there until gravity eats the whole ascent.
            if result.hit_ceiling {
                self.velocity.y = 0.0;
            }
            // Horizontal momentum now survives between ticks, so a wall has to
            // cancel it instead of letting it pile up against the block.
            if result.blocked.x {
                self.velocity.x = 0.0;
            }
            if result.blocked.z {
                self.velocity.z = 0.0;
            }
        } else {
            self.velocity = Vec3::ZERO;
        }
    }

    /// Advance survival vitals one step (no-op semantics in creative — callers
    /// should only invoke this in survival). `sprinting` raises hunger drain.
    pub fn tick_survival(&mut self, dt: f32, sprinting: bool) {
        let v = self.vitals;
        // Exertion drains the saturation buffer first, then hunger itself.
        let drain = (v.hunger_drain_base
            + if sprinting {
                v.hunger_drain_sprint
            } else {
                0.0
            })
            * dt;
        if self.saturation > 0.0 {
            self.saturation = (self.saturation - drain).max(0.0);
        } else {
            self.hunger = (self.hunger - drain).max(0.0);
        }

        // Natural regeneration while well-fed.
        if self.hunger >= v.regen_hunger_threshold && self.health < v.max_health {
            self.health = (self.health + v.regen_rate * dt).min(v.max_health);
            self.saturation = (self.saturation - drain).max(0.0);
        }

        // Starvation once hunger is fully depleted.
        if self.hunger <= 0.0 {
            self.health = (self.health - v.starve_rate * dt).max(0.0);
        }
    }

    /// Switch game mode, applying the rule changes that follow from it.
    pub fn set_mode(&mut self, mode: GameMode) {
        self.mode = mode;
        if !mode.can_fly() {
            self.flying = false;
        }
        if mode.is_creative() {
            // Creative is invulnerable; restore vitals so you can't die there.
            self.health = self.vitals.max_health;
            self.hunger = self.vitals.max_hunger;
            self.saturation = self.vitals.max_hunger;
        }
    }

    /// Apply damage (clamped, and only in a mode that takes damage). Worn armor
    /// absorbs 4% per defense point, up to the 20 points that cap at an 80%
    /// reduction — so a fully armored player always takes at least a fifth.
    pub fn damage(&mut self, amount: f32) {
        if self.mode.takes_damage() {
            let absorbed = self.defense.clamp(0.0, MAX_DEFENSE) / DEFENSE_PER_FULL_ABSORB;
            let taken = amount * (1.0 - absorbed);
            self.health = (self.health - taken).max(0.0);
        }
    }

    /// Restore health up to the maximum.
    pub fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount).min(self.vitals.max_health);
    }

    /// Eat: restore hunger, and saturation up to the new hunger level.
    pub fn feed(&mut self, hunger: f32, saturation: f32) {
        self.hunger = (self.hunger + hunger).min(self.vitals.max_hunger);
        self.saturation = (self.saturation + saturation).min(self.hunger);
    }

    pub fn is_dead(&self) -> bool {
        self.mode.takes_damage() && self.health <= 0.0
    }

    /// Whether the player has room to eat (hunger below the maximum).
    pub fn is_hungry(&self) -> bool {
        self.hunger < self.vitals.max_hunger
    }

    /// Reset vitals and motion for a respawn at `position`.
    pub fn respawn_at(&mut self, position: Vec3) {
        self.teleport(position);
        self.velocity = Vec3::ZERO;
        self.health = self.vitals.max_health;
        self.hunger = self.vitals.max_hunger;
        self.saturation = self.vitals.max_hunger;
        self.on_ground = false;
    }

    /// Move the player without simulating the trip (respawn, save load, host
    /// restore). Resets the interpolation and fall-damage anchors so the camera
    /// doesn't sweep across the world and the jump doesn't land as a fall.
    pub fn teleport(&mut self, position: Vec3) {
        self.position = position;
        self.prev_position = position;
        self.fall_peak_y = position.y;
        self.jump_origin_y = position.y;
    }

    pub fn toggle_perspective(&mut self) {
        self.perspective = self.perspective.next();
    }
}

#[cfg(test)]
mod tests;

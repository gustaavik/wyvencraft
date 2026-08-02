//! Player state: transform, movement intent, and physics integration.
//!
//! All tuning numbers (speeds, gravity, vitals model) come from the "player"
//! entity kind in `assets/entities.toml`; the formulas live here.

use glam::Vec3;

use crate::core::{Aabb, BlockPos, FIXED_DT, GameMode};
use crate::entity::kind::{EntityKind, MovementParams, PhysicsParams, VitalsParams};
use crate::entity::physics::{self};

/// Max physics steps simulated in one frame, so a long stall (chunk load,
/// alt-tab) can't spiral into a huge catch-up burst.
const MAX_PHYSICS_STEPS: u32 = 5;
/// Defense points beyond which armor stops helping (an 80% reduction).
const MAX_DEFENSE: f32 = 20.0;
/// Defense points that would absorb a hit entirely, were `MAX_DEFENSE` not lower.
const DEFENSE_PER_FULL_ABSORB: f32 = 25.0;

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

    /// Eye position blended `alpha` of the way from the previous fixed step to
    /// the current one. Physics ticks at a fixed rate, so rendering above that
    /// rate must interpolate or the camera visibly steps.
    pub fn interpolated_eye_position(&self, alpha: f32) -> Vec3 {
        self.prev_position
            .lerp(self.position, alpha.clamp(0.0, 1.0))
            + Vec3::new(0.0, self.movement.eye_height, 0.0)
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

    /// Advance one fixed simulation step.
    pub fn update(&mut self, input: MovementInput, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
        self.prev_position = self.position;

        // Horizontal wish-direction relative to yaw (ignore pitch for walking).
        let (sy, cy) = self.yaw.sin_cos();
        let forward = Vec3::new(sy, 0.0, -cy);
        let right = Vec3::new(cy, 0.0, sy);
        let mut wish = forward * input.forward + right * input.strafe;
        if wish.length_squared() > 1.0 {
            wish = wish.normalize();
        }

        // Flight only takes effect in a mode that permits it.
        let flying = self.flying && self.mode.can_fly();

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
        let target = wish * speed;
        if flying || self.on_ground {
            self.velocity.x = target.x;
            self.velocity.z = target.z;
        } else {
            let t = (self.movement.air_control * dt).clamp(0.0, 1.0);
            self.velocity.x += (target.x - self.velocity.x) * t;
            self.velocity.z += (target.z - self.velocity.z) * t;
        }

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

        let was_on_ground = self.on_ground;
        let result = physics::move_and_collide(self.aabb(), self.velocity * dt, is_solid);
        self.position += result.delta;
        self.on_ground = result.on_ground;

        // Fall-damage bookkeeping: track the peak height of an airborne arc and,
        // on landing, hurt the player for the distance fallen beyond the safe margin.
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

        // Zero out velocity components that were blocked.
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
mod tests {
    use super::*;
    use crate::entity::kind::EntityRegistry;

    /// Full health/hunger per the builtin "player" kind (20 = 10 hearts).
    const MAX_HEALTH: f32 = 20.0;
    const MAX_HUNGER: f32 = 20.0;

    /// A test player built from the builtin entity definitions.
    fn test_player(position: Vec3, mode: GameMode) -> Player {
        Player::new(position, mode, EntityRegistry::builtin().player())
    }

    /// Flat ground filling everything below y = 65.
    fn flat_ground(p: BlockPos) -> bool {
        p.y < 65
    }

    /// Jump key held, no other intent.
    fn holding_jump() -> MovementInput {
        MovementInput {
            jump: true,
            ..Default::default()
        }
    }

    /// Run a player until it is settled on the ground and ready to jump.
    fn settled(position: Vec3, solid: impl Fn(BlockPos) -> bool + Copy) -> Player {
        let mut player = test_player(position, GameMode::Survival);
        for _ in 0..10 {
            player.update(MovementInput::default(), FIXED_DT, solid);
        }
        assert!(player.on_ground, "test setup: player should be grounded");
        player
    }

    /// Regression: a player spawned flush on the ground at an integer Y (as
    /// [`find_spawn`](crate::state) produces) must come to rest instead of sinking
    /// through the world over successive ticks.
    #[test]
    fn spawned_player_rests_and_does_not_fall_through() {
        // Solid ground fills y < 65; spawn feet flush on the block top at y = 65.0.
        let solid = |p: BlockPos| p.y < 65;
        let mut player = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        let dt = 1.0 / 60.0;

        for _ in 0..240 {
            player.update(MovementInput::default(), dt, solid);
        }

        assert!(
            (65.0..=65.05).contains(&player.position.y),
            "player did not rest on the ground; y = {}",
            player.position.y
        );
        assert!(player.on_ground, "player should be grounded after settling");
    }

    /// Drop the player from well above the ground and let it settle; in survival
    /// it should lose health proportional to the (large) fall distance.
    #[test]
    fn long_fall_damages_in_survival() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = test_player(Vec3::new(0.5, 85.0, 0.5), GameMode::Survival);
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert!(player.on_ground, "player should land");
        assert!(
            player.health < MAX_HEALTH,
            "a ~20-block fall should deal damage; health = {}",
            player.health
        );
    }

    /// Teleporting down is not falling down. `/tp` and a save restore both move
    /// the player without simulating the trip, so the fall anchor has to move
    /// with them — otherwise arriving deals damage for a descent that never
    /// happened, which is `long_fall_damages_in_survival` firing by mistake.
    #[test]
    fn teleporting_down_does_not_land_as_a_fall() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = test_player(Vec3::new(0.5, 250.0, 0.5), GameMode::Survival);
        let dt = 1.0 / 60.0;

        // Fall a while, so the anchor is far above the ground.
        for _ in 0..120 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert!(player.position.y < 250.0, "the player should be falling");

        player.teleport(Vec3::new(0.5, 66.0, 0.5));
        player.velocity = Vec3::ZERO;
        for _ in 0..120 {
            player.update(MovementInput::default(), dt, solid);
        }

        assert!(player.on_ground, "player should settle");
        assert_eq!(
            player.health, MAX_HEALTH,
            "the ~185 blocks skipped by the teleport must not be charged as a fall"
        );
    }

    /// A short fall (below the safe margin) deals no damage.
    #[test]
    fn short_fall_is_harmless() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = test_player(Vec3::new(0.5, 67.0, 0.5), GameMode::Survival);
        let dt = 1.0 / 60.0;
        for _ in 0..240 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert_eq!(player.health, MAX_HEALTH, "a 2-block fall should be safe");
    }

    /// Armor mitigates every damage source, including the fall damage raised
    /// from inside `update` — which is why `defense` is a field, not an argument.
    #[test]
    fn armor_softens_damage_down_to_the_floor() {
        let mut bare = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        bare.damage(10.0);
        assert_eq!(bare.health, MAX_HEALTH - 10.0, "no armor, no mitigation");

        // The shipped full set is 17 points: 17/25 = 68% absorbed.
        let mut armored = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        armored.defense = 17.0;
        armored.damage(10.0);
        assert!(
            (armored.health - (MAX_HEALTH - 3.2)).abs() < 1e-3,
            "17 points should absorb 68%; health = {}",
            armored.health
        );

        // Defense past the cap keeps the 20% floor rather than granting immunity.
        let mut invincible = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        invincible.defense = 999.0;
        invincible.damage(10.0);
        assert!(
            (invincible.health - (MAX_HEALTH - 2.0)).abs() < 1e-3,
            "damage floors at 20%; health = {}",
            invincible.health
        );
    }

    /// The same long fall in creative deals no damage (invulnerable).
    #[test]
    fn long_fall_is_harmless_in_creative() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = test_player(Vec3::new(0.5, 85.0, 0.5), GameMode::Creative);
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert_eq!(player.health, MAX_HEALTH, "creative takes no fall damage");
    }

    /// Regression: hitting a block overhead must cancel the ascent. Before the
    /// fix, `velocity.y` kept its full `+9.0` through the head bonk and the
    /// player hung under the block for ~0.32 s while gravity ate the ascent.
    #[test]
    fn head_bonk_cancels_the_ascent() {
        // A 2-block-tall pocket: floor below y = 65, ceiling from y = 67 up.
        // The 1.8-tall player has 0.2 blocks of headroom, far less than a jump.
        let solid = |p: BlockPos| p.y < 65 || p.y >= 67;
        let mut player = settled(Vec3::new(0.5, 65.0, 0.5), solid);

        let mut peak = player.position.y;
        let mut stuck_rising: f32 = 0.0;
        for _ in 0..20 {
            let before = player.position.y;
            player.update(holding_jump(), FIXED_DT, solid);
            peak = peak.max(player.position.y);
            // Pinned against the ceiling (no rise) while still pushing upward.
            if player.position.y <= before + 1.0e-6 {
                stuck_rising = stuck_rising.max(player.velocity.y);
            }
        }

        assert!(
            (65.15..65.25).contains(&peak),
            "the player should rise into the ceiling and stop; peak = {peak}"
        );
        assert_eq!(
            stuck_rising, 0.0,
            "upward velocity survived the head bonk (v = {stuck_rising})"
        );
    }

    /// Jump height must not depend on framerate — the reason player physics is
    /// stepped at a fixed rate instead of on the frame delta.
    #[test]
    fn jump_height_is_independent_of_framerate() {
        // Hold jump across half a second of wall-clock time and record the apex.
        let peak_at = |frame_dt: f32, frames: usize| {
            let mut player = settled(Vec3::new(0.5, 65.0, 0.5), flat_ground);
            let mut accum = 0.0;
            let mut peak = player.position.y;
            for _ in 0..frames {
                player.step_fixed(holding_jump(), frame_dt, &mut accum, flat_ground);
                peak = peak.max(player.position.y);
            }
            peak
        };

        let slow = peak_at(1.0 / 30.0, 15);
        let fast = peak_at(1.0 / 144.0, 72);
        assert!(
            (slow - fast).abs() < 0.02,
            "apex differed with framerate: {slow} at 30 fps vs {fast} at 144 fps"
        );
        assert!(slow > 66.0, "the jump should clear a block; peak = {slow}");
    }

    /// The variable-height jump is floored: even the shortest possible tap must
    /// still clear a one-block step, or stepping up would become a coin flip.
    #[test]
    fn a_tapped_jump_still_clears_a_one_block_step() {
        let mut player = settled(Vec3::new(0.5, 65.0, 0.5), flat_ground);

        // One tick of Space, then released for the rest of the arc.
        player.update(holding_jump(), FIXED_DT, flat_ground);
        let mut peak = player.position.y;
        for _ in 0..60 {
            player.update(MovementInput::default(), FIXED_DT, flat_ground);
            peak = peak.max(player.position.y);
        }

        assert!(
            peak >= 66.0,
            "a tapped jump must still clear one block; peak = {peak}"
        );
        // ...but it must be visibly shorter than holding the key.
        let mut held = settled(Vec3::new(0.5, 65.0, 0.5), flat_ground);
        let mut held_peak = held.position.y;
        for _ in 0..60 {
            held.update(holding_jump(), FIXED_DT, flat_ground);
            held_peak = held_peak.max(held.position.y);
        }
        assert!(
            held_peak > peak + 0.15,
            "holding jump should go higher: {held_peak} vs {peak}"
        );
    }

    /// Mid-air steering ramps instead of snapping, so a direction change in the
    /// air can't reverse the player's momentum inside a single tick.
    #[test]
    fn air_control_ramps_instead_of_snapping() {
        let mut player = settled(Vec3::new(0.5, 65.0, 0.5), flat_ground);

        // Sprint forward (yaw 0 faces -Z) and jump: ground movement is instant.
        let forward = MovementInput {
            forward: 1.0,
            sprint: true,
            jump: true,
            ..Default::default()
        };
        player.update(forward, FIXED_DT, flat_ground);
        let launch_speed = player.velocity.z;
        assert!(launch_speed < -6.0, "should launch at sprint speed");

        // Now reverse in mid-air.
        let backward = MovementInput {
            forward: -1.0,
            ..forward
        };
        player.update(backward, FIXED_DT, flat_ground);
        assert!(
            player.velocity.z < 0.0,
            "one airborne tick must not flip momentum; vz = {}",
            player.velocity.z
        );

        for _ in 0..30 {
            player.update(backward, FIXED_DT, flat_ground);
        }
        assert!(!player.on_ground, "test setup: still airborne");
        assert!(
            player.velocity.z > 5.0,
            "air control should converge within ~0.5 s; vz = {}",
            player.velocity.z
        );
    }

    /// Ground movement stays instant — air control must not make walking mushy.
    #[test]
    fn ground_movement_stays_instant() {
        let mut player = settled(Vec3::new(0.5, 65.0, 0.5), flat_ground);
        let forward = MovementInput {
            forward: 1.0,
            ..Default::default()
        };
        player.update(forward, FIXED_DT, flat_ground);
        let walk = player.movement().walk_speed;
        assert!(
            (player.velocity.z + walk).abs() < 1.0e-4,
            "walking should reach full speed on the first tick; vz = {}",
            player.velocity.z
        );
    }

    #[test]
    fn starvation_drains_health_and_eating_restores_hunger() {
        let mut player = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        player.hunger = 0.0;
        player.saturation = 0.0;
        player.tick_survival(1.0, false);
        assert!(player.health < MAX_HEALTH, "starvation should hurt");

        player.feed(8.0, 4.0);
        assert!((player.hunger - 8.0).abs() < 1e-3);
        assert!(player.saturation <= player.hunger);
    }

    #[test]
    fn regen_when_well_fed() {
        let mut player = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        player.health = 10.0;
        player.hunger = MAX_HUNGER;
        player.saturation = MAX_HUNGER;
        player.tick_survival(1.0, false);
        assert!(player.health > 10.0, "well-fed players regenerate health");
    }

    #[test]
    fn switching_to_creative_clears_flight_rules_and_heals() {
        let mut player = test_player(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        player.health = 3.0;
        player.set_mode(GameMode::Creative);
        assert!(player.mode.can_fly());
        assert_eq!(player.health, MAX_HEALTH);

        player.flying = true;
        player.set_mode(GameMode::Survival);
        assert!(!player.flying, "leaving creative disables flight");
    }
}

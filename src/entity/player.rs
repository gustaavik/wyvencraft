//! Player state: transform, movement intent, and physics integration.

use glam::Vec3;

use crate::core::{Aabb, BlockPos, GameMode};
use crate::entity::physics::{self};

/// Player collision box dimensions.
pub const PLAYER_WIDTH: f32 = 0.6;
pub const PLAYER_HEIGHT: f32 = 1.8;
pub const PLAYER_EYE_HEIGHT: f32 = 1.62;

const GRAVITY: f32 = 28.0; // blocks/s^2
const JUMP_SPEED: f32 = 9.0;
const WALK_SPEED: f32 = 4.3;
const SPRINT_SPEED: f32 = 6.5;
const FLY_SPEED: f32 = 12.0;
const TERMINAL_VELOCITY: f32 = -60.0;

/// Full health/hunger (Minecraft-style: 20 = 10 hearts / 10 food shanks).
pub const MAX_HEALTH: f32 = 20.0;
pub const MAX_HUNGER: f32 = 20.0;
/// Falls shorter than this many blocks deal no damage.
const SAFE_FALL: f32 = 3.0;
/// Health lost per block fallen beyond [`SAFE_FALL`].
const FALL_DMG_PER_BLOCK: f32 = 1.0;
/// Hunger drained per second while idle / while sprinting (added).
const HUNGER_DRAIN_BASE: f32 = 0.05;
const HUNGER_DRAIN_SPRINT: f32 = 0.15;
/// At/above this hunger the player naturally regenerates health.
const REGEN_HUNGER_THRESHOLD: f32 = 18.0;
const REGEN_RATE: f32 = 1.0;
/// Health lost per second while at zero hunger (starvation).
const STARVE_RATE: f32 = 1.0;

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
    /// Highest Y reached since last leaving the ground; drives fall-damage.
    fall_peak_y: f32,
}

impl Player {
    pub fn new(position: Vec3, mode: GameMode) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: false,
            perspective: Perspective::First,
            mode,
            health: MAX_HEALTH,
            hunger: MAX_HUNGER,
            saturation: MAX_HUNGER,
            fall_peak_y: position.y,
        }
    }

    /// Eye position used for the camera and raycasting.
    pub fn eye_position(&self) -> Vec3 {
        self.position + Vec3::new(0.0, PLAYER_EYE_HEIGHT, 0.0)
    }

    /// Collision box in world space.
    pub fn aabb(&self) -> Aabb {
        let half = PLAYER_WIDTH * 0.5;
        Aabb::new(
            self.position - Vec3::new(half, 0.0, half),
            self.position + Vec3::new(half, PLAYER_HEIGHT, half),
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

    /// Advance one fixed simulation step.
    pub fn update(&mut self, input: MovementInput, dt: f32, is_solid: impl Fn(BlockPos) -> bool) {
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
            FLY_SPEED
        } else if input.sprint {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };

        self.velocity.x = wish.x * speed;
        self.velocity.z = wish.z * speed;

        if flying {
            let vertical = (input.jump as i32 - input.sneak as i32) as f32;
            self.velocity.y = vertical * FLY_SPEED;
        } else {
            self.velocity.y = (self.velocity.y - GRAVITY * dt).max(TERMINAL_VELOCITY);
            if input.jump && self.on_ground {
                self.velocity.y = JUMP_SPEED;
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
                if self.mode.takes_damage() && dist > SAFE_FALL {
                    self.damage((dist - SAFE_FALL) * FALL_DMG_PER_BLOCK);
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
        } else {
            self.velocity = Vec3::ZERO;
        }
    }

    /// Advance survival vitals one step (no-op semantics in creative — callers
    /// should only invoke this in survival). `sprinting` raises hunger drain.
    pub fn tick_survival(&mut self, dt: f32, sprinting: bool) {
        // Exertion drains the saturation buffer first, then hunger itself.
        let drain = (HUNGER_DRAIN_BASE + if sprinting { HUNGER_DRAIN_SPRINT } else { 0.0 }) * dt;
        if self.saturation > 0.0 {
            self.saturation = (self.saturation - drain).max(0.0);
        } else {
            self.hunger = (self.hunger - drain).max(0.0);
        }

        // Natural regeneration while well-fed.
        if self.hunger >= REGEN_HUNGER_THRESHOLD && self.health < MAX_HEALTH {
            self.health = (self.health + REGEN_RATE * dt).min(MAX_HEALTH);
            self.saturation = (self.saturation - drain).max(0.0);
        }

        // Starvation once hunger is fully depleted.
        if self.hunger <= 0.0 {
            self.health = (self.health - STARVE_RATE * dt).max(0.0);
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
            self.health = MAX_HEALTH;
            self.hunger = MAX_HUNGER;
            self.saturation = MAX_HUNGER;
        }
    }

    /// Apply damage (clamped, and only in a mode that takes damage).
    pub fn damage(&mut self, amount: f32) {
        if self.mode.takes_damage() {
            self.health = (self.health - amount).max(0.0);
        }
    }

    /// Restore health up to the maximum.
    pub fn heal(&mut self, amount: f32) {
        self.health = (self.health + amount).min(MAX_HEALTH);
    }

    /// Eat: restore hunger, and saturation up to the new hunger level.
    pub fn feed(&mut self, hunger: f32, saturation: f32) {
        self.hunger = (self.hunger + hunger).min(MAX_HUNGER);
        self.saturation = (self.saturation + saturation).min(self.hunger);
    }

    pub fn is_dead(&self) -> bool {
        self.mode.takes_damage() && self.health <= 0.0
    }

    /// Whether the player has room to eat (hunger below the maximum).
    pub fn is_hungry(&self) -> bool {
        self.hunger < MAX_HUNGER
    }

    /// Reset vitals and motion for a respawn at `position`.
    pub fn respawn_at(&mut self, position: Vec3) {
        self.position = position;
        self.velocity = Vec3::ZERO;
        self.health = MAX_HEALTH;
        self.hunger = MAX_HUNGER;
        self.saturation = MAX_HUNGER;
        self.fall_peak_y = position.y;
        self.on_ground = false;
    }

    pub fn toggle_perspective(&mut self) {
        self.perspective = self.perspective.next();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Regression: a player spawned flush on the ground at an integer Y (as
    /// [`find_spawn`](crate::state) produces) must come to rest instead of sinking
    /// through the world over successive ticks.
    #[test]
    fn spawned_player_rests_and_does_not_fall_through() {
        // Solid ground fills y < 65; spawn feet flush on the block top at y = 65.0.
        let solid = |p: BlockPos| p.y < 65;
        let mut player = Player::new(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
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
        let mut player = Player::new(Vec3::new(0.5, 85.0, 0.5), GameMode::Survival);
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

    /// A short fall (below the safe margin) deals no damage.
    #[test]
    fn short_fall_is_harmless() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = Player::new(Vec3::new(0.5, 67.0, 0.5), GameMode::Survival);
        let dt = 1.0 / 60.0;
        for _ in 0..240 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert_eq!(player.health, MAX_HEALTH, "a 2-block fall should be safe");
    }

    /// The same long fall in creative deals no damage (invulnerable).
    #[test]
    fn long_fall_is_harmless_in_creative() {
        let solid = |p: BlockPos| p.y < 65;
        let mut player = Player::new(Vec3::new(0.5, 85.0, 0.5), GameMode::Creative);
        let dt = 1.0 / 60.0;
        for _ in 0..600 {
            player.update(MovementInput::default(), dt, solid);
        }
        assert_eq!(player.health, MAX_HEALTH, "creative takes no fall damage");
    }

    #[test]
    fn starvation_drains_health_and_eating_restores_hunger() {
        let mut player = Player::new(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
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
        let mut player = Player::new(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        player.health = 10.0;
        player.hunger = MAX_HUNGER;
        player.saturation = MAX_HUNGER;
        player.tick_survival(1.0, false);
        assert!(player.health > 10.0, "well-fed players regenerate health");
    }

    #[test]
    fn switching_to_creative_clears_flight_rules_and_heals() {
        let mut player = Player::new(Vec3::new(0.5, 65.0, 0.5), GameMode::Survival);
        player.health = 3.0;
        player.set_mode(GameMode::Creative);
        assert!(player.mode.can_fly());
        assert_eq!(player.health, MAX_HEALTH);

        player.flying = true;
        player.set_mode(GameMode::Survival);
        assert!(!player.flying, "leaving creative disables flight");
    }
}

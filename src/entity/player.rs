//! Player state: transform, movement intent, and physics integration.

use glam::Vec3;

use crate::core::{Aabb, BlockPos};
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
}

impl Player {
    pub fn new(position: Vec3) -> Self {
        Self {
            position,
            velocity: Vec3::ZERO,
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            flying: false,
            perspective: Perspective::First,
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

        let speed = if self.flying {
            FLY_SPEED
        } else if input.sprint {
            SPRINT_SPEED
        } else {
            WALK_SPEED
        };

        self.velocity.x = wish.x * speed;
        self.velocity.z = wish.z * speed;

        if self.flying {
            let vertical = (input.jump as i32 - input.sneak as i32) as f32;
            self.velocity.y = vertical * FLY_SPEED;
        } else {
            self.velocity.y = (self.velocity.y - GRAVITY * dt).max(TERMINAL_VELOCITY);
            if input.jump && self.on_ground {
                self.velocity.y = JUMP_SPEED;
            }
        }

        let result = physics::move_and_collide(self.aabb(), self.velocity * dt, is_solid);
        self.position += result.delta;
        self.on_ground = result.on_ground;

        // Zero out velocity components that were blocked.
        if !self.flying {
            if result.on_ground && self.velocity.y < 0.0 {
                self.velocity.y = 0.0;
            }
        } else {
            self.velocity = Vec3::ZERO;
        }
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
        let mut player = Player::new(Vec3::new(0.5, 65.0, 0.5));
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
}

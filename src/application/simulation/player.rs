//! The local player: stepping them, hurting them, bringing them back.

use super::Simulation;
use crate::domain::entity::MovementInput;

impl Simulation {
    /// Step the local player one frame — unless they are dead, which is the one
    /// real freeze: there is nothing left to simulate, and a corpse sliding to
    /// a halt under the respawn dialog reads as a bug.
    ///
    /// Physics runs at a fixed rate on `frame_dt`, not on the frame delta, so
    /// jump height is the same at every framerate; survival vitals tick on the
    /// clamped `dt`. Returns how far the fixed step is into its next tick, for
    /// the camera and the body to interpolate by — `None` when frozen.
    pub fn step_player(&mut self, movement: MovementInput, frame_dt: f32, dt: f32) -> Option<f32> {
        if self.dead {
            return None;
        }
        // Refresh the worn defense first: `step_fixed` can raise fall damage
        // internally, and it must be mitigated by whatever is worn *now*.
        self.player.defense = self.inventory.total_defense(&self.rules.items);
        let health_before = self.player.health;
        let world = &self.world;
        let alpha = self
            .player
            .step_fixed(movement, frame_dt, &mut self.physics_accum, |p| {
                world.is_solid_for_collision(p)
            });
        // A health drop across the step means fall damage landed; the health
        // delta is the only signal that escapes it, and it's enough.
        if self.player.health < health_before {
            self.inventory.wear_armor(1);
        }

        // Survival vitals: hunger drain, regen, starvation.
        if self.player.mode.takes_damage() {
            self.player.tick_survival(dt, movement.sprint);
            if self.player.is_dead() {
                self.dead = true;
                self.breaking = None;
            }
        }
        Some(alpha)
    }

    /// Route damage to the local player: armor mitigates inside
    /// `Player::damage`, worn pieces take wear, and death freezes control.
    pub fn damage_local_player(&mut self, amount: f32) {
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

    /// Reset the player at the world spawn after death.
    pub fn respawn(&mut self) {
        self.player.respawn_at(self.spawn);
        self.dead = false;
        self.breaking = None;
    }

    /// Advance the local body's animation from how the player actually moved.
    ///
    /// Its legs follow real horizontal speed even with the inventory open:
    /// physics keeps running there, so a player who opened it mid-stride is
    /// still moving, and forcing the idle pose would have them gliding to a
    /// stop with their feet planted — in full view of the camera that just
    /// panned onto them.
    pub fn animate_player(&mut self, dt: f32) {
        let v = self.player.velocity;
        let motion = crate::domain::entity::Motion::new(
            glam::Vec3::new(v.x, 0.0, v.z).length(),
            v.y,
            !self.player.on_ground,
        );
        self.player_anim.advance(motion, self.player.yaw, dt);
    }
}

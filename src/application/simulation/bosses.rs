//! The parts of a boss's attacks that are pure simulation: arrows and
//! summoned minions. Hitting players and announcing the fight stay with
//! whoever runs the fight, since those talk to people.

use glam::Vec3;

use super::Simulation;
use crate::application::ecs::components::{Kind, Mob, Transform};
use crate::application::ecs::{With, spawn};
use crate::application::protocol::ServerMessage;
use crate::domain::core::Rng64;
use crate::domain::entity::MobId;

/// Where volley projectiles leave the boss, ahead of its face.
const MUZZLE: f32 = 0.8;

/// A fan of arrows, as a boss's `volley` attack spells it.
#[derive(Debug, Clone, Copy)]
pub struct Volley {
    pub damage: f32,
    pub count: u8,
    pub spread_deg: f32,
    pub speed: f32,
    pub gravity: f32,
    pub lifetime: f32,
}

impl Simulation {
    /// Loose a fan of `volley.count` arrows from `eye` at `target`, lofted to
    /// cancel their drop over the distance, `spread_deg` wide.
    pub fn loose_volley(&mut self, eye: Vec3, target: Vec3, volley: Volley) {
        let Volley {
            damage,
            count,
            spread_deg,
            speed,
            gravity,
            lifetime,
        } = volley;
        let to = target - eye;
        let lift = 0.5 * gravity * to.length() / speed.max(0.001);
        let dir = to.normalize_or_zero();
        for i in 0..count {
            let t = if count > 1 {
                f32::from(i) / f32::from(count - 1) - 0.5
            } else {
                0.0
            };
            let turned = glam::Quat::from_rotation_y((spread_deg * t).to_radians()) * dir;
            let velocity = turned * speed + Vec3::Y * lift;
            let origin = eye + turned * MUZZLE;
            self.emit(ServerMessage::ArrowSpawned {
                position: origin.to_array(),
                velocity: velocity.to_array(),
                gravity,
                lifetime,
            });
            spawn::arrow(&mut self.ecs, origin, velocity, damage, gravity, lifetime);
        }
    }

    /// Call between `count[0]` and `count[1]` of `entity` to the side of boss
    /// `boss` standing at `position`, never more than `cap` of them alive
    /// nearby at once. Seeded from the world, the boss and the id counter, so
    /// a replayed fight summons the same way.
    pub fn summon_minions(
        &mut self,
        boss: MobId,
        position: Vec3,
        entity: &str,
        count: [u8; 2],
        cap: u32,
    ) {
        let alive = self
            .ecs
            .query::<(&Kind, &Transform, With<Mob>)>()
            .filter(|(_, (kind, t, ()))| {
                kind.name == entity && t.position.distance(position) < 48.0
            })
            .count() as u32;
        let mut rng = Rng64::new(self.world.seed() ^ boss.0 ^ self.mobs.next_id);
        let want = rng.range_u32(u32::from(count[0]), u32::from(count[1]));
        for i in 0..want.min(cap.saturating_sub(alive)) {
            let angle = i as f32 * 2.1 + rng.range_f32(0.0, 1.0);
            let spot = position + Vec3::new(angle.cos(), 0.0, angle.sin()) * 3.0;
            let ground = self
                .find_ground(spot.x, spot.z, position.y as i32 + 6)
                .unwrap_or(position.y);
            self.spawn_mob(entity, Vec3::new(spot.x, ground, spot.z));
        }
    }
}

//! Which way, and how far, to something the player is heading for.

use glam::Vec3;

/// A target as seen from the player: turn `angle` radians (positive = the way
/// yaw increases, clockwise seen from above; zero = dead ahead) and walk
/// `distance` blocks.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bearing {
    pub angle: f32,
    pub distance: f32,
}

/// The bearing from `from`, facing `yaw`, to `to`. Horizontal only: a compass
/// does not care that the altar is up a hill.
///
/// Uses the player's own convention — facing `yaw` looks along
/// `(sin yaw, -cos yaw)` in `(x, z)`, see `Player::look_direction`.
pub fn bearing(from: Vec3, yaw: f32, to: Vec3) -> Bearing {
    let (dx, dz) = (to.x - from.x, to.z - from.z);
    let heading = dx.atan2(-dz);
    Bearing {
        angle: wrap(heading - yaw),
        distance: dx.hypot(dz),
    }
}

/// An angle folded into `(-π, π]`.
fn wrap(angle: f32) -> f32 {
    use std::f32::consts::{PI, TAU};
    let a = (angle + PI).rem_euclid(TAU) - PI;
    if a <= -PI { a + TAU } else { a }
}

#[cfg(test)]
mod tests {
    use std::f32::consts::{FRAC_PI_2, PI};

    use super::*;

    fn close(a: f32, b: f32) -> bool {
        (a - b).abs() < 1e-4
    }

    /// Yaw 0 faces −z, so a target straight down −z is dead ahead.
    #[test]
    fn a_target_straight_ahead_has_no_angle() {
        let b = bearing(Vec3::ZERO, 0.0, Vec3::new(0.0, 50.0, -10.0));
        assert!(close(b.angle, 0.0));
        assert!(close(b.distance, 10.0), "height is ignored");
    }

    /// The angle is the turn that would face it: turning by it and asking
    /// again gives zero.
    #[test]
    fn turning_by_the_angle_faces_the_target() {
        let from = Vec3::new(3.0, 0.0, 4.0);
        let to = Vec3::new(-20.0, 0.0, 17.0);
        for yaw in [0.0, 1.0, -2.5, 3.1] {
            let b = bearing(from, yaw, to);
            assert!(close(bearing(from, yaw + b.angle, to).angle, 0.0));
        }
    }

    #[test]
    fn a_target_behind_is_half_a_turn_away() {
        let b = bearing(Vec3::ZERO, 0.0, Vec3::new(0.0, 0.0, 10.0));
        assert!(close(b.angle.abs(), PI));
    }

    #[test]
    fn a_target_to_the_side_is_a_quarter_turn() {
        let b = bearing(Vec3::ZERO, 0.0, Vec3::new(10.0, 0.0, 0.0));
        assert!(close(b.angle.abs(), FRAC_PI_2));
    }

    #[test]
    fn angles_wrap_into_a_half_turn_either_way() {
        for yaw in [-20.0, -7.0, 0.0, 7.0, 20.0] {
            let b = bearing(Vec3::ZERO, yaw, Vec3::new(5.0, 0.0, 1.0));
            assert!(b.angle > -PI - 1e-4 && b.angle <= PI + 1e-4);
        }
    }
}

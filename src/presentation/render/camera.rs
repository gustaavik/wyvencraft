//! A [`Shot`] turned into a renderer [`Camera`].
//!
//! Where the camera *should* sit — the polar shot, its blend, and the
//! clearance trace that pulls it in front of a wall — is pure and lives in
//! [`crate::domain::entity::camera`]. Only this last step names
//! `wyven_render`, so only this step is presentation.

use glam::{Vec2, Vec3};
use wyven_render::Camera;

use crate::domain::entity::camera::Shot;

/// Resolving a [`Shot`] into a [`Camera`], as a method so call sites read
/// `shot.camera(..)` exactly as they did when it lived on `Shot` itself.
pub trait ShotCamera {
    /// Resolve to a camera. `distance` is what the world actually allowed —
    /// [`crate::domain::entity::camera::clear_distance`] along
    /// [`Shot::offset`] with `self.distance` desired.
    fn camera(self, eye: Vec3, yaw: f32, distance: f32, fov_degrees: f32, aspect: f32) -> Camera;
}

impl ShotCamera for Shot {
    fn camera(self, eye: Vec3, yaw: f32, distance: f32, fov_degrees: f32, aspect: f32) -> Camera {
        let offset = self.offset(yaw);
        let to_camera = offset * distance;
        let mut camera = Camera::new(fov_degrees, aspect);
        camera.position = eye + to_camera;
        // Look back at the aim point. With `aim == 0` this is exactly
        // `-offset` for any positive distance; the fallback covers first
        // person, where the distance is zero and there is no vector to
        // normalize, and a clamp that collapsed the camera onto the eye.
        camera.forward = (Vec3::Y * self.aim - to_camera)
            .try_normalize()
            .unwrap_or(-offset);
        camera.projection_offset = Vec2::new(self.shift, 0.0);
        camera
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::entity::Perspective;
    use crate::domain::entity::camera::INSPECT_AIM;

    const DESIRED: f32 = 4.0;

    /// The look direction a perspective's camera must face.
    fn look(yaw: f32, pitch: f32) -> Vec3 {
        let (sy, cy) = yaw.sin_cos();
        let (sp, cp) = pitch.sin_cos();
        Vec3::new(cp * sy, sp, -cp * cy).normalize()
    }

    /// First person is `ThirdBack` at zero distance, which has to land the
    /// camera exactly on the eye looking along the look direction — what the
    /// old `None` + `unwrap_or((Vec3::ZERO, look))` pair did explicitly.
    #[test]
    fn first_person_puts_the_camera_on_the_eye() {
        let eye = Vec3::new(3.0, 70.0, -2.0);
        for pitch_step in -4..=4 {
            let pitch = pitch_step as f32 * 0.35;
            let yaw = 1.1;
            let shot = Perspective::First.shot(pitch, DESIRED);
            assert_eq!(shot.distance, 0.0);

            let camera = shot.camera(eye, yaw, 0.0, 70.0, 16.0 / 9.0);
            assert!((camera.position - eye).length() < 1e-5);
            assert!(
                (camera.forward - look(yaw, pitch)).length() < 1e-5,
                "first person looks along the look direction"
            );
        }
    }

    /// The inspect shot stands in front of the player, looking back at them.
    #[test]
    fn the_inspect_shot_looks_at_the_players_face() {
        let eye = Vec3::new(0.0, 1.62, 0.0);
        for yaw_step in -4..=4 {
            let yaw = yaw_step as f32 * 0.7;
            let facing = look(yaw, 0.0);
            let shot = Shot::inspect(70f32.to_radians(), 0.17);
            let camera = shot.camera(eye, yaw, shot.distance, 70.0, 16.0 / 9.0);

            assert!(
                (camera.position - eye).dot(facing) > 0.0,
                "the camera must stand where the player is facing"
            );
            assert!(camera.forward.dot(facing) < 0.0, "and look back at them");
        }
    }

    /// The lens shift is what puts the model on the left, and it is derived
    /// from a screen fraction so it holds at any aspect ratio.
    #[test]
    fn the_inspect_shot_slides_the_subject_off_centre() {
        let eye = Vec3::new(0.0, 1.62, 0.0);
        let chest = eye + Vec3::Y * INSPECT_AIM;
        for aspect in [16.0 / 9.0, 4.0 / 3.0, 21.0 / 9.0] {
            let shot = Shot::inspect(70f32.to_radians(), 0.17);
            let camera = shot.camera(eye, 0.0, shot.distance, 70.0, aspect);
            let projected = camera.project(chest).expect("the chest is in front");
            assert!(
                (0.10..0.28).contains(&projected.x),
                "at aspect {aspect} the model landed at {}, not in the left third",
                projected.x
            );
        }
    }
}

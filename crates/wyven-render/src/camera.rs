//! View/projection matrices and frustum extraction.
//!
//! Uses a right-handed view with a `[0,1]` depth range and a flipped Y to match
//! Vulkan clip space (so no negative-height viewport is needed).

use glam::{Mat4, Vec2, Vec3};

use wyven_core::Frustum;

pub struct Camera {
    pub position: Vec3,
    /// Normalized forward look direction.
    pub forward: Vec3,
    pub fov_y: f32, // radians
    pub aspect: f32,
    pub near: f32,
    pub far: f32,
}

impl Camera {
    pub fn new(fov_y_degrees: f32, aspect: f32) -> Self {
        Self {
            position: Vec3::ZERO,
            forward: Vec3::NEG_Z,
            fov_y: fov_y_degrees.to_radians(),
            aspect,
            near: 0.1,
            far: 1000.0,
        }
    }

    pub fn set_aspect(&mut self, width: f32, height: f32) {
        self.aspect = if height > 0.0 { width / height } else { 1.0 };
    }

    /// Radius of the sphere through the near plane's four corners, measured from
    /// the eye. Nothing nearer than this is drawn, so it is exactly the clearance
    /// a camera needs in front of a surface to keep it out of frame.
    ///
    /// Derived from `near`, `fov_y` and `aspect` rather than tuned: the field of
    /// view is a player setting, and a padding sized for the shipped 70° would
    /// let a wide FOV clip through walls again.
    pub fn near_radius(&self) -> f32 {
        let half_h = self.near * (self.fov_y * 0.5).tan();
        let half_w = half_h * self.aspect;
        (self.near * self.near + half_h * half_h + half_w * half_w).sqrt()
    }

    pub fn view_matrix(&self) -> Mat4 {
        Mat4::look_to_rh(self.position, self.forward, Vec3::Y)
    }

    pub fn projection_matrix(&self) -> Mat4 {
        let mut proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far);
        // Flip Y for Vulkan's clip space (origin top-left, +Y down).
        proj.y_axis.y *= -1.0;
        proj
    }

    pub fn view_projection(&self) -> Mat4 {
        self.projection_matrix() * self.view_matrix()
    }

    /// Inverse of the projection times a *translation-free* view, for the sky
    /// pass. Unprojecting a clip-space point with this yields a pure world-space
    /// ray direction (independent of camera position), and it uses the same
    /// flipped projection as [`Self::view_projection`] so the sun aligns with the
    /// world geometry.
    pub fn sky_inv_view_proj(&self) -> Mat4 {
        let view_rotation = Mat4::look_to_rh(Vec3::ZERO, self.forward, Vec3::Y);
        (self.projection_matrix() * view_rotation).inverse()
    }

    /// Frustum for culling. Built from the *unflipped* view-proj so plane math
    /// stays in conventional world orientation.
    pub fn frustum(&self) -> Frustum {
        let proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far);
        Frustum::from_view_proj(proj * self.view_matrix())
    }

    /// Project a world point to normalized screen coordinates.
    ///
    /// Returns `(x, y)` in `[0,1]`, origin **top-left** — ready to multiply by a
    /// widget's size. `None` when the point is at or behind the eye plane, where
    /// the perspective divide is meaningless and would otherwise place a point
    /// behind you cheerfully in front of you.
    ///
    /// No Y flip is applied here because [`projection_matrix`](Self::projection_matrix)
    /// already carries one for Vulkan, which happens to be the same convention
    /// screen-space UI uses. A point may land outside `[0,1]`; that is a caller's
    /// business, since a nameplate slightly off-screen is still worth clipping
    /// rather than discarding.
    ///
    /// Kept in `render` and returning a plain [`Vec2`] so this module stays free
    /// of any UI dependency.
    pub fn project(&self, world: Vec3) -> Option<Vec2> {
        let clip = self.view_projection() * world.extend(1.0);
        if clip.w <= f32::EPSILON {
            return None;
        }
        let ndc = clip.truncate() / clip.w;
        Some(Vec2::new(ndc.x * 0.5 + 0.5, ndc.y * 0.5 + 0.5))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Looking down -Z from the origin, which is the orientation the projection
    /// math is written for.
    fn camera() -> Camera {
        let mut camera = Camera::new(70.0, 16.0 / 9.0);
        camera.position = Vec3::ZERO;
        camera.forward = -Vec3::Z;
        camera
    }

    #[test]
    fn a_point_straight_ahead_lands_in_the_centre() {
        let screen = camera().project(Vec3::new(0.0, 0.0, -10.0)).unwrap();

        assert!((screen.x - 0.5).abs() < 1.0e-5, "x was {}", screen.x);
        assert!((screen.y - 0.5).abs() < 1.0e-5, "y was {}", screen.y);
    }

    /// The convention that matters for drawing: y grows *downward*, matching
    /// screen space. Getting this backwards would put every nameplate under its
    /// player's feet.
    #[test]
    fn a_point_above_the_camera_projects_to_the_upper_half() {
        let screen = camera().project(Vec3::new(0.0, 2.0, -10.0)).unwrap();
        assert!(
            screen.y < 0.5,
            "expected the upper half, got y = {}",
            screen.y
        );
    }

    #[test]
    fn a_point_to_the_right_projects_to_the_right_half() {
        let screen = camera().project(Vec3::new(2.0, 0.0, -10.0)).unwrap();
        assert!(
            screen.x > 0.5,
            "expected the right half, got x = {}",
            screen.x
        );
    }

    /// Without the `w` check, a point behind the camera divides through to a
    /// plausible-looking on-screen position — so every player behind you would
    /// get a nameplate, mirrored.
    #[test]
    fn a_point_behind_the_camera_does_not_project() {
        assert!(camera().project(Vec3::new(0.0, 0.0, 10.0)).is_none());
        assert!(camera().project(Vec3::ZERO).is_none());
    }

    #[test]
    fn a_point_off_to_the_side_projects_outside_the_unit_range() {
        // Still a valid projection — clipping is the caller's decision.
        let screen = camera().project(Vec3::new(100.0, 0.0, -1.0)).unwrap();
        assert!(
            screen.x > 1.0,
            "expected off-screen right, got {}",
            screen.x
        );
    }

    /// The whole point of deriving it: a player who widens their FOV must get a
    /// wider berth around walls, or the third-person camera starts clipping again.
    #[test]
    fn the_near_radius_grows_with_the_field_of_view() {
        let narrow = Camera::new(70.0, 16.0 / 9.0).near_radius();
        let wide = Camera::new(110.0, 16.0 / 9.0).near_radius();

        assert!(wide > narrow, "110° gave {wide}, 70° gave {narrow}");
        assert!(
            narrow > 0.1,
            "the corner is further from the eye than the near plane itself, got {narrow}"
        );
    }

    #[test]
    fn projection_tracks_the_camera_as_it_turns() {
        let mut camera = camera();
        let behind = Vec3::new(0.0, 0.0, 10.0);
        assert!(camera.project(behind).is_none());

        camera.forward = Vec3::Z;
        let screen = camera.project(behind).expect("now in front");
        assert!((screen.x - 0.5).abs() < 1.0e-5);
    }
}

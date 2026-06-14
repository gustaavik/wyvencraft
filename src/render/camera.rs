//! View/projection matrices and frustum extraction.
//!
//! Uses a right-handed view with a `[0,1]` depth range and a flipped Y to match
//! Vulkan clip space (so no negative-height viewport is needed).

use glam::{Mat4, Vec3};

use crate::core::Frustum;

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

    /// Frustum for culling. Built from the *unflipped* view-proj so plane math
    /// stays in conventional world orientation.
    pub fn frustum(&self) -> Frustum {
        let proj = Mat4::perspective_rh(self.fov_y, self.aspect, self.near, self.far);
        Frustum::from_view_proj(proj * self.view_matrix())
    }
}

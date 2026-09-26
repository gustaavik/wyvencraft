//! Turning simulation state into meshes: the box and rigged entity models, the
//! first-person view model, and the camera built from a [`crate::domain::entity::camera::Shot`].
//!
//! Everything here names `wyven_render`, which is exactly why it is not in
//! `domain`: the rules for how a body moves or where a camera should sit are
//! pure, and only turning them into vertices needs the renderer's types.

pub mod camera;
pub mod model;
pub mod rigged;
pub mod viewmodel;

#[cfg(test)]
mod meshing_tests;

pub use model::{HumanoidModel, ModelBox, QuadrupedModel};
pub use rigged::{Character, HeadLook, HumanoidRig};
pub use viewmodel::HandPose;

#[cfg(test)]
mod tests {
    /// The biome rules restate the identity tint rather than naming the
    /// renderer; this is what keeps the two spellings the same value.
    #[test]
    fn the_domain_identity_tint_is_the_renderers() {
        assert_eq!(
            crate::domain::world::generation::config::NO_TINT,
            wyven_render::NO_TINT
        );
    }
}

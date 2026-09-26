//! The components entities are made of.

use glam::Vec3;

use crate::domain::core::Aabb;
use crate::domain::entity::dropped_item::centred_aabb;
use crate::domain::entity::kind::PhysicsParams;

pub use crate::domain::entity::{ItemDrop, Projectile};

/// Where an entity is and which way it faces.
///
/// `position` is the body's anchor: the centre for small free bodies (drops,
/// arrows), which is what [`Body::aabb`] assumes.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transform {
    pub position: Vec3,
    /// Radians about +Y, 0 = facing -Z.
    pub yaw: f32,
    pub pitch: f32,
}

impl Transform {
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }
}

/// Blocks per second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity(pub Vec3);

/// A collision body, tuned by its entity kind's `[entity.physics]`.
#[derive(Debug, Clone, Copy)]
pub struct Body(pub PhysicsParams);

impl Body {
    /// The collision box of a body centred on `position`.
    pub fn aabb(&self, position: Vec3) -> Aabb {
        centred_aabb(position, &self.0)
    }
}

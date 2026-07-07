//! Entities and their physics: the local player, remote players, and the
//! collision/movement code they share. Entity tuning is data-driven via the
//! [`kind::EntityRegistry`] loaded from `assets/entities.toml`.

pub mod animation;
pub mod dropped_item;
pub mod kind;
pub mod model;
pub mod physics;
pub mod player;

pub use animation::AnimationState;
pub use dropped_item::DroppedItem;
pub use kind::{EntityKind, EntityRegistry};
pub use model::{HumanoidModel, ModelBox, Pose};
pub use player::{MovementInput, Perspective, Player};

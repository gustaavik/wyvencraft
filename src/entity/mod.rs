//! Entities and their physics: the local player, remote players, and the
//! collision/movement code they share.

pub mod animation;
pub mod dropped_item;
pub mod model;
pub mod physics;
pub mod player;

pub use animation::AnimationState;
pub use dropped_item::{DROP_SIZE, DroppedItem};
pub use model::{HumanoidModel, ModelBox, Pose};
pub use player::{MovementInput, Perspective, Player};

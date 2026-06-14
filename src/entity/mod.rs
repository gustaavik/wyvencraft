//! Entities and their physics: the local player, remote players, and the
//! collision/movement code they share.

pub mod model;
pub mod physics;
pub mod player;

pub use model::{HumanoidModel, ModelBox};
pub use player::{MovementInput, Perspective, Player};

//! Entities and their physics: the local player, remote players, and the
//! collision/movement code they share. Entity tuning is data-driven via the
//! [`kind::EntityRegistry`] loaded from `assets/entities.toml`.

pub mod animation;
pub mod boss;
pub mod brain;
pub mod camera;
pub mod dropped_item;
pub mod kind;
pub mod mob;
pub mod physics;
pub mod player;
pub mod projectile;
pub mod spawning;

pub use animation::{AnimationState, Motion, Pose};
pub use brain::{Perception, PlayerSighting};
pub use dropped_item::DroppedItem;
pub use kind::{EntityKind, EntityRegistry};
pub use mob::{Mob, MobAction, MobId};
pub use player::{MovementInput, Perspective, Player};
pub use projectile::Arrow;
pub use spawning::{SpawnConfig, SpawnRequest, Spawner};

//! The session's entities, stored in a [`wyven_ecs::Ecs`].
//!
//! - [`components`] — the data. Shared bodies ([`components::Transform`],
//!   [`components::Velocity`], [`components::Body`]) plus the domain types that
//!   say what an entity *is* ([`crate::domain::entity::ItemDrop`],
//!   [`crate::domain::entity::Projectile`]).
//! - [`spawn`] — the bundles each kind of entity is created with, so there is
//!   one spelling of "a dropped item" and one of "an arrow".
//! - [`systems`] — plain functions over the store. Each takes exactly what it
//!   reads and returns what it could not apply itself (a hit, a pickup), so
//!   every one is testable with an `Ecs` and nothing else.
//!
//! Conventions:
//! - Components are data. Rules live on the domain types, and a system is the
//!   loop that applies them.
//! - A system never reaches for a world, a session or a screen it was not
//!   handed. That is what keeps the dependency visible where it is called.
//! - Structural changes made mid-iteration go through a
//!   [`wyven_ecs::CommandBuffer`] and are applied before the system returns.

pub mod components;
pub mod spawn;
pub mod systems;

pub use wyven_ecs::{CommandBuffer, Ecs, Entity, With, Without};

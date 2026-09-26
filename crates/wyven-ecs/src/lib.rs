//! A small entity-component store.
//!
//! - [`Entity`] — a generational handle, so a handle kept past its entity's
//!   death never aliases whatever takes the slot next.
//! - [`SparseSet`] — one per component type: O(1) insert/remove/lookup and
//!   contiguous iteration.
//! - [`Ecs::query`] / [`Ecs::for_each_mut`] — tuple queries with `&T`,
//!   `&mut T`, `Option<..>`, [`With`] and [`Without`] terms.
//! - [`CommandBuffer`] — spawns and despawns recorded mid-iteration and
//!   applied after it.
//!
//! What it deliberately does not have: a scheduler, global resources, change
//! detection, or parallelism. Systems are plain functions with explicit
//! parameters, so what a system touches is spelled out where it is called —
//! and so each one is testable with nothing but an `Ecs` and its inputs.
//!
//! Nothing here knows what a component *means*. Like every `wyven-*` crate,
//! it is engine code: the game decides what a position or a mob is.
//!
//! ```
//! use wyven_ecs::{CommandBuffer, Ecs, Without};
//!
//! struct Position(f32);
//! struct Velocity(f32);
//! struct Health(u32);
//! struct Frozen;
//!
//! let mut ecs = Ecs::new();
//! let mover = ecs.spawn((Position(0.0), Velocity(2.0), Health(1)));
//! ecs.spawn((Position(5.0), Velocity(1.0), Frozen));
//!
//! // A system: a plain function over the store.
//! ecs.for_each_mut::<(&mut Position, &Velocity, Without<Frozen>), _>(|_, (p, v, ())| {
//!     p.0 += v.0;
//! });
//! assert_eq!(ecs.get::<Position>(mover).map(|p| p.0), Some(2.0));
//!
//! // Structural changes wait until iteration is over.
//! let mut commands = CommandBuffer::new();
//! for (entity, (health,)) in ecs.query::<(&Health,)>() {
//!     if health.0 > 0 {
//!         commands.despawn(entity);
//!     }
//! }
//! ecs.apply(&mut commands);
//! assert!(!ecs.is_alive(mover));
//! ```

#![forbid(unsafe_code)]

mod bundle;
mod commands;
mod ecs;
mod entity;
pub mod query;
mod sparse_set;
mod storage;

pub use bundle::Bundle;
pub use commands::CommandBuffer;
pub use ecs::Ecs;
pub use entity::Entity;
pub use query::{Query, QueryMut, With, Without};
pub use sparse_set::SparseSet;

#[cfg(test)]
mod tests;

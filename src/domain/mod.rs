//! The rules of the game: what a block, an item, a mob or a biome *is*, and
//! how they behave. Pure — no filesystem, no sockets, no GPU, no egui — which
//! `tests/architecture.rs` enforces. Everything else in the crate depends on
//! this layer; it depends on nothing but engine primitives.

pub mod chat;
pub mod content;
pub mod core;
pub mod entity;
pub mod inventory;
pub mod progression;
pub mod world;

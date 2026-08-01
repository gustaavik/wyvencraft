//! Wyvencraft — a Minecraft-style voxel sandbox engine.
//!
//! The crate is organised into domain modules with deliberately one-directional
//! dependencies:
//!
//! ```text
//! core  ← (everything)
//! render ← core
//! world  ← core, render(mesh/vertex)
//! entity ← core
//! inventory ← core, world
//! content ← world, inventory   (registries loaded from assets/*.toml)
//! input  ← core, config, entity
//! net    ← core
//! save   ← core, world, inventory, entity   (world/player persistence)
//! ui     ← inventory, net, (egui)
//! state  ← all of the above
//! app    ← state, content, render, config   (owns the window + event loop)
//! ```

pub mod app;
pub mod boot;
pub mod config;
pub mod content;
pub mod core;
pub mod entity;
pub mod input;
pub mod inventory;
pub mod net;
pub mod render;
pub mod save;
pub mod state;
pub mod ui;
pub mod world;

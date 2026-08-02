//! Wyvencraft — a Minecraft-style voxel sandbox engine.
//!
//! The crate is organised into domain modules with deliberately one-directional
//! dependencies:
//!
//! ```text
//! core  ← (everything)
//! render ← core
//! model  ← core, render        (.gltf/.bbmodel files → geometry + texture)
//! world  ← core, render(mesh/vertex)
//! entity ← core, model
//! inventory ← core, world, model
//! content ← world, inventory, model   (registries loaded from assets/*.toml)
//! input  ← core, config, entity
//! net    ← core
//! save   ← core, world, inventory, entity   (world/player persistence)
//! ui     ← inventory, net, (egui)
//! state  ← all of the above
//! boot   ← core, net, save     (pure env → BootPlan; no window or GPU)
//! app    ← state, boot, content, render, config   (owns the window + event loop)
//! ```
//!
//! I/O boundaries are crossed through ports, each with a real implementation and
//! a test double: [`content::ContentSource`], [`save::WorldRepository`],
//! [`state::session::Session`], [`boot::Environment`]. Hot paths (meshing, chunk
//! generation, fluid ticking) deliberately use none — the indirection buys
//! nothing there and would cost frame time.

pub mod app;
pub mod boot;
pub mod config;
pub mod content;
pub mod core;
pub mod entity;
pub mod input;
pub mod inventory;
pub mod model;
pub mod net;
pub mod render;
pub mod save;
pub mod state;
pub mod ui;
pub mod world;

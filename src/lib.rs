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
//! chat   ← core, net           (message log, command parsing, the ops list)
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
//!
//! [`chat::CommandContext`] is a port for a different reason: not I/O, but to
//! invert a dependency. Chat commands are policy and belong in `chat`, yet they
//! act on registries and inventories owned by `state` — which already depends on
//! `chat`. Commands depend on the port; `state` implements it.

pub use wyven_assets;
pub use wyven_core;

pub mod app;
pub mod auth;
pub mod boot;
pub mod chat;
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

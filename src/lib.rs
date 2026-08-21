//! Wyvencraft — a Minecraft-style voxel sandbox.
//!
//! This crate is the **game**. The engine it is built on is the `wyven-*`
//! workspace members under `crates/`, which know nothing about grass, zombies or
//! survival mode:
//!
//! ```text
//! wyven-core     coordinate/voxel types, math, RNG, frame timing
//! wyven-assets   AssetSource port (Fs/Embedded/Map), PNG decoding
//! wyven-render   Vulkan: context, pipelines, meshes, textures, atlas, camera
//! wyven-model    .gltf / .bbmodel / Blockbench block JSON -> ModelMesh
//! wyven-voxel    chunk store, background loader, culled mesher, raycast, World
//! wyven-net      renet transport, generic over the protocol and the join gate
//! wyven-input    winit events -> frame-coherent InputState
//! wyven-auth     account sessions, key cache, Ed25519 ticket verification
//! wyven-app      window, egui, event loop, screen stack
//! ```
//!
//! The dependency direction is one-way and enforced by cargo, not by
//! convention: no `wyven-*` crate lists this one, so a violation stops
//! compiling rather than being noticed in review.
//!
//! # Where the line sits
//!
//! Anything a *different* game would want unchanged is engine. Anything that
//! encodes what Wyvencraft is — a block's name and hardness, a 20-minute day,
//! survival versus creative, what a pickaxe is for — is here. Five traits carry
//! the meaning across:
//!
//! | Trait | Declared by | Implemented here by |
//! |---|---|---|
//! | [`wyven_render::TileSource`] | render | [`art::WyvencraftArt`] |
//! | [`wyven_voxel::BlockCatalog`] | voxel | [`content::BlockAppearance`] |
//! | [`wyven_voxel::BlockProperties`] | voxel | [`world::BlockRegistry`] |
//! | [`wyven_voxel::WorldGenerator`] | voxel | [`world::NoiseGenerator`] |
//! | [`wyven_net::Protocol`] / [`wyven_net::JoinVerifier`] | net | [`net::WyvenProtocol`] / [`net::TicketJoin`] |
//! | [`wyven_app::Game`] | app | [`state::Wyvencraft`] |
//!
//! # This crate's own modules
//!
//! ```text
//! core      ← wyven-core + GameMode and DayCycle (rules, not primitives)
//! art       ← render        procedural tiles, skin, armor and mob sheets
//! world     ← voxel         block table, worldgen, fluid rules
//! inventory ← world         items, stacks, crafting, mining
//! entity    ← inventory     player, physics, mobs, brains, projectiles
//! content   ← all of it     registries loaded from assets/*.toml
//! chat      ← net           message log, commands, the ops list
//! save      ← world, entity world/player persistence under saves/
//! ui        ← inventory     HUD and inventory egui views
//! net       ← wyven-net     the wire protocol and the join gate
//! config    ← wyven-input   settings, keybinds, and the movement intent
//! boot      ← save, net     pure env -> BootPlan, then plan -> first screen
//! state     ← everything    the screens, and the Game impl that starts them
//! app       ← state         twenty lines: name the game, hand it to the runner
//! ```
//!
//! I/O boundaries are crossed through ports, each with a real implementation and
//! a test double: [`content::ContentSource`], [`save::WorldRepository`],
//! [`state::session::Session`], [`boot::Environment`]. Hot paths (meshing, chunk
//! generation, fluid ticking) deliberately use none — the indirection buys
//! nothing there and would cost frame time, which is why `mesh_chunk` takes
//! `&impl BlockCatalog` and not `&dyn`.
//!
//! [`chat::CommandContext`] is a port for a different reason: not I/O, but to
//! invert a dependency. Chat commands are policy and belong in `chat`, yet they
//! act on registries and inventories owned by `state` — which already depends on
//! `chat`. Commands depend on the port; `state` implements it.

pub use wyven_app;
pub use wyven_assets;
pub use wyven_auth;
pub use wyven_core;
pub use wyven_input;
pub use wyven_model;
pub use wyven_net;
pub use wyven_render;
pub use wyven_voxel;

pub mod app;
pub mod art;
pub mod boot;
pub mod chat;
pub mod config;
pub mod content;
pub mod core;
pub mod entity;
pub mod inventory;
pub mod net;
pub mod save;
pub mod state;
pub mod ui;
pub mod world;

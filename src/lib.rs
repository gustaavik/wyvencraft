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
//! wyven-audio    audio device output and mixing
//! wyven-ecs      entity-component store: generational handles, queries, commands
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
//! | [`wyven_render::TileSource`] | render | [`presentation::art::WyvencraftArt`] |
//! | [`wyven_voxel::BlockCatalog`] | voxel | [`presentation::content::BlockAppearance`] |
//! | [`wyven_voxel::BlockProperties`] | voxel | [`domain::world::BlockRegistry`] |
//! | [`wyven_voxel::WorldGenerator`] | voxel | [`domain::world::NoiseGenerator`] |
//! | [`wyven_net::Protocol`] / [`wyven_net::JoinVerifier`] | net | [`infrastructure::net::WyvenProtocol`] / [`infrastructure::net::TicketJoin`] |
//! | [`wyven_app::Game`] | app | [`presentation::screens::Wyvencraft`] |
//!
//! # This crate's layers
//!
//! Inside the game the dependencies point inward, and `tests/architecture.rs`
//! fails the build of the test suite on any import that points outward:
//!
//! ```text
//! presentation ─┐
//!               ├─→ application ─→ domain
//! infrastructure┘
//! ```
//!
//! ```text
//! domain            the rules — pure: no files, sockets, GPU or egui
//!   core            wyven-core + GameMode, DayCycle, ids, seeds
//!   world           block table, worldgen, structures, fluid rules
//!   inventory       the one item registry and its capabilities, stacks, crafting
//!   entity          player, physics, mobs, brains, bosses, projectiles, animation
//!   progression     shrines read, altars revealed, bosses beaten; compass bearings
//!   chat            message log, commands, the ops list's rules
//!   content         Registries: every hashed definition, the content hash, reference checks
//! application       use cases and the ports they need
//!   protocol        the wire messages: the contract between peers
//!   session         the Session port (who decides, what arrived) + FakeSession
//!   sync            remote-player snapshot smoothing
//!   simulation      Simulation: the world, its ECS, the local player, and the use cases on them
//!   ecs             components, spawn bundles and systems over wyven_ecs
//!   networking      Networking: the session, its peers and the ops list
//!   boot_plan       pure env -> BootPlan
//!   content         loading the Registries from assets/*.toml through the asset port
//! infrastructure    adapters: files, sockets, audio devices, assets/
//!   save            world/player persistence under saves/
//!   net             transports, sessions, the join gate, server list, status probe
//!   audio           sound registry, AudioManager, menu-music timing
//!   paths/fs/profile/ops/recipes/desktop   the data dir and what lives in it
//! presentation      everything that draws or reads input
//!   screens         the Screen impls, and the Game impl that starts them
//!   content         GameContent = Registries + Visuals (textures, models, icons, labels) + sounds
//!   ui              egui views
//!   render          entity meshes, the view model, the camera
//!   art             PNG tiles; atlas layout for skin, armor, mob and crack sheets
//!   config          settings, keybinds, the movement intent
//!   editor          the F6 item placement editor
//! boot, app         the composition root: the only code that names concrete
//!                   adapters and wires them into the first screen
//! ```
//!
//! I/O boundaries are crossed through ports, each with a real implementation and
//! a test double: [`presentation::content::ContentSource`],
//! [`infrastructure::save::WorldRepository`], [`application::session::Session`],
//! [`application::boot_plan::Environment`], [`wyven_audio::AudioBackend`]
//! (`RodioBackend`/`NullAudioBackend`, chosen once in
//! [`presentation::screens::Wyvencraft`]'s startup via
//! [`infrastructure::audio::open_default_backend`]). Hot paths (meshing, chunk
//! generation, fluid ticking) deliberately use none — the indirection buys
//! nothing there and would cost frame time, which is why `mesh_chunk` takes
//! `&impl BlockCatalog` and not `&dyn`.
//!
//! [`domain::chat::CommandContext`] is a port for a different reason: not I/O,
//! but to invert a dependency. Chat commands are policy and belong in the
//! domain, yet they act on registries and inventories owned by the in-game
//! screen — which already depends on `chat`. Commands depend on the port; the
//! screen implements it.

pub use wyven_app;
pub use wyven_assets;
pub use wyven_audio;
pub use wyven_auth;
pub use wyven_core;
pub use wyven_ecs;
pub use wyven_input;
pub use wyven_model;
pub use wyven_net;
pub use wyven_render;
pub use wyven_voxel;

pub mod app;
pub mod application;
pub mod boot;
pub mod domain;
pub mod infrastructure;
pub mod presentation;

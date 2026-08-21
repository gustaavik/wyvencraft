//! Vulkan rendering layer (built on `vulkano`).
//!
//! Boundaries: this layer knows about GPU resources, pipelines, and meshes but
//! NOT about the game world — callers hand it [`mesh::CpuMesh`] data and camera
//! state. That keeps `render` free of any dependency on `world`/`entity`.

pub mod armor;
pub mod block_textures;
pub mod camera;
pub mod context;
pub mod debug;
pub mod icons;
pub mod mesh;
pub mod mobskin;
pub mod pipeline;
pub mod renderer;
pub mod shaders;
pub mod skin;
pub mod texture;
pub mod tile_registry;
pub mod tiles;
pub mod vertex;

pub use block_textures::{BlockTextureArray, BlockTextureSet};
pub use camera::Camera;
pub use context::RenderContext;
pub use mesh::{CpuMesh, GpuLines, GpuMesh};
pub use renderer::{LightParams, PreviewFrame, Renderer, SceneFrame, SkyParams, TexturedMesh};
pub use texture::{Rgba8, Texture};
pub use tile_registry::{TileEntry, TileRegistry};
pub use vertex::{ChunkVertex, EntityVertex, LineVertex, NO_TINT, anim_flags};

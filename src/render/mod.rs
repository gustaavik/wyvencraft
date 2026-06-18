//! Vulkan rendering layer (built on `vulkano`).
//!
//! Boundaries: this layer knows about GPU resources, pipelines, and meshes but
//! NOT about the game world — callers hand it [`mesh::CpuMesh`] data and camera
//! state. That keeps `render` free of any dependency on `world`/`entity`.

pub mod camera;
pub mod context;
pub mod debug;
pub mod mesh;
pub mod pipeline;
pub mod renderer;
pub mod shaders;
pub mod texture;
pub mod vertex;

pub use camera::Camera;
pub use context::RenderContext;
pub use mesh::{CpuMesh, GpuMesh};
pub use renderer::{LightParams, Renderer, SceneFrame, SkyParams};
pub use texture::TextureAtlas;
pub use vertex::{ChunkVertex, EntityVertex, LineVertex};

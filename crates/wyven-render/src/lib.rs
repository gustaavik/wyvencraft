//! Vulkan rendering: the GPU half of the engine.
//!
//! Boundaries: this layer knows about GPU resources, pipelines, and meshes but
//! NOT about the game world — callers hand it [`mesh::CpuMesh`] data and camera
//! state. That keeps `render` free of any dependency on `world`/`entity`.
//!
//! It knows nothing about the *art* either. [`TileRegistry`] allocates atlas
//! slots and assembles pixels; where a named tile's art comes from is a
//! [`TileSource`] the caller injects.

pub mod block_textures;
pub mod camera;
pub mod context;
pub mod debug;
pub mod icons;
pub mod mesh;
pub mod pipeline;
pub mod renderer;
pub mod shaders;
pub mod texture;
pub mod tile_registry;
pub mod vertex;

pub use block_textures::{BlockTextureArray, BlockTextureSet};
pub use camera::Camera;
pub use context::RenderContext;
pub use mesh::{CpuMesh, GpuLines, GpuMesh};
pub use renderer::{
    ForegroundFrame, LightParams, PreviewFrame, Renderer, SceneFrame, SkyParams, TexturedMesh,
};
pub use texture::{Rgba8, Texture};
pub use tile_registry::{
    MISSING_TEXTURE, NoTiles, ReservedTiles, TileEntry, TileRegistry, TileRgba, TileSource,
    decode_tile,
};
pub use vertex::{ChunkVertex, EntityVertex, LineVertex, NO_OVERLAY, NO_TINT, anim_flags};

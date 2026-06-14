//! Background chunk streaming: generates and meshes chunks around the player on
//! worker threads so the render loop never stalls.
//!
//! Design: *producer/consumer*. A pool of workers ([`rayon`]) pulls
//! generate/mesh jobs and pushes finished results back over a
//! [`crossbeam_channel`]; the main thread drains results and uploads meshes.
//!
//! NOTE: implemented in milestone M3. The synchronous path on
//! [`crate::world::World::ensure_chunk`] is used until then.

use crate::core::ChunkPos;

/// A finished generation/meshing job ready for GPU upload on the main thread.
pub struct ChunkLoadResult {
    pub pos: ChunkPos,
    // Filled in M3: generated chunk data + CPU mesh output.
}

/// Owns the worker pool and result channel. Placeholder until M3.
#[derive(Default)]
pub struct ChunkLoader {
    _private: (),
}

impl ChunkLoader {
    pub fn new() -> Self {
        Self::default()
    }
}

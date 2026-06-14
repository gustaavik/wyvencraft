//! The voxel world: a map of loaded chunks plus block-level access helpers.
//!
//! Chunk *streaming* (background generation/meshing around players) is layered on
//! top of this in [`crate::world::loader`]; `World` itself stays a synchronous,
//! testable data structure.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::core::{BlockId, BlockPos, ChunkPos};
use crate::world::block::BlockRegistry;
use crate::world::chunk::Chunk;
use crate::world::generation::WorldGenerator;

pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
    generator: Box<dyn WorldGenerator>,
    registry: Arc<BlockRegistry>,
    /// Chunks whose mesh is stale (need (re)building on the GPU).
    dirty: HashSet<ChunkPos>,
}

impl World {
    pub fn new(generator: Box<dyn WorldGenerator>, registry: Arc<BlockRegistry>) -> Self {
        Self {
            chunks: HashMap::new(),
            generator,
            registry,
            dirty: HashSet::new(),
        }
    }

    pub fn seed(&self) -> u64 {
        self.generator.seed()
    }

    pub fn registry(&self) -> &Arc<BlockRegistry> {
        &self.registry
    }

    pub fn is_loaded(&self, pos: ChunkPos) -> bool {
        self.chunks.contains_key(&pos)
    }

    pub fn chunk(&self, pos: ChunkPos) -> Option<&Chunk> {
        self.chunks.get(&pos)
    }

    pub fn loaded_chunks(&self) -> impl Iterator<Item = &Chunk> {
        self.chunks.values()
    }

    pub fn loaded_count(&self) -> usize {
        self.chunks.len()
    }

    /// Insert an already-built chunk (from a generator thread or the network).
    pub fn insert_chunk(&mut self, chunk: Chunk) {
        let pos = chunk.pos;
        self.chunks.insert(pos, chunk);
        self.mark_dirty_with_neighbors(pos);
    }

    /// Generate a chunk synchronously if not present. Convenient for tests and
    /// the bootstrap path; the main game uses the async loader.
    pub fn ensure_chunk(&mut self, pos: ChunkPos) {
        if !self.chunks.contains_key(&pos) {
            let chunk = self.generator.generate(pos);
            self.insert_chunk(chunk);
        }
    }

    pub fn unload_chunk(&mut self, pos: ChunkPos) {
        self.chunks.remove(&pos);
        self.dirty.remove(&pos);
    }

    /// Block id at a world position; [`BlockId::AIR`] if the chunk isn't loaded
    /// or the position is out of vertical range.
    pub fn block_at(&self, pos: BlockPos) -> BlockId {
        let Some(local) = pos.to_local() else {
            return BlockId::AIR;
        };
        match self.chunks.get(&pos.chunk()) {
            Some(chunk) => chunk.get(local),
            None => BlockId::AIR,
        }
    }

    /// Whether the block at `pos` collides with entities / can be targeted.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        self.registry.get(self.block_at(pos)).solid
    }

    /// Place/replace a block. Returns the previous block, or `None` if the chunk
    /// isn't loaded. Marks affected chunk meshes dirty.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> Option<BlockId> {
        let local = pos.to_local()?;
        let chunk_pos = pos.chunk();
        let prev = self.chunks.get_mut(&chunk_pos)?.set(local, block);
        if prev != block {
            self.mark_dirty_with_neighbors(chunk_pos);
        }
        Some(prev)
    }

    /// Mark a chunk and its 4 horizontal neighbours dirty (a border edit changes
    /// the neighbour's culled faces too).
    fn mark_dirty_with_neighbors(&mut self, pos: ChunkPos) {
        self.dirty.insert(pos);
        for (dx, dz) in [(-1, 0), (1, 0), (0, -1), (0, 1)] {
            let n = ChunkPos::new(pos.x + dx, pos.z + dz);
            if self.chunks.contains_key(&n) {
                self.dirty.insert(n);
            }
        }
    }

    /// Take the current set of dirty chunk positions (and clear it). The caller
    /// is responsible for rebuilding their meshes.
    pub fn take_dirty(&mut self) -> Vec<ChunkPos> {
        self.dirty.drain().collect()
    }
}

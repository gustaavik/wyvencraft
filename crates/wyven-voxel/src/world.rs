//! The voxel world: a map of loaded chunks plus block-level access helpers.
//!
//! Chunk *streaming* (background generation/meshing around players) is layered on
//! top of this in [`crate::loader`]; `World` itself stays a synchronous,
//! testable data structure.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use crate::catalog::BlockProperties;
use crate::chunk::Chunk;
use crate::generate::WorldGenerator;
use wyven_core::{BlockId, BlockPos, ChunkPos, LocalPos};

pub struct World {
    chunks: HashMap<ChunkPos, Chunk>,
    generator: Arc<dyn WorldGenerator>,
    blocks: Arc<dyn BlockProperties>,
    /// Chunks whose mesh is stale (need (re)building on the GPU).
    dirty: HashSet<ChunkPos>,
    /// Persistent record of every edit that diverges from generated terrain,
    /// keyed by chunk then local position. Survives chunk unload/reload (it is
    /// re-applied in [`World::insert_chunk`]) and is the authoritative set a host
    /// replays to joining peers. Independent of which chunks are currently loaded.
    edits: HashMap<ChunkPos, HashMap<LocalPos, BlockId>>,
}

impl World {
    pub fn new(generator: Arc<dyn WorldGenerator>, blocks: Arc<dyn BlockProperties>) -> Self {
        Self {
            chunks: HashMap::new(),
            generator,
            blocks,
            dirty: HashSet::new(),
            edits: HashMap::new(),
        }
    }

    /// Shareable handle to the generator (for background streaming workers).
    pub fn generator(&self) -> Arc<dyn WorldGenerator> {
        self.generator.clone()
    }

    pub fn seed(&self) -> u64 {
        self.generator.seed()
    }

    /// The predicate table this world reads block ids through.
    pub fn blocks(&self) -> &Arc<dyn BlockProperties> {
        &self.blocks
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

    pub fn loaded_positions(&self) -> impl Iterator<Item = ChunkPos> + '_ {
        self.chunks.keys().copied()
    }

    pub fn loaded_count(&self) -> usize {
        self.chunks.len()
    }

    /// Insert an already-built chunk (from a generator thread or the network).
    /// Re-applies any recorded edits for this position so edits survive a chunk
    /// being unloaded and regenerated (and apply to chunks that streamed in after
    /// the edit was received).
    pub fn insert_chunk(&mut self, mut chunk: Chunk) {
        let pos = chunk.pos;
        if let Some(overlay) = self.edits.get(&pos) {
            for (&local, &block) in overlay {
                chunk.set(local, block);
            }
        }
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

    /// Whether the block at `pos` collides with entities.
    pub fn is_solid(&self, pos: BlockPos) -> bool {
        self.blocks.is_solid(self.block_at(pos))
    }

    /// Whether the block at `pos` can be put in the crosshair.
    ///
    /// Deliberately wider than [`is_solid`](Self::is_solid): decoration you can
    /// walk through (a flower) must still be breakable. Fluids stay out — the
    /// crosshair reaches through water — and so does air, which is invisible.
    pub fn is_targetable(&self, pos: BlockPos) -> bool {
        self.blocks.is_targetable(self.block_at(pos))
    }

    /// Whether placing a block at `pos` should swallow what is already there
    /// rather than stack on its face. See [`Block::is_replaceable`].
    ///
    /// [`Block::is_replaceable`]: crate::block::Block::is_replaceable
    pub fn is_replaceable(&self, pos: BlockPos) -> bool {
        self.blocks.is_replaceable(self.block_at(pos))
    }

    /// Like [`is_solid`](Self::is_solid), but treats *unloaded* chunks as solid so
    /// entities can't fall/pass through terrain that hasn't streamed in yet. Positions
    /// outside the vertical range stay open (sky above / void below).
    pub fn is_solid_for_collision(&self, pos: BlockPos) -> bool {
        let Some(local) = pos.to_local() else {
            return false;
        };
        match self.chunks.get(&pos.chunk()) {
            Some(chunk) => self.blocks.is_solid(chunk.get(local)),
            None => true,
        }
    }

    /// Place/replace a block. Returns the previous block, or `None` if the chunk
    /// isn't loaded. Marks affected chunk meshes dirty and records the edit in the
    /// persistent overlay so it survives unload/reload and can be replayed to peers.
    pub fn set_block(&mut self, pos: BlockPos, block: BlockId) -> Option<BlockId> {
        let local = pos.to_local()?;
        let chunk_pos = pos.chunk();
        let prev = self.chunks.get_mut(&chunk_pos)?.set(local, block);
        if prev != block {
            self.edits
                .entry(chunk_pos)
                .or_default()
                .insert(local, block);
            self.mark_dirty_with_neighbors(chunk_pos);
        }
        Some(prev)
    }

    /// Apply an authoritative edit received from the network. Unlike
    /// [`set_block`](Self::set_block), this records the edit in the overlay *even if
    /// the chunk isn't loaded yet* — when that chunk later streams in,
    /// [`insert_chunk`](Self::insert_chunk) re-applies it. Applies to the loaded
    /// chunk and marks it dirty when present.
    pub fn apply_edit(&mut self, pos: BlockPos, block: BlockId) {
        let Some(local) = pos.to_local() else {
            return;
        };
        let chunk_pos = pos.chunk();
        self.edits
            .entry(chunk_pos)
            .or_default()
            .insert(local, block);
        if let Some(chunk) = self.chunks.get_mut(&chunk_pos) {
            let prev = chunk.set(local, block);
            if prev != block {
                self.mark_dirty_with_neighbors(chunk_pos);
            }
        }
    }

    /// Flatten the persistent edit overlay to absolute `(BlockPos, BlockId)` pairs.
    /// Used by the host to replay the world's modifications to a joining client.
    pub fn collect_edits(&self) -> Vec<(BlockPos, BlockId)> {
        let mut out = Vec::new();
        for (chunk_pos, overlay) in &self.edits {
            let origin = chunk_pos.origin();
            for (local, block) in overlay {
                let pos = BlockPos::new(
                    origin.x + local.x as i32,
                    local.y as i32,
                    origin.z + local.z as i32,
                );
                out.push((pos, *block));
            }
        }
        out
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::{FlatGenerator, TestCatalog};

    /// A world over a flat generator and a two-block catalog — no game, no
    /// block table, no content files.
    fn test_world() -> World {
        let mut catalog = TestCatalog::new();
        let stone = catalog.cube(1);
        let generator: Arc<dyn WorldGenerator> = Arc::new(FlatGenerator::new(42, 64, stone));
        World::new(generator, catalog.shared())
    }

    /// A non-air block sufficiently high up to land on baseline air, so the edit is
    /// a genuine divergence from generated terrain.
    const EDIT_POS: BlockPos = BlockPos::new(3, 200, 5);
    const EDIT_BLOCK: BlockId = BlockId(1);

    #[test]
    fn edit_survives_unload_and_regeneration() {
        let mut world = test_world();
        world.ensure_chunk(EDIT_POS.chunk());

        assert!(world.set_block(EDIT_POS, EDIT_BLOCK).is_some());
        assert_eq!(world.block_at(EDIT_POS), EDIT_BLOCK);

        // Unload then regenerate the chunk: the overlay must be re-applied.
        world.unload_chunk(EDIT_POS.chunk());
        assert_eq!(world.block_at(EDIT_POS), BlockId::AIR, "chunk is unloaded");

        world.ensure_chunk(EDIT_POS.chunk());
        assert_eq!(
            world.block_at(EDIT_POS),
            EDIT_BLOCK,
            "edit should be restored after regeneration"
        );
    }

    #[test]
    fn apply_edit_buffers_until_chunk_loads() {
        let mut world = test_world();

        // The chunk isn't loaded yet — apply_edit only records into the overlay.
        world.apply_edit(EDIT_POS, EDIT_BLOCK);
        assert_eq!(
            world.block_at(EDIT_POS),
            BlockId::AIR,
            "chunk not loaded yet"
        );

        // Once the chunk streams in (here via synchronous generation), it applies.
        world.ensure_chunk(EDIT_POS.chunk());
        assert_eq!(world.block_at(EDIT_POS), EDIT_BLOCK);
    }

    #[test]
    fn collect_edits_returns_recorded_edits() {
        let mut world = test_world();
        world.ensure_chunk(EDIT_POS.chunk());
        world.set_block(EDIT_POS, EDIT_BLOCK);

        let edits = world.collect_edits();
        assert_eq!(edits, vec![(EDIT_POS, EDIT_BLOCK)]);
    }
}

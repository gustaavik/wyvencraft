//! A single column-chunk of voxels (`16 x 16 x 256`) and its block storage.
//!
//! Storage is currently a flat `Vec<BlockId>`; the public API hides this so it
//! can later be swapped for palette/section compression without touching callers.

use crate::core::{BlockId, CHUNK_VOLUME, ChunkPos, LocalPos};

/// Block data for one chunk plus bookkeeping flags.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Chunk {
    pub pos: ChunkPos,
    blocks: Vec<BlockId>,
    /// Count of non-air blocks; lets us skip meshing/saving empty chunks cheaply.
    solid_count: u32,
    /// Set when block data changed and the GPU mesh is stale.
    #[serde(skip)]
    dirty: bool,
    /// Set when block data changed relative to procedural generation and the
    /// chunk should be persisted / sent to joining peers.
    #[serde(skip)]
    modified: bool,
}

impl Chunk {
    /// An all-air chunk at `pos`.
    pub fn new(pos: ChunkPos) -> Self {
        Self {
            pos,
            blocks: vec![BlockId::AIR; CHUNK_VOLUME],
            solid_count: 0,
            dirty: true,
            modified: false,
        }
    }

    #[inline]
    pub fn get(&self, local: LocalPos) -> BlockId {
        self.blocks[local.index()]
    }

    /// Set a block, updating `solid_count` and dirty flags. Returns the previous
    /// block so callers can compute drops.
    pub fn set(&mut self, local: LocalPos, block: BlockId) -> BlockId {
        let idx = local.index();
        let prev = self.blocks[idx];
        if prev == block {
            return prev;
        }
        if prev.is_air() && !block.is_air() {
            self.solid_count += 1;
        } else if !prev.is_air() && block.is_air() {
            self.solid_count = self.solid_count.saturating_sub(1);
        }
        self.blocks[idx] = block;
        self.dirty = true;
        self.modified = true;
        prev
    }

    /// Bulk fill used by the world generator (does not flag `modified`, since the
    /// result is reproducible from the seed).
    pub fn set_generated(&mut self, local: LocalPos, block: BlockId) {
        let idx = local.index();
        let prev = self.blocks[idx];
        if prev.is_air() && !block.is_air() {
            self.solid_count += 1;
        } else if !prev.is_air() && block.is_air() {
            self.solid_count = self.solid_count.saturating_sub(1);
        }
        self.blocks[idx] = block;
    }

    /// True if the chunk contains nothing but air.
    pub fn is_empty(&self) -> bool {
        self.solid_count == 0
    }

    pub fn is_dirty(&self) -> bool {
        self.dirty
    }

    pub fn mark_clean(&mut self) {
        self.dirty = false;
    }

    pub fn mark_dirty(&mut self) {
        self.dirty = true;
    }

    /// Whether this chunk diverges from generated terrain (needs saving/syncing).
    pub fn is_modified(&self) -> bool {
        self.modified
    }

    /// Raw read access to block data (for meshing).
    pub fn blocks(&self) -> &[BlockId] {
        &self.blocks
    }
}

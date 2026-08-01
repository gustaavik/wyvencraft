//! Chunk streaming: request/insert/unload around the player, and feeding the
//! renderer's mesh queue.
//!
//! This is pure simulation — it decides *which* chunks exist. Turning them into
//! GPU meshes is [`SceneCache`](super::view::SceneCache)'s job.

use super::{InGameState, REQUEST_BUDGET, UNLOAD_MARGIN};
use crate::core::{BlockPos, ChunkPos};

impl InGameState {
    /// Request/insert/unload chunks around the player using the worker pool.
    pub(super) fn update_streaming(&mut self, radius: i32) {
        let center = BlockPos::from_world(self.player.position).chunk();

        // 1. Insert finished chunks (discard any that drifted out of range).
        let mut inserted = 0;
        for chunk in self.loader.drain_ready() {
            if center.chebyshev_distance(chunk.pos) <= radius + UNLOAD_MARGIN {
                self.world.insert_chunk(chunk);
                inserted += 1;
            }
        }
        if inserted > 0 {
            log::debug!(
                "streamed +{inserted} chunks (loaded={}, pending={})",
                self.world.loaded_count(),
                self.loader.pending_count()
            );
        }

        // 2. Request missing chunks within the radius, nearest first.
        let mut wanted: Vec<ChunkPos> = Vec::new();
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                let pos = ChunkPos::new(center.x + dx, center.z + dz);
                if !self.world.is_loaded(pos) && !self.loader.is_pending(pos) {
                    wanted.push(pos);
                }
            }
        }
        wanted.sort_by_key(|p| center.chebyshev_distance(*p));
        for pos in wanted.into_iter().take(REQUEST_BUDGET) {
            self.loader.request(pos);
        }

        // 3. Unload distant chunks and their meshes.
        let to_unload: Vec<ChunkPos> = self
            .world
            .loaded_positions()
            .filter(|p| center.chebyshev_distance(*p) > radius + UNLOAD_MARGIN)
            .collect();
        for pos in to_unload {
            self.world.unload_chunk(pos);
            self.view.forget_chunk(pos);
        }
    }
}

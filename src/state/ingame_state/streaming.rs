//! Chunk streaming: request/insert/unload around the player, and feeding the
//! renderer's mesh queue.
//!
//! This is pure simulation — it decides *which* chunks exist. Turning them into
//! GPU meshes is [`SceneCache`](super::view::SceneCache)'s job.
//!
//! A host keeps chunks loaded around **every** player, not just its own: a
//! client standing at a far-off boss altar needs its block edits applied and
//! the boss it summoned simulated, and both only happen in loaded chunks. The
//! remote players' regions are for simulation only — the host still meshes
//! whatever its own camera can see, and `view.forget_chunk` on unload is a
//! no-op for a chunk that was never meshed.

use super::{InGameState, REQUEST_BUDGET, UNLOAD_MARGIN};
use crate::core::{BlockPos, ChunkPos};

/// Chunks a host keeps loaded around each *remote* player: enough for the
/// block they are editing, the mobs around them and a boss arena, without
/// generating a second render distance's worth of terrain per client.
pub(super) const SIM_RADIUS: i32 = 4;

/// A point chunks are kept loaded around, and how far.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) struct Anchor {
    pub center: ChunkPos,
    pub radius: i32,
}

impl Anchor {
    /// How far outside this anchor's radius `pos` lies (≤ 0 inside it).
    fn overshoot(self, pos: ChunkPos) -> i32 {
        self.center.chebyshev_distance(pos) - self.radius
    }
}

/// The smallest overshoot of `pos` across every anchor — how far it is from
/// being wanted by *anyone*. `i32::MAX` with no anchors at all.
fn overshoot(anchors: &[Anchor], pos: ChunkPos) -> i32 {
    anchors
        .iter()
        .map(|a| a.overshoot(pos))
        .min()
        .unwrap_or(i32::MAX)
}

/// Whether some anchor still wants `pos` kept, allowing `margin` chunks of
/// hysteresis so a player pacing along a border does not thrash the loader.
pub(super) fn is_kept(anchors: &[Anchor], pos: ChunkPos, margin: i32) -> bool {
    overshoot(anchors, pos) <= margin
}

/// Every chunk inside some anchor's radius, deduplicated, nearest-to-an-anchor
/// first, so the local player's own ground always wins a tight budget.
pub(super) fn wanted_chunks(anchors: &[Anchor]) -> Vec<ChunkPos> {
    let mut wanted: Vec<ChunkPos> = Vec::new();
    for a in anchors {
        for dx in -a.radius..=a.radius {
            for dz in -a.radius..=a.radius {
                wanted.push(ChunkPos::new(a.center.x + dx, a.center.z + dz));
            }
        }
    }
    wanted.sort_by_key(|p| {
        (
            anchors
                .iter()
                .map(|a| a.center.chebyshev_distance(*p))
                .min(),
            p.x,
            p.z,
        )
    });
    wanted.dedup();
    wanted
}

impl InGameState {
    /// Where chunks must exist this frame: the local player at the render
    /// distance, plus — on a host — every remote player at [`SIM_RADIUS`].
    pub(super) fn stream_anchors(&self, radius: i32) -> Vec<Anchor> {
        let mut anchors = vec![Anchor {
            center: BlockPos::from_world(self.player.position).chunk(),
            radius,
        }];
        if self.session.serves_peers() {
            anchors.extend(self.peers.players.values().map(|rp| Anchor {
                center: BlockPos::from_world(rp.position()).chunk(),
                radius: SIM_RADIUS.min(radius),
            }));
        }
        anchors
    }

    /// Request/insert/unload chunks around the players using the worker pool.
    pub(super) fn update_streaming(&mut self, radius: i32) {
        let anchors = self.stream_anchors(radius);

        // 1. Insert finished chunks (discard any that drifted out of range).
        let mut inserted = 0;
        for chunk in self.loader.drain_ready() {
            if is_kept(&anchors, chunk.pos, UNLOAD_MARGIN) {
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

        // 2. Request missing chunks, nearest first.
        let missing: Vec<ChunkPos> = wanted_chunks(&anchors)
            .into_iter()
            .filter(|p| !self.world.is_loaded(*p) && !self.loader.is_pending(*p))
            .take(REQUEST_BUDGET)
            .collect();
        for pos in missing {
            self.loader.request(pos);
        }

        // 3. Unload chunks nobody is near, and their meshes.
        let to_unload: Vec<ChunkPos> = self
            .world
            .loaded_positions()
            .filter(|p| !is_kept(&anchors, *p, UNLOAD_MARGIN))
            .collect();
        for pos in to_unload {
            self.world.unload_chunk(pos);
            self.view.forget_chunk(pos);
        }
    }
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::net::PlayerId;
    use crate::state::session::FakeSession;

    fn anchor(x: i32, z: i32, radius: i32) -> Anchor {
        Anchor {
            center: ChunkPos::new(x, z),
            radius,
        }
    }

    #[test]
    fn a_chunk_is_kept_while_any_anchor_is_near_it() {
        let anchors = [anchor(0, 0, 4), anchor(100, 0, 2)];
        assert!(is_kept(&anchors, ChunkPos::new(4, 0), 0));
        assert!(is_kept(&anchors, ChunkPos::new(102, 1), 0));
        assert!(is_kept(&anchors, ChunkPos::new(103, 0), 1), "margin");
        assert!(!is_kept(&anchors, ChunkPos::new(50, 0), 2));
        assert!(!is_kept(&[], ChunkPos::new(0, 0), 2));
    }

    #[test]
    fn wanted_chunks_cover_every_anchor_once_nearest_first() {
        let anchors = [anchor(0, 0, 1), anchor(1, 0, 1)];
        let wanted = wanted_chunks(&anchors);
        // 3×3 around each, overlapping in a 2×3 band → 12 distinct chunks.
        assert_eq!(wanted.len(), 12);
        assert_eq!(
            anchors
                .iter()
                .map(|a| a.center.chebyshev_distance(wanted[0]))
                .min(),
            Some(0)
        );
        let mut sorted = wanted.clone();
        sorted.sort_by_key(|p| (p.x, p.z));
        sorted.dedup();
        assert_eq!(sorted.len(), wanted.len(), "no duplicates");
    }

    #[test]
    fn a_host_anchors_chunks_around_a_far_off_remote_player() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        state.set_session(Box::new(FakeSession::host()));
        let far = Vec3::new(2000.0, 90.0, -1500.0);
        state.peers.entry(PlayerId(1), far);

        let anchors = state.stream_anchors(8);
        let target = BlockPos::from_world(far).chunk();
        assert!(anchors.contains(&anchor(target.x, target.z, SIM_RADIUS)));
        assert!(is_kept(&anchors, target, 0));
    }

    #[test]
    fn a_client_streams_only_around_itself() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        state.set_session(Box::new(FakeSession::client(PlayerId(1))));
        state.peers.entry(PlayerId(0), Vec3::new(2000.0, 90.0, 0.0));
        assert_eq!(state.stream_anchors(8).len(), 1);
    }
}

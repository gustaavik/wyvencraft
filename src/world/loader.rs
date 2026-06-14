//! Background chunk streaming: generates chunks around the player on worker
//! threads so the render loop never stalls on terrain generation.
//!
//! Design: *producer/consumer*. A pool of worker threads pulls generate jobs off
//! a shared [`crossbeam_channel`] and pushes finished [`Chunk`]s back; the main
//! thread requests positions and drains the results for insertion + meshing.
//!
//! Meshing stays on the main thread (it needs neighbour block data that lives in
//! the `World`) but is budgeted per frame by the caller, which keeps frames
//! smooth without shipping neighbour snapshots to workers.

use std::collections::HashSet;
use std::sync::Arc;
use std::thread::JoinHandle;

use crossbeam_channel::{Receiver, Sender, unbounded};

use crate::core::ChunkPos;
use crate::world::chunk::Chunk;
use crate::world::generation::WorldGenerator;

/// Owns the generation worker pool and the request/result channels.
pub struct ChunkLoader {
    request_tx: Sender<ChunkPos>,
    result_rx: Receiver<Chunk>,
    /// Positions currently requested or in-flight (avoids duplicate work).
    pending: HashSet<ChunkPos>,
    workers: Vec<JoinHandle<()>>,
}

impl ChunkLoader {
    pub fn new(generator: Arc<dyn WorldGenerator>, worker_count: usize) -> Self {
        let worker_count = worker_count.max(1);
        let (request_tx, request_rx) = unbounded::<ChunkPos>();
        let (result_tx, result_rx) = unbounded::<Chunk>();

        let mut workers = Vec::with_capacity(worker_count);
        for _ in 0..worker_count {
            // crossbeam receivers are MPMC, so all workers share one queue.
            let jobs: Receiver<ChunkPos> = request_rx.clone();
            let results = result_tx.clone();
            let generator = generator.clone();
            workers.push(std::thread::spawn(move || {
                while let Ok(pos) = jobs.recv() {
                    let chunk = generator.generate(pos);
                    if results.send(chunk).is_err() {
                        break; // main side dropped; shut the worker down
                    }
                }
            }));
        }

        Self {
            request_tx,
            result_rx,
            pending: HashSet::new(),
            workers,
        }
    }

    /// Queue `pos` for generation if it isn't already in flight.
    pub fn request(&mut self, pos: ChunkPos) {
        if self.pending.insert(pos) {
            // Send failure only happens if all workers died; ignore.
            let _ = self.request_tx.send(pos);
        }
    }

    pub fn is_pending(&self, pos: ChunkPos) -> bool {
        self.pending.contains(&pos)
    }

    pub fn pending_count(&self) -> usize {
        self.pending.len()
    }

    /// Collect all chunks finished since the last call.
    pub fn drain_ready(&mut self) -> Vec<Chunk> {
        let mut ready = Vec::new();
        while let Ok(chunk) = self.result_rx.try_recv() {
            self.pending.remove(&chunk.pos);
            ready.push(chunk);
        }
        ready
    }

    /// Forget a pending request (e.g. it left the load radius before finishing).
    /// The worker may still produce it; `drain_ready` simply won't find it
    /// pending and the caller can discard out-of-range results.
    pub fn cancel(&mut self, pos: ChunkPos) {
        self.pending.remove(&pos);
    }
}

impl Drop for ChunkLoader {
    fn drop(&mut self) {
        // Close the request channel so workers' `recv()` returns Err and they exit.
        let (dead_tx, _) = unbounded();
        self.request_tx = dead_tx;
        for worker in self.workers.drain(..) {
            let _ = worker.join();
        }
    }
}

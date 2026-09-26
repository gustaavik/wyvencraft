//! Chunk meshes: what is loaded, what is queued, and the per-frame budget
//! that turns queued chunks into GPU meshes.

use super::*;
use wyven_voxel::meshing::ChunkMeshOutput;

impl SceneCache {
    // --- Chunk meshes ---------------------------------------------------------------

    /// Chunk meshes currently uploaded (debug HUD).
    pub fn loaded_mesh_count(&self) -> usize {
        self.meshes.len()
    }

    /// Chunk meshes waiting to be rebuilt (debug HUD).
    pub fn queued_mesh_count(&self) -> usize {
        self.mesh_queue.len()
    }

    /// Drop a chunk's meshes when it unloads.
    pub fn forget_chunk(&mut self, pos: ChunkPos) {
        self.meshes.remove(&pos);
        self.transparent_meshes.remove(&pos);
        self.array_meshes.remove(&pos);
        self.array_transparent_meshes.remove(&pos);
        self.model_meshes.remove(&pos);
    }

    /// Move freshly-dirtied chunks into the mesh queue (deduped).
    pub fn enqueue_dirty(&mut self, dirty: impl IntoIterator<Item = ChunkPos>) {
        for pos in dirty {
            if self.queued.insert(pos) {
                self.mesh_queue.push_back(pos);
            }
        }
    }

    /// Rebuild up to `budget` chunk meshes this frame.
    pub fn process_mesh_budget(
        &mut self,
        ctx: &Arc<RenderContext>,
        world: &World,
        blocks: BlockAppearance<'_>,
        budget: usize,
    ) {
        for _ in 0..budget {
            let Some(pos) = self.mesh_queue.pop_front() else {
                break;
            };
            self.queued.remove(&pos);

            let generator = world.generator();
            let output = world.chunk(pos).map(|chunk| {
                mesh_chunk(
                    chunk,
                    &blocks,
                    |p| world.block_at(p),
                    |x, z, index| generator.biome_tint(x, z, index),
                )
            });
            match output {
                Some(output) => self.upload_chunk(ctx, pos, &output, blocks.models),
                // Chunk was unloaded before we got to it.
                None => self.forget_chunk(pos),
            }
        }
    }

    /// Replace everything drawn for chunk `pos` with a fresh meshing of it.
    fn upload_chunk(
        &mut self,
        ctx: &Arc<RenderContext>,
        pos: ChunkPos,
        output: &ChunkMeshOutput,
        models: &ModelRegistry,
    ) {
        replace_mesh(ctx, &mut self.meshes, pos, &output.opaque, "opaque");
        replace_mesh(
            ctx,
            &mut self.transparent_meshes,
            pos,
            &output.transparent,
            "transparent",
        );
        // Blockbench-authored blocks: one mesh per chunk however many block
        // types and textures it holds, because the layer index rides on the
        // vertex.
        replace_mesh(
            ctx,
            &mut self.array_meshes,
            pos,
            &output.array_opaque,
            "block",
        );
        replace_mesh(
            ctx,
            &mut self.array_transparent_meshes,
            pos,
            &output.array_transparent,
            "blended block",
        );
        // Model-backed blocks: one mesh per model in this chunk, each needing
        // its texture resident before it can be drawn.
        let mut baked = Vec::new();
        for (id, mesh) in &output.models {
            self.ensure_model_texture(ctx, models, *id);
            match GpuMesh::upload(&ctx.memory_allocator, mesh) {
                Ok(Some(gpu)) => baked.push((gpu, *id)),
                Ok(None) => {}
                Err(err) => log::error!("model mesh upload failed at {pos:?}: {err:?}"),
            }
        }
        if baked.is_empty() {
            self.model_meshes.remove(&pos);
        } else {
            self.model_meshes.insert(pos, baked);
        }
    }
}

/// Upload `mesh` as chunk `pos`'s entry in `meshes`, or drop the entry when
/// the chunk has nothing of that kind any more. A failed upload keeps
/// whatever was there and says which kind (`what`) failed.
fn replace_mesh(
    ctx: &Arc<RenderContext>,
    meshes: &mut HashMap<ChunkPos, GpuMesh>,
    pos: ChunkPos,
    mesh: &CpuMesh,
    what: &str,
) {
    match GpuMesh::upload(&ctx.memory_allocator, mesh) {
        Ok(Some(gpu)) => {
            meshes.insert(pos, gpu);
        }
        Ok(None) => {
            meshes.remove(&pos);
        }
        Err(err) => log::error!("{what} mesh upload failed at {pos:?}: {err:?}"),
    }
}

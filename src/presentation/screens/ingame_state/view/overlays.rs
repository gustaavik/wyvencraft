//! The crack overlay on the block being mined, and the outline on the
//! block under the crosshair.

use super::*;

impl SceneCache {
    /// (Re)build the crack overlay for the block being mined; drop it when idle.
    /// Cheap enough to rebuild every frame (six quads).
    pub fn update_break_overlay(
        &mut self,
        ctx: &Arc<RenderContext>,
        breaking: Option<(BlockPos, Aabb, f32)>,
    ) {
        self.break_mesh = breaking.and_then(|(block, box_, progress)| {
            // No crack art on disk means no overlay at all: it is drawn *over*
            // the block being mined, so a missing-texture marker would hide the
            // thing you are looking at rather than read as art that is absent.
            let overlay = mesh_block_overlay(box_, cracks::tile(progress)?);
            match GpuMesh::upload(&ctx.memory_allocator, &overlay) {
                Ok(mesh) => mesh,
                Err(err) => {
                    log::error!("break overlay upload failed at {block:?}: {err:?}");
                    None
                }
            }
        });
    }

    /// (Re)build the selection outline on the targeted block. The geometry only
    /// depends on the block position, so it's cached until the target changes.
    pub fn update_target_outline(
        &mut self,
        ctx: &Arc<RenderContext>,
        target: Option<(BlockPos, Aabb)>,
    ) {
        let block = target.map(|(block, _)| block);
        if block == self.outline_block {
            return;
        }
        self.outline_block = block;
        self.outline_mesh = target.and_then(|(block, box_)| {
            let mut vertices = Vec::new();
            debug::push_block_outline(&mut vertices, box_, OUTLINE_COLOR);
            match GpuLines::upload(&ctx.memory_allocator, &vertices) {
                Ok(lines) => lines,
                Err(err) => {
                    log::error!("selection outline upload failed at {block:?}: {err:?}");
                    None
                }
            }
        });
    }
}

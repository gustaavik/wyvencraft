//! Face-culling chunk mesher.

use super::ChunkMeshOutput;
use crate::core::{BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, Direction};
use crate::render::texture::atlas_uv;
use crate::render::vertex::ChunkVertex;
use crate::world::block::BlockRegistry;
use crate::world::chunk::Chunk;

/// Per-face shading so faces read as 3D even before real lighting/AO.
fn face_shade(dir: Direction) -> f32 {
    match dir {
        Direction::PosY => 1.0,
        Direction::NegY => 0.5,
        Direction::PosX | Direction::NegX => 0.75,
        Direction::PosZ | Direction::NegZ => 0.62,
    }
}

/// The four corner offsets (within a unit cube) of each face, plus matching UV
/// corners. Winding is consistent; the voxel pipeline can cull back faces.
fn face_geometry(dir: Direction) -> ([[f32; 3]; 4], [[f32; 2]; 4]) {
    let uv = [[0.0, 1.0], [1.0, 1.0], [1.0, 0.0], [0.0, 0.0]];
    let corners = match dir {
        Direction::PosX => [
            [1.0, 0.0, 1.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [1.0, 1.0, 1.0],
        ],
        Direction::NegX => [
            [0.0, 0.0, 0.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [0.0, 1.0, 0.0],
        ],
        Direction::PosY => [
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
        Direction::NegY => [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
        ],
        Direction::PosZ => [
            [1.0, 0.0, 1.0],
            [0.0, 0.0, 1.0],
            [0.0, 1.0, 1.0],
            [1.0, 1.0, 1.0],
        ],
        Direction::NegZ => [
            [0.0, 0.0, 0.0],
            [1.0, 0.0, 0.0],
            [1.0, 1.0, 0.0],
            [0.0, 1.0, 0.0],
        ],
    };
    (corners, uv)
}

/// Build the renderable mesh for `chunk`.
///
/// `neighbor` resolves block ids for world positions outside this chunk (for
/// correct culling at chunk borders); it should return [`BlockId::AIR`] for
/// not-yet-loaded chunks.
pub fn mesh_chunk(
    chunk: &Chunk,
    registry: &BlockRegistry,
    neighbor: impl Fn(BlockPos) -> BlockId,
) -> ChunkMeshOutput {
    let mut out = ChunkMeshOutput::default();
    let origin = chunk.pos.origin();

    for ly in 0..CHUNK_HEIGHT {
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let local = crate::core::LocalPos {
                    x: lx as u8,
                    y: ly as u16,
                    z: lz as u8,
                };
                let id = chunk.get(local);
                let block = registry.get(id);
                if !block.is_visible() {
                    continue;
                }

                let world = BlockPos::new(origin.x + lx, ly, origin.z + lz);

                for dir in Direction::ALL {
                    let np = world.offset(dir);
                    // Read in-chunk neighbours directly; defer to the sampler at
                    // borders / outside the vertical range.
                    let neighbor_id = if np.y < 0 || np.y >= CHUNK_HEIGHT {
                        BlockId::AIR
                    } else if np.x >= origin.x
                        && np.x < origin.x + CHUNK_SIZE
                        && np.z >= origin.z
                        && np.z < origin.z + CHUNK_SIZE
                    {
                        chunk.get(np.to_local().expect("y in range"))
                    } else {
                        neighbor(np)
                    };
                    let neighbor_block = registry.get(neighbor_id);

                    let visible = if block.is_transparent() {
                        neighbor_id != id && !neighbor_block.is_opaque()
                    } else {
                        !neighbor_block.is_opaque()
                    };
                    if !visible {
                        continue;
                    }

                    let (corners, uvs) = face_geometry(dir);
                    let normal = dir.normal().to_array();
                    let ao = face_shade(dir);
                    let tile = block.textures.tile(dir);

                    let base = [world.x as f32, world.y as f32, world.z as f32];
                    let quad = std::array::from_fn(|i| ChunkVertex {
                        position: [
                            base[0] + corners[i][0],
                            base[1] + corners[i][1],
                            base[2] + corners[i][2],
                        ],
                        normal,
                        uv: atlas_uv(tile, uvs[i]),
                        ao,
                    });

                    if block.is_transparent() {
                        out.transparent.push_quad(quad);
                    } else {
                        out.opaque.push_quad(quad);
                    }
                }
            }
        }
    }

    out
}

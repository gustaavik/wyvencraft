//! Face-culling chunk mesher.

use glam::Vec3;

use super::ChunkMeshOutput;
use crate::core::{BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, Direction};
use crate::render::mesh::CpuMesh;
use crate::render::texture::atlas_uv;
use crate::render::tiles;
use crate::render::vertex::{ChunkVertex, FLAG_WATER};
use crate::world::block::{BlockRegistry, FaceTextures};
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

                    // Transparent and cutout blocks also cull faces shared with
                    // a same-id neighbour (no interior faces inside a canopy or
                    // a body of water).
                    let visible = if block.is_transparent() || block.is_cutout() {
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
                    let flags = if tiles::is_water(tile) { FLAG_WATER } else { 0 };

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
                        flags,
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

/// Build an overlay mesh over the block at `pos`, textured with `tile` on all
/// six faces — used for the crack animation while a block is being mined. The
/// cube is inflated slightly around its centre so it never z-fights the block.
pub fn mesh_block_overlay(pos: BlockPos, tile: u32) -> CpuMesh {
    const INFLATE: f32 = 0.004;
    let scale = 1.0 + 2.0 * INFLATE;
    let center = [pos.x as f32 + 0.5, pos.y as f32 + 0.5, pos.z as f32 + 0.5];

    let mut mesh = CpuMesh::new();
    for dir in Direction::ALL {
        let (corners, uvs) = face_geometry(dir);
        let normal = dir.normal().to_array();
        let quad = std::array::from_fn(|i| ChunkVertex {
            position: std::array::from_fn(|a| center[a] + (corners[i][a] - 0.5) * scale),
            normal,
            uv: atlas_uv(tile, uvs[i]),
            ao: 1.0,
            flags: 0,
        });
        mesh.push_quad(quad);
    }
    mesh
}

/// Append a small textured cube (a dropped item) to `mesh`: edge length `size`,
/// centred on `center`, spun `yaw` radians around Y. Faces are shaded and
/// textured like regular blocks so drops read as miniatures of their block.
pub fn push_item_cube(
    mesh: &mut CpuMesh,
    center: Vec3,
    size: f32,
    yaw: f32,
    textures: &FaceTextures,
) {
    let (sin, cos) = yaw.sin_cos();
    let rotate = |v: [f32; 3]| [v[0] * cos + v[2] * sin, v[1], -v[0] * sin + v[2] * cos];

    for dir in Direction::ALL {
        let (corners, uvs) = face_geometry(dir);
        let normal = rotate(dir.normal().to_array());
        let ao = face_shade(dir);
        let tile = textures.tile(dir);
        let quad = std::array::from_fn(|i| {
            let local = rotate([
                (corners[i][0] - 0.5) * size,
                (corners[i][1] - 0.5) * size,
                (corners[i][2] - 0.5) * size,
            ]);
            ChunkVertex {
                position: [
                    center.x + local[0],
                    center.y + local[1],
                    center.z + local[2],
                ],
                normal,
                uv: atlas_uv(tile, uvs[i]),
                ao,
                flags: 0,
            }
        });
        mesh.push_quad(quad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn item_cube_is_centred_and_compact() {
        let mut mesh = CpuMesh::new();
        let center = Vec3::new(1.0, 70.0, -2.0);
        let size = 0.25;
        push_item_cube(
            &mut mesh,
            center,
            size,
            0.7,
            &FaceTextures::uniform(tiles::STONE),
        );
        assert_eq!(mesh.vertices.len(), 24); // 6 faces × 4 vertices
        assert_eq!(mesh.indices.len(), 36);
        // Every vertex stays within the cube's rotated bounding radius.
        let r = size * std::f32::consts::SQRT_2 * 0.5 + 1e-4;
        for v in &mesh.vertices {
            assert!((v.position[0] - center.x).abs() <= r);
            assert!((v.position[1] - center.y).abs() <= size * 0.5 + 1e-4);
            assert!((v.position[2] - center.z).abs() <= r);
        }
    }

    #[test]
    fn block_overlay_covers_all_faces_and_wraps_the_block() {
        let mesh = mesh_block_overlay(BlockPos::new(2, 5, -3), tiles::CRACK_0);
        assert_eq!(mesh.vertices.len(), 24); // 6 faces × 4 vertices
        assert_eq!(mesh.indices.len(), 36);
        for v in &mesh.vertices {
            // Every vertex sits just outside the unit cube of the block.
            assert!(v.position[0] > 1.9 && v.position[0] < 3.1);
            assert!(v.position[1] > 4.9 && v.position[1] < 6.1);
            assert!(v.position[2] > -3.1 && v.position[2] < -1.9);
            // And its UVs stay inside the crack tile.
            let uv0 = atlas_uv(tiles::CRACK_0, [0.0, 0.0]);
            let uv1 = atlas_uv(tiles::CRACK_0, [1.0, 1.0]);
            assert!(v.uv[0] >= uv0[0] && v.uv[0] <= uv1[0]);
            assert!(v.uv[1] >= uv0[1] && v.uv[1] <= uv1[1]);
        }
    }
}

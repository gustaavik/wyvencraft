//! Face-culling chunk mesher.

use glam::Vec3;

use super::{BlockModels, ChunkMeshOutput};
use crate::core::math::rotate_y;
use crate::core::{Aabb, BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, Direction};
use crate::render::mesh::CpuMesh;
use crate::render::texture::atlas_uv;
use crate::render::vertex::{ChunkVertex, FLAG_WATER, NO_TINT};
use crate::world::block::{Block, BlockRegistry, FaceTextures, FluidInfo};
use crate::world::blockmodel::BakedBlockModel;
use crate::world::chunk::Chunk;

/// How many biome colours a block model's `tintindex` can choose between.
/// Minecraft's numbering: `0` grass, `1` foliage.
pub const TINT_SOURCES: usize = 2;

/// Top of a source water block: two texels (2/16) below the block top.
/// Flowing water scales down from here with its level.
const WATER_SURFACE: f32 = 14.0 / 16.0;

/// Rendered surface height of a fluid cell (fraction of a block).
fn water_surface(fluid: FluidInfo) -> f32 {
    f32::from(fluid.level) / f32::from(fluid.max_level) * WATER_SURFACE
}

/// Top-corner heights (indexed `[x][z]` by corner offset 0/1) for the water
/// block at `pos`: each corner takes the highest surface of the four cells
/// meeting there, or a full block if any of them continues upward (a falling
/// column or water under a ceiling). The result depends only on the corner's
/// position, so neighbouring water of different levels computes identical
/// heights at shared corners and connects into one continuous sheet.
fn water_corner_heights(
    pos: BlockPos,
    sample: &impl Fn(BlockPos) -> BlockId,
    registry: &BlockRegistry,
) -> [[f32; 2]; 2] {
    let mut heights = [[0.0f32; 2]; 2];
    for cx in 0..2i32 {
        for cz in 0..2i32 {
            let mut h = 0.0f32;
            for (ox, oz) in [(cx - 1, cz - 1), (cx, cz - 1), (cx - 1, cz), (cx, cz)] {
                let cell = BlockPos::new(pos.x + ox, pos.y, pos.z + oz);
                let Some(fluid) = registry.fluid(sample(cell)) else {
                    continue;
                };
                let above = sample(cell.offset(Direction::PosY));
                if registry.is_fluid(above) || registry.get(above).is_opaque() {
                    h = 1.0;
                    break;
                }
                h = h.max(water_surface(fluid));
            }
            heights[cx as usize][cz as usize] = h;
        }
    }
    heights
}

/// Per-face shading so faces read as 3D even before real lighting/AO.
fn face_shade(dir: Direction) -> f32 {
    match dir {
        Direction::PosY => 1.0,
        Direction::NegY => 0.68,
        Direction::PosX | Direction::NegX => 0.85,
        Direction::PosZ | Direction::NegZ => 0.78,
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

/// Yaw for a model-backed block that asked for a random one: a SplitMix64-style
/// mix of its world position, so the same block re-meshes to the same angle and
/// a field of plants doesn't read as a grid of clones.
fn block_yaw(pos: BlockPos) -> f32 {
    let mut h = (pos.x as u64).wrapping_mul(0xC2B2_AE3D_27D4_EB4F)
        ^ (pos.y as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15)
        ^ (pos.z as u64).wrapping_mul(0x1656_67B1_9E37_79F9);
    h ^= h >> 30;
    h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
    h ^= h >> 27;
    (h >> 40) as f32 / 16_777_216.0 * std::f32::consts::TAU
}

/// Build the renderable mesh for `chunk`.
///
/// `neighbor` resolves block ids for world positions outside this chunk (for
/// correct culling at chunk borders); it should return [`BlockId::AIR`] for
/// not-yet-loaded chunks. `models` supplies the geometry of model-backed
/// blocks; pass [`BlockModels::none`] when there are none.
///
/// `tint(x, z, index)` is the biome colour at a world column for one of the
/// [`TINT_SOURCES`] a block model's `tintindex` can name. It is a closure rather
/// than a table so the mesher stays pure: biome sampling belongs to the
/// generator, and tests pass `|_, _, _| NO_TINT` and never touch one.
pub fn mesh_chunk(
    chunk: &Chunk,
    registry: &BlockRegistry,
    models: BlockModels<'_>,
    neighbor: impl Fn(BlockPos) -> BlockId,
    tint: impl Fn(i32, i32, u8) -> [u8; 4],
) -> ChunkMeshOutput {
    let mut out = ChunkMeshOutput::default();
    let origin = chunk.pos.origin();

    // Read in-chunk neighbours directly; defer to the sampler at borders /
    // outside the vertical range.
    let sample = |np: BlockPos| -> BlockId {
        if np.y < 0 || np.y >= CHUNK_HEIGHT {
            BlockId::AIR
        } else if np.x >= origin.x
            && np.x < origin.x + CHUNK_SIZE
            && np.z >= origin.z
            && np.z < origin.z + CHUNK_SIZE
        {
            chunk.get(np.to_local().expect("y in range"))
        } else {
            neighbor(np)
        }
    };

    // Whether a block hides the face its neighbour shares with it.
    //
    // A Blockbench-authored block answers from measured geometry, so a modelled
    // slab cannot hide what it does not actually cover; an atlas cube answers
    // from its declared render type, which is all a `textures = [...]` block has
    // to offer. Having one predicate for both is what lets the old and new paths
    // cull against each other while blocks migrate over one at a time.
    let occludes = |block: BlockId, dir: Direction| -> bool {
        match models.baked_of(block) {
            Some(baked) => baked.occludes[dir as usize],
            None => registry.get(block).is_opaque(),
        }
    };

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

                // A Blockbench-authored block replaces the whole cube too, but
                // unlike the `.bbmodel` path below it stays in the chunk's own
                // vertex buffer: its texture is a layer of the shared array, so
                // it needs no separate draw, and its `cullface` data lets it
                // take part in neighbour culling like an ordinary cube.
                if let Some(baked) = models.baked_of(id) {
                    // Same rule the cube path below uses: a transparent or
                    // cutout block also drops the face it shares with a
                    // neighbour of its own kind. Without it a leaf canopy —
                    // whose texture is see-through, so it occludes nothing —
                    // would emit every face of every block inside it.
                    let hidden = |dir: Direction| {
                        let neighbor = sample(world.offset(dir));
                        occludes(neighbor, dir.opposite())
                            || (neighbor == id && (block.is_cutout() || block.is_transparent()))
                    };
                    push_baked_model(&mut out, baked, block, world, hidden, &tint);
                    continue;
                }

                // A model-backed block replaces the whole cube: its geometry
                // brings its own texture, so it goes to a per-model bucket and
                // no atlas face is emitted for it at all.
                if let Some((placement, model)) = models.of(id) {
                    let yaw = if placement.random_yaw {
                        block_yaw(world)
                    } else {
                        0.0
                    };
                    // Baked about the cell's horizontal centre, so an authored
                    // `offset = [-0.5, 0, -0.5]` turns the model about itself.
                    let centre =
                        Vec3::new(world.x as f32 + 0.5, world.y as f32, world.z as f32 + 0.5);
                    let baked = model.mesh.to_cpu_mesh(
                        centre,
                        yaw,
                        placement.scale,
                        placement.rotation,
                        placement.offset,
                    );
                    let bucket = out.models.entry(placement.id).or_default();
                    bucket.push_indexed(baked.vertices, baked.indices);
                    continue;
                }

                // Fluids render with a lowered, per-corner surface so blocks
                // of different levels slope into each other; everything else
                // keeps flat full-height tops.
                let water = registry.fluid(id);
                let heights = match water {
                    Some(_) => water_corner_heights(world, &sample, registry),
                    None => [[1.0; 2]; 2],
                };

                for dir in Direction::ALL {
                    let np = world.offset(dir);
                    let neighbor_id = sample(np);
                    let hidden = occludes(neighbor_id, dir.opposite());

                    // Transparent and cutout blocks also cull faces shared with
                    // a same-id neighbour (no interior faces inside a canopy or
                    // a body of water); all levels of a fluid count as one body.
                    let visible = if let Some(f) = water {
                        registry
                            .fluid(neighbor_id)
                            .is_none_or(|nf| nf.group != f.group)
                            && !hidden
                    } else if block.is_transparent() || block.is_cutout() {
                        neighbor_id != id && !hidden
                    } else {
                        !hidden
                    };
                    if !visible {
                        continue;
                    }

                    let (corners, uvs) = face_geometry(dir);
                    let normal = dir.normal().to_array();
                    let ao = face_shade(dir);
                    let tile = block.textures.tile(dir);
                    let flags = if block.face_animated(dir) {
                        FLAG_WATER
                    } else {
                        0
                    };

                    let base = [world.x as f32, world.y as f32, world.z as f32];
                    let quad = std::array::from_fn(|i| {
                        // Corner y is 0 or 1: top corners take the (possibly
                        // per-corner lowered) surface height, so the top face
                        // and the side faces' upper edges move together.
                        let y = if corners[i][1] > 0.5 {
                            heights[corners[i][0] as usize][corners[i][2] as usize]
                        } else {
                            0.0
                        };
                        ChunkVertex {
                            position: [
                                base[0] + corners[i][0],
                                base[1] + y,
                                base[2] + corners[i][2],
                            ],
                            normal,
                            uv: atlas_uv(tile, uvs[i]),
                            ao,
                            flags,
                            // Atlas geometry: the block texture array's layer
                            // and tint are the modelled path's business.
                            layer: 0,
                            tint: NO_TINT,
                        }
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

/// Emit one Blockbench-authored block into the chunk's array-textured buffers.
///
/// Each quad is already in block-local `0..1` space, so placing it is a
/// translation; the only per-cell decisions are whether a `cullface` quad is
/// hidden by its neighbour, and what colour a `tintindex` quad takes here.
///
/// `hidden(dir)` answers "does the neighbour in `dir` cover the face we share".
fn push_baked_model(
    out: &mut ChunkMeshOutput,
    model: &BakedBlockModel,
    block: &Block,
    world: BlockPos,
    hidden: impl Fn(Direction) -> bool,
    tint: &impl Fn(i32, i32, u8) -> [u8; 4],
) {
    let base = Vec3::new(world.x as f32, world.y as f32, world.z as f32);
    // A model that asked for a random yaw turns about its cell's centre. Its
    // `cullface` data is dropped along with the rotation: a turned face no
    // longer points at the neighbour it named, and anything wanting a random
    // yaw is a plant that covers no cell face to begin with.
    let yaw = model.random_yaw.then(|| block_yaw(world));
    let centre = Vec3::new(0.5, 0.0, 0.5);
    // Sampled at most once per block: biome colour varies by column, and all of
    // a block's faces belong to the same column.
    let mut biome_tints: [Option<[u8; 4]>; TINT_SOURCES] = [None; TINT_SOURCES];

    for quad in &model.quads {
        if let Some(dir) = quad.cull
            && yaw.is_none()
            && hidden(dir)
        {
            continue;
        }
        let tint = match quad.tint {
            Some(index) => {
                let slot = usize::from(index).min(TINT_SOURCES - 1);
                *biome_tints[slot].get_or_insert_with(|| tint(world.x, world.z, index))
            }
            None => NO_TINT,
        };
        let place = |p: Vec3| match yaw {
            Some(yaw) => base + centre + rotate_y(p - centre, yaw),
            None => base + p,
        };
        let normal = match yaw {
            Some(yaw) => rotate_y(Vec3::from(quad.normal), yaw).to_array(),
            None => quad.normal,
        };
        let vertices = std::array::from_fn(|i| ChunkVertex {
            position: place(quad.positions[i]).to_array(),
            normal,
            uv: quad.uvs[i],
            ao: quad.shade,
            flags: 0,
            layer: quad.layer,
            tint,
        });
        if block.is_transparent() {
            out.array_transparent.push_quad(vertices);
        } else {
            out.array_opaque.push_quad(vertices);
        }
    }
}

/// Build an overlay mesh over `box_`, textured with `tile` on all six faces —
/// used for the crack animation while a block is being mined. Callers pass the
/// block's targeting box, so cracks appear on the mushroom rather than around
/// the cell it stands in. Inflated slightly so it never z-fights the block.
pub fn mesh_block_overlay(box_: Aabb, tile: u32) -> CpuMesh {
    const INFLATE: f32 = 0.004;
    let center = (box_.min + box_.max) * 0.5;
    let size = (box_.max - box_.min) + Vec3::splat(2.0 * INFLATE);

    let mut mesh = CpuMesh::new();
    for dir in Direction::ALL {
        let (corners, uvs) = face_geometry(dir);
        let normal = dir.normal().to_array();
        let quad = std::array::from_fn(|i| ChunkVertex {
            position: std::array::from_fn(|a| center[a] + (corners[i][a] - 0.5) * size[a]),
            normal,
            uv: atlas_uv(tile, uvs[i]),
            ao: 1.0,
            flags: 0,
            layer: 0,
            tint: NO_TINT,
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
                layer: 0,
                tint: NO_TINT,
            }
        });
        mesh.push_quad(quad);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::GameContent;
    use crate::core::{ChunkPos, LocalPos};
    use crate::render::tiles;
    use crate::world::block::blocks;

    #[test]
    fn water_surface_is_two_texels_low() {
        let registry = BlockRegistry::with_builtins();
        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
        let out = mesh_chunk(
            &chunk,
            &registry,
            BlockModels::none(),
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        assert_eq!(out.transparent.vertices.len(), 24, "all six faces drawn");
        let max_y = out
            .transparent
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert_eq!(max_y, 10.0 + WATER_SURFACE);
    }

    #[test]
    fn stacked_water_merges_into_one_body() {
        let registry = BlockRegistry::with_builtins();
        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
        chunk.set(LocalPos { x: 4, y: 11, z: 4 }, blocks::WATER);
        let out = mesh_chunk(
            &chunk,
            &registry,
            BlockModels::none(),
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        // The shared face is culled from both blocks: 2 x 5 faces remain.
        assert_eq!(out.transparent.vertices.len(), 40);
        // The lower block reaches the upper block seamlessly (full height);
        // only the top of the column is lowered.
        let max_y = out
            .transparent
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert_eq!(max_y, 11.0 + WATER_SURFACE);
        assert!(
            out.transparent
                .vertices
                .iter()
                .any(|v| v.position[1] == 11.0)
        );
    }

    #[test]
    fn flowing_water_height_scales_with_level() {
        let registry = BlockRegistry::with_builtins();
        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 4 }, registry.flowing(0, 4));
        let out = mesh_chunk(
            &chunk,
            &registry,
            BlockModels::none(),
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        let max_y = out
            .transparent
            .vertices
            .iter()
            .map(|v| v.position[1])
            .fold(f32::MIN, f32::max);
        assert_eq!(max_y, 10.0 + WATER_SURFACE * 0.5);
    }

    #[test]
    fn adjacent_water_levels_connect_at_shared_corners() {
        let registry = BlockRegistry::with_builtins();
        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
        chunk.set(LocalPos { x: 5, y: 10, z: 4 }, registry.flowing(0, 4));
        let out = mesh_chunk(
            &chunk,
            &registry,
            BlockModels::none(),
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        let max_top_at = |x: f32| {
            out.transparent
                .vertices
                .iter()
                .filter(|v| v.position[0] == x && v.position[1] > 10.0)
                .map(|v| v.position[1])
                .fold(f32::MIN, f32::max)
        };
        // The flow block's edge shared with the source rises to the source's
        // surface — no step between the two blocks...
        assert_eq!(max_top_at(5.0), 10.0 + WATER_SURFACE);
        // ...while its far edge stays at its own level's height, so the top
        // face slopes downhill.
        assert_eq!(max_top_at(6.0), 10.0 + WATER_SURFACE * 0.5);
    }

    /// A model-backed block against the real shipped content: its geometry must
    /// go to the per-model bucket and *nothing* to the atlas passes, because a
    /// cube face would sample the block atlas with the model's own UVs.
    ///
    /// `load()` rather than `builtin()`: the embedded fallback carries the TOML
    /// but not the `.bbmodel` files, so nothing would have a model to bake.
    #[test]
    fn a_model_block_meshes_into_its_own_bucket_and_emits_no_cube_faces() {
        let content = GameContent::load();
        let models = content.block_models();
        let plant = content.blocks.find("blue bells").expect("shipped block");
        let (placement, _) = models.of(plant).expect("blue bells declares a model");

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, plant);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        assert!(out.opaque.is_empty(), "no atlas geometry");
        assert!(out.transparent.is_empty(), "no blended geometry");
        let bucket = out.models.get(&placement.id).expect("bucket for the model");
        assert!(!bucket.is_empty());

        // Baked into its own cell: `offset = [-0.5, 0, -0.5]` re-centres the
        // 0..1 authored model, so any yaw keeps it inside the block's column.
        for v in &bucket.vertices {
            assert!((4.0..=5.0).contains(&v.position[0]), "x {:?}", v.position);
            assert!((6.0..=7.0).contains(&v.position[2]), "z {:?}", v.position);
            assert!(v.position[1] >= 9.9, "y {:?}", v.position);
        }
    }

    // --- Blockbench-authored blocks ------------------------------------------

    /// The shipped grass block against the real content: it must go to the
    /// array buffers and emit *no* atlas geometry, since its faces sample
    /// 256-pixel array layers that no atlas UV could address.
    ///
    /// `load()` rather than `builtin()`: the embedded fallback carries the TOML
    /// but not `assets/blocks/*.json`, so nothing would have a model to bake.
    #[test]
    fn a_blockbench_block_meshes_into_the_array_buffers() {
        let content = GameContent::load();
        let models = content.block_models();
        let grass = content.blocks.find("grass").expect("shipped block");
        assert!(models.baked_of(grass).is_some(), "grass should be modelled");

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, grass);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        assert!(out.opaque.is_empty(), "no atlas geometry");
        assert!(out.transparent.is_empty(), "no blended atlas geometry");
        assert!(out.models.is_empty(), "no per-model bucket");
        assert!(!out.array_opaque.is_empty());
        // Six cube faces plus the four tinted overlay sides.
        assert_eq!(out.array_opaque.vertices.len(), 10 * 4);
        // Confined to its own cell, give or take the sliver the overlay is
        // nudged out by so it wins the depth test against the cube beneath it.
        const SLACK: f32 = 0.01;
        for v in &out.array_opaque.vertices {
            assert_ne!(
                v.layer, 0,
                "content must not sample the missing-texture layer"
            );
            assert!(
                (4.0 - SLACK..=5.0 + SLACK).contains(&v.position[0]),
                "x {:?}",
                v.position
            );
            assert!(
                (10.0 - SLACK..=11.0 + SLACK).contains(&v.position[1]),
                "y {:?}",
                v.position
            );
        }
    }

    /// `cullface` is the whole point of the format: without it every buried
    /// block would draw all six faces.
    #[test]
    fn stacked_blockbench_blocks_cull_the_face_they_share() {
        let content = GameContent::load();
        let models = content.block_models();
        let dirt = content.blocks.find("dirt").expect("shipped block");

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, dirt);
        let lone = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );
        assert_eq!(lone.array_opaque.vertices.len(), 6 * 4, "all six faces");

        chunk.set(LocalPos { x: 4, y: 11, z: 6 }, dirt);
        let stacked = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );
        assert_eq!(
            stacked.array_opaque.vertices.len(),
            10 * 4,
            "the shared face is dropped from both blocks"
        );
    }

    /// The old and new paths have to cull against each other, or the seam
    /// between a migrated block and one still on the atlas would show through.
    #[test]
    fn a_blockbench_block_and_an_atlas_block_cull_each_other() {
        let content = GameContent::load();
        let models = content.block_models();
        let dirt = content.blocks.find("dirt").expect("modelled");
        let atlas = content.blocks.find("bedrock").expect("still on the atlas");
        assert!(
            models.baked_of(atlas).is_none(),
            "bedrock must not be modelled"
        );

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, dirt);
        chunk.set(LocalPos { x: 4, y: 11, z: 6 }, atlas);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        assert_eq!(out.array_opaque.vertices.len(), 5 * 4, "dirt loses its top");
        assert_eq!(out.opaque.vertices.len(), 5 * 4, "bedrock loses its bottom");
    }

    /// A `tintindex` face takes the biome colour; everything else keeps the
    /// colour its texture was painted with.
    #[test]
    fn only_tinted_faces_take_the_biome_colour() {
        const BIOME: [u8; 4] = [10, 200, 30, 255];
        let content = GameContent::load();
        let models = content.block_models();
        let grass = content.blocks.find("grass").expect("shipped block");

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, grass);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| BIOME,
        );

        let tinted = out.array_opaque.vertices.iter().filter(|v| v.tint == BIOME);
        // The grass top plus the four side overlays.
        assert_eq!(tinted.count(), 5 * 4);
        assert!(
            out.array_opaque.vertices.iter().any(|v| v.tint == NO_TINT),
            "the dirt bottom and the base sides must stay untinted"
        );
    }

    /// Leaves are a cutout, so they occlude nothing — which would leave every
    /// face of every block inside a canopy drawn. They cull against each other
    /// instead, exactly as the atlas path has always done.
    #[test]
    fn stacked_leaves_cull_the_face_they_share() {
        let content = GameContent::load();
        let models = content.block_models();
        let leaves = content.blocks.find("oak leaves").expect("shipped block");
        let baked = models.baked_of(leaves).expect("modelled");
        assert_eq!(
            baked.occludes, [false; 6],
            "a see-through texture must not claim to occlude"
        );

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 4, y: 10, z: 6 }, leaves);
        chunk.set(LocalPos { x: 4, y: 11, z: 6 }, leaves);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );
        assert_eq!(
            out.array_opaque.vertices.len(),
            10 * 4,
            "the shared face is dropped from both blocks"
        );
    }

    /// A `random_yaw` model turns about its own cell, so two instances read as
    /// different plants rather than a grid of clones — and a turned face no
    /// longer points where its `cullface` claims, so culling is dropped.
    #[test]
    fn a_random_yaw_block_turns_with_its_position() {
        let content = GameContent::load();
        let models = content.block_models();
        let flower = content.blocks.find("cornflower").expect("shipped block");
        assert!(models.baked_of(flower).expect("modelled").random_yaw);

        let mesh_at = |x: u8, z: u8| {
            let mut chunk = Chunk::new(ChunkPos::new(0, 0));
            chunk.set(LocalPos { x, y: 10, z }, flower);
            mesh_chunk(
                &chunk,
                &content.blocks,
                models,
                |_| BlockId::AIR,
                |_, _, _| NO_TINT,
            )
            .array_opaque
        };
        let a = mesh_at(0, 0);
        let b = mesh_at(1, 0);
        assert_eq!(a.vertices.len(), b.vertices.len());
        assert_ne!(
            a.vertices[0].normal, b.vertices[0].normal,
            "neighbouring flowers must not share an angle"
        );

        // Turned or not, it stays inside its own column.
        for (mesh, x) in [(&a, 0.0f32), (&b, 1.0)] {
            for v in &mesh.vertices {
                assert!(
                    (x - 0.05..=x + 1.05).contains(&v.position[0]),
                    "{:?} left its cell",
                    v.position
                );
            }
        }
    }

    /// Two elements sharing a box is how the grass overlay is authored; without
    /// the nudge the overlay would lose the depth test and never appear.
    #[test]
    fn the_grass_overlay_sits_just_outside_the_block_it_covers() {
        let content = GameContent::load();
        let models = content.block_models();
        let grass = content.blocks.find("grass").expect("shipped block");

        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x: 0, y: 10, z: 0 }, grass);
        let out = mesh_chunk(
            &chunk,
            &content.blocks,
            models,
            |_| BlockId::AIR,
            |_, _, _| NO_TINT,
        );

        // North is -Z, so the overlay pokes out below z = 0.
        let min_z = out
            .array_opaque
            .vertices
            .iter()
            .map(|v| v.position[2])
            .fold(f32::MAX, f32::min);
        assert!(
            min_z < 0.0,
            "the overlay should sit proud of the cube: {min_z}"
        );
        assert!(min_z > -0.01, "but only barely: {min_z}");
    }

    /// Two of the same plant must land at different angles, and each must
    /// re-mesh to the angle it had before — the yaw is a hash of the position,
    /// not a counter or a random draw.
    #[test]
    fn random_yaw_varies_by_position_and_is_stable() {
        let a = block_yaw(BlockPos::new(4, 70, 6));
        let b = block_yaw(BlockPos::new(5, 70, 6));
        assert_ne!(a, b, "neighbours must not share an angle");
        assert_eq!(a, block_yaw(BlockPos::new(4, 70, 6)), "re-mesh is stable");
        for pos in [
            BlockPos::new(0, 0, 0),
            BlockPos::new(-31, 64, 999),
            BlockPos::new(i32::MIN, 1, i32::MAX),
        ] {
            let yaw = block_yaw(pos);
            assert!(
                (0.0..std::f32::consts::TAU).contains(&yaw),
                "{pos:?}: {yaw}"
            );
        }
    }

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
        let cell = Aabb::block(Vec3::new(2.0, 5.0, -3.0));
        let mesh = mesh_block_overlay(cell, tiles::CRACK_0);
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

    /// Cracks on ground cover wrap the plant's own box, not the cell around it.
    #[test]
    fn block_overlay_follows_a_smaller_hitbox() {
        let box_ = Aabb::new(Vec3::new(2.3, 5.0, -2.7), Vec3::new(2.7, 5.5, -2.3));
        let mesh = mesh_block_overlay(box_, tiles::CRACK_0);
        for v in &mesh.vertices {
            assert!(v.position[0] > 2.29 && v.position[0] < 2.71);
            assert!(v.position[1] > 4.99 && v.position[1] < 5.51);
            assert!(v.position[2] > -2.71 && v.position[2] < -2.29);
        }
    }
}

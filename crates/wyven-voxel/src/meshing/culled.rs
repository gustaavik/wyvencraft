//! Face-culling chunk mesher.

use glam::Vec3;

use super::ChunkMeshOutput;
use crate::appearance::{FaceTextures, FluidInfo, RenderType};
use crate::blockmodel::BakedBlockModel;
use crate::catalog::BlockCatalog;
use crate::chunk::Chunk;
use wyven_core::math::rotate_y;
use wyven_core::{Aabb, BlockId, BlockPos, CHUNK_HEIGHT, CHUNK_SIZE, Direction};
use wyven_render::mesh::CpuMesh;
use wyven_render::texture::atlas_uv;
use wyven_render::vertex::{ChunkVertex, NO_OVERLAY, NO_TINT, anim_flags};

/// How many biome colours a block model's `tintindex` can choose between.
/// Minecraft's numbering: `0` grass, `1` foliage, `2` water.
pub const TINT_SOURCES: usize = 3;

/// Top of a source fluid block: two texels (2/16) below the block top. Public
/// because it is part of the rendered contract — a game asserting on where its
/// water surface lands needs the same number the mesher used.
/// Flowing water scales down from here with its level.
pub const WATER_SURFACE: f32 = 14.0 / 16.0;

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
    catalog: &impl BlockCatalog,
) -> [[f32; 2]; 2] {
    let mut heights = [[0.0f32; 2]; 2];
    for cx in 0..2i32 {
        for cz in 0..2i32 {
            let mut h = 0.0f32;
            for (ox, oz) in [(cx - 1, cz - 1), (cx, cz - 1), (cx - 1, cz), (cx, cz)] {
                let cell = BlockPos::new(pos.x + ox, pos.y, pos.z + oz);
                let Some(fluid) = catalog.fluid(sample(cell)) else {
                    continue;
                };
                let above = sample(cell.offset(Direction::PosY));
                if catalog.is_fluid(above) || catalog.is_opaque(above) {
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
pub(super) fn face_shade(dir: Direction) -> f32 {
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
pub fn block_yaw(pos: BlockPos) -> f32 {
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
    catalog: &impl BlockCatalog,
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
        match catalog.baked(block) {
            Some(baked) => baked.occludes[dir as usize],
            None => catalog.is_opaque(block),
        }
    };

    for ly in 0..CHUNK_HEIGHT {
        for lz in 0..CHUNK_SIZE {
            for lx in 0..CHUNK_SIZE {
                let local = wyven_core::LocalPos {
                    x: lx as u8,
                    y: ly as u16,
                    z: lz as u8,
                };
                let id = chunk.get(local);
                if !catalog.is_visible(id) {
                    continue;
                }

                let world = BlockPos::new(origin.x + lx, ly, origin.z + lz);

                // A Blockbench-authored block replaces the whole cube too, but
                // unlike the `.bbmodel` path below it stays in the chunk's own
                // vertex buffer: its texture is a layer of the shared array, so
                // it needs no separate draw, and its `cullface` data lets it
                // take part in neighbour culling like an ordinary cube.
                if let Some(baked) = catalog.baked(id) {
                    // Same rule the cube path below uses: a transparent or
                    // cutout block also drops the face it shares with a
                    // neighbour of its own kind. Without it a leaf canopy —
                    // whose texture is see-through, so it occludes nothing —
                    // would emit every face of every block inside it.
                    let hidden = |dir: Direction| {
                        let neighbor = sample(world.offset(dir));
                        occludes(neighbor, dir.opposite())
                            || (neighbor == id
                                && (catalog.is_cutout(id) || catalog.is_transparent(id)))
                    };
                    push_baked_model(
                        &mut out,
                        baked,
                        catalog.render_type(id),
                        world,
                        hidden,
                        &tint,
                    );
                    continue;
                }

                // A model-backed block replaces the whole cube: its geometry
                // brings its own texture, so it goes to a per-model bucket and
                // no atlas face is emitted for it at all.
                if let Some((placement, model)) = catalog.placed_model(id) {
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
                let water = catalog.fluid(id);
                let heights = match water {
                    Some(_) => water_corner_heights(world, &sample, catalog),
                    None => [[1.0; 2]; 2],
                };
                // Sampled at most once per block: biome colour varies by
                // column, and all of a block's faces share one.
                let mut fluid_tint: [Option<[u8; 4]>; TINT_SOURCES] = [None; TINT_SOURCES];

                for dir in Direction::ALL {
                    let np = world.offset(dir);
                    let neighbor_id = sample(np);
                    let hidden = occludes(neighbor_id, dir.opposite());

                    // Transparent and cutout blocks also cull faces shared with
                    // a same-id neighbour (no interior faces inside a canopy or
                    // a body of water); all levels of a fluid count as one body.
                    let visible = if let Some(f) = water {
                        catalog
                            .fluid(neighbor_id)
                            .is_none_or(|nf| nf.group != f.group)
                            && !hidden
                    } else if catalog.is_transparent(id) || catalog.is_cutout(id) {
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
                    let tile = catalog.face_textures(id).tile(dir);

                    // A fluid with an animation strip samples the block texture
                    // array like a modelled block does; one without keeps the
                    // atlas tile its `textures` named.
                    let animated = water.and_then(|f| Some((f, catalog.fluid_texture(id)?)));
                    let (layer, flags, vertex_tint) = match animated {
                        Some((f, tex)) => {
                            // Minecraft's rule: a source is still everywhere, a
                            // flowing block only on the faces you look down on.
                            let still =
                                f.is_source() || matches!(dir, Direction::PosY | Direction::NegY);
                            let layers = if still { tex.still } else { tex.flowing };
                            let colour = match tex.tint {
                                Some(index) => {
                                    let slot = usize::from(index).min(TINT_SOURCES - 1);
                                    *fluid_tint[slot]
                                        .get_or_insert_with(|| tint(world.x, world.z, index))
                                }
                                None => NO_TINT,
                            };
                            (layers.first, anim_flags(layers.frames, tex.fps), colour)
                        }
                        None => (0, 0, NO_TINT),
                    };

                    let base = [world.x as f32, world.y as f32, world.z as f32];
                    let quad = std::array::from_fn(|i| {
                        // Corner y is 0 or 1: top corners take the (possibly
                        // per-corner lowered) surface height, so the top face
                        // and the side faces' upper edges move together.
                        let top = corners[i][1] > 0.5;
                        let y = if top {
                            heights[corners[i][0] as usize][corners[i][2] as usize]
                        } else {
                            0.0
                        };
                        let uv = match animated {
                            // Crop the texture to the lowered surface instead of
                            // stretching it: a side face's top edge sits `y` up
                            // the cell, and `v` runs downward from 0.
                            Some(_) if top && !matches!(dir, Direction::PosY | Direction::NegY) => {
                                [uvs[i][0], 1.0 - y]
                            }
                            Some(_) => uvs[i],
                            None => atlas_uv(tile, uvs[i]),
                        };
                        ChunkVertex {
                            position: [
                                base[0] + corners[i][0],
                                base[1] + y,
                                base[2] + corners[i][2],
                            ],
                            normal,
                            uv,
                            ao,
                            flags,
                            layer,
                            tint: vertex_tint,
                            // The cube mesher draws one texture per face; an
                            // overlay only ever comes off a block model.
                            overlay_layer: NO_OVERLAY,
                            overlay_tint: NO_TINT,
                        }
                    });

                    match (animated.is_some(), catalog.is_transparent(id)) {
                        (true, true) => out.array_transparent.push_quad(quad),
                        (true, false) => out.array_opaque.push_quad(quad),
                        (false, true) => out.transparent.push_quad(quad),
                        (false, false) => out.opaque.push_quad(quad),
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
    render: RenderType,
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
        let mut resolve_tint = |index: Option<u8>| match index {
            Some(index) => {
                let slot = usize::from(index).min(TINT_SOURCES - 1);
                *biome_tints[slot].get_or_insert_with(|| tint(world.x, world.z, index))
            }
            None => NO_TINT,
        };
        let vertex_tint = resolve_tint(quad.tint);
        // An overlay carries its own `tintindex`: the grass block tints the
        // grass growing over its side, not the dirt underneath.
        let overlay_tint = resolve_tint(quad.overlay_tint);
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
            tint: vertex_tint,
            overlay_layer: quad.overlay_layer.unwrap_or(NO_OVERLAY),
            overlay_tint,
        });
        if matches!(render, RenderType::Transparent) {
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
            overlay_layer: NO_OVERLAY,
            overlay_tint: NO_TINT,
        });
        mesh.push_quad(quad);
    }
    mesh
}

/// Append a small textured cube (a dropped block) to `mesh`: edge length `size`,
/// centred on `center`, spun `yaw` radians around Y. Faces are shaded and
/// textured like regular blocks so drops read as miniatures of their block.
///
/// Only *block* items belong here. An item with nothing but a flat icon is a
/// [`push_item_sprite`](super::sprite::push_item_sprite) instead — six faces of
/// the same picture reads as six apples rather than one.
pub fn push_item_cube(
    mesh: &mut CpuMesh,
    center: Vec3,
    size: f32,
    yaw: f32,
    textures: &FaceTextures,
) {
    for dir in Direction::ALL {
        let (corners, uvs) = face_geometry(dir);
        let normal = rotate_y(Vec3::from(dir.normal().to_array()), yaw).to_array();
        let ao = face_shade(dir);
        let tile = textures.tile(dir);
        let quad = std::array::from_fn(|i| {
            let local = rotate_y(
                Vec3::from(std::array::from_fn::<f32, 3, _>(|a| {
                    (corners[i][a] - 0.5) * size
                })),
                yaw,
            );
            ChunkVertex {
                position: (center + local).to_array(),
                normal,
                uv: atlas_uv(tile, uvs[i]),
                ao,
                flags: 0,
                layer: 0,
                tint: NO_TINT,
                overlay_layer: NO_OVERLAY,
                overlay_tint: NO_TINT,
            }
        });
        mesh.push_quad(quad);
    }
}

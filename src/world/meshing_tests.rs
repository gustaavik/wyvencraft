//! The chunk mesher, exercised against Wyvencraft's real content.
//!
//! `wyven_voxel` owns the mesher and tests it against a synthetic catalog — that
//! is what proves it needs no game. These tests are the other half: they run it
//! over `assets/blocks.toml`, the shipped `assets/blocks/*.json` and this game's
//! fluid definitions, which is where a bad `cullface`, a mis-columned water
//! animation or an untinted leaf would actually show up.
//!
//! They live in the game crate because that is the only side that has content.

use glam::Vec3;

use crate::art::tiles;
use crate::content::{BlockAppearance, FluidTexture, GameContent};
use crate::core::{Aabb, BlockId, BlockPos, ChunkPos, LocalPos};
use crate::world::block::{BlockRegistry, blocks};
use crate::world::meshing::culled::{WATER_SURFACE, block_yaw};
use crate::world::meshing::{ChunkMeshOutput, mesh_block_overlay, mesh_chunk, push_item_cube};
use crate::world::{BlockCatalog, Chunk, FaceTextures};
use wyven_render::block_textures::AnimatedLayers;
use wyven_render::vertex::{ANIM_FIELD_MASK, ANIM_FPS_SHIFT, ANIM_FRAMES_SHIFT};
use wyven_render::{CpuMesh, NO_TINT, texture::atlas_uv};

/// A fluid animation on distinct layer runs, so a face's column is
/// identifiable from the layer alone. Every fluid block shares one entry.
const WATER_ANIM: FluidTexture = FluidTexture {
    still: AnimatedLayers {
        first: 100,
        frames: 64,
    },
    flowing: AnimatedLayers {
        first: 200,
        frames: 64,
    },
    fps: 12,
    tint: Some(2),
};

/// The tint every fluid face should end up carrying in these tests.
const BIOME_WATER: [u8; 4] = [11, 22, 33, 255];

/// `BlockModels` where every block id is that fluid animation — the mesher
/// only ever asks about ids it is meshing, all of which are water here.
/// Every fluid id in `registry` animating through `WATER_ANIM`; nothing else
/// has a strip. The mesher only ever asks about ids it is meshing.
fn every_block_is_water(registry: &BlockRegistry) -> Vec<Option<FluidTexture>> {
    (0..registry.len())
        .map(|i| registry.fluid(BlockId(i as u16)).map(|_| WATER_ANIM))
        .collect()
}

/// A catalog over `registry` with no models at all — the fluid branch under
/// test takes its layers from `fluids`, not from geometry.
fn fluid_catalog<'a>(
    registry: &'a BlockRegistry,
    fluids: &'a [Option<FluidTexture>],
) -> BlockAppearance<'a> {
    BlockAppearance {
        blocks: registry,
        face_tiles: &[],
        models: EMPTY_MODELS.get_or_init(wyven_model::ModelRegistry::new),
        placed: &[],
        baked: &[],
        fluids,
    }
}

static EMPTY_MODELS: std::sync::OnceLock<wyven_model::ModelRegistry> = std::sync::OnceLock::new();

/// A catalog over `registry` with no visual tables at all: every block falls
/// back to the atlas tiles `blocks.toml` named for it.
fn plain_catalog(registry: &BlockRegistry) -> BlockAppearance<'_> {
    fluid_catalog(registry, &[])
}

fn mesh_water(chunk: &Chunk, registry: &BlockRegistry) -> ChunkMeshOutput {
    let fluids = every_block_is_water(registry);
    mesh_chunk(
        chunk,
        &fluid_catalog(registry, &fluids),
        |_| BlockId::AIR,
        |_, _, index| {
            assert_eq!(index, 2, "water asks for the water tint source");
            BIOME_WATER
        },
    )
}

/// Water is drawn from the block texture array now, not the atlas — so the
/// atlas buckets must stay empty even though water is not a model.
#[test]
fn animated_water_goes_to_the_array_buckets() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_water(&chunk, &registry);

    assert_eq!(out.array_transparent.vertices.len(), 24, "all six faces");
    assert!(out.transparent.is_empty(), "nothing left on the atlas");
    assert!(out.opaque.is_empty());
}

/// Minecraft's rule: a source block is still on every face.
#[test]
fn a_source_block_is_still_on_every_face() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_water(&chunk, &registry);

    assert!(
        out.array_transparent
            .vertices
            .iter()
            .all(|v| v.layer == WATER_ANIM.still.first)
    );
}

/// ...while a flowing block only keeps the still art where you look down on
/// it; its sides take the streaked flowing column.
#[test]
fn a_flowing_block_takes_the_flow_column_on_its_sides() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, registry.flowing(0, 4));
    let out = mesh_water(&chunk, &registry);

    for vertex in &out.array_transparent.vertices {
        let horizontal = vertex.normal[1] == 0.0;
        let expected = if horizontal {
            WATER_ANIM.flowing.first
        } else {
            WATER_ANIM.still.first
        };
        assert_eq!(vertex.layer, expected, "normal {:?}", vertex.normal);
    }
}

#[test]
fn every_water_vertex_carries_the_animation_and_the_biome_colour() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_water(&chunk, &registry);

    for vertex in &out.array_transparent.vertices {
        assert_eq!(vertex.tint, BIOME_WATER);
        assert_eq!(
            (vertex.flags >> ANIM_FRAMES_SHIFT) & ANIM_FIELD_MASK,
            u32::from(WATER_ANIM.still.frames)
        );
        assert_eq!(
            (vertex.flags >> ANIM_FPS_SHIFT) & ANIM_FIELD_MASK,
            u32::from(WATER_ANIM.fps)
        );
    }
}

/// The surface sits two texels low, so a side face is shorter than the cell.
/// Its UVs must be cropped to match or the texture stretches, and the waves
/// would not line up with the top face they meet.
#[test]
fn a_side_faces_uvs_are_cropped_to_the_lowered_surface() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_water(&chunk, &registry);

    let sides = out
        .array_transparent
        .vertices
        .iter()
        .filter(|v| v.normal[1] == 0.0);
    for vertex in sides {
        let expected = if vertex.position[1] > 10.5 {
            1.0 - WATER_SURFACE
        } else {
            1.0
        };
        assert_eq!(vertex.uv[1], expected, "v at y {}", vertex.position[1]);
    }
}

/// A fluid whose strip failed to load keeps the old atlas path, so a bad
/// PNG costs the art rather than the geometry.
#[test]
fn a_fluid_without_a_strip_stays_on_the_atlas() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_chunk(
        &chunk,
        &plain_catalog(&registry),
        |_| BlockId::AIR,
        |_, _, _| NO_TINT,
    );

    assert_eq!(out.transparent.vertices.len(), 24);
    assert!(out.array_transparent.is_empty());
}

#[test]
fn water_surface_is_two_texels_low() {
    let registry = BlockRegistry::with_builtins();
    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 4 }, blocks::WATER);
    let out = mesh_chunk(
        &chunk,
        &plain_catalog(&registry),
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
        &plain_catalog(&registry),
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
        &plain_catalog(&registry),
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
        &plain_catalog(&registry),
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
    let catalog = content.appearance();
    let plant = content.blocks.find("blue_bells").expect("shipped block");
    let (placement, _) = catalog
        .placed_model(plant)
        .expect("blue bells declares a model");

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, plant);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);

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
    let catalog = content.appearance();
    let grass = content.blocks.find("grass").expect("shipped block");
    assert!(catalog.baked(grass).is_some(), "grass should be modelled");

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, grass);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);

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
    let catalog = content.appearance();
    let dirt = content.blocks.find("dirt").expect("shipped block");

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, dirt);
    let lone = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);
    assert_eq!(lone.array_opaque.vertices.len(), 6 * 4, "all six faces");

    chunk.set(LocalPos { x: 4, y: 11, z: 6 }, dirt);
    let stacked = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);
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
    let catalog = content.appearance();
    let dirt = content.blocks.find("dirt").expect("modelled");
    let atlas = content.blocks.find("bedrock").expect("still on the atlas");
    assert!(
        catalog.baked(atlas).is_none(),
        "bedrock must not be modelled"
    );

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, dirt);
    chunk.set(LocalPos { x: 4, y: 11, z: 6 }, atlas);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);

    assert_eq!(out.array_opaque.vertices.len(), 5 * 4, "dirt loses its top");
    assert_eq!(out.opaque.vertices.len(), 5 * 4, "bedrock loses its bottom");
}

/// A `tintindex` face takes the biome colour; everything else keeps the
/// colour its texture was painted with.
#[test]
fn only_tinted_faces_take_the_biome_colour() {
    const BIOME: [u8; 4] = [10, 200, 30, 255];
    let content = GameContent::load();
    let catalog = content.appearance();
    let grass = content.blocks.find("grass").expect("shipped block");

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, grass);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| BIOME);

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
    let catalog = content.appearance();
    let leaves = content.blocks.find("oak_leaves").expect("shipped block");
    let baked = catalog.baked(leaves).expect("modelled");
    assert_eq!(
        baked.occludes, [false; 6],
        "a see-through texture must not claim to occlude"
    );

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 4, y: 10, z: 6 }, leaves);
    chunk.set(LocalPos { x: 4, y: 11, z: 6 }, leaves);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);
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
    let catalog = content.appearance();
    let flower = content.blocks.find("cornflower").expect("shipped block");
    assert!(catalog.baked(flower).expect("modelled").random_yaw);

    let mesh_at = |x: u8, z: u8| {
        let mut chunk = Chunk::new(ChunkPos::new(0, 0));
        chunk.set(LocalPos { x, y: 10, z }, flower);
        mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT).array_opaque
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
    let catalog = content.appearance();
    let grass = content.blocks.find("grass").expect("shipped block");

    let mut chunk = Chunk::new(ChunkPos::new(0, 0));
    chunk.set(LocalPos { x: 0, y: 10, z: 0 }, grass);
    let out = mesh_chunk(&chunk, &catalog, |_| BlockId::AIR, |_, _, _| NO_TINT);

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

//! Doubles that let the voxel layer be exercised with no game loaded.
//!
//! The point of [`BlockCatalog`] and [`BlockProperties`] is that a block id
//! needs no block *table* to mean something. These are the proof: a flat
//! generator and a catalog built from a short list of declarations, enough to
//! drive meshing, streaming and world edits.

use std::collections::HashMap;
use std::sync::Arc;

use wyven_core::{BlockId, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos, LocalPos};

use crate::appearance::{FaceTextures, FluidInfo, RenderType};
use crate::blockmodel::BakedBlockModel;
use crate::catalog::{BlockCatalog, BlockProperties};
use crate::chunk::Chunk;
use crate::fluid_texture::FluidTexture;
use crate::generate::WorldGenerator;
use wyven_model::{Model, ModelRegistry};

/// Terrain that is `block` strictly below `ground` and air at or above it.
/// Deterministic in nothing but its own numbers, which is all a generator has
/// to be.
pub struct FlatGenerator {
    pub seed: u64,
    pub ground: i32,
    pub block: BlockId,
}

impl FlatGenerator {
    /// Solid `block` up to (not including) y = `ground`.
    pub fn new(seed: u64, ground: i32, block: BlockId) -> Self {
        Self {
            seed,
            ground,
            block,
        }
    }
}

impl WorldGenerator for FlatGenerator {
    fn seed(&self) -> u64 {
        self.seed
    }

    fn generate(&self, pos: ChunkPos) -> Chunk {
        let mut chunk = Chunk::new(pos);
        for y in 0..self.ground.clamp(0, CHUNK_HEIGHT) {
            for z in 0..CHUNK_SIZE {
                for x in 0..CHUNK_SIZE {
                    chunk.set(
                        LocalPos {
                            x: x as u8,
                            y: y as u16,
                            z: z as u8,
                        },
                        self.block,
                    );
                }
            }
        }
        chunk
    }
}

/// One block id's declaration, as far as the voxel layer is concerned.
#[derive(Clone)]
pub struct TestBlock {
    pub render: RenderType,
    pub textures: FaceTextures,
    pub fluid: Option<FluidInfo>,
    pub fluid_texture: Option<FluidTexture>,
    pub baked: Option<BakedBlockModel>,
    pub solid: bool,
}

impl Default for TestBlock {
    fn default() -> Self {
        Self {
            render: RenderType::Opaque,
            textures: FaceTextures::uniform(1),
            fluid: None,
            fluid_texture: None,
            baked: None,
            solid: true,
        }
    }
}

/// A catalog assembled from declarations, indexed by `BlockId`.
///
/// Id 0 is always air (invisible, non-solid); everything else is whatever it was
/// pushed as. Unknown ids read as air, so a test never has to enumerate the
/// blocks it does not care about.
#[derive(Default)]
pub struct TestCatalog {
    blocks: Vec<TestBlock>,
    models: ModelRegistry,
    placed: HashMap<BlockId, crate::appearance::BlockModel>,
}

impl TestCatalog {
    /// A catalog holding only air.
    pub fn new() -> Self {
        Self {
            blocks: vec![TestBlock {
                render: RenderType::Invisible,
                solid: false,
                ..TestBlock::default()
            }],
            ..Self::default()
        }
    }

    /// Declare the next block id.
    pub fn push(&mut self, block: TestBlock) -> BlockId {
        self.blocks.push(block);
        BlockId((self.blocks.len() - 1) as u16)
    }

    /// Declare a plain opaque cube textured with `tile` on every face.
    pub fn cube(&mut self, tile: u32) -> BlockId {
        self.push(TestBlock {
            textures: FaceTextures::uniform(tile),
            ..TestBlock::default()
        })
    }

    /// The model registry backing [`BlockCatalog::placed_model`], for tests that
    /// need one.
    pub fn models_mut(&mut self) -> &mut ModelRegistry {
        &mut self.models
    }

    /// Attach a placed model to `id`.
    pub fn place(&mut self, id: BlockId, placement: crate::appearance::BlockModel) {
        self.placed.insert(id, placement);
    }

    fn get(&self, id: BlockId) -> &TestBlock {
        self.blocks.get(id.0 as usize).unwrap_or(&self.blocks[0])
    }

    /// Wrap in an `Arc` for [`World::new`](crate::World::new).
    pub fn shared(self) -> Arc<dyn BlockProperties> {
        Arc::new(TestProperties(self.blocks))
    }
}

impl BlockCatalog for TestCatalog {
    fn render_type(&self, id: BlockId) -> RenderType {
        self.get(id).render
    }

    fn face_textures(&self, id: BlockId) -> FaceTextures {
        self.get(id).textures
    }

    fn fluid(&self, id: BlockId) -> Option<FluidInfo> {
        self.get(id).fluid
    }

    fn baked(&self, id: BlockId) -> Option<&BakedBlockModel> {
        self.get(id).baked.as_ref()
    }

    fn placed_model(&self, id: BlockId) -> Option<(&crate::appearance::BlockModel, &Model)> {
        let placement = self.placed.get(&id)?;
        Some((placement, self.models.get(placement.id)?))
    }

    fn fluid_texture(&self, id: BlockId) -> Option<&FluidTexture> {
        self.get(id).fluid_texture.as_ref()
    }
}

/// The [`BlockProperties`] half of a [`TestCatalog`].
struct TestProperties(Vec<TestBlock>);

impl TestProperties {
    fn get(&self, id: BlockId) -> &TestBlock {
        self.0.get(id.0 as usize).unwrap_or(&self.0[0])
    }
}

impl BlockProperties for TestProperties {
    fn is_solid(&self, id: BlockId) -> bool {
        self.get(id).solid
    }

    fn is_targetable(&self, id: BlockId) -> bool {
        let b = self.get(id);
        b.solid || (!matches!(b.render, RenderType::Invisible) && b.fluid.is_none())
    }

    fn is_replaceable(&self, id: BlockId) -> bool {
        let b = self.get(id);
        !b.solid && b.fluid.is_none() && !matches!(b.render, RenderType::Invisible)
    }
}

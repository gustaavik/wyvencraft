//! Mobs, arrows and dropped items, and the per-model textures they bring.

use super::*;

impl SceneCache {
    /// Rebuild one mesh per visible mob — the authority's own simulated mobs
    /// plus, on a client, the host's replicas (whose animation is driven from
    /// their rendered movement, like remote players).
    pub fn update_mob_meshes<'a>(
        &mut self,
        ctx: &Arc<RenderContext>,
        mobs: impl IntoIterator<Item = MobSprite<'a>>,
        models: &ModelRegistry,
    ) {
        self.mob_meshes.clear();
        let visuals: Vec<_> = mobs
            .into_iter()
            .filter_map(|mob| mob_mesh(mob.visual, mob.position, mob.yaw, &mob.pose, models))
            .collect();
        for visual in visuals {
            if let Some(id) = visual.model {
                self.ensure_model_texture(ctx, models, id);
            }
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &visual.mesh) {
                self.mob_meshes.push((gpu, visual.model));
            }
        }
    }

    /// Upload a model's texture the first time something asks to draw it.
    ///
    /// Model textures cannot be built with the block atlas at startup: game
    /// states are constructed before the `Renderer` (and its device) exists, so
    /// the first frame that needs one is the earliest point this can happen.
    pub(in super::super) fn ensure_model_texture(
        &mut self,
        ctx: &Arc<RenderContext>,
        models: &ModelRegistry,
        id: ModelId,
    ) {
        let index = id.0 as usize;
        if self.model_textures.len() <= index {
            self.model_textures.resize_with(index + 1, || None);
        }
        if self.model_textures[index].is_some() {
            return;
        }
        let Some(model) = models.get(id) else {
            return;
        };
        match Texture::create(ctx, &model.texture) {
            Ok(texture) => self.model_textures[index] = Some(texture),
            // Without a texture the mesh would sample whatever was bound last,
            // so it is simply not drawn (see `textured_mesh`).
            Err(err) => log::warn!("could not upload model texture: {err}"),
        }
    }

    /// Pair a mesh with its model texture, or `None` if the texture is missing.
    pub(super) fn textured_mesh<'a>(
        &'a self,
        mesh: &'a GpuMesh,
        id: ModelId,
    ) -> Option<TexturedMesh<'a>> {
        let texture = self.model_textures.get(id.0 as usize)?.as_ref()?;
        Some(TexturedMesh { mesh, texture })
    }

    /// Bake a model under `transform` and upload it, keeping its id alongside so
    /// the draw can bind the right texture.
    pub(super) fn bake_model(
        &mut self,
        ctx: &Arc<RenderContext>,
        models: &ModelRegistry,
        id: ModelId,
        transform: glam::Mat4,
    ) -> Option<(GpuMesh, ModelId)> {
        self.ensure_model_texture(ctx, models, id);
        let mesh = models.get(id)?.mesh.bake(transform);
        let gpu = GpuMesh::upload(&ctx.memory_allocator, &mesh)
            .ok()
            .flatten()?;
        Some((gpu, id))
    }

    /// Rebuild the combined arrow mesh (small cubes, like the drops pass).
    ///
    /// `shaft` is the arrow item's own faces, resolved by the caller — an arrow
    /// in flight is not an inventory stack, so it cannot look itself up.
    pub fn update_arrows_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        arrows: impl IntoIterator<Item = ArrowSprite>,
        shaft: FaceTextures,
    ) {
        let mut mesh = CpuMesh::new();
        for arrow in arrows {
            push_item_cube(&mut mesh, arrow.position, arrow.size, arrow.yaw, &shaft);
        }
        self.arrows_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    // --- World overlays -------------------------------------------------------------

    /// Rebuild the combined drop meshes (opaque + transparent passes). Drops are
    /// few and tiny, so a per-frame rebuild stays cheap, like remote players.
    ///
    /// Takes an iterator of plain [`DropSprite`]s rather than entities, so the
    /// view never learns where drops are stored — and so the item placement
    /// editor can chain a still preview drop onto the real ones, down this exact
    /// path rather than an approximation of it.
    pub fn update_drops_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        drops: impl IntoIterator<Item = DropSprite>,
        content: ModelContent<'_>,
    ) {
        let (shape, tiles) = (content.shape, content.tiles);
        let mut opaque = CpuMesh::new();
        let mut transparent = CpuMesh::new();
        // Drops whose item declares a model are drawn as that model instead of
        // the default spinning cube. Those cannot join the merged cube meshes —
        // each needs its own texture bound — so they are grouped by model, one
        // mesh per model however many drops share it.
        let mut by_model: HashMap<ModelId, CpuMesh> = HashMap::new();
        for item in drops {
            if let Some(model) = content.of(item.item)
                && let Some(loaded) = content.models.get(model.id)
            {
                let transform = model_mesh::anchor(item.center, item.yaw, 0.0)
                    * content.local(item.item, model, DisplayContext::Ground);
                let mesh = loaded.mesh.bake(transform);
                let entry = by_model.entry(model.id).or_default();
                entry.push_indexed(mesh.vertices, mesh.indices);
                continue;
            }
            let (shape, is_transparent) = shape(item.item);
            let target = if is_transparent {
                &mut transparent
            } else {
                &mut opaque
            };
            match shape {
                ItemShape::Cube(faces) => {
                    push_item_cube(target, item.center, item.size, item.yaw, &faces)
                }
                ItemShape::Sprite(tile) => {
                    let sprite = self
                        .item_sprites
                        .entry(tile)
                        .or_insert_with(|| ItemSprite::new(tile, tiles.art(tile)));
                    push_item_sprite(target, sprite, item.center, item.size, item.yaw);
                }
            }
        }
        self.drops_mesh = GpuMesh::upload(&ctx.memory_allocator, &opaque)
            .ok()
            .flatten();
        self.drops_mesh_transparent = GpuMesh::upload(&ctx.memory_allocator, &transparent)
            .ok()
            .flatten();

        self.drops_model_meshes.clear();
        for (id, mesh) in by_model {
            self.ensure_model_texture(ctx, content.models, id);
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.drops_model_meshes.push((gpu, id));
            }
        }
    }
}

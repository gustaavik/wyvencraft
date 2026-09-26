//! Assembling the frame: every mesh this cache holds, culled and split into
//! the render passes.

use super::*;

impl SceneCache {
    // --- Frames ---------------------------------------------------------------------

    /// Collect this frame's visible geometry: frustum-culled chunk meshes plus
    /// every entity and overlay mesh, split by render pass.
    ///
    /// Takes the finished camera rather than building one: it is derived once,
    /// by [`InGameState::world_camera`], and shared with the nameplate pass so
    /// the two cannot disagree about where the viewer is.
    pub fn scene_frame(
        &self,
        player: &Player,
        day_cycle: &DayCycle,
        clock: f32,
        camera: Camera,
    ) -> SceneFrame<'_> {
        let frustum = camera.frustum();
        let in_view = |pos: &ChunkPos| chunk_in_view(&frustum, *pos);

        // Chunk meshes, frustum-culled by their column AABB. Blockbench-authored
        // blocks sample the block texture array, so they are separate lists
        // even though they are the same geometry kind.
        let mut opaque = visible(&self.meshes, in_view);
        let mut transparent = visible(&self.transparent_meshes, in_view);
        let array_opaque = visible(&self.array_meshes, in_view);
        let array_transparent = visible(&self.array_transparent_meshes, in_view);
        self.push_entity_meshes(&mut opaque, &mut transparent);

        let (sky, light) = sky_and_light(day_cycle, &camera);
        SceneFrame {
            view_proj: camera.view_projection(),
            sky,
            light,
            time: clock,
            opaque,
            transparent,
            array_opaque,
            array_transparent,
            textured: self.textured_meshes(in_view),
            lines: self.outline_mesh.as_ref(),
            foreground: self.foreground_frame(player, camera.aspect),
        }
    }

    /// Every atlas-sampling entity mesh, into the pass it draws in. None is
    /// frustum-culled: there are few, and the break overlay's block is within
    /// reach by definition.
    fn push_entity_meshes<'a>(
        &'a self,
        opaque: &mut Vec<&'a GpuMesh>,
        transparent: &mut Vec<&'a GpuMesh>,
    ) {
        // Crack overlay on the block being mined, blended over everything else.
        if let Some(mesh) = &self.break_mesh {
            transparent.push(mesh);
        }

        // The local player model (third person only) + remote players + mobs.
        if let Some(mesh) = &self.player_mesh {
            opaque.push(mesh);
        }
        opaque.extend(&self.remote_meshes);
        opaque.extend(
            self.mob_meshes
                .iter()
                .filter_map(|(mesh, model)| model.is_none().then_some(mesh)),
        );
        if let Some(mesh) = &self.arrows_mesh {
            opaque.push(mesh);
        }

        // A held item with no model of its own, split by pass for the same
        // reason drops are: a glass block in the fist must blend like glass.
        // Every fist in the world, not just this one's — a peer holding glass
        // has to blend too.
        for (mesh, is_transparent) in self.held_atlas.iter().chain(&self.remote_held_atlas) {
            if *is_transparent {
                transparent.push(mesh);
            } else {
                opaque.push(mesh);
            }
        }

        // Dropped items, split by pass like the blocks they represent.
        if let Some(mesh) = &self.drops_mesh {
            opaque.push(mesh);
        }
        if let Some(mesh) = &self.drops_mesh_transparent {
            transparent.push(mesh);
        }
    }

    /// Everything drawn from a model file, each with its own texture.
    /// Model-backed blocks are culled by their chunk column like the atlas
    /// meshes; entities are not.
    fn textured_meshes(&self, in_view: impl Fn(&ChunkPos) -> bool) -> Vec<TexturedMesh<'_>> {
        self.mob_meshes
            .iter()
            .filter_map(|(mesh, model)| self.textured_mesh(mesh, (*model)?))
            .chain(
                self.drops_model_meshes
                    .iter()
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            .chain(
                self.held_mesh
                    .iter()
                    .chain(&self.remote_held)
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            .chain(
                self.model_meshes
                    .iter()
                    .filter(|(pos, _)| in_view(pos))
                    .flat_map(|(_, chunk)| chunk.iter())
                    .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id)),
            )
            .collect()
    }

    /// The view model, framed by its own camera.
    ///
    /// Its own, because the field of view a player picks for the world should
    /// not distort their own hand — and because the renderer clears depth before
    /// drawing it, so it needs no relationship to the world's near plane.
    fn foreground_frame(&self, player: &Player, aspect: f32) -> Option<ForegroundFrame<'_>> {
        let arm = self.hand_mesh.as_ref()?;
        let mut camera = Camera::new(viewmodel::HAND_FOV_DEGREES, aspect);
        // The hand is baked in world space against the same eye the world pass
        // uses, so the foreground camera has to sit exactly there too — only its
        // field of view differs.
        camera.position = player.interpolated_eye_position(self.render_alpha);
        camera.forward = player.look_direction();
        Some(ForegroundFrame {
            view_proj: camera.view_projection(),
            atlas: std::iter::once(arm)
                .chain(self.hand_held_atlas.as_ref())
                .collect(),
            textured: self
                .hand_held_mesh
                .iter()
                .filter_map(|(mesh, id)| self.textured_mesh(mesh, *id))
                .collect(),
        })
    }
}

/// Which display table places a held item that has no model file of its own.
///
/// A cube takes Minecraft's `block/block` numbers; a flat sprite is precisely
/// the geometry `item/generated` describes, so it takes that model's standard
/// placement — the same one every extruded 2D item already uses, which is why
/// an apple in the fist and a lump of coal in the fist agree.
pub(super) fn held_placement(
    shape: ItemShape,
    context: DisplayContext,
    block_display: &wyven_model::DisplayTransforms,
) -> wyven_model::display::ItemTransform {
    match shape {
        ItemShape::Cube(_) => block_display.get(context).unwrap_or_default(),
        ItemShape::Sprite(_) => wyven_model::generated::default_display()
            .get(context)
            .unwrap_or_default(),
    }
}

/// Whether any of a chunk column is inside the frustum.
fn chunk_in_view(frustum: &wyven_core::Frustum, pos: ChunkPos) -> bool {
    let origin = pos.origin();
    let aabb = Aabb::new(
        Vec3::new(origin.x as f32, 0.0, origin.z as f32),
        Vec3::new(
            (origin.x + CHUNK_SIZE) as f32,
            CHUNK_HEIGHT as f32,
            (origin.z + CHUNK_SIZE) as f32,
        ),
    );
    frustum.intersects_aabb(aabb)
}

/// The meshes of every chunk `in_view` keeps.
fn visible(
    meshes: &HashMap<ChunkPos, GpuMesh>,
    in_view: impl Fn(&ChunkPos) -> bool,
) -> Vec<&GpuMesh> {
    meshes
        .iter()
        .filter(|(pos, _)| in_view(pos))
        .map(|(_, mesh)| mesh)
        .collect()
}

/// The sky and the light it casts, from the time of day, for this camera.
fn sky_and_light(day_cycle: &DayCycle, camera: &Camera) -> (SkyParams, LightParams) {
    let atmo = day_cycle.atmosphere();
    (
        SkyParams {
            inv_view_proj: camera.sky_inv_view_proj(),
            sun_dir: atmo.sun_dir,
            zenith_color: atmo.zenith_color,
            horizon_color: atmo.horizon_color,
            sun_color: atmo.sun_color,
            star_intensity: atmo.star_intensity,
            moon_intensity: atmo.moon_intensity,
        },
        LightParams {
            light_dir: atmo.light_dir,
            light_color: atmo.light_color,
            ambient: atmo.ambient,
        },
    )
}

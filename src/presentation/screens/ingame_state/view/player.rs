//! The local player's body, their first-person hand and what it holds, and
//! other players' bodies — every rigged character.

use super::*;

impl SceneCache {
    // --- Animated models ------------------------------------------------------------

    /// Resolve the player's rigged model, its clips and the bones that matter.
    ///
    /// Idempotent and cheap after the first call, which is why it can sit on the
    /// per-frame path: content is only reachable from there, and a model that
    /// fails to load simply leaves the player undrawn rather than the frame
    /// panicking — the same fail-soft the rest of the model pipeline takes.
    pub fn bind_player_rig(&mut self, entities: &EntityRegistry, models: &ModelRegistry) {
        if self.player_rig.is_some() {
            return;
        }
        let Some(kind) = entities.find("player") else {
            return;
        };
        let VisualSpec::Rigged(visual) = &kind.visual else {
            return;
        };
        let Some(movement) = kind.movement.as_ref() else {
            return;
        };
        let Some(id) = models.find(&visual.path) else {
            return;
        };
        let Some(rig) = models.get(id).and_then(|model| model.rig.as_ref()) else {
            log::warn!(
                "{} carries no rig; the player will not be drawn",
                visual.path
            );
            return;
        };
        self.player_rig = Some(PlayerRig {
            model: id,
            scale: visual.scale,
            sheet: visual
                .skin
                .as_deref()
                .and_then(mobskin::origin_for)
                .unwrap_or(skin::SKIN_ORIGIN),
            clips: HumanoidRig::bind(rig, movement),
        });
    }

    /// Drop the third-person body and whatever it was holding. One helper
    /// because the two must always go together: a held item left behind after
    /// the body it hung off is gone would float in the world on its own.
    fn clear_player_meshes(&mut self) {
        self.player_mesh = None;
        self.held_mesh = None;
        self.held_atlas = None;
    }

    /// The player as something drawable, or `None` before the rig is bound.
    fn character<'a>(&'a self, models: &'a ModelRegistry) -> Option<Character<'a>> {
        let rig = self.player_rig.as_ref()?;
        Some(Character {
            model: models.get(rig.model)?,
            clips: &rig.clips,
            scale: rig.scale,
            sheet: rig.sheet,
        })
    }

    /// Rebuild the player model mesh in third person, or the view model in
    /// first — never both, since in first person the body is the camera.
    ///
    /// `inspect` is how far through the inventory's camera pan we are. Past
    /// [`INSPECT_MODEL_FROM`] it forces the world model on even in first
    /// person, which would otherwise swing the camera out to frame an empty
    /// stage, and suppresses the view-model arm at the same instant, since the
    /// two are alternatives. It also levels the model's head: the pose inherits
    /// the player's pitch, so a player who opened the inventory while looking
    /// up would be shown a model staring at the ceiling.
    pub fn update_player_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        anim: &AnimationState,
        inventory: &Inventory,
        inspect: f32,
        content: ModelContent<'_>,
    ) {
        let show_body = inspect >= INSPECT_MODEL_FROM;
        if player.perspective.is_first_person() && !show_body {
            self.player_mesh = None;
            self.held_mesh = None;
            self.held_atlas = None;
            self.update_hand_meshes(ctx, player, anim, inventory, content);
            return;
        }
        self.hand_mesh = None;
        self.hand_held_mesh = None;
        self.hand_held_atlas = None;

        // The body is drawn at the torso yaw, which lags the look yaw the camera
        // uses; the head bone's own turn is what puts the face back where the
        // player looks. The held item hangs off the hand *bone* under the same
        // pose, so it cannot drift out of the fist however the elbow bends.
        let body_yaw = anim.body_yaw();
        // Drawn at the interpolated position, not the raw one: physics steps at
        // a fixed rate while this runs every frame, and the camera is built from
        // the *same* interpolation. Baking the body at `player.position` instead
        // makes it lurch one tick's worth against a camera that glides — which
        // reads as the whole player juddering, most of all in a jump, where a
        // tick is 0.15 blocks straight up.
        let render_position = player.interpolated_position(self.render_alpha);
        let baked = {
            let Some(character) = self.character(content.models) else {
                self.clear_player_meshes();
                return;
            };
            let look = HeadLook {
                yaw: anim.head_offset(),
                pitch: player.pitch,
            };
            character.pose(anim, look).map(|pose| {
                (
                    character.bake(&pose, render_position, body_yaw),
                    character.hand_anchor(&pose, render_position, body_yaw),
                )
            })
        };
        let Some((mesh, anchor)) = baked else {
            self.clear_player_meshes();
            return;
        };
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();

        let held = inventory.selected_stack().map(|stack| stack.item);
        self.held_mesh = anchor.and_then(|a| self.bake_held(ctx, content, held, a));
        self.held_atlas = anchor.and_then(|a| self.bake_held_atlas(ctx, content, held, a));
    }

    /// Build the held item for something with **no model file**: the same cube
    /// or sprite a dropped stack of it would be, placed by `transform`.
    ///
    /// This is what keeps a block from being invisible in the hand. The geometry
    /// is built in `0..1` model space — the space a Blockbench export occupies —
    /// so the caller's placement matrix positions it by exactly the path an
    /// authored model takes.
    ///
    /// `normal_basis` is passed separately because the two hands want different
    /// answers: in third person the item really is out in the world and should
    /// light like the body carrying it, while in first person the transform
    /// carries the camera's own rotation and lighting taken from it would pulse
    /// as the player turns. See [`CpuMesh::transformed`].
    fn shaped_item_mesh(
        &mut self,
        ctx: &Arc<RenderContext>,
        shape: ItemShape,
        tiles: &TileRegistry,
        transform: glam::Mat4,
        normal_basis: glam::Mat4,
    ) -> Option<GpuMesh> {
        let mut mesh = CpuMesh::new();
        match shape {
            ItemShape::Cube(faces) => {
                push_item_cube(&mut mesh, Vec3::splat(0.5), 1.0, 0.0, &faces);
            }
            ItemShape::Sprite(tile) => {
                let sprite = self
                    .item_sprites
                    .entry(tile)
                    .or_insert_with(|| ItemSprite::new(tile, tiles.art(tile)));
                push_item_sprite(&mut mesh, sprite, Vec3::splat(0.5), 1.0, 0.0);
            }
        }
        let placed = mesh.transformed(transform, normal_basis);
        GpuMesh::upload(&ctx.memory_allocator, &placed)
            .ok()
            .flatten()
    }

    /// The third-person counterpart of [`SceneCache::bake_held`], for an
    /// item with no model. Returns the mesh and whether it belongs in the
    /// blended pass, so a held glass block reads like the block it places.
    fn bake_held_atlas(
        &mut self,
        ctx: &Arc<RenderContext>,
        content: ModelContent<'_>,
        item: Option<ItemId>,
        anchor: Mat4,
    ) -> Option<(GpuMesh, bool)> {
        let item = item?;
        // A model always wins; this is only the fallback for items without one.
        if content.of(item).is_some() {
            return None;
        }
        let (shape, is_transparent) = (content.shape)(item);
        let local = content.atlas_local(item, shape, DisplayContext::ThirdPersonRightHand);
        let transform = anchor * local;
        let mesh = self.shaped_item_mesh(ctx, shape, content.tiles, transform, transform)?;
        Some((mesh, is_transparent))
    }

    /// Bake the model of the item in `anchor`'s hand, if it has one.
    fn bake_held(
        &mut self,
        ctx: &Arc<RenderContext>,
        content: ModelContent<'_>,
        item: Option<ItemId>,
        anchor: Mat4,
    ) -> Option<(GpuMesh, ModelId)> {
        let item = item?;
        let held = content.of(item)?;
        let local = content.local(item, held, DisplayContext::ThirdPersonRightHand);
        self.bake_model(ctx, content.models, held.id, anchor * local)
    }

    /// Rebuild the first-person view model: the player's own arm, and whatever
    /// it holds.
    ///
    /// The arm always draws; the item only when it has a model of its own, the
    /// same rule third person follows. Both hang off one [`HandPose::frame`], so
    /// the item cannot drift out of the fist.
    fn update_hand_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        player: &Player,
        anim: &AnimationState,
        inventory: &Inventory,
        content: ModelContent<'_>,
    ) {
        let pose = HandPose {
            eye: player.interpolated_eye_position(self.render_alpha),
            yaw: player.yaw,
            pitch: player.pitch,
            swing: anim.swing_progress(),
            walk_phase: anim.walk_phase(),
            walk_amount: anim.walk_amount(),
        };
        let frame = pose.frame();

        // The arm is posed entirely by `frame` — bob and swing — and takes
        // nothing from the body's clips or its head look. See `arm_mesh`.
        let arm = self
            .character(content.models)
            .map(|character| viewmodel::arm_mesh(&character, frame));
        self.hand_mesh =
            arm.and_then(|arm| GpuMesh::upload(&ctx.memory_allocator, &arm).ok().flatten());

        let selected = inventory.selected_stack().map(|stack| stack.item);
        let held = content.held(inventory);
        self.hand_held_mesh = held.zip(selected).and_then(|(held, item)| {
            let local = content.local(item, held, DisplayContext::FirstPersonRightHand);
            let transform = viewmodel::item_anchor(frame) * local;
            self.bake_model(ctx, content.models, held.id, transform)
        });

        // No model file: draw the cube or sprite the ground would draw, rather
        // than an empty fist. Keyed off `held` rather than off the mesh above,
        // so a model that merely failed to upload does not fall back to the
        // magenta placeholder `item_shape` returns for model-backed items.
        self.hand_held_atlas = match (held, inventory.selected_stack()) {
            (None, Some(stack)) => {
                let (shape, _) = (content.shape)(stack.item);
                let local =
                    content.atlas_local(stack.item, shape, DisplayContext::FirstPersonRightHand);
                let transform = viewmodel::item_anchor(frame) * local;
                // Lit by the placement alone: `transform` carries the camera's
                // rotation, and using it would make the block pulse as you spin.
                self.shaped_item_mesh(ctx, shape, content.tiles, transform, local)
            }
            _ => None,
        };
    }

    /// Rebuild GPU meshes for remote players, advancing each one's animation
    /// from the movement observed since the previous frame.
    pub fn update_remote_meshes(
        &mut self,
        ctx: &Arc<RenderContext>,
        peers: impl IntoIterator<Item = PeerSprite>,
        content: ModelContent<'_>,
    ) {
        self.remote_meshes.clear();
        self.remote_held.clear();
        self.remote_held_atlas.clear();
        let snapshots: Vec<PeerSprite> = peers.into_iter().collect();
        let mut baked: Vec<CpuMesh> = Vec::with_capacity(snapshots.len());
        // The fist and what is in it, gathered here and baked below: `character`
        // borrows `self` for as long as the pose does, and baking an item needs
        // `self` mutably for the sprite cache.
        let mut hands: Vec<(Mat4, Option<ItemId>)> = Vec::with_capacity(snapshots.len());
        for PeerSprite {
            position: pos,
            pitch,
            anim,
            held,
        } in snapshots
        {
            let Some(character) = self.character(content.models) else {
                break;
            };
            let look = HeadLook {
                yaw: anim.head_offset(),
                pitch,
            };
            // Derived here rather than sent: only the look yaw crosses the wire,
            // and a torso that follows it is cosmetic, so every peer can work it
            // out for itself.
            if let Some(pose) = character.pose(&anim, look) {
                let body_yaw = anim.body_yaw();
                baked.push(character.bake(&pose, pos, body_yaw));
                // The same pose and yaw the body was baked at, so a peer's item
                // rides its fist exactly the way the local player's does.
                if let Some(anchor) = character.hand_anchor(&pose, pos, body_yaw) {
                    hands.push((anchor, held));
                }
            }
        }
        for mesh in baked {
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.remote_meshes.push(gpu);
            }
        }
        for (anchor, held) in hands {
            if let Some(mesh) = self.bake_held(ctx, content, held, anchor) {
                self.remote_held.push(mesh);
            }
            if let Some(mesh) = self.bake_held_atlas(ctx, content, held, anchor) {
                self.remote_held_atlas.push(mesh);
            }
        }
    }
}

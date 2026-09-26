//! The one per-frame seam between simulation and GPU.

use super::*;

impl super::super::InGameState {
    /// Bring every GPU resource in line with the simulation state this frame.
    ///
    /// This is the single seam where rendering meets simulation: it is the only
    /// method in the in-game state that touches a [`RenderContext`]. Everything
    /// above it — streaming, mobs, fluids, interaction — is plain logic that
    /// runs without a GPU, which is what makes it testable.
    pub(in super::super) fn refresh_view(&mut self, ctx: &Arc<RenderContext>) {
        // Overlays on the block under the crosshair. Both are drawn around the
        // block's targeting box, so cracks and outline hug a mushroom the same
        // way the crosshair does.
        let breaking = self
            .sim
            .breaking
            .as_ref()
            .map(|b| (b.block, self.hitbox_at(b.block), b.progress));
        self.view.update_break_overlay(ctx, breaking);
        let target = if self.sim.dead {
            None
        } else {
            self.targeted_block()
                .map(|hit| (hit.block, self.hitbox_at(hit.block)))
        };
        self.view.update_target_outline(ctx, target);

        // Chunk meshes: queue what the world dirtied, then spend the budget.
        let dirty = self.sim.world.take_dirty();
        self.view.enqueue_dirty(dirty);
        self.view.process_mesh_budget(
            ctx,
            &self.sim.world,
            BlockAppearance {
                blocks: &self.content.rules.blocks,
                face_tiles: &self.content.visuals.block_face_tiles,
                models: &self.content.visuals.models,
                placed: &self.content.visuals.block_models,
                baked: &self.content.visuals.baked_models,
                fluids: &self.content.visuals.fluid_textures,
            },
            super::super::MESH_BUDGET,
        );

        // Loose entities. One `Arc` clone releases the borrow on `self` for
        // the closure below, where three deep `Vec` clones used to.
        let loaded = self.content.clone();
        let (items, blocks) = (&loaded.rules.items, &loaded.rules.blocks);
        let models = &loaded.visuals.models;
        // What shape an item is, and which pass it belongs in. Shared by the
        // drops and by every hand that has to fall back to a cube or a sprite.
        let shape = |item| {
            let is_transparent = items
                .get(item)
                .get::<Placeable>()
                .is_some_and(|p| blocks.get(p.block).is_transparent());
            (loaded.item_shape(item), is_transparent)
        };
        let content = ModelContent {
            models,
            item_models: &loaded.visuals.item_models,
            tiles: &loaded.visuals.tiles,
            shape: &shape,
            placement: &self.editor,
            block_display: &loaded.visuals.block_item_display,
        };
        // The editor's ground preview, when it has one, rides along with the
        // real drops so it is drawn by exactly the same code.
        let preview = self.editor_ground_preview();
        let drops = self.sim.ecs.query::<(&ItemDrop, &Transform, &Body)>().map(
            |(_, (drop, transform, body))| DropSprite {
                item: drop.stack.item,
                center: drop.render_center(transform.position, &body.physics),
                size: drop.render_size(&body.physics),
                yaw: drop.spin_yaw(),
            },
        );
        self.view
            .update_drops_mesh(ctx, drops.chain(preview), content);
        // Simulated and replicated mobs alike: a kind, a place and an
        // animation. Replicas were animated in `update`, so this only reads.
        let mobs = self
            .sim
            .ecs
            .query::<(&Kind, &Transform, &Animation, &MobId)>()
            .map(|(_, (kind, transform, anim, _))| MobSprite {
                visual: &kind.visual,
                position: transform.position,
                yaw: anim.0.body_yaw(),
                pose: anim.0.pose(0.0),
            });
        self.view.update_mob_meshes(ctx, mobs, models);
        let arrows = self
            .sim
            .ecs
            .query::<(&Projectile, &Transform, &Velocity)>()
            .map(|(_, (_, transform, velocity))| ArrowSprite {
                position: transform.position,
                yaw: Projectile::yaw(velocity.0),
                size: Projectile::SIZE,
            });
        self.view
            .update_arrows_mesh(ctx, arrows, loaded.visuals.arrow_faces);

        self.view.update_player_mesh(
            ctx,
            &self.sim.player,
            &self.sim.player_anim,
            &self.sim.inventory,
            self.inventory_anim.progress(),
            content,
        );
        // Cheap after the first call, and this is the only place the entity
        // registry and the model registry are both in reach.
        self.view.bind_player_rig(&loaded.rules.entities, models);
        let alpha = self.view.render_alpha;
        let peers = self
            .sim
            .ecs
            .query::<(&RemotePlayer, &Animation)>()
            .map(|(_, (rp, anim))| PeerSprite {
                position: rp.interpolated_position(alpha),
                pitch: rp.pitch,
                anim: anim.0,
                held: rp.equipment.held.map(ItemId),
            });
        self.view.update_remote_meshes(ctx, peers, content);
    }
}

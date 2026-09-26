//! The one per-frame seam between simulation and GPU.

use super::*;
use crate::application::ecs::Ecs;

impl super::super::InGameState {
    /// Bring every GPU resource in line with the simulation state this frame.
    ///
    /// This is the single seam where rendering meets simulation: it is the only
    /// method in the in-game state that touches a [`RenderContext`]. Everything
    /// above it — streaming, mobs, fluids, interaction — is plain logic that
    /// runs without a GPU, which is what makes it testable.
    pub(in super::super) fn refresh_view(&mut self, ctx: &Arc<RenderContext>) {
        self.refresh_overlays(ctx);
        self.refresh_chunks(ctx);
        self.refresh_entities(ctx);
    }

    /// Overlays on the block under the crosshair. Both are drawn around the
    /// block's targeting box, so cracks and outline hug a mushroom the same
    /// way the crosshair does.
    fn refresh_overlays(&mut self, ctx: &Arc<RenderContext>) {
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
    }

    /// Chunk meshes: queue what the world dirtied, then spend the budget.
    fn refresh_chunks(&mut self, ctx: &Arc<RenderContext>) {
        let dirty = self.sim.world.take_dirty();
        self.view.enqueue_dirty(dirty);
        self.view.process_mesh_budget(
            ctx,
            &self.sim.world,
            self.content.appearance(),
            super::super::MESH_BUDGET,
        );
    }

    /// Every entity mesh: drops, mobs, arrows, the local player and everyone
    /// else. The view is handed plain values, never the ECS.
    fn refresh_entities(&mut self, ctx: &Arc<RenderContext>) {
        // One `Arc` clone releases the borrow on `self` for the closure below,
        // where three deep `Vec` clones used to.
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
        self.view
            .update_drops_mesh(ctx, drop_sprites(&self.sim.ecs).chain(preview), content);
        self.view
            .update_mob_meshes(ctx, mob_sprites(&self.sim.ecs), models);
        self.view.update_arrows_mesh(
            ctx,
            arrow_sprites(&self.sim.ecs),
            loaded.visuals.arrow_faces,
        );
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
        let peers = peer_sprites(&self.sim.ecs, self.view.render_alpha);
        self.view.update_remote_meshes(ctx, peers, content);
    }
}

/// Every dropped item, where it is drawn.
fn drop_sprites(ecs: &Ecs) -> impl Iterator<Item = DropSprite> + '_ {
    ecs.query::<(&ItemDrop, &Transform, &Body)>()
        .map(|(_, (drop, transform, body))| DropSprite {
            item: drop.stack.item,
            center: drop.render_center(transform.position, &body.physics),
            size: drop.render_size(&body.physics),
            yaw: drop.spin_yaw(),
        })
}

/// Simulated and replicated mobs alike: a kind, a place and an animation.
/// Replicas were animated in `update`, so this only reads.
fn mob_sprites(ecs: &Ecs) -> impl Iterator<Item = MobSprite<'_>> {
    ecs.query::<(&Kind, &Transform, &Animation, &MobId)>()
        .map(|(_, (kind, transform, anim, _))| MobSprite {
            visual: &kind.visual,
            position: transform.position,
            yaw: anim.0.body_yaw(),
            pose: anim.0.pose(0.0),
        })
}

/// Every arrow in flight, pointing the way it flies.
fn arrow_sprites(ecs: &Ecs) -> impl Iterator<Item = ArrowSprite> + '_ {
    ecs.query::<(&Projectile, &Transform, &Velocity)>()
        .map(|(_, (_, transform, velocity))| ArrowSprite {
            position: transform.position,
            yaw: Projectile::yaw(velocity.0),
            size: Projectile::SIZE,
        })
}

/// Every other player, at the interpolated position their nameplate follows.
fn peer_sprites(ecs: &Ecs, alpha: f32) -> impl Iterator<Item = PeerSprite> + '_ {
    ecs.query::<(&RemotePlayer, &Animation)>()
        .map(move |(_, (rp, anim))| PeerSprite {
            position: rp.interpolated_position(alpha),
            pitch: rp.pitch,
            anim: anim.0,
            held: rp.equipment.held.map(ItemId),
        })
}

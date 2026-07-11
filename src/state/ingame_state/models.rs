//! Animated humanoid model meshes: the local player (third person), the
//! inventory preview, and every remote player.

use std::sync::Arc;

use glam::Vec3;

use super::{InGameState, REMOTE_MAX_SPEED, RemoteAnim};
use crate::entity::AnimationState;
use crate::inventory::{ARMOR_SIZE, ItemId, ItemRegistry};
use crate::net::PlayerId;
use crate::render::{GpuMesh, RenderContext};

impl InGameState {
    /// Rebuild the player model mesh in third person; drop it in first person.
    pub(super) fn update_player_mesh(&mut self, ctx: &Arc<RenderContext>) {
        if self.player.perspective.is_first_person() {
            self.player_mesh = None;
            return;
        }
        let pose = self.player_anim.pose(self.player.pitch);
        let armor = self.inventory.equipped_armor();
        let mesh = self.player_model.build_mesh_armored(
            self.player.position,
            self.player.yaw,
            &pose,
            &armor,
            &self.items,
        );
        self.player_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    /// Rebuild the inventory-preview player mesh: the model at the origin, turned
    /// to `preview_yaw`, wearing the currently equipped armor. Only built while
    /// the inventory is open; cleared otherwise so the offscreen pass is skipped.
    pub(super) fn update_preview_mesh(&mut self, ctx: &Arc<RenderContext>) {
        if !self.inventory_open {
            self.preview_mesh = None;
            return;
        }
        // The preview head tracks the cursor (yaw + pitch), independent of the
        // world player's facing; limbs still idle-animate from the anim state.
        let mut pose = self.player_anim.pose(self.player.pitch);
        pose.head_yaw = self.preview_look.0;
        pose.head_pitch = self.preview_look.1;
        let armor = self.inventory.equipped_armor();
        let mesh = self.player_model.build_mesh_armored(
            Vec3::ZERO,
            self.preview_yaw,
            &pose,
            &armor,
            &self.items,
        );
        self.preview_mesh = GpuMesh::upload(&ctx.memory_allocator, &mesh).ok().flatten();
    }

    /// Rebuild GPU meshes for remote players, advancing each one's animation from the
    /// movement observed since the previous frame.
    pub(super) fn update_remote_meshes(&mut self, ctx: &Arc<RenderContext>, dt: f32) {
        self.remote_meshes.clear();
        // Snapshot the render-relevant fields first so we can mutate `remote_anims`
        // and read `player_model` without holding a borrow on `remote_players`.
        // (id, position, yaw, pitch, armor item ids)
        type Snapshot = (PlayerId, Vec3, f32, f32, [Option<u16>; ARMOR_SIZE]);
        let snapshots: Vec<Snapshot> = self
            .remote_players
            .values()
            .map(|rp| (rp.id, rp.position(), rp.yaw, rp.pitch, rp.armor))
            .collect();
        for (id, pos, yaw, pitch, armor_ids) in snapshots {
            let state = self.remote_anims.entry(id).or_insert_with(|| RemoteAnim {
                anim: AnimationState::new(),
                last_pos: pos,
            });
            let delta = pos - state.last_pos;
            let speed =
                (Vec3::new(delta.x, 0.0, delta.z).length() / dt.max(1e-4)).min(REMOTE_MAX_SPEED);
            state.anim.advance(speed, dt);
            state.last_pos = pos;
            let pose = state.anim.pose(pitch);

            let armor = armor_item_ids(armor_ids, &self.items);
            let mesh = self
                .player_model
                .build_mesh_armored(pos, yaw, &pose, &armor, &self.items);
            if let Ok(Some(gpu)) = GpuMesh::upload(&ctx.memory_allocator, &mesh) {
                self.remote_meshes.push(gpu);
            }
        }
        // Drop animation state for players that have left.
        self.remote_anims
            .retain(|id, _| self.remote_players.contains_key(id));
    }
}

/// Resolve wire armor ids to `ItemId`s the local registry knows, dropping any
/// out-of-range id (a peer running divergent content is already refused, but be
/// safe: raw ids index the registry directly).
fn armor_item_ids(
    ids: [Option<u16>; ARMOR_SIZE],
    items: &ItemRegistry,
) -> [Option<ItemId>; ARMOR_SIZE] {
    ids.map(|id| id.filter(|i| (*i as usize) < items.len()).map(ItemId))
}

//! Block interaction and world entities the player creates: raycast targeting,
//! breaking/placing, survival mining progress, dropped items, and the crack /
//! selection overlays.

use glam::Vec3;

use super::{BreakState, InGameState};
use crate::art::tiles;
use crate::core::{Aabb, BlockId, BlockPos};
use crate::entity::DroppedItem;
use crate::inventory::{ItemId, ItemRegistry, ItemStack};
use crate::world::Target;
use crate::world::block::Drops;
use wyven_voxel::FaceTextures;

impl InGameState {
    /// What the crosshair would hit at `pos`: the whole cell for an ordinary
    /// block, the model's own smaller box for ground cover, nothing for air and
    /// fluids. Also what the selection outline and crack overlay are drawn
    /// around, so all three always agree.
    pub(super) fn target_at(&self, pos: BlockPos) -> Option<Target> {
        if !self.world.is_targetable(pos) {
            return None;
        }
        let block = self.world.block_at(pos);
        // Both model paths measure a hitbox from their own geometry, so a
        // Blockbench-authored flower is no more targetable than a `.bbmodel`
        // one. A block with neither is an ordinary cube and fills its cell.
        let hitbox = self
            .content
            .baked_models
            .get(block.0 as usize)
            .and_then(|m| m.as_ref().and_then(|m| m.hitbox))
            .or_else(|| {
                self.content
                    .block_models
                    .get(block.0 as usize)
                    .and_then(|m| m.map(|m| m.hitbox))
            });
        match hitbox {
            Some(hitbox) => Some(Target::Box(hitbox.translate(Vec3::new(
                pos.x as f32,
                pos.y as f32,
                pos.z as f32,
            )))),
            None => Some(Target::Cell),
        }
    }

    /// The world-space box of whatever occupies `pos` — the block's own model
    /// hitbox, or its full cell. Used to draw the overlays that must line up
    /// with what [`Self::target_at`] lets the crosshair hit.
    pub(super) fn hitbox_at(&self, pos: BlockPos) -> Aabb {
        let corner = Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32);
        match self.target_at(pos) {
            Some(Target::Box(box_)) => box_,
            _ => Aabb::block(corner),
        }
    }

    /// The block the player is currently looking at within reach, if any.
    pub(super) fn targeted_block(&self) -> Option<crate::world::RaycastHit> {
        crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            self.player.movement().reach,
            |p| self.target_at(p),
        )
    }

    /// Remove the block at `pos`. In survival the block pops out as a dropped
    /// item; in creative it just disappears. Broadcasts the edit. Returns `true`
    /// on a hit.
    pub(super) fn break_block_at(&mut self, pos: BlockPos) -> bool {
        let Some(prev) = self.world.set_block(pos, BlockId::AIR) else {
            return false;
        };
        if prev.is_air() {
            return false;
        }
        self.fluids.block_changed(pos);
        if self.player.mode.consumes_blocks()
            && let Some(stack) = self.block_drop_stack(prev)
        {
            // Scatter direction varies with the animation clock — cheap pseudo-random.
            let angle = self.view.elapsed * 9.73;
            self.drops.push(DroppedItem::block_drop(
                stack,
                pos,
                angle,
                self.content.entities.dropped_item(),
            ));
        }
        self.broadcast_local_edit(pos, BlockId::AIR);
        true
    }

    /// What breaking a block of type `block` yields, per its `drops` component
    /// (`assets/blocks.toml`) and the held tool. `None` when nothing drops.
    fn block_drop_stack(&self, block: BlockId) -> Option<ItemStack> {
        let self_item = || {
            self.content
                .items
                .item_for_block(block)
                .map(ItemStack::single)
        };
        match &self.content.blocks.get(block).drops {
            Drops::SelfItem => self_item(),
            Drops::None => None,
            Drops::SelfWithTool { kind } => {
                let held = self
                    .inventory
                    .item_in_selected()
                    .and_then(|id| self.content.items.tool(id));
                (held.is_some_and(|tool| tool.kind == *kind)).then(self_item)?
            }
            Drops::Item { name, count } => {
                let id = self.content.items.find(name)?;
                Some(ItemStack::new(
                    id,
                    (*count).clamp(1, self.content.items.max_stack(id)),
                ))
            }
        }
    }

    /// Toss one item from the selected hotbar slot out in front of the player.
    pub(super) fn drop_selected_item(&mut self) {
        let Some(stack) = self.inventory.take_one_selected() else {
            return;
        };
        self.drops.push(DroppedItem::thrown(
            stack,
            self.player.eye_position(),
            self.player.look_direction(),
            self.content.entities.dropped_item(),
        ));
    }

    /// Advance drop physics, collect drops the player walks over, cull expired ones.
    pub(super) fn update_drops(&mut self, dt: f32) {
        for item in &mut self.drops {
            item.update(dt, |p| self.world.is_solid_for_collision(p));
        }
        let player_aabb = self.player.aabb();
        let dead = self.dead;
        self.drops.retain_mut(|item| {
            if item.expired() {
                return false;
            }
            let reach = player_aabb.expand(Vec3::splat(item.pickup_range()));
            if dead || !item.can_pickup() || !reach.intersects(item.aabb()) {
                return true;
            }
            let leftover = self.inventory.add(item.stack, &self.content.items);
            if leftover == 0 {
                false
            } else {
                // Inventory full: whatever didn't fit stays on the ground.
                item.stack.count = leftover;
                true
            }
        });
    }

    /// Survival timed mining: accumulate break progress on the targeted block
    /// while the dig button is held, breaking it once progress reaches 1.0.
    pub(super) fn update_mining(&mut self, digging: bool, dt: f32) {
        if !digging {
            self.breaking = None;
            return;
        }
        let Some(hit) = self.targeted_block() else {
            self.breaking = None;
            return;
        };
        let block = self.content.blocks.get(self.world.block_at(hit.block));
        if !block.is_breakable() {
            self.breaking = None;
            return;
        }
        // Effective tool: the held item, if it's a tool.
        let tool = self
            .inventory
            .item_in_selected()
            .and_then(|id| self.content.items.tool(id));
        let seconds = crate::inventory::break_seconds(block.hardness, block.material, tool);

        // Reset progress when the targeted block changes.
        let prior = match &self.breaking {
            Some(b) if b.block == hit.block => b.progress,
            _ => 0.0,
        };
        let progress = prior + dt / seconds.max(1.0e-3);
        if progress >= 1.0 {
            self.view.trigger_swing();
            if self.break_block_at(hit.block) {
                self.inventory.damage_selected_tool();
            }
            self.breaking = None;
        } else {
            self.breaking = Some(BreakState {
                block: hit.block,
                progress,
            });
        }
    }

    /// Right-click: eat the held food when hungry, otherwise place its block.
    pub(super) fn use_selected(&mut self) {
        let Some(item_id) = self.inventory.item_in_selected() else {
            return;
        };
        if let Some(food) = self.content.items.food(item_id)
            && self.player.mode.takes_damage()
            && self.player.is_hungry()
        {
            self.player.feed(food.hunger, food.saturation);
            self.inventory.consume_selected(1);
            self.view.trigger_swing();
            return;
        }
        self.place_block(item_id);
    }

    /// Place the selected item's block against the targeted face. Consumes from
    /// the inventory only in survival (creative has infinite blocks).
    fn place_block(&mut self, item_id: ItemId) {
        let Some(block) = self.content.items.get(item_id).place_block else {
            return;
        };
        let Some(hit) = self.targeted_block() else {
            return;
        };
        // Ground cover is swallowed rather than stacked on: without this,
        // building next to a flower would leave blocks perched on top of it.
        let target = if self.world.is_replaceable(hit.block) {
            hit.block
        } else {
            hit.place_position()
        };
        // Don't place inside the player.
        if Aabb::block(Vec3::new(target.x as f32, target.y as f32, target.z as f32))
            .intersects(self.player.aabb())
        {
            return;
        }
        if self.world.set_block(target, block).is_some() {
            self.fluids.block_changed(target);
            if self.player.mode.consumes_blocks() {
                self.inventory.consume_selected(1);
            }
            self.broadcast_local_edit(target, block);
            self.view.trigger_swing();
        }
    }
}

/// Atlas tiles for a dropped item's cube: the block's own faces for block
/// items; simple stand-in tiles for tools and food (no dedicated item art yet).
///
/// `block_faces` is `content::GameContent::block_face_tiles` — the tiles derived
/// from a Blockbench-authored block's own 256-pixel textures, which is what a
/// dropped stack of one is drawn with. Anything not authored that way falls back
/// to the tiles `blocks.toml` named.
pub(super) fn drop_textures(
    item: ItemId,
    items: &ItemRegistry,
    block_faces: &[Option<FaceTextures>],
) -> FaceTextures {
    let def = items.get(item);
    match def.place_block {
        Some(block) => block_faces
            .get(block.0 as usize)
            .copied()
            .flatten()
            .unwrap_or(crate::content::MISSING_FACES),
        None if def.tool.is_some() => FaceTextures::uniform(tiles::WOOD_BARK),
        None => FaceTextures::uniform(tiles::LEAVES),
    }
}

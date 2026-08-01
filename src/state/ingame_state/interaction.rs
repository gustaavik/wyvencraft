//! Block interaction and world entities the player creates: raycast targeting,
//! breaking/placing, survival mining progress, dropped items, and the crack /
//! selection overlays.

use glam::Vec3;

use super::{BreakState, InGameState};
use crate::core::{Aabb, BlockId, BlockPos};
use crate::entity::DroppedItem;
use crate::inventory::{ItemId, ItemRegistry, ItemStack};
use crate::render::tiles;
use crate::world::block::{BlockRegistry, Drops, FaceTextures};

impl InGameState {
    /// The block the player is currently looking at within reach, if any.
    pub(super) fn targeted_block(&self) -> Option<crate::world::RaycastHit> {
        crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            self.player.movement().reach,
            |p| self.world.is_solid(p),
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
                self.entities.dropped_item(),
            ));
        }
        self.broadcast_local_edit(pos, BlockId::AIR);
        true
    }

    /// What breaking a block of type `block` yields, per its `drops` component
    /// (`assets/blocks.toml`) and the held tool. `None` when nothing drops.
    fn block_drop_stack(&self, block: BlockId) -> Option<ItemStack> {
        let self_item = || self.items.item_for_block(block).map(ItemStack::single);
        match &self.blocks.get(block).drops {
            Drops::SelfItem => self_item(),
            Drops::None => None,
            Drops::SelfWithTool { kind } => {
                let held = self
                    .inventory
                    .item_in_selected()
                    .and_then(|id| self.items.tool(id));
                (held.is_some_and(|tool| tool.kind == *kind)).then(self_item)?
            }
            Drops::Item { name, count } => {
                let id = self.items.find(name)?;
                Some(ItemStack::new(
                    id,
                    (*count).clamp(1, self.items.max_stack(id)),
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
            self.entities.dropped_item(),
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
            let leftover = self.inventory.add(item.stack, &self.items);
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
        let block = self.blocks.get(self.world.block_at(hit.block));
        if !block.is_breakable() {
            self.breaking = None;
            return;
        }
        // Effective tool: the held item, if it's a tool.
        let tool = self
            .inventory
            .item_in_selected()
            .and_then(|id| self.items.tool(id));
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
        if let Some(food) = self.items.food(item_id)
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
        let Some(block) = self.items.get(item_id).place_block else {
            return;
        };
        let Some(hit) = self.targeted_block() else {
            return;
        };
        let target = hit.place_position();
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
pub(super) fn drop_textures(
    item: ItemId,
    items: &ItemRegistry,
    blocks: &BlockRegistry,
) -> FaceTextures {
    let def = items.get(item);
    match def.place_block {
        Some(block) => blocks.get(block).textures,
        None if def.tool.is_some() => FaceTextures::uniform(tiles::WOOD_BARK),
        None => FaceTextures::uniform(tiles::LEAVES),
    }
}

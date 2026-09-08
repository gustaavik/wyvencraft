//! Block interaction and world entities the player creates: raycast targeting,
//! breaking/placing, survival mining progress, dropped items, and the crack /
//! selection overlays.

use glam::Vec3;

use super::{BreakState, InGameState};
use crate::core::{Aabb, BlockId, BlockPos};
use crate::entity::DroppedItem;
use crate::inventory::{Consumable, ItemId, ItemStack, Placeable, Tool};
use crate::world::Target;
use crate::world::block::Drops;

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
            Drops::RequiresCapability { capability } => {
                // What the held item can *do*, not what it is called.
                let capable = self
                    .inventory
                    .item_in_selected()
                    .is_some_and(|id| self.content.items.has(id, capability));
                capable.then(self_item)?
            }
            Drops::Item { id: drop, count } => {
                let id = self.content.items.find(drop)?;
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
        self.throw(stack);
    }

    /// Throw `stack` out in front of the player.
    ///
    /// The one place that spells this, so every route an item takes out of the
    /// inventory — the drop key, and a held stack that no longer fits when the
    /// panel closes — lands the same way.
    pub(super) fn throw(&mut self, stack: ItemStack) {
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
        // Mining is a run of blows, so the arm swings for as long as the button
        // is down rather than once when it went down. Keeping the swing alive
        // (rather than triggering it) is what lets each arc finish before the
        // next begins; the blow that finally breaks the block is covered by the
        // same loop, so it needs no trigger of its own.
        self.view.keep_swinging();
        // Effective tool: the held item, if it's a tool.
        let tool = self
            .inventory
            .item_in_selected()
            .and_then(|id| self.content.items.component::<Tool>(id));
        let seconds = crate::inventory::break_seconds(block.hardness, block.material, tool);

        // Reset progress when the targeted block changes.
        let prior = match &self.breaking {
            Some(b) if b.block == hit.block => b.progress,
            _ => 0.0,
        };
        let progress = prior + dt / seconds.max(1.0e-3);
        if progress >= 1.0 {
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

    /// Right-click: offer the held item to each hook in turn, first one that
    /// handles it wins.
    pub(super) fn use_selected(&mut self) {
        let Some(item_id) = self.inventory.item_in_selected() else {
            return;
        };
        for hook in USE_HOOKS {
            if hook(self, item_id) {
                return;
            }
        }
    }

    /// Eat the held item, if it is edible and the player has room for it.
    fn try_consume(&mut self, item_id: ItemId) -> bool {
        let Some(&Consumable { hunger, saturation }) =
            self.content.items.component::<Consumable>(item_id)
        else {
            return false;
        };
        if !self.player.mode.takes_damage() || !self.player.is_hungry() {
            return false;
        }
        self.player.feed(hunger, saturation);
        self.inventory.consume_selected(1);
        self.view.trigger_swing();
        true
    }

    /// Place the selected item's block against the targeted face. Consumes from
    /// the inventory only in survival (creative has infinite blocks).
    fn try_place(&mut self, item_id: ItemId) -> bool {
        let Some(&Placeable { block }) = self.content.items.component::<Placeable>(item_id) else {
            return false;
        };
        // Every exit below reports the click handled: holding a block *is* a
        // placement attempt, and a hook after this one must not get a second
        // go at it just because the ray missed.
        let Some(hit) = self.targeted_block() else {
            return true;
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
            return true;
        }
        if self.world.set_block(target, block).is_some() {
            self.fluids.block_changed(target);
            if self.player.mode.consumes_blocks() {
                self.inventory.consume_selected(1);
            }
            self.broadcast_local_edit(target, block);
            self.view.trigger_swing();
        }
        true
    }
}

/// What right-clicking with an item can do, in priority order: the first hook
/// that reports it handled the click wins.
///
/// The *code* half of the capability system. What an item declares is open (see
/// `inventory::component::COMPONENTS`); what the game does about it is code, so
/// a new usable capability is one function above and one line here — and the
/// precedence between them is stated in one place instead of being implied by
/// the order of an `if` chain.
const USE_HOOKS: &[fn(&mut InGameState, ItemId) -> bool] =
    &[InGameState::try_consume, InGameState::try_place];

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;

    /// Put `item` in the selected hotbar slot of a fresh survival session.
    fn holding(item: &str) -> InGameState {
        let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        let id = state.content.items.find(item).expect("shipped item");
        let stack = state.content.items.full_stack(id);
        state.inventory.set_selected(0);
        state.inventory.set_slot(0, Some(stack));
        state
    }

    /// The first hook that claims the click wins, and eating claims it only
    /// when the player can actually eat.
    #[test]
    fn right_clicking_food_eats_it_when_hungry() {
        let mut state = holding("bread");
        let before = state.inventory.slot(0).expect("bread").count;
        state.player.hunger = 1.0;
        assert!(
            state.player.is_hungry(),
            "the fixture must leave room to eat"
        );

        state.use_selected();

        assert!(state.player.hunger > 1.0, "eating restores hunger");
        assert_eq!(
            state.inventory.slot(0).expect("bread").count,
            before - 1,
            "one loaf is consumed"
        );
    }

    /// A full player's right-click falls *through* the eating hook rather than
    /// being swallowed by it — which is what lets an item that both feeds and
    /// places still place when the player is full.
    #[test]
    fn right_clicking_food_while_full_consumes_nothing() {
        let mut state = holding("bread");
        let before = state.inventory.slot(0).expect("bread").count;
        assert!(!state.player.is_hungry(), "a fresh player is fed");

        state.use_selected();

        assert_eq!(
            state.inventory.slot(0).expect("bread").count,
            before,
            "a full player eats nothing"
        );
    }

    /// The placing hook, end to end: a block item puts its block in the world
    /// and spends one from the stack.
    #[test]
    fn right_clicking_a_block_item_places_its_block() {
        let mut state = holding("cobblestone");
        let before = state.inventory.slot(0).expect("cobblestone").count;
        let cobblestone = state
            .content
            .blocks
            .find("cobblestone")
            .expect("shipped block");
        // Aim at a solid block two ahead, so the ray has a face to build on.
        let look = state.player.look_direction();
        let at = BlockPos::from_world(state.player.eye_position() + look * 2.0);
        state.world.set_block(at, cobblestone);
        let hit = state.targeted_block().expect("a face to place against");

        state.use_selected();

        assert_eq!(
            state.world.block_at(hit.place_position()),
            cobblestone,
            "the block lands on the targeted face"
        );
        assert_eq!(
            state.inventory.slot(0).expect("cobblestone").count,
            before - 1,
            "survival spends one"
        );
    }

    /// An item carrying neither capability is inert on right-click: no hook
    /// claims it, and nothing in the world or the inventory moves.
    #[test]
    fn right_clicking_an_item_with_no_usable_capability_does_nothing() {
        let mut state = holding("stick");
        let before = state.inventory.slot(0).expect("stick").count;

        state.use_selected();

        assert_eq!(state.inventory.slot(0).expect("stick").count, before);
    }
}

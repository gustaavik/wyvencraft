//! Inventory-screen interactions: open/close, click-to-move between slots, and
//! crafting from the recipe list.

use super::InGameState;
use crate::entity::DroppedItem;
use crate::inventory::ItemStack;

impl InGameState {
    /// Open/close the inventory screen; returns a held stack to storage on close.
    pub(super) fn toggle_inventory(&mut self) {
        self.inventory_open = !self.inventory_open;
        if !self.inventory_open
            && let Some(held) = self.held.take()
        {
            self.inventory.add(held, &self.content.items);
        }
    }

    /// Click-to-move logic for an inventory slot (pick up / place / merge / swap).
    pub(super) fn handle_slot_click(&mut self, index: usize) {
        // An armor slot only accepts its own piece. Taking a piece back off is
        // always allowed, so this only gates the held stack going in.
        if let Some(held) = self.held
            && !self
                .inventory
                .can_equip(index, held.item, &self.content.items)
        {
            return;
        }
        match (self.held, self.inventory.slot(index)) {
            (None, Some(stack)) => {
                self.held = Some(stack);
                self.inventory.set_slot(index, None);
            }
            (Some(held), None) => {
                self.inventory.set_slot(index, Some(held));
                self.held = None;
            }
            (Some(mut held), Some(mut stack)) => {
                if held.item == stack.item {
                    let max = self.content.items.max_stack(stack.item);
                    let leftover = stack.merge(held, max);
                    self.inventory.set_slot(index, Some(stack));
                    self.held = if leftover == 0 {
                        None
                    } else {
                        held.count = leftover;
                        Some(held)
                    };
                } else {
                    // Swap held and slot.
                    self.inventory.set_slot(index, Some(held));
                    self.held = Some(stack);
                }
            }
            (None, None) => {}
        }
    }

    /// Craft the recipe at `index`: consume its ingredients and store the
    /// output; whatever doesn't fit is tossed out in front of the player.
    pub(super) fn handle_craft(&mut self, index: usize) {
        let Some(recipe) = self.recipes.get(index) else {
            return;
        };
        let Some(stack) = recipe.craft(&mut self.inventory, &self.content.items) else {
            return;
        };
        let leftover = self.inventory.add(stack, &self.content.items);
        if leftover > 0 {
            self.drops.push(DroppedItem::thrown(
                ItemStack {
                    count: leftover,
                    ..stack
                },
                self.player.eye_position(),
                self.player.look_direction(),
                self.content.entities.dropped_item(),
            ));
        }
    }
}

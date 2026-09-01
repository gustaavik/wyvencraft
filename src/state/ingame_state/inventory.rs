//! Inventory-screen interactions: open/close and click-to-move between slots.

use super::InGameState;
use crate::inventory::ItemStack;

impl InGameState {
    /// Open/close the inventory screen; returns a held stack to storage on close.
    ///
    /// Whatever no longer fits is thrown rather than dropped on the floor of the
    /// function: closing the panel is not a reason to destroy items, and a full
    /// inventory is exactly when a player is most likely to be holding one.
    pub(super) fn toggle_inventory(&mut self) {
        self.inventory_open = !self.inventory_open;
        if !self.inventory_open
            && let Some(held) = self.held.take()
        {
            let leftover = self.inventory.add(held, &self.content.items);
            if leftover > 0 {
                self.throw(ItemStack {
                    count: leftover,
                    ..held
                });
            }
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
}

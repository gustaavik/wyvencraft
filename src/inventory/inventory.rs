//! The player inventory container: a grid of slots plus a 9-slot hotbar.

use super::item::{ItemId, ItemRegistry, ItemStack};

/// Number of quick-access hotbar slots (also the first slots of the inventory).
pub const HOTBAR_SIZE: usize = 9;
/// Total inventory slots (hotbar + main grid), Minecraft-style 9x4.
pub const INVENTORY_SIZE: usize = 36;

/// A fixed array of optional item stacks plus the currently selected hotbar slot.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
    selected: usize,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: vec![None; INVENTORY_SIZE],
            selected: 0,
        }
    }

    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    pub fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    pub fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = stack.filter(|s| !s.is_empty());
        }
    }

    // --- Hotbar ---

    pub fn selected_index(&self) -> usize {
        self.selected
    }

    pub fn set_selected(&mut self, index: usize) {
        self.selected = index % HOTBAR_SIZE;
    }

    /// Scroll the hotbar selection by `delta` slots (wrapping).
    pub fn scroll_selected(&mut self, delta: i32) {
        let n = HOTBAR_SIZE as i32;
        self.selected = (((self.selected as i32 + delta) % n + n) % n) as usize;
    }

    pub fn selected_stack(&self) -> Option<ItemStack> {
        self.slot(self.selected)
    }

    // --- Mutation ---

    /// Add items, stacking onto existing stacks first, then filling empty slots.
    /// Returns the number of items that didn't fit.
    pub fn add(&mut self, mut stack: ItemStack, registry: &ItemRegistry) -> u8 {
        let max = registry.max_stack(stack.item);

        // First pass: top up matching stacks.
        for slot in self.slots.iter_mut() {
            if stack.is_empty() {
                return 0;
            }
            if let Some(existing) = slot
                && existing.item == stack.item
            {
                stack.count = existing.merge(stack, max);
            }
        }
        // Second pass: empty slots.
        for slot in self.slots.iter_mut() {
            if stack.is_empty() {
                return 0;
            }
            if slot.is_none() {
                let put = stack.count.min(max);
                // Struct-update (not `new`) so a re-collected tool keeps its wear.
                *slot = Some(ItemStack {
                    count: put,
                    ..stack
                });
                stack.count -= put;
            }
        }
        stack.count
    }

    /// Remove up to `amount` of the selected stack (e.g. when placing a block).
    /// Returns how many were actually consumed.
    pub fn consume_selected(&mut self, amount: u8) -> u8 {
        let Some(stack) = self.slots[self.selected].as_mut() else {
            return 0;
        };
        let taken = amount.min(stack.count);
        stack.count -= taken;
        if stack.count == 0 {
            self.slots[self.selected] = None;
        }
        taken
    }

    /// Wear down the selected tool by one point; clears the slot when it breaks.
    /// No-op for items without durability. Returns `true` if the tool broke.
    pub fn damage_selected_tool(&mut self) -> bool {
        let Some(stack) = self.slots[self.selected].as_mut() else {
            return false;
        };
        let Some(remaining) = stack.durability.as_mut() else {
            return false;
        };
        *remaining = remaining.saturating_sub(1);
        if *remaining == 0 {
            self.slots[self.selected] = None;
            true
        } else {
            false
        }
    }

    /// Take a single item off the selected stack — the last one takes the whole
    /// stack (preserving tool durability). Used by the drop key.
    pub fn take_one_selected(&mut self) -> Option<ItemStack> {
        let slot = self.slots[self.selected].as_mut()?;
        if slot.count <= 1 {
            return self.slots[self.selected].take();
        }
        slot.count -= 1;
        Some(ItemStack::single(slot.item))
    }

    pub fn item_in_selected(&self) -> Option<ItemId> {
        self.selected_stack().map(|s| s.item)
    }

    // --- Crafting support ---

    /// Total number of `item` across all slots.
    pub fn count_of(&self, item: ItemId) -> u32 {
        self.slots
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Remove up to `amount` of `item`, draining later slots first so crafting
    /// eats from the storage grid before the hotbar. Returns how many were
    /// actually removed.
    pub fn remove(&mut self, item: ItemId, amount: u32) -> u32 {
        let mut remaining = amount;
        for slot in self.slots.iter_mut().rev() {
            if remaining == 0 {
                break;
            }
            if let Some(stack) = slot
                && stack.item == item
            {
                let taken = (stack.count as u32).min(remaining) as u8;
                stack.count -= taken;
                remaining -= taken as u32;
                if stack.count == 0 {
                    *slot = None;
                }
            }
        }
        amount - remaining
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn take_one_from_a_stack_leaves_the_rest() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId(3), 5)));
        inv.set_selected(0);
        let taken = inv.take_one_selected().expect("stack has items");
        assert_eq!(taken.count, 1);
        assert_eq!(inv.slot(0).expect("rest stays").count, 4);
    }

    #[test]
    fn take_one_from_a_single_item_empties_the_slot_and_keeps_durability() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::with_durability(ItemId(1), 37)));
        inv.set_selected(0);
        let taken = inv.take_one_selected().expect("slot has an item");
        assert_eq!(taken.durability, Some(37));
        assert!(inv.slot(0).is_none());
        assert!(inv.take_one_selected().is_none());
    }

    #[test]
    fn adding_a_used_tool_keeps_its_durability() {
        let blocks = crate::world::block::BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let mut inv = Inventory::new();
        let leftover = inv.add(
            ItemStack::with_durability(items.find("wooden pickaxe").unwrap(), 12),
            &items,
        );
        assert_eq!(leftover, 0);
        assert_eq!(inv.slot(0).expect("tool stored").durability, Some(12));
    }

    #[test]
    fn remove_drains_later_slots_before_the_hotbar() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::new(ItemId(7), 10))); // hotbar
        inv.set_slot(20, Some(ItemStack::new(ItemId(7), 5))); // storage grid
        assert_eq!(inv.count_of(ItemId(7)), 15);

        let removed = inv.remove(ItemId(7), 8);
        assert_eq!(removed, 8);
        assert!(inv.slot(20).is_none(), "storage stack is drained first");
        assert_eq!(inv.slot(0).expect("hotbar keeps the rest").count, 7);

        assert_eq!(inv.remove(ItemId(7), 100), 7, "removal caps at what exists");
        assert_eq!(inv.count_of(ItemId(7)), 0);
    }

    #[test]
    fn tool_breaks_after_durability_depletes() {
        let mut inv = Inventory::new();
        inv.set_slot(0, Some(ItemStack::with_durability(ItemId(0), 2)));
        inv.set_selected(0);
        assert!(!inv.damage_selected_tool(), "first use just wears the tool");
        assert!(inv.damage_selected_tool(), "second use breaks the tool");
        assert!(inv.slot(0).is_none(), "broken tool leaves an empty slot");
    }
}

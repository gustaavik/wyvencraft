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
            if let Some(existing) = slot {
                if existing.item == stack.item {
                    stack.count = existing.merge(stack, max);
                }
            }
        }
        // Second pass: empty slots.
        for slot in self.slots.iter_mut() {
            if stack.is_empty() {
                return 0;
            }
            if slot.is_none() {
                let put = stack.count.min(max);
                *slot = Some(ItemStack::new(stack.item, put));
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

    pub fn item_in_selected(&self) -> Option<ItemId> {
        self.selected_stack().map(|s| s.item)
    }
}

impl Default for Inventory {
    fn default() -> Self {
        Self::new()
    }
}

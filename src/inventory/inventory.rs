//! The player inventory container: a grid of slots plus a 9-slot hotbar, with
//! the six equipped armor slots appended after the storage region.
//!
//! Slot layout: `0..HOTBAR_SIZE` hotbar, `HOTBAR_SIZE..INVENTORY_SIZE` main
//! grid, `ARMOR_START..TOTAL_SLOTS` equipped armor (one per
//! [`ArmorSlot`], in [`ArmorSlot::ALL`] order). Storage-facing operations
//! ([`Inventory::add`], [`Inventory::count_of`], [`Inventory::remove`]) are
//! bounded to `..ARMOR_START`, so crafting never eats worn armor and pickups
//! never land in an armor slot.

use super::item::{ArmorSlot, ItemId, ItemRegistry, ItemStack};

/// Number of quick-access hotbar slots (also the first slots of the inventory).
pub const HOTBAR_SIZE: usize = 9;
/// Storage slots (hotbar + main grid), Minecraft-style 9x4.
pub const INVENTORY_SIZE: usize = 36;
/// First equipped-armor slot; storage ends here.
pub const ARMOR_START: usize = INVENTORY_SIZE;
/// One equipped slot per [`ArmorSlot`].
pub const ARMOR_SIZE: usize = ArmorSlot::ALL.len();
/// Every addressable slot: storage + armor.
pub const TOTAL_SLOTS: usize = ARMOR_START + ARMOR_SIZE;

/// A fixed array of optional item stacks plus the currently selected hotbar slot.
#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub struct Inventory {
    slots: Vec<Option<ItemStack>>,
    selected: usize,
}

impl Inventory {
    pub fn new() -> Self {
        Self {
            slots: vec![None; TOTAL_SLOTS],
            selected: 0,
        }
    }

    pub fn slots(&self) -> &[Option<ItemStack>] {
        &self.slots
    }

    pub fn slot(&self, index: usize) -> Option<ItemStack> {
        self.slots.get(index).copied().flatten()
    }

    /// Write a slot verbatim. Deliberately unchecked: this is also the bulk
    /// path for save restore and the multiplayer inventory sync, which must
    /// never reject. Interactive equipping goes through [`Inventory::can_equip`].
    pub fn set_slot(&mut self, index: usize, stack: Option<ItemStack>) {
        if let Some(slot) = self.slots.get_mut(index) {
            *slot = stack.filter(|s| !s.is_empty());
        }
    }

    // --- Armor ---

    /// Inventory slot index of an equipment slot.
    #[inline]
    pub fn armor_slot_index(slot: ArmorSlot) -> usize {
        ARMOR_START + slot.index()
    }

    /// The stack equipped in `slot`, if any.
    pub fn equipped(&self, slot: ArmorSlot) -> Option<ItemStack> {
        self.slot(Self::armor_slot_index(slot))
    }

    /// The equipped item of each [`ArmorSlot`], in `ArmorSlot::ALL` order.
    pub fn equipped_armor(&self) -> [Option<ItemId>; ARMOR_SIZE] {
        std::array::from_fn(|i| self.slot(ARMOR_START + i).map(|s| s.item))
    }

    /// Whether `item` may be placed in slot `index`. Storage slots take
    /// anything; an armor slot only takes armor declaring that exact slot.
    pub fn can_equip(&self, index: usize, item: ItemId, items: &ItemRegistry) -> bool {
        let Some(offset) = index.checked_sub(ARMOR_START) else {
            return true; // storage slot
        };
        match (ArmorSlot::ALL.get(offset), items.armor(item)) {
            (Some(&slot), Some(armor)) => armor.slot == slot,
            _ => false,
        }
    }

    /// Total defense points of the worn pieces.
    pub fn total_defense(&self, items: &ItemRegistry) -> f32 {
        self.slots[ARMOR_START..]
            .iter()
            .flatten()
            .filter_map(|stack| items.armor(stack.item))
            .map(|armor| armor.defense)
            .sum()
    }

    /// Wear every worn piece down by `amount`, clearing the ones that break.
    pub fn wear_armor(&mut self, amount: u16) {
        for slot in self.slots[ARMOR_START..].iter_mut() {
            let Some(stack) = slot else { continue };
            let Some(remaining) = stack.durability.as_mut() else {
                continue;
            };
            *remaining = remaining.saturating_sub(amount);
            if *remaining == 0 {
                *slot = None;
            }
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

    /// Add items to the storage region, stacking onto existing stacks first,
    /// then filling empty slots. Returns the number of items that didn't fit.
    pub fn add(&mut self, mut stack: ItemStack, registry: &ItemRegistry) -> u8 {
        let max = registry.max_stack(stack.item);

        // First pass: top up matching stacks.
        for slot in self.slots[..ARMOR_START].iter_mut() {
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
        for slot in self.slots[..ARMOR_START].iter_mut() {
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

    /// Total number of `item` across the storage slots. Worn armor doesn't count.
    pub fn count_of(&self, item: ItemId) -> u32 {
        self.slots[..ARMOR_START]
            .iter()
            .flatten()
            .filter(|s| s.item == item)
            .map(|s| s.count as u32)
            .sum()
    }

    /// Remove up to `amount` of `item` from storage, draining later slots first
    /// so crafting eats from the storage grid before the hotbar (and never off
    /// the player's back). Returns how many were actually removed.
    pub fn remove(&mut self, item: ItemId, amount: u32) -> u32 {
        let mut remaining = amount;
        for slot in self.slots[..ARMOR_START].iter_mut().rev() {
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
            ItemStack::with_durability(items.find("wooden_pickaxe").unwrap(), 12),
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

    /// Storage operations must stop at `ARMOR_START`, or a full inventory would
    /// spill pickups onto the player's head and crafting would melt down their
    /// armor for parts.
    #[test]
    fn storage_operations_never_touch_the_armor_slots() {
        let blocks = crate::world::block::BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let stone = items.find("stone").unwrap();

        let mut inv = Inventory::new();
        // Fill every storage slot; the armor region stays empty.
        for index in 0..ARMOR_START {
            inv.set_slot(index, Some(ItemStack::new(stone, 64)));
        }

        let leftover = inv.add(ItemStack::new(stone, 10), &items);
        assert_eq!(leftover, 10, "a full inventory rejects the whole stack");
        for index in ARMOR_START..TOTAL_SLOTS {
            assert!(inv.slot(index).is_none(), "slot {index} took an overflow");
        }

        // A helmet on the player's head is invisible to crafting.
        let helmet = items.find("helmet").unwrap();
        inv.set_slot(
            Inventory::armor_slot_index(ArmorSlot::Helmet),
            Some(ItemStack::single(helmet)),
        );
        assert_eq!(inv.count_of(helmet), 0, "worn armor is not stock");
        assert_eq!(
            inv.remove(helmet, 1),
            0,
            "crafting cannot consume worn armor"
        );
        assert!(
            inv.equipped(ArmorSlot::Helmet).is_some(),
            "helmet stays worn"
        );
    }

    #[test]
    fn armor_slots_only_take_their_own_piece() {
        let blocks = crate::world::block::BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let inv = Inventory::new();

        let helmet = items.find("helmet").unwrap();
        let boots = items.find("boots").unwrap();
        let pickaxe = items.find("wooden_pickaxe").unwrap();
        let head = Inventory::armor_slot_index(ArmorSlot::Helmet);

        assert!(inv.can_equip(head, helmet, &items));
        assert!(!inv.can_equip(head, boots, &items), "boots are not a hat");
        assert!(
            !inv.can_equip(head, pickaxe, &items),
            "a pickaxe is not armor"
        );
        assert!(inv.can_equip(0, pickaxe, &items), "storage takes anything");
    }

    #[test]
    fn worn_armor_sums_defense_and_wears_out() {
        let blocks = crate::world::block::BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let mut inv = Inventory::new();
        assert_eq!(inv.total_defense(&items), 0.0, "bare player has no defense");

        for slot in ArmorSlot::ALL {
            let id = items.find(slot.label().to_lowercase().as_str()).unwrap();
            inv.set_slot(
                Inventory::armor_slot_index(slot),
                Some(items.full_stack(id)),
            );
        }
        let full = inv.total_defense(&items);
        assert!(full > 0.0, "a full set defends");

        // Wear the whole set past the flimsiest piece's durability.
        let flimsiest = ArmorSlot::ALL
            .iter()
            .filter_map(|&s| inv.equipped(s))
            .filter_map(|s| s.durability)
            .min()
            .expect("armor carries durability");
        inv.wear_armor(flimsiest);
        assert!(
            inv.total_defense(&items) < full,
            "a broken piece stops defending"
        );
    }
}

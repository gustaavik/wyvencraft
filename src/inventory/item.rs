//! Item definitions, stacks, and the [`ItemRegistry`].
//!
//! Items are kept independent of rendering. Every placeable block has a
//! corresponding item, generated from the [`BlockRegistry`].

use crate::core::BlockId;
use crate::world::block::BlockRegistry;

/// Identifier of an item type; index into the [`ItemRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub u16);

/// Static description of an item type.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub max_stack: u8,
    /// If set, using this item places the given block.
    pub place_block: Option<BlockId>,
}

/// A stack of identical items in one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[derive(serde::Serialize, serde::Deserialize)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u8,
}

impl ItemStack {
    pub fn new(item: ItemId, count: u8) -> Self {
        Self { item, count }
    }

    pub fn single(item: ItemId) -> Self {
        Self { item, count: 1 }
    }

    pub fn is_empty(&self) -> bool {
        self.count == 0
    }

    /// Try to merge `other` into this stack up to `max_stack`. Returns the
    /// leftover that didn't fit (count 0 if fully merged).
    pub fn merge(&mut self, other: ItemStack, max_stack: u8) -> u8 {
        if self.item != other.item {
            return other.count;
        }
        let space = max_stack.saturating_sub(self.count);
        let moved = space.min(other.count);
        self.count += moved;
        other.count - moved
    }

    /// Split off up to `amount` items into a new stack.
    pub fn split(&mut self, amount: u8) -> ItemStack {
        let taken = amount.min(self.count);
        self.count -= taken;
        ItemStack::new(self.item, taken)
    }
}

/// Lookup table of item types. Index alignment with block ids is *not* assumed;
/// use [`ItemRegistry::item_for_block`] to map.
pub struct ItemRegistry {
    items: Vec<Item>,
    /// `block_to_item[block_id] = Some(item_id)` for placeable blocks.
    block_to_item: Vec<Option<ItemId>>,
}

impl ItemRegistry {
    /// Build an item for every visible block in the block registry.
    pub fn from_blocks(blocks: &BlockRegistry) -> Self {
        let mut items = Vec::new();
        let mut block_to_item = vec![None; blocks.len()];

        for (block_id, block) in blocks.iter() {
            if block_id.is_air() {
                continue;
            }
            let item_id = ItemId(items.len() as u16);
            items.push(Item {
                name: block.name.to_string(),
                max_stack: 64,
                place_block: Some(block_id),
            });
            block_to_item[block_id.0 as usize] = Some(item_id);
        }

        Self {
            items,
            block_to_item,
        }
    }

    pub fn get(&self, id: ItemId) -> &Item {
        &self.items[id.0 as usize]
    }

    pub fn item_for_block(&self, block: BlockId) -> Option<ItemId> {
        self.block_to_item.get(block.0 as usize).copied().flatten()
    }

    pub fn max_stack(&self, id: ItemId) -> u8 {
        self.get(id).max_stack
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

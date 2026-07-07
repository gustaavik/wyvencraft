//! Item definitions, stacks, and the [`ItemRegistry`].
//!
//! Items are kept independent of rendering. Every placeable block has a
//! corresponding item, generated from the [`BlockRegistry`]; on top of those the
//! registry also defines a few hand-authored tools and foods used by survival.

use crate::core::BlockId;
use crate::world::block::{BlockRegistry, is_flowing_water};

/// Identifier of an item type; index into the [`ItemRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub u16);

/// A tool category, matched against a block's material for mining speed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum ToolKind {
    Pickaxe,
    Axe,
    Shovel,
    Shears,
}

/// What eating an item restores.
#[derive(Debug, Clone, Copy)]
pub struct FoodValue {
    pub hunger: f32,
    pub saturation: f32,
}

/// Static description of an item type.
#[derive(Debug, Clone)]
pub struct Item {
    pub name: String,
    pub max_stack: u8,
    /// If set, using this item places the given block.
    pub place_block: Option<BlockId>,
    /// If set, the item is a tool of this category.
    pub tool: Option<ToolKind>,
    /// Mining-speed multiplier when the tool matches the block (1.0 = bare hand).
    pub dig_speed: f32,
    /// If set, the item has durability and wears out at zero.
    pub max_durability: Option<u16>,
    /// If set, the item is edible.
    pub food: Option<FoodValue>,
}

impl Item {
    /// A placeable block item (stacks to 64).
    pub fn block(name: impl Into<String>, block: BlockId) -> Self {
        Self {
            name: name.into(),
            max_stack: 64,
            place_block: Some(block),
            tool: None,
            dig_speed: 1.0,
            max_durability: None,
            food: None,
        }
    }

    /// A non-stacking tool with a dig-speed multiplier and durability.
    pub fn tool(name: impl Into<String>, kind: ToolKind, dig_speed: f32, durability: u16) -> Self {
        Self {
            name: name.into(),
            max_stack: 1,
            place_block: None,
            tool: Some(kind),
            dig_speed,
            max_durability: Some(durability),
            food: None,
        }
    }

    /// An edible item.
    pub fn food(name: impl Into<String>, hunger: f32, saturation: f32) -> Self {
        Self {
            name: name.into(),
            max_stack: 64,
            place_block: None,
            tool: None,
            dig_speed: 1.0,
            max_durability: None,
            food: Some(FoodValue { hunger, saturation }),
        }
    }
}

/// A stack of identical items in one slot.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct ItemStack {
    pub item: ItemId,
    pub count: u8,
    /// Remaining durability for tools; `None` for items without durability.
    pub durability: Option<u16>,
}

impl ItemStack {
    pub fn new(item: ItemId, count: u8) -> Self {
        Self {
            item,
            count,
            durability: None,
        }
    }

    pub fn single(item: ItemId) -> Self {
        Self {
            item,
            count: 1,
            durability: None,
        }
    }

    /// A single item carrying a starting durability (a fresh tool).
    pub fn with_durability(item: ItemId, durability: u16) -> Self {
        Self {
            item,
            count: 1,
            durability: Some(durability),
        }
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
    // Well-known non-block items, for the survival starter kit and the palette.
    pub wooden_pickaxe: ItemId,
    pub wooden_axe: ItemId,
    pub wooden_shovel: ItemId,
    pub shears: ItemId,
    pub apple: ItemId,
    pub bread: ItemId,
}

impl ItemRegistry {
    /// Build an item for every visible block, then the hand-authored tools/foods.
    pub fn from_blocks(blocks: &BlockRegistry) -> Self {
        let mut items: Vec<Item> = Vec::new();
        let mut block_to_item = vec![None; blocks.len()];

        for (block_id, block) in blocks.iter() {
            // Flowing water is fluid-simulation state, not a placeable block.
            if block_id.is_air() || is_flowing_water(block_id) {
                continue;
            }
            let item_id = ItemId(items.len() as u16);
            items.push(Item::block(block.name, block_id));
            block_to_item[block_id.0 as usize] = Some(item_id);
        }

        let mut push = |item: Item| {
            let id = ItemId(items.len() as u16);
            items.push(item);
            id
        };
        let wooden_pickaxe = push(Item::tool("wooden pickaxe", ToolKind::Pickaxe, 2.0, 60));
        let wooden_axe = push(Item::tool("wooden axe", ToolKind::Axe, 2.0, 60));
        let wooden_shovel = push(Item::tool("wooden shovel", ToolKind::Shovel, 2.0, 60));
        let shears = push(Item::tool("shears", ToolKind::Shears, 5.0, 120));
        let apple = push(Item::food("apple", 4.0, 2.4));
        let bread = push(Item::food("bread", 5.0, 6.0));

        Self {
            items,
            block_to_item,
            wooden_pickaxe,
            wooden_axe,
            wooden_shovel,
            shears,
            apple,
            bread,
        }
    }

    pub fn get(&self, id: ItemId) -> &Item {
        &self.items[id.0 as usize]
    }

    pub fn item_for_block(&self, block: BlockId) -> Option<ItemId> {
        self.block_to_item.get(block.0 as usize).copied().flatten()
    }

    /// Look up an item by its exact name (as used by recipe files).
    pub fn find(&self, name: &str) -> Option<ItemId> {
        self.items
            .iter()
            .position(|item| item.name == name)
            .map(|i| ItemId(i as u16))
    }

    pub fn max_stack(&self, id: ItemId) -> u8 {
        self.get(id).max_stack
    }

    pub fn tool(&self, id: ItemId) -> Option<ToolKind> {
        self.get(id).tool
    }

    pub fn dig_speed(&self, id: ItemId) -> f32 {
        self.get(id).dig_speed
    }

    pub fn food(&self, id: ItemId) -> Option<FoodValue> {
        self.get(id).food
    }

    pub fn max_durability(&self, id: ItemId) -> Option<u16> {
        self.get(id).max_durability
    }

    /// A full, ready-to-use stack of `id` (max count, or a fresh tool).
    pub fn full_stack(&self, id: ItemId) -> ItemStack {
        match self.max_durability(id) {
            Some(dur) => ItemStack::with_durability(id, dur),
            None => ItemStack::new(id, self.max_stack(id)),
        }
    }

    /// Iterate every item with its id (used by the creative palette).
    pub fn iter(&self) -> impl Iterator<Item = (ItemId, &Item)> {
        self.items
            .iter()
            .enumerate()
            .map(|(i, item)| (ItemId(i as u16), item))
    }

    pub fn len(&self) -> usize {
        self.items.len()
    }

    pub fn is_empty(&self) -> bool {
        self.items.is_empty()
    }
}

//! Item definitions, stacks, and the [`ItemRegistry`].
//!
//! Items are kept independent of rendering. Every placeable block gets an
//! auto-generated item; `assets/items.toml` then declares the rest (tools,
//! foods, armor) as data, expressing behavior through typed components
//! ([`ToolSpec`], [`FoodValue`], [`ArmorSpec`]) that the gameplay code
//! dispatches on — never on item identity.
//!
//! An item is identified by its **id** (see [`crate::core::ident`]) — the key
//! saves, recipes, the wire and `/give` all use. The label the player reads is
//! presentation and rides out of the parse in [`ItemVisuals`] alongside the
//! models, so it never reaches [`Item`] and never feeds `content_hash`.

use crate::core::BlockId;
use crate::core::ident::is_valid_id;
use crate::world::block::{BlockMaterial, BlockRegistry};
use wyven_model::ModelSpec;

/// Embedded copy of the shipped item definitions, used when
/// `assets/items.toml` is missing or invalid.
pub const BUILTIN_ITEMS: &str = include_str!("../../assets/items.toml");

/// Identifier of an item type; index into the [`ItemRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub u16);

/// Tool behavior component (`[item.tool]` in `assets/items.toml`).
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ToolSpec {
    /// Free-form tool kind; matched against block `drops` rules
    /// (e.g. leaves only drop for kind `"shears"`).
    pub kind: String,
    /// Mining-speed multiplier when the tool matches the block (hand = 1.0).
    pub dig_speed: f32,
    /// Uses before the tool wears out.
    pub durability: u16,
    /// Block materials this tool mines at full speed.
    pub harvests: Vec<BlockMaterial>,
    /// Melee damage per swing. `None` means this tool is no better than a bare
    /// fist, so the fist's damage stays defined in exactly one place
    /// (`state::ingame_state::mobs::PLAYER_ATTACK_DAMAGE`) instead of being
    /// duplicated as a default here.
    #[serde(default)]
    pub damage: Option<f32>,
}

/// Food behavior component: what eating the item restores.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct FoodValue {
    pub hunger: f32,
    pub saturation: f32,
}

/// Which equipment slot an armor piece occupies. The discriminants are the
/// order of the armor slots in the inventory (see `super::inventory::ARMOR_START`)
/// and of the labelled column in the inventory screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArmorSlot {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
    Glove,
    Cape,
}

impl ArmorSlot {
    pub const ALL: [ArmorSlot; 6] = [
        ArmorSlot::Helmet,
        ArmorSlot::Chestplate,
        ArmorSlot::Leggings,
        ArmorSlot::Boots,
        ArmorSlot::Glove,
        ArmorSlot::Cape,
    ];

    /// Offset of this slot within the inventory's armor region.
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Display name, also the label shown beside the slot.
    pub fn label(self) -> &'static str {
        match self {
            ArmorSlot::Helmet => "Helmet",
            ArmorSlot::Chestplate => "Chestplate",
            ArmorSlot::Leggings => "Leggings",
            ArmorSlot::Boots => "Boots",
            ArmorSlot::Glove => "Glove",
            ArmorSlot::Cape => "Cape",
        }
    }
}

/// Armor behavior component (`[item.armor]` in `assets/items.toml`): which slot
/// the piece fits, how much damage it absorbs, and how long it lasts.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ArmorSpec {
    pub slot: ArmorSlot,
    /// Defense points; summed across worn pieces (see `Player::damage`).
    pub defense: f32,
    /// Uses before the piece wears out.
    pub durability: u16,
}

/// Static description of an item type.
#[derive(Debug, Clone)]
pub struct Item {
    /// Machine-readable key: `[a-z0-9_]`, unique, and the save/wire format.
    /// The player-facing label lives on `content`, not here — see
    /// [`ItemVisuals::display_names`].
    pub id: String,
    pub max_stack: u8,
    /// If set, using this item places the given block.
    pub place_block: Option<BlockId>,
    /// If set, the item is a tool.
    pub tool: Option<ToolSpec>,
    /// If set, the item is edible.
    pub food: Option<FoodValue>,
    /// If set, the item is wearable armor.
    pub armor: Option<ArmorSpec>,
}

impl Item {
    /// A placeable block item (stacks to 64), sharing the block's id.
    pub fn block(id: impl Into<String>, block: BlockId) -> Self {
        Self {
            id: id.into(),
            max_stack: 64,
            place_block: Some(block),
            tool: None,
            food: None,
            armor: None,
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

// ---- TOML schema -----------------------------------------------------------

#[derive(serde::Deserialize)]
struct ItemFile {
    #[serde(default)]
    item: Vec<ItemDef>,
    starter_kit: Option<StarterKitDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct ItemDef {
    id: String,
    /// Overrides the label derived from `id`, for the ids the rule gets wrong.
    display_name: Option<String>,
    max_stack: Option<u8>,
    place_block: Option<String>,
    tool: Option<ToolSpec>,
    food: Option<FoodValue>,
    armor: Option<ArmorSpec>,
    /// `[item.model]` — the 3D model this item is drawn as when it is held or
    /// lying in the world. Purely visual, so it is handed back out of band
    /// rather than stored on [`Item`]: see [`ItemRegistry::from_toml_with_models`].
    model: Option<ModelSpec>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct StarterKitDef {
    #[serde(default)]
    survival: Vec<KitEntry>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct KitEntry {
    item: String,
    #[serde(default = "default_kit_count")]
    count: u8,
}

fn default_kit_count() -> u8 {
    1
}

/// The presentation-only data an item file carries, reported out of the parse
/// rather than stored on [`Item`] — the item-side twin of
/// [`crate::world::block::BlockVisuals`].
///
/// Both vectors are indexed by [`ItemId`] and cover every item, block items
/// included. Keeping them here rather than on [`Item`] is what stops them
/// reaching `content_hash`, which gates multiplayer joins: two players whose
/// sword is drawn — or labelled — differently have no reason to be refused a
/// shared world.
#[derive(Debug, Default)]
pub struct ItemVisuals {
    /// `[item.model]` — what a held or dropped stack is drawn as.
    pub models: Vec<Option<ModelSpec>>,
    /// `display_name = "..."` — an explicit label, where title-casing the id
    /// would get it wrong. `None` means "derive it", which `content` does;
    /// carrying the `Option` is what lets a block item fall back to the
    /// *block's* label rather than to its own derived one.
    pub display_names: Vec<Option<String>>,
}

/// Lookup table of item types. Index alignment with block ids is *not* assumed;
/// use [`ItemRegistry::item_for_block`] to map.
#[derive(Debug)]
pub struct ItemRegistry {
    items: Vec<Item>,
    /// `block_to_item[block_id] = Some(item_id)` for placeable blocks.
    block_to_item: Vec<Option<ItemId>>,
    /// The resolved survival starter kit, in hotbar order.
    starter_survival: Vec<ItemStack>,
}

impl ItemRegistry {
    /// Build the registry from the embedded copy of `assets/items.toml`.
    /// Infallible: the shipped file is validated by the golden tests.
    pub fn from_blocks(blocks: &BlockRegistry) -> Self {
        Self::from_toml(BUILTIN_ITEMS, blocks).expect("embedded items.toml must parse")
    }

    /// Parse an items file against the loaded blocks. An item is auto-generated
    /// for every visible, non-flowing block first; `[[item]]` entries then
    /// override an auto item by id or append new items (declared order defines
    /// the numeric [`ItemId`]s after the block items).
    ///
    /// Structural errors — bad TOML, or a malformed id — fail the whole file,
    /// and the caller falls back to [`ItemRegistry::from_blocks`]. Bad
    /// *references* inside entries (an unknown `place_block`) only degrade that
    /// entry with a warning, following the recipes-file precedent.
    pub fn from_toml(text: &str, blocks: &BlockRegistry) -> Result<Self, String> {
        Self::from_toml_with_visuals(text, blocks, &mut ItemVisuals::default())
    }

    /// Like [`ItemRegistry::from_toml`], but also reports each item's
    /// `[item.model]` and `display_name` in [`ItemVisuals`], indexed by
    /// [`ItemId`].
    ///
    /// Both are presentation and are kept off [`Item`] on purpose, for the same
    /// reason as `content::ItemIcon`: `Item` feeds `content_hash`, which gates
    /// multiplayer joins, and two players whose swords are drawn — or
    /// labelled — differently have no reason to be refused a shared world.
    pub fn from_toml_with_visuals(
        text: &str,
        blocks: &BlockRegistry,
        visuals: &mut ItemVisuals,
    ) -> Result<Self, String> {
        let file: ItemFile = toml::from_str(text).map_err(|e| e.to_string())?;

        let ItemVisuals {
            models,
            display_names,
        } = visuals;
        models.clear();
        display_names.clear();
        let mut items: Vec<Item> = Vec::new();
        let mut block_to_item = vec![None; blocks.len()];
        for (block_id, block) in blocks.iter() {
            // Flowing fluid is simulation state, not a placeable block.
            if block_id.is_air() || blocks.is_flowing_fluid(block_id) {
                continue;
            }
            let item_id = ItemId(items.len() as u16);
            items.push(Item::block(block.id.clone(), block_id));
            block_to_item[block_id.0 as usize] = Some(item_id);
        }

        for def in file.item {
            // A malformed id fails the whole file: an id is the key recipes,
            // drops, saves and `/give` all spell, so accepting one that cannot
            // be typed as a single token would break those references silently.
            if !is_valid_id(&def.id) {
                return Err(format!(
                    "item {:?}: an id must be lowercase letters, digits and underscores",
                    def.id
                ));
            }
            let place_block = def.place_block.as_ref().and_then(|block_id| {
                let id = blocks.find(block_id);
                if id.is_none() {
                    log::warn!("item {:?}: unknown place_block {block_id:?}", def.id);
                }
                id
            });
            let index = match items.iter().position(|i| i.id == def.id) {
                // Override an auto-generated block item: only the fields the
                // entry specifies change.
                Some(idx) => {
                    let item = &mut items[idx];
                    if let Some(max_stack) = def.max_stack {
                        item.max_stack = max_stack;
                    }
                    if place_block.is_some() {
                        item.place_block = place_block;
                    }
                    if def.tool.is_some() {
                        item.tool = def.tool;
                    }
                    if def.food.is_some() {
                        item.food = def.food;
                    }
                    if def.armor.is_some() {
                        item.armor = def.armor;
                    }
                    idx
                }
                None => {
                    let single = def.tool.is_some() || def.armor.is_some();
                    let max_stack = def.max_stack.unwrap_or(if single { 1 } else { 64 });
                    items.push(Item {
                        id: def.id,
                        max_stack,
                        place_block,
                        tool: def.tool,
                        food: def.food,
                        armor: def.armor,
                    });
                    items.len() - 1
                }
            };
            if let Some(model) = def.model {
                models.resize(items.len().max(models.len()), None);
                models[index] = Some(model);
            }
            if let Some(label) = def.display_name {
                display_names.resize(items.len().max(display_names.len()), None);
                display_names[index] = Some(label);
            }
        }
        models.resize(items.len(), None);
        display_names.resize(items.len(), None);

        let mut reg = Self {
            items,
            block_to_item,
            starter_survival: Vec::new(),
        };
        if let Some(kit) = file.starter_kit {
            for entry in kit.survival {
                let Some(id) = reg.find(&entry.item) else {
                    log::warn!("starter kit: unknown item {:?}", entry.item);
                    continue;
                };
                // Tools spawn fresh; stackables spawn `count`.
                let stack = match reg.max_durability(id) {
                    Some(durability) => ItemStack::with_durability(id, durability),
                    None => ItemStack::new(id, entry.count.clamp(1, reg.max_stack(id))),
                };
                reg.starter_survival.push(stack);
            }
        }
        Ok(reg)
    }

    /// What a fresh survival player spawns with, in hotbar order.
    pub fn starter_kit_survival(&self) -> &[ItemStack] {
        &self.starter_survival
    }

    pub fn get(&self, id: ItemId) -> &Item {
        &self.items[id.0 as usize]
    }

    pub fn item_for_block(&self, block: BlockId) -> Option<ItemId> {
        self.block_to_item.get(block.0 as usize).copied().flatten()
    }

    /// Look up an item by its exact id (as used by recipe files, saves and
    /// `/give`).
    pub fn find(&self, id: &str) -> Option<ItemId> {
        self.items
            .iter()
            .position(|item| item.id == id)
            .map(|i| ItemId(i as u16))
    }

    pub fn max_stack(&self, id: ItemId) -> u8 {
        self.get(id).max_stack
    }

    pub fn tool(&self, id: ItemId) -> Option<&ToolSpec> {
        self.get(id).tool.as_ref()
    }

    pub fn food(&self, id: ItemId) -> Option<FoodValue> {
        self.get(id).food
    }

    pub fn armor(&self, id: ItemId) -> Option<ArmorSpec> {
        self.get(id).armor
    }

    /// Starting durability of a fresh item, for the components that wear out.
    /// An item is never both a tool and armor; tools win if a file says otherwise.
    pub fn max_durability(&self, id: ItemId) -> Option<u16> {
        let item = self.get(id);
        item.tool
            .as_ref()
            .map(|tool| tool.durability)
            .or_else(|| item.armor.map(|armor| armor.durability))
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden snapshot of the shipped item set. The data-driven loader must
    /// reproduce this exactly: names are the save format and registration
    /// order defines the numeric ids synced over the network.
    #[test]
    fn builtin_items_golden() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);

        // One placeable item per visible, non-flowing block, in block order.
        let block_items = [
            "stone",
            "dirt",
            "grass",
            "sand",
            "water",
            "oak_log",
            "oak_leaves",
            "glass",
            "bedrock",
            "snow",
            "gravel",
            "clay",
            "coal_ore",
            "iron_ore",
            "copper_ore",
            "cobblestone",
            "blue_bells",
            "red_flower",
            "red_mushroom",
            "brown_mushroom",
            "cornflower",
        ];
        const STONE: &[BlockMaterial] = &[BlockMaterial::Stone];
        const WOOD: &[BlockMaterial] = &[BlockMaterial::Wood];
        const PLANT: &[BlockMaterial] = &[BlockMaterial::Plant];
        const DIGGABLE: &[BlockMaterial] = &[BlockMaterial::Dirt, BlockMaterial::Sand];
        /// One expected tool: name, kind, dig_speed, durability, harvests, damage.
        type ToolRow = (
            &'static str,
            &'static str,
            f32,
            u16,
            &'static [BlockMaterial],
            Option<f32>,
        );
        let tools: [ToolRow; 14] = [
            ("wooden_pickaxe", "pickaxe", 2.0, 60, STONE, None),
            ("wooden_axe", "axe", 2.0, 60, WOOD, Some(3.0)),
            ("wooden_shovel", "shovel", 2.0, 60, DIGGABLE, None),
            ("shears", "shears", 5.0, 120, PLANT, None),
            ("vine_sword", "sword", 1.5, 200, PLANT, Some(4.0)),
            ("wooden_sword", "sword", 1.5, 60, PLANT, Some(4.0)),
            ("stone_pickaxe", "pickaxe", 4.0, 132, STONE, None),
            ("stone_axe", "axe", 4.0, 132, WOOD, Some(4.0)),
            ("stone_shovel", "shovel", 4.0, 132, DIGGABLE, None),
            ("stone_sword", "sword", 1.5, 132, PLANT, Some(5.0)),
            ("iron_pickaxe", "pickaxe", 6.0, 250, STONE, None),
            ("iron_axe", "axe", 6.0, 250, WOOD, Some(5.0)),
            ("iron_shovel", "shovel", 6.0, 250, DIGGABLE, None),
            ("iron_sword", "sword", 1.5, 250, PLANT, Some(6.0)),
        ];
        // (name, hunger, saturation)
        let foods = [
            ("apple", 4.0, 2.4),
            ("bread", 5.0, 6.0),
            ("raw_beef", 3.0, 1.8),
            ("mutton", 2.0, 1.2),
            ("raw_chicken", 2.0, 1.2),
            ("cooked_beef", 8.0, 12.8),
            ("cooked_mutton", 6.0, 9.6),
            ("cooked_chicken", 6.0, 7.2),
            ("cooked_porkchop", 8.0, 12.8),
        ];
        // Plain stackables declared between the foods and the armor: mob and
        // block materials with no components at all.
        let materials = [
            "leather",
            "feather",
            "string",
            "arrow",
            "coal",
            "clay_ball",
            "flint",
            "copper_ingot",
        ];
        // (name, slot, defense, durability)
        let armors = [
            ("copper_helmet", ArmorSlot::Helmet, 2.0, 120),
            ("copper_chestplate", ArmorSlot::Chestplate, 6.0, 240),
            ("copper_leggings", ArmorSlot::Leggings, 5.0, 200),
            ("copper_boots", ArmorSlot::Boots, 2.0, 120),
            ("glove", ArmorSlot::Glove, 1.0, 80),
            ("cape", ArmorSlot::Cape, 1.0, 80),
        ];

        // Plain stackables with no components at all, declared after the armor.
        let plain = ["stick"];

        assert_eq!(
            items.len(),
            block_items.len()
                + tools.len()
                + foods.len()
                + materials.len()
                + armors.len()
                + plain.len(),
            "item count changed"
        );

        for (i, &name) in block_items.iter().enumerate() {
            let item = items.get(ItemId(i as u16));
            assert_eq!(item.id, name, "item {i}: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            assert_eq!(item.place_block, blocks.find(name), "{name}: place_block");
            assert!(item.tool.is_none() && item.food.is_none(), "{name}: plain");
            // Blocks map back to their item.
            let block_id = blocks.find(name).unwrap();
            assert_eq!(
                items.item_for_block(block_id),
                Some(ItemId(i as u16)),
                "{name}: item_for_block"
            );
        }

        for (offset, &(name, kind, dig_speed, durability, harvests, damage)) in
            tools.iter().enumerate()
        {
            let id = ItemId((block_items.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "tool: name");
            assert_eq!(item.max_stack, 1, "{name}: max_stack");
            let tool = item.tool.as_ref().expect("tool spec");
            assert_eq!(tool.kind, kind, "{name}: kind");
            assert_eq!(tool.dig_speed, dig_speed, "{name}: dig_speed");
            assert_eq!(tool.durability, durability, "{name}: durability");
            assert_eq!(tool.harvests, harvests, "{name}: harvests");
            assert_eq!(tool.damage, damage, "{name}: damage");
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        for (offset, &(name, hunger, saturation)) in foods.iter().enumerate() {
            let id = ItemId((block_items.len() + tools.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "food: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            let food = item.food.expect("food value");
            assert_eq!(food.hunger, hunger, "{name}: hunger");
            assert_eq!(food.saturation, saturation, "{name}: saturation");
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        for (offset, &name) in materials.iter().enumerate() {
            let id = ItemId((block_items.len() + tools.len() + foods.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "material: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            assert!(
                item.tool.is_none()
                    && item.food.is_none()
                    && item.armor.is_none()
                    && item.place_block.is_none(),
                "{name}: carries no components"
            );
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        for (offset, &(name, slot, defense, durability)) in armors.iter().enumerate() {
            let id = ItemId(
                (block_items.len() + tools.len() + foods.len() + materials.len() + offset) as u16,
            );
            let item = items.get(id);
            assert_eq!(item.id, name, "armor: name");
            assert_eq!(item.max_stack, 1, "{name}: max_stack");
            let armor = item.armor.expect("armor spec");
            assert_eq!(armor.slot, slot, "{name}: slot");
            assert_eq!(armor.defense, defense, "{name}: defense");
            assert_eq!(armor.durability, durability, "{name}: durability");
            assert_eq!(items.find(name), Some(id), "{name}: find");
            // Armor wears like a tool: a fresh piece carries full durability.
            assert_eq!(
                items.full_stack(id),
                ItemStack::with_durability(id, durability),
                "{name}: full_stack"
            );
        }

        for (offset, &name) in plain.iter().enumerate() {
            let id = ItemId(
                (block_items.len()
                    + tools.len()
                    + foods.len()
                    + materials.len()
                    + armors.len()
                    + offset) as u16,
            );
            let item = items.get(id);
            assert_eq!(item.id, name, "plain: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            assert!(
                item.tool.is_none()
                    && item.food.is_none()
                    && item.armor.is_none()
                    && item.place_block.is_none(),
                "{name}: carries no components"
            );
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        // The starter kit resolves in hotbar order: four fresh tools, then the
        // two foods with their counts.
        let kit = items.starter_kit_survival();
        assert_eq!(kit.len(), 6, "starter kit size");
        for (slot, name) in [
            "wooden_pickaxe",
            "wooden_axe",
            "wooden_shovel",
            "vine_sword",
        ]
        .iter()
        .enumerate()
        {
            let id = items.find(name).unwrap();
            assert_eq!(kit[slot], items.full_stack(id), "kit slot {slot}");
        }
        assert_eq!(
            kit[4],
            ItemStack::new(items.find("apple").unwrap(), 5),
            "kit apples"
        );
        assert_eq!(
            kit[5],
            ItemStack::new(items.find("bread").unwrap(), 3),
            "kit bread"
        );
    }

    /// A malformed id fails the whole file, like the block table: an id is the
    /// key recipes, drops, saves and `/give` all spell, so one that cannot be
    /// typed as a single token would break those references silently.
    #[test]
    fn a_malformed_item_id_rejects_the_whole_file() {
        let blocks = BlockRegistry::with_builtins();
        for bad in ["Wooden Pickaxe", "wooden pickaxe", "wooden-pickaxe", ""] {
            let text = format!("[[item]]\nid = \"{bad}\"\n");
            let err = ItemRegistry::from_toml(&text, &blocks)
                .err()
                .unwrap_or_else(|| panic!("{bad:?} should be rejected"));
            assert!(err.contains("id"), "{bad:?}: unhelpful error {err:?}");
        }
    }

    /// Every shipped id is well formed — the loader enforces it, but asserting
    /// it here names the rule where the item set is snapshotted.
    #[test]
    fn every_builtin_item_id_is_well_formed() {
        let blocks = BlockRegistry::with_builtins();
        for (_, item) in ItemRegistry::from_blocks(&blocks).iter() {
            assert!(is_valid_id(&item.id), "malformed item id {:?}", item.id);
        }
    }

    /// A label is presentation, so like a model it rides out in `ItemVisuals`
    /// and never reaches `Item`, which feeds `content_hash`.
    #[test]
    fn display_names_ride_out_of_band_and_stay_off_item() {
        let blocks = BlockRegistry::with_builtins();
        let mut visuals = ItemVisuals::default();
        let items = ItemRegistry::from_toml_with_visuals(
            "[[item]]\nid = \"tnt\"\ndisplay_name = \"TNT\"\n\n[[item]]\nid = \"stick\"\n",
            &blocks,
            &mut visuals,
        )
        .expect("valid file");

        assert_eq!(visuals.display_names.len(), items.len(), "one per item");
        let tnt = items.find("tnt").expect("declared");
        let stick = items.find("stick").expect("declared");
        assert_eq!(
            visuals.display_names[tnt.0 as usize].as_deref(),
            Some("TNT"),
            "authored"
        );
        assert_eq!(
            visuals.display_names[stick.0 as usize], None,
            "left for `content` to derive"
        );
        assert!(
            !format!("{:?}", items.get(tnt)).contains("TNT"),
            "display name must stay off Item"
        );
    }

    /// `[item.model]` is reported alongside the registry rather than stored on
    /// `Item`: it is visual-only and must not reach `content_hash`.
    #[test]
    fn item_models_are_reported_out_of_band() {
        let blocks = BlockRegistry::with_builtins();
        let mut visuals = ItemVisuals::default();
        let items = ItemRegistry::from_toml_with_visuals(BUILTIN_ITEMS, &blocks, &mut visuals)
            .expect("builtin items parse");
        let models = &visuals.models;

        assert_eq!(models.len(), items.len(), "one entry per item");

        let sword = items.find("vine_sword").expect("vine_sword");
        let spec = models[sword.0 as usize]
            .as_ref()
            .expect("vine sword declares a model");
        // Not the exact extension: either export of this object is valid here,
        // and pinning one would make swapping formats a test failure.
        assert!(
            spec.path.starts_with("assets/models/vine_sword."),
            "unexpected model path {:?}",
            spec.path
        );
        assert_eq!(spec.scale, 0.35);
        assert_eq!(spec.offset, [-0.5, 0.75, -0.5]);

        // Everything else is model-less, and nothing about the model leaks onto
        // the item itself. The item's *id* is legitimately "vine_sword", so what
        // must be absent is the model file it points at.
        let apple = items.find("apple").expect("apple");
        assert!(models[apple.0 as usize].is_none());
        let debug = format!("{:?}", items.get(sword));
        assert!(
            !debug.contains("assets/models") && !debug.contains(".bbmodel"),
            "the model path leaked onto Item: {debug}"
        );
    }

    /// The tiered tools are flat in the XY plane, unlike `vine_sword`, so they
    /// all carry the quarter-turn that stands them broadside in the fist. A
    /// tool that silently lost it would render edge-on and near-invisible.
    #[test]
    fn tiered_tool_models_are_turned_broadside() {
        let blocks = BlockRegistry::with_builtins();
        let mut visuals = ItemVisuals::default();
        let items = ItemRegistry::from_toml_with_visuals(BUILTIN_ITEMS, &blocks, &mut visuals)
            .expect("builtin items parse");
        let models = &visuals.models;

        for tier in ["wooden", "stone", "iron"] {
            for shape in ["pickaxe", "axe", "shovel", "sword"] {
                let name = format!("{tier}_{shape}");
                let id = items.find(&name).unwrap_or_else(|| panic!("{name} exists"));
                let spec = models[id.0 as usize]
                    .as_ref()
                    .unwrap_or_else(|| panic!("{name} declares a model"));
                assert_eq!(
                    spec.path,
                    format!("assets/models/{tier}_{shape}.bbmodel"),
                    "{name}: model path"
                );
                assert_eq!(spec.rotation, [0.0, 90.0, 0.0], "{name}: rotation");
            }
        }
    }

    /// A tier is only worth crafting if it strictly beats the one below it. The
    /// gradient lives in `dig_speed` and `durability` for the digging shapes,
    /// and in `damage` and `durability` for swords (which never dig).
    #[test]
    fn each_tool_shape_improves_with_every_tier() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);

        let spec = |name: &str| {
            let id = items.find(name).unwrap_or_else(|| panic!("{name} exists"));
            items.tool(id).expect("tool spec").clone()
        };

        for shape in ["pickaxe", "axe", "shovel"] {
            let tiers: Vec<_> = ["wooden", "stone", "iron"]
                .iter()
                .map(|tier| spec(&format!("{tier}_{shape}")))
                .collect();
            for pair in tiers.windows(2) {
                let (lo, hi) = (&pair[0], &pair[1]);
                assert!(hi.dig_speed > lo.dig_speed, "{shape}: dig_speed");
                assert!(hi.durability > lo.durability, "{shape}: durability");
                assert_eq!(
                    hi.harvests, lo.harvests,
                    "{shape}: harvests are the tier-independent part"
                );
                assert_eq!(hi.kind, lo.kind, "{shape}: kind");
            }
        }

        let swords: Vec<_> = ["wooden", "stone", "iron"]
            .iter()
            .map(|tier| spec(&format!("{tier}_sword")))
            .collect();
        for pair in swords.windows(2) {
            let (lo, hi) = (&pair[0], &pair[1]);
            assert!(hi.damage > lo.damage, "sword: damage");
            assert!(hi.durability > lo.durability, "sword: durability");
            assert_eq!(hi.dig_speed, lo.dig_speed, "sword: dig_speed is flat");
        }
    }

    /// Digging tools deliberately do *not* fight better than a fist — only the
    /// shapes with an edge declare `damage`.
    #[test]
    fn only_the_fighting_shapes_carry_damage() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);

        for tier in ["wooden", "stone", "iron"] {
            for shape in ["pickaxe", "shovel"] {
                let name = format!("{tier}_{shape}");
                let id = items.find(&name).expect("tool exists");
                assert_eq!(items.tool(id).unwrap().damage, None, "{name}: no damage");
            }
            for shape in ["sword", "axe"] {
                let name = format!("{tier}_{shape}");
                let id = items.find(&name).expect("tool exists");
                assert!(
                    items.tool(id).unwrap().damage.is_some(),
                    "{name}: declares damage"
                );
            }
        }
    }
}

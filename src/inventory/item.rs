//! Item definitions, stacks, and the [`ItemRegistry`] — the one registry of
//! everything the player can hold.
//!
//! An [`Item`] is an id, a stack size, and a **set of capability components**
//! ([`super::component`]): `placeable`, `tool`, `consumable`, `equippable`,
//! `shearable`. Gameplay asks an item for the capability it needs and gets
//! `None` if the item has not got it — it never asks *what the item is*. Every
//! placeable block gets an item automatically; `assets/items.toml` then declares
//! the rest, and overrides any auto item by id.
//!
//! Items are kept independent of rendering. An item is identified by its **id**
//! (see [`crate::core::ident`]) — the key saves, recipes, the wire and `/give`
//! all use. The label the player reads is presentation and rides out of the
//! parse in [`ItemVisuals`] alongside the models, so it never reaches [`Item`]
//! and never feeds `content_hash`.

use crate::core::ident::is_valid_id;
use crate::world::block::BlockRegistry;
use wyven_model::ModelSpec;

use super::component::{self, ComponentCtx, ItemComponent, Placeable};

/// Embedded copy of the shipped item definitions, used when
/// `assets/items.toml` is missing or invalid.
pub const BUILTIN_ITEMS: &str = include_str!("../../assets/items.toml");

/// How many of an item fit in one slot when nothing says otherwise.
const DEFAULT_MAX_STACK: u8 = 64;

/// Identifier of an item type; index into the [`ItemRegistry`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ItemId(pub u16);

/// Static description of an item type.
#[derive(Debug)]
pub struct Item {
    /// Machine-readable key: `[a-z0-9_]`, unique, and the save/wire format.
    /// The player-facing label lives on `content`, not here — see
    /// [`ItemVisuals::display_names`].
    pub id: String,
    pub max_stack: u8,
    /// What this item can do, at most one component per key and **always in key
    /// order**.
    ///
    /// The ordering is not cosmetic. `Item`'s `Debug` feeds
    /// `content::content_hash`, which gates multiplayer joins, so two peers who
    /// spelled the same tables in a different order must hash identically.
    /// [`Item::set`] is the only mutator and maintains it.
    components: Vec<Box<dyn ItemComponent>>,
}

impl Item {
    /// A placeable block item (stacks to 64), sharing the block's id.
    pub fn block(id: impl Into<String>, block: crate::core::BlockId) -> Self {
        Self {
            id: id.into(),
            max_stack: DEFAULT_MAX_STACK,
            components: vec![Box::new(Placeable { block })],
        }
    }

    /// An item with no capabilities at all (a crafting material).
    pub fn plain(id: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            max_stack: DEFAULT_MAX_STACK,
            components: Vec::new(),
        }
    }

    /// This item's `C` capability, if it has one.
    ///
    /// The single seam between "what the data declared" and "what the code
    /// does": a call site depends on the one capability it uses and on nothing
    /// else about `Item`.
    pub fn get<C: ItemComponent>(&self) -> Option<&C> {
        self.components
            .iter()
            .find_map(|component| component::downcast::<C>(component.as_ref()))
    }

    /// Whether this item carries the capability named `key` — the by-name form,
    /// for the one caller that has a string rather than a type (a block's
    /// `drops = { requires = "..." }`).
    pub fn has(&self, key: &str) -> bool {
        self.components.iter().any(|c| c.key() == key)
    }

    /// Add `component`, replacing any it already has under that key.
    ///
    /// Insertion is by binary search, so the key ordering `components` promises
    /// is maintained by construction rather than by a sort somebody has to
    /// remember to call.
    fn set(&mut self, component: Box<dyn ItemComponent>) {
        let key = component.key();
        match self.components.binary_search_by(|held| held.key().cmp(key)) {
            Ok(index) => self.components[index] = component,
            Err(index) => self.components.insert(index, component),
        }
    }

    /// The stack size the declared capabilities imply: the tightest ceiling any
    /// of them sets, or [`DEFAULT_MAX_STACK`]. An authored `max_stack` wins over
    /// this.
    fn derived_max_stack(&self) -> u8 {
        self.components
            .iter()
            .filter_map(|component| component.stack_limit())
            .min()
            .unwrap_or(DEFAULT_MAX_STACK)
    }

    /// Starting durability, from the first capability in key order that wears.
    /// An item that is both a tool and a worn piece is a data error; the loader
    /// warns about it once, and this stays deterministic rather than guessing.
    fn max_durability(&self) -> Option<u16> {
        self.components
            .iter()
            .find_map(|component| component.durability())
    }

    /// How many of this item's capabilities declare a durability. Used only by
    /// the loader, to warn about the ambiguous case exactly once.
    fn wearing_capabilities(&self) -> usize {
        self.components
            .iter()
            .filter(|component| component.durability().is_some())
            .count()
    }
}

impl Clone for Item {
    fn clone(&self) -> Self {
        Self {
            id: self.id.clone(),
            max_stack: self.max_stack,
            components: self.components.iter().map(|c| c.clone_box()).collect(),
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

/// One `[[item]]` entry.
///
/// Everything that is not one of the four named fields is a capability table,
/// collected raw and handed to the parser in [`component::COMPONENTS`] that
/// claims its key — which is what makes adding a capability cost nothing here.
/// That is also why there is no `deny_unknown_fields` (serde forbids it
/// alongside `flatten`): a table nothing claims is rejected explicitly below,
/// **by name**, which is a better diagnostic than serde's would have been.
#[derive(serde::Deserialize)]
struct ItemDef {
    id: String,
    /// Overrides the label derived from `id`, for the ids the rule gets wrong.
    display_name: Option<String>,
    max_stack: Option<u8>,
    /// `[item.model]` — the 3D model this item is drawn as when it is held or
    /// lying in the world. Purely visual, so it is handed back out of band
    /// rather than stored on [`Item`]: see [`ItemRegistry::from_toml_with_visuals`].
    model: Option<ModelSpec>,
    #[serde(flatten)]
    components: toml::Table,
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
    /// Structural errors — bad TOML, a malformed id, an unknown capability
    /// table, a capability that will not parse — fail the whole file, and the
    /// caller falls back to [`ItemRegistry::from_blocks`]. Bad *references*
    /// inside a capability (an unknown placeable block) only degrade that
    /// capability with a warning, following the recipes-file precedent.
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
        // An authored `max_stack` wins over the one the capabilities imply, and
        // has to be remembered per item because the derived value is only
        // settled once every override has been merged.
        let mut authored_stack: Vec<Option<u8>> = Vec::new();
        let mut block_to_item = vec![None; blocks.len()];
        for (block_id, block) in blocks.iter() {
            // Flowing fluid is simulation state, not a placeable block.
            if block_id.is_air() || blocks.is_flowing_fluid(block_id) {
                continue;
            }
            let item_id = ItemId(items.len() as u16);
            items.push(Item::block(block.id.clone(), block_id));
            authored_stack.push(None);
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
            let ctx = ComponentCtx {
                blocks,
                item: &def.id,
            };
            let mut declared: Vec<Box<dyn ItemComponent>> = Vec::new();
            for (key, value) in def.components {
                let Some(parser) = component::parser_for(&key) else {
                    return Err(format!("item {:?}: unknown component [item.{key}]", def.id));
                };
                if let Some(built) = parser.parse(value, &ctx)? {
                    declared.push(built);
                }
            }

            let index = match items.iter().position(|i| i.id == def.id) {
                // Override an auto-generated block item, or an earlier entry:
                // only the capabilities this entry declares are replaced, so an
                // override that says nothing about placement keeps it.
                Some(idx) => idx,
                None => {
                    items.push(Item::plain(def.id));
                    authored_stack.push(None);
                    items.len() - 1
                }
            };
            for built in declared {
                items[index].set(built);
            }
            if def.max_stack.is_some() {
                authored_stack[index] = def.max_stack;
            }
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

        // Stack sizes settle only now: an override may have added the very
        // capability that caps them.
        for (item, authored) in items.iter_mut().zip(&authored_stack) {
            item.max_stack = authored.unwrap_or_else(|| item.derived_max_stack());
            if item.wearing_capabilities() > 1 {
                log::warn!(
                    "item {:?}: more than one capability declares a durability; \
                     the first in key order wins",
                    item.id
                );
            }
        }

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

    /// The `C` capability of item `id`, if it has one — the shorthand for
    /// `registry.get(id).get::<C>()`, which is most of what callers want.
    pub fn component<C: ItemComponent>(&self, id: ItemId) -> Option<&C> {
        self.get(id).get::<C>()
    }

    /// Whether item `id` carries the capability named `key`.
    pub fn has(&self, id: ItemId, key: &str) -> bool {
        self.get(id).has(key)
    }

    pub fn item_for_block(&self, block: crate::core::BlockId) -> Option<ItemId> {
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

    /// Starting durability of a fresh item, for the capabilities that wear out.
    pub fn max_durability(&self, id: ItemId) -> Option<u16> {
        self.get(id).max_durability()
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
    use crate::inventory::component::{ArmorSlot, Consumable, Equippable, Shearable, Tool};
    use crate::world::block::BlockMaterial;

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
        /// One expected tool: name, dig_speed, durability, harvests, damage.
        type ToolRow = (
            &'static str,
            f32,
            u16,
            &'static [BlockMaterial],
            Option<f32>,
        );
        let tools: [ToolRow; 14] = [
            ("wooden_pickaxe", 2.0, 60, STONE, None),
            ("wooden_axe", 2.0, 60, WOOD, Some(3.0)),
            ("wooden_shovel", 2.0, 60, DIGGABLE, None),
            ("shears", 5.0, 120, PLANT, None),
            ("vine_sword", 1.5, 200, PLANT, Some(4.0)),
            ("wooden_sword", 1.5, 60, PLANT, Some(4.0)),
            ("stone_pickaxe", 4.0, 132, STONE, None),
            ("stone_axe", 4.0, 132, WOOD, Some(4.0)),
            ("stone_shovel", 4.0, 132, DIGGABLE, None),
            ("stone_sword", 1.5, 132, PLANT, Some(5.0)),
            ("iron_pickaxe", 6.0, 250, STONE, None),
            ("iron_axe", 6.0, 250, WOOD, Some(5.0)),
            ("iron_shovel", 6.0, 250, DIGGABLE, None),
            ("iron_sword", 1.5, 250, PLANT, Some(6.0)),
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
            ("raw_porkchop", 3.0, 1.8),
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
            assert_eq!(
                item.get::<Placeable>().map(|p| p.block),
                blocks.find(name),
                "{name}: places itself"
            );
            assert!(
                item.get::<Tool>().is_none() && item.get::<Consumable>().is_none(),
                "{name}: plain"
            );
            // Blocks map back to their item.
            let block_id = blocks.find(name).unwrap();
            assert_eq!(
                items.item_for_block(block_id),
                Some(ItemId(i as u16)),
                "{name}: item_for_block"
            );
        }

        for (offset, &(name, dig_speed, durability, harvests, damage)) in tools.iter().enumerate() {
            let id = ItemId((block_items.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "tool: name");
            assert_eq!(item.max_stack, 1, "{name}: max_stack");
            let tool = item.get::<Tool>().expect("tool capability");
            assert_eq!(tool.dig_speed, dig_speed, "{name}: dig_speed");
            assert_eq!(tool.durability, durability, "{name}: durability");
            assert_eq!(tool.harvests, harvests, "{name}: harvests");
            assert_eq!(tool.damage, damage, "{name}: damage");
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        // Shearing is a capability of its own, not a tool kind — it is what
        // oak leaves ask for, and only this one item answers.
        let shearers: Vec<&str> = items
            .iter()
            .filter(|(_, item)| item.get::<Shearable>().is_some())
            .map(|(_, item)| item.id.as_str())
            .collect();
        assert_eq!(shearers, ["shears"], "only shears shear");

        for (offset, &(name, hunger, saturation)) in foods.iter().enumerate() {
            let id = ItemId((block_items.len() + tools.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "food: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            let food = item.get::<Consumable>().expect("consumable capability");
            assert_eq!(food.hunger, hunger, "{name}: hunger");
            assert_eq!(food.saturation, saturation, "{name}: saturation");
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        for (offset, &name) in materials.iter().enumerate() {
            let id = ItemId((block_items.len() + tools.len() + foods.len() + offset) as u16);
            let item = items.get(id);
            assert_eq!(item.id, name, "material: name");
            assert_eq!(item.max_stack, 64, "{name}: max_stack");
            assert!(item.components.is_empty(), "{name}: carries no components");
            assert_eq!(items.find(name), Some(id), "{name}: find");
        }

        for (offset, &(name, slot, defense, durability)) in armors.iter().enumerate() {
            let id = ItemId(
                (block_items.len() + tools.len() + foods.len() + materials.len() + offset) as u16,
            );
            let item = items.get(id);
            assert_eq!(item.id, name, "armor: name");
            assert_eq!(item.max_stack, 1, "{name}: max_stack");
            let worn = item.get::<Equippable>().expect("equippable capability");
            assert_eq!(worn.slot, slot, "{name}: slot");
            assert_eq!(worn.defense, defense, "{name}: defense");
            assert_eq!(worn.durability, durability, "{name}: durability");
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
            assert!(item.components.is_empty(), "{name}: carries no components");
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

    /// A table nothing claims is almost always a typo for one that exists, and
    /// silently dropping it would ship an item missing the behaviour its author
    /// wrote down. The error names the key, because that is what has to change.
    #[test]
    fn an_unknown_capability_rejects_the_whole_file_and_names_it() {
        let blocks = BlockRegistry::with_builtins();
        let err = ItemRegistry::from_toml(
            "[[item]]\nid = \"stick\"\n\n[item.edible]\nhunger = 1.0\n",
            &blocks,
        )
        .expect_err("`edible` is not a capability");
        assert!(err.contains("edible"), "unhelpful error {err:?}");
    }

    /// A capability that fails to *parse* is structural and fails the file, so
    /// a typo inside a table cannot ship as silently-default numbers.
    #[test]
    fn a_malformed_capability_rejects_the_whole_file() {
        let blocks = BlockRegistry::with_builtins();
        assert!(
            ItemRegistry::from_toml(
                "[[item]]\nid = \"bread\"\n\n[item.consumable]\nhunger = 1.0\n",
                &blocks,
            )
            .is_err(),
            "a consumable without saturation is incomplete"
        );
    }

    /// `Item`'s `Debug` feeds `content_hash`, which gates multiplayer joins —
    /// so two peers who wrote the same capabilities in a different order must
    /// produce byte-identical items. Storing components in key order is what
    /// guarantees it.
    #[test]
    fn capability_order_in_the_file_does_not_change_the_item() {
        let blocks = BlockRegistry::with_builtins();
        let one = "[[item]]\nid = \"cleaver\"\n\n[item.shearable]\n\n[item.tool]\n\
                   dig_speed = 1.0\ndurability = 5\nharvests = [\"plant\"]\n";
        let other = "[[item]]\nid = \"cleaver\"\n\n[item.tool]\n\
                     dig_speed = 1.0\ndurability = 5\nharvests = [\"plant\"]\n\n[item.shearable]\n";
        let render = |text: &str| {
            let items = ItemRegistry::from_toml(text, &blocks).expect("valid file");
            let id = items.find("cleaver").expect("declared");
            format!("{:?}", items.get(id))
        };
        assert_eq!(render(one), render(other));
    }

    /// Stack size is derived from the capabilities rather than from a list of
    /// item kinds, so a *new* wearing capability gets the rule for free.
    #[test]
    fn wearing_capabilities_cap_the_stack_and_an_authored_size_still_wins() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_toml(
            "[[item]]\nid = \"plain\"\n\n\
             [[item]]\nid = \"digger\"\n[item.tool]\n\
             dig_speed = 1.0\ndurability = 5\nharvests = []\n\n\
             [[item]]\nid = \"hat\"\n[item.equippable]\n\
             slot = \"helmet\"\ndefense = 1.0\ndurability = 5\n\n\
             [[item]]\nid = \"oddity\"\nmax_stack = 16\n[item.tool]\n\
             dig_speed = 1.0\ndurability = 5\nharvests = []\n",
            &blocks,
        )
        .expect("valid file");
        let stack = |name: &str| items.max_stack(items.find(name).expect("declared"));
        assert_eq!(stack("plain"), 64, "nothing caps it");
        assert_eq!(stack("digger"), 1, "a tool wears");
        assert_eq!(stack("hat"), 1, "a worn piece wears");
        assert_eq!(stack("oddity"), 16, "an authored size wins over the rule");
    }

    /// An entry naming an auto-generated block item edits it rather than
    /// shadowing it: capabilities it does not mention — placement above all —
    /// survive, which is what lets `blue_bells` add a model and stay placeable.
    #[test]
    fn an_override_replaces_by_key_and_keeps_what_it_does_not_mention() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_toml(
            "[[item]]\nid = \"stone\"\nmax_stack = 32\n\n[item.consumable]\n\
             hunger = 1.0\nsaturation = 1.0\n",
            &blocks,
        )
        .expect("valid file");
        let stone = items.find("stone").expect("auto block item");
        assert_eq!(
            items.component::<Placeable>(stone).map(|p| p.block),
            blocks.find("stone"),
            "the inherited placement survives"
        );
        assert!(
            items.component::<Consumable>(stone).is_some(),
            "the declared capability is added"
        );
        assert_eq!(items.max_stack(stone), 32);
    }

    /// The whole point of asking for a capability: an item that has not got it
    /// says so, instead of the caller testing what kind of item it is.
    #[test]
    fn an_absent_capability_reads_as_none() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let stick = items.find("stick").expect("stick");
        assert!(items.component::<Tool>(stick).is_none());
        assert!(items.component::<Shearable>(stick).is_none());
        assert!(!items.has(stick, "shearable"));
        assert!(items.has(items.find("shears").expect("shears"), "shearable"));
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
            spec.path.starts_with("assets/models/items/vine_sword."),
            "unexpected model path {:?}",
            spec.path
        );
        assert_eq!(spec.scale, 0.35);
        assert_eq!(spec.offset, [-0.5, 0.75, -0.5]);

        // An item with no art yet declares no model either — flat items get one
        // extruded from their sprite, and `bread` has no sprite to extrude.
        let bread = items.find("bread").expect("bread");
        assert!(models[bread.0 as usize].is_none());

        // Nothing about the model leaks onto the item itself. The item's *id* is
        // legitimately "vine_sword", so what must be absent is the file it
        // points at.
        let debug = format!("{:?}", items.get(sword));
        assert!(
            !debug.contains("assets/models") && !debug.contains(".bbmodel"),
            "the model path leaked onto Item: {debug}"
        );
    }

    /// The tiered tools are flat in the XY plane, unlike `vine_sword`, so they
    /// all carry the quarter-turn that stands them broadside in the fist. A
    /// tool that silently lost it would render edge-on and near-invisible.
    ///
    /// The extension is deliberately not pinned: tools are migrating from
    /// `.bbmodel` to the Java Block/Item `.json` export one at a time, and a
    /// re-authored tool places itself from its own `display` block instead. The
    /// spec rotation stays as the fallback for any context that block does not
    /// name, which is why it is still asserted for every tier.
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
                let stem = format!("assets/models/items/{tier}_{shape}.");
                assert!(
                    spec.path.starts_with(&stem)
                        && (spec.path.ends_with(".bbmodel") || spec.path.ends_with(".json")),
                    "{name}: model path {}",
                    spec.path
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
            items
                .component::<Tool>(id)
                .expect("tool capability")
                .clone()
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
                assert_eq!(
                    items.component::<Tool>(id).unwrap().damage,
                    None,
                    "{name}: no damage"
                );
            }
            for shape in ["sword", "axe"] {
                let name = format!("{tier}_{shape}");
                let id = items.find(&name).expect("tool exists");
                assert!(
                    items.component::<Tool>(id).unwrap().damage.is_some(),
                    "{name}: declares damage"
                );
            }
        }
    }
}

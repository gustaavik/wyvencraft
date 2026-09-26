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
//! (see [`crate::domain::core::ident`]) — the key saves, recipes, the wire and `/give`
//! all use. The label the player reads is presentation and rides out of the
//! parse in [`ItemVisuals`] alongside the models, so it never reaches [`Item`]
//! and never feeds `content_hash`.

use crate::domain::core::ident::is_valid_id;
use crate::domain::world::block::BlockRegistry;
use wyven_model::ModelSpec;

use super::component::{self, ComponentCtx, ItemComponent, Placeable};

/// Embedded copy of the shipped item definitions, used when
/// `assets/items.toml` is missing or invalid.
pub const BUILTIN_ITEMS: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/items.toml"));

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
    pub fn block(id: impl Into<String>, block: crate::domain::core::BlockId) -> Self {
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
/// [`crate::domain::world::block::BlockVisuals`].
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

    pub fn item_for_block(&self, block: crate::domain::core::BlockId) -> Option<ItemId> {
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
mod tests;

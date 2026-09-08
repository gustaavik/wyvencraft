//! The capabilities an item can carry, and the table that parses them.
//!
//! An item is not a fixed set of fields with three behaviours bolted on: it is
//! an id, a stack size, and a set of **typed capability components**. Placing a
//! block, digging, eating and wearing are each one [`ItemComponent`]
//! declared as its own `[item.<key>]` table in `assets/items.toml`, parsed by
//! the one [`ComponentParser`] in [`COMPONENTS`] that claims that key.
//!
//! **Adding a capability is a new module here and one entry in [`COMPONENTS`]**
//! — the same shape as `chat::command::COMMANDS` and `ModelRegistry::LOADERS`,
//! and for the same reason: nothing in the loader, the registry or the stack
//! rules has to learn the new name. Gameplay reaches it with
//! [`Item::get`](super::Item::get), which is a downcast, so a call site depends
//! only on the capability it actually uses and never on the shape of `Item`.
//!
//! What a capability *does* is still code — this half is only what the data
//! declares. The hooks that act on it live where the action does
//! (`state::ingame_state::interaction` for right-click, `inventory::mining` for
//! digging).

use std::any::Any;
use std::fmt;

use crate::world::block::BlockRegistry;

mod consumable;
mod equippable;
mod placeable;
mod tool;

pub use consumable::Consumable;
pub use equippable::{ArmorSlot, Equippable};
pub use placeable::Placeable;
pub use tool::Tool;

use consumable::ConsumableParser;
use equippable::EquippableParser;
use placeable::PlaceableParser;
use tool::ToolParser;

/// One typed behaviour an item carries, parsed from its `[item.<key>]` table.
///
/// `Any` is a supertrait so [`Item::get`](super::Item::get) can downcast a
/// stored component back to its concrete type; `Send + Sync` because the item
/// registry lives inside the `Arc<GameContent>` shared across threads.
///
/// The two default methods are the *cross-cutting* rules — the ones the registry
/// applies without knowing which capabilities exist. A capability that neither
/// caps a stack nor wears out implements neither.
pub trait ItemComponent: fmt::Debug + Any + Send + Sync {
    /// The `[item.<key>]` table this component parses from, and the key it is
    /// stored under. Components are held in key order, which is what keeps
    /// `content::content_hash` — computed from `Item`'s `Debug` — independent of
    /// the order the tables happen to be spelled in.
    fn key(&self) -> &'static str;

    /// `Item` is `Clone` and a `Box<dyn ItemComponent>` is not.
    fn clone_box(&self) -> Box<dyn ItemComponent>;

    /// Ceiling this capability puts on a stack: `Some(1)` for anything that
    /// wears, because a stack of five half-worn pickaxes is not a thing.
    fn stack_limit(&self) -> Option<u8> {
        None
    }

    /// Uses before the item wears out, for the capabilities that wear.
    fn durability(&self) -> Option<u16> {
        None
    }
}

/// What a parser may consult beyond its own table.
///
/// Only [`Placeable`] uses `blocks` today (to turn `block = "stone"` into a
/// `BlockId`); `item` is here so a parser can name the offending entry in its
/// error, which is the difference between a usable diagnostic and a shrug.
pub struct ComponentCtx<'a> {
    pub blocks: &'a BlockRegistry,
    pub item: &'a str,
}

/// Turns one `[item.<key>]` table into its component.
///
/// Implementors are unit structs held in [`COMPONENTS`] as `&'static dyn` — they
/// carry no state, so the registry is a `const` and dispatch needs no allocation.
pub trait ComponentParser: Sync {
    /// The table name this parser claims. Must equal the [`ItemComponent::key`]
    /// of what it produces (asserted by a test).
    fn key(&self) -> &'static str;

    /// Parse the table.
    ///
    /// `Err` is a **structural** fault — a missing field, a bad type — and
    /// rejects the whole file, because a half-read item would silently change
    /// what the player is holding. `Ok(None)` is a *reference* that did not
    /// resolve (an unknown block name): the parser has already warned, and the
    /// item simply goes without that capability, following the same fail-soft
    /// rule the recipe and drop tables use.
    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String>;
}

/// Every capability this build knows about. **Adding one: a new module above
/// and one entry here.**
///
/// Listed in key order so the loader's `sort_by_key` is a no-op on a
/// well-behaved file and the order is obvious to read.
pub const COMPONENTS: &[&dyn ComponentParser] = &[
    &ConsumableParser,
    &EquippableParser,
    &PlaceableParser,
    &ToolParser,
];

/// The parser claiming `key`, if any.
pub fn parser_for(key: &str) -> Option<&'static dyn ComponentParser> {
    COMPONENTS.iter().copied().find(|p| p.key() == key)
}

/// Downcast a stored component back to its concrete type.
///
/// Free function rather than a trait method because it is generic, and a
/// generic method would make [`ItemComponent`] not object safe.
pub(super) fn downcast<C: ItemComponent>(component: &dyn ItemComponent) -> Option<&C> {
    (component as &dyn Any).downcast_ref::<C>()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The key is how a table is routed *and* how components are ordered, so a
    /// parser producing a component under a different key would file it where
    /// nothing looks for it.
    #[test]
    fn every_parser_key_matches_the_component_it_produces() {
        let blocks = BlockRegistry::with_builtins();
        let ctx = ComponentCtx {
            blocks: &blocks,
            item: "probe",
        };
        // Minimal well-formed tables, one per capability.
        let samples: &[(&str, &str)] = &[
            ("consumable", "hunger = 1.0\nsaturation = 1.0\n"),
            (
                "equippable",
                "slot = \"helmet\"\ndefense = 1.0\ndurability = 1\n",
            ),
            ("placeable", "block = \"stone\"\n"),
            (
                "tool",
                "kind = \"pickaxe\"\ndig_speed = 1.0\ndurability = 1\n",
            ),
        ];
        assert_eq!(
            samples.len(),
            COMPONENTS.len(),
            "a capability went untested"
        );
        for (key, body) in samples {
            let parser = parser_for(key).unwrap_or_else(|| panic!("no parser for {key:?}"));
            let value: toml::Value = toml::from_str(body).expect("sample table parses");
            let component = parser
                .parse(value, &ctx)
                .unwrap_or_else(|err| panic!("{key}: {err}"))
                .unwrap_or_else(|| panic!("{key}: sample should resolve"));
            assert_eq!(component.key(), *key, "{key}: filed under the wrong key");
        }
    }

    /// Two parsers claiming one key would make routing depend on list order.
    #[test]
    fn component_keys_are_unique() {
        let mut keys: Vec<&str> = COMPONENTS.iter().map(|p| p.key()).collect();
        let count = keys.len();
        keys.sort_unstable();
        keys.dedup();
        assert_eq!(keys.len(), count, "duplicate component key");
    }

    /// `COMPONENTS` is listed in key order so the list reads the way the
    /// components are stored.
    #[test]
    fn components_are_listed_in_key_order() {
        let keys: Vec<&str> = COMPONENTS.iter().map(|p| p.key()).collect();
        let mut sorted = keys.clone();
        sorted.sort_unstable();
        assert_eq!(keys, sorted);
    }
}

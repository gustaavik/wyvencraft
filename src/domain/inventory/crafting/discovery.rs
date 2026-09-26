//! Recipe discovery, Valheim-style: you learn a recipe the first time you hold
//! any of its ingredients (or the thing it makes).
//!
//! What is remembered is the set of **items** a player has held, not a list of
//! recipes. Recipes are derived from it on demand, which is what lets a host
//! change its recipe file without invalidating anyone's progress, and lets a
//! world saved before discovery existed work at once: the first frame's
//! [`KnownItems::learn_from`] teaches whatever the player is already carrying.
//!
//! Saves carry it as string ids (the save convention); the wire as numeric
//! ids, which `content_hash` guarantees both peers agree on.

use super::{Recipe, RecipeBook};
use crate::domain::inventory::{Inventory, ItemId, ItemRegistry};

/// Every item a player has ever held, as a bitset indexed by [`ItemId`].
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct KnownItems {
    bits: Vec<bool>,
}

impl KnownItems {
    pub fn knows(&self, item: ItemId) -> bool {
        self.bits.get(item.0 as usize).copied().unwrap_or(false)
    }

    /// Learn one item; `true` if it was new.
    pub fn learn(&mut self, item: ItemId) -> bool {
        let i = item.0 as usize;
        if i >= self.bits.len() {
            self.bits.resize(i + 1, false);
        }
        !std::mem::replace(&mut self.bits[i], true)
    }

    /// Learn everything in the inventory, worn armor included; `true` if
    /// anything was new. Cheap enough to run every frame, and doing so is
    /// what catches every way an item can arrive — a pickup, `/give`, a
    /// craft, a restore — in one place.
    pub fn learn_from(&mut self, inventory: &Inventory) -> bool {
        let mut any = false;
        for stack in inventory.slots().iter().flatten() {
            any |= self.learn(stack.item);
        }
        any
    }

    /// Learn everything `other` knows; `true` if anything was new.
    pub fn merge(&mut self, other: &KnownItems) -> bool {
        let mut any = false;
        for item in other.items() {
            any |= self.learn(item);
        }
        any
    }

    /// Whether `recipe` is revealed: its output, or any ingredient, is known.
    pub fn knows_recipe(&self, recipe: &Recipe) -> bool {
        self.knows(recipe.output) || recipe.ingredients.iter().any(|&(i, _)| self.knows(i))
    }

    /// Indices into `book` of every revealed recipe, in book order.
    pub fn known_recipes(&self, book: &RecipeBook) -> Vec<usize> {
        book.recipes()
            .iter()
            .enumerate()
            .filter(|(_, r)| self.knows_recipe(r))
            .map(|(i, _)| i)
            .collect()
    }

    pub fn items(&self) -> impl Iterator<Item = ItemId> + '_ {
        self.bits
            .iter()
            .enumerate()
            .filter(|(_, known)| **known)
            .map(|(i, _)| ItemId(i as u16))
    }

    /// String ids, for a save file.
    pub fn to_ids(&self, items: &ItemRegistry) -> Vec<String> {
        self.items()
            .filter(|&i| (i.0 as usize) < items.len())
            .map(|i| items.get(i).id.clone())
            .collect()
    }

    /// Back from a save file. An id this build does not know is skipped —
    /// the item it named no longer exists, so neither does anything it taught.
    pub fn from_ids(ids: &[String], items: &ItemRegistry) -> Self {
        let mut known = Self::default();
        for id in ids {
            match items.find(id) {
                Some(item) => {
                    known.learn(item);
                }
                None => log::debug!("discovery: unknown item {id:?}, skipped"),
            }
        }
        known
    }

    /// Numeric ids, for the wire.
    pub fn to_wire(&self) -> Vec<u16> {
        self.items().map(|i| i.0).collect()
    }

    /// Back from the wire; an id past the registry is dropped rather than
    /// trusted, since a peer sent it.
    pub fn from_wire(ids: &[u16], items: &ItemRegistry) -> Self {
        let mut known = Self::default();
        for &id in ids {
            if (id as usize) < items.len() {
                known.learn(ItemId(id));
            }
        }
        known
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::inventory::ItemStack;
    use crate::domain::inventory::crafting::station_ids;
    use crate::domain::world::block::BlockRegistry;

    fn setup() -> (ItemRegistry, RecipeBook) {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let book = RecipeBook::from_toml(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/recipes.toml")),
            &items,
            &station_ids(&blocks),
        )
        .unwrap();
        (items, book)
    }

    fn named(items: &ItemRegistry, book: &RecipeBook, indices: &[usize]) -> Vec<String> {
        indices
            .iter()
            .map(|&i| items.get(book.recipes()[i].output).id.clone())
            .collect()
    }

    #[test]
    fn nothing_is_known_until_something_is_held() {
        let (_, book) = setup();
        assert!(KnownItems::default().known_recipes(&book).is_empty());
    }

    #[test]
    fn holding_an_ingredient_reveals_every_recipe_that_uses_it() {
        let (items, book) = setup();
        let mut inv = Inventory::new();
        inv.add(
            ItemStack::new(items.find("cobblestone").unwrap(), 3),
            &items,
        );

        let mut known = KnownItems::default();
        assert!(known.learn_from(&inv), "something new was learned");
        assert!(!known.learn_from(&inv), "and nothing the second time");

        let revealed = named(&items, &book, &known.known_recipes(&book));
        for expected in ["stone_pickaxe", "stone_sword", "forge"] {
            assert!(revealed.iter().any(|r| r == expected), "{expected} hidden");
        }
        assert!(
            !revealed.iter().any(|r| r == "cooked_beef"),
            "cobblestone teaches nothing about cooking"
        );
    }

    #[test]
    fn holding_the_output_reveals_its_recipe_too() {
        let (items, book) = setup();
        let mut known = KnownItems::default();
        known.learn(items.find("arrow").unwrap());
        let revealed = named(&items, &book, &known.known_recipes(&book));
        assert!(revealed.iter().any(|r| r == "arrow"));
    }

    #[test]
    fn ids_and_wire_both_round_trip() {
        let (items, _) = setup();
        let mut known = KnownItems::default();
        for id in ["oak_log", "coal", "arrow"] {
            known.learn(items.find(id).unwrap());
        }
        assert_eq!(KnownItems::from_ids(&known.to_ids(&items), &items), known);
        assert_eq!(KnownItems::from_wire(&known.to_wire(), &items), known);
    }

    #[test]
    fn unknown_ids_and_out_of_range_wire_ids_are_dropped() {
        let (items, _) = setup();
        let from_save = KnownItems::from_ids(&["coal".into(), "unobtainium".into()], &items);
        assert_eq!(from_save.items().count(), 1);

        let from_wire = KnownItems::from_wire(&[0, u16::MAX], &items);
        assert_eq!(from_wire.to_wire(), vec![0]);
    }

    #[test]
    fn merging_reports_whether_anything_was_new() {
        let (items, _) = setup();
        let coal = items.find("coal").unwrap();
        let mut a = KnownItems::default();
        let mut b = KnownItems::default();
        b.learn(coal);
        assert!(a.merge(&b));
        assert!(!a.merge(&b));
        assert!(a.knows(coal));
    }
}

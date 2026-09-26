//! Whether a recipe can be made here and now, and making it.
//!
//! A craft is **all or nothing**. It is worked out on a copy of the inventory —
//! ingredients out, output in — and committed only if every crafted item found
//! a slot. So a craft never half-happens, and never drops the result on the
//! floor: a full inventory refuses with [`CraftError::NoRoom`] instead, which
//! the panel can say out loud. Consuming the ingredients first is what lets a
//! craft into a full inventory still succeed when it empties a slot.

use super::Recipe;
use super::station::StationSet;
use crate::domain::inventory::{Inventory, ItemId, ItemRegistry, ItemStack};

/// What stands between the player and a recipe, most fundamental first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Availability {
    /// Can be made, up to `max` times in one go.
    Craftable { max: u32 },
    /// The station it needs is not within reach.
    MissingStation,
    /// Not enough of at least one ingredient.
    MissingIngredients,
    /// Everything is here, but not even one craft's output would fit.
    NoRoom,
}

impl Availability {
    pub fn is_craftable(self) -> bool {
        matches!(self, Self::Craftable { .. })
    }
}

/// Why a craft was refused. The same reasons as [`Availability`], minus the
/// one that says yes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftError {
    MissingStation,
    MissingIngredients,
    NoRoom,
}

/// How many times the ingredients on hand would stretch.
pub fn ingredient_limit(recipe: &Recipe, inventory: &Inventory) -> u32 {
    recipe
        .ingredients
        .iter()
        .map(|&(item, need)| inventory.count_of(item) / need.max(1))
        .min()
        .unwrap_or(0)
}

/// What stands between the player and `recipe`.
pub fn availability(
    recipe: &Recipe,
    inventory: &Inventory,
    items: &ItemRegistry,
    stations: &StationSet,
) -> Availability {
    if !stations.satisfies(recipe.station.as_deref()) {
        return Availability::MissingStation;
    }
    let limit = ingredient_limit(recipe, inventory);
    if limit == 0 {
        return Availability::MissingIngredients;
    }
    if !fits(recipe, inventory, items, 1) {
        return Availability::NoRoom;
    }
    // The largest count that still fits. Room only shrinks as the count grows
    // for any real inventory, so a binary search over it is enough.
    let (mut lo, mut hi) = (1, limit);
    while lo < hi {
        let mid = lo + (hi - lo).div_ceil(2);
        if fits(recipe, inventory, items, mid) {
            lo = mid;
        } else {
            hi = mid - 1;
        }
    }
    Availability::Craftable { max: lo }
}

/// Make `recipe` `times` times, or nothing at all. Returns how many were made.
pub fn craft(
    recipe: &Recipe,
    inventory: &mut Inventory,
    items: &ItemRegistry,
    stations: &StationSet,
    times: u32,
) -> Result<u32, CraftError> {
    let times = times.max(1);
    if !stations.satisfies(recipe.station.as_deref()) {
        return Err(CraftError::MissingStation);
    }
    if ingredient_limit(recipe, inventory) < times {
        return Err(CraftError::MissingIngredients);
    }
    let after = crafted(recipe, inventory, items, times).ok_or(CraftError::NoRoom)?;
    *inventory = after;
    Ok(times)
}

fn fits(recipe: &Recipe, inventory: &Inventory, items: &ItemRegistry, times: u32) -> bool {
    crafted(recipe, inventory, items, times).is_some()
}

/// The inventory as it would be after crafting `times`, or `None` if the
/// output would not all fit. Assumes the ingredients are there.
fn crafted(
    recipe: &Recipe,
    inventory: &Inventory,
    items: &ItemRegistry,
    times: u32,
) -> Option<Inventory> {
    let mut after = inventory.clone();
    for &(item, need) in &recipe.ingredients {
        after.remove(item, need * times);
    }
    let total = recipe.count as u32 * times;
    add_fresh(&mut after, items, recipe.output, total).then_some(after)
}

/// Add `total` newly made `item`s, a stack at a time. Fresh items start at full
/// durability, which is why this builds each stack rather than one big count.
fn add_fresh(inventory: &mut Inventory, items: &ItemRegistry, item: ItemId, total: u32) -> bool {
    let per_stack = items.max_stack(item).max(1) as u32;
    let mut left = total;
    while left > 0 {
        let n = left.min(per_stack) as u8;
        let stack = match items.max_durability(item) {
            Some(durability) => ItemStack {
                count: n,
                ..ItemStack::with_durability(item, durability)
            },
            None => ItemStack::new(item, n),
        };
        if inventory.add(stack, items) > 0 {
            return false;
        }
        left -= n as u32;
    }
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::inventory::TOTAL_SLOTS;
    use crate::domain::inventory::crafting::RecipeBook;
    use crate::domain::world::block::BlockRegistry;

    fn registry() -> ItemRegistry {
        ItemRegistry::from_blocks(&BlockRegistry::with_builtins())
    }

    fn id(items: &ItemRegistry, name: &str) -> ItemId {
        items.find(name).unwrap_or_else(|| panic!("no item {name}"))
    }

    fn recipe(items: &ItemRegistry, output: &str, count: u8, ins: &[(&str, u32)]) -> Recipe {
        Recipe {
            output: id(items, output),
            count,
            ingredients: ins.iter().map(|&(n, c)| (id(items, n), c)).collect(),
            station: None,
        }
    }

    fn hand() -> StationSet {
        StationSet::default()
    }

    /// Fill every storage slot with single, unstackable-with-anything items.
    fn fill_with_tools(inv: &mut Inventory, items: &ItemRegistry) {
        let pick = id(items, "wooden_pickaxe");
        for i in 0..crate::domain::inventory::ARMOR_START {
            inv.set_slot(i, Some(ItemStack::with_durability(pick, 5)));
        }
    }

    #[test]
    fn crafting_consumes_the_ingredients_and_produces_the_output() {
        let items = registry();
        let sticks = recipe(&items, "stick", 4, &[("oak_log", 1)]);
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(id(&items, "oak_log"), 3), &items);

        assert_eq!(craft(&sticks, &mut inv, &items, &hand(), 2), Ok(2));
        assert_eq!(inv.count_of(id(&items, "oak_log")), 1);
        assert_eq!(inv.count_of(id(&items, "stick")), 8);
    }

    #[test]
    fn a_refused_craft_changes_nothing() {
        let items = registry();
        let arrows = recipe(
            &items,
            "arrow",
            4,
            &[("flint", 1), ("stick", 1), ("feather", 1)],
        );
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(id(&items, "flint"), 2), &items);
        inv.add(ItemStack::new(id(&items, "stick"), 2), &items);
        let before = inv.slots().to_vec();

        assert_eq!(
            availability(&arrows, &inv, &items, &hand()),
            Availability::MissingIngredients
        );
        assert_eq!(
            craft(&arrows, &mut inv, &items, &hand(), 1),
            Err(CraftError::MissingIngredients)
        );
        assert_eq!(inv.slots(), &before[..], "nothing was consumed");
    }

    #[test]
    fn a_station_recipe_needs_its_station_in_reach() {
        let items = registry();
        let glass = Recipe {
            station: Some("forge".into()),
            ..recipe(&items, "glass", 1, &[("sand", 1)])
        };
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(id(&items, "sand"), 1), &items);

        assert_eq!(
            availability(&glass, &inv, &items, &hand()),
            Availability::MissingStation
        );
        assert_eq!(
            craft(&glass, &mut inv, &items, &hand(), 1),
            Err(CraftError::MissingStation)
        );

        let forge: StationSet = ["forge"].into_iter().collect();
        assert_eq!(
            availability(&glass, &inv, &items, &forge),
            Availability::Craftable { max: 1 }
        );
        assert_eq!(craft(&glass, &mut inv, &items, &forge, 1), Ok(1));
    }

    #[test]
    fn no_room_refuses_and_leaves_the_inventory_untouched() {
        let items = registry();
        let sticks = recipe(&items, "stick", 4, &[("oak_log", 1)]);
        let mut inv = Inventory::new();
        fill_with_tools(&mut inv, &items);
        // One log in a slot of its own, so crafting frees that slot — but
        // there are two logs, so one craft leaves the slot occupied.
        inv.set_slot(0, Some(ItemStack::new(id(&items, "oak_log"), 2)));
        let before = inv.slots().to_vec();

        assert_eq!(
            availability(&sticks, &inv, &items, &hand()),
            Availability::NoRoom
        );
        assert_eq!(
            craft(&sticks, &mut inv, &items, &hand(), 1),
            Err(CraftError::NoRoom)
        );
        assert_eq!(inv.slots(), &before[..]);
    }

    /// Using up the last of an ingredient frees its slot for the output, so a
    /// full inventory can still craft.
    #[test]
    fn a_full_inventory_crafts_when_the_ingredients_free_a_slot() {
        let items = registry();
        let sticks = recipe(&items, "stick", 4, &[("oak_log", 1)]);
        let mut inv = Inventory::new();
        fill_with_tools(&mut inv, &items);
        inv.set_slot(0, Some(ItemStack::new(id(&items, "oak_log"), 1)));

        assert_eq!(
            availability(&sticks, &inv, &items, &hand()),
            Availability::Craftable { max: 1 }
        );
        assert_eq!(craft(&sticks, &mut inv, &items, &hand(), 1), Ok(1));
        assert_eq!(inv.count_of(id(&items, "stick")), 4);
    }

    /// `max` is whichever runs out first: the ingredients or the room.
    #[test]
    fn max_is_the_smaller_of_the_ingredient_and_room_limits() {
        let items = registry();
        let pick = recipe(
            &items,
            "stone_pickaxe",
            1,
            &[("cobblestone", 3), ("stick", 2)],
        );
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(id(&items, "cobblestone"), 30), &items);
        inv.add(ItemStack::new(id(&items, "stick"), 8), &items);
        // Sticks limit it to 4; plenty of room.
        assert_eq!(
            availability(&pick, &inv, &items, &hand()),
            Availability::Craftable { max: 4 }
        );

        // Now leave only two free slots: pickaxes don't stack, and neither
        // ingredient stack empties, so room caps it at 2.
        let mut tight = Inventory::new();
        fill_with_tools(&mut tight, &items);
        tight.set_slot(0, Some(ItemStack::new(id(&items, "cobblestone"), 30)));
        tight.set_slot(1, Some(ItemStack::new(id(&items, "stick"), 16)));
        tight.set_slot(2, None);
        tight.set_slot(3, None);
        assert_eq!(
            availability(&pick, &tight, &items, &hand()),
            Availability::Craftable { max: 2 }
        );
        assert_eq!(craft(&pick, &mut tight, &items, &hand(), 2), Ok(2));
        assert_eq!(
            craft(&pick, &mut tight, &items, &hand(), 1),
            Err(CraftError::NoRoom)
        );
    }

    #[test]
    fn crafted_tools_start_with_full_durability() {
        let items = registry();
        let pick_id = id(&items, "stone_pickaxe");
        let pick = recipe(
            &items,
            "stone_pickaxe",
            1,
            &[("cobblestone", 3), ("stick", 2)],
        );
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(id(&items, "cobblestone"), 6), &items);
        inv.add(ItemStack::new(id(&items, "stick"), 4), &items);

        craft(&pick, &mut inv, &items, &hand(), 2).expect("crafts");
        let made: Vec<_> = inv.slots()[..TOTAL_SLOTS]
            .iter()
            .flatten()
            .filter(|s| s.item == pick_id)
            .collect();
        assert_eq!(made.len(), 2, "tools do not stack");
        for tool in made {
            assert_eq!(tool.durability, items.max_durability(pick_id));
        }
    }

    /// Every shipped recipe crafts once from exactly its ingredients at every
    /// station — which checks the output count fits a stack and nothing in the
    /// file is uncraftable by construction.
    #[test]
    fn every_shipped_recipe_crafts_from_exactly_its_ingredients() {
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let stations = super::super::station_ids(&blocks);
        let everywhere: StationSet = stations.iter().map(String::as_str).collect();
        let book = RecipeBook::from_toml(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/recipes.toml")),
            &items,
            &stations,
        )
        .unwrap();
        for recipe in book.recipes() {
            let mut inv = Inventory::new();
            for &(item, need) in &recipe.ingredients {
                let mut left = need;
                while left > 0 {
                    let n = left.min(items.max_stack(item) as u32) as u8;
                    inv.add(ItemStack::new(item, n), &items);
                    left -= n as u32;
                }
            }
            let name = &items.get(recipe.output).id;
            assert_eq!(
                craft(recipe, &mut inv, &items, &everywhere, 1),
                Ok(1),
                "{name}"
            );
            assert_eq!(
                inv.count_of(recipe.output),
                recipe.count as u32,
                "{name}: the output did not all land"
            );
        }
    }
}

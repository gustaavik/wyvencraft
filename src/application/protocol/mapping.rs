//! Converting between the in-memory model and the protocol's wire types.

use crate::application::protocol::{Equipment, NetItemStack, RecipeData};
use crate::domain::inventory::crafting::{NamedRecipe, resolve_named};
use crate::domain::inventory::{ARMOR_START, Inventory, ItemRegistry, RecipeBook};

/// What an inventory is wearing and holding, for the wire.
pub fn equipment_of(inventory: &Inventory) -> Equipment {
    Equipment {
        armor: inventory.equipped_armor().map(|slot| slot.map(|id| id.0)),
        held: inventory.selected_stack().map(|stack| stack.item.0),
    }
}

/// The same, read out of a wire inventory snapshot: a client reports its slots
/// and which one it has selected, so the host never has to be told separately
/// what that client is holding.
pub fn equipment_from_slots(slots: &[Option<NetItemStack>], selected: u32) -> Equipment {
    Equipment {
        armor: std::array::from_fn(|i| {
            slots
                .get(ARMOR_START + i)
                .and_then(|slot| slot.map(|s| s.item))
        }),
        held: slots
            .get(selected as usize)
            .and_then(|slot| slot.map(|s| s.item)),
    }
}

/// Convert the local inventory to its wire form for `SyncInventory`.
pub fn inventory_to_wire(inventory: &Inventory) -> (Vec<Option<NetItemStack>>, u32) {
    let slots = inventory
        .slots()
        .iter()
        .map(|slot| {
            slot.map(|stack| NetItemStack {
                item: stack.item.0,
                count: stack.count,
                durability: stack.durability,
            })
        })
        .collect();
    (slots, inventory.selected_index() as u32)
}

/// Serialize the recipe book back to item ids for the `Welcome` message.
pub fn recipes_to_wire(book: &RecipeBook, items: &ItemRegistry) -> Vec<RecipeData> {
    book.recipes()
        .iter()
        .map(|recipe| RecipeData {
            output: items.get(recipe.output).id.clone(),
            count: recipe.count as u32,
            ingredients: recipe
                .ingredients
                .iter()
                .map(|&(item, n)| (items.get(item).id.clone(), n))
                .collect(),
            station: recipe.station.clone(),
        })
        .collect()
}

/// Rebuild a recipe book from a host's wire data. Recipes naming items this
/// build doesn't know are skipped with a warning (mismatched versions).
pub fn recipes_from_wire(
    data: &[RecipeData],
    items: &ItemRegistry,
    stations: &[String],
) -> RecipeBook {
    let resolved = data
        .iter()
        .filter_map(|r| {
            let named = NamedRecipe {
                output: &r.output,
                count: r.count,
                ingredients: &r.ingredients,
                station: r.station.as_deref(),
            };
            resolve_named(&named, items, stations)
        })
        .collect();
    RecipeBook::from_recipes(resolved)
}

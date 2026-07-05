//! Data-driven crafting: shapeless recipes loaded from `assets/recipes.toml`.
//!
//! Recipe files reference items by name; names are resolved against the
//! [`ItemRegistry`] once at load time, so the rest of the game works with plain
//! [`ItemId`]s. A copy of the shipped recipe file is compiled into the binary as
//! a fallback for when the on-disk file is missing or fails to parse.

use std::collections::BTreeMap;

use serde::Deserialize;

use super::inventory::Inventory;
use super::item::{ItemId, ItemRegistry, ItemStack};

/// On-disk recipe file, relative to the working directory (the repo root under
/// `cargo run`). Editable without recompiling; read once at world start.
pub const RECIPES_PATH: &str = "assets/recipes.toml";
/// The shipped recipe file, compiled in as a fallback.
const BUILTIN_RECIPES: &str = include_str!("../../assets/recipes.toml");

/// Raw shape of the TOML file: a list of `[[recipe]]` entries.
#[derive(Deserialize)]
struct RecipeFile {
    #[serde(default)]
    recipe: Vec<RecipeDef>,
}

/// One `[[recipe]]` entry as written by the user, before name resolution.
#[derive(Deserialize)]
struct RecipeDef {
    /// Name of the crafted item.
    output: String,
    /// How many the craft produces (default 1).
    #[serde(default = "default_count")]
    count: u32,
    /// Item name -> count consumed from the inventory.
    ingredients: BTreeMap<String, u32>,
}

fn default_count() -> u32 {
    1
}

/// A validated, name-resolved shapeless recipe.
pub struct Recipe {
    pub output: ItemId,
    pub count: u8,
    pub ingredients: Vec<(ItemId, u32)>,
}

impl Recipe {
    /// Whether `inventory` holds every ingredient in sufficient quantity.
    pub fn can_craft(&self, inventory: &Inventory) -> bool {
        self.ingredients
            .iter()
            .all(|&(item, count)| inventory.count_of(item) >= count)
    }

    /// Consume the ingredients from `inventory` and return the crafted stack
    /// (with full durability for tools), or `None` if ingredients are missing.
    /// The caller decides where the output goes (inventory, ground, ...).
    pub fn craft(&self, inventory: &mut Inventory, items: &ItemRegistry) -> Option<ItemStack> {
        if !self.can_craft(inventory) {
            return None;
        }
        for &(item, count) in &self.ingredients {
            inventory.remove(item, count);
        }
        Some(ItemStack {
            item: self.output,
            count: self.count,
            durability: items.max_durability(self.output),
        })
    }
}

/// All loaded recipes, in file order.
pub struct RecipeBook {
    recipes: Vec<Recipe>,
}

impl RecipeBook {
    /// Load `assets/recipes.toml`, falling back to the compiled-in copy if the
    /// file is missing or unparseable. Never fails.
    pub fn load(items: &ItemRegistry) -> Self {
        match std::fs::read_to_string(RECIPES_PATH) {
            Ok(text) => match Self::from_toml(&text, items) {
                Ok(book) => {
                    log::info!("loaded {} recipes from {RECIPES_PATH}", book.recipes.len());
                    return book;
                }
                Err(err) => {
                    log::warn!("failed to parse {RECIPES_PATH}: {err}; using built-in recipes");
                }
            },
            Err(err) => {
                log::info!("could not read {RECIPES_PATH} ({err}); using built-in recipes");
            }
        }
        Self::from_toml(BUILTIN_RECIPES, items).expect("built-in recipe file parses")
    }

    /// Parse recipes from TOML text, resolving item names against the registry.
    /// Entries naming unknown items or with zero counts are skipped with a
    /// warning; a malformed file is an error.
    pub fn from_toml(text: &str, items: &ItemRegistry) -> Result<Self, toml::de::Error> {
        let file: RecipeFile = toml::from_str(text)?;
        let mut recipes = Vec::new();
        for def in file.recipe {
            if let Some(recipe) = resolve(def, items) {
                recipes.push(recipe);
            }
        }
        Ok(Self { recipes })
    }

    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    pub fn get(&self, index: usize) -> Option<&Recipe> {
        self.recipes.get(index)
    }
}

/// Validate one raw recipe and resolve its item names, or drop it with a warning.
fn resolve(def: RecipeDef, items: &ItemRegistry) -> Option<Recipe> {
    let skip = |why: String| {
        log::warn!("skipping recipe for {:?}: {why}", def.output);
        None::<Recipe>
    };
    let Some(output) = items.find(&def.output) else {
        return skip(format!("unknown output item {:?}", def.output));
    };
    if def.count == 0 || def.count > u8::MAX as u32 {
        return skip(format!("output count {} out of range 1-255", def.count));
    }
    if def.ingredients.is_empty() {
        return skip("no ingredients".to_string());
    }
    let mut ingredients = Vec::with_capacity(def.ingredients.len());
    for (name, count) in &def.ingredients {
        let Some(item) = items.find(name) else {
            return skip(format!("unknown ingredient {name:?}"));
        };
        if *count == 0 {
            return skip(format!("ingredient {name:?} has count 0"));
        }
        ingredients.push((item, *count));
    }
    Some(Recipe {
        output,
        count: def.count as u8,
        ingredients,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockRegistry;

    fn registry() -> ItemRegistry {
        ItemRegistry::from_blocks(&BlockRegistry::with_builtins())
    }

    #[test]
    fn builtin_recipe_file_parses_and_fully_resolves() {
        let items = registry();
        let book = RecipeBook::from_toml(BUILTIN_RECIPES, &items).expect("valid TOML");
        let defs: RecipeFile = toml::from_str(BUILTIN_RECIPES).expect("valid TOML");
        assert_eq!(
            book.recipes().len(),
            defs.recipe.len(),
            "every shipped recipe must name real items"
        );
        assert!(!book.recipes().is_empty());
    }

    #[test]
    fn unknown_items_and_bad_counts_skip_only_that_recipe() {
        let items = registry();
        let text = r#"
            [[recipe]]
            output = "plutonium"
            ingredients = { stone = 1 }

            [[recipe]]
            output = "glass"
            ingredients = { unobtainium = 2 }

            [[recipe]]
            output = "glass"
            count = 0
            ingredients = { sand = 1 }

            [[recipe]]
            output = "glass"
            ingredients = { sand = 1 }
        "#;
        let book = RecipeBook::from_toml(text, &items).expect("valid TOML");
        assert_eq!(book.recipes().len(), 1, "only the last recipe is valid");
        assert_eq!(book.get(0).unwrap().output, items.find("glass").unwrap());
    }

    #[test]
    fn crafting_consumes_ingredients_and_produces_the_output() {
        let items = registry();
        let text = r#"
            [[recipe]]
            output = "glass"
            count = 2
            ingredients = { sand = 3, "coal ore" = 1 }
        "#;
        let book = RecipeBook::from_toml(text, &items).expect("valid TOML");
        let recipe = book.get(0).unwrap();

        let sand = items.find("sand").unwrap();
        let coal = items.find("coal ore").unwrap();
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(sand, 4), &items);

        assert!(!recipe.can_craft(&inv), "missing the coal ore");
        assert!(recipe.craft(&mut inv, &items).is_none());
        assert_eq!(inv.count_of(sand), 4, "failed craft consumes nothing");

        inv.add(ItemStack::new(coal, 1), &items);
        let out = recipe.craft(&mut inv, &items).expect("craftable now");
        assert_eq!(out.item, items.find("glass").unwrap());
        assert_eq!(out.count, 2);
        assert_eq!(inv.count_of(sand), 1);
        assert_eq!(inv.count_of(coal), 0);
    }

    #[test]
    fn crafted_tools_start_with_full_durability() {
        let items = registry();
        let text = r#"
            [[recipe]]
            output = "wooden pickaxe"
            ingredients = { wood = 3 }
        "#;
        let book = RecipeBook::from_toml(text, &items).expect("valid TOML");
        let mut inv = Inventory::new();
        inv.add(ItemStack::new(items.find("wood").unwrap(), 3), &items);

        let out = book.get(0).unwrap().craft(&mut inv, &items).expect("crafts");
        assert_eq!(out.item, items.wooden_pickaxe);
        assert_eq!(out.durability, items.max_durability(items.wooden_pickaxe));
    }
}

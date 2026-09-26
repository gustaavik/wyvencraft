//! Data-driven crafting: shapeless recipes loaded from `assets/recipes.toml`.
//!
//! Recipe files reference items by name; names are resolved against the
//! [`ItemRegistry`] once at load time, so the rest of the game works with plain
//! [`ItemId`]s. A copy of the shipped recipe file is compiled into the binary as
//! a fallback for when the on-disk file is missing or fails to parse.
//!
//! The rest of crafting is pure and lives beside the loader:
//!
//! - [`craft`] — whether a recipe can be made here and now, and making it.
//! - [`station`] — which crafting stations stand within reach.
//! - [`discovery`] — which recipes a player has learned, Valheim-style: a
//!   recipe is known once you have held any of its ingredients.

pub mod craft;
pub mod discovery;
pub mod station;

use std::collections::BTreeMap;

use serde::Deserialize;

use super::item::{ItemId, ItemRegistry};

pub use craft::{Availability, CraftError, availability, craft};
pub use discovery::KnownItems;
pub use station::{STATION_RADIUS, StationSet, station_ids, stations_near};

/// On-disk recipe file, relative to the working directory (the repo root under
/// `cargo run`). Editable without recompiling; read once at world start.
pub const RECIPES_PATH: &str = "assets/recipes.toml";
/// The shipped recipe file, compiled in as a fallback.
const BUILTIN_RECIPES: &str = include_str!("../../../assets/recipes.toml");

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
    /// The crafting station (a block's `station`) that must be within reach;
    /// omitted means it is made by hand, anywhere.
    station: Option<String>,
}

fn default_count() -> u32 {
    1
}

/// A validated, name-resolved shapeless recipe.
#[derive(Debug, Clone, PartialEq)]
pub struct Recipe {
    pub output: ItemId,
    pub count: u8,
    pub ingredients: Vec<(ItemId, u32)>,
    /// The station that must be within reach, or `None` for a hand recipe.
    pub station: Option<String>,
}

/// All loaded recipes, in file order.
pub struct RecipeBook {
    recipes: Vec<Recipe>,
}

impl RecipeBook {
    /// Load `assets/recipes.toml`, falling back to the compiled-in copy if the
    /// file is missing or unparseable. Never fails.
    ///
    /// `stations` are the station names the loaded blocks declare
    /// ([`station_ids`]); a recipe naming any other is skipped.
    pub fn load(items: &ItemRegistry, stations: &[String]) -> Self {
        match std::fs::read_to_string(RECIPES_PATH) {
            Ok(text) => match Self::from_toml(&text, items, stations) {
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
        Self::from_toml(BUILTIN_RECIPES, items, stations).expect("built-in recipe file parses")
    }

    /// Parse recipes from TOML text, resolving item names against the registry.
    /// Entries naming unknown items or with zero counts are skipped with a
    /// warning; a malformed file is an error.
    pub fn from_toml(
        text: &str,
        items: &ItemRegistry,
        stations: &[String],
    ) -> Result<Self, toml::de::Error> {
        let file: RecipeFile = toml::from_str(text)?;
        let mut recipes = Vec::new();
        for def in file.recipe {
            let ingredients: Vec<(String, u32)> = def.ingredients.into_iter().collect();
            let named = NamedRecipe {
                output: &def.output,
                count: def.count,
                ingredients: &ingredients,
                station: def.station.as_deref(),
            };
            if let Some(recipe) = resolve_named(&named, items, stations) {
                recipes.push(recipe);
            }
        }
        Ok(Self { recipes })
    }

    /// A book from already-resolved recipes (e.g. synced from a multiplayer host).
    pub fn from_recipes(recipes: Vec<Recipe>) -> Self {
        Self { recipes }
    }

    pub fn recipes(&self) -> &[Recipe] {
        &self.recipes
    }

    pub fn get(&self, index: usize) -> Option<&Recipe> {
        self.recipes.get(index)
    }
}

/// One recipe spelled by names, as the file and the wire both carry it.
pub struct NamedRecipe<'a> {
    pub output: &'a str,
    pub count: u32,
    pub ingredients: &'a [(String, u32)],
    pub station: Option<&'a str>,
}

/// Validate one name-based recipe and resolve it against the registry, or drop
/// it with a warning. Both the TOML entries and recipes received from a
/// multiplayer host funnel through here.
pub fn resolve_named(
    named: &NamedRecipe<'_>,
    items: &ItemRegistry,
    stations: &[String],
) -> Option<Recipe> {
    let &NamedRecipe {
        output,
        count,
        ingredients,
        station,
    } = named;
    let skip = |why: String| {
        log::warn!("skipping recipe for {output:?}: {why}");
        None::<Recipe>
    };
    let Some(output_id) = items.find(output) else {
        return skip(format!("unknown output item {output:?}"));
    };
    if count == 0 || count > u8::MAX as u32 {
        return skip(format!("output count {count} out of range 1-255"));
    }
    if ingredients.is_empty() {
        return skip("no ingredients".to_string());
    }
    if let Some(station) = station
        && !stations.iter().any(|s| s == station)
    {
        return skip(format!("no block is the station {station:?}"));
    }
    let mut resolved = Vec::with_capacity(ingredients.len());
    for (name, n) in ingredients {
        let Some(item) = items.find(name) else {
            return skip(format!("unknown ingredient {name:?}"));
        };
        if *n == 0 {
            return skip(format!("ingredient {name:?} has count 0"));
        }
        resolved.push((item, *n));
    }
    Some(Recipe {
        output: output_id,
        count: count as u8,
        ingredients: resolved,
        station: station.map(str::to_string),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockRegistry;

    fn registry() -> ItemRegistry {
        ItemRegistry::from_blocks(&BlockRegistry::with_builtins())
    }

    fn stations() -> Vec<String> {
        station_ids(&BlockRegistry::with_builtins())
    }

    #[test]
    fn builtin_recipe_file_parses_and_fully_resolves() {
        let items = registry();
        let book = RecipeBook::from_toml(BUILTIN_RECIPES, &items, &stations()).expect("valid TOML");
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
        let book = RecipeBook::from_toml(text, &items, &stations()).expect("valid TOML");
        assert_eq!(book.recipes().len(), 1, "only the last recipe is valid");
        assert_eq!(book.recipes()[0].output, items.find("glass").unwrap());
    }

    #[test]
    fn a_recipe_naming_an_unknown_station_is_skipped_alone() {
        let items = registry();
        let text = r#"
            [[recipe]]
            output = "glass"
            ingredients = { sand = 1 }
            station = "alchemy_table"

            [[recipe]]
            output = "glass"
            ingredients = { sand = 1 }
            station = "forge"
        "#;
        let book = RecipeBook::from_toml(text, &items, &stations()).expect("valid TOML");
        assert_eq!(book.recipes().len(), 1);
        assert_eq!(book.recipes()[0].station.as_deref(), Some("forge"));
    }

    /// The whole point of stations: the shipped file uses both, and leaves the
    /// first rung — a workbench out of logs — craftable by hand.
    #[test]
    fn the_shipped_book_uses_both_stations_and_the_workbench_is_hand_made() {
        let items = registry();
        let book = RecipeBook::from_toml(BUILTIN_RECIPES, &items, &stations()).unwrap();
        let at = |s: &str| {
            book.recipes()
                .iter()
                .any(|r| r.station.as_deref() == Some(s))
        };
        assert!(at("workbench") && at("forge"));
        let workbench = items.find("workbench").expect("the block has an item");
        let recipe = book
            .recipes()
            .iter()
            .find(|r| r.output == workbench)
            .expect("a workbench recipe");
        assert_eq!(recipe.station, None, "the first station must be hand-made");
    }
}

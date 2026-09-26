//! Reading the recipe book from the install's `assets/` directory.

use crate::domain::inventory::ItemRegistry;
use crate::domain::inventory::crafting::{BUILTIN_RECIPES, RECIPES_PATH, RecipeBook};

/// Load `assets/recipes.toml`, falling back to the compiled-in copy if the
/// file is missing or unparseable. Never fails.
///
/// `stations` are the station names the loaded blocks declare
/// ([`crate::domain::inventory::crafting::station_ids`]); a recipe naming any
/// other is skipped.
pub fn load_recipe_book(items: &ItemRegistry, stations: &[String]) -> RecipeBook {
    match std::fs::read_to_string(RECIPES_PATH) {
        Ok(text) => match RecipeBook::from_toml(&text, items, stations) {
            Ok(book) => {
                log::info!(
                    "loaded {} recipes from {RECIPES_PATH}",
                    book.recipes().len()
                );
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
    RecipeBook::from_toml(BUILTIN_RECIPES, items, stations).expect("built-in recipe file parses")
}

//! Inventory data model: items, stacks, and the player container/hotbar.
//! Rendering of these lives separately in [`crate::ui`].

pub mod crafting;
pub mod held_label;
#[allow(clippy::module_inception)]
pub mod inventory;
pub mod item;
pub mod mining;

pub use crafting::{Recipe, RecipeBook};
pub use held_label::HeldLabel;
pub use inventory::{ARMOR_SIZE, ARMOR_START, HOTBAR_SIZE, INVENTORY_SIZE, Inventory, TOTAL_SLOTS};
pub use item::{ArmorSlot, ArmorSpec, FoodValue, Item, ItemId, ItemRegistry, ItemStack, ToolSpec};
pub use mining::break_seconds;

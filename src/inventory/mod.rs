//! Inventory data model: items, stacks, and the player container/hotbar.
//! Rendering of these lives separately in [`crate::ui`].

pub mod component;
pub mod crafting;
pub mod held_label;
#[allow(clippy::module_inception)]
pub mod inventory;
pub mod item;
pub mod mining;

pub use component::{ArmorSlot, Consumable, Equippable, ItemComponent, Placeable, Tool};
pub use crafting::{Recipe, RecipeBook};
pub use held_label::HeldLabel;
pub use inventory::{ARMOR_SIZE, ARMOR_START, HOTBAR_SIZE, INVENTORY_SIZE, Inventory, TOTAL_SLOTS};
pub use item::{Item, ItemId, ItemRegistry, ItemStack};
pub use mining::break_seconds;

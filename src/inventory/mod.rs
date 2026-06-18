//! Inventory data model: items, stacks, and the player container/hotbar.
//! Rendering of these lives separately in [`crate::ui`].

#[allow(clippy::module_inception)]
pub mod inventory;
pub mod item;
pub mod mining;

pub use inventory::{HOTBAR_SIZE, INVENTORY_SIZE, Inventory};
pub use item::{FoodValue, Item, ItemId, ItemRegistry, ItemStack, ToolKind};
pub use mining::break_seconds;

//! Inventory data model: items, stacks, and the player container/hotbar.
//! Rendering of these lives separately in [`crate::ui`].

#[allow(clippy::module_inception)]
pub mod inventory;
pub mod item;

pub use inventory::{Inventory, HOTBAR_SIZE, INVENTORY_SIZE};
pub use item::{Item, ItemId, ItemRegistry, ItemStack};

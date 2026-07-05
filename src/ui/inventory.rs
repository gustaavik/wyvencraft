//! The inventory screen: a 9x4 slot grid (main + hotbar) drawn with egui, plus a
//! creative item palette when in creative mode and a crafting panel listing the
//! recipes from the [`RecipeBook`].
//!
//! Interaction is click-to-move (Minecraft-style): the caller owns a "held"
//! stack; this view reports the slot clicked (or palette item picked, or recipe
//! crafted) and renders the grid + held label. The move/craft logic lives with
//! the inventory owner.

use egui::{Color32, Context};

use crate::core::GameMode;
use crate::inventory::{
    HOTBAR_SIZE, INVENTORY_SIZE, Inventory, ItemId, ItemRegistry, ItemStack, RecipeBook,
};

const SLOT: f32 = 44.0;

/// What the player did in the inventory screen this frame.
pub enum InvAction {
    /// Clicked an inventory slot (pick up / place / merge / swap).
    Slot(usize),
    /// Picked an item from the creative palette (grab a full stack).
    Pick(ItemId),
    /// Clicked a recipe's craft button (index into the recipe book).
    Craft(usize),
}

fn slot_label(stack: Option<ItemStack>, items: &ItemRegistry) -> String {
    match stack {
        Some(s) => {
            let name: String = items.get(s.item).name.chars().take(3).collect();
            format!("{name}\n{}", s.count)
        }
        None => String::new(),
    }
}

/// Draw the inventory window. Returns the action taken this frame, if any.
pub fn draw_inventory(
    ctx: &Context,
    inventory: &Inventory,
    items: &ItemRegistry,
    recipes: &RecipeBook,
    held: Option<ItemStack>,
    mode: GameMode,
) -> Option<InvAction> {
    let mut action = None;

    egui::Window::new("Inventory")
        .collapsible(false)
        .resizable(false)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            // Creative palette: every item, click to grab a full stack.
            if mode.is_creative() {
                ui.label(egui::RichText::new("Creative palette").strong());
                egui::ScrollArea::vertical()
                    .id_salt("palette")
                    .max_height(170.0)
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            for (id, item) in items.iter() {
                                let label: String = item.name.chars().take(3).collect();
                                let button =
                                    egui::Button::new(label).min_size(egui::vec2(SLOT, SLOT));
                                if ui.add(button).on_hover_text(&item.name).clicked() {
                                    action = Some(InvAction::Pick(id));
                                }
                            }
                        });
                    });
                ui.separator();
            }

            // Crafting panel: one row per recipe, craftable ones enabled.
            if !recipes.recipes().is_empty() {
                ui.label(egui::RichText::new("Crafting").strong());
                egui::ScrollArea::vertical()
                    .id_salt("crafting")
                    .max_height(150.0)
                    .show(ui, |ui| {
                        for (index, recipe) in recipes.recipes().iter().enumerate() {
                            let craftable = recipe.can_craft(inventory);
                            ui.horizontal(|ui| {
                                let button =
                                    egui::Button::new("Craft").min_size(egui::vec2(64.0, 24.0));
                                if ui.add_enabled(craftable, button).clicked() {
                                    action = Some(InvAction::Craft(index));
                                }
                                let output = &items.get(recipe.output).name;
                                let needs: Vec<String> = recipe
                                    .ingredients
                                    .iter()
                                    .map(|&(item, n)| format!("{n} {}", items.get(item).name))
                                    .collect();
                                let label = if recipe.count > 1 {
                                    format!("{}x {output} — {}", recipe.count, needs.join(", "))
                                } else {
                                    format!("{output} — {}", needs.join(", "))
                                };
                                let color = if craftable {
                                    Color32::WHITE
                                } else {
                                    Color32::GRAY
                                };
                                ui.label(egui::RichText::new(label).color(color));
                            });
                        }
                    });
                ui.separator();
            }

            let mut slot_button = |ui: &mut egui::Ui, index: usize| {
                let label = slot_label(inventory.slot(index), items);
                let selected = index < HOTBAR_SIZE && index == inventory.selected_index();
                let mut button = egui::Button::new(label).min_size(egui::vec2(SLOT, SLOT));
                if selected {
                    button = button.stroke(egui::Stroke::new(2.0, Color32::WHITE));
                }
                if ui.add(button).clicked() {
                    action = Some(InvAction::Slot(index));
                }
            };

            // Main inventory grid: slots HOTBAR_SIZE..INVENTORY_SIZE (3 rows of 9).
            let mut index = HOTBAR_SIZE;
            while index < INVENTORY_SIZE {
                ui.horizontal(|ui| {
                    for _ in 0..HOTBAR_SIZE {
                        if index < INVENTORY_SIZE {
                            slot_button(ui, index);
                            index += 1;
                        }
                    }
                });
            }

            ui.add_space(10.0);

            // Hotbar row: slots 0..HOTBAR_SIZE.
            ui.horizontal(|ui| {
                for i in 0..HOTBAR_SIZE {
                    slot_button(ui, i);
                }
            });

            ui.add_space(6.0);
            let holding = match held {
                Some(s) => format!("Holding: {} × {}", items.get(s.item).name, s.count),
                None => "Holding: —".to_string(),
            };
            ui.label(holding);
            ui.label(
                egui::RichText::new("Click a slot to pick up / place. E or Esc to close.")
                    .small()
                    .color(Color32::GRAY),
            );
        });

    action
}

//! The inventory screen: armor slots and a live player preview down the left,
//! a crafting list (or the creative item palette) on the right, and the storage
//! grid + hotbar across the bottom — matching `inventory overhaul.png`.
//!
//! Interaction is click-to-move (Minecraft-style): the caller owns a "held"
//! stack; this view reports the slot clicked (or palette item picked, recipe
//! crafted, or preview dragged) and paints the grid and the held stack under
//! the cursor. The move/craft logic lives with the inventory owner.

use egui::{Align2, Color32, Context, FontId, Rect, Sense, Stroke, StrokeKind, pos2, vec2};

use crate::content::ItemIcon;
use crate::core::GameMode;
use crate::inventory::{
    ARMOR_START, ArmorSlot, HOTBAR_SIZE, INVENTORY_SIZE, Inventory, ItemId, ItemRegistry,
    ItemStack, RecipeBook,
};
use crate::state::UiTextures;
use crate::ui::icon::draw_item_icon;

const SLOT: f32 = 46.0;
const PAD: f32 = 5.0;
const GAP: f32 = 14.0;
/// Width of the right-hand panel (crafting list / creative palette).
const SIDE_PANEL_W: f32 = 360.0;

// Light-grey panel palette, matching the mockup regardless of egui's theme.
const PANEL_BG: Color32 = Color32::from_rgb(196, 196, 196);
const SLOT_BG: Color32 = Color32::from_rgb(122, 122, 122);
const SLOT_HOVER: Color32 = Color32::from_rgb(150, 150, 152);
const SLOT_STROKE: Color32 = Color32::from_rgb(92, 92, 92);
const DISABLED_BG: Color32 = Color32::from_rgb(104, 104, 104);
const TEXT: Color32 = Color32::from_rgb(40, 40, 42);
const DISABLED_TEXT: Color32 = Color32::from_rgb(96, 96, 98);

/// What the player did in the inventory screen this frame.
pub enum InvAction {
    /// Clicked an inventory slot (pick up / place / merge / swap).
    Slot(usize),
    /// Picked an item from the creative palette (grab a full stack).
    Pick(ItemId),
    /// Clicked a recipe's craft button (index into the recipe book).
    Craft(usize),
    /// Dragged across the player preview by this many pixels horizontally.
    Rotate(f32),
}

/// The inventory screen's output for a frame: the discrete action (if any) plus
/// where the preview model's head should look — the cursor position mapped to a
/// head (yaw, pitch), so the model looks toward the cursor without touching the
/// world player's facing.
pub struct InvOutput {
    pub action: Option<InvAction>,
    pub head_look: Option<(f32, f32)>,
}

/// Maximum head turn / tilt for the cursor-tracking preview (radians).
const HEAD_LOOK_MAX_YAW: f32 = 0.7;
const HEAD_LOOK_MAX_PITCH: f32 = 0.5;

/// Everything the view needs to draw a slot's contents, bundled so the many
/// slot calls don't each take a fistful of arguments.
struct View<'a> {
    inventory: &'a Inventory,
    items: &'a ItemRegistry,
    icons: &'a [ItemIcon],
    tex: UiTextures,
}

/// Draw the inventory window. Returns the frame's action and head-look.
#[allow(clippy::too_many_arguments)]
pub fn draw_inventory(
    ctx: &Context,
    inventory: &Inventory,
    items: &ItemRegistry,
    recipes: &RecipeBook,
    icons: &[ItemIcon],
    held: Option<ItemStack>,
    mode: GameMode,
    tex: UiTextures,
) -> InvOutput {
    let view = View {
        inventory,
        items,
        icons,
        tex,
    };
    let frame = egui::Frame::new()
        .fill(PANEL_BG)
        .inner_margin(14.0)
        .corner_radius(6.0);

    // Set by the preview box each frame to where the head should look; a `Cell`
    // so the layout closures can write it without a mutable-capture tangle.
    let head_look = std::cell::Cell::new(None);

    let inner = egui::Window::new("Inventory")
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .show(ctx, |ui| {
            let mut action = None;

            // --- Top row: armor column, player preview, right-hand panel. ---
            let top = ui
                .horizontal_top(|ui| {
                    let mut a = view.armor_column(ui);
                    ui.add_space(GAP);
                    let (pv_action, pv_rect) = preview_box(ui, tex.preview);
                    // Head tracks the cursor relative to the preview box centre.
                    if let Some(cursor) = ui.ctx().pointer_latest_pos() {
                        head_look.set(Some(look_angles(pv_rect, cursor)));
                    }
                    a = a.or(pv_action);
                    ui.add_space(GAP);
                    a.or(view.side_panel(ui, recipes, mode))
                })
                .inner;
            action = action.or(top);

            ui.add_space(12.0);

            // --- Storage grid: three rows of nine. ---
            let mut index = HOTBAR_SIZE;
            while index < INVENTORY_SIZE {
                let row = ui
                    .horizontal(|ui| {
                        let mut a = None;
                        for _ in 0..HOTBAR_SIZE {
                            if index < INVENTORY_SIZE {
                                a = a.or(view.slot(ui, index, None, false));
                                index += 1;
                            }
                        }
                        a
                    })
                    .inner;
                action = action.or(row);
            }

            ui.add_space(10.0);

            // --- Hotbar row: the selected slot gets a white outline. ---
            let hotbar = ui
                .horizontal(|ui| {
                    let mut a = None;
                    for i in 0..HOTBAR_SIZE {
                        let selected = i == inventory.selected_index();
                        a = a.or(view.slot(ui, i, None, selected));
                    }
                    a
                })
                .inner;
            action.or(hotbar)
        });
    // `show` → Option (window open) of InnerResponse whose `.inner` is itself
    // Option (the body ran / wasn't collapsed); flatten both to the action.
    let action = inner.and_then(|r| r.inner).flatten();

    // The held stack follows the cursor, painted on a foreground layer so it
    // rides above the window.
    if let Some(stack) = held
        && let Some(pos) = ctx.pointer_latest_pos()
    {
        let painter = ctx.layer_painter(egui::LayerId::new(
            egui::Order::Foreground,
            egui::Id::new("held_item"),
        ));
        let rect = Rect::from_center_size(pos, vec2(40.0, 40.0));
        draw_item_icon(&painter, rect, view.icon_of(stack.item), tex);
        paint_count(&painter, rect, stack.count);
    }

    InvOutput {
        action,
        head_look: head_look.get(),
    }
}

/// Map a cursor position to head (yaw, pitch), relative to the preview box's
/// centre. The model faces the viewer, so a rightward cursor turns the head to
/// screen-right and a downward cursor tilts it down; both clamp to a natural range.
fn look_angles(box_rect: Rect, cursor: egui::Pos2) -> (f32, f32) {
    let d = cursor - box_rect.center();
    let nx = (d.x / (box_rect.width() * 0.5)).clamp(-1.0, 1.0);
    let ny = (d.y / (box_rect.height() * 0.5)).clamp(-1.0, 1.0);
    (-nx * HEAD_LOOK_MAX_YAW, -ny * HEAD_LOOK_MAX_PITCH)
}

/// The offscreen player-model image; dragging it rotates the preview. Returns
/// the drag action (if any) and the box rect (for cursor head-tracking).
fn preview_box(ui: &mut egui::Ui, preview: egui::TextureId) -> (Option<InvAction>, Rect) {
    let height = 6.0 * (SLOT + PAD) - PAD; // matches the armor column height
    let width = height * 0.48; // PREVIEW_SIZE aspect (see app.rs)
    let (rect, resp) = ui.allocate_exact_size(vec2(width, height), Sense::drag());
    let painter = ui.painter();
    painter.rect_filled(rect, 4.0, Color32::from_rgb(8, 8, 10));
    painter.image(
        preview,
        rect,
        Rect::from_min_max(pos2(0.0, 0.0), pos2(1.0, 1.0)),
        Color32::WHITE,
    );
    painter.rect_stroke(
        rect,
        4.0,
        Stroke::new(2.0_f32, SLOT_STROKE),
        StrokeKind::Inside,
    );
    let action = (resp.dragged() && resp.drag_delta().x != 0.0)
        .then(|| InvAction::Rotate(resp.drag_delta().x));
    (action, rect)
}

impl View<'_> {
    fn icon_of(&self, item: ItemId) -> ItemIcon {
        self.icons[item.0 as usize]
    }

    /// One inventory slot: background, optional ghost hint, item icon, count and
    /// durability. Returns the click action, if any.
    fn slot(
        &self,
        ui: &mut egui::Ui,
        index: usize,
        ghost: Option<u32>,
        selected: bool,
    ) -> Option<InvAction> {
        let (rect, resp) = ui.allocate_exact_size(vec2(SLOT, SLOT), Sense::click());
        let painter = ui.painter();
        let bg = if resp.hovered() { SLOT_HOVER } else { SLOT_BG };
        painter.rect_filled(rect, 3.0, bg);

        let inner = rect.shrink(5.0);
        match self.inventory.slot(index) {
            Some(stack) => {
                draw_item_icon(painter, inner, self.icon_of(stack.item), self.tex);
                paint_count(painter, rect, stack.count);
                self.paint_durability(painter, rect, stack);
            }
            None => {
                if let Some(tile) = ghost {
                    // Faded silhouette hinting what the slot holds.
                    draw_item_icon(painter, inner, ItemIcon::Flat(tile), self.tex);
                    painter.rect_filled(
                        inner,
                        0.0,
                        Color32::from_rgba_unmultiplied(SLOT_BG.r(), SLOT_BG.g(), SLOT_BG.b(), 150),
                    );
                }
            }
        }

        let stroke = if selected {
            Stroke::new(2.5_f32, Color32::WHITE)
        } else {
            Stroke::new(1.0_f32, SLOT_STROKE)
        };
        painter.rect_stroke(rect, 3.0, stroke, StrokeKind::Inside);

        resp.clicked().then_some(InvAction::Slot(index))
    }

    /// A tool/armor durability bar along the bottom of a slot.
    fn paint_durability(&self, painter: &egui::Painter, cell: Rect, stack: ItemStack) {
        let (Some(dur), Some(max)) = (stack.durability, self.items.max_durability(stack.item))
        else {
            return;
        };
        if max == 0 || dur >= max {
            return;
        }
        let ratio = dur as f32 / max as f32;
        let bar_w = SLOT - 10.0;
        let track = Rect::from_min_size(
            pos2(cell.left() + 5.0, cell.bottom() - 8.0),
            vec2(bar_w, 4.0),
        );
        painter.rect_filled(track, 1.0, Color32::from_black_alpha(160));
        let fill = Rect::from_min_size(track.min, vec2(bar_w * ratio, 4.0));
        let r = ((1.0 - ratio) * 255.0) as u8;
        let g = (ratio * 220.0) as u8;
        painter.rect_filled(fill, 1.0, Color32::from_rgb(r.max(40), g, 40));
    }

    /// The left column: one labelled slot per armor piece, empties showing a
    /// faded ghost of what fits.
    fn armor_column(&self, ui: &mut egui::Ui) -> Option<InvAction> {
        ui.vertical(|ui| {
            let mut a = None;
            for piece in ArmorSlot::ALL {
                let index = ARMOR_START + piece.index();
                let ghost = self.armor_ghost(piece);
                let row = ui
                    .horizontal(|ui| {
                        // `horizontal` centres cross-axis, so the label sits
                        // vertically centred against the taller slot.
                        let clicked = self.slot(ui, index, ghost, false);
                        ui.add_space(6.0);
                        // Fixed-width label cell so the column edge stays straight.
                        ui.allocate_ui(vec2(84.0, SLOT), |ui| {
                            ui.label(egui::RichText::new(piece.label()).size(15.0).color(TEXT));
                        });
                        clicked
                    })
                    .inner;
                a = a.or(row);
                ui.add_space(PAD);
            }
            a
        })
        .inner
    }

    /// The icon tile of the armor item that fits `piece`, for the ghost hint.
    fn armor_ghost(&self, piece: ArmorSlot) -> Option<u32> {
        self.items.iter().find_map(|(id, item)| {
            let fits = item.armor.map(|a| a.slot) == Some(piece);
            match (fits, self.icon_of(id)) {
                (true, ItemIcon::Flat(tile)) => Some(tile),
                (true, ItemIcon::Cube { top, .. }) => Some(top),
                _ => None,
            }
        })
    }

    /// The right-hand panel: the crafting list in survival, the item palette in
    /// creative (crafting is meaningless there, so the panel is never wasted).
    fn side_panel(
        &self,
        ui: &mut egui::Ui,
        recipes: &RecipeBook,
        mode: GameMode,
    ) -> Option<InvAction> {
        let panel_h = 6.0 * (SLOT + PAD) - PAD;
        ui.allocate_ui(vec2(SIDE_PANEL_W, panel_h), |ui| {
            // `allocate_ui` reserves space but does not clip; clamp everything
            // drawn here to the panel box so a wide row or an overscrolled icon
            // can't spill into the rest of the window.
            ui.set_clip_rect(ui.max_rect());
            let header = if mode.is_creative() {
                "Items"
            } else {
                "Crafting"
            };
            ui.label(egui::RichText::new(header).strong().color(TEXT));
            // Fixed inner extents (below the header, minus the scrollbar) so the
            // scroll list can't stretch the auto-sized window or push past the
            // panel edge — the regression this replaced relied on `allocate_ui`
            // bounding the scroll area, which it doesn't during layout passes.
            let inner_w = SIDE_PANEL_W - 16.0;
            let inner_h = panel_h - 24.0;
            if mode.is_creative() {
                self.palette(ui, inner_w, inner_h)
            } else {
                self.crafting_list(ui, recipes, inner_w, inner_h)
            }
        })
        .inner
    }

    /// Creative palette: every item as a click-to-grab icon, wrapping to fit.
    fn palette(&self, ui: &mut egui::Ui, width: f32, height: f32) -> Option<InvAction> {
        egui::ScrollArea::vertical()
            .id_salt("palette")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                ui.horizontal_wrapped(|ui| {
                    let mut a = None;
                    for (id, item) in self.items.iter() {
                        let (rect, resp) = ui.allocate_exact_size(vec2(40.0, 40.0), Sense::click());
                        let painter = ui.painter();
                        painter.rect_filled(rect, 3.0, SLOT_BG);
                        draw_item_icon(painter, rect.shrink(4.0), self.icon_of(id), self.tex);
                        let resp = resp.on_hover_text(&item.name);
                        if resp.clicked() {
                            a = a.or(Some(InvAction::Pick(id)));
                        }
                    }
                    a
                })
                .inner
            })
            .inner
    }

    /// Survival crafting list: one clickable row per recipe, greyed when the
    /// ingredients aren't in the storage grid.
    fn crafting_list(
        &self,
        ui: &mut egui::Ui,
        recipes: &RecipeBook,
        width: f32,
        height: f32,
    ) -> Option<InvAction> {
        egui::ScrollArea::vertical()
            .id_salt("crafting")
            .max_height(height)
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.set_width(width);
                let mut a = None;
                for (index, recipe) in recipes.recipes().iter().enumerate() {
                    let craftable = recipe.can_craft(self.inventory);
                    a = a.or(self.recipe_row(ui, index, recipe, craftable));
                    ui.add_space(4.0);
                }
                a
            })
            .inner
    }

    fn recipe_row(
        &self,
        ui: &mut egui::Ui,
        index: usize,
        recipe: &crate::inventory::Recipe,
        craftable: bool,
    ) -> Option<InvAction> {
        let width = ui.available_width();
        let (rect, resp) = ui.allocate_exact_size(vec2(width, 44.0), Sense::click());
        let painter = ui.painter();
        let bg = if !craftable {
            DISABLED_BG
        } else if resp.hovered() {
            SLOT_HOVER
        } else {
            SLOT_BG
        };
        painter.rect_filled(rect, 4.0, bg);

        let mid = rect.center().y;
        let icon = 30.0;
        let mut x = rect.left() + 8.0;
        for &(item, n) in &recipe.ingredients {
            let r = Rect::from_min_size(pos2(x, mid - icon / 2.0), vec2(icon, icon));
            draw_item_icon(painter, r, self.icon_of(item), self.tex);
            painter.text(
                r.right_bottom(),
                Align2::RIGHT_BOTTOM,
                format!("{n}"),
                FontId::monospace(12.0),
                Color32::WHITE,
            );
            x += icon + 12.0;
        }
        painter.text(
            pos2(x, mid),
            Align2::LEFT_CENTER,
            "→",
            FontId::proportional(18.0),
            TEXT,
        );
        x += 26.0;
        let out = Rect::from_min_size(pos2(x, mid - icon / 2.0), vec2(icon, icon));
        draw_item_icon(painter, out, self.icon_of(recipe.output), self.tex);
        x += icon + 8.0;
        let name = &self.items.get(recipe.output).name;
        let label = if recipe.count > 1 {
            format!("{}× {name}", recipe.count)
        } else {
            name.clone()
        };
        painter.text(
            pos2(x, mid),
            Align2::LEFT_CENTER,
            label,
            FontId::proportional(15.0),
            if craftable { TEXT } else { DISABLED_TEXT },
        );

        (resp.clicked() && craftable).then_some(InvAction::Craft(index))
    }
}

/// Paint a stack count in the slot's bottom-right corner (hidden for singles).
fn paint_count(painter: &egui::Painter, cell: Rect, count: u8) {
    if count <= 1 {
        return;
    }
    let pos = cell.right_bottom() - vec2(3.0, 2.0);
    // A cheap drop shadow keeps the number legible over any icon.
    painter.text(
        pos + vec2(1.0, 1.0),
        Align2::RIGHT_BOTTOM,
        count.to_string(),
        FontId::proportional(13.0),
        Color32::BLACK,
    );
    painter.text(
        pos,
        Align2::RIGHT_BOTTOM,
        count.to_string(),
        FontId::proportional(13.0),
        Color32::WHITE,
    );
}

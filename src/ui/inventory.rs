//! The inventory panel: a four-slot armor column down its left edge and the
//! whole 9x4 storage grid — hotbar included as its bottom row — to their right.
//!
//! Interaction is click-to-move (Minecraft-style): the caller owns a "held"
//! stack; this view reports what the player did to a slot and paints the grid
//! and the held stack under the cursor. The move logic lives with the
//! inventory owner. Left click moves a whole stack, right click splits one in
//! half (or places a single item), and taking a stack outside the panel throws
//! it into the world.
//!
//! **The panel unfolds out of the hotbar.** Its bottom row is laid out from
//! [`crate::ui::slot`]'s metrics, which is what the HUD hotbar uses too, so at
//! progress 0 the two are the same nine cells at the same size in the same
//! place and the hand-off between them is invisible. Everything above that row
//! is revealed by a clip rect growing out of it. Both endpoints are *computed*
//! ([`layout`] and [`hud::hotbar_rect`]) rather than read back from egui, which
//! matters because the animation starts on a frame where egui has placed
//! nothing yet.

use egui::{Align2, Color32, Context, FontId, Rect, Sense, pos2, vec2};

use crate::content::ItemIcon;
use crate::core::GameMode;
use crate::inventory::{
    ARMOR_SIZE, ARMOR_START, ArmorSlot, HOTBAR_SIZE, INVENTORY_SIZE, Inventory, ItemId,
    ItemRegistry, ItemStack,
};
use crate::state::UiTextures;
use crate::ui::hud;
use crate::ui::icon::draw_item_icon;
use crate::ui::ninepatch;
use crate::ui::slot::{self, GAP, PITCH, SIZE, SlotContents};

/// The storage grid: nine columns, and every slot the inventory has.
/// The bottom row is the hotbar, which is why there is no gap above it.
const GRID_COLS: usize = HOTBAR_SIZE;
const GRID_ROWS: usize = INVENTORY_SIZE / GRID_COLS;

/// The dark carcass's margin around its contents, and the gap between the
/// armor column and the grid.
const PANEL_PAD: f32 = 10.0;
const COLUMN_GAP: f32 = 10.0;
/// How far the panel floats inside the screen's right edge.
const SCREEN_MARGIN: f32 = 28.0;
/// Extra space between the storage rows and the hotbar row below them.
///
/// They are one contiguous 9x4 block of slot *indices*, but the bottom row is
/// the hotbar — the nine you carry — and it reads better set apart from the
/// twenty-seven you are only storing. Each group keeps its own padding either
/// side of this, so the visible channel is `GAP + HOTBAR_GAP + GAP`.
const HOTBAR_GAP: f32 = 10.0;
/// Height of the creative palette strip below the grid.
const PALETTE_H: f32 = 118.0;
/// Size of the stack riding on the cursor.
const HELD_ICON: f32 = 40.0;

/// What the player did in the inventory screen this frame.
pub enum InvAction {
    /// Left-clicked a slot: pick up / place / merge / swap the whole stack.
    Slot(usize),
    /// Right-clicked a slot: take half of it, or place a single item into it.
    Split(usize),
    /// Picked an item from the creative palette (grab a full stack).
    Pick(ItemId),
    /// Dragged a slot's stack clear of the panel: throw it into the world.
    DropSlot(usize),
    /// Clicked outside the panel with a stack on the cursor. `all` throws the
    /// lot, as a left click does; a right click parts with one item.
    DropHeld { all: bool },
    /// Pressed the drop key over a slot: throw one item out of it.
    DropOne(usize),
}

/// What happened to a slot this frame.
#[derive(Default, Clone, Copy)]
struct SlotHit {
    primary: bool,
    secondary: bool,
    /// A press that began on this slot and was released beyond the panel.
    dragged_out: bool,
}

impl SlotHit {
    /// The action this hit means, most specific first: leaving the panel beats
    /// a split, which beats an ordinary move.
    fn action(self, index: usize) -> Option<InvAction> {
        if self.dragged_out {
            Some(InvAction::DropSlot(index))
        } else if self.secondary {
            Some(InvAction::Split(index))
        } else if self.primary {
            Some(InvAction::Slot(index))
        } else {
            None
        }
    }
}

/// Where the panel rests and how much screen it leaves for the player model.
///
/// Pure, so the camera (which needs to know where *not* to put the model) and
/// the painter derive their numbers from one place and cannot disagree.
#[derive(Clone, Copy, Debug)]
pub struct InventoryLayout {
    /// The panel at rest.
    pub panel: Rect,
    /// The armor column's cells and their padding.
    pub armor: Rect,
    /// The whole grid, both groups and the channel between them.
    pub grid: Rect,
    /// The three storage rows' backing.
    pub storage: Rect,
    /// The grid's bottom row — the hotbar, and the animation's anchor.
    pub hotbar_row: Rect,
    /// The creative palette strip; `Rect::NOTHING` in survival.
    pub palette: Rect,
    /// Centre of the column left clear for the player model, as a fraction of
    /// the screen's width.
    pub stage_center_x: f32,
}

/// Lay the panel out for a screen of this size.
pub fn layout(screen: Rect, creative: bool) -> InventoryLayout {
    // Every row but the last, then the channel, then the hotbar row with
    // padding of its own — the two groups are backed separately.
    let storage_h = (GRID_ROWS - 1) as f32 * PITCH + GAP;
    let grid_size = vec2(
        GRID_COLS as f32 * PITCH + GAP,
        storage_h + HOTBAR_GAP + SIZE + 2.0 * GAP,
    );
    let armor_size = vec2(PITCH + GAP, ARMOR_SIZE as f32 * PITCH + GAP);
    let strip = if creative {
        PALETTE_H + COLUMN_GAP
    } else {
        0.0
    };

    let body = vec2(
        armor_size.x + COLUMN_GAP + grid_size.x,
        grid_size.y.max(armor_size.y) + strip,
    );
    let panel_size = body + vec2(2.0 * PANEL_PAD, 2.0 * PANEL_PAD);

    // Flush right, vertically centred. Clamped to the screen so a window
    // narrower than the panel shows the panel rather than half of it.
    let left = (screen.right() - SCREEN_MARGIN - panel_size.x).max(screen.left());
    let top = (screen.center().y - panel_size.y * 0.5).max(screen.top());
    let panel = Rect::from_min_size(pos2(left, top), panel_size);

    let armor = Rect::from_min_size(panel.min + vec2(PANEL_PAD, PANEL_PAD), armor_size);
    let grid = Rect::from_min_size(
        pos2(armor.right() + COLUMN_GAP, panel.top() + PANEL_PAD),
        grid_size,
    );
    let storage = Rect::from_min_size(grid.min, vec2(grid_size.x, storage_h));
    // The bottom row, with padding of its own around it — which is exactly the
    // shape `hud::hotbar_rect` produces, so the two coincide at progress 0.
    let hotbar_row = Rect::from_min_size(
        pos2(grid.left(), storage.bottom() + HOTBAR_GAP),
        vec2(grid_size.x, SIZE + 2.0 * GAP),
    );
    let palette = if creative {
        Rect::from_min_size(
            pos2(grid.left(), grid.bottom() + COLUMN_GAP),
            vec2(grid_size.x, PALETTE_H),
        )
    } else {
        Rect::NOTHING
    };

    // Everything left of the panel is the model's stage; centre it in that.
    let stage_center_x = if screen.width() > 0.0 {
        (panel.left() - screen.left()) / screen.width() * 0.5
    } else {
        0.25
    };

    InventoryLayout {
        panel,
        armor,
        grid,
        storage,
        hotbar_row,
        palette,
        stage_center_x,
    }
}

/// Everything the view needs to draw a slot's contents, bundled so the many
/// slot calls don't each take a fistful of arguments.
struct View<'a> {
    inventory: &'a Inventory,
    items: &'a ItemRegistry,
    icons: &'a [ItemIcon],
    /// Display name per `ItemId` — what a hovered slot names. Passed as a
    /// slice rather than read off `Item`, because a label is presentation and
    /// deliberately lives on `content`, not in the hashed registry.
    names: &'a [String],
    tex: UiTextures,
}

/// Draw the inventory panel at `progress` through its unfold.
///
/// `progress` is 0 at the hotbar and 1 at rest; the caller owns the easing.
/// Clicks are only reported once the panel has arrived, so a slot can never be
/// hit while it is still travelling under the cursor.
#[allow(clippy::too_many_arguments)]
pub fn draw_inventory(
    ctx: &Context,
    inventory: &Inventory,
    items: &ItemRegistry,
    icons: &[ItemIcon],
    names: &[String],
    held: Option<ItemStack>,
    mode: GameMode,
    progress: f32,
    // `drop_pressed`: the drop key went down this frame — throw one of whatever
    // is under the cursor. Passed in rather than read from egui because the
    // binding is the game's, and egui's key enum is not winit's.
    drop_pressed: bool,
    tex: UiTextures,
) -> Option<InvAction> {
    let view = View {
        inventory,
        items,
        icons,
        names,
        tex,
    };
    let screen = ctx.screen_rect();
    let l = layout(screen, mode.is_creative());
    let t = progress.clamp(0.0, 1.0);
    let interactive = t >= 1.0;

    // The panel's hotbar row travels from the HUD hotbar to its resting place.
    // Both rects are the same size, so this is a pure translation — no scaling,
    // and so no half-pixel slots on the way.
    let row = lerp_rect(hud::hotbar_rect(screen), l.hotbar_row, t);
    let shift = row.min - l.hotbar_row.min;

    // Reveal the rest of the panel by growing the clip out of that row.
    let panel = l.panel.translate(shift);
    let clip = lerp_rect(row, panel, t);

    // The carcass and the rows above the hotbar fade in slightly behind the
    // unfold, so the reveal reads as one motion rather than a wipe.
    let body = tint(smoothstep(0.10, 0.65, t));

    let mut action = egui::Area::new(egui::Id::new("inventory"))
        .fixed_pos(clip.min)
        .order(egui::Order::Middle)
        .show(ctx, |ui| {
            ui.set_clip_rect(clip);
            let painter = ui.painter().clone();

            ninepatch::draw_nine(&painter, panel, ninepatch::PANEL, body, tex.gui);
            // One bed per group, so the channel between them shows the dark
            // carcass through and the hotbar reads as its own row.
            for bed in [l.storage, l.hotbar_row] {
                ninepatch::draw_nine(
                    &painter,
                    bed.translate(shift),
                    ninepatch::GRID,
                    body,
                    tex.gui,
                );
            }

            let armor = l.armor.translate(shift);
            let grid = l.grid.translate(shift);
            let mut action = view
                .armor_column(ui, &painter, armor, panel, body, interactive)
                .or(view.grid(ui, &painter, grid, panel, body, interactive));

            if mode.is_creative() {
                let palette = l.palette.translate(shift);
                action = action.or(view.palette(ui, &painter, palette, body, interactive));
            }
            action
        })
        .inner;

    // The drop key acts on whatever the player is dealing with: the stack on
    // the cursor if they are carrying one, otherwise the slot under it. One
    // item either way — the whole stack has the drag-it-out gesture.
    if action.is_none() && interactive && drop_pressed {
        action = match (held, ctx.pointer_latest_pos()) {
            (Some(_), _) => Some(InvAction::DropHeld { all: false }),
            (None, Some(cursor)) => slot_under(&l, shift, cursor).map(InvAction::DropOne),
            (None, None) => None,
        };
    }

    // A stack on the cursor, clicked away from the panel: throw it. This is the
    // other half of dragging one out — the click-to-move model has no "release"
    // of its own, so putting a stack down outside the panel is what parts with
    // it. Right click parts with a single item, matching what it does in a slot.
    if action.is_none()
        && interactive
        && held.is_some()
        && ctx.pointer_latest_pos().is_some_and(|p| !panel.contains(p))
    {
        let (primary, secondary) =
            ctx.input(|i| (i.pointer.primary_pressed(), i.pointer.secondary_pressed()));
        if primary {
            action = Some(InvAction::DropHeld { all: true });
        } else if secondary {
            action = Some(InvAction::DropHeld { all: false });
        }
    }

    // The held stack and the tooltip ride above the panel on their own layer.
    let painter = ctx.layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("inventory_overlay"),
    ));
    match (held, ctx.pointer_latest_pos()) {
        (Some(stack), Some(pos)) => {
            let rect = Rect::from_center_size(pos, vec2(HELD_ICON, HELD_ICON));
            draw_item_icon(&painter, rect, view.icon_of(stack.item), tex);
            paint_count(&painter, rect, stack.count);
        }
        (None, Some(pos)) if interactive => {
            if let Some(name) = view.name_under(&l, shift, pos, mode) {
                draw_tooltip(&painter, screen, pos, name, tex);
            }
        }
        _ => {}
    }

    action
}

/// A tooltip naming the hovered item, offset from the cursor and kept on screen.
fn draw_tooltip(
    painter: &egui::Painter,
    screen: Rect,
    cursor: egui::Pos2,
    name: &str,
    tex: UiTextures,
) {
    const OFFSET: egui::Vec2 = egui::vec2(18.0, 18.0);
    const PAD: f32 = 10.0;

    let galley =
        painter.layout_no_wrap(name.to_string(), FontId::proportional(15.0), Color32::WHITE);
    let size = galley.size() + vec2(2.0 * PAD, 2.0 * PAD);

    // Flip to the other side of the cursor rather than run off the screen.
    let mut min = cursor + OFFSET;
    if min.x + size.x > screen.right() {
        min.x = cursor.x - OFFSET.x - size.x;
    }
    if min.y + size.y > screen.bottom() {
        min.y = cursor.y - OFFSET.y - size.y;
    }
    let rect = Rect::from_min_size(min, size);

    ninepatch::draw_nine(painter, rect, ninepatch::TOOLTIP, Color32::WHITE, tex.gui);
    painter.galley(rect.min + vec2(PAD, PAD), galley, Color32::PLACEHOLDER);
}

impl View<'_> {
    fn icon_of(&self, item: ItemId) -> ItemIcon {
        self.icons[item.0 as usize]
    }

    /// What a hovered slot names. Falls back to the id so a registry and a
    /// label list that have drifted out of step still say *something*.
    fn name_of(&self, item: ItemId) -> &str {
        self.names
            .get(item.0 as usize)
            .map(String::as_str)
            .unwrap_or_else(|| &self.items.get(item).id)
    }

    /// The display name of whatever the cursor is over, for the tooltip.
    ///
    /// Hit-tested against the same rects the painter uses rather than read off
    /// a `Response`, so the tooltip cannot name something other than what is
    /// drawn under the cursor.
    fn name_under(
        &self,
        l: &InventoryLayout,
        shift: egui::Vec2,
        cursor: egui::Pos2,
        mode: GameMode,
    ) -> Option<&str> {
        if let Some(index) = slot_under(l, shift, cursor) {
            return self.inventory.slot(index).map(|s| self.name_of(s.item));
        }
        if mode.is_creative() {
            for (id, cell) in self.palette_cells(l.palette.translate(shift)) {
                if cell.contains(cursor) {
                    return Some(self.name_of(id));
                }
            }
        }
        None
    }

    /// The armor column: one slot per piece, empties showing a faded ghost of
    /// what fits.
    fn armor_column(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        panel: Rect,
        tint: Color32,
        interactive: bool,
    ) -> Option<InvAction> {
        let mut action = None;
        for (index, cell) in armor_cells(rect) {
            let piece = ArmorSlot::ALL[index - ARMOR_START];
            slot::paint_slot(
                painter,
                cell,
                self.contents(index),
                self.armor_ghost(piece),
                false,
                tint,
                self.tex,
            );
            if interactive {
                action = action.or(interact(ui, cell, panel).action(index));
            }
        }
        action
    }

    /// The whole 9x4 grid, one contiguous block. The bottom row is the hotbar,
    /// which is why it is the one row that shows a selection.
    ///
    /// That row is drawn at **full opacity whatever `tint` says**: it is the
    /// HUD hotbar continuing, and it is the only thing on screen at progress 0,
    /// where the fade has not started and the HUD's own copy is already hidden.
    /// Fading it with the rest would blink the hotbar out for the first frames
    /// of every open.
    fn grid(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        panel: Rect,
        tint: Color32,
        interactive: bool,
    ) -> Option<InvAction> {
        let mut action = None;
        for (index, cell) in grid_cells(rect) {
            let is_hotbar = index < HOTBAR_SIZE;
            let selected = is_hotbar && index == self.inventory.selected_index();
            slot::paint_slot(
                painter,
                cell,
                self.contents(index),
                None,
                selected,
                if is_hotbar { Color32::WHITE } else { tint },
                self.tex,
            );
            if interactive {
                action = action.or(interact(ui, cell, panel).action(index));
            }
        }
        action
    }

    /// Creative palette: items as click-to-grab icons in the strip below the
    /// grid. The only in-UI way to get items in creative.
    fn palette(
        &self,
        ui: &mut egui::Ui,
        painter: &egui::Painter,
        rect: Rect,
        tint: Color32,
        interactive: bool,
    ) -> Option<InvAction> {
        ninepatch::draw_nine(painter, rect, ninepatch::GRID, tint, self.tex.gui);
        let mut action = None;
        for (id, cell) in self.palette_cells(rect) {
            slot::paint_slot(
                painter,
                cell,
                Some(SlotContents {
                    stack: ItemStack::single(id),
                    icon: self.icon_of(id),
                    items: self.items,
                }),
                None,
                false,
                tint,
                self.tex,
            );
            if interactive && ui.allocate_rect(cell, Sense::click()).clicked() {
                action = action.or(Some(InvAction::Pick(id)));
            }
        }
        action
    }

    /// Palette cells, wrapped left to right and clipped to the strip.
    fn palette_cells(&self, rect: Rect) -> Vec<(ItemId, Rect)> {
        if !rect.is_positive() {
            return Vec::new();
        }
        let cols = ((rect.width() - GAP) / PITCH).floor().max(1.0) as usize;
        let rows = ((rect.height() - GAP) / PITCH).floor().max(1.0) as usize;
        self.items
            .iter()
            .take(cols * rows)
            .enumerate()
            .map(|(n, (id, _))| {
                let (row, col) = (n / cols, n % cols);
                (
                    id,
                    Rect::from_min_size(
                        rect.min + vec2(GAP + col as f32 * PITCH, GAP + row as f32 * PITCH),
                        vec2(SIZE, SIZE),
                    ),
                )
            })
            .collect()
    }

    fn contents(&self, index: usize) -> Option<SlotContents<'_>> {
        self.inventory.slot(index).map(|stack| SlotContents {
            stack,
            icon: self.icon_of(stack.item),
            items: self.items,
        })
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
}

/// The inventory slot under `cursor`, if any.
///
/// Hit-tested against the same rects the painter uses, so the drop key and the
/// tooltip can never disagree with what is drawn under the cursor. Palette
/// entries are deliberately excluded: they are an infinite source, not slots.
fn slot_under(l: &InventoryLayout, shift: egui::Vec2, cursor: egui::Pos2) -> Option<usize> {
    armor_cells(l.armor.translate(shift))
        .chain(grid_cells(l.grid.translate(shift)))
        .find(|(_, cell)| cell.contains(cursor))
        .map(|(index, _)| index)
}

/// What the player did to `cell` this frame.
///
/// Senses drags as well as clicks so a stack can be pulled out of the panel and
/// thrown. A press that turns into a drag does not also report `clicked`, so
/// the two gestures never both fire for one press.
fn interact(ui: &mut egui::Ui, cell: Rect, panel: Rect) -> SlotHit {
    let response = ui.allocate_rect(cell, Sense::click_and_drag());
    let released_outside = response.drag_stopped()
        && ui
            .ctx()
            .pointer_latest_pos()
            .is_some_and(|p| !panel.contains(p));
    SlotHit {
        primary: response.clicked(),
        secondary: response.secondary_clicked(),
        dragged_out: released_outside,
    }
}

/// The armor column's cells, paired with the slot index each shows.
fn armor_cells(rect: Rect) -> impl Iterator<Item = (usize, Rect)> {
    (0..ARMOR_SIZE).map(move |i| {
        (
            ARMOR_START + i,
            Rect::from_min_size(
                rect.min + vec2(GAP, GAP + i as f32 * PITCH),
                vec2(SIZE, SIZE),
            ),
        )
    })
}

/// The grid's cells, paired with the slot index each shows.
///
/// Storage sits above the hotbar on screen, but the hotbar is slots `0..9` — so
/// the last drawn row is the *first* nine indices, and the rows above it run
/// `9..36` in order.
fn grid_cells(rect: Rect) -> impl Iterator<Item = (usize, Rect)> {
    (0..GRID_ROWS).flat_map(move |row| {
        (0..GRID_COLS).map(move |col| {
            let last = row + 1 == GRID_ROWS;
            let index = if last {
                col
            } else {
                HOTBAR_SIZE + row * GRID_COLS + col
            };
            // The hotbar row clears the channel, and picks up the leading
            // padding of its own backing on the way.
            let y = GAP + row as f32 * PITCH + if last { HOTBAR_GAP + GAP } else { 0.0 };
            (
                index,
                Rect::from_min_size(
                    rect.min + vec2(GAP + col as f32 * PITCH, y),
                    vec2(SIZE, SIZE),
                ),
            )
        })
    })
}

fn lerp_rect(a: Rect, b: Rect, t: f32) -> Rect {
    let lerp = |a: f32, b: f32| a + (b - a) * t;
    Rect::from_min_max(
        pos2(lerp(a.min.x, b.min.x), lerp(a.min.y, b.min.y)),
        pos2(lerp(a.max.x, b.max.x), lerp(a.max.y, b.max.y)),
    )
}

/// GLSL-style smoothstep, for fading one part in behind another.
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn tint(alpha: f32) -> Color32 {
    Color32::from_rgba_unmultiplied(255, 255, 255, (alpha.clamp(0.0, 1.0) * 255.0) as u8)
}

/// Paint a stack count in the bottom-right of `cell`, hidden for singles.
///
/// The slot painter has its own copy for cells; this one is for the stack
/// riding on the cursor, which is not in a slot.
fn paint_count(painter: &egui::Painter, cell: Rect, count: u8) {
    if count <= 1 {
        return;
    }
    let pos = cell.right_bottom() - vec2(3.0, 2.0);
    let font = FontId::proportional(14.0);
    painter.text(
        pos + vec2(1.0, 1.0),
        Align2::RIGHT_BOTTOM,
        count.to_string(),
        font.clone(),
        Color32::BLACK,
    );
    painter.text(
        pos,
        Align2::RIGHT_BOTTOM,
        count.to_string(),
        font,
        Color32::WHITE,
    );
}

#[cfg(test)]
mod tests {
    use super::*;

    fn screen(w: f32, h: f32) -> Rect {
        Rect::from_min_size(pos2(0.0, 0.0), vec2(w, h))
    }

    /// The animation's whole no-gap, no-double-draw guarantee: at progress 0
    /// the panel's bottom row is the HUD hotbar, exactly. Asserted rather than
    /// eyeballed, because the two are laid out by different functions.
    #[test]
    fn the_panels_bottom_row_is_the_shape_of_the_hotbar() {
        for (w, h) in [(1920.0, 1080.0), (1280.0, 720.0), (2560.0, 1080.0)] {
            let s = screen(w, h);
            let l = layout(s, false);
            let hud_rect = hud::hotbar_rect(s);
            assert!(
                (l.hotbar_row.width() - hud_rect.width()).abs() < 1e-3
                    && (l.hotbar_row.height() - hud_rect.height()).abs() < 1e-3,
                "at {w}x{h}: panel row {:?} vs hotbar {:?}",
                l.hotbar_row.size(),
                hud_rect.size()
            );
        }
    }

    /// ...and the nine cells inside it land on the hotbar's nine cells, so the
    /// items do not jump on the frame the panel takes over from the HUD.
    #[test]
    fn the_bottom_rows_cells_land_on_the_hotbars() {
        let s = screen(1920.0, 1080.0);
        let l = layout(s, false);
        let hud_rect = hud::hotbar_rect(s);
        // At progress 0 the row is translated onto the hotbar.
        let shift = hud_rect.min - l.hotbar_row.min;

        let bottom: Vec<_> = grid_cells(l.grid.translate(shift))
            .filter(|(index, _)| *index < HOTBAR_SIZE)
            .collect();
        assert_eq!(bottom.len(), HOTBAR_SIZE);
        for (index, cell) in bottom {
            let expected = hud::hotbar_cell(hud_rect, index);
            assert!(
                (cell.min.x - expected.min.x).abs() < 1e-3
                    && (cell.min.y - expected.min.y).abs() < 1e-3,
                "slot {index}: {cell:?} vs {expected:?}"
            );
        }
    }

    /// The camera frames the model in the space the panel leaves. If the two
    /// disagree the model ends up behind the panel.
    #[test]
    fn the_panel_rests_clear_of_the_model_stage() {
        for (w, h) in [(1920.0, 1080.0), (2560.0, 1080.0), (1440.0, 1080.0)] {
            let s = screen(w, h);
            let l = layout(s, false);
            assert!(
                l.stage_center_x * 2.0 * w <= l.panel.left() + 1e-3,
                "at {w}x{h} the stage runs under the panel"
            );
            assert!(l.stage_center_x > 0.0, "at {w}x{h} there is no stage left");
        }
    }

    /// Every storage slot is drawn exactly once, and nothing else is.
    #[test]
    fn the_grid_covers_every_storage_slot_once() {
        let l = layout(screen(1920.0, 1080.0), false);
        let mut seen: Vec<usize> = grid_cells(l.grid).map(|(i, _)| i).collect();
        seen.sort_unstable();
        assert_eq!(seen, (0..INVENTORY_SIZE).collect::<Vec<_>>());
    }

    /// And every armor slot, in `ArmorSlot::ALL` order.
    #[test]
    fn the_armor_column_covers_every_armor_slot_once() {
        let l = layout(screen(1920.0, 1080.0), false);
        let seen: Vec<usize> = armor_cells(l.armor).map(|(i, _)| i).collect();
        assert_eq!(
            seen,
            (ARMOR_START..ARMOR_START + ARMOR_SIZE).collect::<Vec<_>>()
        );
    }

    /// Creative adds the palette strip; survival must not reserve space for it.
    #[test]
    fn the_palette_strip_is_creative_only() {
        let s = screen(1920.0, 1080.0);
        let survival = layout(s, false);
        let creative = layout(s, true);
        assert!(!survival.palette.is_positive());
        assert!(creative.palette.is_positive());
        assert!(
            creative.panel.height() > survival.panel.height(),
            "the strip has to make the panel taller"
        );
    }

    /// The camera's framing and the panel's position are two halves of one
    /// layout, and this is the seam between them: `stage_center_x` goes to
    /// `Shot::inspect`, comes back as a lens shift, and has to land the model
    /// in the clear column beside the panel.
    ///
    /// The regression this exists for: `camera_shot` was handing `layout` a
    /// *normalised* rect (width = the aspect ratio, about 1.8) where it wants
    /// points. A 558-point panel does not fit in 1.8 points, so the stage
    /// clamped to zero width and the shift went to its -1.0 extreme, pinning
    /// the model against the left edge half off screen. Testing `Shot::inspect`
    /// against a hardcoded fraction missed it completely — only running the
    /// two together does.
    #[test]
    fn the_camera_frames_the_model_in_the_column_the_panel_leaves() {
        use crate::entity::camera::Shot;
        use glam::Vec3;

        for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1080.0)] {
            let s = screen(w, h);
            let l = layout(s, false);
            let fov_y = 70f32.to_radians();
            let shot = Shot::inspect(fov_y, l.stage_center_x);

            let eye = Vec3::new(0.0, 1.62, 0.0);
            let camera = shot.camera(eye, 0.0, shot.distance, 70.0, w / h);
            let chest = camera
                .project(eye + Vec3::Y * -0.55)
                .expect("the chest is in front of the camera");

            let model_x = chest.x * w;
            assert!(
                model_x > 0.06 * w,
                "at {w}x{h} the model sits at {model_x:.0}px, jammed against the left edge"
            );
            assert!(
                model_x < l.panel.left(),
                "at {w}x{h} the model sits at {model_x:.0}px, under the panel at {:.0}px",
                l.panel.left()
            );
        }
    }

    /// The hotbar has to read as its own row, so there must be a real channel
    /// of carcass between the two beds — not merely the slot padding the rows
    /// inside each group already have.
    #[test]
    fn the_hotbar_row_is_set_apart_from_the_storage_rows() {
        let l = layout(screen(1920.0, 1080.0), false);
        assert!(
            l.hotbar_row.top() - l.storage.bottom() >= HOTBAR_GAP - 1e-3,
            "the two beds are only {} apart",
            l.hotbar_row.top() - l.storage.bottom()
        );

        // And the cells either side of it are further apart than two rows
        // within the storage block, which is what actually reads on screen.
        let cells: Vec<_> = grid_cells(l.grid).collect();
        let row_of = |index: usize| {
            cells
                .iter()
                .find(|(i, _)| *i == index)
                .expect("slot is drawn")
                .1
        };
        let within_storage = row_of(HOTBAR_SIZE + GRID_COLS).top() - row_of(HOTBAR_SIZE).bottom();
        let across_channel = row_of(0).top() - row_of(INVENTORY_SIZE - GRID_COLS).bottom();
        assert!(
            across_channel > within_storage * 2.0,
            "the channel ({across_channel}) barely beats the row gap ({within_storage})"
        );
    }

    /// Every cell has to sit inside the panel that frames it.
    #[test]
    fn every_cell_sits_inside_the_panel() {
        let l = layout(screen(1920.0, 1080.0), true);
        for (index, cell) in grid_cells(l.grid).chain(armor_cells(l.armor)) {
            assert!(
                l.panel.contains_rect(cell),
                "slot {index} at {cell:?} escapes {:?}",
                l.panel
            );
        }
    }
}

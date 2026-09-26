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
//! [`crate::presentation::ui::slot`]'s metrics, which is what the HUD hotbar uses too, so at
//! progress 0 the two are the same nine cells at the same size in the same
//! place and the hand-off between them is invisible. Everything above that row
//! is revealed by a clip rect growing out of it. Both endpoints are *computed*
//! ([`layout`] and [`hud::hotbar_rect`]) rather than read back from egui, which
//! matters because the animation starts on a frame where egui has placed
//! nothing yet.
//!
//! In survival the crafting pane ([`crate::presentation::ui::crafting`]) sits above the panel
//! with the same width. It is laid out here, so the panel and the pane are
//! centred as one stack. It fades in behind the unfold instead of growing out
//! of the hotbar. It takes the width it needs from the panel, not from the
//! model's stage beside it.

use egui::{Align2, Color32, Context, FontId, Rect, pos2, vec2};

use crate::domain::core::GameMode;
use crate::domain::inventory::{
    ARMOR_SIZE, ARMOR_START, ArmorSlot, Equippable, HOTBAR_SIZE, INVENTORY_SIZE, Inventory, ItemId,
    ItemRegistry, ItemStack,
};
use crate::presentation::content::ItemIcon;
use crate::presentation::ui::UiTextures;
use crate::presentation::ui::crafting::{self, CRAFT_H, CraftAction, CraftingView};
use crate::presentation::ui::hud;
use crate::presentation::ui::icon::draw_item_icon;
use crate::presentation::ui::ninepatch;
use crate::presentation::ui::slot::{self, GAP, PITCH, SIZE, SlotContents};

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
/// Space between the crafting pane and the panel below it.
const CRAFT_GAP: f32 = 10.0;

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
    /// Something in the crafting pane.
    Craft(CraftAction),
}

/// What a press landed on, remembered until the button comes back up.
///
/// Stored in egui's temp memory rather than a `Response`, because a slot has to
/// act on press-and-release over itself *however long it took and however much
/// the mouse slid in between*. egui only calls that a click within 6 points and
/// 0.8 seconds ([`egui::InputOptions`]); past either it is a drag and the click
/// is dropped on the floor. For a button that is the right call. For an
/// inventory slot it is the single most confusing thing the panel can do —
/// the click visibly lands on the slot and nothing happens.
#[derive(Clone, Copy, PartialEq)]
enum PressTarget {
    Slot(usize),
    Palette(ItemId),
    /// Pressed on the world beyond the panel.
    Outside,
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
    /// The crafting pane above the panel; `Rect::NOTHING` in creative, where
    /// the palette hands out everything and there is nothing to craft.
    pub crafting: Rect,
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

    // Flush right, and the stack — crafting pane over panel — vertically
    // centred. Clamped to the screen so a window narrower than the panel shows
    // the panel rather than half of it.
    let above = if creative { 0.0 } else { CRAFT_H + CRAFT_GAP };
    let left = (screen.right() - SCREEN_MARGIN - panel_size.x).max(screen.left());
    let stack_top = (screen.center().y - (panel_size.y + above) * 0.5).max(screen.top());
    let panel = Rect::from_min_size(pos2(left, stack_top + above), panel_size);
    let crafting = if creative {
        Rect::NOTHING
    } else {
        Rect::from_min_size(pos2(left, stack_top), vec2(panel_size.x, CRAFT_H))
    };

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
        crafting,
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
    // The crafting pane's contents; `None` draws no pane (creative).
    crafting_view: Option<&CraftingView<'_>>,
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

    egui::Area::new(egui::Id::new("inventory"))
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

            view.armor_column(&painter, l.armor.translate(shift), body);
            view.grid(&painter, l.grid.translate(shift), body);
            if mode.is_creative() {
                view.palette(&painter, l.palette.translate(shift), body);
            }
        });

    // The crafting pane rides the same translation, and fades in behind the
    // unfold rather than growing out of the hotbar: it has no row down there
    // to grow from.
    let crafting_rect = l.crafting.translate(shift);
    let mut action = crafting_view
        .and_then(|cv| {
            let opacity = smoothstep(0.35, 1.0, t);
            crafting::draw_crafting(ctx, cv, crafting_rect, opacity, interactive)
        })
        .map(InvAction::Craft);

    // Interaction is resolved from the raw pointer rather than from egui
    // `Response`s — see `gesture` — so it is decided here rather than inside
    // the painting closure.
    if action.is_none() && interactive {
        let bounds = Bounds {
            panel,
            crafting: crafting_rect,
        };
        action = gesture(
            ctx,
            &view,
            &l,
            shift,
            bounds,
            held.is_some(),
            mode.is_creative(),
        );
    }

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
    fn armor_column(&self, painter: &egui::Painter, rect: Rect, tint: Color32) {
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
        }
    }

    /// The whole 9x4 grid, one contiguous block. The bottom row is the hotbar,
    /// which is why it is the one row that shows a selection.
    ///
    /// That row is drawn at **full opacity whatever `tint` says**: it is the
    /// HUD hotbar continuing, and it is the only thing on screen at progress 0,
    /// where the fade has not started and the HUD's own copy is already hidden.
    /// Fading it with the rest would blink the hotbar out for the first frames
    /// of every open.
    fn grid(&self, painter: &egui::Painter, rect: Rect, tint: Color32) {
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
        }
    }

    /// Creative palette: items as click-to-grab icons in the strip below the
    /// grid. The only in-UI way to get items in creative.
    fn palette(&self, painter: &egui::Painter, rect: Rect, tint: Color32) {
        ninepatch::draw_nine(painter, rect, ninepatch::GRID, tint, self.tex.gui);
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
        }
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
            let fits = item.get::<Equippable>().map(|w| w.slot) == Some(piece);
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

/// What counts as "on the panel" for a gesture: the panel itself, and the
/// crafting pane above it. The pane handles its own clicks, but a press there
/// must not read as clicking the world behind, and a stack carried over it
/// must not be thrown.
#[derive(Clone, Copy)]
struct Bounds {
    panel: Rect,
    crafting: Rect,
}

impl Bounds {
    fn contains(self, p: egui::Pos2) -> bool {
        self.panel.contains(p) || self.crafting.contains(p)
    }
}

/// Resolve this frame's pointer into an action, by hand.
///
/// Press remembers what it landed on; release decides. Anything between —
/// dawdling, or sliding the mouse a few pixels — is ignored, which is what
/// makes a slot feel like a slot. See [`PressTarget`].
fn gesture(
    ctx: &Context,
    view: &View<'_>,
    l: &InventoryLayout,
    shift: egui::Vec2,
    panel: Bounds,
    holding: bool,
    creative: bool,
) -> Option<InvAction> {
    let id = egui::Id::new("inventory_press");
    let (pressed, released, cursor) = ctx.input(|i| {
        (
            [i.pointer.primary_pressed(), i.pointer.secondary_pressed()],
            [i.pointer.primary_released(), i.pointer.secondary_released()],
            i.pointer.latest_pos(),
        )
    });
    let cursor = cursor?;

    let target_at = |p: egui::Pos2| {
        if let Some(index) = slot_under(l, shift, p) {
            PressTarget::Slot(index)
        } else if creative
            && let Some((item, _)) = view
                .palette_cells(l.palette.translate(shift))
                .into_iter()
                .find(|(_, cell)| cell.contains(p))
        {
            PressTarget::Palette(item)
        } else if panel.contains(p) {
            // The panel's chrome: swallowed, so a miss between slots does not
            // read as clicking the world behind.
            PressTarget::Slot(usize::MAX)
        } else {
            PressTarget::Outside
        }
    };

    if pressed[0] || pressed[1] {
        ctx.data_mut(|d| d.insert_temp(id, target_at(cursor)));
        return None;
    }
    if !(released[0] || released[1]) {
        return None;
    }
    let from: PressTarget = ctx.data_mut(|d| {
        let target = d.get_temp::<PressTarget>(id);
        d.remove::<PressTarget>(id);
        target
    })?;
    let secondary = released[1];

    match from {
        PressTarget::Slot(usize::MAX) => None,
        PressTarget::Slot(index) => {
            if !panel.contains(cursor) {
                // Carried clear of the panel: throw it.
                Some(if secondary {
                    InvAction::DropOne(index)
                } else {
                    InvAction::DropSlot(index)
                })
            } else if secondary {
                Some(InvAction::Split(index))
            } else {
                Some(InvAction::Slot(index))
            }
        }
        // The palette is an infinite source: nothing to split, nothing to throw.
        PressTarget::Palette(item) => {
            (!secondary && panel.contains(cursor)).then_some(InvAction::Pick(item))
        }
        PressTarget::Outside => {
            (holding && !panel.contains(cursor)).then_some(InvAction::DropHeld { all: !secondary })
        }
    }
}

/// The armor column's cells, paired with the slot index each shows./// The armor column's cells, paired with the slot index each shows.
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
mod tests;

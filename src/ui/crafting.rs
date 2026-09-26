//! The crafting pane: a recipe list and a detail view, sitting on top of the
//! inventory panel. Modelled on Valheim and Terraria rather than a Minecraft
//! grid. You pick what to make instead of arranging ingredients, and the pane
//! puts what a recipe needs beside what you have.
//!
//! - **The list** shows the recipes the player has discovered, in file order,
//!   grouped by where they are made (by hand, workbench, forge). The order is
//!   fixed, so rows never jump around as the inventory changes. Each section
//!   header says whether its station is in reach. Recipes that cannot be made
//!   right now are dimmed rather than hidden, unless "Craftable only" is on.
//! - **The detail** shows the selected recipe's materials as `have / need`,
//!   green or red, and a Craft button that says why it is disabled when it is.
//!   Shift-click crafts as many as the materials and the free space allow.
//!
//! Like the inventory, this only reports what the player did. The caller
//! decides what that means.

use egui::{
    Align2, Color32, Context, FontId, Painter, Pos2, Rect, Response, Sense, Ui, pos2, vec2,
};

use crate::content::ItemIcon;
use crate::core::ident::title_case;
use crate::inventory::crafting::{Availability, Recipe, StationSet};
use crate::inventory::{Inventory, ItemId, ItemRegistry, ItemStack};
use crate::state::UiTextures;
use crate::ui::icon::draw_item_icon;
use crate::ui::ninepatch;
use crate::ui::slot::{self, SlotContents};

/// Height of the pane. Its width is the inventory panel's.
pub const CRAFT_H: f32 = 300.0;

const PAD: f32 = 10.0;
const HEADER_H: f32 = 26.0;
const SECTION_H: f32 = 22.0;
const ROW_H: f32 = 34.0;
const ROW_ICON: f32 = 26.0;
const MATERIAL_H: f32 = 28.0;
const MATERIAL_ICON: f32 = 22.0;
const BUTTON_H: f32 = 34.0;
/// Share of the pane's inner width that goes to the list.
const LIST_SHARE: f32 = 0.5;

const TEXT: Color32 = Color32::from_rgb(240, 228, 204);
const DIM: Color32 = Color32::from_rgb(150, 136, 116);
const GOOD: Color32 = Color32::from_rgb(140, 214, 112);
const BAD: Color32 = Color32::from_rgb(232, 108, 92);
const GOLD: Color32 = Color32::from_rgb(255, 204, 72);
const SELECTED: Color32 = Color32::from_rgba_premultiplied(56, 46, 30, 60);
const HOVER: Color32 = Color32::from_rgba_premultiplied(18, 18, 18, 18);
const WELL: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 110);
const SCRIM: Color32 = Color32::from_rgba_premultiplied(0, 0, 0, 120);
const BUTTON: Color32 = Color32::from_rgb(92, 138, 64);
const BUTTON_HOT: Color32 = Color32::from_rgb(112, 164, 78);
const BUTTON_OFF: Color32 = Color32::from_rgb(70, 60, 50);

/// One discovered recipe, as the list shows it.
pub struct RecipeEntry<'a> {
    /// Index into the recipe book, which is what actions refer to.
    pub index: usize,
    pub recipe: &'a Recipe,
    pub availability: Availability,
    /// Discovered since the player last looked at it.
    pub new: bool,
}

/// Everything the pane draws, gathered by the caller.
pub struct CraftingView<'a> {
    /// Discovered recipes in book order, already filtered if
    /// `craftable_only` is on.
    pub entries: &'a [RecipeEntry<'a>],
    /// How many recipes are discovered, and how many exist.
    pub discovered: usize,
    pub total: usize,
    /// Every station, in block order, which is the order of the sections.
    pub stations: &'a [String],
    pub nearby: &'a StationSet,
    pub selected: Option<usize>,
    pub craftable_only: bool,
    pub inventory: &'a Inventory,
    pub items: &'a ItemRegistry,
    pub icons: &'a [ItemIcon],
    pub names: &'a [String],
    pub tex: UiTextures,
}

/// What the player did in the pane this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CraftAction {
    /// Clicked a recipe in the list.
    Select(usize),
    /// Pressed Craft. `all` when shift was held.
    Craft { index: usize, all: bool },
    /// Flipped the "Craftable only" filter.
    ToggleCraftableOnly,
}

/// Where the pane's parts sit, computed rather than read back from egui so
/// they can be tested.
#[derive(Debug, Clone, Copy)]
pub struct PaneLayout {
    pub header: Rect,
    pub list: Rect,
    pub detail: Rect,
    pub toggle: Rect,
}

pub fn pane_layout(rect: Rect) -> PaneLayout {
    let inner = rect.shrink(PAD);
    let header = Rect::from_min_size(inner.min, vec2(inner.width(), HEADER_H));
    let body = Rect::from_min_max(pos2(inner.left(), header.bottom() + 6.0), inner.max);
    let split = body.left() + body.width() * LIST_SHARE;
    PaneLayout {
        header,
        list: Rect::from_min_max(body.min, pos2(split - 4.0, body.bottom())),
        detail: Rect::from_min_max(pos2(split + 4.0, body.top()), body.max),
        toggle: Rect::from_min_max(pos2(header.right() - 124.0, header.top()), header.max),
    }
}

/// Draw the pane in `rect` at `opacity`. Input is taken only when
/// `interactive`, which is the same rule the inventory grid follows: nothing
/// can be clicked while it is still moving under the cursor.
pub fn draw_crafting(
    ctx: &Context,
    view: &CraftingView<'_>,
    rect: Rect,
    opacity: f32,
    interactive: bool,
) -> Option<CraftAction> {
    if opacity <= 0.0 || !rect.is_positive() {
        return None;
    }
    let l = pane_layout(rect);
    let mut action = None;
    egui::Area::new(egui::Id::new("crafting"))
        .fixed_pos(rect.min)
        .order(egui::Order::Middle)
        .interactable(interactive)
        .show(ctx, |ui| {
            ui.multiply_opacity(opacity.clamp(0.0, 1.0));
            // Claim the whole pane, so the area is the right size for hover
            // and hit-testing on the very first frame.
            ui.allocate_rect(rect, Sense::hover());
            let painter = ui.painter().clone();
            ninepatch::draw_nine(
                &painter,
                rect,
                ninepatch::PANEL,
                Color32::WHITE,
                view.tex.gui,
            );
            // Dark wells rather than the inventory's light bed: this pane is
            // mostly text, and the light bed washes the dimmed rows out.
            painter.rect_filled(l.list, 4.0, WELL);
            painter.rect_filled(l.detail.expand(4.0), 4.0, WELL);

            let shown = shown_recipe(view);
            action = header(ui, &painter, view, &l)
                .or(list(ui, view, l.list, shown))
                .or(detail(ui, &painter, view, l.detail, shown));
        });
    action.filter(|_| interactive)
}

/// The list's sections in the order they are drawn: by hand first, then each
/// station in block order, each holding its recipes in book order. Empty
/// sections are left out.
fn sections<'a>(view: &'a CraftingView<'a>) -> Vec<(Option<&'a str>, Vec<&'a RecipeEntry<'a>>)> {
    std::iter::once(None)
        .chain(view.stations.iter().map(|s| Some(s.as_str())))
        .map(|station| {
            let rows = view
                .entries
                .iter()
                .filter(|e| e.recipe.station.as_deref() == station)
                .collect::<Vec<_>>();
            (station, rows)
        })
        .filter(|(_, rows)| !rows.is_empty())
        .collect()
}

/// The recipe in the detail view: the selection if it is still listed, else
/// the first one that can be made right now, else the first one drawn. Never
/// `None` while the list has anything in it, so the detail is never blank
/// for no reason.
fn shown_recipe<'a>(view: &'a CraftingView<'a>) -> Option<&'a RecipeEntry<'a>> {
    let drawn: Vec<&RecipeEntry<'_>> = sections(view).into_iter().flat_map(|(_, r)| r).collect();
    view.selected
        .and_then(|i| drawn.iter().copied().find(|e| e.index == i))
        .or_else(|| {
            drawn
                .iter()
                .copied()
                .find(|e| e.availability.is_craftable())
        })
        .or_else(|| drawn.first().copied())
}

/// A press and release over `response`, however far the mouse slid between
/// them. egui's own `clicked` gives up past a few points of travel, which reads
/// as the pane ignoring you; see `ui::inventory::PressTarget` for the same fix.
fn released_on(response: &Response) -> bool {
    response.clicked() || (response.drag_stopped() && response.contains_pointer())
}

fn header(
    ui: &mut Ui,
    painter: &Painter,
    view: &CraftingView<'_>,
    l: &PaneLayout,
) -> Option<CraftAction> {
    let title = painter.text(
        l.header.left_center(),
        Align2::LEFT_CENTER,
        "Crafting",
        FontId::proportional(18.0),
        TEXT,
    );
    painter.text(
        pos2(title.right() + 10.0, l.header.center().y + 1.0),
        Align2::LEFT_CENTER,
        format!("{} / {} discovered", view.discovered, view.total),
        FontId::proportional(13.0),
        DIM,
    );

    // The Terraria filter, as a checkbox.
    let response = ui.interact(
        l.toggle,
        ui.id().with("craftable_only"),
        Sense::click_and_drag(),
    );
    let hot = response.hovered();
    let boxed = Rect::from_center_size(
        pos2(l.toggle.left() + 9.0, l.toggle.center().y),
        vec2(14.0, 14.0),
    );
    painter.rect_filled(boxed, 2.0, Color32::from_black_alpha(120));
    painter.rect_stroke(
        boxed,
        2.0,
        egui::Stroke::new(1.0_f32, if hot { TEXT } else { DIM }),
        egui::StrokeKind::Inside,
    );
    if view.craftable_only {
        painter.rect_filled(boxed.shrink(3.0), 1.0, GOOD);
    }
    painter.text(
        pos2(boxed.right() + 6.0, l.toggle.center().y),
        Align2::LEFT_CENTER,
        "Craftable only",
        FontId::proportional(13.0),
        if hot { TEXT } else { DIM },
    );
    released_on(&response).then_some(CraftAction::ToggleCraftableOnly)
}

/// The recipe list, grouped by where each recipe is made.
fn list(
    ui: &mut Ui,
    view: &CraftingView<'_>,
    rect: Rect,
    shown: Option<&RecipeEntry<'_>>,
) -> Option<CraftAction> {
    let inner = rect.shrink(4.0);
    if view.entries.is_empty() {
        let message = if view.discovered == 0 {
            "Pick up materials to\ndiscover recipes."
        } else {
            "Nothing can be made here.\nGather materials or\nfind a crafting station."
        };
        ui.painter().text(
            inner.center(),
            Align2::CENTER_CENTER,
            message,
            FontId::proportional(14.0),
            DIM,
        );
        return None;
    }

    let mut action = None;
    let sections = sections(view);

    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        egui::ScrollArea::vertical()
            .id_salt("crafting_list")
            .auto_shrink([false, false])
            .drag_to_scroll(false)
            .max_height(inner.height())
            .show(ui, |ui| {
                for (station, rows) in &sections {
                    section_header(ui, *station, view.nearby);
                    for entry in rows {
                        let selected = shown.is_some_and(|s| s.index == entry.index);
                        if recipe_row(ui, view, entry, selected) {
                            action = Some(CraftAction::Select(entry.index));
                        }
                    }
                }
            });
    });
    action
}

/// "BY HAND", or a station's name with whether one is in reach.
fn section_header(ui: &mut Ui, station: Option<&str>, nearby: &StationSet) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width(), SECTION_H), Sense::hover());
    let painter = ui.painter();
    let text_pos = pos2(rect.left() + 6.0, rect.center().y + 2.0);
    let font = FontId::proportional(12.0);
    match station {
        None => {
            painter.text(text_pos, Align2::LEFT_CENTER, "BY HAND", font, DIM);
        }
        Some(station) => {
            let label = title_case(station).to_uppercase();
            painter.text(text_pos, Align2::LEFT_CENTER, label, font.clone(), DIM);
            let (text, colour) = if nearby.contains(station) {
                ("nearby", GOOD)
            } else {
                ("not nearby", BAD)
            };
            painter.text(
                pos2(rect.right() - 8.0, text_pos.y),
                Align2::RIGHT_CENTER,
                text,
                font,
                colour,
            );
        }
    }
}

/// One recipe in the list; `true` if it was clicked.
fn recipe_row(
    ui: &mut Ui,
    view: &CraftingView<'_>,
    entry: &RecipeEntry<'_>,
    selected: bool,
) -> bool {
    let (rect, response) =
        ui.allocate_exact_size(vec2(ui.available_width(), ROW_H), Sense::click_and_drag());
    let painter = ui.painter().with_clip_rect(rect.intersect(ui.clip_rect()));
    if selected {
        painter.rect_filled(rect, 3.0, SELECTED);
    } else if response.hovered() {
        painter.rect_filled(rect, 3.0, HOVER);
    }

    let ready = entry.availability.is_craftable();
    let recipe = entry.recipe;
    let icon = Rect::from_min_size(
        pos2(rect.left() + 6.0, rect.center().y - ROW_ICON * 0.5),
        vec2(ROW_ICON, ROW_ICON),
    );
    draw_item_icon(&painter, icon, icon_of(view, recipe.output), view.tex);
    if !ready {
        painter.rect_filled(icon, 0.0, SCRIM);
    }

    let mut right = rect.right() - 8.0;
    if recipe.count > 1 {
        let r = painter.text(
            pos2(right, rect.center().y),
            Align2::RIGHT_CENTER,
            format!("×{}", recipe.count),
            FontId::proportional(13.0),
            DIM,
        );
        right = r.left() - 6.0;
    }
    if entry.new {
        right = new_badge(&painter, pos2(right, rect.center().y)).left() - 6.0;
    }

    let name_clip = Rect::from_min_max(rect.min, pos2(right, rect.bottom()));
    painter
        .with_clip_rect(name_clip.intersect(ui.clip_rect()))
        .text(
            pos2(icon.right() + 8.0, rect.center().y),
            Align2::LEFT_CENTER,
            name_of(view, recipe.output),
            FontId::proportional(15.0),
            if ready { TEXT } else { DIM },
        );
    released_on(&response)
}

/// A small gold "NEW" tag ending at `right_center`.
fn new_badge(painter: &Painter, right_center: Pos2) -> Rect {
    let galley = painter.layout_no_wrap(
        "NEW".to_string(),
        FontId::proportional(10.0),
        Color32::from_rgb(40, 30, 10),
    );
    let size = galley.size() + vec2(8.0, 4.0);
    let rect = Rect::from_min_size(
        pos2(right_center.x - size.x, right_center.y - size.y * 0.5),
        size,
    );
    painter.rect_filled(rect, 3.0, GOLD);
    painter.galley(rect.min + vec2(4.0, 2.0), galley, Color32::PLACEHOLDER);
    rect
}

/// The selected recipe: what it makes, what it takes, and the Craft button.
fn detail(
    ui: &mut Ui,
    painter: &Painter,
    view: &CraftingView<'_>,
    rect: Rect,
    shown: Option<&RecipeEntry<'_>>,
) -> Option<CraftAction> {
    let entry = shown?;
    let recipe = entry.recipe;

    // What it makes, in a slot, with its count, exactly as it will sit in the
    // inventory.
    let cell = Rect::from_min_size(rect.min, vec2(slot::SIZE, slot::SIZE));
    slot::paint_slot(
        painter,
        cell,
        Some(SlotContents {
            stack: ItemStack::new(recipe.output, recipe.count),
            icon: icon_of(view, recipe.output),
            items: view.items,
        }),
        None,
        false,
        Color32::WHITE,
        view.tex,
    );
    painter.text(
        pos2(cell.right() + 10.0, cell.top() + 12.0),
        Align2::LEFT_CENTER,
        name_of(view, recipe.output),
        FontId::proportional(17.0),
        TEXT,
    );
    let (where_text, where_colour) = match recipe.station.as_deref() {
        None => ("Made by hand, anywhere".to_string(), DIM),
        Some(s) if view.nearby.contains(s) => (format!("At a {} — nearby", title_case(s)), GOOD),
        Some(s) => (format!("Needs a {} nearby", title_case(s)), BAD),
    };
    painter.text(
        pos2(cell.right() + 10.0, cell.bottom() - 12.0),
        Align2::LEFT_CENTER,
        where_text,
        FontId::proportional(13.0),
        where_colour,
    );

    // Materials: have / need, green when there is enough.
    let mut y = cell.bottom() + 12.0;
    painter.text(
        pos2(rect.left(), y),
        Align2::LEFT_TOP,
        "MATERIALS",
        FontId::proportional(12.0),
        DIM,
    );
    y += 18.0;
    for &(item, need) in &recipe.ingredients {
        let row = Rect::from_min_size(pos2(rect.left(), y), vec2(rect.width(), MATERIAL_H));
        material_row(painter, view, row, item, need);
        y += MATERIAL_H;
    }

    // The button, pinned to the bottom so it never moves between recipes.
    let hint_h = 18.0;
    let button = Rect::from_min_max(
        pos2(rect.left(), rect.bottom() - hint_h - BUTTON_H),
        pos2(rect.right(), rect.bottom() - hint_h),
    );
    craft_button(ui, painter, entry, button)
}

fn material_row(painter: &Painter, view: &CraftingView<'_>, row: Rect, item: ItemId, need: u32) {
    let have = view.inventory.count_of(item);
    let icon = Rect::from_min_size(
        pos2(row.left() + 2.0, row.center().y - MATERIAL_ICON * 0.5),
        vec2(MATERIAL_ICON, MATERIAL_ICON),
    );
    draw_item_icon(painter, icon, icon_of(view, item), view.tex);
    let counts = painter.text(
        pos2(row.right() - 2.0, row.center().y),
        Align2::RIGHT_CENTER,
        format!("{have} / {need}"),
        FontId::proportional(14.0),
        if have >= need { GOOD } else { BAD },
    );
    let name_clip = Rect::from_min_max(row.min, pos2(counts.left() - 6.0, row.bottom()));
    painter.with_clip_rect(name_clip).text(
        pos2(icon.right() + 8.0, row.center().y),
        Align2::LEFT_CENTER,
        name_of(view, item),
        FontId::proportional(14.0),
        TEXT,
    );
}

fn craft_button(
    ui: &mut Ui,
    painter: &Painter,
    entry: &RecipeEntry<'_>,
    button: Rect,
) -> Option<CraftAction> {
    let response = ui.interact(
        button,
        ui.id().with("craft_button"),
        Sense::click_and_drag(),
    );
    let shift = ui.input(|i| i.modifiers.shift);
    let (enabled, max) = match entry.availability {
        Availability::Craftable { max } => (true, max),
        _ => (false, 0),
    };
    let fill = match (enabled, response.hovered()) {
        (false, _) => BUTTON_OFF,
        (true, true) => BUTTON_HOT,
        (true, false) => BUTTON,
    };
    painter.rect_filled(button, 4.0, fill);
    let label = if enabled && shift && max > 1 {
        format!("Craft ×{max}")
    } else {
        "Craft".to_string()
    };
    painter.text(
        button.center(),
        Align2::CENTER_CENTER,
        label,
        FontId::proportional(16.0),
        if enabled { TEXT } else { DIM },
    );

    let (hint, colour) = match entry.availability {
        Availability::Craftable { max } if max > 1 => {
            (format!("Shift-click to craft all ({max})"), DIM)
        }
        Availability::Craftable { .. } => (String::new(), DIM),
        Availability::MissingStation => {
            let station = entry.recipe.station.as_deref().unwrap_or("station");
            (
                format!("Stand near a {} to craft this", title_case(station)),
                BAD,
            )
        }
        Availability::MissingIngredients => ("Missing materials".to_string(), BAD),
        Availability::NoRoom => ("Inventory full".to_string(), BAD),
    };
    painter.text(
        pos2(button.center().x, button.bottom() + 10.0),
        Align2::CENTER_CENTER,
        hint,
        FontId::proportional(12.0),
        colour,
    );

    (enabled && released_on(&response)).then_some(CraftAction::Craft {
        index: entry.index,
        all: shift,
    })
}

fn icon_of(view: &CraftingView<'_>, item: ItemId) -> ItemIcon {
    view.icons
        .get(item.0 as usize)
        .copied()
        .unwrap_or(ItemIcon::Flat(0))
}

fn name_of<'a>(view: &'a CraftingView<'_>, item: ItemId) -> &'a str {
    view.names
        .get(item.0 as usize)
        .map(String::as_str)
        .unwrap_or_else(|| &view.items.get(item).id)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::inventory::crafting::{RecipeBook, station_ids};
    use crate::world::block::BlockRegistry;

    struct Fixture {
        items: ItemRegistry,
        book: RecipeBook,
        stations: Vec<String>,
        nearby: StationSet,
        inventory: Inventory,
        icons: Vec<ItemIcon>,
        names: Vec<String>,
    }

    impl Fixture {
        fn new() -> Self {
            let blocks = BlockRegistry::with_builtins();
            let items = ItemRegistry::from_blocks(&blocks);
            let stations = station_ids(&blocks);
            let book =
                RecipeBook::from_toml(include_str!("../../assets/recipes.toml"), &items, &stations)
                    .unwrap();
            let icons = vec![ItemIcon::Flat(0); items.len()];
            let names = vec![String::new(); items.len()];
            Self {
                items,
                book,
                stations,
                nearby: StationSet::default(),
                inventory: Inventory::new(),
                icons,
                names,
            }
        }

        /// An entry per named output, with a made-up availability.
        fn entries(&self, rows: &[(&str, Availability)]) -> Vec<RecipeEntry<'_>> {
            rows.iter()
                .map(|&(output, availability)| {
                    let item = self.items.find(output).unwrap();
                    let index = self
                        .book
                        .recipes()
                        .iter()
                        .position(|r| r.output == item)
                        .unwrap();
                    RecipeEntry {
                        index,
                        recipe: &self.book.recipes()[index],
                        availability,
                        new: false,
                    }
                })
                .collect()
        }

        fn view<'a>(
            &'a self,
            entries: &'a [RecipeEntry<'a>],
            selected: Option<usize>,
        ) -> CraftingView<'a> {
            CraftingView {
                entries,
                discovered: entries.len(),
                total: self.book.recipes().len(),
                stations: &self.stations,
                nearby: &self.nearby,
                selected,
                craftable_only: false,
                inventory: &self.inventory,
                items: &self.items,
                icons: &self.icons,
                names: &self.names,
                tex: UiTextures {
                    atlas: egui::TextureId::Managed(0),
                    model_icons: egui::TextureId::Managed(0),
                    model_count: 1,
                    gui: egui::TextureId::Managed(0),
                },
            }
        }
    }

    const READY: Availability = Availability::Craftable { max: 3 };
    const AWAY: Availability = Availability::MissingStation;

    /// Hand recipes come first whatever the book order says, then the
    /// stations in block order: the forge section never precedes the bench's.
    #[test]
    fn sections_are_hand_then_each_station_in_block_order() {
        let f = Fixture::new();
        // Book order puts the pickaxe (workbench) before the glass (forge) and
        // both before the bread (hand).
        let entries = f.entries(&[("wooden_pickaxe", AWAY), ("glass", AWAY), ("bread", READY)]);
        let view = f.view(&entries, None);
        let order: Vec<Option<&str>> = sections(&view).iter().map(|(s, _)| *s).collect();
        assert_eq!(order, [None, Some("workbench"), Some("forge")]);
    }

    /// With nothing selected, the detail shows something the player can make
    /// now rather than the first thing in the file.
    #[test]
    fn the_default_selection_prefers_what_can_be_made_now() {
        let f = Fixture::new();
        let entries = f.entries(&[("wooden_pickaxe", AWAY), ("stick", AWAY), ("bread", READY)]);
        let view = f.view(&entries, None);
        let shown = shown_recipe(&view).unwrap();
        assert_eq!(f.items.get(shown.recipe.output).id, "bread");

        // A real selection wins, craftable or not.
        let pick = entries[0].index;
        let view = f.view(&entries, Some(pick));
        assert_eq!(shown_recipe(&view).unwrap().index, pick);

        // A selection that has been filtered out falls back rather than
        // leaving the detail blank.
        let only_bread = f.entries(&[("bread", READY)]);
        let view = f.view(&only_bread, Some(pick));
        assert_eq!(shown_recipe(&view).unwrap().index, only_bread[0].index);
    }

    /// One frame's input: its events, and the modifiers held during it.
    type Frame = (Vec<egui::Event>, egui::Modifiers);

    fn run(
        ctx: &Context,
        view: &CraftingView<'_>,
        rect: Rect,
        frame: Frame,
    ) -> Option<CraftAction> {
        // Held modifiers live on the frame's input, not on the click event —
        // which is where the winit integration puts them too.
        let (events, modifiers) = frame;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(Pos2::ZERO, vec2(1280.0, 720.0))),
            events,
            modifiers,
            ..Default::default()
        };
        let mut action = None;
        let _ = ctx.run(input, |ctx| {
            action = draw_crafting(ctx, view, rect, 1.0, true)
        });
        action
    }

    fn click(pos: Pos2, modifiers: egui::Modifiers) -> Vec<Frame> {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers,
        };
        vec![
            (vec![egui::Event::PointerMoved(pos)], modifiers),
            (vec![button(true)], modifiers),
            (vec![button(false)], modifiers),
        ]
    }

    /// Clicking a row selects it; shift-clicking Craft crafts them all.
    #[test]
    fn a_row_click_selects_and_a_shift_click_on_craft_crafts_all() {
        let f = Fixture::new();
        let entries = f.entries(&[("stick", READY), ("bread", READY)]);
        let view = f.view(&entries, None);
        let rect = Rect::from_min_size(pos2(600.0, 60.0), vec2(558.0, CRAFT_H));
        let l = pane_layout(rect);
        let ctx = Context::default();
        run(&ctx, &view, rect, (vec![], egui::Modifiers::NONE));

        // One "by hand" section: its header, then stick, then bread.
        let list = l.list.shrink(4.0);
        let bread_row = pos2(list.left() + 60.0, list.top() + SECTION_H + ROW_H * 1.5);
        let mut got = None;
        for events in click(bread_row, egui::Modifiers::NONE) {
            got = got.or(run(&ctx, &view, rect, events));
        }
        assert_eq!(got, Some(CraftAction::Select(entries[1].index)));

        let button = pos2(
            l.detail.center().x,
            l.detail.bottom() - 18.0 - BUTTON_H * 0.5,
        );
        let mut got = None;
        for events in click(button, egui::Modifiers::SHIFT) {
            got = got.or(run(&ctx, &view, rect, events));
        }
        assert_eq!(
            got,
            Some(CraftAction::Craft {
                index: entries[0].index,
                all: true
            })
        );
    }

    /// A recipe that cannot be made has a Craft button that does nothing.
    #[test]
    fn a_disabled_craft_button_reports_nothing() {
        let f = Fixture::new();
        let entries = f.entries(&[("wooden_pickaxe", AWAY)]);
        let view = f.view(&entries, None);
        let rect = Rect::from_min_size(pos2(600.0, 60.0), vec2(558.0, CRAFT_H));
        let l = pane_layout(rect);
        let ctx = Context::default();
        run(&ctx, &view, rect, (vec![], egui::Modifiers::NONE));
        let button = pos2(
            l.detail.center().x,
            l.detail.bottom() - 18.0 - BUTTON_H * 0.5,
        );
        for events in click(button, egui::Modifiers::NONE) {
            assert_eq!(run(&ctx, &view, rect, events), None);
        }
    }

    #[test]
    fn the_list_and_detail_share_the_body_without_overlapping() {
        let rect = Rect::from_min_size(pos2(100.0, 50.0), vec2(558.0, CRAFT_H));
        let l = pane_layout(rect);
        for part in [l.header, l.list, l.detail, l.toggle] {
            assert!(rect.contains_rect(part), "{part:?} escapes the pane");
        }
        assert!(l.list.right() < l.detail.left());
        assert!(l.header.bottom() <= l.list.top());
        assert!(l.header.contains_rect(l.toggle));
        assert!(
            l.detail.height() >= 18.0 + slot::SIZE + 30.0 + 3.0 * MATERIAL_H + BUTTON_H + 18.0,
            "a three-material recipe and the button must both fit"
        );
    }
}

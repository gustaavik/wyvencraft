//! Tests for [`super`]: `crafting.rs`.

use super::*;
use crate::domain::inventory::crafting::{RecipeBook, station_ids};
use crate::domain::world::block::BlockRegistry;

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
        let book = RecipeBook::from_toml(
            include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/recipes.toml")),
            &items,
            &stations,
        )
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

fn run(ctx: &Context, view: &CraftingView<'_>, rect: Rect, frame: Frame) -> Option<CraftAction> {
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

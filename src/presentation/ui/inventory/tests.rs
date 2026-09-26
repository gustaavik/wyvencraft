//! Tests for [`super`]: `inventory.rs`.

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

// --- Interaction ------------------------------------------------------

use crate::domain::inventory::ItemStack;

struct Harness {
    ctx: Context,
    inventory: Inventory,
    items: ItemRegistry,
    icons: Vec<ItemIcon>,
    names: Vec<String>,
    screen: Rect,
}

impl Harness {
    fn new() -> Self {
        let blocks = crate::domain::world::block::BlockRegistry::with_builtins();
        let items = ItemRegistry::from_blocks(&blocks);
        let icons = vec![ItemIcon::Flat(0); items.len()];
        let names = vec![String::new(); items.len()];
        Self {
            ctx: Context::default(),
            inventory: Inventory::new(),
            items,
            icons,
            names,
            screen: screen(1920.0, 1080.0),
        }
    }

    fn tex(&self) -> UiTextures {
        UiTextures {
            atlas: egui::TextureId::Managed(0),
            model_icons: egui::TextureId::Managed(0),
            model_count: 1,
            gui: egui::TextureId::Managed(0),
        }
    }

    /// Run one frame carrying `events`, and report the action it produced.
    fn frame(&self, events: Vec<egui::Event>, held: Option<ItemStack>) -> Option<InvAction> {
        let input = egui::RawInput {
            screen_rect: Some(self.screen),
            events,
            ..Default::default()
        };
        let mut action = None;
        let _ = self.ctx.clone().run(input, |ctx| {
            action = draw_inventory(
                ctx,
                &self.inventory,
                &self.items,
                &self.icons,
                &self.names,
                held,
                GameMode::Survival,
                1.0,
                false,
                self.tex(),
                None,
            );
        });
        action
    }

    /// The centre of a storage slot, in screen coordinates.
    fn slot_centre(&self, index: usize) -> egui::Pos2 {
        let l = layout(self.screen, false);
        grid_cells(l.grid)
            .chain(armor_cells(l.armor))
            .find(|(i, _)| *i == index)
            .expect("slot is drawn")
            .1
            .center()
    }
}

fn press(pos: egui::Pos2, primary: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: if primary {
            egui::PointerButton::Primary
        } else {
            egui::PointerButton::Secondary
        },
        pressed: true,
        modifiers: Default::default(),
    }
}

fn release(pos: egui::Pos2, primary: bool) -> egui::Event {
    egui::Event::PointerButton {
        pos,
        button: if primary {
            egui::PointerButton::Primary
        } else {
            egui::PointerButton::Secondary
        },
        pressed: false,
        modifiers: Default::default(),
    }
}

/// The bug this exists for: egui only reports a click when press and
/// release are within 6 points and 0.8 seconds of each other. A slot has to
/// act on a press-and-release over itself however far the mouse slid in
/// between — anything else reads as the panel ignoring you at random.
#[test]
fn a_slot_still_acts_when_the_mouse_slides_during_the_click() {
    let h = Harness::new();
    let centre = h.slot_centre(12);
    // Well past egui's 6-point click threshold, but still on the slot.
    let drifted = centre + vec2(9.0, 5.0);

    assert!(h.frame(vec![press(centre, true)], None).is_none());
    assert!(
        h.frame(vec![egui::Event::PointerMoved(drifted)], None)
            .is_none()
    );
    let action = h.frame(vec![release(drifted, true)], None);
    assert!(
        matches!(action, Some(InvAction::Slot(12))),
        "a drifted click on slot 12 was dropped"
    );
}

/// The same for the split, which has no drag fallback of its own.
#[test]
fn a_drifted_right_click_still_splits() {
    let h = Harness::new();
    let centre = h.slot_centre(12);
    let drifted = centre + vec2(-8.0, 7.0);

    h.frame(vec![press(centre, false)], None);
    h.frame(vec![egui::Event::PointerMoved(drifted)], None);
    assert!(
        matches!(
            h.frame(vec![release(drifted, false)], None),
            Some(InvAction::Split(12))
        ),
        "a drifted right click was dropped"
    );
}

/// Press decides *what*, release decides *where*: carried clear of the
/// panel, the same gesture throws instead of picking up.
#[test]
fn a_press_carried_out_of_the_panel_throws_the_stack() {
    let h = Harness::new();
    let outside = pos2(80.0, 540.0);
    assert!(
        !layout(h.screen, false).panel.contains(outside),
        "test setup: that point must be off the panel"
    );

    h.frame(vec![press(h.slot_centre(12), true)], None);
    h.frame(vec![egui::Event::PointerMoved(outside)], None);
    assert!(matches!(
        h.frame(vec![release(outside, true)], None),
        Some(InvAction::DropSlot(12))
    ));
}

/// ...and a press that begins outside only throws what is on the cursor,
/// so clicking the world beside the panel with an empty hand does nothing.
#[test]
fn a_press_outside_the_panel_throws_only_what_is_held() {
    let h = Harness::new();
    let outside = pos2(80.0, 540.0);
    let stone = h.items.find("stone").expect("stone");

    h.frame(vec![press(outside, true)], None);
    assert!(h.frame(vec![release(outside, true)], None).is_none());

    let carrying = Some(ItemStack::new(stone, 4));
    h.frame(vec![press(outside, true)], carrying);
    assert!(matches!(
        h.frame(vec![release(outside, true)], carrying),
        Some(InvAction::DropHeld { all: true })
    ));
}

/// The crafting pane sits over the panel, the same width, on screen, and
/// out of the model's column. In creative there is none.
#[test]
fn the_crafting_pane_sits_above_the_panel_in_survival_only() {
    for (w, h) in [(1280.0, 720.0), (1920.0, 1080.0), (2560.0, 1080.0)] {
        let s = screen(w, h);
        let l = layout(s, false);
        assert!(
            s.contains_rect(l.crafting),
            "at {w}x{h} the pane is off screen"
        );
        assert!(
            s.contains_rect(l.panel),
            "at {w}x{h} the panel is off screen"
        );
        assert!(
            l.crafting.bottom() <= l.panel.top(),
            "at {w}x{h} they overlap"
        );
        assert_eq!(l.crafting.left(), l.panel.left());
        assert_eq!(l.crafting.width(), l.panel.width());
        assert!(l.stage_center_x * 2.0 * w <= l.crafting.left() + 1e-3);
    }
    assert!(!layout(screen(1920.0, 1080.0), true).crafting.is_positive());
}

/// The pane handles its own clicks, but for the grid's gestures it is part
/// of the panel: carrying a stack over it must not throw the stack.
#[test]
fn a_stack_carried_onto_the_crafting_pane_is_not_thrown() {
    let h = Harness::new();
    let pane = layout(h.screen, false).crafting.center();
    let outside = pos2(80.0, 540.0);
    let stone = h.items.find("stone").expect("stone");
    let carrying = Some(ItemStack::new(stone, 4));

    // From a slot, released on the pane: an ordinary click on that slot,
    // exactly as a release on the panel's chrome is — never a throw.
    h.frame(vec![press(h.slot_centre(12), true)], None);
    h.frame(vec![egui::Event::PointerMoved(pane)], None);
    assert!(matches!(
        h.frame(vec![release(pane, true)], None),
        Some(InvAction::Slot(12))
    ));

    // Pressed on the pane with a stack on the cursor.
    h.frame(vec![press(pane, true)], carrying);
    assert!(h.frame(vec![release(pane, true)], carrying).is_none());

    // And the world beyond both still takes a throw.
    h.frame(vec![press(outside, true)], carrying);
    assert!(matches!(
        h.frame(vec![release(outside, true)], carrying),
        Some(InvAction::DropHeld { all: true })
    ));
}

/// A press that lands on the panel's chrome rather than a slot is
/// swallowed, so a miss between cells cannot read as a click on the world.
#[test]
fn a_press_on_the_panels_chrome_does_nothing() {
    let h = Harness::new();
    let l = layout(h.screen, false);
    let chrome = l.panel.left_top() + vec2(3.0, 3.0);
    assert!(l.panel.contains(chrome) && slot_under(&l, vec2(0.0, 0.0), chrome).is_none());

    h.frame(vec![press(chrome, true)], None);
    assert!(h.frame(vec![release(chrome, true)], None).is_none());
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
    use crate::domain::entity::camera::Shot;
    use crate::presentation::render::camera::ShotCamera;
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

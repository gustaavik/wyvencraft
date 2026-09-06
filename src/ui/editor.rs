//! The item placement panel.
//!
//! A view and nothing else: it reads an [`EditorSession`] and returns the one
//! [`EditorAction`] the developer took this frame, exactly the way
//! [`crate::ui::inventory`] returns an `InvAction`. It opens no files, owns no
//! state, and knows nothing about how a placement reaches the renderer.

use egui::{Context, RichText};
use wyven_model::display::{DisplayContext, ItemTransform};

use crate::editor::{
    CONTEXTS, EditorAction, EditorSession, Placement, PlacementKind, SpecPlacement,
};

/// Wide enough for three labelled number fields on one row.
const FIELD: f32 = 68.0;
const PANEL_WIDTH: f32 = 340.0;

/// Degrees per pixel dragged. Slow enough to land on a whole degree.
const ANGLE_SPEED: f64 = 0.25;
/// Sixteenths of a block per pixel — `translation`'s unit.
const PIXEL_SPEED: f64 = 0.05;
const SCALE_SPEED: f64 = 0.002;

/// Draw the panel, and report what was done to it.
pub fn draw_editor(ctx: &Context, session: &EditorSession) -> Option<EditorAction> {
    let mut action = None;

    egui::Window::new("Item placement")
        .default_width(PANEL_WIDTH)
        .default_pos(egui::pos2(16.0, 16.0))
        .resizable(false)
        .show(ctx, |ui| {
            if session.targets().is_empty() {
                ui.label("No item declares a model.");
                return;
            }
            draw_picker(ui, session, &mut action);
            ui.separator();
            draw_contexts(ui, session, &mut action);
            draw_source(ui, session);
            ui.separator();
            draw_values(ui, session, &mut action);
            ui.separator();
            draw_buttons(ui, session, &mut action);
            draw_status(ui, session);
        });

    action
}

fn draw_picker(ui: &mut egui::Ui, session: &EditorSession, action: &mut Option<EditorAction>) {
    ui.horizontal(|ui| {
        let selected = session
            .current()
            .map_or("—", |target| target.name.as_str())
            .to_string();
        egui::ComboBox::from_id_salt("editor_item")
            .selected_text(selected)
            .width(190.0)
            .show_ui(ui, |ui| {
                for (index, target) in session.targets().iter().enumerate() {
                    if ui
                        .selectable_label(index == session.selected(), &target.name)
                        .clicked()
                    {
                        *action = Some(EditorAction::Select(index));
                    }
                }
            });

        let mut follow = session.follows_held();
        // Following the hand is how this is actually used: hold the thing, look
        // at it, move it. The dropdown is for reaching something you are not
        // holding.
        if ui
            .checkbox(&mut follow, "follow hand")
            .on_hover_text("Select whatever is in the hotbar slot you are holding")
            .changed()
        {
            *action = Some(EditorAction::FollowHeld(follow));
        }
    });
}

fn draw_contexts(ui: &mut egui::Ui, session: &EditorSession, action: &mut Option<EditorAction>) {
    let shared = session.current_kind() == Some(PlacementKind::Spec);
    ui.horizontal(|ui| {
        for context in CONTEXTS {
            let selected = context == session.context();
            if ui
                .selectable_label(selected, context_label(context))
                .clicked()
            {
                *action = Some(EditorAction::Context(context));
            }
        }
    });
    if shared {
        // Not a limitation to hide: this is exactly what `local_transform` does
        // when a model declares no `display` entry, and a panel that implied
        // otherwise would have the developer chasing a change that went
        // everywhere.
        ui.label(
            RichText::new("this model has no display block — one placement serves every context")
                .small()
                .weak(),
        );
    }
}

fn draw_source(ui: &mut egui::Ui, session: &EditorSession) {
    let Some(file) = session.current_file() else {
        return;
    };
    let layer = match session.current_kind() {
        Some(PlacementKind::Display) => "display block",
        _ => "[item.model]",
    };
    ui.label(RichText::new(format!("{layer} · {file}")).small().weak());
}

fn draw_values(ui: &mut egui::Ui, session: &EditorSession, action: &mut Option<EditorAction>) {
    let Some(value) = session.value() else {
        ui.label("nothing to edit");
        return;
    };
    match value {
        Placement::Display(transform) => draw_transform(ui, transform, action),
        Placement::Spec(spec) => draw_spec(ui, spec, action),
    }
}

fn draw_transform(ui: &mut egui::Ui, transform: ItemTransform, action: &mut Option<EditorAction>) {
    let mut edited = transform;
    let mut changed = false;
    changed |= triple(ui, "Rotation", "°", &mut edited.rotation, ANGLE_SPEED);
    changed |= triple(ui, "Translate", "px", &mut edited.translation, PIXEL_SPEED);
    changed |= scale_row(ui, &mut edited.scale);
    if changed {
        *action = Some(EditorAction::Edit(Placement::Display(edited)));
    }
}

fn draw_spec(ui: &mut egui::Ui, spec: SpecPlacement, action: &mut Option<EditorAction>) {
    let mut edited = spec;
    let mut changed = false;
    changed |= triple(ui, "Rotation", "°", &mut edited.rotation, ANGLE_SPEED);
    changed |= triple(ui, "Offset", "", &mut edited.offset, 0.005);
    ui.horizontal(|ui| {
        ui.add_sized([76.0, 18.0], egui::Label::new("Scale"));
        changed |= ui
            .add_sized(
                [FIELD, 18.0],
                egui::DragValue::new(&mut edited.scale)
                    .speed(SCALE_SPEED)
                    .range(0.001..=8.0),
            )
            .changed();
        ui.label(RichText::new("uniform").small().weak());
    });
    if changed {
        *action = Some(EditorAction::Edit(Placement::Spec(edited)));
    }
}

/// One labelled X/Y/Z row.
fn triple(ui: &mut egui::Ui, label: &str, unit: &str, values: &mut [f32; 3], speed: f64) -> bool {
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([76.0, 18.0], egui::Label::new(label));
        for (axis, value) in ["X", "Y", "Z"].into_iter().zip(values.iter_mut()) {
            changed |= ui
                .add_sized(
                    [FIELD, 18.0],
                    egui::DragValue::new(value).speed(speed).prefix(axis),
                )
                .changed();
        }
        if !unit.is_empty() {
            ui.label(RichText::new(unit).small().weak());
        }
    });
    changed
}

/// `display` scale is per-axis, but it is almost always uniform, so the common
/// case is one number and the odd case is still reachable.
fn scale_row(ui: &mut egui::Ui, scale: &mut [f32; 3]) -> bool {
    let uniform = scale[0] == scale[1] && scale[1] == scale[2];
    let mut changed = false;
    ui.horizontal(|ui| {
        ui.add_sized([76.0, 18.0], egui::Label::new("Scale"));
        if uniform {
            let mut all = scale[0];
            if ui
                .add_sized(
                    [FIELD, 18.0],
                    egui::DragValue::new(&mut all)
                        .speed(SCALE_SPEED)
                        .range(0.0..=8.0),
                )
                .changed()
            {
                *scale = [all; 3];
                changed = true;
            }
            ui.label(RichText::new("uniform").small().weak());
            return;
        }
        for (axis, value) in ["X", "Y", "Z"].into_iter().zip(scale.iter_mut()) {
            changed |= ui
                .add_sized(
                    [FIELD, 18.0],
                    egui::DragValue::new(value)
                        .speed(SCALE_SPEED)
                        .range(0.0..=8.0)
                        .prefix(axis),
                )
                .changed();
        }
    });
    changed
}

fn draw_buttons(ui: &mut egui::Ui, session: &EditorSession, action: &mut Option<EditorAction>) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(session.is_dirty(), egui::Button::new("Save"))
            .on_hover_text("Write this placement back to the file it came from")
            .clicked()
        {
            *action = Some(EditorAction::Save);
        }
        if ui
            .add_enabled(
                session.is_dirty() || session.is_stale(),
                egui::Button::new("Revert"),
            )
            .on_hover_text("Throw away unsaved changes and take what the file says now")
            .clicked()
        {
            *action = Some(EditorAction::Reload);
        }
        if ui
            .add_enabled(session.can_undo(), egui::Button::new("Undo"))
            .clicked()
        {
            *action = Some(EditorAction::Undo);
        }
        if ui.button("Close").clicked() {
            *action = Some(EditorAction::Close);
        }
    });
}

fn draw_status(ui: &mut egui::Ui, session: &EditorSession) {
    ui.horizontal(|ui| {
        if session.is_stale() {
            ui.label(RichText::new("⚠ changed on disk").small().color(WARN));
        } else if session.is_dirty() {
            ui.label(RichText::new("● unsaved").small().color(DIRTY));
        }
        ui.label(
            RichText::new(format!(
                "watching {} files · {}",
                session.watching(),
                session.status()
            ))
            .small()
            .weak(),
        );
    });
}

const DIRTY: egui::Color32 = egui::Color32::from_rgb(240, 180, 60);
const WARN: egui::Color32 = egui::Color32::from_rgb(230, 90, 70);

fn context_label(context: DisplayContext) -> &'static str {
    match context {
        DisplayContext::FirstPersonRightHand => "First person",
        DisplayContext::ThirdPersonRightHand => "Third person",
        DisplayContext::Gui => "GUI",
        DisplayContext::Ground => "Ground",
        DisplayContext::Fixed => "Fixed",
        DisplayContext::Head => "Head",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::EditorTarget;
    use crate::editor::store::InMemoryStore;
    use crate::inventory::ItemId;

    struct NoStamps;
    impl crate::editor::Stamps for NoStamps {
        fn modified(&self, _path: &str) -> Option<std::time::SystemTime> {
            None
        }
    }

    /// One headless egui frame; how many shapes it emitted. Enough to tell
    /// "drew the panel" from "drew nothing", with no GPU. Same helper shape as
    /// `ui::hud`'s.
    fn shapes_from(draw: impl Fn(&Context)) -> usize {
        let ctx = Context::default();
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(
                egui::Pos2::ZERO,
                egui::vec2(1280.0, 720.0),
            )),
            ..Default::default()
        };
        let _ = ctx.run(input(), |ctx| draw(ctx));
        let output = ctx.run(input(), |ctx| draw(ctx));
        output
            .shapes
            .iter()
            .filter(|shape| !matches!(shape.shape, egui::Shape::Noop))
            .count()
    }

    fn session(declared: Vec<DisplayContext>) -> EditorSession {
        let store = InMemoryStore::default().with(
            "wooden_sword",
            DisplayContext::FirstPersonRightHand,
            match declared.is_empty() {
                true => Placement::Spec(SpecPlacement::default()),
                false => Placement::Display(ItemTransform::default()),
            },
        );
        let mut session = EditorSession::new(true, Box::new(store));
        session.set_targets(vec![EditorTarget {
            item: ItemId(3),
            id: "wooden_sword".to_string(),
            name: "Wooden Sword".to_string(),
            model: "assets/models/items/wooden_sword.json".to_string(),
            declared,
        }]);
        session.toggle(&NoStamps);
        session
    }

    #[test]
    fn the_panel_paints() {
        let session = session(CONTEXTS.to_vec());
        assert!(
            shapes_from(|ctx| {
                draw_editor(ctx, &session);
            }) > 0
        );
    }

    #[test]
    fn a_spec_backed_item_paints_too() {
        let session = session(Vec::new());
        assert!(
            shapes_from(|ctx| {
                draw_editor(ctx, &session);
            }) > 0
        );
    }

    /// A session with nothing to edit must not panic on an empty selection.
    #[test]
    fn an_empty_target_list_paints_a_message() {
        let session = EditorSession::new(true, Box::new(InMemoryStore::default()));
        assert!(
            shapes_from(|ctx| {
                draw_editor(ctx, &session);
            }) > 0
        );
    }

    #[test]
    fn drawing_reports_no_action_when_nothing_is_touched() {
        let session = session(CONTEXTS.to_vec());
        let ctx = Context::default();
        let output = ctx.run(Default::default(), |ctx| {
            assert_eq!(draw_editor(ctx, &session), None);
        });
        assert!(!output.shapes.is_empty());
    }

    #[test]
    fn every_context_the_panel_offers_has_a_label() {
        for context in CONTEXTS {
            assert!(!context_label(context).is_empty());
        }
    }
}

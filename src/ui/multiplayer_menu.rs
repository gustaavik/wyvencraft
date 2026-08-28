//! The server browser: a list of saved servers and what to do with them.
//!
//! Draws and reports, like every other view here — it owns no server, no
//! selection and no list, only the text a `TextEdit` has to own while it is
//! being typed into. The state it renders lives in
//! [`ServerBrowser`](crate::state::multiplayer_menu::ServerBrowser), and the
//! screen turns the action below back into calls on it.

use egui::{Align, Color32, Context, Key, Layout, RichText, Sense};

use crate::state::multiplayer_menu::{RowState, ServerRow};

/// Width of the list and the button rows beneath it.
const WIDTH: f32 = 520.0;
const BUTTON: [f32; 2] = [160.0, 32.0];
/// Buttons in the top row (Join / Direct Connect / Refresh) and the bottom one
/// (Add / Edit / Delete / Back). The two rows are laid out to the same total
/// width, so the block of controls reads as one panel rather than two ragged
/// lines — which means the bottom row's width is *derived* from the top's, not
/// chosen.
const TOP_BUTTONS: f32 = 3.0;
const BOTTOM_BUTTONS: f32 = 4.0;
/// Height of a bottom-row button. They stay shorter than the top row: these are
/// the secondary actions, and matching the width is what lines the rows up —
/// matching the height as well would flatten the difference.
const SMALL_BUTTON_HEIGHT: f32 = 28.0;
/// A dialog's own buttons, sized independently — they line up with each other,
/// not with the rows behind the dialog.
const DIALOG_BUTTON: [f32; 2] = [100.0, SMALL_BUTTON_HEIGHT];
const FIELD_WIDTH: f32 = 300.0;

/// Round trips at or under this feel immediate; past the second, poor.
const PING_GOOD_MS: u32 = 80;
const PING_FAIR_MS: u32 = 200;

/// What the player did in the browser this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MpAction {
    Select(usize),
    /// Double-clicked a row, or pressed Join with it selected.
    Join(usize),
    Refresh,
    OpenAdd,
    /// Edit this row — the caller fills the fields before opening the dialog.
    OpenEdit(usize),
    OpenDirect,
    /// Delete this row (the browser decides whether that needs confirming).
    Delete(usize),
    /// The open dialog's primary button — the caller reads [`DialogFields`].
    Confirm,
    /// The open dialog was dismissed.
    Cancel,
    Back,
}

/// Which dialog is open over the list.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Dialog {
    Add,
    Edit(usize),
    /// Connect to an address without saving it.
    Direct,
}

/// The text the open dialog is editing.
#[derive(Debug, Default)]
pub struct DialogFields {
    pub name: String,
    pub address: String,
}

impl DialogFields {
    pub fn clear(&mut self) {
        self.name.clear();
        self.address.clear();
    }
}

/// Everything the browser hands the view for one frame.
pub struct BrowserView<'a> {
    pub rows: &'a [ServerRow<'a>],
    pub selected: Option<usize>,
    /// The row whose Delete button is armed and asking to be sure.
    pub confirming_delete: Option<usize>,
    pub refreshing: bool,
    pub error: Option<&'a str>,
}

/// Draw the server browser. Returns what the player did.
pub fn draw_multiplayer(
    ctx: &Context,
    view: BrowserView<'_>,
    dialog: Option<Dialog>,
    fields: &mut DialogFields,
) -> Option<MpAction> {
    let mut action = None;

    // The buttons sit in their own bottom panel rather than after the list, so
    // they stay put at the foot of the window however many servers are saved —
    // a control that moves as the list grows is a control you have to look for.
    // Declared before the central panel because egui gives panels their space in
    // declaration order, and the list is what should absorb the remainder.
    egui::TopBottomPanel::bottom("multiplayer_actions")
        .show_separator_line(false)
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                if let Some(err) = view.error {
                    ui.add_space(6.0);
                    ui.colored_label(Color32::LIGHT_RED, err);
                }
                ui.add_space(10.0);
                action = draw_buttons(ui, &view).or(action);
                ui.add_space(16.0);
            });
        });

    egui::CentralPanel::default().show(ctx, |ui| {
        ui.vertical_centered(|ui| {
            ui.add_space(40.0);
            ui.heading("Multiplayer");
            ui.add_space(4.0);
            ui.label(
                RichText::new("Double-click a server to join it.")
                    .small()
                    .color(Color32::GRAY),
            );
            ui.add_space(12.0);

            action = draw_list(ui, &view).or(action);
        });
    });

    // Drawn last so it sits over the list, and given the action outright: a
    // click inside the dialog must never also count as a click on the row
    // underneath it.
    if let Some(dialog) = dialog
        && let Some(dialog_action) = draw_dialog(ctx, dialog, fields)
    {
        return Some(dialog_action);
    }

    action
}

fn draw_list(ui: &mut egui::Ui, view: &BrowserView<'_>) -> Option<MpAction> {
    if view.rows.is_empty() {
        ui.add_space(40.0);
        ui.label(RichText::new("No servers yet — add one below.").color(Color32::GRAY));
        ui.add_space(40.0);
        return None;
    }

    let mut action = None;
    egui::ScrollArea::vertical()
        // Everything left between the heading and the buttons. A fixed height
        // would leave a gap under the list on a tall window now that the
        // buttons no longer follow it.
        .max_height(ui.available_height())
        .show(ui, |ui| {
            ui.set_width(WIDTH);
            for (index, row) in view.rows.iter().enumerate() {
                // Deferred out of the loop the way every other list here does
                // it: the row borrows `view`, so acting on the click inside
                // would borrow it twice.
                action = draw_row(ui, index, row, view).or(action);
            }
        });
    action
}

fn draw_row(
    ui: &mut egui::Ui,
    index: usize,
    row: &ServerRow<'_>,
    view: &BrowserView<'_>,
) -> Option<MpAction> {
    let selected = view.selected == Some(index);
    let frame = egui::Frame::new()
        .fill(if selected {
            ui.visuals().selection.bg_fill
        } else {
            Color32::TRANSPARENT
        })
        .inner_margin(egui::Margin::symmetric(8, 6))
        .corner_radius(4);

    let response = frame
        .show(ui, |ui| {
            ui.set_width(WIDTH - 16.0);
            ui.horizontal(|ui| {
                ui.vertical(|ui| {
                    ui.label(RichText::new(row.name).strong());
                    ui.label(RichText::new(subtitle(row)).small().color(Color32::GRAY));
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    draw_status(ui, row);
                });
            });
        })
        .response
        // The frame itself senses nothing; asking for clicks on its whole
        // rectangle is what makes the row a target rather than just the
        // words in it.
        .interact(Sense::click());

    ui.separator();

    if response.double_clicked() && row.joinable {
        return Some(MpAction::Join(index));
    }
    if response.clicked() {
        return Some(MpAction::Select(index));
    }
    None
}

/// The second line of a row: the address, and the host's own world name once
/// it has told us one (they are rarely the same, and both are worth seeing).
fn subtitle(row: &ServerRow<'_>) -> String {
    match row.state {
        RowState::Online { world, .. } if world != row.name => {
            format!("{} · {}", row.address, world)
        }
        _ => row.address.to_string(),
    }
}

fn draw_status(ui: &mut egui::Ui, row: &ServerRow<'_>) {
    match row.state {
        RowState::Unknown => {
            ui.label(RichText::new("—").color(Color32::DARK_GRAY));
        }
        RowState::Querying => {
            ui.spinner();
        }
        RowState::Online {
            online,
            max,
            ping_ms,
            compatible,
            ..
        } => {
            if compatible {
                ui.label(RichText::new(format!("{ping_ms} ms")).color(ping_color(ping_ms)));
            } else {
                ui.label(RichText::new("Incompatible").color(Color32::LIGHT_RED))
                    .on_hover_text(
                        "This server runs different blocks or items — it would turn you away.",
                    );
            }
            ui.add_space(12.0);
            ui.label(
                RichText::new(format!("{online}/{max}"))
                    .small()
                    .color(Color32::GRAY),
            );
        }
        RowState::Offline(reason) => {
            ui.label(RichText::new(reason).small().color(Color32::GRAY));
        }
    }
}

fn ping_color(ping_ms: u32) -> Color32 {
    if ping_ms <= PING_GOOD_MS {
        Color32::LIGHT_GREEN
    } else if ping_ms <= PING_FAIR_MS {
        Color32::YELLOW
    } else {
        Color32::LIGHT_RED
    }
}

fn draw_buttons(ui: &mut egui::Ui, view: &BrowserView<'_>) -> Option<MpAction> {
    let mut action = None;
    let selected = view.selected.filter(|index| *index < view.rows.len());
    let joinable = selected.is_some_and(|index| view.rows[index].joinable);

    // Both rows span the same distance: N buttons and N-1 gaps, solved for the
    // one unknown. Read from `spacing` rather than assumed, so a restyled gap
    // keeps the rows aligned instead of quietly knocking them apart.
    let gap = ui.spacing().item_spacing.x;
    let row_width = BUTTON[0] * TOP_BUTTONS + gap * (TOP_BUTTONS - 1.0);
    let small_button = [
        (row_width - gap * (BOTTOM_BUTTONS - 1.0)) / BOTTOM_BUTTONS,
        SMALL_BUTTON_HEIGHT,
    ];

    ui.allocate_ui(egui::vec2(row_width, 0.0), |ui| {
        ui.horizontal(|ui| {
            ui.add_enabled_ui(joinable, |ui| {
                if ui
                    .add_sized(BUTTON, egui::Button::new("Join Server"))
                    .clicked()
                    && let Some(index) = selected
                {
                    action = Some(MpAction::Join(index));
                }
            });
            if ui
                .add_sized(BUTTON, egui::Button::new("Direct Connect"))
                .clicked()
            {
                action = Some(MpAction::OpenDirect);
            }
            ui.add_enabled_ui(!view.refreshing, |ui| {
                let label = if view.refreshing {
                    "Refreshing..."
                } else {
                    "Refresh"
                };
                if ui.add_sized(BUTTON, egui::Button::new(label)).clicked() {
                    action = Some(MpAction::Refresh);
                }
            });
        });

        ui.add_space(6.0);

        ui.horizontal(|ui| {
            if ui
                .add_sized(small_button, egui::Button::new("Add Server"))
                .clicked()
            {
                action = Some(MpAction::OpenAdd);
            }
            ui.add_enabled_ui(selected.is_some(), |ui| {
                if ui
                    .add_sized(small_button, egui::Button::new("Edit"))
                    .clicked()
                    && let Some(index) = selected
                {
                    action = Some(MpAction::OpenEdit(index));
                }
                let confirming = selected.is_some() && view.confirming_delete == selected;
                let label = if confirming { "Confirm?" } else { "Delete" };
                if ui
                    .add_sized(small_button, egui::Button::new(label))
                    .clicked()
                    && let Some(index) = selected
                {
                    action = Some(MpAction::Delete(index));
                }
            });
            if ui
                .add_sized(small_button, egui::Button::new("Back"))
                .clicked()
            {
                action = Some(MpAction::Back);
            }
        });
    });

    action
}

fn draw_dialog(ctx: &Context, dialog: Dialog, fields: &mut DialogFields) -> Option<MpAction> {
    let (title, confirm_label, named) = match dialog {
        Dialog::Add => ("Add Server", "Done", true),
        Dialog::Edit(_) => ("Edit Server", "Save", true),
        Dialog::Direct => ("Direct Connect", "Connect", false),
    };

    let mut action = None;
    let frame = egui::Frame::new()
        .fill(ctx.style().visuals.window_fill)
        .stroke(ctx.style().visuals.window_stroke)
        .inner_margin(16.0)
        .corner_radius(6);

    egui::Window::new(title)
        .title_bar(false)
        .collapsible(false)
        .resizable(false)
        .frame(frame)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(RichText::new(title).strong());
                ui.add_space(10.0);

                if named {
                    ui.add_sized(
                        [FIELD_WIDTH, 24.0],
                        egui::TextEdit::singleline(&mut fields.name).hint_text("Server name"),
                    );
                    ui.add_space(6.0);
                }

                let address = ui.add_sized(
                    [FIELD_WIDTH, 24.0],
                    egui::TextEdit::singleline(&mut fields.address)
                        .hint_text("Server address (host or host:port)"),
                );
                // The address is the field that matters, so it takes focus and
                // Enter submits from it — typing an address and pressing return
                // is the whole interaction.
                if !address.has_focus() && !named {
                    address.request_focus();
                }
                if address.lost_focus() && ui.input(|i| i.key_pressed(Key::Enter)) {
                    action = Some(MpAction::Confirm);
                }

                ui.add_space(12.0);
                ui.horizontal(|ui| {
                    if ui
                        .add_sized(DIALOG_BUTTON, egui::Button::new(confirm_label))
                        .clicked()
                    {
                        action = Some(MpAction::Confirm);
                    }
                    if ui
                        .add_sized(DIALOG_BUTTON, egui::Button::new("Cancel"))
                        .clicked()
                    {
                        action = Some(MpAction::Cancel);
                    }
                });
            });
        });

    if ctx.input(|i| i.key_pressed(Key::Escape)) {
        action = Some(MpAction::Cancel);
    }
    action
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The colour is the only thing that tells a player at a glance whether a
    /// server is worth joining, so the thresholds are worth pinning.
    #[test]
    fn ping_colour_worsens_as_the_round_trip_grows() {
        assert_eq!(ping_color(0), Color32::LIGHT_GREEN);
        assert_eq!(ping_color(PING_GOOD_MS), Color32::LIGHT_GREEN);
        assert_eq!(ping_color(PING_GOOD_MS + 1), Color32::YELLOW);
        assert_eq!(ping_color(PING_FAIR_MS), Color32::YELLOW);
        assert_eq!(ping_color(PING_FAIR_MS + 1), Color32::LIGHT_RED);
    }

    #[test]
    fn a_rows_subtitle_names_the_world_when_it_differs_from_the_label() {
        let row = ServerRow {
            name: "Friend's box",
            address: "example.com:25565",
            state: RowState::Online {
                world: "Cliffs",
                online: 1,
                max: 17,
                ping_ms: 10,
                compatible: true,
            },
            joinable: true,
        };
        assert_eq!(subtitle(&row), "example.com:25565 · Cliffs");
    }

    /// A server the player named after its world should not read
    /// "Cliffs · Cliffs".
    #[test]
    fn a_rows_subtitle_does_not_repeat_a_matching_name() {
        let row = ServerRow {
            name: "Cliffs",
            address: "example.com:25565",
            state: RowState::Online {
                world: "Cliffs",
                online: 1,
                max: 17,
                ping_ms: 10,
                compatible: true,
            },
            joinable: true,
        };
        assert_eq!(subtitle(&row), "example.com:25565");
    }
}

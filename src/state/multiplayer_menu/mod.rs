//! Multiplayer: the player's saved servers, and what is happening on them.
//!
//! Three pieces, deliberately apart:
//!
//! * [`browser`] is the state and the rules — the list, the selection, what a
//!   delete confirms and which rows may be joined. No egui, fully tested.
//! * [`crate::ui::multiplayer_menu`] draws it and reports what was clicked.
//! * [`MultiplayerMenuState`] is this file: it owns the two, holds the text the
//!   dialogs are being typed into, and turns an action into either a call on
//!   the browser or a [`Transition`].
//!
//! Hosting is not here. It moved to the singleplayer menu, next to the world it
//! opens — the two questions "which of my worlds do I want to share" and "whose
//! world do I want to visit" have nothing to do with each other.

pub mod browser;

pub use browser::{RowState, ServerBrowser, ServerRow};

use super::connecting_state::ConnectingState;
use super::{GameState, MainMenuState, StateContext, Transition, Wyvencraft};
use crate::net::serverlist::FileServerStore;
use crate::net::status::NetStatusProbe;
use crate::ui::multiplayer_menu::{BrowserView, Dialog, DialogFields, MpAction, draw_multiplayer};

pub struct MultiplayerMenuState {
    browser: ServerBrowser,
    /// The dialog over the list, if one is open.
    dialog: Option<Dialog>,
    /// What that dialog is being typed into. Owned here because a `TextEdit`
    /// has to own its buffer, and the view is stateless.
    fields: DialogFields,
}

impl MultiplayerMenuState {
    /// The real browser: `servers.toml` and a probe that speaks to real hosts.
    pub fn new(ctx: &StateContext) -> Self {
        Self::with(ServerBrowser::new(
            Box::new(FileServerStore::new()),
            Box::new(NetStatusProbe::new(&ctx.shared.account)),
            ctx.shared.content.hash,
        ))
    }

    /// Around an already-built browser, so tests (and any future caller with a
    /// different store) need no filesystem.
    pub fn with(browser: ServerBrowser) -> Self {
        Self {
            browser,
            dialog: None,
            fields: DialogFields::default(),
        }
    }

    fn open(&mut self, dialog: Dialog) {
        self.fields.clear();
        if let Dialog::Edit(index) = dialog
            && let Some(entry) = self.browser.entry(index)
        {
            self.fields.name = entry.name.clone();
            self.fields.address = entry.address.clone();
        }
        self.dialog = Some(dialog);
    }

    /// Apply the open dialog's primary button. Returns a transition when it was
    /// a Direct Connect, which leaves this screen behind.
    fn confirm(&mut self, ctx: &mut StateContext) -> Transition {
        let Some(dialog) = self.dialog else {
            return Transition::None;
        };
        let outcome = match dialog {
            Dialog::Add => self.browser.add(&self.fields.name, &self.fields.address),
            Dialog::Edit(index) => {
                self.browser
                    .update(index, &self.fields.name, &self.fields.address)
            }
            // Not saved, and not resolved here either: the connecting screen
            // does the lookup on its own worker.
            Dialog::Direct => {
                let address = self.fields.address.trim().to_string();
                if address.is_empty() {
                    Err("Enter a server address".to_string())
                } else {
                    self.dialog = None;
                    return Transition::Replace(Box::new(ConnectingState::new(
                        address,
                        &ctx.shared.account,
                    )));
                }
            }
        };
        // Left open on failure, with the typed text still in it — the browser's
        // error label says what was wrong with it.
        if outcome.is_ok() {
            self.dialog = None;
        }
        Transition::None
    }

    fn apply(&mut self, action: MpAction, ctx: &mut StateContext) -> Transition {
        match action {
            MpAction::Select(index) => self.browser.select(index),
            MpAction::Join(index) => {
                if let Some(target) = self.browser.target(index) {
                    return Transition::Replace(Box::new(ConnectingState::new(
                        target,
                        &ctx.shared.account,
                    )));
                }
            }
            MpAction::Refresh => self.browser.refresh(),
            MpAction::OpenAdd => self.open(Dialog::Add),
            MpAction::OpenEdit(index) => self.open(Dialog::Edit(index)),
            MpAction::OpenDirect => self.open(Dialog::Direct),
            MpAction::Delete(index) => {
                self.browser.remove(index);
            }
            MpAction::Confirm => return self.confirm(ctx),
            MpAction::Cancel => self.dialog = None,
            MpAction::Back => return Transition::Replace(Box::new(MainMenuState::new())),
        }
        Transition::None
    }
}

impl GameState<Wyvencraft> for MultiplayerMenuState {
    fn name(&self) -> &'static str {
        "MultiplayerMenu"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = false;
        // Every in-flight query advances here rather than in `ui`, so a probe
        // keeps making progress on a frame the list happens not to be drawn.
        self.browser.tick(ctx.dt);
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        let rows = self.browser.rows();
        let view = BrowserView {
            rows: &rows,
            selected: self.browser.selected(),
            confirming_delete: (0..rows.len()).find(|i| self.browser.is_confirming_delete(*i)),
            refreshing: self.browser.is_refreshing(),
            error: self.browser.error(),
        };
        let action = draw_multiplayer(egui_ctx, view, self.dialog, &mut self.fields);
        drop(rows);

        match action {
            Some(action) => self.apply(action, ctx),
            None => Transition::None,
        }
    }
}

//! Main menu: choose singleplayer / multiplayer / quit.

use super::{
    GameState, MultiplayerMenuState, SingleplayerMenuState, StateContext, Transition, Wyvencraft,
};

#[derive(Default)]
pub struct MainMenuState;

impl MainMenuState {
    pub fn new() -> Self {
        Self
    }
}

impl GameState<Wyvencraft> for MainMenuState {
    fn name(&self) -> &'static str {
        "MainMenu"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        // Menus run with a free cursor.
        ctx.grab_cursor = false;
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        let mut transition = Transition::None;
        let account = &ctx.shared.account;
        let signed_in_as = account.username();
        let multiplayer_available = account.can_play_multiplayer();

        egui::CentralPanel::default().show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.add_space(70.0);
                ui.heading(egui::RichText::new("Wyvencraft").size(48.0));

                ui.add_space(6.0);
                match &signed_in_as {
                    Some(username) => {
                        ui.label(egui::RichText::new(format!("Signed in as {username}")).weak());
                    }
                    None => {
                        ui.label(egui::RichText::new("Playing offline").weak());
                    }
                }

                ui.add_space(36.0);

                let button = |ui: &mut egui::Ui, label: &str| {
                    ui.add_sized([220.0, 36.0], egui::Button::new(label))
                        .clicked()
                };

                if button(ui, "Singleplayer") {
                    transition = Transition::Replace(Box::new(SingleplayerMenuState::new()));
                }
                ui.add_space(8.0);

                // Greyed out rather than hidden: a player who expected
                // multiplayer should see why it is unavailable, not wonder where
                // it went. `can_play_multiplayer` is the single source of truth
                // — the connect path checks the same thing.
                ui.add_enabled_ui(multiplayer_available, |ui| {
                    let response = ui.add_sized([220.0, 36.0], egui::Button::new("Multiplayer"));
                    if response.clicked() {
                        transition = Transition::Replace(Box::new(MultiplayerMenuState::new(ctx)));
                    }
                    if !multiplayer_available {
                        response.on_hover_text("Sign in to play with other people.");
                    }
                });
                ui.add_space(8.0);

                if button(ui, "Quit") {
                    transition = Transition::Quit;
                }

                ui.add_space(20.0);
                // Signing *in* is not offered: there is no login screen to send
                // anyone to. The launcher owns that, and hands the session over
                // through `profile.toml`. Signing out stays, because forgetting
                // a session on a shared machine has to be possible from here.
                if signed_in_as.is_some() {
                    if ui.link("Sign out").clicked() {
                        account.sign_out();
                        // The stored refresh token goes with it, or the next
                        // launch would silently restore the session.
                        if let Err(err) = crate::save::store_account(None) {
                            log::warn!("could not clear the stored account: {err}");
                        }
                    }
                } else {
                    ui.label(
                        egui::RichText::new("Sign in from the Wyvencraft launcher")
                            .weak()
                            .small(),
                    );
                }
            });
        });
        transition
    }
}

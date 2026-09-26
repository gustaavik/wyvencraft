//! What the in-game screen paints over the world each frame: the death
//! screen, the inventory panel, the HUD and the debug overlay.

use super::InGameState;
use super::frame::format_time_of_day;
use crate::presentation::screens::StateContext;
use crate::presentation::ui::hud;

/// The death screen. True when Respawn was clicked.
pub(super) fn draw_death_screen(egui_ctx: &egui::Context) -> bool {
    let mut respawn = false;
    egui::Area::new(egui::Id::new("death_screen"))
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .show(egui_ctx, |ui| {
            ui.vertical_centered(|ui| {
                ui.label(
                    egui::RichText::new("You died")
                        .size(40.0)
                        .color(egui::Color32::from_rgb(220, 40, 40)),
                );
                ui.add_space(12.0);
                if ui
                    .add_sized([180.0, 40.0], egui::Button::new("Respawn"))
                    .clicked()
                {
                    respawn = true;
                }
            });
        });
    respawn
}

impl InGameState {
    /// The inventory and, in survival, the crafting pane above it — and
    /// whatever the player did with them.
    pub(super) fn draw_inventory_panel(
        &mut self,
        egui_ctx: &egui::Context,
        ctx: &mut StateContext,
    ) {
        use crate::presentation::ui::inventory::InvAction;

        let entries = self.crafting_entries();
        let crafting_view = (!self.sim.player.mode.is_creative()).then(|| {
            crate::presentation::ui::crafting::CraftingView {
                entries: &entries,
                discovered: self.crafting.revealed.len(),
                total: self.sim.recipes.recipes().len(),
                stations: &self.crafting.stations,
                nearby: &self.crafting.nearby,
                selected: self.crafting.selected,
                craftable_only: self.crafting.craftable_only,
                inventory: &self.sim.inventory,
                items: &self.content.rules.items,
                icons: &ctx.shared.content.visuals.item_icons,
                names: &ctx.shared.content.visuals.item_display_names,
                tex: ctx.shared.ui_tex,
            }
        });
        let out = crate::presentation::ui::inventory::draw_inventory(
            egui_ctx,
            &self.sim.inventory,
            &self.content.rules.items,
            &ctx.shared.content.visuals.item_icons,
            &ctx.shared.content.visuals.item_display_names,
            self.sim.held,
            self.sim.player.mode,
            self.inventory_anim.progress(),
            // The panel has the keyboard to itself here: it holds no text
            // field, so egui consumes nothing and the binding still lands.
            ctx.input
                .just_pressed(ctx.shared.settings.controls.keybinds.drop_item),
            ctx.shared.ui_tex,
            crafting_view.as_ref(),
        );
        if let Some(action) = out {
            match action {
                InvAction::Slot(index) => self.handle_slot_click(index),
                InvAction::Split(index) => self.handle_slot_split(index),
                InvAction::Pick(id) => {
                    self.sim.held = Some(self.content.rules.items.full_stack(id))
                }
                InvAction::DropSlot(index) => self.drop_slot(index),
                InvAction::DropHeld { all } => self.drop_held(all),
                InvAction::DropOne(index) => self.drop_one(index),
                InvAction::Craft(craft) => self.handle_craft_action(craft),
            }
        }
    }

    /// The playing HUD: nameplates, chat, crosshair, hotbar, the held item's
    /// name, mode, compass, boss bar and — in survival — vitals.
    pub(super) fn draw_hud(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) {
        // Before the HUD, so a name can never sit on top of the hotbar or the
        // vitals.
        self.draw_nameplates(egui_ctx, ctx.aspect);

        self.draw_chat(egui_ctx);

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(
            egui_ctx,
            &self.sim.inventory,
            &self.content.rules.items,
            &ctx.shared.content.visuals.item_icons,
            ctx.shared.ui_tex,
        );
        // Name whatever is in hand, until it fades. Observing here rather than
        // wherever the selection changes catches every route into the hand —
        // scrolling, the number keys, picking a block up, a tool breaking.
        let survival = self.sim.player.mode.takes_damage();
        self.held_label
            .observe(self.sim.inventory.item_in_selected());
        self.held_label.tick(ctx.dt);
        if let Some((item, alpha)) = self.held_label.visible() {
            let name = ctx.shared.content.item_display_name(item);
            hud::draw_held_label(egui_ctx, name, alpha, survival);
        }
        hud::draw_mode_indicator(egui_ctx, self.sim.player.mode.label());
        crate::presentation::ui::compass::draw_compass(
            egui_ctx,
            self.sim.player.yaw,
            &self.waypoints(),
        );
        if let Some(bar) = self.boss_bar() {
            crate::presentation::ui::boss_bar::draw_boss_bar(egui_ctx, &bar);
        }

        // Survival HUD: vitals and break progress.
        if survival {
            hud::draw_vitals(
                egui_ctx,
                self.sim.player.health,
                self.sim.player.vitals().max_health,
                self.sim.player.hunger,
                self.sim.player.vitals().max_hunger,
            );
        }
    }

    /// The F3 overlay's lines.
    pub(super) fn debug_lines(&self, ctx: &StateContext) -> Vec<String> {
        let fps = if ctx.dt > 0.0 { 1.0 / ctx.dt } else { 0.0 };
        let p = self.sim.player.position;
        let facing = self.sim.player.look_direction();
        let lines = vec![
            format!("Wyvencraft — {fps:.0} fps"),
            format!("xyz: {:.2} {:.2} {:.2}", p.x, p.y, p.z),
            format!("facing: {:.2} {:.2} {:.2}", facing.x, facing.y, facing.z),
            format!(
                "chunks: {} loaded / {} meshes / {} queued / {} pending",
                self.sim.world.loaded_count(),
                self.view.loaded_mesh_count(),
                self.view.queued_mesh_count(),
                self.sim.loader.pending_count()
            ),
            format!("on_ground: {}", self.sim.player.on_ground),
            self.biome_line(),
            format!(
                "mobs: {} live / {} arrows / {} drops",
                self.sim
                    .ecs
                    .count::<crate::application::ecs::components::MobId>(),
                self.sim
                    .ecs
                    .count::<crate::application::ecs::components::Projectile>(),
                self.sim
                    .ecs
                    .count::<crate::application::ecs::components::ItemDrop>()
            ),
            format!("net: {}", self.net_status()),
            format!(
                "time: {}",
                format_time_of_day(self.sim.day_cycle.time_of_day())
            ),
            format!("world: {}", self.save.world_name()),
        ];
        lines
    }
}

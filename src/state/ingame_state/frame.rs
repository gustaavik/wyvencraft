//! The [`GameState`] implementation: the per-frame update, the egui HUD /
//! inventory / death UI, and the scene + preview render frames.

use winit::event::MouseButton;

use super::{AUTOSAVE_INTERVAL, DOUBLE_TAP_WINDOW, InGameState, PREVIEW_DRAG_SENSITIVITY};
use crate::render::{PreviewFrame, SceneFrame};
use crate::state::{GameState, PauseMenuState, StateContext, Transition};
use crate::ui::hud;

impl GameState for InGameState {
    fn name(&self) -> &'static str {
        "InGame"
    }

    fn on_exit(&mut self, _ctx: &mut StateContext) {
        // Fires when pausing (Push), quitting to the menu (ReplaceAll), and on
        // app shutdown (Quit / window close) — every path that leaves the world.
        self.save_world();
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        let kb = ctx.settings.controls.keybinds.clone();
        // While the chat bar is open, egui owns the keyboard and gameplay keys
        // never reach `InputState` at all. These guards cover the one frame
        // between opening the bar and the widget taking focus.
        let typing = self.chat.composer.open;

        if !typing && !self.dead && !self.inventory_open {
            if ctx.input.just_pressed(kb.chat) {
                self.chat.composer.begin("");
            } else if ctx.input.just_pressed(kb.chat_command) {
                self.chat.composer.begin("/");
            }
        }

        if !typing && !self.dead && ctx.input.just_pressed(kb.inventory) {
            self.toggle_inventory();
        }
        // Esc closes the inventory if open, otherwise opens the pause overlay.
        if !typing && ctx.input.just_pressed(kb.pause) {
            if self.inventory_open {
                self.toggle_inventory();
            } else {
                return Transition::Push(Box::new(PauseMenuState::new()));
            }
        }

        if self.inventory_open || self.dead || self.chat.composer.open {
            // Inventory screen / death screen: free cursor, freeze player control,
            // and abandon any in-progress mining.
            ctx.grab_cursor = false;
            self.breaking = None;
        } else {
            ctx.grab_cursor = true;

            if ctx.input.just_pressed(kb.toggle_perspective) {
                self.player.toggle_perspective();
            }
            if ctx.input.just_pressed(kb.toggle_debug) {
                self.show_debug = !self.show_debug;
            }

            // Live game-mode toggle (F4).
            if ctx.input.just_pressed(kb.toggle_gamemode) {
                self.player.set_mode(self.player.mode.toggled());
                self.breaking = None;
                self.broadcast_mode_change();
            }

            // Creative flight: double-tap the jump key within the window.
            self.jump_tap_timer += ctx.dt;
            if ctx.input.just_pressed(kb.jump) {
                if self.player.mode.can_fly() && self.jump_tap_timer < DOUBLE_TAP_WINDOW {
                    self.player.flying = !self.player.flying;
                }
                self.jump_tap_timer = 0.0;
            }

            // Mouse look.
            let sens = ctx.settings.controls.mouse_sensitivity * 0.0025;
            let pitch_sign = if ctx.settings.controls.invert_y {
                1.0
            } else {
                -1.0
            };
            let delta = ctx.input.mouse_delta();
            self.player
                .rotate(delta.x * sens, pitch_sign * delta.y * sens);

            // Hotbar selection via scroll.
            let scroll = ctx.input.scroll_delta();
            if scroll != 0.0 {
                self.inventory.scroll_selected(-scroll.signum() as i32);
            }
            // Hotbar selection via the number keys.
            for (i, key) in kb.hotbar.iter().enumerate() {
                if ctx.input.just_pressed(*key) {
                    self.inventory.set_selected(i);
                }
            }

            // Toss one item from the selected slot onto the ground.
            if ctx.input.just_pressed(kb.drop_item) {
                self.drop_selected_item();
            }

            // Movement + physics. Refresh the worn defense first: `update` can
            // raise fall damage internally, and it must be mitigated by whatever
            // the player is wearing right now.
            let movement = ctx.input.movement(&kb);
            let dt = ctx.dt.min(0.05);
            self.player.defense = self.inventory.total_defense(&self.items);
            let health_before = self.player.health;
            // Player physics is stepped at a fixed rate, not on the frame delta,
            // so jump height is the same at every framerate.
            self.view.render_alpha =
                self.player
                    .step_fixed(movement, ctx.dt, &mut self.physics_accum, |p| {
                        self.world.is_solid_for_collision(p)
                    });
            // A health drop across `update` means fall damage landed; the health
            // delta is the only signal that escapes it, and it's enough.
            if self.player.health < health_before {
                self.inventory.wear_armor(1);
            }

            // Survival vitals: hunger drain, regen, starvation.
            if self.player.mode.takes_damage() {
                self.player.tick_survival(dt, movement.sprint);
                if self.player.is_dead() {
                    self.dead = true;
                    self.breaking = None;
                }
            }

            // Block interaction. The main-hand swing fires on every left click,
            // even when punching air (no block hit).
            if ctx.input.mouse_just_pressed(MouseButton::Left) {
                self.view.trigger_swing();
            }
            // A mob in the crosshair takes the hit (and blocks mining on the
            // block behind it); otherwise the click falls through to blocks.
            let mob_target = self.targeted_mob();
            if self.player.mode.instant_break() {
                // Creative: instant break on click.
                self.breaking = None;
                if ctx.input.mouse_just_pressed(MouseButton::Left) {
                    match mob_target {
                        Some(index) => self.attack_mob(index),
                        None => {
                            if let Some(hit) = self.targeted_block() {
                                self.break_block_at(hit.block);
                            }
                        }
                    }
                }
            } else if let Some(index) = mob_target {
                // Survival with a mob in reach: swing on click, don't mine.
                self.breaking = None;
                if ctx.input.mouse_just_pressed(MouseButton::Left) {
                    self.attack_mob(index);
                }
            } else {
                // Survival: progressive mining while the dig button is held.
                let digging = ctx.input.mouse_held(MouseButton::Left);
                self.update_mining(digging, dt);
            }
            if ctx.input.mouse_just_pressed(MouseButton::Right) {
                self.use_selected();
            }
        }

        self.view.fov_degrees = ctx.settings.render.fov_degrees;
        // Wrap the animation clock so f32 precision never degrades over long
        // sessions. The period must stay a whole multiple of the water loop
        // (WATER_FRAMES / WATER_FPS = 0.8 s in voxel.frag) to wrap seamlessly.
        self.view.elapsed = (self.view.elapsed + ctx.dt) % 3600.0;
        // Ages the chat lines so old ones fade off the HUD.
        self.chat.log.tick(ctx.dt);
        self.day_cycle.advance(ctx.dt);
        // Periodic autosave for persistent worlds (also fires on pause/exit).
        if self.save.is_persistent() {
            self.save.autosave_timer += ctx.dt;
            if self.save.autosave_timer >= AUTOSAVE_INTERVAL {
                self.save.autosave_timer = 0.0;
                self.save_world();
            }
        }
        self.pump_network(ctx.dt);
        // Water flow: singleplayer/host simulate authoritatively and broadcast
        // each change; clients receive them as ordinary BlockChanged edits.
        if self.session.is_authority() {
            for (pos, block) in self.fluids.tick(&mut self.world, ctx.dt) {
                self.broadcast_local_edit(pos, block);
            }
            // Mobs are host-authoritative like fluids; clients only render
            // the replicated copies.
            self.update_mobs(ctx.dt.min(0.05));
            self.update_spawning(ctx.dt);
        }
        self.update_streaming(ctx.settings.render.render_distance);
        // Drops and arrows keep simulating even with the inventory or death
        // screen open.
        self.update_drops(ctx.dt.min(0.05));
        self.update_arrows(ctx.dt.min(0.05));

        // Simulation for this frame is settled; bring the GPU state in line.
        self.refresh_view(ctx.resources.render, ctx.dt.min(0.05));
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        use crate::ui::inventory::InvAction;

        // Death screen takes over everything else.
        if self.dead {
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
            if respawn {
                self.respawn();
            }
            return Transition::None;
        }

        if self.inventory_open {
            let out = crate::ui::inventory::draw_inventory(
                egui_ctx,
                &self.inventory,
                &self.items,
                &self.recipes,
                &ctx.resources.content.item_icons,
                self.held,
                self.player.mode,
                ctx.resources.ui_tex,
            );
            if let Some(look) = out.head_look {
                self.view.preview.look = look;
            }
            if let Some(action) = out.action {
                match action {
                    InvAction::Slot(index) => self.handle_slot_click(index),
                    InvAction::Pick(id) => self.held = Some(self.items.full_stack(id)),
                    InvAction::Craft(index) => self.handle_craft(index),
                    InvAction::Rotate(dx) => {
                        self.view.preview.yaw -= dx * PREVIEW_DRAG_SENSITIVITY;
                    }
                }
            }
            return Transition::None;
        }

        self.draw_chat(egui_ctx);

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(
            egui_ctx,
            &self.inventory,
            &self.items,
            &ctx.resources.content.item_icons,
            ctx.resources.ui_tex,
        );
        hud::draw_mode_indicator(egui_ctx, self.player.mode.label());

        // Survival HUD: vitals and break progress.
        if self.player.mode.takes_damage() {
            hud::draw_vitals(
                egui_ctx,
                self.player.health,
                self.player.vitals().max_health,
                self.player.hunger,
                self.player.vitals().max_hunger,
            );
        }

        if self.show_debug {
            let fps = if ctx.dt > 0.0 { 1.0 / ctx.dt } else { 0.0 };
            let p = self.player.position;
            let facing = self.player.look_direction();
            let lines = vec![
                format!("Wyvencraft — {fps:.0} fps"),
                format!("xyz: {:.2} {:.2} {:.2}", p.x, p.y, p.z),
                format!("facing: {:.2} {:.2} {:.2}", facing.x, facing.y, facing.z),
                format!(
                    "chunks: {} loaded / {} meshes / {} queued / {} pending",
                    self.world.loaded_count(),
                    self.view.loaded_mesh_count(),
                    self.view.queued_mesh_count(),
                    self.loader.pending_count()
                ),
                format!("on_ground: {}", self.player.on_ground),
                format!(
                    "mobs: {} live / {} arrows / {} drops",
                    self.mobs.len() + self.remote_mobs.len(),
                    self.arrows.len(),
                    self.drops.len()
                ),
                format!("net: {}", self.net_status()),
                format!("time: {}", format_time_of_day(self.day_cycle.time_of_day())),
                format!("world: {}", self.save.world_name()),
            ];
            hud::draw_debug(egui_ctx, &lines);
        }

        Transition::None
    }

    fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        Some(self.view.scene_frame(&self.player, &self.day_cycle, aspect))
    }

    fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        self.view.preview_frame()
    }
}

/// Format a normalized time-of-day `[0,1)` (0.0 = midnight) as a 24-hour clock.
fn format_time_of_day(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

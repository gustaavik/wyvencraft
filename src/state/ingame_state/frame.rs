//! The [`GameState`] implementation: the per-frame update, the egui HUD /
//! inventory / death UI, and the scene render frame.

use winit::event::MouseButton;

use super::{AUTOSAVE_INTERVAL, DOUBLE_TAP_WINDOW, InGameState};
use crate::state::{GameState, PauseMenuState, StateContext, Transition, Wyvencraft};
use crate::ui::hud;
use crate::ui::nameplate::{self, Nameplate};
use wyven_render::SceneFrame;

impl InGameState {
    /// Paint every visible player's username above their model.
    ///
    /// The camera is rebuilt here rather than threaded through, and it is
    /// bit-identical to the one the world pass will use: `update` has already
    /// run this frame and set `render_alpha`, and `SceneCache::camera` is a pure
    /// function of the player and that alpha.
    ///
    /// `aspect` comes from egui's screen rect rather than the swapchain. The
    /// scale factor is uniform, so the ratio matches — and `ui` is not given the
    /// physical size.
    fn draw_nameplates(&self, egui_ctx: &egui::Context) {
        if self.peers.players.is_empty() {
            return;
        }

        let screen = egui_ctx.screen_rect();
        if screen.height() <= 0.0 {
            return;
        }
        let aspect = screen.width() / screen.height();
        let camera = self
            .view
            .camera(&self.player, aspect, self.camera_distance(aspect));

        let alpha = self.view.render_alpha;
        let plates: Vec<Nameplate<'_>> = self
            .peers
            .players
            .values()
            .map(|remote| {
                let position = remote.interpolated_position(alpha);
                Nameplate {
                    name: remote.name.as_str(),
                    position,
                    occluded: self.nameplate_occluded(&camera, position),
                }
            })
            .collect();

        nameplate::draw_nameplates(egui_ctx, &camera, plates);
    }

    /// Whether solid world sits between the eye and a player's nameplate.
    ///
    /// egui paints after the world pass with no depth information, so without
    /// this a name reads straight through terrain. The march uses `is_solid` and
    /// `Target::Cell` — the same predicate mob line-of-sight uses — so a flower
    /// or a pane of glass never hides someone.
    fn nameplate_occluded(&self, camera: &wyven_render::Camera, position: glam::Vec3) -> bool {
        let anchor = position + glam::Vec3::Y * nameplate::ANCHOR_HEIGHT;
        let to_anchor = anchor - camera.position;
        let distance = to_anchor.length();
        if distance <= f32::EPSILON {
            return false;
        }

        crate::world::raycast(camera.position, to_anchor, distance, |at| {
            self.world
                .is_solid(at)
                .then_some(crate::world::Target::Cell)
        })
        .is_some()
    }
}

impl GameState<Wyvencraft> for InGameState {
    fn name(&self) -> &'static str {
        "InGame"
    }

    fn on_exit(&mut self, _ctx: &mut StateContext) {
        // Fires when pausing (Push), quitting to the menu (ReplaceAll), and on
        // app shutdown (Quit / window close) — every path that leaves the world.
        self.save_world();
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        let kb = ctx.shared.settings.controls.keybinds.clone();
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

        // One tick per frame, here rather than in `ui`: `scene_frame` is `&self`
        // and reads the same progress, so it must already be advanced by the
        // time the world camera is derived.
        self.inventory_anim.tick(ctx.dt);

        if self.inventory_anim.active() || self.dead || self.chat.composer.open {
            // Inventory screen / death screen: free cursor, freeze player control,
            // and abandon any in-progress mining. Keyed off the *animation*, so
            // control stays frozen through the close sweep and comes back only
            // once the camera is back on the eye — where there is nothing left
            // to blend and the hand-back is seamless.
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
            let sens = ctx.shared.settings.controls.mouse_sensitivity * 0.0025;
            let pitch_sign = if ctx.shared.settings.controls.invert_y {
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
            let movement = crate::config::movement(ctx.input, &kb);
            let dt = ctx.dt.min(0.05);
            self.player.defense = self.inventory.total_defense(&self.content.items);
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

        self.view.fov_degrees = ctx.shared.settings.render.fov_degrees;
        // Wrap the animation clock so f32 precision never degrades over long
        // sessions. The period must stay a whole multiple of every animated
        // texture's loop or the wrap skips a frame: `voxel_array.frag` steps a
        // layer at `fps`, so what has to divide 3600 * fps is the frame count
        // (water's `[block.fluid.texture]` is 64 frames at 8 fps, and the
        // blocks loader refuses a pairing that does not divide evenly).
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
            for (pos, block) in self
                .fluids
                .tick(&mut self.world, &self.content.blocks, ctx.dt)
            {
                self.broadcast_local_edit(pos, block);
            }
            // Mobs are host-authoritative like fluids; clients only render
            // the replicated copies.
            self.update_mobs(ctx.dt.min(0.05));
            self.update_spawning(ctx.dt);
        }
        self.update_streaming(ctx.shared.settings.render.render_distance);
        // Drops and arrows keep simulating even with the inventory or death
        // screen open.
        self.update_drops(ctx.dt.min(0.05));
        self.update_arrows(ctx.dt.min(0.05));

        // Simulation for this frame is settled; bring the GPU state in line.
        self.refresh_view(&ctx.shared.render, ctx.dt.min(0.05));
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

        // The panel is drawn for the whole sweep, not just while open, because
        // the close animation runs after `inventory_open` has already gone
        // false. It takes over from the HUD hotbar rather than covering it: the
        // two are the same nine slots and at progress 0 they coincide exactly,
        // so exactly one of them is drawn and the swap is invisible.
        if self.inventory_anim.active() {
            let out = crate::ui::inventory::draw_inventory(
                egui_ctx,
                &self.inventory,
                &self.content.items,
                &ctx.shared.content.item_icons,
                &ctx.shared.content.item_display_names,
                self.held,
                self.player.mode,
                self.inventory_anim.progress(),
                ctx.shared.ui_tex,
            );
            if let Some(action) = out {
                match action {
                    InvAction::Slot(index) => self.handle_slot_click(index),
                    InvAction::Pick(id) => self.held = Some(self.content.items.full_stack(id)),
                }
            }
            return Transition::None;
        }

        // Before the HUD, so a name can never sit on top of the hotbar or the
        // vitals.
        self.draw_nameplates(egui_ctx);

        self.draw_chat(egui_ctx);

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(
            egui_ctx,
            &self.inventory,
            &self.content.items,
            &ctx.shared.content.item_icons,
            ctx.shared.ui_tex,
        );
        // Name whatever is in hand, until it fades. Observing here rather than
        // wherever the selection changes catches every route into the hand —
        // scrolling, the number keys, picking a block up, a tool breaking.
        let survival = self.player.mode.takes_damage();
        self.held_label.observe(self.inventory.item_in_selected());
        self.held_label.tick(ctx.dt);
        if let Some((item, alpha)) = self.held_label.visible() {
            let name = ctx.shared.content.item_display_name(item);
            hud::draw_held_label(egui_ctx, name, alpha, survival);
        }
        hud::draw_mode_indicator(egui_ctx, self.player.mode.label());

        // Survival HUD: vitals and break progress.
        if survival {
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
                    self.mobs.live.len() + self.mobs.remote.len(),
                    self.mobs.arrows.len(),
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
        Some(self.view.scene_frame(
            &self.player,
            &self.day_cycle,
            aspect,
            self.camera_distance(aspect),
        ))
    }
}

/// Format a normalized time-of-day `[0,1)` (0.0 = midnight) as a 24-hour clock.
fn format_time_of_day(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

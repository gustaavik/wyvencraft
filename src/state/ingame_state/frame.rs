//! The [`GameState`] implementation: the per-frame update, the egui HUD /
//! inventory / death UI, and the scene + preview render frames.

use glam::Vec3;
use winit::event::MouseButton;

use super::{
    AUTOSAVE_INTERVAL, DOUBLE_TAP_WINDOW, InGameState, NetRole, PREVIEW_DRAG_SENSITIVITY,
    THIRD_PERSON_DISTANCE,
};
use crate::core::{Aabb, CHUNK_HEIGHT, CHUNK_SIZE, ChunkPos};
use crate::entity::Perspective;
use crate::render::{Camera, GpuMesh, LightParams, PreviewFrame, SceneFrame, SkyParams};
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

        if !self.dead && ctx.input.just_pressed(kb.inventory) {
            self.toggle_inventory();
        }
        // Esc closes the inventory if open, otherwise opens the pause overlay.
        if ctx.input.just_pressed(kb.pause) {
            if self.inventory_open {
                self.toggle_inventory();
            } else {
                return Transition::Push(Box::new(PauseMenuState::new()));
            }
        }

        if self.inventory_open || self.dead {
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
            self.player
                .update(movement, dt, |p| self.world.is_solid_for_collision(p));
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
                self.player_anim.trigger_swing();
            }
            if self.player.mode.instant_break() {
                // Creative: instant break on click.
                self.breaking = None;
                if ctx.input.mouse_just_pressed(MouseButton::Left)
                    && let Some(hit) = self.targeted_block()
                {
                    self.break_block_at(hit.block);
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

        self.fov_degrees = ctx.settings.render.fov_degrees;
        // Wrap the animation clock so f32 precision never degrades over long
        // sessions. The period must stay a whole multiple of the water loop
        // (WATER_FRAMES / WATER_FPS = 0.8 s in voxel.frag) to wrap seamlessly.
        self.elapsed = (self.elapsed + ctx.dt) % 3600.0;
        self.day_cycle.advance(ctx.dt);
        // Periodic autosave for persistent worlds (also fires on pause/exit).
        if self.save.is_some() {
            self.autosave_timer += ctx.dt;
            if self.autosave_timer >= AUTOSAVE_INTERVAL {
                self.autosave_timer = 0.0;
                self.save_world();
            }
        }
        self.update_break_overlay(ctx.render);
        self.update_target_outline(ctx.render);
        self.pump_network(ctx.dt);
        // Water flow: singleplayer/host simulate authoritatively and broadcast
        // each change; clients receive them as ordinary BlockChanged edits.
        if !matches!(self.net, NetRole::Client { .. }) {
            for (pos, block) in self.fluids.tick(&mut self.world, ctx.dt) {
                self.broadcast_local_edit(pos, block);
            }
        }
        self.update_streaming(ctx.settings.render.render_distance);
        self.enqueue_dirty();
        self.process_mesh_budget(ctx.render);
        // Drops keep simulating even with the inventory or death screen open.
        self.update_drops(ctx.dt.min(0.05));
        self.update_drops_mesh(ctx.render);

        // Advance + rebuild animated player models. The local player settles to idle
        // while the inventory is open (movement is frozen).
        let anim_dt = ctx.dt.min(0.05);
        let local_speed = if self.inventory_open {
            0.0
        } else {
            let v = self.player.velocity;
            Vec3::new(v.x, 0.0, v.z).length()
        };
        self.player_anim.advance(local_speed, anim_dt);
        self.update_player_mesh(ctx.render);
        self.update_preview_mesh(ctx.render);
        self.update_remote_meshes(ctx.render, anim_dt);
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
                &ctx.content.item_icons,
                self.held,
                self.player.mode,
                ctx.ui_tex,
            );
            if let Some(look) = out.head_look {
                self.preview_look = look;
            }
            if let Some(action) = out.action {
                match action {
                    InvAction::Slot(index) => self.handle_slot_click(index),
                    InvAction::Pick(id) => self.held = Some(self.items.full_stack(id)),
                    InvAction::Craft(index) => self.handle_craft(index),
                    InvAction::Rotate(dx) => {
                        self.preview_yaw -= dx * PREVIEW_DRAG_SENSITIVITY;
                    }
                }
            }
            return Transition::None;
        }

        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(
            egui_ctx,
            &self.inventory,
            &self.items,
            &ctx.content.item_icons,
            ctx.ui_tex.atlas,
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
        if let Some(breaking) = &self.breaking {
            hud::draw_break_progress(egui_ctx, breaking.progress);
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
                    self.meshes.len(),
                    self.mesh_queue.len(),
                    self.loader.pending_count()
                ),
                format!("on_ground: {}", self.player.on_ground),
                format!("net: {}", self.net_status()),
                format!("time: {}", format_time_of_day(self.day_cycle.time_of_day())),
                format!(
                    "world: {}",
                    self.save
                        .as_ref()
                        .map(|s| s.meta.name.as_str())
                        .unwrap_or("(unsaved)")
                ),
            ];
            hud::draw_debug(egui_ctx, &lines);
        }

        Transition::None
    }

    fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        let mut camera = Camera::new(self.fov_degrees, aspect);
        let eye = self.player.eye_position();
        let look = self.player.look_direction();
        match self.player.perspective {
            Perspective::First => {
                camera.position = eye;
                camera.forward = look;
            }
            Perspective::ThirdBack => {
                camera.position = eye - look * THIRD_PERSON_DISTANCE;
                camera.forward = look;
            }
            Perspective::ThirdFront => {
                camera.position = eye + look * THIRD_PERSON_DISTANCE;
                camera.forward = -look;
            }
        }
        let frustum = camera.frustum();
        let in_view = |pos: &ChunkPos| {
            let origin = pos.origin();
            let aabb = Aabb::new(
                Vec3::new(origin.x as f32, 0.0, origin.z as f32),
                Vec3::new(
                    (origin.x + CHUNK_SIZE) as f32,
                    CHUNK_HEIGHT as f32,
                    (origin.z + CHUNK_SIZE) as f32,
                ),
            );
            frustum.intersects_aabb(aabb)
        };

        // Frustum-cull chunk meshes by their column AABB.
        let mut opaque: Vec<&GpuMesh> = self
            .meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();
        let mut transparent: Vec<&GpuMesh> = self
            .transparent_meshes
            .iter()
            .filter(|(pos, _)| in_view(pos))
            .map(|(_, mesh)| mesh)
            .collect();

        // Crack overlay on the block being mined, blended over everything else.
        // No frustum check: the target is a single nearby block within reach.
        if let Some(mesh) = &self.break_mesh {
            transparent.push(mesh);
        }

        // The local player model (third person only) + remote players.
        if let Some(mesh) = &self.player_mesh {
            opaque.push(mesh);
        }
        for mesh in &self.remote_meshes {
            opaque.push(mesh);
        }

        // Dropped items, split by pass like the blocks they represent.
        if let Some(mesh) = &self.drops_mesh {
            opaque.push(mesh);
        }
        if let Some(mesh) = &self.drops_mesh_transparent {
            transparent.push(mesh);
        }

        let atmo = self.day_cycle.atmosphere();
        let sky = SkyParams {
            inv_view_proj: camera.sky_inv_view_proj(),
            sun_dir: atmo.sun_dir,
            zenith_color: atmo.zenith_color,
            horizon_color: atmo.horizon_color,
            sun_color: atmo.sun_color,
            star_intensity: atmo.star_intensity,
            moon_intensity: atmo.moon_intensity,
        };
        let light = LightParams {
            light_dir: atmo.light_dir,
            light_color: atmo.light_color,
            ambient: atmo.ambient,
        };

        Some(SceneFrame {
            view_proj: camera.view_projection(),
            sky,
            light,
            time: self.elapsed,
            opaque,
            transparent,
            lines: self.outline_mesh.as_ref(),
        })
    }

    fn preview_frame(&self) -> Option<PreviewFrame<'_>> {
        let model = self.preview_mesh.as_ref()?;
        // Fixed head-height orbit looking at the model built at the origin.
        // The preview image is 0.48:1 (see PREVIEW_SIZE in app.rs).
        let target = Vec3::new(0.0, 1.05, 0.0);
        let mut camera = Camera::new(32.0, 0.48);
        camera.position = Vec3::new(0.0, 1.05, 3.9);
        camera.forward = (target - camera.position).normalize();
        // Neutral, mostly-ambient light so the model reads clearly against the
        // dark preview backdrop, independent of the world's time of day.
        let light = LightParams {
            light_dir: Vec3::new(0.3, 0.8, 0.5).normalize(),
            light_color: Vec3::splat(0.9),
            ambient: 0.55,
        };
        Some(PreviewFrame {
            view_proj: camera.view_projection(),
            light,
            model,
        })
    }
}

/// Format a normalized time-of-day `[0,1)` (0.0 = midnight) as a 24-hour clock.
fn format_time_of_day(t: f32) -> String {
    let minutes = (t.rem_euclid(1.0) * 24.0 * 60.0) as u32;
    format!("{:02}:{:02}", (minutes / 60) % 24, minutes % 60)
}

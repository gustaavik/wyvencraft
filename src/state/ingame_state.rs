//! The playing state: owns the world, the local player, and the inventory, and
//! ties together input → simulation. (Rendering is attached when the renderer is
//! wired into the app in later milestones; the 3D pass reads this state's data.)

use std::collections::HashMap;
use std::sync::Arc;

use glam::Vec3;
use winit::event::MouseButton;

use super::{GameState, PauseMenuState, StateContext, Transition};
use crate::core::{BlockPos, ChunkPos, CHUNK_HEIGHT};
use crate::entity::Player;
use crate::inventory::{Inventory, ItemRegistry, ItemStack};
use crate::render::{Camera, GpuMesh, RenderContext, SceneFrame};
use crate::ui::hud;
use crate::world::block::blocks;
use crate::world::meshing::mesh_chunk;
use crate::world::{BlockRegistry, NoiseGenerator, World};

/// Reach distance for breaking/placing blocks.
const REACH: f32 = 5.0;
/// Chunks kept generated synchronously each frame (small until the M3 async
/// loader takes over).
const SYNC_LOAD_RADIUS: i32 = 3;

pub struct InGameState {
    pub world: World,
    pub player: Player,
    pub blocks: Arc<BlockRegistry>,
    pub items: ItemRegistry,
    pub inventory: Inventory,
    pub show_debug: bool,
    /// GPU meshes for loaded chunks, keyed by chunk position.
    chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    fov_degrees: f32,
}

impl InGameState {
    pub fn new(seed: u64) -> Self {
        let blocks = Arc::new(BlockRegistry::with_builtins());
        let items = ItemRegistry::from_blocks(&blocks);
        let generator = Box::new(NoiseGenerator::new(seed));
        let mut world = World::new(generator, blocks.clone());

        // Generate spawn surroundings and drop the player on the surface.
        let center = ChunkPos::new(0, 0);
        for dx in -SYNC_LOAD_RADIUS..=SYNC_LOAD_RADIUS {
            for dz in -SYNC_LOAD_RADIUS..=SYNC_LOAD_RADIUS {
                world.ensure_chunk(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
        let spawn = find_spawn(&world);

        // Creative-style starter hotbar so placing blocks works immediately.
        let mut inventory = Inventory::new();
        let starter = [
            blocks::STONE,
            blocks::DIRT,
            blocks::GRASS,
            blocks::SAND,
            blocks::WOOD,
            blocks::LEAVES,
            blocks::GLASS,
            blocks::SNOW,
            blocks::BEDROCK,
        ];
        for (slot, block) in starter.iter().enumerate() {
            if let Some(item) = items.item_for_block(*block) {
                inventory.set_slot(slot, Some(ItemStack::new(item, 64)));
            }
        }

        Self {
            world,
            player: Player::new(spawn),
            blocks,
            items,
            inventory,
            show_debug: false,
            chunk_meshes: HashMap::new(),
            fov_degrees: 70.0,
        }
    }

    /// Rebuild GPU meshes for any chunks whose blocks changed this frame.
    fn rebuild_dirty_meshes(&mut self, ctx: &Arc<RenderContext>) {
        for pos in self.world.take_dirty() {
            let output = self
                .world
                .chunk(pos)
                .map(|chunk| mesh_chunk(chunk, &self.blocks, |p| self.world.block_at(p)));
            match output {
                Some(output) => match GpuMesh::upload(&ctx.memory_allocator, &output.opaque) {
                    Ok(Some(mesh)) => {
                        self.chunk_meshes.insert(pos, mesh);
                    }
                    // Empty chunk (all air / fully occluded): nothing to draw.
                    Ok(None) => {
                        self.chunk_meshes.remove(&pos);
                    }
                    Err(err) => log::error!("chunk mesh upload failed at {pos:?}: {err:?}"),
                },
                None => {
                    self.chunk_meshes.remove(&pos);
                }
            }
        }
    }

    /// Stream chunks around the player (synchronous placeholder for the M3 loader).
    fn update_loaded_chunks(&mut self, radius: i32) {
        let center = BlockPos::from_world(self.player.position).chunk();
        let radius = radius.min(SYNC_LOAD_RADIUS);
        for dx in -radius..=radius {
            for dz in -radius..=radius {
                self.world
                    .ensure_chunk(ChunkPos::new(center.x + dx, center.z + dz));
            }
        }
    }

    /// Break the block the player is looking at; drop it into the inventory.
    fn try_break_block(&mut self) {
        let hit = crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            REACH,
            |p| self.world.is_solid(p),
        );
        if let Some(hit) = hit {
            if let Some(prev) = self.world.set_block(hit.block, crate::core::BlockId::AIR) {
                if let Some(item) = self.items.item_for_block(prev) {
                    self.inventory
                        .add(crate::inventory::ItemStack::single(item), &self.items);
                }
            }
        }
    }

    /// Place the currently selected block against the targeted face.
    fn try_place_block(&mut self) {
        let hit = crate::world::raycast(
            self.player.eye_position(),
            self.player.look_direction(),
            REACH,
            |p| self.world.is_solid(p),
        );
        let Some(hit) = hit else { return };
        let Some(item_id) = self.inventory.item_in_selected() else {
            return;
        };
        let Some(block) = self.items.get(item_id).place_block else {
            return;
        };
        let target = hit.place_position();
        // Don't place inside the player.
        if crate::core::math::Aabb::block(Vec3::new(
            target.x as f32,
            target.y as f32,
            target.z as f32,
        ))
        .intersects(self.player.aabb())
        {
            return;
        }
        if self.world.set_block(target, block).is_some() {
            self.inventory.consume_selected(1);
        }
    }
}

impl GameState for InGameState {
    fn name(&self) -> &'static str {
        "InGame"
    }

    fn update(&mut self, ctx: &mut StateContext) -> Transition {
        ctx.grab_cursor = true;
        let binds = &ctx.settings.controls.keybinds;

        // Esc opens the pause overlay (world keeps rendering, frozen).
        if ctx.input.just_pressed(binds.pause) {
            return Transition::Push(Box::new(PauseMenuState::new()));
        }
        if ctx.input.just_pressed(binds.toggle_perspective) {
            self.player.toggle_perspective();
        }
        if ctx.input.just_pressed(binds.toggle_debug) {
            self.show_debug = !self.show_debug;
        }

        // Mouse look.
        let sens = ctx.settings.controls.mouse_sensitivity * 0.0025;
        let delta = ctx.input.mouse_delta();
        let pitch_sign = if ctx.settings.controls.invert_y { 1.0 } else { -1.0 };
        self.player.rotate(delta.x * sens, pitch_sign * delta.y * sens);

        // Hotbar selection via scroll.
        let scroll = ctx.input.scroll_delta();
        if scroll != 0.0 {
            self.inventory.scroll_selected(-scroll.signum() as i32);
        }

        // Movement + physics.
        let movement = ctx.input.movement(binds);
        let dt = ctx.dt.min(0.05);
        let solid_snapshot = |p: BlockPos| self.world.is_solid(p);
        // Borrow split: copy player out is unnecessary; closure borrows world only.
        self.player.update(movement, dt, solid_snapshot);

        // Block interaction.
        if ctx.input.mouse_just_pressed(MouseButton::Left) {
            self.try_break_block();
        }
        if ctx.input.mouse_just_pressed(MouseButton::Right) {
            self.try_place_block();
        }

        self.fov_degrees = ctx.settings.render.fov_degrees;
        self.update_loaded_chunks(ctx.settings.render.render_distance);
        self.rebuild_dirty_meshes(ctx.render);
        Transition::None
    }

    fn ui(&mut self, egui_ctx: &egui::Context, ctx: &mut StateContext) -> Transition {
        hud::draw_crosshair(egui_ctx);
        hud::draw_hotbar(egui_ctx, &self.inventory, &self.items);

        if self.show_debug {
            let fps = if ctx.dt > 0.0 { 1.0 / ctx.dt } else { 0.0 };
            let p = self.player.position;
            let facing = self.player.look_direction();
            let lines = vec![
                format!("Wyvencraft — {fps:.0} fps"),
                format!("xyz: {:.2} {:.2} {:.2}", p.x, p.y, p.z),
                format!("facing: {:.2} {:.2} {:.2}", facing.x, facing.y, facing.z),
                format!("chunks: {} ({} meshes)", self.world.loaded_count(), self.chunk_meshes.len()),
                format!("on_ground: {}", self.player.on_ground),
            ];
            hud::draw_debug(egui_ctx, &lines);
        }

        Transition::None
    }

    fn scene_frame(&self, aspect: f32) -> Option<SceneFrame<'_>> {
        let mut camera = Camera::new(self.fov_degrees, aspect);
        camera.position = self.player.eye_position();
        camera.forward = self.player.look_direction();
        let opaque = self.chunk_meshes.values().collect();
        Some(SceneFrame {
            view_proj: camera.view_projection(),
            opaque,
        })
    }
}

/// Find a safe spawn (top solid block at the origin column + 1).
fn find_spawn(world: &World) -> Vec3 {
    for y in (0..CHUNK_HEIGHT).rev() {
        if world.is_solid(BlockPos::new(0, y, 0)) {
            return Vec3::new(0.5, (y + 1) as f32, 0.5);
        }
    }
    Vec3::new(0.5, 80.0, 0.5)
}

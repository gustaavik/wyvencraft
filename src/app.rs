//! Application bootstrap: creates the window + Vulkan context (via `vulkano-util`)
//! and runs the winit event loop, driving the [`StateStack`] and rendering egui.
//!
//! M1 brings up the window, the Vulkan/MoltenVK device, the per-frame
//! update/render pump, and egui (so the main menu is visible & clickable). The 3D
//! voxel pass is layered on in M2.

use std::sync::Arc;

use egui_winit_vulkano::{Gui, GuiConfig};
use vulkano::device::DeviceFeatures;
use vulkano::image::sampler::{Filter, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::AllocationCreateInfo;
use vulkano::swapchain::PresentMode;
use vulkano::sync::GpuFuture;
use vulkano_util::context::{VulkanoConfig, VulkanoContext};
use vulkano_util::window::{VulkanoWindows, WindowDescriptor, WindowMode};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, WindowId};

use crate::boot::{BootPlan, SystemEnv, WorldChoice};
use crate::config::Settings;
use crate::content::GameContent;
use crate::core::{Clock, GameMode};
use crate::input::InputState;
use crate::net::Host;
use crate::save::{self, SaveError, SavedGame, WorldSave};
use crate::state::{
    ConnectingState, GameState, InGameState, LoadingState, MainMenuState, Resources, StateContext,
    StateStack, UiTextures,
};
use wyven_model::ModelRegistry;
use wyven_render::{GpuMesh, RenderContext, Renderer, Texture, TexturedMesh, icons};

/// Fixed size of the inventory player-model preview image, in pixels. The 0.48
/// aspect (tall and narrow) matches the mockup's black box; the image is
/// downscaled into whatever rect the inventory reserves.
const PREVIEW_SIZE: [u32; 2] = [384, 800];

/// Seed used by ephemeral (never-saved) host worlds.
const EPHEMERAL_SEED: u64 = 0x57_56_4E_01;

/// Open (or create) the world a [`BootPlan`] asks for. `Ephemeral` has no save
/// directory at all, so it yields `None` and the caller builds a throwaway world.
fn open_boot_world(world: &WorldChoice, mode: GameMode) -> Option<Result<SavedGame, SaveError>> {
    let WorldChoice::Named { name, seed } = world else {
        return None;
    };
    let seed = seed.unwrap_or_else(save::random_seed);
    Some(
        WorldSave::open_or_create(&save::saves_root(), name, seed, mode)
            .and_then(|save| save.load()),
    )
}

/// Set up the account for a dev-boot plan, which skips the login screen.
///
/// `WYVEN_BOOT_INGAME`, `WYVEN_HOST` and `WYVEN_JOIN` exist so the game can be
/// launched headlessly with no clicking, and that has to keep working. So a boot
/// plan never shows the login screen: it signs in with `WYVEN_USERNAME` if the
/// auth server can be reached, and otherwise runs offline.
///
/// This is a *developer* path, not a way around the login gate — an offline
/// client still cannot join anyone, because it has no ticket to present.
fn boot_account(account: &crate::auth::AccountState) {
    let Ok(username) = std::env::var("WYVEN_USERNAME") else {
        log::info!("no WYVEN_USERNAME; booting offline");
        account.set_offline();
        return;
    };
    let Ok(password) = std::env::var("WYVEN_PASSWORD") else {
        log::info!("WYVEN_USERNAME set but no WYVEN_PASSWORD; booting offline");
        account.set_offline();
        return;
    };

    let client = crate::auth::HttpAuthClient::from_env();
    match crate::auth::AuthClient::login(&client, &username, &password) {
        Ok(session) => {
            log::info!("booted signed in as {}", session.identity);
            // Cache the ticket keys too, so a `WYVEN_HOST=1` boot can verify the
            // clients that join it.
            if let Ok(keys) = crate::auth::AuthClient::public_keys(&client)
                && !keys.is_empty()
                && let Err(err) = crate::auth::KeyCache::new().store(&keys)
            {
                log::warn!("could not cache auth keys: {err}");
            }
            account.sign_in(session);
        }
        Err(err) => {
            log::warn!("boot sign-in failed ({err}); continuing offline");
            account.set_offline();
        }
    }
}

/// Turn a [`BootPlan`] into the state the app starts on. This is where the
/// plan's decisions become effects: opening saves, binding sockets.
fn initial_state(
    plan: BootPlan,
    content: &Arc<GameContent>,
    account: &crate::auth::AccountState,
) -> Box<dyn GameState> {
    // Only the menu path is gated. Every other plan is a dev-boot flag, which
    // must stay usable without a window to click in.
    if !matches!(plan, BootPlan::MainMenu) {
        boot_account(account);
    }

    match plan {
        BootPlan::MainMenu => Box::new(crate::state::LoginState::new(account.clone())),
        BootPlan::Singleplayer { world, mode } => match open_boot_world(&world, mode) {
            Some(Ok(game)) => Box::new(LoadingState::saved(game)),
            Some(Err(err)) => {
                log::error!("WYVEN_WORLD load failed ({err}); starting ephemeral world");
                Box::new(LoadingState::singleplayer(mode))
            }
            None => Box::new(LoadingState::singleplayer(mode)),
        },
        BootPlan::Host { world, mode, port } => {
            let (seed, game) = match open_boot_world(&world, mode) {
                Some(Ok(game)) => (game.save.meta.seed, Some(game)),
                Some(Err(err)) => {
                    log::error!("WYVEN_WORLD load failed ({err}); hosting ephemeral world");
                    (EPHEMERAL_SEED, None)
                }
                None => (EPHEMERAL_SEED, None),
            };
            match Host::bind(port, seed) {
                Ok(host) => match game {
                    Some(game) => {
                        Box::new(InGameState::new_host_saved(content.clone(), game, host))
                    }
                    None => Box::new(InGameState::new_host(content.clone(), seed, host, mode)),
                },
                Err(err) => {
                    log::error!("host bind failed: {err}");
                    Box::new(MainMenuState::new())
                }
            }
        }
        BootPlan::Join { address } => Box::new(ConnectingState::new(address, account)),
    }
}

/// Top-level run errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("event loop error: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
}

/// Render every loaded model into its cell of an offscreen icon sheet, once.
///
/// Items with a file-loaded model can't be drawn as an atlas tile, so the UI
/// samples this instead. It is built here, at startup, because it never changes:
/// the alternative is an offscreen pass per visible inventory slot per frame.
/// The meshes and textures are temporary — the GPU work is waited on before
/// they drop, and only the rendered sheet survives.
fn build_icon_sheet(
    ctx: &Arc<RenderContext>,
    renderer: &mut Renderer,
    models: &ModelRegistry,
    color_format: vulkano::format::Format,
) -> Arc<ImageView> {
    let count = models.len() as u32;
    let [width, height] = icons::sheet_size(count);
    let image = Image::new(
        ctx.memory_allocator.clone(),
        ImageCreateInfo {
            image_type: ImageType::Dim2d,
            format: color_format,
            extent: [width, height, 1],
            usage: ImageUsage::COLOR_ATTACHMENT | ImageUsage::SAMPLED,
            ..Default::default()
        },
        AllocationCreateInfo::default(),
    )
    .expect("icon sheet image");
    let view = ImageView::new_default(image).expect("icon sheet view");

    // Bake each model into the unit box the icon camera frames, and upload it
    // alongside its own texture. A model that fails either step is skipped: its
    // cell stays empty rather than taking the whole sheet down with it.
    let uploaded: Vec<(GpuMesh, Texture)> = (0..count)
        .filter_map(|i| {
            let model = models.get(wyven_model::ModelId(i))?;
            let mesh = model.mesh.bake(icons::frame(model.bounds));
            let gpu = GpuMesh::upload(&ctx.memory_allocator, &mesh)
                .ok()
                .flatten()?;
            let texture = Texture::create(ctx, &model.texture)
                .map_err(|err| log::warn!("icon texture upload failed: {err}"))
                .ok()?;
            Some((gpu, texture))
        })
        .collect();
    let batch: Vec<TexturedMesh<'_>> = uploaded
        .iter()
        .map(|(mesh, texture)| TexturedMesh { mesh, texture })
        .collect();

    let future = renderer.draw_icons(
        vulkano::sync::now(ctx.device().clone()).boxed(),
        view.clone(),
        &batch,
    );
    future
        .then_signal_fence_and_flush()
        .expect("flush icon sheet")
        .wait(None)
        .expect("wait icon sheet");
    log::info!("rendered {} item model icon(s)", batch.len());
    view
}

/// Entry point invoked from `main`.
pub fn run() -> Result<(), AppError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);

    let mut app = App::new();
    event_loop.run_app(&mut app)?;
    Ok(())
}

struct App {
    context: VulkanoContext,
    render_context: Arc<RenderContext>,
    /// Block/item registries loaded from `assets/*.toml` at startup.
    content: Arc<GameContent>,
    /// Who this client is signed in as. Owned here and lent to every state
    /// through `Resources`, so nothing needs a global.
    account: crate::auth::AccountState,
    windows: VulkanoWindows,
    gui: Option<Gui>,
    renderer: Option<Renderer>,
    /// Offscreen colour image the player-model preview renders into, sampled by
    /// egui inside the inventory screen. Created once with the window.
    preview_image: Option<Arc<ImageView>>,
    /// egui handles for the block atlas and the preview image (registered once).
    ui_textures: Option<UiTextures>,
    settings: Settings,
    input: InputState,
    clock: Clock,
    stack: StateStack,
    /// Tracks the applied cursor-grab state so we only update the window on change.
    cursor_grabbed: bool,
}

impl App {
    fn new() -> Self {
        // MoltenVK is a "portability subset" device: egui uploads font/texture
        // images with a component swizzle, which requires this feature.
        let config = VulkanoConfig {
            device_features: DeviceFeatures {
                // Our world pass uses dynamic rendering (no VkRenderPass objects).
                dynamic_rendering: true,
                // MoltenVK (portability subset) needs this for egui's swizzled
                // texture image views.
                image_view_format_swizzle: true,
                ..DeviceFeatures::empty()
            },
            ..VulkanoConfig::default()
        };
        let context = VulkanoContext::new(config);
        log::info!("Vulkan device: {}", context.device_name());
        let render_context = RenderContext::from_vulkano(&context);
        let content = GameContent::load();

        // Dev convenience env vars skip the menus — see `boot` for the rules:
        //   WYVEN_BOOT_INGAME / WYVEN_HOST / WYVEN_JOIN / WYVEN_MODE /
        //   WYVEN_WORLD / WYVEN_SEED.
        let account = crate::auth::AccountState::new();
        let initial = initial_state(BootPlan::from_env(&SystemEnv), &content, &account);

        Self {
            context,
            render_context,
            content,
            account,
            windows: VulkanoWindows::default(),
            gui: None,
            renderer: None,
            preview_image: None,
            ui_textures: None,
            settings: Settings::default(),
            input: InputState::new(),
            clock: Clock::new(),
            stack: StateStack::new(initial),
            cursor_grabbed: false,
        }
    }

    /// Lock/hide or free the OS cursor to match the active state's request.
    fn apply_cursor_grab(&mut self, grab: bool) {
        if grab == self.cursor_grabbed {
            return;
        }
        if let Some(window) = self.windows.get_primary_window() {
            if grab {
                // Locked is preferred; fall back to Confined (macOS support varies).
                let _ = window
                    .set_cursor_grab(CursorGrabMode::Locked)
                    .or_else(|_| window.set_cursor_grab(CursorGrabMode::Confined));
                window.set_cursor_visible(false);
            } else {
                let _ = window.set_cursor_grab(CursorGrabMode::None);
                window.set_cursor_visible(true);
            }
        }
        self.cursor_grabbed = grab;
    }

    /// Run one update + render frame.
    fn frame(&mut self, event_loop: &ActiveEventLoop) {
        let dt = self.clock.tick();
        let elapsed = self.clock.elapsed();

        // egui textures are registered in `resumed`, which always precedes the
        // first frame; without a window there is nothing to render anyway.
        let Some(ui_tex) = self.ui_textures else {
            return;
        };

        // --- Update the active state ---
        let aspect = self
            .windows
            .get_primary_renderer()
            .map(|r| r.aspect_ratio())
            .unwrap_or(1.0);

        let mut grab = self.cursor_grabbed;
        {
            let mut ctx = StateContext {
                settings: &mut self.settings,
                input: &self.input,
                resources: Resources {
                    render: &self.render_context,
                    content: &self.content,
                    ui_tex,
                    account: &self.account,
                },
                dt,
                elapsed,
                grab_cursor: grab,
            };
            let alive = self.stack.update(&mut ctx);
            grab = ctx.grab_cursor;
            if !alive {
                event_loop.exit();
                return;
            }
        }

        // --- Build the egui UI for this frame ---
        if let Some(gui) = self.gui.as_mut() {
            gui.begin_frame();
        }
        let egui_ctx = self.gui.as_ref().map(|g| g.context());
        if let Some(egui_ctx) = egui_ctx {
            let mut ctx = StateContext {
                settings: &mut self.settings,
                input: &self.input,
                resources: Resources {
                    render: &self.render_context,
                    content: &self.content,
                    ui_tex,
                    account: &self.account,
                },
                dt,
                elapsed,
                grab_cursor: grab,
            };
            let alive = self.stack.ui(&egui_ctx, &mut ctx);
            grab = ctx.grab_cursor;
            if !alive {
                event_loop.exit();
                return;
            }
        }

        self.apply_cursor_grab(grab);
        self.input.end_frame();

        // --- Render: world pass (always clears) → egui overlay → present ---
        let before = {
            let Some(renderer) = self.windows.get_primary_renderer_mut() else {
                return;
            };
            match renderer.acquire(None, |_| {}) {
                Ok(future) => future,
                // Swapchain out of date (e.g. mid-resize); skip this frame.
                Err(_) => return,
            }
        };

        let image = self
            .windows
            .get_primary_renderer()
            .unwrap()
            .swapchain_image_view();

        // Player-model preview into its offscreen image first, so the swapchain
        // image's only writer before the egui overlay is the world pass (exactly
        // the inventory-closed path — inserting this pass *between* world and egui
        // breaks vulkano's swapchain layout tracking). Only runs with the
        // inventory open; egui samples the finished image via the future chain.
        let preview = self.stack.preview_frame();
        let before = match (
            preview.as_ref(),
            self.renderer.as_mut(),
            self.preview_image.as_ref(),
        ) {
            (Some(preview), Some(renderer), Some(target)) => {
                renderer.draw_model(before, target.clone(), preview)
            }
            _ => before,
        };

        let scene = self.stack.scene_frame(aspect);
        let after_scene = match self.renderer.as_mut() {
            Some(renderer) => renderer.draw(before, image.clone(), scene.as_ref()),
            None => before,
        };
        drop(scene);

        let after = match self.gui.as_mut() {
            Some(gui) => gui.draw_on_image(after_scene, image),
            None => after_scene,
        };

        if let Some(renderer) = self.windows.get_primary_renderer_mut() {
            renderer.present(after, true);
        }
    }
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.windows.get_primary_renderer().is_some() {
            return; // already created
        }

        let present_mode = if self.settings.window.vsync {
            PresentMode::Fifo
        } else {
            PresentMode::Immediate
        };
        let descriptor = WindowDescriptor {
            width: self.settings.window.width as f32,
            height: self.settings.window.height as f32,
            title: self.settings.window.title.clone(),
            present_mode,
            mode: WindowMode::Windowed,
            ..Default::default()
        };

        self.windows
            .create_window(event_loop, &self.context, &descriptor, |_| {});

        let window_renderer = self.windows.get_primary_renderer().unwrap();
        let color_format = window_renderer.swapchain_format();

        // egui is drawn as an overlay on top of the world pass (which clears).
        let gui = Gui::new(
            event_loop,
            window_renderer.surface(),
            window_renderer.graphics_queue(),
            color_format,
            GuiConfig {
                is_overlay: true,
                ..Default::default()
            },
        );
        let mut renderer = Renderer::new(
            self.render_context.clone(),
            color_format,
            self.content.tiles.atlas_rgba(),
            &self.content.block_textures,
        );

        // Pre-render the 3D icon for every model-backed item.
        let icon_sheet = build_icon_sheet(
            &self.render_context,
            &mut renderer,
            &self.content.models,
            color_format,
        );

        // Offscreen colour target for the inventory's live player preview, in
        // the swapchain format the pipelines were built with, usable both as a
        // render target and as an egui-sampled texture.
        let preview_image = Image::new(
            self.render_context.memory_allocator.clone(),
            ImageCreateInfo {
                image_type: ImageType::Dim2d,
                format: color_format,
                extent: [PREVIEW_SIZE[0], PREVIEW_SIZE[1], 1],
                usage: ImageUsage::COLOR_ATTACHMENT | ImageUsage::SAMPLED,
                ..Default::default()
            },
            AllocationCreateInfo::default(),
        )
        .expect("preview image");
        let preview_view = ImageView::new_default(preview_image).expect("preview view");

        // Register both images with egui once: the atlas (nearest, for crisp
        // pixel-art item icons) and the preview (linear, since the 3D render is
        // downscaled into the panel).
        let mut gui = gui;
        let atlas = gui.register_user_image_view(
            renderer.atlas_view(),
            SamplerCreateInfo {
                mag_filter: Filter::Nearest,
                min_filter: Filter::Nearest,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        );
        let preview = gui.register_user_image_view(
            preview_view.clone(),
            SamplerCreateInfo {
                mag_filter: Filter::Linear,
                min_filter: Filter::Linear,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        );
        // Linear, like the preview: an icon cell is rendered larger than the
        // slot it lands in, so this is a downscale.
        let model_icons = gui.register_user_image_view(
            icon_sheet,
            SamplerCreateInfo {
                mag_filter: Filter::Linear,
                min_filter: Filter::Linear,
                address_mode: [SamplerAddressMode::ClampToEdge; 3],
                ..Default::default()
            },
        );

        self.gui = Some(gui);
        self.renderer = Some(renderer);
        self.preview_image = Some(preview_view);
        self.ui_textures = Some(UiTextures {
            atlas,
            model_icons,
            model_count: self.content.models.len() as u32,
            preview,
        });
        log::info!("Window, renderer and egui initialised");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Let egui see the event first; it reports whether it consumed it.
        let consumed = self.gui.as_mut().map(|g| g.update(&event)).unwrap_or(false);

        match event {
            WindowEvent::CloseRequested => {
                // Run every state's exit hook (world autosave) before quitting;
                // closing the window never goes through StateStack::apply.
                let ui_tex = self.ui_textures.unwrap_or(UiTextures {
                    atlas: egui::TextureId::default(),
                    model_icons: egui::TextureId::default(),
                    model_count: 0,
                    preview: egui::TextureId::default(),
                });
                let mut ctx = StateContext {
                    settings: &mut self.settings,
                    input: &self.input,
                    resources: Resources {
                        render: &self.render_context,
                        content: &self.content,
                        ui_tex,
                        account: &self.account,
                    },
                    dt: 0.0,
                    elapsed: self.clock.elapsed(),
                    grab_cursor: false,
                };
                self.stack.shutdown(&mut ctx);
                event_loop.exit()
            }
            WindowEvent::Resized(_) => {
                if let Some(renderer) = self.windows.get_primary_renderer_mut() {
                    renderer.resize();
                }
            }
            WindowEvent::Focused(false) => {
                self.input.clear_all();
                self.apply_cursor_grab(false);
            }
            WindowEvent::KeyboardInput { event, .. } if !consumed => {
                if let PhysicalKey::Code(code) = event.physical_key {
                    self.input.on_key(code, event.state);
                }
            }
            WindowEvent::MouseInput { state, button, .. } if !consumed => {
                self.input.on_mouse_button(button, state);
            }
            WindowEvent::MouseWheel { delta, .. } if !consumed => {
                let amount = match delta {
                    MouseScrollDelta::LineDelta(_, y) => y,
                    MouseScrollDelta::PixelDelta(p) => p.y as f32 / 40.0,
                };
                self.input.on_scroll(amount);
            }
            WindowEvent::RedrawRequested => self.frame(event_loop),
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        // Raw mouse motion drives the camera only while the cursor is grabbed.
        if self.cursor_grabbed
            && let DeviceEvent::MouseMotion { delta } = event
        {
            self.input.on_mouse_motion(delta.0, delta.1);
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = self.windows.get_primary_window() {
            window.request_redraw();
        }
    }
}

//! The winit application: window, device, event loop and frame pump.

use std::sync::Arc;

use egui_winit_vulkano::{Gui, GuiConfig};
use vulkano::device::DeviceFeatures;
use vulkano::format::Format;
use vulkano::swapchain::PresentMode;
use vulkano_util::context::{VulkanoConfig, VulkanoContext};
use vulkano_util::window::{VulkanoWindows, WindowDescriptor, WindowMode};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, WindowId};
use wyven_core::Clock;
use wyven_input::InputState;
use wyven_render::{RenderContext, Renderer};

use crate::screen::{Frame, ScreenStack};
use crate::{Game, RendererTextures};

/// Window size, title and vsync — everything the runner needs before it can
/// open anything.
#[derive(Debug, Clone)]
pub struct WindowConfig {
    pub width: u32,
    pub height: u32,
    pub title: String,
    pub vsync: bool,
}

/// What a game is handed once the window, device, renderer and egui exist.
///
/// Borrowed for the length of [`Game::start`] only: everything a game wants to
/// keep from it, it clones or renders into something of its own.
pub struct Boot<'a> {
    /// Device, queue and allocators, for uploads and offscreen targets.
    pub render: &'a Arc<RenderContext>,
    /// The live renderer, for one-shot passes at startup (an icon sheet, say).
    pub renderer: &'a mut Renderer,
    /// The egui context, for registering images as sampled textures.
    pub gui: &'a mut Gui,
    /// The swapchain's colour format. An offscreen target that the same
    /// pipelines will draw into has to match it.
    pub color_format: Format,
}

/// Top-level run errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("event loop error: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
}

/// Open a window and run `game` until it quits.
pub fn run<G: Game>(game: G) -> Result<(), AppError> {
    let event_loop = EventLoop::new()?;
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::new(game);
    event_loop.run_app(&mut app)?;
    Ok(())
}

/// Everything before `start` lives in `Pending`; everything after in `Running`.
/// Making that an enum is what lets `start` consume the game by value and hand
/// back a payload, instead of every field being an `Option` the frame loop has
/// to re-check.
enum Stage<G: Game> {
    Pending(G),
    Running {
        shared: G::Shared,
        stack: ScreenStack<G>,
    },
    /// Between the two, and after a panic-free shutdown.
    Empty,
}

struct App<G: Game> {
    context: VulkanoContext,
    render_context: Arc<RenderContext>,
    windows: VulkanoWindows,
    gui: Option<Gui>,
    renderer: Option<Renderer>,
    stage: Stage<G>,
    window: WindowConfig,
    input: InputState,
    clock: Clock,
    /// Tracks the applied cursor-grab state, so the window is only told on
    /// change.
    cursor_grabbed: bool,
}

impl<G: Game> App<G> {
    fn new(game: G) -> Self {
        // MoltenVK is a "portability subset" device, and two features must stay
        // on or the app aborts at startup:
        let config = VulkanoConfig {
            device_features: DeviceFeatures {
                // The world pass uses dynamic rendering (no VkRenderPass).
                dynamic_rendering: true,
                // egui uploads its font/texture images with a component
                // swizzle, which the portability subset gates behind this.
                image_view_format_swizzle: true,
                ..DeviceFeatures::empty()
            },
            ..VulkanoConfig::default()
        };
        let context = VulkanoContext::new(config);
        log::info!("Vulkan device: {}", context.device_name());
        let render_context = RenderContext::from_vulkano(&context);
        let window = game.window();

        Self {
            context,
            render_context,
            windows: VulkanoWindows::default(),
            gui: None,
            renderer: None,
            stage: Stage::Pending(game),
            window,
            input: InputState::new(),
            clock: Clock::new(),
            cursor_grabbed: false,
        }
    }

    /// Lock and hide, or free, the OS cursor to match what the active screen
    /// asked for.
    fn apply_cursor_grab(&mut self, grab: bool) {
        if grab == self.cursor_grabbed {
            return;
        }
        if let Some(window) = self.windows.get_primary_window() {
            if grab {
                // Locked is preferred; fall back to Confined (macOS varies).
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
        let aspect = self
            .windows
            .get_primary_renderer()
            .map(|r| r.aspect_ratio())
            .unwrap_or(1.0);

        let mut grab = self.cursor_grabbed;
        let egui_ctx = self.gui.as_ref().map(|g| g.context());

        // --- Update the active screen ---
        {
            let Stage::Running { shared, stack } = &mut self.stage else {
                return;
            };
            let mut frame = Frame {
                input: &self.input,
                dt,
                elapsed,
                aspect,
                grab_cursor: grab,
                shared,
            };
            let alive = stack.update(&mut frame);
            grab = frame.grab_cursor;
            if !alive {
                event_loop.exit();
                return;
            }
        }

        // --- Build this frame's egui UI ---
        if let Some(gui) = self.gui.as_mut() {
            gui.begin_frame();
        }
        if let Some(egui_ctx) = egui_ctx {
            let Stage::Running { shared, stack } = &mut self.stage else {
                return;
            };
            let mut frame = Frame {
                input: &self.input,
                dt,
                elapsed,
                aspect,
                grab_cursor: grab,
                shared,
            };
            let alive = stack.ui(&egui_ctx, &mut frame);
            grab = frame.grab_cursor;
            if !alive {
                event_loop.exit();
                return;
            }
        }

        self.apply_cursor_grab(grab);
        self.input.end_frame();

        // --- Render: offscreen preview → world pass (clears) → egui → present ---
        let before = {
            let Some(renderer) = self.windows.get_primary_renderer_mut() else {
                return;
            };
            match renderer.acquire(None, |_| {}) {
                Ok(future) => future,
                // Swapchain out of date (mid-resize); skip this frame.
                Err(_) => return,
            }
        };

        let image = self
            .windows
            .get_primary_renderer()
            .unwrap()
            .swapchain_image_view();

        // The preview goes first, so the swapchain image's only writer before
        // the egui overlay is the world pass — exactly the no-preview path.
        // Inserting this *between* world and egui breaks vulkano's swapchain
        // layout tracking. egui samples the finished image via the future chain.
        let target = match &self.stage {
            Stage::Running { shared, .. } => G::preview_target(shared).cloned(),
            _ => None,
        };
        let preview = match &self.stage {
            Stage::Running { stack, .. } => stack.preview_frame(),
            _ => None,
        };
        let before = match (preview.as_ref(), self.renderer.as_mut(), target) {
            (Some(preview), Some(renderer), Some(target)) => {
                renderer.draw_model(before, target, preview)
            }
            _ => before,
        };

        let scene = match &self.stage {
            Stage::Running { stack, .. } => stack.scene_frame(aspect),
            _ => None,
        };
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

    /// Run every screen's exit hook. Closing the window directly never goes
    /// through a transition, so this is what gives a screen holding unsaved work
    /// its last chance.
    fn shutdown(&mut self) {
        let elapsed = self.clock.elapsed();
        let input = &self.input;
        if let Stage::Running { shared, stack } = &mut self.stage {
            let mut frame = Frame {
                input,
                dt: 0.0,
                elapsed,
                aspect: 1.0,
                grab_cursor: false,
                shared,
            };
            stack.shutdown(&mut frame);
        }
    }
}

impl<G: Game> ApplicationHandler for App<G> {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.windows.get_primary_renderer().is_some() {
            return; // already created
        }

        let present_mode = if self.window.vsync {
            PresentMode::Fifo
        } else {
            PresentMode::Immediate
        };
        let descriptor = WindowDescriptor {
            width: self.window.width as f32,
            height: self.window.height as f32,
            title: self.window.title.clone(),
            present_mode,
            mode: WindowMode::Windowed,
            ..Default::default()
        };
        self.windows
            .create_window(event_loop, &self.context, &descriptor, |_| {});

        let window_renderer = self.windows.get_primary_renderer().unwrap();
        let color_format = window_renderer.swapchain_format();

        // egui draws as an overlay on top of the world pass, which clears.
        let mut gui = Gui::new(
            event_loop,
            window_renderer.surface(),
            window_renderer.graphics_queue(),
            color_format,
            GuiConfig {
                is_overlay: true,
                ..Default::default()
            },
        );

        let Stage::Pending(game) = std::mem::replace(&mut self.stage, Stage::Empty) else {
            return;
        };
        let RendererTextures { atlas, blocks } = game.textures();
        let mut renderer = Renderer::new(self.render_context.clone(), color_format, atlas, blocks);

        let (shared, first) = game.start(Boot {
            render: &self.render_context,
            renderer: &mut renderer,
            gui: &mut gui,
            color_format,
        });

        self.gui = Some(gui);
        self.renderer = Some(renderer);
        self.stage = Stage::Running {
            shared,
            stack: ScreenStack::new(first),
        };
        log::info!("Window, renderer and egui initialised");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // egui sees every event first and reports whether it consumed it. That
        // is what makes a focused text field safe: gameplay keys never reach
        // `InputState` while one has focus.
        let consumed = self.gui.as_mut().map(|g| g.update(&event)).unwrap_or(false);

        match event {
            WindowEvent::CloseRequested => {
                self.shutdown();
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

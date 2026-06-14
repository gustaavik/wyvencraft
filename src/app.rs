//! Application bootstrap: creates the window + Vulkan context (via `vulkano-util`)
//! and runs the winit event loop, driving the [`StateStack`] and rendering egui.
//!
//! M1 brings up the window, the Vulkan/MoltenVK device, the per-frame
//! update/render pump, and egui (so the main menu is visible & clickable). The 3D
//! voxel pass is layered on in M2.

use std::sync::Arc;

use egui_winit_vulkano::{Gui, GuiConfig};
use vulkano::device::DeviceFeatures;
use vulkano::swapchain::PresentMode;
use vulkano_util::context::{VulkanoConfig, VulkanoContext};
use vulkano_util::window::{VulkanoWindows, WindowDescriptor, WindowMode};
use winit::application::ApplicationHandler;
use winit::event::{DeviceEvent, DeviceId, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::PhysicalKey;
use winit::window::{CursorGrabMode, WindowId};

use crate::config::Settings;
use crate::core::Clock;
use crate::input::InputState;
use crate::render::{RenderContext, Renderer};
use crate::state::{GameState, LoadingState, MainMenuState, StateContext, StateStack};

/// Top-level run errors.
#[derive(Debug, thiserror::Error)]
pub enum AppError {
    #[error("event loop error: {0}")]
    EventLoop(#[from] winit::error::EventLoopError),
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
    windows: VulkanoWindows,
    gui: Option<Gui>,
    renderer: Option<Renderer>,
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

        // Dev convenience: WYVEN_BOOT_INGAME=1 skips the menu and loads a world.
        let initial: Box<dyn GameState> = if std::env::var_os("WYVEN_BOOT_INGAME").is_some() {
            Box::new(LoadingState::singleplayer())
        } else {
            Box::new(MainMenuState::new())
        };

        Self {
            context,
            render_context,
            windows: VulkanoWindows::default(),
            gui: None,
            renderer: None,
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
                render: &self.render_context,
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
                render: &self.render_context,
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
        self.gui = Some(gui);
        self.renderer = Some(Renderer::new(self.render_context.clone(), color_format));
        log::info!("Window, renderer and egui initialised");
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        // Let egui see the event first; it reports whether it consumed it.
        let consumed = self
            .gui
            .as_mut()
            .map(|g| g.update(&event))
            .unwrap_or(false);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
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
        if self.cursor_grabbed {
            if let DeviceEvent::MouseMotion { delta } = event {
                self.input.on_mouse_motion(delta.0, delta.1);
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(window) = self.windows.get_primary_window() {
            window.request_redraw();
        }
    }
}

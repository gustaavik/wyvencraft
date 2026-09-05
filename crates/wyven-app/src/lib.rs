//! Window, Vulkan surface, egui integration, and the frame pump.
//!
//! [`run`] owns everything that is the same for any game built on these crates:
//! creating the window and device, feeding winit events to egui and then to
//! [`wyven_input`], driving a [`ScreenStack`], and the fixed order of passes
//! each frame — world, egui overlay, present.
//!
//! What it does *not* own is anything a particular game decides. That arrives
//! through one trait, [`Game`]: which textures the renderer is built with, what
//! to do once the GPU exists, and what payload its screens carry. The runner
//! never learns what a block or an inventory is.

pub mod capture;
pub mod screen;

mod runner;

pub use capture::ScreenshotConfig;
pub use runner::{AppError, Boot, WindowConfig, run};
pub use screen::{Frame, Screen, ScreenStack, Transition};

use wyven_render::BlockTextureSet;

/// The textures the renderer is built with, supplied before the first frame.
pub struct RendererTextures<'a> {
    /// The 16-pixel atlas, already assembled into RGBA8.
    pub atlas: Vec<u8>,
    /// The 256-pixel block texture array.
    pub blocks: &'a BlockTextureSet,
}

/// A game the runner can host.
///
/// Implemented once, on a type holding whatever the game needs before a window
/// exists (loaded content, a boot plan). [`Game::start`] consumes it and yields
/// the payload its screens will carry.
pub trait Game: Sized + 'static {
    /// Everything this game's screens read and write. Opaque to the runner: it
    /// carries the value from frame to frame and hands out `&mut` to it, and
    /// that is the whole of its interest.
    type Shared;

    /// Window size, title and vsync. Read once, before the window opens.
    fn window(&self) -> WindowConfig;

    /// The atlas and block-texture array the renderer is built with.
    fn textures(&self) -> RendererTextures<'_>;

    /// Which key saves a screenshot, and where the files go. `None` — the
    /// default — disables capture entirely, and with it the `TRANSFER_SRC`
    /// swapchain usage it needs.
    ///
    /// The runner owns the readback; *this* is the part only the game can
    /// answer, which is why it arrives through the trait rather than being
    /// spelled out in [`run`].
    fn screenshots(&self) -> Option<ScreenshotConfig> {
        None
    }

    /// Called once the window, device, renderer and egui context exist.
    ///
    /// This is where a game does its one-shot GPU work — baking icon sheets,
    /// creating offscreen targets, registering textures with egui — and decides
    /// which screen to open on.
    fn start(self, boot: Boot<'_>) -> (Self::Shared, Box<dyn Screen<Self>>);
}

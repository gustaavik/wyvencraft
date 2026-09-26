//! What every Wyvencraft screen is handed, and the [`Game`] impl that starts it.
//!
//! `wyven_app` carries this from frame to frame and hands out `&mut` to it; it
//! has no opinion about a single field. That is the point of the split — the
//! engine's per-frame interface is four scalars, and *this* is where the game
//! decides what its own screens get to see.

use std::sync::Arc;

use egui_winit_vulkano::Gui;
use vulkano::format::Format;
use vulkano::image::sampler::{Filter, SamplerAddressMode, SamplerCreateInfo};
use vulkano::image::view::ImageView;
use vulkano::image::{Image, ImageCreateInfo, ImageType, ImageUsage};
use vulkano::memory::allocator::AllocationCreateInfo;
use vulkano::sync::GpuFuture;
use wyven_app::{Boot, Game, RendererTextures, Screen, ScreenshotConfig, WindowConfig};
use wyven_assets::decode_png;
use wyven_model::{DisplayContext, ModelRegistry};
use wyven_render::{GpuMesh, RenderContext, Renderer, Texture, TexturedMesh, icons};

use crate::application::boot_plan::{SystemEnv, screenshot_at};
use crate::infrastructure::content::GameContent;
use crate::presentation::config::Settings;
pub use crate::presentation::ui::UiTextures;

/// Everything Wyvencraft's screens read and write.
pub struct Shared {
    pub settings: Settings,
    /// GPU device + allocators, for screens that upload meshes or textures.
    pub render: Arc<RenderContext>,
    /// Block/item/entity registries, shared by every session.
    pub content: Arc<GameContent>,
    /// Who this client is signed in as. Owned here and lent to every screen, so
    /// nothing needs a global.
    pub account: wyven_auth::AccountState,
    pub ui_tex: UiTextures,
    /// Sound/music playback. Real device or silent fallback, chosen once in
    /// [`Game::start`] — see [`crate::infrastructure::audio::open_default_backend`].
    pub audio: crate::infrastructure::audio::AudioManager,
    /// The main menu theme's fade/restart timing and live handle.
    ///
    /// Lives here rather than on any one screen so it plays continuously
    /// across every menu-flow screen (main menu, singleplayer, multiplayer
    /// and its sub-screens) — see [`Shared::tick_menu_music`].
    menu_music: crate::infrastructure::audio::MenuMusic,
}

impl Shared {
    /// Advance the main menu theme's fade/restart/fade-out timing. Call this
    /// from every screen the track should stay audible through, and — while
    /// [`Shared::menu_music_active`] says a fade-out is still in progress —
    /// from whatever screen it is fading out *into* as well.
    pub fn tick_menu_music(&mut self, dt: f32) {
        self.menu_music.tick(dt, &mut self.audio);
    }

    /// Whether the menu theme still needs ticking — playing, fading in,
    /// waiting to restart, or fading out. `false` once it is fully silent.
    pub fn menu_music_active(&self) -> bool {
        self.menu_music.is_active()
    }

    /// Begin fading the main menu theme out, rather than cutting it. Call
    /// this once, from the screen that actually leaves the menu context
    /// (entering a world) — the fade itself is then driven by that screen's
    /// own `tick_menu_music` calls for as long as `menu_music_active` says
    /// there is still something to fade.
    pub fn stop_menu_music(&mut self) {
        self.menu_music.fade_out();
    }
}

/// Builds the screen the app opens on, once content is loaded and a window
/// exists. Supplied by the composition root (`crate::boot`), which is what lets
/// the screens stay ignorant of how startup decides between them.
pub type FirstScreen =
    Box<dyn FnOnce(&Arc<GameContent>, &wyven_auth::AccountState) -> Box<dyn Screen<Wyvencraft>>>;

/// The game, before a window exists.
pub struct Wyvencraft {
    content: Arc<GameContent>,
    account: wyven_auth::AccountState,
    settings: Settings,
    first_screen: FirstScreen,
}

impl Wyvencraft {
    /// Load content. No window, no GPU yet. `first_screen` is called once
    /// [`Game::start`] has a window to hand the screen to.
    pub fn new(first_screen: FirstScreen) -> Self {
        Self {
            content: GameContent::load(),
            account: wyven_auth::AccountState::new(),
            settings: Settings::default(),
            first_screen,
        }
    }
}

impl Game for Wyvencraft {
    type Shared = Shared;

    fn window(&self) -> WindowConfig {
        WindowConfig {
            width: self.settings.window.width,
            height: self.settings.window.height,
            title: self.settings.window.title.clone(),
            vsync: self.settings.window.vsync,
        }
    }

    /// F2 by default, into `<data>/screenshots/`.
    ///
    /// Always `Some`: the capture path is what makes a visual change checkable
    /// at all, and returning `None` here is what would switch it off.
    fn screenshots(&self) -> Option<ScreenshotConfig> {
        Some(ScreenshotConfig {
            key: self.settings.controls.keybinds.screenshot,
            dir: crate::infrastructure::paths::screenshots_root(),
            auto_at: screenshot_at(&SystemEnv),
        })
    }

    fn textures(&self) -> RendererTextures<'_> {
        RendererTextures {
            atlas: self.content.tiles.atlas_rgba(),
            blocks: &self.content.block_textures,
        }
    }

    fn start(self, boot: Boot<'_>) -> (Shared, Box<dyn Screen<Self>>) {
        // Pre-render the 3D icon for every model-backed item.
        let icon_sheet = build_icon_sheet(
            boot.render,
            boot.renderer,
            &self.content.models,
            boot.color_format,
        );

        // Register the three images with egui once. The atlas and the UI sheet
        // are nearest — both are pixel art, and the UI sheet's bevels are two
        // texels wide, which linear filtering would turn to mush. The icon
        // sheet is linear, since it is rendered larger than the rect it lands in.
        let atlas = register(boot.gui, boot.renderer.atlas_view(), Filter::Nearest);
        let model_icons = register(boot.gui, icon_sheet, Filter::Linear);
        let gui = register(boot.gui, load_gui_sheet(boot.render), Filter::Nearest);

        // Opening the audio device is a one-shot OS-level side effect, so it
        // happens here — the moment the app is actually starting — rather
        // than in `Wyvencraft::new()`, which only loads content.
        let audio = crate::infrastructure::audio::AudioManager::new(
            &self.content.sounds,
            &wyven_assets::FsSource::cwd(),
            crate::infrastructure::audio::open_default_backend(),
        );
        // The theme's own authored ceiling (`assets/audio.toml`), not a
        // hardcoded 1.0 — `play_music` always takes an explicit volume
        // (the fade envelope), so without this the registry's `volume` for
        // a music entry would silently do nothing.
        let mainmenu_volume = self
            .content
            .sounds
            .find(crate::infrastructure::audio::MAINMENU_THEME)
            .map_or(1.0, |def| def.volume);

        let shared = Shared {
            settings: self.settings,
            render: boot.render.clone(),
            content: self.content.clone(),
            account: self.account.clone(),
            ui_tex: UiTextures {
                atlas,
                model_icons,
                model_count: self.content.models.len() as u32,
                gui,
            },
            audio,
            menu_music: crate::infrastructure::audio::MenuMusic::new(
                crate::infrastructure::audio::MAINMENU_THEME,
                mainmenu_volume,
            ),
        };
        let first = (self.first_screen)(&self.content, &self.account);
        (shared, first)
    }
}

/// The nine-slice UI sheet, uploaded for egui to sample.
///
/// The one place an arbitrary PNG becomes an egui texture: `decode_png` accepts
/// any size (unlike `art::load_png`, which forces the atlas's tile size), and
/// `Texture::create` puts it on the GPU. Fail-soft with the compiled-in copy,
/// like every other asset — a missing or corrupt file should not stop the game
/// booting, and the built-in sheet is always correct.
fn load_gui_sheet(ctx: &Arc<RenderContext>) -> Arc<ImageView> {
    const PATH: &str = "assets/textures/gui/panel.png";
    const BUILTIN: &[u8] = include_bytes!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/assets/textures/gui/panel.png"
    ));

    let bytes = match std::fs::read(PATH) {
        Ok(bytes) => bytes,
        Err(err) => {
            log::info!("could not read {PATH} ({err}); using the built-in UI sheet");
            BUILTIN.to_vec()
        }
    };
    let image = decode_png(&bytes)
        .or_else(|err| {
            log::warn!("ignoring {PATH}: {err}; using the built-in UI sheet");
            decode_png(BUILTIN)
        })
        .expect("built-in UI sheet decodes");
    Texture::create(ctx, &image)
        .expect("upload UI sheet")
        .image_view
}

fn register(gui: &mut Gui, view: Arc<ImageView>, filter: Filter) -> egui::TextureId {
    gui.register_user_image_view(
        view,
        SamplerCreateInfo {
            mag_filter: filter,
            min_filter: filter,
            address_mode: [SamplerAddressMode::ClampToEdge; 3],
            ..Default::default()
        },
    )
}

/// Render every loaded model into its cell of an offscreen icon sheet, once.
///
/// Items with a file-loaded model can't be drawn as an atlas tile, so the UI
/// samples this instead. It is built at startup because it never changes: the
/// alternative is an offscreen pass per visible inventory slot per frame. The
/// meshes and textures are temporary — the GPU work is waited on before they
/// drop, and only the rendered sheet survives.
fn build_icon_sheet(
    ctx: &Arc<RenderContext>,
    renderer: &mut Renderer,
    models: &ModelRegistry,
    color_format: Format,
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
    // alongside its own texture. A model that fails either step leaves its cell
    // empty rather than taking the whole sheet down with it — and keeps its
    // place in the slice, because the cell index *is* the `ModelId` that
    // `ItemIcon::Model` looks the icon up by.
    let uploaded: Vec<Option<(GpuMesh, Texture)>> = (0..count)
        .map(|i| {
            let model = models.get(wyven_model::ModelId(i))?;
            // A model that says where it belongs in an inventory slot is posed
            // by its author; everything else is fitted to the cell automatically.
            let frame = match model.placement_for(DisplayContext::Gui) {
                Some(gui) => icons::frame_authored(gui.matrix()),
                None => icons::frame(model.bounds),
            };
            let mesh = model.mesh.bake(frame);
            let gpu = GpuMesh::upload(&ctx.memory_allocator, &mesh)
                .ok()
                .flatten()?;
            let texture = Texture::create(ctx, &model.texture)
                .map_err(|err| log::warn!("icon texture upload failed: {err}"))
                .ok()?;
            Some((gpu, texture))
        })
        .collect();
    let batch: Vec<Option<TexturedMesh<'_>>> = uploaded
        .iter()
        .map(|entry| {
            entry
                .as_ref()
                .map(|(mesh, texture)| TexturedMesh { mesh, texture })
        })
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
    log::info!(
        "rendered {} item model icon(s)",
        batch.iter().flatten().count()
    );
    view
}

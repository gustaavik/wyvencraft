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
use wyven_app::{Boot, Game, RendererTextures, Screen, WindowConfig};
use wyven_model::{DisplayContext, ModelRegistry};
use wyven_render::{GpuMesh, RenderContext, Renderer, Texture, TexturedMesh, icons};

use crate::boot::{BootPlan, SystemEnv};
use crate::config::Settings;
use crate::content::GameContent;

/// Fixed size of the inventory player-model preview image, in pixels. The 0.48
/// aspect (tall and narrow) matches the mockup's black box; the image is
/// downscaled into whatever rect the inventory reserves.
const PREVIEW_SIZE: [u32; 2] = [384, 800];

/// egui texture handles the game registers once and hands to the UI each frame:
/// the block atlas (for tile-based item icons), the sheet of pre-rendered 3D
/// icons (for items with a model), and the offscreen player-model preview.
#[derive(Clone, Copy)]
pub struct UiTextures {
    pub atlas: egui::TextureId,
    /// One cell per loaded model, indexed by `ModelId` — see
    /// [`wyven_render::icons`]. `model_count` is how many cells it holds, which
    /// the UI needs to turn a cell index into UVs.
    pub model_icons: egui::TextureId,
    pub model_count: u32,
    pub preview: egui::TextureId,
}

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
    /// Offscreen colour image the player-model preview renders into, sampled by
    /// egui inside the inventory screen.
    preview_image: Arc<ImageView>,
}

/// The game, before a window exists.
pub struct Wyvencraft {
    content: Arc<GameContent>,
    account: wyven_auth::AccountState,
    settings: Settings,
    plan: BootPlan,
}

impl Wyvencraft {
    /// Load content and decide what to boot into. No window, no GPU yet.
    pub fn new() -> Self {
        let content = GameContent::load();
        let account = wyven_auth::AccountState::new();
        // Dev convenience env vars skip the menus — see `boot` for the rules:
        //   WYVEN_BOOT_INGAME / WYVEN_HOST / WYVEN_JOIN / WYVEN_MODE /
        //   WYVEN_WORLD / WYVEN_SEED.
        let plan = BootPlan::from_env(&SystemEnv);
        Self {
            content,
            account,
            settings: Settings::default(),
            plan,
        }
    }
}

impl Default for Wyvencraft {
    fn default() -> Self {
        Self::new()
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
        let preview_image = create_preview_image(boot.render, boot.color_format);

        // Register the three images with egui once. The atlas is nearest, for
        // crisp pixel-art item icons; the other two are linear, since both are
        // rendered larger than the rect they land in.
        let atlas = register(boot.gui, boot.renderer.atlas_view(), Filter::Nearest);
        let model_icons = register(boot.gui, icon_sheet, Filter::Linear);
        let preview = register(boot.gui, preview_image.clone(), Filter::Linear);

        let shared = Shared {
            settings: self.settings,
            render: boot.render.clone(),
            content: self.content.clone(),
            account: self.account.clone(),
            ui_tex: UiTextures {
                atlas,
                model_icons,
                model_count: self.content.models.len() as u32,
                preview,
            },
            preview_image,
        };
        let first = crate::boot::initial_screen(self.plan, &self.content, &self.account);
        (shared, first)
    }

    fn preview_target(shared: &Shared) -> Option<&Arc<ImageView>> {
        Some(&shared.preview_image)
    }
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

/// Offscreen colour target for the inventory's live player preview, in the
/// swapchain format the pipelines were built with, usable both as a render
/// target and as an egui-sampled texture.
fn create_preview_image(ctx: &Arc<RenderContext>, color_format: Format) -> Arc<ImageView> {
    let image = Image::new(
        ctx.memory_allocator.clone(),
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
    ImageView::new_default(image).expect("preview view")
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

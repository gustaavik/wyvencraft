//! The egui textures every UI view samples.

/// egui texture handles the game registers once and hands to the UI each frame:
/// the block atlas (for tile-based item icons), the sheet of pre-rendered 3D
/// icons (for items with a model), and the nine-slice UI sheet.
///
/// Registering is only possible during [`wyven_app::Game::start`], which is the one moment
/// a `&mut Gui` exists — so everything the UI will ever sample is loaded here,
/// and a screen can never pull in a texture of its own later.
#[derive(Clone, Copy)]
pub struct UiTextures {
    pub atlas: egui::TextureId,
    /// One cell per loaded model, indexed by `ModelId` — see
    /// [`wyven_render::icons`]. `model_count` is how many cells it holds, which
    /// the UI needs to turn a cell index into UVs.
    pub model_icons: egui::TextureId,
    pub model_count: u32,
    /// Panel frames, slots and the tooltip backing — see [`crate::presentation::ui::ninepatch`].
    pub gui: egui::TextureId,
}

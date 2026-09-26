//! Everything loaded from `assets/`, as the screens receive it.
//!
//! [`GameContent`] is three values side by side, loaded once at app startup
//! and shared via `Arc` — the renderer needs the textures before any game state
//! exists, and every session (singleplayer, host, client) reads the same
//! definitions:
//!
//! - [`GameContent::rules`] — the [`Registries`]: every definition that feeds
//!   the content hash. Loaded by [`crate::application::content`].
//! - [`GameContent::visuals`] — the [`Visuals`]: textures, models, icons and
//!   labels, resolved against those rules. Never hashed.
//! - [`GameContent::sounds`] — the sound registry. Never hashed either: two
//!   peers with different local audio, or no audio device at all, must still
//!   be able to share a world.
//!
//! Where the bytes come from is a [`ContentSource`], so the same load path
//! serves the real `assets/` directory, the builtins-only build, and test
//! fixtures.

use std::sync::Arc;

use wyven_assets::load_or_builtin;
use wyven_voxel::FaceTextures;

use crate::application::content::load_registries;
use crate::domain::content::Registries;
use crate::domain::inventory::{ItemId, Placeable};
use crate::infrastructure::audio::SoundRegistry;

pub mod catalog;
mod visuals;

pub use catalog::BlockAppearance;
pub use visuals::{BLOCK_ITEM_MODEL, ItemIcon, ItemModel, ItemShape, MISSING_FACES, Visuals};
pub use wyven_voxel::FluidTexture;

// The byte-source port lives in `wyven_assets` — the model loaders need it
// too, and they sit below game content. Re-exported under the old name so
// every caller and fixture keeps reading.
pub use wyven_assets::{AssetSource as ContentSource, EmbeddedSource, FsSource, MapSource};

const AUDIO_PATH: &str = "assets/audio.toml";

/// The rules, how they look, and how they sound.
pub struct GameContent {
    /// Every gameplay-affecting definition, and the hash peers compare.
    pub rules: Registries,
    /// Every texture, model, icon and label. Never hashed.
    pub visuals: Visuals,
    /// Sound/music definitions from `assets/audio.toml`. Never hashed.
    pub sounds: Arc<SoundRegistry>,
}

impl GameContent {
    /// Load content from `assets/` (CWD-relative, like recipes and saves),
    /// falling back to the embedded builtin copies. Never fails.
    pub fn load() -> Arc<Self> {
        Self::from_source(&FsSource::cwd())
    }

    /// The embedded builtin content only — used by tests and as the fallback.
    pub fn builtin() -> Arc<Self> {
        Self::from_source(&EmbeddedSource)
    }

    /// Rules first, since the visuals are indexed by the ids they hand out and
    /// named by the files they parse; sounds stand alone.
    pub fn from_source(source: &dyn ContentSource) -> Arc<Self> {
        let (rules, specs) = load_registries(source);
        let sounds = Arc::new(load_or_builtin(
            source,
            AUDIO_PATH,
            "sounds",
            &mut (),
            |text, _| SoundRegistry::from_toml(text),
            |_| SoundRegistry::builtin(),
            |reg| format!("{} sounds", reg.len()),
        ));
        let visuals = Visuals::load(source, &rules, specs);
        Arc::new(Self {
            rules,
            visuals,
            sounds,
        })
    }

    /// The model geometry the chunk mesher needs, borrowed from this content.
    pub fn appearance(&self) -> BlockAppearance<'_> {
        BlockAppearance {
            blocks: &self.rules.blocks,
            face_tiles: &self.visuals.block_face_tiles,
            models: &self.visuals.models,
            placed: &self.visuals.block_models,
            baked: &self.visuals.baked_models,
            fluids: &self.visuals.fluid_textures,
        }
    }

    /// See [`Visuals::face_textures`].
    pub fn face_textures(&self, block: crate::domain::core::BlockId) -> FaceTextures {
        self.visuals.face_textures(block)
    }

    /// What shape a loose `item` takes where it lies in the world.
    ///
    /// An item with a `[item.model]` is drawn as that model instead and never
    /// reaches here; anything else with no art at all gets the missing marker,
    /// which is the same thing its inventory icon shows.
    pub fn item_shape(&self, item: ItemId) -> ItemShape {
        // Read the decision off the icon rather than re-deriving it from the
        // `Placeable` capability: the two must agree, and a fluid is the case
        // that proves it — water places a block but its icon is the flat still
        // frame, so asking `Placeable` would give a dropped bucket-of-nothing a
        // cube the inventory never shows.
        match self.visuals.item_icons.get(item.0 as usize) {
            Some(&ItemIcon::Flat(tile)) => ItemShape::Sprite(tile),
            Some(&ItemIcon::Cube { .. }) => ItemShape::Cube(
                self.rules
                    .items
                    .get(item)
                    .get::<Placeable>()
                    .map_or(MISSING_FACES, |p| self.visuals.face_textures(p.block)),
            ),
            // A model-backed item is drawn as its model and never reaches here.
            _ => ItemShape::Cube(MISSING_FACES),
        }
    }

    /// What the player reads for `block`. Falls back to the id, which is always
    /// something rather than an empty label.
    pub fn block_display_name(&self, block: crate::domain::core::BlockId) -> &str {
        self.visuals
            .block_display_names
            .get(block.0 as usize)
            .map(String::as_str)
            .unwrap_or_else(|| &self.rules.blocks.get(block).id)
    }

    /// What the player reads for `item` — the inventory tooltip, the label
    /// above the hotbar, and the name `/give` echoes back.
    pub fn item_display_name(&self, item: ItemId) -> &str {
        self.visuals
            .item_display_names
            .get(item.0 as usize)
            .map(String::as_str)
            .unwrap_or_else(|| &self.rules.items.get(item).id)
    }
}

#[cfg(test)]
mod tests;

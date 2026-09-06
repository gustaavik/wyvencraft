//! The in-game item placement editor: a developer tool, not a game feature.
//!
//! Positioning a held item used to be a compile-and-look loop — edit a number in
//! `assets/models/items/<id>.json` or `assets/items.toml`, rebuild, boot into a
//! world, equip the item, squint. This makes it a drag: open the panel, pick a
//! context, move the numbers, watch the item move in your hand, save it back to
//! the file it came from.
//!
//! ## How a live edit reaches the screen
//!
//! It does not push. [`SceneCache`](crate::state::ingame_state) re-bakes the arm
//! and the held item every single frame with no dirty check, so an edited value
//! is on screen the next frame simply by being readable. The only wiring is
//! [`PlacementSource`], which the five placement seams consult before falling
//! back to the shipped value. With no editor open the table is empty and the
//! seams behave exactly as they did.
//!
//! ## The layers
//!
//! - [`placement`] — what a placement *is*, and the override table. Pure.
//! - [`json_display`] / [`toml_spec`] — reading and writing the two file formats
//!   a placement can live in. Pure `&str` → `String`, so the risky part is
//!   tested without a filesystem.
//! - [`store`] — the [`PlacementStore`] port that puts those on disk, plus the
//!   in-memory double.
//! - [`watch`] — file stamps, so an edit made in Blockbench or a text editor
//!   shows up in the running game.
//! - [`session`] — what is selected, what is dirty, what to undo. No egui, no
//!   I/O of its own.
//!
//! Drawing lives in [`crate::ui::editor`], which touches no files and owns no
//! game state.

pub mod json_display;
pub mod placement;
pub mod session;
pub mod store;
pub mod toml_spec;
pub mod watch;

pub use placement::{
    Placement, PlacementKind, PlacementOverrides, PlacementSource, ShippedPlacements, SpecPlacement,
};
pub use session::{CONTEXTS, EditorAction, EditorSession, EditorTarget};
pub use store::{FileStore, PlacementStore, Target};
pub use watch::{FsStamps, Stamps};

/// Every item the editor can move, in display-name order.
///
/// Built once when the world is entered. `declared` is read from the model
/// itself rather than guessed from the file extension, so the panel edits
/// whichever layer is actually placing each context — the same choice
/// `wyven_model::mesh::local_transform` makes at draw time.
pub fn targets_from(content: &crate::content::GameContent) -> Vec<EditorTarget> {
    let mut targets: Vec<EditorTarget> = content
        .item_models
        .iter()
        .enumerate()
        .filter_map(|(index, model)| {
            let model = model.as_ref()?;
            let item = crate::inventory::ItemId(u16::try_from(index).ok()?);
            let loaded = content.models.get(model.id)?;
            Some(EditorTarget {
                item,
                id: content.items.get(item).id.clone(),
                name: content.item_display_name(item).to_string(),
                model: content.models.path_of(model.id)?.to_string(),
                declared: CONTEXTS
                    .into_iter()
                    .filter(|context| loaded.placement_for(*context).is_some())
                    .collect(),
            })
        })
        .collect();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::{FsSource, GameContent};

    /// The real shipped content, models included. `GameContent::builtin()` is
    /// not enough here: it reads the embedded TOMLs but no model *files*, so
    /// every `[item.model]` resolves to nothing and there is nothing to edit.
    fn shipped() -> std::sync::Arc<GameContent> {
        GameContent::from_source(&FsSource::rooted(env!("CARGO_MANIFEST_DIR")))
    }

    /// The list the panel offers, built from the content the game actually
    /// loaded — which is the only thing that can tell a model that places
    /// itself from one placed by `[item.model]`.
    #[test]
    fn every_item_with_a_model_is_editable() {
        let content = shipped();
        let targets = targets_from(&content);

        let with_models = content.item_models.iter().flatten().count();
        assert_eq!(targets.len(), with_models);
        assert!(targets.len() >= 38, "{} targets", targets.len());
        assert!(
            targets.windows(2).all(|pair| pair[0].name <= pair[1].name),
            "sorted so the dropdown reads alphabetically"
        );
    }

    /// The shipped split: the Java models place themselves in every context,
    /// and only the four `.bbmodel` plants fall back to `[item.model]`.
    #[test]
    fn a_java_model_declares_its_contexts_and_a_bbmodel_does_not() {
        let content = shipped();
        let targets = targets_from(&content);

        let sword = targets
            .iter()
            .find(|target| target.id == "wooden_sword")
            .expect("the worked example");
        assert!(sword.model.ends_with(".json"));
        for context in CONTEXTS {
            assert_eq!(
                sword.kind(context),
                PlacementKind::Display,
                "{context:?} is authored in the model"
            );
        }

        let plant = targets
            .iter()
            .find(|target| target.id == "blue_bells")
            .expect("a .bbmodel item");
        assert!(plant.model.ends_with(".bbmodel"));
        assert_eq!(plant.kind(CONTEXTS[0]), PlacementKind::Spec);
    }

    /// A flat item is a three-line stub, and the panel still has to open on the
    /// `item/generated` numbers it is drawn with.
    #[test]
    fn a_generated_stub_counts_as_placing_itself() {
        let content = shipped();
        let targets = targets_from(&content);
        let apple = targets
            .iter()
            .find(|target| target.id == "apple")
            .expect("apple has a model stub");
        assert_eq!(apple.kind(CONTEXTS[0]), PlacementKind::Display);
    }
}

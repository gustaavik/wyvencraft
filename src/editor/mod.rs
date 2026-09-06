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

use wyven_model::display::DisplayContext;

pub mod json_display;
pub mod placement;
pub mod session;
pub mod store;
pub mod toml_spec;
pub mod watch;

pub use placement::{
    Placement, PlacementKey, PlacementKind, PlacementOverrides, PlacementSource, ShippedPlacements,
    SpecPlacement,
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
                key: PlacementKey::Item(item),
                id: content.items.get(item).id.clone(),
                name: content.item_display_name(item).to_string(),
                model: content.models.path_of(model.id)?.to_string(),
                offers: CONTEXTS.to_vec(),
                declared: CONTEXTS
                    .into_iter()
                    .filter(|context| loaded.placement_for(*context).is_some())
                    .collect(),
            })
        })
        .collect();
    targets.sort_by(|a, b| a.name.cmp(&b.name));
    // First in the list rather than sorted in among the items, because it is not
    // one of them: it is the cube every block is held as.
    targets.insert(0, block_item_target());
    targets
}

/// The one placement every block item shares.
///
/// A block item has no model file — it is drawn as a cube built from the block's
/// own faces — so there is nothing per-block to move, and forty identical rows
/// in the dropdown would be forty ways to edit one number. Minecraft shares a
/// single `block/block` display for exactly the same reason.
///
/// Only the two hand contexts: the inventory icon is painted by
/// [`crate::ui::icon`] and a block lying on the ground is sized by its drop
/// entity, and neither passes through a `display` entry at all.
fn block_item_target() -> EditorTarget {
    let hands = vec![
        DisplayContext::FirstPersonRightHand,
        DisplayContext::ThirdPersonRightHand,
    ];
    EditorTarget {
        key: PlacementKey::BlockItem,
        id: "block".to_string(),
        name: "▣ Block items (all)".to_string(),
        model: crate::content::BLOCK_ITEM_MODEL.to_string(),
        offers: hands.clone(),
        declared: hands,
    }
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
        assert_eq!(
            targets.len(),
            with_models + 1,
            "every modelled item, plus the shared block-item cube"
        );
        assert!(targets.len() >= 39, "{} targets", targets.len());
        assert_eq!(
            targets[0].key,
            PlacementKey::BlockItem,
            "the shared cube leads, rather than sorting in among the items"
        );
        assert!(
            targets[1..]
                .windows(2)
                .all(|pair| pair[0].name <= pair[1].name),
            "and the items after it read alphabetically"
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

    /// The fix for "I can't reposition blocks": every block item is the same
    /// cube, so it is one entry backed by one file, offering only the two
    /// contexts a cube is actually placed by a `display` entry in.
    #[test]
    fn block_items_are_one_shared_target_with_only_the_hand_contexts() {
        let block = &targets_from(&shipped())[0];
        assert_eq!(block.key, PlacementKey::BlockItem);
        assert_eq!(block.model, crate::content::BLOCK_ITEM_MODEL);

        assert!(block.offers(DisplayContext::FirstPersonRightHand));
        assert!(block.offers(DisplayContext::ThirdPersonRightHand));
        assert!(
            !block.offers(DisplayContext::Gui),
            "the inventory icon is painted, not placed by a display entry"
        );
        assert!(
            !block.offers(DisplayContext::Ground),
            "and a block on the floor is sized by its drop entity"
        );

        for context in [
            DisplayContext::FirstPersonRightHand,
            DisplayContext::ThirdPersonRightHand,
        ] {
            assert_eq!(
                block.kind(context),
                PlacementKind::Display,
                "saved into {}, not items.toml",
                crate::content::BLOCK_ITEM_MODEL
            );
        }
    }

    /// The shipped file has to *place* both hands, not match any particular
    /// numbers — those are the developer's, and moving them is the whole point
    /// of the editor. What must hold is that neither hand is left to the
    /// fallback, so the file is the single answer to where a held block sits.
    #[test]
    fn the_shipped_block_model_places_both_hands() {
        let content = shipped();
        for context in [
            DisplayContext::FirstPersonRightHand,
            DisplayContext::ThirdPersonRightHand,
        ] {
            assert!(
                content.block_item_display.get(context).is_some(),
                "{} declares no {context:?}",
                crate::content::BLOCK_ITEM_MODEL
            );
        }
    }

    /// A block item has no `[item.model]` spec to fall back to, so an entry
    /// deleted from that file must cost *that entry* and not leave the block
    /// you are holding at the origin at full size.
    #[test]
    fn a_missing_entry_falls_back_per_context_rather_than_wholesale() {
        use crate::content::{BLOCK_ITEM_MODEL, GameContent, MapSource};

        let builtin = crate::entity::viewmodel::default_block_display();
        let only_first = r#"{ "display": { "firstperson_righthand": { "scale": [2, 2, 2] } } }"#;
        let content = GameContent::from_source(
            &MapSource::new().with_bytes(BLOCK_ITEM_MODEL, only_first.as_bytes().to_vec()),
        );

        assert_eq!(
            content
                .block_item_display
                .get(DisplayContext::FirstPersonRightHand)
                .map(|t| t.scale),
            Some([2.0; 3]),
            "the declared hand is the file's"
        );
        assert_eq!(
            content
                .block_item_display
                .get(DisplayContext::ThirdPersonRightHand),
            builtin.get(DisplayContext::ThirdPersonRightHand),
            "and the one it left out keeps the builtin, not the identity"
        );
    }

    /// A file that cannot be read at all still leaves blocks holdable.
    #[test]
    fn no_file_at_all_is_the_builtin_placement() {
        use crate::content::GameContent;

        let content = GameContent::from_source(&crate::content::MapSource::new());
        assert_eq!(
            content.block_item_display,
            crate::entity::viewmodel::default_block_display()
        );
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

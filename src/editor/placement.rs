//! What the editor is editing, and how a live edit reaches the renderer.
//!
//! The five seams that place a held item — first person, third person, both
//! atlas fallbacks, and a dropped item on the ground — all resolve their matrix
//! through [`PlacementSource`]. The game answers "nothing to say", so a release
//! build is the shipped placement exactly; the editor answers with whatever the
//! developer is dragging right now. That is the whole of the coupling: no seam
//! knows an editor exists.
//!
//! Boundaries: pure data plus two matrices. Nothing here reads a file, touches
//! egui, or knows what a frame is.

use std::collections::HashMap;

use glam::{Mat4, Vec3};
use wyven_model::display::{DisplayContext, ItemTransform};
use wyven_model::mesh as model_mesh;

use crate::inventory::ItemId;

/// Where a held item's placement comes from for one draw.
///
/// `None` means "the shipped value" — the caller falls back to
/// [`crate::content::ItemModel::local`], which is what it did before this
/// existed. An override is therefore never able to *break* a placement it does
/// not mention.
pub trait PlacementSource {
    fn local(&self, item: ItemId, context: DisplayContext) -> Option<Mat4>;
}

/// The shipped placement, unmodified. What runs when no editor is open.
#[derive(Debug, Clone, Copy, Default)]
pub struct ShippedPlacements;

impl PlacementSource for ShippedPlacements {
    fn local(&self, _item: ItemId, _context: DisplayContext) -> Option<Mat4> {
        None
    }
}

/// The `[item.model]` numbers, as `assets/items.toml` authors them.
///
/// Rotation is kept in **degrees** here, not radians as `content::ItemModel`
/// keeps it, because degrees are what the developer types and what the file
/// stores; converting once at the matrix is cheaper than converting twice a
/// frame in the panel.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct SpecPlacement {
    pub scale: f32,
    pub offset: [f32; 3],
    pub rotation: [f32; 3],
}

impl Default for SpecPlacement {
    fn default() -> Self {
        Self {
            scale: 1.0,
            offset: [0.0; 3],
            rotation: [0.0; 3],
        }
    }
}

impl SpecPlacement {
    /// The same matrix the shipped fallback builds, by the same function — so
    /// what the editor previews and what the game draws cannot drift apart.
    pub fn matrix(&self) -> Mat4 {
        model_mesh::local_transform(
            None,
            self.scale,
            Vec3::from(self.rotation.map(f32::to_radians)),
            Vec3::from(self.offset),
        )
    }
}

/// Which file a context's placement lives in.
///
/// Read off the model, never chosen: a context the model file itself places is
/// a `display` entry in that `.json`, and a context it says nothing about is
/// placed by `[item.model]`. That is exactly the choice
/// [`wyven_model::mesh::local_transform`] makes, which is what keeps the panel
/// editing the number that is actually placing the item.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum PlacementKind {
    /// A `display` entry in the model's own `.json`.
    Display,
    /// The `[item.model]` numbers, shared by every context the model does not
    /// place itself.
    Spec,
}

/// One editable placement.
///
/// Which arm an item gets is decided by its model file and never by the editor:
/// a Java-model `.json` carries a per-context `display` entry, a `.bbmodel` or
/// `.gltf` cannot express one at all, so it takes the single `[item.model]`
/// spec that serves every context. Mixing them up would let the panel write a
/// value into a file that cannot hold it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Placement {
    /// A `display` entry, per context, in the model's own `.json`.
    Display(ItemTransform),
    /// The `[item.model]` numbers, shared by every context.
    Spec(SpecPlacement),
}

impl Placement {
    pub fn matrix(&self) -> Mat4 {
        match self {
            Self::Display(transform) => transform.matrix(),
            Self::Spec(spec) => spec.matrix(),
        }
    }

    /// Whether this kind of placement is authored once for every context.
    ///
    /// The panel greys its context tabs when it is: editing a spec while the
    /// `gui` tab is up still moves the item in the hand, and hiding that would
    /// mislead.
    pub fn is_shared_across_contexts(&self) -> bool {
        matches!(self, Self::Spec(_))
    }

    pub fn kind(&self) -> PlacementKind {
        match self {
            Self::Display(_) => PlacementKind::Display,
            Self::Spec(_) => PlacementKind::Spec,
        }
    }
}

/// Every placement the developer has moved away from its shipped value.
///
/// Empty is the identity: an empty table is indistinguishable from
/// [`ShippedPlacements`], which is what lets the editor be wired in
/// unconditionally and cost one hash lookup per seam when it is closed.
#[derive(Debug, Default, Clone)]
pub struct PlacementOverrides {
    entries: HashMap<ItemId, ItemOverride>,
}

/// One item's overrides. A model has display entries or a spec, never both —
/// but the table is built from files on disk, so it stores what it is told
/// rather than asserting which.
#[derive(Debug, Default, Clone)]
struct ItemOverride {
    display: HashMap<DisplayContext, ItemTransform>,
    spec: Option<SpecPlacement>,
}

impl PlacementOverrides {
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Record an edit. A [`Placement::Spec`] ignores `context` — one spec is
    /// all the file can hold.
    pub fn set(&mut self, item: ItemId, context: DisplayContext, value: Placement) {
        let entry = self.entries.entry(item).or_default();
        match value {
            Placement::Display(transform) => {
                entry.display.insert(context, transform);
            }
            Placement::Spec(spec) => entry.spec = Some(spec),
        }
    }

    /// The override in force for this item and context, if any.
    pub fn get(&self, item: ItemId, context: DisplayContext) -> Option<Placement> {
        let entry = self.entries.get(&item)?;
        if let Some(transform) = entry.display.get(&context) {
            return Some(Placement::Display(*transform));
        }
        entry.spec.map(Placement::Spec)
    }

    /// Drop one override, so the item falls back to what shipped.
    pub fn clear(&mut self, item: ItemId, context: DisplayContext) {
        let Some(entry) = self.entries.get_mut(&item) else {
            return;
        };
        entry.display.remove(&context);
        entry.spec = None;
        if entry.display.is_empty() && entry.spec.is_none() {
            self.entries.remove(&item);
        }
    }
}

impl PlacementSource for PlacementOverrides {
    fn local(&self, item: ItemId, context: DisplayContext) -> Option<Mat4> {
        self.get(item, context).map(|p| p.matrix())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SWORD: ItemId = ItemId(3);
    const PICKAXE: ItemId = ItemId(4);

    fn transform(rotation_x: f32) -> ItemTransform {
        ItemTransform {
            rotation: [rotation_x, 0.0, 0.0],
            translation: [0.0, 1.0, 1.0],
            scale: [0.8; 3],
        }
    }

    /// The spec arm must be the *shipped* fallback, not a re-derivation of it.
    /// These are `wooden_pickaxe`'s numbers out of `assets/items.toml`.
    #[test]
    fn a_spec_placement_is_the_shipped_fallback_matrix() {
        let spec = SpecPlacement {
            scale: 0.55,
            offset: [-0.5, 0.0, -0.5],
            rotation: [0.0, 90.0, 0.0],
        };
        let expected = model_mesh::local_transform(
            None,
            0.55,
            Vec3::new(0.0, 90f32.to_radians(), 0.0),
            Vec3::new(-0.5, 0.0, -0.5),
        );
        assert_eq!(spec.matrix(), expected);
    }

    /// The display arm must be `ItemTransform`'s own matrix, which is the one
    /// every authored `display` entry in `assets/` is measured against.
    #[test]
    fn a_display_placement_is_the_item_transforms_matrix() {
        let t = transform(-99.9);
        assert_eq!(Placement::Display(t).matrix(), t.matrix());
    }

    #[test]
    fn an_empty_table_is_transparent() {
        let overrides = PlacementOverrides::default();
        assert!(overrides.is_empty());
        assert_eq!(
            overrides.local(SWORD, DisplayContext::FirstPersonRightHand),
            None
        );
    }

    #[test]
    fn a_display_override_applies_to_its_own_context_only() {
        let mut overrides = PlacementOverrides::default();
        let t = transform(45.0);
        overrides.set(
            SWORD,
            DisplayContext::FirstPersonRightHand,
            Placement::Display(t),
        );

        assert_eq!(
            overrides.local(SWORD, DisplayContext::FirstPersonRightHand),
            Some(t.matrix())
        );
        assert_eq!(
            overrides.local(SWORD, DisplayContext::ThirdPersonRightHand),
            None,
            "an untouched context still ships"
        );
        assert_eq!(
            overrides.local(PICKAXE, DisplayContext::FirstPersonRightHand),
            None,
            "an untouched item still ships"
        );
    }

    /// One `[item.model]` table is all a `.bbmodel` can be placed by, so an
    /// override of it has to reach every context — otherwise the ground preview
    /// would show the old value while the hand showed the new one.
    #[test]
    fn a_spec_override_reaches_every_context() {
        let mut overrides = PlacementOverrides::default();
        let spec = SpecPlacement {
            scale: 0.4,
            offset: [-0.5, 0.75, -0.5],
            rotation: [0.0; 3],
        };
        overrides.set(SWORD, DisplayContext::Gui, Placement::Spec(spec));

        for context in [
            DisplayContext::FirstPersonRightHand,
            DisplayContext::ThirdPersonRightHand,
            DisplayContext::Gui,
            DisplayContext::Ground,
        ] {
            assert_eq!(overrides.local(SWORD, context), Some(spec.matrix()));
        }
    }

    #[test]
    fn clearing_an_override_falls_back_to_the_shipped_value() {
        let mut overrides = PlacementOverrides::default();
        overrides.set(
            SWORD,
            DisplayContext::Ground,
            Placement::Display(transform(10.0)),
        );
        overrides.clear(SWORD, DisplayContext::Ground);

        assert_eq!(overrides.local(SWORD, DisplayContext::Ground), None);
        assert!(overrides.is_empty(), "the item's entry is gone with it");
    }

    #[test]
    fn a_spec_is_shared_and_a_display_entry_is_not() {
        assert!(Placement::Spec(SpecPlacement::default()).is_shared_across_contexts());
        assert!(!Placement::Display(transform(0.0)).is_shared_across_contexts());
    }
}

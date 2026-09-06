//! Putting a placement on disk, and getting it back.
//!
//! One trait with three implementations, and the choice between the first two is
//! made by the model file itself: a Java-model `.json` can hold a per-context
//! `display` entry, a `.bbmodel` or `.gltf` cannot, so its placement lives in
//! `[item.model]` in `assets/items.toml` instead. Neither store knows the other
//! exists — [`FileStore`] picks between them the way `ModelLoader::LOADERS`
//! picks a parser, so a third format is a third impl and one line here.
//!
//! The parsing and formatting both stores do is not theirs: it lives in
//! [`super::json_display`] and [`super::toml_spec`] as pure `&str` → `String`,
//! which is where the tests are. What is left here is `read_to_string`,
//! `write`, and the dispatch.

use std::cell::{Cell, RefCell};
use std::collections::HashMap;

use wyven_model::display::DisplayContext;

use super::json_display;
use super::placement::{Placement, PlacementKind};
use super::toml_spec;

/// The one file every `[item.model]` spec lives in.
pub const ITEMS_PATH: &str = "assets/items.toml";

/// Which item's placement, in which context.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    /// The item's id — what `assets/items.toml` keys on and `/give` takes.
    pub item: String,
    /// The model file the item points at, `assets/`-relative.
    pub model: String,
    pub context: DisplayContext,
    /// Which layer places this context — and so which file a save touches.
    pub kind: PlacementKind,
}

/// Where a placement is read from and written back to.
pub trait PlacementStore {
    /// Whether this store owns the file `target` names.
    fn handles(&self, target: &Target) -> bool;
    fn read(&self, target: &Target) -> Result<Placement, String>;
    fn write(&self, target: &Target, value: &Placement) -> Result<(), String>;
    /// The file a save touches — what the panel names and the watcher watches.
    fn file(&self, target: &Target) -> String;
}

/// A `display` entry in the model's own `.json`.
pub struct JsonDisplayStore;

impl JsonDisplayStore {
    fn is_java_model(target: &Target) -> bool {
        target
            .model
            .rsplit_once('.')
            .is_some_and(|(_, extension)| extension.eq_ignore_ascii_case("json"))
    }
}

impl PlacementStore for JsonDisplayStore {
    fn handles(&self, target: &Target) -> bool {
        target.kind == PlacementKind::Display
    }

    fn read(&self, target: &Target) -> Result<Placement, String> {
        let text = read_file(&target.model)?;
        json_display::read_display(&text, target.context).map(Placement::Display)
    }

    fn write(&self, target: &Target, value: &Placement) -> Result<(), String> {
        let Placement::Display(transform) = value else {
            return Err("a .json model is placed by its `display` block".to_string());
        };
        // Only a Java-model `.json` has a `display` block to write into. A
        // `.bbmodel` reaching here would mean the caller decided the kind from
        // something other than the model, so say so rather than rewriting a
        // file that cannot hold the value.
        if !Self::is_java_model(target) {
            return Err(format!("{} has no `display` block", target.model));
        }
        let text = read_file(&target.model)?;
        let written = json_display::write_display(&text, target.context, transform)?;
        write_file(&target.model, &written)
    }

    fn file(&self, target: &Target) -> String {
        target.model.clone()
    }
}

/// The `[item.model]` numbers in `assets/items.toml`.
///
/// The fallback: it handles anything, so it must stay last in
/// [`FileStore::STORES`].
pub struct TomlSpecStore;

impl PlacementStore for TomlSpecStore {
    fn handles(&self, _target: &Target) -> bool {
        true
    }

    fn read(&self, target: &Target) -> Result<Placement, String> {
        let text = read_file(ITEMS_PATH)?;
        toml_spec::read_spec(&text, &target.item).map(Placement::Spec)
    }

    fn write(&self, target: &Target, value: &Placement) -> Result<(), String> {
        let Placement::Spec(spec) = value else {
            return Err("this model has no `display` block to write into".to_string());
        };
        let text = read_file(ITEMS_PATH)?;
        let written = toml_spec::write_spec(&text, &target.item, spec)?;
        write_file(ITEMS_PATH, &written)
    }

    fn file(&self, _target: &Target) -> String {
        ITEMS_PATH.to_string()
    }
}

/// The real store: whichever of the two owns the file this target names.
pub struct FileStore;

impl FileStore {
    /// In order. [`TomlSpecStore`] handles everything, so it is the fallback and
    /// has to come last.
    const STORES: &'static [&'static dyn PlacementStore] = &[&JsonDisplayStore, &TomlSpecStore];

    fn store(target: &Target) -> &'static dyn PlacementStore {
        Self::STORES
            .iter()
            .copied()
            .find(|store| store.handles(target))
            .unwrap_or(&TomlSpecStore)
    }
}

impl PlacementStore for FileStore {
    fn handles(&self, _target: &Target) -> bool {
        true
    }

    fn read(&self, target: &Target) -> Result<Placement, String> {
        Self::store(target).read(target)
    }

    fn write(&self, target: &Target, value: &Placement) -> Result<(), String> {
        Self::store(target).write(target, value)
    }

    fn file(&self, target: &Target) -> String {
        Self::store(target).file(target)
    }
}

fn read_file(path: &str) -> Result<String, String> {
    std::fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))
}

fn write_file(path: &str, text: &str) -> Result<(), String> {
    std::fs::write(path, text).map_err(|err| format!("{path}: {err}"))
}

/// The test double. Holds what a file would, without being one.
#[derive(Default)]
pub struct InMemoryStore {
    values: RefCell<HashMap<(String, DisplayContext), Placement>>,
    writes: Cell<usize>,
    /// Set to make every write fail, which is how the panel's error path is
    /// reached without an unwritable directory.
    pub fail_writes: Cell<bool>,
}

impl InMemoryStore {
    pub fn with(self, item: &str, context: DisplayContext, value: Placement) -> Self {
        self.values
            .borrow_mut()
            .insert((item.to_string(), context), value);
        self
    }

    pub fn writes(&self) -> usize {
        self.writes.get()
    }
}

impl PlacementStore for InMemoryStore {
    fn handles(&self, _target: &Target) -> bool {
        true
    }

    fn read(&self, target: &Target) -> Result<Placement, String> {
        self.values
            .borrow()
            .get(&(target.item.clone(), target.context))
            .copied()
            .ok_or_else(|| format!("no placement for {}", target.item))
    }

    fn write(&self, target: &Target, value: &Placement) -> Result<(), String> {
        if self.fail_writes.get() {
            return Err("the file is read-only".to_string());
        }
        self.writes.set(self.writes.get() + 1);
        self.values
            .borrow_mut()
            .insert((target.item.clone(), target.context), *value);
        Ok(())
    }

    /// Names the file the real store would, so a session test can assert which
    /// file a save touches without a double that lies about it.
    fn file(&self, target: &Target) -> String {
        FileStore.file(target)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::editor::placement::SpecPlacement;

    fn target(model: &str) -> Target {
        Target {
            item: "wooden_sword".to_string(),
            model: model.to_string(),
            context: DisplayContext::FirstPersonRightHand,
            kind: PlacementKind::Display,
        }
    }

    fn spec_target(model: &str) -> Target {
        Target {
            kind: PlacementKind::Spec,
            ..target(model)
        }
    }

    /// The whole of the "both, depending on the model" rule.
    /// The whole of the "both, depending on the model" rule: a context the
    /// model places itself is saved into that model, and everything else lands
    /// in `assets/items.toml`.
    #[test]
    fn a_declared_context_saves_into_the_model_and_the_rest_into_items_toml() {
        let declared = target("assets/models/items/wooden_sword.json");
        let undeclared = spec_target("assets/models/items/wooden_sword.json");

        assert_eq!(
            FileStore.file(&declared),
            "assets/models/items/wooden_sword.json"
        );
        assert_eq!(FileStore.file(&undeclared), ITEMS_PATH);
        assert_eq!(FileStore.file(&spec_target("a.bbmodel")), ITEMS_PATH);
    }

    /// A `.bbmodel` has no `display` block, so a `Display` placement pointed at
    /// one is a bug upstream. Better a refusal than a rewritten file.
    #[test]
    fn a_display_write_refuses_a_file_that_cannot_hold_one() {
        let at = target("assets/models/blocks/plant1.bbmodel");
        let value = Placement::Display(Default::default());
        assert!(JsonDisplayStore.write(&at, &value).is_err());
        assert!(JsonDisplayStore.handles(&target("A.JSON")));
    }

    /// Reading the real shipped sword through the store, not just the parser.
    #[test]
    fn the_shipped_sword_reads_through_the_file_store() {
        let mut at = target("assets/models/items/wooden_sword.json");
        at.model = format!(
            "{}/{}",
            env!("CARGO_MANIFEST_DIR"),
            "assets/models/items/wooden_sword.json"
        );
        let Ok(Placement::Display(transform)) = FileStore.read(&at) else {
            panic!("expected a display placement");
        };
        assert_eq!(transform.rotation, [-99.9, 87.78, 95.45]);
    }

    #[test]
    fn a_store_refuses_a_placement_of_the_wrong_kind() {
        let spec = Placement::Spec(SpecPlacement::default());
        let at = target(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/items/wooden_sword.json"
        ));
        assert!(JsonDisplayStore.write(&at, &spec).is_err());
    }

    #[test]
    fn the_double_round_trips_and_counts_its_writes() {
        let spec = Placement::Spec(SpecPlacement {
            scale: 0.5,
            ..SpecPlacement::default()
        });
        let store = InMemoryStore::default();
        let at = spec_target("a.bbmodel");

        assert!(store.read(&at).is_err(), "nothing seeded yet");
        store.write(&at, &spec).expect("writes");
        assert_eq!(store.read(&at), Ok(spec));
        assert_eq!(store.writes(), 1);

        store.fail_writes.set(true);
        assert!(store.write(&at, &spec).is_err());
        assert_eq!(store.writes(), 1, "a failed write is not counted");
    }
}

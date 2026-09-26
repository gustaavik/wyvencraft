//! What is selected, what has been moved, and what is not saved yet.
//!
//! The orchestrator: it owns the override table the renderer reads, the store
//! that puts values on disk, and the watcher that notices someone else putting
//! values there. It draws nothing and opens no files itself — both of those
//! arrive as ports, which is what lets the whole of it be tested with no egui,
//! no filesystem and no GPU.
//!
//! ## Which layer a context is edited in
//!
//! Not a preference — it is read off the model. A context the model file itself
//! places is edited as a `display` entry and saved into that `.json`; a context
//! it says nothing about is placed by `[item.model]`, so that is what the panel
//! edits and `assets/items.toml` is what a save touches. This mirrors
//! `wyven_model::mesh::local_transform` exactly, which is the point: the panel
//! can only ever edit the number that is actually placing the item.

use std::collections::{HashMap, HashSet};

use glam::Mat4;
use wyven_model::display::DisplayContext;

use super::placement::{
    Placement, PlacementKey, PlacementKind, PlacementOverrides, PlacementSource,
};
use super::store::{PlacementStore, Target};
use super::watch::{DEFAULT_INTERVAL, FileWatcher, Stamps};

/// The contexts the panel offers, in tab order.
pub const CONTEXTS: [DisplayContext; 4] = [
    DisplayContext::FirstPersonRightHand,
    DisplayContext::ThirdPersonRightHand,
    DisplayContext::Gui,
    DisplayContext::Ground,
];

/// How many edits can be walked back.
const UNDO_DEPTH: usize = 64;

/// One thing the editor can move.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorTarget {
    /// What this placement belongs to. Every block item shares one key, because
    /// every block item is the same cube.
    pub key: PlacementKey,
    /// The item's id, as `assets/items.toml` keys it.
    pub id: String,
    /// What the player reads.
    pub name: String,
    /// The model file, `assets/`-relative.
    pub model: String,
    /// Contexts this target is *drawn* in, and so the tabs the panel offers.
    ///
    /// Usually all four. A block item has only the two hands: its inventory icon
    /// is painted by `ui::icon` and the block lying on the ground is sized by
    /// its own drop entity, and neither goes anywhere near a `display` entry —
    /// so offering those tabs would be offering numbers that do nothing.
    pub offers: Vec<DisplayContext>,
    /// Which of `offers` the model file places itself in. The rest fall to the
    /// `[item.model]` spec — which is exactly what `local_transform` does.
    pub declared: Vec<DisplayContext>,
}

impl EditorTarget {
    pub fn kind(&self, context: DisplayContext) -> PlacementKind {
        match self.declared.contains(&context) {
            true => PlacementKind::Display,
            false => PlacementKind::Spec,
        }
    }

    pub fn offers(&self, context: DisplayContext) -> bool {
        self.offers.contains(&context)
    }
}

/// One editable value. A spec is shared by every context it covers, so it has
/// one slot however many tabs display it — otherwise editing it on the `gui`
/// tab and again on the `ground` tab would look like two separate changes to
/// one number.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
enum Slot {
    Display(usize, DisplayContext),
    Spec(usize),
}

/// What the panel asks the session to do.
#[derive(Debug, Clone, PartialEq)]
pub enum EditorAction {
    Select(usize),
    Context(DisplayContext),
    /// The developer moved a number.
    Edit(Placement),
    Save,
    /// Throw away unsaved changes and take what the file says now.
    Reload,
    Undo,
    /// Follow whatever is in the hand instead of holding a selection.
    FollowHeld(bool),
    Close,
}

/// The editor's whole state.
pub struct EditorSession {
    /// Off unless `WYVEN_EDITOR=1`. A player must never be able to open a tool
    /// that writes into `assets/`.
    enabled: bool,
    open: bool,
    follow_held: bool,
    targets: Vec<EditorTarget>,
    selected: usize,
    context: DisplayContext,
    overrides: PlacementOverrides,
    /// What each slot's file said the last time it was read or written. The
    /// difference between this and the override *is* "unsaved".
    disk: HashMap<Slot, Placement>,
    /// Slots whose file changed underneath an unsaved edit.
    stale: HashSet<Slot>,
    undo: Vec<(Slot, Placement)>,
    store: Box<dyn PlacementStore>,
    watcher: FileWatcher,
    status: String,
}

impl EditorSession {
    pub fn new(enabled: bool, store: Box<dyn PlacementStore>) -> Self {
        Self {
            enabled,
            open: false,
            follow_held: true,
            targets: Vec::new(),
            selected: 0,
            context: DisplayContext::FirstPersonRightHand,
            overrides: PlacementOverrides::default(),
            disk: HashMap::new(),
            stale: HashSet::new(),
            undo: Vec::new(),
            store,
            watcher: FileWatcher::new(DEFAULT_INTERVAL),
            status: String::new(),
        }
    }

    /// A session that can never open. What every build without `WYVEN_EDITOR=1`
    /// gets, and what the tests of everything else use.
    pub fn disabled() -> Self {
        Self::new(false, Box::new(super::store::FileStore))
    }

    pub fn set_targets(&mut self, targets: Vec<EditorTarget>) {
        self.targets = targets;
        self.selected = 0;
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    pub fn follows_held(&self) -> bool {
        self.follow_held
    }

    pub fn targets(&self) -> &[EditorTarget] {
        &self.targets
    }

    pub fn selected(&self) -> usize {
        self.selected
    }

    pub fn context(&self) -> DisplayContext {
        self.context
    }

    pub fn status(&self) -> &str {
        &self.status
    }

    pub fn watching(&self) -> usize {
        self.watcher.watching()
    }

    pub fn current(&self) -> Option<&EditorTarget> {
        self.targets.get(self.selected)
    }

    /// Which file a save of the current selection would touch.
    pub fn current_file(&self) -> Option<String> {
        Some(
            self.store
                .file(&self.target_at(self.selected, self.context)?),
        )
    }

    pub fn current_kind(&self) -> Option<PlacementKind> {
        Some(self.current()?.kind(self.context))
    }

    /// The value the panel shows and edits.
    pub fn value(&self) -> Option<Placement> {
        self.overrides
            .get(self.current()?.key, self.context)
            .or_else(|| {
                self.disk
                    .get(&self.slot(self.selected, self.context)?)
                    .copied()
            })
    }

    /// Whether the current selection has changes the file does not have.
    pub fn is_dirty(&self) -> bool {
        self.dirty_at(self.selected, self.context)
    }

    /// Whether the file behind the current selection moved under an unsaved
    /// edit — the one case a reload is not applied on its own.
    pub fn is_stale(&self) -> bool {
        self.slot(self.selected, self.context)
            .is_some_and(|slot| self.stale.contains(&slot))
    }

    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }

    /// Open or close the panel. Opening starts the watch, closing stops it —
    /// so a session nobody opened polls nothing.
    pub fn toggle(&mut self, stamps: &dyn Stamps) {
        if !self.enabled {
            return;
        }
        self.open = !self.open;
        match self.open {
            true => self.begin_watching(stamps),
            false => self.watcher.forget_all(),
        }
    }

    /// Whether the current target is drawn in this context at all.
    pub fn offers(&self, context: DisplayContext) -> bool {
        self.current().is_some_and(|target| target.offers(context))
    }

    /// The contexts the panel should show tabs for.
    pub fn contexts(&self) -> &[DisplayContext] {
        self.current()
            .map_or(&CONTEXTS[..0], |target| &target.offers)
    }

    /// Move off a tab the newly selected target does not have.
    fn settle_context(&mut self) {
        if self.offers(self.context) {
            return;
        }
        if let Some(&first) = self.contexts().first() {
            self.context = first;
        }
    }

    /// Select whatever is in the hand, when the panel is following it.
    ///
    /// Takes a resolved [`PlacementKey`] rather than an [`ItemId`] because only
    /// the state layer can tell a block item — which shares one placement with
    /// every other block item — from an item with a model of its own.
    pub fn follow(&mut self, held: Option<PlacementKey>, stamps: &dyn Stamps) {
        if !self.open || !self.follow_held {
            return;
        }
        let Some(held) = held else { return };
        let Some(index) = self.targets.iter().position(|target| target.key == held) else {
            return;
        };
        if index != self.selected {
            self.selected = index;
            self.settle_context();
            self.seed(stamps);
        }
    }

    /// Poll the watched files and take in whatever changed.
    pub fn tick(&mut self, dt: f32, stamps: &dyn Stamps) {
        if !self.open {
            return;
        }
        for path in self.watcher.tick(dt, stamps) {
            self.reload_file(&path);
        }
    }

    pub fn apply(&mut self, action: EditorAction, stamps: &dyn Stamps) {
        match action {
            EditorAction::Select(index) if index < self.targets.len() => {
                self.selected = index;
                self.follow_held = false;
                self.settle_context();
                self.seed(stamps);
            }
            EditorAction::Select(_) => {}
            // A tab the target does not offer is not a tab at all — a block item
            // has no `gui` placement to move.
            EditorAction::Context(context) if self.offers(context) => {
                self.context = context;
                self.seed(stamps);
            }
            EditorAction::Context(_) => {}
            EditorAction::Edit(value) => self.edit(value),
            EditorAction::Save => self.save(stamps),
            EditorAction::Reload => self.reload_selection(),
            EditorAction::Undo => self.undo(),
            EditorAction::FollowHeld(follow) => self.follow_held = follow,
            EditorAction::Close => {
                self.open = false;
                self.watcher.forget_all();
            }
        }
    }

    /// Watch every file a target's placement could live in.
    fn begin_watching(&mut self, stamps: &dyn Stamps) {
        self.watcher.forget_all();
        for index in 0..self.targets.len() {
            for context in self.targets[index].offers.clone() {
                if let Some(target) = self.target_at(index, context) {
                    let path = self.store.file(&target);
                    if !self.watcher.is_watching(&path) {
                        self.watcher.watch(&path, stamps);
                    }
                }
            }
        }
        self.seed(stamps);
        self.status = format!("watching {} files", self.watcher.watching());
    }

    /// Read the current selection's value off disk, unless it is already being
    /// edited. Seeding is what makes the panel open showing the number that is
    /// actually placing the item, rather than an identity it never had.
    fn seed(&mut self, stamps: &dyn Stamps) {
        let Some(slot) = self.slot(self.selected, self.context) else {
            return;
        };
        if self.disk.contains_key(&slot) {
            return;
        }
        let Some(target) = self.target_at(self.selected, self.context) else {
            return;
        };
        match self.store.read(&target) {
            Ok(value) => {
                self.disk.insert(slot, value);
                let path = self.store.file(&target);
                if !self.watcher.is_watching(&path) {
                    self.watcher.watch(&path, stamps);
                }
            }
            Err(err) => self.status = err,
        }
    }

    fn edit(&mut self, value: Placement) {
        let (Some(target), Some(slot)) = (
            self.targets.get(self.selected).cloned(),
            self.slot(self.selected, self.context),
        ) else {
            return;
        };
        if let Some(previous) = self.value()
            && previous != value
        {
            self.undo.push((slot, previous));
            if self.undo.len() > UNDO_DEPTH {
                self.undo.remove(0);
            }
        }
        self.overrides.set(target.key, self.context, value);
    }

    fn save(&mut self, stamps: &dyn Stamps) {
        let (Some(target), Some(slot), Some(value)) = (
            self.target_at(self.selected, self.context),
            self.slot(self.selected, self.context),
            self.value(),
        ) else {
            return;
        };
        match self.store.write(&target, &value) {
            Ok(()) => {
                self.disk.insert(slot, value);
                self.stale.remove(&slot);
                // Re-baseline, or the next poll would see our own write and
                // reload it over whatever has been moved since.
                self.watcher.watch(&self.store.file(&target), stamps);
                self.status = format!("saved {}", self.store.file(&target));
            }
            Err(err) => self.status = format!("could not save: {err}"),
        }
    }

    /// Take what the current selection's file says now, discarding unsaved work.
    fn reload_selection(&mut self) {
        let (Some(target), Some(slot)) = (
            self.target_at(self.selected, self.context),
            self.slot(self.selected, self.context),
        ) else {
            return;
        };
        match self.store.read(&target) {
            Ok(value) => {
                self.disk.insert(slot, value);
                self.stale.remove(&slot);
                // Take the file's value, not the one the game booted with: an
                // external edit may have moved it since.
                if let Some(key) = self.targets.get(self.selected).map(|target| target.key) {
                    self.overrides.set(key, self.context, value);
                }
                self.status = format!("reloaded {}", self.store.file(&target));
            }
            Err(err) => self.status = format!("could not reload: {err}"),
        }
    }

    /// Take in a file that changed on disk.
    ///
    /// A slot the developer has not touched is adopted silently — that is the
    /// whole point of the watcher. One with unsaved changes is only *flagged*:
    /// throwing away work because someone else saved would be the worst thing
    /// this tool could do.
    fn reload_file(&mut self, path: &str) {
        let slots: Vec<(usize, DisplayContext)> = (0..self.targets.len())
            .flat_map(|index| {
                self.targets[index]
                    .offers
                    .clone()
                    .into_iter()
                    .map(move |context| (index, context))
            })
            .collect();

        // Deliberately *not* limited to values the panel has already opened: a
        // developer who re-exports from Blockbench moves whichever context they
        // were working on, which is rarely the tab that happens to be up. A slot
        // this session has never looked at still has to take the new value, or
        // half of "edit the file and it just changes" would silently not work.
        // Each value is read once — `Slot` collapses the four tabs of a
        // spec-placed model onto the one number behind them.
        let mut visited: HashSet<Slot> = HashSet::new();
        let mut reloaded = 0;
        let mut blocked = 0;
        for (index, context) in slots {
            let (Some(target), Some(slot)) =
                (self.target_at(index, context), self.slot(index, context))
            else {
                continue;
            };
            if !visited.insert(slot) || self.store.file(&target) != path {
                continue;
            }
            if self.dirty_at(index, context) {
                self.stale.insert(slot);
                blocked += 1;
                continue;
            }
            match self.store.read(&target) {
                Ok(value) => {
                    self.disk.insert(slot, value);
                    self.overrides.set(self.targets[index].key, context, value);
                    reloaded += 1;
                }
                Err(err) => self.status = err,
            }
        }

        self.status = match (reloaded, blocked) {
            (0, 0) => return,
            (_, 0) => format!("reloaded {path}"),
            (0, _) => format!("{path} changed on disk — Revert to take it"),
            _ => format!("reloaded {path} ({blocked} kept, edited here)"),
        };
    }

    fn undo(&mut self) {
        let Some((slot, value)) = self.undo.pop() else {
            return;
        };
        let (index, context) = match slot {
            Slot::Display(index, context) => (index, context),
            Slot::Spec(index) => (index, self.context),
        };
        let Some(target) = self.targets.get(index) else {
            return;
        };
        self.overrides.set(target.key, context, value);
        self.selected = index;
        self.context = context;
    }

    fn dirty_at(&self, index: usize, context: DisplayContext) -> bool {
        let (Some(slot), Some(target)) = (self.slot(index, context), self.targets.get(index))
        else {
            return false;
        };
        match (
            self.overrides.get(target.key, context),
            self.disk.get(&slot),
        ) {
            (Some(current), Some(saved)) => current != *saved,
            _ => false,
        }
    }

    fn slot(&self, index: usize, context: DisplayContext) -> Option<Slot> {
        Some(match self.targets.get(index)?.kind(context) {
            PlacementKind::Display => Slot::Display(index, context),
            PlacementKind::Spec => Slot::Spec(index),
        })
    }

    fn target_at(&self, index: usize, context: DisplayContext) -> Option<Target> {
        let target = self.targets.get(index)?;
        Some(Target {
            item: target.id.clone(),
            model: target.model.clone(),
            context,
            kind: target.kind(context),
        })
    }
}

impl PlacementSource for EditorSession {
    /// Applied whether or not the panel is open: closing it must not make the
    /// item jump back to a value the developer has not saved yet.
    fn local(&self, key: PlacementKey, context: DisplayContext) -> Option<Mat4> {
        self.overrides.local(key, context)
    }
}

#[cfg(test)]
mod tests;

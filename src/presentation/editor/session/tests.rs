//! Tests for [`super`]: `session.rs`.

use std::time::SystemTime;

use wyven_model::display::ItemTransform;

use super::super::placement::SpecPlacement;
use super::super::store::InMemoryStore;
use super::*;
use crate::domain::inventory::ItemId;

/// Nothing here polls a real file, so the stamps never move.
struct NoStamps;
impl Stamps for NoStamps {
    fn modified(&self, _path: &str) -> Option<SystemTime> {
        None
    }
}

/// A clock the test moves by hand, so "someone saved the file" needs no
/// filesystem and no sleeping.
#[derive(Default)]
struct MovingStamps(std::cell::Cell<u64>);
impl MovingStamps {
    fn bump(&self) {
        self.0.set(self.0.get() + 1);
    }
}
impl Stamps for MovingStamps {
    fn modified(&self, _path: &str) -> Option<SystemTime> {
        Some(SystemTime::UNIX_EPOCH + std::time::Duration::from_secs(self.0.get()))
    }
}

const SWORD: PlacementKey = PlacementKey::Item(ItemId(3));
const PLANT: PlacementKey = PlacementKey::Item(ItemId(9));
const FIRST: DisplayContext = DisplayContext::FirstPersonRightHand;
const THIRD: DisplayContext = DisplayContext::ThirdPersonRightHand;

fn transform(x: f32) -> ItemTransform {
    ItemTransform {
        rotation: [x, 0.0, 0.0],
        translation: [0.0, 1.0, 1.0],
        scale: [0.8; 3],
    }
}

fn targets() -> Vec<EditorTarget> {
    vec![
        EditorTarget {
            key: SWORD,
            id: "wooden_sword".to_string(),
            name: "Wooden Sword".to_string(),
            model: "assets/models/items/wooden_sword.json".to_string(),
            offers: CONTEXTS.to_vec(),
            declared: CONTEXTS.to_vec(),
        },
        EditorTarget {
            key: PLANT,
            id: "blue_bells".to_string(),
            name: "Blue Bells".to_string(),
            model: "assets/models/blocks/plant1.bbmodel".to_string(),
            offers: CONTEXTS.to_vec(),
            declared: Vec::new(),
        },
    ]
}

/// A session over the in-memory store, seeded with both items' values.
fn session() -> EditorSession {
    let store = InMemoryStore::default()
        .with("wooden_sword", FIRST, Placement::Display(transform(-99.9)))
        .with("wooden_sword", THIRD, Placement::Display(transform(-83.4)))
        .with(
            "blue_bells",
            FIRST,
            Placement::Spec(SpecPlacement {
                scale: 0.4,
                offset: [-0.5, 0.0, -0.5],
                rotation: [0.0; 3],
            }),
        );
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&NoStamps);
    session
}

#[test]
fn a_disabled_session_never_opens() {
    let mut session = EditorSession::new(false, Box::new(InMemoryStore::default()));
    session.set_targets(targets());
    session.toggle(&NoStamps);
    assert!(
        !session.is_open(),
        "F6 must do nothing without WYVEN_EDITOR=1"
    );
    assert!(!session.is_enabled());
}

/// The panel has to open showing the number that is actually placing the
/// item — an identity it never had would look like the editor broke it.
#[test]
fn opening_seeds_the_current_value_from_disk() {
    let session = session();
    assert!(session.is_open());
    assert_eq!(session.value(), Some(Placement::Display(transform(-99.9))));
    assert!(!session.is_dirty());
}

#[test]
fn an_edit_is_dirty_until_it_is_saved() {
    let mut session = session();
    session.apply(
        EditorAction::Edit(Placement::Display(transform(10.0))),
        &NoStamps,
    );

    assert!(session.is_dirty());
    assert_eq!(session.value(), Some(Placement::Display(transform(10.0))));
    assert!(
        session.local(SWORD, FIRST).is_some(),
        "and it is what the renderer sees"
    );

    session.apply(EditorAction::Save, &NoStamps);
    assert!(!session.is_dirty());
    assert!(
        session.status().starts_with("saved"),
        "{}",
        session.status()
    );
}

#[test]
fn a_failed_save_says_so_and_keeps_the_edit() {
    let store =
        InMemoryStore::default().with("wooden_sword", FIRST, Placement::Display(transform(0.0)));
    store.fail_writes.set(true);
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&NoStamps);

    session.apply(
        EditorAction::Edit(Placement::Display(transform(5.0))),
        &NoStamps,
    );
    session.apply(EditorAction::Save, &NoStamps);

    assert!(session.is_dirty(), "the work is not thrown away");
    assert!(
        session.status().contains("read-only"),
        "{}",
        session.status()
    );
}

#[test]
fn reverting_takes_what_the_file_says() {
    let mut session = session();
    session.apply(
        EditorAction::Edit(Placement::Display(transform(10.0))),
        &NoStamps,
    );
    session.apply(EditorAction::Reload, &NoStamps);

    assert!(!session.is_dirty());
    assert_eq!(session.value(), Some(Placement::Display(transform(-99.9))));
}

#[test]
fn undo_walks_back_and_returns_to_where_the_edit_was_made() {
    let mut session = session();
    session.apply(
        EditorAction::Edit(Placement::Display(transform(1.0))),
        &NoStamps,
    );
    session.apply(
        EditorAction::Edit(Placement::Display(transform(2.0))),
        &NoStamps,
    );
    session.apply(EditorAction::Context(THIRD), &NoStamps);

    assert!(session.can_undo());
    session.apply(EditorAction::Undo, &NoStamps);
    assert_eq!(session.context(), FIRST, "undo goes back to what it undid");
    assert_eq!(session.value(), Some(Placement::Display(transform(1.0))));

    session.apply(EditorAction::Undo, &NoStamps);
    assert_eq!(session.value(), Some(Placement::Display(transform(-99.9))));
    assert!(!session.can_undo());
}

#[test]
fn re_editing_the_same_value_does_not_grow_the_undo_stack() {
    let mut session = session();
    let same = Placement::Display(transform(-99.9));
    session.apply(EditorAction::Edit(same), &NoStamps);
    assert!(!session.can_undo());
}

/// Each context of a model that places itself is its own value; moving the
/// first-person entry must not move the third-person one.
#[test]
fn declared_contexts_are_edited_independently() {
    let mut session = session();
    session.apply(
        EditorAction::Edit(Placement::Display(transform(10.0))),
        &NoStamps,
    );
    session.apply(EditorAction::Context(THIRD), &NoStamps);

    assert_eq!(session.value(), Some(Placement::Display(transform(-83.4))));
    assert!(!session.is_dirty(), "the other context is untouched");
    assert_eq!(session.current_kind(), Some(PlacementKind::Display));
}

/// A `.bbmodel` has one `[item.model]` table and no `display` block, so one
/// number serves every context — and the panel must not pretend otherwise.
#[test]
fn an_undeclared_model_is_edited_as_one_shared_spec() {
    let mut session = session();
    session.apply(EditorAction::Select(1), &NoStamps);

    assert_eq!(session.current_kind(), Some(PlacementKind::Spec));
    assert_eq!(session.current_file().as_deref(), Some("assets/items.toml"));
    assert!(session.value().expect("seeded").is_shared_across_contexts());

    let moved = Placement::Spec(SpecPlacement {
        scale: 0.9,
        offset: [0.0; 3],
        rotation: [0.0; 3],
    });
    session.apply(EditorAction::Edit(moved), &NoStamps);
    for context in CONTEXTS {
        assert_eq!(
            session.local(PLANT, context),
            Some(moved.matrix()),
            "{context:?} moves with the spec"
        );
    }

    session.apply(EditorAction::Context(DisplayContext::Ground), &NoStamps);
    assert!(session.is_dirty(), "one spec, one unsaved change");
}

#[test]
fn selecting_stops_following_the_hand_and_following_selects() {
    let mut session = session();
    assert!(session.follows_held());

    session.follow(Some(PLANT), &NoStamps);
    assert_eq!(session.selected(), 1);

    session.apply(EditorAction::Select(0), &NoStamps);
    assert!(!session.follows_held(), "picking one by hand pins it");
    session.follow(Some(PLANT), &NoStamps);
    assert_eq!(session.selected(), 0, "and the hand no longer moves it");
}

#[test]
fn closing_the_panel_keeps_unsaved_work_on_screen() {
    let mut session = session();
    session.apply(
        EditorAction::Edit(Placement::Display(transform(10.0))),
        &NoStamps,
    );
    session.apply(EditorAction::Close, &NoStamps);

    assert!(!session.is_open());
    assert_eq!(
        session.local(SWORD, FIRST),
        Some(transform(10.0).matrix()),
        "closing the panel must not make the item jump"
    );
    assert_eq!(session.watching(), 0, "and the watcher stops");
}

/// The point of the watcher: change the file in Blockbench or a text
/// editor and the running game takes it, with no restart and no click.
#[test]
fn an_external_change_is_adopted_when_nothing_is_unsaved() {
    let stamps = MovingStamps::default();
    let store =
        InMemoryStore::default().with("wooden_sword", FIRST, Placement::Display(transform(-99.9)));
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&stamps);
    assert_eq!(session.value(), Some(Placement::Display(transform(-99.9))));

    // Someone else writes the file.
    let at = Target {
        item: "wooden_sword".to_string(),
        model: "assets/models/items/wooden_sword.json".to_string(),
        context: FIRST,
        kind: PlacementKind::Display,
    };
    session
        .store
        .write(&at, &Placement::Display(transform(42.0)))
        .expect("writes");
    stamps.bump();
    session.tick(1.0, &stamps);

    assert_eq!(session.value(), Some(Placement::Display(transform(42.0))));
    assert_eq!(
        session.local(SWORD, FIRST),
        Some(transform(42.0).matrix()),
        "and the hand follows it this frame"
    );
    assert!(!session.is_dirty());
}

/// The one thing this tool must never do is throw away work because someone
/// else saved. An unsaved slot is flagged, not overwritten.
#[test]
fn an_external_change_under_an_unsaved_edit_is_flagged_rather_than_taken() {
    let stamps = MovingStamps::default();
    let store =
        InMemoryStore::default().with("wooden_sword", FIRST, Placement::Display(transform(-99.9)));
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&stamps);

    session.apply(
        EditorAction::Edit(Placement::Display(transform(7.0))),
        &stamps,
    );
    let at = Target {
        item: "wooden_sword".to_string(),
        model: "assets/models/items/wooden_sword.json".to_string(),
        context: FIRST,
        kind: PlacementKind::Display,
    };
    session
        .store
        .write(&at, &Placement::Display(transform(42.0)))
        .expect("writes");
    stamps.bump();
    session.tick(1.0, &stamps);

    assert_eq!(
        session.value(),
        Some(Placement::Display(transform(7.0))),
        "the unsaved edit stands"
    );
    assert!(session.is_stale(), "but the panel says the file moved");
    assert!(
        session.status().contains("changed on disk"),
        "{}",
        session.status()
    );

    session.apply(EditorAction::Reload, &stamps);
    assert_eq!(session.value(), Some(Placement::Display(transform(42.0))));
    assert!(!session.is_stale());
}

/// The game writes the file too. Seeing its own write and reloading it
/// would stamp on whatever was moved in the meantime.
#[test]
fn the_editors_own_save_does_not_come_back_as_an_external_change() {
    let stamps = MovingStamps::default();
    let store =
        InMemoryStore::default().with("wooden_sword", FIRST, Placement::Display(transform(-99.9)));
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&stamps);

    session.apply(
        EditorAction::Edit(Placement::Display(transform(3.0))),
        &stamps,
    );
    session.apply(EditorAction::Save, &stamps);
    stamps.bump();

    session.apply(
        EditorAction::Edit(Placement::Display(transform(4.0))),
        &stamps,
    );
    session.tick(1.0, &stamps);
    assert_eq!(
        session.value(),
        Some(Placement::Display(transform(4.0))),
        "the edit made after the save survives the poll"
    );
}

/// The gap this closes: a Blockbench re-export moves whichever context the
/// author was working on, which is rarely the tab that happens to be up.
#[test]
fn an_external_change_reaches_a_context_the_panel_never_opened() {
    let stamps = MovingStamps::default();
    let store = InMemoryStore::default()
        .with("wooden_sword", FIRST, Placement::Display(transform(-99.9)))
        .with("wooden_sword", THIRD, Placement::Display(transform(-83.4)));
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(targets());
    session.toggle(&stamps);
    // Only the first-person tab has ever been looked at.
    assert_eq!(session.context(), FIRST);
    assert_eq!(session.local(SWORD, THIRD), None);

    let at = Target {
        item: "wooden_sword".to_string(),
        model: "assets/models/items/wooden_sword.json".to_string(),
        context: THIRD,
        kind: PlacementKind::Display,
    };
    session
        .store
        .write(&at, &Placement::Display(transform(12.0)))
        .expect("writes");
    stamps.bump();
    session.tick(1.0, &stamps);

    assert_eq!(
        session.local(SWORD, THIRD),
        Some(transform(12.0).matrix()),
        "the third-person placement follows the file too"
    );
}

#[test]
fn a_closed_session_polls_nothing() {
    let stamps = MovingStamps::default();
    let mut session = session();
    session.apply(EditorAction::Close, &stamps);
    stamps.bump();
    session.tick(1.0, &stamps);
    assert_eq!(
        session.status(),
        "watching 2 files",
        "status is from opening"
    );
}

/// Selecting the block cube while sitting on a tab it does not have must
/// land somewhere real, not leave the panel showing nothing.
#[test]
fn selecting_a_target_without_the_current_tab_moves_to_one_it_has() {
    let store = InMemoryStore::default().with("block", FIRST, Placement::Display(transform(45.0)));
    let mut session = EditorSession::new(true, Box::new(store));
    let hands = vec![FIRST, THIRD];
    session.set_targets(vec![
        targets()[0].clone(),
        EditorTarget {
            key: PlacementKey::BlockItem,
            id: "block".to_string(),
            name: "Block items".to_string(),
            model: "assets/models/items/block.json".to_string(),
            offers: hands.clone(),
            declared: hands,
        },
    ]);
    session.toggle(&NoStamps);

    session.apply(EditorAction::Context(DisplayContext::Gui), &NoStamps);
    assert_eq!(session.context(), DisplayContext::Gui);

    session.apply(EditorAction::Select(1), &NoStamps);
    assert_eq!(session.context(), FIRST, "moved onto a tab the cube has");
    assert_eq!(session.contexts(), [FIRST, THIRD]);
    assert_eq!(session.value(), Some(Placement::Display(transform(45.0))));

    // And a tab it does not have cannot be selected while it is up.
    session.apply(EditorAction::Context(DisplayContext::Ground), &NoStamps);
    assert_eq!(session.context(), FIRST);
}

/// One value, every block: editing the cube has to move the dirt in your
/// hand and the stone in the next slot alike.
#[test]
fn the_block_cube_override_reaches_every_block_item() {
    let hands = vec![FIRST, THIRD];
    let store = InMemoryStore::default().with("block", FIRST, Placement::Display(transform(0.0)));
    let mut session = EditorSession::new(true, Box::new(store));
    session.set_targets(vec![EditorTarget {
        key: PlacementKey::BlockItem,
        id: "block".to_string(),
        name: "Block items".to_string(),
        model: "assets/models/items/block.json".to_string(),
        offers: hands.clone(),
        declared: hands,
    }]);
    session.toggle(&NoStamps);

    let moved = transform(33.0);
    session.apply(EditorAction::Edit(Placement::Display(moved)), &NoStamps);
    assert_eq!(
        session.local(PlacementKey::BlockItem, FIRST),
        Some(moved.matrix())
    );
    assert_eq!(
        session.local(PlacementKey::Item(ItemId(3)), FIRST),
        None,
        "and leaves an item with a model of its own alone"
    );
}

#[test]
fn an_out_of_range_selection_is_ignored() {
    let mut session = session();
    session.apply(EditorAction::Select(99), &NoStamps);
    assert_eq!(session.selected(), 0);
}

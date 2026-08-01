//! Where a session's world state gets persisted.
//!
//! The in-game state used to hold an `Option<WorldSave>` and guard every write
//! with `if self.save.is_none() { return; }` — the `None` standing in for
//! "client" and "ephemeral dev-boot world". [`WorldRepository`] replaces that
//! with a null object ([`NullWorldRepository`]), so the state layer always has
//! *a* repository and never branches on whether persistence exists.
//!
//! It also gives saving a seam: [`InMemoryWorldRepository`] exercises the
//! capture → store → reload path with no `saves/` directory involved.

use std::sync::{Arc, Mutex};

use crate::core::GameMode;

use super::{MobsData, PlayerData, PlayerRecords, SaveError, WorldData, WorldSave};

/// Everything one save call persists. Bundling it keeps [`WorldRepository`] to
/// a single method, and keeps the caller from having to know that the metadata
/// fields (mode/spawn/time) live in a different file to the `.dat` payloads.
pub struct WorldSnapshot<'a> {
    pub world: &'a WorldData,
    pub player: &'a PlayerData,
    pub players: &'a PlayerRecords,
    pub mobs: &'a MobsData,
    pub game_mode: GameMode,
    pub spawn: [f32; 3],
    pub time_of_day: f32,
}

/// A destination for world saves.
pub trait WorldRepository {
    /// Persist a snapshot.
    fn store(&mut self, snapshot: &WorldSnapshot<'_>) -> Result<(), SaveError>;

    /// Whether this repository actually writes anywhere. Callers check it to
    /// skip *building* a snapshot — capturing the edit overlay and mob list is
    /// real work that a no-op destination shouldn't pay for.
    fn is_persistent(&self) -> bool;

    /// World name, for log messages.
    fn world_name(&self) -> &str;
}

/// Writes to a `saves/<slug>/` directory — the real game's persistence.
pub struct FileWorldRepository {
    save: WorldSave,
}

impl FileWorldRepository {
    pub fn new(save: WorldSave) -> Self {
        Self { save }
    }

    /// The underlying save handle (its `meta` carries seed/slug for the menus).
    pub fn save(&self) -> &WorldSave {
        &self.save
    }
}

impl WorldRepository for FileWorldRepository {
    fn store(&mut self, snapshot: &WorldSnapshot<'_>) -> Result<(), SaveError> {
        self.save.meta.game_mode = snapshot.game_mode;
        self.save.meta.spawn = snapshot.spawn;
        self.save.meta.time_of_day = snapshot.time_of_day;
        self.save.write(
            snapshot.world,
            snapshot.player,
            snapshot.players,
            snapshot.mobs,
        )
    }

    fn is_persistent(&self) -> bool {
        true
    }

    fn world_name(&self) -> &str {
        &self.save.meta.name
    }
}

/// Discards everything. Used by multiplayer clients (the host owns the world)
/// and by `WYVEN_BOOT_INGAME` worlds started without `WYVEN_WORLD`.
pub struct NullWorldRepository;

impl WorldRepository for NullWorldRepository {
    fn store(&mut self, _snapshot: &WorldSnapshot<'_>) -> Result<(), SaveError> {
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        false
    }

    /// Matches what the debug HUD showed for a `None` save handle.
    fn world_name(&self) -> &str {
        "(unsaved)"
    }
}

/// Keeps snapshots in memory. For tests that assert on what a save captured
/// without touching the filesystem.
///
/// The state layer takes ownership of its repository (`Box<dyn …>`), so the
/// recording lives behind a shared handle: build the repository, keep its
/// [`log`](InMemoryWorldRepository::log), hand the repository over, and read
/// the log afterwards. That avoids putting `Any` downcasting on the trait
/// purely to serve tests.
#[derive(Default)]
pub struct InMemoryWorldRepository {
    log: SaveLog,
}

/// Shared record of what an [`InMemoryWorldRepository`] has stored.
pub type SaveLog = Arc<Mutex<SaveRecord>>;

#[derive(Default)]
pub struct SaveRecord {
    /// The most recent snapshot, as owned data.
    pub last: Option<StoredWorld>,
    /// How many times [`WorldRepository::store`] has been called.
    pub writes: usize,
}

/// An owned copy of one [`WorldSnapshot`].
pub struct StoredWorld {
    pub world: WorldData,
    pub player: PlayerData,
    pub players: PlayerRecords,
    pub mobs: MobsData,
    pub game_mode: GameMode,
    pub spawn: [f32; 3],
    pub time_of_day: f32,
}

impl InMemoryWorldRepository {
    /// A handle to what this repository records, readable after the repository
    /// itself has been handed to the state.
    pub fn log(&self) -> SaveLog {
        Arc::clone(&self.log)
    }
}

impl WorldRepository for InMemoryWorldRepository {
    fn store(&mut self, snapshot: &WorldSnapshot<'_>) -> Result<(), SaveError> {
        let mut log = self.log.lock().expect("save log poisoned");
        log.last = Some(StoredWorld {
            world: snapshot.world.clone(),
            player: snapshot.player.clone(),
            players: snapshot.players.clone(),
            mobs: snapshot.mobs.clone(),
            game_mode: snapshot.game_mode,
            spawn: snapshot.spawn,
            time_of_day: snapshot.time_of_day,
        });
        log.writes += 1;
        Ok(())
    }

    fn is_persistent(&self) -> bool {
        true
    }

    fn world_name(&self) -> &str {
        "<in-memory>"
    }
}

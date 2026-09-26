//! World persistence: save directories, on-disk formats, and the profile file
//! carrying the client's stable multiplayer identity.
//!
//! A world saves as `saves/<slug>/` containing:
//! - `level.toml` — human-readable metadata (name, seed, mode, spawn, time).
//! - `world.dat` — the block-edit overlay (name-based palette), bincode.
//! - `player.dat` — the save owner's player + inventory, bincode.
//! - `players.dat` — per-identity records for multiplayer clients, bincode.
//! - `mobs.dat` — the mob population, by kind name, bincode.
//! - `progression.dat` — shrines read, altars revealed, bosses beaten, bincode.
//! - `discovery.dat` — which items each player has held (what reveals their
//!   crafting recipes), by id, bincode. Fails soft to "nothing yet".
//!
//! Worlds regenerate terrain from the seed on load; only the divergence from
//! generated terrain (the edit overlay) is stored — the same model the host
//! already uses to replay world state to joining clients. Items and blocks are
//! stored by *name* (not numeric id) because ids are registry-insertion-order
//! indices and would corrupt saves when block/item lists change across builds.
//!
//! All writes go through a temp-file + rename so a crash mid-save can't corrupt
//! an existing save.

pub mod data;
pub mod records;
pub mod repository;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::domain::core::GameMode;
use crate::domain::core::day_cycle::DEFAULT_START;
use crate::domain::progression::WorldProgression;

pub use data::{
    DiscoveryData, ItemStackData, MobData, MobsData, PlayerData, PlayerRecords, WorldData,
};
pub use repository::{
    FileWorldRepository, InMemoryWorldRepository, NullWorldRepository, SaveLog, WorldRepository,
    WorldSnapshot,
};

/// On-disk format version, stamped into `level.toml` and every `.dat` header.
pub const SAVE_VERSION: u32 = 2;
/// Directory holding all world saves. A leaf name under
/// [`crate::infrastructure::paths::data_dir`], not a path — see that module for where it sits.
pub use crate::infrastructure::paths::SAVES_DIR;

const LEVEL_FILE: &str = "level.toml";
const WORLD_FILE: &str = "world.dat";
const PLAYER_FILE: &str = "player.dat";
const PLAYERS_FILE: &str = "players.dat";
const MOBS_FILE: &str = "mobs.dat";
const PROGRESSION_FILE: &str = "progression.dat";
const DISCOVERY_FILE: &str = "discovery.dat";
/// Local player profile (stable multiplayer identity), next to `saves/`.

#[derive(Debug, thiserror::Error)]
pub enum SaveError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("level.toml: {0}")]
    TomlDe(#[from] toml::de::Error),
    #[error("level.toml: {0}")]
    TomlSer(#[from] toml::ser::Error),
    #[error("corrupt save data: {0}")]
    Decode(#[from] bincode::error::DecodeError),
    #[error("encode: {0}")]
    Encode(#[from] bincode::error::EncodeError),
    #[error("save version {0} not supported (this build reads {SAVE_VERSION})")]
    Version(u32),
    #[error("corrupt save: {0}")]
    Corrupt(String),
    #[error("a world named '{0}' already exists")]
    AlreadyExists(String),
}

/// Live metadata for one world; round-trips through `level.toml`.
#[derive(Debug, Clone)]
pub struct WorldMeta {
    pub name: String,
    pub seed: u64,
    pub game_mode: GameMode,
    pub spawn: [f32; 3],
    pub time_of_day: f32,
    pub last_played_unix: u64,
}

/// `level.toml` wire form. The seed is a decimal *string* because TOML integers
/// are i64 and seeds are u64 (string-hashed seeds routinely exceed `i64::MAX`).
#[derive(Serialize, Deserialize)]
struct LevelToml {
    version: u32,
    name: String,
    seed: String,
    game_mode: GameMode,
    spawn: [f32; 3],
    time_of_day: f32,
    last_played_unix: u64,
}

/// Handle to one world's save directory; the in-game state keeps it to write
/// saves back to the same slot.
pub struct WorldSave {
    dir: PathBuf,
    pub slug: String,
    pub meta: WorldMeta,
}

/// Everything read off disk for one world, ready to build an `InGameState`.
/// `world`/`player` are `None` for a freshly created (never saved) world.
pub struct SavedGame {
    pub save: WorldSave,
    pub world: Option<WorldData>,
    pub player: Option<PlayerData>,
    pub players: PlayerRecords,
    pub mobs: MobsData,
    /// Shrines read, altars revealed, bosses beaten. Empty for a new world.
    pub progression: WorldProgression,
    /// Items each player has held. Empty for a new or pre-crafting world.
    pub discovery: DiscoveryData,
}

/// The `.dat` payloads of one save, borrowed from the running game.
pub struct SavePayload<'a> {
    pub world: &'a WorldData,
    pub player: &'a PlayerData,
    pub players: &'a PlayerRecords,
    pub mobs: &'a MobsData,
    pub progression: &'a WorldProgression,
    pub discovery: &'a DiscoveryData,
}

/// One row of the world list in the menus.
pub struct WorldEntry {
    pub slug: String,
    pub meta: WorldMeta,
}

impl WorldSave {
    /// Create a new world directory and its initial `level.toml`. The spawn is a
    /// placeholder until the first in-game save records the real one.
    pub fn create(root: &Path, name: &str, seed: u64, mode: GameMode) -> Result<Self, SaveError> {
        let slug = slugify(name);
        let dir = root.join(&slug);
        if dir.join(LEVEL_FILE).exists() {
            return Err(SaveError::AlreadyExists(name.trim().to_string()));
        }
        fs::create_dir_all(&dir)?;
        let save = Self {
            dir,
            slug,
            meta: WorldMeta {
                name: name.trim().to_string(),
                seed,
                game_mode: mode,
                spawn: [0.5, 80.0, 0.5],
                time_of_day: DEFAULT_START,
                last_played_unix: unix_now(),
            },
        };
        save.write_level()?;
        log::info!(
            "created world '{}' (slug '{}', seed {seed})",
            save.meta.name,
            save.slug
        );
        Ok(save)
    }

    /// Open an existing world by slug, parsing and validating its `level.toml`.
    pub fn open(root: &Path, slug: &str) -> Result<Self, SaveError> {
        let dir = root.join(slug);
        let text = fs::read_to_string(dir.join(LEVEL_FILE))?;
        let level: LevelToml = toml::from_str(&text)?;
        if level.version != SAVE_VERSION {
            return Err(SaveError::Version(level.version));
        }
        let seed = level
            .seed
            .trim()
            .parse::<u64>()
            .map_err(|_| SaveError::Corrupt(format!("invalid seed '{}'", level.seed)))?;
        Ok(Self {
            dir,
            slug: slug.to_string(),
            meta: WorldMeta {
                name: level.name,
                seed,
                game_mode: level.game_mode,
                spawn: level.spawn,
                time_of_day: level.time_of_day,
                last_played_unix: level.last_played_unix,
            },
        })
    }

    /// Open the world named `name` if it exists, else create it with `seed` and
    /// `mode`. Used by the `WYVEN_WORLD` boot path.
    pub fn open_or_create(
        root: &Path,
        name: &str,
        seed: u64,
        mode: GameMode,
    ) -> Result<Self, SaveError> {
        let slug = slugify(name);
        if root.join(&slug).join(LEVEL_FILE).exists() {
            Self::open(root, &slug)
        } else {
            Self::create(root, name, seed, mode)
        }
    }

    /// Read the world's data files. A missing `world.dat`/`player.dat` means a
    /// fresh world/player (`None`); a *corrupt* `world.dat` is a hard error (the
    /// terrain edits are the world), while corrupt player files fail soft so the
    /// terrain remains playable.
    pub fn load(self) -> Result<SavedGame, SaveError> {
        let world = read_dat::<WorldData>(&self.dir.join(WORLD_FILE))?;
        let player = read_dat::<PlayerData>(&self.dir.join(PLAYER_FILE)).unwrap_or_else(|err| {
            log::warn!("ignoring corrupt player.dat for '{}': {err}", self.slug);
            None
        });
        let players = read_dat::<PlayerRecords>(&self.dir.join(PLAYERS_FILE))
            .unwrap_or_else(|err| {
                log::warn!("ignoring corrupt players.dat for '{}': {err}", self.slug);
                None
            })
            .unwrap_or_default();
        // Missing = a pre-mobs save (fresh population); corrupt fails soft
        // like the player files — the terrain remains playable.
        let mobs = read_dat::<MobsData>(&self.dir.join(MOBS_FILE))
            .unwrap_or_else(|err| {
                log::warn!("ignoring corrupt mobs.dat for '{}': {err}", self.slug);
                None
            })
            .unwrap_or_default();
        // Fails soft like the mob population: losing it costs the players a
        // walk back to a shrine, never the world.
        let progression = read_dat::<WorldProgression>(&self.dir.join(PROGRESSION_FILE))
            .unwrap_or_else(|err| {
                log::warn!(
                    "ignoring corrupt progression.dat for '{}': {err}",
                    self.slug
                );
                None
            })
            .unwrap_or_default();
        // Missing = a world from before crafting discovery; its players
        // re-learn from what they carry on the first frame.
        let discovery = read_dat::<DiscoveryData>(&self.dir.join(DISCOVERY_FILE))
            .unwrap_or_else(|err| {
                log::warn!("ignoring corrupt discovery.dat for '{}': {err}", self.slug);
                None
            })
            .unwrap_or_default();
        Ok(SavedGame {
            save: self,
            world,
            player,
            players,
            mobs,
            progression,
            discovery,
        })
    }

    /// Persist the world: metadata + edits + the local player + remote-player
    /// records + the mob population + progression. Bumps `last_played`.
    pub fn write(&mut self, payload: &SavePayload<'_>) -> Result<(), SaveError> {
        let SavePayload {
            world,
            player,
            players,
            mobs,
            progression,
            discovery,
        } = payload;
        self.meta.last_played_unix = unix_now();
        self.write_level()?;
        write_dat(&self.dir.join(WORLD_FILE), world)?;
        write_dat(&self.dir.join(PLAYER_FILE), player)?;
        write_dat(&self.dir.join(PLAYERS_FILE), players)?;
        write_dat(&self.dir.join(MOBS_FILE), mobs)?;
        write_dat(&self.dir.join(PROGRESSION_FILE), progression)?;
        write_dat(&self.dir.join(DISCOVERY_FILE), discovery)?;
        Ok(())
    }

    fn write_level(&self) -> Result<(), SaveError> {
        let level = LevelToml {
            version: SAVE_VERSION,
            name: self.meta.name.clone(),
            seed: self.meta.seed.to_string(),
            game_mode: self.meta.game_mode,
            spawn: self.meta.spawn,
            time_of_day: self.meta.time_of_day,
            last_played_unix: self.meta.last_played_unix,
        };
        let text = toml::to_string_pretty(&level)?;
        write_atomic(&self.dir.join(LEVEL_FILE), text.as_bytes())?;
        Ok(())
    }
}

/// The saves root: `<data dir>/saves`.
///
/// Deliberately not next to `assets/`. The install directory is replaced
/// wholesale by a launcher applying an update, and a world must survive that.
pub fn saves_root() -> PathBuf {
    crate::infrastructure::paths::saves_root()
}

/// Scan the saves root for worlds. Unreadable/corrupt entries are skipped with
/// a warning; a missing root is simply an empty list. Sorted most-recent first.
pub fn list_worlds(root: &Path) -> Vec<WorldEntry> {
    let Ok(entries) = fs::read_dir(root) else {
        return Vec::new();
    };
    let mut worlds: Vec<WorldEntry> = entries
        .flatten()
        .filter(|e| e.path().is_dir())
        .filter_map(|e| {
            let slug = e.file_name().to_string_lossy().to_string();
            match WorldSave::open(root, &slug) {
                Ok(save) => Some(WorldEntry {
                    slug,
                    meta: save.meta,
                }),
                Err(err) => {
                    log::warn!("skipping unreadable world '{slug}': {err}");
                    None
                }
            }
        })
        .collect();
    worlds.sort_by_key(|w| std::cmp::Reverse(w.meta.last_played_unix));
    worlds
}

/// Delete a world's directory (irreversible; the menus double-confirm).
pub fn delete_world(root: &Path, slug: &str) -> std::io::Result<()> {
    fs::remove_dir_all(root.join(slug))
}

/// Directory-safe slug for a world name: lowercase `[a-z0-9-]`, dashes collapsed.
pub fn slugify(name: &str) -> String {
    let mut slug = String::new();
    let mut last_dash = true; // suppress a leading dash
    for c in name.trim().chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
            last_dash = false;
        } else if !last_dash {
            slug.push('-');
            last_dash = true;
        }
    }
    while slug.ends_with('-') {
        slug.pop();
    }
    if slug.is_empty() {
        "world".to_string()
    } else if is_reserved_device_name(&slug) {
        // Windows refuses these as directory names. Suffixed on every OS so a
        // save copied between machines keeps the same slug.
        format!("{slug}-world")
    } else {
        slug
    }
}

/// Names Windows reserves for devices, which no file or directory may take.
fn is_reserved_device_name(slug: &str) -> bool {
    match slug {
        "con" | "prn" | "aux" | "nul" => true,
        _ => {
            let (prefix, digit) = slug.split_at(slug.len().min(3));
            matches!(prefix, "com" | "lpt")
                && digit.len() == 1
                && digit.as_bytes()[0].is_ascii_digit()
        }
    }
}

pub use crate::domain::core::seed::{parse_seed, random_seed};
use crate::infrastructure::fs::write_atomic;
pub use crate::infrastructure::profile::{
    AccountProfile, client_identity, local_identity, store_account, stored_account,
};

/// `.dat` framing: 4-byte LE version header followed by a bincode payload.
fn write_dat<T: Serialize>(path: &Path, value: &T) -> Result<(), SaveError> {
    let mut bytes = SAVE_VERSION.to_le_bytes().to_vec();
    bytes.extend(bincode::serde::encode_to_vec(
        value,
        bincode::config::standard(),
    )?);
    write_atomic(path, &bytes)?;
    Ok(())
}

/// Read a `.dat` file; `Ok(None)` if it doesn't exist. The version header is
/// checked before decoding the payload.
fn read_dat<T: DeserializeOwned>(path: &Path) -> Result<Option<T>, SaveError> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(err) => return Err(err.into()),
    };
    let Some(header) = bytes.get(0..4) else {
        return Err(SaveError::Corrupt(format!(
            "{} is truncated",
            path.display()
        )));
    };
    let version = u32::from_le_bytes(header.try_into().expect("4-byte slice"));
    if version != SAVE_VERSION {
        return Err(SaveError::Version(version));
    }
    let (value, _) = bincode::serde::decode_from_slice(&bytes[4..], bincode::config::standard())?;
    Ok(Some(value))
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A unique temp root per test so parallel tests don't collide.
    fn temp_root(tag: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("wyven-save-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    #[test]
    fn slugify_normalizes_names() {
        assert_eq!(slugify("My World!"), "my-world");
        assert_eq!(slugify("  Cliffs & CAVES 2 "), "cliffs-caves-2");
        assert_eq!(slugify("!!!"), "world");
        assert_eq!(slugify(""), "world");
    }

    #[test]
    fn slugify_avoids_windows_device_names() {
        assert_eq!(slugify("CON"), "con-world");
        assert_eq!(slugify("nul"), "nul-world");
        assert_eq!(slugify("Com1"), "com1-world");
        assert_eq!(slugify("lpt9"), "lpt9-world");
        assert_eq!(slugify("com0"), "com0-world");
        assert_eq!(slugify("com10"), "com10");
        assert_eq!(slugify("console"), "console");
    }

    #[test]
    fn create_write_open_load_roundtrip() {
        let root = temp_root("roundtrip");
        let mut save =
            WorldSave::create(&root, "Test World", u64::MAX, GameMode::Creative).unwrap();
        assert_eq!(save.slug, "test-world");

        // Duplicate creation is rejected.
        assert!(matches!(
            WorldSave::create(&root, "Test  World!", 1, GameMode::Survival),
            Err(SaveError::AlreadyExists(_))
        ));

        let world = WorldData {
            palette: vec!["stone".into()],
            edits: vec![(crate::domain::core::BlockPos::new(1, 70, -3), 0)],
        };
        let player = PlayerData {
            position: [1.0, 72.0, -3.0],
            yaw: 0.5,
            pitch: -0.25,
            flying: true,
            health: 17.0,
            hunger: 12.0,
            saturation: 3.0,
            selected_slot: 4,
            slots: vec![Some(ItemStackData {
                id: "stone".into(),
                count: 12,
                durability: None,
            })],
        };
        let mut players = PlayerRecords::default();
        players.0.insert(77, player.clone());
        let mobs = MobsData {
            mobs: vec![MobData {
                kind: "cow".into(),
                position: [8.0, 65.0, -2.0],
                health: 6.5,
                night_spawned: false,
            }],
        };
        save.meta.spawn = [0.5, 71.0, 0.5];
        save.meta.time_of_day = 0.42;
        let mut progression = WorldProgression::default();
        progression.read_shrine(
            crate::domain::core::BlockPos::new(40, 97, 12),
            "meadows_altar",
            crate::domain::core::BlockPos::new(300, 95, -80),
        );
        progression.defeat("elder stag");
        let mut discovery = DiscoveryData {
            owner: vec!["oak_log".into(), "coal".into()],
            ..DiscoveryData::default()
        };
        discovery.players.insert(77, vec!["flint".into()]);
        save.write(&SavePayload {
            world: &world,
            player: &player,
            players: &players,
            mobs: &mobs,
            progression: &progression,
            discovery: &discovery,
        })
        .unwrap();

        let game = WorldSave::open(&root, "test-world")
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(game.save.meta.name, "Test World");
        assert_eq!(game.save.meta.seed, u64::MAX);
        assert_eq!(game.save.meta.spawn, [0.5, 71.0, 0.5]);
        assert!((game.save.meta.time_of_day - 0.42).abs() < 1e-6);
        let loaded_world = game.world.expect("world.dat present");
        assert_eq!(loaded_world.palette, world.palette);
        assert_eq!(loaded_world.edits, world.edits);
        let loaded_player = game.player.expect("player.dat present");
        assert_eq!(loaded_player.slots, player.slots);
        assert_eq!(loaded_player.selected_slot, 4);
        assert_eq!(game.players.0.get(&77).unwrap().slots, player.slots);
        assert_eq!(game.mobs, mobs, "mob population round-trips");
        assert_eq!(game.progression, progression, "progression round-trips");
        assert_eq!(game.discovery, discovery, "discovery round-trips");

        // A save without mobs.dat (pre-mobs world) still loads, empty.
        fs::remove_file(root.join("test-world").join(MOBS_FILE)).unwrap();
        let game = WorldSave::open(&root, "test-world")
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(game.mobs, MobsData::default(), "missing mobs.dat = empty");

        // Likewise progression: missing means nothing achieved yet.
        fs::remove_file(root.join("test-world").join(PROGRESSION_FILE)).unwrap();
        let game = WorldSave::open(&root, "test-world")
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(game.progression, WorldProgression::default());

        // And discovery: a world from before crafting existed loads with
        // nobody knowing anything yet.
        fs::remove_file(root.join("test-world").join(DISCOVERY_FILE)).unwrap();
        let game = WorldSave::open(&root, "test-world")
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(game.discovery, DiscoveryData::default());

        // No temp files left behind by the atomic writes.
        let leftovers: Vec<_> = fs::read_dir(root.join("test-world"))
            .unwrap()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|ext| ext == "tmp"))
            .collect();
        assert!(leftovers.is_empty());

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn listing_and_deleting_worlds() {
        let root = temp_root("list");
        assert!(list_worlds(&root).is_empty(), "missing root lists empty");

        WorldSave::create(&root, "Alpha", 1, GameMode::Survival).unwrap();
        WorldSave::create(&root, "Beta", 2, GameMode::Creative).unwrap();
        // A stray non-world directory is skipped.
        fs::create_dir_all(root.join("not-a-world")).unwrap();

        let worlds = list_worlds(&root);
        assert_eq!(worlds.len(), 2);

        delete_world(&root, "alpha").unwrap();
        let worlds = list_worlds(&root);
        assert_eq!(worlds.len(), 1);
        assert_eq!(worlds[0].meta.name, "Beta");

        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn corrupt_and_mismatched_files_error_cleanly() {
        let root = temp_root("corrupt");
        let save = WorldSave::create(&root, "Broken", 9, GameMode::Survival).unwrap();
        let dir = root.join("broken");

        // Garbage world.dat (valid header, bad payload) is a hard load error.
        let mut bytes = SAVE_VERSION.to_le_bytes().to_vec();
        bytes.extend(b"garbage");
        fs::write(dir.join(WORLD_FILE), &bytes).unwrap();
        assert!(save.load().is_err());

        // A future version header is rejected before decoding.
        let save = WorldSave::open(&root, "broken").unwrap();
        fs::write(dir.join(WORLD_FILE), 999u32.to_le_bytes()).unwrap();
        assert!(matches!(save.load(), Err(SaveError::Version(999))));

        // Corrupt level.toml: open() errors, list skips it.
        fs::write(dir.join(LEVEL_FILE), "not really toml [").unwrap();
        assert!(WorldSave::open(&root, "broken").is_err());
        assert!(list_worlds(&root).is_empty());

        let _ = fs::remove_dir_all(&root);
    }
}

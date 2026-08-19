//! World persistence: save directories, on-disk formats, and the profile file
//! carrying the client's stable multiplayer identity.
//!
//! A world saves as `saves/<slug>/` containing:
//! - `level.toml` — human-readable metadata (name, seed, mode, spawn, time).
//! - `world.dat` — the block-edit overlay (name-based palette), bincode.
//! - `player.dat` — the save owner's player + inventory, bincode.
//! - `players.dat` — per-identity records for multiplayer clients, bincode.
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
pub mod repository;

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::core::GameMode;
use crate::core::day_cycle::DEFAULT_START;

pub use data::{ItemStackData, MobData, MobsData, PlayerData, PlayerRecords, WorldData};
pub use repository::{
    FileWorldRepository, InMemoryWorldRepository, NullWorldRepository, SaveLog, WorldRepository,
    WorldSnapshot,
};

/// On-disk format version, stamped into `level.toml` and every `.dat` header.
pub const SAVE_VERSION: u32 = 1;
/// Directory holding all world saves, relative to the working directory
/// (matching the `assets/` convention).
pub const SAVES_DIR: &str = "saves";

const LEVEL_FILE: &str = "level.toml";
const WORLD_FILE: &str = "world.dat";
const PLAYER_FILE: &str = "player.dat";
const PLAYERS_FILE: &str = "players.dat";
const MOBS_FILE: &str = "mobs.dat";
/// Local player profile (stable multiplayer identity), next to `saves/`.
const PROFILE_FILE: &str = "profile.toml";

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
        Ok(SavedGame {
            save: self,
            world,
            player,
            players,
            mobs,
        })
    }

    /// Persist the world: metadata + edits + the local player + remote-player
    /// records + the mob population. Bumps `last_played`.
    pub fn write(
        &mut self,
        world: &WorldData,
        player: &PlayerData,
        players: &PlayerRecords,
        mobs: &MobsData,
    ) -> Result<(), SaveError> {
        self.meta.last_played_unix = unix_now();
        self.write_level()?;
        write_dat(&self.dir.join(WORLD_FILE), world)?;
        write_dat(&self.dir.join(PLAYER_FILE), player)?;
        write_dat(&self.dir.join(PLAYERS_FILE), players)?;
        write_dat(&self.dir.join(MOBS_FILE), mobs)?;
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

/// The saves root, relative to the working directory (like `assets/`).
pub fn saves_root() -> PathBuf {
    PathBuf::from(SAVES_DIR)
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
    } else {
        slug
    }
}

/// A time-derived seed (shared by every "random world" path).
pub fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

/// Interpret seed text from the UI: blank → random, a number (decimal or `0x`
/// hex) → itself, anything else → a stable hash of the string.
pub fn parse_seed(text: &str) -> u64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return random_seed();
    }
    if let Ok(n) = trimmed.parse::<u64>() {
        return n;
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        && let Ok(n) = u64::from_str_radix(hex, 16)
    {
        return n;
    }
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    trimmed.hash(&mut hasher);
    hasher.finish()
}

#[derive(Serialize, Deserialize)]
struct ProfileToml {
    /// Decimal string for the same u64-in-TOML reason as the world seed.
    client_id: String,
    /// The signed-in account, when there is one.
    ///
    /// `#[serde(default)]` so a `profile.toml` written before accounts existed
    /// still parses — it simply has no account and the player logs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<AccountProfile>,
}

/// A signed-in session as it survives a restart, so a player is not asked for
/// their password every launch.
///
/// The refresh token is the sensitive part. It lives here for the same reason
/// every game launcher keeps one: the alternative is retyping a password on
/// every start. `profile.toml` is gitignored, and the token is single-use and
/// revocable — a stolen one stops working the moment the real client refreshes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProfile {
    /// Account uuid, as text.
    pub account_id: String,
    pub username: String,
    pub refresh_token: String,
}

/// The account this client last signed in as, if any.
pub fn stored_account() -> Option<AccountProfile> {
    let text = fs::read_to_string(PROFILE_FILE).ok()?;
    toml::from_str::<ProfileToml>(&text).ok()?.account
}

/// Remember (or forget, with `None`) the signed-in account.
///
/// Read-modify-write so the `client_id` already in the file is preserved: it is
/// the offline fallback identity, and regenerating it would orphan any
/// singleplayer save made before signing in.
pub fn store_account(account: Option<AccountProfile>) -> Result<(), String> {
    let path = PathBuf::from(PROFILE_FILE);
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str::<ProfileToml>(&text).ok());

    let profile = ProfileToml {
        client_id: existing
            .map(|profile| profile.client_id)
            .unwrap_or_else(|| local_identity().to_string()),
        account,
    };

    let text = toml::to_string_pretty(&profile)
        .map_err(|err| format!("could not serialize profile: {err}"))?;
    write_atomic(&path, text.as_bytes())
        .map_err(|err| format!("could not write {PROFILE_FILE}: {err}"))
}

/// The machine-local identity, used only when nobody is signed in.
///
/// This is what `client_identity` always was: a random `u64` minted on first
/// launch. With accounts it is no longer how a *multiplayer* peer is
/// identified — that comes from the verified ticket — but singleplayer saves
/// still need some stable key, and an offline player has nothing better.
pub fn local_identity() -> u64 {
    client_identity()
}

/// The stable multiplayer identity this machine connects with (the netcode
/// client id). Persisted in `profile.toml` on first use so a host can recognise
/// a returning player and hand back their saved inventory/position.
/// `WYVEN_CLIENT_ID` overrides it (e.g. to run two clients from one directory).
///
/// Prefer [`crate::auth::AccountState::netcode_id`] where an account may be
/// signed in: it derives the id from the account, so a save follows the player
/// rather than the machine.
pub fn client_identity() -> u64 {
    if let Ok(v) = std::env::var("WYVEN_CLIENT_ID")
        && let Ok(id) = v.trim().parse::<u64>()
    {
        return id;
    }
    let path = PathBuf::from(PROFILE_FILE);
    if let Ok(text) = fs::read_to_string(&path)
        && let Ok(profile) = toml::from_str::<ProfileToml>(&text)
        && let Ok(id) = profile.client_id.trim().parse::<u64>()
    {
        return id;
    }
    let id = (random_seed() ^ (u64::from(std::process::id())).rotate_left(32)).max(1);
    let profile = ProfileToml {
        client_id: id.to_string(),
        account: None,
    };
    match toml::to_string_pretty(&profile) {
        Ok(text) => {
            if let Err(err) = write_atomic(&path, text.as_bytes()) {
                log::warn!("could not persist profile.toml: {err}");
            } else {
                log::info!("created player profile (client id {id})");
            }
        }
        Err(err) => log::warn!("could not serialize profile: {err}"),
    }
    id
}

/// Write via a temp file in the same directory + rename, so an interrupted save
/// never leaves a half-written file behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

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
    fn parse_seed_accepts_numbers_hex_and_strings() {
        assert_eq!(parse_seed("42"), 42);
        assert_eq!(parse_seed(" 0xFF "), 255);
        // A u64 beyond i64::MAX must parse (the reason the seed is a string in TOML).
        assert_eq!(parse_seed("18446744073709551615"), u64::MAX);
        // Text seeds hash deterministically.
        assert_eq!(parse_seed("wyvern"), parse_seed("wyvern"));
        assert_ne!(parse_seed("wyvern"), parse_seed("dragon"));
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
            edits: vec![(crate::core::BlockPos::new(1, 70, -3), 0)],
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
                name: "stone".into(),
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
        save.write(&world, &player, &players, &mobs).unwrap();

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

        // A save without mobs.dat (pre-mobs world) still loads, empty.
        fs::remove_file(root.join("test-world").join(MOBS_FILE)).unwrap();
        let game = WorldSave::open(&root, "test-world")
            .unwrap()
            .load()
            .unwrap();
        assert_eq!(game.mobs, MobsData::default(), "missing mobs.dat = empty");

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

//! Where the game keeps the files it writes.
//!
//! Two roots, deliberately separate:
//!
//! * **The install directory** is the working directory, and holds `assets/`.
//!   It is read-only in spirit — a launcher replaces the whole of it to apply an
//!   update, so nothing that must survive belongs there.
//! * **The data directory** is everything the player accumulates: `saves/`,
//!   `profile.toml`, `ops.toml`, `authkeys.toml`. It lives outside the install
//!   so an update cannot take a world with it.
//!
//! Only the second is resolved here. `assets/` stays working-directory relative
//! (see [`crate::content`]) precisely because it belongs to the install.
//!
//! Resolution order, decided once per process:
//!
//! 1. `WYVEN_DATA_DIR`, when set to something non-empty. This is what a launcher
//!    sets, and what lets two clients on one machine keep separate state.
//! 2. `<OS application data>/Wyvencraft/data` — so
//!    `~/Library/Application Support/Wyvencraft/data`,
//!    `%APPDATA%\Wyvencraft\data`, `~/.local/share/Wyvencraft/data`. The
//!    `data` leaf is not decoration: the launcher keeps installed builds and
//!    logs as siblings of it, and starting the game by hand must land in the
//!    same place the launcher points it at, or a player would have two
//!    unrelated sets of worlds depending on how they started the game.
//! 3. The working directory, if the OS will not name one. Never a hard failure:
//!    a game that cannot find a home directory should still run.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// Directory name under the OS application-data root.
const APP_DIR: &str = "Wyvencraft";

/// Subdirectory of [`APP_DIR`] holding player state.
///
/// Kept in step with the launcher's `internal/paths`, which puts `versions/`
/// and `logs/` beside it.
const DATA_SUBDIR: &str = "data";

/// Overrides the resolved data directory.
pub const DATA_DIR_VAR: &str = "WYVEN_DATA_DIR";

/// Directory holding all world saves.
pub const SAVES_DIR: &str = "saves";
/// Local player profile: the client id, and the signed-in account if any.
pub const PROFILE_FILE: &str = "profile.toml";
/// Command authorization list, keyed by account uuid.
pub const OPS_FILE: &str = "ops.toml";
/// Cached auth-server public keys, used to verify join tickets.
pub const KEYS_FILE: &str = "authkeys.toml";

static DATA_DIR: OnceLock<PathBuf> = OnceLock::new();

/// The directory holding everything the player accumulates.
///
/// Resolved once and then memoised, so the answer cannot change midway through
/// a session — a save written to one place and read from another is the kind of
/// bug that looks like data loss.
pub fn data_dir() -> &'static Path {
    DATA_DIR.get_or_init(resolve_data_dir)
}

/// The saves root. Worlds live in `<data>/saves/<slug>/`.
pub fn saves_root() -> PathBuf {
    data_dir().join(SAVES_DIR)
}

pub fn profile_path() -> PathBuf {
    data_dir().join(PROFILE_FILE)
}

pub fn ops_path() -> PathBuf {
    data_dir().join(OPS_FILE)
}

pub fn keys_path() -> PathBuf {
    data_dir().join(KEYS_FILE)
}

fn resolve_data_dir() -> PathBuf {
    let (root, source) = match std::env::var(DATA_DIR_VAR) {
        Ok(value) if !value.trim().is_empty() => (PathBuf::from(value), DATA_DIR_VAR),
        _ => match app_data_root() {
            Some(root) => (
                root.join(APP_DIR).join(DATA_SUBDIR),
                "the OS application-data directory",
            ),
            // No home directory to speak of — a CI runner, a locked-down
            // service account. The working directory is what the game used
            // before this module existed, so falling back to it is the least
            // surprising thing that still works.
            None => (
                PathBuf::from("."),
                "the working directory (no app-data dir)",
            ),
        },
    };

    // Created eagerly. Every writer below assumes the root exists, and finding
    // out otherwise at the moment a world is saved is far too late.
    if let Err(err) = std::fs::create_dir_all(&root) {
        log::error!(
            "could not create {} ({err}); saves may fail",
            root.display()
        );
    }

    log::info!("game data in {} (from {source})", root.display());
    root
}

/// The OS application-data root, without the `Wyvencraft` suffix.
fn app_data_root() -> Option<PathBuf> {
    dirs::data_dir()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The four runtime files are siblings under one root. A launcher relies on
    /// this: it points `WYVEN_DATA_DIR` at one directory and expects to find
    /// (and write) all of them there.
    #[test]
    fn every_runtime_file_sits_directly_under_the_data_root() {
        let root = data_dir();

        for path in [profile_path(), ops_path(), keys_path(), saves_root()] {
            assert_eq!(
                path.parent(),
                Some(root),
                "{} escaped the root",
                path.display()
            );
        }
    }

    #[test]
    fn the_runtime_files_keep_the_names_the_launcher_contract_names() {
        assert_eq!(profile_path().file_name().unwrap(), PROFILE_FILE);
        assert_eq!(ops_path().file_name().unwrap(), OPS_FILE);
        assert_eq!(keys_path().file_name().unwrap(), KEYS_FILE);
        assert_eq!(saves_root().file_name().unwrap(), SAVES_DIR);
    }

    /// The launcher puts `versions/` and `logs/` beside the data directory and
    /// passes it as `WYVEN_DATA_DIR`. If the default resolved anywhere else, a
    /// player would get one set of worlds through the launcher and another when
    /// starting the game by hand.
    #[test]
    fn the_default_data_root_is_the_one_the_launcher_points_at() {
        let Some(app_data) = app_data_root() else {
            return; // no home directory on this machine; the CWD fallback applies
        };
        let expected = app_data.join(APP_DIR).join(DATA_SUBDIR);

        assert_eq!(expected.file_name().unwrap(), "data");
        assert_eq!(
            expected.parent().unwrap().file_name().unwrap(),
            "Wyvencraft"
        );
    }

    /// `assets/` is deliberately *not* here: it belongs to the install, which a
    /// launcher replaces wholesale on update.
    #[test]
    fn the_data_root_is_absolute_unless_it_fell_back_to_the_working_directory() {
        let root = data_dir();
        assert!(root.is_absolute() || root == Path::new("."));
    }
}

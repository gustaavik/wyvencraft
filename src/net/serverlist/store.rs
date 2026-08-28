//! Where the saved server list lives.
//!
//! A port, for the reason every other I/O port here exists: the browser's whole
//! job is "read a list, mutate it, write it back", and that is only testable if
//! the writing end can be something other than the player's real
//! `servers.toml`. Three impls, all shipped — the double is not `#[cfg(test)]`
//! because the browser *owns* its store, so a test has to be able to hand one
//! over and still read it back afterwards.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use super::{SERVERS_FILE, ServerList};

/// A place the saved server list is kept.
pub trait ServerStore {
    /// The saved list. Fail-soft: a missing or unreadable file is an empty list,
    /// never an error — a player with a broken `servers.toml` should still get
    /// a working browser.
    fn load(&self) -> ServerList;

    /// Persist the list. The `Err` string is shown in the menu.
    fn save(&mut self, list: &ServerList) -> Result<(), String>;
}

/// `servers.toml` in the data directory — the real game's storage.
pub struct FileServerStore {
    path: PathBuf,
}

impl FileServerStore {
    /// At the data directory's `servers.toml`.
    pub fn new() -> Self {
        Self::at(crate::paths::servers_path())
    }

    /// At an explicit path, so tests need no data directory.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Default for FileServerStore {
    fn default() -> Self {
        Self::new()
    }
}

impl ServerStore for FileServerStore {
    fn load(&self) -> ServerList {
        match std::fs::read_to_string(&self.path) {
            Ok(text) => ServerList::from_toml(&text),
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("no {SERVERS_FILE}; the server list starts empty");
                ServerList::default()
            }
            Err(err) => {
                log::warn!("could not read {SERVERS_FILE}: {err}; the server list starts empty");
                ServerList::default()
            }
        }
    }

    fn save(&mut self, list: &ServerList) -> Result<(), String> {
        let text = list.to_toml()?;
        // The same temp-file-and-rename every other settings file here uses: an
        // interrupted write must not leave a player with half a server list.
        crate::save::write_atomic(&self.path, text.as_bytes())
            .map_err(|err| format!("could not write {SERVERS_FILE}: {err}"))
    }
}

/// Keeps the list in memory, and lets a test read it back after the browser has
/// taken ownership — the same handle trick as `InMemoryWorldRepository`.
#[derive(Default)]
pub struct InMemoryServerStore {
    state: SavedServers,
}

/// Shared view of what an [`InMemoryServerStore`] holds.
pub type SavedServers = Arc<Mutex<StoredServers>>;

#[derive(Default)]
pub struct StoredServers {
    pub list: ServerList,
    /// How many times [`ServerStore::save`] has been called.
    pub writes: usize,
    /// When set, every `save` fails with this message.
    pub fail_with: Option<String>,
}

impl InMemoryServerStore {
    /// Pre-loaded with `list`.
    pub fn with(list: ServerList) -> Self {
        let store = Self::default();
        store.state.lock().expect("server store poisoned").list = list;
        store
    }

    /// A handle to what this store holds, usable after the store itself has
    /// been handed to the browser.
    pub fn handle(&self) -> SavedServers {
        Arc::clone(&self.state)
    }
}

impl ServerStore for InMemoryServerStore {
    fn load(&self) -> ServerList {
        self.state
            .lock()
            .expect("server store poisoned")
            .list
            .clone()
    }

    fn save(&mut self, list: &ServerList) -> Result<(), String> {
        let mut state = self.state.lock().expect("server store poisoned");
        state.writes += 1;
        if let Some(reason) = &state.fail_with {
            return Err(reason.clone());
        }
        state.list = list.clone();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::serverlist::ServerEntry;

    fn temp_path(tag: &str) -> PathBuf {
        std::env::temp_dir().join(format!(
            "wyven-servers-{tag}-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ))
    }

    #[test]
    fn a_saved_list_reads_back_unchanged() {
        let path = temp_path("roundtrip");
        let _ = std::fs::remove_file(&path);
        let mut store = FileServerStore::at(&path);

        let list = ServerList::new(vec![
            ServerEntry::parse("My Server", "play.example.com:6091").unwrap(),
        ]);
        store.save(&list).expect("writes");

        assert_eq!(store.load(), list);
        let _ = std::fs::remove_file(&path);
    }

    /// The common case on a fresh install, and the case after a player deletes
    /// the file by hand. Neither is an error.
    #[test]
    fn a_missing_file_loads_as_an_empty_list() {
        let store = FileServerStore::at(temp_path("absent"));
        assert!(store.load().is_empty());
    }

    #[test]
    fn the_double_records_what_it_was_given() {
        let store = InMemoryServerStore::default();
        let handle = store.handle();
        let mut store: Box<dyn ServerStore> = Box::new(store);

        let list = ServerList::new(vec![ServerEntry::parse("a", "a.example.com").unwrap()]);
        store.save(&list).expect("writes");

        let state = handle.lock().unwrap();
        assert_eq!(state.writes, 1);
        assert_eq!(state.list, list);
    }
}

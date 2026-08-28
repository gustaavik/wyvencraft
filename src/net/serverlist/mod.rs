//! The servers a player has saved, as shown in the multiplayer browser.
//!
//! Pure data and pure policy: an entry is a name and an address *as typed*, not
//! a resolved [`SocketAddr`](std::net::SocketAddr). Keeping the text is what
//! lets a saved server follow a hostname whose IP changes, and it keeps DNS —
//! which blocks — off the frame that draws the list. Resolution happens on the
//! worker that connects (see [`crate::net::status`] and
//! [`crate::state::connecting_state`]).
//!
//! Persistence is [`store`]; nothing here touches a filesystem.

pub mod store;

use serde::{Deserialize, Serialize};

pub use store::{FileServerStore, InMemoryServerStore, SavedServers, ServerStore};

/// The file this list is kept in, for log messages.
pub use crate::paths::SERVERS_FILE;

/// Longest name and address a saved entry may carry.
///
/// Not a storage limit — a guard on what one hand-edited line can do to the
/// layout of every other row.
const MAX_NAME_LEN: usize = 40;
const MAX_ADDRESS_LEN: usize = 128;

/// One saved server.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ServerEntry {
    /// What the player called it. Theirs, not the host's — the host's own world
    /// name arrives with the status reply and is shown beside this.
    pub name: String,
    /// `host` or `host:port`, exactly as typed.
    pub address: String,
}

impl ServerEntry {
    /// Build an entry from raw form input, or say why it can't be one.
    ///
    /// The only validation that belongs here is what a *list* needs: a name to
    /// show and an address to try. Whether the address resolves is not knowable
    /// without blocking, and whether the server answers is not knowable at all
    /// until it is asked — both are the probe's business, not the form's.
    pub fn parse(name: &str, address: &str) -> Result<Self, String> {
        let name = name.trim();
        let address = address.trim();
        if address.is_empty() {
            return Err("Enter a server address".to_string());
        }
        if address.len() > MAX_ADDRESS_LEN {
            return Err("That address is too long".to_string());
        }
        if address.split_whitespace().count() > 1 {
            return Err("An address cannot contain spaces".to_string());
        }
        // Falls back to the address so a row is never blank; a player who leaves
        // the name empty plainly means "call it what I typed".
        let name = if name.is_empty() { address } else { name };
        if name.len() > MAX_NAME_LEN {
            return Err("That name is too long".to_string());
        }
        Ok(Self {
            name: name.to_string(),
            address: address.to_string(),
        })
    }
}

/// The saved servers, in the order the player put them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServerList {
    entries: Vec<ServerEntry>,
}

impl ServerList {
    pub fn new(entries: Vec<ServerEntry>) -> Self {
        Self { entries }
    }

    /// Parse a `servers.toml`. Never fails: an entry with no address is skipped
    /// with a warning, and a file that is not TOML at all yields an empty list.
    /// Losing one hand-edited line must not lose the other nine.
    pub fn from_toml(text: &str) -> Self {
        let file: ServersToml = match toml::from_str(text) {
            Ok(file) => file,
            Err(err) => {
                log::warn!("{SERVERS_FILE} is malformed ({err}); starting with no saved servers");
                return Self::default();
            }
        };
        let entries = file
            .server
            .into_iter()
            .filter_map(
                |entry| match ServerEntry::parse(&entry.name, &entry.address) {
                    Ok(entry) => Some(entry),
                    Err(reason) => {
                        log::warn!("{SERVERS_FILE}: skipping '{}' — {reason}", entry.name);
                        None
                    }
                },
            )
            .collect();
        Self { entries }
    }

    pub fn to_toml(&self) -> Result<String, String> {
        let file = ServersToml {
            server: self.entries.clone(),
        };
        toml::to_string_pretty(&file).map_err(|err| format!("could not serialize servers: {err}"))
    }

    pub fn add(&mut self, entry: ServerEntry) {
        self.entries.push(entry);
    }

    /// Overwrite the entry at `index`, if there is one.
    pub fn replace(&mut self, index: usize, entry: ServerEntry) -> bool {
        match self.entries.get_mut(index) {
            Some(slot) => {
                *slot = entry;
                true
            }
            None => false,
        }
    }

    /// Remove the entry at `index`, returning it.
    pub fn remove(&mut self, index: usize) -> Option<ServerEntry> {
        (index < self.entries.len()).then(|| self.entries.remove(index))
    }

    pub fn get(&self, index: usize) -> Option<&ServerEntry> {
        self.entries.get(index)
    }

    pub fn iter(&self) -> impl Iterator<Item = &ServerEntry> {
        self.entries.iter()
    }

    /// Every address in the list, in order — what the probe is handed.
    pub fn addresses(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|entry| entry.address.clone())
            .collect()
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

/// The on-disk shape: `[[server]]` tables, like `[[block]]` in the content files.
#[derive(Serialize, Deserialize, Default)]
struct ServersToml {
    #[serde(default)]
    server: Vec<ServerEntry>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(name: &str, address: &str) -> ServerEntry {
        ServerEntry::parse(name, address).expect("valid entry")
    }

    #[test]
    fn a_list_survives_a_round_trip_through_toml() {
        let list = ServerList::new(vec![
            entry("My Server", "play.example.com:25565"),
            entry("LAN box", "192.168.1.20:25565"),
        ]);

        let text = list.to_toml().expect("serializes");
        assert_eq!(ServerList::from_toml(&text), list);
    }

    #[test]
    fn a_missing_file_reads_as_an_empty_list() {
        assert!(ServerList::from_toml("").is_empty());
    }

    /// One hand-edited line with no address must not cost the player the rest
    /// of their servers — the same fail-soft-per-entry rule `ops.toml` uses.
    #[test]
    fn one_bad_entry_is_skipped_and_the_others_survive() {
        let list = ServerList::from_toml(
            r#"
            [[server]]
            name = "Good"
            address = "example.com:25565"

            [[server]]
            name = "Broken"
            address = ""

            [[server]]
            name = "Also good"
            address = "10.0.0.4"
            "#,
        );

        assert_eq!(list.len(), 2);
        assert_eq!(list.get(0).unwrap().name, "Good");
        assert_eq!(list.get(1).unwrap().name, "Also good");
    }

    /// TOML that isn't a server list at all is a warning and an empty browser,
    /// never a panic on the way to the menu.
    #[test]
    fn a_file_that_is_not_a_server_list_reads_as_empty() {
        assert!(ServerList::from_toml("this is not = = toml").is_empty());
    }

    #[test]
    fn an_unnamed_entry_is_called_by_its_address() {
        let entry = ServerEntry::parse("   ", " play.example.com ").expect("valid");
        assert_eq!(entry.name, "play.example.com");
        assert_eq!(entry.address, "play.example.com");
    }

    #[test]
    fn an_entry_needs_an_address() {
        assert!(ServerEntry::parse("Nameless", "  ").is_err());
        assert!(ServerEntry::parse("Spaced", "one two").is_err());
    }

    #[test]
    fn removing_shifts_the_entries_after_it() {
        let mut list = ServerList::new(vec![
            entry("a", "a.example.com"),
            entry("b", "b.example.com"),
            entry("c", "c.example.com"),
        ]);

        assert_eq!(list.remove(1).unwrap().name, "b");
        assert_eq!(list.addresses(), ["a.example.com", "c.example.com"]);
        assert!(list.remove(9).is_none());
    }

    #[test]
    fn replacing_keeps_the_entrys_place() {
        let mut list = ServerList::new(vec![
            entry("a", "a.example.com"),
            entry("b", "b.example.com"),
        ]);

        assert!(list.replace(0, entry("renamed", "z.example.com")));
        assert!(!list.replace(7, entry("nowhere", "n.example.com")));
        assert_eq!(list.get(0).unwrap().name, "renamed");
        assert_eq!(list.len(), 2);
    }
}

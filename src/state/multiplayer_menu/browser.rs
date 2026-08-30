//! The server browser's state and rules, with no egui anywhere in it.
//!
//! Separated from the screen so the interesting half — what selection survives a
//! delete, when the list is written back, which rows may be joined — can be
//! tested against an [`InMemoryServerStore`](crate::net::serverlist::InMemoryServerStore)
//! and a [`FakeStatusProbe`](crate::net::status::FakeStatusProbe), with no
//! window, socket or auth server anywhere near it.

use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crate::net::serverlist::{ServerEntry, ServerList, ServerStore};
use crate::net::status::{StatusOutcome, StatusProbe};

/// What one row of the list currently knows about itself.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RowState<'a> {
    /// Nobody has asked yet.
    Unknown,
    /// Being asked right now.
    Querying,
    Online {
        /// The world's own name, as the host reports it.
        world: &'a str,
        online: u32,
        max: u32,
        ping_ms: u32,
        /// Whether this host's content matches ours. A mismatch is refused at
        /// the `Welcome`, so there is no point letting anyone travel to it.
        compatible: bool,
    },
    Offline(&'a str),
}

/// One row, as the view needs it.
#[derive(Debug, Clone, Copy)]
pub struct ServerRow<'a> {
    /// What the player filed it under.
    pub name: &'a str,
    pub address: &'a str,
    pub state: RowState<'a>,
    /// Whether Join would do anything for this row.
    pub joinable: bool,
}

/// The saved servers, what is known about them, and which one is selected.
pub struct ServerBrowser {
    list: ServerList,
    store: Box<dyn ServerStore>,
    probe: Box<dyn StatusProbe>,
    /// Last answer per address. Kept across refreshes so a row does not blank
    /// out while it is being asked again.
    status: HashMap<String, StatusOutcome>,
    /// Addresses with a query outstanding.
    querying: HashSet<String>,
    /// Our own content fingerprint, to compare a host's against.
    content_hash: u64,
    selected: Option<usize>,
    /// Index awaiting the second (confirming) Delete click.
    confirm_delete: Option<usize>,
    error: Option<String>,
}

impl ServerBrowser {
    /// Load the saved list and ask every server on it what it is.
    pub fn new(
        store: Box<dyn ServerStore>,
        probe: Box<dyn StatusProbe>,
        content_hash: u64,
    ) -> Self {
        let list = store.load();
        let mut browser = Self {
            list,
            store,
            probe,
            status: HashMap::new(),
            querying: HashSet::new(),
            content_hash,
            // Pre-selecting the first row is what makes Join and double-click
            // agree on an empty-handed arrival: with nothing selected, the
            // button a player reaches for first does nothing.
            selected: None,
            confirm_delete: None,
            error: None,
        };
        browser.selected = (!browser.list.is_empty()).then_some(0);
        browser.refresh();
        browser
    }

    /// Ask every saved server for its status again.
    ///
    /// All of them, not just the one that changed: a probe presents one ticket
    /// for the whole sweep, so asking ten costs barely more than asking one, and
    /// a list where every row was measured at the same moment is the one worth
    /// comparing pings across.
    pub fn refresh(&mut self) {
        let addresses = self.list.addresses();
        self.querying = addresses.iter().cloned().collect();
        self.probe.begin(addresses);
    }

    /// Collect whatever the probe has resolved. Call once a frame.
    pub fn tick(&mut self, dt: f32) {
        for (address, outcome) in self.probe.poll(Duration::from_secs_f32(dt.max(1.0e-4))) {
            self.querying.remove(&address);
            self.status.insert(address, outcome);
        }
    }

    /// Whether a refresh is still running.
    pub fn is_refreshing(&self) -> bool {
        !self.querying.is_empty() || self.probe.is_busy()
    }

    pub fn is_empty(&self) -> bool {
        self.list.is_empty()
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn error(&self) -> Option<&str> {
        self.error.as_deref()
    }

    /// Whether `index` is the row asking to be confirmed before it is deleted.
    pub fn is_confirming_delete(&self, index: usize) -> bool {
        self.confirm_delete == Some(index)
    }

    pub fn select(&mut self, index: usize) {
        if index < self.list.len() {
            self.selected = Some(index);
            // A delete confirmation belongs to the row it was started on; moving
            // away from that row is a clear enough "no".
            self.confirm_delete = None;
        }
    }

    /// The entry at `index`, for filling in an edit form.
    pub fn entry(&self, index: usize) -> Option<&ServerEntry> {
        self.list.get(index)
    }

    /// Every row, in list order.
    pub fn rows(&self) -> Vec<ServerRow<'_>> {
        self.list
            .iter()
            .map(|entry| {
                let state = self.state_of(&entry.address);
                ServerRow {
                    name: &entry.name,
                    address: &entry.address,
                    state,
                    joinable: !matches!(
                        state,
                        RowState::Online {
                            compatible: false,
                            ..
                        }
                    ),
                }
            })
            .collect()
    }

    fn state_of(&self, address: &str) -> RowState<'_> {
        if self.querying.contains(address) {
            return RowState::Querying;
        }
        match self.status.get(address) {
            Some(StatusOutcome::Online(status)) => RowState::Online {
                world: &status.name,
                online: status.online,
                max: status.max,
                ping_ms: status.ping_ms,
                compatible: status.content_hash == self.content_hash,
            },
            Some(StatusOutcome::Offline(reason)) => RowState::Offline(reason),
            None => RowState::Unknown,
        }
    }

    /// Save a new server, select it, and ask what it is.
    pub fn add(&mut self, name: &str, address: &str) -> Result<(), String> {
        let entry = ServerEntry::parse(name, address)?;
        self.list.add(entry);
        self.selected = Some(self.list.len() - 1);
        self.persist();
        self.refresh();
        Ok(())
    }

    /// Overwrite the entry at `index`.
    pub fn update(&mut self, index: usize, name: &str, address: &str) -> Result<(), String> {
        let entry = ServerEntry::parse(name, address)?;
        if !self.list.replace(index, entry) {
            return Err("That server is no longer in the list".to_string());
        }
        self.persist();
        self.refresh();
        Ok(())
    }

    /// Delete the entry at `index` — but only on the second call for that row,
    /// so a stray click cannot lose a server address the player may not have
    /// written down anywhere else. Returns whether it actually deleted.
    pub fn remove(&mut self, index: usize) -> bool {
        if self.confirm_delete != Some(index) {
            self.confirm_delete = Some(index);
            return false;
        }
        self.confirm_delete = None;
        if self.list.remove(index).is_none() {
            return false;
        }
        // Follow the deletion: the row that took its place, or the new last row
        // when the list just lost its tail.
        self.selected = match self.list.len() {
            0 => None,
            len => Some(index.min(len - 1)),
        };
        self.persist();
        true
    }

    /// The address to connect to for `index`, if it is worth trying.
    pub fn target(&mut self, index: usize) -> Option<String> {
        let entry = self.list.get(index)?;
        if let RowState::Online {
            compatible: false, ..
        } = self.state_of(&entry.address)
        {
            self.error =
                Some("That server runs different content — you would be turned away.".to_string());
            return None;
        }
        self.error = None;
        Some(entry.address.clone())
    }

    fn persist(&mut self) {
        match self.store.save(&self.list) {
            Ok(()) => self.error = None,
            Err(err) => self.error = Some(err),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::serverlist::{InMemoryServerStore, SavedServers};
    use crate::net::status::{FakeStatusProbe, ServerStatus};

    /// Content hash both sides agree on, so rows are compatible unless a test
    /// deliberately says otherwise.
    const OURS: u64 = 0xABC;

    fn list(entries: &[(&str, &str)]) -> ServerList {
        ServerList::new(
            entries
                .iter()
                .map(|(name, address)| ServerEntry::parse(name, address).unwrap())
                .collect(),
        )
    }

    fn browser(saved: &[(&str, &str)], probe: FakeStatusProbe) -> (ServerBrowser, SavedServers) {
        let store = InMemoryServerStore::with(list(saved));
        let handle = store.handle();
        let mut browser = ServerBrowser::new(Box::new(store), Box::new(probe), OURS);
        browser.tick(1.0 / 60.0);
        (browser, handle)
    }

    #[test]
    fn a_saved_list_is_loaded_and_queried_on_open() {
        let probe = FakeStatusProbe::new()
            .serving(OURS)
            .online("a.example.com", "Cliffs", 3, 17);
        let (browser, _) = browser(&[("A", "a.example.com")], probe);

        let rows = browser.rows();
        assert_eq!(rows.len(), 1);
        assert_eq!(rows[0].name, "A");
        assert_eq!(
            rows[0].state,
            RowState::Online {
                world: "Cliffs",
                online: 3,
                max: 17,
                ping_ms: 20,
                compatible: true,
            }
        );
        assert!(rows[0].joinable);
    }

    #[test]
    fn a_server_that_does_not_answer_reads_offline() {
        let probe =
            FakeStatusProbe::new().otherwise(StatusOutcome::Offline("Can't connect".to_string()));
        let (browser, _) = browser(&[("A", "a.example.com")], probe);

        assert_eq!(browser.rows()[0].state, RowState::Offline("Can't connect"));
    }

    /// Until an answer comes back the row has to say something, and "still
    /// asking" is not the same as "offline" — a slow server must not look dead.
    #[test]
    fn a_row_with_no_answer_yet_reads_as_still_being_asked() {
        // A probe with no prepared answer never resolves anything.
        let (browser, _) = browser(&[("A", "a.example.com")], FakeStatusProbe::new());

        assert_eq!(browser.rows()[0].state, RowState::Querying);
        assert!(browser.is_refreshing());
    }

    #[test]
    fn adding_a_server_saves_it_selects_it_and_asks_about_it() {
        let (mut browser, saved) = browser(&[], FakeStatusProbe::new());

        browser
            .add("My Server", " play.example.com ")
            .expect("added");

        assert_eq!(browser.selected(), Some(0));
        assert_eq!(browser.rows()[0].address, "play.example.com");
        let state = saved.lock().unwrap();
        assert_eq!(state.list.len(), 1, "written through to the store");
        assert_eq!(state.writes, 1);
    }

    #[test]
    fn a_server_with_no_address_is_refused_and_nothing_is_written() {
        let (mut browser, saved) = browser(&[], FakeStatusProbe::new());

        assert!(browser.add("Nameless", "   ").is_err());
        assert!(browser.is_empty());
        assert_eq!(saved.lock().unwrap().writes, 0);
    }

    #[test]
    fn editing_keeps_the_entrys_place_in_the_list() {
        let (mut browser, saved) = browser(
            &[("A", "a.example.com"), ("B", "b.example.com")],
            FakeStatusProbe::new(),
        );

        browser
            .update(0, "Renamed", "z.example.com")
            .expect("updated");

        let rows = browser.rows();
        assert_eq!(
            (rows[0].name, rows[0].address),
            ("Renamed", "z.example.com")
        );
        assert_eq!(rows[1].name, "B");
        assert_eq!(saved.lock().unwrap().list.len(), 2);
    }

    /// One click arms it, the second does it. A server address is often the only
    /// copy a player has.
    #[test]
    fn deleting_takes_two_clicks() {
        let (mut browser, saved) = browser(&[("A", "a.example.com")], FakeStatusProbe::new());

        assert!(!browser.remove(0), "the first click only arms it");
        assert!(browser.is_confirming_delete(0));
        assert_eq!(browser.rows().len(), 1);
        assert_eq!(saved.lock().unwrap().writes, 0);

        assert!(browser.remove(0));
        assert!(browser.is_empty());
        assert_eq!(browser.selected(), None);
        assert_eq!(saved.lock().unwrap().writes, 1);
    }

    #[test]
    fn selecting_another_row_calls_off_a_pending_delete() {
        let (mut browser, _) = browser(
            &[("A", "a.example.com"), ("B", "b.example.com")],
            FakeStatusProbe::new(),
        );

        browser.remove(1);
        assert!(browser.is_confirming_delete(1));
        browser.select(0);
        assert!(!browser.is_confirming_delete(1));
    }

    /// Deleting must not leave the selection pointing past the end of the list,
    /// or Join would reach for a row that is not there.
    #[test]
    fn the_selection_survives_deleting_the_last_row() {
        let (mut browser, _) = browser(
            &[("A", "a.example.com"), ("B", "b.example.com")],
            FakeStatusProbe::new(),
        );

        browser.select(1);
        browser.remove(1);
        assert!(browser.remove(1));
        assert_eq!(browser.selected(), Some(0));
        assert_eq!(browser.target(0).as_deref(), Some("a.example.com"));
    }

    /// A host on different content refuses the join at the `Welcome`. Better to
    /// say so on the row than to spend twelve seconds finding out.
    #[test]
    fn a_server_on_different_content_cannot_be_joined() {
        let probe = FakeStatusProbe::new().answering(
            "a.example.com",
            StatusOutcome::Online(Box::new(ServerStatus {
                name: "Elsewhere".to_string(),
                online: 1,
                max: 17,
                ping_ms: 12,
                content_hash: OURS ^ 0xFF,
            })),
        );
        let (mut browser, _) = browser(&[("A", "a.example.com")], probe);

        assert!(!browser.rows()[0].joinable);
        assert_eq!(browser.target(0), None);
        assert!(
            browser
                .error()
                .is_some_and(|e| e.contains("different content"))
        );
    }

    #[test]
    fn a_failing_store_reports_why_and_keeps_the_list_usable() {
        let store = InMemoryServerStore::default();
        let handle = store.handle();
        handle.lock().unwrap().fail_with = Some("disk is full".to_string());
        let mut browser =
            ServerBrowser::new(Box::new(store), Box::new(FakeStatusProbe::new()), OURS);

        browser.add("A", "a.example.com").expect("accepted");

        assert_eq!(browser.error(), Some("disk is full"));
        assert_eq!(browser.rows().len(), 1, "still shown, just not saved");
    }
}

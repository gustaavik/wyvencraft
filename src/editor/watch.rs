//! Noticing that a file changed underneath the running game.
//!
//! This is the "hot reload" half: re-export a model from Blockbench, or change a
//! number in `assets/items.toml` in a text editor, and the item in your hand
//! moves without a restart.
//!
//! It polls modification stamps rather than subscribing to the OS. There is no
//! watcher crate in the workspace and this does not need one: the watched set is
//! the forty-odd files a `[item.model]` names, and four `stat` calls a second
//! over forty paths is not measurable next to a frame that re-bakes a rigged
//! player. Polling also has the property a dev tool wants most — it cannot leak
//! a thread, and it stops when the panel closes.
//!
//! The stamps come through a port so the whole thing tests against a scripted
//! clock: no filesystem, and no test that has to sleep.

use std::collections::HashMap;
use std::time::SystemTime;

/// How often the watched set is polled, in seconds.
pub const DEFAULT_INTERVAL: f32 = 0.25;

/// When a file was last written.
pub trait Stamps {
    fn modified(&self, path: &str) -> Option<SystemTime>;
}

/// The real filesystem.
pub struct FsStamps;

impl Stamps for FsStamps {
    fn modified(&self, path: &str) -> Option<SystemTime> {
        std::fs::metadata(path).ok()?.modified().ok()
    }
}

/// The paths being watched and the stamp each was last seen with.
#[derive(Debug, Default)]
pub struct FileWatcher {
    seen: HashMap<String, Option<SystemTime>>,
    elapsed: f32,
    interval: f32,
}

impl FileWatcher {
    pub fn new(interval: f32) -> Self {
        Self {
            seen: HashMap::new(),
            elapsed: 0.0,
            interval,
        }
    }

    /// How many files are being watched — the panel says so, because a watcher
    /// that quietly found nothing to watch looks exactly like one that works.
    pub fn watching(&self) -> usize {
        self.seen.len()
    }

    pub fn is_watching(&self, path: &str) -> bool {
        self.seen.contains_key(path)
    }

    /// Start watching `path`, or re-baseline one already watched.
    ///
    /// Re-baselining is what a save does. Without it the watcher would see the
    /// game's own write on the next poll and reload the value the developer just
    /// saved, over the top of whatever they had moved on to.
    pub fn watch(&mut self, path: &str, stamps: &dyn Stamps) {
        self.seen.insert(path.to_string(), stamps.modified(path));
    }

    pub fn forget_all(&mut self) {
        self.seen.clear();
        self.elapsed = 0.0;
    }

    /// Advance the poll clock and report the files that changed.
    ///
    /// A file that has *vanished* is remembered as gone but never reported: many
    /// editors save by writing a temporary file and renaming it over the target,
    /// so a poll landing in that window would otherwise report a change and then
    /// fail to read it. The rename's new stamp is caught on the next poll.
    pub fn tick(&mut self, dt: f32, stamps: &dyn Stamps) -> Vec<String> {
        self.elapsed += dt;
        if self.elapsed < self.interval {
            return Vec::new();
        }
        self.elapsed = 0.0;

        let mut changed = Vec::new();
        for (path, seen) in &mut self.seen {
            let now = stamps.modified(path);
            if now != *seen {
                if now.is_some() {
                    changed.push(path.clone());
                }
                *seen = now;
            }
        }
        // A `HashMap` walk has no order, and a caller reporting "3 files
        // reloaded" in a different order each time reads as a bug.
        changed.sort();
        changed
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::time::Duration;

    use super::*;

    /// Stamps the test sets by hand. No filesystem, and nothing to sleep for.
    #[derive(Default)]
    struct FakeStamps(RefCell<HashMap<String, Option<SystemTime>>>);

    impl FakeStamps {
        fn set(&self, path: &str, at: Option<SystemTime>) {
            self.0.borrow_mut().insert(path.to_string(), at);
        }

        fn touch(&self, path: &str, seconds: u64) {
            self.set(
                path,
                Some(SystemTime::UNIX_EPOCH + Duration::from_secs(seconds)),
            );
        }
    }

    impl Stamps for FakeStamps {
        fn modified(&self, path: &str) -> Option<SystemTime> {
            self.0.borrow().get(path).copied().flatten()
        }
    }

    fn watcher(stamps: &FakeStamps, paths: &[&str]) -> FileWatcher {
        let mut watcher = FileWatcher::new(DEFAULT_INTERVAL);
        for path in paths {
            watcher.watch(path, stamps);
        }
        watcher
    }

    #[test]
    fn an_unchanged_file_reports_nothing() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);

        assert_eq!(watcher.tick(1.0, &stamps), Vec::<String>::new());
        assert_eq!(watcher.tick(1.0, &stamps), Vec::<String>::new());
        assert_eq!(watcher.watching(), 1);
    }

    #[test]
    fn a_changed_file_is_reported_exactly_once() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);

        stamps.touch("a.json", 11);
        assert_eq!(watcher.tick(1.0, &stamps), vec!["a.json".to_string()]);
        assert_eq!(
            watcher.tick(1.0, &stamps),
            Vec::<String>::new(),
            "the new stamp is the baseline now"
        );
    }

    /// The poll clock is what keeps this off the frame budget.
    #[test]
    fn nothing_is_polled_before_the_interval_elapses() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);

        stamps.touch("a.json", 11);
        assert_eq!(watcher.tick(0.01, &stamps), Vec::<String>::new());
        assert_eq!(watcher.tick(0.01, &stamps), Vec::<String>::new());
        assert_eq!(
            watcher.tick(DEFAULT_INTERVAL, &stamps),
            vec!["a.json".to_string()]
        );
    }

    /// Half of "save by rename": the target is briefly gone. Reporting that
    /// would send the session off to read a file that is not there yet.
    #[test]
    fn a_vanished_file_is_remembered_but_not_reported() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);

        stamps.set("a.json", None);
        assert_eq!(watcher.tick(1.0, &stamps), Vec::<String>::new());

        stamps.touch("a.json", 12);
        assert_eq!(
            watcher.tick(1.0, &stamps),
            vec!["a.json".to_string()],
            "and the rename is caught on the next poll"
        );
    }

    /// What a save does, so the game does not reload its own write over the top
    /// of whatever the developer moved on to.
    #[test]
    fn re_baselining_swallows_the_games_own_write() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);

        stamps.touch("a.json", 11);
        watcher.watch("a.json", &stamps);
        assert_eq!(watcher.tick(1.0, &stamps), Vec::<String>::new());
    }

    #[test]
    fn several_changes_are_reported_in_a_stable_order() {
        let stamps = FakeStamps::default();
        for path in ["c.json", "a.json", "b.json"] {
            stamps.touch(path, 10);
        }
        let mut watcher = watcher(&stamps, &["c.json", "a.json", "b.json"]);

        for path in ["c.json", "a.json", "b.json"] {
            stamps.touch(path, 11);
        }
        assert_eq!(watcher.tick(1.0, &stamps), ["a.json", "b.json", "c.json"]);
    }

    #[test]
    fn an_unwatched_file_is_never_polled() {
        let stamps = FakeStamps::default();
        stamps.touch("a.json", 10);
        let mut watcher = watcher(&stamps, &["a.json"]);
        assert!(watcher.is_watching("a.json"));
        assert!(!watcher.is_watching("b.json"));

        stamps.touch("b.json", 99);
        assert_eq!(watcher.tick(1.0, &stamps), Vec::<String>::new());

        watcher.forget_all();
        assert_eq!(watcher.watching(), 0);
    }
}

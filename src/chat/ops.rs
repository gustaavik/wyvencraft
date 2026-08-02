//! Who is authorized to run commands.
//!
//! Keyed by the stable *client identity* (`save::client_identity()`) rather than
//! by [`PlayerId`](crate::net::PlayerId): player ids are assigned per session
//! and shift on every reconnect, so they can't carry a permission. The identity
//! is the same u64 the host already uses to hand a returning player their saved
//! inventory back, which makes it the only durable handle on a person.
//!
//! The file is read exactly like `profile.toml` (CWD-relative, fail-soft): an
//! absent or malformed `ops.toml` ops nobody rather than taking the game down.
//! Only the authority ever loads it — a client has nothing to decide.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;

/// Ops file, next to `profile.toml` and `saves/` in the working directory.
pub const OPS_FILE: &str = "ops.toml";

/// The set of authorized identities, with the names the file gave them.
#[derive(Debug, Clone, Default)]
pub struct OpsList {
    /// identity → display name (the identity itself when the file omits one).
    ops: HashMap<u64, String>,
}

impl OpsList {
    /// Read `ops.toml` from the working directory. Never fails: a missing file
    /// is the normal case (nobody but the host is an op) and a broken one is
    /// logged and treated the same way.
    pub fn load() -> Self {
        let text = match fs::read_to_string(OPS_FILE) {
            Ok(text) => text,
            Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
                log::debug!("no {OPS_FILE}; only the host may run commands");
                return Self::default();
            }
            Err(err) => {
                log::warn!("could not read {OPS_FILE}: {err}; only the host may run commands");
                return Self::default();
            }
        };
        match Self::from_toml(&text) {
            Ok(ops) => {
                log::info!(
                    "{OPS_FILE}: {} authorized ({})",
                    ops.len(),
                    ops.names().collect::<Vec<_>>().join(", ")
                );
                ops
            }
            Err(err) => {
                log::warn!("{OPS_FILE} is malformed ({err}); only the host may run commands");
                Self::default()
            }
        }
    }

    /// Parse an ops file. Entries whose `id` isn't a u64 are skipped with a
    /// warning — one typo shouldn't revoke everyone else.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: OpsToml = toml::from_str(text).map_err(|err| err.to_string())?;
        let mut ops = HashMap::new();
        for entry in file.ops {
            match entry.id.trim().parse::<u64>() {
                Ok(id) => {
                    let name = entry.name.unwrap_or_else(|| id.to_string());
                    ops.insert(id, name);
                }
                Err(_) => log::warn!(
                    "{OPS_FILE}: '{}' is not a client id (expected a decimal number in quotes)",
                    entry.id
                ),
            }
        }
        Ok(Self { ops })
    }

    pub fn is_op(&self, identity: u64) -> bool {
        self.ops.contains_key(&identity)
    }

    pub fn len(&self) -> usize {
        self.ops.len()
    }

    pub fn is_empty(&self) -> bool {
        self.ops.is_empty()
    }

    /// The display names, for logging and the F3-style status line.
    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.ops.values().map(String::as_str)
    }
}

#[derive(Deserialize)]
struct OpsToml {
    #[serde(default)]
    ops: Vec<OpEntry>,
}

#[derive(Deserialize)]
struct OpEntry {
    /// Decimal string, for the same u64-in-TOML reason as the world seed and
    /// `profile.toml`'s `client_id`.
    id: String,
    /// Optional, purely so the file is readable by a human.
    #[serde(default)]
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default state of every install: no file, so the host is the only one
    /// who can run commands. This must be the *safe* direction — an unreadable
    /// ops file granting everyone access would be a silent privilege escalation.
    #[test]
    fn an_absent_ops_list_ops_nobody() {
        let ops = OpsList::default();
        assert!(ops.is_empty());
        assert!(!ops.is_op(1));
        assert!(!ops.is_op(0));
    }

    #[test]
    fn an_op_is_matched_by_identity() {
        let ops = OpsList::from_toml(
            r#"
            ops = [
              { id = "18446744073", name = "gustav" },
              { id = "42" },
            ]
            "#,
        )
        .expect("a well-formed ops file parses");
        assert_eq!(ops.len(), 2);
        assert!(ops.is_op(18_446_744_073));
        assert!(ops.is_op(42));
        assert!(!ops.is_op(43));
        // A nameless entry falls back to its id, so the load log is never blank.
        let mut names: Vec<&str> = ops.names().collect();
        names.sort_unstable();
        assert_eq!(names, ["42", "gustav"]);
    }

    /// Fail-soft like every other data file in the project: a broken `ops.toml`
    /// is a warning and an empty list, never a panic mid-session.
    #[test]
    fn a_malformed_ops_file_fails_soft() {
        assert!(OpsList::from_toml("ops = [ this is not toml").is_err());
        // A bad id inside an otherwise valid file only drops that entry.
        let ops = OpsList::from_toml(
            r#"
            ops = [
              { id = "not-a-number" },
              { id = "7" },
            ]
            "#,
        )
        .expect("the file itself is valid TOML");
        assert_eq!(ops.len(), 1);
        assert!(ops.is_op(7));
    }

    /// An empty file is valid and means the same as no file.
    #[test]
    fn an_empty_ops_file_ops_nobody() {
        let ops = OpsList::from_toml("").expect("an empty file is valid");
        assert!(ops.is_empty());
    }
}

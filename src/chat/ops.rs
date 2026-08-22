//! Who is authorized to run commands.
//!
//! Keyed by **account id** rather than by [`PlayerId`](crate::net::PlayerId):
//! player ids are assigned per session and shift on every reconnect, so they
//! can't carry a permission.
//!
//! It used to be keyed by the local `profile.toml` client id, which the client
//! asserted for itself over an unauthenticated handshake — so anyone who learned
//! an op's number *became* that op by setting one environment variable. Now the
//! key is the uuid inside a signed join ticket, which nobody can mint. That is
//! the change that turns this file from an honour system into a permission.
//!
//! The file is read exactly like `profile.toml` (CWD-relative, fail-soft): an
//! absent or malformed `ops.toml` ops nobody rather than taking the game down.
//! Only the authority ever loads it — a client has nothing to decide.

use std::collections::HashMap;
use std::fs;

use serde::Deserialize;
use uuid::Uuid;

/// Ops file, next to `profile.toml` and `saves/` in the data directory.
pub use crate::paths::OPS_FILE;

/// The set of authorized accounts, with the names the file gave them.
#[derive(Debug, Clone, Default)]
pub struct OpsList {
    /// account id → display name (the id itself when the file omits one).
    ops: HashMap<Uuid, String>,
}

impl OpsList {
    /// Read `ops.toml` from the data directory. Never fails: a missing file
    /// is the normal case (nobody but the host is an op) and a broken one is
    /// logged and treated the same way.
    pub fn load() -> Self {
        let text = match fs::read_to_string(crate::paths::ops_path()) {
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

    /// Parse an ops file. Entries whose `id` isn't a uuid are skipped with a
    /// warning — one typo shouldn't revoke everyone else.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: OpsToml = toml::from_str(text).map_err(|err| err.to_string())?;
        let mut ops = HashMap::new();
        for entry in file.ops {
            match Uuid::parse_str(entry.id.trim()) {
                Ok(id) => {
                    let name = entry.name.unwrap_or_else(|| id.to_string());
                    ops.insert(id, name);
                }
                Err(_) => log::warn!(
                    "{OPS_FILE}: '{}' is not an account id (expected a uuid, as shown by /whoami)",
                    entry.id
                ),
            }
        }
        Ok(Self { ops })
    }

    pub fn is_op(&self, account: &Uuid) -> bool {
        self.ops.contains_key(account)
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
    /// The account uuid, as text — what `/whoami` prints and what the auth
    /// server's `/accounts/me` returns.
    id: String,
    /// Optional, purely so the file is readable by a human.
    #[serde(default)]
    name: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const GUSTAV: &str = "6200dcc7-4f94-4632-adc8-37924e5cda4b";
    const SOMEONE: &str = "11111111-2222-3333-4444-555555555555";

    fn uuid(text: &str) -> Uuid {
        Uuid::parse_str(text).expect("test uuid")
    }

    /// The default state of every install: no file, so the host is the only one
    /// who can run commands. This must be the *safe* direction — an unreadable
    /// ops file granting everyone access would be a silent privilege escalation.
    #[test]
    fn an_absent_ops_list_ops_nobody() {
        let ops = OpsList::default();
        assert!(ops.is_empty());
        assert!(!ops.is_op(&uuid(GUSTAV)));
        assert!(!ops.is_op(&Uuid::nil()));
    }

    #[test]
    fn an_op_is_matched_by_account_id() {
        let ops = OpsList::from_toml(&format!(
            r#"
            ops = [
              {{ id = "{GUSTAV}", name = "gustav" }},
              {{ id = "{SOMEONE}" }},
            ]
            "#
        ))
        .expect("a well-formed ops file parses");

        assert_eq!(ops.len(), 2);
        assert!(ops.is_op(&uuid(GUSTAV)));
        assert!(ops.is_op(&uuid(SOMEONE)));
        assert!(!ops.is_op(&Uuid::nil()));

        // A nameless entry falls back to its id, so the load log is never blank.
        let mut names: Vec<&str> = ops.names().collect();
        names.sort_unstable();
        assert_eq!(names, [SOMEONE, "gustav"]);
    }

    #[test]
    fn uuid_matching_ignores_formatting_differences() {
        let ops = OpsList::from_toml(&format!(
            "ops = [ {{ id = \"  {}  \" }} ]",
            GUSTAV.to_uppercase()
        ))
        .expect("parses");

        // Uuid parsing is case-insensitive and we trim, so an op does not lose
        // their permissions to a stray space or a capital letter.
        assert!(ops.is_op(&uuid(GUSTAV)));
    }

    /// Fail-soft like every other data file in the project: a broken `ops.toml`
    /// is a warning and an empty list, never a panic mid-session.
    #[test]
    fn a_malformed_ops_file_fails_soft() {
        assert!(OpsList::from_toml("ops = [ this is not toml").is_err());

        // A bad id inside an otherwise valid file only drops that entry.
        let ops = OpsList::from_toml(&format!(
            r#"
            ops = [
              {{ id = "not-a-uuid" }},
              {{ id = "18446744073" }},
              {{ id = "{GUSTAV}" }},
            ]
            "#
        ))
        .expect("the file itself is valid TOML");

        assert_eq!(ops.len(), 1);
        assert!(ops.is_op(&uuid(GUSTAV)));
    }

    /// The old format keyed on a decimal client id. Those entries no longer
    /// parse, and must fail *closed* — an unrecognised line granting ops would
    /// be exactly the escalation this rewrite exists to remove.
    #[test]
    fn a_pre_account_ops_file_ops_nobody() {
        let ops = OpsList::from_toml(
            r#"
            ops = [
              { id = "18446744073", name = "gustav" },
              { id = "42" },
            ]
            "#,
        )
        .expect("the file itself is valid TOML");

        assert!(ops.is_empty());
    }

    /// An empty file is valid and means the same as no file.
    #[test]
    fn an_empty_ops_file_ops_nobody() {
        let ops = OpsList::from_toml("").expect("an empty file is valid");
        assert!(ops.is_empty());
    }
}

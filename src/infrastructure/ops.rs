//! Loading the ops list from the data directory.
//!
//! The list's rules — the format, and who counts as an op — are
//! [`crate::domain::chat::OpsList`]. This is only the file read, kept out here
//! so the domain never touches a filesystem.

use std::fs;

use crate::domain::chat::OpsList;
use crate::domain::chat::ops::OPS_FILE;
use crate::infrastructure::paths;

/// Read `ops.toml` from the data directory. Never fails: a missing file
/// is the normal case (nobody but the host is an op) and a broken one is
/// logged and treated the same way.
pub fn load_ops() -> OpsList {
    let text = match fs::read_to_string(paths::ops_path()) {
        Ok(text) => text,
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => {
            log::debug!("no {OPS_FILE}; only the host may run commands");
            return OpsList::default();
        }
        Err(err) => {
            log::warn!("could not read {OPS_FILE}: {err}; only the host may run commands");
            return OpsList::default();
        }
    };
    match OpsList::from_toml(&text) {
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
            OpsList::default()
        }
    }
}

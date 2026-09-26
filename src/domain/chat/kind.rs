//! What kind of line a chat message is.

use serde::{Deserialize, Serialize};

/// How a chat line reads: someone talking, command output, or a refusal.
///
/// Decided by the rules in this module and carried on the wire unchanged —
/// [`crate::application::protocol`] re-exports it rather than defining its
/// own, so the dependency runs protocol → domain, never the reverse.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum ChatKind {
    /// A player said something.
    Player,
    /// Output from a command, or an announcement.
    System,
    /// A command was refused or didn't parse.
    Error,
}

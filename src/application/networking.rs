//! How a session reaches other peers: its role, who else is here, and who may
//! run commands.

use crate::application::peers::Peers;
use crate::application::session::Session;
use crate::domain::chat::OpsList;

pub struct Networking {
    /// This session's networking role: who has authority, and how messages
    /// reach the other peers (a no-op transport in singleplayer).
    pub session: Box<dyn Session>,
    /// The other peers in this session and what we still owe them.
    pub peers: Peers,
    /// Who may run op-only commands, by stable client identity. Loaded from
    /// `ops.toml` on the authority; always empty on a client, which never
    /// decides anything.
    pub ops: OpsList,
}

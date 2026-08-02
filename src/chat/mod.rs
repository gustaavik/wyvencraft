//! Chat: what players say to each other, and the commands they may run.
//!
//! Everything here is pure data and pure functions — no GPU, no session, and
//! the single filesystem read (`ops.toml`) is fail-soft in the same shape as
//! `profile.toml`. That is deliberate: the interesting parts of a chat system
//! are *parsing* (`/give raw beef 5` → an item name and a count) and *deciding*
//! (may this person run it?), and both are worth testing without a window, a
//! socket, or a world.
//!
//! Commands live one-per-file behind the [`command::ChatCommand`] trait and are
//! found through the [`command::COMMANDS`] registry, so adding one touches no
//! existing command. They reach the world only through
//! [`command::CommandContext`], a four-method port the state layer implements —
//! which is what lets `chat` hold the *policy* while `state`, the layer that
//! actually owns registries and inventories, holds the *mechanism*.
//!
//! That inversion is also what makes the authorization real: a client never
//! reaches [`command::resolve`] for its own input at all. It hands the raw line
//! to the host, the only peer that consults [`OpsList`].
//!
//! [`state::ingame_state::chat`]: crate::state::ingame_state

pub mod command;
pub mod composer;
pub mod log;
pub mod ops;

pub use command::{
    COMMANDS, ChatCommand, CommandContext, FakeContext, Invocation, Permission, Position, resolve,
    suggest, unauthorized_message, unknown_command_message,
};
pub use composer::Composer;
pub use log::{ChatLine, ChatLog};
pub use ops::OpsList;

/// Re-exported for convenience: a line's kind is decided here but travels on
/// the wire, so it is defined in [`crate::net::protocol`].
pub use crate::net::ChatKind;

/// This peer's chat: the history it has seen and the line it is typing.
///
/// Purely local — chat state is never saved or synced; only the *messages* are.
#[derive(Debug, Clone, Default)]
pub struct ChatState {
    pub log: ChatLog,
    pub composer: Composer,
}

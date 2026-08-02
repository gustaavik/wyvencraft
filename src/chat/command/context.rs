//! What a command is allowed to do to the session it runs in.
//!
//! This is the port that lets commands live in `chat` at all. A command needs to
//! touch the item registry, an inventory and the network — all of which live in
//! `state`, which depends on `chat` and not the other way round. Inverting that
//! (commands depend on this abstraction; the state layer implements it) is what
//! keeps the dependency arrow pointing the right way.
//!
//! A command gets the slice of the session it actually needs, not
//! `&mut InGameState` — so implementing one cannot reach the world, the mobs, or
//! the save, and a test can stand the whole thing up in three lines (see
//! [`FakeContext`](super::FakeContext)).
//!
//! The runner is implied: the implementation binds the player who typed the line,
//! so no command has to thread a `PlayerId` through, and none can act on someone
//! else by accident.
//!
//! This is the one interface that grows over time — a command needing a
//! capability nobody has needed yet has to have it exposed here. That is the
//! honest cost of a single dispatchable `&mut dyn CommandContext`: splitting into
//! per-capability supertraits would still hand every command the same surface,
//! so it would buy ceremony rather than isolation. Keep additions grouped by
//! concern, and keep them about the *runner*, never about the session at large.
//!
//! Positions are plain `[f32; 3]` rather than `glam::Vec3` for the same reason
//! [`NetVec3`](crate::net::NetVec3) is: this is a boundary, and plain data keeps
//! `chat` free of a maths dependency it otherwise has no use for.

use crate::net::ChatKind;

/// A world position as commands see it: `[x, y, z]`.
pub type Position = [f32; 3];

/// The session a command runs against, from the command's point of view.
pub trait CommandContext {
    // --- Everyone ---------------------------------------------------------------

    /// Whether the runner may use op-only commands.
    fn is_op(&self) -> bool;

    /// Say something back to the runner (only they see it).
    fn reply(&mut self, kind: ChatKind, text: String);

    // --- Items ------------------------------------------------------------------

    /// Every item name this build knows, for resolution and near-miss
    /// suggestions. Item names are the stable identity across builds — numeric
    /// ids are insertion-order indices — so commands address items by name.
    fn item_names(&self) -> Vec<String>;

    /// Put `count` of the item called `name` into the runner's inventory,
    /// splitting it into stacks and dropping any overflow at their feet.
    ///
    /// `name` must be one of [`item_names`](CommandContext::item_names); an
    /// unknown name is a caller bug and fails soft with a logged warning.
    fn give_item(&mut self, name: &str, count: u32);

    // --- Position ---------------------------------------------------------------

    /// Where the runner is now (the anchor for relative coordinates).
    fn position(&self) -> Position;

    /// Move the runner, without simulating the trip.
    fn teleport(&mut self, position: Position);

    /// Every *other* player in the session and where they are, for resolving a
    /// destination by name. Excludes the runner: teleporting to yourself is a
    /// no-op worth reporting as "no such player" rather than silently accepting.
    fn player_positions(&self) -> Vec<(String, Position)>;
}

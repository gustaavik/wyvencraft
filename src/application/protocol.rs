//! Wire protocol: messages exchanged between the host and connected peers, and
//! the channels they travel on.
//!
//! Design: *command/message pattern*. Positions use plain `[f32; 3]` (not glam
//! types) to keep the wire format stable and glam-feature-independent.

use serde::{Deserialize, Serialize};

pub use crate::domain::chat::ChatKind;
use crate::domain::core::{BlockId, BlockPos, GameMode};
use crate::domain::inventory::ARMOR_SIZE;
use crate::domain::progression::WorldProgression;
use wyven_net::PlayerId;

/// 3D vector as it appears on the wire.
pub type NetVec3 = [f32; 3];

/// One crafting recipe as it travels on the wire. Items are referenced by name
/// (not id) so the mapping stays stable even if registries differ across
/// builds; unknown names are skipped by the receiver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeData {
    pub output: String,
    pub count: u32,
    /// Item name -> count consumed from the inventory.
    pub ingredients: Vec<(String, u32)>,
    /// The crafting station it needs within reach, `None` for a hand recipe.
    pub station: Option<String>,
}

/// An item stack as it travels on the wire. Raw numeric ids (like block edits):
/// a session assumes both ends run the same build; the *disk* format is the
/// layer that converts to stable names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct NetItemStack {
    pub item: u16,
    pub count: u8,
    pub durability: Option<u16>,
}

/// Saved state the host hands back to a returning player in the `Welcome`, so
/// their position/vitals/inventory persist across sessions.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct PlayerRestore {
    pub position: NetVec3,
    pub yaw: f32,
    pub pitch: f32,
    pub health: f32,
    pub hunger: f32,
    pub saturation: f32,
    pub slots: Vec<Option<NetItemStack>>,
    pub selected: u32,
    /// Every item this player has held — what reveals their crafting recipes.
    pub known_items: Vec<u16>,
}

/// Messages a client sends to the host.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ClientMessage {
    /// Player movement update (sent on the unreliable channel).
    Move {
        position: NetVec3,
        yaw: f32,
        pitch: f32,
    },
    /// Request to break the block at `pos`.
    Break { pos: BlockPos },
    /// Request to place `block` at `pos`.
    Place { pos: BlockPos, block: BlockId },
    /// Report the client's current survival vitals to the host.
    Stats {
        health: f32,
        hunger: f32,
        saturation: f32,
    },
    /// Report the client's inventory so the host can persist it in the world
    /// save (sent throttled, only when it changed).
    SyncInventory {
        slots: Vec<Option<NetItemStack>>,
        selected: u32,
    },
    /// Notify the host the client switched game mode.
    SetMode(GameMode),
    /// A line the player typed — ordinary chat *or* a `/command`, sent raw.
    ///
    /// A client deliberately does not parse or run commands itself: the host is
    /// the only peer that knows who is authorized, so it is the only peer that
    /// decides. See `InGameState::dispatch_chat`.
    Chat(String),
    /// Sent once after entering the world: "I'm in-game, send me the current world
    /// state." The host replies with the accumulated block edits as [`ServerMessage::WorldEdits`]
    /// and one [`ServerMessage::MobSpawned`] per live mob. Pull-based (rather than
    /// pushed on join) so it can't be lost to the connecting state draining
    /// channels before the in-game state exists.
    RequestWorldState,
    /// Melee swing landed on mob `id` (the host validates range and applies).
    Attack { id: u64 },
    /// "Tell me what this server is, I am not staying." The server-list probe
    /// sends this instead of [`ClientMessage::RequestWorldState`]; the host
    /// answers with [`ServerMessage::Status`] and — because this peer never
    /// asked for the world — never announces it and never records it. A playing
    /// client must never send it.
    RequestStatus,
    /// Right-click on the block at `pos` — a shrine's wayrune, a boss altar.
    /// The host checks reach and asks the *seed* what stands there, so a client
    /// cannot conjure a shrine by naming a position.
    ///
    /// Carries the client's inventory as it stands at the click, which the
    /// host adopts before judging an offering. A separate `SyncInventory`
    /// cannot do that job: `Channel::Reliable` is *unordered*, so a sync sent
    /// first may still arrive second and leave the offering counted against a
    /// stale copy.
    UseBlock {
        pos: BlockPos,
        slots: Vec<Option<NetItemStack>>,
        selected: u32,
    },
    /// Every item this client has held, sent whenever it learns a new one, so
    /// the host can save which crafting recipes it has discovered and hand
    /// them back in its next [`PlayerRestore`]. The whole set rather than a
    /// delta: `Channel::Reliable` is unordered, and a set is idempotent.
    SyncKnown { items: Vec<u16> },
}

/// Everything a remote player's body is drawn with, as one value.
///
/// One struct rather than a field per thing, so the host's "only re-send on
/// change" check covers all of it by construction: something new that a remote
/// body draws cannot be added and then left stale on every other client.
///
/// `armor` is sized from [`ARMOR_SIZE`] rather than spelled out, so adding or
/// removing a slot cannot leave the wire disagreeing with the inventory it
/// describes.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Equipment {
    /// One item id per armor slot, in `ArmorSlot::ALL` order (`None` = empty).
    pub armor: [Option<u16>; ARMOR_SIZE],
    /// The item in the main hand — what the fist is drawn holding.
    pub held: Option<u16>,
}

/// Messages the host sends to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    /// First message on join: world seed + identity + spawn point + the host's
    /// current time-of-day (so the joining client's sky matches) + the session's
    /// game mode + the host's crafting recipes (authoritative for the session,
    /// so everyone crafts by the same rules regardless of local recipe files).
    /// `restored` carries the player's saved state when the host's world save
    /// recognises this client's identity (`spawn` already points at it then).
    /// `content_hash` fingerprints the host's loaded content (blocks/items/
    /// entities/worldgen definitions); block and item ids cross the wire raw,
    /// so clients refuse to join when their own hash differs.
    Welcome {
        seed: u64,
        your_id: PlayerId,
        spawn: NetVec3,
        time_of_day: f32,
        game_mode: GameMode,
        content_hash: u64,
        recipes: Vec<RecipeData>,
        restored: Option<PlayerRestore>,
    },
    PlayerJoined {
        id: PlayerId,
        name: String,
    },
    PlayerLeft {
        id: PlayerId,
    },
    /// Authoritative position snapshot for a (remote) player.
    PlayerState {
        id: PlayerId,
        position: NetVec3,
        yaw: f32,
        pitch: f32,
    },
    /// A single authoritative block edit to apply.
    BlockChanged {
        pos: BlockPos,
        block: BlockId,
    },
    /// A batch of authoritative edits replayed to a joining client so it sees the
    /// world's modifications (blocks broken/placed before it joined). Sent in
    /// response to [`ClientMessage::RequestWorldState`], possibly across several
    /// messages for large worlds.
    WorldEdits {
        edits: Vec<(BlockPos, BlockId)>,
    },
    /// Authoritative survival vitals + mode for a (remote) player.
    PlayerStats {
        id: PlayerId,
        health: f32,
        hunger: f32,
        mode: GameMode,
    },
    /// What a player is wearing and holding. Sent reliably on change (and to a
    /// joining client for everyone already in), so remote player models render
    /// equipment without bloating the per-tick movement snapshot.
    PlayerEquipment {
        id: PlayerId,
        equipment: Equipment,
    },
    /// A mob came into existence (spawned, or replayed to a joining client).
    /// Kind travels by name (the recipe-wire precedent): unknown names are
    /// skipped with a warning; the content hash already gates real mismatches.
    MobSpawned {
        id: u64,
        kind: String,
        position: NetVec3,
    },
    /// Positions + facings of every live mob, batched once per host frame
    /// (sent on the unreliable channel, like player movement).
    MobStates {
        mobs: Vec<(u64, NetVec3, f32)>,
    },
    /// A mob took damage (authoritative health mirror / hurt feedback).
    MobHurt {
        id: u64,
        health: f32,
    },
    /// A mob left the world. `killed_by` names the killing player, if any —
    /// that peer (and only that peer) rolls and spawns the loot locally,
    /// consistent with block drops being per-peer local.
    MobDespawned {
        id: u64,
        killed_by: Option<PlayerId>,
    },
    /// A mob launched a projectile. Fire-and-forget: clients simulate the
    /// arc locally for display; damage stays host-side. Carries its own
    /// ballistics so no kind lookup is needed.
    ArrowSpawned {
        position: NetVec3,
        velocity: NetVec3,
        gravity: f32,
        lifetime: f32,
    },
    /// A mob (or its arrow) hit the addressed player. The client applies it
    /// to itself through its own armor mitigation and reports the result
    /// back via its normal `Stats` sync (clients own their vitals).
    PlayerDamaged {
        id: PlayerId,
        amount: f32,
    },
    /// A chat line to display. `from` is `None` for command output and system
    /// announcements. Broadcast for ordinary chat; addressed to one player for
    /// the reply to their command.
    ///
    /// The host sends the *raw* text and the speaker's id, not a pre-formatted
    /// line, so each peer renders names its own way.
    Chat {
        from: Option<PlayerId>,
        kind: ChatKind,
        text: String,
    },
    /// The host hands items to the addressed player (the result of a `/give`).
    ///
    /// Clients own their inventory — the host only mirrors it for the save — so
    /// a grant is an *instruction to add*, not a state overwrite. The receiver
    /// applies it exactly as if it had picked the items up: whatever doesn't fit
    /// lands on the ground.
    GrantItems {
        to: PlayerId,
        stacks: Vec<NetItemStack>,
    },
    /// The host moves the addressed player (the result of a `/tp`).
    ///
    /// Like `GrantItems`, an instruction rather than an overwrite: clients own
    /// their position and report it back with `Move`, so the host asks them to
    /// go rather than asserting where they are.
    Teleport {
        to: PlayerId,
        position: NetVec3,
    },
    /// The answer to [`ClientMessage::RequestStatus`]: what the server-list row
    /// shows. Sent only to the peer that asked, and to nobody else ever.
    ///
    /// `content_hash` is the same fingerprint [`ServerMessage::Welcome`] carries,
    /// repeated here so the browser can mark a row incompatible immediately
    /// rather than after a connect that is doomed to be refused.
    Status {
        /// The hosted world's name.
        name: String,
        /// Players in the world right now, the host included.
        online: u32,
        /// The most it will hold, the host included.
        max: u32,
        content_hash: u64,
    },
    /// The world's whole progression, sent to a joining client and re-sent
    /// whenever it changes. Small (a few positions and names), so a snapshot is
    /// simpler and safer than a stream of deltas that could be missed.
    Progression(WorldProgression),
    /// A player read a shrine and it revealed a structure — the chat line and
    /// compass flash. `by` is `None` for a reveal by command.
    Revealed {
        structure: String,
        anchor: BlockPos,
        by: Option<PlayerId>,
    },
    /// The host takes items from the addressed player — an altar offering.
    /// The mirror of [`ServerMessage::GrantItems`]: clients own their
    /// inventory, so the host instructs rather than overwrites.
    ConsumeItems {
        to: PlayerId,
        stacks: Vec<NetItemStack>,
    },
    /// A boss crossed into its next phase.
    BossPhase {
        id: u64,
        phase: u8,
    },
    /// A boss is winding up an attack — the telegraph every peer shows so a
    /// player can dodge. Damage stays host-side.
    BossTelegraph {
        id: u64,
        attack: String,
        windup: f32,
    },
    /// A boss fell. `participants` are the players who were in the arena —
    /// each rolls the boss's loot locally, like any other mob drop.
    BossDefeated {
        id: u64,
        kind: String,
        /// Where it fell — where each participant's loot drops. Carried rather
        /// than read off the replica, because the unordered reliable channel
        /// may deliver the boss's `MobDespawned` first.
        position: NetVec3,
        participants: Vec<PlayerId>,
    },
}

#[cfg(test)]
mod tests;

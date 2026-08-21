//! The vocabulary a host and its clients share regardless of what they say:
//! who a peer is, which channel a message travels on, and how it is encoded.

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Server-assigned identifier for a connected player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);

/// Logical network channels, mapped to renet channel ids.
///
/// The ids are `renet`'s `DefaultChannel` numbering, because both `Host` and
/// `Client` build with `ConnectionConfig::default()`. Note that **1 is reliable
/// *unordered* and 2 is reliable *ordered*** — that is renet's mapping, not a
/// typo here. renet also drains the channels in id order against a per-tick byte
/// budget, so gameplay snapshots get the wire before a bulk transfer does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    /// Frequent, loss-tolerant updates (movement snapshots). Dropped rather than
    /// queued when the budget is tight, which is what keeps them current.
    Unreliable = 0,
    /// Guaranteed delivery, **no ordering between messages** (block edits,
    /// inventory, chat, mob lifecycle events). Each message arrives exactly once,
    /// but two sent in sequence may be applied in either order — nothing on this
    /// channel may depend on its neighbour having landed first.
    Reliable = 1,
    /// Guaranteed *and* ordered, for large transfers sliced across frames (the
    /// initial world-edit sync). The ordering is incidental to its purpose; what
    /// this channel is really for is yielding to the two above it.
    Chunk = 2,
}

impl Channel {
    pub fn id(self) -> u8 {
        self as u8
    }
}

/// Serialize a message to bytes (bincode).
pub fn encode<T: Serialize>(msg: &T) -> Vec<u8> {
    bincode::serde::encode_to_vec(msg, bincode::config::standard())
        .expect("message serialization should not fail")
}

/// Deserialize a message; returns `None` on malformed input.
pub fn decode<T: DeserializeOwned>(bytes: &[u8]) -> Option<T> {
    bincode::serde::decode_from_slice(bytes, bincode::config::standard())
        .ok()
        .map(|(value, _)| value)
}

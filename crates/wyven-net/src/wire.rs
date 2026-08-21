//! The vocabulary a host and its clients share regardless of what they say:
//! who a peer is, which channel a message travels on, and how it is encoded.

use serde::Deserialize;
use serde::Serialize;
use serde::de::DeserializeOwned;

/// Server-assigned identifier for a connected player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);

/// Logical network channels, mapped to renet channel ids.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Channel {
    /// Frequent, loss-tolerant updates (movement snapshots).
    Unreliable = 0,
    /// Ordered, guaranteed delivery (block edits, inventory, chat).
    Reliable = 1,
    /// Large transfers sliced across frames (initial modified-chunk sync).
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

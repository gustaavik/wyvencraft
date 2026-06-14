//! Wire protocol: messages exchanged between the host and connected peers, and
//! the channels they travel on.
//!
//! Design: *command/message pattern*. Positions use plain `[f32; 3]` (not glam
//! types) to keep the wire format stable and glam-feature-independent.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::core::{BlockId, BlockPos};

/// 3D vector as it appears on the wire.
pub type NetVec3 = [f32; 3];

/// Server-assigned identifier for a connected player.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PlayerId(pub u64);

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
    /// Chat message.
    Chat(String),
}

/// Messages the host sends to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    /// First message on join: world seed + identity + spawn point.
    Welcome {
        seed: u64,
        your_id: PlayerId,
        spawn: NetVec3,
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
}

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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn client_message_roundtrips() {
        let msg = ClientMessage::Place {
            pos: BlockPos::new(1, 2, 3),
            block: BlockId(7),
        };
        let bytes = encode(&msg);
        let back: ClientMessage = decode(&bytes).unwrap();
        assert!(matches!(
            back,
            ClientMessage::Place {
                pos: BlockPos { x: 1, y: 2, z: 3 },
                block: BlockId(7)
            }
        ));
    }
}

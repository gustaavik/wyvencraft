//! Wire protocol: messages exchanged between the host and connected peers, and
//! the channels they travel on.
//!
//! Design: *command/message pattern*. Positions use plain `[f32; 3]` (not glam
//! types) to keep the wire format stable and glam-feature-independent.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::core::{BlockId, BlockPos, GameMode};

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
    /// Report the client's current survival vitals to the host.
    Stats { health: f32, hunger: f32 },
    /// Notify the host the client switched game mode.
    SetMode(GameMode),
    /// Chat message.
    Chat(String),
}

/// Messages the host sends to clients.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum ServerMessage {
    /// First message on join: world seed + identity + spawn point + the host's
    /// current time-of-day (so the joining client's sky matches) + the session's
    /// game mode.
    Welcome {
        seed: u64,
        your_id: PlayerId,
        spawn: NetVec3,
        time_of_day: f32,
        game_mode: GameMode,
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
    /// Authoritative survival vitals + mode for a (remote) player.
    PlayerStats {
        id: PlayerId,
        health: f32,
        hunger: f32,
        mode: GameMode,
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

    #[test]
    fn welcome_carries_game_mode() {
        let msg = ServerMessage::Welcome {
            seed: 42,
            your_id: PlayerId(1),
            spawn: [0.5, 80.0, 0.5],
            time_of_day: 0.25,
            game_mode: GameMode::Creative,
        };
        let back: ServerMessage = decode(&encode(&msg)).unwrap();
        assert!(matches!(
            back,
            ServerMessage::Welcome {
                game_mode: GameMode::Creative,
                ..
            }
        ));
    }

    #[test]
    fn set_mode_and_stats_roundtrip() {
        let set =
            decode::<ClientMessage>(&encode(&ClientMessage::SetMode(GameMode::Survival))).unwrap();
        assert!(matches!(set, ClientMessage::SetMode(GameMode::Survival)));

        let stats = decode::<ServerMessage>(&encode(&ServerMessage::PlayerStats {
            id: PlayerId(2),
            health: 15.0,
            hunger: 8.0,
            mode: GameMode::Survival,
        }))
        .unwrap();
        assert!(matches!(
            stats,
            ServerMessage::PlayerStats {
                id: PlayerId(2),
                mode: GameMode::Survival,
                ..
            }
        ));
    }
}

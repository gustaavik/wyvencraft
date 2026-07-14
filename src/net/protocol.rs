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

/// One crafting recipe as it travels on the wire. Items are referenced by name
/// (not id) so the mapping stays stable even if registries differ across
/// builds; unknown names are skipped by the receiver.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct RecipeData {
    pub output: String,
    pub count: u32,
    /// Item name -> count consumed from the inventory.
    pub ingredients: Vec<(String, u32)>,
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
    /// Chat message.
    Chat(String),
    /// Sent once after entering the world: "I'm in-game, send me the current world
    /// state." The host replies with the accumulated block edits as [`ServerMessage::WorldEdits`]
    /// and one [`ServerMessage::MobSpawned`] per live mob. Pull-based (rather than
    /// pushed on join) so it can't be lost to the connecting state draining
    /// channels before the in-game state exists.
    RequestWorldState,
    /// Melee swing landed on mob `id` (the host validates range and applies).
    Attack { id: u64 },
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
    /// A player's equipped armor, one item id per armor slot (`None` = empty).
    /// Sent reliably on change (and to a joining client for everyone already in),
    /// so remote player models render armor without bloating the per-tick
    /// movement snapshot.
    PlayerEquipment {
        id: PlayerId,
        armor: [Option<u16>; 6],
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
    fn welcome_carries_game_mode_and_recipes() {
        let recipe = RecipeData {
            output: "wooden pickaxe".to_string(),
            count: 1,
            ingredients: vec![("wood".to_string(), 3)],
        };
        let msg = ServerMessage::Welcome {
            seed: 42,
            your_id: PlayerId(1),
            spawn: [0.5, 80.0, 0.5],
            time_of_day: 0.25,
            game_mode: GameMode::Creative,
            content_hash: 0xDEAD_BEEF,
            recipes: vec![recipe.clone()],
            restored: None,
        };
        let back: ServerMessage = decode(&encode(&msg)).unwrap();
        match back {
            ServerMessage::Welcome {
                game_mode: GameMode::Creative,
                content_hash,
                recipes,
                ..
            } => {
                assert_eq!(content_hash, 0xDEAD_BEEF);
                assert_eq!(recipes, vec![recipe]);
            }
            _ => panic!("expected a creative-mode Welcome"),
        }
    }

    #[test]
    fn request_world_state_roundtrips() {
        let back = decode::<ClientMessage>(&encode(&ClientMessage::RequestWorldState)).unwrap();
        assert!(matches!(back, ClientMessage::RequestWorldState));
    }

    #[test]
    fn world_edits_roundtrips() {
        let msg = ServerMessage::WorldEdits {
            edits: vec![
                (BlockPos::new(1, 2, 3), BlockId::AIR),
                (BlockPos::new(-4, 70, 9), BlockId(7)),
            ],
        };
        let back: ServerMessage = decode(&encode(&msg)).unwrap();
        match back {
            ServerMessage::WorldEdits { edits } => {
                assert_eq!(edits.len(), 2);
                assert_eq!(edits[0], (BlockPos::new(1, 2, 3), BlockId::AIR));
                assert_eq!(edits[1], (BlockPos::new(-4, 70, 9), BlockId(7)));
            }
            _ => panic!("expected WorldEdits"),
        }
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

    #[test]
    fn sync_inventory_roundtrips() {
        let msg = ClientMessage::SyncInventory {
            slots: vec![
                Some(NetItemStack {
                    item: 3,
                    count: 12,
                    durability: None,
                }),
                None,
                Some(NetItemStack {
                    item: 17,
                    count: 1,
                    durability: Some(42),
                }),
            ],
            selected: 2,
        };
        let back: ClientMessage = decode(&encode(&msg)).unwrap();
        match back {
            ClientMessage::SyncInventory { slots, selected } => {
                assert_eq!(selected, 2);
                assert_eq!(slots.len(), 3);
                assert_eq!(slots[2].unwrap().durability, Some(42));
            }
            _ => panic!("expected SyncInventory"),
        }
    }

    #[test]
    fn mob_messages_roundtrip() {
        let attack = decode::<ClientMessage>(&encode(&ClientMessage::Attack { id: 9 })).unwrap();
        assert!(matches!(attack, ClientMessage::Attack { id: 9 }));

        let spawned = ServerMessage::MobSpawned {
            id: 3,
            kind: "cow".to_string(),
            position: [10.0, 64.0, -3.0],
        };
        match decode::<ServerMessage>(&encode(&spawned)).unwrap() {
            ServerMessage::MobSpawned { id, kind, position } => {
                assert_eq!((id, kind.as_str()), (3, "cow"));
                assert_eq!(position, [10.0, 64.0, -3.0]);
            }
            other => panic!("expected MobSpawned, got {other:?}"),
        }

        let states = ServerMessage::MobStates {
            mobs: vec![(3, [1.0, 2.0, 3.0], 0.5), (4, [4.0, 5.0, 6.0], -1.0)],
        };
        match decode::<ServerMessage>(&encode(&states)).unwrap() {
            ServerMessage::MobStates { mobs } => {
                assert_eq!(mobs.len(), 2);
                assert_eq!(mobs[1], (4, [4.0, 5.0, 6.0], -1.0));
            }
            other => panic!("expected MobStates, got {other:?}"),
        }

        let hurt = decode::<ServerMessage>(&encode(&ServerMessage::MobHurt { id: 3, health: 4.5 }))
            .unwrap();
        assert!(matches!(hurt, ServerMessage::MobHurt { id: 3, health } if health == 4.5));

        let despawned = decode::<ServerMessage>(&encode(&ServerMessage::MobDespawned {
            id: 3,
            killed_by: Some(PlayerId(2)),
        }))
        .unwrap();
        assert!(matches!(
            despawned,
            ServerMessage::MobDespawned {
                id: 3,
                killed_by: Some(PlayerId(2)),
            }
        ));

        let arrow = ServerMessage::ArrowSpawned {
            position: [0.0, 70.0, 0.0],
            velocity: [18.0, 2.0, 0.0],
            gravity: 20.0,
            lifetime: 8.0,
        };
        match decode::<ServerMessage>(&encode(&arrow)).unwrap() {
            ServerMessage::ArrowSpawned {
                velocity, gravity, ..
            } => {
                assert_eq!(velocity, [18.0, 2.0, 0.0]);
                assert_eq!(gravity, 20.0);
            }
            other => panic!("expected ArrowSpawned, got {other:?}"),
        }

        let damaged = decode::<ServerMessage>(&encode(&ServerMessage::PlayerDamaged {
            id: PlayerId(1),
            amount: 3.0,
        }))
        .unwrap();
        assert!(matches!(
            damaged,
            ServerMessage::PlayerDamaged {
                id: PlayerId(1),
                amount,
            } if amount == 3.0
        ));
    }

    #[test]
    fn welcome_carries_restored_player_state() {
        let restore = PlayerRestore {
            position: [4.0, 71.0, -9.0],
            yaw: 1.5,
            pitch: -0.2,
            health: 13.0,
            hunger: 9.0,
            saturation: 1.5,
            slots: vec![
                None,
                Some(NetItemStack {
                    item: 5,
                    count: 30,
                    durability: None,
                }),
            ],
            selected: 1,
        };
        let msg = ServerMessage::Welcome {
            seed: 7,
            your_id: PlayerId(3),
            spawn: restore.position,
            time_of_day: 0.5,
            game_mode: GameMode::Survival,
            content_hash: 1,
            recipes: vec![],
            restored: Some(restore.clone()),
        };
        let back: ServerMessage = decode(&encode(&msg)).unwrap();
        match back {
            ServerMessage::Welcome { restored, .. } => assert_eq!(restored, Some(restore)),
            _ => panic!("expected Welcome"),
        }
    }
}

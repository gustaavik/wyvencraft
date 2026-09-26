//! Tests for [`super`]: `protocol.rs`.

use super::*;
use wyven_net::{decode, encode};

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
        output: "wooden_pickaxe".to_string(),
        count: 1,
        ingredients: vec![("wood".to_string(), 3)],
        station: Some("workbench".to_string()),
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
fn sync_known_roundtrips() {
    let msg = ClientMessage::SyncKnown {
        items: vec![1, 2, 300],
    };
    match decode::<ClientMessage>(&encode(&msg)).unwrap() {
        ClientMessage::SyncKnown { items } => assert_eq!(items, vec![1, 2, 300]),
        other => panic!("expected SyncKnown, got {other:?}"),
    }
}

#[test]
fn request_world_state_roundtrips() {
    let back = decode::<ClientMessage>(&encode(&ClientMessage::RequestWorldState)).unwrap();
    assert!(matches!(back, ClientMessage::RequestWorldState));
}

#[test]
fn request_status_roundtrips() {
    let back = decode::<ClientMessage>(&encode(&ClientMessage::RequestStatus)).unwrap();
    assert!(matches!(back, ClientMessage::RequestStatus));
}

/// Every field the server browser shows comes off this one message, so a
/// silent loss of any of them would leave a row looking permanently blank.
#[test]
fn status_carries_everything_a_server_row_shows() {
    let msg = ServerMessage::Status {
        name: "Cliffs & Caves".to_string(),
        online: 3,
        max: 17,
        content_hash: 0xDEAD_BEEF,
    };
    let back: ServerMessage = decode(&encode(&msg)).unwrap();
    match back {
        ServerMessage::Status {
            name,
            online,
            max,
            content_hash,
        } => {
            assert_eq!(name, "Cliffs & Caves");
            assert_eq!((online, max), (3, 17));
            assert_eq!(content_hash, 0xDEAD_BEEF);
        }
        _ => panic!("expected a Status"),
    }
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

    let hurt =
        decode::<ServerMessage>(&encode(&ServerMessage::MobHurt { id: 3, health: 4.5 })).unwrap();
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

/// Both directions of chat, including the item grant that carries a
/// command's result back to a client.
#[test]
fn chat_messages_roundtrip() {
    let said = decode::<ClientMessage>(&encode(&ClientMessage::Chat(
        "/give raw raw_beef 3".to_string(),
    )))
    .unwrap();
    assert!(matches!(said, ClientMessage::Chat(text) if text == "/give raw raw_beef 3"));

    let relayed = ServerMessage::Chat {
        from: Some(PlayerId(4)),
        kind: ChatKind::Player,
        text: "hello".to_string(),
    };
    match decode::<ServerMessage>(&encode(&relayed)).unwrap() {
        ServerMessage::Chat { from, kind, text } => {
            assert_eq!(from, Some(PlayerId(4)));
            assert_eq!(kind, ChatKind::Player);
            assert_eq!(text, "hello");
        }
        other => panic!("expected Chat, got {other:?}"),
    }

    let refused = ServerMessage::Chat {
        from: None,
        kind: ChatKind::Error,
        text: "you are not authorized to use /give".to_string(),
    };
    match decode::<ServerMessage>(&encode(&refused)).unwrap() {
        ServerMessage::Chat {
            from: None,
            kind: ChatKind::Error,
            ..
        } => {}
        other => panic!("expected a system Error line, got {other:?}"),
    }

    let granted = ServerMessage::GrantItems {
        to: PlayerId(2),
        stacks: vec![
            NetItemStack {
                item: 9,
                count: 64,
                durability: None,
            },
            NetItemStack {
                item: 17,
                count: 1,
                durability: Some(60),
            },
        ],
    };
    match decode::<ServerMessage>(&encode(&granted)).unwrap() {
        ServerMessage::GrantItems { to, stacks } => {
            assert_eq!(to, PlayerId(2));
            assert_eq!(stacks.len(), 2);
            assert_eq!(stacks[0].count, 64);
            assert_eq!(stacks[1].durability, Some(60));
        }
        other => panic!("expected GrantItems, got {other:?}"),
    }

    let moved = ServerMessage::Teleport {
        to: PlayerId(2),
        position: [10.5, 72.0, -3.25],
    };
    match decode::<ServerMessage>(&encode(&moved)).unwrap() {
        ServerMessage::Teleport { to, position } => {
            assert_eq!(to, PlayerId(2));
            assert_eq!(position, [10.5, 72.0, -3.25]);
        }
        other => panic!("expected Teleport, got {other:?}"),
    }
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
        known_items: vec![3, 5, 40],
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

/// Both halves of what a remote body is drawn with survive the wire. `held`
/// travels beside the armor deliberately: they change at the same moments
/// and are drawn by the same pass, so one message covers both and the
/// host's change check cannot go stale on one of them.
#[test]
fn equipment_carries_both_the_armor_and_the_hand() {
    let mut equipment = Equipment {
        armor: [None; ARMOR_SIZE],
        held: Some(12),
    };
    equipment.armor[0] = Some(3);
    let msg = ServerMessage::PlayerEquipment {
        id: PlayerId(2),
        equipment,
    };
    let back: ServerMessage = decode(&encode(&msg)).unwrap();
    match back {
        ServerMessage::PlayerEquipment { id, equipment: got } => {
            assert_eq!(id, PlayerId(2));
            assert_eq!(got, equipment);
        }
        other => panic!("expected PlayerEquipment, got {other:?}"),
    }
}

/// An empty fist is a real state, not an absent field: a player who puts
/// their pickaxe away must stop being drawn holding it.
#[test]
fn an_empty_hand_round_trips_as_empty() {
    let msg = ServerMessage::PlayerEquipment {
        id: PlayerId(1),
        equipment: Equipment::default(),
    };
    let back: ServerMessage = decode(&encode(&msg)).unwrap();
    match back {
        ServerMessage::PlayerEquipment { equipment, .. } => {
            assert_eq!(equipment.held, None);
            assert_eq!(equipment.armor, [None; ARMOR_SIZE]);
        }
        other => panic!("expected PlayerEquipment, got {other:?}"),
    }
}

#[test]
fn use_block_roundtrips() {
    let msg = ClientMessage::UseBlock {
        pos: BlockPos::new(-40, 97, 12),
        slots: vec![None],
        selected: 0,
    };
    let back = decode::<ClientMessage>(&encode(&msg)).unwrap();
    assert!(matches!(
        back,
        ClientMessage::UseBlock {
            pos: BlockPos {
                x: -40,
                y: 97,
                z: 12
            },
            ..
        }
    ));
}

#[test]
fn progression_messages_roundtrip() {
    let mut progression = WorldProgression::default();
    progression.read_shrine(
        BlockPos::new(1, 2, 3),
        "meadows_altar",
        BlockPos::new(300, 95, -80),
    );
    progression.defeat("elder stag");
    match decode::<ServerMessage>(&encode(&ServerMessage::Progression(progression.clone())))
        .unwrap()
    {
        ServerMessage::Progression(back) => assert_eq!(back, progression),
        other => panic!("expected Progression, got {other:?}"),
    }

    let revealed = ServerMessage::Revealed {
        structure: "meadows_altar".into(),
        anchor: BlockPos::new(300, 95, -80),
        by: Some(PlayerId(2)),
    };
    match decode::<ServerMessage>(&encode(&revealed)).unwrap() {
        ServerMessage::Revealed {
            structure,
            anchor,
            by,
        } => {
            assert_eq!(structure, "meadows_altar");
            assert_eq!(anchor, BlockPos::new(300, 95, -80));
            assert_eq!(by, Some(PlayerId(2)));
        }
        other => panic!("expected Revealed, got {other:?}"),
    }
}

#[test]
fn consume_items_roundtrips() {
    let msg = ServerMessage::ConsumeItems {
        to: PlayerId(3),
        stacks: vec![NetItemStack {
            item: 44,
            count: 1,
            durability: None,
        }],
    };
    match decode::<ServerMessage>(&encode(&msg)).unwrap() {
        ServerMessage::ConsumeItems { to, stacks } => {
            assert_eq!(to, PlayerId(3));
            assert_eq!(stacks[0].item, 44);
        }
        other => panic!("expected ConsumeItems, got {other:?}"),
    }
}

#[test]
fn boss_messages_roundtrip() {
    let phase =
        decode::<ServerMessage>(&encode(&ServerMessage::BossPhase { id: 7, phase: 1 })).unwrap();
    assert!(matches!(
        phase,
        ServerMessage::BossPhase { id: 7, phase: 1 }
    ));

    let telegraph = ServerMessage::BossTelegraph {
        id: 7,
        attack: "stomp".into(),
        windup: 0.9,
    };
    match decode::<ServerMessage>(&encode(&telegraph)).unwrap() {
        ServerMessage::BossTelegraph { id, attack, windup } => {
            assert_eq!((id, attack.as_str()), (7, "stomp"));
            assert!((windup - 0.9).abs() < 1e-6);
        }
        other => panic!("expected BossTelegraph, got {other:?}"),
    }

    let defeated = ServerMessage::BossDefeated {
        id: 7,
        kind: "elder stag".into(),
        position: [3.0, 97.0, -8.0],
        participants: vec![PlayerId(0), PlayerId(4)],
    };
    match decode::<ServerMessage>(&encode(&defeated)).unwrap() {
        ServerMessage::BossDefeated {
            kind, participants, ..
        } => {
            assert_eq!(kind, "elder stag");
            assert_eq!(participants, vec![PlayerId(0), PlayerId(4)]);
        }
        other => panic!("expected BossDefeated, got {other:?}"),
    }
}

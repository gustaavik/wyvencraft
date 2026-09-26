//! Tests for [`super`]: the in-game screen's networking.

use std::net::UdpSocket;
use std::sync::Arc;
use std::time::Instant;

use wyven_auth::{AccountState, AuthClient, FakeAuthClient, KeyCache};

use super::*;
use crate::application::protocol::NetItemStack;
use crate::application::session::{FakeHandle, FakeSession};
use crate::domain::core::GameMode;
use crate::domain::inventory::{ARMOR_SIZE, ARMOR_START, Inventory};
use crate::domain::world::block::blocks;
use crate::infrastructure::net::status::{NetStatusProbe, StatusOutcome, StatusProbe};
use crate::infrastructure::net::{Client, Host, TicketJoin, host_config};
use crate::infrastructure::save::{ItemStackData, PlayerData};
use crate::presentation::content::GameContent;

/// A client reports its slots and which one it has selected, and that is
/// enough — the host is never told separately what a client is holding, so
/// the two can never disagree.
#[test]
fn a_clients_hand_is_read_out_of_the_inventory_it_reported() {
    let stack = |item| {
        Some(NetItemStack {
            item,
            count: 1,
            durability: None,
        })
    };
    let mut slots = vec![None; ARMOR_START + ARMOR_SIZE];
    slots[0] = stack(7);
    slots[2] = stack(9);
    slots[ARMOR_START] = stack(4);

    let equipment = equipment_from_slots(&slots, 2);
    assert_eq!(equipment.held, Some(9), "the selected slot, not the first");
    assert_eq!(equipment.armor[0], Some(4));
}

/// Selecting an empty slot empties the fist rather than leaving the last
/// item in it, and a selection past the end cannot panic.
#[test]
fn an_empty_or_missing_selection_empties_the_hand() {
    let slots = vec![
        Some(NetItemStack {
            item: 7,
            count: 1,
            durability: None,
        }),
        None,
    ];
    assert_eq!(equipment_from_slots(&slots, 1).held, None);
    assert_eq!(equipment_from_slots(&slots, 99).held, None);
}

/// The whole path a peer's held item takes: a client reports its inventory,
/// the host works out what that client is holding, broadcasts it, and a
/// receiving client stores it on the body it draws.
///
/// Nothing new crosses the wire to make this work — `SyncInventory` was
/// already being sent for the save and for melee damage, so a peer's fist
/// costs one extra field on a message that was already there.
#[test]
fn a_clients_held_item_reaches_the_peers_that_draw_it() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 42,
        account: None,
    });
    state.pump_network(1.0 / 60.0);

    let sword = state
        .content
        .rules
        .items
        .find("iron_sword")
        .expect("iron_sword");
    let mut slots = vec![None; 3];
    slots[2] = Some(NetItemStack {
        item: sword.0,
        count: 1,
        durability: None,
    });
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::SyncInventory { slots, selected: 2 },
    });
    state.pump_network(1.0 / 60.0);

    let held = handle
        .lock()
        .broadcasts()
        .iter()
        .filter_map(|msg| match msg {
            ServerMessage::PlayerEquipment { id, equipment } if *id == pid => Some(equipment.held),
            _ => None,
        })
        .next_back()
        .expect("the host broadcast what the client is holding");
    assert_eq!(held, Some(sword.0));

    // And the receiving end puts it on the body it will draw.
    let (mut peer, peer_handle) = client_session(PlayerId(2));
    peer_handle.deliver(Inbound::Update(ServerMessage::PlayerEquipment {
        id: pid,
        equipment: Equipment {
            armor: [None; ARMOR_SIZE],
            held: Some(sword.0),
        },
    }));
    peer.pump_network(1.0 / 60.0);
    assert_eq!(
        players::get(&peer.sim.ecs, pid).map(|rp| rp.equipment.held),
        Some(Some(sword.0)),
        "the peer's body knows what it is holding"
    );
}

/// An in-game state driven by a fake session, plus the handle to script it.
fn host_session() -> (InGameState, FakeHandle) {
    let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
    let session = FakeSession::host();
    let handle = session.handle();
    state.set_session(Box::new(session));
    (state, handle)
}

fn client_session(local: PlayerId) -> (InGameState, FakeHandle) {
    let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
    let session = FakeSession::client(local);
    let handle = session.handle();
    state.set_session(Box::new(session));
    (state, handle)
}

/// Put `name` in the first hotbar slot and select it.
fn hold(state: &mut InGameState, name: &str) {
    let id = state
        .content
        .rules
        .items
        .find(name)
        .unwrap_or_else(|| panic!("{name}"));
    state
        .sim
        .inventory
        .set_slot(0, Some(state.content.rules.items.full_stack(id)));
    state.sim.inventory.set_selected(0);
}

/// A swing is worth whatever the held item says, and a tool with no
/// `damage` component — or an empty hand — is worth a bare fist.
#[test]
fn a_local_swing_takes_its_damage_from_the_held_item() {
    let (mut state, _handle) = host_session();

    hold(&mut state, "iron_sword");
    assert_eq!(state.sim.melee_damage(), 6.0, "iron_sword");
    hold(&mut state, "wooden_sword");
    assert_eq!(state.sim.melee_damage(), 4.0, "wooden_sword");
    hold(&mut state, "iron_axe");
    assert_eq!(state.sim.melee_damage(), 5.0, "iron_axe");

    hold(&mut state, "iron_pickaxe");
    assert_eq!(
        state.sim.melee_damage(),
        mobs::PLAYER_ATTACK_DAMAGE,
        "a pickaxe is no better than a fist"
    );

    state.sim.inventory.set_slot(0, None);
    assert_eq!(
        state.sim.melee_damage(),
        mobs::PLAYER_ATTACK_DAMAGE,
        "an empty hand is a fist"
    );
}

/// The host resolves a client's swing against the inventory that client
/// last reported, not against the host's own held item.
#[test]
fn a_clients_swing_takes_its_damage_from_their_reported_inventory() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 42,
        account: None,
    });
    state.pump_network(1.0 / 60.0);

    // The host is holding nothing special; the client reports an iron sword.
    assert_eq!(
        state.client_melee_damage(pid),
        mobs::PLAYER_ATTACK_DAMAGE,
        "nothing reported yet, so a fist"
    );

    let sword = state
        .content
        .rules
        .items
        .find("iron_sword")
        .expect("iron_sword");
    let mut slots = vec![None; 3];
    slots[2] = Some(NetItemStack {
        item: sword.0,
        count: 1,
        durability: state.content.rules.items.max_durability(sword),
    });
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::SyncInventory { slots, selected: 2 },
    });
    state.pump_network(1.0 / 60.0);

    assert_eq!(
        state.client_melee_damage(pid),
        6.0,
        "the client's iron sword"
    );
    assert_eq!(
        state.sim.melee_damage(),
        mobs::PLAYER_ATTACK_DAMAGE,
        "the host's own swing is unaffected"
    );
}

/// A first-time joiner is welcomed with this world's seed and their new id,
/// announced to everyone once it asks for the world, and registered as a
/// remote player.
#[test]
fn a_joining_player_is_welcomed_and_announced() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 42,
        account: None,
    });
    // What a real client sends on its first connected frame, and what marks
    // this peer as a player rather than a status probe.
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::RequestWorldState,
    });

    state.pump_network(1.0 / 60.0);

    let net = handle.lock();
    let welcome = net.messages_to(pid);
    let Some(ServerMessage::Welcome {
        seed,
        your_id,
        restored,
        ..
    }) = welcome.first()
    else {
        panic!("the joiner must receive a Welcome, got {welcome:?}");
    };
    assert_eq!(*seed, state.sim.world.seed());
    assert_eq!(*your_id, pid);
    assert!(
        restored.is_none(),
        "a first-time joiner has nothing to restore"
    );
    assert!(
        net.broadcasts()
            .iter()
            .any(|m| matches!(m, ServerMessage::PlayerJoined { id, .. } if *id == pid)),
        "everyone is told about the join"
    );
    drop(net);
    assert!(players::find(&state.sim.ecs, pid).is_some());
    assert_eq!(state.net.peers.identities.get(&pid), Some(&42));
}

/// The server browser's probe holds a real ticket and gets a real
/// `PlayerId`, so the only thing keeping a Refresh from reading as a join to
/// everyone playing is that it never asks for the world.
#[test]
fn a_status_query_is_answered_without_announcing_anybody() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 42,
        account: None,
    });
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::RequestStatus,
    });

    state.pump_network(1.0 / 60.0);

    let net = handle.lock();
    let Some(ServerMessage::Status {
        online,
        max,
        content_hash,
        ..
    }) = net
        .messages_to(pid)
        .into_iter()
        .find(|m| matches!(m, ServerMessage::Status { .. }))
    else {
        panic!("the probe must be told the status");
    };
    assert_eq!(*online, 1, "the host itself is on the server");
    assert_eq!(*max, (crate::infrastructure::net::MAX_CLIENTS + 1) as u32);
    assert_eq!(*content_hash, state.content.rules.hash());
    assert!(
        !net.broadcasts()
            .iter()
            .any(|m| matches!(m, ServerMessage::PlayerJoined { .. })),
        "nobody playing should see a status query"
    );
}

/// The dangerous half of the same story: a probe is handed spawn-fresh
/// vitals in its `Welcome`, so recording it on the way out would overwrite
/// the real player's saved health, hunger and position for that account.
#[test]
fn a_peer_that_never_played_does_not_overwrite_its_accounts_saved_state() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    let identity = 7;
    let saved = PlayerData {
        position: [12.0, 65.0, -8.0],
        yaw: 1.5,
        pitch: 0.2,
        flying: false,
        health: 3.0,
        hunger: 4.0,
        saturation: 0.0,
        selected_slot: 3,
        slots: vec![None; crate::domain::inventory::TOTAL_SLOTS],
    };
    state.save.records.0.insert(identity, saved.clone());

    handle.deliver(Inbound::Joined {
        player: pid,
        identity,
        account: None,
    });
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::RequestStatus,
    });
    state.pump_network(1.0 / 60.0);
    handle.deliver(Inbound::Left { player: pid });
    state.pump_network(1.0 / 60.0);

    let record = state.save.records.0.get(&identity).expect("still recorded");
    assert_eq!(record.health, saved.health, "vitals were rewritten");
    assert_eq!(record.position, saved.position, "position was rewritten");
    assert!(
        !handle
            .lock()
            .broadcasts()
            .iter()
            .any(|m| matches!(m, ServerMessage::PlayerLeft { .. })),
        "nobody was told they arrived, so nobody is told they left"
    );
}

/// A returning identity gets its saved position and inventory handed back
/// in the `Welcome`, rather than starting fresh.
#[test]
fn a_returning_player_gets_their_saved_state_back() {
    let (mut state, handle) = host_session();
    let bread = state.content.rules.items.find("bread").unwrap();
    let identity = 7;

    // This world remembers them from a previous session.
    let mut slots = vec![None; crate::domain::inventory::TOTAL_SLOTS];
    slots[3] = Some(ItemStackData {
        id: "bread".to_string(),
        count: 5,
        durability: None,
    });
    state.save.records.0.insert(
        identity,
        PlayerData {
            position: [12.0, 65.0, -8.0],
            yaw: 1.5,
            pitch: 0.2,
            flying: false,
            health: 14.0,
            hunger: 11.0,
            saturation: 2.0,
            selected_slot: 3,
            slots,
        },
    );

    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity,
        account: None,
    });
    state.pump_network(1.0 / 60.0);

    let net = handle.lock();
    let welcome = net.messages_to(pid);
    let Some(ServerMessage::Welcome {
        spawn, restored, ..
    }) = welcome.first()
    else {
        panic!("expected a Welcome");
    };
    let restored = restored.as_ref().expect("a returning player is restored");
    assert_eq!(
        *spawn,
        [12.0, 65.0, -8.0],
        "they resume where they left off"
    );
    assert_eq!(restored.health, 14.0);
    assert_eq!(restored.selected, 3);
    let stack = restored.slots[3].expect("their bread survives the round trip");
    assert_eq!(stack.item, bread.0);
    assert_eq!(stack.count, 5);
}

/// What a client discovers is kept by the host, merged across reports that
/// may arrive in any order, and handed back when that identity returns.
#[test]
fn a_clients_discoveries_are_kept_and_handed_back_on_rejoin() {
    let (mut state, handle) = host_session();
    let items = state.content.rules.items.clone();
    let wire =
        |names: &[&str]| -> Vec<u16> { names.iter().map(|n| items.find(n).unwrap().0).collect() };
    let identity = 7;
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity,
        account: None,
    });
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::RequestWorldState,
    });
    // A real client reports its inventory too, which is what gives the
    // host a record to restore at all.
    let (slots, selected) = inventory_to_wire(&Inventory::new());
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::SyncInventory { slots, selected },
    });
    // The later, larger set first, then a stale smaller one: nothing lost.
    for names in [&["oak_log", "coal"][..], &["oak_log"][..]] {
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::SyncKnown { items: wire(names) },
        });
    }
    state.pump_network(1.0 / 60.0);
    let mut kept = state.save.discovery.players[&identity].clone();
    kept.sort();
    assert_eq!(kept, ["coal", "oak_log"]);

    handle.deliver(Inbound::Left { player: pid });
    state.pump_network(1.0 / 60.0);

    let again = PlayerId(2);
    handle.deliver(Inbound::Joined {
        player: again,
        identity,
        account: None,
    });
    state.pump_network(1.0 / 60.0);
    let net = handle.lock();
    let Some(ServerMessage::Welcome {
        restored: Some(restored),
        ..
    }) = net.messages_to(again).first()
    else {
        panic!("a returning player is restored");
    };
    let mut known = restored.known_items.clone();
    known.sort();
    let mut expected = wire(&["oak_log", "coal"]);
    expected.sort();
    assert_eq!(known, expected);
}

/// A client reports what it learns — once per new item, not every frame.
#[test]
fn a_client_reports_new_discoveries_once() {
    let (mut state, handle) = client_session(PlayerId(4));
    state.sim.inventory = Inventory::new();
    state.tick_crafting();
    let reports = |handle: &FakeHandle| {
        handle
            .lock()
            .requests()
            .iter()
            .filter(|m| matches!(m, ClientMessage::SyncKnown { .. }))
            .count()
    };
    assert_eq!(reports(&handle), 0, "nothing learned, nothing to say");

    hold(&mut state, "flint");
    state.tick_crafting();
    state.tick_crafting();
    assert_eq!(reports(&handle), 1);
}

/// A leaving player is snapshotted into the persistent records (so a rejoin
/// restores them) and dropped from the live session.
#[test]
fn a_leaving_player_is_recorded_and_forgotten() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 99,
        account: None,
    });
    // Asking for the world is what makes them a player rather than a passing
    // status query, and so what makes them worth recording.
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::RequestWorldState,
    });
    state.pump_network(1.0 / 60.0);

    // They report an inventory, then disconnect.
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::SyncInventory {
            slots: vec![None; crate::domain::inventory::TOTAL_SLOTS],
            selected: 2,
        },
    });
    handle.deliver(Inbound::Left { player: pid });
    state.pump_network(1.0 / 60.0);

    assert!(
        players::find(&state.sim.ecs, pid).is_none(),
        "dropped from the session"
    );
    assert!(!state.net.peers.identities.contains_key(&pid));
    assert_eq!(
        state.save.records.0.get(&99).map(|r| r.selected_slot),
        Some(2),
        "their state is kept against their identity for a rejoin"
    );
    assert!(
        handle
            .lock()
            .broadcasts()
            .iter()
            .any(|m| matches!(m, ServerMessage::PlayerLeft { id } if *id == pid)),
        "everyone is told about the departure"
    );
}

/// The host applies a client's edit to its own world and echoes it, which
/// is what makes the host authoritative over terrain.
#[test]
fn a_client_edit_request_is_applied_and_echoed() {
    let (mut state, handle) = host_session();
    let pos = BlockPos::new(1, 80, 1);
    state.sim.world.set_block(pos, blocks::STONE);

    handle.deliver(Inbound::Request {
        player: PlayerId(1),
        msg: ClientMessage::Break { pos },
    });
    state.pump_network(1.0 / 60.0);

    assert!(
        state.sim.world.block_at(pos).is_air(),
        "the host applied the break"
    );
    assert!(
        handle.lock().broadcasts().iter().any(|m| matches!(
            m,
            ServerMessage::BlockChanged { pos: p, block } if *p == pos && block.is_air()
        )),
        "and echoed it to every peer"
    );
}

/// The same local edit means different things by role: the authority
/// asserts it, a client can only ask.
#[test]
fn a_local_edit_is_asserted_by_a_host_and_requested_by_a_client() {
    let pos = BlockPos::new(4, 70, 4);

    let (mut host, host_net) = host_session();
    host.broadcast_local_edit(pos, blocks::STONE);
    assert!(
        host_net
            .lock()
            .broadcasts()
            .iter()
            .any(|m| matches!(m, ServerMessage::BlockChanged { pos: p, .. } if *p == pos)),
        "a host asserts the edit"
    );
    assert!(host_net.lock().requests().is_empty(), "and asks no one");

    let (mut client, client_net) = client_session(PlayerId(2));
    client.broadcast_local_edit(pos, blocks::STONE);
    let net = client_net.lock();
    assert!(
        net.requests()
            .iter()
            .any(|m| matches!(m, ClientMessage::Place { pos: p, .. } if *p == pos)),
        "a client requests a placement, got {:?}",
        net.requests()
    );
    assert!(net.broadcasts().is_empty(), "and asserts nothing");
}

/// Reach is validated host-side: a client's `Attack` lands only when they
/// were actually next to the mob. Both directions matter — a test that only
/// checked the rejection would pass even if attacks never applied at all.
#[test]
fn a_client_attack_is_reach_validated() {
    let (mut state, handle) = host_session();
    let pid = PlayerId(1);
    handle.deliver(Inbound::Joined {
        player: pid,
        identity: 1,
        account: None,
    });
    state.pump_network(1.0 / 60.0);
    // The joiner is placed at the host's position (no saved record).
    let attacker = players::get(&state.sim.ecs, pid).unwrap().position();

    // In reach: the swing lands.
    let near = state.sim.spawn_mob("cow", attacker).expect("cow spawns");
    let full_health = state.simulated_mobs()[0].health;
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::Attack { id: near.0 },
    });
    state.pump_network(1.0 / 60.0);
    assert!(
        state.simulated_mobs()[0].health < full_health,
        "a swing from next to the mob lands"
    );

    // Out of reach: the same message does nothing.
    let hurt_health = state.simulated_mobs()[0].health;
    let cow = state.simulated_mobs()[0].entity;
    state
        .sim
        .ecs
        .get_mut::<crate::application::ecs::components::Transform>(cow)
        .unwrap()
        .position = attacker + Vec3::new(500.0, 0.0, 0.0);
    handle.deliver(Inbound::Request {
        player: pid,
        msg: ClientMessage::Attack { id: near.0 },
    });
    state.pump_network(1.0 / 60.0);
    assert_eq!(
        state.simulated_mobs()[0].health,
        hurt_health,
        "the same swing from 500 blocks away is rejected"
    );
}

/// A client applies the host's block edits verbatim — no validation, the
/// host is truth.
#[test]
fn a_client_applies_the_hosts_edits() {
    let (mut state, handle) = client_session(PlayerId(2));
    let pos = BlockPos::new(-3, 90, 7);

    handle.deliver(Inbound::Update(ServerMessage::BlockChanged {
        pos,
        block: blocks::STONE,
    }));
    state.pump_network(1.0 / 60.0);

    assert_eq!(state.sim.world.block_at(pos), blocks::STONE);
}

/// A client's copy of a host mob lives its whole life from messages: it
/// appears, moves, mirrors its health, and — killed by this player —
/// leaves loot behind as it goes.
#[test]
fn a_client_replicates_a_host_mob_from_spawn_to_kill() {
    use crate::application::ecs::components::{Health, Replica, Transform};
    use crate::application::ecs::systems::mobs::find;

    let local = PlayerId(2);
    let (mut state, handle) = client_session(local);
    let id = 41;
    handle.deliver(Inbound::Update(ServerMessage::MobSpawned {
        id,
        kind: "cow".to_string(),
        position: [1.0, 70.0, 1.0],
    }));
    state.pump_network(1.0 / 60.0);
    let cow = find(&state.sim.ecs, MobId(id)).expect("the replica exists");
    assert!(
        state.sim.ecs.has::<Replica>(cow),
        "and is a replica, not simulated"
    );
    assert!(state.simulated_mobs().is_empty());

    handle.deliver(Inbound::Update(ServerMessage::MobStates {
        mobs: vec![(id, [4.0, 70.0, 1.0], 1.5)],
    }));
    handle.deliver(Inbound::Update(ServerMessage::MobHurt { id, health: 3.0 }));
    state.pump_network(1.0 / 60.0);
    let at = state.sim.ecs.get::<Transform>(cow).unwrap();
    assert_eq!((at.position, at.yaw), (Vec3::new(4.0, 70.0, 1.0), 1.5));
    assert_eq!(state.sim.ecs.get::<Health>(cow).unwrap().current, 3.0);

    handle.deliver(Inbound::Update(ServerMessage::MobDespawned {
        id,
        killed_by: Some(local),
    }));
    state.pump_network(1.0 / 60.0);
    assert!(!state.sim.ecs.is_alive(cow), "the replica is gone");
    assert!(
        state.sim.drops().next().is_some(),
        "the killer rolls the loot"
    );
}

/// A client tells the host where it is, so other players see it move.
#[test]
fn a_client_reports_its_position_to_the_host() {
    let (mut state, handle) = client_session(PlayerId(2));
    state.sim.player.position = Vec3::new(3.0, 71.0, -5.0);
    state.pump_network(1.0 / 60.0);

    let net = handle.lock();
    assert!(
        net.requests().iter().any(|m| matches!(
            m,
            ClientMessage::Move { position, .. } if *position == [3.0, 71.0, -5.0]
        )),
        "the client reports its position"
    );
    assert!(
        net.requests()
            .iter()
            .any(|m| matches!(m, ClientMessage::RequestWorldState)),
        "and asks for the world's existing edits exactly once on connect"
    );
    assert_eq!(net.flushes, 1, "one flush per frame");
}

fn now_unix() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// A port nothing else is on, found by letting the OS pick one and handing
/// it straight back.
fn free_port() -> u16 {
    let socket = UdpSocket::bind("127.0.0.1:0").expect("a spare port");
    socket.local_addr().expect("bound").port()
}

fn temp_keys(tag: &str) -> std::path::PathBuf {
    std::env::temp_dir().join(format!(
        "wyven-authkeys-{tag}-{}-{:?}.toml",
        std::process::id(),
        std::thread::current().id()
    ))
}

/// The whole status path over a real socket: a host bound on loopback
/// verifying a real Ed25519 ticket, and a probe that connects, asks, is
/// answered, and leaves.
///
/// Every other test here fakes the transport away, which is the right
/// default — but the one thing this feature adds that cannot be faked is
/// that a probe *is* a client, and that a host will answer one without
/// treating it as a player.
#[test]
fn a_probe_reaches_a_real_host_and_is_answered_without_joining_it() {
    let keys_path = temp_keys("probe");
    // Stamped against the wall clock, because the host on the other end of
    // the socket checks its own: the double's fixed default sits years in
    // the future and would be refused as "not valid yet".
    let auth: Arc<dyn AuthClient> = Arc::new(
        FakeAuthClient::new()
            .with_account("gustav", "hunter2")
            .with_account("mira", "hunter2")
            .issuing_at(now_unix()),
    );
    KeyCache::at(&keys_path)
        .store(&auth.public_keys().expect("the double publishes keys"))
        .expect("keys are cached");

    // A host on a real port, refusing anyone it cannot verify — exactly the
    // gate a live server runs behind.
    let port = free_port();
    let content = GameContent::builtin();
    let ours = content.rules.hash();
    let host =
        Host::bind(port, 4242, host_config(), TicketJoin::at(&keys_path)).expect("binds loopback");
    assert!(host.can_verify(), "the host must be able to check tickets");
    let mut server = InGameState::new_host(content, 4242, host, GameMode::Survival);

    let account = AccountState::new();
    account.sign_in(auth.login("gustav", "hunter2").expect("signs in"));
    let mut probe = NetStatusProbe::with_client(&account, Arc::clone(&auth));
    probe.begin(vec![format!("127.0.0.1:{port}")]);

    let dt = Duration::from_millis(16);
    let deadline = Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        server.pump_network(dt.as_secs_f32());
        if let Some((_, outcome)) = probe.poll(dt).into_iter().next() {
            break outcome;
        }
        assert!(Instant::now() < deadline, "the probe never got an answer");
        std::thread::sleep(dt);
    };

    let StatusOutcome::Online(status) = outcome else {
        panic!("expected the host to answer, got {outcome:?}");
    };
    // Counted while the probe is still connected, which is the point: the
    // probe holds a `PlayerId` at this moment and must not be one of the
    // players the row reports.
    assert_eq!(status.online, 1, "only the host is in the world");
    assert_eq!(
        status.max,
        (crate::infrastructure::net::MAX_CLIENTS + 1) as u32
    );
    assert_eq!(status.content_hash, ours);
    assert!(!status.name.is_empty(), "a row needs something to show");

    // --- and the other half: a real client still announces itself ---
    //
    // Announcing moved off the connect event and onto the first request for
    // the world, which is the change that makes a probe invisible. This is
    // the half that has to keep working: a peer that *does* ask for the
    // world must still be counted, or the browser would report every server
    // as empty.
    //
    // A second account, because a netcode id is derived from the account and
    // netcode admits each id once: one person cannot be playing on a server
    // and querying it in the same breath.
    let player_account = AccountState::new();
    player_account.sign_in(auth.login("mira", "hunter2").expect("signs in"));
    let ticket =
        crate::infrastructure::net::ticket::issue(&player_account, auth.as_ref(), now_unix())
            .expect("a ticket for the player");
    let mut player = Client::connect(
        format!("127.0.0.1:{port}").parse().expect("loopback"),
        player_account.netcode_id().expect("signed in"),
        crate::infrastructure::net::PROTOCOL_ID,
        Some(ticket.slot),
    )
    .expect("connects");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut asked = false;
    loop {
        server.pump_network(dt.as_secs_f32());
        player.pump(dt).expect("still connected");
        let _ = player.receive();
        if !asked && player.is_connected() {
            player.send(&ClientMessage::RequestWorldState, Channel::Reliable);
            asked = true;
        }
        let _ = player.flush();
        if asked && server.net.peers.announced.len() == 1 {
            break;
        }
        assert!(Instant::now() < deadline, "the client was never announced");
        std::thread::sleep(dt);
    }

    probe.begin(vec![format!("127.0.0.1:{port}")]);
    let deadline = Instant::now() + Duration::from_secs(10);
    let outcome = loop {
        server.pump_network(dt.as_secs_f32());
        player.pump(dt).expect("still connected");
        let _ = player.flush();
        if let Some((_, outcome)) = probe.poll(dt).into_iter().next() {
            break outcome;
        }
        assert!(Instant::now() < deadline, "the second probe got no answer");
        std::thread::sleep(dt);
    };
    let StatusOutcome::Online(status) = outcome else {
        panic!("expected the host to answer again, got {outcome:?}");
    };
    assert_eq!(status.online, 2, "the host and the player who joined");

    player.disconnect();
    let _ = std::fs::remove_file(&keys_path);
}

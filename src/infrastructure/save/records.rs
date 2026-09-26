//! A host's per-identity player records: snapshotting a connected player
//! into them, and handing one back to a returning player.

use std::collections::HashMap;

use wyven_net::PlayerId;

use crate::application::ecs::Ecs;
use crate::application::ecs::systems::players;
use crate::application::protocol::{NetItemStack, PlayerRestore};
use crate::domain::inventory::crafting::KnownItems;
use crate::domain::inventory::{ItemId, ItemRegistry};
use crate::infrastructure::save::{ItemStackData, PlayerData, PlayerRecords};

/// Snapshot one connected player into the host's persistent per-identity
/// records. A free function over the individual fields so it can be called
/// from inside a borrow of the session's networking state.
pub fn record_remote(
    records: &mut PlayerRecords,
    identities: &HashMap<PlayerId, u64>,
    ecs: &Ecs,
    remote_inventories: &HashMap<PlayerId, (Vec<Option<NetItemStack>>, u32)>,
    items: &ItemRegistry,
    pid: PlayerId,
) {
    let Some(&identity) = identities.get(&pid) else {
        return;
    };
    let Some(rp) = players::get(ecs, pid) else {
        return;
    };
    // A client that never reported an inventory keeps its previous record's.
    let (slots, selected) = match remote_inventories.get(&pid) {
        Some((slots, selected)) => (wire_slots_to_ids(slots, items), *selected),
        None => match records.0.get(&identity) {
            Some(prev) => (prev.slots.clone(), prev.selected_slot),
            // Never reported an inventory and no history: don't record at all,
            // so a rejoin starts fresh (starter kit) instead of empty-handed.
            None => return,
        },
    };
    records.0.insert(
        identity,
        PlayerData {
            position: rp.position().to_array(),
            yaw: rp.yaw,
            pitch: rp.pitch,
            flying: false,
            health: rp.health,
            hunger: rp.hunger,
            saturation: rp.saturation,
            selected_slot: selected,
            slots,
        },
    );
}

/// Convert wire inventory slots to the id-based on-disk form. Numeric ids out
/// of this build's registry range (mismatched peer) become empty slots.
pub fn wire_slots_to_ids(
    slots: &[Option<NetItemStack>],
    items: &ItemRegistry,
) -> Vec<Option<ItemStackData>> {
    slots
        .iter()
        .map(|slot| {
            slot.and_then(|s| {
                ((s.item as usize) < items.len()).then(|| ItemStackData {
                    id: items.get(ItemId(s.item)).id.clone(),
                    count: s.count,
                    durability: s.durability,
                })
            })
        })
        .collect()
}

/// Convert a saved record back to wire form for a returning client's `Welcome`,
/// with the items it had discovered (`known`, string ids). Item names this
/// build no longer knows are dropped.
pub fn record_to_restore(
    record: &PlayerData,
    known: &[String],
    items: &ItemRegistry,
) -> PlayerRestore {
    PlayerRestore {
        position: record.position,
        yaw: record.yaw,
        pitch: record.pitch,
        health: record.health,
        hunger: record.hunger,
        saturation: record.saturation,
        slots: record
            .slots
            .iter()
            .map(|slot| {
                slot.as_ref().and_then(|s| {
                    items.find(&s.id).map(|id| NetItemStack {
                        item: id.0,
                        count: s.count,
                        durability: s.durability,
                    })
                })
            })
            .collect(),
        selected: record.selected_slot,
        known_items: KnownItems::from_ids(known, items).to_wire(),
    }
}

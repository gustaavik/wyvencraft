//! How world progression and block uses cross the wire.
//!
//! The host owns [`WorldProgression`](crate::progression::WorldProgression)
//! and re-sends it whole whenever it changes; a client only ever mirrors it.
//! Kept apart from `net` (already the largest file in the state layer) so the
//! biome/boss loop's networking reads as one piece.

use super::InGameState;
use super::block_use::describe_way;
use super::net::inventory_to_wire;
use crate::core::BlockPos;
use crate::core::ident::title_case;
use crate::inventory::{ARMOR_START, ItemStack};
use crate::net::{Channel, ChatKind, ClientMessage, NetItemStack, PlayerId, ServerMessage};

impl InGameState {
    /// Client: ask the host to use the block at `pos`, with the inventory as
    /// it stands now riding in the same message — an altar takes its offering
    /// from the host's copy, which must be this one, not whichever sync the
    /// unordered channel happened to deliver last.
    pub(super) fn request_block_use(&mut self, pos: BlockPos) {
        let (slots, selected) = inventory_to_wire(&self.inventory);
        self.session.request(
            &ClientMessage::UseBlock {
                pos,
                slots,
                selected,
            },
            Channel::Reliable,
        );
        self.peers.last_synced_inventory = Some(self.inventory.clone());
    }

    /// Authority: send the whole progression to everyone.
    pub(super) fn broadcast_progression(&mut self) {
        self.session.broadcast(
            &ServerMessage::Progression(self.progression.clone()),
            Channel::Reliable,
        );
    }

    /// Authority: a structure was revealed — tell everyone, and say so in chat
    /// to every player but the one who read the shrine (they already got the
    /// directions).
    pub(super) fn announce_reveal(
        &mut self,
        structure: &str,
        anchor: BlockPos,
        by: Option<PlayerId>,
    ) {
        self.broadcast_progression();
        self.session.broadcast(
            &ServerMessage::Revealed {
                structure: structure.to_string(),
                anchor,
                by,
            },
            Channel::Reliable,
        );
        if by != Some(self.session.local_id()) {
            self.note_reveal(structure, anchor, by);
        }
    }

    /// Authority: a system line for everyone — here and on every client.
    pub(super) fn announce(&mut self, text: String) {
        self.chat.log.push(ChatKind::System, text.clone());
        self.session.broadcast(
            &ServerMessage::Chat {
                from: None,
                kind: ChatKind::System,
                text,
            },
            Channel::Reliable,
        );
    }

    /// The chat line for someone else's reveal, with directions from here.
    fn note_reveal(&mut self, structure: &str, anchor: BlockPos, by: Option<PlayerId>) {
        let here = BlockPos::from_world(self.player.position);
        let name = title_case(structure);
        let way = describe_way(here, anchor);
        let line = match by {
            Some(id) => format!(
                "{} read a wayrune: the {name} is revealed, {way}.",
                self.player_name(id)
            ),
            None => format!("The {name} is revealed, {way}."),
        };
        self.chat.log.push(ChatKind::System, line);
    }

    /// Authority: take `stacks` from `actor` — straight out of our inventory if
    /// that is us, otherwise as an instruction to the client, mirrored into the
    /// host's copy so a second offering in the same breath is refused.
    pub(super) fn take_items(&mut self, actor: PlayerId, stacks: &[ItemStack]) {
        if actor == self.session.local_id() {
            for stack in stacks {
                self.inventory.remove(stack.item, u32::from(stack.count));
            }
            return;
        }
        let wire: Vec<NetItemStack> = stacks
            .iter()
            .map(|s| NetItemStack {
                item: s.item.0,
                count: s.count,
                durability: s.durability,
            })
            .collect();
        if let Some((slots, _)) = self.peers.inventories.get_mut(&actor) {
            for stack in stacks {
                remove_from_wire(slots, stack.item.0, u32::from(stack.count));
            }
        }
        self.session.send_to(
            actor,
            &ServerMessage::ConsumeItems {
                to: actor,
                stacks: wire,
            },
            Channel::Reliable,
        );
    }

    /// Authority: how many of `item` `actor` holds — our own inventory, or the
    /// copy a client last reported.
    pub(super) fn count_items(&self, actor: PlayerId, item: crate::inventory::ItemId) -> u32 {
        if actor == self.session.local_id() {
            return self.inventory.count_of(item);
        }
        self.peers.inventories.get(&actor).map_or(0, |(slots, _)| {
            storage(slots)
                .iter()
                .flatten()
                .filter(|s| s.item == item.0)
                .map(|s| u32::from(s.count))
                .sum()
        })
    }

    /// Client: apply a progression-related update from the host. Returns the
    /// message back if it was not one of ours.
    pub(super) fn apply_progression_update(&mut self, msg: ServerMessage) -> Option<ServerMessage> {
        let local_id = self.session.local_id();
        match msg {
            ServerMessage::Progression(progression) => self.progression = progression,
            ServerMessage::Revealed {
                structure,
                anchor,
                by,
            } => {
                if by != Some(local_id) {
                    self.note_reveal(&structure, anchor, by);
                }
            }
            ServerMessage::ConsumeItems { to, stacks } if to == local_id => {
                for stack in stacks {
                    let item = crate::inventory::ItemId(stack.item);
                    if (stack.item as usize) < self.content.items.len() {
                        self.inventory.remove(item, u32::from(stack.count));
                    }
                }
                // Already consistent with the host's copy: no need to echo.
                self.peers.last_synced_inventory = Some(self.inventory.clone());
            }
            other => return Some(other),
        }
        None
    }
}

/// The storage part of a wire-form inventory: everything but the armor worn,
/// which — as in `Inventory::count_of` — is never an ingredient or offering.
fn storage(slots: &[Option<NetItemStack>]) -> &[Option<NetItemStack>] {
    &slots[..slots.len().min(ARMOR_START)]
}

/// Take up to `count` of `item` out of a wire-form inventory, exactly as
/// `Inventory::remove` does on the client — storage only, later slots first —
/// so the host's copy and the client's own agree afterwards.
fn remove_from_wire(slots: &mut [Option<NetItemStack>], item: u16, mut count: u32) {
    let end = slots.len().min(ARMOR_START);
    for slot in slots[..end].iter_mut().rev() {
        if count == 0 {
            return;
        }
        let Some(stack) = slot else { continue };
        if stack.item != item {
            continue;
        }
        let take = count.min(u32::from(stack.count));
        stack.count -= take as u8;
        count -= take;
        if stack.count == 0 {
            *slot = None;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stack(item: u16, count: u8) -> Option<NetItemStack> {
        Some(NetItemStack {
            item,
            count,
            durability: None,
        })
    }

    #[test]
    fn removing_from_a_wire_inventory_takes_across_slots() {
        let mut slots = vec![stack(3, 2), stack(5, 9), stack(3, 4)];
        remove_from_wire(&mut slots, 3, 5);
        assert_eq!(
            slots,
            vec![stack(3, 1), stack(5, 9), None],
            "later slots first"
        );
        remove_from_wire(&mut slots, 3, 99);
        assert_eq!(slots, vec![None, stack(5, 9), None]);
    }

    #[test]
    fn worn_armor_is_never_taken() {
        let mut slots = vec![None; ARMOR_START + 1];
        slots[ARMOR_START] = stack(3, 1);
        slots[0] = stack(3, 1);
        remove_from_wire(&mut slots, 3, 5);
        assert_eq!(slots[0], None);
        assert_eq!(slots[ARMOR_START], stack(3, 1));
        assert_eq!(storage(&slots).len(), ARMOR_START);
    }

    use super::super::InGameState;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::state::session::{FakeSession, Inbound};
    use crate::world::block::blocks;

    fn hosting() -> (InGameState, crate::state::session::FakeHandle) {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let session = FakeSession::host();
        let handle = session.handle();
        state.set_session(Box::new(session));
        (state, handle)
    }

    fn holding(state: &InGameState, item: &str) -> Vec<Option<NetItemStack>> {
        let id = state.content.items.find(item).unwrap();
        vec![Some(NetItemStack {
            item: id.0,
            count: 1,
            durability: None,
        })]
    }

    /// The host judges a use by the inventory inside the `UseBlock` itself,
    /// whatever older sync it happened to hold.
    #[test]
    fn a_use_block_carries_the_inventory_it_is_judged_by() {
        let (mut state, handle) = hosting();
        let pid = PlayerId(1);
        state.peers.inventories.insert(pid, (vec![], 0));
        let slots = holding(&state, "stag_effigy");
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::UseBlock {
                pos: BlockPos::new(0, 300, 0),
                slots: slots.clone(),
                selected: 0,
            },
        });
        state.pump_network(1.0 / 60.0);
        assert_eq!(state.peers.inventories.get(&pid), Some(&(slots, 0)));
    }

    /// A modified client breaking a tier-2 ore with a tier-1 pickaxe is
    /// refused and told to put the block back.
    #[test]
    fn a_client_break_below_the_ore_tier_is_undone() {
        let (mut state, handle) = hosting();
        let pid = PlayerId(1);
        let pos = BlockPos::new(1, 60, 1);
        state.world.set_block(pos, blocks::TIN_ORE);
        let slots = holding(&state, "stone_pickaxe");
        state.peers.inventories.insert(pid, (slots, 0));
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Break { pos },
        });
        state.pump_network(1.0 / 60.0);
        assert_eq!(state.world.block_at(pos), blocks::TIN_ORE);
        let guard = handle.lock();
        assert!(guard.messages_to(pid).iter().any(|m| matches!(
            m,
            ServerMessage::BlockChanged { pos: p, block } if *p == pos && *block == blocks::TIN_ORE
        )));
        drop(guard);

        let slots = holding(&state, "antler_pickaxe");
        state.peers.inventories.insert(pid, (slots, 0));
        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Break { pos },
        });
        state.pump_network(1.0 / 60.0);
        assert!(
            state.world.block_at(pos).is_air(),
            "the right tool breaks it"
        );
    }
}

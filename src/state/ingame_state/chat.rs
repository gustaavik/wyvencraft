//! Chat and commands for [`InGameState`]: who said what, who may run what, and
//! what a command does to the world.
//!
//! The split with [`crate::chat`] is the usual one — that module decides *what*
//! a command means, this one owns the registries, the inventory and the session,
//! so it is where a `/give` actually lands. Concretely, this file holds
//! [`SessionContext`]: the real implementation of the
//! [`CommandContext`](crate::chat::CommandContext) port that commands are
//! written against, alongside the in-memory `FakeContext` used by their tests.
//!
//! **The authority is the only peer that runs a command.** A client hands its
//! raw line to the host as [`ClientMessage::Chat`] and waits; the host resolves
//! it, checks the ops list, and answers. That is what makes authorization worth
//! anything: there is no client-side path to skip, exactly like block edits and
//! melee swings.
//!
//! Two helpers keep every command role-agnostic, so no command implementation
//! asks whether it is running for the local player or a remote one:
//! [`InGameState::reply`] (say something back) and [`InGameState::grant`] (hand
//! over items). Binding the actor into [`SessionContext`] is what removes the
//! `PlayerId` from the commands' own vocabulary entirely.

use glam::Vec3;

use super::InGameState;
use crate::chat::{self, ChatKind, ChatState, CommandContext, Invocation, Permission, Position};
use crate::entity::DroppedItem;
use crate::inventory::{ItemId, ItemRegistry, ItemStack};
use crate::net::{Channel, ClientMessage, NetItemStack, NetVec3, PlayerId, ServerMessage};
use crate::ui::chat::ChatAction;

impl InGameState {
    // --- Local input ---------------------------------------------------------------

    /// Draw the chat overlay and act on what the player did to it.
    pub(super) fn draw_chat(&mut self, egui_ctx: &egui::Context) {
        // Destructured so the log can be read while the draft is borrowed
        // mutably by the text widget.
        let ChatState { log, composer } = &mut self.chat;
        let focus = composer.take_focus_request();
        let action =
            crate::ui::chat::draw_chat(egui_ctx, log, composer.open, &mut composer.draft, focus);
        let submitted = match action {
            Some(ChatAction::Submit) => composer.submit(),
            Some(ChatAction::Cancel) => {
                composer.close();
                None
            }
            Some(ChatAction::HistoryPrev) => {
                composer.history_prev();
                None
            }
            Some(ChatAction::HistoryNext) => {
                composer.history_next();
                None
            }
            None => None,
        };
        if let Some(text) = submitted {
            self.submit_chat(text);
        }
    }

    /// Handle a line the local player submitted on the chat bar.
    pub(super) fn submit_chat(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.session.is_authority() {
            let me = self.session.local_id();
            self.dispatch_chat(me, text);
        } else {
            // Send it raw — commands *and* ordinary messages. Nothing is echoed
            // locally: the host's reply is the single copy, which is what keeps
            // a client from seeing its own message twice.
            self.session
                .request(&ClientMessage::Chat(text), Channel::Reliable);
        }
    }

    // --- Authority: interpreting a line ---------------------------------------------

    /// Interpret one submitted line on behalf of `actor`, who may be the local
    /// player or a connected client.
    ///
    /// Authority-only: a client never reaches this, which is the whole point.
    pub(super) fn dispatch_chat(&mut self, actor: PlayerId, text: String) {
        // Resolve to an owned command + arguments first: the borrow of `text`
        // has to end before the line is relayed (which moves it) or a command
        // runs (which borrows `self` mutably through the context).
        let (command, args) = match chat::resolve(&text) {
            Invocation::Message => return self.relay_chat(actor, text),
            Invocation::Unknown { typed } => {
                let message = chat::unknown_command_message(typed);
                return self.reply(actor, ChatKind::Error, message);
            }
            Invocation::Command { command, args } => (command, args.to_string()),
        };

        // Permission is checked here, once, rather than inside every command —
        // an implementation cannot forget to do it.
        if command.permission() == Permission::Op && !self.is_op(actor) {
            let name = command.name();
            self.reply(actor, ChatKind::Error, chat::unauthorized_message(name));
            log::info!("player {} was refused /{name}", actor.0);
            return;
        }

        let mut ctx = SessionContext { state: self, actor };
        command.run(&args, &mut ctx);
    }

    /// Show an ordinary message here and pass it on to everyone else.
    /// `broadcast` is a no-op in singleplayer, and never loops back to us — so
    /// pushing it to our own log first is what makes the host see its own words.
    fn relay_chat(&mut self, from: PlayerId, text: String) {
        let line = format!("<{}> {text}", self.player_name(from));
        self.chat.log.push(ChatKind::Player, line);
        self.session.broadcast(
            &ServerMessage::Chat {
                from: Some(from),
                kind: ChatKind::Player,
                text,
            },
            Channel::Reliable,
        );
    }

    /// Whether `actor` may run op-only commands.
    ///
    /// The local player of an authoritative session is always an op — they own
    /// the process, so there is nothing to enforce against them. Everyone else
    /// is matched by the stable identity the host recorded when they joined.
    fn is_op(&self, actor: PlayerId) -> bool {
        if actor == self.session.local_id() {
            return true;
        }
        self.peers
            .identities
            .get(&actor)
            .is_some_and(|identity| self.ops.is_op(*identity))
    }

    /// Split `count` of `name` into stacks and hand them to `actor`.
    ///
    /// The mechanical half of `/give`: the command already validated the name
    /// against [`CommandContext::item_names`], so an unresolvable one here is a
    /// caller bug and fails soft.
    fn give_item(&mut self, actor: PlayerId, name: &str, count: u32) {
        let Some(id) = self.items.find(name) else {
            log::warn!("asked to give unknown item '{name}'; ignoring");
            return;
        };
        let stacks = build_stacks(id, count, &self.items);
        self.grant(actor, stacks);
        log::info!("gave player {} {count} × {name}", actor.0);
    }

    /// Move `actor` to `position`: directly if that is us, otherwise as an
    /// instruction they apply to themselves (clients own their position).
    ///
    /// Velocity is cleared so a teleport mid-fall doesn't carry the descent into
    /// the destination; `Player::teleport` resets the interpolation and
    /// fall-damage anchors, so arriving is not treated as landing.
    fn teleport(&mut self, actor: PlayerId, position: Position) {
        let [x, y, z] = position;
        log::info!("teleporting player {} to {x:.1} {y:.1} {z:.1}", actor.0);
        if actor == self.session.local_id() {
            self.player.teleport(Vec3::from_array(position));
            self.player.velocity = Vec3::ZERO;
            self.breaking = None;
            return;
        }
        self.session.send_to(
            actor,
            &ServerMessage::Teleport {
                to: actor,
                position,
            },
            Channel::Reliable,
        );
    }

    /// Apply a `Teleport` addressed to us.
    pub(super) fn apply_teleport(&mut self, position: NetVec3) {
        self.player.teleport(Vec3::from_array(position));
        self.player.velocity = Vec3::ZERO;
        self.breaking = None;
    }

    // --- Role-agnostic effects ------------------------------------------------------

    /// Say something back to whoever ran the command: into our own log if that
    /// is us, otherwise addressed to them over the wire.
    fn reply(&mut self, actor: PlayerId, kind: ChatKind, text: String) {
        if actor == self.session.local_id() {
            self.chat.log.push(kind, text);
        } else {
            self.session.send_to(
                actor,
                &ServerMessage::Chat {
                    from: None,
                    kind,
                    text,
                },
                Channel::Reliable,
            );
        }
    }

    /// Hand items to `actor` — straight into our inventory if that is us,
    /// otherwise as a `GrantItems` they apply to themselves. Clients own their
    /// inventory, so the host asks rather than writes.
    fn grant(&mut self, actor: PlayerId, stacks: Vec<ItemStack>) {
        if actor == self.session.local_id() {
            self.receive_stacks(stacks);
            return;
        }
        let wire: Vec<NetItemStack> = stacks
            .iter()
            .map(|stack| NetItemStack {
                item: stack.item.0,
                count: stack.count,
                durability: stack.durability,
            })
            .collect();
        self.session.send_to(
            actor,
            &ServerMessage::GrantItems {
                to: actor,
                stacks: wire,
            },
            Channel::Reliable,
        );
    }

    /// Put stacks into the local inventory, tossing whatever doesn't fit out in
    /// front of the player — the same overflow rule as crafting.
    pub(super) fn receive_stacks(&mut self, stacks: Vec<ItemStack>) {
        for stack in stacks {
            let leftover = self.inventory.add(stack, &self.items);
            if leftover > 0 {
                self.drops.push(DroppedItem::thrown(
                    ItemStack {
                        count: leftover,
                        ..stack
                    },
                    self.player.eye_position(),
                    self.player.look_direction(),
                    self.entities.dropped_item(),
                ));
            }
        }
    }

    // --- Inbound from the network ---------------------------------------------------

    /// Show a chat line that arrived from the host.
    pub(super) fn show_remote_chat(
        &mut self,
        from: Option<PlayerId>,
        kind: ChatKind,
        text: String,
    ) {
        let line = match from {
            Some(id) => format!("<{}> {text}", self.player_name(id)),
            None => text,
        };
        self.chat.log.push(kind, line);
    }

    /// Apply a `GrantItems` addressed to us. Unknown item ids are skipped rather
    /// than indexed into the registry — the content hash gates real mismatches,
    /// but a malformed message must not panic the client.
    pub(super) fn apply_granted_items(&mut self, wire: &[NetItemStack]) {
        let stacks: Vec<ItemStack> = wire
            .iter()
            .filter_map(|stack| stack_from_wire(*stack, &self.items))
            .collect();
        self.receive_stacks(stacks);
    }

    /// Display name for a player id. Names are still generated rather than
    /// chosen, matching what `welcome_player` puts in the peer list.
    fn player_name(&self, id: PlayerId) -> String {
        self.peers
            .players
            .get(&id)
            .map(|player| player.name.clone())
            .unwrap_or_else(|| format!("Player {}", id.0))
    }

    #[cfg(test)]
    pub(super) fn set_ops(&mut self, ops: crate::chat::OpsList) {
        self.ops = ops;
    }
}

/// The live session as a command sees it: this state, bound to the player who
/// typed the line.
///
/// Binding the actor here is what keeps `PlayerId` out of the commands' own
/// vocabulary — a command can only ever affect its runner, because there is no
/// method on the port that takes anyone else.
struct SessionContext<'a> {
    state: &'a mut InGameState,
    actor: PlayerId,
}

impl CommandContext for SessionContext<'_> {
    fn is_op(&self) -> bool {
        self.state.is_op(self.actor)
    }

    fn reply(&mut self, kind: ChatKind, text: String) {
        self.state.reply(self.actor, kind, text);
    }

    fn item_names(&self) -> Vec<String> {
        self.state
            .items
            .iter()
            .map(|(_, item)| item.name.clone())
            .collect()
    }

    fn give_item(&mut self, name: &str, count: u32) {
        self.state.give_item(self.actor, name, count);
    }

    fn position(&self) -> Position {
        match self.state.peers.players.get(&self.actor) {
            Some(player) => player.position().to_array(),
            None => self.state.player.position.to_array(),
        }
    }

    fn teleport(&mut self, position: Position) {
        self.state.teleport(self.actor, position);
    }

    fn player_positions(&self) -> Vec<(String, Position)> {
        let local = self.state.session.local_id();
        // The local player only appears here when someone *else* is the runner —
        // a command never lists its own runner as a destination.
        let own = (self.actor != local).then(|| {
            (
                self.state.player_name(local),
                self.state.player.position.to_array(),
            )
        });
        self.state
            .peers
            .players
            .iter()
            .filter(|(id, _)| **id != self.actor)
            .map(|(_, player)| (player.name.clone(), player.position().to_array()))
            .chain(own)
            .collect()
    }
}

/// Split `count` items into deliverable stacks.
///
/// Tools and armor arrive one per stack at full durability (there is no such
/// thing as a stack of 5 half-worn pickaxes); everything else is chunked at the
/// item's max stack size, which matters because `ItemStack::count` is a `u8`.
fn build_stacks(id: ItemId, count: u32, items: &ItemRegistry) -> Vec<ItemStack> {
    if let Some(durability) = items.max_durability(id) {
        return (0..count)
            .map(|_| ItemStack::with_durability(id, durability))
            .collect();
    }
    let max = u32::from(items.max_stack(id)).max(1);
    let mut remaining = count;
    let mut stacks = Vec::new();
    while remaining > 0 {
        let take = remaining.min(max);
        stacks.push(ItemStack::new(id, take as u8));
        remaining -= take;
    }
    stacks
}

/// Wire → memory, rejecting item ids this build doesn't have.
fn stack_from_wire(wire: NetItemStack, items: &ItemRegistry) -> Option<ItemStack> {
    if usize::from(wire.item) >= items.len() || wire.count == 0 {
        log::warn!("ignoring granted stack with unknown item id {}", wire.item);
        return None;
    }
    Some(ItemStack {
        item: ItemId(wire.item),
        count: wire.count,
        durability: wire.durability,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::chat::OpsList;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::state::session::{FakeHandle, FakeSession, Inbound};

    /// An in-game state driven by a fake session, plus the handle to script it.
    fn host_session() -> (InGameState, FakeHandle) {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        let session = FakeSession::host();
        let handle = session.handle();
        state.set_session(Box::new(session));
        (state, handle)
    }

    fn client_session(local: PlayerId) -> (InGameState, FakeHandle) {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        let session = FakeSession::client(local);
        let handle = session.handle();
        state.set_session(Box::new(session));
        (state, handle)
    }

    /// Register `pid` as a joined client with `identity`, the way `welcome_player`
    /// does, so the ops lookup has something to match against.
    fn join(state: &mut InGameState, handle: &FakeHandle, pid: PlayerId, identity: u64) {
        handle.deliver(Inbound::Joined {
            player: pid,
            identity,
        });
        state.pump_network(1.0 / 60.0);
    }

    fn count_of(state: &InGameState, name: &str) -> u32 {
        let id = state.items.find(name).expect("the item exists");
        state.inventory.count_of(id)
    }

    #[test]
    fn a_singleplayer_give_fills_the_inventory() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/give bread 12".to_string());
        assert_eq!(count_of(&state, "bread"), 12);
        assert!(
            state.chat.log.lines().any(|l| l.text.contains("gave 12")),
            "the runner is told what happened"
        );
    }

    /// The item registry names things with spaces, so this is the case a naive
    /// "first token is the item" parser would break on.
    #[test]
    fn a_multi_word_item_can_be_given() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/give raw beef 3".to_string());
        assert_eq!(count_of(&state, "raw beef"), 3);
    }

    /// A tool has durability, so five of them are five stacks of one — not one
    /// stack of five, which the inventory could not represent meaningfully.
    #[test]
    fn giving_tools_hands_over_one_fresh_tool_per_stack() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/give wooden pickaxe 3".to_string());

        let id = state.items.find("wooden pickaxe").unwrap();
        let full = state.items.max_durability(id).unwrap();
        let picks: Vec<_> = state
            .inventory
            .slots()
            .iter()
            .flatten()
            .filter(|s| s.item == id)
            .collect();
        assert_eq!(picks.len(), 3, "one slot each");
        assert!(
            picks
                .iter()
                .all(|s| s.count == 1 && s.durability == Some(full))
        );
    }

    #[test]
    fn a_typo_gets_a_suggestion_and_no_items() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/give brea 5".to_string());
        assert_eq!(count_of(&state, "bread"), 0, "nothing was given");
        let last = state.chat.log.lines().next_back().expect("an error line");
        assert_eq!(last.kind, ChatKind::Error);
        assert!(
            last.text.contains("did you mean 'bread'"),
            "got {:?}",
            last.text
        );
    }

    /// More than fits must not vanish: the overflow lands on the ground, the
    /// same rule crafting already follows.
    #[test]
    fn a_granted_stack_that_does_not_fit_lands_on_the_ground() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        let stone = state.items.find("stone").unwrap();
        let max = state.items.max_stack(stone);

        // Fill every storage slot, so nothing can be absorbed.
        for slot in 0..crate::inventory::INVENTORY_SIZE {
            state
                .inventory
                .set_slot(slot, Some(ItemStack::new(stone, max)));
        }
        let before = state.inventory.count_of(stone);

        state.submit_chat("/give bread 5".to_string());

        assert_eq!(state.inventory.count_of(stone), before, "nothing displaced");
        assert_eq!(count_of(&state, "bread"), 0, "no room for it");
        let bread = state.items.find("bread").unwrap();
        let dropped: u32 = state
            .drops
            .iter()
            .filter(|d| d.stack.item == bread)
            .map(|d| u32::from(d.stack.count))
            .sum();
        assert_eq!(dropped, 5, "all five are on the floor instead");
    }

    #[test]
    fn a_singleplayer_tp_moves_the_player() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/tp 10 70 -20".to_string());
        assert_eq!(state.player.position, Vec3::new(10.0, 70.0, -20.0));
    }

    /// Relative coordinates anchor on the runner, which is the whole reason the
    /// port exposes `position()`.
    #[test]
    fn a_relative_tp_is_measured_from_where_the_player_stands() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.player.position = Vec3::new(4.0, 65.0, 8.0);
        state.submit_chat("/tp ~ ~30 ~".to_string());
        assert_eq!(state.player.position, Vec3::new(4.0, 95.0, 8.0));
    }

    /// Arriving mid-plunge must not carry the descent into the destination.
    /// (That the trip itself isn't charged as a fall is `Player::teleport`'s
    /// job, pinned by `teleporting_down_does_not_land_as_a_fall`.)
    #[test]
    fn teleporting_drops_the_momentum_you_arrived_with() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.player.position = Vec3::new(0.0, 200.0, 0.0);
        state.player.velocity = Vec3::new(0.0, -40.0, 0.0);

        state.submit_chat("/tp 0 20 0".to_string());

        assert_eq!(state.player.position, Vec3::new(0.0, 20.0, 0.0));
        assert_eq!(state.player.velocity, Vec3::ZERO);
    }

    #[test]
    fn a_tp_outside_the_world_is_refused_and_the_player_stays_put() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        let before = state.player.position;
        state.submit_chat("/tp 0 -5 0".to_string());
        assert_eq!(state.player.position, before);
        let last = state.chat.log.lines().next_back().expect("an error line");
        assert_eq!(last.kind, ChatKind::Error);
    }

    /// The host resolving a client's `/tp <player>`: it knows everyone's
    /// position, and the client owns its own, so the answer is an instruction.
    #[test]
    fn an_op_client_is_told_to_teleport_to_the_host() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        state.set_ops(OpsList::from_toml("ops = [{ id = \"5\" }]").unwrap());
        join(&mut state, &handle, pid, 5);
        state.player.position = Vec3::new(64.0, 71.0, -8.0);

        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Chat("/tp Player 0".to_string()),
        });
        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        assert!(
            net.messages_to(pid).iter().any(|m| matches!(
                m,
                ServerMessage::Teleport { to, position }
                    if *to == pid && *position == [64.0, 71.0, -8.0]
            )),
            "the client is told where to go, got {:?}",
            net.messages_to(pid)
        );
        drop(net);
        assert_eq!(
            state.player.position,
            Vec3::new(64.0, 71.0, -8.0),
            "and the host itself does not move"
        );
    }

    #[test]
    fn a_client_applies_a_teleport_addressed_to_it() {
        let local = PlayerId(2);
        let (mut state, handle) = client_session(local);

        handle.deliver(Inbound::Update(ServerMessage::Teleport {
            to: local,
            position: [1.0, 80.0, 2.0],
        }));
        state.pump_network(1.0 / 60.0);

        assert_eq!(state.player.position, Vec3::new(1.0, 80.0, 2.0));
    }

    #[test]
    fn a_client_ignores_a_teleport_addressed_to_someone_else() {
        let (mut state, handle) = client_session(PlayerId(2));
        let before = state.player.position;

        handle.deliver(Inbound::Update(ServerMessage::Teleport {
            to: PlayerId(9),
            position: [1.0, 80.0, 2.0],
        }));
        state.pump_network(1.0 / 60.0);

        assert_eq!(state.player.position, before);
    }

    /// Registry-driven, so a command added to `chat::COMMANDS` is covered here
    /// the day it lands: *every* op-only command must be refused to a client
    /// who isn't in `ops.toml`, and the refusal must come before the command
    /// parses its arguments (so nothing it does can happen).
    #[test]
    fn every_op_only_command_is_refused_to_an_unauthorized_client() {
        for command in chat::COMMANDS
            .iter()
            .filter(|c| c.permission() == Permission::Op)
        {
            let (mut state, handle) = host_session();
            let pid = PlayerId(1);
            join(&mut state, &handle, pid, 999);

            handle.deliver(Inbound::Request {
                player: pid,
                // Deliberately unparseable arguments: the gate must fire first.
                msg: ClientMessage::Chat(format!("/{} !!!", command.name())),
            });
            state.pump_network(1.0 / 60.0);

            let net = handle.lock();
            assert!(
                net.messages_to(pid).iter().any(|m| matches!(
                    m,
                    ServerMessage::Chat { kind: ChatKind::Error, text, .. }
                        if text.contains("not authorized")
                )),
                "/{} must be refused, got {:?}",
                command.name(),
                net.messages_to(pid)
            );
        }
    }

    /// The authorization boundary. A client who isn't in `ops.toml` gets a
    /// refusal and, crucially, *no* items — if this ever regressed, every
    /// connected player could spawn anything.
    #[test]
    fn an_unauthorized_client_is_refused_and_gets_nothing() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        join(&mut state, &handle, pid, 999);

        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Chat("/give bread 5".to_string()),
        });
        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        assert!(
            net.messages_to(pid).iter().any(|m| matches!(
                m,
                ServerMessage::Chat {
                    kind: ChatKind::Error,
                    text,
                    ..
                } if text.contains("not authorized")
            )),
            "they are told why, got {:?}",
            net.messages_to(pid)
        );
        assert!(
            !net.sent.iter().any(|s| matches!(
                s,
                crate::state::session::Sent::To(_, ServerMessage::GrantItems { .. }, _)
                    | crate::state::session::Sent::Broadcast(ServerMessage::GrantItems { .. }, _)
            )),
            "and nothing is granted, to them or anyone"
        );
    }

    /// The other half of the boundary: an identity listed in `ops.toml` does get
    /// the items, delivered as an instruction they apply themselves.
    #[test]
    fn an_op_client_is_granted_the_items() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        let identity = 4242;
        state.set_ops(OpsList::from_toml(&format!("ops = [{{ id = \"{identity}\" }}]")).unwrap());
        join(&mut state, &handle, pid, identity);

        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Chat("/give bread 5".to_string()),
        });
        state.pump_network(1.0 / 60.0);

        let net = handle.lock();
        let granted = net
            .messages_to(pid)
            .into_iter()
            .find_map(|m| match m {
                ServerMessage::GrantItems { stacks, .. } => Some(stacks.clone()),
                _ => None,
            })
            .expect("an op's /give is granted");
        drop(net);

        let bread = state.items.find("bread").unwrap();
        assert_eq!(granted.len(), 1);
        assert_eq!(granted[0].item, bread.0);
        assert_eq!(granted[0].count, 5);
        assert_eq!(
            state.inventory.count_of(bread),
            0,
            "the host's own inventory is untouched"
        );
    }

    /// A rejoining player brings a new `PlayerId` but the same identity, which
    /// is exactly why the ops list is keyed by identity.
    #[test]
    fn authorization_follows_the_identity_not_the_player_id() {
        let (mut state, handle) = host_session();
        let identity = 77;
        state.set_ops(OpsList::from_toml("ops = [{ id = \"77\" }]").unwrap());
        // Same person, a different session id than last time.
        join(&mut state, &handle, PlayerId(3), identity);

        handle.deliver(Inbound::Request {
            player: PlayerId(3),
            msg: ClientMessage::Chat("/give bread".to_string()),
        });
        state.pump_network(1.0 / 60.0);

        assert!(
            handle
                .lock()
                .messages_to(PlayerId(3))
                .iter()
                .any(|m| matches!(m, ServerMessage::GrantItems { .. })),
        );
    }

    #[test]
    fn a_plain_message_is_shown_locally_and_relayed_to_everyone() {
        let (mut state, handle) = host_session();
        let pid = PlayerId(1);
        join(&mut state, &handle, pid, 1);

        handle.deliver(Inbound::Request {
            player: pid,
            msg: ClientMessage::Chat("hello everyone".to_string()),
        });
        state.pump_network(1.0 / 60.0);

        assert!(
            state
                .chat
                .log
                .lines()
                .any(|l| l.text.contains("hello everyone")),
            "the host sees it — a broadcast never loops back"
        );
        assert!(
            handle.lock().broadcasts().iter().any(|m| matches!(
                m,
                ServerMessage::Chat { from: Some(id), text, .. }
                    if *id == pid && text == "hello everyone"
            )),
            "and so does everyone else"
        );
    }

    /// The client half of the authorization property: it must not evaluate the
    /// command itself, even though `chat::parse` is right there.
    #[test]
    fn a_client_asks_the_host_rather_than_running_the_command_itself() {
        let (mut state, handle) = client_session(PlayerId(2));
        state.submit_chat("/give bread 64".to_string());

        let net = handle.lock();
        assert!(
            net.requests()
                .iter()
                .any(|m| matches!(m, ClientMessage::Chat(text) if text == "/give bread 64")),
            "the raw line goes to the host, got {:?}",
            net.requests()
        );
        drop(net);
        assert_eq!(count_of(&state, "bread"), 0, "nothing happens locally");
        assert!(
            state.chat.log.is_empty(),
            "not even an echo: the host's reply is the only copy"
        );
    }

    /// The receiving end of a `/give` run on the host's behalf.
    #[test]
    fn a_client_applies_the_items_the_host_grants_it() {
        let local = PlayerId(2);
        let (mut state, handle) = client_session(local);
        let bread = state.items.find("bread").unwrap();

        handle.deliver(Inbound::Update(ServerMessage::GrantItems {
            to: local,
            stacks: vec![NetItemStack {
                item: bread.0,
                count: 7,
                durability: None,
            }],
        }));
        state.pump_network(1.0 / 60.0);

        assert_eq!(state.inventory.count_of(bread), 7);
    }

    /// A grant addressed to someone else arrives on the wire only by accident,
    /// but must never land in our inventory if it does.
    #[test]
    fn a_client_ignores_a_grant_addressed_to_someone_else() {
        let (mut state, handle) = client_session(PlayerId(2));
        let bread = state.items.find("bread").unwrap();

        handle.deliver(Inbound::Update(ServerMessage::GrantItems {
            to: PlayerId(9),
            stacks: vec![NetItemStack {
                item: bread.0,
                count: 7,
                durability: None,
            }],
        }));
        state.pump_network(1.0 / 60.0);

        assert_eq!(state.inventory.count_of(bread), 0);
    }

    /// The content hash gates divergent builds, but a malformed message still
    /// must not index past the registry and panic.
    #[test]
    fn an_unknown_granted_item_id_is_skipped_not_fatal() {
        let local = PlayerId(2);
        let (mut state, handle) = client_session(local);

        handle.deliver(Inbound::Update(ServerMessage::GrantItems {
            to: local,
            stacks: vec![NetItemStack {
                item: u16::MAX,
                count: 4,
                durability: None,
            }],
        }));
        state.pump_network(1.0 / 60.0);

        assert!(state.inventory.slots().iter().all(Option::is_none));
    }

    #[test]
    fn help_lists_more_for_an_op_than_for_everyone_else() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/help".to_string());
        assert!(
            state.chat.log.lines().any(|l| l.text.contains("/give")),
            "the local player of a singleplayer world is always an op"
        );
    }

    #[test]
    fn an_unparseable_command_reports_itself_without_side_effects() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Creative);
        state.submit_chat("/weather clear".to_string());
        let last = state.chat.log.lines().next_back().expect("an error line");
        assert_eq!(last.kind, ChatKind::Error);
        assert!(last.text.contains("unknown command"), "got {:?}", last.text);
    }
}

//! Chat and commands for [`InGameState`]: who said what, who may run what, and
//! what a command does to the world.
//!
//! The split with [`crate::domain::chat`] is the usual one — that module decides *what*
//! a command means, this one owns the registries, the inventory and the session,
//! so it is where a `/give` actually lands. Concretely, this file holds
//! [`SessionContext`]: the real implementation of the
//! [`CommandContext`](crate::domain::chat::CommandContext) port that commands are
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
use crate::domain::chat::{
    self, ChatKind, ChatState, CommandContext, Invocation, ItemName, Permission, Position,
};
use crate::domain::core::BlockPos;
use crate::domain::inventory::{ItemId, ItemRegistry, ItemStack};
use crate::domain::progression::WorldProgression;
use crate::infrastructure::net::{
    Channel, ClientMessage, NetItemStack, NetVec3, PlayerId, ServerMessage,
};
use crate::presentation::ui::chat::ChatAction;
use std::path::Path;

impl InGameState {
    // --- Local input ---------------------------------------------------------------

    /// Draw the chat overlay and act on what the player did to it.
    pub(super) fn draw_chat(&mut self, egui_ctx: &egui::Context) {
        // Destructured so the log can be read while the draft is borrowed
        // mutably by the text widget.
        let ChatState { log, composer } = &mut self.chat;
        let focus = composer.take_focus_request();
        let outcome = crate::presentation::ui::chat::draw_chat(
            egui_ctx,
            log,
            composer.open,
            &mut composer.draft,
            focus,
        );
        // Handled before the input action, which on this same frame is the
        // `Cancel` that clicking away from the composer produced. Both are real:
        // the file opens *and* the composer closes.
        if let Some(path) = outcome.open {
            crate::infrastructure::desktop::show_file(&path);
        }
        let submitted = match outcome.action {
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

    /// Tell the player, and only the player, where a screenshot went.
    ///
    /// Pushed straight onto the local log rather than sent anywhere:
    /// [`ChatState`] is per-peer and never synced, so this is invisible to
    /// everyone else on the server by construction — and it would be meaningless
    /// to them anyway, since the path names a file on this machine.
    pub(super) fn note_screenshot(&mut self, path: &Path) {
        self.chat
            .log
            .push_link(ChatKind::System, "Saved screenshot as ", path.to_path_buf());
    }

    /// Handle a line the local player submitted on the chat bar.
    pub(super) fn submit_chat(&mut self, text: String) {
        let text = text.trim().to_string();
        if text.is_empty() {
            return;
        }
        if self.net.session.is_authority() {
            let me = self.net.session.local_id();
            self.dispatch_chat(me, text);
        } else {
            // Send it raw — commands *and* ordinary messages. Nothing is echoed
            // locally: the host's reply is the single copy, which is what keeps
            // a client from seeing its own message twice.
            self.net
                .session
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
        self.net.session.broadcast(
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
    /// is matched by the **verified account** the host recorded when they
    /// joined.
    ///
    /// A player with no recorded account is never an op. That is the important
    /// half: `peers.accounts` is only ever written from a checked ticket
    /// signature, so there is no way to be authorized without having proved who
    /// you are.
    fn is_op(&self, actor: PlayerId) -> bool {
        if actor == self.net.session.local_id() {
            return true;
        }
        self.net
            .peers
            .accounts
            .get(&actor)
            .is_some_and(|account| self.net.ops.is_op(&account.account_id))
    }

    /// Split `count` of `name` into stacks and hand them to `actor`.
    ///
    /// The mechanical half of `/give`: the command already validated the name
    /// against [`CommandContext::item_names`], so an unresolvable one here is a
    /// caller bug and fails soft.
    fn give_item(&mut self, actor: PlayerId, name: &str, count: u32) {
        let Some(id) = self.content.rules.items.find(name) else {
            log::warn!("asked to give unknown item '{name}'; ignoring");
            return;
        };
        let stacks = build_stacks(id, count, &self.content.rules.items);
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
        if actor == self.net.session.local_id() {
            self.sim.player.teleport(Vec3::from_array(position));
            self.sim.player.velocity = Vec3::ZERO;
            self.sim.breaking = None;
            return;
        }
        self.net.session.send_to(
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
        self.sim.player.teleport(Vec3::from_array(position));
        self.sim.player.velocity = Vec3::ZERO;
        self.sim.breaking = None;
    }

    // --- Role-agnostic effects ------------------------------------------------------

    /// Say something back to whoever ran the command: into our own log if that
    /// is us, otherwise addressed to them over the wire.
    pub(super) fn reply(&mut self, actor: PlayerId, kind: ChatKind, text: String) {
        if actor == self.net.session.local_id() {
            self.chat.log.push(kind, text);
        } else {
            self.net.session.send_to(
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
        if actor == self.net.session.local_id() {
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
        self.net.session.send_to(
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
            let leftover = self.sim.inventory.add(stack, &self.content.rules.items);
            if leftover > 0 {
                self.sim.throw(ItemStack {
                    count: leftover,
                    ..stack
                });
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
            .filter_map(|stack| stack_from_wire(*stack, &self.content.rules.items))
            .collect();
        self.receive_stacks(stacks);
    }

    /// Display name for a player id. Names are still generated rather than
    /// chosen, matching what `welcome_player` puts in the peer list.
    pub(super) fn player_name(&self, id: PlayerId) -> String {
        crate::application::ecs::systems::players::get(&self.sim.ecs, id)
            .map(|player| player.name.clone())
            .unwrap_or_else(|| format!("Player {}", id.0))
    }

    #[cfg(test)]
    pub(super) fn set_ops(&mut self, ops: crate::domain::chat::OpsList) {
        self.net.ops = ops;
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

    fn item_names(&self) -> Vec<ItemName> {
        self.state
            .content
            .rules
            .items
            .iter()
            .map(|(id, item)| ItemName {
                id: item.id.clone(),
                display: self.state.content.item_display_name(id).to_string(),
            })
            .collect()
    }

    fn give_item(&mut self, id: &str, count: u32) {
        self.state.give_item(self.actor, id, count);
    }

    fn position(&self) -> Position {
        match crate::application::ecs::systems::players::get(&self.state.sim.ecs, self.actor) {
            Some(player) => player.position().to_array(),
            None => self.state.sim.player.position.to_array(),
        }
    }

    fn teleport(&mut self, position: Position) {
        self.state.teleport(self.actor, position);
    }

    fn player_positions(&self) -> Vec<(String, Position)> {
        let local = self.state.net.session.local_id();
        // The local player only appears here when someone *else* is the runner —
        // a command never lists its own runner as a destination.
        let own = (self.actor != local).then(|| {
            (
                self.state.player_name(local),
                self.state.sim.player.position.to_array(),
            )
        });
        crate::application::ecs::systems::players::all(&self.state.sim.ecs)
            .filter(|player| player.id != self.actor)
            .map(|player| (player.name.clone(), player.position().to_array()))
            .chain(own)
            .collect()
    }

    fn structure_ids(&self) -> Vec<String> {
        let config = self.state.sim.structures.config();
        config.all().iter().map(|s| s.id.clone()).collect()
    }

    fn boss_ids(&self) -> Vec<String> {
        self.state
            .content
            .rules
            .entities
            .iter()
            .filter(|kind| kind.boss.is_some())
            .map(|kind| kind.name.clone())
            .collect()
    }

    fn locate(&self, structure: &str) -> Option<Position> {
        self.state
            .locate_structure(structure, self.position())
            .map(stand_on)
    }

    fn reveal(&mut self, structure: &str) -> Option<Position> {
        let anchor = self.state.locate_structure(structure, self.position())?;
        if self.state.sim.progression.reveal(structure, anchor) {
            self.state.announce_reveal(structure, anchor, None);
        }
        Some(stand_on(anchor))
    }

    fn defeat_boss(&mut self, boss: &str) -> bool {
        let news = self.state.sim.progression.defeat(boss);
        self.state.broadcast_progression();
        news
    }

    fn reset_progression(&mut self) {
        self.state.sim.progression.reset();
        self.state.broadcast_progression();
    }

    fn progression(&self) -> WorldProgression {
        self.state.sim.progression.clone()
    }
}

/// Where to stand on a structure anchored at `anchor`: the centre of the
/// block above its ground layer.
fn stand_on(anchor: BlockPos) -> Position {
    [
        anchor.x as f32 + 0.5,
        anchor.y as f32 + 1.0,
        anchor.z as f32 + 0.5,
    ]
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
mod tests;

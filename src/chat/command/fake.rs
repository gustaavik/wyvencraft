//! An in-memory [`CommandContext`] for tests: fixed answers, recorded effects.
//!
//! The third impl of the port, alongside the real one in
//! `state::ingame_state::chat` — the same shape as `FakeSession`, `MapSource`
//! and `InMemoryWorldRepository`. Because the port is four methods wide, a
//! command can be exercised end to end without a world, a registry, a socket or
//! a GPU: what `/give bread 12` *means* is testable in three lines.

use super::{CommandContext, ItemName, Position};
use crate::core::ident::title_case;
use crate::net::ChatKind;

/// A context that answers from fixed data and records everything done to it.
#[derive(Debug, Clone, Default)]
pub struct FakeContext {
    /// What [`CommandContext::is_op`] reports.
    pub is_op: bool,
    /// What [`CommandContext::item_names`] returns.
    pub items: Vec<ItemName>,
    /// Where the runner stands.
    pub position: Position,
    /// The other players in the session.
    pub players: Vec<(String, Position)>,
    /// Every reply, in order.
    pub replies: Vec<(ChatKind, String)>,
    /// Every `(item, count)` handed over, in order.
    pub given: Vec<(String, u32)>,
    /// Every destination teleported to, in order.
    pub teleports: Vec<Position>,
}

impl FakeContext {
    /// Build from bare ids, deriving each label the way `content` does — so a
    /// test says `["wooden_pickaxe"]` and still gets "Wooden Pickaxe" back.
    pub fn new<'a>(is_op: bool, items: impl IntoIterator<Item = &'a str>) -> Self {
        Self {
            is_op,
            items: items
                .into_iter()
                .map(|id| ItemName {
                    id: id.to_string(),
                    display: title_case(id),
                })
                .collect(),
            ..Self::default()
        }
    }

    /// Stand the runner at `position` (the anchor for relative coordinates).
    pub fn at(mut self, position: Position) -> Self {
        self.position = position;
        self
    }

    /// Put other players in the session, for destinations addressed by name.
    pub fn with_players<'a>(
        mut self,
        players: impl IntoIterator<Item = (&'a str, Position)>,
    ) -> Self {
        self.players = players
            .into_iter()
            .map(|(name, position)| (name.to_string(), position))
            .collect();
        self
    }

    /// Every reply of one kind, joined — for asserting on what the runner saw
    /// without pinning the exact number of lines.
    pub fn said(&self, kind: ChatKind) -> String {
        self.replies
            .iter()
            .filter(|(k, _)| *k == kind)
            .map(|(_, text)| text.as_str())
            .collect::<Vec<_>>()
            .join("\n")
    }
}

impl CommandContext for FakeContext {
    fn is_op(&self) -> bool {
        self.is_op
    }

    fn reply(&mut self, kind: ChatKind, text: String) {
        self.replies.push((kind, text));
    }

    fn item_names(&self) -> Vec<ItemName> {
        self.items.clone()
    }

    fn give_item(&mut self, id: &str, count: u32) {
        self.given.push((id.to_string(), count));
    }

    fn position(&self) -> Position {
        self.position
    }

    fn teleport(&mut self, position: Position) {
        self.position = position;
        self.teleports.push(position);
    }

    fn player_positions(&self) -> Vec<(String, Position)> {
        self.players.clone()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_records_what_a_command_did() {
        let mut ctx = FakeContext::new(true, ["bread"]);
        ctx.reply(ChatKind::System, "hello".to_string());
        ctx.reply(ChatKind::Error, "nope".to_string());
        ctx.give_item("bread", 3);

        assert!(ctx.is_op());
        assert_eq!(
            ctx.item_names(),
            [ItemName {
                id: "bread".into(),
                display: "Bread".into()
            }]
        );
        assert_eq!(ctx.given, [("bread".to_string(), 3)]);
        assert_eq!(ctx.said(ChatKind::System), "hello");
        assert_eq!(ctx.said(ChatKind::Error), "nope");
        assert_eq!(ctx.said(ChatKind::Player), "");
    }
}

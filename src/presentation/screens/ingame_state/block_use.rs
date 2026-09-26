//! Right-clicking a block that does something: reading a shrine's wayrune,
//! making an offering at a boss altar.
//!
//! The authority decides every use. The local player of a singleplayer or
//! hosted world acts directly; a client sends `ClientMessage::UseBlock` and
//! the host runs the same [`InGameState::use_block`] on its behalf, after
//! checking reach. Either way the structure is confirmed from the **seed**
//! ([`Structures::instance_at`](crate::domain::world::structure::Structures::instance_at)),
//! not from the block in the world, so a wayrune placed by hand is just a
//! carved stone and a client cannot conjure a shrine by naming a position.

use glam::Vec3;

use super::InGameState;
use crate::domain::core::BlockPos;
use crate::domain::core::ident::title_case;
use crate::domain::world::block::Interaction;
use crate::domain::world::structure::{Cell, Instance};
use crate::infrastructure::net::{ChatKind, PlayerId};

/// How many grid cells out a shrine searches for the structure it reveals.
const SHRINE_SEARCH_RINGS: i32 = 6;
/// Slack (blocks) on a client's reach, for the position it last reported
/// lagging the one it clicked from.
const REACH_SLACK: f32 = 2.0;

/// One block use, as the authority carries it out.
#[derive(Debug, Clone, Copy)]
pub(super) struct UseAt {
    /// Who used it.
    pub actor: PlayerId,
    /// The block used.
    pub pos: BlockPos,
    /// The structure the seed says the block belongs to.
    pub instance: Instance,
}

impl InGameState {
    /// Right-click on the targeted block, if it is one that does something.
    /// Returns whether the block claimed the click — which it does before any
    /// held item gets a say, so reading a shrine never places the block in
    /// your hand against it.
    pub(super) fn use_targeted_block(&mut self) -> bool {
        let Some(hit) = self.targeted_block() else {
            return false;
        };
        let block = self.world.block_at(hit.block);
        if self.content.rules.blocks.get(block).interact.is_none() {
            return false;
        }
        self.view.trigger_swing();
        if self.session.is_authority() {
            self.use_block(self.session.local_id(), hit.block);
        } else {
            self.request_block_use(hit.block);
        }
        true
    }

    /// Authority: whether `actor` stands close enough to use `pos`. The local
    /// player's reach was already enforced by the crosshair ray.
    pub(super) fn within_use_reach(&self, actor: PlayerId, pos: BlockPos) -> bool {
        if actor == self.session.local_id() {
            return true;
        }
        let Some(player) = self.peers.players.get(&actor) else {
            return false;
        };
        let centre = Vec3::new(pos.x as f32 + 0.5, pos.y as f32 + 0.5, pos.z as f32 + 0.5);
        let eye = player.position() + Vec3::Y * self.player.movement().eye_height;
        eye.distance(centre) <= self.player.movement().reach + REACH_SLACK
    }

    /// Authority: carry out `actor` using the block at `pos`.
    pub(super) fn use_block(&mut self, actor: PlayerId, pos: BlockPos) {
        if !self.within_use_reach(actor, pos) {
            log::info!("player {} tried to use {pos:?} out of reach", actor.0);
            return;
        }
        let Some((instance, Cell::Block(block))) = self.structures.instance_at(pos) else {
            self.reply(
                actor,
                ChatKind::System,
                "It is only carved stone — its power belongs to the old places.".into(),
            );
            return;
        };
        let Some(interaction) = self.content.rules.blocks.get(block).interact.clone() else {
            return;
        };
        let at = UseAt {
            actor,
            pos,
            instance,
        };
        match interaction {
            Interaction::Shrine => self.read_shrine(&at),
            Interaction::Altar { boss } => self.offer_at_altar(&at, &boss),
        }
    }

    /// Read a shrine: find the structure it points to, reveal it to everyone,
    /// and tell the reader which way to go.
    fn read_shrine(&mut self, at: &UseAt) {
        let config = self.structures.config();
        let Some(target) = config.get(at.instance.structure).reveals else {
            self.reply(at.actor, ChatKind::System, "The wayrune is blank.".into());
            return;
        };
        let target_id = config.get(target).id.clone();
        let name = title_case(&target_id);
        let Some(found) = self.structures.nearest(target, at.pos, SHRINE_SEARCH_RINGS) else {
            let text = format!("The runes are silent: no {name} lies within reach.");
            self.reply(at.actor, ChatKind::System, text);
            return;
        };
        let way = describe_way(at.pos, found.anchor);
        self.reply(
            at.actor,
            ChatKind::System,
            format!("The wayrune reveals the {name} — {way}."),
        );
        if self
            .progression
            .read_shrine(at.pos, &target_id, found.anchor)
        {
            self.announce_reveal(&target_id, found.anchor, Some(at.actor));
        }
    }
}

/// "342 blocks north-east" — distance and eight-point direction from `from`
/// to `to`. North is −z, Minecraft's convention.
pub(super) fn describe_way(from: BlockPos, to: BlockPos) -> String {
    let (dx, dz) = ((to.x - from.x) as f32, (to.z - from.z) as f32);
    let distance = dx.hypot(dz).round() as i32;
    if distance == 0 {
        return "right here".into();
    }
    const POINTS: [&str; 8] = [
        "north",
        "north-east",
        "east",
        "south-east",
        "south",
        "south-west",
        "west",
        "north-west",
    ];
    // Clockwise from north: north is −z, east is +x.
    let angle = dx.atan2(-dz).rem_euclid(std::f32::consts::TAU);
    let sector = ((angle / std::f32::consts::FRAC_PI_4).round() as usize) % 8;
    format!("{distance} blocks {}", POINTS[sector])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::session::FakeSession;
    use crate::domain::core::GameMode;
    use crate::domain::world::block::blocks;
    use crate::infrastructure::net::ServerMessage;
    use crate::presentation::content::GameContent;

    #[test]
    fn directions_follow_the_compass() {
        let o = BlockPos::new(0, 0, 0);
        assert_eq!(
            describe_way(o, BlockPos::new(0, 5, -100)),
            "100 blocks north"
        );
        assert_eq!(describe_way(o, BlockPos::new(100, 0, 0)), "100 blocks east");
        assert_eq!(
            describe_way(o, BlockPos::new(-70, 0, 70)),
            "99 blocks south-west"
        );
        assert_eq!(describe_way(o, o), "right here");
    }

    /// The wayrune of the nearest shrine to spawn in `state`'s world.
    fn nearest_wayrune(state: &InGameState) -> BlockPos {
        let config = state.structures.config();
        let shrine = config.find("meadows_shrine").unwrap();
        let instance = state
            .structures
            .nearest(shrine, BlockPos::new(0, 0, 0), 4)
            .expect("a shrine near spawn");
        let template = &config.get(shrine).template;
        for dx in -3..=3 {
            for dz in -3..=3 {
                for dy in 0..4 {
                    if template.cell(instance.rot, dx, dy, dz) == Cell::Block(blocks::WAYRUNE) {
                        let a = instance.anchor;
                        return BlockPos::new(a.x + dx, a.y + dy, a.z + dz);
                    }
                }
            }
        }
        panic!("the shrine template has no wayrune");
    }

    #[test]
    fn reading_a_shrine_reveals_the_nearest_altar() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let rune = nearest_wayrune(&state);
        state.use_block(PlayerId(0), rune);
        let revealed: Vec<_> = state.progression.revealed().collect();
        assert_eq!(revealed.len(), 1);
        assert_eq!(revealed[0].0, "meadows_altar");
        assert!(state.progression.read_shrines.contains(&rune));
    }

    #[test]
    fn a_hand_placed_wayrune_reveals_nothing() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        state.use_block(PlayerId(0), BlockPos::new(3, 150, 3));
        assert_eq!(state.progression.revealed().count(), 0);
    }

    /// A client asks; the host checks the client really stands at the shrine
    /// before revealing anything, then tells everyone.
    #[test]
    fn a_clients_shrine_use_is_reach_checked_and_broadcast() {
        let mut state = InGameState::new(GameContent::builtin(), 5, GameMode::Survival);
        let session = FakeSession::host();
        let handle = session.handle();
        state.set_session(Box::new(session));
        let rune = nearest_wayrune(&state);
        let pid = PlayerId(1);

        // Far away: refused.
        state.peers.entry(pid, Vec3::new(5000.0, 90.0, 5000.0));
        state.use_block(pid, rune);
        assert_eq!(state.progression.revealed().count(), 0);

        // Standing beside it: revealed, and everyone hears.
        let beside = Vec3::new(
            rune.x as f32 + 1.5,
            rune.y as f32 - 1.0,
            rune.z as f32 + 0.5,
        );
        state.peers.players.remove(&pid);
        state.peers.entry(pid, beside);
        state.use_block(pid, rune);
        assert_eq!(state.progression.revealed().count(), 1);
        let guard = handle.lock();
        let sent = guard.broadcasts();
        assert!(
            sent.iter()
                .any(|m| matches!(m, ServerMessage::Progression(p) if p.revealed().count() == 1))
        );
        assert!(
            sent.iter()
                .any(|m| matches!(m, ServerMessage::Revealed { by: Some(id), .. } if *id == pid))
        );
    }
}

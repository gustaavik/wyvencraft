//! Other players: kept as entities, found by [`PlayerId`], animated from what
//! their snapshots show.
//!
//! The *local* player is not here. It is the one entity every frame reads a
//! hundred times — the camera, the input, the inventory and the HUD all hang
//! off it — so the session keeps it as a singleton (a "resource", in ECS
//! terms) rather than looking it up by handle at every one of those sites.

use wyven_net::PlayerId;

use super::mobs::animate_observed;
use crate::application::ecs::components::{Animation, LastSeen, RemotePlayer};
use crate::application::ecs::{Ecs, Entity, spawn};

/// The entity standing for player `id`, if this peer knows them. A linear
/// scan: a session holds a handful of players.
pub fn find(ecs: &Ecs, id: PlayerId) -> Option<Entity> {
    ecs.query::<(&RemotePlayer,)>()
        .find(|(_, (player,))| player.id == id)
        .map(|(entity, _)| entity)
}

pub fn get(ecs: &Ecs, id: PlayerId) -> Option<&RemotePlayer> {
    ecs.get::<RemotePlayer>(find(ecs, id)?)
}

pub fn get_mut(ecs: &mut Ecs, id: PlayerId) -> Option<&mut RemotePlayer> {
    let entity = find(ecs, id)?;
    ecs.get_mut::<RemotePlayer>(entity)
}

/// Player `id`, created on first mention — named after their id and standing
/// at `fallback` until the message that introduces them properly arrives.
pub fn entry(ecs: &mut Ecs, id: PlayerId, fallback: glam::Vec3) -> &mut RemotePlayer {
    let entity = match find(ecs, id) {
        Some(entity) => entity,
        None => spawn::remote_player(
            ecs,
            RemotePlayer::new(id, format!("Player {}", id.0), fallback),
        ),
    };
    ecs.get_mut::<RemotePlayer>(entity)
        .expect("a remote-player entity carries a RemotePlayer")
}

/// Forget player `id`. False if they were not known.
pub fn remove(ecs: &mut Ecs, id: PlayerId) -> bool {
    find(ecs, id).is_some_and(|entity| ecs.despawn(entity))
}

/// Every other player this peer knows.
pub fn all(ecs: &Ecs) -> impl Iterator<Item = &RemotePlayer> {
    ecs.query::<(&RemotePlayer,)>().map(|(_, (player,))| player)
}

/// Animate every other player from how their *drawn* body moved — the
/// interpolated position at `alpha`, the same one their nameplate follows —
/// so a jump that lands between two snapshots does not flicker across the
/// airborne threshold.
pub fn animate(ecs: &mut Ecs, dt: f32, alpha: f32, max_speed: f32) {
    ecs.for_each_mut::<(&RemotePlayer, &mut Animation, &mut LastSeen), _>(
        |_, (player, anim, seen)| {
            let drawn = player.interpolated_position(alpha);
            animate_observed(&mut anim.0, &mut seen.0, drawn, player.yaw, dt, max_speed);
        },
    );
}

#[cfg(test)]
mod tests {
    use glam::Vec3;

    use super::*;

    #[test]
    fn entry_creates_once_and_finds_after() {
        let mut ecs = Ecs::new();
        entry(&mut ecs, PlayerId(3), Vec3::ONE).name = "gustav".into();
        assert_eq!(entry(&mut ecs, PlayerId(3), Vec3::ZERO).name, "gustav");
        assert_eq!(all(&ecs).count(), 1);
        assert_eq!(get(&ecs, PlayerId(3)).unwrap().position(), Vec3::ONE);
        assert!(get(&ecs, PlayerId(4)).is_none());
    }

    #[test]
    fn remove_forgets_a_player() {
        let mut ecs = Ecs::new();
        entry(&mut ecs, PlayerId(1), Vec3::ZERO);
        assert!(remove(&mut ecs, PlayerId(1)));
        assert!(!remove(&mut ecs, PlayerId(1)));
        assert!(find(&ecs, PlayerId(1)).is_none());
    }

    #[test]
    fn a_walking_player_animates_from_their_snapshots() {
        let mut ecs = Ecs::new();
        for i in 1..=30 {
            let at = Vec3::new(i as f32 * 0.07, 70.0, 0.0);
            entry(&mut ecs, PlayerId(1), Vec3::new(0.0, 70.0, 0.0)).push_snapshot(at, 0.0, 0.0);
            animate(&mut ecs, 1.0 / 60.0, 1.0, 12.0);
        }
        let entity = find(&ecs, PlayerId(1)).unwrap();
        assert!(ecs.get::<Animation>(entity).unwrap().0.speed() > 1.0);
    }
}

//! The components entities are made of.
//!
//! Shared bodies ([`Transform`], [`Velocity`], [`Body`], [`Animation`]) sit
//! beside the domain types that say what an entity *is* — an
//! [`ItemDrop`], a [`Projectile`], a [`Mob`] with its [`Health`] and, on a
//! boss, its [`Boss`]. A client's copy of a host-simulated mob carries
//! [`Replica`] in place of the mob's mind.

use glam::Vec3;

use crate::domain::core::Aabb;
use crate::domain::entity::AnimationState;
use crate::domain::entity::brain::Perception;
use crate::domain::entity::dropped_item::centred_aabb;
use crate::domain::entity::kind::{PhysicsParams, VisualSpec};
use crate::domain::entity::mob::feet_aabb;

pub use crate::domain::entity::{Boss, Health, Intent, ItemDrop, Mob, MobId, Projectile};

/// Where an entity is and which way it faces.
///
/// `position` is the body's anchor — see [`Anchor`].
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Transform {
    pub position: Vec3,
    /// Radians about +Y, 0 = facing -Z.
    pub yaw: f32,
    pub pitch: f32,
}

impl Transform {
    pub fn at(position: Vec3) -> Self {
        Self {
            position,
            ..Self::default()
        }
    }
}

/// Blocks per second.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Velocity(pub Vec3);

/// Which point of its box a body's [`Transform::position`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Anchor {
    /// The centre: small free bodies — drops.
    Centre,
    /// The middle of the bottom face: anything that stands — mobs, players.
    Feet,
}

/// A collision body, tuned by its entity kind's `[entity.physics]`.
#[derive(Debug, Clone, Copy)]
pub struct Body {
    pub physics: PhysicsParams,
    pub anchor: Anchor,
}

impl Body {
    pub fn centred(physics: PhysicsParams) -> Self {
        Self {
            physics,
            anchor: Anchor::Centre,
        }
    }

    pub fn standing(physics: PhysicsParams) -> Self {
        Self {
            physics,
            anchor: Anchor::Feet,
        }
    }

    /// The collision box of this body at `position`.
    pub fn aabb(&self, position: Vec3) -> Aabb {
        match self.anchor {
            Anchor::Centre => centred_aabb(position, &self.physics),
            Anchor::Feet => feet_aabb(position, &self.physics),
        }
    }
}

/// Which entity kind this is (`entities.toml` name — the save and wire
/// identity) and how it is drawn.
#[derive(Debug, Clone)]
pub struct Kind {
    pub name: String,
    pub visual: VisualSpec,
}

/// Procedural walk/idle/swing animation.
#[derive(Debug, Clone, Copy)]
pub struct Animation(pub AnimationState);

/// What a mob perceived this frame, and which of the caller's targets that is.
/// `None` for a mob sitting the frame out — one in an unloaded chunk, which
/// every later mob pass then skips, cooldowns and all.
#[derive(Debug, Clone, Copy, Default)]
pub struct Sensed(pub Option<Sight>);

/// One mob's perception this frame.
#[derive(Debug, Clone, Copy)]
pub struct Sight {
    pub perception: Perception,
    /// Index into the target list the perceive pass was given.
    pub target: Option<usize>,
}

/// This frame's decision, handed from the think pass to the ones after it.
#[derive(Debug, Clone, Copy, Default)]
pub struct Decision(pub Intent);

/// A client's copy of a host-simulated mob: moved by snapshots, animated from
/// the movement it observes (see [`LastSeen`]).
#[derive(Debug, Clone, Copy, Default)]
pub struct Replica {
    /// A boss's fight phase as the host last reported it (`BossPhase`).
    pub phase: u8,
}

/// Where an entity animated from *observed* movement was drawn last frame —
/// a mob replica or another player, whose speed is read off the difference
/// because only positions cross the wire.
#[derive(Debug, Clone, Copy)]
pub struct LastSeen(pub Vec3);

pub use crate::application::sync::RemotePlayer;

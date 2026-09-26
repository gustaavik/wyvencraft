//! Bosses: mobs with an `[entity.boss]` component — a title, an arena, an
//! offering that summons them, and a phased move set.
//!
//! - [`params`] — the component as `entities.toml` spells it, validated at
//!   load so a phase naming a missing attack rejects the file.
//! - [`brain`] — the pure decision-maker: which attack to wind up and when,
//!   and when the fight crosses into its next phase. Like
//!   [`crate::entity::brain::MobBrain`] it perceives nothing itself and is
//!   seeded, so a fight replays identically under test.
//!
//! Locomotion is still the ordinary hostile [`MobBrain`](crate::entity::brain::MobBrain)
//! chase; a boss only replaces *how it attacks*.

pub mod brain;
pub mod params;

pub use brain::{BossBrain, BossMove, BossSense, BossThought};
pub use params::{AttackEffect, BossAttack, BossParams, BossPhase, Offering};

//! The mob decision layer: a small explicit state machine that turns what a
//! mob perceives into what it wants to do.
//!
//! Deliberately pure — no world access, no globals, no wall-clock. The caller
//! builds a [`Perception`] (which requires the world: distances, line of
//! sight), the brain returns an [`Intent`], and the mob body applies it with
//! real physics. Randomness comes from an injected seeded [`Rng64`], so the
//! same seed and the same perceptions replay the same decisions — this is
//! what makes AI behavior unit-testable.
//!
//! One brain drives every species: differences (hostility, ranges, speeds)
//! come entirely from the kind's [`MobParams`] — dispatch on components,
//! never on identity.

use glam::Vec3;

use crate::core::Rng64;
use crate::entity::kind::{Behavior, MobParams};

/// Seconds a hostile keeps chasing after losing sight of its target.
const CHASE_MEMORY: f32 = 4.0;
/// How long a hurt passive mob runs before calming down.
const FLEE_SECONDS: f32 = 4.0;
/// Idle stand time range (s) between wanders.
const IDLE_RANGE: (f32, f32) = (1.0, 4.0);
/// Wander walk time range (s) before standing again.
const WANDER_RANGE: (f32, f32) = (2.0, 5.0);

/// What a mob currently knows about the nearest attackable player.
#[derive(Debug, Clone, Copy)]
pub struct PlayerSighting {
    /// Vector from the mob's eye to the target's eye.
    pub offset: Vec3,
    pub distance: f32,
    /// Line of sight: no solid block between the eyes.
    pub visible: bool,
}

/// Everything the brain is allowed to know for one decision.
#[derive(Debug, Clone, Copy)]
pub struct Perception {
    pub on_ground: bool,
    /// The nearest player that could be attacked, if any is in play.
    pub target: Option<PlayerSighting>,
    /// The mob took damage since the last decision.
    pub hurt: bool,
}

/// How fast the mob wants to move.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gait {
    Stand,
    Walk,
    Run,
}

/// One decision: where to face, how to move, whether to attack.
#[derive(Debug, Clone, Copy)]
pub struct Intent {
    /// Desired facing (radians about Y, 0 = -Z); `None` keeps the current yaw.
    pub yaw: Option<f32>,
    pub gait: Gait,
    /// Move opposite the facing (a ranged mob backing away, still aiming).
    pub backward: bool,
    /// Attack the perceived target now (the body still gates the cooldown).
    pub attack: bool,
}

impl Intent {
    fn stand() -> Self {
        Self {
            yaw: None,
            gait: Gait::Stand,
            backward: false,
            attack: false,
        }
    }
}

#[derive(Debug, Clone, Copy)]
enum BrainState {
    /// Standing still until the clock passes `until`.
    Idle { until: f32 },
    /// Walking along `yaw` until the clock passes `until`.
    Wander { yaw: f32, until: f32 },
    /// Pursuing the target; `unseen` accumulates time without line of sight.
    Chase { unseen: f32 },
    /// Running along `yaw` (away from the attacker) until `until`.
    Flee { yaw: f32, until: f32 },
}

/// Per-mob decision state. Cheap to create; seeded once at spawn.
#[derive(Debug, Clone)]
pub struct MobBrain {
    state: BrainState,
    /// Accumulated decision time (s since spawn).
    clock: f32,
    rng: Rng64,
}

/// Facing that points along `offset` (horizontal), matching the engine's
/// yaw convention: forward = `(sin yaw, 0, -cos yaw)`.
pub fn yaw_toward(offset: Vec3) -> f32 {
    offset.x.atan2(-offset.z)
}

impl MobBrain {
    pub fn new(seed: u64) -> Self {
        Self {
            state: BrainState::Idle { until: 0.0 },
            clock: 0.0,
            rng: Rng64::new(seed),
        }
    }

    /// Decide what to do for the next `dt` seconds.
    pub fn think(&mut self, p: &Perception, cfg: &MobParams, dt: f32) -> Intent {
        self.clock += dt;

        // Threat responses preempt whatever the mob was doing.
        match cfg.behavior {
            // A fixture, not a creature: it makes no decisions at all.
            // Returning before the state machine runs is what keeps it from
            // picking a wander heading or spinning to face whoever hit it —
            // `Intent::stand` leaves `yaw` as `None`, so the body never turns
            // it. Being immovable is a separate question, answered by
            // `knockback_resistance`.
            Behavior::Inert => return Intent::stand(),
            Behavior::Hostile => {
                // Aggro on a visible player in range, or on whoever just hit us.
                let aggro = p
                    .target
                    .is_some_and(|t| (t.visible && t.distance <= cfg.aggro_range) || p.hurt);
                if aggro && !matches!(self.state, BrainState::Chase { .. }) {
                    self.state = BrainState::Chase { unseen: 0.0 };
                }
            }
            Behavior::Passive if p.hurt => {
                // Passive mobs bolt directly away from the attacker.
                let yaw = match p.target {
                    Some(t) => yaw_toward(-t.offset),
                    None => self.rng.range_f32(0.0, std::f32::consts::TAU),
                };
                self.state = BrainState::Flee {
                    yaw,
                    until: self.clock + FLEE_SECONDS,
                };
            }
            Behavior::Passive => {}
        }

        match self.state {
            BrainState::Idle { until } => {
                if self.clock >= until {
                    let yaw = self.rng.range_f32(0.0, std::f32::consts::TAU);
                    let until = self.clock + self.rng.range_f32(WANDER_RANGE.0, WANDER_RANGE.1);
                    self.state = BrainState::Wander { yaw, until };
                }
                Intent::stand()
            }
            BrainState::Wander { yaw, until } => {
                if self.clock >= until {
                    let until = self.clock + self.rng.range_f32(IDLE_RANGE.0, IDLE_RANGE.1);
                    self.state = BrainState::Idle { until };
                    return Intent::stand();
                }
                Intent {
                    yaw: Some(yaw),
                    gait: Gait::Walk,
                    backward: false,
                    attack: false,
                }
            }
            BrainState::Chase { unseen } => {
                let Some(target) = p.target else {
                    self.state = BrainState::Idle { until: self.clock };
                    return Intent::stand();
                };
                // Track how long the target has been out of sight/range and
                // give up once the memory window runs out.
                let in_view = target.visible && target.distance <= cfg.aggro_range;
                let unseen = if in_view { 0.0 } else { unseen + dt };
                if unseen > CHASE_MEMORY {
                    self.state = BrainState::Idle { until: self.clock };
                    return Intent::stand();
                }
                self.state = BrainState::Chase { unseen };

                let yaw = Some(yaw_toward(target.offset));
                let attack = target.visible && target.distance <= cfg.attack_range;
                if let Some(ranged) = &cfg.ranged {
                    // Ranged: hold the firing band — retreat when crowded,
                    // advance when out of range, otherwise stand and shoot.
                    let (gait, backward) = if target.distance < ranged.keep_distance {
                        (Gait::Walk, true)
                    } else if target.distance > cfg.attack_range {
                        (Gait::Run, false)
                    } else {
                        (Gait::Stand, false)
                    };
                    Intent {
                        yaw,
                        gait,
                        backward,
                        attack,
                    }
                } else {
                    // Melee: close the gap, stop pushing once in reach.
                    let gait = if attack { Gait::Stand } else { Gait::Run };
                    Intent {
                        yaw,
                        gait,
                        backward: false,
                        attack,
                    }
                }
            }
            BrainState::Flee { yaw, until } => {
                if self.clock >= until {
                    self.state = BrainState::Idle { until: self.clock };
                    return Intent::stand();
                }
                Intent {
                    yaw: Some(yaw),
                    gait: Gait::Run,
                    backward: false,
                    attack: false,
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::EntityRegistry;

    fn params(kind: &str) -> MobParams {
        EntityRegistry::builtin()
            .find(kind)
            .and_then(|k| k.mob.clone())
            .expect("builtin mob kind")
    }

    fn calm() -> Perception {
        Perception {
            on_ground: true,
            target: None,
            hurt: false,
        }
    }

    fn seen(distance: f32) -> Perception {
        Perception {
            on_ground: true,
            target: Some(PlayerSighting {
                offset: Vec3::new(0.0, 0.0, -distance),
                distance,
                visible: true,
            }),
            hurt: false,
        }
    }

    /// Step a brain with a constant perception, returning every intent.
    fn run(brain: &mut MobBrain, p: Perception, cfg: &MobParams, steps: usize) -> Vec<Intent> {
        (0..steps).map(|_| brain.think(&p, cfg, 0.1)).collect()
    }

    #[test]
    fn same_seed_replays_the_same_decisions() {
        let cfg = params("cow");
        let mut a = MobBrain::new(7);
        let mut b = MobBrain::new(7);
        for _ in 0..200 {
            let (ia, ib) = (a.think(&calm(), &cfg, 0.1), b.think(&calm(), &cfg, 0.1));
            assert_eq!(ia.yaw, ib.yaw);
            assert_eq!(ia.gait, ib.gait);
        }
    }

    #[test]
    fn calm_mobs_alternate_idle_and_wander() {
        let cfg = params("cow");
        let mut brain = MobBrain::new(3);
        let intents = run(&mut brain, calm(), &cfg, 300); // 30 s
        assert!(
            intents.iter().any(|i| i.gait == Gait::Walk),
            "should wander"
        );
        assert!(intents.iter().any(|i| i.gait == Gait::Stand), "should idle");
        assert!(
            intents.iter().all(|i| !i.attack && i.gait != Gait::Run),
            "calm mobs never attack or run"
        );
    }

    #[test]
    fn hostiles_chase_visible_players_and_attack_in_reach() {
        let cfg = params("zombie");
        let mut brain = MobBrain::new(1);
        // In aggro range but out of reach: run at the player, don't swing yet.
        let far = brain.think(&seen(10.0), &cfg, 0.1);
        assert_eq!(far.gait, Gait::Run);
        assert!(!far.attack);
        let expected = yaw_toward(Vec3::new(0.0, 0.0, -10.0));
        assert_eq!(far.yaw, Some(expected), "faces the target");
        // In reach: stop and swing.
        let close = brain.think(&seen(1.0), &cfg, 0.1);
        assert_eq!(close.gait, Gait::Stand);
        assert!(close.attack);
    }

    #[test]
    fn hostiles_ignore_players_beyond_aggro_range() {
        let cfg = params("zombie");
        let mut brain = MobBrain::new(1);
        let intent = brain.think(&seen(cfg.aggro_range + 5.0), &cfg, 0.1);
        assert!(!intent.attack);
        assert_ne!(intent.gait, Gait::Run, "no chase before aggro");
    }

    #[test]
    fn chase_persists_briefly_without_line_of_sight_then_lapses() {
        let cfg = params("zombie");
        let mut brain = MobBrain::new(1);
        brain.think(&seen(8.0), &cfg, 0.1); // acquire
        let mut hidden = seen(8.0);
        hidden.target.as_mut().unwrap().visible = false;
        // Just after losing sight the zombie keeps coming...
        let intent = brain.think(&hidden, &cfg, 0.1);
        assert_eq!(intent.gait, Gait::Run, "memory keeps the chase alive");
        // ...but gives up once the memory window passes.
        for _ in 0..((CHASE_MEMORY / 0.1) as usize + 2) {
            brain.think(&hidden, &cfg, 0.1);
        }
        let intent = brain.think(&hidden, &cfg, 0.1);
        assert_ne!(intent.gait, Gait::Run, "chase should lapse");
    }

    #[test]
    fn ranged_mobs_hold_the_firing_band() {
        let cfg = params("skeleton");
        let keep = cfg.ranged.unwrap().keep_distance;
        let mut brain = MobBrain::new(1);
        // Too close: back away while still facing (and firing at) the target.
        let crowded = brain.think(&seen(keep - 2.0), &cfg, 0.1);
        assert!(crowded.backward, "backs away inside keep_distance");
        assert!(crowded.attack, "still firing while retreating");
        assert_eq!(crowded.yaw, Some(yaw_toward(Vec3::new(0.0, 0.0, -1.0))));
        // In the band: stand and fire.
        let banded = brain.think(&seen(keep + 1.0), &cfg, 0.1);
        assert_eq!(banded.gait, Gait::Stand);
        assert!(banded.attack && !banded.backward);
        // Beyond firing range: advance.
        let far = brain.think(&seen(cfg.attack_range + 3.0), &cfg, 0.1);
        assert_eq!(far.gait, Gait::Run);
        assert!(!far.attack);
    }

    #[test]
    fn hurt_passives_flee_away_from_the_attacker() {
        let cfg = params("cow");
        let mut brain = MobBrain::new(1);
        let mut p = seen(3.0);
        p.hurt = true;
        let intent = brain.think(&p, &cfg, 0.1);
        assert_eq!(intent.gait, Gait::Run);
        // Attacker is at -Z; flight is toward +Z. Compare as directions to
        // dodge the ±π seam in the raw angles.
        let yaw = intent.yaw.expect("flee has a heading");
        let (fx, fz) = (yaw.sin(), -yaw.cos());
        assert!(
            fz > 0.99 && fx.abs() < 0.01,
            "should run to +Z: ({fx}, {fz})"
        );
        // Keeps running (without further hurt) for the flee window, then calms.
        let later = run(&mut brain, seen(3.0), &cfg, 50); // 5 s > FLEE_SECONDS
        assert!(later[..20].iter().all(|i| i.gait == Gait::Run));
        assert!(later.last().unwrap().gait != Gait::Run, "calms down");
    }

    #[test]
    fn hurt_hostiles_aggro_even_without_line_of_sight() {
        let cfg = params("zombie");
        let mut brain = MobBrain::new(1);
        let mut p = seen(30.0); // beyond aggro range
        p.target.as_mut().unwrap().visible = false;
        p.hurt = true;
        let intent = brain.think(&p, &cfg, 0.1);
        assert_eq!(intent.gait, Gait::Run, "retaliates against the attacker");
    }

    /// An inanimate kind never decides anything: no wander heading, no turn
    /// toward an attacker, no gait. `yaw: None` is the load-bearing part — the
    /// body only rewrites its facing when the brain asks for one.
    #[test]
    fn inert_kinds_never_act() {
        let cfg = params("vine sword");
        assert_eq!(cfg.behavior, Behavior::Inert, "the prop should be inert");
        let mut brain = MobBrain::new(7);

        // Long enough to pass several idle/wander transitions for a live mob.
        for intent in run(&mut brain, calm(), &cfg, 400) {
            assert_eq!(intent.yaw, None, "must never turn");
            assert_eq!(intent.gait, Gait::Stand);
            assert!(!intent.attack);
        }

        // Being hit, with the attacker in plain view, changes nothing.
        let mut hit = seen(1.5);
        hit.hurt = true;
        let intent = brain.think(&hit, &cfg, 0.1);
        assert_eq!(intent.yaw, None, "must not spin to face the attacker");
        assert_eq!(intent.gait, Gait::Stand);
    }

    /// The same brain with the flag cleared *does* wander, so the test above is
    /// measuring the flag rather than a mob that happens to sit still.
    #[test]
    fn the_same_kind_wanders_once_it_is_animate() {
        let mut cfg = params("vine sword");
        cfg.behavior = Behavior::Passive;
        cfg.walk_speed = 1.0;
        let mut brain = MobBrain::new(7);
        let intents = run(&mut brain, calm(), &cfg, 400);
        assert!(
            intents.iter().any(|i| i.yaw.is_some()),
            "an animate kind picks wander headings"
        );
    }
}

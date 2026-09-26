//! A boss's attack decisions: pure, seeded, and blind — facts and `dt` in, a
//! move out, exactly like [`crate::entity::brain::MobBrain`].
//!
//! Each attack runs *wind-up → release → recovery*. Committing to an attack
//! starts its wind-up (every peer shows the telegraph for that long), the
//! release is when it lands, and a short recovery follows before anything
//! new is chosen, so a boss reads as a sequence of dodgeable beats rather
//! than a stream of hits.

use crate::core::Rng64;

use super::params::BossParams;

/// Pause after every release before the next wind-up may start.
const RECOVERY_SECONDS: f32 = 0.6;
/// A fresh boss catches its breath before its first move.
const OPENING_SECONDS: f32 = 1.5;

/// What the boss knows this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BossSense {
    /// Current health over maximum, `0..=1`.
    pub health_fraction: f32,
    /// Distance to the target it is chasing, if it can see one.
    pub target_distance: Option<f32>,
}

/// What the boss does with its attacks this frame.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum BossMove {
    /// Nothing new: chasing, recovering, or mid-wind-up.
    Idle,
    /// Committed to `attack` (an index into the boss's attacks); it lands in
    /// `seconds`.
    Windup { attack: usize, seconds: f32 },
    /// `attack` lands now.
    Release { attack: usize },
}

/// One frame's decision.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct BossThought {
    pub action: BossMove,
    /// Set on the frame the fight enters a new phase.
    pub new_phase: Option<u8>,
}

#[derive(Debug, Clone)]
pub struct BossBrain {
    phase: u8,
    /// Seconds until each attack (by index) may be chosen again.
    cooldowns: Vec<f32>,
    /// The attack being wound up, and the seconds left until it lands.
    winding: Option<(usize, f32)>,
    /// Seconds until a new attack may be chosen at all.
    recovery: f32,
    rng: Rng64,
}

impl BossBrain {
    pub fn new(params: &BossParams, seed: u64) -> Self {
        Self {
            phase: 0,
            cooldowns: vec![0.0; params.attacks.len()],
            winding: None,
            recovery: OPENING_SECONDS,
            rng: Rng64::new(seed),
        }
    }

    pub fn phase(&self) -> u8 {
        self.phase
    }

    /// Whether an attack is being wound up — the boss plants itself while it
    /// is, so the telegraph points where the blow will land.
    pub fn is_winding(&self) -> bool {
        self.winding.is_some()
    }

    /// The speed multiplier of the current phase.
    pub fn speed(&self, params: &BossParams) -> f32 {
        params.phases[usize::from(self.phase)].speed
    }

    pub fn think(&mut self, sense: &BossSense, params: &BossParams, dt: f32) -> BossThought {
        for cooldown in &mut self.cooldowns {
            *cooldown = (*cooldown - dt).max(0.0);
        }
        self.recovery = (self.recovery - dt).max(0.0);

        // Phases only ever advance: healing back above a threshold does not
        // take the enraged moves away again.
        let phase = params
            .phase_at(sense.health_fraction)
            .max(usize::from(self.phase)) as u8;
        let new_phase = (phase != self.phase).then_some(phase);
        self.phase = phase;

        if let Some((attack, left)) = self.winding {
            let left = left - dt;
            if left > 0.0 {
                self.winding = Some((attack, left));
                return BossThought {
                    action: BossMove::Idle,
                    new_phase,
                };
            }
            return BossThought {
                action: self.release(attack, params),
                new_phase,
            };
        }

        let action = match sense.target_distance {
            Some(distance) if self.recovery <= 0.0 => self.choose(distance, params),
            _ => BossMove::Idle,
        };
        BossThought { action, new_phase }
    }

    /// Pick an available attack for a target `distance` away, weighted.
    fn choose(&mut self, distance: f32, params: &BossParams) -> BossMove {
        let phase = &params.phases[usize::from(self.phase)];
        let candidates: Vec<usize> = phase
            .attack_indices
            .iter()
            .copied()
            .filter(|&i| self.cooldowns[i] <= 0.0 && distance <= params.attacks[i].range)
            .collect();
        let weights: Vec<u32> = candidates
            .iter()
            .map(|&i| params.attacks[i].weight)
            .collect();
        let Some(pick) = self.rng.pick_weighted(&weights) else {
            return BossMove::Idle;
        };
        let attack = candidates[pick];
        let seconds = params.attacks[attack].windup;
        if seconds <= 0.0 {
            return self.release(attack, params);
        }
        self.winding = Some((attack, seconds));
        BossMove::Windup { attack, seconds }
    }

    fn release(&mut self, attack: usize, params: &BossParams) -> BossMove {
        self.winding = None;
        self.cooldowns[attack] = params.attacks[attack].cooldown;
        self.recovery = RECOVERY_SECONDS;
        BossMove::Release { attack }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn params() -> BossParams {
        let mut boss: BossParams = toml::from_str(
            r#"
            title = "Test"
            arena_radius = 30
            offering = { item = "x" }

            [[attack]]
            id = "bite"
            range = 3
            cooldown = 2
            windup = 0.5
            effect = { kind = "melee", damage = 5 }

            [[attack]]
            id = "spit"
            range = 20
            cooldown = 4
            windup = 0.8
            effect = { kind = "volley", damage = 2, count = 3, spread_deg = 20, speed = 15 }

            [[phase]]
            below = 1.0
            attacks = ["bite"]

            [[phase]]
            below = 0.5
            attacks = ["bite", "spit"]
            speed = 1.5
            "#,
        )
        .unwrap();
        boss.validate("test").unwrap();
        boss
    }

    const DT: f32 = 1.0 / 20.0;

    fn sense(health: f32, distance: Option<f32>) -> BossSense {
        BossSense {
            health_fraction: health,
            target_distance: distance,
        }
    }

    /// Run `seconds` of fight, collecting every non-idle move.
    fn run(brain: &mut BossBrain, p: &BossParams, s: BossSense, seconds: f32) -> Vec<BossMove> {
        (0..(seconds / DT) as usize)
            .map(|_| brain.think(&s, p, DT).action)
            .filter(|m| *m != BossMove::Idle)
            .collect()
    }

    #[test]
    fn every_release_follows_its_windup() {
        let p = params();
        let mut brain = BossBrain::new(&p, 7);
        let moves = run(&mut brain, &p, sense(0.3, Some(2.0)), 30.0);
        assert!(moves.len() > 4);
        let mut pending: Option<usize> = None;
        for m in moves {
            match m {
                BossMove::Windup { attack, .. } => {
                    assert!(pending.is_none(), "two wind-ups at once");
                    pending = Some(attack);
                }
                BossMove::Release { attack } => {
                    assert_eq!(pending.take(), Some(attack), "release without its wind-up");
                }
                BossMove::Idle => unreachable!(),
            }
        }
    }

    #[test]
    fn attacks_outside_the_phase_are_never_chosen() {
        let p = params();
        let mut brain = BossBrain::new(&p, 3);
        // Healthy and far away: phase one has only the bite, which cannot reach.
        assert!(run(&mut brain, &p, sense(1.0, Some(10.0)), 20.0).is_empty());
    }

    #[test]
    fn the_phase_changes_once_and_unlocks_new_attacks() {
        let p = params();
        let mut brain = BossBrain::new(&p, 3);
        let first = brain.think(&sense(0.9, None), &p, DT);
        assert_eq!(first.new_phase, None);
        let crossing = brain.think(&sense(0.4, None), &p, DT);
        assert_eq!(crossing.new_phase, Some(1));
        assert_eq!(brain.think(&sense(0.4, None), &p, DT).new_phase, None);
        // Healing back up does not calm it down.
        assert_eq!(brain.think(&sense(0.9, None), &p, DT).new_phase, None);
        assert_eq!(brain.phase(), 1);
        let moves = run(&mut brain, &p, sense(0.4, Some(10.0)), 20.0);
        assert!(
            moves.contains(&BossMove::Release { attack: 1 }),
            "spits once enraged"
        );
    }

    #[test]
    fn cooldowns_space_out_an_attack() {
        let p = params();
        let mut brain = BossBrain::new(&p, 11);
        let mut releases = Vec::new();
        for step in 0..(30.0 / DT) as usize {
            if let BossMove::Release { attack: 0 } =
                brain.think(&sense(1.0, Some(2.0)), &p, DT).action
            {
                releases.push(step as f32 * DT);
            }
        }
        for pair in releases.windows(2) {
            assert!(pair[1] - pair[0] >= p.attacks[0].cooldown - DT, "{pair:?}");
        }
    }

    #[test]
    fn the_same_seed_fights_the_same_fight() {
        let p = params();
        let (mut a, mut b) = (BossBrain::new(&p, 99), BossBrain::new(&p, 99));
        let s = sense(0.3, Some(2.5));
        assert_eq!(run(&mut a, &p, s, 20.0), run(&mut b, &p, s, 20.0));
    }
}

//! `[entity.boss]` — what makes a mob a boss.
//!
//! ```toml
//! [entity.boss]
//! title = "The Elder Stag"
//! arena_radius = 40
//! offering = { item = "stag_effigy", count = 1 }
//!
//! [[entity.boss.attack]]
//! id = "gore"
//! range = 3.5          # chosen only while the target is this close
//! cooldown = 1.6
//! windup = 0.4         # the telegraph: seconds between committing and landing
//! effect = { kind = "melee", damage = 9 }
//!
//! [[entity.boss.phase]]
//! below = 1.0          # this phase starts at or below this fraction of health
//! attacks = ["gore"]
//! ```

use crate::core::ident::is_valid_id;

/// The boss component. Present only on kinds that also carry `[entity.mob]`.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossParams {
    /// Shown on the boss bar and in announcements.
    pub title: String,
    /// Players within this distance of the boss are in the fight: they see
    /// the bar, share the loot, and keep the boss from leashing.
    pub arena_radius: f32,
    /// Seconds the arena may stand empty before the boss gives up and leaves,
    /// taking the offering with it.
    #[serde(default = "default_leash")]
    pub leash_seconds: f32,
    /// What must be offered at its altar to summon it.
    pub offering: Offering,
    #[serde(rename = "attack")]
    pub attacks: Vec<BossAttack>,
    #[serde(rename = "phase")]
    pub phases: Vec<BossPhase>,
}

fn default_leash() -> f32 {
    30.0
}

#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Offering {
    /// An item id from `items.toml`.
    pub item: String,
    #[serde(default = "one")]
    pub count: u8,
}

fn one() -> u8 {
    1
}

/// One move in a boss's set.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossAttack {
    pub id: String,
    /// Chosen only while the target is at most this far away.
    pub range: f32,
    /// Seconds before this attack may be chosen again.
    pub cooldown: f32,
    /// Seconds between committing to the attack and it landing — the window
    /// every peer's telegraph shows and a player dodges in.
    #[serde(default)]
    pub windup: f32,
    /// Relative pick chance among the attacks currently available.
    #[serde(default = "one_u32")]
    pub weight: u32,
    pub effect: AttackEffect,
}

fn one_u32() -> u32 {
    1
}

/// What an attack does when it lands. A new kind of attack is one variant here
/// plus its arm in `state::ingame_state::bosses::land_attack` — the compiler
/// names the site.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AttackEffect {
    /// Strike the target, if still within the attack's range.
    Melee { damage: f32 },
    /// Hit everyone within `radius` of the boss, flinging them outward.
    Slam {
        damage: f32,
        radius: f32,
        #[serde(default)]
        knockback: f32,
    },
    /// A fan of `count` projectiles at the target, `spread_deg` wide.
    Volley {
        damage: f32,
        count: u8,
        spread_deg: f32,
        speed: f32,
        #[serde(default = "default_gravity")]
        gravity: f32,
        #[serde(default = "default_lifetime")]
        lifetime: f32,
    },
    /// Call `count` (a `[min, max]` range) of `entity` to the boss's side,
    /// never more than `cap` alive at once.
    Summon {
        entity: String,
        count: [u8; 2],
        cap: u32,
    },
}

fn default_gravity() -> f32 {
    10.0
}

fn default_lifetime() -> f32 {
    4.0
}

/// A stage of the fight: the moves available once health falls to `below`.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BossPhase {
    /// Starts when health is at or below this fraction of the maximum. The
    /// first phase must say `1.0`; each later one must be lower.
    pub below: f32,
    pub attacks: Vec<String>,
    /// Multiplier on walk/run speed — enraged bosses close faster.
    #[serde(default = "one_f32")]
    pub speed: f32,
    /// `attacks`, resolved to indices into [`BossParams::attacks`] at load.
    #[serde(skip)]
    pub attack_indices: Vec<usize>,
}

fn one_f32() -> f32 {
    1.0
}

impl BossParams {
    /// Check the component hangs together and resolve phase attack names.
    /// Called by the entity registry; an error rejects the whole file, because
    /// a boss whose phase names a missing attack would silently fight with
    /// half its moves.
    pub fn validate(&mut self, kind: &str) -> Result<(), String> {
        let fail = |e: String| format!("boss {kind:?}: {e}");
        if self.attacks.is_empty() {
            return Err(fail("declares no [[entity.boss.attack]]".into()));
        }
        for (i, attack) in self.attacks.iter().enumerate() {
            if !is_valid_id(&attack.id) {
                return Err(fail(format!("attack id {:?} is not a valid id", attack.id)));
            }
            if self.attacks[..i].iter().any(|a| a.id == attack.id) {
                return Err(fail(format!("attack {:?} declared twice", attack.id)));
            }
            if let AttackEffect::Summon { count, .. } = &attack.effect
                && count[0] > count[1]
            {
                return Err(fail(format!(
                    "attack {:?}: count must be [min, max]",
                    attack.id
                )));
            }
        }
        let Some(first) = self.phases.first() else {
            return Err(fail("declares no [[entity.boss.phase]]".into()));
        };
        if first.below < 1.0 {
            return Err(fail("the first phase must start at below = 1.0".into()));
        }
        for pair in self.phases.windows(2) {
            if pair[1].below >= pair[0].below {
                return Err(fail("phases must be listed with `below` falling".into()));
            }
        }
        let attacks = &self.attacks;
        for phase in &mut self.phases {
            if phase.attacks.is_empty() {
                return Err(fail(format!("phase below {} has no attacks", phase.below)));
            }
            phase.attack_indices = phase
                .attacks
                .iter()
                .map(|id| {
                    attacks
                        .iter()
                        .position(|a| a.id == *id)
                        .ok_or_else(|| fail(format!("phase names unknown attack {id:?}")))
                })
                .collect::<Result<_, String>>()?;
        }
        Ok(())
    }

    /// The phase a boss at `health_fraction` of its health is in.
    pub fn phase_at(&self, health_fraction: f32) -> usize {
        self.phases
            .iter()
            .rposition(|p| health_fraction <= p.below)
            .unwrap_or(0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STAG: &str = r#"
        title = "The Elder Stag"
        arena_radius = 40
        offering = { item = "stag_effigy" }

        [[attack]]
        id = "gore"
        range = 3.5
        cooldown = 1.6
        windup = 0.4
        effect = { kind = "melee", damage = 9 }

        [[attack]]
        id = "stomp"
        range = 5
        cooldown = 6
        windup = 0.9
        effect = { kind = "slam", damage = 7, radius = 6, knockback = 9 }

        [[phase]]
        below = 1.0
        attacks = ["gore"]

        [[phase]]
        below = 0.5
        attacks = ["gore", "stomp"]
        speed = 1.3
    "#;

    fn parse(text: &str) -> Result<BossParams, String> {
        let mut boss: BossParams = toml::from_str(text).map_err(|e| e.to_string())?;
        boss.validate("test")?;
        Ok(boss)
    }

    #[test]
    fn a_well_formed_boss_resolves_its_phases() {
        let boss = parse(STAG).unwrap();
        assert_eq!(boss.offering.count, 1);
        assert_eq!(boss.leash_seconds, 30.0);
        assert_eq!(boss.phases[0].attack_indices, vec![0]);
        assert_eq!(boss.phases[1].attack_indices, vec![0, 1]);
        assert_eq!(
            boss.attacks[1].effect,
            AttackEffect::Slam {
                damage: 7.0,
                radius: 6.0,
                knockback: 9.0
            }
        );
    }

    #[test]
    fn the_phase_follows_health_down() {
        let boss = parse(STAG).unwrap();
        assert_eq!(boss.phase_at(1.0), 0);
        assert_eq!(boss.phase_at(0.51), 0);
        assert_eq!(boss.phase_at(0.5), 1);
        assert_eq!(boss.phase_at(0.0), 1);
    }

    #[test]
    fn a_phase_naming_a_missing_attack_is_rejected() {
        let text = STAG.replace(r#"attacks = ["gore", "stomp"]"#, r#"attacks = ["roar"]"#);
        assert!(parse(&text).unwrap_err().contains("unknown attack"));
    }

    #[test]
    fn phases_out_of_order_are_rejected() {
        let text = STAG.replace("below = 0.5", "below = 1.0");
        assert!(parse(&text).unwrap_err().contains("falling"));
        let text = STAG.replace("below = 1.0", "below = 0.9");
        assert!(parse(&text).unwrap_err().contains("1.0"));
    }

    #[test]
    fn an_unknown_attack_kind_or_field_is_rejected() {
        assert!(parse(&STAG.replace("kind = \"melee\"", "kind = \"laser\"")).is_err());
        let extra = STAG.replace("damage = 9 }", "damage = 9, colour = 3 }");
        assert!(parse(&extra).is_err(), "unknown field inside an effect");
    }
}

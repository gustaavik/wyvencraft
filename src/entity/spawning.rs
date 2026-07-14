//! Mob spawn rules, loaded from `assets/spawning.toml`, and the spawn
//! planner that applies them.
//!
//! Validation is strict, like worldgen: every `entity` must name a kind in
//! the registry that carries `[entity.mob]`, or the whole file is rejected
//! and the caller falls back to the builtin copy. Silently spawning the
//! wrong mob would be worse than ignoring the file.
//!
//! The planner ([`Spawner::tick`]) is pure: the world enters only through
//! injected closures (live counts, ground lookup) and randomness through the
//! spawner's seeded [`Rng64`] — the same seed and inputs replay the same
//! spawn plan, which is what makes the system unit-testable. The state layer
//! owns the impure parts: sampling the real world and instantiating mobs.

use glam::Vec3;

use crate::core::Rng64;
use crate::entity::EntityRegistry;

/// Embedded copy of the shipped spawn rules, used when
/// `assets/spawning.toml` is missing or invalid.
pub const BUILTIN_SPAWNING: &str = include_str!("../../assets/spawning.toml");

/// Global spawn limits.
#[derive(Debug, Clone, Copy)]
pub struct SpawnLimits {
    /// Cap on simultaneous live mobs across all types.
    pub max_mobs: usize,
    /// Seconds between spawn passes.
    pub spawn_interval: f32,
    /// Candidate positions rolled per pass.
    pub attempts: u32,
    /// Spawn ring around a player: at least/at most this far away.
    pub min_player_distance: f32,
    pub max_player_distance: f32,
    /// Mobs farther than this from every player despawn.
    pub despawn_distance: f32,
}

/// One mob type's spawn rules.
#[derive(Debug, Clone)]
pub struct SpawnEntry {
    /// Entity kind name (validated against the registry at load).
    pub entity: String,
    /// Relative pick weight among eligible entries.
    pub weight: u32,
    /// Mobs placed together per successful roll (`min..=max`).
    pub group: (u8, u8),
    /// Most simultaneous live mobs of this type.
    pub cap: usize,
    pub night_only: bool,
    pub despawn_in_daylight: bool,
}

/// The validated spawn configuration.
#[derive(Debug, Clone)]
pub struct SpawnConfig {
    pub limits: SpawnLimits,
    pub entries: Vec<SpawnEntry>,
}

impl SpawnConfig {
    /// The embedded rules. Infallible: validated by tests.
    pub fn builtin(entities: &EntityRegistry) -> Self {
        Self::from_toml(BUILTIN_SPAWNING, entities).expect("embedded spawning.toml must parse")
    }

    /// Parse + strictly validate a spawning file against the entity registry.
    pub fn from_toml(text: &str, entities: &EntityRegistry) -> Result<Self, String> {
        let file: SpawningFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut entries = Vec::with_capacity(file.spawn.len());
        for def in file.spawn {
            let kind = entities
                .find(&def.entity)
                .ok_or_else(|| format!("unknown entity {:?}", def.entity))?;
            if kind.mob.is_none() {
                return Err(format!(
                    "entity {:?} has no [entity.mob] component",
                    def.entity
                ));
            }
            let [min, max] = def.group;
            if min == 0 || max < min {
                return Err(format!("spawn {:?}: bad group [{min}, {max}]", def.entity));
            }
            if def.weight == 0 {
                return Err(format!("spawn {:?}: weight must be positive", def.entity));
            }
            entries.push(SpawnEntry {
                entity: def.entity,
                weight: def.weight,
                group: (min, max),
                cap: def.cap as usize,
                night_only: def.night_only,
                despawn_in_daylight: def.despawn_in_daylight,
            });
        }
        let l = file.limits;
        if l.min_player_distance > l.max_player_distance {
            return Err("limits: min_player_distance exceeds max_player_distance".into());
        }
        Ok(Self {
            limits: SpawnLimits {
                max_mobs: l.max_mobs as usize,
                spawn_interval: l.spawn_interval,
                attempts: l.attempts,
                min_player_distance: l.min_player_distance,
                max_player_distance: l.max_player_distance,
                despawn_distance: l.despawn_distance,
            },
            entries,
        })
    }

    /// The entry for a kind name (for despawn rules on live mobs).
    pub fn entry(&self, entity: &str) -> Option<&SpawnEntry> {
        self.entries.iter().find(|e| e.entity == entity)
    }
}

// --- TOML schema layer (private; raw strings, strict fields) ---

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawningFile {
    limits: LimitsDef,
    #[serde(default)]
    spawn: Vec<SpawnDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct LimitsDef {
    max_mobs: u32,
    spawn_interval: f32,
    attempts: u32,
    min_player_distance: f32,
    max_player_distance: f32,
    despawn_distance: f32,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SpawnDef {
    entity: String,
    weight: u32,
    group: [u8; 2],
    cap: u32,
    #[serde(default)]
    night_only: bool,
    #[serde(default)]
    despawn_in_daylight: bool,
}

// --- The planner ---

/// A mob the planner wants placed. The state layer instantiates it.
#[derive(Debug, Clone, PartialEq)]
pub struct SpawnRequest {
    pub entity: String,
    pub position: Vec3,
}

/// Periodic spawn scheduler (the fluid sim's timer pattern). Owned by the
/// authority; `tick` plans, the caller applies.
pub struct Spawner {
    timer: f32,
    rng: Rng64,
}

impl Spawner {
    pub fn new(seed: u64) -> Self {
        Self {
            timer: 0.0,
            rng: Rng64::new(seed),
        }
    }

    /// Advance the schedule by `dt`; on a due pass, roll up to
    /// `limits.attempts` spawn candidates. `anchors` are player positions
    /// (spawning rings around them), `total`/`count_of` report live mob
    /// counts, and `find_ground(x, z)` samples the world for a walkable
    /// surface (`None` = unsuitable or unloaded). Pure given its inputs.
    #[allow(clippy::too_many_arguments)] // the planner's full (pure) input set
    pub fn tick(
        &mut self,
        cfg: &SpawnConfig,
        dt: f32,
        is_night: bool,
        anchors: &[Vec3],
        total: usize,
        count_of: impl Fn(&str) -> usize,
        find_ground: impl Fn(f32, f32) -> Option<f32>,
    ) -> Vec<SpawnRequest> {
        self.timer += dt;
        if self.timer < cfg.limits.spawn_interval {
            return Vec::new();
        }
        self.timer = 0.0;
        if anchors.is_empty() {
            return Vec::new();
        }

        fn planned_of(planned: &[SpawnRequest], name: &str) -> usize {
            planned.iter().filter(|r| r.entity == name).count()
        }

        let mut planned: Vec<SpawnRequest> = Vec::new();
        let mut live = total;
        for _ in 0..cfg.limits.attempts {
            if live >= cfg.limits.max_mobs {
                break;
            }
            // Eligible entries right now: night gate + per-type cap.
            let eligible: Vec<&SpawnEntry> = cfg
                .entries
                .iter()
                .filter(|e| !e.night_only || is_night)
                .filter(|e| count_of(&e.entity) + planned_of(&planned, &e.entity) < e.cap)
                .collect();
            let weights: Vec<u32> = eligible.iter().map(|e| e.weight).collect();
            let Some(pick) = self.rng.pick_weighted(&weights) else {
                break; // nothing eligible; later attempts won't differ
            };
            let entry = eligible[pick];

            // A ring position around a random player.
            let anchor = anchors[(self.rng.next_u64() % anchors.len() as u64) as usize];
            let angle = self.rng.range_f32(0.0, std::f32::consts::TAU);
            let radius = self.rng.range_f32(
                cfg.limits.min_player_distance,
                cfg.limits.max_player_distance,
            );
            let (cx, cz) = (
                anchor.x + angle.cos() * radius,
                anchor.z + angle.sin() * radius,
            );

            // Place the group, re-grounding each member near the center.
            let group = self
                .rng
                .range_u32(entry.group.0.into(), entry.group.1.into());
            for _ in 0..group {
                if live >= cfg.limits.max_mobs
                    || count_of(&entry.entity) + planned_of(&planned, &entry.entity) >= entry.cap
                {
                    break;
                }
                let x = cx + self.rng.range_f32(-3.0, 3.0);
                let z = cz + self.rng.range_f32(-3.0, 3.0);
                let Some(y) = find_ground(x, z) else {
                    continue;
                };
                // The ring bound is per-member: group scatter must not creep
                // inside the minimum player distance.
                let too_close = anchors.iter().any(|a| {
                    let d = Vec3::new(x - a.x, 0.0, z - a.z).length();
                    d < cfg.limits.min_player_distance
                });
                if too_close {
                    continue;
                }
                planned.push(SpawnRequest {
                    entity: entry.entity.clone(),
                    position: Vec3::new(x, y, z),
                });
                live += 1;
            }
        }
        planned
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn registry() -> EntityRegistry {
        EntityRegistry::builtin()
    }

    fn config() -> SpawnConfig {
        SpawnConfig::builtin(&registry())
    }

    /// Run enough ticks to trigger exactly one spawn pass.
    fn one_pass(
        spawner: &mut Spawner,
        cfg: &SpawnConfig,
        is_night: bool,
        count_of: impl Fn(&str) -> usize,
        total: usize,
    ) -> Vec<SpawnRequest> {
        spawner.tick(
            cfg,
            cfg.limits.spawn_interval,
            is_night,
            &[Vec3::new(0.0, 64.0, 0.0)],
            total,
            count_of,
            |_, _| Some(64.0),
        )
    }

    #[test]
    fn builtin_spawning_golden() {
        let cfg = config();
        assert_eq!(cfg.limits.max_mobs, 40);
        assert_eq!(cfg.limits.spawn_interval, 2.0);
        assert_eq!(cfg.limits.attempts, 6);
        assert_eq!(cfg.limits.min_player_distance, 16.0);
        assert_eq!(cfg.limits.max_player_distance, 48.0);
        assert_eq!(cfg.limits.despawn_distance, 96.0);
        assert_eq!(cfg.entries.len(), 4);

        let cow = cfg.entry("cow").expect("cow entry");
        assert_eq!((cow.weight, cow.group, cow.cap), (10, (1, 3), 10));
        assert!(!cow.night_only && !cow.despawn_in_daylight);
        let sheep = cfg.entry("sheep").expect("sheep entry");
        assert_eq!((sheep.weight, sheep.group, sheep.cap), (10, (2, 4), 10));
        let zombie = cfg.entry("zombie").expect("zombie entry");
        assert_eq!((zombie.weight, zombie.group, zombie.cap), (12, (1, 2), 10));
        assert!(zombie.night_only && zombie.despawn_in_daylight);
        let skeleton = cfg.entry("skeleton").expect("skeleton entry");
        assert_eq!(
            (skeleton.weight, skeleton.group, skeleton.cap),
            (8, (1, 1), 6)
        );
        assert!(skeleton.night_only && skeleton.despawn_in_daylight);
    }

    #[test]
    fn unknown_or_unmoblike_entities_reject_the_file() {
        let reg = registry();
        let bad_name = r#"
            [limits]
            max_mobs = 10
            spawn_interval = 2.0
            attempts = 4
            min_player_distance = 8.0
            max_player_distance = 32.0
            despawn_distance = 64.0
            [[spawn]]
            entity = "dragon"
            weight = 1
            group = [1, 1]
            cap = 1
        "#;
        assert!(SpawnConfig::from_toml(bad_name, &reg).is_err());

        let not_a_mob = bad_name.replace("dragon", "player");
        assert!(SpawnConfig::from_toml(&not_a_mob, &reg).is_err());

        let bad_group = bad_name
            .replace("dragon", "cow")
            .replace("[1, 1]", "[3, 1]");
        assert!(SpawnConfig::from_toml(&bad_group, &reg).is_err());

        let typo = bad_name
            .replace("dragon", "cow")
            .replace("weight", "wieght");
        assert!(SpawnConfig::from_toml(&typo, &reg).is_err());
    }

    #[test]
    fn same_seed_plans_the_same_spawns() {
        let cfg = config();
        let mut a = Spawner::new(9);
        let mut b = Spawner::new(9);
        for _ in 0..5 {
            let pa = one_pass(&mut a, &cfg, true, |_| 0, 0);
            let pb = one_pass(&mut b, &cfg, true, |_| 0, 0);
            assert_eq!(pa, pb);
        }
    }

    #[test]
    fn nothing_spawns_before_the_interval_elapses() {
        let cfg = config();
        let mut spawner = Spawner::new(1);
        let plan = spawner.tick(
            &cfg,
            cfg.limits.spawn_interval * 0.4,
            true,
            &[Vec3::ZERO],
            0,
            |_| 0,
            |_, _| Some(64.0),
        );
        assert!(plan.is_empty());
    }

    #[test]
    fn hostiles_only_spawn_at_night() {
        let cfg = config();
        let mut spawner = Spawner::new(2);
        // Daytime passes never plan zombies or skeletons.
        for _ in 0..20 {
            let plan = one_pass(&mut spawner, &cfg, false, |_| 0, 0);
            assert!(
                plan.iter()
                    .all(|r| r.entity == "cow" || r.entity == "sheep"),
                "day plan contained a hostile: {plan:?}"
            );
        }
        // Night passes eventually do.
        let mut saw_hostile = false;
        for _ in 0..20 {
            let plan = one_pass(&mut spawner, &cfg, true, |_| 0, 0);
            saw_hostile |= plan
                .iter()
                .any(|r| r.entity == "zombie" || r.entity == "skeleton");
        }
        assert!(saw_hostile, "night passes should plan hostiles");
    }

    #[test]
    fn caps_bound_the_plan() {
        let cfg = config();
        let mut spawner = Spawner::new(3);
        // Global cap: at the limit, nothing spawns.
        let plan = one_pass(&mut spawner, &cfg, true, |_| 10, cfg.limits.max_mobs);
        assert!(plan.is_empty(), "global cap should block spawns");
        // Per-type cap: full cow/sheep counts at day → nothing eligible.
        let plan = one_pass(&mut spawner, &cfg, false, |_| 10, 20);
        assert!(plan.is_empty(), "capped types should not spawn");
        // A long day run never overshoots any per-type cap.
        let mut cows = 0usize;
        for _ in 0..200 {
            let plan = one_pass(
                &mut spawner,
                &cfg,
                false,
                |name| {
                    if name == "cow" { cows } else { 10 }
                },
                cows + 10,
            );
            for r in &plan {
                assert_eq!(r.entity, "cow", "only cows are eligible here");
            }
            cows += plan.len();
        }
        assert!(cows <= cfg.entry("cow").unwrap().cap, "cap held: {cows}");
        assert!(cows > 0, "some cows should have spawned");
    }

    #[test]
    fn spawns_land_in_the_ring_and_on_the_ground() {
        let cfg = config();
        let mut spawner = Spawner::new(4);
        let anchor = Vec3::new(100.0, 70.0, -50.0);
        for _ in 0..20 {
            let plan = spawner.tick(
                &cfg,
                cfg.limits.spawn_interval,
                true,
                &[anchor],
                0,
                |_| 0,
                |x, z| Some(63.0 + (x + z).sin()), // varied "terrain"
            );
            for r in &plan {
                let d = Vec3::new(r.position.x - anchor.x, 0.0, r.position.z - anchor.z).length();
                assert!(
                    d >= cfg.limits.min_player_distance
                        && d <= cfg.limits.max_player_distance + 3.0 * std::f32::consts::SQRT_2,
                    "distance {d} outside the ring (+group scatter)"
                );
                let ground = 63.0 + (r.position.x + r.position.z).sin();
                assert!((r.position.y - ground).abs() < 1e-5, "grounded");
            }
        }
    }

    #[test]
    fn unsuitable_ground_yields_no_spawns() {
        let cfg = config();
        let mut spawner = Spawner::new(5);
        let plan = spawner.tick(
            &cfg,
            cfg.limits.spawn_interval,
            true,
            &[Vec3::ZERO],
            0,
            |_| 0,
            |_, _| None, // nothing walkable anywhere
        );
        assert!(plan.is_empty());
    }
}

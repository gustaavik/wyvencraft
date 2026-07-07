//! Entity kind definitions, loaded from `assets/entities.toml`.
//!
//! An [`EntityKind`] is a bundle of typed tuning components; concrete entity
//! types ([`crate::entity::Player`], [`crate::entity::DroppedItem`]) copy the
//! components they need at construction. This keeps hot code monomorphic (no
//! dyn dispatch) while making every number data. A future entity type is a
//! new `[[entity]]` entry plus, at most, one new component implemented once
//! in Rust.

/// Embedded copy of the shipped entity definitions, used when
/// `assets/entities.toml` is missing or invalid.
pub const BUILTIN_ENTITIES: &str = include_str!("../../assets/entities.toml");

/// Gravity + collision-box tuning shared by every simulated entity.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PhysicsParams {
    /// Blocks/s² downward.
    pub gravity: f32,
    /// Most negative vertical velocity (blocks/s).
    pub terminal_velocity: f32,
    /// Collision box edge (X/Z) and height (Y).
    pub width: f32,
    pub height: f32,
    /// Exponential horizontal damping per second while on the ground.
    #[serde(default)]
    pub ground_friction: f32,
}

/// Player-style locomotion.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MovementParams {
    pub walk_speed: f32,
    pub sprint_speed: f32,
    pub fly_speed: f32,
    pub jump_speed: f32,
    /// Camera/raycast origin above the feet.
    pub eye_height: f32,
    /// Block interaction distance.
    pub reach: f32,
}

/// Survival health/hunger model.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct VitalsParams {
    pub max_health: f32,
    pub max_hunger: f32,
    /// Falls shorter than this many blocks deal no damage.
    pub safe_fall: f32,
    /// Health lost per block fallen beyond `safe_fall`.
    pub fall_damage_per_block: f32,
    /// Hunger drained per second while idle / extra while sprinting.
    pub hunger_drain_base: f32,
    pub hunger_drain_sprint: f32,
    /// At/above this hunger the entity regenerates `regen_rate` health/s.
    pub regen_hunger_threshold: f32,
    pub regen_rate: f32,
    /// Health lost per second at zero hunger.
    pub starve_rate: f32,
}

/// Dropped-item behavior: spawn impulses, pickup rules, lifetime.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ItemEntityParams {
    pub despawn_seconds: f32,
    /// Pop speed for drops spawned by breaking a block.
    pub pop_horizontal: f32,
    pub pop_vertical: f32,
    /// Launch speed for items tossed with the drop key.
    pub throw_speed: f32,
    pub throw_lift: f32,
    /// Grace periods before a fresh drop can be picked up.
    pub block_drop_delay: f32,
    pub thrown_delay: f32,
    /// Distance beyond the player's box that collects the drop.
    pub pickup_range: f32,
}

/// Spin/bob tuning for the item-cube visual.
#[derive(Debug, Clone, Copy, Default, serde::Deserialize)]
pub struct ItemCubeParams {
    /// Rad/s around Y.
    pub spin_rate: f32,
    /// Idle bob height (blocks) and rate (rad/s).
    pub bob_amplitude: f32,
    pub bob_rate: f32,
}

/// How the entity is drawn.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualSpec {
    /// The skinned box model ([`crate::entity::HumanoidModel`]).
    Humanoid,
    /// A small spinning cube textured like the carried item.
    ItemCube(ItemCubeParams),
}

/// One entity type: a name plus the components it carries.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct EntityKind {
    pub name: String,
    pub physics: PhysicsParams,
    #[serde(default)]
    pub movement: Option<MovementParams>,
    #[serde(default)]
    pub vitals: Option<VitalsParams>,
    #[serde(default)]
    pub item: Option<ItemEntityParams>,
    pub visual: VisualSpec,
}

#[derive(serde::Deserialize)]
struct EntityFile {
    #[serde(default)]
    entity: Vec<EntityKind>,
}

/// Lookup table of entity kinds. The engine's two required kinds are
/// validated at load and exposed directly.
#[derive(Debug)]
pub struct EntityRegistry {
    kinds: Vec<EntityKind>,
    player: usize,
    dropped_item: usize,
}

impl EntityRegistry {
    /// Build the registry from the embedded copy of `assets/entities.toml`.
    /// Infallible: the shipped file is validated by tests.
    pub fn builtin() -> Self {
        Self::from_toml(BUILTIN_ENTITIES).expect("embedded entities.toml must parse")
    }

    /// Parse an entities file. Fails (→ caller falls back to the builtin
    /// copy) when the required kinds are missing their required components.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: EntityFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let kinds = file.entity;
        for (i, kind) in kinds.iter().enumerate() {
            if kinds[..i].iter().any(|other| other.name == kind.name) {
                return Err(format!("duplicate entity {:?}", kind.name));
            }
        }
        let require = |name: &str| {
            kinds
                .iter()
                .position(|k| k.name == name)
                .ok_or_else(|| format!("missing required entity {name:?}"))
        };
        let player = require("player")?;
        if kinds[player].movement.is_none() || kinds[player].vitals.is_none() {
            return Err("entity \"player\" needs [entity.movement] and [entity.vitals]".into());
        }
        let dropped_item = require("dropped item")?;
        if kinds[dropped_item].item.is_none() {
            return Err("entity \"dropped item\" needs [entity.item]".into());
        }
        Ok(Self {
            kinds,
            player,
            dropped_item,
        })
    }

    pub fn find(&self, name: &str) -> Option<&EntityKind> {
        self.kinds.iter().find(|k| k.name == name)
    }

    /// The local-player kind (movement + vitals guaranteed present).
    pub fn player(&self) -> &EntityKind {
        &self.kinds[self.player]
    }

    /// The dropped-item kind (item params guaranteed present).
    pub fn dropped_item(&self) -> &EntityKind {
        &self.kinds[self.dropped_item]
    }

    pub fn iter(&self) -> impl Iterator<Item = &EntityKind> {
        self.kinds.iter()
    }

    pub fn len(&self) -> usize {
        self.kinds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.kinds.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Golden snapshot: the shipped entities.toml carries exactly the tuning
    /// the hardcoded constants used to.
    #[test]
    fn builtin_entities_golden() {
        let reg = EntityRegistry::builtin();
        assert_eq!(reg.len(), 2);

        let player = reg.player();
        assert_eq!(player.physics.gravity, 28.0);
        assert_eq!(player.physics.terminal_velocity, -60.0);
        assert_eq!((player.physics.width, player.physics.height), (0.6, 1.8));
        let movement = player.movement.expect("player movement");
        assert_eq!(movement.walk_speed, 4.3);
        assert_eq!(movement.sprint_speed, 6.5);
        assert_eq!(movement.fly_speed, 12.0);
        assert_eq!(movement.jump_speed, 9.0);
        assert_eq!(movement.eye_height, 1.62);
        assert_eq!(movement.reach, 5.0);
        let vitals = player.vitals.expect("player vitals");
        assert_eq!((vitals.max_health, vitals.max_hunger), (20.0, 20.0));
        assert_eq!(vitals.safe_fall, 3.0);
        assert_eq!(vitals.fall_damage_per_block, 1.0);
        assert_eq!(vitals.hunger_drain_base, 0.05);
        assert_eq!(vitals.hunger_drain_sprint, 0.15);
        assert_eq!(vitals.regen_hunger_threshold, 18.0);
        assert_eq!((vitals.regen_rate, vitals.starve_rate), (1.0, 1.0));
        assert!(matches!(player.visual, VisualSpec::Humanoid));

        let drop = reg.dropped_item();
        assert_eq!((drop.physics.width, drop.physics.height), (0.25, 0.25));
        assert_eq!(drop.physics.ground_friction, 10.0);
        let item = drop.item.expect("drop item params");
        assert_eq!(item.despawn_seconds, 300.0);
        assert_eq!((item.pop_horizontal, item.pop_vertical), (1.4, 3.2));
        assert_eq!((item.throw_speed, item.throw_lift), (6.0, 2.0));
        assert_eq!((item.block_drop_delay, item.thrown_delay), (0.3, 1.5));
        assert_eq!(item.pickup_range, 1.0);
        match drop.visual {
            VisualSpec::ItemCube(cube) => {
                assert_eq!(cube.spin_rate, 1.8);
                assert_eq!(cube.bob_amplitude, 0.03);
                assert_eq!(cube.bob_rate, 2.4);
            }
            VisualSpec::Humanoid => panic!("dropped item should be an item cube"),
        }
    }

    #[test]
    fn files_missing_required_kinds_are_rejected() {
        assert!(EntityRegistry::from_toml("").is_err());
        let no_vitals = r#"
            [[entity]]
            name = "player"
            [entity.physics]
            gravity = 28.0
            terminal_velocity = -60.0
            width = 0.6
            height = 1.8
            [entity.visual]
            kind = "humanoid"
        "#;
        assert!(EntityRegistry::from_toml(no_vitals).is_err());
    }
}

//! Entity kind definitions, loaded from `assets/entities.toml`.
//!
//! An [`EntityKind`] is a bundle of typed tuning components; concrete entity
//! types ([`crate::entity::Player`], [`crate::entity::DroppedItem`]) copy the
//! components they need at construction. This keeps hot code monomorphic (no
//! dyn dispatch) while making every number data. A future entity type is a
//! new `[[entity]]` entry plus, at most, one new component implemented once
//! in Rust.

use wyven_model::ModelSpec;

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
    /// Rate (per second) at which airborne horizontal velocity converges on the
    /// wish velocity. Ground movement stays instant; in the air this is what
    /// stops a mid-flight direction change from snapping.
    #[serde(default = "default_air_control")]
    pub air_control: f32,
    /// Minimum height (blocks) a jump still reaches when the key is released
    /// immediately. Floors the variable-height jump so a tap can always clear a
    /// one-block step.
    #[serde(default = "default_min_jump_height")]
    pub min_jump_height: f32,
    /// Rate (per second) at which a grounded entity sheds speed once it stops
    /// asking to move. *Only* the stop ramps: accelerating and changing
    /// direction stay instant, so this is not general ground friction and does
    /// not make steering feel loose.
    ///
    /// It exists because releasing the controls has to mean "coast", not
    /// "stop": the inventory releases them while physics keeps running, and a
    /// player who was walking would otherwise halt in a single step in full
    /// view of the camera that just panned onto them.
    #[serde(default = "default_stop_rate")]
    pub stop_rate: f32,
}

fn default_air_control() -> f32 {
    6.0
}

fn default_min_jump_height() -> f32 {
    1.2
}

/// Roughly a fifth of a second to shed walking speed — long enough to read as
/// momentum, short enough that normal play still feels like an instant stop.
fn default_stop_rate() -> f32 {
    18.0
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

/// What drives a mob's decisions.
///
/// One axis with named values rather than a flag per trait: "is it hostile",
/// "does it act at all" and every future disposition are the *same* question,
/// and answering it with independent booleans makes half their combinations
/// meaningless (`hostile = true, inanimate = true`?). Adding a disposition is a
/// variant here plus its arm in [`crate::entity::brain::MobBrain::think`], and
/// the compiler names every site that has to account for it.
///
/// Deliberately *not* about physics: how hard something is to shove is
/// [`MobParams::knockback_resistance`], so an inert prop can still be sent
/// flying, and a boss can stand its ground while chasing you.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Behavior {
    /// Wanders idly and bolts when hurt. The default disposition.
    #[default]
    Passive,
    /// Chases visible players and attacks them; retaliates when hit.
    Hostile,
    /// Decides nothing at all: never wanders, never turns, never reacts. A
    /// fixture — a statue or a placed model that wants physics and a collision
    /// box but no behavior. The targeting ranges below are unused.
    Inert,
}

/// Mob behavior tuning: health, locomotion speeds, and (for hostiles) how the
/// mob acquires and attacks a target. Presence of this component is what makes
/// an entity kind a mob (simulated by the AI brain and eligible for spawning).
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobParams {
    pub max_health: f32,
    /// Wander speed / chase-or-flee speed (blocks/s).
    pub walk_speed: f32,
    pub run_speed: f32,
    pub jump_speed: f32,
    /// What the mob does with what it perceives.
    #[serde(default)]
    pub behavior: Behavior,
    /// Fraction of an incoming knockback impulse the mob shrugs off: `0.0`
    /// (default) takes the full shove, `1.0` cannot be moved by a hit, and the
    /// values between are "heavy". A scalar rather than an immovable flag,
    /// because weight is a spectrum and it is independent of [`Behavior`].
    #[serde(default)]
    pub knockback_resistance: f32,
    /// Distance within which a hostile notices a visible player.
    #[serde(default)]
    pub aggro_range: f32,
    /// Attack triggers within this distance (melee reach, or firing range).
    #[serde(default)]
    pub attack_range: f32,
    /// Melee damage per hit (unused by ranged mobs).
    #[serde(default)]
    pub attack_damage: f32,
    /// Seconds between attacks.
    #[serde(default = "default_attack_cooldown")]
    pub attack_cooldown: f32,
    /// Present on mobs that attack by firing a projectile instead of meleeing.
    #[serde(default)]
    pub ranged: Option<RangedParams>,
    /// What the mob drops on death.
    #[serde(default)]
    pub drops: Vec<MobDrop>,
}

fn default_attack_cooldown() -> f32 {
    1.0
}

/// Projectile attack tuning (the skeleton's bow). The projectile itself is not
/// an entity kind: these numbers fully describe it.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RangedParams {
    /// Launch speed (blocks/s) and damage on a player hit.
    pub projectile_speed: f32,
    pub projectile_damage: f32,
    /// The mob backs away when the target is closer than this.
    pub keep_distance: f32,
    /// Blocks/s² pulling the projectile down.
    #[serde(default = "default_projectile_gravity")]
    pub projectile_gravity: f32,
    /// Seconds before an airborne projectile despawns.
    #[serde(default = "default_projectile_lifetime")]
    pub lifetime: f32,
}

fn default_projectile_gravity() -> f32 {
    20.0
}

fn default_projectile_lifetime() -> f32 {
    8.0
}

/// One entry of a mob's death-drop table: `min..=max` of the named item.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobDrop {
    pub item: String,
    pub min: u8,
    pub max: u8,
}

/// Spin/bob tuning for the item-cube visual.
#[derive(Debug, Clone, Copy, serde::Deserialize)]
pub struct ItemCubeParams {
    /// Rad/s around Y.
    pub spin_rate: f32,
    /// Idle bob height (blocks) and rate (rad/s).
    pub bob_amplitude: f32,
    pub bob_rate: f32,
    /// How much larger the drop is *drawn* than its collision box.
    ///
    /// Separate from `[entity.physics] width` on purpose: a drop you can see
    /// from across the room should not also be a bigger obstacle, or catch on
    /// scenery it visually clears. The rendered box is lifted so it still rests
    /// on the ground rather than sinking into it.
    #[serde(default = "one")]
    pub scale: f32,
}

fn one() -> f32 {
    1.0
}

impl Default for ItemCubeParams {
    fn default() -> Self {
        Self {
            spin_rate: 0.0,
            bob_amplitude: 0.0,
            bob_rate: 0.0,
            scale: 1.0,
        }
    }
}

/// Humanoid-model options. Defaults reproduce the player: its own skin sheet
/// and a normal rest pose.
#[derive(Debug, Clone, Default, serde::Deserialize)]
pub struct HumanoidVisual {
    /// Named mob skin sheet (`render::mobskin`); `None` = the player skin.
    #[serde(default)]
    pub skin: Option<String>,
    /// Hold both arms straight forward (the zombie shamble).
    #[serde(default)]
    pub arms_forward: bool,
}

/// Four-legged box model (cow, sheep): a body slab on four legs with a head at
/// the front. Part sizes are in skin pixels (16 px = 1 block), like the
/// humanoid model's proportions.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct QuadrupedVisual {
    /// Named mob skin sheet (`render::mobskin`).
    pub skin: String,
    /// Part extents in px: `[width, height, depth]` (depth runs nose→tail).
    pub body: [f32; 3],
    pub head: [f32; 3],
    pub leg: [f32; 3],
    /// Where each part's unwrap starts on the sheet — Minecraft's `texOffs`,
    /// which is what mob art is drawn against. The extents above give the rest:
    /// a part's six face rects follow from its offset and its size.
    ///
    /// The defaults are the layout Minecraft's own quadrupeds share; the cow is
    /// the odd one out and names its own `body_uv`.
    #[serde(default = "head_uv")]
    pub head_uv: [u32; 2],
    #[serde(default = "body_uv")]
    pub body_uv: [u32; 2],
    #[serde(default = "leg_uv")]
    pub leg_uv: [u32; 2],
}

fn head_uv() -> [u32; 2] {
    [0, 0]
}

fn body_uv() -> [u32; 2] {
    [28, 8]
}

fn leg_uv() -> [u32; 2] {
    [0, 16]
}

/// How the entity is drawn.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualSpec {
    /// The skinned box model ([`crate::entity::HumanoidModel`]).
    Humanoid(HumanoidVisual),
    /// A small spinning cube textured like the carried item.
    ItemCube(ItemCubeParams),
    /// The four-legged box model ([`crate::entity::QuadrupedModel`]).
    Quadruped(QuadrupedVisual),
    /// Geometry loaded from a model file ([`wyven_model`]), with its own
    /// texture rather than a slot in the block atlas.
    Model(ModelSpec),
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
    #[serde(default)]
    pub mob: Option<MobParams>,
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
        assert_eq!(reg.len(), 9);

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
        match &player.visual {
            VisualSpec::Humanoid(v) => {
                assert_eq!(v.skin, None, "player uses the player skin");
                assert!(!v.arms_forward);
            }
            other => panic!("player should be humanoid, got {other:?}"),
        }

        let drop = reg.dropped_item();
        assert_eq!((drop.physics.width, drop.physics.height), (0.25, 0.25));
        assert_eq!(drop.physics.ground_friction, 10.0);
        let item = drop.item.expect("drop item params");
        assert_eq!(item.despawn_seconds, 300.0);
        assert_eq!((item.pop_horizontal, item.pop_vertical), (1.4, 3.2));
        assert_eq!((item.throw_speed, item.throw_lift), (6.0, 2.0));
        assert_eq!((item.block_drop_delay, item.thrown_delay), (0.3, 1.5));
        assert_eq!(item.pickup_range, 1.0);
        match &drop.visual {
            VisualSpec::ItemCube(cube) => {
                assert_eq!(cube.spin_rate, 1.8);
                assert_eq!(cube.bob_amplitude, 0.03);
                assert_eq!(cube.bob_rate, 2.4);
                assert_eq!(cube.scale, 2.0);
            }
            other => panic!("dropped item should be an item cube, got {other:?}"),
        }

        // The passive mobs: quadrupeds with a drop table, no hostility.
        let cow = reg.find("cow").expect("cow kind");
        assert_eq!((cow.physics.width, cow.physics.height), (0.9, 1.4));
        let mob = cow.mob.as_ref().expect("cow mob params");
        assert_eq!(mob.max_health, 10.0);
        assert_eq!((mob.walk_speed, mob.run_speed), (1.4, 3.0));
        assert_eq!(mob.jump_speed, 9.0);
        assert_eq!(mob.behavior, Behavior::Passive);
        assert!(mob.ranged.is_none());
        assert_eq!(mob.attack_cooldown, 1.0, "defaulted");
        assert_eq!(mob.drops.len(), 2);
        assert_eq!(mob.drops[0].item, "raw_beef");
        assert_eq!((mob.drops[0].min, mob.drops[0].max), (1, 3));
        assert_eq!(mob.drops[1].item, "leather");
        assert_eq!((mob.drops[1].min, mob.drops[1].max), (1, 2));
        match &cow.visual {
            VisualSpec::Quadruped(v) => {
                assert_eq!(v.skin, "cow");
                assert_eq!(v.body, [12.0, 10.0, 18.0]);
                assert_eq!(v.head, [8.0, 8.0, 6.0]);
                assert_eq!(v.leg, [4.0, 12.0, 4.0]);
                assert_eq!(v.body_uv, [18, 4], "the cow's own body unwrap");
            }
            other => panic!("cow should be a quadruped, got {other:?}"),
        }

        let sheep = reg.find("sheep").expect("sheep kind");
        let mob = sheep.mob.as_ref().expect("sheep mob params");
        assert_eq!(mob.max_health, 8.0);
        assert_eq!(mob.behavior, Behavior::Passive);
        assert_eq!(mob.drops[0].item, "mutton");
        assert_eq!((mob.drops[0].min, mob.drops[0].max), (1, 2));
        assert!(matches!(&sheep.visual, VisualSpec::Quadruped(v) if v.skin == "sheep"));

        // Box sizes and unwrap offsets are what make a real mob sheet land on
        // the right faces, so they are pinned together with the tuning.
        let pig = reg.find("pig").expect("pig kind");
        match &pig.visual {
            VisualSpec::Quadruped(v) => {
                assert_eq!(v.skin, "pig");
                assert_eq!(
                    (v.body, v.head, v.leg),
                    ([10.0, 8.0, 16.0], [8.0; 3], [4.0, 6.0, 4.0])
                );
                // The pig takes every default offset; the cow overrides its body.
                assert_eq!((v.head_uv, v.body_uv, v.leg_uv), ([0, 0], [28, 8], [0, 16]));
            }
            other => panic!("pig should be a quadruped, got {other:?}"),
        }

        // The hostiles: a melee humanoid and a ranged one.
        let zombie = reg.find("zombie").expect("zombie kind");
        assert_eq!((zombie.physics.width, zombie.physics.height), (0.6, 1.8));
        let mob = zombie.mob.as_ref().expect("zombie mob params");
        assert_eq!(mob.behavior, Behavior::Hostile);
        assert_eq!(mob.max_health, 20.0);
        assert_eq!((mob.aggro_range, mob.attack_range), (16.0, 1.6));
        assert_eq!((mob.attack_damage, mob.attack_cooldown), (3.0, 1.0));
        assert!(mob.ranged.is_none());
        assert!(mob.drops.is_empty());
        match &zombie.visual {
            VisualSpec::Humanoid(v) => {
                assert_eq!(v.skin.as_deref(), Some("zombie"));
                assert!(v.arms_forward);
            }
            other => panic!("zombie should be humanoid, got {other:?}"),
        }

        let skeleton = reg.find("skeleton").expect("skeleton kind");
        let mob = skeleton.mob.as_ref().expect("skeleton mob params");
        assert_eq!(mob.behavior, Behavior::Hostile);
        assert_eq!(mob.max_health, 16.0);
        assert_eq!((mob.aggro_range, mob.attack_range), (18.0, 12.0));
        assert_eq!(mob.attack_cooldown, 1.6);
        let ranged = mob.ranged.expect("skeleton ranged params");
        assert_eq!(ranged.projectile_speed, 18.0);
        assert_eq!(ranged.projectile_damage, 3.0);
        assert_eq!(ranged.keep_distance, 8.0);
        assert_eq!(ranged.projectile_gravity, 20.0, "defaulted");
        assert_eq!(ranged.lifetime, 8.0, "defaulted");
        assert!(
            matches!(&skeleton.visual, VisualSpec::Humanoid(v) if v.skin.as_deref() == Some("skeleton") && !v.arms_forward)
        );

        // The file-model visual: a path plus placement, and nothing heavier —
        // this spec is cloned onto every mob that uses it.
        let prop = reg.find("vine sword").expect("vine sword kind");
        let mob = prop.mob.as_ref().expect("vine sword mob params");
        assert_eq!(mob.behavior, Behavior::Inert, "the prop must not act");
        assert_eq!(mob.knockback_resistance, 1.0, "and cannot be shoved");
        match &prop.visual {
            VisualSpec::Model(spec) => {
                assert_eq!(spec.path, "assets/models/items/vine_sword.bbmodel");
                assert_eq!(spec.scale, 1.0, "defaulted");
                assert_eq!(spec.offset, [-0.5, 0.889, -0.5]);
            }
            other => panic!("vine sword should use a model visual, got {other:?}"),
        }
    }

    #[test]
    fn mob_component_is_optional_but_strict() {
        // A kind without [entity.mob] parses (it's just not a mob) ...
        let plain = r#"
            [[entity]]
            name = "player"
            [entity.physics]
            gravity = 28.0
            terminal_velocity = -60.0
            width = 0.6
            height = 1.8
            [entity.movement]
            walk_speed = 4.3
            sprint_speed = 6.5
            fly_speed = 12.0
            jump_speed = 9.0
            eye_height = 1.62
            reach = 5.0
            [entity.vitals]
            max_health = 20.0
            max_hunger = 20.0
            safe_fall = 3.0
            fall_damage_per_block = 1.0
            hunger_drain_base = 0.05
            hunger_drain_sprint = 0.15
            regen_hunger_threshold = 18.0
            regen_rate = 1.0
            starve_rate = 1.0
            [entity.visual]
            kind = "humanoid"

            [[entity]]
            name = "dropped item"
            [entity.physics]
            gravity = 28.0
            terminal_velocity = -60.0
            width = 0.25
            height = 0.25
            [entity.item]
            despawn_seconds = 300.0
            pop_horizontal = 1.4
            pop_vertical = 3.2
            throw_speed = 6.0
            throw_lift = 2.0
            block_drop_delay = 0.3
            thrown_delay = 1.5
            pickup_range = 1.0
            [entity.visual]
            kind = "item_cube"
            spin_rate = 1.8
            bob_amplitude = 0.03
            bob_rate = 2.4
        "#;
        let reg = EntityRegistry::from_toml(plain).expect("plain file parses");
        assert!(reg.player().mob.is_none());
        // `scale` is optional: a visual that omits it is drawn at its collision size.
        match &reg.dropped_item().visual {
            VisualSpec::ItemCube(cube) => assert_eq!(cube.scale, 1.0, "scale defaults to 1"),
            other => panic!("expected an item cube, got {other:?}"),
        }

        // ... while a misspelled mob field rejects the file.
        let bad = format!(
            "{plain}\n\
            [[entity]]\n\
            name = \"cow\"\n\
            [entity.physics]\n\
            gravity = 28.0\n\
            terminal_velocity = -60.0\n\
            width = 0.9\n\
            height = 1.4\n\
            [entity.mob]\n\
            max_health = 10.0\n\
            walk_sped = 1.4\n\
            run_speed = 3.0\n\
            jump_speed = 9.0\n\
            [entity.visual]\n\
            kind = \"humanoid\"\n"
        );
        assert!(EntityRegistry::from_toml(&bad).is_err());
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

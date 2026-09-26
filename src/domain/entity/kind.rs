//! Entity kind definitions, loaded from `assets/entities.toml`.
//!
//! An [`EntityKind`] is a bundle of typed tuning components; concrete entity
//! types ([`crate::domain::entity::Player`], [`crate::domain::entity::DroppedItem`]) copy the
//! components they need at construction. This keeps hot code monomorphic (no
//! dyn dispatch) while making every number data. A future entity type is a
//! new `[[entity]]` entry plus, at most, one new component implemented once
//! in Rust.

use wyven_model::ModelSpec;

/// Embedded copy of the shipped entity definitions, used when
/// `assets/entities.toml` is missing or invalid.
pub const BUILTIN_ENTITIES: &str =
    include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/assets/entities.toml"));

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
/// variant here plus its arm in [`crate::domain::entity::brain::MobBrain::think`], and
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

/// A rigged model: geometry, bones and keyframe clips from one file, drawn
/// against a shared atlas sheet rather than a texture of its own.
///
/// Distinct from [`VisualSpec::Model`] in the two things a *character* needs and
/// a prop does not: a skeleton to pose, and a skin that the game owns and can
/// swap. The skin is why this is a game-side spec rather than a field on
/// `ModelSpec` — the engine has no idea what an atlas sheet is.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct RiggedVisual {
    /// `assets/`-relative path to a `.bbmodel` carrying a rig.
    pub path: String,
    /// Uniform scale applied to the model's own units, which is how an authored
    /// figure is fitted to the collision box its entity declares.
    #[serde(default = "one")]
    pub scale: f32,
    /// Named mob skin sheet (`art::mobskin`); `None` = the player skin block.
    #[serde(default)]
    pub skin: Option<String>,
}

/// How the entity is drawn.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum VisualSpec {
    /// The skinned box model ([`crate::presentation::render::HumanoidModel`]).
    Humanoid(HumanoidVisual),
    /// A small spinning cube textured like the carried item.
    ItemCube(ItemCubeParams),
    /// The four-legged box model ([`crate::presentation::render::QuadrupedModel`]).
    Quadruped(QuadrupedVisual),
    /// Geometry loaded from a model file ([`wyven_model`]), with its own
    /// texture rather than a slot in the block atlas.
    Model(ModelSpec),
    /// A bone-animated model file drawn against an atlas skin sheet.
    Rigged(RiggedVisual),
}

impl VisualSpec {
    /// The model file this visual needs loaded, if any. One place rather than a
    /// `match` at every call site, so a new file-backed visual is a variant here
    /// and nothing else.
    pub fn model_path(&self) -> Option<&str> {
        match self {
            VisualSpec::Model(spec) => Some(&spec.path),
            VisualSpec::Rigged(visual) => Some(&visual.path),
            VisualSpec::Humanoid(_) | VisualSpec::ItemCube(_) | VisualSpec::Quadruped(_) => None,
        }
    }
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
    /// `[entity.boss]` — makes a mob a boss (see [`crate::domain::entity::boss`]).
    #[serde(default)]
    pub boss: Option<crate::domain::entity::boss::BossParams>,
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
        let mut kinds = file.entity;
        for kind in &mut kinds {
            if let Some(boss) = &mut kind.boss {
                if kind.mob.is_none() {
                    return Err(format!(
                        "entity {:?}: [entity.boss] needs [entity.mob]",
                        kind.name
                    ));
                }
                boss.validate(&kind.name)?;
            }
        }
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
mod tests;

//! Tests for [`super`]: `kind.rs`.

use super::*;

/// Golden snapshot: the shipped entities.toml carries exactly the tuning
/// the hardcoded constants used to.
#[test]
fn builtin_entities_golden() {
    let reg = EntityRegistry::builtin();
    // The nine vanilla kinds, plus the Meadows' deer, thornling and boss.
    assert_eq!(reg.len(), 12);

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
        VisualSpec::Rigged(v) => {
            assert_eq!(v.path, "assets/models/entity/player/player.bbmodel");
            assert_eq!(v.skin, None, "player uses the player skin block");
            // Stands the 1.096875-block model up in its 1.8-block box.
            assert!((v.scale - 1.8 / 1.096_875).abs() < 1e-4, "{}", v.scale);
        }
        other => panic!("player should be rigged, got {other:?}"),
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

/// The Meadows boss parses whole: a mob with a validated boss component
/// whose phases resolve to its own attacks.
#[test]
fn the_elder_stag_is_a_boss() {
    let reg = EntityRegistry::builtin();
    let stag = reg.find("elder stag").expect("elder stag");
    assert!(stag.mob.is_some());
    let boss = stag.boss.as_ref().expect("boss component");
    assert_eq!(boss.offering.item, "stag_effigy");
    assert_eq!(boss.phases.len(), 2);
    assert!(boss.phases[1].attack_indices.len() > boss.phases[0].attack_indices.len());
    assert!(reg.find("deer").unwrap().boss.is_none());
}

#[test]
fn a_malformed_boss_rejects_the_file() {
    let text = BUILTIN_ENTITIES.replacen("[entity.boss]\ntitle", "[entity.oops]\ntitle", 1);
    assert!(EntityRegistry::from_toml(&text).is_err(), "unknown table");
    let text = BUILTIN_ENTITIES.replace(
        "attacks = [\"gore\", \"stomp\", \"antler_lightning\"]",
        "attacks = [\"gore\", \"moonbeam\"]",
    );
    let err = EntityRegistry::from_toml(&text).unwrap_err();
    assert!(err.contains("moonbeam"), "{err}");
}

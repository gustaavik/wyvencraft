//! Tests for [`super`]: `item.rs`.

use super::*;
use crate::domain::inventory::component::{ArmorSlot, Consumable, Equippable, Tool};

/// Golden snapshot of the shipped item set. The data-driven loader must
/// reproduce this exactly: names are the save format and registration
/// order defines the numeric ids synced over the network.
#[test]
fn builtin_items_golden() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_blocks(&blocks);

    // One placeable item per visible, non-flowing block, in block order.
    let block_items = [
        "stone",
        "dirt",
        "grass",
        "sand",
        "water",
        "oak_log",
        "oak_leaves",
        "glass",
        "bedrock",
        "snow",
        "gravel",
        "clay",
        "coal_ore",
        "iron_ore",
        "copper_ore",
        "cobblestone",
        "blue_bells",
        "red_flower",
        "red_mushroom",
        "brown_mushroom",
        "cornflower",
        "deepstone",
        "mud",
        "packed_snow",
        "basalt",
        "ash",
        "mossy_cobblestone",
        "tin_ore",
        "silver_ore",
        "cinder_ore",
        "wayrune",
        "elder_altar",
        "workbench",
        "forge",
    ];
    /// One expected tool: name, kind, dig_speed, durability, damage.
    type ToolRow = (&'static str, &'static str, f32, u16, Option<f32>);
    let tools: [ToolRow; 14] = [
        ("wooden_pickaxe", "pickaxe", 2.0, 60, None),
        ("wooden_axe", "axe", 2.0, 60, Some(3.0)),
        ("wooden_shovel", "shovel", 2.0, 60, None),
        ("shears", "shears", 5.0, 120, None),
        ("vine_sword", "sword", 1.5, 200, Some(4.0)),
        ("wooden_sword", "sword", 1.5, 60, Some(4.0)),
        ("stone_pickaxe", "pickaxe", 4.0, 132, None),
        ("stone_axe", "axe", 4.0, 132, Some(4.0)),
        ("stone_shovel", "shovel", 4.0, 132, None),
        ("stone_sword", "sword", 1.5, 132, Some(5.0)),
        ("iron_pickaxe", "pickaxe", 6.0, 250, None),
        ("iron_axe", "axe", 6.0, 250, Some(5.0)),
        ("iron_shovel", "shovel", 6.0, 250, None),
        ("iron_sword", "sword", 1.5, 250, Some(6.0)),
    ];
    // (name, hunger, saturation)
    let foods = [
        ("apple", 4.0, 2.4),
        ("bread", 5.0, 6.0),
        ("raw_beef", 3.0, 1.8),
        ("mutton", 2.0, 1.2),
        ("raw_chicken", 2.0, 1.2),
        ("cooked_beef", 8.0, 12.8),
        ("cooked_mutton", 6.0, 9.6),
        ("cooked_chicken", 6.0, 7.2),
        ("raw_porkchop", 3.0, 1.8),
        ("cooked_porkchop", 8.0, 12.8),
    ];
    // Plain stackables declared between the foods and the armor: mob and
    // block materials with no components at all.
    let materials = [
        "leather",
        "feather",
        "string",
        "arrow",
        "coal",
        "clay_ball",
        "flint",
        "copper_ingot",
    ];
    // (name, slot, defense, durability)
    let armors = [
        ("copper_helmet", ArmorSlot::Helmet, 2.0, 120),
        ("copper_chestplate", ArmorSlot::Chestplate, 6.0, 240),
        ("copper_leggings", ArmorSlot::Leggings, 5.0, 200),
        ("copper_boots", ArmorSlot::Boots, 2.0, 120),
    ];

    // Plain stackables with no components at all, declared after the armor.
    let plain = ["stick"];
    // The Meadows' progression loot, last in the file.
    let meadows = [
        "antler_shard",
        "raw_venison",
        "cooked_venison",
        "stag_effigy",
        "elder_antler",
        "elder_stag_trophy",
        "antler_pickaxe",
    ];

    assert_eq!(
        items.len(),
        block_items.len()
            + tools.len()
            + foods.len()
            + materials.len()
            + armors.len()
            + plain.len()
            + meadows.len(),
        "item count changed"
    );

    for (i, &name) in block_items.iter().enumerate() {
        let item = items.get(ItemId(i as u16));
        assert_eq!(item.id, name, "item {i}: name");
        assert_eq!(item.max_stack, 64, "{name}: max_stack");
        assert_eq!(
            item.get::<Placeable>().map(|p| p.block),
            blocks.find(name),
            "{name}: places itself"
        );
        assert!(
            item.get::<Tool>().is_none() && item.get::<Consumable>().is_none(),
            "{name}: plain"
        );
        // Blocks map back to their item.
        let block_id = blocks.find(name).unwrap();
        assert_eq!(
            items.item_for_block(block_id),
            Some(ItemId(i as u16)),
            "{name}: item_for_block"
        );
    }

    for (offset, &(name, kind, dig_speed, durability, damage)) in tools.iter().enumerate() {
        let id = ItemId((block_items.len() + offset) as u16);
        let item = items.get(id);
        assert_eq!(item.id, name, "tool: name");
        assert_eq!(item.max_stack, 1, "{name}: max_stack");
        let tool = item.get::<Tool>().expect("tool capability");
        assert_eq!(tool.kind, kind, "{name}: kind");
        assert_eq!(tool.dig_speed, dig_speed, "{name}: dig_speed");
        assert_eq!(tool.durability, durability, "{name}: durability");
        assert_eq!(tool.damage, damage, "{name}: damage");
        assert_eq!(items.find(name), Some(id), "{name}: find");
    }

    // A tool says only what shape it is. Nothing here lists a block, and
    // nothing in this file needs touching when a block is added.
    let shearers: Vec<&str> = items
        .iter()
        .filter(|(_, item)| item.get::<Tool>().is_some_and(|t| t.kind == "shears"))
        .map(|(_, item)| item.id.as_str())
        .collect();
    assert_eq!(shearers, ["shears"], "one item is shears-shaped");

    for (offset, &(name, hunger, saturation)) in foods.iter().enumerate() {
        let id = ItemId((block_items.len() + tools.len() + offset) as u16);
        let item = items.get(id);
        assert_eq!(item.id, name, "food: name");
        assert_eq!(item.max_stack, 64, "{name}: max_stack");
        let food = item.get::<Consumable>().expect("consumable capability");
        assert_eq!(food.hunger, hunger, "{name}: hunger");
        assert_eq!(food.saturation, saturation, "{name}: saturation");
        assert_eq!(items.find(name), Some(id), "{name}: find");
    }

    for (offset, &name) in materials.iter().enumerate() {
        let id = ItemId((block_items.len() + tools.len() + foods.len() + offset) as u16);
        let item = items.get(id);
        assert_eq!(item.id, name, "material: name");
        assert_eq!(item.max_stack, 64, "{name}: max_stack");
        assert!(item.components.is_empty(), "{name}: carries no components");
        assert_eq!(items.find(name), Some(id), "{name}: find");
    }

    for (offset, &(name, slot, defense, durability)) in armors.iter().enumerate() {
        let id = ItemId(
            (block_items.len() + tools.len() + foods.len() + materials.len() + offset) as u16,
        );
        let item = items.get(id);
        assert_eq!(item.id, name, "armor: name");
        assert_eq!(item.max_stack, 1, "{name}: max_stack");
        let worn = item.get::<Equippable>().expect("equippable capability");
        assert_eq!(worn.slot, slot, "{name}: slot");
        assert_eq!(worn.defense, defense, "{name}: defense");
        assert_eq!(worn.durability, durability, "{name}: durability");
        assert_eq!(items.find(name), Some(id), "{name}: find");
        // Armor wears like a tool: a fresh piece carries full durability.
        assert_eq!(
            items.full_stack(id),
            ItemStack::with_durability(id, durability),
            "{name}: full_stack"
        );
    }

    for (offset, &name) in plain.iter().enumerate() {
        let id = ItemId(
            (block_items.len()
                + tools.len()
                + foods.len()
                + materials.len()
                + armors.len()
                + offset) as u16,
        );
        let item = items.get(id);
        assert_eq!(item.id, name, "plain: name");
        assert_eq!(item.max_stack, 64, "{name}: max_stack");
        assert!(item.components.is_empty(), "{name}: carries no components");
        assert_eq!(items.find(name), Some(id), "{name}: find");
    }

    // The starter kit resolves in hotbar order: four fresh tools, then the
    // two foods with their counts.
    let kit = items.starter_kit_survival();
    assert_eq!(kit.len(), 6, "starter kit size");
    for (slot, name) in [
        "wooden_pickaxe",
        "wooden_axe",
        "wooden_shovel",
        "vine_sword",
    ]
    .iter()
    .enumerate()
    {
        let id = items.find(name).unwrap();
        assert_eq!(kit[slot], items.full_stack(id), "kit slot {slot}");
    }
    assert_eq!(
        kit[4],
        ItemStack::new(items.find("apple").unwrap(), 5),
        "kit apples"
    );
    assert_eq!(
        kit[5],
        ItemStack::new(items.find("bread").unwrap(), 3),
        "kit bread"
    );
}

/// A malformed id fails the whole file, like the block table: an id is the
/// key recipes, drops, saves and `/give` all spell, so one that cannot be
/// typed as a single token would break those references silently.
#[test]
fn a_malformed_item_id_rejects_the_whole_file() {
    let blocks = BlockRegistry::with_builtins();
    for bad in ["Wooden Pickaxe", "wooden pickaxe", "wooden-pickaxe", ""] {
        let text = format!("[[item]]\nid = \"{bad}\"\n");
        let err = ItemRegistry::from_toml(&text, &blocks)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} should be rejected"));
        assert!(err.contains("id"), "{bad:?}: unhelpful error {err:?}");
    }
}

/// A table nothing claims is almost always a typo for one that exists, and
/// silently dropping it would ship an item missing the behaviour its author
/// wrote down. The error names the key, because that is what has to change.
#[test]
fn an_unknown_capability_rejects_the_whole_file_and_names_it() {
    let blocks = BlockRegistry::with_builtins();
    let err = ItemRegistry::from_toml(
        "[[item]]\nid = \"stick\"\n\n[item.edible]\nhunger = 1.0\n",
        &blocks,
    )
    .expect_err("`edible` is not a capability");
    assert!(err.contains("edible"), "unhelpful error {err:?}");
}

/// A capability that fails to *parse* is structural and fails the file, so
/// a typo inside a table cannot ship as silently-default numbers.
#[test]
fn a_malformed_capability_rejects_the_whole_file() {
    let blocks = BlockRegistry::with_builtins();
    assert!(
        ItemRegistry::from_toml(
            "[[item]]\nid = \"bread\"\n\n[item.consumable]\nhunger = 1.0\n",
            &blocks,
        )
        .is_err(),
        "a consumable without saturation is incomplete"
    );
}

/// `Item`'s `Debug` feeds `content_hash`, which gates multiplayer joins —
/// so two peers who wrote the same capabilities in a different order must
/// produce byte-identical items. Storing components in key order is what
/// guarantees it.
#[test]
fn capability_order_in_the_file_does_not_change_the_item() {
    let blocks = BlockRegistry::with_builtins();
    let one = "[[item]]\nid = \"cleaver\"\n\n[item.consumable]\n\
               hunger = 1.0\nsaturation = 1.0\n\n[item.tool]\n\
               kind = \"sword\"\ndig_speed = 1.0\ndurability = 5\n";
    let other = "[[item]]\nid = \"cleaver\"\n\n[item.tool]\n\
                 kind = \"sword\"\ndig_speed = 1.0\ndurability = 5\n\n\
                 [item.consumable]\nhunger = 1.0\nsaturation = 1.0\n";
    let render = |text: &str| {
        let items = ItemRegistry::from_toml(text, &blocks).expect("valid file");
        let id = items.find("cleaver").expect("declared");
        format!("{:?}", items.get(id))
    };
    assert_eq!(render(one), render(other));
}

/// Stack size is derived from the capabilities rather than from a list of
/// item kinds, so a *new* wearing capability gets the rule for free.
#[test]
fn wearing_capabilities_cap_the_stack_and_an_authored_size_still_wins() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_toml(
        "[[item]]\nid = \"plain\"\n\n\
         [[item]]\nid = \"digger\"\n[item.tool]\n\
         kind = \"pickaxe\"\ndig_speed = 1.0\ndurability = 5\n\n\
         [[item]]\nid = \"hat\"\n[item.equippable]\n\
         slot = \"helmet\"\ndefense = 1.0\ndurability = 5\n\n\
         [[item]]\nid = \"oddity\"\nmax_stack = 16\n[item.tool]\n\
         kind = \"pickaxe\"\ndig_speed = 1.0\ndurability = 5\n",
        &blocks,
    )
    .expect("valid file");
    let stack = |name: &str| items.max_stack(items.find(name).expect("declared"));
    assert_eq!(stack("plain"), 64, "nothing caps it");
    assert_eq!(stack("digger"), 1, "a tool wears");
    assert_eq!(stack("hat"), 1, "a worn piece wears");
    assert_eq!(stack("oddity"), 16, "an authored size wins over the rule");
}

/// An entry naming an auto-generated block item edits it rather than
/// shadowing it: capabilities it does not mention — placement above all —
/// survive, which is what lets `blue_bells` add a model and stay placeable.
#[test]
fn an_override_replaces_by_key_and_keeps_what_it_does_not_mention() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_toml(
        "[[item]]\nid = \"stone\"\nmax_stack = 32\n\n[item.consumable]\n\
         hunger = 1.0\nsaturation = 1.0\n",
        &blocks,
    )
    .expect("valid file");
    let stone = items.find("stone").expect("auto block item");
    assert_eq!(
        items.component::<Placeable>(stone).map(|p| p.block),
        blocks.find("stone"),
        "the inherited placement survives"
    );
    assert!(
        items.component::<Consumable>(stone).is_some(),
        "the declared capability is added"
    );
    assert_eq!(items.max_stack(stone), 32);
}

/// The whole point of asking for a capability: an item that has not got it
/// says so, instead of the caller testing what kind of item it is.
#[test]
fn an_absent_capability_reads_as_none() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_blocks(&blocks);
    let stick = items.find("stick").expect("stick");
    assert!(items.component::<Tool>(stick).is_none());
    assert!(items.component::<Consumable>(stick).is_none());
    assert!(!items.has(stick, "tool"));
    assert!(items.has(items.find("shears").expect("shears"), "tool"));
}

/// Every shipped id is well formed — the loader enforces it, but asserting
/// it here names the rule where the item set is snapshotted.
#[test]
fn every_builtin_item_id_is_well_formed() {
    let blocks = BlockRegistry::with_builtins();
    for (_, item) in ItemRegistry::from_blocks(&blocks).iter() {
        assert!(is_valid_id(&item.id), "malformed item id {:?}", item.id);
    }
}

/// A label is presentation, so like a model it rides out in `ItemVisuals`
/// and never reaches `Item`, which feeds `content_hash`.
#[test]
fn display_names_ride_out_of_band_and_stay_off_item() {
    let blocks = BlockRegistry::with_builtins();
    let mut visuals = ItemVisuals::default();
    let items = ItemRegistry::from_toml_with_visuals(
        "[[item]]\nid = \"tnt\"\ndisplay_name = \"TNT\"\n\n[[item]]\nid = \"stick\"\n",
        &blocks,
        &mut visuals,
    )
    .expect("valid file");

    assert_eq!(visuals.display_names.len(), items.len(), "one per item");
    let tnt = items.find("tnt").expect("declared");
    let stick = items.find("stick").expect("declared");
    assert_eq!(
        visuals.display_names[tnt.0 as usize].as_deref(),
        Some("TNT"),
        "authored"
    );
    assert_eq!(
        visuals.display_names[stick.0 as usize], None,
        "left for `content` to derive"
    );
    assert!(
        !format!("{:?}", items.get(tnt)).contains("TNT"),
        "display name must stay off Item"
    );
}

/// `[item.model]` is reported alongside the registry rather than stored on
/// `Item`: it is visual-only and must not reach `content_hash`.
#[test]
fn item_models_are_reported_out_of_band() {
    let blocks = BlockRegistry::with_builtins();
    let mut visuals = ItemVisuals::default();
    let items = ItemRegistry::from_toml_with_visuals(BUILTIN_ITEMS, &blocks, &mut visuals)
        .expect("builtin items parse");
    let models = &visuals.models;

    assert_eq!(models.len(), items.len(), "one entry per item");

    let sword = items.find("vine_sword").expect("vine_sword");
    let spec = models[sword.0 as usize]
        .as_ref()
        .expect("vine sword declares a model");
    // Not the exact extension: either export of this object is valid here,
    // and pinning one would make swapping formats a test failure.
    assert!(
        spec.path.starts_with("assets/models/items/vine_sword."),
        "unexpected model path {:?}",
        spec.path
    );
    assert_eq!(spec.scale, 0.35);
    assert_eq!(spec.offset, [-0.5, 0.75, -0.5]);

    // An item with no art yet declares no model either — flat items get one
    // extruded from their sprite, and `bread` has no sprite to extrude.
    let bread = items.find("bread").expect("bread");
    assert!(models[bread.0 as usize].is_none());

    // Nothing about the model leaks onto the item itself. The item's *id* is
    // legitimately "vine_sword", so what must be absent is the file it
    // points at.
    let debug = format!("{:?}", items.get(sword));
    assert!(
        !debug.contains("assets/models") && !debug.contains(".bbmodel"),
        "the model path leaked onto Item: {debug}"
    );
}

/// The tiered tools are flat in the XY plane, unlike `vine_sword`, so they
/// all carry the quarter-turn that stands them broadside in the fist. A
/// tool that silently lost it would render edge-on and near-invisible.
///
/// The extension is deliberately not pinned: tools are migrating from
/// `.bbmodel` to the Java Block/Item `.json` export one at a time, and a
/// re-authored tool places itself from its own `display` block instead. The
/// spec rotation stays as the fallback for any context that block does not
/// name, which is why it is still asserted for every tier.
#[test]
fn tiered_tool_models_are_turned_broadside() {
    let blocks = BlockRegistry::with_builtins();
    let mut visuals = ItemVisuals::default();
    let items = ItemRegistry::from_toml_with_visuals(BUILTIN_ITEMS, &blocks, &mut visuals)
        .expect("builtin items parse");
    let models = &visuals.models;

    for tier in ["wooden", "stone", "iron"] {
        for shape in ["pickaxe", "axe", "shovel", "sword"] {
            let name = format!("{tier}_{shape}");
            let id = items.find(&name).unwrap_or_else(|| panic!("{name} exists"));
            let spec = models[id.0 as usize]
                .as_ref()
                .unwrap_or_else(|| panic!("{name} declares a model"));
            let stem = format!("assets/models/items/{tier}_{shape}.");
            assert!(
                spec.path.starts_with(&stem)
                    && (spec.path.ends_with(".bbmodel") || spec.path.ends_with(".json")),
                "{name}: model path {}",
                spec.path
            );
            assert_eq!(spec.rotation, [0.0, 90.0, 0.0], "{name}: rotation");
        }
    }
}

/// A tier is only worth crafting if it strictly beats the one below it. The
/// gradient lives in `dig_speed` and `durability` for the digging shapes,
/// and in `damage` and `durability` for swords (which never dig).
#[test]
fn each_tool_shape_improves_with_every_tier() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_blocks(&blocks);

    let spec = |name: &str| {
        let id = items.find(name).unwrap_or_else(|| panic!("{name} exists"));
        items
            .component::<Tool>(id)
            .expect("tool capability")
            .clone()
    };

    for shape in ["pickaxe", "axe", "shovel"] {
        let tiers: Vec<_> = ["wooden", "stone", "iron"]
            .iter()
            .map(|tier| spec(&format!("{tier}_{shape}")))
            .collect();
        for pair in tiers.windows(2) {
            let (lo, hi) = (&pair[0], &pair[1]);
            assert!(hi.dig_speed > lo.dig_speed, "{shape}: dig_speed");
            assert!(hi.durability > lo.durability, "{shape}: durability");
            assert_eq!(
                hi.kind, lo.kind,
                "{shape}: kind is the tier-independent part"
            );
        }
    }

    let swords: Vec<_> = ["wooden", "stone", "iron"]
        .iter()
        .map(|tier| spec(&format!("{tier}_sword")))
        .collect();
    for pair in swords.windows(2) {
        let (lo, hi) = (&pair[0], &pair[1]);
        assert!(hi.damage > lo.damage, "sword: damage");
        assert!(hi.durability > lo.durability, "sword: durability");
        assert_eq!(hi.dig_speed, lo.dig_speed, "sword: dig_speed is flat");
    }
}

/// Digging tools deliberately do *not* fight better than a fist — only the
/// shapes with an edge declare `damage`.
#[test]
fn only_the_fighting_shapes_carry_damage() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_blocks(&blocks);

    for tier in ["wooden", "stone", "iron"] {
        for shape in ["pickaxe", "shovel"] {
            let name = format!("{tier}_{shape}");
            let id = items.find(&name).expect("tool exists");
            assert_eq!(
                items.component::<Tool>(id).unwrap().damage,
                None,
                "{name}: no damage"
            );
        }
        for shape in ["sword", "axe"] {
            let name = format!("{tier}_{shape}");
            let id = items.find(&name).expect("tool exists");
            assert!(
                items.component::<Tool>(id).unwrap().damage.is_some(),
                "{name}: declares damage"
            );
        }
    }
}

/// Each pickaxe's tier is what the ore gates are balanced against: the
/// starting tools cannot touch Darkwood's ore, the Elder Stag's antler
/// pickaxe can.
#[test]
fn pickaxe_tiers_climb_with_the_progression() {
    let blocks = BlockRegistry::with_builtins();
    let items = ItemRegistry::from_blocks(&blocks);
    let tier = |name: &str| {
        let id = items.find(name).unwrap_or_else(|| panic!("{name}"));
        items.get(id).get::<Tool>().expect("a tool").tier
    };
    assert_eq!(tier("wooden_pickaxe"), 1);
    assert_eq!(tier("stone_pickaxe"), 1);
    assert_eq!(tier("antler_pickaxe"), 2);
    assert_eq!(tier("iron_pickaxe"), 4);
    let tin = blocks.get(blocks.find("tin_ore").unwrap());
    assert_eq!(tin.harvest.as_ref().unwrap().tier, 2);
}

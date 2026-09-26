//! Loading the whole of `GameContent` — rules, visuals and sounds — from
//! real and fixture sources.

use super::*;
use crate::application::content::{BLOCKS_PATH, ENTITIES_PATH, ITEMS_PATH, WORLDGEN_PATH};
use crate::domain::core::Direction;
use crate::domain::world::BlockRegistry;
use crate::domain::world::block::BUILTIN_BLOCKS;

#[test]
fn builtin_content_loads() {
    let content = GameContent::builtin();
    assert!(!content.rules.blocks.is_empty());
    assert!(!content.rules.items.is_empty());
}

/// Models come off the real `assets/` tree: every path the shipped
/// definitions name must resolve to a real, non-empty model. Which export
/// they name is deliberately not asserted — the two formats describe the
/// same object and are meant to be swappable in the data files.
#[test]
fn models_named_by_definitions_are_loaded_and_resolved() {
    let content = GameContent::load();
    assert!(
        !content.visuals.models.is_empty(),
        "the shipped content names models"
    );
    for kind in content.rules.entities.iter() {
        if let Some(path) = kind.visual.model_path() {
            let id = content
                .visuals
                .models
                .find(path)
                .unwrap_or_else(|| panic!("{} names an unloadable model", kind.name));
            assert!(
                content
                    .visuals
                    .models
                    .get(id)
                    .is_some_and(|m| m.triangle_count() > 0)
            );
        }
    }

    let sword = content
        .rules
        .items
        .find("vine_sword")
        .expect("vine sword item");
    let model = content.visuals.item_models[sword.0 as usize].expect("vine sword has a model");
    assert_eq!(model.scale, 0.35);
    let loaded = content
        .visuals
        .models
        .get(model.id)
        .expect("model is in the registry");
    // The Java export, not the `.bbmodel` beside it: only that one carries a
    // `display` block, which is what places the sword in each hand. It has
    // fewer triangles because a Java export writes only the faces the author
    // actually textured.
    assert_eq!(loaded.triangle_count(), 204);

    // `item_models` is indexed by `ItemId`, so it must cover every item even
    // though almost none of them declare a model.
    assert_eq!(content.visuals.item_models.len(), content.rules.items.len());
}

/// Every `[block.model]` in the shipped data must resolve to real geometry,
/// and — because a block and the item that places it name the same file —
/// both must land on the *same* `ModelId`, so the pair costs one parse, one
/// GPU texture and one icon cell rather than two.
#[test]
fn block_models_load_and_share_their_items_model_id() {
    let content = GameContent::load();

    // Indexed by `BlockId`, so it must cover every block.
    assert_eq!(
        content.visuals.block_models.len(),
        content.rules.blocks.len()
    );

    let mut declared = Vec::new();
    for (id, block) in content.rules.blocks.iter() {
        let Some(model) = content.visuals.block_models[id.0 as usize] else {
            continue;
        };
        let loaded = content
            .visuals
            .models
            .get(model.id)
            .unwrap_or_else(|| panic!("{}: model id is not in the registry", block.id));
        assert!(
            loaded.triangle_count() > 0,
            "{}: model has no geometry",
            block.id
        );
        assert!(model.scale > 0.0, "{}: non-positive scale", block.id);

        let item = content
            .rules
            .items
            .find(&block.id)
            .unwrap_or_else(|| panic!("{}: no placeable item", block.id));
        let item_model = content.visuals.item_models[item.0 as usize]
            .unwrap_or_else(|| panic!("{}: item declares no model", block.id));
        assert_eq!(
            item_model.id, model.id,
            "{}: block and item parsed the file twice",
            block.id
        );
        declared.push(block.id.clone());
    }

    assert_eq!(
        declared,
        ["blue_bells", "red_flower", "red_mushroom", "brown_mushroom"],
        "shipped model-backed blocks"
    );
}

/// Ground cover must not be targetable across the whole cell it stands in.
/// The boxes are derived from the models, so this also catches a model
/// re-export that quietly changed size.
#[test]
fn ground_cover_hitboxes_are_smaller_than_their_cells() {
    let content = GameContent::load();
    // (name, width, height) as measured off the shipped models. A golden:
    // a re-export that changes a plant's size should show up here rather
    // than as a crosshair that quietly stops feeling right.
    let expected = [
        ("blue_bells", 0.83, 0.99),
        ("red_flower", 0.49, 0.63),
        ("red_mushroom", 0.41, 0.56),
        ("brown_mushroom", 0.83, 0.63),
    ];
    for (name, width, height) in expected {
        let id = content.rules.blocks.find(name).expect("shipped block");
        let hitbox = content.visuals.block_models[id.0 as usize]
            .expect("has a model")
            .hitbox;
        let size = hitbox.max - hitbox.min;
        assert!(
            (size.x - width).abs() < 0.02 && (size.y - height).abs() < 0.02,
            "{name}: {size:?} is not {width} x {height}"
        );
        // Square in plan, so `random_yaw` cannot leave it lopsided...
        assert!((size.x - size.z).abs() < 1e-5, "{name}: not square in plan");
        // ...centred, standing on the cell floor, and inside the cell...
        assert!(
            (size.x * 0.5 + hitbox.min.x - 0.5).abs() < 1e-5,
            "{name}: off-centre"
        );
        assert_eq!(hitbox.min.y, 0.0, "{name}: floating");
        assert!(
            hitbox.min.x >= 0.0 && hitbox.max.x <= 1.0,
            "{name}: escapes its cell"
        );
        // ...smaller than the cell it stands in, which is the whole point...
        assert!(size.x < 1.0 && size.y < 1.0, "{name}: fills its cell");
        // ...and big enough to actually click on.
        assert!(
            size.x >= 0.1 && size.y >= 0.1,
            "{name}: {size:?} too small to hit"
        );
    }
}

/// A block with neither `textures` nor `[block.model]` would render as the
/// magenta marker on all six faces. Rejecting the file is louder, and the
/// caller still falls back to the builtin blocks.
#[test]
fn a_block_without_textures_or_a_model_is_rejected() {
    let bad = r#"
        [[block]]
        name = "ghost"
        render = "opaque"
        solid = true
        hardness = 1.0
    "#;
    let err = BlockRegistry::from_toml(bad).expect_err("must not parse");
    assert!(err.contains("ghost"), "{err}");
}

/// Every `[item.model]` in the shipped data must resolve to real geometry.
///
/// This reads the actual `assets/` tree, so it is the check that catches a
/// The shipped water strip must actually load: a mistyped path or a
/// mis-shaped PNG degrades water to the magenta marker, since it no longer
/// declares any atlas `textures` to fall back to.
#[test]
fn the_shipped_water_animation_loads_for_every_fluid_block() {
    let content = GameContent::load();
    let water = content.rules.blocks.find("water").expect("shipped block");
    let tex = content.visuals.fluid_textures[water.0 as usize].expect("water is animated");
    assert_eq!(tex.still.frames, 64);
    assert_eq!(tex.flowing.frames, 64);
    assert_ne!(
        tex.still.first, tex.flowing.first,
        "the two columns are separate runs of layers"
    );
    assert_eq!(tex.tint, Some(2), "water takes the biome water colour");

    // The strip's own alpha is a placeholder; `opacity` is what decides how
    // much of the riverbed shows through, since a body of water is one
    // blended sheet however deep it is.
    let frame = content
        .visuals
        .block_textures
        .layer(tex.still.first)
        .expect("still frame");
    let alpha = frame.pixels[3];
    assert!(
        (200..255).contains(&alpha),
        "water should read as water, not as glass: alpha {alpha}"
    );

    // The inventory icon and the dropped-item cube still sample the atlas,
    // so a fluid needs a real stand-in tile there rather than the marker.
    let faces = content.face_textures(water);
    assert_ne!(
        faces.tile(crate::domain::core::Direction::PosY),
        0,
        "water must not fall back to the missing-texture tile"
    );

    // The auto-registered flowing blocks share the source's entry, or a
    // spreading stream would fall back to the marker halfway down a hill.
    for level in 1..=7 {
        let id = content.rules.blocks.flowing(0, level);
        assert_eq!(
            content.visuals.fluid_textures[id.0 as usize],
            Some(tex),
            "water flow {level}"
        );
    }
}

/// A fluid whose strip cannot be read must not fail the load — it degrades
/// to the block's own `textures`, like every other content failure.
#[test]
fn an_unreadable_fluid_strip_degrades_to_no_animation() {
    let blocks = BUILTIN_BLOCKS.replace(
        "assets/textures/blocks/water_flow.png",
        "assets/textures/blocks/no_such_fluid.png",
    );
    let content = GameContent::from_source(&MapSource::new().with(BLOCKS_PATH, &blocks));
    let water = content.rules.blocks.find("water").expect("declared");
    assert!(content.visuals.fluid_textures[water.0 as usize].is_none());
    assert_eq!(
        content.rules.blocks.len(),
        BlockRegistry::with_builtins().len(),
        "the rest of the block set is untouched"
    );
}

/// Every block naming a `block_model` must actually have baked geometry —
/// a mistyped path or an unreadable export otherwise degrades quietly to an
/// invisible block, which is far harder to notice than a broken icon.
#[test]
fn every_blockbench_block_in_the_shipped_data_loads() {
    let content = GameContent::load();
    let modelled: Vec<&str> = content
        .visuals
        .baked_models
        .iter()
        .enumerate()
        .filter(|(_, m)| m.is_some())
        .map(|(id, _)| {
            content
                .rules
                .blocks
                .get(crate::domain::core::BlockId(id as u16))
                .id
                .as_str()
        })
        .collect();
    assert_eq!(
        modelled,
        vec![
            "stone",
            "dirt",
            "grass",
            "sand",
            "oak_log",
            "oak_leaves",
            "snow",
            "gravel",
            "coal_ore",
            "iron_ore",
            "copper_ore",
            "cobblestone",
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
        ],
        "the blocks migrated to Blockbench so far"
    );

    // Solid terrain cubes must fill their cell so neighbours can cull
    // against them. The other two legitimately do not: leaves are a cutout,
    // and a cornflower is two crossed planes.
    let see_through = ["oak_leaves", "cornflower"];

    for (id, baked) in content.visuals.baked_models.iter().enumerate() {
        let Some(baked) = baked else { continue };
        let name = content
            .rules
            .blocks
            .get(crate::domain::core::BlockId(id as u16))
            .id
            .as_str();
        assert!(!baked.quads.is_empty(), "{name}: no geometry");
        let expected = !see_through.contains(&name);
        assert_eq!(
            baked.occludes, [expected; 6],
            "{name}: wrong occlusion for what it is"
        );
        for quad in &baked.quads {
            assert_ne!(quad.layer, 0, "{name}: sampling the missing-texture layer");
        }
    }
}

/// A flower is two crossed planes, so the crosshair must not reach it from
/// the far corner of its cell — and its box must be a real box, not the
/// inverted one a model with no vertical extent used to produce.
#[test]
fn a_modelled_plant_gets_a_hitbox_smaller_than_its_cell() {
    let content = GameContent::load();
    let id = content
        .rules
        .blocks
        .find("cornflower")
        .expect("shipped block");
    let baked = content.visuals.baked_models[id.0 as usize]
        .as_ref()
        .expect("cornflower is modelled");

    assert!(baked.random_yaw, "flowers vary their angle");
    let hitbox = baked.hitbox.expect("a plant does not fill its cell");
    let size = hitbox.max - hitbox.min;
    assert!(size.x > 0.0 && size.y > 0.0 && size.z > 0.0, "{size}");
    assert!(size.y <= 1.0, "taller than its cell: {size}");

    // A full cube needs no box of its own — the raycast marches through it.
    let stone = content.rules.blocks.find("stone").expect("shipped block");
    let cube = content.visuals.baked_models[stone.0 as usize]
        .as_ref()
        .expect("stone is modelled");
    assert!(
        cube.hitbox.is_none(),
        "a full cube should stay a plain cell"
    );
}

/// Dropped stacks and inventory icons still draw six-sided atlas cubes, so a
/// Blockbench block needs a small stand-in tile per face derived from its own
/// textures. Without it they would all show the magenta marker.
#[test]
fn a_blockbench_block_has_derived_atlas_tiles_for_its_faces() {
    let content = GameContent::load();
    let grass = content.rules.blocks.find("grass").expect("shipped block");

    // `Block` carries no tile index at all any more — every one of them is
    // derived from art, and art must never reach `content_hash`.

    let faces = content.face_textures(grass);
    for dir in Direction::ALL {
        assert_ne!(
            faces.tile(dir),
            0,
            "{dir:?} fell back to the missing marker"
        );
    }
    // The top and the bottom come from different art (grass vs dirt).
    assert_ne!(faces.tile(Direction::PosY), faces.tile(Direction::NegY));
}

/// mistyped path, a Blockbench export the loader cannot read, or a model
/// saved without its texture — all of which otherwise degrade quietly to a
/// magenta icon at runtime.
#[test]
fn every_item_model_in_the_shipped_data_loads() {
    let content = GameContent::load();
    let mut declared = Vec::new();

    for (id, item) in content.rules.items.iter() {
        let index = id.0 as usize;
        let Some(model) = content.visuals.item_models[index] else {
            continue;
        };
        let loaded = content
            .visuals
            .models
            .get(model.id)
            .unwrap_or_else(|| panic!("{}: model id is not in the registry", item.id));
        assert!(
            loaded.triangle_count() > 0,
            "{}: model has no geometry",
            item.id
        );
        assert!(model.scale > 0.0, "{}: non-positive scale", item.id);
        // An item with a model always icons as that model.
        assert!(
            matches!(content.visuals.item_icons[index], ItemIcon::Model(drawn) if drawn == model.id),
            "{}: does not icon as its model",
            item.id
        );
        declared.push(item.id.clone());
    }

    // The thirteen tiered tools (the antler pickaxe among them), the vine
    // sword, the four ground-cover blocks whose items are drawn as their
    // own model, and the twenty-seven flat items extruded from their
    // sprites by `item/generated`.
    assert_eq!(declared.len(), 45, "declared item models: {declared:?}");
}

/// The shipped items file with every model path pointed somewhere else.
/// Extension-agnostic on purpose: the data files are meant to be able to
/// name either export, and a fixture keyed to one of them would quietly
/// stop substituting anything the day the other is chosen.
fn items_with_repointed_models() -> String {
    let base = crate::domain::inventory::item::BUILTIN_ITEMS;
    let repointed = base.replace("assets/models/items/vine_sword", "assets/models/elsewhere");
    assert_ne!(base, repointed, "fixture substituted nothing");
    repointed
}

/// A dropped block is a miniature of itself; a dropped apple is its icon.
/// Wrapping a flat icon around a cube is what this split exists to stop.
#[test]
fn only_block_items_are_shaped_as_cubes() {
    let content = GameContent::builtin();
    let shape = |id: &str| {
        let item = content
            .rules
            .items
            .find(id)
            .unwrap_or_else(|| panic!("no {id}"));
        content.item_shape(item)
    };
    assert!(matches!(shape("dirt"), ItemShape::Cube(_)), "dirt");
    assert!(matches!(shape("apple"), ItemShape::Sprite(_)), "apple");
    assert!(matches!(shape("coal"), ItemShape::Sprite(_)), "coal");
    // Water is a fluid: its icon is derived from the still frame, so it is
    // flat art like any other icon rather than six faces of a cube.
    assert!(matches!(shape("water"), ItemShape::Sprite(_)), "water");
}

/// The shipped apple art has transparent corners, so its extruded silhouette
/// must come out smaller than a full card. This is what proves the alpha
/// tracing reads real art and not just the synthetic fixtures in the mesher.
#[test]
fn a_real_icon_traces_less_than_a_full_card() {
    let content = GameContent::builtin();
    let apple = content.rules.items.find("apple").expect("no apple");
    let ItemShape::Sprite(tile) = content.item_shape(apple) else {
        panic!("apple should be a sprite");
    };
    let art = content.visuals.tiles.art(tile).expect("apple art");
    let full = wyven_voxel::ItemSprite::new(tile, None).rim_len();
    let traced = wyven_voxel::ItemSprite::new(tile, Some(art)).rim_len();
    assert!(traced > 0, "apple traced no silhouette at all");
    assert!(
        traced < full,
        "apple traced a full card ({traced} of {full})"
    );
}

/// A model is visual-only: swapping one must not change the fingerprint that
/// gates multiplayer joins, or two players with differently-drawn swords
/// would be refused a shared world.
#[test]
fn item_models_do_not_feed_the_content_hash() {
    // Both sides come from the same source kind, so the model path is the
    // only thing that differs between them.
    let base = GameContent::from_source(
        &MapSource::new().with(ITEMS_PATH, crate::domain::inventory::item::BUILTIN_ITEMS),
    );
    let repointed =
        GameContent::from_source(&MapSource::new().with(ITEMS_PATH, items_with_repointed_models()));
    assert_eq!(
        base.rules.items.len(),
        repointed.rules.items.len(),
        "the items themselves are unchanged"
    );
    assert_eq!(base.rules.hash(), repointed.rules.hash());
}

/// A label is presentation for exactly the same reason a model is: two peers
/// running a translated items file must still be able to share a world.
#[test]
fn display_names_do_not_feed_the_content_hash() {
    let base_text = crate::domain::inventory::item::BUILTIN_ITEMS;
    // Give every declared item a label it did not have before.
    let relabelled = base_text.replace("\n[[item]]\n", "\n[[item]]\ndisplay_name = \"Ding\"\n");
    assert_ne!(base_text, relabelled, "fixture substituted nothing");

    let base = GameContent::from_source(&MapSource::new().with(ITEMS_PATH, base_text));
    let renamed = GameContent::from_source(&MapSource::new().with(ITEMS_PATH, relabelled));

    assert!(
        renamed
            .visuals
            .item_display_names
            .iter()
            .any(|n| n == "Ding"),
        "the fixture's labels did not take effect"
    );
    assert_eq!(
        base.rules.items.len(),
        renamed.rules.items.len(),
        "same items"
    );
    assert_eq!(base.rules.hash(), renamed.rules.hash());
}

/// The three ways an item gets its label, in precedence order.
#[test]
fn item_display_names_resolve_by_authored_then_block_then_id() {
    let content = GameContent::load();

    // Derived from the id: nothing in the shipped data spells this one out.
    let pickaxe = content
        .rules
        .items
        .find("wooden_pickaxe")
        .expect("shipped item");
    assert_eq!(content.item_display_name(pickaxe), "Wooden Pickaxe");

    // A block item takes the block's label, so the two can never disagree.
    let log_block = content.rules.blocks.find("oak_log").expect("shipped block");
    let log_item = content
        .rules
        .items
        .item_for_block(log_block)
        .expect("oak_log is placeable");
    assert_eq!(content.block_display_name(log_block), "Oak Log");
    assert_eq!(content.item_display_name(log_item), "Oak Log");

    // Every item has *some* label, and none of them leak the underscored id.
    assert_eq!(
        content.visuals.item_display_names.len(),
        content.rules.items.len()
    );
    for (id, item) in content.rules.items.iter() {
        let label = content.item_display_name(id);
        assert!(!label.is_empty(), "{}: empty label", item.id);
        assert!(
            !label.contains('_'),
            "{}: label kept an underscore",
            item.id
        );
    }
}

/// An authored `display_name` wins over the derived one, and a block item
/// with no label of its own inherits the block's authored label.
#[test]
fn an_authored_display_name_overrides_the_derived_one() {
    let blocks = format!(
        "{BUILTIN_BLOCKS}\n\
         [[block]]\n\
         id = \"tnt\"\n\
         display_name = \"TNT\"\n\
         render = \"opaque\"\n\
         solid = true\n\
         hardness = 1.0\n\
         textures = \"stone\"\n"
    );
    let content = GameContent::from_source(&MapSource::new().with(BLOCKS_PATH, blocks));

    let block = content.rules.blocks.find("tnt").expect("fixture block");
    let item = content.rules.items.find("tnt").expect("its placeable item");
    assert_eq!(content.block_display_name(block), "TNT");
    assert_eq!(
        content.item_display_name(item),
        "TNT",
        "the block item inherits the block's label rather than deriving \"Tnt\""
    );
}

/// A model path that does not resolve degrades that one item's appearance
/// and nothing else — the world still boots.
#[test]
fn an_unloadable_model_leaves_the_rest_of_the_content_intact() {
    let content =
        GameContent::from_source(&MapSource::new().with(ITEMS_PATH, items_with_repointed_models()));
    let sword = content
        .rules
        .items
        .find("vine_sword")
        .expect("item still exists");
    assert!(content.visuals.item_models[sword.0 as usize].is_none());
    assert!(!content.rules.items.is_empty());
    assert!(content.visuals.models.is_empty(), "nothing resolved");
}

/// An empty source serves no files, so every registry must fall back to its
/// builtin — i.e. `from_source` over nothing is exactly `builtin()`. This is
/// what lets the two constructors share one code path.
#[test]
fn an_empty_source_is_the_builtin_content() {
    let empty = GameContent::from_source(&MapSource::new());
    let builtin = GameContent::builtin();
    assert_eq!(empty.rules.hash(), builtin.rules.hash());
    assert_eq!(empty.rules.blocks.len(), builtin.rules.blocks.len());
    assert_eq!(empty.rules.items.len(), builtin.rules.items.len());
}

/// Definitions really do come from the source: a fixture adding a block
/// yields a registry containing it, with its item and icon derived.
///
/// The fixture extends the builtin blocks rather than replacing them,
/// because `worldgen.toml` names concrete blocks ("wood", "stone", ...) and
/// resolving it against a registry missing them is a hard error by design.
#[test]
fn definitions_are_read_from_the_source() {
    let blocks = format!(
        "{BUILTIN_BLOCKS}\n\
         [[block]]\n\
         id = \"testonium\"\n\
         render = \"opaque\"\n\
         solid = true\n\
         hardness = 1.0\n\
         textures = \"stone\"\n"
    );
    let content = GameContent::from_source(&MapSource::new().with(BLOCKS_PATH, blocks));

    let builtin = GameContent::builtin();
    assert_eq!(
        content.rules.blocks.len(),
        builtin.rules.blocks.len() + 1,
        "the fixture's block is registered on top of the builtins"
    );
    assert!(content.rules.blocks.find("testonium").is_some());
    // The auto-generated placeable item comes with it, and every item
    // resolved an icon — proving the fixture's textures reached the same
    // TileRegistry the atlas is built from.
    assert!(content.rules.items.find("testonium").is_some());
    assert_eq!(content.visuals.item_icons.len(), content.rules.items.len());
    assert_ne!(
        content.rules.hash(),
        builtin.rules.hash(),
        "new block changes the hash"
    );
}

/// Fail-soft is per file: a malformed blocks.toml costs only the blocks,
/// and the other four registries still load from the source.
#[test]
fn a_malformed_file_falls_back_alone() {
    let source = MapSource::new()
        .with(BLOCKS_PATH, "this is not valid toml {{{")
        .with(
            ENTITIES_PATH,
            crate::domain::entity::kind::BUILTIN_ENTITIES
                .replace("max_health = 20.0", "max_health = 17.0"),
        );
    let content = GameContent::from_source(&source);

    let builtin = GameContent::builtin();
    assert_eq!(
        content.rules.blocks.len(),
        builtin.rules.blocks.len(),
        "bad blocks.toml falls back to the builtin blocks"
    );
    // ...while the entities file, which parsed fine, was still honoured.
    assert_ne!(
        content.rules.hash(),
        builtin.rules.hash(),
        "the tweaked entities file must still take effect"
    );
}

/// Worldgen is strict by design (an unknown block name would silently
/// generate the wrong terrain), so a bad name rejects the whole file.
#[test]
fn unknown_worldgen_block_rejects_the_file() {
    let bad = crate::domain::world::generation::config::BUILTIN_WORLDGEN
        .replace("bedrock = \"bedrock\"", "bedrock = \"no such block\"");
    let content = GameContent::from_source(&MapSource::new().with(WORLDGEN_PATH, bad));
    let builtin = GameContent::builtin();
    assert_eq!(
        content.rules.hash(),
        builtin.rules.hash(),
        "a rejected worldgen file leaves the builtin content in place"
    );
}

/// A sound is presentation exactly like a texture or a model: two peers
/// running different sound packs, or no audio device at all, must still
/// be able to share a world.
#[test]
fn sound_registry_does_not_feed_the_content_hash() {
    let base = GameContent::from_source(
        &MapSource::new().with(AUDIO_PATH, crate::infrastructure::audio::BUILTIN_AUDIO),
    );
    let extra = format!(
        "{}\n[[sound]]\nid = \"extra\"\npath = \"x.wav\"\ncategory = \"sfx\"\nvolume = 1.0\n",
        crate::infrastructure::audio::BUILTIN_AUDIO
    );
    let changed = GameContent::from_source(&MapSource::new().with(AUDIO_PATH, extra));
    assert_ne!(
        base.sounds.len(),
        changed.sounds.len(),
        "fixture actually differs"
    );
    assert_eq!(base.rules.hash(), changed.rules.hash());
}

//! Tests for [`super`]: `block.rs`.

use super::*;

/// The shader's animation clock wraps every 3600 s. A loop that does not
/// divide that evenly jumps mid-swell once an hour — subtle enough to ship
/// by accident, so the loader refuses it outright.
#[test]
fn a_fluid_loop_that_does_not_divide_the_animation_clock_is_rejected() {
    // 64 frames at 6 fps: 6 * 3600 leaves 32 frames over.
    let bad = BUILTIN_BLOCKS.replace("fps = 8", "fps = 6");
    let err = BlockRegistry::from_toml(&bad).expect_err("must not parse");
    assert!(err.contains("3600"), "{err}");

    // ...while the shipped pairing, and every other multiple of 4, is fine.
    for fps in ["4", "8", "12", "20"] {
        let text = BUILTIN_BLOCKS.replace("fps = 8", &format!("fps = {fps}"));
        assert!(
            BlockRegistry::from_toml(&text).is_ok(),
            "{fps} fps over 64 frames should be accepted"
        );
    }
}

/// Golden snapshot of the shipped block set. The data-driven loader must
/// reproduce this exactly: ids are the save format (matched verbatim by
/// [`BlockRegistry::find`]) and registration order defines the numeric ids
/// used in chunk storage and on the wire.
#[test]
fn builtin_blocks_golden() {
    use RenderType as R;
    const INF: f32 = f32::INFINITY;
    const NONE: &[&str] = &[];
    const PICK: &[&str] = &["pickaxe"];
    const AXE: &[&str] = &["axe"];
    const SHOVEL: &[&str] = &["shovel"];
    const SHEARS: &[&str] = &["shears"];
    const CUTTERS: &[&str] = &["shears", "sword"];
    /// id, render, solid, hardness, the tools it asks for, whether it insists.
    type BlockRow = (&'static str, R, bool, f32, &'static [&'static str], bool);
    let expected: [BlockRow; 42] = [
        ("air", R::Invisible, false, 0.0, NONE, false),
        ("stone", R::Opaque, true, 1.5, PICK, false),
        ("dirt", R::Opaque, true, 0.5, SHOVEL, false),
        ("grass", R::Opaque, true, 0.6, SHOVEL, false),
        ("sand", R::Opaque, true, 0.5, SHOVEL, false),
        ("water", R::Transparent, false, INF, NONE, false),
        ("oak_log", R::Opaque, true, 2.0, AXE, false),
        ("oak_leaves", R::Cutout, true, 0.2, SHEARS, true),
        ("glass", R::Transparent, true, 0.3, NONE, false),
        ("bedrock", R::Opaque, true, INF, NONE, false),
        ("snow", R::Opaque, true, 0.2, SHOVEL, false),
        ("gravel", R::Opaque, true, 0.6, SHOVEL, false),
        ("clay", R::Opaque, true, 0.6, SHOVEL, false),
        ("coal_ore", R::Opaque, true, 3.0, PICK, false),
        ("iron_ore", R::Opaque, true, 3.0, PICK, false),
        ("copper_ore", R::Opaque, true, 3.0, PICK, false),
        ("cobblestone", R::Opaque, true, 2.0, PICK, false),
        ("blue_bells", R::Cutout, false, 0.0, CUTTERS, false),
        ("red_flower", R::Cutout, false, 0.0, CUTTERS, false),
        ("red_mushroom", R::Cutout, false, 0.0, CUTTERS, false),
        ("brown_mushroom", R::Cutout, false, 0.0, CUTTERS, false),
        ("cornflower", R::Cutout, false, 0.0, CUTTERS, false),
        ("deepstone", R::Opaque, true, 3.0, PICK, false),
        ("mud", R::Opaque, true, 0.5, SHOVEL, false),
        ("packed_snow", R::Opaque, true, 0.8, SHOVEL, false),
        ("basalt", R::Opaque, true, 2.0, PICK, false),
        ("ash", R::Opaque, true, 0.5, SHOVEL, false),
        ("mossy_cobblestone", R::Opaque, true, 2.0, PICK, false),
        ("tin_ore", R::Opaque, true, 3.0, PICK, false),
        ("silver_ore", R::Opaque, true, 4.0, PICK, false),
        ("cinder_ore", R::Opaque, true, 5.0, PICK, false),
        // Structure hearts: unbreakable, so no harvest table.
        ("wayrune", R::Opaque, true, INF, NONE, false),
        ("elder_altar", R::Opaque, true, INF, NONE, false),
        // Crafting stations.
        ("workbench", R::Opaque, true, 2.0, AXE, false),
        ("forge", R::Opaque, true, 3.0, PICK, false),
        // A flowing block inherits its source's harvest rule, which for
        // water is "nothing is good at it".
        ("water_flow_1", R::Transparent, false, INF, NONE, false),
        ("water_flow_2", R::Transparent, false, INF, NONE, false),
        ("water_flow_3", R::Transparent, false, INF, NONE, false),
        ("water_flow_4", R::Transparent, false, INF, NONE, false),
        ("water_flow_5", R::Transparent, false, INF, NONE, false),
        ("water_flow_6", R::Transparent, false, INF, NONE, false),
        ("water_flow_7", R::Transparent, false, INF, NONE, false),
    ];
    let reg = BlockRegistry::with_builtins();
    assert_eq!(reg.len(), expected.len(), "block count changed");
    for (i, &(id, render, solid, hardness, tools, required)) in expected.iter().enumerate() {
        let block = reg.get(BlockId(i as u16));
        assert_eq!(block.id, id, "block {i}: id");
        assert_eq!(block.render, render, "{id}: render");
        assert_eq!(block.solid, solid, "{id}: solid");
        assert_eq!(block.hardness, hardness, "{id}: hardness");
        match &block.harvest {
            Some(harvest) => {
                assert_eq!(harvest.tools, tools, "{id}: harvest tools");
                assert_eq!(harvest.required, required, "{id}: harvest required");
            }
            None => assert!(tools.is_empty(), "{id}: expected a harvest table"),
        }
    }
}

/// A block names one tool or several; both spellings land in the same
/// `Vec`, so nothing downstream has to know which was written.
#[test]
fn a_harvest_table_accepts_one_tool_or_a_list() {
    let one = parse(
        r#"
        [[block]]
        id = "rock"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"
        [block.harvest]
        tool = "pickaxe"
    "#,
    )
    .expect("valid file");
    let harvest = one.get(BlockId(1)).harvest.as_ref().expect("harvest");
    assert_eq!(harvest.tools, ["pickaxe"]);
    assert!(!harvest.required, "a gate is opt-in");

    let many = parse(
        r#"
        [[block]]
        id = "weed"
        render = "cutout"
        solid = false
        hardness = 0.0
        textures = "stone"
        [block.harvest]
        tool = ["shears", "sword"]
        required = true
    "#,
    )
    .expect("valid file");
    let harvest = many.get(BlockId(1)).harvest.as_ref().expect("harvest");
    assert_eq!(harvest.tools, ["shears", "sword"]);
    assert!(harvest.accepts("sword") && !harvest.accepts("axe"));
    assert!(harvest.required);
}

/// `[block.harvest]` with nothing in it is either a half-finished edit or,
/// with `required`, a block nothing in the game could ever harvest. Both
/// are worth failing the file over — omitting the table already says "no
/// tool is good at this".
#[test]
fn a_harvest_table_naming_no_tool_is_rejected() {
    let err = parse(
        r#"
        [[block]]
        id = "rock"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"
        [block.harvest]
        tool = []
    "#,
    )
    .expect_err("an empty tool list says nothing");
    assert!(err.contains("rock"), "unhelpful error {err:?}");
}

/// A tool kind is matched against `[item.tool] kind` by exact string, so a
/// spelling no item could write is a fault rather than a silent mismatch.
#[test]
fn a_malformed_tool_kind_is_rejected() {
    assert!(
        parse(
            r#"
        [[block]]
        id = "rock"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"
        [block.harvest]
        tool = "Pick Axe"
    "#,
        )
        .is_err()
    );
}

/// The rule the whole game is built on, pinned where the block table can
/// see it: naming a tool makes a block *faster* to mine, never gated. Only
/// a block that says `required` withholds its drop.
#[test]
fn naming_a_tool_does_not_by_itself_gate_the_drop() {
    let reg = BlockRegistry::with_builtins();
    let gated: Vec<&str> = reg
        .iter()
        .filter(|(_, b)| b.harvest.as_ref().is_some_and(|h| h.required))
        .map(|(_, b)| b.id.as_str())
        .collect();
    assert_eq!(gated, ["oak_leaves"], "only leaves insist on their tool");
}

/// Every shipped id is well formed. The loader enforces this, but asserting
/// it here names the rule where the block set is snapshotted.
#[test]
fn every_builtin_block_id_is_well_formed() {
    for (_, block) in BlockRegistry::with_builtins().iter() {
        assert!(is_valid_id(&block.id), "malformed block id {:?}", block.id);
    }
}

#[test]
fn water_levels_match_registry_order() {
    let reg = BlockRegistry::with_builtins();
    assert_eq!(reg.find("water"), Some(blocks::WATER));
    assert_eq!(reg.find("water_flow_1"), Some(blocks::WATER_FLOW_1));
    assert_eq!(reg.find("water_flow_7"), Some(blocks::WATER_FLOW_7));

    let source = reg.fluid(blocks::WATER).expect("water is a fluid");
    assert!(source.is_source());
    assert_eq!((source.group, source.level, source.max_level), (0, 8, 8));
    for level in 1..=7u8 {
        let id = reg.flowing(0, level);
        let f = reg.fluid(id).expect("flow block is a fluid");
        assert_eq!((f.group, f.level, f.max_level), (0, level, 8));
        assert!(reg.is_fluid(id) && reg.is_flowing_fluid(id));
    }
    assert!(reg.is_fluid(blocks::WATER) && !reg.is_flowing_fluid(blocks::WATER));
    assert_eq!(reg.fluid(blocks::STONE), None);
    assert_eq!(reg.fluid(BlockId::AIR), None);
}

/// The behavior components declared in blocks.toml parse into the typed
/// component fields.
#[test]
fn blocks_toml_components_parse() {
    let reg = BlockRegistry::with_builtins();
    // Leaves drop themselves, but only for shears — and that gate is the
    // block's `[block.harvest]`, not a shape of drop rule, so it composes
    // with any of them.
    assert_eq!(reg.get(blocks::OAK_LEAVES).drops, Drops::SelfItem);
    let leaves = reg
        .get(blocks::OAK_LEAVES)
        .harvest
        .as_ref()
        .expect("harvest");
    assert!(leaves.required, "leaves insist on their tool");
    assert!(leaves.accepts("shears"));
    assert!(!leaves.accepts("axe"));
    // Mining stone yields cobblestone, exactly as the recipes assume.
    assert_eq!(
        reg.get(blocks::STONE).drops,
        Drops::Item {
            id: "cobblestone".into(),
            count: 1
        }
    );
    assert_eq!(reg.get(blocks::WATER_FLOW_1).drops, Drops::None);
    // Flow blocks inherit the source's look and physics.
    let (water, flow) = (reg.get(blocks::WATER), reg.get(blocks::WATER_FLOW_1));
    assert_eq!(flow.render, water.render);
    assert_eq!(flow.solid, water.solid);
    assert_eq!(flow.hardness, water.hardness);
}

/// Parse a blocks file with a throwaway tile registry.
fn parse(text: &str) -> Result<BlockRegistry, String> {
    BlockRegistry::from_toml(text)
}

/// Structural errors reject the whole file (the loader then falls back to
/// the builtin copy); reordering trips the well-known-id validation.
#[test]
fn invalid_block_files_are_rejected() {
    assert!(parse("not toml [").is_err());
    assert!(parse("").is_err());
    let air = r#"
        [[block]]
        id = "air"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"
    "#;
    assert!(parse(air).is_err());
    let dup = r#"
        [[block]]
        id = "stone"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"

        [[block]]
        id = "stone"
        render = "opaque"
        solid = true
        hardness = 1.0
        textures = "stone"
    "#;
    assert!(parse(dup).is_err());
    // A minimal but valid file parses fine — numeric ids are session-local,
    // so files are free to define any block set.
    let minimal = parse(
        r#"
        [[block]]
        id = "dirt"
        render = "opaque"
        solid = true
        hardness = 0.5
        textures = "dirt"
    "#,
    )
    .expect("valid file");
    assert_eq!(minimal.len(), 2, "air + dirt");
}

/// A malformed id rejects the whole table rather than just its own entry:
/// dropping one block would renumber every later `BlockId` and silently
/// orphan the worldgen/recipe/drop references pointing past it.
#[test]
fn a_malformed_id_rejects_the_whole_file() {
    for bad in ["Oak Log", "oak log", "oak-log", "OAKLOG", ""] {
        let text = format!(
            r#"
            [[block]]
            id = "dirt"
            render = "opaque"
            solid = true
            hardness = 0.5
            textures = "dirt"

            [[block]]
            id = "{bad}"
            render = "opaque"
            solid = true
            hardness = 2.0
            textures = "wood_bark"
        "#
        );
        let err = parse(&text)
            .err()
            .unwrap_or_else(|| panic!("{bad:?} should be rejected"));
        assert!(err.contains("id"), "{bad:?}: unhelpful error {err:?}");
    }
}

/// A block's label is presentation, so it never reaches `Block` — it rides
/// out in `BlockVisuals`, unresolved, for `content` to derive or override.
#[test]
fn display_names_ride_out_of_band_and_stay_off_block() {
    let mut visuals = BlockVisuals::default();
    let reg = BlockRegistry::from_toml_with_models(
        r#"
        [[block]]
        id = "dirt"
        render = "opaque"
        solid = true
        hardness = 0.5
        textures = "dirt"

        [[block]]
        id = "tnt"
        display_name = "TNT"
        render = "opaque"
        solid = true
        hardness = 0.5
        textures = "stone"
    "#,
        &mut visuals,
    )
    .expect("valid file");

    assert_eq!(visuals.display_names.len(), reg.len(), "one per block");
    let dirt = reg.find("dirt").expect("declared");
    let tnt = reg.find("tnt").expect("declared");
    assert_eq!(visuals.display_names[dirt.0 as usize], None, "derived");
    assert_eq!(
        visuals.display_names[tnt.0 as usize].as_deref(),
        Some("TNT"),
        "authored"
    );
    // The label must not leak onto the hashed struct.
    assert!(
        !format!("{:?}", reg.get(tnt)).contains("TNT"),
        "display name must stay off Block"
    );
}

/// A fluid's auto-registered flow blocks take the source's id, so they stay
/// typeable and stay valid save keys.
#[test]
fn flow_block_ids_are_derived_from_the_source_id() {
    let reg = BlockRegistry::with_builtins();
    for level in 1..=7u8 {
        let id = reg.flowing(0, level);
        assert_eq!(reg.get(id).id, format!("water_flow_{level}"));
    }
}

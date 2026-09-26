//! Cross-file checks for the biome/boss loop.
//!
//! Each file validates its own references strictly, but some point *across*
//! files that load in an order where the target cannot yet be seen: a block's
//! `[block.interact]` names a boss in `entities.toml`, a boss's offering names
//! an item, a summon names another entity, and a block's harvest tier needs
//! some tool that meets it. Here is the first place that sees everything, so
//! these are checked here — as warnings, like `unknown_harvest_tools`, because
//! a dangling reference degrades one feature rather than the whole world.
//!
//! The shipped data must report none; a test pins that.

use crate::domain::entity::EntityRegistry;
use crate::domain::entity::boss::AttackEffect;
use crate::domain::inventory::{ItemRegistry, Tool};
use crate::domain::world::BlockRegistry;
use crate::domain::world::block::Interaction;
use crate::domain::world::structure::{Cell, StructureConfig};

/// Every dangling cross-file reference, phrased for the log.
pub fn dangling_references(
    blocks: &BlockRegistry,
    items: &ItemRegistry,
    entities: &EntityRegistry,
    structures: &StructureConfig,
) -> Vec<String> {
    let mut problems = Vec::new();
    for (_, block) in blocks.iter() {
        if let Some(Interaction::Altar { boss }) = &block.interact {
            match entities.find(boss) {
                Some(kind) if kind.boss.is_some() => {}
                Some(_) => problems.push(format!(
                    "block {:?}: altar summons {boss:?}, which has no [entity.boss]",
                    block.id
                )),
                None => problems.push(format!(
                    "block {:?}: altar summons unknown entity {boss:?}",
                    block.id
                )),
            }
        }
    }
    for kind in entities.iter() {
        let Some(boss) = &kind.boss else {
            continue;
        };
        if items.find(&boss.offering.item).is_none() {
            problems.push(format!(
                "boss {:?}: offering {:?} is no item",
                kind.name, boss.offering.item
            ));
        }
        for attack in &boss.attacks {
            if let AttackEffect::Summon { entity, .. } = &attack.effect
                && entities.find(entity).is_none_or(|k| k.mob.is_none())
            {
                problems.push(format!(
                    "boss {:?}: attack {:?} summons {entity:?}, which is no mob",
                    kind.name, attack.id
                ));
            }
        }
    }
    for structure in structures.all() {
        let reveals_altar = structure.reveals.is_some();
        let has_rune = any_block_with(structure, blocks, |i| *i == Interaction::Shrine);
        if reveals_altar && !has_rune {
            problems.push(format!(
                "structure {:?} reveals something but holds no shrine block to read",
                structure.id
            ));
        }
    }
    problems
}

/// Whether any cell of `structure`'s template is a block whose interaction
/// satisfies `wanted`.
fn any_block_with(
    structure: &crate::domain::world::structure::StructureDef,
    blocks: &BlockRegistry,
    wanted: impl Fn(&Interaction) -> bool,
) -> bool {
    let template = &structure.template;
    let reach = template.reach();
    (-reach..=reach).any(|dx| {
        (-reach..=reach).any(|dz| {
            (-template.below()..=template.above()).any(|dy| {
                matches!(template.cell(0, dx, dy, dz), Cell::Block(id)
                    if blocks.get(id).interact.as_ref().is_some_and(&wanted))
            })
        })
    })
}

/// Blocks whose harvest tier no tool of an accepted kind reaches: they could
/// never be mined, only looked at.
pub fn unreachable_tiers<'a>(
    blocks: &'a BlockRegistry,
    items: &ItemRegistry,
) -> Vec<(&'a str, u8)> {
    let tools: Vec<&Tool> = items
        .iter()
        .filter_map(|(_, item)| item.get::<Tool>())
        .collect();
    blocks
        .iter()
        .filter_map(|(_, block)| {
            let harvest = block.harvest.as_ref()?;
            let reachable = harvest.tier == 0
                || tools
                    .iter()
                    .any(|t| harvest.accepts(&t.kind) && t.tier >= harvest.tier);
            (!reachable).then_some((block.id.as_str(), harvest.tier))
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::content::GameContent;

    #[test]
    fn the_shipped_content_has_no_dangling_references() {
        let content = GameContent::builtin();
        let problems = dangling_references(
            &content.blocks,
            &content.items,
            &content.entities,
            &content.structures,
        );
        assert!(problems.is_empty(), "{problems:#?}");
    }

    /// Every shipped tier is reachable except the Ashlands' cinder ore, whose
    /// tier-5 tool arrives with the Ashlands milestone. Pinned exactly, so any
    /// *other* ore left out of reach fails here.
    #[test]
    fn only_the_ashlands_tier_waits_for_its_tool() {
        let content = GameContent::builtin();
        assert_eq!(
            unreachable_tiers(&content.blocks, &content.items),
            [("cinder_ore", 5)]
        );
    }

    #[test]
    fn an_altar_for_a_missing_boss_is_reported() {
        let text = crate::domain::world::block::BUILTIN_BLOCKS
            .replace("boss = \"elder stag\"", "boss = \"x\"");
        let blocks = BlockRegistry::from_toml(&text).unwrap();
        let content = GameContent::builtin();
        let problems = dangling_references(
            &blocks,
            &content.items,
            &content.entities,
            &content.structures,
        );
        assert!(
            problems.iter().any(|p| p.contains("unknown entity \"x\"")),
            "{problems:?}"
        );
    }

    /// Silver needs tier 4; take away the iron pickaxe's tier and nothing can
    /// mine it.
    #[test]
    fn a_tier_no_tool_meets_is_reported() {
        let text =
            crate::domain::inventory::item::BUILTIN_ITEMS.replace("tier = 4\n", "tier = 3\n");
        let blocks = BlockRegistry::with_builtins();
        let items = ItemRegistry::from_toml(&text, &blocks).unwrap();
        let unreachable = unreachable_tiers(&blocks, &items);
        assert!(unreachable.contains(&("silver_ore", 4)), "{unreachable:?}");
    }
}

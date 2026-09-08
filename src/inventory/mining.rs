//! Mining-time model: how long it takes to break a block, given its hardness,
//! the tool it asks for, and the tool (if any) the player is holding.

use crate::world::block::Harvest;

use super::component::Tool;

/// Speed bonus when the held tool is one the block asks for.
const CORRECT_TOOL_FACTOR: f32 = 1.5;
/// Penalty when mining with a bare hand or the wrong tool.
const WRONG_TOOL_FACTOR: f32 = 5.0;
/// Floor on break time so even trivial blocks aren't literally instant in survival.
const MIN_BREAK_SECONDS: f32 = 0.05;

/// Seconds to break a block of the given `hardness` with an optional held tool.
///
/// The match is the **block's** call: `harvest` is what the block declared in
/// `[block.harvest]`, and a tool counts when its `kind` is in that list. A block
/// that asks for nothing (glass, bedrock) is mined at the slow rate by
/// everything, which is what it did before any tool was good at it. Returns
/// `INFINITY` for unbreakable blocks.
pub fn break_seconds(hardness: f32, harvest: Option<&Harvest>, tool: Option<&Tool>) -> f32 {
    if !hardness.is_finite() {
        return f32::INFINITY;
    }
    let matched = match (harvest, tool) {
        (Some(harvest), Some(tool)) => harvest.accepts(&tool.kind),
        _ => false,
    };
    let seconds = match tool {
        Some(tool) if matched => hardness * CORRECT_TOOL_FACTOR / tool.dig_speed.max(0.1),
        _ => hardness * WRONG_TOOL_FACTOR,
    };
    seconds.max(MIN_BREAK_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(kind: &str, dig_speed: f32) -> Tool {
        Tool {
            kind: kind.into(),
            dig_speed,
            durability: 60,
            damage: None,
        }
    }

    fn wants(tools: &[&str]) -> Harvest {
        Harvest {
            tools: tools.iter().map(|t| (*t).to_string()).collect(),
            required: false,
        }
    }

    #[test]
    fn the_tool_a_block_asks_for_is_faster_than_a_hand() {
        let stone = wants(&["pickaxe"]);
        let pick = tool("pickaxe", 2.0);
        let with_pick = break_seconds(1.5, Some(&stone), Some(&pick));
        let by_hand = break_seconds(1.5, Some(&stone), None);
        assert!(with_pick < by_hand);
    }

    #[test]
    fn a_tool_the_block_did_not_ask_for_is_no_faster_than_a_hand() {
        let stone = wants(&["pickaxe"]);
        let shovel = tool("shovel", 2.0);
        let with_shovel = break_seconds(1.5, Some(&stone), Some(&shovel));
        let by_hand = break_seconds(1.5, Some(&stone), None);
        assert_eq!(with_shovel, by_hand);
    }

    /// A block may name more than one shape, which is how a flower is cut just
    /// as well by shears or a sword without either of them listing flowers.
    #[test]
    fn a_block_can_ask_for_any_of_several_tools() {
        let plant = wants(&["shears", "sword"]);
        let by_hand = break_seconds(0.2, Some(&plant), None);
        for kind in ["shears", "sword"] {
            let cutter = tool(kind, 5.0);
            assert!(
                break_seconds(0.2, Some(&plant), Some(&cutter)) < by_hand,
                "{kind} should cut it fast"
            );
        }
        let pick = tool("pickaxe", 5.0);
        assert_eq!(
            break_seconds(0.2, Some(&plant), Some(&pick)),
            by_hand,
            "a pickaxe is not on the list"
        );
    }

    /// A block that names no tool is the case the old `material = "other"` and
    /// `"glass"` covered: nothing is good at it, so everything is slow.
    #[test]
    fn a_block_that_asks_for_nothing_is_slow_for_everyone() {
        let pick = tool("pickaxe", 6.0);
        assert_eq!(
            break_seconds(0.3, None, Some(&pick)),
            break_seconds(0.3, None, None)
        );
    }

    /// The whole point of a tier: the same block, strictly less time per tier.
    #[test]
    fn a_higher_tier_pickaxe_digs_stone_faster() {
        let stone = wants(&["pickaxe"]);
        let times: Vec<f32> = [2.0, 4.0, 6.0]
            .iter()
            .map(|&dig_speed| {
                let pick = tool("pickaxe", dig_speed);
                break_seconds(1.5, Some(&stone), Some(&pick))
            })
            .collect();
        assert!(times[0] > times[1], "stone beats wooden");
        assert!(times[1] > times[2], "iron beats stone");
        assert!(
            times[0] < break_seconds(1.5, Some(&stone), None),
            "even the worst pickaxe beats a bare hand"
        );
    }

    #[test]
    fn unbreakable_blocks_take_forever() {
        assert!(break_seconds(f32::INFINITY, None, None).is_infinite());
    }
}

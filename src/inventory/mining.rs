//! Mining-time model: how long it takes to break a block, given its hardness and
//! material and the tool (if any) the player is holding.

use crate::world::block::BlockMaterial;

use super::item::ToolSpec;

/// Speed bonus when the held tool matches the block material.
const CORRECT_TOOL_FACTOR: f32 = 1.5;
/// Penalty when mining with a bare hand or the wrong tool.
const WRONG_TOOL_FACTOR: f32 = 5.0;
/// Floor on break time so even trivial blocks aren't literally instant in survival.
const MIN_BREAK_SECONDS: f32 = 0.05;

/// Seconds to break a block of the given `hardness`/`material` with an optional
/// held tool. A tool matches when the block's material is in its `harvests`
/// list (declared in `assets/items.toml`). Returns `INFINITY` for unbreakable
/// blocks.
pub fn break_seconds(hardness: f32, material: BlockMaterial, tool: Option<&ToolSpec>) -> f32 {
    if !hardness.is_finite() {
        return f32::INFINITY;
    }
    let seconds = match tool {
        Some(tool) if tool.harvests.contains(&material) => {
            hardness * CORRECT_TOOL_FACTOR / tool.dig_speed.max(0.1)
        }
        _ => hardness * WRONG_TOOL_FACTOR,
    };
    seconds.max(MIN_BREAK_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tool(kind: &str, dig_speed: f32, harvests: &[BlockMaterial]) -> ToolSpec {
        ToolSpec {
            kind: kind.into(),
            dig_speed,
            durability: 60,
            harvests: harvests.to_vec(),
        }
    }

    #[test]
    fn correct_tool_is_faster_than_hand() {
        let pick = tool("pickaxe", 2.0, &[BlockMaterial::Stone]);
        let with_pick = break_seconds(1.5, BlockMaterial::Stone, Some(&pick));
        let by_hand = break_seconds(1.5, BlockMaterial::Stone, None);
        assert!(with_pick < by_hand);
    }

    #[test]
    fn wrong_tool_is_no_faster_than_hand() {
        let shovel = tool("shovel", 2.0, &[BlockMaterial::Dirt, BlockMaterial::Sand]);
        let with_shovel = break_seconds(1.5, BlockMaterial::Stone, Some(&shovel));
        let by_hand = break_seconds(1.5, BlockMaterial::Stone, None);
        assert_eq!(with_shovel, by_hand);
    }

    #[test]
    fn shears_cut_plants_faster_than_hand() {
        let shears = tool("shears", 5.0, &[BlockMaterial::Plant]);
        let with_shears = break_seconds(0.2, BlockMaterial::Plant, Some(&shears));
        let by_hand = break_seconds(0.2, BlockMaterial::Plant, None);
        assert!(with_shears < by_hand);
    }

    #[test]
    fn unbreakable_blocks_take_forever() {
        assert!(break_seconds(f32::INFINITY, BlockMaterial::Other, None).is_infinite());
    }
}

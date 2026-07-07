//! Mining-time model: how long it takes to break a block, given its hardness and
//! material and the tool (if any) the player is holding.

use crate::world::block::BlockMaterial;

use super::item::ToolKind;

/// Speed bonus when the held tool matches the block material.
const CORRECT_TOOL_FACTOR: f32 = 1.5;
/// Penalty when mining with a bare hand or the wrong tool.
const WRONG_TOOL_FACTOR: f32 = 5.0;
/// Floor on break time so even trivial blocks aren't literally instant in survival.
const MIN_BREAK_SECONDS: f32 = 0.05;

/// Whether `kind` is the preferred tool for `material`.
pub fn tool_matches(kind: ToolKind, material: BlockMaterial) -> bool {
    matches!(
        (kind, material),
        (ToolKind::Pickaxe, BlockMaterial::Stone)
            | (ToolKind::Axe, BlockMaterial::Wood)
            | (ToolKind::Shovel, BlockMaterial::Dirt)
            | (ToolKind::Shovel, BlockMaterial::Sand)
            | (ToolKind::Shears, BlockMaterial::Plant)
    )
}

/// Seconds to break a block of the given `hardness`/`material` with an optional
/// `(tool, dig_speed)`. Returns `INFINITY` for unbreakable blocks.
pub fn break_seconds(hardness: f32, material: BlockMaterial, tool: Option<(ToolKind, f32)>) -> f32 {
    if !hardness.is_finite() {
        return f32::INFINITY;
    }
    let seconds = match tool {
        Some((kind, dig_speed)) if tool_matches(kind, material) => {
            hardness * CORRECT_TOOL_FACTOR / dig_speed.max(0.1)
        }
        _ => hardness * WRONG_TOOL_FACTOR,
    };
    seconds.max(MIN_BREAK_SECONDS)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn correct_tool_is_faster_than_hand() {
        let with_pick = break_seconds(1.5, BlockMaterial::Stone, Some((ToolKind::Pickaxe, 2.0)));
        let by_hand = break_seconds(1.5, BlockMaterial::Stone, None);
        assert!(with_pick < by_hand);
    }

    #[test]
    fn wrong_tool_is_no_faster_than_hand() {
        let with_shovel = break_seconds(1.5, BlockMaterial::Stone, Some((ToolKind::Shovel, 2.0)));
        let by_hand = break_seconds(1.5, BlockMaterial::Stone, None);
        assert_eq!(with_shovel, by_hand);
    }

    #[test]
    fn shears_cut_plants_faster_than_hand() {
        let with_shears = break_seconds(0.2, BlockMaterial::Plant, Some((ToolKind::Shears, 5.0)));
        let by_hand = break_seconds(0.2, BlockMaterial::Plant, None);
        assert!(with_shears < by_hand);
    }

    #[test]
    fn unbreakable_blocks_take_forever() {
        assert!(break_seconds(f32::INFINITY, BlockMaterial::Other, None).is_infinite());
    }
}

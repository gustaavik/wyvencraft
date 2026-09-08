//! `[item.tool]` — the item digs, and maybe fights.

use super::{ComponentCtx, ComponentParser, ItemComponent};
use crate::core::ident::is_valid_id;

/// Tool capability: what shape of tool the item is, how fast it mines, how long
/// it lasts, and what it hits for.
///
/// A tool declares only what it *is*. Which blocks it is good at is the
/// **block's** business (`[block.harvest]` in `assets/blocks.toml`), so adding a
/// block never means editing a tool, and a tool never has to enumerate the
/// world it might be swung at.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// What shape of tool this is — `pickaxe`, `axe`, `shovel`, `sword`,
    /// `shears`. Blocks name these in `[block.harvest] tool`; the set is pure
    /// data, so a new shape is two content edits and no Rust.
    pub kind: String,
    /// Mining-speed multiplier on a block that wants this kind (hand = 1.0).
    pub dig_speed: f32,
    /// Uses before the tool wears out.
    pub durability: u16,
    /// Melee damage per swing. `None` means this tool is no better than a bare
    /// fist, so the fist's damage stays defined in exactly one place
    /// (`state::ingame_state::mobs::PLAYER_ATTACK_DAMAGE`) instead of being
    /// duplicated as a default here.
    #[serde(default)]
    pub damage: Option<f32>,
}

impl ItemComponent for Tool {
    fn key(&self) -> &'static str {
        "tool"
    }

    fn clone_box(&self) -> Box<dyn ItemComponent> {
        Box::new(self.clone())
    }

    /// A tool carries wear, and wear is per-item, so it cannot stack.
    fn stack_limit(&self) -> Option<u8> {
        Some(1)
    }

    fn durability(&self) -> Option<u16> {
        Some(self.durability)
    }
}

pub struct ToolParser;

impl ComponentParser for ToolParser {
    fn key(&self) -> &'static str {
        "tool"
    }

    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let tool: Tool = value
            .try_into()
            .map_err(|err| format!("item {:?}: [item.tool]: {err}", ctx.item))?;
        // A kind is a cross-file reference, spelled identically in
        // `[block.harvest]`, so it takes the same shape as every other content
        // id: one lowercase token, no room for a stray space to make two kinds
        // that look alike.
        if !is_valid_id(&tool.kind) {
            return Err(format!(
                "item {:?}: tool kind {:?} must be lowercase letters, digits and underscores",
                ctx.item, tool.kind
            ));
        }
        Ok(Some(Box::new(tool)))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::block::BlockRegistry;

    fn parse(body: &str) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let blocks = BlockRegistry::with_builtins();
        let ctx = ComponentCtx {
            blocks: &blocks,
            item: "probe",
        };
        let value: toml::Value = toml::from_str(body).map_err(|err| err.to_string())?;
        ToolParser.parse(value, &ctx)
    }

    #[test]
    fn a_tool_parses_its_kind_and_numbers() {
        let component = parse("kind = \"pickaxe\"\ndig_speed = 2.0\ndurability = 60\n")
            .expect("valid")
            .expect("resolves");
        let tool = super::super::downcast::<Tool>(component.as_ref()).expect("a Tool");
        assert_eq!(tool.kind, "pickaxe");
        assert_eq!(tool.dig_speed, 2.0);
        assert_eq!(tool.durability, 60);
        assert_eq!(tool.damage, None, "a digging shape hits like a fist");
    }

    /// Wear is per-item, so a tool must never stack — and the rule lives on the
    /// component rather than in the loader, which is what lets a new wearing
    /// capability get it without editing the loader.
    #[test]
    fn a_tool_caps_its_stack_and_reports_its_durability() {
        let component = parse("kind = \"axe\"\ndig_speed = 1.0\ndurability = 7\n")
            .expect("valid")
            .expect("resolves");
        assert_eq!(component.stack_limit(), Some(1));
        assert_eq!(component.durability(), Some(7));
    }

    /// A kind is matched against `[block.harvest]` by exact string, so a
    /// spelling a block could never write is a fault, not a quirk.
    #[test]
    fn a_malformed_kind_is_rejected_and_names_the_item() {
        let err = parse("kind = \"Pick Axe\"\ndig_speed = 1.0\ndurability = 1\n")
            .expect_err("a kind is one lowercase token");
        assert!(err.contains("probe"), "unhelpful error {err:?}");
    }

    /// The tool no longer says which blocks it is for — that moved to
    /// `[block.harvest]`, and leaving the old field in place would look like it
    /// still worked.
    #[test]
    fn the_old_harvests_field_is_rejected() {
        assert!(
            parse("kind = \"axe\"\ndig_speed = 1.0\ndurability = 1\nharvests = [\"wood\"]\n")
                .is_err()
        );
    }

    #[test]
    fn a_missing_required_field_is_rejected() {
        assert!(parse("dig_speed = 1.0\n").is_err());
    }
}

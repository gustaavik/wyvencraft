//! `[item.tool]` — the item digs, and maybe fights.

use super::{ComponentCtx, ComponentParser, ItemComponent};
use crate::world::block::BlockMaterial;

/// Tool capability: how fast the item mines, what it mines well, how long it
/// lasts, and what it hits for.
///
/// There is deliberately no tool *kind* here. A kind used to gate block drops
/// by string comparison, which is dispatch on identity; the gate is now a
/// capability of its own (`drops = { requires = "shearable" }`), so a tool is
/// described purely by what it does.
#[derive(Debug, Clone, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tool {
    /// Mining-speed multiplier when the tool matches the block (hand = 1.0).
    pub dig_speed: f32,
    /// Uses before the tool wears out.
    pub durability: u16,
    /// Block materials this tool mines at full speed.
    pub harvests: Vec<BlockMaterial>,
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
    fn a_tool_parses_its_numbers_and_materials() {
        let component = parse("dig_speed = 2.0\ndurability = 60\nharvests = [\"stone\"]\n")
            .expect("valid")
            .expect("resolves");
        let tool = super::super::downcast::<Tool>(component.as_ref()).expect("a Tool");
        assert_eq!(tool.dig_speed, 2.0);
        assert_eq!(tool.durability, 60);
        assert_eq!(tool.harvests, [BlockMaterial::Stone]);
        assert_eq!(tool.damage, None, "a digging shape hits like a fist");
    }

    /// Wear is per-item, so a tool must never stack — and the rule lives on the
    /// component rather than in the loader, which is what lets a new wearing
    /// capability get it without editing the loader.
    #[test]
    fn a_tool_caps_its_stack_and_reports_its_durability() {
        let component = parse("dig_speed = 1.0\ndurability = 7\nharvests = []\n")
            .expect("valid")
            .expect("resolves");
        assert_eq!(component.stack_limit(), Some(1));
        assert_eq!(component.durability(), Some(7));
    }

    /// A misspelled field is a structural fault: accepting it would silently
    /// give the player a tool with default numbers.
    #[test]
    fn an_unknown_field_is_rejected_and_names_the_item() {
        let err = parse("dig_speed = 1.0\ndurability = 1\nharvests = []\nkind = \"pickaxe\"\n")
            .expect_err("kind was removed");
        assert!(err.contains("probe"), "unhelpful error {err:?}");
    }

    #[test]
    fn a_missing_required_field_is_rejected() {
        assert!(parse("dig_speed = 1.0\n").is_err());
    }
}

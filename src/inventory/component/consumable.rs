//! `[item.consumable]` — the item can be eaten.

use super::{ComponentCtx, ComponentParser, ItemComponent};

/// Consumable capability: what one use restores.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Consumable {
    pub hunger: f32,
    pub saturation: f32,
}

impl ItemComponent for Consumable {
    fn key(&self) -> &'static str {
        "consumable"
    }

    fn clone_box(&self) -> Box<dyn ItemComponent> {
        Box::new(*self)
    }
}

pub struct ConsumableParser;

impl ComponentParser for ConsumableParser {
    fn key(&self) -> &'static str {
        "consumable"
    }

    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let food: Consumable = value
            .try_into()
            .map_err(|err| format!("item {:?}: [item.consumable]: {err}", ctx.item))?;
        Ok(Some(Box::new(food)))
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
        ConsumableParser.parse(value, &ctx)
    }

    #[test]
    fn a_consumable_parses_what_it_restores() {
        let component = parse("hunger = 5.0\nsaturation = 6.0\n")
            .expect("valid")
            .expect("resolves");
        let food = super::super::downcast::<Consumable>(component.as_ref()).expect("a Consumable");
        assert_eq!(food.hunger, 5.0);
        assert_eq!(food.saturation, 6.0);
    }

    /// Food stacks and never wears, so it takes both defaults — the reason
    /// bread arrives 64 to a slot and a pickaxe does not.
    #[test]
    fn a_consumable_neither_caps_a_stack_nor_wears() {
        let component = parse("hunger = 1.0\nsaturation = 1.0\n")
            .expect("valid")
            .expect("resolves");
        assert_eq!(component.stack_limit(), None);
        assert_eq!(component.durability(), None);
    }

    #[test]
    fn a_missing_field_is_rejected() {
        assert!(parse("hunger = 1.0\n").is_err());
    }
}

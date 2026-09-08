//! `[item.shearable]` — the item shears.
//!
//! A marker: it declares nothing, because what it gates is asked elsewhere. A
//! block whose `drops` say `{ requires = "shearable" }` yields its item only
//! when the held item carries this, which is what replaced matching a free-form
//! tool kind string against a free-form block string.

use super::{ComponentCtx, ComponentParser, ItemComponent};

/// Shearing capability. Presence is the whole payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shearable {}

impl ItemComponent for Shearable {
    fn key(&self) -> &'static str {
        "shearable"
    }

    fn clone_box(&self) -> Box<dyn ItemComponent> {
        Box::new(*self)
    }
}

pub struct ShearableParser;

impl ComponentParser for ShearableParser {
    fn key(&self) -> &'static str {
        "shearable"
    }

    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let marker: Shearable = value
            .try_into()
            .map_err(|err| format!("item {:?}: [item.shearable]: {err}", ctx.item))?;
        Ok(Some(Box::new(marker)))
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
        ShearableParser.parse(value, &ctx)
    }

    /// The table is written empty (`[item.shearable]` and nothing under it), so
    /// an empty table has to be the valid form.
    #[test]
    fn an_empty_table_declares_the_capability() {
        let component = parse("").expect("valid").expect("resolves");
        assert_eq!(component.key(), "shearable");
        assert!(super::super::downcast::<Shearable>(component.as_ref()).is_some());
    }

    /// A marker takes no fields; accepting one would let a typo look like it
    /// configured something.
    #[test]
    fn a_field_is_rejected() {
        assert!(parse("targets = [\"sheep\"]\n").is_err());
    }
}

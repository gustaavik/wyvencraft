//! `[item.placeable]` — using the item puts a block in the world.

use super::{ComponentCtx, ComponentParser, ItemComponent};
use crate::core::BlockId;

/// Placeable capability: which block this item puts down.
///
/// The block is resolved to a [`BlockId`] at load, not looked up by name on
/// every right-click — placement is an input-latency path, and a name that no
/// longer resolves should be a warning at startup rather than a click that
/// quietly does nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Placeable {
    pub block: BlockId,
}

impl ItemComponent for Placeable {
    fn key(&self) -> &'static str {
        "placeable"
    }

    fn clone_box(&self) -> Box<dyn ItemComponent> {
        Box::new(*self)
    }
}

/// The authored form: a block *name*, since a numeric id is an insertion-order
/// index no file may depend on.
#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct PlaceableDef {
    block: String,
}

pub struct PlaceableParser;

impl ComponentParser for PlaceableParser {
    fn key(&self) -> &'static str {
        "placeable"
    }

    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let def: PlaceableDef = value
            .try_into()
            .map_err(|err| format!("item {:?}: [item.placeable]: {err}", ctx.item))?;
        // A reference that does not resolve degrades the entry rather than the
        // file: the item stays holdable, it just cannot be placed.
        let Some(block) = ctx.blocks.find(&def.block) else {
            log::warn!(
                "item {:?}: unknown placeable block {:?}",
                ctx.item,
                def.block
            );
            return Ok(None);
        };
        Ok(Some(Box::new(Placeable { block })))
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
        PlaceableParser.parse(value, &ctx)
    }

    #[test]
    fn a_block_name_resolves_to_its_id() {
        let blocks = BlockRegistry::with_builtins();
        let component = parse("block = \"stone\"\n")
            .expect("valid")
            .expect("resolves");
        let placeable =
            super::super::downcast::<Placeable>(component.as_ref()).expect("a Placeable");
        assert_eq!(Some(placeable.block), blocks.find("stone"));
    }

    /// Unknown *references* degrade the entry, unknown *structure* fails the
    /// file — the same split the recipe and drop tables use.
    #[test]
    fn an_unknown_block_degrades_the_entry_rather_than_the_file() {
        let degraded = parse("block = \"unobtainium\"\n").expect("the file still parses");
        assert!(degraded.is_none(), "the item keeps no placeable capability");
    }

    #[test]
    fn a_missing_block_name_is_rejected() {
        assert!(parse("").is_err());
    }
}

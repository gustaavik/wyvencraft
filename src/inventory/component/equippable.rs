//! `[item.equippable]` — the item is worn.

use super::{ComponentCtx, ComponentParser, ItemComponent};

/// Which equipment slot a worn piece occupies. The discriminants are the order
/// of the armor slots in the inventory (see `inventory::ARMOR_START`) and of the
/// labelled column in the inventory screen.
///
/// Named for the *slot region*, not the capability: it is what the save format,
/// the wire `Equipment` array and the inventory layout are all indexed by.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ArmorSlot {
    Helmet,
    Chestplate,
    Leggings,
    Boots,
}

impl ArmorSlot {
    pub const ALL: [ArmorSlot; 4] = [
        ArmorSlot::Helmet,
        ArmorSlot::Chestplate,
        ArmorSlot::Leggings,
        ArmorSlot::Boots,
    ];

    /// Offset of this slot within the inventory's armor region.
    #[inline]
    pub fn index(self) -> usize {
        self as usize
    }

    /// Display name, also the label shown beside the slot.
    pub fn label(self) -> &'static str {
        match self {
            ArmorSlot::Helmet => "Helmet",
            ArmorSlot::Chestplate => "Chestplate",
            ArmorSlot::Leggings => "Leggings",
            ArmorSlot::Boots => "Boots",
        }
    }
}

/// Equippable capability: which slot the piece fits, how much damage it
/// absorbs, and how long it lasts.
#[derive(Debug, Clone, Copy, PartialEq, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Equippable {
    pub slot: ArmorSlot,
    /// Defense points; summed across worn pieces (see `Player::damage`).
    pub defense: f32,
    /// Uses before the piece wears out.
    pub durability: u16,
}

impl ItemComponent for Equippable {
    fn key(&self) -> &'static str {
        "equippable"
    }

    fn clone_box(&self) -> Box<dyn ItemComponent> {
        Box::new(*self)
    }

    /// Worn pieces carry wear, and wear is per-item.
    fn stack_limit(&self) -> Option<u8> {
        Some(1)
    }

    fn durability(&self) -> Option<u16> {
        Some(self.durability)
    }
}

pub struct EquippableParser;

impl ComponentParser for EquippableParser {
    fn key(&self) -> &'static str {
        "equippable"
    }

    fn parse(
        &self,
        value: toml::Value,
        ctx: &ComponentCtx,
    ) -> Result<Option<Box<dyn ItemComponent>>, String> {
        let worn: Equippable = value
            .try_into()
            .map_err(|err| format!("item {:?}: [item.equippable]: {err}", ctx.item))?;
        Ok(Some(Box::new(worn)))
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
        EquippableParser.parse(value, &ctx)
    }

    #[test]
    fn a_worn_piece_parses_its_slot_and_numbers() {
        let component = parse("slot = \"chestplate\"\ndefense = 6.0\ndurability = 240\n")
            .expect("valid")
            .expect("resolves");
        let worn = super::super::downcast::<Equippable>(component.as_ref()).expect("an Equippable");
        assert_eq!(worn.slot, ArmorSlot::Chestplate);
        assert_eq!(worn.defense, 6.0);
        assert_eq!(worn.durability, 240);
        assert_eq!(component.stack_limit(), Some(1));
        assert_eq!(component.durability(), Some(240));
    }

    /// A slot that does not exist would leave the piece unequippable with no
    /// sign of why, so it fails the file instead.
    #[test]
    fn an_unknown_slot_is_rejected() {
        assert!(parse("slot = \"cape\"\ndefense = 1.0\ndurability = 1\n").is_err());
    }

    /// The discriminants index the inventory's armor region, so their order is
    /// load-bearing for saves and the wire, not cosmetic.
    #[test]
    fn slot_indices_follow_the_declared_order() {
        for (expected, slot) in ArmorSlot::ALL.iter().enumerate() {
            assert_eq!(slot.index(), expected, "{slot:?}");
        }
    }
}

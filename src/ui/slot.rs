//! One inventory cell, painted the same way everywhere.
//!
//! The HUD hotbar and the inventory grid used to paint slots twice over, with
//! different sizes (48/4 against 46/5), different icon insets, different count
//! colours (yellow against white-on-shadow) and two separate durability bars.
//! That was survivable while they were separate widgets on separate screens.
//!
//! It stopped being survivable when the panel started animating out of the
//! hotbar: the hotbar *is* the grid's bottom row, and on the frame the two swap
//! they are drawn at the same size in the same place. Any difference between
//! them shows up as a one-frame pop. So there is one painter, and both callers
//! go through it.

use egui::{Align2, Color32, FontId, Painter, Rect, TextureId, vec2};

use crate::content::ItemIcon;
use crate::inventory::{ItemRegistry, ItemStack};
use crate::state::UiTextures;
use crate::ui::icon::draw_item_icon;
use crate::ui::ninepatch::{self, SLOT, SLOT_SELECTED};

/// Side of a slot, and the gap between neighbouring slots.
///
/// Shared by the hotbar and the grid — see the module note. Changing either
/// moves both, which is the point.
pub const SIZE: f32 = 48.0;
pub const GAP: f32 = 4.0;
/// Centre-to-centre spacing of adjacent slots.
pub const PITCH: f32 = SIZE + GAP;

/// How far the icon sits inside its cell, leaving the bevel visible.
const ICON_INSET: f32 = 6.0;

/// What a slot is showing.
pub struct SlotContents<'a> {
    pub stack: ItemStack,
    pub icon: ItemIcon,
    pub items: &'a ItemRegistry,
}

/// Paint one slot: its cell art, then whatever is in it.
///
/// `ghost` is a faded atlas tile hinting what an empty slot accepts (the armor
/// column). `tint` multiplies everything, so a panel can fade in as one.
pub fn paint_slot(
    painter: &Painter,
    rect: Rect,
    contents: Option<SlotContents<'_>>,
    ghost: Option<u32>,
    selected: bool,
    tint: Color32,
    tex: UiTextures,
) {
    let patch = if selected { SLOT_SELECTED } else { SLOT };
    ninepatch::draw_nine(painter, rect, patch, tint, tex.gui);

    let inner = rect.shrink(ICON_INSET);
    match contents {
        Some(contents) => {
            draw_item_icon(painter, inner, contents.icon, tex);
            paint_count(painter, rect, contents.stack.count, tint);
            paint_durability(painter, rect, contents.stack, contents.items, tint);
        }
        None => {
            if let Some(tile) = ghost {
                // A silhouette of what fits, faded well back so it reads as a
                // hint rather than as an item sitting in the slot.
                draw_item_icon(painter, inner, ItemIcon::Flat(tile), tex);
                painter.rect_filled(inner, 0.0, scale_alpha(GHOST_SCRIM, tint));
            }
        }
    }
}

/// The scrim that fades an armor slot's ghost hint back.
const GHOST_SCRIM: Color32 = Color32::from_rgba_premultiplied(150, 150, 150, 170);

/// Multiply `color`'s alpha by `tint`'s, so a whole panel can fade as one.
fn scale_alpha(color: Color32, tint: Color32) -> Color32 {
    let a = (color.a() as u32 * tint.a() as u32 / 255) as u8;
    Color32::from_rgba_unmultiplied(color.r(), color.g(), color.b(), a)
}

/// A stack count in the slot's bottom-right corner, hidden for singles.
fn paint_count(painter: &Painter, cell: Rect, count: u8, tint: Color32) {
    if count <= 1 {
        return;
    }
    let pos = cell.right_bottom() - vec2(4.0, 2.0);
    let text = count.to_string();
    let font = FontId::proportional(14.0);
    // A cheap drop shadow keeps the number legible over any icon.
    painter.text(
        pos + vec2(1.0, 1.0),
        Align2::RIGHT_BOTTOM,
        &text,
        font.clone(),
        scale_alpha(Color32::BLACK, tint),
    );
    painter.text(pos, Align2::RIGHT_BOTTOM, text, font, tint);
}

/// A wear bar along the bottom of a slot, for a damaged tool or armor piece.
fn paint_durability(
    painter: &Painter,
    cell: Rect,
    stack: ItemStack,
    items: &ItemRegistry,
    tint: Color32,
) {
    let (Some(durability), Some(max)) = (stack.durability, items.max_durability(stack.item)) else {
        return;
    };
    if max == 0 || durability >= max {
        return;
    }
    let ratio = durability as f32 / max as f32;
    let width = cell.width() - 2.0 * ICON_INSET;
    let track = Rect::from_min_size(
        cell.left_bottom() + vec2(ICON_INSET, -ICON_INSET - 4.0),
        vec2(width, 4.0),
    );
    painter.rect_filled(
        track,
        1.0,
        scale_alpha(Color32::from_black_alpha(190), tint),
    );
    let fill = Rect::from_min_size(track.min, vec2(width * ratio, 4.0));
    painter.rect_filled(fill, 1.0, scale_alpha(durability_color(ratio), tint));
}

/// Green at full, red at empty — the usual wear ramp.
pub fn durability_color(ratio: f32) -> Color32 {
    let r = ((1.0 - ratio) * 255.0) as u8;
    let g = (ratio * 220.0) as u8;
    Color32::from_rgb(r.max(40), g, 40)
}

/// A 9-slice backing drawn behind arbitrary content.
pub fn draw_backing(
    painter: &Painter,
    rect: Rect,
    patch: ninepatch::NinePatch,
    tint: Color32,
    tex: TextureId,
) {
    ninepatch::draw_nine(painter, rect, patch, tint, tex);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The hotbar and the grid's bottom row are the same nine slots drawn in
    /// the same place on the frame the panel takes over from the HUD. They can
    /// only line up if they are laid out from the same numbers.
    #[test]
    fn nine_slots_span_the_same_width_however_they_are_stepped() {
        let by_pitch = 9.0 * PITCH - GAP;
        let by_parts = 9.0 * SIZE + 8.0 * GAP;
        assert_eq!(by_pitch, by_parts);
    }

    #[test]
    fn a_worn_item_ramps_from_green_to_red() {
        let full = durability_color(1.0);
        let empty = durability_color(0.0);
        assert!(full.g() > full.r(), "full durability reads green");
        assert!(empty.r() > empty.g(), "spent durability reads red");
    }
}

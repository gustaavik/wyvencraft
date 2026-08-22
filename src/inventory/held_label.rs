//! How long the name of the item you just selected stays on screen.
//!
//! Pure timing state, kept out of `ui` for the same reason [`ChatLog`] is: the
//! label's *lifetime* is a rule about the selection, testable without a GPU,
//! while painting it is `ui::hud`'s job. It lives beside [`Inventory`] because
//! it tracks the selection, exactly like `Inventory::selected_index` — the
//! difference is only that this half is ephemeral and never saved.
//!
//! [`ChatLog`]: crate::chat::ChatLog
//! [`Inventory`]: super::Inventory

use super::ItemId;

/// Seconds the label stays fully opaque after the held item changes.
const HOLD: f32 = 2.0;
/// Seconds it then takes to fade out.
const FADE: f32 = 1.0;

/// The fading name of the currently held item.
///
/// Feed it [`observe`](HeldLabel::observe) with what the player is holding and
/// [`tick`](HeldLabel::tick) with the frame delta; [`alpha`](HeldLabel::alpha)
/// is then the opacity to paint at, and `0.0` means paint nothing.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HeldLabel {
    /// What was held when the timer last restarted.
    item: Option<ItemId>,
    /// Seconds since it did.
    age: f32,
}

impl HeldLabel {
    /// Note what the player is holding now, restarting the timer if it changed.
    ///
    /// Restarting on *any* change — including to an empty hand and back — is
    /// what makes scrolling the hotbar re-show the name, which is the whole
    /// point of the label.
    pub fn observe(&mut self, item: Option<ItemId>) {
        if self.item != item {
            self.item = item;
            self.age = 0.0;
        }
    }

    /// Age the label by one frame.
    pub fn tick(&mut self, dt: f32) {
        // Saturating rather than unbounded: a long pause (or a debugger) must
        // not accumulate a value that loses precision once it is added to.
        self.age = (self.age + dt.max(0.0)).min(HOLD + FADE);
    }

    /// The item to name, and how opaque to paint it — `None` once it has faded
    /// out, or when nothing is held.
    pub fn visible(&self) -> Option<(ItemId, f32)> {
        let item = self.item?;
        let alpha = self.alpha();
        (alpha > 0.0).then_some((item, alpha))
    }

    /// Opacity in `0.0..=1.0`: fully on for [`HOLD`], then linear to nothing
    /// over [`FADE`].
    pub fn alpha(&self) -> f32 {
        if self.item.is_none() {
            return 0.0;
        }
        if self.age <= HOLD {
            return 1.0;
        }
        (1.0 - (self.age - HOLD) / FADE).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ITEM: ItemId = ItemId(3);
    const OTHER: ItemId = ItemId(4);

    #[test]
    fn nothing_held_shows_nothing() {
        let mut label = HeldLabel::default();
        label.observe(None);
        assert_eq!(label.visible(), None);
    }

    #[test]
    fn a_newly_held_item_is_fully_opaque() {
        let mut label = HeldLabel::default();
        label.observe(Some(ITEM));
        assert_eq!(label.visible(), Some((ITEM, 1.0)));
    }

    #[test]
    fn it_holds_then_fades_then_disappears() {
        let mut label = HeldLabel::default();
        label.observe(Some(ITEM));

        label.tick(HOLD);
        assert_eq!(label.alpha(), 1.0, "still fully on at the end of the hold");

        label.tick(FADE / 2.0);
        let mid = label.alpha();
        assert!((0.0..1.0).contains(&mid), "mid-fade alpha was {mid}");
        assert!((mid - 0.5).abs() < 1e-3, "fade should be linear, got {mid}");

        label.tick(FADE);
        assert_eq!(label.alpha(), 0.0);
        assert_eq!(label.visible(), None, "faded out");
    }

    /// Scrolling the hotbar is the whole reason the label exists, so a change
    /// of item must bring it back at full opacity.
    #[test]
    fn changing_the_held_item_restarts_the_timer() {
        let mut label = HeldLabel::default();
        label.observe(Some(ITEM));
        label.tick(HOLD + FADE);
        assert_eq!(label.visible(), None, "faded out first");

        label.observe(Some(OTHER));
        assert_eq!(label.visible(), Some((OTHER, 1.0)));
    }

    /// Re-observing the *same* item every frame must not keep it on screen.
    #[test]
    fn observing_the_same_item_does_not_restart_the_timer() {
        let mut label = HeldLabel::default();
        for _ in 0..100 {
            label.observe(Some(ITEM));
            label.tick(0.1);
        }
        assert_eq!(
            label.visible(),
            None,
            "should have faded despite re-observing"
        );
    }

    /// Emptying the hand hides the label immediately rather than leaving the
    /// previous item's name hanging over an empty slot.
    #[test]
    fn emptying_the_hand_hides_the_label() {
        let mut label = HeldLabel::default();
        label.observe(Some(ITEM));
        label.observe(None);
        assert_eq!(label.visible(), None);
        assert_eq!(label.alpha(), 0.0);
    }

    /// A stalled frame must not push `age` somewhere arithmetic gets unreliable.
    #[test]
    fn a_huge_frame_delta_is_clamped() {
        let mut label = HeldLabel::default();
        label.observe(Some(ITEM));
        label.tick(1.0e9);
        assert_eq!(label.alpha(), 0.0);
        label.tick(1.0e9);
        assert!(label.alpha().is_finite());
    }
}

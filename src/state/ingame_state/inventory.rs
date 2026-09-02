//! Inventory-screen interactions: open/close, the open/close animation, and
//! click-to-move between slots.

use super::InGameState;
use crate::inventory::ItemStack;

/// Seconds for a full open (or close) sweep.
const OPEN_SECONDS: f32 = 0.25;
/// How close to an endpoint counts as arrived — well under a frame's step, so
/// it only ever swallows accumulated float error.
const SNAP: f32 = 1.0e-4;

/// How far through the inventory transition we are.
///
/// One scalar in `0..=1`: 0 is pure gameplay (camera on the eye, panel folded
/// into the hotbar), 1 is the inventory at rest. The panel and the camera both
/// read [`OpenAnim::progress`], so there is one curve and nothing to keep in
/// step.
///
/// `t` advances **linearly** and the easing is applied on read. That is what
/// makes an interrupted sweep work: pressing E halfway just flips `opening` and
/// `t` carries on from where it is. Storing the eased value instead would apply
/// the curve twice on the way back and kink at the reversal.
///
/// The curve is smoothstep rather than the exponential blend used elsewhere in
/// the codebase ([`crate::entity::AnimationState`]) because both endpoints have
/// to be *reached*, not approached: the panel must land pixel-exact on its
/// resting rect or every slot's hit box is permanently a hair off, and the
/// camera has to actually stop.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub(super) struct OpenAnim {
    /// Linear progress; eased on read, never stored eased.
    t: f32,
    /// Which end `tick` is walking toward. The position is the state; this is
    /// only the direction.
    opening: bool,
}

impl OpenAnim {
    pub fn set_open(&mut self, open: bool) {
        self.opening = open;
    }

    /// Advance one frame at a constant rate, so a half-open panel closes in
    /// half the time — which is what makes a double-tap feel like one gesture.
    pub fn tick(&mut self, dt: f32) {
        let step = dt.max(0.0) / OPEN_SECONDS;
        self.t = if self.opening {
            (self.t + step).min(1.0)
        } else {
            (self.t - step).max(0.0)
        };
        // Adding a sixtieth fifteen times does not land on 1.0, and the residue
        // is signed the wrong way to be caught by the clamps above: a closed
        // panel would read as fractionally open, which keeps `active` true and
        // suppresses the HUD hotbar for a frame it should have come back on.
        if !self.opening && self.t < SNAP {
            self.t = 0.0;
        } else if self.opening && self.t > 1.0 - SNAP {
            self.t = 1.0;
        }
    }

    /// Eased progress, `0..=1`. Smoothstep: zero velocity at both ends, and
    /// symmetric, so a mid-flight reversal changes the velocity's sign without
    /// changing its magnitude.
    pub fn progress(self) -> f32 {
        let t = self.t;
        t * t * (3.0 - 2.0 * t)
    }

    /// Is the inventory doing anything at all this frame? Gates which hotbar is
    /// drawn and whether the player's body is meshed.
    pub fn active(self) -> bool {
        self.opening || self.t > 0.0
    }
}

impl InGameState {
    /// Open/close the inventory screen; returns a held stack to storage on close.
    ///
    /// Whatever no longer fits is thrown rather than dropped on the floor of the
    /// function: closing the panel is not a reason to destroy items, and a full
    /// inventory is exactly when a player is most likely to be holding one.
    pub(super) fn toggle_inventory(&mut self) {
        self.inventory_open = !self.inventory_open;
        self.inventory_anim.set_open(self.inventory_open);
        if !self.inventory_open
            && let Some(held) = self.held.take()
        {
            let leftover = self.inventory.add(held, &self.content.items);
            if leftover > 0 {
                self.throw(ItemStack {
                    count: leftover,
                    ..held
                });
            }
        }
    }

    /// Right-click on a slot: split a stack in half, or place a single item.
    ///
    /// Mirrors the left-click path's armor gate, so a right click can no more
    /// put a pickaxe on your head than a left one can.
    pub(super) fn handle_slot_split(&mut self, index: usize) {
        if let Some(held) = self.held
            && !self
                .inventory
                .can_equip(index, held.item, &self.content.items)
        {
            return;
        }
        match (self.held, self.inventory.slot(index)) {
            // Empty hand: take the larger half, so a single item is picked up
            // whole rather than being unsplittable.
            (None, Some(mut stack)) => {
                let taken = stack.count.div_ceil(2);
                // Split off the remainder and keep the taken part, rather than
                // the other way round: `ItemStack::split` builds a fresh stack
                // and would drop the durability of whatever it returns.
                let rest = stack.split(stack.count - taken);
                self.held = Some(stack);
                self.inventory
                    .set_slot(index, (rest.count > 0).then_some(rest));
            }
            // Holding something: put one of it down.
            (Some(mut held), slot) => {
                match slot {
                    Some(mut stack) if stack.item == held.item => {
                        if stack.count >= self.content.items.max_stack(stack.item) {
                            return;
                        }
                        stack.count += 1;
                        self.inventory.set_slot(index, Some(stack));
                    }
                    // A different item is left alone: right-click places, it
                    // never swaps. Swapping is what left click is for.
                    Some(_) => return,
                    // Copied from the held stack rather than built fresh, so a
                    // single item put down keeps its durability.
                    None => self
                        .inventory
                        .set_slot(index, Some(ItemStack { count: 1, ..held })),
                }
                // One decrement, after whichever branch ran — `split` would
                // have taken it too, and the pair double-counted.
                held.count -= 1;
                self.held = (held.count > 0).then_some(held);
            }
            (None, None) => {}
        }
    }

    /// Throw a slot's whole stack into the world — the drag-it-out gesture.
    pub(super) fn drop_slot(&mut self, index: usize) {
        if let Some(stack) = self.inventory.slot(index) {
            self.inventory.set_slot(index, None);
            self.throw(stack);
        }
    }

    /// Throw a single item out of a slot — the drop key over a slot.
    pub(super) fn drop_one(&mut self, index: usize) {
        if let Some(one) = self.inventory.take_one(index) {
            self.throw(one);
        }
    }

    /// Throw the stack on the cursor, all of it or one item.
    pub(super) fn drop_held(&mut self, all: bool) {
        let Some(mut held) = self.held else {
            return;
        };
        if all {
            self.held = None;
            self.throw(held);
            return;
        }
        let one = held.split(1);
        self.held = (held.count > 0).then_some(held);
        self.throw(one);
    }

    /// Click-to-move logic for an inventory slot (pick up / place / merge / swap).
    pub(super) fn handle_slot_click(&mut self, index: usize) {
        // An armor slot only accepts its own piece. Taking a piece back off is
        // always allowed, so this only gates the held stack going in.
        if let Some(held) = self.held
            && !self
                .inventory
                .can_equip(index, held.item, &self.content.items)
        {
            return;
        }
        match (self.held, self.inventory.slot(index)) {
            (None, Some(stack)) => {
                self.held = Some(stack);
                self.inventory.set_slot(index, None);
            }
            (Some(held), None) => {
                self.inventory.set_slot(index, Some(held));
                self.held = None;
            }
            (Some(mut held), Some(mut stack)) => {
                if held.item == stack.item {
                    let max = self.content.items.max_stack(stack.item);
                    let leftover = stack.merge(held, max);
                    self.inventory.set_slot(index, Some(stack));
                    self.held = if leftover == 0 {
                        None
                    } else {
                        held.count = leftover;
                        Some(held)
                    };
                } else {
                    // Swap held and slot.
                    self.inventory.set_slot(index, Some(held));
                    self.held = Some(stack);
                }
            }
            (None, None) => {}
        }
    }
}

#[cfg(test)]
mod anim_tests {
    use super::*;

    /// Run `frames` steps of `dt` toward `open`.
    fn run(anim: &mut OpenAnim, open: bool, frames: usize, dt: f32) {
        anim.set_open(open);
        for _ in 0..frames {
            anim.tick(dt);
        }
    }

    #[test]
    fn it_takes_the_full_duration_to_open_and_to_close() {
        let dt = 1.0 / 60.0;
        let frames = (OPEN_SECONDS / dt).ceil() as usize;

        let mut anim = OpenAnim::default();
        run(&mut anim, true, frames, dt);
        assert_eq!(anim.progress(), 1.0);
        assert!(anim.active());

        run(&mut anim, false, frames, dt);
        assert_eq!(anim.progress(), 0.0);
        assert!(!anim.active(), "a finished close is idle again");
    }

    /// The reason `t` is stored linear and eased on read: pressing E again
    /// halfway must resume from where the panel *is*, not restart or jump.
    #[test]
    fn reversing_halfway_resumes_from_where_it_was() {
        let dt = 1.0 / 60.0;
        let mut anim = OpenAnim::default();
        run(&mut anim, true, 8, dt);
        let midpoint = anim.progress();
        assert!(
            midpoint > 0.05 && midpoint < 0.95,
            "test needs a genuine midpoint, got {midpoint}"
        );

        anim.set_open(false);
        let mut previous = midpoint;
        for _ in 0..40 {
            anim.tick(dt);
            let now = anim.progress();
            assert!(
                now <= previous + 1e-6,
                "progress went back up: {previous} -> {now}"
            );
            assert!((0.0..=1.0).contains(&now), "progress left its range: {now}");
            previous = now;
        }
        assert_eq!(previous, 0.0, "a reversal still reaches the end");
    }

    /// Symmetric easing is what makes that reversal smooth: the speed on the
    /// way back matches the speed on the way out. A cubic-out would not.
    #[test]
    fn the_curve_is_symmetric_so_a_reversal_does_not_jerk() {
        for step in 0..=10 {
            let t = step as f32 / 10.0;
            let mut a = OpenAnim::default();
            a.set_open(true);
            a.tick(t * OPEN_SECONDS);

            let mut b = OpenAnim::default();
            b.set_open(true);
            b.tick((1.0 - t) * OPEN_SECONDS);

            assert!(
                (a.progress() + b.progress() - 1.0).abs() < 1e-5,
                "progress({t}) + progress(1-{t}) != 1"
            );
        }
    }

    /// A stalled frame must land exactly on the end rather than overshoot, and
    /// a negative delta must not run the panel backwards.
    #[test]
    fn a_huge_frame_delta_lands_exactly_on_the_end() {
        let mut anim = OpenAnim::default();
        anim.set_open(true);
        anim.tick(1_000.0);
        assert_eq!(anim.progress(), 1.0);

        anim.tick(-5.0);
        assert_eq!(anim.progress(), 1.0, "a negative delta is not a rewind");
    }

    /// The frame that E is pressed must already be moving, so the panel is
    /// drawn (and the HUD hotbar is not) with no dead frame between them.
    #[test]
    fn it_is_active_from_the_first_tick() {
        let mut anim = OpenAnim::default();
        assert!(!anim.active());
        anim.set_open(true);
        assert!(anim.active(), "active before the first tick, not after");
        assert_eq!(anim.progress(), 0.0, "but still folded into the hotbar");
    }
}

#[cfg(test)]
mod interaction_tests {
    use super::*;
    use crate::content::GameContent;
    use crate::core::GameMode;
    use crate::inventory::{ArmorSlot, Inventory, ItemStack};

    fn state() -> InGameState {
        InGameState::new(GameContent::builtin(), 7, GameMode::Survival)
    }

    /// A stack, in a storage slot that starts empty.
    fn stocked(state: &mut InGameState, index: usize, id: &str, count: u8) {
        let item = state.content.items.find(id).expect("builtin item");
        state
            .inventory
            .set_slot(index, Some(ItemStack::new(item, count)));
    }

    const SLOT: usize = 9;

    #[test]
    fn right_click_takes_the_larger_half_of_a_stack() {
        let mut state = state();
        stocked(&mut state, SLOT, "stone", 7);
        state.handle_slot_split(SLOT);

        assert_eq!(state.held.expect("a stack on the cursor").count, 4);
        assert_eq!(state.inventory.slot(SLOT).expect("the rest").count, 3);
    }

    /// An even stack halves cleanly, and a single item comes up whole — there
    /// is no half of one, and leaving it behind would make it unpickable.
    #[test]
    fn right_click_halves_evenly_and_takes_a_single_item_whole() {
        let mut even = state();
        stocked(&mut even, SLOT, "stone", 8);
        even.handle_slot_split(SLOT);
        assert_eq!(even.held.expect("held").count, 4);
        assert_eq!(even.inventory.slot(SLOT).expect("rest").count, 4);

        let mut state = state();
        stocked(&mut state, SLOT, "stone", 1);
        state.handle_slot_split(SLOT);
        assert_eq!(state.held.expect("held").count, 1);
        assert!(state.inventory.slot(SLOT).is_none(), "the slot is emptied");
    }

    #[test]
    fn right_click_while_holding_places_one_item_at_a_time() {
        let mut state = state();
        let stone = state.content.items.find("stone").expect("stone");
        state.held = Some(ItemStack::new(stone, 3));

        state.handle_slot_split(SLOT);
        assert_eq!(state.inventory.slot(SLOT).expect("placed").count, 1);
        assert_eq!(state.held.expect("still holding").count, 2);

        state.handle_slot_split(SLOT);
        assert_eq!(state.inventory.slot(SLOT).expect("topped up").count, 2);
        assert_eq!(state.held.expect("still holding").count, 1);

        // The last one leaves the cursor empty rather than a zero-count stack.
        state.handle_slot_split(SLOT);
        assert_eq!(state.inventory.slot(SLOT).expect("topped up").count, 3);
        assert!(state.held.is_none(), "the cursor empties out");
    }

    /// Right click places; it never swaps. Swapping is left click's job, and
    /// doing both would make a misclick on a full slot lose your place.
    #[test]
    fn right_click_onto_a_different_item_does_nothing() {
        let mut state = state();
        stocked(&mut state, SLOT, "dirt", 5);
        let stone = state.content.items.find("stone").expect("stone");
        state.held = Some(ItemStack::new(stone, 3));

        state.handle_slot_split(SLOT);
        assert_eq!(
            state.held.expect("held").count,
            3,
            "the cursor is untouched"
        );
        assert_eq!(state.inventory.slot(SLOT).expect("slot").count, 5);
    }

    /// The armor gate is the left-click path's, and right click must not be a
    /// way around it.
    #[test]
    fn right_click_cannot_put_the_wrong_thing_in_an_armor_slot() {
        let mut state = state();
        let stone = state.content.items.find("stone").expect("stone");
        state.held = Some(ItemStack::new(stone, 4));

        let helmet = Inventory::armor_slot_index(ArmorSlot::Helmet);
        state.handle_slot_split(helmet);
        assert!(state.inventory.slot(helmet).is_none(), "stone is not a hat");
        assert_eq!(state.held.expect("held").count, 4);
    }

    #[test]
    fn the_drop_key_parts_with_one_item_and_leaves_the_rest() {
        let mut state = state();
        stocked(&mut state, SLOT, "stone", 5);

        state.drop_one(SLOT);
        assert_eq!(state.inventory.slot(SLOT).expect("the rest").count, 4);
        assert_eq!(state.drops.len(), 1);
    }

    /// The last item empties the slot rather than leaving a zero-count stack
    /// behind, and a tool dropped this way keeps the wear it had.
    #[test]
    fn dropping_the_last_item_empties_the_slot_and_keeps_its_wear() {
        let mut state = state();
        let pick = state
            .content
            .items
            .find("wooden_pickaxe")
            .expect("builtin pickaxe");
        let worn = ItemStack {
            durability: Some(7),
            ..ItemStack::single(pick)
        };
        state.inventory.set_slot(SLOT, Some(worn));

        state.drop_one(SLOT);
        assert!(state.inventory.slot(SLOT).is_none(), "the slot empties");
        assert_eq!(state.drops.len(), 1);
        assert_eq!(
            state.drops[0].stack.durability,
            Some(7),
            "a dropped tool keeps its wear"
        );
    }

    #[test]
    fn the_drop_key_on_an_empty_slot_does_nothing() {
        let mut state = state();
        state.drop_one(SLOT);
        assert!(state.drops.is_empty());
    }

    #[test]
    fn dragging_a_slot_out_of_the_panel_throws_the_whole_stack() {
        let mut state = state();
        stocked(&mut state, SLOT, "stone", 12);
        assert!(state.drops.is_empty());

        state.drop_slot(SLOT);
        assert!(state.inventory.slot(SLOT).is_none(), "the slot empties");
        assert_eq!(state.drops.len(), 1, "and it lands in the world");
    }

    #[test]
    fn a_held_stack_can_be_thrown_whole_or_one_at_a_time() {
        let mut state = state();
        let stone = state.content.items.find("stone").expect("stone");

        state.held = Some(ItemStack::new(stone, 3));
        state.drop_held(false);
        assert_eq!(state.held.expect("two left").count, 2);
        assert_eq!(state.drops.len(), 1);

        state.drop_held(true);
        assert!(state.held.is_none(), "the cursor empties");
        assert_eq!(state.drops.len(), 2);
    }

    /// Nothing on the cursor means nothing to throw — a click on the world
    /// behind the panel must not conjure an item.
    #[test]
    fn throwing_with_an_empty_cursor_does_nothing() {
        let mut state = state();
        state.drop_held(true);
        state.drop_held(false);
        assert!(state.drops.is_empty());
    }
}

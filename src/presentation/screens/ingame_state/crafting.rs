//! Crafting in play: what the player has discovered, which stations are in
//! reach, and making things.
//!
//! The rules are pure and live in [`crate::domain::inventory::crafting`]; this is the
//! glue that feeds them the world and the inventory. Crafting is **local** on
//! every peer, like every other inventory change: a client's inventory is
//! client-reported (`SyncInventory`) already, so there is no authority path to
//! add. Only discovery crosses the wire, and only so the host can save it.

use std::collections::BTreeSet;

use super::InGameState;
use crate::domain::core::BlockPos;
use crate::domain::core::ident::title_case;
use crate::domain::inventory::crafting::{
    Availability, CraftError, KnownItems, STATION_RADIUS, StationSet, availability, craft,
    stations_near,
};
use crate::infrastructure::net::{Channel, ChatKind, ClientMessage};
use crate::presentation::ui::crafting::{CraftAction, RecipeEntry};

/// How many new recipes one chat line names before it summarises the rest.
const ANNOUNCE_NAMES: usize = 3;

/// The crafting panel's state, and what the player has learned.
#[derive(Default)]
pub(super) struct CraftingState {
    /// Every item this player has held; recipes are revealed from it.
    pub known: KnownItems,
    /// Indices into the recipe book of the revealed recipes, in book order.
    pub revealed: Vec<usize>,
    /// Revealed since the player last looked — drawn with a NEW badge.
    pub unseen: BTreeSet<usize>,
    /// The recipe shown in the detail pane.
    pub selected: Option<usize>,
    /// Terraria's view: hide everything that cannot be made right now.
    pub craftable_only: bool,
    /// The stations within reach, refreshed while the panel is open.
    pub nearby: StationSet,
    /// Every station the blocks declare, in block order, for the panel's chips.
    pub stations: Vec<String>,
    /// Whether discovery has run once. The first pass teaches whatever the
    /// player loaded in with, and announcing all of that would be noise.
    primed: bool,
}

impl CraftingState {
    pub fn new(stations: Vec<String>) -> Self {
        Self {
            stations,
            ..Self::default()
        }
    }
}

impl InGameState {
    /// Once a frame: learn from what the player carries, announce what that
    /// reveals, and look around for stations while the panel is up.
    pub(super) fn tick_crafting(&mut self) {
        let mut learned = self.crafting.known.learn_from(&self.sim.inventory);
        if let Some(held) = self.sim.held {
            learned |= self.crafting.known.learn(held.item);
        }
        if learned || !self.crafting.primed {
            self.refresh_revealed();
            if learned && !self.net.session.is_authority() {
                let items = self.crafting.known.to_wire();
                self.net
                    .session
                    .request(&ClientMessage::SyncKnown { items }, Channel::Reliable);
            }
        }
        if self.inventory_anim.active() {
            self.refresh_stations();
        }
    }

    /// Recompute the revealed recipes and announce the new ones.
    fn refresh_revealed(&mut self) {
        let revealed = self.crafting.known.known_recipes(&self.sim.recipes);
        let fresh: Vec<usize> = revealed
            .iter()
            .copied()
            .filter(|i| !self.crafting.revealed.contains(i))
            .collect();
        self.crafting.revealed = revealed;
        if !std::mem::replace(&mut self.crafting.primed, true) || fresh.is_empty() {
            return;
        }
        self.crafting.unseen.extend(fresh.iter().copied());
        let names: Vec<&str> = fresh
            .iter()
            .filter_map(|&i| self.sim.recipes.get(i))
            .map(|r| self.item_name(r.output))
            .collect();
        self.chat.log.push(ChatKind::System, announcement(&names));
    }

    /// The stations within reach of the player's eye.
    pub(super) fn refresh_stations(&mut self) {
        let center = BlockPos::from_world(self.sim.player.eye_position());
        self.crafting.nearby =
            stations_near(center, STATION_RADIUS, &self.content.rules.blocks, |p| {
                self.sim.world.block_at(p)
            });
    }

    /// Right-click on a station opens the crafting panel. Returns whether the
    /// click was a station's; the caller skips this while sneaking, so a block
    /// can still be placed against a workbench.
    pub(super) fn open_targeted_station(&mut self) -> bool {
        let Some(hit) = self.targeted_block() else {
            return false;
        };
        let block = self
            .content
            .rules
            .blocks
            .get(self.sim.world.block_at(hit.block));
        if block.station.is_none() || self.sim.player.mode.is_creative() {
            return false;
        }
        if !self.inventory_open {
            self.toggle_inventory();
        }
        self.refresh_stations();
        true
    }

    /// Show a recipe in the detail pane, which also marks it seen.
    pub(super) fn select_recipe(&mut self, index: usize) {
        self.crafting.selected = Some(index);
        self.crafting.unseen.remove(&index);
    }

    /// What stands between the player and recipe `index` right now.
    pub(super) fn recipe_availability(&self, index: usize) -> Option<Availability> {
        let recipe = self.sim.recipes.get(index)?;
        Some(availability(
            recipe,
            &self.sim.inventory,
            &self.content.rules.items,
            &self.crafting.nearby,
        ))
    }

    /// Craft recipe `index` once, or as many times as possible when `all`.
    pub(super) fn handle_craft(&mut self, index: usize, all: bool) {
        // Stations are rescanned every frame the panel is up, but a craft is
        // the one moment it has to be exact, so look again.
        self.refresh_stations();
        let times = match (all, self.recipe_availability(index)) {
            (true, Some(Availability::Craftable { max })) => max,
            _ => 1,
        };
        let Some(recipe) = self.sim.recipes.get(index) else {
            return;
        };
        let items = &self.content.rules.items;
        match craft(
            recipe,
            &mut self.sim.inventory,
            items,
            &self.crafting.nearby,
            times,
        ) {
            Ok(_) => self.crafting.unseen.remove(&index),
            Err(err) => {
                let text = match err {
                    CraftError::MissingStation => {
                        let station = recipe.station.as_deref().unwrap_or("station");
                        format!("You need to be near a {}.", title_case(station))
                    }
                    CraftError::MissingIngredients => "You are missing materials.".to_string(),
                    CraftError::NoRoom => "Your inventory is full.".to_string(),
                };
                self.chat.log.push(ChatKind::Error, text);
                false
            }
        };
    }

    /// The discovered recipes the pane lists, in book order, each with what
    /// stands between the player and it right now.
    pub(super) fn crafting_entries(&self) -> Vec<RecipeEntry<'_>> {
        self.crafting
            .revealed
            .iter()
            .filter_map(|&index| {
                let recipe = self.sim.recipes.get(index)?;
                let availability = self.recipe_availability(index)?;
                let entry = RecipeEntry {
                    index,
                    recipe,
                    availability,
                    new: self.crafting.unseen.contains(&index),
                };
                (!self.crafting.craftable_only || availability.is_craftable()).then_some(entry)
            })
            .collect()
    }

    /// Carry out what the player did in the crafting pane.
    pub(super) fn handle_craft_action(&mut self, action: CraftAction) {
        match action {
            CraftAction::Select(index) => self.select_recipe(index),
            CraftAction::Craft { index, all } => {
                self.select_recipe(index);
                self.handle_craft(index, all);
            }
            CraftAction::ToggleCraftableOnly => {
                self.crafting.craftable_only = !self.crafting.craftable_only;
            }
        }
    }

    /// An item's display name, falling back to its id.
    pub(super) fn item_name(&self, item: crate::domain::inventory::ItemId) -> &str {
        self.content
            .visuals
            .item_display_names
            .get(item.0 as usize)
            .map(String::as_str)
            .unwrap_or_else(|| &self.content.rules.items.get(item).id)
    }
}

/// "New recipe: Stone Pickaxe", or a summary once there are too many to list.
fn announcement(names: &[&str]) -> String {
    match names {
        [one] => format!("New recipe: {one}"),
        _ if names.len() <= ANNOUNCE_NAMES => format!("New recipes: {}", names.join(", ")),
        _ => format!(
            "New recipes: {} and {} more",
            names[..ANNOUNCE_NAMES].join(", "),
            names.len() - ANNOUNCE_NAMES
        ),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::domain::core::GameMode;
    use crate::domain::inventory::{Inventory, ItemStack};
    use crate::domain::world::block::blocks;
    use crate::presentation::content::GameContent;

    fn state() -> InGameState {
        let mut state = InGameState::new(GameContent::builtin(), 7, GameMode::Survival);
        state.sim.inventory = Inventory::new();
        state
    }

    fn give(state: &mut InGameState, id: &str, count: u8) {
        let item = state.content.rules.items.find(id).expect("builtin item");
        state
            .sim
            .inventory
            .add(ItemStack::new(item, count), &state.content.rules.items);
    }

    fn index_of(state: &InGameState, output: &str) -> usize {
        let item = state.content.rules.items.find(output).unwrap();
        state
            .sim
            .recipes
            .recipes()
            .iter()
            .position(|r| r.output == item)
            .unwrap_or_else(|| panic!("no recipe for {output}"))
    }

    fn system_lines(state: &InGameState) -> Vec<String> {
        state
            .chat
            .log
            .lines()
            .map(|line| line.text.clone())
            .collect()
    }

    #[test]
    fn the_first_pass_is_silent_and_later_discoveries_are_announced_once() {
        let mut state = state();
        give(&mut state, "oak_log", 4);
        state.tick_crafting();
        assert!(system_lines(&state).is_empty(), "loading in is not news");
        assert!(
            state
                .crafting
                .revealed
                .contains(&index_of(&state, "workbench"))
        );
        assert!(state.crafting.unseen.is_empty());

        give(&mut state, "feather", 1);
        state.tick_crafting();
        state.tick_crafting();
        let lines = system_lines(&state);
        assert_eq!(
            lines,
            vec!["New recipe: Arrow".to_string()],
            "once, not per frame"
        );
        assert!(state.crafting.unseen.contains(&index_of(&state, "arrow")));

        state.select_recipe(index_of(&state, "arrow"));
        assert!(
            state.crafting.unseen.is_empty(),
            "looking at it clears the badge"
        );
    }

    #[test]
    fn a_hand_recipe_crafts_anywhere() {
        let mut state = state();
        give(&mut state, "oak_log", 4);
        state.handle_craft(index_of(&state, "workbench"), false);
        let bench = state.content.rules.items.find("workbench").unwrap();
        assert_eq!(state.sim.inventory.count_of(bench), 1);
        assert_eq!(
            state
                .sim
                .inventory
                .count_of(state.content.rules.items.find("oak_log").unwrap()),
            0
        );
    }

    /// A workbench recipe fails away from a workbench and works beside one —
    /// and the failure says why, rather than doing nothing.
    #[test]
    fn a_station_recipe_needs_the_station_in_the_world() {
        let mut state = state();
        give(&mut state, "cobblestone", 3);
        give(&mut state, "stick", 2);
        let pick = index_of(&state, "stone_pickaxe");
        let pick_item = state.content.rules.items.find("stone_pickaxe").unwrap();

        state.handle_craft(pick, false);
        assert_eq!(state.sim.inventory.count_of(pick_item), 0);
        assert!(
            state
                .chat
                .log
                .lines()
                .any(|l| l.text.contains("near a Workbench")),
            "the refusal names the station"
        );

        let beside = BlockPos::from_world(state.sim.player.eye_position());
        state.sim.world.set_block(
            BlockPos::new(beside.x + 2, beside.y, beside.z),
            blocks::WORKBENCH,
        );
        state.handle_craft(pick, false);
        assert_eq!(state.sim.inventory.count_of(pick_item), 1);
    }

    #[test]
    fn crafting_all_makes_as_many_as_the_materials_allow() {
        let mut state = state();
        give(&mut state, "oak_log", 5);
        state.handle_craft(index_of(&state, "stick"), true);
        let stick = state.content.rules.items.find("stick").unwrap();
        assert_eq!(state.sim.inventory.count_of(stick), 20);
    }

    #[test]
    fn a_long_announcement_is_summarised() {
        assert_eq!(announcement(&["A"]), "New recipe: A");
        assert_eq!(announcement(&["A", "B"]), "New recipes: A, B");
        assert_eq!(
            announcement(&["A", "B", "C", "D", "E"]),
            "New recipes: A, B, C and 2 more"
        );
    }
}

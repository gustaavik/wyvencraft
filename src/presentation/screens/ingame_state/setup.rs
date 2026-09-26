//! Construction of [`InGameState`] for each kind of session: fresh
//! singleplayer/host, a world loaded from disk, or a client joining a host.

use std::sync::Arc;

use glam::Vec3;

use super::crafting::CraftingState;
use super::persistence::Persistence;
use super::view::SceneCache;
use super::{DOUBLE_TAP_WINDOW, InGameState};
use crate::application::boot_plan;
use crate::application::networking::Networking;
use crate::application::peers::Peers;
use crate::application::protocol::mapping::recipes_from_wire;
use crate::application::session::Session;
use crate::application::simulation::{Simulation, SimulationStart};
use crate::domain::chat::{ChatState, OpsList};
use crate::domain::core::{DayCycle, GameMode};
use crate::domain::inventory::HeldLabel;
use crate::domain::inventory::crafting::{KnownItems, station_ids};
use crate::infrastructure::net::session::{ClientSession, HostSession, SingleplayerSession};
use crate::infrastructure::net::{Client, Host, NetVec3, PlayerId, PlayerRestore, RecipeData};
use crate::infrastructure::recipes::load_recipe_book;
use crate::infrastructure::save::restore::restore_into;
use crate::infrastructure::save::{FileWorldRepository, SavedGame};
use crate::presentation::content::GameContent;
use crate::presentation::editor::EditorSession;

impl InGameState {
    /// Singleplayer world.
    pub fn new(content: Arc<GameContent>, seed: u64, mode: GameMode) -> Self {
        Self::build(
            content,
            seed,
            Box::new(SingleplayerSession),
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Host a multiplayer session (the host also plays locally).
    pub fn new_host(content: Arc<GameContent>, seed: u64, host: Host, mode: GameMode) -> Self {
        Self::build(
            content,
            seed,
            Box::new(HostSession::new(host)),
            None,
            DayCycle::default(),
            mode,
            None,
        )
    }

    /// Singleplayer session of a world loaded from (or just created on) disk.
    pub fn new_saved(content: Arc<GameContent>, game: SavedGame) -> Self {
        Self::from_save(content, game, Box::new(SingleplayerSession))
    }

    /// Host a multiplayer session of a world loaded from (or created on) disk.
    pub fn new_host_saved(content: Arc<GameContent>, game: SavedGame, host: Host) -> Self {
        Self::from_save(content, game, Box::new(HostSession::new(host)))
    }

    /// Build from a saved world: regenerate terrain from the saved seed, replay
    /// the edit overlay, and restore the player/inventory/clock. This runs
    /// before the first network pump, so restored edits are already in
    /// `World::edits` before any client can request world state.
    fn from_save(content: Arc<GameContent>, game: SavedGame, session: Box<dyn Session>) -> Self {
        let SavedGame {
            save,
            world,
            player,
            players,
            mobs,
            progression,
            discovery,
        } = game;
        // Anchor spawn-area generation at the saved player position (or the
        // world's recorded spawn) so there's ground under a restored player.
        let anchor = player
            .as_ref()
            .map(|p| Vec3::from_array(p.position))
            .or_else(|| world.as_ref().map(|_| Vec3::from_array(save.meta.spawn)));
        let mut state = Self::build(
            content,
            save.meta.seed,
            session,
            anchor,
            DayCycle::new(save.meta.time_of_day),
            save.meta.game_mode,
            None,
        );
        restore_into(
            &mut state.sim,
            Vec3::from_array(save.meta.spawn),
            world.as_ref(),
            player.as_ref(),
            mobs,
            progression,
        );
        log::info!(
            "loaded world '{}' (seed {}, time {:.3}, player {})",
            save.meta.name,
            save.meta.seed,
            save.meta.time_of_day,
            if player.is_some() {
                "restored"
            } else {
                "fresh"
            },
        );
        state.crafting.known = KnownItems::from_ids(&discovery.owner, &state.content.rules.items);
        state.save.records = players;
        state.save.discovery = discovery;
        state.save.repository = Box::new(FileWorldRepository::new(save));
        state
    }

    /// Join a multiplayer session as a client (world built from the host's seed).
    /// `spawn` is the position the host assigned us in its `Welcome`; `time_of_day`
    /// seeds our day/night clock to the host's so skies match on join; `mode` is the
    /// session's game mode as told by the host; `recipes` are the host's crafting
    /// recipes (authoritative — the client's own recipe file is ignored);
    /// `restored` is our saved state if the host's world remembers us.
    #[allow(clippy::too_many_arguments)]
    pub fn new_client(
        content: Arc<GameContent>,
        seed: u64,
        client: Client,
        local_id: PlayerId,
        spawn: NetVec3,
        time_of_day: f32,
        mode: GameMode,
        recipes: Vec<RecipeData>,
        restored: Option<PlayerRestore>,
    ) -> Self {
        let mut state = Self::build(
            content,
            seed,
            Box::new(ClientSession::new(client, local_id)),
            Some(Vec3::from_array(spawn)),
            DayCycle::new(time_of_day),
            mode,
            Some(recipes),
        );
        if let Some(restored) = &restored {
            state.apply_restore(restored);
        }
        state
    }

    /// Build the in-game state. `spawn_override` (clients) places the player at the
    /// host-provided position and anchors synchronous generation there; otherwise the
    /// spawn is found over the origin column. `recipe_data` (clients) is the host's
    /// recipe book from the `Welcome`; hosts and singleplayer load the local file.
    fn build(
        content: Arc<GameContent>,
        seed: u64,
        session: Box<dyn Session>,
        spawn_override: Option<Vec3>,
        day_cycle: DayCycle,
        mode: GameMode,
        recipe_data: Option<Vec<RecipeData>>,
    ) -> Self {
        // Everything below reads through `content`; four of these used to be
        // deep-cloned into `Arc`s the state held separately, which was a copy of
        // the whole item/block model tables per session for no reason.
        let items = content.rules.items.clone();
        let stations = station_ids(&content.rules.blocks);
        let recipes = match recipe_data {
            Some(data) => {
                let book = recipes_from_wire(&data, &items, &stations);
                log::info!("using {} crafting recipes from host", book.recipes().len());
                book
            }
            None => load_recipe_book(&items, &stations),
        };

        // Read before `session` is moved into the struct below.
        let authoritative = session.is_authority();
        let sim = Simulation::new(
            content.rules.clone(),
            seed,
            SimulationStart {
                spawn: spawn_override,
                day_cycle,
                mode,
                recipes,
                authoritative,
            },
        );

        let mut state = Self {
            sim,
            net: Networking {
                session,
                peers: Peers::default(),
                // A client never authorizes anything, so it never reads the file.
                ops: if authoritative {
                    crate::infrastructure::ops::load_ops()
                } else {
                    OpsList::default()
                },
            },
            held_label: HeldLabel::default(),
            crafting: CraftingState::new(stations),
            show_debug: false,
            editor: EditorSession::disabled(),
            view: SceneCache::new(),
            inventory_open: false,
            inventory_anim: Default::default(),
            // Replaced by the real rect on the first `ui` pass, which always
            // precedes the first `scene_frame`.
            screen: egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1920.0, 1080.0)),
            chat: ChatState::default(),
            jump_tap_timer: DOUBLE_TAP_WINDOW * 2.0,
            save: Persistence::none(),
            content,
        };
        state.apply_dev_boot_options();
        state
    }

    /// The `WYVEN_*` developer switches that act on a session once it exists:
    /// travel and debug spawns (authority only), the starting camera, the
    /// item editor, and an inventory open at boot. See `application::boot_plan`.
    fn apply_dev_boot_options(&mut self) {
        if self.net.session.is_authority() {
            self.debug_goto_from_env();
            self.debug_spawn_from_env();
        }
        // Every session, a joining client included: verifying anything drawn on
        // the player's own body needs *both* ends of a two-process run to be
        // outside their own head. Read here rather than in `boot::start` for
        // exactly that reason — a client's state is built behind
        // `ConnectingState`, which that function never sees.
        if let Some(perspective) = boot_plan::boot_perspective(&boot_plan::SystemEnv) {
            log::info!("WYVEN_PERSPECTIVE: opening in {perspective:?}");
            self.sim.player.perspective = perspective;
        }
        // The editor writes into `assets/`, so it is off unless a developer
        // asked for it. Built here rather than in `boot::start` for the same
        // reason the perspective is: a client's state is put together behind
        // `ConnectingState`, which that function never sees.
        if boot_plan::editor_enabled(&boot_plan::SystemEnv) {
            let targets = crate::presentation::editor::targets_from(&self.content);
            log::info!(
                "WYVEN_EDITOR: item placement editor on ({} items)",
                targets.len()
            );
            self.editor =
                EditorSession::new(true, Box::new(crate::presentation::editor::FileStore));
            self.editor.set_targets(targets);
            if boot_plan::editor_opens_at_boot(&boot_plan::SystemEnv) {
                self.toggle_editor();
            }
        }
        // Ignored if the editor already took the screen: the two fight over
        // the camera, which is why E is refused while the editor is up.
        if boot_plan::inventory_opens_at_boot(&boot_plan::SystemEnv) && !self.editor_open() {
            log::info!("WYVEN_INVENTORY: opening the inventory at boot");
            self.toggle_inventory();
        }
    }
}

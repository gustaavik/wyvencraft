//! Turning a [`BootPlan`] into the screen the app opens on.
//!
//! This is where the plan's decisions become effects: opening saves, binding
//! sockets, signing in. Kept apart from [`plan`], which is pure — it reads the
//! environment and decides, and is tested with no window, GPU or socket.

use std::sync::Arc;

use wyven_app::Screen;
use wyven_auth::AccountState;

use crate::content::GameContent;
use crate::core::GameMode;
use crate::net::Host;
use crate::save::{self, SaveError, SavedGame, WorldSave};
use crate::state::{
    ConnectingState, InGameState, LoadingState, LoginState, MainMenuState, Wyvencraft,
};

use super::{BootPlan, WorldChoice};

/// Seed used by ephemeral (never-saved) host worlds.
const EPHEMERAL_SEED: u64 = 0x57_56_4E_01;

/// Open (or create) the world a [`BootPlan`] asks for. `Ephemeral` has no save
/// directory at all, so it yields `None` and the caller builds a throwaway world.
fn open_boot_world(world: &WorldChoice, mode: GameMode) -> Option<Result<SavedGame, SaveError>> {
    let WorldChoice::Named { name, seed } = world else {
        return None;
    };
    let seed = seed.unwrap_or_else(save::random_seed);
    Some(
        WorldSave::open_or_create(&save::saves_root(), name, seed, mode)
            .and_then(|save| save.load()),
    )
}

/// Set up the account for a dev-boot plan, which skips the login screen.
///
/// `WYVEN_BOOT_INGAME`, `WYVEN_HOST` and `WYVEN_JOIN` exist so the game can be
/// launched headlessly with no clicking, and that has to keep working. So a boot
/// plan never shows the login screen: it signs in with `WYVEN_USERNAME` if the
/// auth server can be reached, and otherwise runs offline.
///
/// This is a *developer* path, not a way around the login gate — an offline
/// client still cannot join anyone, because it has no ticket to present.
fn boot_account(account: &AccountState) {
    let Ok(username) = std::env::var("WYVEN_USERNAME") else {
        log::info!("no WYVEN_USERNAME; booting offline");
        account.set_offline();
        return;
    };
    let Ok(password) = std::env::var("WYVEN_PASSWORD") else {
        log::info!("WYVEN_USERNAME set but no WYVEN_PASSWORD; booting offline");
        account.set_offline();
        return;
    };

    let client = wyven_auth::HttpAuthClient::from_env();
    match wyven_auth::AuthClient::login(&client, &username, &password) {
        Ok(session) => {
            log::info!("booted signed in as {}", session.identity);
            // Cache the ticket keys too, so a `WYVEN_HOST=1` boot can verify the
            // clients that join it.
            if let Ok(keys) = wyven_auth::AuthClient::public_keys(&client)
                && !keys.is_empty()
                && let Err(err) = wyven_auth::KeyCache::new().store(&keys)
            {
                log::warn!("could not cache auth keys: {err}");
            }
            account.sign_in(session);
        }
        Err(err) => {
            log::warn!("boot sign-in failed ({err}); continuing offline");
            account.set_offline();
        }
    }
}

/// Turn a [`BootPlan`] into the screen the app opens on.
pub fn initial_screen(
    plan: BootPlan,
    content: &Arc<GameContent>,
    account: &AccountState,
) -> Box<dyn Screen<Wyvencraft>> {
    // Only the menu path is gated. Every other plan is a dev-boot flag, which
    // must stay usable without a window to click in.
    if !matches!(plan, BootPlan::MainMenu) {
        boot_account(account);
    }

    match plan {
        BootPlan::MainMenu => Box::new(LoginState::new(account.clone())),
        BootPlan::Singleplayer { world, mode } => match open_boot_world(&world, mode) {
            Some(Ok(game)) => Box::new(LoadingState::saved(game)),
            Some(Err(err)) => {
                log::error!("WYVEN_WORLD load failed ({err}); starting ephemeral world");
                Box::new(LoadingState::singleplayer(mode))
            }
            None => Box::new(LoadingState::singleplayer(mode)),
        },
        BootPlan::Host { world, mode, port } => {
            let (seed, game) = match open_boot_world(&world, mode) {
                Some(Ok(game)) => (game.save.meta.seed, Some(game)),
                Some(Err(err)) => {
                    log::error!("WYVEN_WORLD load failed ({err}); hosting ephemeral world");
                    (EPHEMERAL_SEED, None)
                }
                None => (EPHEMERAL_SEED, None),
            };
            match Host::bind(
                port,
                seed,
                crate::net::host_config(),
                crate::net::TicketJoin::from_cache(),
            ) {
                Ok(host) => match game {
                    Some(game) => {
                        Box::new(InGameState::new_host_saved(content.clone(), game, host))
                    }
                    None => Box::new(InGameState::new_host(content.clone(), seed, host, mode)),
                },
                Err(err) => {
                    log::error!("host bind failed: {err}");
                    Box::new(MainMenuState::new())
                }
            }
        }
        BootPlan::Join { address } => Box::new(ConnectingState::new(address, account)),
    }
}

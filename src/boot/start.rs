//! Turning a [`BootPlan`] into the screen the app opens on.
//!
//! This is where the plan's decisions become effects: opening saves, binding
//! sockets, signing in. Kept apart from [`super::plan`], which is pure — it reads the
//! environment and decides, and is tested with no window, GPU or socket.

use std::sync::Arc;

use wyven_app::Screen;
use wyven_auth::{AccountState, AuthClient, AuthSession, KeyCache};

use crate::content::GameContent;
use crate::core::GameMode;
use crate::net::Host;
use crate::save::{self, AccountProfile, SaveError, SavedGame, WorldSave};
use crate::state::{ConnectingState, InGameState, LoadingState, MainMenuState, Wyvencraft};

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

/// Establish who is playing, before the first screen is built.
///
/// The game has no login screen: signing in is the launcher's job, and it hands
/// the result over by writing the `[account]` table of `profile.toml`. So this
/// has three paths, tried in order:
///
/// 1. **A stored session** — the normal case. Refresh it and carry on.
/// 2. **`WYVEN_USERNAME` / `WYVEN_PASSWORD`** — the developer path, so the game
///    can be launched headlessly with no launcher and no clicking.
/// 3. **Neither** — run offline. Singleplayer works; multiplayer does not,
///    because an offline client has no ticket to present.
///
/// It never fails and never blocks the player out. An unreachable auth server
/// means offline play, not a locked door.
fn boot_account(account: &AccountState) {
    let client = wyven_auth::HttpAuthClient::from_env();

    if let Some(stored) = save::stored_account() {
        match client.refresh(&stored.refresh_token) {
            Ok(session) => {
                log::info!("restored session for {}", session.identity);
                adopt_session(account, &client, session);
                return;
            }
            // The server *refused* it: consumed, revoked, or expired. The token
            // is dead, so drop it — keeping it would mean retrying a doomed
            // refresh on every launch.
            Err(err) if !err.is_offline() => {
                log::warn!("stored session rejected ({err}); signing out");
                if let Err(err) = save::store_account(None) {
                    log::warn!("could not clear the stored account: {err}");
                }
                account.set_offline();
                return;
            }
            // Could not reach the server. The token is probably still good, so
            // keep it and try again next launch.
            Err(err) => {
                log::warn!("could not reach the account server ({err}); playing offline");
                account.set_offline();
                return;
            }
        }
    }

    let (Ok(username), Ok(password)) = (
        std::env::var("WYVEN_USERNAME"),
        std::env::var("WYVEN_PASSWORD"),
    ) else {
        log::info!("no stored session and no WYVEN_USERNAME; playing offline");
        account.set_offline();
        return;
    };

    match client.login(&username, &password) {
        Ok(session) => {
            log::info!("signed in as {} from the environment", session.identity);
            adopt_session(account, &client, session);
        }
        Err(err) => {
            log::warn!("boot sign-in failed ({err}); continuing offline");
            account.set_offline();
        }
    }
}

/// Record a session: on disk first, then in memory, then cache the ticket keys.
///
/// The order is the refresh-token discipline, and it is not arbitrary. Rotation
/// is destructive and single-use — the token that was just spent is already dead
/// server-side. If the process died between receiving the new pair and writing
/// it, the player would be signed out everywhere with nothing to show for it.
/// So the write happens before anything else can go wrong.
fn adopt_session(account: &AccountState, client: &impl AuthClient, session: AuthSession) {
    if let Err(err) = save::store_account(Some(AccountProfile {
        account_id: session.identity.account_id.to_string(),
        username: session.identity.username.clone(),
        refresh_token: session.refresh_token.clone(),
    })) {
        log::warn!("could not persist the session: {err}");
    }

    account.sign_in(session);

    // Fetch the ticket keys while the server is known reachable. This client
    // may host later, and a host with no keys turns everyone away — so the
    // moment to cache them is now, not when someone tries to join.
    match client.public_keys() {
        Ok(keys) if !keys.is_empty() => {
            match KeyCache::at(crate::paths::keys_path()).store(&keys) {
                Ok(()) => log::info!("cached {} auth key(s) for hosting", keys.len()),
                Err(err) => log::warn!("could not cache auth keys: {err}"),
            }
        }
        Ok(_) => log::warn!("the auth server published no keys"),
        Err(err) => log::warn!("could not fetch auth keys: {err}"),
    }
}

/// Turn a [`BootPlan`] into the screen the app opens on.
pub fn initial_screen(
    plan: BootPlan,
    content: &Arc<GameContent>,
    account: &AccountState,
) -> Box<dyn Screen<Wyvencraft>> {
    // Every plan, the menu included. There is no login screen to defer this
    // to any more, so if the account is not established here it never is.
    boot_account(account);

    match plan {
        BootPlan::MainMenu => Box::new(MainMenuState::new()),
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

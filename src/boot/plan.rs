//! What the app should do at startup, decided from the environment.
//!
//! The `WYVEN_*` dev variables (see `CLAUDE.md`) skip the menus and drop
//! straight into a world, a hosted session, or a join. Deciding *which* used to
//! be a forty-line `if`/`else if` chain inside `App::new`, tangled up with
//! Vulkan setup and content loading — so the one part with real branching logic
//! could only be exercised by launching a window.
//!
//! [`BootPlan::from_env`] is that decision as a pure function over an
//! [`Environment`]. The app still performs the effects (opening the save,
//! binding the socket); it just no longer decides and acts in the same breath.

use std::collections::HashMap;
use std::net::SocketAddr;

use crate::core::GameMode;
use crate::net::DEFAULT_PORT;

/// Read-only view of the process environment.
pub trait Environment {
    fn get(&self, key: &str) -> Option<String>;

    /// Whether a variable is set at all (some flags are presence-only).
    fn is_set(&self, key: &str) -> bool {
        self.get(key).is_some()
    }
}

/// The real process environment.
pub struct SystemEnv;

impl Environment for SystemEnv {
    fn get(&self, key: &str) -> Option<String> {
        std::env::var(key).ok()
    }

    fn is_set(&self, key: &str) -> bool {
        std::env::var_os(key).is_some()
    }
}

/// A fixed set of variables, for tests.
#[derive(Default)]
pub struct MapEnv(HashMap<String, String>);

impl MapEnv {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn with(mut self, key: &str, value: &str) -> Self {
        self.0.insert(key.to_string(), value.to_string());
        self
    }
}

impl Environment for MapEnv {
    fn get(&self, key: &str) -> Option<String> {
        self.0.get(key).cloned()
    }
}

/// Which world a boot plan should play, when it plays one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum WorldChoice {
    /// Load-or-create this named world under `saves/`; it persists.
    Named {
        name: String,
        /// Seed if this creates a *new* world. `None` means pick at random —
        /// left unresolved here so the plan stays pure.
        seed: Option<u64>,
    },
    /// A throwaway world with a fixed seed. Never saved.
    Ephemeral,
}

/// What to do once the window and renderer exist.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootPlan {
    /// Normal startup: show the main menu.
    MainMenu,
    /// Drop straight into a singleplayer world.
    Singleplayer { world: WorldChoice, mode: GameMode },
    /// Host a session (the host also plays locally).
    Host {
        world: WorldChoice,
        mode: GameMode,
        port: u16,
    },
    /// Join a session at this address.
    Join { address: SocketAddr },
}

impl BootPlan {
    /// Decide what to boot into. Never fails: an unparseable `WYVEN_JOIN`
    /// address falls back to the menu, matching the previous behaviour.
    pub fn from_env(env: &dyn Environment) -> Self {
        let mode = match env.get("WYVEN_MODE").as_deref() {
            Some("creative") | Some("Creative") => GameMode::Creative,
            _ => GameMode::Survival,
        };
        let world = match env.get("WYVEN_WORLD") {
            Some(name) => WorldChoice::Named {
                seed: env.get("WYVEN_SEED").map(|s| crate::save::parse_seed(&s)),
                name,
            },
            None => WorldChoice::Ephemeral,
        };

        // Precedence matches the original chain: in-game, then host, then join.
        if env.is_set("WYVEN_BOOT_INGAME") {
            Self::Singleplayer { world, mode }
        } else if env.is_set("WYVEN_HOST") {
            Self::Host {
                world,
                mode,
                port: DEFAULT_PORT,
            }
        } else if let Some(join) = env.get("WYVEN_JOIN") {
            match join.parse::<SocketAddr>() {
                Ok(address) => Self::Join { address },
                Err(err) => {
                    log::error!("WYVEN_JOIN '{join}' is not a valid address ({err}); showing menu");
                    Self::MainMenu
                }
            }
        } else {
            Self::MainMenu
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_bare_environment_shows_the_menu() {
        assert_eq!(BootPlan::from_env(&MapEnv::new()), BootPlan::MainMenu);
    }

    #[test]
    fn boot_ingame_starts_an_ephemeral_survival_world() {
        let plan = BootPlan::from_env(&MapEnv::new().with("WYVEN_BOOT_INGAME", "1"));
        assert_eq!(
            plan,
            BootPlan::Singleplayer {
                world: WorldChoice::Ephemeral,
                mode: GameMode::Survival,
            }
        );
    }

    /// `WYVEN_WORLD` is what makes a boot world persist; without it the world is
    /// explicitly throwaway. This distinction decides whether the session ever
    /// writes to disk, so it is worth pinning down.
    #[test]
    fn naming_a_world_makes_it_persistent() {
        let plan = BootPlan::from_env(
            &MapEnv::new()
                .with("WYVEN_BOOT_INGAME", "1")
                .with("WYVEN_WORLD", "testworld")
                .with("WYVEN_MODE", "creative"),
        );
        assert_eq!(
            plan,
            BootPlan::Singleplayer {
                world: WorldChoice::Named {
                    name: "testworld".to_string(),
                    seed: None,
                },
                mode: GameMode::Creative,
            }
        );
    }

    /// A seed only matters alongside a named world (it seeds creation), and it
    /// accepts the same forms `parse_seed` does — decimal, hex, or text.
    #[test]
    fn a_seed_is_parsed_for_a_named_world() {
        let plan = BootPlan::from_env(
            &MapEnv::new()
                .with("WYVEN_HOST", "1")
                .with("WYVEN_WORLD", "seeded")
                .with("WYVEN_SEED", "12345"),
        );
        let BootPlan::Host { world, port, .. } = plan else {
            panic!("expected a host plan, got {plan:?}");
        };
        assert_eq!(
            world,
            WorldChoice::Named {
                name: "seeded".to_string(),
                seed: Some(12345),
            }
        );
        assert_eq!(port, DEFAULT_PORT);
    }

    #[test]
    fn join_parses_an_address() {
        let plan = BootPlan::from_env(&MapEnv::new().with("WYVEN_JOIN", "127.0.0.1:6091"));
        assert_eq!(
            plan,
            BootPlan::Join {
                address: "127.0.0.1:6091".parse().unwrap(),
            }
        );
    }

    /// A typo'd address must not take the app down — it falls back to the menu.
    #[test]
    fn an_unparseable_join_address_falls_back_to_the_menu() {
        let plan = BootPlan::from_env(&MapEnv::new().with("WYVEN_JOIN", "not an address"));
        assert_eq!(plan, BootPlan::MainMenu);
    }

    /// In-game wins over host, and host over join, as the original chain did.
    #[test]
    fn boot_modes_have_a_fixed_precedence() {
        let env = MapEnv::new()
            .with("WYVEN_BOOT_INGAME", "1")
            .with("WYVEN_HOST", "1")
            .with("WYVEN_JOIN", "127.0.0.1:6091");
        assert!(matches!(
            BootPlan::from_env(&env),
            BootPlan::Singleplayer { .. }
        ));

        let env = MapEnv::new()
            .with("WYVEN_HOST", "1")
            .with("WYVEN_JOIN", "127.0.0.1:6091");
        assert!(matches!(BootPlan::from_env(&env), BootPlan::Host { .. }));
    }
}

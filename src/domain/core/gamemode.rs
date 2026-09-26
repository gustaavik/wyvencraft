//! The player's game mode and the gameplay rules each mode implies.
//!
//! Lives in `core` (rather than `entity` or `state`) so the wire protocol can
//! serialize it without reaching across the dependency graph: `core ← everything`.

use serde::{Deserialize, Serialize};

/// How the world treats the player: a survival challenge or a creative sandbox.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
pub enum GameMode {
    /// Health, hunger, fall damage, timed mining, finite blocks.
    #[default]
    Survival,
    /// Flight, invulnerability, instant break, infinite blocks.
    Creative,
}

impl GameMode {
    pub fn is_creative(self) -> bool {
        matches!(self, GameMode::Creative)
    }

    /// Whether the player may toggle flight.
    pub fn can_fly(self) -> bool {
        self.is_creative()
    }

    /// Whether the player can take damage (fall, starvation, ...).
    pub fn takes_damage(self) -> bool {
        matches!(self, GameMode::Survival)
    }

    /// Whether placing a block consumes it from the inventory.
    pub fn consumes_blocks(self) -> bool {
        matches!(self, GameMode::Survival)
    }

    /// Whether breaking a block is instant (vs. timed by hardness).
    pub fn instant_break(self) -> bool {
        self.is_creative()
    }

    /// The other mode (used by the F4 live toggle).
    pub fn toggled(self) -> Self {
        match self {
            GameMode::Survival => GameMode::Creative,
            GameMode::Creative => GameMode::Survival,
        }
    }

    /// Human-readable name for menus and the HUD.
    pub fn label(self) -> &'static str {
        match self {
            GameMode::Survival => "Survival",
            GameMode::Creative => "Creative",
        }
    }
}

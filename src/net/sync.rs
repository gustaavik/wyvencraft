//! Client-side state for remote players: snapshot buffering and interpolation so
//! other players move smoothly between authoritative updates.

use glam::Vec3;

use crate::core::GameMode;
use crate::net::protocol::PlayerId;

/// Display placeholders until the first `PlayerStats` sync arrives (the
/// authoritative values live in the host's player entity).
const DEFAULT_HEALTH: f32 = 20.0;
const DEFAULT_HUNGER: f32 = 20.0;

/// A non-local player as seen by this client.
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
    /// Previous and latest authoritative positions, lerped for rendering.
    previous: Vec3,
    current: Vec3,
    pub yaw: f32,
    pub pitch: f32,
    /// Latest synced survival vitals + mode (for overhead UI / future use).
    pub health: f32,
    pub hunger: f32,
    /// Host-side only: reported with `Stats` so player records persist it.
    pub saturation: f32,
    pub mode: GameMode,
}

impl RemotePlayer {
    pub fn new(id: PlayerId, name: String, position: Vec3) -> Self {
        Self {
            id,
            name,
            previous: position,
            current: position,
            yaw: 0.0,
            pitch: 0.0,
            health: DEFAULT_HEALTH,
            hunger: DEFAULT_HUNGER,
            saturation: DEFAULT_HUNGER,
            mode: GameMode::Survival,
        }
    }

    /// Record a new authoritative snapshot.
    pub fn push_snapshot(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        self.previous = self.current;
        self.current = position;
        self.yaw = yaw;
        self.pitch = pitch;
    }

    /// Record synced survival vitals from a `PlayerStats` message.
    pub fn set_stats(&mut self, health: f32, hunger: f32, mode: GameMode) {
        self.health = health;
        self.hunger = hunger;
        self.mode = mode;
    }

    /// Smoothed render position; `alpha` in `[0,1]` between the last two snapshots.
    pub fn interpolated_position(&self, alpha: f32) -> Vec3 {
        self.previous.lerp(self.current, alpha.clamp(0.0, 1.0))
    }

    /// Latest authoritative position.
    pub fn position(&self) -> Vec3 {
        self.current
    }
}

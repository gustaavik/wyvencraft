//! Client-side state for remote players: snapshot buffering and interpolation so
//! other players move smoothly between authoritative updates.

use glam::Vec3;

use crate::net::protocol::PlayerId;

/// A non-local player as seen by this client.
pub struct RemotePlayer {
    pub id: PlayerId,
    pub name: String,
    /// Previous and latest authoritative positions, lerped for rendering.
    previous: Vec3,
    current: Vec3,
    pub yaw: f32,
    pub pitch: f32,
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
        }
    }

    /// Record a new authoritative snapshot.
    pub fn push_snapshot(&mut self, position: Vec3, yaw: f32, pitch: f32) {
        self.previous = self.current;
        self.current = position;
        self.yaw = yaw;
        self.pitch = pitch;
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

//! Day/night cycle: a normalized time-of-day clock that drives sky colors, the
//! sun/moon position, and world lighting. Pure logic with no rendering deps — the
//! `state` layer bridges the derived [`Atmosphere`] into the renderer's frame data.

use std::f32::consts::TAU;

use glam::Vec3;

/// Real seconds for one full day↔night cycle (20 minutes).
pub const DEFAULT_DAY_LENGTH_SECS: f32 = 1200.0;
/// Default starting time-of-day (mid-morning) so a fresh world opens in daylight.
pub const DEFAULT_START: f32 = 0.30;

/// Tracks the time of day as a normalized phase in `[0,1)`:
/// `0.0` = midnight, `0.25` = sunrise, `0.5` = noon, `0.75` = sunset.
pub struct DayCycle {
    time_of_day: f32,
    day_length_secs: f32,
}

impl DayCycle {
    /// Create a cycle starting at `start` (wrapped into `[0,1)`) with the default
    /// 20-minute day length.
    pub fn new(start: f32) -> Self {
        Self {
            time_of_day: start.rem_euclid(1.0),
            day_length_secs: DEFAULT_DAY_LENGTH_SECS,
        }
    }

    /// Advance the clock by `dt` real seconds, wrapping at the end of the day.
    pub fn advance(&mut self, dt: f32) {
        self.time_of_day = (self.time_of_day + dt / self.day_length_secs).rem_euclid(1.0);
    }

    pub fn time_of_day(&self) -> f32 {
        self.time_of_day
    }

    /// Reset the clock to `t` (wrapped into `[0,1)`). Used to sync to the host.
    pub fn set_time_of_day(&mut self, t: f32) {
        self.time_of_day = t.rem_euclid(1.0);
    }

    /// True while the sun is below the horizon (mob spawning's night gate).
    pub fn is_night(&self) -> bool {
        self.sun_direction().y < 0.0
    }

    /// Unit direction **toward** the sun in world space (Y-up). The sun rises in
    /// the east, passes overhead at noon, and sets in the west; a small Z tilt
    /// keeps the arc from being perfectly vertical.
    pub fn sun_direction(&self) -> Vec3 {
        let a = self.time_of_day * TAU;
        Vec3::new(a.sin(), -a.cos(), 0.15).normalize()
    }

    /// Derive the full atmosphere (sky colors + world lighting) for the current
    /// time, blending between day/night/dusk keyframe palettes by sun elevation.
    pub fn atmosphere(&self) -> Atmosphere {
        let sun_dir = self.sun_direction();
        let elev = sun_dir.y; // [-1, 1]

        // Day weight: 1 in full daylight, 0 at/under the horizon, with a soft
        // transition band so dawn/dusk fade in and out.
        let day = smoothstep(-0.05, 0.20, elev);
        // Warm-horizon weight: a triangular peak right at the horizon.
        let horizon_glow = 1.0 - (elev / 0.18).abs().min(1.0);

        // Sky keyframe palettes.
        let day_zenith = Vec3::new(0.30, 0.55, 0.92);
        let day_horizon = Vec3::new(0.66, 0.80, 0.96);
        let night_zenith = Vec3::new(0.02, 0.03, 0.09);
        let night_horizon = Vec3::new(0.05, 0.07, 0.14);
        let dusk_horizon = Vec3::new(0.95, 0.52, 0.27);

        let zenith_color = night_zenith.lerp(day_zenith, day);
        let horizon_color = night_horizon
            .lerp(day_horizon, day)
            .lerp(dusk_horizon, horizon_glow * 0.8);

        // World directional-light keyframes.
        let day_light = Vec3::new(1.00, 0.97, 0.88);
        let night_light = Vec3::new(0.48, 0.54, 0.70);
        let dusk_light = Vec3::new(1.00, 0.70, 0.45);
        let light_color = night_light
            .lerp(day_light, day)
            .lerp(dusk_light, horizon_glow * 0.5);

        let ambient = lerp(0.35, 0.82, day);

        // After dusk the key light comes from the moon (opposite the sun) so faces
        // still get some shape; it's kept dim via `light_color` (night palette).
        let light_dir = if elev > 0.0 { sun_dir } else { -sun_dir };

        Atmosphere {
            sun_dir,
            light_dir,
            light_color,
            ambient,
            zenith_color,
            horizon_color,
            sun_color: Vec3::new(1.0, 0.95, 0.80),
            star_intensity: smoothstep(0.10, -0.10, elev), // 0 day → 1 night
            moon_intensity: 1.0 - day,
        }
    }
}

impl Default for DayCycle {
    fn default() -> Self {
        Self::new(DEFAULT_START)
    }
}

/// Sky + lighting values for a moment in the day. Plain data handed to the render
/// layer — keeps `render` free of any dependency on this module.
#[derive(Clone, Copy, Debug)]
pub struct Atmosphere {
    /// Unit direction toward the sun (world space, Y-up).
    pub sun_dir: Vec3,
    /// Direction of the dominant light (sun by day, moon at night).
    pub light_dir: Vec3,
    /// Tint/intensity of the directional world light.
    pub light_color: Vec3,
    /// Ambient floor for world shading in `[0,1]`.
    pub ambient: f32,
    /// Sky color straight up.
    pub zenith_color: Vec3,
    /// Sky color at the horizon.
    pub horizon_color: Vec3,
    /// Sun disc tint.
    pub sun_color: Vec3,
    /// Star visibility, `0` by day → `1` at night.
    pub star_intensity: f32,
    /// Moon disc visibility, `0` by day → `1` at night.
    pub moon_intensity: f32,
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t.clamp(0.0, 1.0)
}

/// GLSL-style smoothstep; supports a descending range (`edge0 > edge1`).
fn smoothstep(edge0: f32, edge1: f32, x: f32) -> f32 {
    if (edge1 - edge0).abs() < f32::EPSILON {
        return if x < edge0 { 0.0 } else { 1.0 };
    }
    let t = ((x - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn advance_wraps_past_end_of_day() {
        let mut cycle = DayCycle::new(0.9);
        cycle.advance(0.2 * DEFAULT_DAY_LENGTH_SECS);
        assert!((cycle.time_of_day() - 0.1).abs() < 1.0e-4);
    }

    #[test]
    fn full_cycle_returns_to_start() {
        let mut cycle = DayCycle::new(0.0);
        cycle.advance(DEFAULT_DAY_LENGTH_SECS);
        assert!(cycle.time_of_day() < 1.0e-3 || cycle.time_of_day() > 1.0 - 1.0e-3);
    }

    #[test]
    fn set_time_of_day_wraps() {
        let mut cycle = DayCycle::new(0.0);
        cycle.set_time_of_day(1.25);
        assert!((cycle.time_of_day() - 0.25).abs() < 1.0e-6);
    }

    #[test]
    fn sun_is_overhead_at_noon() {
        let sun = DayCycle::new(0.5).sun_direction();
        assert!(sun.y > 0.9, "noon sun should point up, got {sun:?}");
    }

    #[test]
    fn sun_is_below_at_midnight() {
        let sun = DayCycle::new(0.0).sun_direction();
        assert!(sun.y < -0.9, "midnight sun should point down, got {sun:?}");
    }

    #[test]
    fn sun_is_near_horizon_at_sunrise_and_sunset() {
        assert!(DayCycle::new(0.25).sun_direction().y.abs() < 0.2);
        assert!(DayCycle::new(0.75).sun_direction().y.abs() < 0.2);
    }

    #[test]
    fn night_spans_midnight_but_not_noon() {
        assert!(DayCycle::new(0.0).is_night(), "midnight is night");
        assert!(DayCycle::new(0.9).is_night(), "late evening is night");
        assert!(!DayCycle::new(0.5).is_night(), "noon is day");
        assert!(!DayCycle::new(0.3).is_night(), "mid-morning is day");
    }

    #[test]
    fn daylight_is_brighter_than_night() {
        let noon = DayCycle::new(0.5).atmosphere();
        let midnight = DayCycle::new(0.0).atmosphere();
        assert!(noon.ambient > midnight.ambient);
        assert!(noon.light_color.length() > midnight.light_color.length());
    }

    #[test]
    fn stars_fade_in_at_night() {
        assert!(DayCycle::new(0.5).atmosphere().star_intensity < 0.05);
        assert!(DayCycle::new(0.0).atmosphere().star_intensity > 0.95);
    }
}

//! Frame timing and a fixed-timestep accumulator for deterministic simulation.

use std::time::{Duration, Instant};

/// Simulation runs at this fixed rate; rendering interpolates between ticks.
pub const TICKS_PER_SECOND: u32 = 60;
const FIXED_DT: f32 = 1.0 / TICKS_PER_SECOND as f32;

/// Tracks wall-clock delta time between frames and accumulates fixed-timestep
/// updates so physics/networking advance at a stable rate regardless of FPS.
pub struct Clock {
    last: Instant,
    /// Seconds since the last frame (variable; for camera smoothing etc.).
    delta: f32,
    accumulator: f32,
    /// Total elapsed seconds since the clock was created.
    elapsed: f32,
    frame_count: u64,
}

impl Clock {
    pub fn new() -> Self {
        Self {
            last: Instant::now(),
            delta: 0.0,
            accumulator: 0.0,
            elapsed: 0.0,
            frame_count: 0,
        }
    }

    /// Call once per rendered frame. Returns the variable frame delta in seconds
    /// and feeds the fixed-timestep accumulator.
    pub fn tick(&mut self) -> f32 {
        let now = Instant::now();
        self.delta = (now - self.last).as_secs_f32().min(0.25); // clamp huge stalls
        self.last = now;
        self.elapsed += self.delta;
        self.accumulator += self.delta;
        self.frame_count += 1;
        self.delta
    }

    /// Drain one fixed step if enough time has accumulated. Loop on this:
    /// `while clock.next_fixed_step() { simulate(FIXED_DT); }`
    pub fn next_fixed_step(&mut self) -> bool {
        if self.accumulator >= FIXED_DT {
            self.accumulator -= FIXED_DT;
            true
        } else {
            false
        }
    }

    /// Fraction `[0,1)` through the current fixed step, for render interpolation.
    pub fn interpolation_alpha(&self) -> f32 {
        self.accumulator / FIXED_DT
    }

    pub fn fixed_dt(&self) -> f32 {
        FIXED_DT
    }

    pub fn delta(&self) -> f32 {
        self.delta
    }

    pub fn elapsed(&self) -> f32 {
        self.elapsed
    }

    /// Instantaneous FPS estimate from the last frame delta.
    pub fn fps(&self) -> f32 {
        if self.delta > 0.0 {
            1.0 / self.delta
        } else {
            0.0
        }
    }
}

impl Default for Clock {
    fn default() -> Self {
        Self::new()
    }
}

/// Convenience: a duration from whole milliseconds.
pub fn millis(ms: u64) -> Duration {
    Duration::from_millis(ms)
}

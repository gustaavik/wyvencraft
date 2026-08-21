//! A tiny deterministic random-number generator for gameplay decisions
//! (mob AI timers, spawn rolls, drop counts).
//!
//! The engine has no `rand` dependency by design: every roll must be
//! reproducible from a seed so tests can pin behavior and networked peers
//! can agree. This is a SplitMix64 stream (same mixing constants as the
//! worldgen `feature_hash`), which is plenty for gameplay and has no state
//! beyond one `u64`.

/// A seedable SplitMix64 random stream.
#[derive(Debug, Clone)]
pub struct Rng64(u64);

impl Rng64 {
    pub fn new(seed: u64) -> Self {
        Self(seed)
    }

    pub fn next_u64(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut h = self.0;
        h ^= h >> 30;
        h = h.wrapping_mul(0xBF58_476D_1CE4_E5B9);
        h ^= h >> 27;
        h = h.wrapping_mul(0x94D0_49BB_1331_11EB);
        h ^ (h >> 31)
    }

    /// Uniform in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        // 24 mantissa bits keep the conversion exact and strictly below 1.0.
        (self.next_u64() >> 40) as f32 / (1u32 << 24) as f32
    }

    /// Uniform in `[lo, hi)` (or `lo` when the range is empty).
    pub fn range_f32(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo).max(0.0) * self.next_f32()
    }

    /// Uniform integer in `[lo, hi]` inclusive.
    pub fn range_u32(&mut self, lo: u32, hi: u32) -> u32 {
        if hi <= lo {
            return lo;
        }
        lo + (self.next_u64() % u64::from(hi - lo + 1)) as u32
    }

    /// Index into `weights` picked proportionally to each entry; `None` when
    /// every weight is zero (or the slice is empty).
    pub fn pick_weighted(&mut self, weights: &[u32]) -> Option<usize> {
        let total: u64 = weights.iter().map(|&w| u64::from(w)).sum();
        if total == 0 {
            return None;
        }
        let mut roll = self.next_u64() % total;
        for (i, &w) in weights.iter().enumerate() {
            let w = u64::from(w);
            if roll < w {
                return Some(i);
            }
            roll -= w;
        }
        unreachable!("roll is bounded by the summed weights")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_seed_reproduces_the_stream() {
        let mut a = Rng64::new(42);
        let mut b = Rng64::new(42);
        for _ in 0..100 {
            assert_eq!(a.next_u64(), b.next_u64());
        }
        let mut c = Rng64::new(43);
        assert_ne!(Rng64::new(42).next_u64(), c.next_u64(), "seeds must differ");
    }

    #[test]
    fn floats_stay_in_unit_range_and_vary() {
        let mut rng = Rng64::new(7);
        let mut min: f32 = 1.0;
        let mut max: f32 = 0.0;
        for _ in 0..1000 {
            let v = rng.next_f32();
            assert!((0.0..1.0).contains(&v), "out of range: {v}");
            min = min.min(v);
            max = max.max(v);
        }
        assert!(min < 0.1 && max > 0.9, "poor spread: [{min}, {max}]");
    }

    #[test]
    fn integer_ranges_are_inclusive_and_bounded() {
        let mut rng = Rng64::new(1);
        let mut seen = [false; 4];
        for _ in 0..200 {
            let v = rng.range_u32(2, 5);
            assert!((2..=5).contains(&v));
            seen[(v - 2) as usize] = true;
        }
        assert!(seen.iter().all(|&s| s), "all values in [2,5] should occur");
        assert_eq!(rng.range_u32(9, 9), 9);
        assert_eq!(rng.range_u32(9, 3), 9, "empty range returns lo");
    }

    #[test]
    fn weighted_picks_follow_the_weights() {
        let mut rng = Rng64::new(11);
        assert_eq!(rng.pick_weighted(&[]), None);
        assert_eq!(rng.pick_weighted(&[0, 0]), None);
        let mut counts = [0u32; 3];
        for _ in 0..3000 {
            counts[rng.pick_weighted(&[1, 0, 9]).unwrap()] += 1;
        }
        assert_eq!(counts[1], 0, "zero weight never picked");
        assert!(
            counts[2] > counts[0] * 4,
            "9:1 weights should skew: {counts:?}"
        );
    }
}

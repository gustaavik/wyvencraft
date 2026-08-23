//! The block-break crack overlay: eight PNGs, pinned to a fixed atlas band.
//!
//! The overlay is addressed by *index* rather than by name — the mining code
//! picks a stage from a progress fraction — so the band is reserved
//! ([`crate::art::reserved`]) instead of allocated through the [`TileSource`]
//! like ordinary named art.
//!
//! [`TileSource`]: wyven_render::TileSource

use std::sync::OnceLock;

use wyven_render::{TileRgba, decode_tile};

/// Atlas tile of the first (faintest) crack stage. Row 3 of the atlas.
pub const CRACK_0: u32 = 48;
/// How many stages the overlay steps through between untouched and broken.
pub const CRACK_STAGES: u32 = 8;

/// The stage tiles as `(tile index, pixels)` pairs, for the reserved band.
///
/// A stage whose PNG is absent or unusable is simply left out, which is what
/// makes [`tile`] answer `None` and the mining overlay draw nothing at all. A
/// magenta marker would be worse than no overlay here: it is drawn *over* the
/// block you are breaking, so it would hide the thing you are looking at
/// rather than read as missing art.
pub fn atlas_tiles() -> impl Iterator<Item = (u32, TileRgba)> {
    let art: Vec<_> = (0..CRACK_STAGES)
        .filter_map(|stage| load(stage).map(|art| (CRACK_0 + stage, art)))
        .collect();
    if art.len() as u32 != CRACK_STAGES {
        log::warn!(
            "block-break overlay: {} of {CRACK_STAGES} assets/textures/crack_<n>.png present; \
             no crack overlay will be drawn",
            art.len()
        );
    }
    // Whether the overlay is usable is decided here, while the atlas is being
    // built, so `tile` never touches the filesystem — it answers once per frame
    // for as long as a block is being mined.
    let _ = HAS_ART.set(art.len() as u32 == CRACK_STAGES);
    art.into_iter()
}

/// Set once while the atlas is assembled; see [`atlas_tiles`].
static HAS_ART: OnceLock<bool> = OnceLock::new();

/// Crack-overlay tile for a break progress in `[0, 1]`, or `None` when the
/// overlay has no art and should not be drawn.
pub fn tile(progress: f32) -> Option<u32> {
    if !HAS_ART.get().copied().unwrap_or(false) {
        return None;
    }
    let stage = (progress.clamp(0.0, 1.0) * CRACK_STAGES as f32) as u32;
    Some(CRACK_0 + stage.min(CRACK_STAGES - 1))
}

/// Decode `assets/textures/crack_<stage>.png`, or `None` if it is absent or
/// unusable.
fn load(stage: u32) -> Option<TileRgba> {
    let path = format!("assets/textures/crack_{stage}.png");
    let bytes = std::fs::read(&path).ok()?;
    match decode_tile(&bytes) {
        Ok(art) => Some(art),
        Err(err) => {
            log::warn!("ignoring {path}: {err}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The overlay art has to actually be on disk. Every other test here
    /// tolerates its absence — which is exactly how the whole effect once went
    /// missing in silence, with one startup warning as the only sign.
    #[test]
    fn every_stage_has_art() {
        assert_eq!(
            atlas_tiles().count() as u32,
            CRACK_STAGES,
            "assets/textures/crack_<0..{CRACK_STAGES}>.png must all be present"
        );
    }

    /// Every progress fraction lands inside the reserved band — or, with no
    /// crack art on disk, draws nothing at all rather than a magenta block.
    #[test]
    fn progress_maps_into_the_band_or_nothing() {
        let complete = atlas_tiles().count() as u32 == CRACK_STAGES;
        for progress in [0.0, 0.5, 1.0, 2.0, -1.0] {
            match tile(progress) {
                Some(t) => {
                    assert!(complete, "art is incomplete but an overlay was offered");
                    assert!((CRACK_0..CRACK_0 + CRACK_STAGES).contains(&t));
                }
                None => assert!(!complete, "art is complete but no overlay was offered"),
            }
        }
    }
}

use std::collections::HashSet;

use crate::{AudioBackend, AudioClip, PlaybackHandle};

/// Discards everything.
///
/// The real backend's fallback when no output device is available, and the
/// natural test double — mirrors this game's `NullWorldRepository`.
#[derive(Default)]
pub struct NullAudioBackend {
    next_id: u64,
    /// Handles minted by `play()` and not yet `stop()`-ped. Tracking the
    /// *active* set (rather than the stopped set) is what makes an unknown
    /// handle read as finished too, matching `AudioBackend::is_finished`'s
    /// contract.
    active: HashSet<PlaybackHandle>,
}

impl NullAudioBackend {
    pub fn new() -> Self {
        Self::default()
    }
}

impl AudioBackend for NullAudioBackend {
    fn play(&mut self, _clip: &AudioClip, _volume: f32) -> PlaybackHandle {
        let id = PlaybackHandle::new(self.next_id);
        self.next_id += 1;
        self.active.insert(id);
        id
    }

    /// Never finishes on its own: there is no real playback to reach the end
    /// of, so a session with no audio device sits quietly rather than
    /// retrying a restart on every tick for the rest of the run.
    fn is_finished(&self, handle: PlaybackHandle) -> bool {
        !self.active.contains(&handle)
    }

    fn set_volume(&mut self, _handle: PlaybackHandle, _volume: f32) {}

    fn stop(&mut self, handle: PlaybackHandle) {
        self.active.remove(&handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn clip() -> AudioClip {
        AudioClip::from_bytes(Vec::new())
    }

    #[test]
    fn successive_plays_return_distinct_handles() {
        let mut backend = NullAudioBackend::new();
        let a = backend.play(&clip(), 1.0);
        let b = backend.play(&clip(), 1.0);
        assert_ne!(a, b);
    }

    #[test]
    fn is_finished_only_after_stop() {
        let mut backend = NullAudioBackend::new();
        let handle = backend.play(&clip(), 1.0);
        assert!(!backend.is_finished(handle));
        backend.stop(handle);
        assert!(backend.is_finished(handle));
    }

    #[test]
    fn an_unknown_handle_reads_as_finished() {
        let backend = NullAudioBackend::new();
        assert!(backend.is_finished(PlaybackHandle::new(999)));
    }

    #[test]
    fn set_volume_and_stop_on_an_unknown_handle_never_panics() {
        let mut backend = NullAudioBackend::new();
        backend.set_volume(PlaybackHandle::new(999), 0.5);
        backend.stop(PlaybackHandle::new(999));
    }
}

//! Orchestrates the sound registry and the [`wyven_audio::AudioBackend`].

use std::collections::HashMap;

use wyven_assets::AssetSource;
use wyven_audio::{AudioBackend, AudioClip, PlaybackHandle};

use super::registry::SoundRegistry;

/// Loaded clips, keyed by their `audio.toml` id, plus the backend they play
/// through.
///
/// Eager-load, dependency-injected: every registered sound's bytes are read
/// once at construction, which is simpler than caching lazily per-play, is
/// fully unit-testable with no filesystem or device, and unifies "unknown
/// id" and "id whose file failed to read" into one path — both are just
/// absent from `clips`.
pub struct AudioManager {
    backend: Box<dyn AudioBackend>,
    clips: HashMap<String, (AudioClip, f32)>,
}

impl AudioManager {
    /// Reads every registered sound's bytes once, up front — there are only
    /// a handful, and this is the same moment `GameContent::from_source`
    /// reads everything else. A sound whose file cannot be read logs a
    /// warning and is simply absent from the cache.
    pub fn new(
        sounds: &SoundRegistry,
        source: &dyn AssetSource,
        backend: Box<dyn AudioBackend>,
    ) -> Self {
        let clips = sounds
            .iter()
            .filter_map(|def| match source.read_bytes(&def.path) {
                Ok(bytes) => Some((def.id.clone(), (AudioClip::from_bytes(bytes), def.volume))),
                Err(err) => {
                    log::warn!("could not read sound {:?} ({}): {err}", def.id, def.path);
                    None
                }
            })
            .collect();
        Self { backend, clips }
    }

    /// Fire-and-forget one-shot at the registry's own default volume. This
    /// is the "easy to add new SFX" entry point: one `[[sound]]` row in
    /// `assets/audio.toml` and one call here, no other file changes.
    pub fn play_sound(&mut self, id: &str) {
        let Some((clip, volume)) = self.clips.get(id) else {
            log::warn!("no such sound {id:?}; playing nothing");
            return;
        };
        self.backend.play(clip, *volume);
    }

    /// Starts a caller-managed track at an explicit starting volume — the
    /// caller drives its own fade/loop timing via `set_volume`/`is_finished`.
    /// `None` if `id` is not registered or failed to load.
    pub fn play_music(&mut self, id: &str, volume: f32) -> Option<PlaybackHandle> {
        let (clip, _) = self.clips.get(id).or_else(|| {
            log::warn!("no such music {id:?}; playing nothing");
            None
        })?;
        Some(self.backend.play(clip, volume))
    }

    pub fn set_volume(&mut self, handle: PlaybackHandle, volume: f32) {
        self.backend.set_volume(handle, volume);
    }

    pub fn is_finished(&self, handle: PlaybackHandle) -> bool {
        self.backend.is_finished(handle)
    }

    pub fn stop(&mut self, handle: PlaybackHandle) {
        self.backend.stop(handle);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wyven_assets::MapSource;
    use wyven_audio::NullAudioBackend;

    fn registry() -> SoundRegistry {
        SoundRegistry::from_toml(
            r#"
            [[sound]]
            id = "click"
            path = "assets/audio/click.wav"
            category = "sfx"
            volume = 0.6
            "#,
        )
        .expect("valid fixture")
    }

    #[test]
    fn play_sound_on_an_unknown_id_warns_and_does_not_panic() {
        let mut manager = AudioManager::new(
            &registry(),
            &MapSource::new().with_bytes("assets/audio/click.wav", b"x".to_vec()),
            Box::new(NullAudioBackend::new()),
        );
        manager.play_sound("no_such_sound");
    }

    #[test]
    fn play_music_on_a_loaded_id_returns_a_handle() {
        let mut manager = AudioManager::new(
            &registry(),
            &MapSource::new().with_bytes("assets/audio/click.wav", b"x".to_vec()),
            Box::new(NullAudioBackend::new()),
        );
        assert!(manager.play_music("click", 1.0).is_some());
    }

    #[test]
    fn an_id_whose_file_could_not_be_read_behaves_like_an_unknown_id() {
        // No fixture bytes registered for "click"'s path.
        let mut manager = AudioManager::new(
            &registry(),
            &MapSource::new(),
            Box::new(NullAudioBackend::new()),
        );
        assert!(manager.play_music("click", 1.0).is_none());
    }
}

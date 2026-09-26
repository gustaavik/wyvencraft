//! Sound and music: the registry (`assets/audio.toml`), the manager that
//! plays it, and the menu-music fade/restart timing.
//!
//! Adding a new fire-and-forget sound effect is a data change plus one call:
//! a `[[sound]]` row in `assets/audio.toml` and a
//! `shared.audio.play_sound("id")` at the moment that should trigger it.
//! Nothing here needs to change for that. A sound with its own bespoke
//! timing (a fade, a cooldown, a crossfade) instead gets a small pure state
//! machine of its own, beside whatever owns that moment — [`MenuMusicPlayer`]
//! is the worked example, not a framework to extend.

mod manager;
mod music;
mod registry;

pub use manager::AudioManager;
pub use music::{MenuMusic, MenuMusicPlayer, MusicIntent};
pub use registry::{BUILTIN_AUDIO, SoundCategory, SoundDef, SoundRegistry};

/// The registered id of the main menu's track — see `assets/audio.toml`.
/// Every menu-flow screen that wants it audible ticks the shared
/// [`MenuMusic`] on `Shared` rather than knowing this id itself.
pub const MAINMENU_THEME: &str = "mainmenu_theme";

/// Open the real device, or fall back to silence.
///
/// The one place this game decides between [`wyven_audio::RodioBackend`] and
/// [`wyven_audio::NullAudioBackend`] — mirrors how a session decides between
/// `FileWorldRepository` and `NullWorldRepository`.
pub fn open_default_backend() -> Box<dyn wyven_audio::AudioBackend> {
    match wyven_audio::RodioBackend::try_new() {
        Some(backend) => Box::new(backend),
        None => {
            log::warn!("no audio output device available; sound is disabled");
            Box::new(wyven_audio::NullAudioBackend::new())
        }
    }
}

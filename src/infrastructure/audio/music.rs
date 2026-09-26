//! Fade-in / wait-and-restart timing for one looping music track, and the
//! stateful glue ([`MenuMusic`]) that ties it to a live [`AudioManager`].
//!
//! [`MenuMusicPlayer`] owns no backend and does no I/O — the same shape as
//! `entity::brain::MobBrain::think`: facts and `dt` in, an intent out, the
//! caller performs the actual side effect. This is the template for any
//! future sound that needs its own bespoke timing (see `crate::infrastructure::audio`'s
//! module doc).

use super::manager::AudioManager;

/// Seconds to linearly ramp the track from silent to full volume.
pub const FADE_IN_SECONDS: f32 = 2.0;
/// Seconds of silence after the track ends before it restarts.
pub const RESTART_DELAY_SECONDS: f32 = 5.0;
/// Seconds to linearly ramp the track down to silent when asked to stop.
pub const FADE_OUT_SECONDS: f32 = 1.0;

#[derive(Debug, Clone, Copy, PartialEq, Default)]
enum Phase {
    #[default]
    NotStarted,
    FadingIn {
        elapsed: f32,
    },
    Playing,
    Waiting {
        elapsed: f32,
    },
    /// `from` is the volume (0.0..=1.0) fading out started at — wherever the
    /// track actually was, not assumed to be full, so a stop requested
    /// mid-fade-in ramps down smoothly from its own current level instead of
    /// jumping up to 1.0 first.
    FadingOut {
        elapsed: f32,
        from: f32,
    },
}

/// What the caller should do this frame, in response to `tick`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MusicIntent {
    /// Nothing to do.
    None,
    /// (Re)start the track from silence. `volume` is always 0.0 in practice,
    /// but carried explicitly so the caller never has to know that.
    Start { volume: f32 },
    /// Set the volume of the already-playing track.
    SetVolume(f32),
    /// The fade-out has finished (or there was nothing to fade): actually
    /// stop the backend playback now.
    Stop,
}

/// A looping-with-a-gap music player: fades in, plays to the end, waits
/// [`RESTART_DELAY_SECONDS`], then fades in again — until asked to
/// [`MenuMusicPlayer::request_stop`], at which point it fades out and comes
/// to rest exactly as if it had never started, ready to fade in again on the
/// next `tick`.
#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct MenuMusicPlayer {
    phase: Phase,
}

impl MenuMusicPlayer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether there is still anything going on — playing, fading in,
    /// waiting to restart, or fading out. `false` only at rest (never
    /// started, or fully faded out).
    ///
    /// This is what lets a caller that is not itself a menu screen (a screen
    /// the track is fading out *into*) tick only while a fade-out is
    /// actually in progress, rather than needing some separate signal to
    /// stop asking once it settles — the moment it settles, this goes false
    /// and the caller simply stops calling `tick` at all.
    pub fn is_active(&self) -> bool {
        !matches!(self.phase, Phase::NotStarted)
    }

    /// Begin fading the track out from wherever it currently is, rather than
    /// cutting it. A no-op if it is already fading out or was never started.
    pub fn request_stop(&mut self) {
        let from = match self.phase {
            Phase::NotStarted | Phase::FadingOut { .. } => return,
            Phase::FadingIn { elapsed } => (elapsed / FADE_IN_SECONDS).clamp(0.0, 1.0),
            Phase::Playing => 1.0,
            Phase::Waiting { .. } => 0.0,
        };
        self.phase = if from <= 0.0 {
            // Nothing audible to fade — e.g. waiting out the gap between
            // plays, at silence already.
            Phase::NotStarted
        } else {
            Phase::FadingOut { elapsed: 0.0, from }
        };
    }

    /// `finished` is whether the track this player is tracking has reached
    /// its end *this frame* — the caller already asked the backend.
    pub fn tick(&mut self, dt: f32, finished: bool) -> MusicIntent {
        let dt = dt.max(0.0);
        match self.phase {
            Phase::NotStarted => {
                self.phase = Phase::FadingIn { elapsed: 0.0 };
                MusicIntent::Start { volume: 0.0 }
            }
            Phase::FadingIn { elapsed } => {
                if finished {
                    // A clip shorter than the fade still ends cleanly into
                    // the wait, rather than trying to keep ramping a volume
                    // that no longer has anything playing under it.
                    self.phase = Phase::Waiting { elapsed: 0.0 };
                    return MusicIntent::None;
                }
                let elapsed = elapsed + dt;
                if elapsed >= FADE_IN_SECONDS {
                    self.phase = Phase::Playing;
                    MusicIntent::SetVolume(1.0)
                } else {
                    self.phase = Phase::FadingIn { elapsed };
                    MusicIntent::SetVolume((elapsed / FADE_IN_SECONDS).clamp(0.0, 1.0))
                }
            }
            Phase::Playing => {
                if finished {
                    self.phase = Phase::Waiting { elapsed: 0.0 };
                }
                MusicIntent::None
            }
            Phase::Waiting { elapsed } => {
                let elapsed = elapsed + dt;
                if elapsed >= RESTART_DELAY_SECONDS {
                    self.phase = Phase::FadingIn { elapsed: 0.0 };
                    MusicIntent::Start { volume: 0.0 }
                } else {
                    self.phase = Phase::Waiting { elapsed };
                    MusicIntent::None
                }
            }
            Phase::FadingOut { elapsed, from } => {
                if finished {
                    // Nothing left to ramp down — it ended under us.
                    self.phase = Phase::NotStarted;
                    return MusicIntent::Stop;
                }
                let elapsed = elapsed + dt;
                if elapsed >= FADE_OUT_SECONDS {
                    self.phase = Phase::NotStarted;
                    MusicIntent::Stop
                } else {
                    self.phase = Phase::FadingOut { elapsed, from };
                    MusicIntent::SetVolume(from * (1.0 - elapsed / FADE_OUT_SECONDS))
                }
            }
        }
    }
}

/// Ties a [`MenuMusicPlayer`]'s fade/restart timing to a live [`AudioManager`]
/// playback handle, for one named track played at up to `target_volume` (the
/// track's own authored default from `assets/audio.toml`, resolved once at
/// construction).
///
/// This is what lets several screens share one continuous playthrough:
/// [`MenuMusic`] lives on `Shared` rather than on any one screen, so a screen
/// that wants the track audible just calls [`MenuMusic::tick`] each frame —
/// the fade/restart state and the live handle both live here, not in the
/// screen, so navigating between menu screens can never restart the track or
/// lose track of it. [`MenuMusic::fade_out`] is the one place that actually
/// ends it, called where the game truly leaves the menu context (entering a
/// world) — the fade itself is then driven by that same screen's own `tick`
/// calls for as long as [`MenuMusic::is_active`] says there is still
/// something to fade.
pub struct MenuMusic {
    track: &'static str,
    target_volume: f32,
    player: MenuMusicPlayer,
    handle: Option<wyven_audio::PlaybackHandle>,
}

impl MenuMusic {
    pub fn new(track: &'static str, target_volume: f32) -> Self {
        Self {
            track,
            target_volume,
            player: MenuMusicPlayer::new(),
            handle: None,
        }
    }

    /// Whether there is still anything going on that needs ticking — see
    /// [`MenuMusicPlayer::is_active`].
    pub fn is_active(&self) -> bool {
        self.player.is_active()
    }

    /// Advance the fade/restart/fade-out timing by `dt` and apply whatever
    /// it decides to `audio`. Call this from every screen the track should
    /// stay audible through; skipping a frame (a screen that does not call
    /// it) merely pauses the timing for that frame — playback already under
    /// way keeps sounding regardless, since the backend does not need to be
    /// ticked to keep outputting a sound it was already told to play.
    pub fn tick(&mut self, dt: f32, audio: &mut AudioManager) {
        let finished = self.handle.is_some_and(|h| audio.is_finished(h));
        match self.player.tick(dt, finished) {
            MusicIntent::None => {}
            MusicIntent::Start { volume } => {
                // On the very first tick `handle` is `None` (no-op here); on
                // a restart the old track already finished (that's why we're
                // here), so this is tidy cleanup, not an audible cut.
                if let Some(old) = self.handle.take() {
                    audio.stop(old);
                }
                self.handle = audio.play_music(self.track, volume * self.target_volume);
            }
            MusicIntent::SetVolume(volume) => {
                if let Some(handle) = self.handle {
                    audio.set_volume(handle, volume * self.target_volume);
                }
            }
            MusicIntent::Stop => {
                if let Some(handle) = self.handle.take() {
                    audio.stop(handle);
                }
            }
        }
    }

    /// Begin fading the track out — see [`MenuMusicPlayer::request_stop`].
    /// The fade itself only advances on subsequent `tick` calls, so the
    /// caller keeps ticking (guarded by [`MenuMusic::is_active`]) until it
    /// completes.
    pub fn fade_out(&mut self) {
        self.player.request_stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_fading_in_on_the_first_tick() {
        let mut player = MenuMusicPlayer::new();
        assert_eq!(player.tick(0.0, false), MusicIntent::Start { volume: 0.0 });
    }

    #[test]
    fn fade_volume_ramps_linearly_to_one_over_fade_seconds() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false); // Start
        assert_eq!(
            player.tick(FADE_IN_SECONDS / 2.0, false),
            MusicIntent::SetVolume(0.5)
        );
    }

    #[test]
    fn reaches_full_volume_and_settles_into_playing() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false); // Start
        assert_eq!(
            player.tick(FADE_IN_SECONDS, false),
            MusicIntent::SetVolume(1.0)
        );
        // Playing now: further ticks with no `finished` do nothing.
        assert_eq!(player.tick(1.0, false), MusicIntent::None);
    }

    #[test]
    fn finishing_during_fade_in_goes_straight_to_waiting() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false); // Start
        player.tick(0.1, false); // still fading in
        assert_eq!(player.tick(0.1, true), MusicIntent::None);
        // Now waiting: it must not restart before the delay elapses.
        assert_eq!(
            player.tick(RESTART_DELAY_SECONDS - 0.1, false),
            MusicIntent::None
        );
    }

    #[test]
    fn finishing_while_playing_starts_the_five_second_wait() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false); // Start
        player.tick(FADE_IN_SECONDS, false); // reaches Playing
        assert_eq!(player.tick(1.0, true), MusicIntent::None);
        // Not yet restarted.
        assert_eq!(
            player.tick(RESTART_DELAY_SECONDS - 0.5, false),
            MusicIntent::None
        );
    }

    #[test]
    fn restarts_after_the_wait_elapses_with_a_fresh_fade_in() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false); // Start
        player.tick(FADE_IN_SECONDS, false); // Playing
        player.tick(0.0, true); // finished -> Waiting
        assert_eq!(
            player.tick(RESTART_DELAY_SECONDS, false),
            MusicIntent::Start { volume: 0.0 }
        );
        // And the fade ramps again from zero.
        assert_eq!(
            player.tick(FADE_IN_SECONDS / 4.0, false),
            MusicIntent::SetVolume(0.25)
        );
    }

    #[test]
    fn waiting_does_not_restart_early() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS, false);
        player.tick(0.0, true);
        for _ in 0..10 {
            assert_eq!(
                player.tick(RESTART_DELAY_SECONDS / 11.0, false),
                MusicIntent::None
            );
        }
    }

    #[test]
    fn zero_or_negative_dt_never_panics_or_moves_backward() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        assert_eq!(player.tick(-1.0, false), MusicIntent::SetVolume(0.0));
        assert_eq!(player.tick(0.0, false), MusicIntent::SetVolume(0.0));
    }

    #[test]
    fn a_stop_request_before_anything_started_is_a_no_op() {
        let mut player = MenuMusicPlayer::new();
        assert!(!player.is_active());
        player.request_stop();
        assert!(!player.is_active());
        // Still starts fresh on the next tick, exactly as if nothing happened.
        assert_eq!(player.tick(0.0, false), MusicIntent::Start { volume: 0.0 });
    }

    #[test]
    fn stopping_while_playing_fades_out_from_full_volume_then_stops() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS, false); // reaches Playing at 1.0
        player.request_stop();
        assert!(player.is_active());
        assert_eq!(
            player.tick(FADE_OUT_SECONDS / 2.0, false),
            MusicIntent::SetVolume(0.5)
        );
        assert_eq!(
            player.tick(FADE_OUT_SECONDS / 2.0, false),
            MusicIntent::Stop
        );
        assert!(!player.is_active());
    }

    #[test]
    fn stopping_mid_fade_in_ramps_down_from_its_own_level_not_from_full() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS / 2.0, false); // halfway through fading in, at 0.5
        player.request_stop();
        // Fading out from 0.5, not from 1.0.
        assert_eq!(
            player.tick(FADE_OUT_SECONDS / 2.0, false),
            MusicIntent::SetVolume(0.25)
        );
    }

    #[test]
    fn stopping_while_waiting_for_a_restart_stops_immediately_with_nothing_to_fade() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS, false); // Playing
        player.tick(0.0, true); // finished -> Waiting, already silent
        player.request_stop();
        assert!(!player.is_active());
    }

    #[test]
    fn a_track_that_ends_mid_fade_out_stops_immediately() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS, false);
        player.request_stop();
        assert_eq!(player.tick(0.1, true), MusicIntent::Stop);
        assert!(!player.is_active());
    }

    #[test]
    fn a_second_stop_request_while_already_fading_out_does_not_restart_the_fade() {
        let mut player = MenuMusicPlayer::new();
        player.tick(0.0, false);
        player.tick(FADE_IN_SECONDS, false);
        player.request_stop();
        player.tick(FADE_OUT_SECONDS / 2.0, false);
        player.request_stop(); // must not reset elapsed back to 0
        assert_eq!(
            player.tick(FADE_OUT_SECONDS / 2.0, false),
            MusicIntent::Stop
        );
    }

    fn manager_with_one_track() -> AudioManager {
        let registry = crate::infrastructure::audio::SoundRegistry::from_toml(
            r#"
            [[sound]]
            id = "theme"
            path = "assets/audio/theme.wav"
            category = "music"
            volume = 1.0
            "#,
        )
        .expect("valid fixture");
        AudioManager::new(
            &registry,
            &wyven_assets::MapSource::new().with_bytes("assets/audio/theme.wav", b"x".to_vec()),
            Box::new(wyven_audio::NullAudioBackend::new()),
        )
    }

    #[test]
    fn a_second_screens_first_tick_does_not_restart_an_already_playing_track() {
        let mut audio = manager_with_one_track();
        let mut music = MenuMusic::new("theme", 1.0);
        // One screen's worth of ticks: starts, ramps partway through the fade.
        music.tick(0.0, &mut audio);
        music.tick(FADE_IN_SECONDS / 2.0, &mut audio);
        let handle_after_first_screen = music.handle;

        // Navigating to a different screen that also calls `tick` must not
        // see a fresh `NotStarted` — the state (and the handle) persisted.
        music.tick(0.1, &mut audio);
        assert_eq!(music.handle, handle_after_first_screen);
    }

    #[test]
    fn fade_out_ends_the_track_and_the_next_tick_starts_a_fresh_fade_in() {
        let mut audio = manager_with_one_track();
        let mut music = MenuMusic::new("theme", 1.0);
        music.tick(0.0, &mut audio);
        music.tick(FADE_IN_SECONDS, &mut audio); // reaches Playing
        music.fade_out();

        // The fade-out only advances on ticks, exactly like fading in.
        assert!(music.is_active());
        music.tick(FADE_OUT_SECONDS, &mut audio);
        assert!(!music.is_active());
        assert!(music.handle.is_none());

        // Resuming after the fade-out completes starts over, not mid-track.
        music.tick(0.0, &mut audio);
        assert!(!audio.is_finished(music.handle.expect("restarted")));
    }
}

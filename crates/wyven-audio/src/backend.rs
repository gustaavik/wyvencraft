use crate::{AudioClip, PlaybackHandle};

/// A device an [`AudioClip`] can be played through.
///
/// Four methods on purpose: start, adjust volume, ask whether it finished,
/// stop early. No pan, pitch or 3D position — nothing in this game asks for
/// any of that, and a knob nobody turns is untested by construction.
///
/// No `Send`/`Sync` bound: an [`AudioBackend`] is only ever driven from the
/// single main-loop thread, so it must not force a bound a platform's audio
/// stream type might not satisfy.
pub trait AudioBackend {
    /// Start playing `clip` once, at `volume` (0.0 silent .. 1.0 full).
    ///
    /// Always a single playthrough — a caller wanting to repeat (looping
    /// music included) calls again once [`AudioBackend::is_finished`] says
    /// so, rather than this trait growing a loop flag for the one caller
    /// that wants a *gap* before repeating.
    fn play(&mut self, clip: &AudioClip, volume: f32) -> PlaybackHandle;

    /// Whether `handle`'s sound has finished — or was never tracked at all.
    /// A stale or unknown handle reads as finished, never as still playing.
    fn is_finished(&self, handle: PlaybackHandle) -> bool;

    /// Change a still-playing sound's volume. No-op on a finished or unknown
    /// handle.
    fn set_volume(&mut self, handle: PlaybackHandle, volume: f32);

    /// Stop it immediately, if it is still going.
    fn stop(&mut self, handle: PlaybackHandle);
}

use std::collections::HashMap;
use std::io::Cursor;

use crate::{AudioBackend, AudioClip, PlaybackHandle};

/// Plays clips through the system's default audio output device, via `rodio`.
pub struct RodioBackend {
    // Must outlive every `Player` built from it, or playback stops.
    device: rodio::MixerDeviceSink,
    players: HashMap<PlaybackHandle, rodio::Player>,
    next_id: u64,
}

impl RodioBackend {
    /// Open the default output device. `None` — never a panic — if this
    /// machine has none: no sound hardware, headless CI, a sandboxed shell
    /// with no audio server. The caller falls back to a null backend.
    pub fn try_new() -> Option<Self> {
        let device = rodio::DeviceSinkBuilder::open_default_sink()
            .map_err(|err| log::warn!("no audio output device: {err}"))
            .ok()?;
        Some(Self {
            device,
            players: HashMap::new(),
            next_id: 0,
        })
    }

    /// Drop players that already played out, so a stream of fire-and-forget
    /// `play()` calls (one-shot SFX) never grows this map without bound.
    fn prune_finished(&mut self) {
        self.players.retain(|_, player| !player.empty());
    }
}

impl AudioBackend for RodioBackend {
    fn play(&mut self, clip: &AudioClip, volume: f32) -> PlaybackHandle {
        self.prune_finished();
        let player = rodio::Player::connect_new(self.device.mixer());
        player.set_volume(volume);
        // A fresh `Decoder` per play, over the clip's own byte buffer — cheap
        // for PCM WAV, and what lets a clip be restarted without re-reading
        // its file.
        match rodio::Decoder::new(Cursor::new(clip.bytes().clone())) {
            Ok(source) => player.append(source),
            Err(err) => log::warn!("could not decode a sound clip ({err}); playing silence"),
        }
        let id = PlaybackHandle::new(self.next_id);
        self.next_id += 1;
        self.players.insert(id, player);
        id
    }

    fn is_finished(&self, handle: PlaybackHandle) -> bool {
        self.players.get(&handle).is_none_or(rodio::Player::empty)
    }

    fn set_volume(&mut self, handle: PlaybackHandle, volume: f32) {
        if let Some(player) = self.players.get(&handle) {
            player.set_volume(volume);
        }
    }

    fn stop(&mut self, handle: PlaybackHandle) {
        if let Some(player) = self.players.remove(&handle) {
            player.stop();
        }
    }
}

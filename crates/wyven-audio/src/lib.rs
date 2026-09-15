//! Audio device output and mixing.
//!
//! Everything here knows how to open a device and start/fade/stop a clip.
//! Nothing here knows what a "menu theme" or a "block break" is — that
//! meaning lives on the game side, exactly as `wyven-render` draws triangles
//! without knowing what a block is.
//!
//! [`AudioBackend`] is the seam: [`RodioBackend`] is the real device,
//! [`NullAudioBackend`] is what a session with no audio hardware — or a test
//! — gets instead. Choosing between them happens once, at the game's
//! composition root, exactly like this game's `WorldRepository`/`Session`.

mod backend;
mod clip;
mod handle;
mod null_backend;
mod rodio_backend;

pub use backend::AudioBackend;
pub use clip::AudioClip;
pub use handle::PlaybackHandle;
pub use null_backend::NullAudioBackend;
pub use rodio_backend::RodioBackend;

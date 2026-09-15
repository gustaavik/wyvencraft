use std::sync::Arc;

/// The raw bytes of one loaded sound file, ready to hand to a backend.
///
/// Bytes, not a decoded buffer: decoding is deferred to
/// [`crate::AudioBackend::play`], which is what lets the same clip be
/// replayed — or restarted, as looping music is every time it ends — without
/// re-reading its file, and keeps this crate's public surface free of any
/// particular decoder's types. `Arc<[u8]>` rather than `Vec<u8>` so a repeat
/// `play()` clones a refcount, not the file.
#[derive(Debug, Clone)]
pub struct AudioClip {
    bytes: Arc<[u8]>,
}

impl AudioClip {
    pub fn from_bytes(bytes: Vec<u8>) -> Self {
        Self {
            bytes: bytes.into(),
        }
    }

    pub fn bytes(&self) -> &Arc<[u8]> {
        &self.bytes
    }
}

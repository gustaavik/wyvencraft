/// Opaque handle to one playing (or since-finished) sound.
///
/// Backends mint their own ids; nothing outside a backend constructs one —
/// callers only ever hold, compare and pass one back.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct PlaybackHandle(u64);

impl PlaybackHandle {
    pub(crate) fn new(id: u64) -> Self {
        Self(id)
    }
}

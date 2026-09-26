//! Filesystem helpers shared by every adapter that writes a file.

use std::fs;
use std::path::Path;

/// Write via a temp file in the same directory + rename, so an interrupted save
/// never leaves a half-written file behind.
pub fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, bytes)?;
    fs::rename(&tmp, path)
}

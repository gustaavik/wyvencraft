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

/// A text file's contents, with the path in the error.
pub fn read_text(path: &str) -> Result<String, String> {
    fs::read_to_string(path).map_err(|err| format!("{path}: {err}"))
}

/// Replace a text file's contents, with the path in the error.
pub fn write_text(path: &str, text: &str) -> Result<(), String> {
    fs::write(path, text).map_err(|err| format!("{path}: {err}"))
}

/// When a file was last written, if it exists and the platform says.
pub fn modified(path: &str) -> Option<std::time::SystemTime> {
    fs::metadata(path).ok()?.modified().ok()
}

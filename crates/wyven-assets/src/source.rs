//! Where an asset's bytes come from.
//!
//! Loaders don't care whether a file came off disk, out of the binary, or from a
//! test fixture — only that they get its bytes (or a "not found", which every
//! caller answers with its own builtin fallback). Keeping that behind
//! [`AssetSource`] is what lets content and model loading be tested without
//! touching the filesystem, and what makes a real load and a builtins-only load
//! the same code path with a different source.

use std::collections::HashMap;
use std::io;

/// Supplies the contents of a content file by its `assets/`-relative path.
///
/// Returning [`io::ErrorKind::NotFound`] is the normal "use the builtin" signal;
/// every other error is reported the same way but is worth distinguishing in the
/// log (a permission problem reads very differently from an absent file).
///
/// Bytes are the primitive and text is derived from them, rather than the other
/// way round: the definition files are TOML, but model files bring PNG textures
/// and binary vertex buffers along, and routing those through a `String` would
/// mangle them. Implementors supply [`AssetSource::read_bytes`]; the TOML
/// loaders keep calling [`AssetSource::read`] and are none the wiser.
pub trait AssetSource {
    fn read_bytes(&self, path: &str) -> io::Result<Vec<u8>>;

    /// The same file as UTF-8 text. Non-UTF-8 content reports `InvalidData`,
    /// which the fail-soft loaders treat like any other read failure.
    fn read(&self, path: &str) -> io::Result<String> {
        let bytes = self.read_bytes(path)?;
        String::from_utf8(bytes).map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))
    }
}

/// Reads from the working directory, exactly like `assets/` and `saves/`.
pub struct FsSource;

impl AssetSource for FsSource {
    fn read_bytes(&self, path: &str) -> io::Result<Vec<u8>> {
        std::fs::read(path)
    }

    fn read(&self, path: &str) -> io::Result<String> {
        std::fs::read_to_string(path)
    }
}

/// Supplies nothing, so every loader falls back to its embedded builtin copy.
/// This is what makes `GameContent::builtin()` a plain call to `from_source`.
pub struct EmbeddedSource;

impl AssetSource for EmbeddedSource {
    fn read_bytes(&self, _path: &str) -> io::Result<Vec<u8>> {
        Err(io::Error::new(
            io::ErrorKind::NotFound,
            "no content files (embedded builtins only)",
        ))
    }
}

/// In-memory fixtures for tests: files present in the map are served, anything
/// else falls back to the builtin.
#[derive(Default)]
pub struct MapSource(HashMap<String, Vec<u8>>);

impl MapSource {
    pub fn new() -> Self {
        Self::default()
    }

    /// Serve `text` for `path`.
    pub fn with(self, path: &str, text: impl Into<String>) -> Self {
        self.with_bytes(path, text.into().into_bytes())
    }

    /// Serve raw `bytes` for `path` — model files and their textures.
    pub fn with_bytes(mut self, path: &str, bytes: impl Into<Vec<u8>>) -> Self {
        self.0.insert(path.to_string(), bytes.into());
        self
    }
}

impl AssetSource for MapSource {
    fn read_bytes(&self, path: &str) -> io::Result<Vec<u8>> {
        self.0.get(path).cloned().ok_or_else(|| {
            io::Error::new(io::ErrorKind::NotFound, format!("no fixture for {path}"))
        })
    }
}

/// Read + parse one content file, falling back to the embedded builtin on any
/// problem. This is the single implementation of the fail-soft rule that used to
/// be copy-pasted once per definition file.
///
/// `label` names the content in the fallback message ("using builtin blocks");
/// `describe` renders the success message's subject ("loaded 42 blocks from ..."),
/// since some registries report a count and others have nothing to count.
///
/// `ctx` is threaded through *both* closures rather than captured, because a
/// loader may need the same `&mut` borrow on the parse and the fallback paths
/// alike — two closures can't each hold it. Loaders that need no context pass
/// `&mut ()`.
pub fn load_or_builtin<T, C: ?Sized>(
    source: &dyn AssetSource,
    path: &str,
    label: &str,
    ctx: &mut C,
    parse: impl FnOnce(&str, &mut C) -> Result<T, String>,
    builtin: impl FnOnce(&mut C) -> T,
    describe: impl FnOnce(&T) -> String,
) -> T {
    let text = match source.read(path) {
        Ok(text) => text,
        Err(err) => {
            log::info!("could not read {path} ({err}); using builtin {label}");
            return builtin(ctx);
        }
    };
    // Reborrow so `ctx` is still usable if parsing fails. A partially-applied
    // parse may already have mutated it (e.g. registered some tiles); the
    // builtin then runs over that same state, exactly as before this helper
    // existed.
    match parse(&text, &mut *ctx) {
        Ok(value) => {
            log::info!("loaded {} from {path}", describe(&value));
            value
        }
        Err(err) => {
            log::warn!("failed to parse {path}: {err}; using builtin {label}");
            builtin(ctx)
        }
    }
}

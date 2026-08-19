//! The public keys a host verifies join tickets with.
//!
//! Fetched from the auth server once and cached to `authkeys.toml` next to
//! `profile.toml`. The cache is the point: a host that has fetched the keys even
//! once can go on admitting legitimate players with no internet at all, and a
//! blip at the auth server never stops someone's game.
//!
//! Keys are public. Nothing here is a secret, and the file is safe to read,
//! copy, or commit — though it is gitignored anyway, since it is generated.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use wcauth_ticket::{KeySet, VerifyingKey};

/// Cache file, CWD-relative like `profile.toml`, `ops.toml` and `saves/`.
pub const KEYS_FILE: &str = "authkeys.toml";

/// One trusted key, as stored.
#[derive(Debug, Clone, Serialize, Deserialize)]
struct StoredKey {
    /// Which key id the ticket must name.
    id: u8,
    /// Raw 32-byte Ed25519 public key, base64 (standard alphabet, padded) —
    /// exactly what `GET /api/v1/keys` returns, so the two can be compared by
    /// eye when something is wrong.
    public_key: String,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct KeysToml {
    #[serde(default)]
    keys: Vec<StoredKey>,
}

/// Loads and saves the trusted key set.
pub struct KeyCache {
    path: PathBuf,
}

impl KeyCache {
    /// Cache at the default location.
    pub fn new() -> Self {
        Self {
            path: PathBuf::from(KEYS_FILE),
        }
    }

    /// Cache at an explicit path, for tests.
    pub fn at(path: impl Into<PathBuf>) -> Self {
        Self { path: path.into() }
    }

    /// Read the cached keys.
    ///
    /// Fail-soft in the safe direction: anything unreadable or malformed yields
    /// an *empty* set, and a host with an empty set refuses every join. The
    /// failure mode is "nobody can join", never "anybody can join".
    pub fn load(&self) -> KeySet {
        let Ok(text) = std::fs::read_to_string(&self.path) else {
            log::debug!(
                "no {}; this host cannot verify players yet",
                self.path.display()
            );
            return KeySet::new();
        };
        match toml::from_str::<KeysToml>(&text) {
            Ok(parsed) => Self::to_key_set(&parsed),
            Err(err) => {
                log::warn!("could not parse {}: {err}", self.path.display());
                KeySet::new()
            }
        }
    }

    /// Replace the cached keys.
    pub fn store(&self, keys: &[(u8, [u8; 32])]) -> Result<(), String> {
        let toml_value = KeysToml {
            keys: keys
                .iter()
                .map(|(id, key)| StoredKey {
                    id: *id,
                    public_key: base64_encode(key),
                })
                .collect(),
        };

        let text = toml::to_string_pretty(&toml_value)
            .map_err(|err| format!("could not serialize keys: {err}"))?;
        write_atomic(&self.path, text.as_bytes())
            .map_err(|err| format!("could not write {}: {err}", self.path.display()))
    }

    fn to_key_set(parsed: &KeysToml) -> KeySet {
        let mut set = KeySet::new();
        for stored in &parsed.keys {
            // One bad entry is skipped rather than discarding the file: during a
            // rotation a newer server may publish a key kind this build does not
            // understand, and the keys we *do* understand should keep working.
            let Some(bytes) = base64_decode(&stored.public_key) else {
                log::warn!("auth key {} is not valid base64; skipping", stored.id);
                continue;
            };
            let Ok(bytes) = <[u8; 32]>::try_from(bytes.as_slice()) else {
                log::warn!("auth key {} is not 32 bytes; skipping", stored.id);
                continue;
            };
            match VerifyingKey::from_bytes(&bytes) {
                Ok(key) => set.insert(stored.id, key),
                Err(err) => log::warn!("auth key {} is not a valid Ed25519 key: {err}", stored.id),
            }
        }
        set
    }
}

impl Default for KeyCache {
    fn default() -> Self {
        Self::new()
    }
}

/// Temp file + rename, so an interrupted write never leaves a half-parsed cache
/// — which, being fail-soft toward refusal, would lock everyone out.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let tmp = path.with_extension("tmp");
    std::fs::write(&tmp, bytes)?;
    std::fs::rename(&tmp, path)
}

/// Standard base64, padded.
pub(crate) fn base64_encode(bytes: &[u8]) -> String {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD.encode(bytes)
}

pub(crate) fn base64_decode(text: &str) -> Option<Vec<u8>> {
    use base64::Engine;
    base64::engine::general_purpose::STANDARD
        .decode(text.trim())
        .ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcauth_ticket::SigningKey;

    fn temp_path(name: &str) -> PathBuf {
        let mut path = std::env::temp_dir();
        path.push(format!(
            "wyven-authkeys-{name}-{}-{:?}.toml",
            std::process::id(),
            std::thread::current().id()
        ));
        path
    }

    fn public_key(seed: u8) -> [u8; 32] {
        SigningKey::from_bytes(&[seed; 32])
            .verifying_key()
            .to_bytes()
    }

    #[test]
    fn round_trips_a_key_set_through_the_file() {
        let path = temp_path("roundtrip");
        let cache = KeyCache::at(&path);

        cache
            .store(&[(0, public_key(1)), (7, public_key(2))])
            .unwrap();
        let loaded = cache.load();

        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded.get(0).map(|k| k.to_bytes()), Some(public_key(1)));
        assert_eq!(loaded.get(7).map(|k| k.to_bytes()), Some(public_key(2)));

        let _ = std::fs::remove_file(&path);
    }

    /// A missing cache must mean "refuse everyone", not "trust everyone".
    #[test]
    fn a_missing_file_yields_an_empty_set() {
        assert!(KeyCache::at(temp_path("absent")).load().is_empty());
    }

    #[test]
    fn a_malformed_file_yields_an_empty_set() {
        let path = temp_path("malformed");
        std::fs::write(&path, "this is not toml {{{").unwrap();

        assert!(KeyCache::at(&path).load().is_empty());

        let _ = std::fs::remove_file(&path);
    }

    /// A key this build cannot use must not discard the ones it can.
    #[test]
    fn one_unusable_entry_does_not_discard_the_rest() {
        let path = temp_path("partial");
        std::fs::write(
            &path,
            format!(
                "[[keys]]\nid = 0\npublic_key = \"{}\"\n\
                 [[keys]]\nid = 1\npublic_key = \"not base64!\"\n\
                 [[keys]]\nid = 2\npublic_key = \"c2hvcnQ=\"\n",
                base64_encode(&public_key(1))
            ),
        )
        .unwrap();

        let loaded = KeyCache::at(&path).load();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.get(0).is_some());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn storing_replaces_rather_than_appends() {
        let path = temp_path("replace");
        let cache = KeyCache::at(&path);

        cache.store(&[(0, public_key(1))]).unwrap();
        cache.store(&[(9, public_key(3))]).unwrap();

        let loaded = cache.load();
        assert_eq!(loaded.len(), 1);
        assert!(loaded.get(9).is_some());
        assert!(loaded.get(0).is_none());

        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn base64_round_trips() {
        let key = public_key(4);
        assert_eq!(base64_decode(&base64_encode(&key)).unwrap(), key.to_vec());
        assert_eq!(base64_decode("not base64!"), None);
    }
}

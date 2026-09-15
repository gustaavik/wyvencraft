//! `assets/audio.toml`: every playable clip in the game, by id.
//!
//! Structural validation only — see the module doc on why a bad *path* is
//! not checked here.

use crate::core::ident::is_valid_id;

/// Embedded copy of the shipped sound registry, used when
/// `assets/audio.toml` is missing or invalid.
pub const BUILTIN_AUDIO: &str = include_str!("../../assets/audio.toml");

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SoundCategory {
    Music,
    Sfx,
}

/// One registered sound: its id, where to read it from, and its default
/// playback volume.
#[derive(Debug, Clone)]
pub struct SoundDef {
    pub id: String,
    pub path: String,
    pub category: SoundCategory,
    pub volume: f32,
}

/// The validated sound registry.
#[derive(Debug, Default)]
pub struct SoundRegistry {
    sounds: Vec<SoundDef>,
}

impl SoundRegistry {
    /// The embedded registry. Infallible: validated by tests.
    pub fn builtin() -> Self {
        Self::from_toml(BUILTIN_AUDIO).expect("embedded audio.toml must parse")
    }

    /// Parse + strictly validate a sound registry file. Any structural
    /// fault — bad TOML, an invalid or duplicate id, an out-of-range volume
    /// — rejects the whole file; the caller falls back to `builtin()`.
    ///
    /// A sound's `path` is deliberately *not* checked against the
    /// filesystem here — see `AudioManager::new`, where that is resolved
    /// once, per entry, fail-soft.
    pub fn from_toml(text: &str) -> Result<Self, String> {
        let file: AudioFile = toml::from_str(text).map_err(|e| e.to_string())?;
        let mut sounds = Vec::with_capacity(file.sound.len());
        let mut seen = std::collections::HashSet::new();
        for def in file.sound {
            if !is_valid_id(&def.id) {
                return Err(format!("invalid sound id {:?}", def.id));
            }
            if !seen.insert(def.id.clone()) {
                return Err(format!("duplicate sound id {:?}", def.id));
            }
            if !(0.0..=1.0).contains(&def.volume) {
                return Err(format!(
                    "sound {:?}: volume {} is out of range 0.0..=1.0",
                    def.id, def.volume
                ));
            }
            let category = match def.category {
                CategoryDef::Music => SoundCategory::Music,
                CategoryDef::Sfx => SoundCategory::Sfx,
            };
            sounds.push(SoundDef {
                id: def.id,
                path: def.path,
                category,
                volume: def.volume,
            });
        }
        Ok(Self { sounds })
    }

    pub fn iter(&self) -> impl Iterator<Item = &SoundDef> {
        self.sounds.iter()
    }

    pub fn find(&self, id: &str) -> Option<&SoundDef> {
        self.sounds.iter().find(|s| s.id == id)
    }

    pub fn len(&self) -> usize {
        self.sounds.len()
    }

    pub fn is_empty(&self) -> bool {
        self.sounds.is_empty()
    }
}

// --- TOML schema layer (private; raw strings, strict fields) ---

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct AudioFile {
    #[serde(default, rename = "sound")]
    sound: Vec<SoundFileDef>,
}

#[derive(serde::Deserialize)]
#[serde(deny_unknown_fields)]
struct SoundFileDef {
    id: String,
    path: String,
    category: CategoryDef,
    volume: f32,
}

#[derive(serde::Deserialize)]
#[serde(rename_all = "lowercase")]
enum CategoryDef {
    Music,
    Sfx,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn builtin_audio_parses() {
        let registry = SoundRegistry::builtin();
        assert!(!registry.is_empty());
        assert!(registry.find("mainmenu_theme").is_some());
    }

    #[test]
    fn a_valid_single_entry_round_trips_its_fields() {
        let text = r#"
            [[sound]]
            id = "click"
            path = "assets/audio/click.wav"
            category = "sfx"
            volume = 0.6
        "#;
        let registry = SoundRegistry::from_toml(text).expect("valid file");
        let def = registry.find("click").expect("registered");
        assert_eq!(def.path, "assets/audio/click.wav");
        assert_eq!(def.category, SoundCategory::Sfx);
        assert_eq!(def.volume, 0.6);
    }

    #[test]
    fn an_invalid_id_rejects_the_whole_file() {
        let text = r#"
            [[sound]]
            id = "Click Sound"
            path = "assets/audio/click.wav"
            category = "sfx"
            volume = 0.6
        "#;
        assert!(SoundRegistry::from_toml(text).is_err());
    }

    #[test]
    fn a_duplicate_id_rejects_the_whole_file() {
        let text = r#"
            [[sound]]
            id = "click"
            path = "assets/audio/click.wav"
            category = "sfx"
            volume = 0.6

            [[sound]]
            id = "click"
            path = "assets/audio/click2.wav"
            category = "sfx"
            volume = 0.6
        "#;
        let err = SoundRegistry::from_toml(text).expect_err("must not parse");
        assert!(err.contains("click"), "{err}");
    }

    #[test]
    fn an_out_of_range_volume_rejects_the_whole_file() {
        let text = r#"
            [[sound]]
            id = "click"
            path = "assets/audio/click.wav"
            category = "sfx"
            volume = 1.5
        "#;
        let err = SoundRegistry::from_toml(text).expect_err("must not parse");
        assert!(err.contains("click"), "{err}");
    }
}

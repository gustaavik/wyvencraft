//! How content is named: a machine-readable `id` and the human label derived
//! from it.
//!
//! Every block and item carries an **id** — `wooden_pickaxe`, `oak_log` — and
//! that id is the only string the rest of the game keys on: save files, the
//! wire protocol, `place_block`, `drops`, recipe ingredients, worldgen and
//! `/give`. Restricting it to `[a-z0-9_]` is what makes it safe to type as a
//! single chat token and to embed in a file without quoting.
//!
//! What the *player* reads is a separate string, defaulting to
//! [`title_case`] of the id and overridable per entry with `display_name`.
//! Display names deliberately never reach `Block` or `Item`: those feed
//! `content::content_hash`, which gates multiplayer joins, and two peers whose
//! pickaxe is merely *labelled* differently have no reason to be refused a
//! shared world.

/// Whether `s` is a well-formed content id: non-empty, and ASCII lowercase
/// letters, digits and underscores only.
///
/// Deliberately strict rather than forgiving. An id is a reference key in five
/// different files, so silently accepting `"Wooden Pickaxe"` would mean the id
/// you wrote is not the id every other file has to spell.
pub fn is_valid_id(s: &str) -> bool {
    !s.is_empty()
        && s.bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'_')
}

/// The display name an id falls back to: underscores become spaces and each
/// word is capitalised, so `wooden_pickaxe` reads as `Wooden Pickaxe`.
///
/// This is only a default. An id the rule gets wrong (`tnt` would become
/// `Tnt`) spells its label out with `display_name` instead.
pub fn title_case(id: &str) -> String {
    let mut out = String::with_capacity(id.len());
    for (i, word) in id.split('_').filter(|w| !w.is_empty()).enumerate() {
        if i > 0 {
            out.push(' ');
        }
        let mut chars = word.chars();
        if let Some(first) = chars.next() {
            out.extend(first.to_uppercase());
            out.push_str(chars.as_str());
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_well_formed_ids() {
        for id in [
            "stone",
            "oak_log",
            "wooden_pickaxe",
            "water_flow_1",
            "tnt",
            "b2",
        ] {
            assert!(is_valid_id(id), "{id:?} should be valid");
        }
    }

    #[test]
    fn rejects_ids_that_are_not_single_lowercase_tokens() {
        for id in [
            "",                // empty
            "oak log",         // whitespace
            "Oak_Log",         // uppercase
            "oak-log",         // punctuation
            "oak.log",         // punctuation
            "oak\tlog",        // whitespace
            "café",            // non-ascii
            "wooden pickaxe ", // trailing space
        ] {
            assert!(!is_valid_id(id), "{id:?} should be rejected");
        }
    }

    #[test]
    fn leading_and_trailing_underscores_are_still_valid() {
        // Ugly, but unambiguous as a key — the rule is about what can be typed
        // and referenced, not about taste.
        assert!(is_valid_id("_stone"));
        assert!(is_valid_id("stone_"));
        assert!(is_valid_id("__"));
    }

    #[test]
    fn title_cases_each_word() {
        assert_eq!(title_case("stone"), "Stone");
        assert_eq!(title_case("oak_log"), "Oak Log");
        assert_eq!(title_case("wooden_pickaxe"), "Wooden Pickaxe");
        assert_eq!(title_case("water_flow_1"), "Water Flow 1");
    }

    #[test]
    fn title_case_skips_empty_words() {
        assert_eq!(title_case("_stone"), "Stone");
        assert_eq!(title_case("stone_"), "Stone");
        assert_eq!(title_case("oak__log"), "Oak Log");
        assert_eq!(title_case(""), "");
    }
}

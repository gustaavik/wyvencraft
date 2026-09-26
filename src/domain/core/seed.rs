//! World seeds: what text a player types becomes which number.

use std::time::{SystemTime, UNIX_EPOCH};

/// A time-derived seed (shared by every "random world" path).
pub fn random_seed() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

/// Interpret seed text from the UI: blank → random, a number (decimal or `0x`
/// hex) → itself, anything else → a stable hash of the string.
pub fn parse_seed(text: &str) -> u64 {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return random_seed();
    }
    if let Ok(n) = trimmed.parse::<u64>() {
        return n;
    }
    if let Some(hex) = trimmed
        .strip_prefix("0x")
        .or_else(|| trimmed.strip_prefix("0X"))
        && let Ok(n) = u64::from_str_radix(hex, 16)
    {
        return n;
    }
    use std::hash::{DefaultHasher, Hash, Hasher};
    let mut hasher = DefaultHasher::new();
    trimmed.hash(&mut hasher);
    hasher.finish()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_seed_accepts_numbers_hex_and_strings() {
        assert_eq!(parse_seed("42"), 42);
        assert_eq!(parse_seed(" 0xFF "), 255);
        // A u64 beyond i64::MAX must parse (the reason the seed is a string in TOML).
        assert_eq!(parse_seed("18446744073709551615"), u64::MAX);
        // Text seeds hash deterministically.
        assert_eq!(parse_seed("wyvern"), parse_seed("wyvern"));
        assert_ne!(parse_seed("wyvern"), parse_seed("dragon"));
    }
}

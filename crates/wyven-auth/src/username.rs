//! What the auth server will accept as a player name.
//!
//! Hand-mirrored from the server's `wcauth-domain::username`, for the same
//! reason and with the same care as [`AccountIdentity::netcode_id`]: the game
//! may depend on `wcauth-ticket` but not on the server's domain crate, so the
//! rule is spelled twice and the two must agree.
//!
//! The server stays the authority — nothing here decides whether a name is
//! *available*, only whether it is *well formed*. Knowing the rule locally is
//! what lets the login screen warn about a name as it is written, and refuse it
//! in place instead of spending a round trip to be told the same thing — and it
//! only ever refuses a name the server would refuse too.
//!
//! [`AccountIdentity::netcode_id`]: crate::AccountIdentity::netcode_id

/// Shortest allowed username.
pub const MIN_LEN: usize = 3;

/// Longest allowed username.
///
/// Bound by the join ticket, not by taste: a longer name could be registered
/// but never used to join a game, because it would not fit the ticket's
/// username field.
pub const MAX_LEN: usize = wcauth_ticket::MAX_USERNAME_LEN;

/// Why a name was refused.
///
/// The messages are the server's own wording, so a name refused here reads
/// exactly like one refused over the wire.
#[derive(Debug, Clone, Copy, PartialEq, Eq, thiserror::Error)]
pub enum UsernameError {
    #[error("username must be {MIN_LEN}-{MAX_LEN} characters")]
    Length,
    #[error("username may only contain letters, digits and underscores")]
    Charset,
    #[error("username must start with a letter or digit")]
    LeadingUnderscore,
}

/// Whether `c` may appear in a username. The one place the charset is spelled.
pub fn is_allowed(c: char) -> bool {
    c.is_ascii_alphanumeric() || c == '_'
}

/// Validate a name, returning it trimmed.
///
/// Length is counted in bytes, which is the ticket's unit; the charset is
/// ASCII-only, so bytes and characters agree.
pub fn validate(input: &str) -> Result<&str, UsernameError> {
    let name = input.trim();

    if name.len() < MIN_LEN || name.len() > MAX_LEN {
        return Err(UsernameError::Length);
    }
    if !name.chars().all(is_allowed) {
        return Err(UsernameError::Charset);
    }
    // A leading underscore reads as a system account and sorts oddly in every
    // player list ever built.
    if name.starts_with('_') {
        return Err(UsernameError::LeadingUnderscore);
    }

    Ok(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_ordinary_names() {
        for name in ["gus", "gustav", "Player_1", "0123456789abcdef"] {
            assert_eq!(validate(name), Ok(name), "{name} should be accepted");
        }
    }

    #[test]
    fn trims_surrounding_whitespace() {
        assert_eq!(validate("  gustav \n"), Ok("gustav"));
    }

    #[test]
    fn rejects_names_outside_the_length_bounds() {
        let too_long = "a".repeat(MAX_LEN + 1);
        for name in ["", "ab", too_long.as_str()] {
            assert_eq!(validate(name), Err(UsernameError::Length));
        }
    }

    #[test]
    fn rejects_characters_that_cannot_be_rendered_or_typed() {
        for name in ["gus tav", "gustav!", "günter", "drop-table", "🐉🐉🐉"] {
            assert_eq!(
                validate(name),
                Err(UsernameError::Charset),
                "{name} should be refused"
            );
        }
    }

    #[test]
    fn rejects_a_leading_underscore() {
        assert_eq!(validate("_admin"), Err(UsernameError::LeadingUnderscore));
    }

    /// The bound that matters: anything this accepts must be encodable into a
    /// join ticket, or the account is registerable but unusable.
    #[test]
    fn every_accepted_name_fits_a_join_ticket() {
        let longest = "a".repeat(MAX_LEN);
        assert_eq!(validate(&longest), Ok(longest.as_str()));
        assert!(longest.len() <= wcauth_ticket::MAX_USERNAME_LEN);
    }
}

//! `profile.toml`: this machine's offline identity and the signed-in account.
//!
//! Read by boot (to restore a session), by the network adapters (to present
//! an identity) and by the menu (to sign out) — which is why it is its own
//! adapter rather than a corner of the world-save module.

use std::fs;

use serde::{Deserialize, Serialize};

use crate::domain::core::seed::random_seed;
use crate::infrastructure::fs::write_atomic;
use crate::infrastructure::paths::PROFILE_FILE;

#[derive(Serialize, Deserialize)]
struct ProfileToml {
    /// Decimal string for the same u64-in-TOML reason as the world seed.
    client_id: String,
    /// The signed-in account, when there is one.
    ///
    /// `#[serde(default)]` so a `profile.toml` written before accounts existed
    /// still parses — it simply has no account and the player logs in.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    account: Option<AccountProfile>,
}

/// A signed-in session as it survives a restart, so a player is not asked for
/// their password every launch.
///
/// The refresh token is the sensitive part. It lives here for the same reason
/// every game launcher keeps one: the alternative is retyping a password on
/// every start. `profile.toml` is gitignored, and the token is single-use and
/// revocable — a stolen one stops working the moment the real client refreshes.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AccountProfile {
    /// Account uuid, as text.
    pub account_id: String,
    pub username: String,
    pub refresh_token: String,
}

/// The account this client last signed in as, if any.
pub fn stored_account() -> Option<AccountProfile> {
    let text = fs::read_to_string(crate::infrastructure::paths::profile_path()).ok()?;
    toml::from_str::<ProfileToml>(&text).ok()?.account
}

/// Remember (or forget, with `None`) the signed-in account.
///
/// Read-modify-write so the `client_id` already in the file is preserved: it is
/// the offline fallback identity, and regenerating it would orphan any
/// singleplayer save made before signing in.
pub fn store_account(account: Option<AccountProfile>) -> Result<(), String> {
    let path = crate::infrastructure::paths::profile_path();
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str::<ProfileToml>(&text).ok());

    let profile = ProfileToml {
        client_id: existing
            .map(|profile| profile.client_id)
            .unwrap_or_else(|| local_identity().to_string()),
        account,
    };

    let text = toml::to_string_pretty(&profile)
        .map_err(|err| format!("could not serialize profile: {err}"))?;
    write_atomic(&path, text.as_bytes())
        .map_err(|err| format!("could not write {PROFILE_FILE}: {err}"))
}

/// The machine-local identity, used only when nobody is signed in.
///
/// This is what `client_identity` always was: a random `u64` minted on first
/// launch. With accounts it is no longer how a *multiplayer* peer is
/// identified — that comes from the verified ticket — but singleplayer saves
/// still need some stable key, and an offline player has nothing better.
pub fn local_identity() -> u64 {
    client_identity()
}

/// The stable multiplayer identity this machine connects with (the netcode
/// client id). Persisted in `profile.toml` on first use so a host can recognise
/// a returning player and hand back their saved inventory/position.
/// `WYVEN_CLIENT_ID` overrides it (e.g. to run two clients from one directory).
///
/// Prefer [`wyven_auth::AccountState::netcode_id`] where an account may be
/// signed in: it derives the id from the account, so a save follows the player
/// rather than the machine.
pub fn client_identity() -> u64 {
    if let Ok(v) = std::env::var("WYVEN_CLIENT_ID")
        && let Ok(id) = v.trim().parse::<u64>()
    {
        return id;
    }
    let path = crate::infrastructure::paths::profile_path();
    let existing = fs::read_to_string(&path)
        .ok()
        .and_then(|text| toml::from_str::<ProfileToml>(&text).ok());
    if let Some(profile) = &existing
        && let Ok(id) = profile.client_id.trim().parse::<u64>()
    {
        return id;
    }

    let id = (random_seed() ^ (u64::from(std::process::id())).rotate_left(32)).max(1);
    let profile = ProfileToml {
        client_id: id.to_string(),
        // Carried over rather than dropped. This branch runs whenever the id is
        // missing or unreadable — including on a file a launcher wrote — and
        // discarding the account here would sign the player out for the sake of
        // regenerating a number they never see.
        account: existing.and_then(|profile| profile.account),
    };
    match toml::to_string_pretty(&profile) {
        Ok(text) => {
            if let Err(err) = write_atomic(&path, text.as_bytes()) {
                log::warn!("could not persist profile.toml: {err}");
            } else {
                log::info!("created player profile (client id {id})");
            }
        }
        Err(err) => log::warn!("could not serialize profile: {err}"),
    }
    id
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(tag: &str) -> std::path::PathBuf {
        let root = std::env::temp_dir().join(format!("wyven-profile-{tag}-{}", std::process::id()));
        let _ = fs::remove_dir_all(&root);
        root
    }

    /// Regenerating the client id must not sign the player out.
    ///
    /// A launcher writes `profile.toml` to hand the game a session, and it may
    /// not put a usable `client_id` in it. That sends `client_identity` down
    /// its minting branch, which used to rewrite the file with no `[account]`
    /// table at all — destroying the session it was handed.
    #[test]
    fn regenerating_the_client_id_keeps_the_stored_account() {
        let dir = temp_root("client-id-preserves-account");
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("profile.toml");

        // What a launcher might write: an account, and no usable id.
        std::fs::write(
            &path,
            "client_id = \"\"\n\n[account]\n\
             account_id = \"67757374-6176-0000-0000-000000000000\"\n\
             username = \"gustav\"\n\
             refresh_token = \"rt\"\n",
        )
        .unwrap();

        let text = std::fs::read_to_string(&path).unwrap();
        let before: ProfileToml = toml::from_str(&text).unwrap();
        assert!(before.client_id.trim().parse::<u64>().is_err());
        let account = before.account.expect("the fixture has an account");

        // Mirrors the minting branch of `client_identity`, which cannot be
        // called directly here: it resolves its path through the process-wide
        // data dir.
        let regenerated = ProfileToml {
            client_id: "1787389778214353360".to_owned(),
            account: Some(account),
        };
        std::fs::write(&path, toml::to_string_pretty(&regenerated).unwrap()).unwrap();

        let after: ProfileToml = toml::from_str(&std::fs::read_to_string(&path).unwrap()).unwrap();
        assert!(after.client_id.trim().parse::<u64>().is_ok());
        let kept = after.account.expect("the account must survive");
        assert_eq!(kept.username, "gustav");
        assert_eq!(kept.refresh_token, "rt");

        std::fs::remove_dir_all(&dir).ok();
    }
}

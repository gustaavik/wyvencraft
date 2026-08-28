//! Talking to the auth server.
//!
//! A **port** in the same sense as `content::ContentSource` and
//! `save::WorldRepository`: [`AuthClient`] is the trait, [`HttpAuthClient`] is
//! the real one, [`FakeAuthClient`] is the test double. That is what lets the
//! login screen and the whole boot path be tested with no server running.
//!
//! Blocking, not async. The game has no runtime and one thread that must not
//! stall, so calls happen from a worker thread and the UI polls the result —
//! see the game's `boot::start::boot_account`.

use std::collections::HashMap;
use std::sync::Mutex;
use std::time::Duration;

use wcauth_ticket::SLOT_LEN;

use crate::keys::base64_decode;
use crate::session::{AccountIdentity, AuthSession};

/// How long any single request may take.
///
/// Short: every one of these sits between a player and the thing they clicked.
const TIMEOUT: Duration = Duration::from_secs(10);

/// Why a call to the auth server failed.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AuthError {
    /// The server said no, with a message meant for the player.
    #[error("{message}")]
    Refused { code: String, message: String },
    /// The server could not be reached at all.
    ///
    /// Distinct from [`Refused`](Self::Refused) because the two call for
    /// completely different UI: one means "fix your password", the other means
    /// "you can still play offline".
    #[error("could not reach the account server: {0}")]
    Unreachable(String),
    /// A reply we could not make sense of.
    #[error("the account server sent something unexpected: {0}")]
    Malformed(String),
}

impl AuthError {
    /// Whether this failure means the server is unavailable rather than
    /// unhappy — the signal the login screen uses to offer offline play.
    pub fn is_offline(&self) -> bool {
        matches!(self, Self::Unreachable(_))
    }
}

/// A freshly issued join ticket.
#[derive(Debug, Clone)]
pub struct JoinTicket {
    /// The padded 256-byte netcode `user_data` slot.
    pub slot: [u8; SLOT_LEN],
    /// Unix seconds.
    pub expires_at: u64,
}

/// One trusted verification key.
pub type PublicKey = (u8, [u8; 32]);

/// Everything the game needs from the auth server.
pub trait AuthClient: Send + Sync {
    fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthSession, AuthError>;
    fn login(&self, username: &str, password: &str) -> Result<AuthSession, AuthError>;
    /// Exchange a refresh token for a new session. The old token is spent.
    fn refresh(&self, refresh_token: &str) -> Result<AuthSession, AuthError>;
    /// Mint a join ticket. Called at join time, not at login time — a ticket
    /// lives about two minutes.
    fn issue_ticket(&self, access_token: &str) -> Result<JoinTicket, AuthError>;
    /// The keys a host verifies tickets with. Unauthenticated.
    fn public_keys(&self) -> Result<Vec<PublicKey>, AuthError>;
}

// ------------------------------------------------------------------ HTTP

/// [`AuthClient`] over HTTP.
pub struct HttpAuthClient {
    base_url: String,
    agent: ureq::Agent,
}

impl HttpAuthClient {
    pub fn new(base_url: impl Into<String>) -> Self {
        let config = ureq::Agent::config_builder()
            .timeout_global(Some(TIMEOUT))
            // A refusal is a JSON envelope carrying the one thing the player
            // needs to read ("that username is already taken"). Left at ureq's
            // default, a 4xx becomes a transport-level error with the body
            // thrown away, and every refusal collapses into "the account server
            // refused the request (422)". Statuses are read in
            // [`parse_envelope`] instead, alongside the body.
            .http_status_as_error(false)
            .build();

        Self {
            base_url: base_url.into().trim_end_matches('/').to_string(),
            agent: config.into(),
        }
    }

    /// Point at whatever `WYVEN_AUTH_URL` says, or the default.
    pub fn from_env() -> Self {
        Self::new(crate::auth_url())
    }

    fn url(&self, path: &str) -> String {
        format!("{}{path}", self.base_url)
    }

    /// POST some JSON and pull the `data` object out of the envelope.
    fn post(&self, path: &str, body: serde_json::Value) -> Result<serde_json::Value, AuthError> {
        let response = self
            .agent
            .post(&self.url(path))
            .send_json(&body)
            .map_err(classify)?;
        Self::unwrap_envelope(response)
    }

    fn post_authed(&self, path: &str, token: &str) -> Result<serde_json::Value, AuthError> {
        let response = self
            .agent
            .post(&self.url(path))
            .header("Authorization", &format!("Bearer {token}"))
            .send_empty()
            .map_err(classify)?;
        Self::unwrap_envelope(response)
    }

    fn get(&self, path: &str) -> Result<serde_json::Value, AuthError> {
        let response = self.agent.get(&self.url(path)).call().map_err(classify)?;
        Self::unwrap_envelope(response)
    }

    /// Read the reply and hand it to [`parse_envelope`].
    fn unwrap_envelope(
        mut response: ureq::http::Response<ureq::Body>,
    ) -> Result<serde_json::Value, AuthError> {
        let status = response.status().as_u16();
        let text = response
            .body_mut()
            .read_to_string()
            .map_err(|err| AuthError::Malformed(format!("could not read the reply: {err}")))?;

        parse_envelope(status, &text)
    }

    /// Read the session shape the auth endpoints all return.
    fn parse_session(data: &serde_json::Value) -> Result<AuthSession, AuthError> {
        let account = data
            .get("account")
            .ok_or_else(|| AuthError::Malformed("reply had no account".to_string()))?;

        let account_id = field(account, "id")
            .and_then(|id| uuid::Uuid::parse_str(&id).ok())
            .ok_or_else(|| AuthError::Malformed("account id was not a uuid".to_string()))?;
        let username = field(account, "username")
            .ok_or_else(|| AuthError::Malformed("account had no username".to_string()))?;

        Ok(AuthSession {
            identity: AccountIdentity {
                account_id,
                username,
            },
            access_token: field(data, "access_token")
                .ok_or_else(|| AuthError::Malformed("reply had no access token".to_string()))?,
            access_expires_at: field(data, "access_expires_at")
                .and_then(|value| parse_rfc3339_unix(&value))
                .ok_or_else(|| AuthError::Malformed("access expiry was unreadable".to_string()))?,
            refresh_token: field(data, "refresh_token")
                .ok_or_else(|| AuthError::Malformed("reply had no refresh token".to_string()))?,
        })
    }
}

impl AuthClient for HttpAuthClient {
    fn register(
        &self,
        username: &str,
        email: &str,
        password: &str,
    ) -> Result<AuthSession, AuthError> {
        let data = self.post(
            "/api/v1/auth/register",
            serde_json::json!({ "username": username, "email": email, "password": password }),
        )?;
        Self::parse_session(&data)
    }

    fn login(&self, username: &str, password: &str) -> Result<AuthSession, AuthError> {
        let data = self.post(
            "/api/v1/auth/login",
            serde_json::json!({ "username": username, "password": password }),
        )?;
        Self::parse_session(&data)
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthSession, AuthError> {
        let data = self.post(
            "/api/v1/auth/refresh",
            serde_json::json!({ "refresh_token": refresh_token }),
        )?;
        Self::parse_session(&data)
    }

    fn issue_ticket(&self, access_token: &str) -> Result<JoinTicket, AuthError> {
        let data = self.post_authed("/api/v1/sessions/ticket", access_token)?;

        let encoded = field(&data, "ticket")
            .ok_or_else(|| AuthError::Malformed("reply had no ticket".to_string()))?;
        let bytes = base64_decode(&encoded)
            .ok_or_else(|| AuthError::Malformed("ticket was not base64".to_string()))?;
        let slot: [u8; SLOT_LEN] = bytes.as_slice().try_into().map_err(|_| {
            AuthError::Malformed(format!(
                "ticket was {} bytes, expected {SLOT_LEN}",
                bytes.len()
            ))
        })?;

        Ok(JoinTicket {
            slot,
            expires_at: data
                .get("expires_at")
                .and_then(serde_json::Value::as_u64)
                .ok_or_else(|| AuthError::Malformed("ticket had no expiry".to_string()))?,
        })
    }

    fn public_keys(&self) -> Result<Vec<PublicKey>, AuthError> {
        let data = self.get("/api/v1/keys")?;
        let entries = data
            .get("keys")
            .and_then(serde_json::Value::as_array)
            .ok_or_else(|| AuthError::Malformed("reply had no keys".to_string()))?;

        // Unusable entries are skipped rather than failing the fetch: a newer
        // server may publish a key type this build does not know, and the keys
        // it does know should still be cached.
        Ok(entries
            .iter()
            .filter_map(|entry| {
                let id = u8::try_from(entry.get("id")?.as_u64()?).ok()?;
                let bytes = base64_decode(&field(entry, "public_key")?)?;
                Some((id, <[u8; 32]>::try_from(bytes.as_slice()).ok()?))
            })
            .collect())
    }
}

/// Turn `{"status":...}` into either the payload or a typed error.
///
/// Takes the status and the body as plain values rather than a response, so the
/// whole mapping — the part a player actually reads — is testable with no
/// server and no socket.
fn parse_envelope(status: u16, text: &str) -> Result<serde_json::Value, AuthError> {
    let value: serde_json::Value = match serde_json::from_str(text) {
        Ok(value) => value,
        Err(err) if status < 400 => {
            return Err(AuthError::Malformed(format!("reply was not JSON: {err}")));
        }
        // Not our envelope at all: something between the game and the server
        // answered (proxy, gateway). Say it by status rather than quoting its
        // markup at the player.
        Err(_) => return Err(status_refusal(status)),
    };

    match value.get("status").and_then(|s| s.as_str()) {
        Some("ok") if status < 400 => Ok(value
            .get("data")
            .cloned()
            .unwrap_or(serde_json::Value::Null)),
        // The server's own message is what the player sees: it was written to
        // describe *their* input ("that is not a valid email address",
        // "username must be 3-16 characters"), which no status code can.
        Some("error") => Err(AuthError::Refused {
            code: field(&value, "code").unwrap_or_else(|| "unknown".to_string()),
            message: field(&value, "message").unwrap_or_else(|| status_message(status)),
        }),
        // A body that disagrees with its own status: trust the status.
        _ if status >= 400 => Err(status_refusal(status)),
        _ => Err(AuthError::Malformed(
            "reply had no status field".to_string(),
        )),
    }
}

/// The fallback for a refusal that carried no message of its own.
fn status_refusal(code: u16) -> AuthError {
    AuthError::Refused {
        code: format!("http_{code}"),
        message: status_message(code),
    }
}

fn status_message(code: u16) -> String {
    match code {
        401 => "invalid username or password".to_string(),
        429 => "too many attempts; try again shortly".to_string(),
        500..=599 => "the account server is having trouble".to_string(),
        _ => format!("the account server refused the request ({code})"),
    }
}

/// Distinguish "the server said no" from "there was no server".
fn classify(err: ureq::Error) -> AuthError {
    match err {
        // The agent is configured not to raise statuses as errors, so a status
        // normally arrives as a response and is read by `parse_envelope`. This
        // arm only covers the ones ureq raises itself (a redirect it will not
        // follow) — still a refusal, not a transport failure.
        ureq::Error::StatusCode(code) => status_refusal(code),
        other => AuthError::Unreachable(other.to_string()),
    }
}

fn field(value: &serde_json::Value, key: &str) -> Option<String> {
    value.get(key)?.as_str().map(str::to_string)
}

/// Parse the RFC 3339 timestamps the auth server sends into unix seconds.
///
/// Hand-rolled because the game has no date library and this is the only date it
/// ever reads. The format is fixed by the server —
/// `YYYY-MM-DDTHH:MM:SS[.fff]Z` — so only that shape is accepted; anything else
/// is reported as unreadable rather than guessed at.
fn parse_rfc3339_unix(text: &str) -> Option<u64> {
    let text = text.trim();
    let (date, rest) = text.split_once('T')?;
    let time = rest
        .split_once('.')
        .map(|(head, _)| head)
        .unwrap_or_else(|| rest.trim_end_matches('Z'));
    let time = time.trim_end_matches('Z');

    let mut date_parts = date.split('-');
    let year: i64 = date_parts.next()?.parse().ok()?;
    let month: i64 = date_parts.next()?.parse().ok()?;
    let day: i64 = date_parts.next()?.parse().ok()?;

    let mut time_parts = time.split(':');
    let hour: i64 = time_parts.next()?.parse().ok()?;
    let minute: i64 = time_parts.next()?.parse().ok()?;
    let second: i64 = time_parts.next()?.parse().ok()?;

    if !(1..=12).contains(&month) || !(1..=31).contains(&day) {
        return None;
    }

    // Howard Hinnant's days_from_civil: exact, branch-free, and valid for any
    // proleptic Gregorian date — no lookup tables or leap-year special cases.
    let year = year - i64::from(month <= 2);
    let era = if year >= 0 { year } else { year - 399 } / 400;
    let year_of_era = year - era * 400;
    let day_of_year = (153 * (month + if month > 2 { -3 } else { 9 }) + 2) / 5 + day - 1;
    let day_of_era = year_of_era * 365 + year_of_era / 4 - year_of_era / 100 + day_of_year;
    let days = era * 146_097 + day_of_era - 719_468;

    u64::try_from(days * 86_400 + hour * 3_600 + minute * 60 + second).ok()
}

// ------------------------------------------------------------------ fake

/// Scriptable [`AuthClient`] for tests.
///
/// A working implementation rather than a mock: it holds accounts in a map and
/// The instant tickets from this double claim to have been issued at, unless
/// [`FakeAuthClient::issuing_at`] moves it. A fixed point, so a test that
/// verifies at a fixed `now` keeps passing next year.
const DEFAULT_FAKE_ISSUE_TIME: u64 = 1_800_000_000;

/// really issues sessions, so a test exercises the same control flow the HTTP
/// client drives.
pub struct FakeAuthClient {
    /// username -> password
    accounts: Mutex<HashMap<String, String>>,
    /// When set, every call fails this way — for testing the offline path.
    failure: Mutex<Option<AuthError>>,
    /// Signing key for the tickets this double issues, so a test can verify
    /// them with the matching public key.
    signing_key: wcauth_ticket::SigningKey,
    key_id: u8,
    issued: Mutex<u64>,
    /// The moment tickets are stamped as issued at.
    ///
    /// Fixed by default, so a test can verify against a fixed `now` and get the
    /// same answer on any day. A test standing this double next to a *real*
    /// host — which reads the wall clock — has to move it: a ticket issued in
    /// 2027 is refused today as "not valid yet".
    issued_at: u64,
}

impl FakeAuthClient {
    pub fn new() -> Self {
        Self {
            accounts: Mutex::new(HashMap::new()),
            failure: Mutex::new(None),
            signing_key: wcauth_ticket::SigningKey::from_bytes(&[42; 32]),
            key_id: 1,
            issued: Mutex::new(0),
            issued_at: DEFAULT_FAKE_ISSUE_TIME,
        }
    }

    /// Stamp tickets as issued at `now_unix` rather than at the fixed default.
    pub fn issuing_at(mut self, now_unix: u64) -> Self {
        self.issued_at = now_unix;
        self
    }

    /// Pre-register an account.
    pub fn with_account(self, username: &str, password: &str) -> Self {
        self.accounts
            .lock()
            .expect("fake auth accounts")
            .insert(username.to_lowercase(), password.to_string());
        self
    }

    /// Make every subsequent call fail as if the server were unreachable.
    pub fn offline(self) -> Self {
        *self.failure.lock().expect("fake auth failure") =
            Some(AuthError::Unreachable("no route to host".to_string()));
        self
    }

    /// The key set a host would verify this double's tickets with.
    pub fn key_set(&self) -> wcauth_ticket::KeySet {
        wcauth_ticket::KeySet::new().with(self.key_id, self.signing_key.verifying_key())
    }

    fn check_online(&self) -> Result<(), AuthError> {
        match self.failure.lock().expect("fake auth failure").clone() {
            Some(err) => Err(err),
            None => Ok(()),
        }
    }

    fn session_for(username: &str) -> AuthSession {
        // Deterministic id per username, so a test can assert on it.
        let mut bytes = [0_u8; 16];
        for (slot, byte) in bytes.iter_mut().zip(username.bytes()) {
            *slot = byte;
        }
        AuthSession {
            identity: AccountIdentity {
                account_id: uuid::Uuid::from_bytes(bytes),
                username: username.to_string(),
            },
            access_token: format!("access-for-{username}"),
            access_expires_at: u64::MAX / 2,
            refresh_token: format!("refresh-for-{username}"),
        }
    }
}

impl Default for FakeAuthClient {
    fn default() -> Self {
        Self::new()
    }
}

impl AuthClient for FakeAuthClient {
    fn register(
        &self,
        username: &str,
        _email: &str,
        password: &str,
    ) -> Result<AuthSession, AuthError> {
        self.check_online()?;
        let mut accounts = self.accounts.lock().expect("fake auth accounts");
        if accounts.contains_key(&username.to_lowercase()) {
            return Err(AuthError::Refused {
                code: "username_taken".to_string(),
                message: "that username is already taken".to_string(),
            });
        }
        accounts.insert(username.to_lowercase(), password.to_string());
        Ok(Self::session_for(username))
    }

    fn login(&self, username: &str, password: &str) -> Result<AuthSession, AuthError> {
        self.check_online()?;
        let accounts = self.accounts.lock().expect("fake auth accounts");
        match accounts.get(&username.to_lowercase()) {
            Some(stored) if stored == password => Ok(Self::session_for(username)),
            _ => Err(AuthError::Refused {
                code: "invalid_credentials".to_string(),
                message: "invalid username or password".to_string(),
            }),
        }
    }

    fn refresh(&self, refresh_token: &str) -> Result<AuthSession, AuthError> {
        self.check_online()?;
        let username =
            refresh_token
                .strip_prefix("refresh-for-")
                .ok_or_else(|| AuthError::Refused {
                    code: "session_invalid".to_string(),
                    message: "please sign in again".to_string(),
                })?;
        Ok(Self::session_for(username))
    }

    fn issue_ticket(&self, access_token: &str) -> Result<JoinTicket, AuthError> {
        self.check_online()?;
        let username =
            access_token
                .strip_prefix("access-for-")
                .ok_or_else(|| AuthError::Refused {
                    code: "invalid_credentials".to_string(),
                    message: "invalid username or password".to_string(),
                })?;

        let session = Self::session_for(username);
        let now = self.issued_at;

        // A fresh nonce per ticket, or a host's replay cache would reject the
        // second join in any test that makes two.
        let mut issued = self.issued.lock().expect("fake auth counter");
        *issued += 1;
        let mut nonce = [0_u8; 16];
        nonce[..8].copy_from_slice(&issued.to_le_bytes());

        let slot = wcauth_ticket::sign(
            &wcauth_ticket::TicketClaims {
                account_id: session.identity.account_id,
                username: session.identity.username.clone(),
                issued_at: now,
                expires_at: now + wcauth_ticket::DEFAULT_TTL_SECS,
                nonce,
            },
            self.key_id,
            &self.signing_key,
        )
        .map_err(|err| AuthError::Malformed(err.to_string()))?;

        Ok(JoinTicket {
            slot,
            expires_at: now + wcauth_ticket::DEFAULT_TTL_SECS,
        })
    }

    fn public_keys(&self) -> Result<Vec<PublicKey>, AuthError> {
        self.check_online()?;
        Ok(vec![(
            self.key_id,
            self.signing_key.verifying_key().to_bytes(),
        )])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fake_signs_in_a_known_account() {
        let client = FakeAuthClient::new().with_account("gustav", "hunter2hunter2");
        let session = client.login("gustav", "hunter2hunter2").unwrap();

        assert_eq!(session.identity.username, "gustav");
    }

    #[test]
    fn the_fake_refuses_a_wrong_password() {
        let client = FakeAuthClient::new().with_account("gustav", "hunter2hunter2");

        assert!(matches!(
            client.login("gustav", "wrong"),
            Err(AuthError::Refused { .. })
        ));
    }

    /// The distinction the login screen branches on.
    #[test]
    fn an_offline_server_is_distinguishable_from_a_refusal() {
        let offline = FakeAuthClient::new().with_account("gustav", "pw").offline();
        let err = offline.login("gustav", "pw").unwrap_err();

        assert!(err.is_offline());
        assert!(
            !AuthError::Refused {
                code: "invalid_credentials".to_string(),
                message: "no".to_string()
            }
            .is_offline()
        );
    }

    /// The end-to-end contract, exercised without a server: a ticket the double
    /// issues verifies against the key set it publishes.
    #[test]
    fn a_ticket_from_the_fake_verifies_against_its_published_key() {
        let client = FakeAuthClient::new().with_account("gustav", "hunter2hunter2");
        let session = client.login("gustav", "hunter2hunter2").unwrap();
        let ticket = client.issue_ticket(&session.access_token).unwrap();

        let mut verifier = crate::TicketVerifier::new(client.key_set());
        let identity = verifier.verify(Some(&ticket.slot), 1_800_000_000).unwrap();

        assert_eq!(identity.username, "gustav");
        assert_eq!(identity.account_id, session.identity.account_id);
    }

    #[test]
    fn each_fake_ticket_gets_a_fresh_nonce() {
        let client = FakeAuthClient::new().with_account("gustav", "pw");
        let session = client.login("gustav", "pw").unwrap();

        let first = client.issue_ticket(&session.access_token).unwrap();
        let second = client.issue_ticket(&session.access_token).unwrap();

        let mut verifier = crate::TicketVerifier::new(client.key_set());
        assert!(verifier.verify(Some(&first.slot), 1_800_000_000).is_ok());
        assert!(
            verifier.verify(Some(&second.slot), 1_800_000_000).is_ok(),
            "a second join must not trip the replay cache"
        );
    }

    // --- envelope --------------------------------------------------------

    /// The bug this guards: a refusal used to be classified from its status
    /// before the body was ever read, so "that is not a valid email address"
    /// reached the player as "the account server refused the request (422)".
    #[test]
    fn a_refusal_reports_the_servers_own_message() {
        let err = parse_envelope(
            422,
            r#"{"status":"error","code":"validation_failed","message":"that is not a valid email address"}"#,
        )
        .unwrap_err();

        assert_eq!(
            err,
            AuthError::Refused {
                code: "validation_failed".to_string(),
                message: "that is not a valid email address".to_string(),
            }
        );
        assert_eq!(err.to_string(), "that is not a valid email address");
    }

    #[test]
    fn a_taken_username_reads_as_a_taken_username() {
        let err = parse_envelope(
            409,
            r#"{"status":"error","code":"username_taken","message":"that username is already taken"}"#,
        )
        .unwrap_err();

        assert_eq!(err.to_string(), "that username is already taken");
        assert!(!err.is_offline());
    }

    #[test]
    fn a_success_envelope_yields_its_payload() {
        let data = parse_envelope(200, r#"{"status":"ok","data":{"access_token":"abc"}}"#).unwrap();

        assert_eq!(field(&data, "access_token").as_deref(), Some("abc"));
    }

    /// A gateway between the game and the server answers with markup, not our
    /// envelope. The player gets a sentence, not HTML.
    #[test]
    fn a_reply_that_is_not_our_envelope_falls_back_to_the_status() {
        assert_eq!(
            parse_envelope(502, "<html>Bad Gateway</html>").unwrap_err(),
            AuthError::Refused {
                code: "http_502".to_string(),
                message: "the account server is having trouble".to_string(),
            }
        );
        // Even valid JSON that is not an envelope.
        assert_eq!(
            parse_envelope(401, r#"{"detail":"nope"}"#).unwrap_err(),
            AuthError::Refused {
                code: "http_401".to_string(),
                message: "invalid username or password".to_string(),
            }
        );
    }

    /// Nothing is wrong with the *server*, so a broken 2xx stays malformed —
    /// that is the variant that means "this build cannot read the reply".
    #[test]
    fn a_broken_success_reply_is_malformed_not_refused() {
        assert!(matches!(
            parse_envelope(200, "<html>"),
            Err(AuthError::Malformed(_))
        ));
        assert!(matches!(
            parse_envelope(200, r#"{"data":{}}"#),
            Err(AuthError::Malformed(_))
        ));
    }

    // --- RFC 3339 parsing ------------------------------------------------

    #[test]
    fn parses_the_timestamps_the_auth_server_sends() {
        assert_eq!(parse_rfc3339_unix("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(
            parse_rfc3339_unix("2001-09-09T01:46:40Z"),
            Some(1_000_000_000)
        );
        assert_eq!(
            parse_rfc3339_unix("2026-08-18T11:32:04Z"),
            Some(1_787_052_724)
        );
    }

    #[test]
    fn tolerates_fractional_seconds() {
        // The server emits these; they must not shift the result.
        assert_eq!(
            parse_rfc3339_unix("2026-08-18T11:32:04.456435Z"),
            Some(1_787_052_724)
        );
    }

    /// Leap years are where a hand-rolled date conversion usually goes wrong.
    #[test]
    fn handles_leap_days_and_century_rules() {
        assert_eq!(
            parse_rfc3339_unix("2024-02-29T00:00:00Z"),
            Some(1_709_164_800)
        );
        // 2000 was a leap year; 1900 was not. The 400-year rule.
        assert_eq!(
            parse_rfc3339_unix("2000-03-01T00:00:00Z"),
            Some(951_868_800)
        );
        assert_eq!(
            parse_rfc3339_unix("2100-03-01T00:00:00Z"),
            Some(4_107_542_400)
        );
    }

    #[test]
    fn rejects_what_it_cannot_read_rather_than_guessing() {
        for text in [
            "",
            "not a date",
            "2026-08-18",
            "2026-08-18 11:32:04",
            "2026-13-01T00:00:00Z",
            "2026-08-32T00:00:00Z",
            "1900-01-01T00:00:00Z", // before the epoch: not representable
        ] {
            assert_eq!(parse_rfc3339_unix(text), None, "{text:?} should not parse");
        }
    }
}

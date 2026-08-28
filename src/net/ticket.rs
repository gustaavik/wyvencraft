//! Getting a join ticket, and not losing the account while doing it.
//!
//! [`issue`] wraps [`AccountState::issue_ticket`] with the one thing every
//! caller has to remember: issuing may have *rotated* the refresh token, and
//! that rotation is single-use and destructive. The old token is already dead
//! server-side, so a new one that only ever lived in memory signs the player
//! out on the next launch with nothing to show for it.
//!
//! It matters more since the server browser exists: joining happens a few times
//! a session, but a Refresh asks for a ticket every time a player looks at
//! their list.

use wyven_auth::{AccountState, AuthClient, AuthError, JoinTicket};

use crate::save::{self, AccountProfile};

/// Issue a join ticket, persisting the session if the request rotated it.
///
/// **Blocks** on the auth server. Call it from a worker.
pub fn issue(
    account: &AccountState,
    auth: &dyn AuthClient,
    now_unix: u64,
) -> Result<JoinTicket, AuthError> {
    let before = account.session().map(|session| session.refresh_token);
    let result = account.issue_ticket(auth, now_unix);

    // Checked whether or not the ticket itself succeeded: the rotation happens
    // first, so a ticket request that fails *after* refreshing still leaves a
    // new token that has to be kept.
    if let Some(session) = account.session()
        && before.as_deref() != Some(session.refresh_token.as_str())
        && let Err(err) = save::store_account(Some(AccountProfile {
            account_id: session.identity.account_id.to_string(),
            username: session.identity.username.clone(),
            refresh_token: session.refresh_token.clone(),
        }))
    {
        log::warn!("could not persist the rotated session: {err}");
    }

    result
}

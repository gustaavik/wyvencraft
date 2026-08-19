//! Host-side ticket checking.
//!
//! A joining player puts a signed ticket in netcode's 256-byte `user_data`. This
//! is what a host does with it: check the signature against a cached public key,
//! check it has not been seen before, and turn it into an [`AccountIdentity`]
//! nothing else in the game can forge.
//!
//! Verification is entirely local. The auth server is not contacted, which is
//! what stops a stranger's session from depending on our uptime.

use std::collections::HashMap;

use wcauth_ticket::{KeySet, Nonce, SLOT_LEN, TicketError};

use crate::auth::session::AccountIdentity;

/// Why a joining player was turned away.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VerifyFailure {
    /// No ticket at all — an old client, or someone connecting with a hand-built
    /// packet.
    #[error("no join ticket was presented")]
    Missing,
    /// The ticket did not check out.
    #[error("join ticket rejected: {0}")]
    Invalid(#[from] TicketError),
    /// A ticket with this nonce has already been accepted.
    #[error("join ticket has already been used")]
    Replayed,
    /// The host has no public keys, so it cannot check anything.
    ///
    /// Fails closed: with no keys, every ticket is refused rather than waved
    /// through. A host that cannot verify must not pretend it did.
    #[error("this host has no auth keys and cannot verify players")]
    NoKeys,
}

/// Checks tickets and remembers the ones it has accepted.
pub struct TicketVerifier {
    keys: KeySet,
    /// Nonce -> the ticket's expiry, so entries can be dropped once no ticket
    /// bearing them could still be valid.
    seen: HashMap<Nonce, u64>,
}

impl TicketVerifier {
    pub fn new(keys: KeySet) -> Self {
        Self {
            keys,
            seen: HashMap::new(),
        }
    }

    /// How many keys this host trusts.
    pub fn key_count(&self) -> usize {
        self.keys.len()
    }

    /// Check a ticket and consume its nonce.
    ///
    /// `now_unix` is passed in rather than read from the clock so this stays a
    /// pure function — the whole join path is then testable without sleeping.
    pub fn verify(
        &mut self,
        user_data: Option<&[u8; SLOT_LEN]>,
        now_unix: u64,
    ) -> Result<AccountIdentity, VerifyFailure> {
        if self.keys.is_empty() {
            return Err(VerifyFailure::NoKeys);
        }
        let slot = user_data.ok_or(VerifyFailure::Missing)?;

        let ticket = wcauth_ticket::verify(slot, &self.keys, now_unix)?;

        // Expired entries are cleared before the lookup, so a long-running host
        // does not accumulate a nonce for every player who ever joined.
        self.forget_expired(now_unix);

        // A ticket is a bearer credential for its whole lifetime. Recording the
        // nonce makes it single-use *against this host*, which is the difference
        // between "anyone who copies this can join repeatedly" and "anyone who
        // copies this has one race against the real player".
        if self.seen.contains_key(&ticket.nonce) {
            return Err(VerifyFailure::Replayed);
        }
        self.seen.insert(ticket.nonce, ticket.expires_at);

        Ok(AccountIdentity {
            account_id: ticket.account_id,
            username: ticket.username,
        })
    }

    /// Drop nonces whose tickets can no longer be valid anyway.
    fn forget_expired(&mut self, now_unix: u64) {
        // The skew allowance is included, so a nonce is never forgotten while a
        // ticket carrying it would still verify — that would reopen the replay
        // window this cache exists to close.
        self.seen.retain(|_, expires_at| {
            now_unix <= expires_at.saturating_add(wcauth_ticket::CLOCK_SKEW_SECS)
        });
    }

    /// How many nonces are being remembered. Diagnostics only.
    pub fn remembered_nonces(&self) -> usize {
        self.seen.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wcauth_ticket::{SigningKey, TicketClaims};

    const NOW: u64 = 1_800_000_000;
    const KEY_ID: u8 = 3;

    fn signing_key() -> SigningKey {
        SigningKey::from_bytes(&[11; 32])
    }

    fn verifier() -> TicketVerifier {
        TicketVerifier::new(KeySet::new().with(KEY_ID, signing_key().verifying_key()))
    }

    fn ticket_with(nonce: Nonce, username: &str, issued_at: u64) -> [u8; SLOT_LEN] {
        wcauth_ticket::sign(
            &TicketClaims {
                account_id: uuid::Uuid::from_u128(0xABCD),
                username: username.to_string(),
                issued_at,
                expires_at: issued_at + wcauth_ticket::DEFAULT_TTL_SECS,
                nonce,
            },
            KEY_ID,
            &signing_key(),
        )
        .expect("test ticket signs")
    }

    fn ticket() -> [u8; SLOT_LEN] {
        ticket_with([1; 16], "gustav", NOW)
    }

    #[test]
    fn accepts_a_valid_ticket_and_reports_the_account() {
        let identity = verifier().verify(Some(&ticket()), NOW).unwrap();

        assert_eq!(identity.username, "gustav");
        assert_eq!(identity.account_id, uuid::Uuid::from_u128(0xABCD));
    }

    /// The whole point: a client that presents nothing does not get in.
    #[test]
    fn refuses_a_client_that_presents_no_ticket() {
        assert_eq!(verifier().verify(None, NOW), Err(VerifyFailure::Missing));
    }

    #[test]
    fn refuses_a_ticket_signed_by_an_unknown_key() {
        let stranger = SigningKey::from_bytes(&[99; 32]);
        let forged = wcauth_ticket::sign(
            &TicketClaims {
                account_id: uuid::Uuid::from_u128(1),
                username: "impostor".to_string(),
                issued_at: NOW,
                expires_at: NOW + 120,
                nonce: [2; 16],
            },
            KEY_ID,
            &stranger,
        )
        .unwrap();

        assert!(matches!(
            verifier().verify(Some(&forged), NOW),
            Err(VerifyFailure::Invalid(TicketError::BadSignature))
        ));
    }

    #[test]
    fn refuses_a_tampered_ticket() {
        let mut slot = ticket();
        // Rewrite the username in place; the signature no longer covers it.
        slot[19] = b'X';

        assert!(matches!(
            verifier().verify(Some(&slot), NOW),
            Err(VerifyFailure::Invalid(TicketError::BadSignature))
        ));
    }

    #[test]
    fn refuses_an_expired_ticket() {
        let slot = ticket();
        let past = NOW + wcauth_ticket::DEFAULT_TTL_SECS + wcauth_ticket::CLOCK_SKEW_SECS + 1;

        assert!(matches!(
            verifier().verify(Some(&slot), past),
            Err(VerifyFailure::Invalid(TicketError::Expired))
        ));
    }

    /// The replay defence.
    #[test]
    fn refuses_the_same_ticket_twice() {
        let mut verifier = verifier();
        let slot = ticket();

        assert!(verifier.verify(Some(&slot), NOW).is_ok());
        assert_eq!(
            verifier.verify(Some(&slot), NOW),
            Err(VerifyFailure::Replayed)
        );
    }

    /// ...but a player legitimately rejoining with a fresh ticket must not be
    /// caught by it.
    #[test]
    fn accepts_a_second_join_with_a_fresh_ticket() {
        let mut verifier = verifier();

        assert!(
            verifier
                .verify(Some(&ticket_with([1; 16], "gustav", NOW)), NOW)
                .is_ok()
        );
        assert!(
            verifier
                .verify(Some(&ticket_with([2; 16], "gustav", NOW)), NOW)
                .is_ok()
        );
    }

    /// A host that has been up for weeks must not hold a nonce per join.
    #[test]
    fn forgets_nonces_once_their_tickets_could_not_be_valid() {
        let mut verifier = verifier();
        verifier.verify(Some(&ticket()), NOW).unwrap();
        assert_eq!(verifier.remembered_nonces(), 1);

        // A later join sweeps the old entry.
        let much_later = NOW + 10_000;
        verifier
            .verify(
                Some(&ticket_with([2; 16], "gustav", much_later)),
                much_later,
            )
            .unwrap();

        assert_eq!(
            verifier.remembered_nonces(),
            1,
            "the stale nonce should be gone"
        );
    }

    /// The sweep must never drop a nonce while a ticket bearing it would still
    /// verify — that would reopen the replay window.
    #[test]
    fn keeps_a_nonce_for_as_long_as_its_ticket_could_be_replayed() {
        let mut verifier = verifier();
        let slot = ticket();
        verifier.verify(Some(&slot), NOW).unwrap();

        let last_valid_moment =
            NOW + wcauth_ticket::DEFAULT_TTL_SECS + wcauth_ticket::CLOCK_SKEW_SECS;
        assert_eq!(
            verifier.verify(Some(&slot), last_valid_moment),
            Err(VerifyFailure::Replayed),
            "the nonce was forgotten while the ticket was still valid"
        );
    }

    /// Fail closed. A host with no keys refuses everyone rather than admitting
    /// everyone.
    #[test]
    fn a_host_with_no_keys_refuses_every_join() {
        let mut verifier = TicketVerifier::new(KeySet::new());

        assert_eq!(
            verifier.verify(Some(&ticket()), NOW),
            Err(VerifyFailure::NoKeys)
        );
        assert_eq!(verifier.verify(None, NOW), Err(VerifyFailure::NoKeys));
    }

    /// During a key rotation a host holds both keys and must accept either.
    #[test]
    fn accepts_tickets_from_any_trusted_key() {
        let old = SigningKey::from_bytes(&[11; 32]);
        let new = SigningKey::from_bytes(&[22; 32]);
        let mut verifier = TicketVerifier::new(
            KeySet::new()
                .with(KEY_ID, old.verifying_key())
                .with(KEY_ID + 1, new.verifying_key()),
        );

        let claims = TicketClaims {
            account_id: uuid::Uuid::from_u128(5),
            username: "gustav".to_string(),
            issued_at: NOW,
            expires_at: NOW + 120,
            nonce: [7; 16],
        };
        let by_new = wcauth_ticket::sign(&claims, KEY_ID + 1, &new).unwrap();

        assert!(verifier.verify(Some(&by_new), NOW).is_ok());
    }
}

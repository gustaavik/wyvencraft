//! A [`StatusProbe`] that answers from a table instead of a socket.
//!
//! The third impl of the port, like `FakeSession` and `InMemoryWorldRepository`
//! — shipped rather than `#[cfg(test)]`, because the browser owns its probe and
//! a test has to be able to hand one over.

use std::collections::HashMap;
use std::time::Duration;

use super::{ServerStatus, StatusOutcome, StatusProbe};

/// Answers whatever it was told to, immediately.
#[derive(Default)]
pub struct FakeStatusProbe {
    answers: HashMap<String, StatusOutcome>,
    /// What an address with no prepared answer gets.
    fallback: Option<StatusOutcome>,
    /// The content fingerprint [`FakeStatusProbe::online`] reports — "the same
    /// content as us", unless a test says otherwise.
    content_hash: u64,
    /// Queued by `begin`, handed out by the next `poll`.
    pending: Vec<(String, StatusOutcome)>,
    /// Every target list this probe has been asked about, in order.
    pub asked: Vec<Vec<String>>,
}

impl FakeStatusProbe {
    pub fn new() -> Self {
        Self::default()
    }

    /// The content fingerprint every [`FakeStatusProbe::online`] server reports.
    /// Set it to the browser's own hash to make those servers joinable.
    pub fn serving(mut self, content_hash: u64) -> Self {
        self.content_hash = content_hash;
        self
    }

    /// Answer `address` with a server that is up.
    pub fn online(self, address: &str, name: &str, online: u32, max: u32) -> Self {
        let content_hash = self.content_hash;
        self.answering(
            address,
            StatusOutcome::Online(Box::new(ServerStatus {
                name: name.to_string(),
                online,
                max,
                ping_ms: 20,
                content_hash,
            })),
        )
    }

    /// Answer `address` with a given outcome — an offline reason, or a status
    /// with a content hash of its own.
    pub fn answering(mut self, address: &str, outcome: StatusOutcome) -> Self {
        self.answers.insert(address.to_string(), outcome);
        self
    }

    /// What anything not named above gets. Without one, unnamed addresses are
    /// simply never answered — which is how a test spells "still waiting".
    pub fn otherwise(mut self, outcome: StatusOutcome) -> Self {
        self.fallback = Some(outcome);
        self
    }
}

impl StatusProbe for FakeStatusProbe {
    fn begin(&mut self, targets: Vec<String>) {
        self.asked.push(targets.clone());
        self.pending = targets
            .into_iter()
            .filter_map(|target| {
                let outcome = self
                    .answers
                    .get(&target)
                    .or(self.fallback.as_ref())
                    .cloned()?;
                Some((target, outcome))
            })
            .collect();
    }

    fn poll(&mut self, _dt: Duration) -> Vec<(String, StatusOutcome)> {
        std::mem::take(&mut self.pending)
    }

    fn is_busy(&self) -> bool {
        !self.pending.is_empty()
    }
}

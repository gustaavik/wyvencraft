//! The real probe: a short-lived [`Client`] per server, driven from the menu's
//! own frame tick.
//!
//! Only the parts that block run on a thread — one DNS lookup per target and
//! one ticket request for all of them. Everything after that is a few
//! non-blocking socket pumps a frame, which is why a Refresh of a dozen servers
//! costs no threads and no runtime.

use std::net::SocketAddr;
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::Duration;

use wyven_auth::{AccountState, AuthClient, HttpAuthClient, JoinTicket};

use super::{ServerStatus, StatusOutcome, StatusProbe};
use crate::net::{Channel, Client, ClientMessage, PROTOCOL_ID, ServerMessage, address};

/// How long one server gets to connect and answer before it counts as offline.
///
/// Shorter than the join timeout on purpose: a list of a dozen servers is
/// waited on all at once, and a row that will never answer should say so while
/// the player is still looking at it.
const TIMEOUT_SECS: f32 = 6.0;

/// What the worker thread comes back with.
struct Prepared {
    /// The ticket to present, or why there is none.
    ticket: Result<JoinTicket, String>,
    /// Each requested target, resolved or explained.
    resolved: Vec<(String, Result<SocketAddr, String>)>,
}

/// One server being asked.
struct Query {
    target: String,
    client: Client,
    elapsed: f32,
    /// `elapsed` when the question went out; `None` until we are connected.
    asked_at: Option<f32>,
}

/// Queries servers over the game's own protocol.
pub struct NetStatusProbe {
    account: AccountState,
    auth: Arc<dyn AuthClient>,
    /// The netcode identity to connect as — the same one a real join presents.
    ///
    /// Which means a probe cannot query a server this account is *already*
    /// connected to: netcode refuses a second connection under one id. Harmless
    /// in practice — the browser is a menu, and you cannot be on the menu and in
    /// the world at once.
    identity: u64,
    /// In flight until the worker reports back.
    preparing: Option<Receiver<Prepared>>,
    queries: Vec<Query>,
    /// Resolved but not yet collected by [`StatusProbe::poll`].
    finished: Vec<(String, StatusOutcome)>,
}

impl NetStatusProbe {
    /// Against the real auth server.
    pub fn new(account: &AccountState) -> Self {
        Self::with_client(account, Arc::new(HttpAuthClient::from_env()))
    }

    /// With an injected auth client, for tests.
    pub fn with_client(account: &AccountState, auth: Arc<dyn AuthClient>) -> Self {
        let identity = account
            .netcode_id()
            .unwrap_or_else(crate::save::local_identity);
        Self {
            account: account.clone(),
            auth,
            identity,
            preparing: None,
            queries: Vec::new(),
            finished: Vec::new(),
        }
    }

    /// Open a connection per resolved target. Anything that failed to resolve,
    /// or that we cannot present a ticket for, is finished here and now.
    fn start(&mut self, prepared: Prepared) {
        let ticket = match prepared.ticket {
            Ok(ticket) => ticket.slot,
            Err(reason) => {
                for (target, _) in prepared.resolved {
                    self.finished
                        .push((target, StatusOutcome::Offline(reason.clone())));
                }
                return;
            }
        };

        for (target, resolved) in prepared.resolved {
            let addr = match resolved {
                Ok(addr) => addr,
                Err(reason) => {
                    self.finished.push((target, StatusOutcome::Offline(reason)));
                    continue;
                }
            };
            match Client::connect(addr, self.identity, PROTOCOL_ID, Some(ticket)) {
                Ok(client) => self.queries.push(Query {
                    target,
                    client,
                    elapsed: 0.0,
                    asked_at: None,
                }),
                Err(err) => self
                    .finished
                    .push((target, StatusOutcome::Offline(format!("No route ({err})")))),
            }
        }
    }

    /// Drive one query. `Some` when it is done and should be dropped.
    fn advance(query: &mut Query, dt: Duration) -> Option<StatusOutcome> {
        query.elapsed += dt.as_secs_f32();

        if let Err(err) = query.client.pump(dt) {
            return Some(StatusOutcome::Offline(format!("Disconnected ({err})")));
        }

        // Asked exactly once, on the first frame the handshake is complete, so
        // the round trip measured is one message out and one back — not the
        // netcode handshake that preceded it.
        if query.asked_at.is_none() && query.client.is_connected() {
            query
                .client
                .send(&ClientMessage::RequestStatus, Channel::Reliable);
            query.asked_at = Some(query.elapsed);
        }

        let mut answer = None;
        for msg in query.client.receive() {
            if let ServerMessage::Status {
                name,
                online,
                max,
                content_hash,
            } = msg
            {
                let asked_at = query.asked_at.unwrap_or(query.elapsed);
                answer = Some(StatusOutcome::Online(Box::new(ServerStatus {
                    name,
                    online,
                    max,
                    ping_ms: ((query.elapsed - asked_at) * 1000.0).round().max(0.0) as u32,
                    content_hash,
                })));
            }
        }
        let _ = query.client.flush();

        if answer.is_some() {
            // Say goodbye rather than going quiet: the host would otherwise hold
            // a slot for this peer until the connection times out.
            query.client.disconnect();
            return answer;
        }

        (query.elapsed > TIMEOUT_SECS).then(|| {
            query.client.disconnect();
            StatusOutcome::Offline(if query.asked_at.is_some() {
                "No answer".to_string()
            } else {
                "Can't connect".to_string()
            })
        })
    }
}

impl StatusProbe for NetStatusProbe {
    fn begin(&mut self, targets: Vec<String>) {
        self.queries.clear();
        self.finished.clear();
        self.preparing = None;
        if targets.is_empty() {
            return;
        }

        let now = unix_now();
        let account = self.account.clone();
        let auth = Arc::clone(&self.auth);
        let (tx, rx) = channel();

        // One thread for the whole refresh: N DNS lookups and exactly one ticket
        // request. The ticket is account-scoped rather than server-scoped, so
        // every row of one sweep presents the same one — but a *new* sweep must
        // mint a new one, because a host remembers the nonces it has admitted
        // and refuses a ticket it has already seen.
        std::thread::spawn(move || {
            let resolved = targets
                .into_iter()
                .map(|target| {
                    let addr = address::resolve(&target);
                    (target, addr)
                })
                .collect();
            let ticket = crate::net::ticket::issue(&account, auth.as_ref(), now).map_err(|err| {
                log::warn!("could not get a ticket for the server list: {err}");
                if err.is_offline() {
                    "The account server is unreachable".to_string()
                } else {
                    "Sign in to see servers".to_string()
                }
            });
            let _ = tx.send(Prepared { ticket, resolved });
        });

        self.preparing = Some(rx);
    }

    fn poll(&mut self, dt: Duration) -> Vec<(String, StatusOutcome)> {
        if let Some(rx) = self.preparing.as_ref() {
            match rx.try_recv() {
                Ok(prepared) => {
                    self.preparing = None;
                    self.start(prepared);
                }
                Err(TryRecvError::Empty) => {}
                Err(TryRecvError::Disconnected) => self.preparing = None,
            }
        }

        let mut done = std::mem::take(&mut self.finished);
        self.queries
            .retain_mut(|query| match Self::advance(query, dt) {
                Some(outcome) => {
                    done.push((query.target.clone(), outcome));
                    false
                }
                None => true,
            });
        done
    }

    fn is_busy(&self) -> bool {
        self.preparing.is_some() || !self.queries.is_empty() || !self.finished.is_empty()
    }
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

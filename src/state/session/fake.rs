//! An in-memory session for tests: scripted input, recorded output.
//!
//! With this, the parts of the state layer that used to be reachable only over
//! a live UDP socket — welcoming a joiner, restoring a returning player,
//! validating a client's attack, deciding whether an edit is broadcast or
//! requested — become ordinary unit tests.
//!
//! The state layer takes ownership of its session, so both directions run
//! through a shared [`FakeHandle`]: build the session, keep its handle, hand
//! the session over, then script input and read output through the handle.

use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use super::{Authority, Inbound, Session};
use crate::net::{Channel, ClientMessage, PlayerId, ServerMessage};

/// Something the state layer sent, captured instead of transmitted.
#[derive(Debug)]
pub enum Sent {
    Broadcast(ServerMessage, Channel),
    To(PlayerId, ServerMessage, Channel),
    Request(ClientMessage, Channel),
}

/// The shared inbox/outbox behind a [`FakeSession`].
#[derive(Default)]
pub struct FakeState {
    /// Delivered by the next `poll`, in order.
    pending: Vec<Inbound>,
    /// Everything the state layer has sent, in order.
    pub sent: Vec<Sent>,
    pub flushes: usize,
}

impl FakeState {
    /// Every `ServerMessage` broadcast so far.
    pub fn broadcasts(&self) -> Vec<&ServerMessage> {
        self.sent
            .iter()
            .filter_map(|s| match s {
                Sent::Broadcast(msg, _) => Some(msg),
                _ => None,
            })
            .collect()
    }

    /// Every `ServerMessage` addressed to one player so far.
    pub fn messages_to(&self, player: PlayerId) -> Vec<&ServerMessage> {
        self.sent
            .iter()
            .filter_map(|s| match s {
                Sent::To(id, msg, _) if *id == player => Some(msg),
                _ => None,
            })
            .collect()
    }

    /// Every `ClientMessage` sent to the host so far.
    pub fn requests(&self) -> Vec<&ClientMessage> {
        self.sent
            .iter()
            .filter_map(|s| match s {
                Sent::Request(msg, _) => Some(msg),
                _ => None,
            })
            .collect()
    }
}

/// A handle to a [`FakeSession`]'s inbox and outbox, usable after the session
/// itself has been handed to the state.
#[derive(Clone, Default)]
pub struct FakeHandle(Arc<Mutex<FakeState>>);

impl FakeHandle {
    /// Queue a message for the next `poll`.
    pub fn deliver(&self, msg: Inbound) {
        self.lock().pending.push(msg);
    }

    pub fn lock(&self) -> MutexGuard<'_, FakeState> {
        self.0.lock().expect("fake session poisoned")
    }
}

/// A session with no transport: [`poll`](Session::poll) returns whatever was
/// queued on its handle, and every send is recorded there.
pub struct FakeSession {
    authority: Authority,
    local_id: PlayerId,
    handle: FakeHandle,
}

impl FakeSession {
    /// A fake standing in for a host (authoritative, player 0).
    pub fn host() -> Self {
        Self::new(Authority::Local, super::HOST_PLAYER_ID)
    }

    /// A fake standing in for a client with the given assigned id.
    pub fn client(local_id: PlayerId) -> Self {
        Self::new(Authority::Remote, local_id)
    }

    fn new(authority: Authority, local_id: PlayerId) -> Self {
        Self {
            authority,
            local_id,
            handle: FakeHandle::default(),
        }
    }

    /// A handle to this session's inbox and outbox.
    pub fn handle(&self) -> FakeHandle {
        self.handle.clone()
    }
}

impl Session for FakeSession {
    fn authority(&self) -> Authority {
        self.authority
    }

    fn local_id(&self) -> PlayerId {
        self.local_id
    }

    fn poll(&mut self, _dt: Duration) -> Vec<Inbound> {
        std::mem::take(&mut self.handle.lock().pending)
    }

    fn broadcast(&mut self, msg: &ServerMessage, channel: Channel) {
        self.handle
            .lock()
            .sent
            .push(Sent::Broadcast(msg.clone(), channel));
    }

    fn send_to(&mut self, player: PlayerId, msg: &ServerMessage, channel: Channel) {
        self.handle
            .lock()
            .sent
            .push(Sent::To(player, msg.clone(), channel));
    }

    fn request(&mut self, msg: &ClientMessage, channel: Channel) {
        self.handle
            .lock()
            .sent
            .push(Sent::Request(msg.clone(), channel));
    }

    fn flush(&mut self) {
        self.handle.lock().flushes += 1;
    }

    fn is_connected(&self) -> bool {
        true
    }

    fn status(&self, remote_players: usize) -> String {
        match self.authority {
            Authority::Local => format!("fake host ({remote_players} remote)"),
            Authority::Remote => format!("fake client ({remote_players} remote)"),
        }
    }

    /// A fake host stands in for a real one, so it publishes events — that's
    /// what makes mob spawn/hurt/death observable in tests.
    fn serves_peers(&self) -> bool {
        self.authority == Authority::Local
    }
}

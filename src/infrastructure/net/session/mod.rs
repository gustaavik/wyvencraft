//! The real transports behind [`crate::application::session::Session`]:
//! offline, hosting, and joining.

mod client;
mod host;
mod singleplayer;

pub use client::ClientSession;
pub use host::HostSession;
pub use singleplayer::SingleplayerSession;

//! Host-authoritative netcode transport, over `renet` + `renet_netcode`.
//!
//! One peer runs a [`Host`]; the others connect with a [`Client`]. This crate
//! moves bytes and decides who is allowed to send them; *what* those bytes mean
//! is a [`Protocol`] the game declares, and *who* a peer is is a
//! [`JoinVerifier`] the game supplies. Nothing here knows about blocks,
//! inventories or accounts — which is also why it compiles without the private
//! ticket crate that `wyven-auth` needs.

pub mod client;
pub mod server;
pub mod session;
pub mod wire;

pub use client::Client;
pub use server::{DEFAULT_PORT, Host, HostConfig};
pub use session::{Anonymous, JoinVerifier, OpenJoin, Protocol, UserData};
pub use wire::{Channel, PlayerId, decode, encode};

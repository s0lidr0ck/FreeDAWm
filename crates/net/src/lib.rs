//! FreeDAWm on the local network (spec `docs/superpowers/specs/2026-10-31-lan-remote-recording-design.md`).
//!
//! - [`hub::Hub`]: the opt-in listener (Setup › Network › Allow control from this network): the
//!   service remote web page and its status feed, pairing with a 4-digit code, and the
//!   JSON-lines control channel for paired PCs; remote recorders connect to it.
//! - [`follower::Follower`]: a remote recorder's link to the booth it follows.
//! - [`discovery`]: DNS-SD over mDNS (`_freedawm._tcp`), so PCs find each other by name.
//! - [`glue`]: what the app does each frame with all of that, written against [`glue::RigHost`]
//!   so tests run it on plain engines.
//! - [`sync`] and [`take`]: clock alignment and landing remote takes (MIDI now; audio later
//!   through the same envelope).
//!
//! The network runs on its own threads and never touches the audio thread. Native only: on the
//! web the pure parts (settings, sync math, takes, the wire types) still build.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audio;
pub mod config;
pub mod guard;
pub mod pairing;
pub mod proto;
pub mod sync;
pub mod take;

#[cfg(not(target_arch = "wasm32"))]
pub mod discovery;
#[cfg(not(target_arch = "wasm32"))]
pub mod follower;
#[cfg(not(target_arch = "wasm32"))]
pub mod glue;
#[cfg(not(target_arch = "wasm32"))]
pub mod hub;
#[cfg(not(target_arch = "wasm32"))]
pub mod internet;
#[cfg(not(target_arch = "wasm32"))]
pub mod stream;
#[cfg(not(target_arch = "wasm32"))]
pub mod sys;

/// The DNS-SD service type FreeDAWm PCs announce.
pub const SERVICE_TYPE: &str = "_freedawm._tcp.local.";

//! Pro Tools session (`.ptx`, Pro Tools 10 and later) reader for FreeDAWm's File › Import › Pro
//! Tools Session....
//!
//! The decoder is vendored from ptx-session-kit (<https://github.com/knobmob/ptx-session-kit>,
//! commit 294c59a, MIT; `LICENSE-MIT-ptx-session-kit`): the descrambler ([`scramble`]), the block
//! tree ([`tree`]), the byte readers ([`primitives`]) and the session decoder ([`decode`]). Its
//! writer and its SoundCraft mapper are not taken. What was changed is listed at the top of each
//! file and in `README.md`.
//!
//! Input is hostile: nothing here panics, the file size, block count, nesting depth and every
//! decoded list are bounded, and each scan moves forward, so a damaged or crafted file is an error
//! or a partial result, never a crash or a hang. `tests/hostile.rs` checks that.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable, clippy::indexing_slicing)]

pub mod decode;
pub mod error;
pub mod primitives;
pub mod scramble;
pub mod tree;

pub use decode::{PtxDocument, decode_bytes};
pub use error::PtxError;

/// The largest `.ptx` read (a session file holds no audio; real ones are a few megabytes).
pub const MAX_PTX_BYTES: usize = 128 << 20;

// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/error.rs`. MIT licence, Copyright (c) 2026 John Forward
// (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: the writer's `Unsupported` and the file reader's `Io` removed (the engine
// reads files itself); `TooLarge` added.
//! Errors for the clean-room Pro Tools `.ptx` reader.

/// Errors returned when parsing `.ptx` binary sessions.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PtxError {
    /// Shorter than the 20-byte header plus one block.
    #[error("file too short ({0} bytes): not a Pro Tools session")]
    TooShort(usize),
    /// Larger than [`crate::MAX_PTX_BYTES`].
    #[error("file too large ({0} bytes) for a Pro Tools session")]
    TooLarge(usize),
    /// XOR type byte (`0x12`) is not one of the known types `1` or `5`.
    #[error("unknown scrambling type {0} (known: 1, 5): not a Pro Tools 10 or later session")]
    UnknownXorType(u8),
    /// No step byte yields the stored XOR value (`0x13`).
    #[error("no scrambling step for value {0}: damaged or not a .ptx")]
    NoXorDelta(u8),
    /// Block tree or record count exceeds defensive limits.
    #[error("limit exceeded: {0}")]
    Limit(&'static str),
    /// A binary structure did not match its expected layout.
    #[error("unexpected layout at byte {offset}: {what}")]
    Layout {
        /// Description of the expected structure.
        what: &'static str,
        /// Byte offset in the descrambled payload.
        offset: usize,
    },
}

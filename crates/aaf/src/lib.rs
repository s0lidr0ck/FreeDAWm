//! AAF (and later OMF) interchange readers for FreeDAWm: turning a picture editor's or another
//! DAW's audio timeline into a neutral description ([`desc::Import`]) the engine places as tracks
//! and clips.
//!
//! Three layers, each written from published specifications (the Microsoft compound-file format
//! [MS-CFB], the AAF Object Specification v1.1, the AAF Low-Level Container Specification and the
//! AAF Edit Protocol) and checked against real files from Pro Tools, Media Composer, Premiere and
//! Resolve:
//! - [`cfb`]: the compound-file container (sectors, FAT/DIFAT, mini stream, directory tree);
//! - [`object`]: the AAF stored-object format (property streams, strong and weak references,
//!   sets, vectors, data streams) and the meta dictionary's extension properties;
//! - [`aaf`]: the object model walked from the header through composition, master and source
//!   mobs to the essence, mapped to [`desc::Import`].
//!
//! Input is hostile: nothing here panics, every size and count is bounded, and reference chains
//! that loop are refused.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod aaf;
pub mod cfb;
pub mod desc;
pub mod object;
#[cfg(any(test, feature = "test-writer"))]
pub mod testwrite;
pub mod walk;

pub use desc::Import;
pub use walk::{Options, read};

/// What went wrong reading a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Error(pub String);

impl Error {
    pub fn new(msg: impl Into<String>) -> Error {
        Error(msg.into())
    }
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for Error {}

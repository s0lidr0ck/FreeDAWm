//! FreeDAWm Recorder (`freedawm-recorder`): a small window beside the DAW for
//!
//! 1. quick ideas: one big Record button; audio and MIDI recorded together into a WAV and a MIDI
//!    file named by the local date and time, in a folder of the user's ([`rig`], [`files`]);
//! 2. recording with a FreeDAWm "studio" (the DAW's booth) on another PC, as the DAW's remote
//!    recorder does ([`booth`]);
//! 3. listening to that studio's mix.
//!
//! It is a front end over the DAW's own parts (`soundcraft-playback` for audio and MIDI input
//! and the take writer, `soundcraft-net` for following a booth). It never listens on the
//! network: it only dials a booth the user joined. See `docs/recorder.md`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod booth;
pub mod files;
pub mod keys;
pub mod layers;
pub mod rig;
pub mod settings;
pub mod ui;
pub mod words;

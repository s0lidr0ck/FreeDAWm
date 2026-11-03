//! SoundCraft Band: the MIDI Studio band engine, ported to Rust.
//!
//! - [`midi_types`]: `MidiTypes.h` (the event, the sink).
//! - [`library`]: `Library.h` (players and targets), the embedded factory Library and its loader.
//! - [`band_preset`]: band presets (`PresetStore.cpp`, parse and save).
//! - [`session_tables`]: the session tables (`SessionTables.h`).
//! - [`guitar_shapes`], [`ample_slots`], [`driver_kit`], [`plucked_voice`], [`fiddle_voice`]: voice and driver support.
//! - [`chord_match`], [`midi_chord_import`]: chords from names, notes and MIDI files.
//! - [`meter`], [`chord_theory`], [`tension`], [`song_feel`], [`presets`]: `Meter.h`, `ChordTheory.h`, `Tension.h`,
//!   `SongFeel.h` (+ `FeelPresets.h`), `Presets.h`.
//! - [`song`]: `Song.h` (the song, its timeline, `songLocate`, the edits, `LegacySong`).
//! - [`lanes`]: the DAW's section and chord lanes as a `SongTimeline` (not a port), and a song laid onto lanes.
//! - [`song_file`]: reading song files (`SongFile.cpp`, `SongDoc.cpp`) and the factory songs.
//! - [`live`]: `Live.h` (the chord recogniser, `PerformanceState`, `BandShared`, the `Performer`).
//! - `musician` (crate-private): `Musician.h` and its `.cpp` files, one file each.
//! - [`band_map`]: `BandMap.cpp`, the compute half (the Band view's bars).
//! - [`band`]: `Band.h` (six musicians, frame by frame); [`params`]: `Parameters.h` (host parameters, packing).
//! - [`runtime`]: `BandRuntime`, the processor's audio path (`MidiStudioProcessor::processBlock`), and `BandSettings`.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]
#![forbid(unsafe_code)]

pub mod ample_slots;
pub mod band;
pub mod band_map;
pub mod band_preset;
pub mod chord_match;
pub mod chord_theory;
pub mod driver_kit;
pub mod fiddle_voice;
pub mod guitar_shapes;
pub mod lanes;
pub mod library;
pub mod live;
pub mod meter;
pub mod midi_chord_import;
pub mod midi_types;
pub(crate) mod musician;
pub mod params;
pub mod plucked_voice;
pub mod presets;
pub mod runtime;
pub mod session_tables;
pub mod song;
pub mod song_feel;
pub mod song_file;
pub mod tension;

pub use musician::MusicianDefaults;

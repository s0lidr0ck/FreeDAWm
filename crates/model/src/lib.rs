//! The SoundCraft session document.
//!
//! A [`Session`] is pure data: tracks with playlists of clips, the mixer (inserts, sends, routing,
//! automation), tempo/meter, memory locations and groups. Audio sample data lives in
//! [`SourcePool`] behind `Arc`s, so cloning a session for undo is cheap.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod automation;
pub mod b64;
mod band;
mod clip;
pub mod disk;
mod ids;
mod lyrics;
mod markers;
pub mod migrate;
mod mixer;
mod notes;
mod scout;
mod session;
mod sheet;
mod solo;
mod source;
mod track;
pub mod vocal;

pub use automation::{AutoParam, AutomationLane, AutomationMode, AutomationPoint, CURVE_EPSILON, curve_shape, letter_slot};
pub use band::{BandFeel, BandState, ChordEvent, MAX_BLOCKS, MAX_CHORDS, MusicianSlot, SectionBlock, SectionDef, SectionMusician, SongFeelDials};
pub use clip::{CleanAlt, CleanupPreview, Clip, ClipContent, Fade, FadeShape, MidiCleanup, PatternClip};
pub use ids::{BusId, ClipId, GroupId, MarkerId, SourceId, TrackId};
pub use lyrics::{
    LyricLine, LyricWord, MAX_LINE_WORDS, MAX_LYRIC_LINES, MAX_WORD_CHARS, clip_lines, rebase_lyrics, sanitize_lyrics, to_clip_time,
    track_has_lyrics, track_lines,
};
pub use markers::{Group, MarkerKind, MemoryLocation};
pub use mixer::{
    Bus, ChainError, ChannelFormat, DrumFamily, ForeignDevice, InputMonitor, InputPath, Insert, KIT_FX_BASE, KitBus, KitFx, KitLayout, KitPad,
    MAX_MODULES, MidiIn, Mixer, ModuleId, ModuleRef, OutputPath, PreparedState, REFERENCE_KEYS, Route, SEND_SLOTS, SendSlot, SidechainSource,
    Speaker, StripSize, SurroundPan, fader_db_to_pos, fader_pos_to_db, is_reference_key, track_input_channel,
};
pub use notes::{MAX_NOTE_CHARS, MAX_NOTES, Note, note_typed, sanitize_notes, split_tags};
pub use scout::{
    AutomatedParam, Guide, MAX_PROFILE_BYTES, MAX_SCOUT_ITEMS, PROFILE_ROLES, ProfileKind, ProfileSource, SCOUT_BANDS, ScoutCapture, ScoutMetrics,
    ScoutProfile, ScoutState, ScoutStrip, ScoutTurn, profile_file_stem,
};
pub use session::{
    BitDepthSetting, EditMode, EditState, FEET_FRAMES_START, LEGACY_SESSION_EXTENSION, MAX_COMMENT_CHARS, MAX_NAME_CHARS, SESSION_EXTENSION,
    SESSION_EXTENSIONS, Session, SessionError, Tool, TrackView, ZoomState, ZoomToggle, is_session_extension, is_session_path, same_name,
    truncate_chars,
};
pub use sheet::{
    Draft, LineKind, LyricPlace, LyricSheet, MAX_ALTERNATES, MAX_DRAFTS, MAX_SHEET_LINE_CHARS, MAX_SHEET_LINES, SheetLine, carry_ids, heading_name,
    line_kinds, sheet_texts,
};
pub use solo::SoloScope;
pub use source::{AssetData, AssetPool, SampleCursor, Source, SourceAudio, SourcePool, VideoSource};
pub use track::{FrozenAudio, Playlist, TRACK_COLORS, Track, TrackHeight, TrackKind};

pub use vocal::*;

pub use soundcraft_time as time;

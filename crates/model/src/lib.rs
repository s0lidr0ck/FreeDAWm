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
pub mod clip_automation;
mod control_room;
pub mod disk;
mod ids;
mod live_stretch;
mod lyrics;
mod markers;
mod midi_fx;
mod midi_map;
pub mod migrate;
mod mixer;
mod modulator;
pub mod monitor;
mod notes;
pub mod rate_change;
mod scout;
mod session;
mod sheet;
mod snapshot;
mod solo;
mod source;
mod track;
pub mod vocal;

pub use automation::{
    AutoParam, AutomationLane, AutomationMode, AutomationPoint, BYPASS_PARAM, CURVE_EPSILON, MIX_PARAM, PencilShape, SURROUND_AXES, curve_shape,
    letter_slot,
};
pub use band::{BandFeel, BandState, ChordEvent, MAX_BLOCKS, MAX_CHORDS, MusicianSlot, SectionBlock, SectionDef, SectionMusician, SongFeelDials};
pub use clip::{
    CleanAlt, CleanupPreview, Clip, ClipContent, Fade, FadeShape, FxRender, MAX_TRANSPOSE_CENTS, MidiCleanup, MidiFxRender, PatternClip, ProcessStep,
};
pub use control_room::{ControlRoom, CueMix, MAX_CUES, MAX_MONITOR_SETS, MonitorSet, Talkback};
pub use ids::{BusId, ClipId, GroupId, MarkerId, SourceId, TrackId};
pub use live_stretch::{LiveStretch, PitchRender, PitchRenders, StretchKind};
pub use lyrics::{
    LyricLine, LyricWord, MAX_LINE_WORDS, MAX_LYRIC_LINES, MAX_WORD_CHARS, clip_lines, rebase_lyrics, sanitize_lyrics, to_clip_time,
    track_has_lyrics, track_lines,
};
pub use markers::{Group, LocationView, MarkerKind, MemoryLocation, SetListEntry};
pub use midi_fx::{ARP_PATTERNS, CHORD_SHAPES, MAX_MIDI_FX, MIDI_FX_RATES, MidiFx, MidiFxKind, ROOT_NAMES, SCALES};
pub use midi_map::{MidiMap, MidiSource, MidiSourceKind, MidiTarget, TransportAction};
pub use mixer::{
    Bus, ChainError, ChannelFormat, DrumFamily, ForeignDevice, INSTRUMENT_MODULE, InputMonitor, InputPath, Insert, KIT_FX_BASE, KitBus, KitFx,
    KitLayout, KitPad, MAX_MODULES, MAX_TRACK_DELAY, MidiIn, MidiTransform, Mixer, ModuleId, ModuleRef, OutputPath, PanLink, PreparedState,
    REFERENCE_KEYS, Route, SEND_SLOTS, SPACE_IR_KEY, SendSlot, SidechainSource, Speaker, StripSize, SurroundPan, default_input_channel,
    fader_db_to_pos, fader_pos_to_db, is_reference_key, track_input_channel,
};
pub use modulator::{LFO_SHAPES, LFO_SYNC, MAX_MOD_TARGETS, MAX_MODULATORS, ModTarget, Modulator, ModulatorKind, lfo_value};
pub use notes::{MAX_NOTE_CHARS, MAX_NOTES, Note, note_typed, sanitize_notes, split_tags};
pub use scout::{
    AutomatedParam, Guide, MAX_PROFILE_BYTES, MAX_SCOUT_ITEMS, PROFILE_ROLES, ProfileKind, ProfileSource, SCOUT_BANDS, ScoutCapture, ScoutMetrics,
    ScoutProfile, ScoutState, ScoutStrip, ScoutTurn, profile_file_stem,
};
pub use session::{
    Arrangement, BitDepthSetting, EditMode, EditState, FEET_FRAMES_START, LEGACY_SESSION_EXTENSION, MAX_COMMENT_CHARS, MAX_NAME_CHARS,
    RecordFileType, SESSION_EXTENSION, SESSION_EXTENSIONS, Session, SessionError, Tool, TrackView, ZoomState, ZoomToggle, is_session_extension,
    is_session_path, same_name, truncate_chars,
};
pub use sheet::{
    Draft, LineKind, LyricPlace, LyricSheet, MAX_ALTERNATES, MAX_DRAFTS, MAX_SHEET_LINE_CHARS, MAX_SHEET_LINES, SheetLine, carry_ids, heading_name,
    line_kinds, sheet_texts,
};
pub use snapshot::{MAX_SNAPSHOTS, MixSnapshot, SNAPSHOT_SECTIONS, SnapshotStrip};
pub use solo::SoloScope;
pub use source::{AssetData, AssetPool, SampleCursor, Source, SourceAudio, SourcePool, VideoSource};
pub use track::{
    Articulation, Articulations, FrozenAudio, InstrumentOut, MAX_ARTICULATIONS, MpeZone, Playlist, TRACK_COLORS, Track, TrackHeight, TrackKind,
};

pub use vocal::*;

pub use soundcraft_time as time;

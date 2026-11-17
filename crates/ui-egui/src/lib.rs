//! FreeDAWm's egui user interface (the `soundcraft-ui-egui` crate).
//!
//! The UI is a thin shell over [`soundcraft_engine::Engine`]: it draws engine state and acts by
//! executing commands. It can be replaced without touching anything below it.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod audition;
pub mod auto_lanes;
pub mod band;
pub mod bounce;
pub mod brand_art;
pub mod browser;
pub mod chain_view;
pub mod chamber_plan;
pub mod channel_width;
pub mod chord_pads;
pub mod cleanq_ceiling;
pub mod control;
pub mod control_room;
pub mod credits;
pub mod curve_worker;
pub mod dawproject_report;
pub mod device_return;
pub mod dialogs;
pub mod docking;
pub mod edit_window;
pub mod editor_kit;
pub mod editors;
pub mod engine_ui;
pub mod extra_windows;
pub mod fade_dialog;
pub mod fonts;
pub mod fs_presets;
pub mod hardware_credit;
pub mod hearing_ui;
pub mod hit_picker;
pub mod icons;
mod imprint_ui;
pub mod juke_fx;
#[cfg(not(target_arch = "wasm32"))]
pub mod juke_kits_ui;
pub mod juke_window;
pub mod key_knobs;
pub mod key_ruler;
pub mod levelup_ui;
mod lift;
pub mod link_picker;
pub mod live_midi;
pub mod lyric_ruler;
pub mod lyrics_lane;
pub mod lyrics_panel;
mod mend_ui;
pub mod menus;
pub mod midi_cleanup_panel;
pub mod midi_devices;
pub mod midi_edit_tools;
pub mod midi_editor;
pub mod midi_editor_link;
pub mod midi_editor_window;
pub mod midi_event_list;
mod midi_fx_menu;
pub mod midi_ghosts;
pub mod midi_lanes;
pub mod midi_learn;
pub mod midi_sync_in;
pub mod midi_wide_lanes;
pub mod mix_find;
pub mod mix_window;
#[cfg(not(target_arch = "wasm32"))]
pub mod ml_audition;
#[cfg(not(target_arch = "wasm32"))]
pub mod ml_downloads;
#[cfg(not(target_arch = "wasm32"))]
pub mod ml_text_ui;
#[cfg(not(target_arch = "wasm32"))]
pub mod ml_ui;
mod modulators_ui;
pub mod module_drawer;
pub mod module_graphs;
pub mod monitor_ui;
pub mod musical_typing;
#[cfg(not(target_arch = "wasm32"))]
pub mod net_ui;
pub mod notes_panel;
pub mod open_progress;
pub mod ops_windows;
pub mod palette;
pub mod panels;
pub mod plugin_titles;
pub mod preferences;
pub mod preset_house;
mod recent;
pub mod record_offset;
pub mod recovery;
pub mod render;
pub mod ruler_items;
pub mod sampler_window;
pub mod scope;
pub mod score_editor;
pub mod scout;
pub mod scrub;
pub mod session_key;
pub mod shale_ui;
pub mod shortcut_editor;
pub mod shortcuts;
pub mod sidechain_picker;
pub mod slider_drag;
pub mod split_progress;
pub mod spot;
pub mod step_grid;
pub mod step_input;
pub mod stretch_worker;
pub mod surfaces;
pub mod swale_ui;
pub mod teleprompter;
pub mod tempo_markers;
pub mod tempo_ui;
pub mod theme;
pub mod timeline_link;
pub mod toolbar;
pub mod tour;
pub mod track_notes;
pub mod user_files;
pub mod value_entry;
mod vault_ui;
pub mod video_track;
pub mod video_window;
pub mod vocal_editor;
pub mod vocal_ids;
pub mod vocal_mixer;
pub mod vocal_style;
pub mod vocal_track;
pub mod vocal_worker;
mod wait_note;
pub mod waveform;
pub mod widgets;
pub mod window_sync;
pub mod windows;
pub mod word_finder;

use serde_json::{Value, json};
use soundcraft_engine::{Engine, TransportRequest};
use soundcraft_model::{Route, TrackId};
use soundcraft_playback::record::Trouble;
use soundcraft_playback::{OutputTrouble, Player, SlotKey};
use soundcraft_time::{Range, Samples};
use std::collections::{HashMap, HashSet};
use std::sync::mpsc::Receiver;

pub use control::ControlRequest;
pub use record_offset::{RecordOffset, rate_warning, record_latency};

/// PipeWire's "All sources" input as I/O Setup shows it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AllSources {
    /// Every source to pick from.
    pub sources: Vec<soundcraft_playback::pw::Source>,
    /// What each input channel records (`node:port`), in channel order.
    pub chosen: Vec<String>,
}

/// Which main window is in front (Window › Mix / Edit).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum MainWindow {
    Edit,
    Mix,
}

/// The launch splash's memory in `ui.json` (see the app's `splash` module).
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct SplashMemory {
    /// The index (in `assets/splash/statements.txt`) of the statement the last splash showed.
    pub last_statement: Option<usize>,
}

/// UI state (serde, so agents can read and set it).
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct UiState {
    /// File › Open Recent, newest first (see `recent`).
    pub recent_sessions: Vec<String>,
    pub docking: docking::Workspace,
    #[serde(default = "docking::mix_default")]
    pub mix_docking: docking::Workspace,
    pub window: MainWindow,
    pub show_tracks_list: bool,
    pub show_clip_list: bool,
    pub show_transport: bool,
    pub show_memory_locations: bool,
    pub show_big_counter: bool,
    /// Window › Loudness Meter: the master's loudness and true peak.
    #[serde(default)]
    pub show_loudness: bool,
    /// Window › Mix Snapshots.
    #[serde(default)]
    pub show_mix_snapshots: bool,
    /// Window › Control Room.
    #[serde(default)]
    pub show_control_room: bool,
    /// Talkback is on because F11 is held, the Talk button is held, or Latch is on.
    #[serde(skip)]
    pub talk_held: bool,
    #[serde(skip)]
    pub talk_button: bool,
    #[serde(skip)]
    pub talk_latched: bool,
    pub show_undo_history: bool,
    /// The Notes tab (Window › Notes).
    pub show_notes: bool,
    /// What the Notes tab shows: open, done or all.
    pub notes_show: String,
    /// The Lyrics tab (Window › Lyrics).
    pub show_lyrics: bool,
    /// The first-run tour's open card (see [`tour`]); not saved.
    #[serde(skip)]
    pub tour_step: Option<usize>,
    /// The Lyrics tab shows each line's syllable count.
    pub lyrics_syllables: bool,
    /// The Lyrics tab letters and underlines rhyming end words.
    pub lyrics_rhymes: bool,
    /// Which rhyme types the Lyrics tab colours (`MarkKind::bit`s: perfect, slant, multi, internal).
    #[serde(default = "default_rhyme_kinds")]
    pub lyrics_rhyme_kinds: u8,
    /// The Lyrics tab shows each line's stress pattern in a column.
    pub lyrics_stress: bool,
    /// The Lyrics tab flags lines that do not fit their partner line (syllables, stress).
    #[serde(default = "default_true")]
    pub lyrics_fit: bool,
    /// The Teleprompter (Window › Teleprompter).
    pub show_teleprompter: bool,
    /// The Teleprompter in its own native window.
    pub teleprompter_separate: bool,
    /// That window's last position and inner size, `[x, y, w, h]`.
    pub teleprompter_window: Option<[f32; 4]>,
    pub show_session_info: bool,
    pub show_audio_health: bool,
    pub show_about: bool,
    pub show_midi_editor: bool,
    /// The Vocal Editor (AI › Tune Vocal…); plan 3 draws it.
    pub show_vocal_editor: bool,
    /// The Vocal Editor's Preview toggle: a clicked or edited note plays once (`vocal.preview`).
    #[serde(default = "default_true", rename = "vocal.preview")]
    pub vocal_preview: bool,
    /// The Vocal Editor's zoom relative to the Edit window (¼×..4×, `vocal.zoom_factor`).
    #[serde(default = "default_zoom_factor", rename = "vocal.zoom_factor")]
    pub vocal_zoom_factor: f32,
    /// The Vocal Editor's row height, px per semitone (`vocal.row_h`; its − / + buttons,
    /// Ctrl+Shift+wheel).
    #[serde(default = "default_vocal_row_h", rename = "vocal.row_h")]
    pub vocal_row_h: f32,
    pub show_universe: bool,
    pub narrow_mix: bool,
    /// The Mix view's Channel width bar acts on all channels (else on the selected ones).
    pub mix_width_all: bool,
    /// A module with a VST-faithful editor (`editors`) opens it as a wide drawer, laid out as its
    /// C++ editor, instead of the narrow generic one.
    pub wide_editors: bool,
    /// Every FireStation editor draws its slim layout at the classic drawer's width
    /// (`module_drawer::DRAWER_W`): the same sections in the same order, stacked.
    #[serde(default)]
    pub slim_editors: bool,
    /// The plugin editors' size in the Mix window (1 = as designed; `editor_kit::EDITOR_SCALES`).
    #[serde(default = "one_f32")]
    pub editor_scale: f32,
    /// Per window (viewport): the wide editor under the pointer last frame and the keys it takes
    /// from the DAW (`keyPressed`); absent when the pointer is over none.
    #[serde(skip)]
    pub wide_key_owner: HashMap<egui::ViewportId, editor_kit::KeyOwner>,
    /// Mix window sections: inserts_ae, inserts_fj, sends_ae, sends_fj, io, comments, eq_curve,
    /// color, pitch (the PITCH section on vocal strips).
    pub mix_views: Vec<String>,
    /// The Mix window sections added since settings were first saved that were already offered
    /// (switched on once, see [`UiState::offer_new_mix_views`]): a section the user turned off
    /// stays off. Missing from older `ui.json` files, so they get the new sections once.
    #[serde(default)]
    pub mix_views_offered: Vec<String>,
    /// Edit window track-header columns: io, inserts_ae, sends_ae, comments, color.
    pub edit_views: Vec<String>,
    /// How strongly Edit window tracks take on their colour, 0..1 (0: only the colour strip).
    pub track_tint: f32,
    /// Open plugin windows: (track, module or instrument). Keyed by module id, so a window
    /// follows its module when the chain is reordered.
    pub plugin_windows: Vec<(TrackId, SlotKey)>,
    /// Open full editor windows of built-in modules (their VST-faithful editor, at full size,
    /// in a window of its own: the tile's ⤢ button).
    pub full_editors: Vec<(TrackId, soundcraft_model::ModuleId)>,
    /// Plugin windows moved out of the main window, each into a native window of its own
    /// (the window's Detach button). A key stays here while its window is closed, so the
    /// window comes back detached.
    #[serde(skip)]
    pub detached_plugins: Vec<(TrackId, SlotKey)>,
    /// Plugin windows pinned on top of the others (the window's Pin button).
    #[serde(skip)]
    pub pinned_plugins: Vec<(TrackId, SlotKey)>,
    /// Plugin windows that follow the selected track (the window's Follow button): selecting
    /// another track shows the same plug-in there, if it has one.
    #[serde(skip)]
    pub following_plugins: Vec<(TrackId, SlotKey)>,
    /// The Modulators window, open on this track's modulators.
    #[serde(skip)]
    pub modulators_window: Option<TrackId>,
    /// Where a detached plugin window was last, `[x, y, w, h]` by plugin id (`plugin_windows.rects`):
    /// the next detached window of that plugin opens there.
    #[serde(default, rename = "plugin_windows.rects")]
    pub plugin_window_rects: HashMap<String, [f32; 4]>,
    /// Per Juke track: the view its window shows (Kit, Pad or Steps).
    #[serde(skip)]
    pub juke_tabs: HashMap<TrackId, juke_window::Tab>,
    /// Per Juke track: the pattern clip its Steps view edits.
    #[serde(skip)]
    pub juke_clips: HashMap<TrackId, soundcraft_model::ClipId>,
    /// Juke's hit picker, open on drum takes for a pad.
    #[serde(skip)]
    pub hit_picker: Option<hit_picker::HitPicker>,
    /// The hit the picker last played (outlined).
    #[serde(skip)]
    pub hit_playing: Option<usize>,
    /// A Juke pad to show in its window's Pad view next frame: (track, pad from 0).
    #[serde(skip)]
    pub juke_pad_select: Option<(TrackId, usize)>,
    /// The module whose drawer is open on each Mix window strip (at most one per track).
    pub drawers: HashMap<TrackId, soundcraft_model::ModuleId>,
    /// Mix window strips whose band drawer is open (a musician's controls, beside the strip).
    pub band_drawers: HashSet<TrackId>,
    /// Each open compressor drawer's gain reduction over the last four seconds (dB, newest last).
    #[serde(skip)]
    pub gr_history: HashMap<(TrackId, soundcraft_model::ModuleId), module_drawer::GrHistory>,
    /// CleanQ ceiling Learns in progress, per drawer (`cleanq_ceiling`).
    #[serde(skip)]
    pub cq_learn: HashMap<(TrackId, soundcraft_model::ModuleId), cleanq_ceiling::Learn>,
    /// Each module's preset `dirty` flag as the wide editors keep it (`editor_kit::presets::is_dirty`).
    #[serde(skip)]
    pub preset_dirty: HashMap<(TrackId, soundcraft_model::ModuleId), editor_kit::presets::DirtyRef>,
    /// A drawer tab asked for by `ui.mix.drawer {tab}`, shown by the drawer's next frame.
    #[serde(skip)]
    pub drawer_tab: Option<(TrackId, soundcraft_model::ModuleId, String)>,
    /// The send in the Send window (track, slot).
    #[serde(skip)]
    pub send_window: Option<(TrackId, usize)>,
    /// The marker or chord clicked on a ruler: Delete removes it (`ruler_items`).
    #[serde(skip)]
    pub ruler_item: Option<ruler_items::RulerItem>,
    /// A drag of one end of the loop strip on the main ruler (`edit_window::loop_strip`).
    #[serde(skip)]
    pub loop_drag: Option<edit_window::LoopDrag>,
    /// The edit selection and selected clips when `ruler_item` was picked: a change drops the pick.
    #[serde(skip)]
    pub ruler_pick_sel: Option<(soundcraft_time::Range, Vec<soundcraft_model::ClipId>)>,
    pub audiosuite: Option<String>,
    pub status: String,
    pub show_automation: bool,
    pub show_color_palette: bool,
    pub show_disk_usage: bool,
    pub show_system_usage: bool,
    pub show_task_manager: bool,
    pub show_metadata: bool,
    pub show_event_list: bool,
    pub show_midi_keyboard: bool,
    pub show_workspace: bool,
    pub show_configurations: bool,
    pub show_playback_engine: bool,
    /// The audio device choice (Setup › Playback Engine), kept with the preferences: per machine.
    pub audio: soundcraft_playback::AudioConfig,
    /// The record offset per audio system + device + buffer + rate (`record_offset`): per
    /// machine, because drivers add hidden latency that changes with each.
    pub record_offsets: std::collections::BTreeMap<String, RecordOffset>,
    /// The Mix window lives in its own native window (Window › Mix in Separate Window); the main
    /// window then always shows Edit.
    pub mix_separate: bool,
    /// The Vocal Editor lives in its own native window (Window › Vocal Editor in Separate
    /// Window, `vocal.separate`); closing that window docks it back.
    #[serde(default, rename = "vocal.separate")]
    pub vocal_separate: bool,
    /// That window's last position and inner size, `[x, y, w, h]` (`vocal.window`).
    #[serde(default, rename = "vocal.window")]
    pub vocal_window: Option<[f32; 4]>,
    /// The MIDI Editor lives in its own native window (Window › MIDI Editor in Separate Window,
    /// `midi_editor.separate`); closing that window docks it back.
    #[serde(default, rename = "midi_editor.separate")]
    pub midi_editor_separate: bool,
    /// That window's last position and inner size, `[x, y, w, h]` (`midi_editor.window`).
    #[serde(default, rename = "midi_editor.window")]
    pub midi_editor_window: Option<[f32; 4]>,
    /// Window › Sync: what the Edit and Mix windows share.
    pub sync: window_sync::SyncOptions,
    pub show_io_setup: bool,
    /// Setup › MIDI Input Devices… (`midi_devices`).
    #[serde(default)]
    pub show_midi_inputs: bool,
    /// Setup › MIDI Mappings… (`midi_learn`).
    #[serde(default)]
    pub show_midi_maps: bool,
    /// MIDI learn's transport keys (`midi.transport_maps`): per machine, as the controller is.
    #[serde(default, rename = "midi.transport_maps")]
    pub midi_transport_maps: Vec<soundcraft_model::MidiMap>,
    /// Setup › Control Surfaces… (`surfaces`).
    #[serde(default)]
    pub show_surfaces: bool,
    /// The control surfaces (`surfaces`): per machine, as the hardware is.
    #[serde(default)]
    pub surfaces: Vec<surfaces::SurfaceConfig>,
    /// Setup › Network… (`net_ui`).
    #[serde(default)]
    pub show_network: bool,
    /// MIDI input ports enabled (true) or not, by port name (`midi.inputs`): per machine. A
    /// port not named is enabled.
    #[serde(default, rename = "midi.inputs")]
    pub midi_inputs: std::collections::BTreeMap<String, bool>,
    /// MIDI output ports enabled (true) or not, by port name (`midi.outputs`): per machine. A
    /// port not named is disabled unless a track sends to it.
    #[serde(default, rename = "midi.outputs")]
    pub midi_outputs: std::collections::BTreeMap<String, bool>,
    /// What each MIDI input port is called here, by port name (`midi.aliases`): per machine.
    #[serde(default, rename = "midi.aliases")]
    pub midi_aliases: std::collections::BTreeMap<String, String>,
    /// Each MIDI input port's timing offset in ms, by port name (`midi.offsets`): its messages
    /// arrive that late and are placed that much earlier. Per machine.
    #[serde(default, rename = "midi.offsets")]
    pub midi_offsets: std::collections::BTreeMap<String, f32>,
    /// MIDI Thru (`midi.thru`): record-enabled instrument and MIDI tracks play their live MIDI.
    #[serde(default = "default_true", rename = "midi.thru")]
    pub midi_thru: bool,
    pub show_shortcuts: bool,
    /// Setup › Preferences… and its tab (`preferences::TABS`).
    #[serde(default)]
    pub show_preferences: bool,
    #[serde(default)]
    pub preferences_tab: String,
    /// Preferences › Metering: meter release (dB a second) and peak hold (seconds; 0 none,
    /// `preferences::HOLD_UNTIL_CLEARED` until cleared).
    #[serde(default = "default_meter_release")]
    pub meter_release_db_s: f32,
    #[serde(default = "default_meter_hold")]
    pub meter_hold_secs: f32,
    /// Preferences › Metering › Scale (`widgets::MeterScale` id).
    #[serde(default)]
    pub meter_scale: String,
    /// Preferences › Operation: imported audio is copied into the project's `Audio Files` when
    /// the session is saved (on by default); off, the project refers to the original file.
    #[serde(default = "default_true")]
    pub copy_imported_audio: bool,
    /// Preferences › Mixing: hosted CLAP plug-ins loaded from now on run in their own process
    /// (`ui.plugin_process`; off by default; `FREEDAWM_PLUGIN_PROCESS` overrides it). The app
    /// hands it to the bridge when it loads `ui.json` and when the command changes it.
    #[serde(default)]
    pub plugin_process: bool,
    /// The monitor stage (`monitor_ui`): the master fader's EAR key and its hearing profile.
    /// This computer's, never a session's.
    pub monitor_ear: bool,
    pub monitor_ear_profile: String,
    /// The master fader's SIM key and the simulated output it plays the mix through.
    pub monitor_sim: bool,
    pub monitor_sim_profile: String,
    /// The Ear Profile window.
    #[serde(skip)]
    pub show_ear_profile: bool,
    /// The Hearing Test window.
    #[serde(skip)]
    pub show_hearing_test: bool,
    /// The user's keyboard shortcuts: command id → shortcut ("Cmd+Shift+X"), `""` for none.
    /// Commands not in it keep the registry's (`shortcuts::effective_shortcut`).
    pub shortcuts: shortcuts::Overrides,
    /// Setup › Keyboard Shortcuts' search, key capture and conflict (not saved).
    #[serde(skip)]
    pub shortcut_editor: shortcut_editor::EditorState,
    pub show_clip_effects: bool,
    pub show_beat_detective: bool,
    pub show_tempo_ops: bool,
    pub show_time_ops: bool,
    pub show_midi_ops: bool,
    pub show_rtp: bool,
    pub show_input_quantize: bool,
    pub show_click_countoff: bool,
    pub show_disk_allocation: bool,
    pub show_midi_offsets: bool,
    pub show_score: bool,
    pub show_search: bool,
    /// Window › Video, Window › Video Universe, timecode burn-in in the Video window.
    pub show_video: bool,
    pub show_video_universe: bool,
    pub video_burn_in: bool,
    pub workspace_dir: String,
    /// The level files and clips are auditioned at (Workspace, Clip List), in dB.
    pub preview_db: f32,
    /// The device output pair files and clips are auditioned on: its first channel (0: outputs
    /// 1–2).
    pub preview_output: usize,
    /// Files are auditioned at the session tempo when they say their own (Workspace).
    pub preview_tempo: bool,
    /// Files are auditioned in sync with the session while it plays (Workspace).
    pub preview_sync: bool,
    /// The user's fade presets (Edit › Fades › Create…).
    pub fade_presets: Vec<fade_dialog::FadePreset>,
    pub configurations: Vec<(String, Value)>,
    /// Appearance (`ui.theme`): dark by default, light or follow the system on request.
    pub theme: theme::ThemeMode,
    /// The main window's size (logical points) and maximized state, and the interface scale, as
    /// the desktop app last saw them; it restores them at launch.
    pub window_size: [f32; 2],
    pub window_maximized: bool,
    pub ui_scale: f32,
    /// Appearance › "Show splash screen at launch" (the desktop app's six-second film; off for
    /// automation regardless, see the app's `splash` module).
    #[serde(default = "default_true")]
    pub show_splash: bool,
    /// What the splash remembers between launches (`splash.last_statement`: the statement shown
    /// last, so the next launch picks another).
    pub splash: SplashMemory,
    /// The Claude model Scout asks (not secret; the API key is never here).
    pub scout_model: String,
    /// Where the ML jobs run (Setup › ML Models): `auto`, `gpu` or `cpu`.
    pub ml_device: String,
    /// The plugin browser panel is showing.
    pub show_plugin_browser: bool,
    /// The plugin browser's filters, favourites and recents.
    pub browser: browser::BrowserState,
    /// The Library folder (plugin presets, chains, track presets, Juke presets, templates) when
    /// the user moved it, say into a folder a sync service keeps; empty: the settings folder.
    pub library_dir: String,
}

/// The plugin a successful command put on a track, if it was an insert (for the browser's recents).
fn inserted_plugin(id: &str, params: &Value) -> Option<String> {
    let key = match id {
        "mix.instrument" | "mix.module_add" | "mix.insert" => "plugin",
        "track.new" => "instrument",
        _ => return None,
    };
    params.get(key).and_then(Value::as_str).filter(|p| !p.is_empty()).map(str::to_string)
}

/// Mix window sections added after settings files existed: switched on once for settings saved
/// before them ([`UiState::offer_new_mix_views`]).
pub const NEW_MIX_VIEWS: &[&str] = &["pitch"];

/// Why the audio device cannot change while a render runs (Setup › Playback Engine › Apply,
/// Reconnect audio device): the render holds the player's live plugin instances.
pub const RENDER_HOLDS_DEVICE: &str = "Finish or cancel the render first";

impl UiState {
    /// Switches on, once, each of [`NEW_MIX_VIEWS`] these settings were saved without (an older
    /// `ui.json`), and records it, so a section the user turns off afterwards stays off. Returns
    /// whether anything changed. The host calls it on loading `ui.json`.
    pub fn offer_new_mix_views(&mut self) -> bool {
        let mut changed = false;
        for v in NEW_MIX_VIEWS {
            if self.mix_views_offered.iter().any(|o| o == v) {
                continue;
            }
            if !self.mix_views.iter().any(|m| m == v) {
                self.mix_views.push((*v).to_string());
            }
            self.mix_views_offered.push((*v).to_string());
            changed = true;
        }
        changed
    }
}

impl Default for UiState {
    fn default() -> Self {
        UiState {
            recent_sessions: Vec::new(),
            docking: docking::Workspace::default(),
            mix_docking: docking::mix_default(),
            window: MainWindow::Edit,
            show_tracks_list: true,
            show_clip_list: true,
            show_transport: false,
            show_memory_locations: false,
            show_big_counter: false,
            show_loudness: false,
            show_mix_snapshots: false,
            show_control_room: false,
            talk_held: false,
            talk_button: false,
            talk_latched: false,
            show_undo_history: false,
            show_notes: false,
            notes_show: "open".into(),
            show_lyrics: false,
            tour_step: None,
            lyrics_syllables: true,
            lyrics_rhymes: true,
            lyrics_rhyme_kinds: default_rhyme_kinds(),
            lyrics_stress: false,
            lyrics_fit: true,
            show_teleprompter: false,
            teleprompter_separate: false,
            teleprompter_window: None,
            show_session_info: false,
            show_audio_health: false,
            show_about: false,
            show_midi_editor: false,
            show_vocal_editor: false,
            vocal_preview: true,
            vocal_zoom_factor: 1.0,
            vocal_row_h: vocal_editor::ROW_H,
            show_universe: false,
            narrow_mix: false,
            mix_width_all: false,
            wide_editors: true,
            slim_editors: false,
            editor_scale: 1.0,
            wide_key_owner: HashMap::new(),
            mix_views: vec!["inserts_ae".into(), "sends_ae".into(), "io".into(), "color".into(), "pitch".into()],
            mix_views_offered: NEW_MIX_VIEWS.iter().map(|v| v.to_string()).collect(),
            edit_views: vec!["color".into()],
            track_tint: 0.5,
            plugin_windows: Vec::new(),
            full_editors: Vec::new(),
            detached_plugins: Vec::new(),
            pinned_plugins: Vec::new(),
            following_plugins: Vec::new(),
            modulators_window: None,
            plugin_window_rects: HashMap::new(),
            juke_tabs: HashMap::new(),
            juke_clips: HashMap::new(),
            hit_picker: None,
            hit_playing: None,
            juke_pad_select: None,
            drawers: HashMap::new(),
            band_drawers: HashSet::new(),
            gr_history: HashMap::new(),
            cq_learn: HashMap::new(),
            preset_dirty: HashMap::new(),
            drawer_tab: None,
            ruler_item: None,
            loop_drag: None,
            ruler_pick_sel: None,
            send_window: None,
            audiosuite: None,
            status: String::new(),
            show_automation: false,
            show_color_palette: false,
            show_disk_usage: false,
            show_system_usage: false,
            show_task_manager: false,
            show_metadata: false,
            show_event_list: false,
            show_midi_keyboard: false,
            show_workspace: false,
            show_configurations: false,
            show_playback_engine: false,
            audio: soundcraft_playback::AudioConfig::default(),
            record_offsets: std::collections::BTreeMap::new(),
            mix_separate: false,
            vocal_separate: false,
            midi_editor_separate: false,
            midi_editor_window: None,
            vocal_window: None,
            sync: window_sync::SyncOptions::default(),
            show_io_setup: false,
            show_midi_inputs: false,
            show_surfaces: false,
            show_midi_maps: false,
            midi_transport_maps: Vec::new(),
            surfaces: Vec::new(),
            show_network: false,
            midi_inputs: std::collections::BTreeMap::new(),
            midi_outputs: std::collections::BTreeMap::new(),
            midi_aliases: std::collections::BTreeMap::new(),
            midi_offsets: std::collections::BTreeMap::new(),
            midi_thru: true,
            show_shortcuts: false,
            show_preferences: false,
            preferences_tab: String::new(),
            meter_release_db_s: preferences::DEFAULT_RELEASE,
            meter_hold_secs: preferences::DEFAULT_HOLD,
            meter_scale: String::new(),
            copy_imported_audio: true,
            plugin_process: false,
            monitor_ear: false,
            monitor_ear_profile: String::new(),
            monitor_sim: false,
            monitor_sim_profile: String::new(),
            show_ear_profile: false,
            show_hearing_test: false,
            shortcuts: shortcuts::Overrides::new(),
            shortcut_editor: shortcut_editor::EditorState::default(),
            show_clip_effects: false,
            show_beat_detective: false,
            show_tempo_ops: false,
            show_time_ops: false,
            show_midi_ops: false,
            show_rtp: false,
            show_input_quantize: false,
            show_click_countoff: false,
            show_disk_allocation: false,
            show_midi_offsets: false,
            show_score: false,
            show_search: false,
            show_video: false,
            show_video_universe: false,
            video_burn_in: false,
            workspace_dir: String::new(),
            preview_db: 0.0,
            preview_output: 0,
            preview_tempo: true,
            preview_sync: true,
            fade_presets: Vec::new(),
            configurations: Vec::new(),
            theme: theme::ThemeMode::Dark,
            window_size: [1600.0, 1000.0],
            window_maximized: false,
            ui_scale: 1.0,
            show_splash: true,
            splash: SplashMemory::default(),
            scout_model: scout::ask::DEFAULT_MODEL.into(),
            ml_device: "auto".into(),
            show_plugin_browser: false,
            browser: browser::BrowserState::default(),
            library_dir: String::new(),
        }
    }
}

/// Host services (file dialogs, Scout's Claude call) injected by the app; absent in tests and on
/// the web.
#[derive(Default)]
pub struct Services {
    pub pick_open: Option<Box<dyn Fn(&str, &[&str]) -> Option<String>>>,
    pub pick_save: Option<Box<dyn Fn(&str, &str) -> Option<String>>>,
    pub pick_folder: Option<Box<dyn Fn(&str) -> Option<String>>>,
    /// Sends a Scout question to Claude, streaming the answer back (native only).
    pub ask_claude: Option<scout::ask::AskClaude>,
    /// The Anthropic API key, masked ("sk-ant-…a1b2"), or `None` without one. Never the key.
    pub api_key_status: Option<scout::ask::KeyStatus>,
    /// Stores the key in the host's settings (`None` removes it). The key never enters `UiState`
    /// or the session.
    pub set_api_key: Option<scout::ask::SetKey>,
    /// Where hosted plugins' browser pictures are stored (`<config dir>/PluginThumbs`); `None`
    /// keeps them in memory only.
    pub thumb_dir: Option<std::path::PathBuf>,
    /// Scout's sound-profile library (`<config dir>/profiles`); `None` uses the engine's default
    /// (the settings folder).
    pub profiles_dir: Option<std::path::PathBuf>,
    /// Setup › Network's settings file (`<config dir>/network.json`); `None` keeps them in memory.
    pub network_file: Option<std::path::PathBuf>,
}

/// File-system-safe folder name for a plugin id (`clap:com.x.y` → `clap_com.x.y`).
pub fn preset_folder_name(plugin: &str) -> String {
    plugin.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '-' || c == '_' { c } else { '_' }).collect()
}

/// On the first frame, maximizes a window too big for the screen it opened on (sized on a larger
/// monitor) instead of letting it run off the edge; its saved size is kept for that monitor.
fn default_zoom_factor() -> f32 {
    1.0
}

fn default_vocal_row_h() -> f32 {
    vocal_editor::ROW_H
}

fn default_true() -> bool {
    true
}

fn default_rhyme_kinds() -> u8 {
    0x0f
}

fn default_meter_release() -> f32 {
    preferences::DEFAULT_RELEASE
}

fn default_meter_hold() -> f32 {
    preferences::DEFAULT_HOLD
}

pub fn fit_window_to_screen(ctx: &egui::Context) {
    let (inner, monitor) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().monitor_size));
    if ctx.cumulative_pass_nr() == 0 && inner.zip(monitor).is_some_and(|(r, m)| (r.size() - m).max_elem() > 0.0) {
        ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
    }
}

/// Drag-and-drop payload: an audio file (source id) dragged from the Clip List.
#[derive(Debug, Clone, Copy)]
pub struct DragSource(pub u64);

/// Drag-and-drop payload: a clip (its id) dragged from the Clip List; a copy lands where it is
/// dropped.
#[derive(Debug, Clone, Copy)]
pub struct DragClip(pub u64);

/// Drag-and-drop payload: a clip captured into the Clip List (Clip › Capture); a copy lands
/// where it is dropped.
#[derive(Debug, Clone, Copy)]
pub struct DragCaptured(pub u64);

/// Ballistic meter display state per strip.
#[derive(Debug, Clone, Copy, Default)]
pub struct MeterDisplay {
    /// What the bar shows: the peak, or the RMS level on an RMS scale (`widgets::MeterScale`).
    pub level: [f32; 2],
    pub hold: [f32; 2],
    /// The RMS level (linear), averaged over 300 ms.
    pub rms: [f32; 2],
    /// The mean square behind `rms`, smoothed.
    pub ms: [f32; 2],
    pub hold_age: f32,
    pub clip: bool,
    /// The highest peak (linear) since the last reset: the strip's peak-hold readout.
    pub peak: f32,
    pub gr: f32,
    /// [`soundcraft_mix::PluginFailures::count`] as last seen.
    pub plugin_failures: u32,
    /// Feedback protection silenced the track's monitored input (`StripMeter::feedback`).
    pub feedback: bool,
    /// MIDI activity: the loudest note-on velocity its instrument was given lately (0 to 1),
    /// falling back over about half a second (`StripMeter::midi_velocity`).
    pub midi: f32,
}

/// An in-progress mouse gesture in the Edit window.
#[derive(Debug, Clone)]
pub enum Gesture {
    Select {
        track: TrackId,
        anchor: Samples,
        tracks: Vec<TrackId>,
    },
    MoveClips {
        clips: Vec<soundcraft_model::ClipId>,
        grab_at: Samples,
        delta: Samples,
        from_track: TrackId,
        to_track: TrackId,
        /// Ctrl at the press: the drop duplicates the clips (`clip.copy_to`); `keep_time` (Alt)
        /// puts the copies at the clips' own time.
        copy: bool,
        keep_time: bool,
    },
    /// A trim of a clip's start (`stretch`: Alt held at the press, the material stretches to fill).
    TrimStart {
        clip: soundcraft_model::ClipId,
        track: TrackId,
        to: Samples,
        stretch: bool,
    },
    /// A trim of a clip's end (`stretch` as for the start; `looped`: Shift held at the press, the
    /// loop trim: the clip's material repeats to the new end, [`soundcraft_model::Clip::loop_len`]).
    TrimEnd {
        clip: soundcraft_model::ClipId,
        track: TrackId,
        to: Samples,
        stretch: bool,
        looped: bool,
    },
    Fade {
        clip: soundcraft_model::ClipId,
        track: TrackId,
        fade_in: bool,
        to: Samples,
    },
    /// A crossfade drawn by the Smart Tool from where two clips meet: centred on `edge`, as
    /// wide as the drag to `to` either way.
    Crossfade {
        track: TrackId,
        edge: Samples,
        to: Samples,
    },
    FadeCurve {
        clip: soundcraft_model::ClipId,
        fade_in: bool,
        curvature: f32,
        anchor: (f32, f32),
        height: f32,
    },
    /// The Scrubber dragged: where it was last frame, and the tracks it plays (`scrub`).
    Scrub {
        last: Samples,
        tracks: Vec<TrackId>,
    },
    Fader {
        track: TrackId,
        start_db: f32,
    },
    /// A Grabber drag from an empty part of the timeline: the clips the box touches, on every
    /// track it crosses, are selected (with Shift held at the press, added to the selection).
    ClipBand {
        origin: egui::Pos2,
        to: egui::Pos2,
        /// The clips selected before (Shift), kept under the band's.
        keep: Vec<soundcraft_model::ClipId>,
    },
    /// A warp marker dragged on a track showing Warp: marker `index` of `clip` goes to `to` on
    /// release (the material re-stretches then).
    WarpMarker {
        clip: soundcraft_model::ClipId,
        index: usize,
        to: Samples,
    },
    /// A clip-gain line breakpoint dragged (made where the line was pressed if none was there).
    GainPoint {
        clip: soundcraft_model::ClipId,
        /// Its offset from the clip start as it now stands.
        at: Samples,
    },
    ClipGain {
        clip: soundcraft_model::ClipId,
        track: TrackId,
        pos: f32,
        anchor: (f32, f32),
        fine: bool,
        fader_bottom: egui::Pos2,
    },
    /// A Pencil pass; `line` (Shift at the press) keeps only its first and last point.
    Pencil {
        track: TrackId,
        /// The automation it draws (`None`: a Pencil drag that draws none).
        param: Option<soundcraft_model::AutoParam>,
        points: Vec<(Samples, f32)>,
        line: bool,
    },
    Breakpoint {
        track: TrackId,
        param: soundcraft_model::AutoParam,
        at: Samples,
    },
    /// Dragging a selected breakpoint: all the selected ones move together. `grab` is the
    /// grabbed point's original time and the press; `moved` what has been applied so far.
    MovePoints {
        track: TrackId,
        param: soundcraft_model::AutoParam,
        points: Vec<Samples>,
        grab: (Samples, egui::Pos2),
        moved: (Samples, f32),
    },
    /// Dragging a segment's midpoint handle up or down bends it (`at`: the segment's first point).
    Curve {
        track: TrackId,
        param: soundcraft_model::AutoParam,
        at: Samples,
        origin_y: f32,
        start: f32,
        rising: bool,
    },
    /// Rubber-banding breakpoints on an automation lane.
    RubberBand {
        track: TrackId,
        param: soundcraft_model::AutoParam,
        origin: egui::Pos2,
        to: egui::Pos2,
    },
    MidiClip {
        track: TrackId,
        from: Samples,
        to: Samples,
    },
    ZoomBox {
        start: f32,
    },
}

/// How long after a refused Stop a second one stops a locked take (seconds).
pub const RECORD_LOCK_SECONDS: f64 = 2.0;

/// The selected automation breakpoints of one lane (times), kept by the UI: clicked, rubber-banded
/// or inside the edit selection. Delete removes them; dragging one moves them all.
#[derive(Debug, Clone, PartialEq)]
pub struct AutoSelection {
    pub track: TrackId,
    pub param: soundcraft_model::AutoParam,
    pub points: Vec<Samples>,
}

pub struct SoundApp {
    pub engine: Engine,
    /// The selected automation breakpoints, if any.
    pub auto_sel: Option<AutoSelection>,
    /// The readout shown by the pointer while a breakpoint is dragged ("value · time").
    pub point_readout: Option<String>,
    /// Vocal tuning's background analyses and renders (polled once per frame).
    pub vocal_worker: vocal_worker::VocalWorker,
    /// Stretch renders made in the background while the stretch is heard live.
    pub stretch_worker: stretch_worker::StretchWorker,
    /// The session path as of the last frame, and whether the recent list changed since the host
    /// last saved it (see `recent`).
    recent_seen: Option<String>,
    recent_changed: bool,
    /// When the recent sessions were last checked for existence, which list, and the result.
    recent_checked: Option<(f64, Vec<String>, Vec<bool>)>,
    pub player: Option<Player>,
    /// Hardware MIDI input's rings (lent to each player; native only: `None` on the web).
    pub midi_hub: Option<std::sync::Arc<soundcraft_playback::midi_in::MidiHub>>,
    /// The MIDI input ports (`midi_devices`): the system's once the host sets
    /// `use_system_midi`, or a test's fake.
    pub midi_inputs: Option<soundcraft_playback::midi_in::MidiInputs>,
    /// The MIDI output ports (`midi_devices`): opened as the inputs are.
    pub midi_outputs: Option<soundcraft_playback::midi_out::MidiOutputs>,
    /// The desktop host opens the system's MIDI ports (tests and offscreen renders don't).
    pub use_system_midi: bool,
    /// The current player has been lent the MIDI hub.
    pub(crate) midi_sent: bool,
    /// The MIDI Input Devices window was open last frame (it rescans when it opens).
    pub(crate) midi_window_was_open: bool,
    /// The running control surfaces (`surfaces`).
    pub surfaces: surfaces::Surfaces,
    /// MIDI learn (`midi_learn`): the control waiting for a controller to be moved.
    pub learning: Option<soundcraft_model::MidiTarget>,
    /// The claims last given to the MIDI hub.
    pub(crate) learn_claims: Vec<(usize, u8, u8, u8)>,
    /// The learn menu a right-click opened.
    pub(crate) learn_menu: Option<midi_learn::LearnMenu>,
    /// Transport keys down now (port, channel, number, CC), so one acts once as it goes down.
    pub(crate) learn_switches: std::collections::HashMap<(String, u8, u8, bool), bool>,
    /// The live MIDI the audio thread heard since recording started (`midi_devices::collect`).
    pub(crate) midi_rec: Vec<soundcraft_playback::midi_in::RecordedMidi>,
    /// Loop recording MIDI: the finished passes on the armed tracks while the loop runs
    /// (`midi_devices::preview_loop`).
    pub(crate) midi_loop_preview: Option<soundcraft_engine::midi_record::LoopPreview>,
    /// The hub's count of recorded events lost (the window did not collect them in time) when
    /// the take started: what it counts beyond is this take's loss, reported at Stop.
    pub(crate) midi_rec_lost_at: u64,
    /// What was played on the MIDI inputs during the playback under way (or the last one), not
    /// recorded: Event › More MIDI › Capture MIDI makes a take of it (`midi_devices::capture`).
    pub(crate) midi_capture: Vec<soundcraft_playback::midi_in::RecordedMidi>,
    /// Wait for Note: Record was pressed and the take starts at the first note played (the
    /// length of `midi_capture` when it was last looked at; see `wait_note`).
    pub(crate) wait_note: Option<usize>,
    /// MIDI clock and timecode in, MMC out (`midi_sync_in`).
    pub(crate) sync: midi_sync_in::SyncState,
    /// The last local MIDI take as recorded (events as heard, and where they go), for a remote
    /// recorder to send to the booth (`net_ui`).
    pub(crate) last_midi_take: Option<(soundcraft_engine::midi_record::MidiTake, Vec<soundcraft_engine::midi_record::RecordedEvent>)>,
    /// The last local audio take as recorded (its plan and each track's file), for a remote
    /// recorder to send to the booth (`net_ui`).
    pub(crate) last_audio_take: Option<(soundcraft_engine::io::Recording, Vec<(TrackId, std::path::PathBuf)>)>,
    /// Where the transport last started, and the loop it plays (the take clock's zero).
    pub(crate) play_from: Samples,
    pub(crate) play_loop: Option<Range>,
    /// Where the play under way ends (`None`: plays on), for a Pause to resume it.
    play_end: Option<Samples>,
    /// Paused: where to resume, with the end and loop of the play paused. Any other start or a
    /// locate forgets it.
    pub(crate) paused: Option<(Samples, Option<Samples>, Option<Range>)>,
    /// Varispeed on: the speed every Play runs at (`transport.varispeed`).
    pub varispeed: Option<f32>,
    /// Shuttle under way: its speed (negative: backwards, played as the frames pass through the
    /// preview voice) and, going backwards, when the last frame's audio was sent.
    pub shuttle: f32,
    shuttle_at: Option<std::time::Instant>,
    /// A Half-Speed Play is starting: it keeps its half speed whatever Varispeed says.
    half_speed_play: bool,
    /// A file or clip auditioning (`audition`).
    pub audition: audition::Audition,
    /// Loop Playback and the loop range as they were when the loop playing was last decided (at
    /// Play, or the last change while playing): a change while playing moves the loop.
    loop_basis: (bool, Option<Range>),
    /// The count-off the last Record started with (Options › Count Off): the transport counts
    /// it off before it rolls from `play_from` (`SoundApp::countoff_status`).
    pub(crate) countoff: Option<soundcraft_playback::CountOff>,
    /// Setup › Network: the LAN listener, the remote recorder link and their per-frame glue.
    #[cfg(not(target_arch = "wasm32"))]
    pub net: Option<net_ui::NetState>,
    /// Musical Typing: its window's keys and the computer keys as a live MIDI input.
    pub typing: musical_typing::TypingState,
    /// Chord Pads' window and the pad held.
    pub pads: chord_pads::PadState,
    /// Event › More MIDI › Step Input: its window and the chord being played.
    pub step: step_input::StepInput,
    /// Input capture, opened on first record.
    pub recorder: Option<soundcraft_playback::record::Recorder>,
    record_start: Samples,
    /// Punch range to trim the take to (pre/post-roll recordings).
    punch: Option<Range>,
    recorder_failed: bool,
    /// A player was (re)opened and whether it opened the chosen device is still to be told
    /// (checked once in `logic`, with the audio settings loaded by then).
    device_notice_pending: bool,
    /// The output device that went away, watched for so the app reconnects when it is back.
    device_return: Option<device_return::Returning>,
    /// Input this take lost to a slow disk, as last reported (ms).
    lost_shown: u64,
    /// Where the takes being recorded go, autosaved with the session so a crash can't lose them.
    take_plan: Option<soundcraft_engine::io::Recording>,
    pub ui: UiState,
    pub audio_health_report: Option<Result<Value, String>>,
    pub services: Services,
    /// Set only by a desktop host after installing an OS menu bar.
    pub native_menu_bar: bool,
    /// Folder for crash-recovery autosaves (native apps set it).
    pub autosave_dir: Option<std::path::PathBuf>,
    /// Writes the open session's autosaves in the background, once there is something to write.
    autosaver: Option<soundcraft_engine::autosave::Autosaver>,
    /// The document revision last handed to the autosaver (0 once that autosave is stale).
    autosaved_rev: u64,
    /// Unsaved work that earlier runs left in the autosave folder, offered for recovery.
    pub leftovers: Vec<soundcraft_engine::autosave::Leftover>,
    /// The document revision last autosaved after a caught panic.
    panic_rev: u64,
    /// Folder for user plugin presets (native apps set it).
    pub preset_dir: Option<std::path::PathBuf>,
    /// The Library folder when the preferences name none: the settings folder (native apps set
    /// it, then call [`SoundApp::apply_library_dir`]).
    pub library_default: Option<std::path::PathBuf>,
    last_autosave: Option<std::time::Instant>,
    /// PipeWire's sources and the "All sources" channels in effect (`None` off "All sources"),
    /// looked up at most every two seconds: I/O Setup asks every frame.
    #[cfg(not(target_arch = "wasm32"))]
    all_sources: Option<(std::time::Instant, Option<AllSources>)>,
    pub dialogs: dialogs::Dialogs,
    pub meters: HashMap<TrackId, MeterDisplay>,
    pub main_meter: MeterDisplay,
    /// Reused multichannel snapshot; populated at an adaptive rate by `logic`.
    meter_snapshot: soundcraft_playback::MeterSnapshot,
    pub gesture: Option<Gesture>,
    /// The edit a drag makes on release joins the undo step before it (a separation grab's
    /// move joins its separate).
    pub join_next_edit: bool,
    /// Who a solo leaves playing, as of a session revision: computed once for every strip and
    /// header ([`SoundApp::solo_scope`]).
    solo_cache: Option<(u64, std::sync::Arc<soundcraft_model::SoloScope>)>,
    pub edit_layout: edit_window::EditLayout,
    /// What keeps the Edit and Mix windows in step (`window_sync`).
    pub window_sync: window_sync::WindowSync,
    pub midi: midi_editor::MidiEditorState,
    /// The Score Editor's note value, accidental and step-entry cursor.
    pub score: score_editor::ScoreState,
    /// The Vocal Editor's selection, scroll and panel state.
    pub vocal: vocal_editor::VocalEditorState,
    /// While a Vocal Editor preview plays: the track the player hears soloed (it is handed a
    /// copy of the session with only that track soloed; the document is untouched).
    pub preview_solo: Option<TrackId>,
    /// While a Convert to MIDI audition plays: the copy of the session the player hears (the
    /// notes on screen on the instrument track Convert would make, soloed with the source).
    pub audition_session: Option<std::sync::Arc<soundcraft_model::Session>>,
    pub ops: ops_windows::OpsState,
    pub extra: extra_windows::ExtraState,
    pub palette: palette::PaletteState,
    /// Bring the separate Mix window to the front on the next frame.
    pub(crate) focus_mix: bool,
    /// Bring the separate Vocal Editor window forward on its next frame.
    pub(crate) focus_vocal: bool,
    /// Bring the separate MIDI Editor window forward on its next frame.
    pub(crate) focus_midi_editor: bool,
    pub synthetic: Vec<egui::Event>,
    /// Where a dropped audio file loads into a track's sampler (last frame's rects).
    pub sample_drop_targets: Vec<(egui::Rect, TrackId)>,
    /// Juke pad slots `(pad, layer, robin)` drawn this frame that take a dropped audio file.
    pub kit_drop_targets: Vec<(egui::Rect, TrackId, (usize, usize, usize))>,
    /// Places dropped drum takes are imported: a pad card (`Some(pad)`: the takes become the
    /// pad's layers and round-robins) or a Juke window (`None`: a kit, the takes to pads by name).
    pub kit_import_targets: Vec<(egui::Rect, TrackId, Option<usize>)>,
    /// This frame's places a dragged browser card can land (see `browser::drop`).
    pub(crate) plugin_drop_targets: Vec<browser::drop::Registered>,
    sample_reads: Vec<sampler_window::SampleRead>,
    synthetic_grace: u32,
    control_rx: Option<Receiver<ControlRequest>>,
    pending_shots: Vec<control::PendingShot>,
    /// An insert just picked from its menu, opened once the audio engine has its instance.
    pub(crate) pending_open: Option<(TrackId, soundcraft_model::ModuleId)>,
    /// The wide editors' own state, per editor, track and module (`editor_kit::run`): boxed once
    /// and taken out and put back each frame without re-boxing.
    pub(crate) editor_states: HashMap<egui::Id, Box<dyn std::any::Any + Send>>,
    last_rev: u64,
    /// The monitor stage's state on this side (`monitor_ui`).
    pub monitor: monitor_ui::MonitorUi,
    /// The hearing test under way (`hearing_ui`).
    pub hearing: Option<hearing_ui::HearingTest>,
    /// The band Library last handed to the player (its address; 0 = none yet).
    band_lib_key: usize,
    fonts_ready: bool,
    fonts_installed: bool,
    /// The palette last applied to the context (light?), so visuals are set only on a change.
    theme_applied: Option<bool>,
    /// Last panel host context, used to cancel renderer-owned gestures on explicit replacement.
    pub(crate) docking_context: Option<egui::Context>,
    /// Transport simulation when there is no player (tests, offscreen renders).
    sim: Option<(Samples, Option<Samples>, Option<Range>)>,
    /// When Record last found nothing to record on, and why: the Record buttons show it for a moment.
    record_hint: Option<(f64, &'static str)>,
    /// Options › Recording › Lock Record: when a Stop was refused while a locked take records (a
    /// second Stop within [`RECORD_LOCK_SECONDS`] stops it).
    record_lock_stop: Option<f64>,
    /// The free space on the takes' disk and when it was read (`None` inside: unknown).
    disk_free: Option<(f64, Option<u64>)>,
    /// The record offset key a loopback measurement out there was started with.
    measuring: Option<String>,
    /// The latencies of the take recording now, snapshotted at its first frame.
    take_reported: Option<soundcraft_playback::Latency>,
    pub quit_requested: bool,
    /// Reset floating-window positions next frame (Window › Arrange).
    pub arrange_request: bool,
    /// The automatable controls the pointer holds this frame (a fader, a knob being dragged):
    /// they stay touched for automation writing while held, moving or not ([`SoundApp::hold_control`]).
    held_controls: Vec<(TrackId, soundcraft_model::AutoParam)>,
    pub frame_ms: f32,
    last_frame: Option<f64>,
    last_meter_poll: f64,
    /// Movies for the Video window and Video track (decoded in the background).
    pub video: video_track::VideoPool,
    /// The Bounce Mix in progress, rendered a slice per frame.
    pub bounce: Option<soundcraft_engine::io::Bounce>,
    /// A real-time bounce's pass: the live player prints the range while it plays it, then
    /// `bounce` writes the print (see [`bounce::start`]).
    pub print: Option<bounce::PrintPass>,
    /// Export › Stems in progress, rendered a slice per frame (see [`bounce::start_stems`]).
    pub stems: Option<soundcraft_engine::io::StemsJob>,
    /// The movie bounce (File › Export › Movie) in progress, a few frames per frame.
    #[cfg(not(target_arch = "wasm32"))]
    pub movie: Option<soundcraft_engine::movie_export::MovieBounce>,
    /// The Freeze / Commit / Bounce render running (see [`render`]).
    pub(crate) track_render: Option<soundcraft_engine::render_job::RenderJob>,
    /// Edit › Split into Files copying behind its progress window (see [`split_progress`]).
    pub(crate) split: Option<split_progress::Running>,
    /// The player's live instances on their way to the running render (see [`render::lend`]).
    pub(crate) lending: Option<soundcraft_playback::LendingInstances>,
    /// The project opening behind the progress window (see [`open_progress`]).
    pub open: Option<open_progress::OpenJob>,
    /// The last DAWproject import's report, while its window is open ([`dawproject_report`]).
    pub dawproject_report: Option<soundcraft_engine::dawproject::Report>,
    /// Local ML jobs (desktop only): the dialog, the job on its worker, the Models window.
    #[cfg(not(target_arch = "wasm32"))]
    pub ml: ml_ui::MlUi,
    /// Juke's recorded kits: the installed ones, the kits release's list and its downloads.
    #[cfg(not(target_arch = "wasm32"))]
    pub juke_kits: juke_kits_ui::KitDownloads,
    /// Event › Detect Tempo…: the dialog and its analysis worker.
    pub tempo: tempo_ui::TempoUi,
    /// Scout: the panel, and the capture out on the audio side.
    pub scout: scout::ScoutUi,
    /// Window ▸ Studio: the band's song-wide controls (not saved).
    pub studio: band::studio::StudioUi,
    /// The plugin browser's catalog: built when the panel is first shown and again after a
    /// scan, never per frame.
    pub browser_catalog: Option<browser::catalog::Catalog>,
    /// Which of the CLAP and VST3 hosts had scanned when the catalog was built.
    pub(crate) browser_hosts: (bool, bool),
    /// The plugin scan running in the background (out of process), if any.
    pub plugin_scan: Option<browser::scan_job::ScanJob>,
    /// The browser panel's viewport and rectangle this frame (`None` while it is hidden): typing
    /// over it, in that viewport, goes to its search field rather than to the single-key commands.
    pub browser_rect: Option<(egui::ViewportId, egui::Rect)>,
    /// Chip lists for the browser's current catalog, tab and category overrides.
    pub(crate) browser_chips: Option<browser::panel::ChipCache>,
    /// The Presets tab's list for the selected plugin.
    pub(crate) browser_presets: Option<browser::presets::PresetCache>,
    /// The Library tab's list as last read from its folder.
    pub(crate) browser_library: Option<browser::library::LibraryCache>,
    /// The module last opened (drawer, plugin window or the plugin's own editor), which the
    /// Presets tab follows, and the open module it last saw (see `browser::presets::follow_focus`).
    pub(crate) module_focus: Option<(TrackId, SlotKey)>,
    pub(crate) browser_focus_seen: Option<(TrackId, SlotKey)>,
    /// What the browser panel did on its last frame (tests and diagnostics).
    pub browser_debug: browser::panel::BrowserDebug,
    /// The browser cards' pictures, as textures for the cards drawn so far.
    pub browser_thumbs: browser::thumbs::ThumbCache,
    /// Thumbnail/screenshot harness only — `Some` overrides live meters: the Mix window's strips,
    /// drawers and editors show these values instead of the player's, for offscreen renders
    /// without audio (the browser's pictures, `examples/plugin_thumb.rs`).
    #[doc(hidden)]
    pub preview_meters: Option<soundcraft_playback::MeterSnapshot>,
    /// Test-only: owns the process-wide palette lock while this app exists, so frames painted
    /// and colour assertions in one test can never straddle another test's palette change.
    #[cfg(test)]
    _palette_lock: theme::TestPaletteGuard,
}

/// Why this build cannot record, if it cannot: the browser build has no audio input yet.
pub(crate) fn record_unavailable(web: bool) -> Option<&'static str> {
    web.then_some("The browser build cannot record yet: record in the desktop app (Linux, macOS, Windows or FreeBSD) and open the session here.")
}

impl SoundApp {
    pub fn new(engine: Engine, player: Option<Player>, services: Services) -> Self {
        let thumb_dir = services.thumb_dir.clone();
        SoundApp {
            recent_seen: engine.path.clone(),
            recent_changed: false,
            recent_checked: None,
            engine,
            vocal_worker: vocal_worker::VocalWorker::new(),
            stretch_worker: stretch_worker::StretchWorker::default(),
            player,
            #[cfg(not(target_arch = "wasm32"))]
            midi_hub: Some(soundcraft_playback::midi_in::MidiHub::new()),
            #[cfg(target_arch = "wasm32")]
            midi_hub: None,
            midi_inputs: None,
            midi_outputs: None,
            use_system_midi: false,
            midi_sent: false,
            midi_window_was_open: false,
            surfaces: surfaces::Surfaces::default(),
            learning: None,
            learn_claims: Vec::new(),
            learn_menu: None,
            learn_switches: std::collections::HashMap::new(),
            midi_rec: Vec::new(),
            wait_note: None,
            sync: midi_sync_in::SyncState::default(),
            midi_loop_preview: None,
            midi_rec_lost_at: 0,
            midi_capture: Vec::new(),
            last_midi_take: None,
            last_audio_take: None,
            play_from: 0,
            play_loop: None,
            play_end: None,
            paused: None,
            audition: audition::Audition::default(),
            varispeed: None,
            shuttle: 0.0,
            shuttle_at: None,
            half_speed_play: false,
            loop_basis: (false, None),
            countoff: None,
            #[cfg(not(target_arch = "wasm32"))]
            net: None,
            typing: musical_typing::TypingState::default(),
            pads: chord_pads::PadState::default(),
            step: step_input::StepInput::default(),
            recorder: None,
            record_start: 0,
            punch: None,
            recorder_failed: false,
            device_notice_pending: true,
            device_return: None,
            lost_shown: 0,
            take_plan: None,
            ui: UiState::default(),
            audio_health_report: None,
            services,
            native_menu_bar: false,
            autosave_dir: None,
            autosaver: None,
            autosaved_rev: 0,
            leftovers: Vec::new(),
            panic_rev: 0,
            preset_dir: None,
            library_default: None,
            last_autosave: None,
            #[cfg(not(target_arch = "wasm32"))]
            all_sources: None,
            dialogs: dialogs::Dialogs::default(),
            meters: HashMap::new(),
            main_meter: MeterDisplay::default(),
            meter_snapshot: soundcraft_playback::MeterSnapshot::default(),
            gesture: None,
            join_next_edit: false,
            auto_sel: None,
            point_readout: None,
            solo_cache: None,
            edit_layout: edit_window::EditLayout::default(),
            window_sync: window_sync::WindowSync::default(),
            midi: midi_editor::MidiEditorState::default(),
            score: score_editor::ScoreState::default(),
            vocal: vocal_editor::VocalEditorState::default(),
            preview_solo: None,
            audition_session: None,
            ops: ops_windows::OpsState::default(),
            extra: extra_windows::ExtraState::default(),
            palette: palette::PaletteState::default(),
            focus_mix: false,
            focus_vocal: false,
            focus_midi_editor: false,
            synthetic: Vec::new(),
            sample_drop_targets: Vec::new(),
            kit_drop_targets: Vec::new(),
            kit_import_targets: Vec::new(),
            plugin_drop_targets: Vec::new(),
            sample_reads: Vec::new(),
            synthetic_grace: 0,
            control_rx: None,
            pending_shots: Vec::new(),
            pending_open: None,
            editor_states: HashMap::new(),
            last_rev: 0,
            band_lib_key: 0,
            monitor: monitor_ui::MonitorUi::default(),
            hearing: None,
            fonts_ready: false,
            fonts_installed: false,
            theme_applied: None,
            docking_context: None,
            sim: None,
            record_hint: None,
            record_lock_stop: None,
            disk_free: None,
            measuring: None,
            take_reported: None,
            quit_requested: false,
            arrange_request: false,
            held_controls: Vec::new(),
            frame_ms: 0.0,
            last_frame: None,
            last_meter_poll: f64::NEG_INFINITY,
            video: video_track::VideoPool::default(),
            bounce: None,
            print: None,
            stems: None,
            #[cfg(not(target_arch = "wasm32"))]
            movie: None,
            track_render: None,
            split: None,
            lending: None,
            open: None,
            dawproject_report: None,
            #[cfg(not(target_arch = "wasm32"))]
            ml: ml_ui::MlUi::default(),
            #[cfg(not(target_arch = "wasm32"))]
            juke_kits: juke_kits_ui::KitDownloads::default(),
            tempo: tempo_ui::TempoUi::default(),
            scout: scout::ScoutUi::default(),
            studio: band::studio::StudioUi::default(),
            browser_catalog: None,
            browser_hosts: (false, false),
            plugin_scan: None,
            browser_rect: None,
            browser_chips: None,
            browser_presets: None,
            browser_library: None,
            module_focus: None,
            browser_focus_seen: None,
            browser_debug: Default::default(),
            browser_thumbs: browser::thumbs::ThumbCache::with_dir(thumb_dir),
            preview_meters: None,
            #[cfg(test)]
            _palette_lock: theme::lock_test_palette(),
        }
    }

    pub fn with_control(mut self, rx: Receiver<ControlRequest>) -> Self {
        self.control_rx = Some(rx);
        self
    }

    /// Run a command; errors go to the status line. Returns the result. A command that replaces
    /// the session (`session.open`, `session.new`, `session.close`, `session.revert`) returns
    /// `{opening: true}` at once and loads behind the progress window ([`open_progress`]).
    pub fn run(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.run_with(id, params, true)
    }

    /// [`SoundApp::run`] for programmatic callers (the control channel): a session-replacing
    /// command runs synchronously and returns once the session is in place, as headless hosts do.
    pub fn run_sync(&mut self, id: &str, params: Value) -> Result<Value, String> {
        self.run_with(id, params, false)
    }

    fn run_with(&mut self, id: &str, mut params: Value, open_behind_window: bool) -> Result<Value, String> {
        // Zoom to Selection and Fill Window by key or menu: sized to the timeline on screen, not
        // to the commands' 1200-pixel default (parity bug 23).
        if matches!(id, "view.zoom_to_selection" | "view.zoom_fit")
            && let Some(o) = params.as_object_mut()
            && !o.contains_key("width_px")
        {
            let [x0, _, x1, _] = self.edit_layout.timeline;
            if x1 - x0 >= 100.0 {
                o.insert("width_px".into(), serde_json::json!(x1 - x0));
            }
        }
        // Loop recording MIDI with a pass finished: Undo takes the last pass back.
        if id == "edit.undo" && midi_devices::loop_undo_applies(self) {
            return midi_devices::undo_last_pass(self);
        }
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "file.import_dawproject");
        // (a running render refuses the session commands below, through `render::blocked`)
        #[cfg(not(target_arch = "wasm32"))]
        if replacing && ml_ui::refuses_session_switch(self) {
            return Err(self.ui.status.clone());
        }
        if self.open.is_some() && open_progress::blocked(id) {
            self.ui.status = open_progress::OPENING.into();
            return Err(self.ui.status.clone());
        }
        if self.edit_waits(id) {
            return Err(self.ui.status.clone());
        }
        if self.rendering() && render::blocked(id) {
            self.ui.status = render::RENDERING.into();
            return Err(self.ui.status.clone());
        }
        if let Some(r) = menus::run_ui_command(self, id, &params) {
            return r;
        }
        if replacing && open_behind_window && soundcraft_engine::find_command(id).is_some_and(|c| (c.enabled)(&self.engine).is_ok()) {
            return open_progress::start(self, id, &params);
        }
        // Freeze, Commit and Bounce render first, through the live plugin instances, as a job;
        // the command runs once the render is done.
        if matches!(id, "track.freeze" | "track.commit" | "track.bounce")
            && let Some(r) = render::start(self, id, &params)
        {
            return r;
        }
        // Export › Stems from the menu renders behind a progress window, a slice per frame.
        if id == "file.bounce_stems" && open_behind_window {
            return bounce::start_stems(self, &params);
        }
        // Split into Files from the menu copies its pieces behind a progress window first.
        if id == soundcraft_engine::split_files::ID
            && open_behind_window
            && let Some(r) = split_progress::start(self, &params)
        {
            return r;
        }
        if id.starts_with("session.save") || id.starts_with("file.bounce") || matches!(id, "track.freeze" | "track.commit" | "track.bounce") {
            // Third-party plugins keep state the session can't see: store it first.
            self.capture_plugin_states();
        }
        if id == "mix.instrument"
            && let Some(t) = soundcraft_engine::cmd::tracks_required(&self.engine, id, &params).ok().and_then(|t| t.first().copied())
        {
            // Undo of the replacement must give back the instrument as it sounds now.
            self.capture_slot_state(t, SlotKey::Instrument);
        }
        if matches!(id, "mix.module_copy" | "mix.module_move" | "mix.module_remove") {
            // The copy (or the undo of a removal) must hold what is set in the plugin now.
            let track = soundcraft_engine::cmd::tracks_required(&self.engine, id, &params).ok().and_then(|t| t.first().copied());
            let module = params.get("module").and_then(Value::as_u64).and_then(|m| u32::try_from(m).ok());
            if let (Some(t), Some(m)) = (track, module) {
                self.capture_module_state(t, soundcraft_model::ModuleId(m));
            }
        }
        self.run_command(id, params)
    }

    /// A render (Bounce Mix, Freeze, Commit) is running: the transport cannot start, and the
    /// audio device cannot change ([`SoundApp::apply_audio_config`], [`SoundApp::reconnect_audio`]).
    pub fn rendering(&self) -> bool {
        self.bounce.is_some() || self.stems.is_some() || self.track_render.is_some() || self.movie_running()
    }

    /// A movie bounce is running.
    pub fn movie_running(&self) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.movie.is_some()
        }
        #[cfg(target_arch = "wasm32")]
        {
            false
        }
    }

    /// The second half of [`SoundApp::run`]: the command itself and what the UI does after it.
    /// A render job runs its command through this once its renders are ready.
    pub(crate) fn run_command(&mut self, id: &str, params: Value) -> Result<Value, String> {
        let replacing = matches!(id, "session.new" | "session.open" | "session.close" | "session.revert" | "file.import_dawproject");
        match self.engine.execute(id, &params) {
            Ok(v) => {
                // A sidechain the command silenced (or cleared) shows in the status line.
                if let Some(note) = crate::sidechain_picker::result_note(&v) {
                    log::warn!("{id}: {note}");
                    self.ui.status = note;
                }
                // A command that did its work with a caveat says so in the status line.
                if let Some(w) = v.get("warning").and_then(serde_json::Value::as_str) {
                    self.ui.status = w.to_string();
                }
                if id.starts_with("session.save") && !self.engine.is_dirty() {
                    self.release_autosave(false);
                }
                if replacing {
                    self.after_session_replaced();
                }
                if id == "file.import_dawproject" {
                    dawproject_report::offer(self, &v);
                }
                if id == "app.quit" {
                    self.quit_requested = true;
                }
                if id == "options.scrolling" {
                    // Re-selecting a scrolling mode resumes the playhead follow.
                    self.edit_layout.follow_hold = false;
                }
                if id == "mix.module_action" {
                    // A one-off action for the playing instance (LevelUp's Deliver Reset): not
                    // in the session, so the app hands it to the player itself.
                    self.send_module_action(&v);
                }
                if id == "track.new_beat"
                    && let Some(t) = v["track"].as_u64()
                {
                    // A new beat opens on its steps, in the Juke window.
                    let clip = v["clip"].as_u64().map(soundcraft_model::ClipId);
                    juke_window::show_steps(self, TrackId(t), clip);
                }
                if id == "track.freeze" || id == "track.unfreeze" {
                    self.ui.status = freeze_status(&self.engine, &v);
                }
                self.note_placed(id, &params);
                if id == "track.clear_clip_indicators" {
                    self.meters.values_mut().chain([&mut self.main_meter]).for_each(|m| {
                        m.clip = false;
                        m.peak = 0.0;
                    });
                }
                Ok(v)
            }
            Err(e) => {
                self.ui.status = e.to_string();
                log::warn!("{id}: {e}");
                Err(e.to_string())
            }
        }
    }

    /// Advances the project open in progress, if any (see [`open_progress::step`]); `logic` does
    /// this every frame, and a host that keeps `logic` from running (the launch splash) calls it.
    pub fn open_step(&mut self, ctx: &egui::Context) {
        open_progress::step(self, ctx);
    }

    /// What the UI lets go of when the session has just been replaced (open, new, close, revert):
    /// the autosave of the old one, and views of the old session's modules and picks.
    pub(crate) fn after_session_replaced(&mut self) {
        self.release_autosave(false);
        // They belong to the old session's modules; the new one's ids can match.
        self.ui.drawers.clear();
        self.ui.band_drawers.clear();
        self.ui.gr_history.clear();
        self.ui.plugin_windows.clear();
        self.ui.full_editors.clear();
        self.ui.detached_plugins.clear();
        self.ui.juke_tabs.clear();
        self.ui.juke_clips.clear();
        self.ui.hit_picker = None;
        self.engine.take_hits = None;
        // A marker or chord picked in the old session.
        ruler_items::unpick(self);
    }

    /// While an ML job runs, the session it rendered from must not change under it (its results
    /// land where the source was): edits wait; playback, views and windows don't. True (with the
    /// reason in the status line) when command `id` has to wait. Paths that run commands outside
    /// [`SoundApp::run`] ask this too.
    pub(crate) fn edit_waits(&mut self, id: &str) -> bool {
        #[cfg(not(target_arch = "wasm32"))]
        if self.ml.running.is_some() && ml_ui::blocks(id) {
            self.ui.status = ml_ui::BLOCKED.into();
            return true;
        }
        let _ = id;
        false
    }

    /// Records a plugin a successful command just placed as a browser Recent and asks the host to
    /// save it. Every path that places a plugin outside [`SoundApp::run`] calls this too.
    pub(crate) fn note_placed(&mut self, id: &str, params: &Value) {
        if let Some(plugin) = inserted_plugin(id, params) {
            browser::note_inserted(&mut self.ui.browser, &plugin);
            self.recent_changed = true;
        }
    }

    /// The simulated play with no audio engine (tests): `(from, end, looped)`.
    pub fn sim_play(&self) -> Option<(Samples, Option<Samples>, Option<Range>)> {
        self.sim
    }

    pub fn is_playing(&self) -> bool {
        self.engine.transport.playing
    }

    /// Why the last Record press did nothing, for a few seconds and while it still holds.
    pub(crate) fn record_hint(&self) -> Option<&'static str> {
        let (at, why) = self.record_hint?;
        let fresh = self.last_frame.unwrap_or_default() - at < 4.0;
        (fresh && !self.engine.session().tracks.iter().any(|t| t.mixer.record_arm)).then_some(why)
    }

    /// Current transport position (playhead while playing, else the edit insertion).
    pub fn position(&self) -> Samples {
        match self.paused {
            _ if self.engine.transport.playing => self.engine.transport.position,
            Some((at, ..)) => at,
            None => self.engine.session().edit.selection.start,
        }
    }

    /// The session the player is handed: the document, or, while a Vocal Editor preview plays,
    /// a copy with only the previewed track soloed, or, while a Convert to MIDI audition plays,
    /// that copy.
    pub(crate) fn player_session(&self) -> std::sync::Arc<soundcraft_model::Session> {
        if let Some(a) = &self.audition_session {
            return a.clone();
        }
        let Some(solo) = self.preview_solo else { return self.engine.session_arc() };
        let mut s = (*self.engine.session_arc()).clone();
        for t in &mut s.tracks {
            t.mixer.solo = t.id == solo;
        }
        std::sync::Arc::new(s)
    }

    pub(crate) fn start_play(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>) {
        self.start_play_counted(from, end, looped, None);
    }

    /// [`SoundApp::start_play`] after `countoff`: the transport holds at `from` while it clicks.
    /// Without an audio engine (the simulated transport) there is nothing to count off.
    fn start_play_counted(&mut self, from: Samples, end: Option<Samples>, looped: Option<Range>, countoff: Option<soundcraft_playback::CountOff>) {
        if self.rendering() {
            self.ui.status = render::RENDERING.into();
            return;
        }
        self.play_from = from;
        self.play_loop = looped;
        self.play_end = end;
        self.paused = None;
        // A new playback: what Capture MIDI keeps starts again.
        self.midi_capture.clear();
        self.countoff = countoff.filter(|_| self.player.is_some());
        if let Some(p) = &self.player {
            p.set_band_library(self.engine.band_library());
            p.update_session(self.player_session());
            p.play_counted(from, end, looped, countoff);
            // Varispeed applies to plays, never to a take (the player refuses to record off speed 1).
            if let Some(v) = self.varispeed.filter(|_| !self.engine.transport.recording && !std::mem::take(&mut self.half_speed_play)) {
                p.set_keep_pitch(self.engine.session().edit.flag(soundcraft_engine::cmd::transport::KEEP_PITCH));
                p.set_speed(v);
            }
        } else {
            self.sim = Some((from, end, looped));
        }
        self.note_play_started(from);
    }

    /// A shuttle step or speed (`transport.shuttle*`): forwards it is a play at that speed (the
    /// pitch kept when Varispeed Keeps Pitch is on); backwards the transport is stopped and each
    /// frame plays the stretch it went back over ([`SoundApp::shuttle_back_frame`]).
    fn shuttle(&mut self, step: i8, speed: Option<f32>) {
        let was = self.shuttle;
        let next = match (speed, step) {
            (Some(v), _) => v,
            (None, 1) if was > 0.0 => (was * 2.0).min(4.0),
            (None, 1) => 1.0,
            (None, -1) if was < 0.0 => (was * 2.0).max(-8.0),
            (None, -1) => -1.0,
            _ => 0.0,
        };
        let next = if next.is_finite() { next.clamp(-8.0, 4.0) } else { 0.0 };
        self.shuttle = next;
        self.shuttle_at = None;
        if next > 0.0 {
            if !self.is_playing() {
                let at = self.engine.transport.position;
                self.start_play(at, None, None);
            }
            if let Some(p) = &self.player {
                p.set_keep_pitch(self.engine.session().edit.flag(soundcraft_engine::cmd::transport::KEEP_PITCH));
                p.set_speed(next.max(0.1));
            }
            self.ui.status = format!("Shuttle {next}×");
        } else {
            if self.is_playing() && was > 0.0 || next < 0.0 && self.is_playing() {
                self.stop_play();
            }
            if let Some(p) = &self.player {
                p.set_speed(self.varispeed.unwrap_or(1.0));
            }
            self.shuttle_at = (next < 0.0).then(std::time::Instant::now);
            self.ui.status = if next < 0.0 { format!("Shuttle {next}×") } else { "Shuttle stopped".into() };
        }
    }

    /// A backwards shuttle's frame: the playhead goes back by the time since the last frame at
    /// the shuttle's speed, and the audio passed over plays backwards (every audible track's
    /// clips, through the preview voice, as the Scrubber's does). Stops at the session start.
    fn shuttle_back_frame(&mut self) {
        let Some(last) = self.shuttle_at.filter(|_| self.shuttle < 0.0) else { return };
        if self.is_playing() || self.engine.transport.recording {
            (self.shuttle, self.shuttle_at) = (0.0, None);
            return;
        }
        let now = std::time::Instant::now();
        let dt = now.duration_since(last).as_secs_f64().min(0.25);
        let s = self.engine.session_arc();
        let from = self.engine.transport.position;
        let to = (from - s.sample_rate.samples(dt * f64::from(-self.shuttle))).max(0);
        if to == from {
            if from == 0 {
                (self.shuttle, self.shuttle_at) = (0.0, None);
            }
            return;
        }
        self.shuttle_at = Some(now);
        let tracks: Vec<soundcraft_model::TrackId> =
            s.tracks.iter().filter(|t| t.kind.has_playlist() && !t.kind.is_midi() && !t.mixer.mute).map(|t| t.id).collect();
        if let Some(p) = &self.player
            && let Some((audio, rate)) = crate::scrub::chunk(&s, &tracks, from, to, dt.max(1.0 / 120.0) * 1.25)
        {
            p.preview(audio, rate);
        }
        let _ = self.engine.execute("transport.locate", &json!({"at": to}));
    }

    /// The UI side of playback starting at `from` (the player already told).
    fn note_play_started(&mut self, from: Samples) {
        let e = &self.engine.session().edit;
        self.loop_basis = (e.loop_playback, e.loop_range);
        self.engine.transport.playing = true;
        self.engine.transport.position = from;
        self.engine.automation_jump();
        // A fresh start resumes the playhead follow: clear a manual hold and
        // re-sync the follow tracker so the current view is not mistaken for
        // an outside move on the first frame.
        self.edit_layout.follow_hold = false;
        self.edit_layout.last_scroll = self.engine.session().edit.zoom.scroll;
        self.edit_layout.last_follow_to = None;
    }

    /// While a count-off runs before the transport rolls: the bar and beat sounding (both from
    /// 1), and the count-off's bars.
    pub(crate) fn countoff_status(&self) -> Option<(u32, u32, u32)> {
        let c = self.countoff?;
        let elapsed = self.player.as_ref()?.countoff_elapsed()?;
        let (bar, beat) = c.bar_beat(elapsed);
        Some((bar, beat, c.bars))
    }

    /// The count-off a Record from `from` starts with: Options › Count Off's bars at the tempo
    /// and meter there. Only a Record that starts the transport counts off (not a punch in on
    /// the fly, not Play: Count Off is a record option; `click.only_during_record` is the
    /// metronome's), and not a remote recorder following a booth (`countoff` false: it starts
    /// when the booth's take does).
    fn record_countoff(&self, from: Samples, countoff: bool) -> Option<soundcraft_playback::CountOff> {
        let s = self.engine.session();
        (countoff && s.edit.countoff).then(|| soundcraft_playback::CountOff::plan(s, from)).flatten()
    }

    pub(crate) fn stop_play(&mut self) {
        wait_note::cancel(self);
        let counting = self.countoff_status().is_some();
        self.countoff = None;
        if let Some(p) = &self.player {
            // Half-speed is a separate command. Stop must not leave that rate for the next Play.
            p.set_speed(1.0);
            p.stop();
        }
        if self.engine.transport.recording {
            self.finish_recording();
            if counting {
                // Stopped during the count-off: the transport never rolled, nothing was taken.
                self.ui.status = "Count-off stopped: nothing recorded".into();
            }
        }
        self.sim = None;
        scout::transport_stopped(self);
        // Solo Take lasts until the transport stops (owner's decision).
        if self.engine.session().tracks.iter().any(|t| t.audition_layer.is_some()) {
            self.engine.session_mut().tracks.iter_mut().for_each(|t| t.audition_layer = None);
        }
        self.engine.transport.playing = false;
        self.engine.transport.recording = false;
        let stop_at = self.engine.transport.position;
        // Snapshot the session flags first: `session_mut` below needs a
        // mutable borrow, so no session borrow may span it.
        let (follow_insertion, after_playback) = {
            let s = self.engine.session();
            (s.edit.insertion_follows_playback, s.edit.scrolling == "after_playback")
        };
        if follow_insertion {
            let pos = self.engine.transport.position;
            self.engine.session_mut().edit.selection = Range::point(pos);
        }
        if after_playback {
            // After Playback never scrolls while playing, so bring the stopped
            // position into view now (only if it is off-screen); every other
            // mode keeps its view.
            let at = stop_at.max(0);
            let (scroll, spp) = {
                let z = &self.engine.session().edit.zoom;
                (z.scroll, z.samples_per_px.max(0.01))
            };
            let width = f64::from(self.edit_layout.timeline[2] - self.edit_layout.timeline[0]).max(0.0);
            let end = scroll.saturating_add((width * spp) as Samples);
            if at < scroll || at >= end {
                let _ = self.engine.execute("view.scroll", &json!({ "to": at }));
            }
        }
        self.edit_layout.follow_hold = false;
    }

    fn start_recording(&mut self, countoff: bool) {
        if let Some(why) = record_unavailable(cfg!(target_arch = "wasm32")) {
            self.ui.status = why.into();
            return;
        }
        let armed = self.engine.session().tracks.iter().any(|t| t.mixer.record_arm);
        if !armed {
            let why = if self.engine.session().tracks.is_empty() {
                "Nothing to record on yet: add a track with Track › New…, then click its R button."
            } else {
                "No track is record-enabled: click the R button on the track to record."
            };
            self.ui.status = why.into();
            self.record_hint = Some((self.last_frame.unwrap_or_default(), why));
            return;
        }
        // Half speed reads the input elastically: a take would lose input.
        if let Some(Err(err)) = self.player.as_ref().map(|p| p.can_record()) {
            self.ui.status = format!("Cannot record: {err}");
            return;
        }
        self.ensure_input();
        // A MIDI or instrument track playing modules' MIDI records without an audio input: its
        // take is printed from the mix when the pass ends (`finish_recording`).
        // So does one playing hardware MIDI input: the audio thread hands back what it heard.
        let midi_armed = self
            .engine
            .session()
            .tracks
            .iter()
            .any(|t| t.mixer.record_arm && t.kind.is_midi() && matches!(t.mixer.input, Route::Modules(_) | Route::Midi(_)));
        midi_devices::start_take(self);
        // Armed audio tracks on a remote recorder's input record there (their takes land here).
        let remote_armed = soundcraft_engine::io::remote_armed(self.engine.session());
        let takes = match self.arm_recorder() {
            Ok(takes) => takes,
            Err(_) if midi_armed || remote_armed => Vec::new(),
            Err(err) => {
                self.ui.status = format!("Cannot record: {err}");
                return;
            }
        };
        self.lost_shown = 0;
        if let Some(p) = &self.player {
            p.set_recording(true);
        }
        // Recording at another device rate goes ahead, with a warning.
        if let Some(w) = self.rate_warning() {
            self.ui.status = w;
        }
        let rolling = self.is_playing().then(|| (self.position(), self.play_loop));
        self.engine.transport.recording = true;
        let plan = soundcraft_engine::io::record_start(&self.engine.session().edit, rolling);
        self.record_start = plan.start;
        self.punch = plan.punch;
        if let Some((from, end, looped)) = plan.play {
            // The count-off comes before `from` (after it, the pre-roll plays as usual), at the
            // tempo and meter there; the take starts at `from` once it is over.
            let c = self.record_countoff(from, countoff);
            // Armed tracks play their clips through the pre- and post-roll.
            if let Some(p) = &self.player {
                p.set_punch(self.punch);
            }
            self.start_play_counted(from, end, looped, c);
        }
        self.plan_recording(takes, plan.pass, plan.loop_from);
    }

    /// QuickPunch: Record pressed while a take records punches out (playback goes on, the
    /// clips are heard again) or back in; the take keeps recording throughout, so each punch
    /// has handles, and is placed punch by punch with crossfades when it stops.
    fn quick_punch(&mut self) {
        let at = self.position();
        let Some(plan) = &mut self.take_plan else { return };
        let start = plan.start;
        let punched_in = soundcraft_engine::io::toggle_punch(&mut plan.punches, start, at);
        // Live: inside an open punch the armed tracks hear their input; outside, their clips.
        self.punch = Some(if punched_in { Range::new(at, Samples::MAX) } else { Range::point(at) });
        if let Some(p) = &self.player {
            p.set_punch(self.punch);
        }
        self.ui.status = if punched_in { "Punched in".into() } else { "Punched out: playing on (Record punches in again)".into() };
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Remember where the armed `takes` go (split into passes of `pass` when loop recording),
    /// placed earlier by the record offset, and autosave that plan with the session.
    fn plan_recording(&mut self, takes: Vec<soundcraft_engine::io::Take>, pass: Option<Samples>, loop_from: Option<Samples>) {
        let rate = self.recorder.as_ref().map_or(0, |r| r.sample_rate);
        self.take_reported = None;
        let latency = self.take_latency();
        self.take_plan = Some(soundcraft_engine::io::Recording {
            start: self.record_start,
            pass,
            punch: self.punch,
            rate,
            takes,
            latency,
            loop_from,
            punches: Vec::new(),
        });
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Once the take has its first frame, its plan starts exactly there (`Recorder::start_pos`,
    /// not where the playhead was when Record was pressed) and its offset uses the feed and mix
    /// latencies of that frame. Autosaved again when that changed it.
    fn refine_take_plan(&mut self) {
        let Some(start) = self.recorder.as_ref().and_then(soundcraft_playback::record::Recorder::start_pos) else { return };
        let latency = self.take_latency();
        let Some(plan) = &mut self.take_plan else { return };
        // Each internal take's delay, in tap order (the order the plan lists them).
        let mut changed = (plan.start, plan.latency) != (start, latency);
        let rec = self.recorder.as_ref();
        for (k, t) in plan.takes.iter_mut().filter(|t| t.internal).enumerate() {
            let tap = rec.and_then(|r| r.start_tap_latency(k)).map_or(0, i64::from);
            changed |= t.tap_latency != tap;
            t.tap_latency = tap;
        }
        if !changed {
            return;
        }
        (plan.start, plan.latency) = (start, latency);
        if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Arm the input for the record-enabled tracks; a session never saved records into its
    /// autosave folder. Returns the takes it records.
    /// A track whose input is a bus records it from the mix (internal recording), with no input
    /// device if need be.
    fn arm_recorder(&mut self) -> Result<Vec<soundcraft_engine::io::Take>, String> {
        let name = soundcraft_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved_dir = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(name));
        let device = self.recorder.as_ref().map_or(0, |r| r.channels);
        let plan = soundcraft_engine::io::plan_recording(&self.engine, &unsaved_dir, device)?;
        if plan.takes.is_empty() {
            return Err("no audio track is record-enabled".into());
        }
        if self.recorder.is_none() {
            // Busses only (a hardware input was refused above): no input device is needed.
            let Some(p) = &self.player else { return Err("no audio device".into()) };
            self.recorder = Some(p.internal_recorder());
        }
        let Some(r) = &mut self.recorder else { return Err("no audio input device".into()) };
        let (format, depth, bwf) = soundcraft_engine::io::take_format(self.engine.session());
        let file = soundcraft_engine::io::TakeFile { format, depth, bwf: bwf.is_some() };
        let takes = plan
            .takes
            .iter()
            .map(|(track, inputs, path)| soundcraft_engine::io::Take {
                track: *track,
                path: path.clone(),
                channels: inputs.len(),
                internal: plan.is_tap(*track),
                from_device: plan.from_device(*track),
                tap_latency: 0,
                file: Some(file),
            })
            .collect();
        if let Some(p) = &self.player {
            p.set_record_taps(r.channels, &plan.taps);
        }
        let width = plan.tap_channels();
        r.arm_as(plan.takes, width, &soundcraft_playback::record::TakeFormat { format, depth, bwf })?;
        Ok(takes)
    }

    /// The solo scope of the session as it is now, made once per session revision and shared
    /// by the Mix strips and the Edit headers.
    pub fn solo_scope(&mut self) -> std::sync::Arc<soundcraft_model::SoloScope> {
        let rev = self.engine.revision;
        match &self.solo_cache {
            Some((at, scope)) if *at == rev => std::sync::Arc::clone(scope),
            _ => {
                let scope = std::sync::Arc::new(soundcraft_model::SoloScope::of(self.engine.session()));
                self.solo_cache = Some((rev, std::sync::Arc::clone(&scope)));
                scope
            }
        }
    }

    /// Hands the session to the audio engine now if it changed since it last had it (new
    /// instances made, presets loaded into running ones), and reports plugins that refused a
    /// preset. Reading plugin states first does this, so a state read never comes from before an
    /// edit made this frame (a preset just loaded would be overwritten by the old sound).
    pub(crate) fn sync_player(&mut self) {
        if self.engine.revision == self.last_rev {
            return;
        }
        self.last_rev = self.engine.revision;
        let Some(p) = &self.player else { return };
        p.update_session(self.player_session());
        let refused = p.take_refused_presets();
        if let Some((t, slot)) = refused.first() {
            let s = self.engine.session();
            let ins = s.track(*t).and_then(|tr| match slot {
                SlotKey::Instrument => tr.instrument.as_ref(),
                SlotKey::Module(m) => tr.mixer.module(*m),
            });
            let name = ins.map_or_else(
                || "The plugin".to_string(),
                |i| crate::mix_window::plugin_info(&i.plugin).map_or(i.plugin.clone(), |p| p.name.to_string()),
            );
            self.ui.status = format!("{name} did not accept the preset: it is back at its defaults");
        }
    }

    /// Reads every live third-party plugin's state and stores it on its insert
    /// (`mix.insert_state`), so saves and bounces include it.
    pub fn capture_plugin_states(&mut self) {
        self.sync_player();
        let Some(p) = &self.player else { return };
        for (t, key, data) in p.capture_states() {
            let module = match key {
                SlotKey::Instrument => json!("instrument"),
                SlotKey::Module(m) => json!(m.0),
            };
            let state = soundcraft_model::b64::encode(&data);
            if let Err(e) = self.engine.execute("mix.module_state", &json!({"track": t.0, "module": module, "state": state})) {
                log::warn!("storing plugin state: {e}");
            }
        }
    }

    /// Reads one module's live third-party plugin state and stores it on its insert, before the
    /// module is copied, moved or removed (`mix.module_state`: no undo step, so it joins none).
    pub(crate) fn capture_module_state(&mut self, track: TrackId, module: soundcraft_model::ModuleId) {
        self.capture_slot_state(track, SlotKey::Module(module));
    }

    /// [`SoundApp::capture_module_state`] for any slot, the instrument included (before a preset
    /// is loaded into it, so undo gives back the sound as it is now).
    pub(crate) fn capture_slot_state(&mut self, track: TrackId, slot: SlotKey) {
        self.sync_player();
        let Some(data) = self.player.as_ref().and_then(|p| p.capture_state(track, slot)) else { return };
        let s = self.engine.session();
        let ins = s.track(track).and_then(|t| match slot {
            SlotKey::Instrument => t.instrument.as_ref(),
            SlotKey::Module(m) => t.mixer.module(m),
        });
        if ins.and_then(soundcraft_model::Insert::state_bytes).as_deref() == Some(data.as_slice()) {
            return;
        }
        let module = match slot {
            SlotKey::Instrument => json!("instrument"),
            SlotKey::Module(m) => json!(m.0),
        };
        let state = soundcraft_model::b64::encode(&data);
        if let Err(e) = self.engine.execute("mix.module_state", &json!({"track": track.0, "module": module, "state": state})) {
            log::warn!("storing plugin state: {e}");
        }
    }

    /// Opens the module just picked from a menu: a hosted plugin's own editor (else, with no
    /// audio engine or an editor that can't open here, its plugin window), a built-in's drawer.
    fn open_inserted(&mut self) {
        let Some((track, module)) = self.pending_open.take() else { return };
        chain_view::open_module(self, track, module);
    }

    /// Parameter edits made in plugin editors go into the session (one undo step per gesture).
    /// Hands a `mix.module_action` result to the player (the action's static name from
    /// [`soundcraft_dsp::plugin_actions`], so the audio thread gets no string to free).
    fn send_module_action(&self, v: &Value) {
        let Some(p) = &self.player else { return };
        let track = v.get("track").and_then(Value::as_u64).map(soundcraft_model::TrackId);
        let module = v.get("module").and_then(Value::as_u64).and_then(|m| u32::try_from(m).ok()).map(soundcraft_model::ModuleId);
        let (Some(track), Some(module), Some(action)) = (track, module, v.get("action").and_then(Value::as_str)) else { return };
        let Some(plugin) = self.engine.session().track(track).and_then(|t| t.mixer.module(module)).map(|i| i.plugin.clone()) else { return };
        if let Some(name) = soundcraft_dsp::plugin_actions(&plugin).iter().find(|a| **a == action) {
            p.module_action(track, module, name);
        }
    }

    fn apply_editor_edits(&mut self) {
        let Some(p) = &self.player else { return };
        for (t, key, param, value) in p.idle() {
            // A value the session already has (a plugin reporting every parameter after a
            // restart) is no edit: no command, no undo step.
            let track = self.engine.session().track(t);
            let stored = match key {
                SlotKey::Module(m) => track.and_then(|tr| tr.mixer.module(m)).and_then(|i| i.params.get(param.as_str())),
                SlotKey::Instrument => track.and_then(|tr| tr.instrument.as_ref()).and_then(|i| i.params.get(param.as_str())),
            };
            if stored.is_some_and(|v| (*v - value).abs() <= 1e-6 * v.abs().max(1.0)) {
                continue;
            }
            match key {
                SlotKey::Module(m) => {
                    let key = format!("modparam:{}:{}:{param}", t.0, m.0);
                    let _ =
                        self.engine.execute_merged("mix.module_param", &json!({"track": t.0, "module": m.0, "param": param, "value": value}), &key);
                }
                SlotKey::Instrument => {
                    let key = format!("insparam:{}:{param}", t.0);
                    let _ = self.engine.execute_merged("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value}), &key);
                }
            }
        }
    }

    /// Every two minutes, copy a modified session to the autosave folder (not while recording).
    /// Timed by the wall clock: egui's stands still while the window is hidden.
    fn autosave(&mut self) {
        let Some(dir) = self.autosave_dir.clone() else { return };
        let last = *self.last_autosave.get_or_insert_with(std::time::Instant::now);
        if last.elapsed().as_secs() < 120 || !self.engine.is_dirty() || self.engine.transport.recording {
            return;
        }
        self.last_autosave = Some(std::time::Instant::now());
        self.capture_plugin_states();
        self.autosave_to(&dir);
    }

    /// Autosave a modified session within a second of every change, also while recording. The
    /// plugins' own state reaches the session every two minutes (`autosave`).
    fn autosave_changes(&mut self) {
        if let Some(e) = self.autosaver.as_ref().and_then(soundcraft_engine::autosave::Autosaver::failure) {
            self.ui.status = format!("Autosave failed: {e}");
        }
        if self.autosave_dir.is_none() || self.engine.revision == self.autosaved_rev {
            return;
        }
        self.autosaved_rev = self.engine.revision;
        if !self.engine.is_dirty() {
            self.release_autosave(false);
        } else if let Some(root) = self.autosave_dir.clone() {
            self.queue_autosave(root);
        }
    }

    /// Hand the session to the autosaver for its folder under `root`.
    fn queue_autosave(&mut self, root: std::path::PathBuf) {
        self.autosaved_rev = self.engine.revision;
        let w = self.autosaver.get_or_insert_with(|| soundcraft_engine::autosave::Autosaver::new(root));
        // Loop recording MIDI: without the passes shown meanwhile (a recovery places the take
        // from what was spilled).
        let lifted = self.midi_loop_preview.as_ref().and_then(|p| p.lifted(self.engine.session())).map(std::sync::Arc::new);
        w.write(lifted.unwrap_or_else(|| self.engine.session_arc()), self.engine.path.clone(), self.take_plan.clone());
    }

    /// Write the session to its own folder under `root` and wait for it; false when that failed.
    fn autosave_to(&mut self, root: &std::path::Path) -> bool {
        self.queue_autosave(root.to_path_buf());
        let Some(w) = &self.autosaver else { return false };
        let written = w.flush();
        if let Err(e) = &written {
            self.ui.status = format!("Autosave failed: {e}");
        }
        written.is_ok()
    }

    /// Autosave now, if the app has an autosave folder.
    fn write_autosave(&mut self) -> bool {
        self.autosave_dir.clone().is_some_and(|root| self.autosave_to(&root))
    }

    /// Done with this session's autosave folder: keep it for the next launch, or delete it.
    fn release_autosave(&mut self, keep: bool) {
        if let Some(w) = &self.autosaver {
            w.close(keep);
        }
    }

    /// Done with the open session. Unsaved changes stay in its autosave folder for the next
    /// launch, unless the user quit with FreeDAWm's Quit; a take still recording is finished
    /// and always stays.
    pub fn close_autosave(&mut self) {
        let recording = self.engine.transport.recording;
        if recording {
            self.stop_play();
        }
        let keep = self.engine.is_dirty() && (recording || !self.quit_requested);
        if keep {
            self.capture_plugin_states();
            self.write_autosave();
        }
        self.release_autosave(keep);
        if let Some(w) = &self.autosaver {
            let _ = w.flush();
        }
    }

    /// The graphics device is gone (a driver reset): the window is about to close and the app
    /// moves on to a new window and device. Hosted plugin editors are closed first (their windows
    /// are owned by the main window and would be destroyed under the plugins), and the session,
    /// with the plugins' own state, is autosaved now. True when the session is safe on disk:
    /// unmodified, or autosaved.
    pub fn save_for_recovery(&mut self) -> bool {
        if let Some(p) = &self.player {
            p.close_editors();
        }
        if !self.engine.is_dirty() {
            return true;
        }
        self.capture_plugin_states();
        self.write_autosave()
    }

    /// The app now draws in a new egui context on a new graphics device (see
    /// `save_for_recovery`): fonts and theme are installed again, and textures made for the old
    /// context are dropped, to be made again on demand.
    pub fn reset_graphics(&mut self) {
        self.fonts_ready = false;
        self.fonts_installed = false;
        self.theme_applied = None;
        self.docking_context = None;
        self.browser_thumbs.forget_textures();
        self.video = video_track::VideoPool::default();
    }

    /// Run part of a frame. If it panics, the session is autosaved (once per change) and the app
    /// carries on instead of quitting.
    pub fn guarded(&mut self, f: impl FnOnce(&mut SoundApp)) {
        if std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| f(self))).is_ok() || self.panic_rev == self.engine.revision {
            return;
        }
        self.panic_rev = self.engine.revision;
        let saved = self.engine.is_dirty() && self.write_autosave();
        self.ui.status = if saved {
            "FreeDAWm hit an internal error (see the log). Your changes were autosaved: save them, then restart FreeDAWm.".into()
        } else {
            "FreeDAWm hit an internal error (see the log). Restart FreeDAWm to be safe.".into()
        };
    }

    /// Replace the open session with unsaved work an earlier run left behind.
    pub fn recover(&mut self, left: &soundcraft_engine::autosave::Leftover) {
        if self.engine.transport.recording {
            self.ui.status = "Stop recording before recovering other work.".into();
            return;
        }
        self.close_autosave();
        match soundcraft_engine::autosave::recover(&mut self.engine, left) {
            Ok(missing) => {
                if self.write_autosave() {
                    soundcraft_engine::autosave::discard(left);
                }
                self.ui.status = match missing.len() {
                    0 => format!("Recovered “{}”. Save it to keep the changes.", left.name),
                    n => format!("Recovered “{}”, but {n} audio file(s) are missing. Save it to keep the changes.", left.name),
                };
                // Silent sidechains in the recovered session (also in the engine's messages).
                let notes = self.engine.take_sidechain_notes();
                if !notes.is_empty() {
                    self.ui.status = format!("{} {}", self.ui.status, notes.join("; "));
                }
            }
            Err(e) => self.ui.status = format!("Couldn't recover “{}”: {e}", left.name),
        }
    }

    /// The most recent autosave, if any.
    pub fn latest_autosave(&self) -> Option<std::path::PathBuf> {
        let dir = self.autosave_dir.as_ref()?;
        let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
        for d in std::fs::read_dir(dir).ok()?.flatten() {
            for f in std::fs::read_dir(d.path()).into_iter().flatten().flatten() {
                let p = f.path();
                if soundcraft_model::is_session_path(&p)
                    && let Ok(m) = f.metadata().and_then(|m| m.modified())
                    && best.as_ref().is_none_or(|(t, _)| m > *t)
                {
                    best = Some((m, p));
                }
            }
        }
        best.map(|(_, p)| p)
    }

    /// The open devices' channel counts: (input, output). `None` when that device isn't open (the
    /// input opens on the first arm or monitor) or is the silent clock.
    pub fn device_channels(&self) -> (Option<usize>, Option<usize>) {
        let input = self.recorder.as_ref().map(|r| r.channels);
        let output = self.player.as_ref().filter(|p| !p.silent).map(|p| usize::from(p.device_channels));
        (input, output)
    }

    /// The input device's channel names, in channel order, when it gives them (JACK's ports,
    /// PipeWire's sources); empty otherwise.
    pub fn input_channel_names(&mut self) -> Vec<String> {
        if let Some(r) = &self.recorder {
            return r.channel_names.clone();
        }
        self.all_sources().map(|a| soundcraft_playback::pw::labels(&a.chosen, &a.sources)).unwrap_or_default()
    }

    /// On PipeWire's "All sources" input: every source and the ones its channels record now.
    pub fn all_sources(&mut self) -> Option<AllSources> {
        #[cfg(not(target_arch = "wasm32"))]
        {
            if let Some((at, view)) = &self.all_sources
                && at.elapsed() < std::time::Duration::from_secs(2)
            {
                return view.clone();
            }
            let view = self.ui.audio.all_sources().then(|| AllSources {
                sources: soundcraft_playback::pw::sources(),
                chosen: soundcraft_playback::pw::effective_sources(&self.ui.audio.input_sources),
            });
            self.all_sources = Some((std::time::Instant::now(), view.clone()));
            view
        }
        #[cfg(target_arch = "wasm32")]
        None
    }

    /// Records `keys` too on "All sources" (after its channels in use); the input reopens with
    /// them. The first new channel (0-based), or why not (while recording the input stays).
    pub fn add_sources(&mut self, keys: &[String]) -> Result<u16, String> {
        if self.engine.transport.recording {
            return Err("stop recording before adding a source".into());
        }
        let mut chosen = self.all_sources().ok_or("the input is not PipeWire's All sources")?.chosen;
        let first = chosen.len();
        if first + keys.len() > 64 {
            return Err("All sources records at most 64 channels".into());
        }
        chosen.extend(keys.iter().cloned());
        self.ui.audio.input_sources = chosen;
        #[cfg(not(target_arch = "wasm32"))]
        {
            self.all_sources = None;
        }
        // The input reopens with the new channels on the next record or monitor.
        self.recorder = None;
        self.recorder_failed = false;
        u16::try_from(first).map_err(|e| e.to_string())
    }

    /// The output device's channel names, in channel order, when it gives them; empty otherwise.
    pub fn output_channel_names(&self) -> Vec<String> {
        self.player.as_ref().map(|p| p.output_channel_names.clone()).unwrap_or_default()
    }

    /// Whether the meters are live: while playing, and while armed or input-monitored tracks are heard.
    pub(crate) fn meters_live(&self) -> bool {
        self.engine.transport.playing
            || (self.recorder.is_some() && self.engine.session().tracks.iter().any(|t| t.mixer.record_arm || t.mixer.may_hear_input()))
    }

    /// Reopens the audio devices with `cfg` (Setup › Playback Engine › Apply), keeping the
    /// transport position, and keeps it as `ui.audio`. Plugin instances and their editors are
    /// recreated with the new player. Refused while recording, which would lose the take; `ui.audio`
    /// then stays the configuration of the open device.
    pub fn apply_audio_config(&mut self, cfg: soundcraft_playback::AudioConfig) -> Result<(), String> {
        if self.engine.transport.recording {
            return Err("stop recording before changing the audio device".into());
        }
        if self.rendering() {
            // The render holds the player's live instances (and the player holds their state
            // snapshot and editors): a new player now would tear the old one down under it.
            return Err(RENDER_HOLDS_DEVICE.into());
        }
        if self.open.is_some() {
            // The player is making the opening session's instances: they would go with it.
            return Err(open_progress::OPENING.into());
        }
        self.ui.audio = cfg;
        let at = self.position();
        if let Some(p) = &self.player {
            p.stop();
            // The old player's plugin instances go away with it; their editor windows must be
            // closed here, on the main thread, or they would stay on screen orphaned.
            p.close_editors();
        }
        // The input reopens on the next record or monitor with the new choice.
        self.recorder = None;
        self.recorder_failed = false;
        // Rigs already back on the old player are stored before it goes.
        scout::drain(self);
        // The old player goes first: with ASIO, the SDK holds one driver per process, and opening
        // the new device while the old streams would initialise (or replace) the running driver
        // and then destroy it under the audio thread.
        let (session, audio) = (self.engine.session_arc(), &self.ui.audio);
        reopen(&mut self.player, || soundcraft_playback::Player::with_config(session, audio));
        self.device_notice_pending = true;
        if let Some(p) = &self.player {
            p.locate(at);
        }
        // The new player gets the band on the next frame.
        self.band_lib_key = 0;
        self.midi_sent = false;
        // A capture or a measurement out on the old player never comes back.
        scout::lost(self);
        self.forget_measure();
        Ok(())
    }

    /// Once per opened player: a chosen audio system or device that is missing (so the default
    /// plays), or no device at all, is said on the status line, not only in the log.
    fn tell_device_fallback(&mut self) {
        if !std::mem::take(&mut self.device_notice_pending) {
            return;
        }
        if let Some(n) = self.player.as_ref().and_then(|p| p.fallback_notice(&self.ui.audio)) {
            log::warn!("{n}");
            self.ui.status = n;
        }
    }

    /// Open the input device once (for recording and input monitoring) and feed the player.
    fn ensure_input(&mut self) {
        let Some(p) = &self.player else { return };
        // A recorder of busses only (no input device then) gives way to a device, between takes.
        let internal = self.recorder.as_ref().is_some_and(soundcraft_playback::record::Recorder::is_internal);
        if (self.recorder.is_some() && !(internal && !self.engine.transport.recording)) || self.recorder_failed {
            return;
        }
        // The input opened with the output (one clock) when there is one, else its own stream
        // (never a second ASIO stream).
        match p.open_input(&self.ui.audio) {
            Ok(r) => self.recorder = Some(r),
            Err(e) => {
                self.recorder_failed = true;
                self.ui.status = format!("No audio input: {e}");
            }
        }
    }

    /// A plug-in run in its own process that crashed or hung: said on the status line (it is
    /// silenced already, an effect passing its input through; the session goes on).
    fn watch_bridged_plugins(&mut self) {
        if let Some(last) = soundcraft_mix::take_plugin_crash_reports().pop() {
            self.ui.status = last;
        }
    }

    /// Stop the transport, keeping a take in progress, when the audio output fails.
    fn watch_output(&mut self) {
        let Some(p) = &self.player else { return };
        let Some(trouble) = p.trouble() else { return };
        let name = p.device_name.clone();
        let (cause, then) = match trouble {
            OutputTrouble::Bug => (
                "the audio engine hit an internal error (see the log)".to_string(),
                "Use Reconnect audio device in Setup ▸ Playback Engine, or restart FreeDAWm.",
            ),
            OutputTrouble::Gone => {
                let system = self.player.as_ref().map(|p| p.system.clone()).unwrap_or_default();
                self.device_return = device_return::Watch::new(&system, &name, std::time::Instant::now()).map(device_return::Returning::new);
                let then = if self.device_return.is_some() {
                    "Plug it back in: FreeDAWm reconnects when it returns."
                } else {
                    "Plug it back in, then use Reconnect audio device in Setup ▸ Playback Engine."
                };
                (format!("“{name}” was disconnected"), then)
            }
            OutputTrouble::Stalled => (format!("“{name}” stopped asking for audio"), "Use Reconnect audio device in Setup ▸ Playback Engine."),
        };
        let (recording, playing) = (self.engine.transport.recording, self.engine.transport.playing);
        if recording || playing {
            self.stop_play();
        }
        // A capture out on the dead device never comes back.
        scout::lost(self);
        self.ui.status = if recording {
            format!("Recording stopped: {cause}. The take is kept up to there. {then}")
        } else if playing {
            format!("Playback stopped: {cause}. {then}")
        } else {
            format!("Audio output: {cause}. {then}")
        };
    }

    /// Reconnect by itself when the output device that went away is listed again.
    fn watch_device_return(&mut self) {
        let Some(r) = &mut self.device_return else { return };
        if !r.poll(std::time::Instant::now()) {
            return;
        }
        let name = r.watch.device.clone();
        self.reconnect_audio();
        if self.player.as_ref().is_some_and(|p| p.device_name == name) {
            self.ui.status = format!("“{name}” is back: reconnected.");
        }
    }

    /// Stop recording, keeping the take, when the input device or the disk fails, and say as soon
    /// as a slow disk loses input.
    fn watch_input(&mut self) {
        let recording = self.engine.transport.recording;
        let Some(r) = &mut self.recorder else { return };
        let Some(trouble) = r.trouble() else {
            let lost = r.lost_ms();
            if recording && lost > self.lost_shown {
                self.lost_shown = lost;
                self.ui.status = format!("The disk is too slow: this take has lost {lost} ms of input so far.");
            }
            return;
        };
        if recording {
            self.stop_play();
        }
        let (what, then) = match trouble {
            Trouble::Device(what) => {
                self.recorder = None;
                self.recorder_failed = true;
                (what, "Reconnect it, then use Reconnect audio device in Setup ▸ Playback Engine.")
            }
            Trouble::Disk(what) => (what, "Check the disk, then record again."),
        };
        self.ui.status = if recording {
            format!("Recording stopped: {what}. The take is kept up to there. {then}")
        } else {
            format!("Audio input: {what}. {then}")
        };
    }

    /// Reopen the audio devices, keeping a take in progress. Refused while a render runs through
    /// the player's instances (the status line says so).
    pub fn reconnect_audio(&mut self) {
        if self.rendering() {
            self.ui.status = RENDER_HOLDS_DEVICE.into();
            return;
        }
        if open_progress::refuses(self) {
            return;
        }
        self.device_return = None;
        if self.is_playing() || self.engine.transport.recording {
            self.stop_play();
        }
        if let Some(p) = &self.player {
            // Its plugin editors go with it; closed here, on the main thread (see `apply_audio_config`).
            p.close_editors();
        }
        self.recorder = None;
        self.recorder_failed = false;
        // Rigs already back on the old player are stored before it goes.
        scout::drain(self);
        // The old player goes first (see `apply_audio_config`): ASIO allows one driver at a time.
        let (session, audio) = (self.engine.session_arc(), &self.ui.audio);
        reopen(&mut self.player, || Player::with_config(session, audio));
        self.device_notice_pending = true;
        // The new player gets the band on the next frame.
        self.band_lib_key = 0;
        self.midi_sent = false;
        // A capture or a measurement out on the old player never comes back.
        scout::lost(self);
        self.forget_measure();
    }

    /// Discard Take: stop, and throw the take away. Nothing is placed, the take files go, and
    /// the MIDI heard is dropped.
    /// Reads the free space on the disk the takes go to, at most every few seconds.
    pub(crate) fn refresh_disk_free(&mut self) {
        let now = self.last_frame.unwrap_or_default();
        if self.disk_free.is_some_and(|(at, _)| (at..at + 5.0).contains(&now)) {
            return;
        }
        let name = soundcraft_engine::io::sanitize_name(&self.engine.session().name);
        let unsaved = self.autosave_dir.as_ref().map_or_else(std::env::temp_dir, |d| d.join(name));
        let dir = soundcraft_engine::io::record_dir(&self.engine, &unsaved);
        self.disk_free = Some((now, soundcraft_engine::io::disk_free(&dir)));
    }

    /// How long the armed tracks can still record on the takes' disk (`2 h 05 min`), when known.
    pub(crate) fn record_time_left(&self) -> Option<String> {
        let free = self.disk_free.and_then(|(_, b)| b)?;
        soundcraft_engine::io::record_seconds_left(self.engine.session(), free).map(soundcraft_engine::io::time_left_text)
    }

    /// Options › Recording › Lock Record: whether this Stop is refused because a locked take is
    /// recording. The first Stop only says so; a second within [`RECORD_LOCK_SECONDS`] stops.
    /// Discard Take and Restart Take are deliberate and are not locked.
    fn record_locked(&mut self) -> bool {
        if !self.engine.transport.recording || !self.engine.session().edit.flag(soundcraft_engine::cmd::RECORD_LOCK) {
            self.record_lock_stop = None;
            return false;
        }
        let now = self.last_frame.unwrap_or_default();
        if self.record_lock_stop.is_some_and(|at| now - at <= RECORD_LOCK_SECONDS) {
            self.record_lock_stop = None;
            return false;
        }
        self.record_lock_stop = Some(now);
        self.ui.status = "Recording is locked: press Stop again to stop (Options › Recording › Lock Record)".into();
        true
    }

    fn discard_recording(&mut self) {
        if !self.engine.transport.recording {
            return;
        }
        self.engine.transport.recording = false;
        if let Some(p) = &self.player {
            p.set_recording(false);
        }
        let (takes, _) = self.recorder.as_mut().map(soundcraft_playback::record::Recorder::finish).unwrap_or_default();
        for (_, path) in &takes {
            if let Err(e) = std::fs::remove_file(path) {
                log::warn!("discard take: {}: {e}", path.display());
            }
        }
        self.midi_rec.clear();
        midi_devices::lift_preview(self);
        self.midi_loop_preview = None;
        self.take_plan = None;
        self.punch = None;
        // The autosave forgets the plan (nothing to recover).
        self.autosaved_rev = 0;
        self.stop_play();
        self.ui.status = if takes.is_empty() { "Take discarded".into() } else { format!("Take discarded ({} file(s) deleted)", takes.len()) };
    }

    fn finish_recording(&mut self) {
        self.engine.transport.recording = false;
        if let Some(p) = &self.player {
            p.set_recording(false);
        }
        let end = self.position();
        self.refine_take_plan();
        // The loop passes heard while recording come off: the take is placed for good below.
        midi_devices::collect(self);
        midi_devices::lift_preview(self);
        let plan = self.take_plan.take();
        let latency = self.take_latency();
        let (takes, problem, start) = match &mut self.recorder {
            Some(r) => {
                let (takes, problem) = r.finish();
                let start = r.start_pos().unwrap_or(self.record_start);
                (takes, problem, start)
            }
            None => (Vec::new(), None, self.record_start),
        };
        let tracks = takes.len().max(1);
        let plan = plan.unwrap_or_else(|| soundcraft_engine::io::Recording {
            start,
            pass: None,
            punch: self.punch,
            rate: 0,
            takes: Vec::new(),
            latency,
            loop_from: None,
            punches: Vec::new(),
        });
        self.punch = None;
        self.autosaved_rev = 0;
        // A remote recorder sends these to the booth (`net_ui`).
        self.last_audio_take = (!takes.is_empty()).then(|| (plan.clone(), takes.clone()));
        // Audio, modules' MIDI and MIDI placed below are one undo step.
        let depth = self.engine.undo_depth();
        let mut ids = soundcraft_engine::io::place_recording(&mut self.engine, &plan, takes);
        // Armed tracks playing modules' MIDI: their take is what the mix gave them over the pass
        // (within the punch range), printed now from the session as it stands.
        if let Some(r) = soundcraft_engine::module_record::print_range(&plan, end) {
            ids.extend(soundcraft_engine::module_record::record_module_inputs(&mut self.engine, r, None));
        }
        // Armed tracks on hardware MIDI input: what they heard, as clips.
        ids.extend(midi_devices::finish_take(self, &plan, end));
        self.engine.join_undo_since(depth, "Record");
        self.ui.status =
            if plan.pass.is_some() { format!("Loop-recorded {} take(s)", ids.len() / tracks) } else { format!("Recorded {} clip(s)", ids.len()) };
        if let Some(p) = problem {
            log::warn!("recording: {p}");
            self.ui.status = format!("{} ({p})", self.ui.status);
        }
        let lost = midi_devices::lost_in_take(self);
        if lost > 0 {
            log::warn!("recording: {lost} MIDI events lost");
            self.ui.status =
                format!("{} ({lost} MIDI events were lost: the window was not drawing; a note may run on to the end of its clip)", self.ui.status);
        }
    }

    /// A control of `track` that drives `param` is held by the pointer this frame: automation
    /// writing counts it as touched while it is held, moving or not.
    pub fn hold_control(&mut self, track: TrackId, param: soundcraft_model::AutoParam) {
        if !self.held_controls.iter().any(|(t, p)| *t == track && *p == param) {
            self.held_controls.push((track, param));
        }
    }

    /// While playing, writes automation from the controls as each track's mode says
    /// ([`soundcraft_engine::Engine::automation_pass`]): every automatable control, from wherever
    /// it is changed. The fader under the pointer and the controls [`SoundApp::hold_control`]
    /// reported this frame count as held. A pass and whatever is dragged during it are one undo step.
    fn write_automation(&mut self, ctx: &egui::Context) {
        let mut held = std::mem::take(&mut self.held_controls);
        if ctx.input(|i| i.pointer.any_down())
            && let Some(Gesture::Fader { track, .. }) = &self.gesture
        {
            let trim = self.engine.session().track(*track).is_some_and(|t| t.mixer.automation_mode == soundcraft_model::AutomationMode::Trim);
            let p = if trim { soundcraft_model::AutoParam::Trim } else { soundcraft_model::AutoParam::Volume };
            if !held.iter().any(|(t, x)| t == track && *x == p) {
                held.push((*track, p));
            }
        }
        self.engine.automation_ui_held(held);
        let playing = self.is_playing();
        self.engine.automation_pass(playing);
    }

    /// While playing (not recording: loop passes are takes), Loop Playback turned on or off or
    /// the loop moved or resized changes the loop under way, without a jump
    /// (`Player::set_loop`). Turned on, the loop is the loop range.
    fn follow_loop_changes(&mut self) {
        if !self.is_playing() || self.engine.transport.recording {
            return;
        }
        let e = &self.engine.session().edit;
        let now = (e.loop_playback, e.loop_range.filter(|r| !r.is_empty()));
        if now == self.loop_basis {
            return;
        }
        self.loop_basis = now;
        let want = if now.0 { now.1 } else { None };
        if want == self.play_loop {
            return;
        }
        self.play_loop = want;
        if let Some(p) = &self.player {
            p.set_loop(want);
        }
        if let Some(sim) = &mut self.sim {
            sim.2 = want;
        }
    }

    /// Pause: playing, the transport stops where it is and the selection stays as it was (the
    /// insertion does not follow); paused, it plays on from there with the same end and loop.
    /// Recording, or with nothing paused, it is Stop or Play.
    fn pause(&mut self) {
        if self.is_playing() && !self.engine.transport.recording && self.countoff.is_none() {
            let at = self.engine.transport.position;
            let resume = (at, self.play_end, self.play_loop);
            let selection = self.engine.session().edit.selection;
            self.stop_play();
            self.engine.session_mut().edit.selection = selection;
            self.engine.transport.position = at;
            self.paused = Some(resume);
        } else if self.is_playing() {
            self.stop_play();
        } else if let Some((at, end, looped)) = self.paused.take() {
            self.start_play(at, end.filter(|e| *e > at), looped);
        } else {
            self.play_from_selection();
        }
    }

    /// Play from the selection, honouring loop playback and selection end.
    pub fn play_from_selection(&mut self) {
        let s = self.engine.session();
        let sel = s.edit.selection;
        let preroll = if s.edit.pre_post_roll { s.edit.pre_roll } else { 0 };
        let postroll = if s.edit.pre_post_roll { s.edit.post_roll } else { 0 };
        if let Some(at) = soundcraft_engine::cmd::transport::play_start(s) {
            // Dynamic Transport: from the play start marker, whatever the selection; with Loop
            // Playback on, the loop range loops (from the marker when it is inside the loop).
            match s.edit.loop_range.filter(|lp| s.edit.loop_playback && !lp.is_empty()) {
                Some(lp) => self.start_play(if lp.contains(at) { at } else { lp.start }, None, Some(lp)),
                None => self.start_play((at - preroll).max(0), None, None),
            }
        } else if s.edit.loop_playback && !sel.is_empty() {
            self.start_play(sel.start, None, Some(sel));
        } else if s.edit.loop_playback
            && sel.is_empty()
            && let Some(lp) = s.edit.loop_range.filter(|lp| lp.contains(sel.start))
        {
            // The sticky loop: an insertion inside the loop plays it again (outside it, straight
            // through as if there were no loop).
            self.start_play(lp.start, None, Some(lp));
        } else if !sel.is_empty() {
            self.start_play((sel.start - preroll).max(0), Some(sel.end + postroll), None);
        } else {
            self.start_play((sel.start - preroll).max(0), None, None);
        }
    }

    fn handle_transport(&mut self, dt: f32) {
        let ballistics = preferences::Ballistics::of(&self.ui);
        widgets::set_meter_scale(ballistics.scale);
        let reqs: Vec<TransportRequest> = std::mem::take(&mut self.engine.transport_requests);
        for r in reqs {
            match r {
                TransportRequest::ResetSession => {
                    if let Some(p) = &self.player {
                        p.stop();
                        p.set_recording(false);
                        p.set_speed(1.0);
                        p.locate(self.engine.transport.position);
                    }
                    self.sim = None;
                    // The old capture belongs to the replaced document, never to the new one.
                    self.recorder = None;
                    self.recorder_failed = false;
                    self.punch = None;
                    self.gesture = None;
                    self.engine.automation_reset();
                    self.edit_layout.follow_hold = false;
                    self.meters.clear();
                    self.main_meter = MeterDisplay::default();
                    // The editors' preset dirty flags belong to the old session's modules (the new
                    // one's ids and preset names can match).
                    self.ui.preset_dirty.clear();
                    // A question out with Claude belongs to the old document.
                    scout::ask::session_replaced(self);
                }
                TransportRequest::Play => {
                    if !self.is_playing() {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Stop => {
                    if !self.record_locked() {
                        self.paused = None;
                        self.stop_play();
                    }
                }
                TransportRequest::Pause => self.pause(),
                TransportRequest::TogglePlay => {
                    if self.is_playing() {
                        if !self.record_locked() {
                            self.stop_play();
                        }
                    } else {
                        self.play_from_selection();
                    }
                }
                TransportRequest::Record { countoff } => {
                    if self.engine.transport.recording && self.is_playing() && self.engine.session().edit.quickpunch {
                        self.quick_punch();
                    } else if self.engine.transport.recording {
                        if !self.record_locked() {
                            self.stop_play();
                        }
                    } else if !wait_note::cancel(self) && !wait_note::begin(self) {
                        self.start_recording(countoff);
                    }
                }
                TransportRequest::Locate(at) => {
                    self.paused = None;
                    if self.is_playing() {
                        if let Some(p) = &self.player {
                            p.locate(at);
                        }
                        if let Some(sim) = &mut self.sim {
                            sim.0 = at;
                        }
                        self.engine.automation_jump();
                    }
                    self.engine.transport.position = at;
                }
                TransportRequest::PlayRange { from, end, looped } => self.start_play(from, end, looped),
                TransportRequest::PlaySelection => {
                    // A range plays once; an insertion point plays on from there (it used to
                    // play one sample and stop).
                    let sel = self.engine.session().edit.selection;
                    let end = (!sel.is_empty()).then_some(sel.end);
                    self.start_play(sel.start, end, None);
                }
                TransportRequest::HalfSpeed if self.engine.transport.recording => {
                    // The player keeps full speed while a take records (it would lose input).
                    self.ui.status = "Half speed is not available while recording".into();
                }
                TransportRequest::Varispeed(speed) => {
                    self.varispeed = speed;
                    if self.is_playing()
                        && !self.engine.transport.recording
                        && let Some(p) = &self.player
                    {
                        p.set_speed(speed.unwrap_or(1.0));
                    }
                    self.ui.status = match speed {
                        Some(v) => format!("Varispeed {v:.2}×: plays run at this speed until it is turned off"),
                        None => "Varispeed off".into(),
                    };
                }
                TransportRequest::HalfSpeed => {
                    self.half_speed_play = true;
                    if let Some(p) = &self.player {
                        p.set_speed(0.5);
                    }
                    self.play_from_selection();
                }
                TransportRequest::Shuttle { step, speed } => self.shuttle(step, speed),
                TransportRequest::Scrub(at) => self.engine.transport.position = at,
                TransportRequest::AllNotesOff => {
                    if let Some(p) = &self.player {
                        p.all_notes_off();
                    }
                    self.ui.status = "All MIDI notes off".into();
                }
                TransportRequest::ScoutStart { capture, strips, start, end } => scout::start(self, capture, &strips, start, end),
                TransportRequest::ScoutStop => scout::stop(self),
                TransportRequest::DiscardTake => self.discard_recording(),
                TransportRequest::RestartTake => {
                    if self.engine.transport.recording {
                        self.discard_recording();
                        self.start_recording(true);
                        if self.engine.transport.recording {
                            self.ui.status = "Take discarded; recording again".into();
                        }
                    }
                }
            }
        }
        self.shuttle_back_frame();
        self.follow_loop_changes();
        // Read back the position.
        if let Some(p) = &self.player {
            let playing = p.is_playing();
            let ended = self.engine.transport.playing && !playing;
            if ended {
                p.set_speed(1.0);
            }
            if self.engine.transport.playing {
                self.engine.transport.position = p.position();
                // Automation is written and shown where it is heard: the mix's delay and the
                // output's latency behind the rendered position.
                let lat = p.reported_latency();
                self.engine.automation_heard_delay(i64::from(lat.mix) + i64::from(lat.output));
            }
            let now = self.last_frame.unwrap_or_default();
            let meter_interval = if self.ui.window == MainWindow::Mix { 1.0 / 60.0 } else { 1.0 / 20.0 };
            if now - self.last_meter_poll >= meter_interval {
                p.copy_meters(&mut self.meter_snapshot);
                self.last_meter_poll = now;
            }
            let snap = &self.meter_snapshot;
            for (id, m) in &snap.tracks {
                let d = self.meters.entry(*id).or_default();
                feed_meter(d, m.peak, m.mean_square(), m.gain_reduction_db, dt, ballistics);
                d.midi = (d.midi - dt * 2.0).max(f32::from(m.midi_velocity) / 127.0).clamp(0.0, 1.0);
                if m.feedback != d.feedback {
                    d.feedback = m.feedback;
                    if m.feedback {
                        let name = self.engine.session().track(*id).map_or_else(String::new, |t| t.name.clone());
                        self.ui.status = format!("Feedback protection silenced {name}'s input: turn its monitoring (I) off and on to hear it again");
                    }
                }
                if m.plugin_failures.count != d.plugin_failures {
                    d.plugin_failures = m.plugin_failures.count;
                    if let Some(msg) = failed_plugin_status(self.engine.session(), *id, m.plugin_failures.last) {
                        self.ui.status = msg;
                    }
                }
            }
            let main = snap.main.peak;
            feed_meter(&mut self.main_meter, main, snap.main.mean_square(), 0.0, dt, ballistics);
            if ended {
                self.stop_play();
            }
        } else if let Some((pos, end, looped)) = &mut self.sim {
            let sr = self.engine.session().sample_rate.as_f64();
            *pos += (f64::from(dt) * sr) as i64;
            if let Some(l) = looped
                && *pos >= l.end
            {
                *pos = l.start;
            }
            let stop = end.is_some_and(|e| *pos >= e);
            self.engine.transport.position = *pos;
            if stop {
                self.stop_play();
            }
        }
        if !self.meters_live() {
            for d in self.meters.values_mut() {
                feed_meter(d, [0.0, 0.0], [0.0, 0.0], 0.0, dt, ballistics);
            }
            feed_meter(&mut self.main_meter, [0.0, 0.0], [0.0, 0.0], 0.0, dt, ballistics);
        }
    }

    /// Per-frame logic: control channel, transport, document sync.
    pub fn logic(&mut self, ctx: &egui::Context) {
        if ctx.current_pass_index() > 0 {
            return;
        }
        self.tell_device_fallback();
        audition::poll(self);
        // A ruler pick lasts only while it is still what Delete would mean.
        ruler_items::prune(self);
        // Fonts set now take effect next frame, so draw only from the frame after.
        if !self.fonts_ready {
            if self.fonts_installed {
                self.fonts_ready = true;
            } else {
                fonts::install(ctx);
                self.fonts_installed = true;
                ctx.request_repaint();
            }
        }
        let light = theme::wants_light(ctx, self.ui.theme);
        if self.theme_applied != Some(light) || theme::is_light() != light {
            theme::apply(ctx, light);
            self.theme_applied = Some(light);
        }
        let now = ctx.input(|i| i.time);
        let dt = self.last_frame.replace(now).map_or(1.0 / 60.0, |t| (now - t) as f32).clamp(0.0, 0.25);
        self.frame_ms = self.frame_ms * 0.9 + dt * 1000.0 * 0.1;
        control::drain(self, ctx);
        self.note_recent_session();
        self.apply_editor_edits();
        browser::thumbs::ThumbCache::capture_when_open(self, ctx);
        browser::scan_job::step(self, ctx);
        open_progress::step(self, ctx);
        bounce::step(self, ctx);
        render::step(self, ctx);
        split_progress::step(self, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        ml_ui::step(self, ctx);
        #[cfg(not(target_arch = "wasm32"))]
        ml_audition::step(self, ctx);
        // A MIDI Editor audition that has finished hands the document back.
        midi_editor::step(self);
        // Vocal tuning: analyses and tuned renders in the background, the Vocal Editor's clip
        // (the selected notes', else the one under the playhead) first.
        let playhead = self.position();
        let focus = if self.ui.show_vocal_editor { vocal_editor::focus_clip(self) } else { None };
        vocal_editor::preview::tick(self);
        if self.vocal_worker.poll(&mut self.engine, focus, playhead) {
            ctx.request_repaint();
        }
        if self.vocal_worker.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // Stretches heard live get their offline render in the background.
        if self.stretch_worker.poll(&mut self.engine) {
            ctx.request_repaint();
        }
        if self.stretch_worker.running() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        if !ctx.input(|i| i.pointer.any_down()) {
            if let Some(Gesture::Fader { track, .. }) = self.gesture.take_if(|g| matches!(g, Gesture::Fader { .. })) {
                // A Trim-mode fader let go: the trim lane plays again.
                self.engine.automation_trim(track, None);
            }
            // Keep a running automation pass merged until playback stops.
            if !ctx.egui_wants_keyboard_input() && !self.engine.automation_pass_writing() {
                self.engine.end_merge();
            }
        }
        self.watch_output();
        self.watch_device_return();
        self.watch_bridged_plugins();
        self.watch_input();
        if self.engine.transport.recording {
            self.refine_take_plan();
            midi_devices::collect(self);
            midi_devices::preview_loop(self);
        } else {
            midi_devices::collect_capture(self);
            wait_note::tick(self);
        }
        self.poll_measure();
        if self.measuring.is_some() {
            ctx.request_repaint_after(std::time::Duration::from_millis(50));
        }
        // The LAN remote: its buttons and the booth it follows ask for the transport here, in
        // time for this frame's transport handling.
        #[cfg(not(target_arch = "wasm32"))]
        net_ui::step(self, ctx);
        // Preferences › Operation › Copy imported audio into the project (this computer's).
        self.engine.copy_imported_audio = self.ui.copy_imported_audio;
        // Stop the old playback before sending a replacement session to the audio thread.
        self.handle_transport(dt);
        midi_devices::tick(self);
        surfaces::tick(self);
        midi_learn::tick(self);
        midi_sync_in::tick(self);
        scout::poll(self, ctx);
        // The band's Library can change without an edit (a reload): it is checked too.
        let band_lib = self.engine.band_library();
        let band_lib_key = std::sync::Arc::as_ptr(&band_lib) as usize;
        if self.engine.revision != self.last_rev || band_lib_key != self.band_lib_key {
            self.band_lib_key = band_lib_key;
            if let Some(p) = &self.player {
                p.set_band_library(band_lib);
            }
            self.last_rev = self.engine.revision.wrapping_sub(1);
            self.sync_player();
        }
        // The monitor stage follows its keys (and a new player is told what the last one knew).
        monitor_ui::sync(self);
        self.open_inserted();
        // A drawer closes with its module, however it went (undo included).
        module_drawer::prune(self);
        module_drawer::delay_tap_watch(self, ctx);
        levelup_ui::learn_watch(self, ctx);
        self.autosave_changes();
        self.write_automation(ctx);
        self.autosave();
        if self.recorder.is_none()
            && !self.recorder_failed
            && self.engine.session().tracks.iter().any(|t| t.mixer.record_arm || t.mixer.may_hear_input())
        {
            self.ensure_input();
        }
        // Autosave and the macOS menu bar only run on frames, and plugin editors are native windows egui never hears from.
        // egui takes its predicted frame time off every delay, so add it back.
        let editors = self.player.as_ref().is_some_and(Player::any_editor_open);
        ctx.request_repaint_after_secs(if editors { 0.03 } else { 1.0 } + ctx.input(|i| i.predicted_dt));
        control::collect_screenshots(self, ctx);
    }

    /// Feed queued synthetic input (from the control channel) into egui: one pointer event per
    /// frame (so drags span frames like real ones), keys as press+release pairs, text at once.
    pub fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        // While a synthetic gesture plays (and briefly after), the real pointer is ignored so it
        // cannot interleave with the scripted one.
        if !self.synthetic.is_empty() {
            self.synthetic_grace = 6;
        }
        if self.synthetic_grace > 0 {
            self.synthetic_grace -= 1;
            raw.events.retain(|e| {
                !matches!(e, egui::Event::PointerMoved(_) | egui::Event::PointerButton { .. } | egui::Event::PointerGone | egui::Event::MouseMoved(_))
            });
        }
        let Some(first) = self.synthetic.first().cloned() else { return };
        let take = match first {
            egui::Event::Key { pressed: true, .. } => {
                // Press and the matching release together.
                if matches!(self.synthetic.get(1), Some(egui::Event::Key { pressed: false, .. })) { 2 } else { 1 }
            }
            _ => 1,
        };
        let n = take.min(self.synthetic.len());
        let evs: Vec<egui::Event> = self.synthetic.drain(..n).collect();
        for e in &evs {
            if let egui::Event::PointerButton { pos, .. } | egui::Event::PointerMoved(pos) = e {
                // Keep the hover position current for widgets that check it.
                raw.events.push(egui::Event::PointerMoved(*pos));
            }
        }
        raw.events.extend(evs);
    }

    /// Lay out the whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        // The integration creates this Ui before logic resolves the current frame's palette.
        ui.set_style(ctx.global_style());
        if !self.fonts_ready {
            ctx.request_repaint();
            return;
        }
        if self.arrange_request {
            self.arrange_request = false;
            ctx.memory_mut(|m| m.reset_areas());
        }
        window_sync::tidy(self);
        // Musical Typing takes its letter keys first (while it is on).
        musical_typing::handle_keys(self, &ctx);
        shortcuts::handle(self, &ctx);
        self.browser_rect = None;
        // The wide editor under the pointer claims its keys again as it draws.
        self.ui.wide_key_owner.remove(&ctx.viewport_id());
        self.sample_drop_targets.clear();
        self.kit_drop_targets.clear();
        self.kit_import_targets.clear();
        self.plugin_drop_targets.clear();
        if !self.native_menu_bar {
            menus::menu_bar(self, ui);
        }
        // A selection made by the menus or from outside shows in both windows.
        window_sync::watch_selection(self, None);
        // With the Mix window in its own native window, the main window keeps Edit.
        match if self.ui.mix_separate { MainWindow::Edit } else { self.ui.window } {
            MainWindow::Edit => edit_window::show(self, ui),
            MainWindow::Mix => mix_window::show(self, ui),
        }
        if self.ui.mix_separate {
            mix_window::show_separate(self, &ctx);
        }
        if self.ui.vocal_separate && self.ui.show_vocal_editor {
            vocal_editor::window::show_separate(self, &ctx);
        }
        if self.ui.teleprompter_separate && self.ui.show_teleprompter {
            teleprompter::show_separate(self, &ctx);
        }
        if self.ui.midi_editor_separate && self.ui.show_midi_editor {
            midi_editor_window::show_separate(self, &ctx);
        }
        panels::floating(self, &ctx);
        windows::show(self, &ctx);
        video_window::show(self, &ctx);
        ops_windows::show(self, &ctx);
        step_input::show(self, &ctx);
        extra_windows::show(self, &ctx);
        recovery::show(self, &ctx);
        score_editor::show(self, &ctx);
        scout::panel::show(self, &ctx);
        tour::show(self, &ctx);
        band::studio::show(self, &ctx);
        palette::show(self, &ctx);
        window_sync::settle(self);
        dialogs::show(self, &ctx);
        browser::drop::finish(self, &ctx);
        window_sync::watch_selection(self, None);
        // Here, not in logic(): a covered or minimized window runs only logic(), with no vsync to pace it.
        if self.meters_live() {
            let hz = if self.ui.window == MainWindow::Mix { 60 } else { 30 };
            ctx.request_repaint_after(std::time::Duration::from_millis(1000 / hz));
        } else if self.meters.values().any(|m| m.level[0] > 0.0001) {
            ctx.request_repaint_after(std::time::Duration::from_millis(80));
        } else if !self.pending_shots.is_empty() || !self.synthetic.is_empty() {
            ctx.request_repaint();
        }
    }

    /// UI state as JSON (control channel `ui.inspect`).
    pub fn inspect(&self, ctx: &egui::Context) -> Value {
        let size = ctx.content_rect().size();
        json!({
            "ui": self.ui,
            "window_size": [size.x, size.y],
            "playing": self.is_playing(),
            "position": self.position(),
            "dialog": self.dialogs.open_name(),
            "frame_ms": self.frame_ms,
            "audio_device": self.player.as_ref().map(|p| p.device_name.clone()),
            "status": self.ui.status,
            "midi_editor": midi_edit_tools::state(self),
        })
    }
}

/// Time constant of the RMS (and VU) average, seconds: a steady tone reaches 99 % of its level
/// in 300 ms, the VU's integration time.
const RMS_TAU: f32 = 0.065;

/// One display frame of a strip's meter: `peak` and `ms` (mean square over the blocks since the
/// last poll; NaN when the engine gave none) per channel, `dt` seconds since the last frame.
fn feed_meter(d: &mut MeterDisplay, peak: [f32; 2], ms: [f32; 2], gr: f32, dt: f32, b: preferences::Ballistics) {
    // Instant attack; release and peak hold from Preferences › Metering (26 dB/s, 2 s), or the
    // scale's own fall (a PPM's).
    let fall = 10f32.powf(-b.scale.own_release_db_s().unwrap_or(b.release_db_s) * dt / 20.0);
    let k = if dt.is_finite() && dt > 0.0 { 1.0 - (-dt / RMS_TAU).exp() } else { 0.0 };
    for c in 0..2 {
        let target = ms.get(c).copied().filter(|v| v.is_finite() && *v >= 0.0).unwrap_or(0.0);
        if let (Some(m), Some(r)) = (d.ms.get_mut(c), d.rms.get_mut(c)) {
            *m += (target - *m) * k;
            *r = if *m < 1e-10 { 0.0 } else { m.sqrt() };
        }
    }
    let rms_bar = b.scale.rms_bar();
    for c in 0..2 {
        let v = if rms_bar { d.rms.get(c).copied().unwrap_or(0.0) } else { peak.get(c).copied().unwrap_or(0.0) };
        let lvl = d.level.get(c).copied().unwrap_or(0.0);
        let nv = if rms_bar || v >= lvl { v } else { (lvl * fall).max(v) };
        if let Some(x) = d.level.get_mut(c) {
            *x = if nv < 1e-5 { 0.0 } else { nv };
        }
        // The hold line, the clip light and the peak readout follow the peak on every scale.
        let v = peak.get(c).copied().unwrap_or(0.0);
        if v >= d.hold.get(c).copied().unwrap_or(0.0) {
            if let Some(h) = d.hold.get_mut(c) {
                *h = v;
            }
            d.hold_age = 0.0;
        }
        if v >= 1.0 {
            d.clip = true;
        }
        if v.is_finite() && v > d.peak {
            d.peak = v;
        }
    }
    d.hold_age += dt;
    if b.hold_secs < preferences::HOLD_UNTIL_CLEARED && d.hold_age > b.hold_secs {
        d.hold = d.level;
    }
    d.gr = if gr > d.gr { gr } else { d.gr * fall };
}

/// Replaces what `slot` holds with `make()`, dropping the old one first: an audio device is
/// closed before the next is opened (ASIO allows one driver per process).
fn reopen<T>(slot: &mut Option<T>, make: impl FnOnce() -> T) {
    *slot = None;
    *slot = Some(make());
}

/// The status line naming the plugin `slot` (a module or the instrument) of a track, which the
/// engine took out of the signal path.
fn failed_plugin_status(s: &soundcraft_model::Session, track: TrackId, slot: Option<SlotKey>) -> Option<String> {
    let t = s.track(track)?;
    let ins = match slot? {
        SlotKey::Instrument => t.instrument.as_ref(),
        SlotKey::Module(m) => t.mixer.module(m),
    }?;
    let name = crate::mix_window::plugin_info(&ins.plugin).map_or(ins.plugin.as_str(), |p| p.name);
    Some(format!("{name} on {} put out invalid audio, so it is bypassed until playback stops or starts again.", t.name))
}

/// The status line after `track.freeze` / `track.unfreeze` returned `v`.
fn freeze_status(e: &Engine, v: &Value) -> String {
    let names = |key: &str| -> Vec<String> {
        let ids = v.get(key).and_then(Value::as_array).cloned().unwrap_or_default();
        ids.iter().filter_map(Value::as_u64).filter_map(|i| e.session().tracks.iter().find(|t| t.id.0 == i)).map(|t| t.name.clone()).collect()
    };
    let (frozen, thawed) = (names("frozen"), names("unfrozen"));
    if !frozen.is_empty() {
        let secs = v.get("seconds").and_then(Value::as_f64).unwrap_or(0.0);
        // Where the render can differ from live (a sidechain key from another track) is said.
        let warnings: Vec<&str> =
            v.get("warnings").and_then(Value::as_array).map(|w| w.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
        let mut line = format!("Froze {} ({secs:.1} s rendered)", frozen.join(", "));
        for w in warnings {
            line.push_str("; ");
            line.push_str(w);
        }
        line
    } else if !thawed.is_empty() {
        format!("Unfroze {}", thawed.join(", "))
    } else {
        "Nothing to freeze".into()
    }
}

/// `1.0`, for a serde default.
fn one_f32() -> f32 {
    1.0
}

#[cfg(test)]
mod tests {
    use super::{Services, SoundApp};
    use serde_json::json;

    #[test]
    fn meter_ballistics_follow_the_preferences() {
        use super::{
            MeterDisplay, feed_meter,
            preferences::{Ballistics, HOLD_UNTIL_CLEARED},
        };
        let run = |b: Ballistics, secs: f32| {
            let mut d = MeterDisplay::default();
            feed_meter(&mut d, [0.5, 0.5], [0.25, 0.25], 0.0, 1.0 / 60.0, b);
            for _ in 0..(secs * 60.0) as usize {
                feed_meter(&mut d, [0.0, 0.0], [0.0, 0.0], 0.0, 1.0 / 60.0, b);
            }
            d
        };
        // No hold: the hold line falls with the level at once.
        let none = run(Ballistics { release_db_s: 26.0, hold_secs: 0.0, scale: Default::default() }, 0.5);
        assert!(none.hold[0] < 0.5 && (none.hold[0] - none.level[0]).abs() < 1e-6);
        // Until cleared: still there after ten seconds.
        let held = run(Ballistics { release_db_s: 26.0, hold_secs: HOLD_UNTIL_CLEARED, scale: Default::default() }, 10.0);
        assert_eq!(held.hold[0], 0.5);
        // A faster release falls further in the same time.
        let slow = run(Ballistics { release_db_s: 12.0, hold_secs: 2.0, scale: Default::default() }, 0.5);
        let fast = run(Ballistics { release_db_s: 96.0, hold_secs: 2.0, scale: Default::default() }, 0.5);
        assert!(fast.level[0] < slow.level[0]);
        // An RMS scale: a steady 0.5 sine-like input (mean square 0.125) settles at its RMS within
        // 300 ms, while the hold line and the peak readout keep the peak.
        let rms = Ballistics { release_db_s: 26.0, hold_secs: HOLD_UNTIL_CLEARED, scale: crate::widgets::MeterScale::K20 };
        let mut d = MeterDisplay::default();
        for _ in 0..18 {
            feed_meter(&mut d, [0.5, 0.5], [0.125, 0.125], 0.0, 1.0 / 60.0, rms);
        }
        let want = 0.125f32.sqrt();
        assert!((d.level[0] - want).abs() < 0.02 * want, "{} vs {want}", d.level[0]);
        assert_eq!((d.hold[0], d.peak), (0.5, 0.5));
        assert!(!d.clip);
        // A PPM falls 24 dB in 2.8 s whatever the release preference, from the peak.
        let ppm = run(Ballistics { release_db_s: 96.0, hold_secs: 0.0, scale: crate::widgets::MeterScale::Ppm }, 2.8);
        let fell = 20.0 * (0.5 / ppm.level[0]).log10();
        assert!((fell - 24.0).abs() < 0.6, "fell {fell} dB");
        assert_eq!(crate::widgets::MeterScale::from_id("ppm"), Some(crate::widgets::MeterScale::Ppm));
    }

    #[test]
    fn new_session_stops_playback_and_the_old_clock() {
        for (command, params) in [
            ("session.new", json!({"name": "New project", "sample_rate": 96_000})),
            ("session.new", json!({"template": "demo"})),
            ("session.close", json!({})),
        ] {
            let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
            app.run("transport.play", json!({"from": 48_000})).unwrap();
            app.handle_transport(0.25);
            assert!(app.is_playing());
            assert!(app.position() > 48_000);
            app.run_sync(command, params).unwrap();
            let insertion = app.engine.session().edit.selection.start;
            app.handle_transport(0.25);
            assert!(!app.is_playing(), "{command}");
            assert_eq!(app.position(), insertion, "{command}");
            assert_eq!(app.engine.transport.position, insertion, "{command}");
            app.handle_transport(0.25);
            assert_eq!(app.engine.transport.position, insertion, "the old clock must stay stopped");
            app.run("transport.play", json!({})).unwrap();
            app.handle_transport(0.25);
            assert!(app.is_playing());
            assert_eq!(app.position(), insertion + i64::from(app.engine.session().sample_rate.hz()) / 4);
        }
    }

    #[test]
    fn shuttle_steps_double_and_backwards_moves_the_playhead_back() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.locate", json!({"at": 96_000})).unwrap();
        app.handle_transport(0.0);
        for want in [1.0, 2.0, 4.0, 4.0] {
            app.run("transport.shuttle_forward", json!({})).unwrap();
            app.handle_transport(0.0);
            assert_eq!(app.shuttle, want);
        }
        assert!(app.is_playing(), "forwards is a play");
        app.run("transport.shuttle_back", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.shuttle, -1.0);
        assert!(!app.is_playing(), "backwards stops the play and goes frame by frame");
        let at = app.position();
        app.shuttle_at = Some(std::time::Instant::now() - std::time::Duration::from_millis(100));
        app.handle_transport(0.0);
        let back = at - app.position();
        assert!((3_000..=12_000).contains(&back), "about 0.1 s back: {back}");
        app.run("transport.shuttle_stop", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.shuttle, 0.0);
        assert!(app.run("transport.shuttle", json!({"speed": 9})).is_err());
    }

    #[test]
    fn play_from_and_play_timeline_start_where_they_say() {
        // Bug: `transport.play {from}` played from the selection when stopped, and Play Timeline
        // played the edit selection.
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("edit.select", json!({"start": 0, "end": 0})).unwrap();
        app.run("transport.play", json!({"from": 96_000})).unwrap();
        app.handle_transport(0.0);
        assert!(app.is_playing());
        assert_eq!(app.position(), 96_000);
        app.stop_play();
        app.run("options.link_timeline_edit", json!({"value": false})).unwrap();
        app.run("edit.select", json!({"start": 0, "end": 1000})).unwrap();
        app.engine.session_mut().edit.timeline_selection = soundcraft_time::Range::new(120_000, 132_000);
        app.run("edit.play_timeline", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim, Some((120_000, Some(132_000), None)), "the timeline range, once");
        app.stop_play();
        app.run("options.loop_playback", json!({"value": true})).unwrap();
        app.run("edit.play_timeline", json!({})).unwrap();
        app.handle_transport(0.0);
        let r = soundcraft_time::Range::new(120_000, 132_000);
        assert_eq!(app.sim, Some((120_000, None, Some(r))), "looped with Loop Playback");
    }

    #[test]
    fn reopening_closes_the_old_device_before_opening_the_new() {
        // Bug: Reconnect opened the new device while the old one still streamed.
        use std::cell::RefCell;
        use std::rc::Rc;
        struct Dev(Rc<RefCell<Vec<&'static str>>>, &'static str);
        impl Drop for Dev {
            fn drop(&mut self) {
                self.0.borrow_mut().push(if self.1 == "old" { "close old" } else { "close new" });
            }
        }
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut slot = Some(Dev(log.clone(), "old"));
        let l = log.clone();
        super::reopen(&mut slot, move || {
            l.borrow_mut().push("open new");
            Dev(l.clone(), "new")
        });
        assert_eq!(*log.borrow(), ["close old", "open new"]);
    }

    #[test]
    fn loop_changes_take_effect_while_playing() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("edit.select", json!({"start": 48_000, "end": 96_000})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim.map(|s| s.2), Some(None), "plays the selection straight");
        app.run("options.loop_playback", json!({"value": true})).unwrap();
        app.handle_transport(0.0);
        let r = soundcraft_time::Range::new(48_000, 96_000);
        assert_eq!(app.sim.map(|s| s.2), Some(Some(r)), "Loop Playback on: the loop range loops");
        app.run("loop.set", json!({"start": 48_000, "end": 72_000})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim.map(|s| s.2), Some(Some(soundcraft_time::Range::new(48_000, 72_000))), "resized while playing");
        app.run("options.loop_playback", json!({"value": false})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim.map(|s| s.2), Some(None), "off: plays on");
        assert!(app.is_playing());
    }

    #[test]
    fn dynamic_transport_plays_from_the_play_start_marker() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("options.dynamic_transport", json!({"value": true})).unwrap();
        app.run("edit.select", json!({"start": 48_000, "end": 96_000})).unwrap();
        app.run("transport.set_play_start", json!({"at": 10_000})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim, Some((10_000, None, None)), "from the marker, not the selection, and on past its end");
        app.stop_play();
        // Loop Playback loops the loop range, from the marker when it is inside it.
        app.run("loop.set", json!({"start": 5_000, "end": 20_000})).unwrap();
        app.run("options.loop_playback", json!({"value": true})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        let lp = soundcraft_time::Range::new(5_000, 20_000);
        assert_eq!(app.sim, Some((10_000, None, Some(lp))));
        app.stop_play();
        app.run("transport.set_play_start", json!({"at": 30_000})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim, Some((5_000, None, Some(lp))), "a marker outside the loop starts at the loop");
    }

    #[test]
    fn play_selection_with_an_insertion_point_plays_on() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("edit.select", json!({"start": 48_000, "end": 48_000})).unwrap();
        app.run("transport.play_selection", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(app.is_playing(), "not one sample and stop");
        assert_eq!(app.position(), 48_000 + i64::from(app.engine.session().sample_rate.hz()) / 4);
        app.stop_play();
        app.run("edit.select", json!({"start": 48_000, "end": 60_000})).unwrap();
        app.run("transport.play_selection", json!({})).unwrap();
        app.handle_transport(0.0);
        assert_eq!(app.sim, Some((48_000, Some(60_000), None)));
    }

    #[test]
    fn the_status_names_a_plugin_the_engine_took_out() {
        use soundcraft_model::{ChannelFormat, Insert, TrackKind};
        let mut s = soundcraft_model::Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let m = s.track_mut(t).unwrap().mixer.add_module(Insert::new("gain"), None).unwrap();
        let msg = super::failed_plugin_status(&s, t, Some(soundcraft_mix::SlotKey::Module(m))).unwrap();
        assert!(msg.starts_with("Gain on Vox put out invalid audio"), "{msg}");
        let gone = soundcraft_model::ModuleId(m.0 + 1);
        assert_eq!(super::failed_plugin_status(&s, t, Some(soundcraft_mix::SlotKey::Module(gone))), None);
        assert_eq!(super::failed_plugin_status(&s, t, None), None);
        assert_eq!(super::failed_plugin_status(&s, t, Some(soundcraft_mix::SlotKey::Instrument)), None);
    }

    #[test]
    fn a_new_session_closes_the_old_ones_drawers_and_plugin_windows() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").cloned().unwrap();
        let m = kick.mixer.inserts[0].id;
        app.ui.drawers.insert(kick.id, m);
        app.ui.band_drawers.insert(kick.id);
        app.ui.gr_history.insert((kick.id, m), super::module_drawer::GrHistory::default());
        app.ui.plugin_windows.push((kick.id, super::SlotKey::Module(m)));
        app.run_sync("session.new", json!({"template": "demo"})).unwrap();
        assert!(app.ui.drawers.is_empty(), "the new session's track and module ids can match the old ones");
        assert!(app.ui.band_drawers.is_empty());
        assert!(app.ui.gr_history.is_empty());
        assert!(app.ui.plugin_windows.is_empty());
    }

    #[test]
    fn new_session_cancels_play_queued_before_the_next_frame() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({})).unwrap();
        app.run_sync("session.new", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(!app.is_playing());
        assert_eq!(app.engine.transport.position, 0);
    }

    #[test]
    fn record_with_nothing_armed_says_what_to_do() {
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(!app.is_playing() && !app.engine.transport.recording);
        assert!(app.record_hint().is_some_and(|why| why.contains("Track › New")), "{:?}", app.record_hint());

        app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        assert!(app.record_hint().is_some_and(|why| why.contains("R button")), "{:?}", app.record_hint());
        assert_eq!(Some(app.ui.status.as_str()), app.record_hint());
        app.last_frame = Some(5.0);
        assert_eq!(app.record_hint(), None, "the hint shows for a few seconds");

        app.last_frame = None;
        app.run("transport.record", json!({})).unwrap();
        app.handle_transport(0.25);
        app.run("mix.record_arm", json!({})).unwrap();
        assert_eq!(app.record_hint(), None, "arming a track answers the hint");
    }

    #[test]
    fn a_hidden_window_still_autosaves() {
        let dir = std::env::temp_dir().join(format!("soundcraft-hidden-autosave-{}", std::process::id()));
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.autosave_dir = Some(dir.clone());
        app.run("track.new", json!({"count": 1, "format": "mono"})).unwrap();
        app.last_autosave = std::time::Instant::now().checked_sub(std::time::Duration::from_secs(121));
        let hidden = egui::ViewportInfo { minimized: Some(true), ..Default::default() };
        let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, hidden)].into_iter().collect(), ..Default::default() };
        let _ = egui::Context::default().run_logic(&raw, |ctx| app.logic(ctx));
        assert!(app.latest_autosave().is_some());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn session_reset_does_not_apply_old_stop_edits_to_the_new_session() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.run("transport.play", json!({"from": 48_000})).unwrap();
        app.handle_transport(0.25);
        let mut session = soundcraft_model::Session::default();
        session.edit.selection = soundcraft_time::Range::new(12_000, 24_000);
        session.edit.insertion_follows_playback = true;
        session.edit.scrolling = "after_playback".into();
        let selection = session.edit.selection;
        app.engine.replace_session(session);
        app.handle_transport(0.25);
        assert!(!app.is_playing());
        assert_eq!(app.engine.session().edit.selection, selection);
        assert_eq!(app.position(), selection.start);
        assert_eq!(app.engine.session().edit.zoom.scroll, 0);
        assert!(!app.engine.is_dirty());
    }

    #[test]
    fn clear_all_clip_indicators_clears_every_meter() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.meters.entry(kick).or_default().clip = true;
        app.main_meter.clip = true;
        app.run("track.clear_clip_indicators", json!({})).unwrap();
        assert!(!app.meters[&kick].clip && !app.main_meter.clip);
        // The control channel and MCP (`run_sync`) clear them too.
        app.meters.entry(kick).or_default().clip = true;
        app.run_sync("track.clear_clip_indicators", json!({})).unwrap();
        assert!(!app.meters[&kick].clip);
    }

    #[test]
    fn a_write_pass_replaces_the_automation_it_plays_over() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let fader = app.engine.session().track_by_name("Kick").unwrap().mixer.volume_db;
        let q = i64::from(app.engine.session().sample_rate.hz()) / 4;
        // The old ride: a point every 10 ms for 3 s.
        for at in (0..12 * q).step_by(usize::try_from(q / 25).unwrap()) {
            app.run("automation.set_point", json!({"track": "Kick", "at": at, "value": -30.0})).unwrap();
        }
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
        app.run("transport.locate", json!({"at": 0})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        // Passes a quarter second apart, sparse like a covered window's.
        let ctx = egui::Context::default();
        let pass = |app: &mut SoundApp| {
            app.handle_transport(0.25);
            app.write_automation(&ctx);
        };
        pass(&mut app);
        pass(&mut app);
        // Two jumps: the stretches they skip keep the old ride.
        app.run("transport.locate", json!({"at": 4 * q})).unwrap();
        pass(&mut app);
        app.run("edit.select", json!({"tracks": ["Kick"], "start": 7 * q, "end": 11 * q})).unwrap();
        app.run("transport.play_selection", json!({})).unwrap();
        pass(&mut app);
        app.handle_transport(0.25);
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.write_automation(&ctx);
        let lane = app.engine.session().track_by_name("Kick").unwrap().lane(&soundcraft_model::AutoParam::Volume).unwrap();
        let values = |from, to| lane.points.iter().filter(|p| (from..=to).contains(&p.at)).map(|p| p.value).collect::<Vec<_>>();
        assert_eq!(values(q, 2 * q), [fader; 2]);
        assert_eq!(lane.value_at(3 * q, 0.0), -30.0);
        assert_eq!(lane.value_at(6 * q, 0.0), -30.0);
        // Stop writes the last point, at 9q.
        assert_eq!(values(8 * q, 9 * q), [fader; 2]);
        assert_eq!(lane.value_at(10 * q, 0.0), -30.0);
    }

    #[test]
    fn a_control_held_in_touch_mode_writes_its_own_lane_standing_still() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "touch"})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        let ctx = egui::Context::default();
        let pan = soundcraft_model::AutoParam::Pan(0);
        for _ in 0..30 {
            // What the Mix strip's pan knob reports while the pointer holds it.
            app.hold_control(kick, pan.clone());
            app.handle_transport(0.1);
            app.write_automation(&ctx);
        }
        let held = app.engine.session().track(kick).unwrap().lane(&pan).map_or(0, |l| l.points.len());
        assert!(held >= 25, "{held} points");
        // Let go: no more points.
        for _ in 0..10 {
            app.handle_transport(0.1);
            app.write_automation(&ctx);
        }
        assert_eq!(app.engine.session().track(kick).unwrap().lane(&pan).map_or(0, |l| l.points.len()), held);
        assert!(app.engine.session().track(kick).unwrap().lane(&soundcraft_model::AutoParam::Volume).is_none(), "only what was held is written");
    }

    #[test]
    fn a_pass_and_the_fader_ride_driving_it_are_one_undo_step() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let kick = app.engine.session().track_by_name("Kick").unwrap().id;
        app.run("mix.automation_mode", json!({"tracks": ["Kick"], "mode": "write"})).unwrap();
        app.run("transport.play", json!({})).unwrap();
        let steps = app.engine.undo_history().len();
        let ctx = egui::Context::default();
        for frame in 0..60 {
            app.handle_transport(1.0 / 60.0);
            app.write_automation(&ctx);
            // What the Mix window's fader sends while it is dragged.
            let db = -10.0 + frame as f32 / 10.0;
            app.engine.execute_merged("mix.volume", &json!({"track": kick.0, "db": db}), &format!("fader:{}", kick.0)).unwrap();
        }
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        app.write_automation(&ctx);
        assert_eq!(app.engine.undo_history().len(), steps + 1);
    }

    #[test]
    fn older_settings_get_the_pitch_view_once() {
        // A ui.json saved before the Pitch view: no `mix_views_offered`, no "pitch".
        let mut ui: super::UiState = serde_json::from_value(json!({"mix_views": ["io", "color"]})).unwrap();
        assert!(ui.mix_views_offered.is_empty());
        assert!(ui.offer_new_mix_views());
        assert_eq!(ui.mix_views, ["io", "color", "pitch"]);
        assert_eq!(ui.mix_views_offered, ["pitch"]);
        // Turned off and saved: it stays off on the next load.
        ui.mix_views.retain(|v| v != "pitch");
        let mut back: super::UiState = serde_json::from_value(serde_json::to_value(&ui).unwrap()).unwrap();
        assert!(!back.offer_new_mix_views());
        assert_eq!(back.mix_views, ["io", "color"]);
        // Fresh settings already have it and say so.
        let mut fresh = super::UiState::default();
        assert!(!fresh.offer_new_mix_views());
        assert!(fresh.mix_views.iter().any(|v| v == "pitch"));
    }

    #[test]
    fn preferences_saved_before_the_window_was_remembered_keep_full_scale() {
        let ui: super::UiState = serde_json::from_value(json!({"show_transport": true})).unwrap();
        assert_eq!((ui.window_size, ui.window_maximized, ui.ui_scale), ([1600.0, 1000.0], false, 1.0));
        assert!(ui.show_splash, "the splash is on until turned off");
        let ui: super::UiState = serde_json::from_value(json!({"show_splash": false})).unwrap();
        assert!(!ui.show_splash);
    }

    #[test]
    fn the_splash_remembers_its_last_statement_as_splash_last_statement() {
        let ui: super::UiState = serde_json::from_value(json!({"show_transport": true})).unwrap();
        assert_eq!(ui.splash.last_statement, None, "older settings files have none");
        let mut ui = super::UiState::default();
        ui.splash.last_statement = Some(7);
        let saved = serde_json::to_value(&ui).unwrap();
        assert_eq!(saved["splash"]["last_statement"], json!(7));
        let back: super::UiState = serde_json::from_value(saved).unwrap();
        assert_eq!(back.splash.last_statement, Some(7));
    }

    #[test]
    fn a_window_bigger_than_its_screen_opens_maximized() {
        let maximizes = |w: f32, frames: usize| {
            let ctx = egui::Context::default();
            let info = egui::ViewportInfo {
                inner_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(w, 1000.0))),
                monitor_size: Some(egui::vec2(1728.0, 1117.0)),
                ..Default::default()
            };
            let mut maximized = false;
            for _ in 0..frames {
                let raw = egui::RawInput { viewports: [(egui::ViewportId::ROOT, info.clone())].into_iter().collect(), ..Default::default() };
                let mut out = ctx.run_ui(raw, |ui| super::fit_window_to_screen(ui.ctx()));
                out.textures_delta.clear();
                maximized = out.viewport_output[&egui::ViewportId::ROOT].commands.contains(&egui::ViewportCommand::Maximized(true));
            }
            maximized
        };
        assert!(maximizes(2400.0, 1));
        assert!(!maximizes(1600.0, 1));
        assert!(!maximizes(2400.0, 2));
    }

    #[test]
    fn ordinary_play_after_stop_is_full_speed() {
        use serde_json::json;
        let engine = soundcraft_engine::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
        app.engine.execute("edit.select", &json!({"start": 0, "end": 48000})).unwrap();
        app.engine.execute("options.loop_playback", &json!({"value": true})).unwrap();
        app.engine.execute("options.pre_post_roll", &json!({"value": false})).unwrap();
        let speed = |app: &super::SoundApp| app.player.as_ref().map(|p| p.speed()).unwrap_or(0.0);
        assert!((speed(&app) - 1.0).abs() < 1e-3);

        app.engine.execute("transport.half_speed", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing());
        assert!((speed(&app) - 0.5).abs() < 1e-3, "half-speed playback");

        app.engine.execute("transport.stop", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(!app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "stop restores full speed");

        app.engine.execute("transport.play", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "ordinary play stays at full speed");

        app.engine.execute("transport.stop", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        app.engine.execute("options.loop_playback", &json!({"value": false})).unwrap();
        app.engine.execute("transport.half_speed", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!((speed(&app) - 0.5).abs() < 1e-3);
        app.player.as_ref().unwrap().stop();
        app.handle_transport(1.0 / 60.0);
        assert!(!app.is_playing());
        assert!((speed(&app) - 1.0).abs() < 1e-3, "a half-speed pass that ends on its own restores full speed");
    }

    #[test]
    fn a_device_reopen_forgets_the_measurement_out_there() {
        // It would never come back from the new player, and Measure would say "already running".
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
        app.measuring = Some("old".into());
        app.apply_audio_config(soundcraft_playback::AudioConfig::default()).unwrap();
        assert!(!app.measuring());
        app.measuring = Some("old".into());
        app.reconnect_audio();
        assert!(!app.measuring());
    }

    #[test]
    fn cancel_and_closing_the_window_abandon_a_measure() {
        use serde_json::json;
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
        app.finish_measure(Ok(300));
        let key = app.record_offset_key();
        app.measuring = Some(key.clone());
        assert_eq!(app.run("setup.measure_cancel", json!({})).unwrap(), json!({"cancelled": true}));
        assert!(!app.measuring());
        assert_eq!(app.ui.status, "Measure cancelled");
        assert_eq!(app.run("setup.measure_cancel", json!({})).unwrap(), json!({"cancelled": false}), "none running");
        assert_eq!(app.ui.record_offsets[&key].measured, Some(300), "the previous value stays");
        // Closing the Playback Engine window mid-measure cancels it, however it closes.
        let ctx = egui::Context::default();
        app.ui.show_playback_engine = true;
        ctx.run_ui(egui::RawInput::default(), |ui| crate::windows::playback_engine(&mut app, &ui.ctx().clone())).textures_delta.clear();
        app.measuring = Some(key.clone());
        app.ui.show_playback_engine = false;
        ctx.run_ui(egui::RawInput::default(), |ui| crate::windows::playback_engine(&mut app, &ui.ctx().clone())).textures_delta.clear();
        assert!(!app.measuring(), "closed mid-measure");
        assert_eq!(app.ui.record_offsets[&key].measured, Some(300));
    }

    #[test]
    fn the_offset_key_does_not_wait_for_the_input_to_open() {
        // Resolved by the player at open: neither opening the input nor an unapplied input
        // choice moves it (a manual offset set now is the one takes use).
        let engine = soundcraft_engine::Engine::default();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
        let key = app.record_offset_key();
        let p = app.player.as_ref().unwrap();
        let device = match p.input_name.as_deref() {
            Some(i) if i != p.device_name => format!("{} + {i}", p.device_name),
            _ => p.device_name.clone(),
        };
        assert_eq!(key, soundcraft_playback::devices::latency_key(&p.system, &device, p.device_buffer, p.device_rate));
        app.ui.audio.input = Some("Some Other Input".into());
        assert_eq!(app.record_offset_key(), key);
    }

    #[test]
    fn a_take_keeps_the_latencies_of_its_first_frame() {
        // Once snapshotted, the devices' live figures no longer move the take's offset.
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
        app.take_reported = Some(soundcraft_playback::Latency { input: 100, output: 120, feed: 30, mix: 64 });
        assert_eq!(app.take_latency(), 314);
        assert_eq!(app.take_latency(), 314);
    }

    #[test]
    fn the_browser_build_says_it_cannot_record() {
        // Only the web build is refused, and the reason names what to do instead.
        assert_eq!(super::record_unavailable(false), None);
        let why = super::record_unavailable(true).unwrap();
        assert!(why.contains("cannot record") && why.contains("desktop app"), "{why}");
    }

    #[test]
    fn recording_is_refused_at_half_speed() {
        use serde_json::json;
        let mut engine = soundcraft_engine::Engine::default();
        let t = engine.session_mut().add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, None);
        engine.session_mut().track_mut(t).unwrap().mixer.record_arm = true;
        let player = soundcraft_playback::Player::new(engine.session_arc());
        let mut app = super::SoundApp::new(engine, Some(player), super::Services::default());
        app.engine.execute("transport.half_speed", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        app.engine.execute("transport.record", &json!({})).unwrap();
        app.handle_transport(1.0 / 60.0);
        assert!(!app.engine.transport.recording);
        assert_eq!(app.ui.status, "Cannot record: Recording needs full speed: at half speed the take would lose input. Stop, then record.");
    }

    /// Options › Recording › Lock Record: the first Stop during a take is refused and says so,
    /// a second one soon after stops; without the lock one Stop stops.
    #[test]
    fn a_locked_take_stops_on_the_second_stop() {
        use serde_json::json;
        let mut app = super::SoundApp::new(soundcraft_engine::Engine::default(), None, super::Services::default());
        app.engine.transport.recording = true;
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        assert!(!app.engine.transport.recording, "unlocked: one Stop stops");
        app.run("options.record_lock", json!({"value": true})).unwrap();
        app.engine.transport.recording = true;
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        assert!(app.engine.transport.recording, "locked: the first Stop is refused");
        assert!(app.ui.status.contains("locked"), "{}", app.ui.status);
        app.run("transport.stop", json!({})).unwrap();
        app.handle_transport(0.0);
        assert!(!app.engine.transport.recording, "a second Stop stops");
    }
}

//! Tracks and playlists.

use crate::{AutoParam, AutomationLane, ChannelFormat, Clip, ClipId, Mixer, ModuleId, SourceId, TrackId};
use soundcraft_time::{Range, Samples};

/// Track types (Track › New).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum TrackKind {
    #[default]
    Audio,
    Aux,
    Master,
    Midi,
    Instrument,
    Vca,
    Folder,
    /// Picture: clips reference a movie ([`crate::VideoSource`]); no audio path.
    Video,
}

impl TrackKind {
    pub const ALL: [TrackKind; 8] = [
        TrackKind::Audio,
        TrackKind::Aux,
        TrackKind::Master,
        TrackKind::Midi,
        TrackKind::Instrument,
        TrackKind::Vca,
        TrackKind::Folder,
        TrackKind::Video,
    ];
    pub fn label(self) -> &'static str {
        match self {
            TrackKind::Audio => "Audio Track",
            TrackKind::Aux => "Aux Input",
            TrackKind::Master => "Master Fader",
            TrackKind::Midi => "MIDI Track",
            TrackKind::Instrument => "Instrument Track",
            TrackKind::Vca => "VCA Master",
            TrackKind::Folder => "Routing Folder Track",
            TrackKind::Video => "Video Track",
        }
    }
    pub fn default_name(self) -> &'static str {
        match self {
            TrackKind::Audio => "Audio",
            TrackKind::Aux => "Aux",
            TrackKind::Master => "Master",
            TrackKind::Midi => "MIDI",
            TrackKind::Instrument => "Inst",
            TrackKind::Vca => "VCA",
            TrackKind::Folder => "Folder",
            TrackKind::Video => "Video",
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            TrackKind::Audio => "audio",
            TrackKind::Aux => "aux",
            TrackKind::Master => "master",
            TrackKind::Midi => "midi",
            TrackKind::Instrument => "instrument",
            TrackKind::Vca => "vca",
            TrackKind::Folder => "folder",
            TrackKind::Video => "video",
        }
    }
    pub fn from_id(s: &str) -> Option<TrackKind> {
        let s = s.to_ascii_lowercase();
        TrackKind::ALL.into_iter().find(|k| k.id() == s || k.label().eq_ignore_ascii_case(&s))
    }
    /// Track kinds that hold clips on a timeline.
    pub fn has_playlist(self) -> bool {
        matches!(self, TrackKind::Audio | TrackKind::Midi | TrackKind::Instrument | TrackKind::Video)
    }
    pub fn is_midi(self) -> bool {
        matches!(self, TrackKind::Midi | TrackKind::Instrument)
    }
    /// Kinds that pass audio through the mixer.
    pub fn is_audio_path(self) -> bool {
        matches!(self, TrackKind::Audio | TrackKind::Aux | TrackKind::Master | TrackKind::Instrument | TrackKind::Folder)
    }
}

/// Track heights (View › Track Height presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum TrackHeight {
    /// Any height from Micro to Extreme, in points.
    Custom(u16),
    Micro,
    Mini,
    Small,
    #[default]
    Medium,
    Large,
    Jumbo,
    Extreme,
    /// Fit To Window: this many points.
    Fit(u16),
}

impl TrackHeight {
    pub const ALL: [TrackHeight; 7] = [
        TrackHeight::Micro,
        TrackHeight::Mini,
        TrackHeight::Small,
        TrackHeight::Medium,
        TrackHeight::Large,
        TrackHeight::Jumbo,
        TrackHeight::Extreme,
    ];
    pub fn points(self) -> f32 {
        match self {
            TrackHeight::Custom(points) => f32::from(points).clamp(TrackHeight::Micro.points(), TrackHeight::Extreme.points()),
            TrackHeight::Micro => 16.0,
            TrackHeight::Mini => 24.0,
            TrackHeight::Small => 46.0,
            TrackHeight::Medium => 88.0,
            TrackHeight::Large => 132.0,
            TrackHeight::Jumbo => 220.0,
            TrackHeight::Extreme => 440.0,
            TrackHeight::Fit(points) => f32::from(points.max(16)),
        }
    }
    pub fn label(self) -> &'static str {
        match self {
            TrackHeight::Custom(_) => "custom",
            TrackHeight::Micro => "micro",
            TrackHeight::Mini => "mini",
            TrackHeight::Small => "small",
            TrackHeight::Medium => "medium",
            TrackHeight::Large => "large",
            TrackHeight::Jumbo => "jumbo",
            TrackHeight::Extreme => "extreme",
            TrackHeight::Fit(_) => "fit to window",
        }
    }
    pub fn from_id(s: &str) -> Option<TrackHeight> {
        TrackHeight::ALL.into_iter().find(|h| h.label().eq_ignore_ascii_case(s))
    }
    /// The preset this many points tall, else a custom height clamped to Micro..Extreme.
    pub fn from_points(points: f32) -> TrackHeight {
        let p = points.max(TrackHeight::Micro.points()).min(TrackHeight::Extreme.points()).round() as u16;
        TrackHeight::ALL.into_iter().find(|h| h.points() == f32::from(p)).unwrap_or(TrackHeight::Custom(p))
    }
    /// The next preset taller (or shorter) than this height; Extreme (or Micro) stays.
    pub fn step(self, taller: bool) -> TrackHeight {
        let p = self.points();
        let next =
            if taller { TrackHeight::ALL.into_iter().find(|h| h.points() > p) } else { TrackHeight::ALL.into_iter().rev().find(|h| h.points() < p) };
        next.unwrap_or(self)
    }
}

/// The default track colour palette (original, chosen for SoundCraft).
pub const TRACK_COLORS: [[u8; 3]; 16] = [
    [86, 132, 214],
    [94, 170, 214],
    [80, 190, 168],
    [104, 186, 98],
    [168, 196, 76],
    [222, 196, 72],
    [230, 152, 62],
    [222, 104, 72],
    [214, 82, 110],
    [196, 86, 168],
    [150, 98, 206],
    [110, 104, 214],
    [130, 150, 170],
    [178, 140, 110],
    [120, 170, 140],
    [190, 120, 150],
];

/// A playlist: an alternative take of a track's clips.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Playlist {
    pub name: String,
    /// Clips sorted by start; non-overlapping except at crossfades.
    pub clips: Vec<Clip>,
    /// Comping: the take colour of this playlist shown as a layer, an index into the UI's fixed
    /// palette of 8 (cycling). `None` (older sessions): the layer's position picks it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub take_color: Option<u8>,
}

impl Playlist {
    pub fn new(name: impl Into<String>) -> Self {
        Playlist { name: name.into(), clips: Vec::new(), take_color: None }
    }
    pub fn sort(&mut self) {
        self.clips.sort_by_key(|c| (c.start, c.id.0));
    }
    pub fn clip(&self, id: ClipId) -> Option<&Clip> {
        self.clips.iter().find(|c| c.id == id)
    }
    pub fn clip_mut(&mut self, id: ClipId) -> Option<&mut Clip> {
        self.clips.iter_mut().find(|c| c.id == id)
    }
    pub fn clips_in(&self, r: Range) -> impl Iterator<Item = &Clip> {
        self.clips.iter().filter(move |c| c.range().overlaps(&r))
    }
    pub fn clip_at(&self, at: Samples) -> Option<&Clip> {
        self.clips.iter().rev().find(|c| c.range().contains(at))
    }
    pub fn end(&self) -> Samples {
        self.clips.iter().map(Clip::end).max().unwrap_or(0)
    }
}

/// A frozen track's render (Track › Freeze): its clips through its module chain (the whole chain,
/// or down to and including `upto`), pre-fader, as one audio source played from `start` in place
/// of the clips and those modules. Modules after `upto` stay live.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct FrozenAudio {
    pub source: SourceId,
    /// Timeline position of the source's first sample.
    #[serde(default)]
    pub start: Samples,
    /// The freeze point: the last module the render holds. `None` (and sessions from before
    /// "Freeze up to here"): the whole chain.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub upto: Option<ModuleId>,
}

/// A track.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Track {
    pub id: TrackId,
    pub name: String,
    pub kind: TrackKind,
    pub format: ChannelFormat,
    pub color: [u8; 3],
    pub height: TrackHeight,
    pub playlists: Vec<Playlist>,
    pub active_playlist: usize,
    pub mixer: Mixer,
    pub automation: Vec<AutomationLane>,
    /// Which lane the track shows: `waveform`/`blocks`/`notes`/`regions` or an automation param id.
    pub view: String,
    /// The automation lanes shown under the track's main lane, top to bottom, by param id
    /// ([`crate::AutoParam::id`]), as Pro Tools' automation lanes.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub auto_lanes: Vec<String>,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub inactive: bool,
    /// Frozen: plays [`Track::freeze`] instead of its clips and modules. Kept in step with
    /// `freeze` (a session that names no frozen audio loads unfrozen).
    #[serde(default)]
    pub frozen: bool,
    /// The frozen render while `frozen`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub freeze: Option<FrozenAudio>,
    #[serde(default)]
    pub comments: String,
    /// Folder that contains this track.
    #[serde(default)]
    pub folder: Option<TrackId>,
    #[serde(default = "yes")]
    pub folder_open: bool,
    /// Instrument plugin (instrument tracks) — registry id.
    #[serde(default)]
    pub instrument: Option<crate::Insert>,
    /// Ticks (musical) or samples timebase for clips.
    #[serde(default)]
    pub ticks_timebase: bool,
    /// Elastic Audio algorithm, if enabled.
    #[serde(default)]
    pub elastic: Option<String>,
    /// Pinned to top in the Edit window.
    #[serde(default)]
    pub pinned: bool,
    /// Lyrics as the first build of Transcribe Lyrics stored them (absolute session samples): read
    /// only, moved onto the clips under them when a session loads (`Session::sanitize`); never
    /// written. Lyrics live on clips ([`crate::Clip::lyrics`]).
    #[serde(default, skip_serializing, deserialize_with = "crate::lyrics::de_lyrics")]
    pub lyrics: Vec<crate::LyricLine>,
    /// The Vocal flag (Track › Vocal Track): `None` follows the name ([`crate::looks_vocal`]);
    /// the owner's choice always wins. See [`Track::is_vocal`].
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub vocal: Option<bool>,
    /// Comping: the take layers (every playlist but the active one, which is the comp) are shown
    /// under the track.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub layers_open: bool,
    /// Comping: the layer (playlist index) the next recording pass goes into instead of a new
    /// layer; used once. `None`: each pass makes a new layer.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub record_layer: Option<usize>,
    /// MIDI effects ahead of the instrument (instrument and MIDI tracks), run in this order on
    /// everything the instrument is given; at most [`crate::MAX_MIDI_FX`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub midi_fx: Vec<crate::MidiFx>,
    /// LFOs and envelope followers that move its plug-ins' parameters, at most
    /// [`crate::MAX_MODULATORS`].
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub modulators: Vec<crate::Modulator>,
    /// The input menu's MIDI Effects › Record Effects' Output: a take on this track keeps what
    /// its MIDI effects made of the keys (the clip then plays past them, `Clip::midi_fx_printed`)
    /// instead of the keys as played (off, the default).
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub midi_fx_record: bool,
    /// The MPE zone its instrument is set up for (the input menu's MPE Zone): the MPE
    /// Configuration Message and pitch-bend ranges are sent to it (`None`: not MPE, nothing sent).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mpe: Option<MpeZone>,
    /// Sound variations (an expression map): the articulations its instrument switches between
    /// by keyswitch, chosen in its clips by a controller lane (`None`: none).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub articulations: Option<Articulations>,
    /// The instrument's extra outputs taken by mixer channels (`Plugin::extra_outputs`): each
    /// plays into a bus, which an aux reads (the track's output menu › New Track for Output).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub instrument_outs: Vec<InstrumentOut>,
    /// Track › Lock Track: no command may add, change or remove a clip on any of its playlists
    /// (Insert and Cut Time still move them); recording into it is refused too.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub edit_locked: bool,
    /// Comping: the take layer (playlist index) auditioned in place of the comp, its solo on
    /// in the layer's header, until it is let go. Not saved.
    #[serde(skip)]
    pub audition_layer: Option<usize>,
    /// The note editor's drum map: a name per pitch (pitch, name), sorted by pitch, set by the
    /// user. Overrides the instrument's own drum names; on any other instrument it turns the drum
    /// view on. Absent in sessions saved before.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub drum_map: Vec<(u8, String)>,
    /// Track › Global Transpose › Independent: the track's notes ignore the transpose track.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub transpose_independent: bool,
}

fn yes() -> bool {
    true
}

impl Track {
    /// A module by id: a chain module, or an effect the instrument hosts on its kit buses
    /// ([`crate::KitFx`]).
    pub fn module(&self, id: ModuleId) -> Option<&crate::Insert> {
        self.mixer.module(id).or_else(|| {
            crate::KitFx::place(id)?;
            self.instrument.as_ref()?.kit_fx.iter().map(|f| &f.insert).find(|i| i.id == id)
        })
    }

    pub fn module_mut(&mut self, id: ModuleId) -> Option<&mut crate::Insert> {
        if crate::KitFx::place(id).is_some() {
            return self.instrument.as_mut()?.kit_fx.iter_mut().map(|f| &mut f.insert).find(|i| i.id == id);
        }
        self.mixer.module_mut(id)
    }

    pub fn new(id: TrackId, name: impl Into<String>, kind: TrackKind, format: ChannelFormat, color: [u8; 3]) -> Self {
        let name = name.into();
        let format = if kind.is_midi() && kind == TrackKind::Midi { ChannelFormat::Mono } else { format };
        let view = if kind.is_midi() {
            "notes"
        } else if kind.has_playlist() {
            "waveform"
        } else {
            "volume"
        };
        Track {
            id,
            playlists: vec![Playlist::new(name.clone())],
            name,
            kind,
            format,
            color,
            height: TrackHeight::Medium,
            active_playlist: 0,
            mixer: Mixer::new(format),
            automation: Vec::new(),
            view: view.into(),
            auto_lanes: Vec::new(),
            hidden: false,
            inactive: false,
            frozen: false,
            freeze: None,
            comments: String::new(),
            folder: None,
            folder_open: true,
            instrument: None,
            ticks_timebase: kind.is_midi(),
            elastic: None,
            pinned: false,
            lyrics: Vec::new(),
            vocal: None,
            layers_open: false,
            record_layer: None,
            midi_fx: Vec::new(),
            modulators: Vec::new(),
            midi_fx_record: false,
            mpe: None,
            articulations: None,
            instrument_outs: Vec::new(),
            edit_locked: false,
            audition_layer: None,
            drum_map: Vec::new(),
            transpose_independent: false,
        }
    }

    /// Whether the track's notes follow the global transpose track: MIDI and instrument tracks
    /// not set to Independent, whose instrument is not a drum machine (Juke, the drum synth).
    pub fn follows_global_transpose(&self) -> bool {
        self.kind.is_midi()
            && !self.transpose_independent
            && !self.instrument.as_ref().is_some_and(|i| matches!(i.plugin.as_str(), "juke" | "drum_synth"))
    }

    /// Whether the track is a vocal track: the owner's flag, else its name; audio tracks only.
    pub fn is_vocal(&self) -> bool {
        self.kind == TrackKind::Audio && self.vocal.unwrap_or_else(|| crate::looks_vocal(&self.name))
    }

    pub fn channels(&self) -> usize {
        self.format.channels()
    }

    pub fn playlist(&self) -> Option<&Playlist> {
        self.playlists.get(self.active_playlist)
    }

    pub fn playlist_mut(&mut self) -> Option<&mut Playlist> {
        self.playlists.get_mut(self.active_playlist)
    }

    /// The take layers (comping): every playlist but the active one (the comp), with its index.
    pub fn layers(&self) -> impl Iterator<Item = (usize, &Playlist)> {
        let active = self.active_playlist;
        self.playlists.iter().enumerate().filter(move |(i, _)| *i != active)
    }

    /// Whether `layer` names a take layer (a playlist that isn't the comp).
    pub fn is_layer(&self, layer: usize) -> bool {
        layer != self.active_playlist && layer < self.playlists.len()
    }

    /// The take colour (palette index, 0-7) of layer `layer`: its own, else its position among
    /// the layers.
    pub fn take_color(&self, layer: usize) -> u8 {
        if let Some(c) = self.playlists.get(layer).and_then(|p| p.take_color) {
            return c % 8;
        }
        let pos = self.layers().position(|(i, _)| i == layer).unwrap_or(0);
        (pos % 8) as u8
    }

    pub fn clips(&self) -> &[Clip] {
        self.playlist().map_or(&[], |p| p.clips.as_slice())
    }

    /// The clips playback hears: an auditioned take layer's ([`Track::audition_layer`]) in
    /// place of the comp, else the active playlist's ([`Track::clips`]). Editing always works
    /// on [`Track::clips`].
    pub fn heard_clips(&self) -> &[Clip] {
        match self.audition_layer.filter(|l| self.is_layer(*l)).and_then(|l| self.playlists.get(l)) {
            Some(p) => p.clips.as_slice(),
            None => self.clips(),
        }
    }

    /// The module automation lanes and mappings name by `id`: a chain module, or the
    /// instrument for [`crate::INSTRUMENT_MODULE`].
    pub fn module_or_instrument(&self, id: crate::ModuleId) -> Option<&crate::Insert> {
        if id == crate::INSTRUMENT_MODULE { self.instrument.as_ref() } else { self.mixer.module(id) }
    }

    pub fn lane(&self, p: &AutoParam) -> Option<&AutomationLane> {
        self.automation.iter().find(|l| &l.param == p)
    }

    /// The lane of `p` as playback reads it: `None` while it is suspended (automation Preview).
    pub fn played_lane(&self, p: &AutoParam) -> Option<&AutomationLane> {
        self.lane(p).filter(|l| !l.suspended)
    }

    pub fn lane_mut(&mut self, p: &AutoParam) -> &mut AutomationLane {
        let idx = match self.automation.iter().position(|l| &l.param == p) {
            Some(i) => i,
            None => {
                self.automation.push(AutomationLane::new(p.clone()));
                self.automation.len() - 1
            }
        };
        let len = self.automation.len();
        // `idx` is in range: it came from `position` or is the index of the element just pushed.
        &mut self.automation[idx.min(len - 1)]
    }

    pub fn is_folder(&self) -> bool {
        self.kind == TrackKind::Folder
    }

    /// The frozen render the track plays, while frozen.
    pub fn frozen_audio(&self) -> Option<&FrozenAudio> {
        self.freeze.as_ref().filter(|_| self.frozen)
    }

    /// How many chain modules, from the top, the frozen render holds: the whole chain, or down to
    /// and including the freeze point; 0 while not frozen. A freeze point that names no module
    /// of the chain counts as the whole chain (nothing after it is live).
    pub fn frozen_modules(&self) -> usize {
        let n = self.mixer.inserts.len();
        match self.frozen_audio() {
            None => 0,
            Some(f) => f.upto.and_then(|m| self.mixer.inserts.iter().position(|i| i.id == m)).map_or(n, |p| p.saturating_add(1)),
        }
    }
}

/// One of an instrument's extra outputs ([`Track::instrument_outs`]) and the bus it plays into.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct InstrumentOut {
    /// Which extra output (0: the instrument's first after its main one).
    pub output: u8,
    pub bus: crate::BusId,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_track_defaults() {
        let t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Stereo, TRACK_COLORS[0]);
        assert_eq!(t.channels(), 2);
        assert_eq!(t.view, "waveform");
        assert_eq!(t.playlists.len(), 1);
        let m = Track::new(TrackId(2), "Keys", TrackKind::Midi, ChannelFormat::Stereo, TRACK_COLORS[0]);
        assert_eq!(m.format, ChannelFormat::Mono);
        assert_eq!(m.view, "notes");
    }

    #[test]
    fn lane_mut_creates_once() {
        let mut t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        t.lane_mut(&AutoParam::Volume).set_point(0, -3.0);
        t.lane_mut(&AutoParam::Volume).set_point(10, -6.0);
        assert_eq!(t.automation.len(), 1);
        assert_eq!(t.lane(&AutoParam::Volume).map(|l| l.points.len()), Some(2));
    }

    #[test]
    fn frozen_audio_round_trips_and_old_tracks_load_unfrozen() {
        let mut t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        let old = serde_json::to_string(&t).unwrap();
        assert!(!old.contains("freeze"), "an unfrozen track writes no freeze field");
        let back: Track = serde_json::from_str(&old).unwrap();
        assert_eq!(back.frozen_audio(), None);
        t.frozen = true;
        t.freeze = Some(FrozenAudio { source: SourceId(7), start: 0, upto: None });
        let json = serde_json::to_string(&t).unwrap();
        assert!(!json.contains("upto"), "a whole-chain freeze writes no freeze point");
        let back: Track = serde_json::from_str(&json).unwrap();
        assert_eq!(back.frozen_audio(), Some(&FrozenAudio { source: SourceId(7), start: 0, upto: None }));
        // A legacy `frozen: true` with no audio plays live.
        t.freeze = None;
        assert_eq!(t.frozen_audio(), None);
    }

    #[test]
    fn a_freeze_point_round_trips_and_counts_the_frozen_modules() {
        let mut t = Track::new(TrackId(1), "Gtr", TrackKind::Audio, ChannelFormat::Mono, TRACK_COLORS[0]);
        let a = t.mixer.add_module(crate::Insert::new("gain"), None).unwrap();
        let b = t.mixer.add_module(crate::Insert::new("compressor"), None).unwrap();
        let _c = t.mixer.add_module(crate::Insert::new("eq_7band"), None).unwrap();
        assert_eq!(t.frozen_modules(), 0, "not frozen");
        t.frozen = true;
        t.freeze = Some(FrozenAudio { source: SourceId(7), start: 0, upto: Some(b) });
        assert_eq!(t.frozen_modules(), 2, "down to and including the point");
        let back: Track = serde_json::from_str(&serde_json::to_string(&t).unwrap()).unwrap();
        assert_eq!(back.freeze.unwrap().upto, Some(b));
        t.freeze = Some(FrozenAudio { source: SourceId(7), start: 0, upto: Some(a) });
        assert_eq!(t.frozen_modules(), 1);
        t.freeze = Some(FrozenAudio { source: SourceId(7), start: 0, upto: None });
        assert_eq!(t.frozen_modules(), 3, "whole chain");
        // An old frozen session (no point) is the whole chain.
        let old = r#"{"source":7,"start":0}"#;
        let fz: FrozenAudio = serde_json::from_str(old).unwrap();
        assert_eq!(fz.upto, None);
    }

    #[test]
    fn kind_ids() {
        for k in TrackKind::ALL {
            assert_eq!(TrackKind::from_id(k.id()), Some(k));
        }
    }

    #[test]
    fn custom_heights_round_to_presets_and_step_through_them() {
        assert_eq!(TrackHeight::from_points(88.0), TrackHeight::Medium);
        assert_eq!(TrackHeight::from_points(57.4), TrackHeight::Custom(57));
        assert_eq!(TrackHeight::from_points(9999.0), TrackHeight::Extreme);
        assert_eq!(TrackHeight::from_points(f32::NAN), TrackHeight::Micro);
        assert_eq!(TrackHeight::Custom(u16::MAX).points(), 440.0);
        assert_eq!(TrackHeight::Custom(57).step(true), TrackHeight::Medium);
        assert_eq!(TrackHeight::Custom(57).step(false), TrackHeight::Small);
        assert_eq!(TrackHeight::Extreme.step(true), TrackHeight::Extreme);
        assert_eq!(TrackHeight::Micro.step(false), TrackHeight::Micro);
        let json = serde_json::to_string(&TrackHeight::Custom(57)).unwrap();
        assert_eq!(serde_json::from_str::<TrackHeight>(&json).unwrap(), TrackHeight::Custom(57));
        assert_eq!(serde_json::from_str::<TrackHeight>("\"Large\"").unwrap(), TrackHeight::Large);
    }
}

/// An MPE zone (MIDI Polyphonic Expression): a master channel (1 for the lower zone, 16 for the
/// upper) and `members` channels next to it, one per sounding note, each with its own pitch
/// bend over `bend` semitones (48 by MPE's default) and the master's over `master_bend` (2).
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MpeZone {
    /// The upper zone (master channel 16, members counted down from 15); else the lower.
    #[serde(default)]
    pub upper: bool,
    /// Member channels, 1 to 15.
    pub members: u8,
    /// Member channels' pitch-bend range, semitones (0 to 96).
    pub bend: u8,
    /// The master channel's pitch-bend range, semitones (0 to 96).
    #[serde(default = "MpeZone::default_master_bend")]
    pub master_bend: u8,
}

impl Default for MpeZone {
    fn default() -> Self {
        MpeZone { upper: false, members: 15, bend: 48, master_bend: 2 }
    }
}

impl MpeZone {
    fn default_master_bend() -> u8 {
        2
    }

    /// The master channel (0-based).
    pub fn master(&self) -> u8 {
        if self.upper { 15 } else { 0 }
    }

    /// The member channels (0-based), nearest the master first.
    pub fn member_channels(&self) -> impl Iterator<Item = u8> + '_ {
        let n = self.members.clamp(1, 15);
        (1..=n).map(move |k| if self.upper { 15 - k } else { k })
    }
}

/// Sound variations, or an expression map: a track's articulations (legato, staccato, pizzicato
/// of an orchestral library), each switched to by its keyswitch, a key the instrument takes as
/// "play the following notes this way". A clip chooses one with a controller of the lane `cc`
/// whose value is the articulation's index: playback and chase send that articulation's
/// keyswitch (a short note on the event's channel) in place of the controller, so the change is
/// drawn, moved, cut and copied as any controller point is.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Articulations {
    /// The controller number that chooses (0 to 119); 3 by default (undefined in MIDI).
    pub cc: u8,
    pub items: Vec<Articulation>,
}

impl Default for Articulations {
    fn default() -> Self {
        Articulations { cc: 3, items: Vec::new() }
    }
}

/// One articulation of [`Articulations`].
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Articulation {
    pub name: String,
    /// The key that switches to it (MIDI note number).
    pub keyswitch: u8,
}

/// Most articulations a track holds (a controller value picks one).
pub const MAX_ARTICULATIONS: usize = 128;

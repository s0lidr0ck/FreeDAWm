//! Clips: windows onto audio sources or MIDI sequences, placed on a playlist.

use crate::{ClipId, MAX_NAME_CHARS, SourceId, truncate_chars};
use soundcraft_midi::Sequence;
use soundcraft_midi::cleanup::{CleanupOutput, CleanupSettings};
use soundcraft_time::{MAX_POSITION, Range, Samples};

/// Fade curve shapes (Pro Tools offers standard, equal-power, S-curve and custom presets).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default, serde::Serialize, serde::Deserialize)]
pub enum FadeShape {
    Linear,
    #[default]
    EqualPower,
    SCurve,
    Exponential,
    Logarithmic,
}

impl FadeShape {
    pub const ALL: [FadeShape; 5] = [FadeShape::Linear, FadeShape::EqualPower, FadeShape::SCurve, FadeShape::Exponential, FadeShape::Logarithmic];

    /// Fade-in gain at `t` ∈ [0,1] (fade-outs use `gain(1 - t)`).
    pub fn gain(self, t: f32) -> f32 {
        let t = if t.is_nan() { 0.0 } else { t.clamp(0.0, 1.0) };
        match self {
            FadeShape::Linear => t,
            FadeShape::EqualPower => (t * std::f32::consts::FRAC_PI_2).sin(),
            FadeShape::SCurve => 0.5 - 0.5 * (t * std::f32::consts::PI).cos(),
            FadeShape::Exponential => t * t * t,
            FadeShape::Logarithmic => 1.0 - (1.0 - t) * (1.0 - t) * (1.0 - t),
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            FadeShape::Linear => "Linear",
            FadeShape::EqualPower => "Equal Power",
            FadeShape::SCurve => "S-Curve",
            FadeShape::Exponential => "Exponential",
            FadeShape::Logarithmic => "Logarithmic",
        }
    }

    pub fn from_id(s: &str) -> Option<FadeShape> {
        let s = s.to_ascii_lowercase().replace(['-', '_', ' '], "");
        FadeShape::ALL.into_iter().find(|f| f.label().to_ascii_lowercase().replace(['-', ' '], "") == s)
    }
}

/// A fade at a clip boundary, `len` samples long.
#[derive(Debug, Clone, Copy, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Fade {
    pub len: Samples,
    pub shape: FadeShape,
    /// Continuous bend (-1..1); zero preserves the original shape exactly.
    #[serde(default)]
    pub curvature: f32,
}

impl Fade {
    /// The envelope used by both rendering and audio, with fixed zero/unity endpoints.
    pub fn gain(self, t: f32) -> f32 {
        let base = self.shape.gain(t);
        let curvature = if self.curvature.is_finite() { self.curvature.clamp(-1.0, 1.0) } else { 0.0 };
        if curvature == 0.0 || base <= 0.0 || base >= 1.0 {
            return base;
        }
        let weighted = base * (curvature * 4.0).exp2();
        weighted / (weighted + 1.0 - base)
    }

    /// Invert the bend at the midpoint so a vertical drag follows the pointer.
    pub fn curvature_for_midpoint_gain(self, gain: f32) -> f32 {
        let gain = if gain.is_finite() { gain.clamp(0.001, 0.999) } else { self.gain(0.5) };
        let base = self.shape.gain(0.5);
        (((gain / (1.0 - gain)).log2() - (base / (1.0 - base)).log2()) / 4.0).clamp(-1.0, 1.0)
    }

    pub fn sanitize(&mut self) {
        self.curvature = if self.curvature.is_finite() { self.curvature.clamp(-1.0, 1.0) } else { 0.0 };
    }
}

/// What a clip plays.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub enum ClipContent {
    /// Audio from `source`, starting `offset` samples into the file.
    Audio { source: SourceId, offset: Samples },
    /// MIDI notes, with ticks relative to the clip start.
    Midi { sequence: Sequence },
    /// Picture from the movie `source` ([`crate::VideoSource`], `Session::videos`), starting
    /// `offset` samples into the movie. Video clips carry no audio.
    Video { source: SourceId, offset: Samples },
}

/// Clean Vocal's cleaned audio, kept beside the clip's original source (non-destructive): the clip
/// plays `mix · cleaned + (1 − mix) · original`, sample for sample, before its gain, fades and clip
/// effects. The cleaned source is in the original's time: its sample `k` is the original's sample
/// `origin + k`, so moves, trims, splits and copies keep both aligned (the clip's `offset` and
/// lyrics stay in the original's time). Where the cleaned audio doesn't reach (a trim opened past
/// the cleaned window), the original plays. It only plays while the clip still plays `original`:
/// anything that gives the clip new audio (Process, Clip Gain Render…) leaves it inert.
#[derive(Debug, Clone, Copy, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct CleanAlt {
    /// The cleaned audio (at the session rate, the track's width).
    pub source: SourceId,
    /// The source the cleaned audio was made from.
    pub original: SourceId,
    /// The position in `original` of the cleaned audio's first sample.
    pub origin: Samples,
    /// The cleaned share, 0–1 (1 = all cleaned).
    #[serde(default = "one_f32")]
    pub mix: f32,
    /// Original/Cleaned toggle: `true` plays the original (as mix 0) and keeps `mix`.
    #[serde(default)]
    pub bypass: bool,
}

fn one_f32() -> f32 {
    1.0
}

impl CleanAlt {
    /// `mix`, sanitised to 0–1.
    pub fn amount(&self) -> f32 {
        if self.mix.is_finite() { self.mix.clamp(0.0, 1.0) } else { 1.0 }
    }

    /// The cleaned share that plays: 0 when bypassed.
    pub fn wet(&self) -> f32 {
        if self.bypass { 0.0 } else { self.amount() }
    }
}

/// MIDI Cleanup on a MIDI clip: the settings (saved) and the cleaned notes derived from them
/// (not saved; the engine recomputes the preview whenever its inputs change). The clip's own
/// sequence stays the untouched take until Commit.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct MidiCleanup {
    #[serde(default)]
    pub settings: CleanupSettings,
    /// The A/B switch: true plays (and shows) the untouched take.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub original: bool,
    #[serde(skip)]
    pub preview: Option<std::sync::Arc<CleanupPreview>>,
}

impl PartialEq for MidiCleanup {
    /// The preview is derived, so two cleanups are equal when their settings and switch are.
    fn eq(&self, other: &Self) -> bool {
        self.settings == other.settings && self.original == other.original
    }
}

/// The cleaned result for one set of inputs: `inputs` is the fingerprint of the notes, chords,
/// key, tempo and settings it was made from.
#[derive(Debug, Clone, PartialEq)]
pub struct CleanupPreview {
    pub inputs: u64,
    pub out: CleanupOutput,
}

/// A clip on a playlist.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Clip {
    pub id: ClipId,
    pub name: String,
    /// Timeline start (samples).
    pub start: Samples,
    /// Length (samples, > 0).
    pub length: Samples,
    pub content: ClipContent,
    /// Static clip gain in dB.
    #[serde(default)]
    pub gain_db: f32,
    /// Clip gain envelope breakpoints: (offset from clip start, dB). Empty = flat `gain_db`.
    #[serde(default)]
    pub gain_env: Vec<(Samples, f32)>,
    #[serde(default)]
    pub fade_in: Fade,
    #[serde(default)]
    pub fade_out: Fade,
    #[serde(default)]
    pub muted: bool,
    #[serde(default)]
    pub edit_locked: bool,
    #[serde(default)]
    pub time_locked: bool,
    #[serde(default)]
    pub rating: u8,
    /// Sync point offset from clip start.
    #[serde(default)]
    pub sync_point: Samples,
    /// Optional colour override (RGB).
    #[serde(default)]
    pub color: Option<[u8; 3]>,
    /// Elastic / Vari-speed ratio (1.0 = none) — applied by rendered processing.
    #[serde(default = "one")]
    pub stretch: f64,
    /// Reversed playback (Process › Reverse renders into a new source; this is for previews).
    #[serde(default)]
    pub group: Option<u64>,
    /// Marks the clips made by `clip.loop` (the origin and its copies share one id), so
    /// `clip.unloop` still recognises the loop after the source's content is edited.
    #[serde(default)]
    pub loop_id: Option<u64>,
    /// MIDI clips printed from the band: each note and controller plays on its own channel (the
    /// organ's lower manual on its own), controllers before notes on the same tick. Off for every
    /// other clip, which plays on channel 1.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub midi_channels: bool,
    /// Timed lyrics of this clip's material (Transcribe Lyrics, MIDI import): times are positions
    /// in the clip's source (audio: samples into the file, the clip's `offset` space; MIDI: samples
    /// from the clip start), so they move, trim, split and delete with the clip. Only the words
    /// inside the clip's window show and export.
    #[serde(default, skip_serializing_if = "Vec::is_empty", deserialize_with = "crate::lyrics::de_lyrics")]
    pub lyrics: Vec<crate::LyricLine>,
    /// Clean Vocal's cleaned audio and its mix ([`CleanAlt`]); `None` for every other clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub clean: Option<CleanAlt>,
    /// MIDI Cleanup's settings and derived preview ([`MidiCleanup`]); `None` for every other clip.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub midi_cleanup: Option<MidiCleanup>,
    /// Vocal tuning ([`crate::VocalTune`]); `None` for every untuned clip. Only audio clips use it
    /// ([`Clip::tune_live`]).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tune: Option<crate::VocalTune>,
    /// Comping: on the comp (the active playlist), the take layer (playlist index) this segment
    /// came from. `None`: not from a layer (drawn in the track colour).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub from_layer: Option<usize>,
    /// Warp markers of a straightened clip (Detect Tempo › Straighten): `(position in the original
    /// material, position in the clip)` pairs, both from the clip start, that the audio was
    /// rendered to; empty for every other clip. `clip.remove_warp` clears them with the render.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub warp: Vec<(Samples, Samples)>,
}

fn one() -> f64 {
    1.0
}

impl Clip {
    pub fn audio(id: ClipId, name: impl Into<String>, source: SourceId, offset: Samples, start: Samples, length: Samples) -> Clip {
        Clip {
            id,
            name: name.into(),
            start,
            length: length.max(1),
            content: ClipContent::Audio { source, offset },
            gain_db: 0.0,
            gain_env: Vec::new(),
            fade_in: Fade::default(),
            fade_out: Fade::default(),
            muted: false,
            edit_locked: false,
            time_locked: false,
            rating: 0,
            sync_point: 0,
            color: None,
            stretch: 1.0,
            group: None,
            loop_id: None,
            midi_channels: false,
            lyrics: Vec::new(),
            clean: None,
            midi_cleanup: None,
            tune: None,
            from_layer: None,
            warp: Vec::new(),
        }
    }

    pub fn midi(id: ClipId, name: impl Into<String>, start: Samples, length: Samples, sequence: Sequence) -> Clip {
        let mut c = Clip::audio(id, name, SourceId(0), 0, start, length);
        c.content = ClipContent::Midi { sequence };
        c
    }

    /// A clip showing the movie `source` from `offset` samples into it.
    pub fn video(id: ClipId, name: impl Into<String>, source: SourceId, offset: Samples, start: Samples, length: Samples) -> Clip {
        let mut c = Clip::audio(id, name, SourceId(0), 0, start, length);
        c.content = ClipContent::Video { source, offset };
        c
    }

    /// The notes that play: the cleaned preview when the clip has a cleanup on Cleaned with a
    /// preview, else its own sequence; `None` for audio and video clips.
    pub fn midi_playing(&self) -> Option<&Sequence> {
        let ClipContent::Midi { sequence } = &self.content else { return None };
        match &self.midi_cleanup {
            Some(m) if !m.original => Some(m.preview.as_ref().map_or(sequence, |p| &p.out.sequence)),
            _ => Some(sequence),
        }
    }

    pub fn end(&self) -> Samples {
        self.start.saturating_add(self.length)
    }

    pub fn range(&self) -> Range {
        Range { start: self.start, end: self.end() }
    }

    pub fn is_audio(&self) -> bool {
        matches!(self.content, ClipContent::Audio { .. })
    }

    pub fn is_video(&self) -> bool {
        matches!(self.content, ClipContent::Video { .. })
    }

    /// The audio source this clip plays (None for MIDI and video clips).
    pub fn source(&self) -> Option<SourceId> {
        match self.content {
            ClipContent::Audio { source, .. } => Some(source),
            ClipContent::Midi { .. } | ClipContent::Video { .. } => None,
        }
    }

    /// The cleaned alternate while it applies: the clip still plays the source it was made from.
    pub fn clean_alt(&self) -> Option<&CleanAlt> {
        self.clean.as_ref().filter(|k| self.source() == Some(k.original))
    }

    /// The vocal tuning while it applies: audio clips still playing the source it was made against
    /// ([`crate::VocalTune::source`]; Consolidate and Process give the clip a new one).
    pub fn tune_live(&self) -> Option<&crate::VocalTune> {
        self.tune.as_ref().filter(|t| self.is_audio() && t.source.is_none_or(|s| self.source() == Some(s)))
    }

    /// Whether the clip plays `src` (its source, or its cleaned alternate).
    pub fn uses_source(&self, src: SourceId) -> bool {
        self.source() == Some(src) || self.clean_alt().is_some_and(|k| k.source == src)
    }

    /// The movie this clip shows (video clips only).
    pub fn video_source(&self) -> Option<SourceId> {
        match self.content {
            ClipContent::Video { source, .. } => Some(source),
            ClipContent::Audio { .. } | ClipContent::Midi { .. } => None,
        }
    }

    /// Offset into the audio file or movie (0 for MIDI).
    pub fn source_offset(&self) -> Samples {
        match self.content {
            ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => offset,
            ClipContent::Midi { .. } => 0,
        }
    }

    /// Gain (linear) at `rel` samples from the clip start: clip gain × envelope × fades.
    pub fn gain_at(&self, rel: Samples) -> f32 {
        self.gain_at_with_fades(rel, self.fade_in, self.fade_out)
    }

    /// Evaluate temporary fades while retaining clip gain and its envelope.
    /// UI previews can use this without modifying the clip or rebuilding media.
    pub fn gain_at_with_fades(&self, rel: Samples, fade_in: Fade, fade_out: Fade) -> f32 {
        let mut db = self.gain_db;
        if !self.gain_env.is_empty() {
            db += env_value(&self.gain_env, rel);
        }
        let mut g = db_to_gain(db);
        if fade_in.len > 0 && rel < fade_in.len {
            g *= fade_in.gain(rel as f32 / fade_in.len as f32);
        }
        let from_end = self.length.saturating_sub(rel);
        if fade_out.len > 0 && from_end <= fade_out.len {
            g *= fade_out.gain(from_end as f32 / fade_out.len as f32);
        }
        g
    }

    /// Move the start edge to `new_start` keeping material anchored (trim start).
    pub fn trim_start_to(&mut self, new_start: Samples) {
        let new_start = new_start.min(self.end() - 1);
        let delta = new_start - self.start;
        if let ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } = &mut self.content {
            *offset = offset.saturating_add(delta);
        }
        // MIDI note ticks are re-anchored by the engine, which knows the tempo map; a MIDI clip's
        // lyrics (time from its start) follow here, the ones before the new start dropped as its
        // notes are.
        if matches!(self.content, ClipContent::Midi { .. }) && delta != 0 && !self.lyrics.is_empty() {
            for l in &mut self.lyrics {
                l.words.retain(|w| w.start >= delta);
                for w in &mut l.words {
                    (w.start, w.end) = (w.start - delta, w.end.saturating_sub(delta).max(w.start - delta));
                }
            }
            crate::lyrics::sanitize_lyrics(&mut self.lyrics);
        }
        self.length -= delta;
        self.start = new_start;
        if self.sync_point != 0 {
            self.sync_point = self.sync_point.saturating_sub(delta);
        }
        self.drop_stray_sync();
        self.clamp_fades();
        // Keep the gain at the cut when earlier breakpoints are dropped.
        if self.gain_env.iter().any(|&(t, _)| t < delta) && self.gain_env.iter().all(|&(t, _)| t != delta) {
            self.gain_env.insert(0, (delta, env_value(&self.gain_env, delta)));
        }
        self.gain_env.retain(|&(t, _)| t >= delta);
        self.gain_env.iter_mut().for_each(|(t, _)| *t = t.saturating_sub(delta));
    }

    /// Move the end edge to `new_end`.
    pub fn trim_end_to(&mut self, new_end: Samples) {
        let new_end = new_end.max(self.start + 1);
        self.length = new_end - self.start;
        self.drop_stray_sync();
        self.clamp_fades();
    }

    /// Keep both fades within the clip.
    pub fn clamp_fades(&mut self) {
        self.fade_in.len = self.fade_in.len.clamp(0, self.length);
        self.fade_out.len = self.fade_out.len.clamp(0, self.length);
    }

    /// A sync point (0 means none) that a trim left outside the clip no longer marks anything.
    fn drop_stray_sync(&mut self) {
        if !(0..self.length).contains(&self.sync_point) {
            self.sync_point = 0;
        }
    }

    /// Repair a clip read from a file: positions and lengths within ±[`MAX_POSITION`], as commands
    /// keep theirs, fades within the clip, a name that can be drawn, and a usable gain and stretch.
    pub fn sanitize(&mut self) {
        bound(&mut self.start);
        self.length = self.length.clamp(1, MAX_POSITION);
        bound(&mut self.sync_point);
        self.gain_env.iter_mut().for_each(|(at, _)| bound(at));
        match &mut self.content {
            ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => bound(offset),
            ClipContent::Midi { sequence } => bound_sequence(sequence),
        }
        if let Some(m) = &mut self.midi_cleanup {
            m.settings = m.settings.clamped();
        }
        if !matches!(self.content, ClipContent::Midi { .. }) {
            self.midi_cleanup = None;
        }
        if let Some(t) = &mut self.tune {
            t.sanitize();
        }
        if !self.is_audio() {
            self.tune = None;
        }
        self.clamp_fades();
        self.fade_in.sanitize();
        self.fade_out.sanitize();
        crate::lyrics::sanitize_lyrics(&mut self.lyrics);
        truncate_chars(&mut self.name, MAX_NAME_CHARS);
        if !self.gain_db.is_finite() {
            self.gain_db = 0.0;
        }
        if !self.stretch.is_finite() || self.stretch <= 0.0 {
            self.stretch = 1.0;
        }
    }
}

pub(crate) fn bound(at: &mut Samples) {
    *at = (*at).clamp(-MAX_POSITION, MAX_POSITION);
}

pub(crate) fn bound_sequence(s: &mut Sequence) {
    for n in &mut s.notes {
        bound(&mut n.start);
        n.length = n.length.clamp(1, MAX_POSITION);
    }
    s.ctrls.iter_mut().for_each(|c| bound(&mut c.tick));
}

fn env_value(env: &[(Samples, f32)], at: Samples) -> f32 {
    let mut prev: Option<(Samples, f32)> = None;
    for &(t, v) in env {
        if t >= at {
            return match prev {
                Some((pt, pv)) if t > pt => {
                    let elapsed = i128::from(at).saturating_sub(i128::from(pt));
                    let span = i128::from(t).saturating_sub(i128::from(pt));
                    pv + (v - pv) * (elapsed as f32 / span as f32)
                }
                _ => v,
            };
        }
        prev = Some((t, v));
    }
    prev.map_or(0.0, |(_, v)| v)
}

pub(crate) fn db_to_gain(db: f32) -> f32 {
    if db.is_nan() || db <= -144.0 { 0.0 } else { 10f32.powf(db.min(48.0) / 20.0) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cleanup_clip() -> Clip {
        let mut seq = Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 90, release_velocity: 64, channel: 0, start: 0, length: 100 });
        Clip::midi(ClipId(1), "m", 0, 1000, seq)
    }

    #[test]
    fn old_session_loads_without_cleanup() {
        let mut c = cleanup_clip();
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("midi_cleanup").is_none(), "None omits the key");
        let back: Clip = serde_json::from_value(v).unwrap();
        assert!(back.midi_cleanup.is_none());
        c.midi_cleanup = Some(MidiCleanup::default());
        let v = serde_json::to_value(&c).unwrap();
        assert!(v["midi_cleanup"].get("original").is_none() && v["midi_cleanup"].get("preview").is_none());
        let back: Clip = serde_json::from_value(v).unwrap();
        assert_eq!(back.midi_cleanup, Some(MidiCleanup::default()));
    }

    #[test]
    fn midi_playing_follows_switch() {
        let mut c = cleanup_clip();
        let mut cleaned = Sequence::default();
        cleaned.notes.push(soundcraft_midi::Note { pitch: 72, velocity: 90, release_velocity: 64, channel: 0, start: 0, length: 50 });
        let preview = std::sync::Arc::new(CleanupPreview {
            inputs: 7,
            out: CleanupOutput { sequence: cleaned.clone(), edits: Vec::new(), report: Default::default() },
        });
        assert_eq!(c.midi_playing().map(|s| s.notes[0].pitch), Some(60), "no cleanup");
        c.midi_cleanup = Some(MidiCleanup { preview: Some(preview), ..MidiCleanup::default() });
        assert_eq!(c.midi_playing(), Some(&cleaned));
        c.midi_cleanup.as_mut().unwrap().original = true;
        assert_eq!(c.midi_playing().map(|s| s.notes[0].pitch), Some(60));
        c.midi_cleanup.as_mut().unwrap().original = false;
        c.midi_cleanup.as_mut().unwrap().preview = None;
        assert_eq!(c.midi_playing().map(|s| s.notes[0].pitch), Some(60), "no preview");
        assert!(Clip::audio(ClipId(2), "a", SourceId(0), 0, 0, 10).midi_playing().is_none());
    }

    #[test]
    fn sanitize_clamps_cleanup_and_drops_it_off_midi() {
        let mut c = cleanup_clip();
        let mut m = MidiCleanup::default();
        m.settings.wrong = 255;
        c.midi_cleanup = Some(m.clone());
        c.sanitize();
        assert!(c.midi_cleanup.as_ref().unwrap().settings.wrong <= 100);
        let mut a = Clip::audio(ClipId(2), "a", SourceId(0), 0, 0, 10);
        a.midi_cleanup = Some(m);
        a.sanitize();
        assert!(a.midi_cleanup.is_none());
    }

    #[test]
    fn fade_shapes_hit_endpoints() {
        for s in FadeShape::ALL {
            assert!(s.gain(0.0).abs() < 1e-6, "{s:?}");
            assert!((s.gain(1.0) - 1.0).abs() < 1e-6, "{s:?}");
            assert_eq!(s.gain(f32::NAN), 0.0);
            assert_eq!(FadeShape::from_id(s.label()), Some(s));
        }
    }

    #[test]
    fn fade_curvature_is_bounded_monotonic_and_inverts_at_the_midpoint() {
        for shape in FadeShape::ALL {
            let original = Fade { len: 1000, shape, curvature: 0.0 };
            for i in 0..=128 {
                let t = i as f32 / 128.0;
                assert_eq!(original.gain(t), shape.gain(t), "legacy curves must be unchanged");
            }
            for curvature in [-1.0, -0.5, 0.0, 0.5, 1.0] {
                let fade = Fade { curvature, ..original };
                assert_eq!(fade.gain(0.0), 0.0);
                assert_eq!(fade.gain(1.0), 1.0);
                let mut previous = 0.0;
                for i in 0..=128 {
                    let gain = fade.gain(i as f32 / 128.0);
                    assert!(gain.is_finite() && (0.0..=1.0).contains(&gain));
                    assert!(gain >= previous, "{shape:?}, {curvature}");
                    previous = gain;
                }
                assert!((fade.curvature_for_midpoint_gain(fade.gain(0.5)) - curvature).abs() < 1e-5);
            }
            assert!((Fade { curvature: 0.5, ..original }).gain(0.5) > original.gain(0.5));
            assert!((Fade { curvature: -0.5, ..original }).gain(0.5) < original.gain(0.5));
        }
    }

    #[test]
    fn fade_curvature_loads_legacy_data_and_sanitizes_hostile_values() {
        let fade: Fade = serde_json::from_str(r#"{"len":100,"shape":"SCurve"}"#).unwrap();
        assert_eq!(fade.curvature, 0.0);
        for curvature in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY, -10.0, 10.0] {
            let mut fade = Fade { curvature, ..fade };
            assert!(fade.gain(0.5).is_finite());
            fade.sanitize();
            assert!(fade.curvature.is_finite() && (-1.0..=1.0).contains(&fade.curvature));
            let roundtrip: Fade = serde_json::from_str(&serde_json::to_string(&fade).unwrap()).unwrap();
            assert_eq!(roundtrip, fade);
        }
    }

    #[test]
    fn temporary_fades_keep_clip_gain_and_envelope_without_mutating_the_clip() {
        let mut clip = Clip::audio(ClipId(1), "preview", SourceId(1), 0, 0, 1000);
        clip.gain_db = -6.0;
        clip.gain_env = vec![(0, 0.0), (1000, 6.0)];
        clip.fade_in = Fade { len: 100, shape: FadeShape::SCurve, curvature: 0.25 };
        clip.fade_out = Fade { len: 100, shape: FadeShape::Logarithmic, curvature: -0.1 };
        let before = clip.clone();
        let fade_in = Fade { len: 1000, shape: FadeShape::Linear, curvature: 0.5 };
        let fade_out = Fade { len: 1000, shape: FadeShape::Linear, curvature: -0.5 };
        assert!((clip.gain_at_with_fades(500, fade_in, fade_out) - db_to_gain(-3.0) * 0.16).abs() < 1e-6);
        let mut committed = clip.clone();
        committed.fade_in = fade_in;
        committed.fade_out = fade_out;
        for rel in [0, 50, 500, 999, 1000] {
            assert_eq!(clip.gain_at_with_fades(rel, fade_in, fade_out), committed.gain_at(rel));
        }
        for rel in [i64::MIN, i64::MAX] {
            assert!(clip.gain_at(rel).is_finite());
            assert!(clip.gain_at_with_fades(rel, fade_in, fade_out).is_finite());
        }
        assert_eq!(clip, before);
        clip.gain_env = vec![(i64::MIN, 0.0), (i64::MAX, 6.0)];
        assert!((clip.gain_at(500) - db_to_gain(-3.0)).abs() < 1e-6);
        assert!((clip.gain_at_with_fades(500, fade_in, fade_out) - db_to_gain(-3.0) * 0.16).abs() < 1e-6);
    }

    #[test]
    fn gain_with_fades() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 0, 100, 1000);
        c.fade_in = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        c.fade_out = Fade { len: 100, shape: FadeShape::Linear, curvature: 0.0 };
        assert!(c.gain_at(0).abs() < 1e-6);
        assert!((c.gain_at(50) - 0.5).abs() < 1e-6);
        assert!((c.gain_at(500) - 1.0).abs() < 1e-6);
        assert!((c.gain_at(950) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn gain_envelope_interpolates() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 0, 0, 1000);
        c.gain_env = vec![(0, 0.0), (1000, -20.0)];
        assert!((c.gain_at(500) - db_to_gain(-10.0)).abs() < 1e-4);
    }

    #[test]
    fn trim_start_keeps_gain_envelope_at_cut() {
        for (env, cut, expected) in [
            (vec![(0, 0.0), (1000, -20.0)], 500, vec![(0, -10.0), (500, -20.0)]),
            (vec![(0, 0.0), (100, -20.0)], 500, vec![(0, -20.0)]),
            (vec![(0, 0.0), (500, -10.0), (1000, -20.0)], 500, vec![(0, -10.0), (500, -20.0)]),
            (vec![], 500, vec![]),
            (vec![(100, -3.0)], 0, vec![(100, -3.0)]),
            (vec![(100, -3.0)], -100, vec![(200, -3.0)]),
        ] {
            let mut clip = Clip::audio(ClipId(1), "a", SourceId(1), 100, 0, 1000);
            clip.gain_env = env;
            let gain_at_cut = clip.gain_at(cut);
            clip.trim_start_to(cut);
            assert_eq!(clip.gain_env, expected);
            if cut > 0 {
                assert!((clip.gain_at(0) - gain_at_cut).abs() < 1e-6);
            }
        }
    }

    #[test]
    fn trims_keep_material_anchored() {
        let mut c = Clip::audio(ClipId(1), "a", SourceId(1), 1000, 100, 1000);
        c.trim_start_to(300);
        assert_eq!((c.start, c.length, c.source_offset()), (300, 800, 1200));
        c.trim_end_to(500);
        assert_eq!(c.length, 200);
        c.trim_end_to(-50);
        assert_eq!(c.length, 1);
        // Regression: trimming the start must not leave a fade-out longer than the clip.
        let mut c = Clip::audio(ClipId(2), "b", SourceId(1), 0, 0, 1000);
        c.fade_out = Fade { len: 800, shape: FadeShape::Linear, curvature: 0.0 };
        c.trim_start_to(900);
        assert!(c.fade_out.len <= c.length);
    }
}

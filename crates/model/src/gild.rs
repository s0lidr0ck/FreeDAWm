//! Gild: effects on chosen words of a vocal (docs/gild.md).
//!
//! A [`Gilding`] is kept on the clip whose words it was made from, in the clip's source time like
//! the words themselves ([`crate::LyricWord`]), so moving, trimming, splitting, copying and comping
//! the clip carry it along, and Undo brings it back with the clip. It holds the span (start and end,
//! short fades at each edge), the words it was made from (by [`crate::LyricWord::id`]: the span
//! follows those words when they are retimed, unless its handles were moved by hand), and its
//! effects ([`GildFx`]), each with its own few settings and level.
//!
//! The effects are of two kinds (see the mixer, `soundcraft_mix`):
//! - **made ahead** (Shift, Grit, Double, Swell-in, Harmony): rendered off the audio thread into a
//!   [`GildRender`] (never saved; [`GildCache`]), re-rendered in the background when the clip's
//!   audio or the settings change ([`render_key`]). Shift (replacing) and Grit change the word
//!   itself, in the clip's audio before the track's inserts; the others are added at the Gild
//!   return.
//! - **live** (Delay and Reverb throws): built-in modules the track's strip runs while a gilding's
//!   word plays and its tail rings, fed by the track's signal after its inserts, gated to the span.
//!
//! What the effects add comes back into the same track after its inserts and before its fader,
//! sends and pan ([`GildTrack`]: Dry on/off and a level trim).

use crate::{Clip, ClipContent, ClipId, LyricWord};
use serde::{Deserialize, Deserializer, Serialize};
use soundcraft_time::Samples;
use std::collections::BTreeMap;
use std::sync::Arc;

/// Most gildings one clip keeps (a hostile file is cut down to these).
pub const MAX_GILDINGS: usize = 512;
/// Most words one gilding names.
pub const MAX_GILD_WORDS: usize = 64;
/// Longest gilding (seconds of the clip's audio).
pub const MAX_GILD_SECS: f64 = 30.0;
/// Most harmony voices on one gilding.
pub const MAX_HARMONY_VOICES: usize = 3;

/// A delay throw's note value (tempo-synced).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
pub enum GildNote {
    #[serde(rename = "1/16")]
    Sixteenth,
    #[serde(rename = "1/8t")]
    EighthTriplet,
    #[serde(rename = "1/8")]
    Eighth,
    #[serde(rename = "1/8d")]
    DottedEighth,
    #[serde(rename = "1/4t")]
    QuarterTriplet,
    #[default]
    #[serde(rename = "1/4")]
    Quarter,
    #[serde(rename = "1/4d")]
    DottedQuarter,
    #[serde(rename = "1/2")]
    Half,
    #[serde(rename = "1/1")]
    Whole,
}

impl GildNote {
    pub const ALL: [GildNote; 9] = [
        GildNote::Sixteenth,
        GildNote::EighthTriplet,
        GildNote::Eighth,
        GildNote::DottedEighth,
        GildNote::QuarterTriplet,
        GildNote::Quarter,
        GildNote::DottedQuarter,
        GildNote::Half,
        GildNote::Whole,
    ];

    /// Its length in quarter notes.
    pub fn beats(self) -> f64 {
        match self {
            GildNote::Sixteenth => 0.25,
            GildNote::EighthTriplet => 1.0 / 3.0,
            GildNote::Eighth => 0.5,
            GildNote::DottedEighth => 0.75,
            GildNote::QuarterTriplet => 2.0 / 3.0,
            GildNote::Quarter => 1.0,
            GildNote::DottedQuarter => 1.5,
            GildNote::Half => 2.0,
            GildNote::Whole => 4.0,
        }
    }

    /// The protocol name ("1/4", "1/8d", …).
    pub fn name(self) -> &'static str {
        match self {
            GildNote::Sixteenth => "1/16",
            GildNote::EighthTriplet => "1/8t",
            GildNote::Eighth => "1/8",
            GildNote::DottedEighth => "1/8d",
            GildNote::QuarterTriplet => "1/4t",
            GildNote::Quarter => "1/4",
            GildNote::DottedQuarter => "1/4d",
            GildNote::Half => "1/2",
            GildNote::Whole => "1/1",
        }
    }

    /// How the panel shows it.
    pub fn label(self) -> &'static str {
        match self {
            GildNote::Sixteenth => "1/16",
            GildNote::EighthTriplet => "1/8 triplet",
            GildNote::Eighth => "1/8",
            GildNote::DottedEighth => "dotted 1/8",
            GildNote::QuarterTriplet => "1/4 triplet",
            GildNote::Quarter => "1/4",
            GildNote::DottedQuarter => "dotted 1/4",
            GildNote::Half => "1/2",
            GildNote::Whole => "whole bar",
        }
    }

    pub fn from_name(s: &str) -> Option<GildNote> {
        GildNote::ALL.into_iter().find(|n| n.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// The room a reverb throw or a swell rings in.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GildSpace {
    Room,
    #[default]
    Hall,
    Plate,
    /// A slow, wide wash.
    Cloud,
}

impl GildSpace {
    pub const ALL: [GildSpace; 4] = [GildSpace::Room, GildSpace::Hall, GildSpace::Plate, GildSpace::Cloud];

    pub fn name(self) -> &'static str {
        match self {
            GildSpace::Room => "room",
            GildSpace::Hall => "hall",
            GildSpace::Plate => "plate",
            GildSpace::Cloud => "cloud",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GildSpace::Room => "Room",
            GildSpace::Hall => "Hall",
            GildSpace::Plate => "Plate",
            GildSpace::Cloud => "Cloud",
        }
    }

    pub fn from_name(s: &str) -> Option<GildSpace> {
        GildSpace::ALL.into_iter().find(|n| n.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// Grit's character.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum GritFlavour {
    /// Round, valve-like.
    #[default]
    Warm,
    /// Forward, console-like.
    Bright,
    /// Hard-edged, transistor-like.
    Hard,
}

impl GritFlavour {
    pub const ALL: [GritFlavour; 3] = [GritFlavour::Warm, GritFlavour::Bright, GritFlavour::Hard];

    pub fn name(self) -> &'static str {
        match self {
            GritFlavour::Warm => "warm",
            GritFlavour::Bright => "bright",
            GritFlavour::Hard => "hard",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            GritFlavour::Warm => "Warm",
            GritFlavour::Bright => "Bright",
            GritFlavour::Hard => "Hard",
        }
    }

    pub fn from_name(s: &str) -> Option<GritFlavour> {
        GritFlavour::ALL.into_iter().find(|n| n.name().eq_ignore_ascii_case(s.trim()))
    }
}

/// Delay throw: the word, and only the word, feeds an echo that repeats on after it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildEcho {
    /// Follow the tempo map (the note value), else `ms`.
    pub sync: bool,
    pub note: GildNote,
    /// Free time, 30..1500 ms.
    pub ms: f32,
    /// How much of each repeat comes round again, 0..0.9.
    pub repeats: f32,
    /// Low cut of the repeats, 40..1000 Hz.
    pub low_cut: f32,
    /// High cut of the repeats, 1500..18000 Hz.
    pub high_cut: f32,
    /// Repeats bounce left and right (a stereo track).
    pub bounce: bool,
    /// -36..+6 dB.
    pub level_db: f32,
}

impl Default for GildEcho {
    fn default() -> Self {
        GildEcho { sync: true, note: GildNote::Quarter, ms: 360.0, repeats: 0.4, low_cut: 220.0, high_cut: 5200.0, bounce: false, level_db: -5.0 }
    }
}

/// Reverb throw: the word rings in a space after it is sung.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildVerb {
    pub space: GildSpace,
    /// 0..1.
    pub size: f32,
    /// 0.4..12 s.
    pub decay_s: f32,
    /// 0..200 ms.
    pub pre_ms: f32,
    /// 0..1.5 (1 = as wide as the space is).
    pub width: f32,
    /// -36..+6 dB.
    pub level_db: f32,
}

impl Default for GildVerb {
    fn default() -> Self {
        GildVerb { space: GildSpace::Hall, size: 0.65, decay_s: 2.8, pre_ms: 35.0, width: 1.0, level_db: -6.0 }
    }
}

/// Double: a second, slightly late and slightly detuned copy of the word.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildDouble {
    /// 2..30 cents (the copy sits that far above).
    pub cents: f32,
    /// 5..60 ms late.
    pub ms: f32,
    /// -24..0 dB.
    pub level_db: f32,
}

impl Default for GildDouble {
    fn default() -> Self {
        GildDouble { cents: 9.0, ms: 22.0, level_db: -6.0 }
    }
}

/// Swell-in: a reversed reverb of the word that rises into it and stops where it starts.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildSwell {
    /// How long it rises, 0.3..4 s.
    pub length_s: f32,
    /// Where it ends against the word's start, -400..+100 ms (negative: before it).
    pub offset_ms: f32,
    pub space: GildSpace,
    /// -24..+6 dB.
    pub level_db: f32,
}

impl Default for GildSwell {
    fn default() -> Self {
        GildSwell { length_s: 1.6, offset_ms: 0.0, space: GildSpace::Hall, level_db: -4.0 }
    }
}

/// Shift: the word's pitch and formant moved, in place of the word or layered with it.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildShift {
    /// -12..+12 semitones.
    pub semitones: f32,
    /// -6..+6 semitones of formant (throat size).
    pub formant: f32,
    /// Layered with the word (added at the Gild return) rather than in its place.
    pub layer: bool,
    /// The layer's level, -24..+6 dB (in place: ignored).
    pub level_db: f32,
}

impl Default for GildShift {
    fn default() -> Self {
        GildShift { semitones: -12.0, formant: 0.0, layer: true, level_db: -6.0 }
    }
}

/// Grit: the word saturated or distorted, blended with itself.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildGrit {
    /// 0..1.
    pub drive: f32,
    pub flavour: GritFlavour,
    /// How much of the gritty word is heard, 0..1.
    pub blend: f32,
    /// -12..+6 dB.
    pub level_db: f32,
}

impl Default for GildGrit {
    fn default() -> Self {
        GildGrit { drive: 0.55, flavour: GritFlavour::Warm, blend: 0.6, level_db: 0.0 }
    }
}

/// One harmony voice: scale steps from the sung note in the song's key, level and pan.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildVoice {
    /// -7..+7 scale steps (2: a third above, 4: a fifth above, -2: a third below, 7: an octave).
    pub steps: i8,
    /// -24..+6 dB.
    pub level_db: f32,
    /// -1..1 (a stereo track).
    pub pan: f32,
}

impl Default for GildVoice {
    fn default() -> Self {
        GildVoice { steps: 2, level_db: -6.0, pan: 0.0 }
    }
}

/// Harmony on the word: up to three voices in the song's key.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildHarmony {
    pub voices: Vec<GildVoice>,
}

impl Default for GildHarmony {
    fn default() -> Self {
        GildHarmony { voices: vec![GildVoice::default()] }
    }
}

/// A gilding's effects; `None`: that effect is off. Heard in a fixed order: Shift and Grit change
/// the word, then Double, Harmony, Delay and Reverb are added (each added effect hears the word
/// and what the effects before it added); Swell-in rises into the first word.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct GildFx {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub shift: Option<GildShift>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub grit: Option<GildGrit>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub double: Option<GildDouble>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub harmony: Option<GildHarmony>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub delay: Option<GildEcho>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reverb: Option<GildVerb>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub swell: Option<GildSwell>,
}

/// The effect kinds, in the order they are heard (and listed).
pub const GILD_KINDS: [&str; 7] = ["shift", "grit", "double", "harmony", "delay", "reverb", "swell"];

impl GildFx {
    pub fn is_empty(&self) -> bool {
        self.kinds().is_empty()
    }

    /// The kinds that are on, in [`GILD_KINDS`] order.
    pub fn kinds(&self) -> Vec<&'static str> {
        GILD_KINDS.iter().copied().filter(|k| self.has(k)).collect()
    }

    pub fn has(&self, kind: &str) -> bool {
        match kind {
            "shift" => self.shift.is_some(),
            "grit" => self.grit.is_some(),
            "double" => self.double.is_some(),
            "harmony" => self.harmony.is_some(),
            "delay" => self.delay.is_some(),
            "reverb" => self.reverb.is_some(),
            "swell" => self.swell.is_some(),
            _ => false,
        }
    }

    /// Whether any effect is made ahead of time (rendered, see [`GildRender`]).
    pub fn rendered(&self) -> bool {
        self.shift.is_some() || self.grit.is_some() || self.double.is_some() || self.harmony.is_some() || self.swell.is_some()
    }

    /// Whether the word itself is changed (Shift in place, Grit).
    pub fn changes_word(&self) -> bool {
        self.shift.is_some_and(|s| !s.layer) || self.grit.is_some()
    }

    /// Whether a live effect runs (Delay, Reverb).
    pub fn live(&self) -> bool {
        self.delay.is_some() || self.reverb.is_some()
    }

    /// Every value within its range (non-finite values become the default).
    pub fn sanitize(&mut self) {
        fn c(v: &mut f32, lo: f32, hi: f32, d: f32) {
            *v = if v.is_finite() { v.clamp(lo, hi) } else { d };
        }
        if let Some(x) = &mut self.shift {
            let d = GildShift::default();
            c(&mut x.semitones, -12.0, 12.0, d.semitones);
            c(&mut x.formant, -6.0, 6.0, d.formant);
            c(&mut x.level_db, -24.0, 6.0, d.level_db);
        }
        if let Some(x) = &mut self.grit {
            let d = GildGrit::default();
            c(&mut x.drive, 0.0, 1.0, d.drive);
            c(&mut x.blend, 0.0, 1.0, d.blend);
            c(&mut x.level_db, -12.0, 6.0, d.level_db);
        }
        if let Some(x) = &mut self.double {
            let d = GildDouble::default();
            c(&mut x.cents, 2.0, 30.0, d.cents);
            c(&mut x.ms, 5.0, 60.0, d.ms);
            c(&mut x.level_db, -24.0, 0.0, d.level_db);
        }
        if let Some(x) = &mut self.harmony {
            x.voices.truncate(MAX_HARMONY_VOICES);
            for v in &mut x.voices {
                v.steps = v.steps.clamp(-7, 7);
                c(&mut v.level_db, -24.0, 6.0, -6.0);
                c(&mut v.pan, -1.0, 1.0, 0.0);
            }
            if x.voices.is_empty() {
                x.voices.push(GildVoice::default());
            }
        }
        if let Some(x) = &mut self.delay {
            let d = GildEcho::default();
            c(&mut x.ms, 30.0, 1500.0, d.ms);
            c(&mut x.repeats, 0.0, 0.9, d.repeats);
            c(&mut x.low_cut, 40.0, 1000.0, d.low_cut);
            c(&mut x.high_cut, 1500.0, 18_000.0, d.high_cut);
            c(&mut x.level_db, -36.0, 6.0, d.level_db);
        }
        if let Some(x) = &mut self.reverb {
            let d = GildVerb::default();
            c(&mut x.size, 0.0, 1.0, d.size);
            c(&mut x.decay_s, 0.4, 12.0, d.decay_s);
            c(&mut x.pre_ms, 0.0, 200.0, d.pre_ms);
            c(&mut x.width, 0.0, 1.5, d.width);
            c(&mut x.level_db, -36.0, 6.0, d.level_db);
        }
        if let Some(x) = &mut self.swell {
            let d = GildSwell::default();
            c(&mut x.length_s, 0.3, 4.0, d.length_s);
            c(&mut x.offset_ms, -400.0, 100.0, d.offset_ms);
            c(&mut x.level_db, -24.0, 6.0, d.level_db);
        }
    }
}

/// A gilding: effects on a span of a clip's words. Times are in the clip's source (as its
/// words'): the span plays where the clip plays that part of its audio.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Gilding {
    /// Unique among the clip's gildings (never 0).
    pub id: u32,
    pub start: Samples,
    pub end: Samples,
    /// The handles were moved by hand: the span keeps its own times when its words move.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub held: bool,
    /// The words it was made from ([`LyricWord::id`]).
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<u32>,
    /// Those words' text, as they read when it was made (shown, and used to find the words again
    /// after they were timed anew).
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub text: String,
    /// The fade into and out of the span, 0..200 ms.
    #[serde(default = "fade_in_default")]
    pub fade_in_ms: f32,
    #[serde(default = "fade_out_default")]
    pub fade_out_ms: f32,
    #[serde(default)]
    pub fx: GildFx,
}

fn fade_in_default() -> f32 {
    8.0
}

fn fade_out_default() -> f32 {
    20.0
}

impl Gilding {
    pub fn new(id: u32, start: Samples, end: Samples) -> Gilding {
        Gilding {
            id,
            start,
            end: end.max(start),
            held: false,
            words: Vec::new(),
            text: String::new(),
            fade_in_ms: fade_in_default(),
            fade_out_ms: fade_out_default(),
            fx: GildFx::default(),
        }
    }

    /// Its span on the timeline for `clip` (the part inside the clip's window), or `None` when
    /// none of it is heard.
    pub fn on_timeline(&self, clip: &Clip) -> Option<(Samples, Samples)> {
        let base = source_base(clip);
        let (lo, hi) = (base, base.saturating_add(clip.length));
        let (a, b) = (self.start.max(lo), self.end.min(hi));
        (b > a).then(|| (clip.start.saturating_add(a - base), clip.start.saturating_add(b - base)))
    }

    /// The timeline position of source position `src` of `clip`.
    pub fn timeline_of(clip: &Clip, src: Samples) -> Samples {
        clip.start.saturating_add(src.saturating_sub(source_base(clip)))
    }
}

fn source_base(clip: &Clip) -> Samples {
    match clip.content {
        ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => offset,
        ClipContent::Midi { .. } => 0,
    }
}

/// A track's Gild controls.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GildTrack {
    /// The track's own signal is heard; off, the track gives only what Gild adds (for a duplicate
    /// track that carries only the effects).
    pub dry: bool,
    /// Trim of everything Gild adds on the track, -24..+12 dB.
    pub level_db: f32,
}

impl Default for GildTrack {
    fn default() -> Self {
        GildTrack { dry: true, level_db: 0.0 }
    }
}

impl GildTrack {
    pub fn is_default(&self) -> bool {
        *self == GildTrack::default()
    }

    pub fn sanitize(&mut self) {
        self.level_db = if self.level_db.is_finite() { self.level_db.clamp(-24.0, 12.0) } else { 0.0 };
    }
}

/// A clip's gildings as a session file can hold them: ids unique and non-zero, spans ordered and
/// at most [`MAX_GILD_SECS`] long (`rate`: the session's), values in range, at most
/// [`MAX_GILDINGS`].
pub fn sanitize_gildings(gs: &mut Vec<Gilding>, rate: f64) {
    let longest = (MAX_GILD_SECS * rate.max(1.0)) as Samples;
    gs.truncate(MAX_GILDINGS);
    let mut seen = std::collections::BTreeSet::new();
    let mut next = gs.iter().map(|g| g.id).max().unwrap_or(0);
    for g in gs.iter_mut() {
        if g.id == 0 || !seen.insert(g.id) {
            next = next.saturating_add(1);
            g.id = next;
            seen.insert(g.id);
        }
        g.start = g.start.max(0);
        g.end = g.end.max(g.start).min(g.start.saturating_add(longest));
        g.words.truncate(MAX_GILD_WORDS);
        g.text = g.text.chars().filter(|c| !c.is_control()).take(256).collect();
        for f in [&mut g.fade_in_ms, &mut g.fade_out_ms] {
            *f = if f.is_finite() { f.clamp(0.0, 200.0) } else { 10.0 };
        }
        g.fx.sanitize();
    }
    gs.retain(|g| g.end > g.start);
    gs.sort_by_key(|g| (g.start, g.id));
}

/// Reads gildings leniently: anything unreadable loads as none, never a failed session. The
/// session's sanitize caps them (it knows the rate).
pub(crate) fn de_gild<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<Gilding>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let mut gs = match v {
        serde_json::Value::Array(items) => items.into_iter().filter_map(|x| serde_json::from_value::<Gilding>(x).ok()).collect(),
        _ => Vec::new(),
    };
    sanitize_gildings(&mut gs, 192_000.0);
    Ok(gs)
}

/// Gives every word of `clip` an id ([`LyricWord::id`]) that has none, unique in the clip. A word
/// sharing another's id (the second part of a split word) gets its own, and the gildings made
/// from the word name both parts.
pub fn ensure_word_ids(clip: &mut Clip) {
    let mut next = clip.lyrics.iter().flat_map(|l| l.words.iter()).map(|w| w.id).max().unwrap_or(0);
    let mut seen = std::collections::BTreeSet::new();
    let mut parts: Vec<(u32, u32)> = Vec::new();
    for w in clip.lyrics.iter_mut().flat_map(|l| l.words.iter_mut()) {
        if w.id == 0 || !seen.insert(w.id) {
            next = next.saturating_add(1);
            if w.id != 0 {
                parts.push((w.id, next));
            }
            w.id = next;
            seen.insert(w.id);
        }
    }
    for (was, now) in parts {
        for g in clip.gild.iter_mut().filter(|g| g.words.contains(&was) && g.words.len() < MAX_GILD_WORDS) {
            g.words.push(now);
        }
    }
}

/// The next free gilding id of `clip`.
pub fn next_gild_id(clip: &Clip) -> u32 {
    clip.gild.iter().map(|g| g.id).max().unwrap_or(0).saturating_add(1).max(1)
}

fn words_of<'a>(clip: &'a Clip, ids: &'a [u32]) -> impl Iterator<Item = &'a LyricWord> + 'a {
    clip.lyrics.iter().flat_map(|l| l.words.iter()).filter(move |w| w.id != 0 && ids.contains(&w.id))
}

fn norm(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric() || c.is_whitespace()).collect::<String>().to_lowercase().split_whitespace().collect::<Vec<_>>().join(" ")
}

/// Gildings follow their words (a word retimed, split, merged or timed anew): the span of a
/// gilding not [`Gilding::held`] becomes the span of its words. Words timed anew (new words, no
/// ids) are found again by their text over the old span. Returns whether anything changed.
pub fn follow_words(clip: &mut Clip) -> bool {
    if clip.gild.is_empty() {
        return false;
    }
    let mut changed = false;
    let mut ids: Vec<u32> = clip.lyrics.iter().flat_map(|l| l.words.iter()).map(|w| w.id).collect();
    let n = ids.len();
    ids.sort_unstable();
    ids.dedup();
    if ids.first() == Some(&0) || ids.len() != n {
        ensure_word_ids(clip);
        changed = true;
    }
    let mut spans: Vec<(usize, Samples, Samples, Option<Vec<u32>>)> = Vec::new();
    for (i, g) in clip.gild.iter().enumerate() {
        if g.held || (g.words.is_empty() && g.text.is_empty()) {
            continue;
        }
        let found: Vec<&LyricWord> = words_of(clip, &g.words).collect();
        if !found.is_empty() {
            let a = found.iter().map(|w| w.start).min().unwrap_or(g.start);
            let b = found.iter().map(|w| w.end).max().unwrap_or(g.end);
            if (a, b) != (g.start, g.end) && b > a {
                spans.push((i, a, b, None));
            }
            continue;
        }
        // Its words are gone (timed anew): the words over its old span reading the same.
        let want = norm(&g.text);
        if want.is_empty() {
            continue;
        }
        let mut over: Vec<&LyricWord> = clip.lyrics.iter().flat_map(|l| l.words.iter()).filter(|w| w.end > g.start && w.start < g.end).collect();
        over.sort_by_key(|w| w.start);
        let text = norm(&over.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" "));
        if !over.is_empty() && text == want {
            let a = over.iter().map(|w| w.start).min().unwrap_or(g.start);
            let b = over.iter().map(|w| w.end).max().unwrap_or(g.end);
            spans.push((i, a, b, Some(over.iter().map(|w| w.id).collect())));
        }
    }
    for (i, a, b, ids) in spans {
        if let Some(g) = clip.gild.get_mut(i) {
            g.start = a;
            g.end = b;
            if let Some(ids) = ids {
                g.words = ids;
            }
            changed = true;
        }
    }
    changed
}

// ─── Renders (derived, never saved) ──────────────────────────────────────────────────────────

/// Audio a gilding's render made, in the clip's source time: `channels[c][i]` is at source
/// position `start + i`.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GildAudio {
    pub start: Samples,
    pub channels: Vec<Arc<Vec<f32>>>,
}

impl GildAudio {
    pub fn len(&self) -> usize {
        self.channels.iter().map(|c| c.len()).min().unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Source position just past it.
    pub fn end(&self) -> Samples {
        self.start.saturating_add(Samples::try_from(self.len()).unwrap_or(Samples::MAX))
    }

    /// Channel `ch` (taken `ch % channels`) at source position `at`; `None` outside it. No
    /// allocation: safe on the audio thread.
    pub fn sample_at(&self, ch: usize, at: Samples) -> Option<f32> {
        let n = self.channels.len();
        if n == 0 {
            return None;
        }
        let k = usize::try_from(at.checked_sub(self.start)?).ok()?;
        self.channels.get(ch % n)?.get(k).copied()
    }
}

/// What a gilding's effects made ahead of time ([`GildFx::rendered`]): made from `key`'s inputs.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct GildRender {
    /// [`render_key`] of what it was made from.
    pub key: u64,
    /// The word as changed (Shift in place, Grit), crossfaded into the clip's own audio at its
    /// edges: it plays in place of the clip's audio over its range.
    pub change: Option<GildAudio>,
    /// Added at the Gild return and heard by the Delay and Reverb: Double, Shift layered, Harmony.
    pub layer: Option<GildAudio>,
    /// Added at the Gild return only: the swell, ending where the word starts.
    pub swell: Option<GildAudio>,
}

/// The renders of every gilding by clip and gilding id (never saved, recomputed in the
/// background; the engine carries them across Undo and Redo). Behind an `Arc`, so session
/// snapshots share it.
#[derive(Debug, Clone, Default)]
pub struct GildCache {
    pub renders: Arc<BTreeMap<(ClipId, u32), Arc<GildRender>>>,
    /// Render keys whose job failed, and why: not offered again.
    pub failed: Arc<BTreeMap<u64, String>>,
}

impl GildCache {
    pub fn same(&self, o: &GildCache) -> bool {
        Arc::ptr_eq(&self.renders, &o.renders) && Arc::ptr_eq(&self.failed, &o.failed)
    }

    /// The render of gilding `g` of `clip` when it was made from what they are now. Reads only:
    /// safe on the audio thread.
    pub fn current<'a>(&'a self, clip: &Clip, g: &Gilding, rate: u32) -> Option<&'a GildRender> {
        if self.renders.is_empty() {
            return None;
        }
        let key = render_key(clip, g, rate)?;
        self.renders.get(&(clip.id, g.id)).map(|r| &**r).filter(|r| r.key == key)
    }
}

impl PartialEq for GildCache {
    /// Derived data: two sessions are equal whatever their caches hold.
    fn eq(&self, _: &Self) -> bool {
        true
    }
}

struct Fnv(u64);

impl Fnv {
    fn new() -> Fnv {
        Fnv(0xcbf2_9ce4_8422_2325)
    }
    fn u64(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
    fn f(&mut self, v: f32) {
        self.u64(u64::from(v.to_bits()));
    }
    fn i(&mut self, v: i64) {
        self.u64(v as u64);
    }
}

/// Fingerprint of what a gilding's render is made from (the clip's audio as it plays before its
/// inserts, the span and the made-ahead effects' settings, the rate); `None` when it has no
/// made-ahead effect. No allocation: the mixer checks it on the audio thread.
pub fn render_key(clip: &Clip, g: &Gilding, rate: u32) -> Option<u64> {
    let fx = &g.fx;
    if !fx.rendered() {
        return None;
    }
    let mut h = Fnv::new();
    h.u64(u64::from(rate));
    match clip.content {
        ClipContent::Audio { source, .. } => h.u64(source.0),
        _ => return None,
    }
    h.u64(clip.source_channel.map_or(u64::MAX, u64::from));
    if let Some(k) = clip.clean_alt() {
        h.u64(k.source.0);
        h.f(k.wet());
        h.i(k.origin);
    }
    h.u64(clip.tune_live().map_or(0, |t| t.fingerprint));
    h.i(g.start);
    h.i(g.end);
    h.f(g.fade_in_ms);
    h.f(g.fade_out_ms);
    if let Some(x) = fx.shift {
        h.u64(1);
        h.f(x.semitones);
        h.f(x.formant);
        h.u64(u64::from(x.layer));
        h.f(x.level_db);
    }
    if let Some(x) = fx.grit {
        h.u64(2);
        h.f(x.drive);
        h.u64(x.flavour as u64);
        h.f(x.blend);
        h.f(x.level_db);
    }
    if let Some(x) = fx.double {
        h.u64(3);
        h.f(x.cents);
        h.f(x.ms);
        h.f(x.level_db);
    }
    if let Some(x) = &fx.harmony {
        h.u64(4);
        for v in &x.voices {
            h.i(i64::from(v.steps));
            h.f(v.level_db);
            h.f(v.pan);
        }
    }
    if let Some(x) = fx.swell {
        h.u64(5);
        h.f(x.length_s);
        h.f(x.offset_ms);
        h.u64(x.space as u64);
        h.f(x.level_db);
    }
    Some(h.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{LyricLine, SourceId};

    fn clip_with_words() -> Clip {
        let mut c = Clip::audio(ClipId(1), "v", SourceId(1), 1000, 5000, 100_000);
        let mut w = |s, e, t: &str| LyricWord::new(s, e, t);
        c.lyrics = vec![LyricLine { start: 2000, end: 9000, words: vec![w(2000, 4000, "Amen"), w(5000, 9000, "Lord")], sheet_line: None }];
        c
    }

    #[test]
    fn a_gilding_follows_its_words_unless_held() {
        let mut c = clip_with_words();
        ensure_word_ids(&mut c);
        let id = c.lyrics[0].words[1].id;
        let mut g = Gilding::new(1, 5000, 9000);
        g.words = vec![id];
        g.text = "Lord".into();
        c.gild.push(g);
        c.lyrics[0].words[1].start = 5200;
        c.lyrics[0].words[1].end = 9500;
        assert!(follow_words(&mut c));
        assert_eq!((c.gild[0].start, c.gild[0].end), (5200, 9500));
        c.gild[0].held = true;
        c.lyrics[0].words[1].end = 9900;
        assert!(!follow_words(&mut c));
        assert_eq!(c.gild[0].end, 9500, "held handles keep their own times");
        // Timed anew: new words with no ids, found again by their text.
        c.gild[0].held = false;
        c.lyrics[0].words[1] = LyricWord::new(5100, 9100, "Lord,");
        assert!(follow_words(&mut c));
        assert_eq!((c.gild[0].start, c.gild[0].end), (5100, 9100));
        assert_eq!(c.gild[0].words, vec![c.lyrics[0].words[1].id]);
    }

    #[test]
    fn timeline_span_is_cut_to_the_window() {
        let c = clip_with_words();
        let g = Gilding::new(1, 500, 3000);
        assert_eq!(g.on_timeline(&c), Some((5000, 7000)));
        assert_eq!(Gilding::new(2, 200_000, 300_000).on_timeline(&c), None);
    }

    #[test]
    fn hostile_gildings_are_tamed() {
        let v = serde_json::json!([
            {"id": 0, "start": 50, "end": 10, "fx": {"delay": {"repeats": 9.0, "ms": "x"}}},
            {"id": 3, "start": 0, "end": 99999999999i64, "fade_in_ms": -4, "fx": {"reverb": {"decay_s": 1e9}}},
            {"id": 3, "start": 5, "end": 9},
            "nonsense"
        ]);
        #[derive(Deserialize)]
        struct W(#[serde(deserialize_with = "de_gild")] Vec<Gilding>);
        let W(gs) = serde_json::from_value(v).unwrap();
        assert_eq!(gs.len(), 2, "an empty span and a non-gilding are dropped: {gs:?}");
        let ids: std::collections::BTreeSet<u32> = gs.iter().map(|g| g.id).collect();
        assert_eq!(ids.len(), 2);
        assert!(gs.iter().all(|g| g.fade_in_ms >= 0.0));
        assert!(gs.iter().filter_map(|g| g.fx.reverb).all(|r| r.decay_s <= 12.0));
    }

    #[test]
    fn render_key_follows_settings_and_audio_not_live_effects() {
        let c = clip_with_words();
        let mut g = Gilding::new(1, 2000, 4000);
        assert_eq!(render_key(&c, &g, 48_000), None);
        g.fx.delay = Some(GildEcho::default());
        assert_eq!(render_key(&c, &g, 48_000), None, "a live effect renders nothing");
        g.fx.double = Some(GildDouble::default());
        let k = render_key(&c, &g, 48_000);
        assert!(k.is_some());
        g.fx.delay = Some(GildEcho { repeats: 0.8, ..GildEcho::default() });
        assert_eq!(render_key(&c, &g, 48_000), k);
        g.fx.double = Some(GildDouble { cents: 12.0, ..GildDouble::default() });
        assert_ne!(render_key(&c, &g, 48_000), k);
    }

    #[test]
    fn older_files_shape_is_kept() {
        let c = clip_with_words();
        let v = serde_json::to_value(&c).unwrap();
        assert!(v.get("gild").is_none(), "no gildings: nothing written");
        let w = serde_json::to_value(&c.lyrics[0].words[0]).unwrap();
        assert!(w.get("id").is_none(), "a word without an id is written as before");
    }
}

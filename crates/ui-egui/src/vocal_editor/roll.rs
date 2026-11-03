//! The roll: chord and key strips, time ruler, piano keys and grid (rows shaded by the piano,
//! hatched when out of key), hatched unvoiced blocks, the
//! pitch curves (original, tuned and ghost), the note bars with their offset labels and scoops,
//! the playhead and the colour legend; and the mouse: click/Shift/Ctrl selection, rubber band,
//! the hover tooltip and the transition popup.

use super::layout::{Frames, NoteShape, RollView, curve_points_where, note_shapes};
use super::{hook, store};
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use crate::vocal_style::{fmt_cents, note_name, off_colour};
use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_engine::vocal::{NoteRef, SessionScale};
use soundcraft_model::{AnalysisKey, Clip, ClipId, Session, TrackId};
use soundcraft_time::Samples;
use soundcraft_vocal::{EditCurves, Note, NoteEdit, NoteId, ScaleAt, Transition, VocalAnalysis, moved_target, target_cents, typical_level_db};
use std::collections::{BTreeMap, BTreeSet};
use std::sync::Arc;

/// The piano keys' width (as the MIDI Editor's).
pub const KEY_W: f32 = 48.0;
/// The chord strip's height.
pub const CHORD_H: f32 = 22.0;
/// The key strip's height.
pub const KEY_STRIP_H: f32 = 20.0;
/// The time ruler's height.
pub const RULER_H: f32 = 18.0;
/// The scoop's colour.
pub const SCOOP_BLUE: Color32 = Color32::from_rgb(90, 160, 255);
/// An unvoiced block this long or longer (seconds) is labelled "breath".
const BREATH_SECS: f64 = 0.150;
/// The pitch range the rows cover (cents): C-1 to B9.
pub const TOP_CENTS: f32 = 12_800.0;

/// One analysed clip of the editor track, ready to draw.
#[derive(Debug, Clone)]
pub struct ClipView {
    pub clip: Clip,
    pub a: Arc<VocalAnalysis>,
    pub curves: Option<Arc<EditCurves>>,
    pub edits: BTreeMap<NoteId, NoteEdit>,
    pub scale: SessionScale,
    /// Stretched clips can't be tuned: drawn with a message, no notes.
    pub stretched: bool,
}

/// Where each note id of one recording shows on the track: (analysis key, note id) → the clips
/// whose window holds it (a note cut by a clip edge shows in both clips).
#[derive(Debug, Clone, Default)]
struct Twins {
    by_note: BTreeMap<(u64, NoteId), Vec<ClipId>>,
    /// The analysis key of every analysed, unstretched audio clip on the track.
    key_of: BTreeMap<ClipId, u64>,
}

/// One clip's analysis (its segmentation replayed) and edit curves; `None` before it is analysed.
pub type Derived = Option<(Arc<VocalAnalysis>, Option<Arc<EditCurves>>)>;

/// What one clip's derived data depends on: the cached analysis it hears (by `Arc`) and its
/// tuning (the render fingerprint, or a hash of the edits and scale while it plays Original).
#[derive(Debug, Clone, Default)]
struct ClipKey {
    analysis: Option<Arc<soundcraft_model::ClipAnalysis>>,
    tune: u64,
}

impl PartialEq for ClipKey {
    fn eq(&self, o: &Self) -> bool {
        self.tune == o.tune
            && match (&self.analysis, &o.analysis) {
                (Some(a), Some(b)) => Arc::ptr_eq(a, b),
                (None, None) => true,
                _ => false,
            }
    }
}

impl ClipKey {
    fn of(s: &Session, c: &Clip) -> ClipKey {
        let analysis = AnalysisKey::of(s, c).and_then(|k| s.vocal.analyses.get(&k.hash()).cloned());
        ClipKey { analysis, tune: tune_key(s, c) }
    }

    fn ptr(&self) -> usize {
        self.analysis.as_ref().map_or(0, |a| Arc::as_ptr(a) as usize)
    }
}

/// What `c`'s curves derive from in its tuning: the render fingerprint the engine keeps up to
/// date (`refresh_vocal_fingerprints`: the analysis, segmentation, edits and scale), or, while it
/// plays Original (no fingerprint), a hash of the same inputs. 0 without a live tuning.
fn tune_key(s: &Session, c: &Clip) -> u64 {
    use std::hash::{Hash, Hasher};
    let Some(t) = c.tune_live() else { return 0 };
    if !t.original && t.fingerprint != 0 {
        return t.fingerprint;
    }
    let mut h = std::collections::hash_map::DefaultHasher::new();
    serde_json::to_vec(&t.seg_ops).unwrap_or_default().hash(&mut h);
    serde_json::to_vec(&t.edits).unwrap_or_default().hash(&mut h);
    s.key_signatures.hash(&mut h);
    (s.band.key, s.band.scale, t.original).hash(&mut h);
    c.start.saturating_sub(c.source_offset()).hash(&mut h);
    h.finish() | 1
}

/// The track lane's segments of one clip are kept for this view: the visible span, the lane's
/// rect and the pitch range and typical level (as bits).
pub(crate) type LaneKey = (Samples, Samples, [u32; 4], [u32; 3]);

/// The track lane's pitch line of one clip.
pub(crate) type LaneSegs = Arc<Vec<crate::vocal_track::PitchSeg>>;

/// What was derived for one track at one state of its clips (their windows and [`ClipKey`]s).
#[derive(Debug, Clone, Default)]
struct TrackEntry {
    key: u64,
    typical: Option<f32>,
    twins: Option<Twins>,
    /// The last selection expanded by its twins, and the result.
    expanded: Option<(BTreeSet<NoteRef>, BTreeSet<NoteRef>)>,
    /// The track's note count (inside the clips' windows).
    total: Option<usize>,
    /// The typical level levelling aims at, for (selection, whole track).
    sel_typical: Option<(BTreeSet<NoteRef>, bool, f32)>,
    /// The track lane's pitch range for a visible span.
    range: Option<((Samples, Samples), (f32, f32))>,
    /// The track lane's pitch line per clip, for one view.
    lanes: BTreeMap<ClipId, (LaneKey, LaneSegs)>,
}

/// The vocal data derived from the session, shared by the Vocal Editor, the track pitch lanes and
/// the Mix window's PITCH sections. Each clip's analysis and curves are keyed on what they derive
/// from ([`ClipKey`]), each track's data on its clips' windows and keys: an edit elsewhere (a
/// fader move during playback) rebuilds nothing. The revision only says when to look at the keys
/// again.
#[derive(Debug, Clone, Default)]
pub struct Cache {
    /// The revision and analysis map the keys below were taken at.
    seen: Option<(u64, Arc<BTreeMap<u64, Arc<soundcraft_model::ClipAnalysis>>>)>,
    keys: BTreeMap<ClipId, ClipKey>,
    track_keys: BTreeMap<TrackId, u64>,
    clips: BTreeMap<ClipId, (ClipKey, Derived)>,
    tracks: BTreeMap<TrackId, TrackEntry>,
}

/// Whether a clip is stretched (its notes can't be tuned).
pub fn is_stretched(c: &Clip) -> bool {
    (c.stretch - 1.0).abs() > 1e-9
}

impl Cache {
    /// Takes the keys afresh when the revision or the analysis map changed (and forgets clips and
    /// tracks that are gone); the derived data stays until its own key changes.
    pub(crate) fn check(&mut self, rev: u64, s: &Session) {
        if self.seen.as_ref().is_some_and(|(r, a)| *r == rev && Arc::ptr_eq(a, &s.vocal.analyses)) {
            return;
        }
        self.seen = Some((rev, s.vocal.analyses.clone()));
        self.keys.clear();
        self.track_keys.clear();
        let ids: BTreeSet<ClipId> = s.tracks.iter().flat_map(|t| t.clips()).map(|c| c.id).collect();
        self.clips.retain(|id, _| ids.contains(id));
        self.tracks.retain(|t, _| s.track(*t).is_some());
    }

    fn clip_key(&mut self, s: &Session, c: &Clip) -> ClipKey {
        self.keys.entry(c.id).or_insert_with(|| ClipKey::of(s, c)).clone()
    }

    /// `c`'s analysis and curves, derived again only when its key changed.
    pub(crate) fn get(&mut self, s: &Session, c: &Clip) -> Derived {
        let key = self.clip_key(s, c);
        if let Some((k, d)) = self.clips.get(&c.id)
            && *k == key
        {
            return d.clone();
        }
        let d = key.analysis.as_ref().and_then(|_| {
            let a = super::analysis_for(s, c)?;
            let curves = soundcraft_engine::vocal::curves_for(s, c).map(Arc::new);
            Some((Arc::new(a), curves))
        });
        self.clips.insert(c.id, (key, d.clone()));
        d
    }

    fn track_key(&mut self, s: &Session, track: TrackId) -> u64 {
        use std::hash::{Hash, Hasher};
        if let Some(k) = self.track_keys.get(&track) {
            return *k;
        }
        let mut h = std::collections::hash_map::DefaultHasher::new();
        if let Some(t) = s.track(track) {
            for c in t.clips() {
                let k = self.clip_key(s, c);
                (c.id.0, c.start, c.length, c.source_offset(), c.stretch.to_bits(), c.is_audio(), k.ptr(), k.tune).hash(&mut h);
            }
        }
        // Never 0, the key of an entry not built yet.
        let k = h.finish() | 1;
        self.track_keys.insert(track, k);
        k
    }

    /// `track`'s entry, emptied when its clips changed.
    fn track(&mut self, s: &Session, track: TrackId) -> &mut TrackEntry {
        let key = self.track_key(s, track);
        let e = self.tracks.entry(track).or_default();
        if e.key != key {
            *e = TrackEntry { key, ..TrackEntry::default() };
        }
        e
    }

    /// The track lane's pitch range of `track` over `span`, when worked out for its clips as they are.
    pub(crate) fn range(&mut self, s: &Session, track: TrackId, span: (Samples, Samples)) -> Option<(f32, f32)> {
        self.track(s, track).range.filter(|(sp, _)| *sp == span).map(|(_, r)| r)
    }

    pub(crate) fn set_range(&mut self, s: &Session, track: TrackId, span: (Samples, Samples), r: (f32, f32)) {
        self.track(s, track).range = Some((span, r));
    }

    /// The track lane's pitch line of `clip` for `key`, when drawn before with the track as it is.
    pub(crate) fn lane(&mut self, s: &Session, track: TrackId, clip: ClipId, key: &LaneKey) -> Option<LaneSegs> {
        self.track(s, track).lanes.get(&clip).filter(|(k, _)| k == key).map(|(_, v)| v.clone())
    }

    pub(crate) fn set_lane(&mut self, s: &Session, track: TrackId, clip: ClipId, key: LaneKey, segs: LaneSegs) {
        self.track(s, track).lanes.insert(clip, (key, segs));
    }

    /// `track`'s typical note level.
    pub(crate) fn typical(&mut self, s: &Session, track: TrackId) -> f32 {
        *self.track(s, track).typical.get_or_insert_with(|| track_typical(s, track))
    }

    /// The twins map of `track` (built once per state of its clips from the cached analyses).
    fn twins(&mut self, s: &Session, track: TrackId) -> Twins {
        if let Some(t) = &self.track(s, track).twins {
            return t.clone();
        }
        let mut tw = Twins::default();
        if let Some(t) = s.track(track) {
            for c in t.clips().iter().filter(|c| c.is_audio() && !is_stretched(c)) {
                let Some(k) = AnalysisKey::of(s, c).map(|k| k.hash()) else { continue };
                let Some((a, _)) = self.get(s, c) else { continue };
                let fm = Frames::of(c, a.track.hop_samples);
                tw.key_of.insert(c.id, k);
                for n in a.notes.iter().filter(|n| fm.time(n.end) > c.start && fm.time(n.start) < c.end()) {
                    tw.by_note.entry((k, n.id)).or_default().push(c.id);
                }
            }
        }
        self.track(s, track).twins = Some(tw.clone());
        tw
    }

    /// `refs` and their twins, without the notes of stretched (or unanalysed) clips; remembered
    /// until the selection or the track's clips change.
    fn expand(&mut self, s: &Session, track: TrackId, refs: &BTreeSet<NoteRef>) -> BTreeSet<NoteRef> {
        if let Some((k, v)) = &self.track(s, track).expanded
            && k == refs
        {
            return v.clone();
        }
        let tw = self.twins(s, track);
        let mut out = BTreeSet::new();
        for r in refs {
            let Some(k) = tw.key_of.get(&r.clip) else { continue };
            out.insert(*r);
            if let Some(cs) = tw.by_note.get(&(*k, r.note)) {
                out.extend(cs.iter().map(|c| NoteRef { clip: *c, note: r.note }));
            }
        }
        self.track(s, track).expanded = Some((refs.clone(), out.clone()));
        out
    }

    /// Every note of the track inside its clips' windows, unstretched clips only.
    fn all_notes(&mut self, s: &Session, track: TrackId) -> Vec<NoteRef> {
        let tw = self.twins(s, track);
        let mut out: Vec<NoteRef> = tw.by_note.iter().flat_map(|((_, n), cs)| cs.iter().map(|c| NoteRef { clip: *c, note: *n })).collect();
        out.sort();
        out
    }
}

/// The session, with the shared cache's keys checked against it.
pub(crate) fn checked(app: &mut SoundApp) -> Arc<Session> {
    let s = app.engine.session_arc();
    let rev = app.engine.revision;
    app.vocal.cache.check(rev, &s);
    s
}

/// `clip`'s analysis and curves from the shared cache (`None` before it is analysed).
pub fn derived(app: &mut SoundApp, clip: &Clip) -> Derived {
    let s = checked(app);
    app.vocal.cache.get(&s, clip)
}

/// `refs` and every note that hears the same analysis and has the same id (a note cut by a clip
/// edge shows in both clips: selecting one selects both); notes of stretched clips are dropped.
/// Read from the shared cache: no analysis is copied.
pub fn with_twins(app: &mut SoundApp, track: TrackId, refs: &BTreeSet<NoteRef>) -> BTreeSet<NoteRef> {
    let s = checked(app);
    app.vocal.cache.expand(&s, track, refs)
}

/// The selection with its twins (see [`with_twins`]), remembered until it changes.
pub fn selection_with_twins(app: &mut SoundApp, track: TrackId) -> BTreeSet<NoteRef> {
    let s = checked(app);
    let st = &mut app.vocal;
    st.cache.expand(&s, track, &st.selected)
}

/// Every tunable note of the track (unstretched clips, inside their windows), from the cache.
pub fn tunable_notes(app: &mut SoundApp, track: TrackId) -> Vec<NoteRef> {
    let s = checked(app);
    app.vocal.cache.all_notes(&s, track)
}

/// The number of notes on the track inside its clips' windows (cached per state of its clips).
pub fn note_count(app: &mut SoundApp, track: TrackId) -> usize {
    let s = checked(app);
    let cache = &mut app.vocal.cache;
    if let Some(n) = cache.track(&s, track).total {
        return n;
    }
    let mut n = 0;
    if let Some(t) = s.track(track) {
        for c in t.clips().iter().filter(|c| c.is_audio()) {
            let Some((a, _)) = cache.get(&s, c) else { continue };
            let fm = Frames::of(c, a.track.hop_samples);
            n += a.notes.iter().filter(|x| fm.time(x.end) > c.start && fm.time(x.start) < c.end()).count();
        }
    }
    cache.track(&s, track).total = Some(n);
    n
}

/// Whether any of `clips` has an analysis (no copy once cached).
pub fn any_analysed(app: &mut SoundApp, clips: &[ClipId]) -> bool {
    let s = checked(app);
    let cache = &mut app.vocal.cache;
    clips.iter().any(|id| s.find_clip(*id).is_some_and(|(_, c)| cache.get(&s, c).is_some()))
}

/// The median note level of `track`'s analysed clips (inside their windows); −21 dB without notes.
pub(crate) fn track_typical(s: &Session, track: TrackId) -> f32 {
    let Some(t) = s.track(track) else { return -21.0 };
    let mut levels = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio()) {
        let Some(ca) = AnalysisKey::of(s, c).and_then(|k| s.vocal.analyses.get(&k.hash())) else { continue };
        let fm = Frames::of(c, ca.analysis.track.hop_samples);
        levels.extend(ca.analysis.notes.iter().filter(|n| fm.time(n.end) > c.start && fm.time(n.start) < c.end()).map(|n| n.level_db));
    }
    if levels.is_empty() { -21.0 } else { typical_level_db(&levels) }
}

/// The analysed audio clips of `track` that overlap `[t0, t1)` (all of them with `None`).
pub fn clip_views(app: &mut SoundApp, track: TrackId, range: Option<(Samples, Samples)>) -> Vec<ClipView> {
    let s = checked(app);
    let cache = &mut app.vocal.cache;
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio()) {
        if let Some((t0, t1)) = range
            && (c.end() <= t0 || c.start >= t1)
        {
            continue;
        }
        let Some((a, curves)) = cache.get(&s, c) else { continue };
        out.push(ClipView {
            clip: c.clone(),
            a,
            curves,
            edits: c.tune_live().map(|t| t.edits.clone()).unwrap_or_default(),
            scale: soundcraft_engine::vocal::scale_at(&s, c),
            stretched: is_stretched(c),
        });
    }
    out
}

/// The level the levelling aims at, which the Level lane's line and the panel's caption both
/// show: the track's typical level for a whole-track selection or none, else the selection's.
pub fn selection_typical(app: &mut SoundApp, track: TrackId) -> f32 {
    let whole = app.vocal.whole_track || app.vocal.selected.is_empty();
    if whole {
        return typical(app, track);
    }
    let s = checked(app);
    let st = &mut app.vocal;
    if let Some((sel, w, v)) = &st.cache.track(&s, track).sel_typical
        && *w == whole
        && *sel == st.selected
    {
        return *v;
    }
    let mut levels = Vec::new();
    let mut seen = BTreeSet::new();
    for r in &st.selected {
        let Some((_, c)) = s.find_clip(r.clip) else { continue };
        let Some((a, _)) = st.cache.get(&s, c) else { continue };
        // A note cut by a clip edge counts once.
        let key = AnalysisKey::of(&s, c).map(|k| k.hash());
        if !seen.insert((key, r.note)) {
            continue;
        }
        levels.extend(a.notes.iter().find(|n| n.id == r.note).map(|n| n.level_db));
    }
    let v = if levels.is_empty() { -21.0 } else { typical_level_db(&levels) };
    st.cache.track(&s, track).sel_typical = Some((st.selected.clone(), whole, v));
    v
}

/// The track's typical level (cached with its clips' analyses).
pub fn typical(app: &mut SoundApp, track: TrackId) -> f32 {
    let s = checked(app);
    app.vocal.cache.typical(&s, track)
}

pub use crate::session_key::{has_key, key_at, key_name};

/// The band chords' pitch classes by tick span, sorted by start: built once per frame so each
/// note's lookup is a binary search.
pub struct ChordTones {
    spans: Vec<(i64, i64, [bool; 12])>,
}

impl ChordTones {
    pub fn of(s: &Session) -> ChordTones {
        let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
        let mut spans: Vec<(i64, i64, [bool; 12])> = s
            .band
            .chords
            .iter()
            .filter(|c| c.length_ticks > 0)
            .map(|c| {
                let mut tones = soundcraft_band::chord_match::slot_pitch_classes(i32::from(c.slot), key, scale);
                if c.bass >= 0
                    && let Some(t) = tones.get_mut((key + i32::from(c.bass)).rem_euclid(12) as usize)
                {
                    *t = true;
                }
                (c.tick, c.tick.saturating_add(c.length_ticks), tones)
            })
            .collect();
        spans.sort_by_key(|x| x.0);
        ChordTones { spans }
    }

    /// The pitch classes of the chord sounding at `at` (`None` without one).
    pub fn at(&self, s: &Session, at: Samples) -> Option<[bool; 12]> {
        let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
        let k = self.spans.partition_point(|x| x.0 <= tick);
        k.checked_sub(1).and_then(|i| self.spans.get(i)).filter(|x| tick < x.1).map(|x| x.2)
    }
}

/// The pitch classes of the band chord sounding at `at` (`None` without one).
pub fn chord_tones_at(s: &Session, at: Samples) -> Option<[bool; 12]> {
    ChordTones::of(s).at(s, at)
}

/// The band chord's name at `at`.
pub fn chord_name_at(s: &Session, at: Samples) -> Option<String> {
    let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let c = s.band.chords.iter().find(|c| c.length_ticks > 0 && c.tick <= tick && tick < c.tick.saturating_add(c.length_ticks))?;
    Some(soundcraft_band::chord_theory::name_for(i32::from(c.slot), key, scale, i32::from(c.bass)))
}

/// A note's target pitch: the nearest semitone, or with Snap to key the nearest scale tone.
pub fn target_of(cv: &ClipView, n: &Note) -> f32 {
    let snap = cv.edits.get(&n.id).is_some_and(|e| e.snap_to_key);
    target_cents(n.centre_cents, if snap { cv.scale.scale_at(n.start) } else { None })
}

/// The target a note is judged by after its manual offset: [`target_of`] moved by the offset's
/// whole semitones (a note dragged up two rows is measured against the semitone two up).
pub fn moved_target_of(cv: &ClipView, n: &Note) -> f32 {
    moved_target(target_of(cv, n), cv.edits.get(&n.id).map_or(0.0, |e| e.offset_cents))
}

/// An edited note's tuned centre as the engine computes it (where its bar sits); `None` for a
/// note with no edit.
pub fn tuned_centre_of(cv: &ClipView, n: &Note) -> Option<f32> {
    let e = cv.edits.get(&n.id).filter(|e| !soundcraft_vocal::NoteEdit::clone(e).clamped().is_identity())?;
    Some(soundcraft_vocal::tuned_centre(&cv.a, n, e, &cv.scale))
}

/// Whether a target semitone is outside both the session scale and the chord at `at`.
fn off_key(cv: &ClipView, n: &Note, target: f32, chord: Option<[bool; 12]>) -> bool {
    let pc = ((target / 100.0).round() as i32).rem_euclid(12) as u8;
    let in_scale = cv.scale.scale_at(n.start).is_some_and(|set| set.contains(pc));
    let in_chord = chord.is_some_and(|t| t.get(usize::from(pc)).copied().unwrap_or(false));
    !in_scale && !in_chord
}

/// The roll's screen areas.
#[derive(Debug, Clone, Copy)]
pub struct Areas {
    pub chords: Rect,
    pub key: Rect,
    pub ruler: Rect,
    pub roll: Rect,
    pub keys: Rect,
}

fn dark(t: &Tokens) -> bool {
    let c = t.panel_bg;
    u32::from(c.r()) + u32::from(c.g()) + u32::from(c.b()) < 3 * 128
}

/// One row of the grid as drawn (for the tests): its MIDI note, whether its fill is the light
/// (white-key) one, and whether it is hatched as out of key.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RowFill {
    pub midi: i32,
    pub lit: bool,
    pub hatched: bool,
}

/// The hatch over an out-of-key row: 45° lines every [`HATCH_STEP`] px.
const HATCH_STEP: f32 = 6.0;

/// A diagonal hatch over `row` (clipped to the painter's clip rect).
fn row_hatch(painter: &Painter, row: Rect, c: Color32) {
    let vis = row.intersect(painter.clip_rect());
    if !vis.is_positive() {
        return;
    }
    let p = painter.with_clip_rect(vis);
    let h = row.height();
    let first = ((vis.left() - h - row.left()) / HATCH_STEP).floor().max(0.0);
    let mut x = row.left() + first * HATCH_STEP;
    while x < vis.right() {
        p.line_segment([pos2(x, row.bottom()), pos2(x + h, row.top())], Stroke::new(1.0, c));
        x += HATCH_STEP;
    }
}

/// Rows: the fill follows the piano keys (white-key rows light, black-key rows dark); with a
/// key set, the out-of-key rows carry a diagonal hatch over their fill, so key and piano both
/// read. Names on C and in-key rows. Returns each row as drawn.
fn grid(painter: &Painter, s: &Session, a: &Areas, v: &RollView, scale: Option<soundcraft_vocal::PitchClassSet>, t: &Tokens) -> Vec<RowFill> {
    let top = (v.top_cents / 100.0).ceil() as i32;
    let rows = (a.roll.height() / v.row_h).ceil() as i32 + 2;
    let is_dark = dark(t);
    let (light, deep) = if is_dark { (Color32::from_rgb(44, 45, 49), Color32::from_rgb(36, 37, 40)) } else { (t.playlist_bg, t.playlist_alt) };
    // 22 % black on the dark theme, 10 % on the light.
    let hatch_c = if is_dark { Color32::from_black_alpha(56) } else { Color32::from_black_alpha(26) };
    let mut fills = Vec::with_capacity(rows as usize);
    for k in 0..rows {
        let m = top - k;
        let y = v.y(m as f32 * 100.0 + 50.0);
        let row = Rect::from_min_size(pos2(a.roll.left(), y), vec2(a.roll.width(), v.row_h));
        let pc = m.rem_euclid(12) as u8;
        let black = matches!(pc, 1 | 3 | 6 | 8 | 10);
        let in_key = scale.map(|sc| sc.contains(pc));
        let lit = !black;
        painter.rect_filled(row, 0.0, if lit { light } else { deep });
        let hatched = in_key == Some(false);
        if hatched {
            row_hatch(&painter.with_clip_rect(a.roll), row, hatch_c);
        }
        fills.push(RowFill { midi: m, lit, hatched });
        painter.line_segment([row.left_top(), row.right_top()], Stroke::new(1.0, if is_dark { Color32::from_rgb(29, 30, 33) } else { t.grid_line }));
        let key = Rect::from_min_size(pos2(a.keys.left(), y), vec2(KEY_W - 2.0, v.row_h));
        painter.rect(
            key,
            0.0,
            if black { Color32::from_rgb(32, 33, 36) } else { Color32::from_rgb(231, 231, 231) },
            Stroke::new(1.0, Color32::from_rgb(17, 17, 17)),
            StrokeKind::Inside,
        );
        if (pc == 0 || in_key == Some(true)) && (0..=127).contains(&m) && v.row_h >= 11.0 {
            painter.text(
                pos2(key.right() - 5.0, key.center().y),
                Align2::RIGHT_CENTER,
                note_name(m as f32 * 100.0),
                bold(10.5),
                if black { Color32::from_rgb(187, 187, 187) } else { Color32::from_rgb(51, 51, 51) },
            );
        }
    }
    let _ = s;
    fills
}

/// Seconds between ruler labels: the smallest of a few steps that leaves 60 px between them.
fn ruler_step(v: &RollView, rate: f64) -> f64 {
    let px_per_sec = f64::from(v.rect.width()) / ((v.t1 - v.t0).max(1) as f64 / rate);
    [0.1, 0.25, 0.5, 1.0, 2.0, 5.0, 10.0, 15.0, 30.0, 60.0, 120.0, 300.0].into_iter().find(|s| s * px_per_sec >= 60.0).unwrap_or(600.0)
}

fn fmt_secs(secs: f64) -> String {
    let r = (secs * 100.0).round() / 100.0;
    if (r - r.round()).abs() < 1e-9 { format!("{}s", r.round() as i64) } else { format!("{r}s").trim_end_matches('0').to_string() }
}

/// Draws the chord and key strips; returns the key strip's first text (for the tests).
fn strips(painter: &Painter, s: &Session, a: &Areas, v: &RollView, t: &Tokens, has_key: bool) -> String {
    // Chords.
    painter.text(pos2(a.keys.left() + 6.0, a.chords.center().y), Align2::LEFT_CENTER, "Chords", regular(11.0), t.text_dim);
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(v.t0, sr);
    let span_ticks = (s.tempo.samples_to_ticks(v.t1, sr) - base).max(1) as f32;
    let (x0, w) = (v.rect.left(), v.rect.width());
    crate::midi_cleanup_panel::chord_strip_range(&painter.with_clip_rect(a.chords), s, a.chords, v.t0, v.t1, |tk| x0 + tk as f32 / span_ticks * w);
    // Key.
    painter.text(pos2(a.keys.left() + 6.0, a.key.center().y), Align2::LEFT_CENTER, "Key", regular(11.0), t.text_dim);
    let kp = painter.with_clip_rect(a.key);
    kp.rect_filled(a.key, 0.0, if dark(t) { Color32::from_rgb(36, 52, 63) } else { Color32::from_rgb(214, 229, 240) });
    let text_c = if dark(t) { Color32::from_rgb(207, 227, 245) } else { Color32::from_rgb(30, 60, 90) };
    const HINT: &str = "set key & chords in the timeline, not here";
    let mut strip = String::new();
    if has_key {
        let mut regions: Vec<(Samples, String)> = vec![(Samples::MIN, key_name(s.band.key, s.band.scale))];
        let mut changes: Vec<&(Samples, String)> =
            s.key_signatures.iter().filter(|(_, n)| soundcraft_engine::vocal::parse_key(n).is_some()).collect();
        changes.sort_by_key(|(at, _)| *at);
        for (at, n) in changes {
            if let Some((pc, sc)) = soundcraft_engine::vocal::parse_key(n) {
                regions.push((*at, key_name(pc, sc)));
            }
        }
        for (i, (at, name)) in regions.iter().enumerate() {
            let end = regions.get(i + 1).map_or(Samples::MAX, |r| r.0);
            if end <= v.t0 || *at >= v.t1 {
                continue;
            }
            let xa = v.x((*at).max(v.t0));
            if i > 0 {
                kp.line_segment([pos2(xa, a.key.top()), pos2(xa, a.key.bottom())], Stroke::new(1.0, t.border));
            }
            let label = format!("{name}  (session)");
            kp.text(pos2(xa + 6.0, a.key.center().y), Align2::LEFT_CENTER, &label, bold(11.5), text_c);
            if strip.is_empty() {
                strip = label;
            }
        }
        kp.text(pos2(a.key.right() - 6.0, a.key.center().y), Align2::RIGHT_CENTER, HINT, regular(10.5), t.text_dim);
    } else {
        // Nothing to judge notes by: say so, and where a key is set.
        strip = format!("No key set \u{b7} {HINT}");
        kp.text(pos2(a.key.left() + 6.0, a.key.center().y), Align2::LEFT_CENTER, &strip, regular(11.5), text_c);
    }
    // Ruler.
    painter.rect_filled(a.ruler, 0.0, t.panel_bg);
    let rate = f64::from(sr.hz().max(1));
    let step = ruler_step(v, rate);
    let first = ((v.t0 as f64 / rate) / step).ceil() as i64;
    let rp = painter.with_clip_rect(a.ruler);
    for k in first.. {
        let secs = k as f64 * step;
        let at = (secs * rate) as Samples;
        if at > v.t1 || k > first + 400 {
            break;
        }
        let x = v.x(at);
        rp.line_segment([pos2(x, a.ruler.bottom() - 7.0), pos2(x, a.ruler.bottom())], Stroke::new(1.0, t.ruler_tick));
        rp.text(pos2(x + 3.0, a.ruler.center().y - 1.0), Align2::LEFT_CENTER, fmt_secs(secs), regular(10.5), t.text_dim);
    }
    strip
}

/// The time grid lines over the rows, at the ruler's step (and half steps).
fn time_grid(painter: &Painter, v: &RollView, rate: f64, t: &Tokens) {
    let step = ruler_step(v, rate) / 2.0;
    let first = ((v.t0 as f64 / rate) / step).ceil() as i64;
    for k in first..first + 800 {
        let at = (k as f64 * step * rate) as Samples;
        if at > v.t1 {
            break;
        }
        let major = k % 2 == 0;
        let c = if dark(t) {
            if major { Color32::from_rgb(58, 59, 65) } else { Color32::from_rgb(47, 48, 53) }
        } else if major {
            t.bar_line
        } else {
            t.grid_line
        };
        let x = v.x(at);
        painter.line_segment([pos2(x, v.rect.top()), pos2(x, v.rect.bottom())], Stroke::new(1.0, c));
    }
}

fn hatch(painter: &Painter, r: Rect, c: Color32) {
    // Only the visible part: a block wider than the roll draws no strokes off screen.
    let vis = r.intersect(painter.clip_rect());
    if !vis.is_positive() {
        return;
    }
    let p = painter.with_clip_rect(vis);
    let h = r.height();
    let skip = ((vis.left() - h - r.left()) / 5.0).floor().max(-1.0);
    let mut x = r.left() + skip * 5.0;
    while x < vis.right() {
        p.line_segment([pos2(x, r.bottom()), pos2(x + h, r.top())], Stroke::new(1.3, c));
        x += 5.0;
    }
    let (l, rr) = (r.left().max(vis.left() - 4.0), r.right().min(vis.right() + 4.0));
    let pts = [pos2(l, r.top()), pos2(rr, r.top()), pos2(rr, r.bottom()), pos2(l, r.bottom()), pos2(l, r.top())];
    p.extend(Shape::dashed_line(&pts, Stroke::new(1.0, c.gamma_multiply(0.7)), 2.0, 2.0));
}

/// A stadium (the bar's rounded shape) as a closed polyline.
fn stadium(r: Rect) -> Vec<Pos2> {
    let rad = (r.height() / 2.0).min(r.width() / 2.0);
    let mut pts = Vec::new();
    let (lc, rc) = (pos2(r.left() + rad, r.center().y), pos2(r.right() - rad, r.center().y));
    for k in 0..=12 {
        let a = std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 12.0;
        pts.push(lc + vec2(a.cos(), -a.sin()) * rad);
    }
    for k in 0..=12 {
        let a = -std::f32::consts::FRAC_PI_2 + std::f32::consts::PI * k as f32 / 12.0;
        pts.push(rc + vec2(a.cos(), -a.sin()) * rad);
    }
    if let Some(p) = pts.first().copied() {
        pts.push(p);
    }
    pts
}

fn dashed_round(painter: &Painter, r: Rect, c: Color32) {
    painter.extend(Shape::dashed_line(&stadium(r), Stroke::new(1.2, c), 3.0, 3.0));
}

/// The off-key outline: dotted, rounded like the bar.
fn dotted_round(painter: &Painter, r: Rect, c: Color32) {
    painter.extend(Shape::dotted_line(&stadium(r), c, 3.0, 0.75));
}

/// The colour legend at the roll's bottom left (mockup): the offset scale and thin → fat.
fn legend(painter: &Painter, roll: Rect, t: &Tokens) {
    let box_r = Rect::from_min_size(pos2(roll.left() + 6.0, roll.bottom() - 46.0), vec2(236.0, 40.0));
    if roll.width() < 300.0 || roll.height() < 120.0 {
        return;
    }
    painter.rect(box_r, 4.0, t.panel_bg2.gamma_multiply(0.95), Stroke::new(1.0, t.border), StrokeKind::Inside);
    let (lx, ly) = (box_r.left() + 8.0, box_r.top() + 7.0);
    for k in 0..60 {
        let c = k as f32 / 60.0 * 48.0;
        let r = Rect::from_min_size(pos2(lx + k as f32 * 2.0, ly), vec2(2.2, 8.0));
        painter.rect_filled(r, 0.0, off_colour(c));
    }
    let dim = t.text_dim;
    painter.text(pos2(lx, ly + 18.0), Align2::LEFT_CENTER, "on", regular(10.5), dim);
    painter.text(pos2(lx + 62.0, ly + 18.0), Align2::CENTER_CENTER, "25c", regular(10.5), dim);
    painter.text(pos2(lx + 120.0, ly + 18.0), Align2::RIGHT_CENTER, "48c+", regular(10.5), dim);
    let grey = if dark(t) { Color32::from_rgb(154, 170, 170) } else { Color32::from_rgb(120, 132, 132) };
    painter.rect_filled(Rect::from_min_size(pos2(lx + 134.0, ly + 2.0), vec2(22.0, 4.0)), 2.0, grey);
    painter.rect_filled(Rect::from_min_size(pos2(lx + 160.0, ly - 2.0), vec2(22.0, 12.0)), 6.0, grey);
    painter.text(pos2(lx + 134.0, ly + 18.0), Align2::LEFT_CENTER, "quiet \u{2192} loud", regular(10.5), dim);
}

/// An offset label's colour: the bar's, darkened on the light theme so it reads on light rows.
fn label_colour(c: Color32, is_dark: bool) -> Color32 {
    if is_dark {
        c
    } else {
        let k = |v: u8| (f32::from(v) * 0.62) as u8;
        Color32::from_rgb(k(c.r()), k(c.g()), k(c.b()))
    }
}

/// Hover text for a note: "G#4 −41c → −14c · centre 40% · drift 30%", and for a moved note
/// "A#4 −41c → ±0c · moved +241c · …" (the sung offset from its own target, the tuned one from the
/// target it was moved to).
fn tooltip(cv: &ClipView, sh: &NoteShape) -> String {
    let e = cv.edits.get(&sh.r.note).cloned().unwrap_or_default();
    let sung = cv.a.notes.get(sh.index).map_or(sh.offset, |n| n.centre_cents - target_of(cv, n));
    let moved = if e.offset_cents != 0.0 { format!(" \u{b7} moved {}", fmt_cents(e.offset_cents)) } else { String::new() };
    format!(
        "{} {} \u{2192} {}{moved} \u{b7} centre {:.0}% \u{b7} drift {:.0}%",
        note_name(sh.target),
        fmt_cents(sung),
        fmt_cents(sh.tuned_offset),
        e.centre * 100.0,
        e.drift * 100.0
    )
}

/// Everything drawn this frame, for the mouse and keys.
pub struct Drawn {
    pub view: RollView,
    /// The time ruler (a click locates, a drag scrubs).
    pub ruler: Rect,
    pub shapes: Vec<NoteShape>,
    pub unvoiced: Vec<Rect>,
    pub scoops: Vec<(NoteRef, Rect)>,
    pub clips: Vec<ClipView>,
}

/// Draws the strips, ruler, keys and roll for `track` in `areas`.
pub fn draw(app: &mut SoundApp, ui: &mut Ui, track: TrackId, a: &Areas, v: RollView) -> Drawn {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    let painter = ui.painter().clone();
    let clips = clip_views(app, track, Some((v.t0, v.t1)));
    let typical_db = typical(app, track);
    let key_scale = soundcraft_engine::vocal::scale_at(&s, &Clip::audio(ClipId(0), "", soundcraft_model::SourceId(0), 0, v.t0, 1));
    // Without a key no rows are shaded as the scale and no note is off key.
    let has_key = has_key(&s);
    let row_scale = key_scale.scale_at(0).filter(|_| has_key);
    let rp = painter.with_clip_rect(a.roll.union(a.keys));
    let fills = grid(&rp, &s, a, &v, row_scale, &t);
    hook(ui.ctx(), "row-fills", fills);
    let strip = strips(&painter, &s, a, &v, &t, has_key);
    hook(ui.ctx(), "key-strip", strip);
    let rate = f64::from(s.sample_rate.hz().max(1));
    let roll_p = painter.with_clip_rect(a.roll);
    time_grid(&roll_p, &v, rate, &t);
    // Clips not analysed yet show their audio until their notes land.
    super::draw_waves(app, ui, track, a.roll, (v.t0, v.t1), true);
    let is_dark = dark(&t);
    // Unvoiced blocks at the roll's bottom third.
    let mut unvoiced = Vec::new();
    let uy = a.roll.top() + a.roll.height() * 2.0 / 3.0;
    let hatch_c = if is_dark { Color32::from_rgba_unmultiplied(170, 170, 187, 115) } else { Color32::from_rgba_unmultiplied(90, 90, 110, 140) };
    for cv in clips.iter().filter(|c| !c.stretched) {
        let fm = Frames::of(&cv.clip, cv.a.track.hop_samples);
        for u in &cv.a.unvoiced {
            let (t0, t1) = (fm.clamp(fm.time(u.start)), fm.clamp(fm.time(u.end)));
            if t1 <= t0 || t1 <= v.t0 || t0 >= v.t1 {
                continue;
            }
            let x0 = v.x(t0);
            let r = Rect::from_min_max(pos2(x0, uy - 9.0), pos2(v.x(t1).max(x0 + 7.0), uy + 9.0));
            hatch(&roll_p, r, hatch_c);
            if (t1 - t0) as f64 / rate >= BREATH_SECS {
                roll_p.text(pos2(r.center().x, r.bottom() + 9.0), Align2::CENTER_CENTER, "breath", regular(10.5), t.text_dim);
                store(ui.ctx(), "unvoiced-label", "breath".to_string());
            }
            unvoiced.push(r);
        }
    }
    // Notes.
    let mut layers: Vec<&'static str> = Vec::new();
    let mut shapes = Vec::new();
    let mut scoops = Vec::new();
    let selected = app.vocal.selected.clone();
    let chords = ChordTones::of(&s);
    let sel_c = if is_dark { Color32::WHITE } else { Color32::from_rgb(25, 25, 28) };
    // Offset labels placed so far (a label that would overlap one drops " · off key", then hides).
    let mut labels: Vec<Rect> = Vec::new();
    // The notes outlined as off key (for the tests).
    let mut off_key_refs: Vec<NoteRef> = Vec::new();
    for cv in &clips {
        if cv.stretched {
            let (x0, x1) = (v.x(cv.clip.start.max(v.t0)), v.x(cv.clip.end().min(v.t1)));
            roll_p.rect_filled(Rect::from_min_max(pos2(x0, a.roll.top()), pos2(x1, a.roll.bottom())), 0.0, Color32::from_black_alpha(60));
            roll_p.text(
                pos2((x0 + x1) / 2.0, a.roll.center().y),
                Align2::CENTER_CENTER,
                "Tuning can't follow a stretched clip",
                regular(12.0),
                t.text_dim,
            );
            store(ui.ctx(), "stretched", "Tuning can't follow a stretched clip".to_string());
            continue;
        }
        let list = note_shapes(&v, &cv.clip, &cv.a, cv.curves.as_deref(), typical_db, |n| moved_target_of(cv, n), |n| tuned_centre_of(cv, n));
        let fm = Frames::of(&cv.clip, cv.a.track.hop_samples);
        for mut sh in list {
            let note = cv.a.notes.get(sh.index);
            // A note being dragged sits where the pointer has it, ahead of its curves; its label
            // and colour read against the semitone the pointer has it on.
            if let Some(dc) = app.vocal.drag.as_ref().and_then(|g| g.preview_cents(sh.r)) {
                let dy = -dc / 100.0 * v.row_h;
                sh.rect = sh.rect.translate(vec2(0.0, dy));
                if let Some([_, p1]) = sh.scoop.as_mut() {
                    p1.y += dy;
                }
                if let Some(n) = note {
                    let sent = cv.edits.get(&n.id).map_or(0.0, |e| e.offset_cents);
                    let target = moved_target(target_of(cv, n), sent + dc);
                    sh.tuned_offset += dc + sh.target - target;
                    sh.target = target;
                    sh.colour = off_colour(sh.tuned_offset);
                }
            }
            let sel = selected.contains(&sh.r);
            let off = has_key
                && note.is_some_and(|n| {
                    let mid = n.start.saturating_add(n.end.saturating_sub(n.start) / 2);
                    off_key(cv, n, moved_target_of(cv, n), chords.at(&s, fm.time(mid)))
                });
            if let Some(g) = sh.ghost {
                // The sung note, coloured by how far it was off its own target.
                let sung = note.map_or(sh.offset, |n| n.centre_cents - target_of(cv, n));
                dashed_round(&roll_p, g, off_colour(sung).gamma_multiply(0.6));
            }
            let rad = (sh.rect.height() / 2.0).min(sh.rect.width() / 2.0);
            let fill = off_colour(sh.tuned_offset).gamma_multiply(if sel { 0.95 } else { 0.85 });
            let stroke = if sel { Stroke::new(2.0, sel_c) } else { Stroke::new(1.0, Color32::from_black_alpha(90)) };
            roll_p.rect(sh.rect, CornerRadius::same(rad.clamp(0.0, 255.0) as u8), fill, stroke, StrokeKind::Outside);
            if off {
                off_key_refs.push(sh.r);
                dotted_round(&roll_p, sh.rect.expand(2.5), off_colour(sh.tuned_offset));
            }
            if sh.rect.width() >= 28.0 {
                // Above the bar (and its off-key outline), never over another label.
                let at = pos2(sh.rect.left() + 2.0, sh.rect.top() - if off { 5.5 } else { 3.0 });
                let colour = label_colour(off_colour(sh.tuned_offset), is_dark);
                let short = fmt_cents(sh.tuned_offset);
                let texts = if off { vec![format!("{short} \u{b7} off key"), short] } else { vec![short] };
                for text in texts {
                    let g = roll_p.layout_no_wrap(text, bold(11.0), colour);
                    let r = Rect::from_min_size(pos2(at.x, at.y - g.size().y), g.size());
                    if labels.iter().any(|l| l.intersects(r)) {
                        continue;
                    }
                    roll_p.galley(r.min, g, colour);
                    labels.push(r);
                    break;
                }
            }
            if let Some([p0, p1]) = sh.scoop {
                roll_p.line_segment([p0, p1], Stroke::new(2.5, SCOOP_BLUE));
                let hp = pos2(p1.x, sh.rect.bottom() + 9.0);
                roll_p.line_segment([pos2(p1.x, sh.rect.bottom()), hp], Stroke::new(1.0, SCOOP_BLUE.gamma_multiply(0.7)));
                let d = 7.0;
                roll_p.add(Shape::convex_polygon(
                    vec![hp + vec2(0.0, -d), hp + vec2(d, 0.0), hp + vec2(0.0, d), hp + vec2(-d, 0.0)],
                    SCOOP_BLUE,
                    Stroke::new(1.0, Color32::from_rgb(11, 29, 51)),
                ));
                scoops.push((sh.r, Rect::from_center_size(hp, vec2(2.0 * d + 4.0, 2.0 * d + 4.0))));
            }
            shapes.push(sh);
        }
    }
    // Curves, over the bars (as editor-detail.png): grey where untuned; where tuned, the original
    // as a faint dashed ghost and the tuned curve in white. `layers` records the order for tests.
    hook(ui.ctx(), "cent-labels", labels.clone());
    layers.push("notes");
    let grey = if is_dark { Color32::from_rgba_unmultiplied(242, 242, 242, 140) } else { Color32::from_rgba_unmultiplied(60, 60, 60, 150) };
    let tuned_c = if is_dark { Color32::WHITE } else { Color32::from_rgb(20, 20, 20) };
    for cv in clips.iter().filter(|c| !c.stretched) {
        let shift = cv.curves.as_ref().map(|c| c.shift_cents.as_slice());
        let moved = |f: u32| shift.and_then(|s| s.get(f as usize)).is_some_and(|x| x.abs() > 0.05);
        for line in curve_points_where(&v, &cv.clip, &cv.a.track, None, |f| !moved(f)) {
            roll_p.add(Shape::line(line, Stroke::new(1.5, grey)));
        }
        if shift.is_some() {
            for line in curve_points_where(&v, &cv.clip, &cv.a.track, None, moved) {
                roll_p.extend(Shape::dashed_line(&line, Stroke::new(1.2, grey.gamma_multiply(0.7)), 3.0, 2.0));
            }
            for line in curve_points_where(&v, &cv.clip, &cv.a.track, shift, moved) {
                roll_p.add(Shape::line(line, Stroke::new(2.0, tuned_c)));
            }
        }
    }
    layers.push("curves");
    hook(ui.ctx(), "off-key", off_key_refs);
    store(ui.ctx(), "layers", layers);
    legend(&roll_p, a.roll, &t);
    // Playhead.
    let ph = app.position();
    if ph >= v.t0 && ph <= v.t1 {
        let x = v.x(ph);
        painter.line_segment([pos2(x, a.ruler.top()), pos2(x, a.roll.bottom())], Stroke::new(1.5, t.playhead));
        painter.add(Shape::convex_polygon(
            vec![pos2(x - 6.0, a.ruler.top()), pos2(x + 6.0, a.ruler.top()), pos2(x, a.ruler.top() + 7.0)],
            t.playhead,
            Stroke::NONE,
        ));
    }
    store(ui.ctx(), "shapes", shapes.clone());
    store(ui.ctx(), "unvoiced", unvoiced.clone());
    store(ui.ctx(), "scoops", scoops.clone());
    store(ui.ctx(), "view", v);
    store(ui.ctx(), "roll", a.roll);
    store(ui.ctx(), "ruler", a.ruler);
    Drawn { view: v, ruler: a.ruler, shapes, unvoiced, scoops, clips }
}

/// A selection change by a click on `r` with the held modifiers (the roll and the Level lane).
pub(crate) fn click_select(app: &mut SoundApp, track: TrackId, r: Option<NoteRef>, toggle: bool) {
    app.vocal.whole_track = false;
    match r {
        Some(r) => {
            let both = with_twins(app, track, &BTreeSet::from([r]));
            let st = &mut app.vocal;
            if toggle {
                if st.selected.contains(&r) {
                    for x in &both {
                        st.selected.remove(x);
                    }
                } else {
                    st.selected.extend(both);
                }
            } else {
                st.selected = both;
            }
        }
        None if !toggle => app.vocal.selected.clear(),
        None => {}
    }
}

/// Puts the playhead at `at` (the ruler, a blank click): an insertion point there, then a
/// locate. `transport.locate` alone keeps an Edit selection that contains `at`, and a stopped
/// playhead is that selection's start, so a click inside one would not move it.
fn locate(app: &mut SoundApp, at: Samples) {
    let at = at.max(0);
    let _ = app.run("edit.select", json!({"start": at}));
    let _ = app.run("transport.locate", json!({"at": at}));
}

/// The mouse over the roll: selection, rubber band (a drag on blank space), a blank click
/// locating, tooltip, scoop handles and the popup; and over the ruler: a click puts the
/// playhead there, a drag scrubs it (as the Edit window's and
/// the MIDI Editor's rulers).
pub fn interact(app: &mut SoundApp, ui: &mut Ui, track: TrackId, d: &Drawn) {
    let ruler = ui.interact(d.ruler, ui.id().with(("vocal-ruler", track.0)), Sense::click_and_drag());
    if (ruler.clicked() || ruler.dragged())
        && let Some(p) = ruler.interact_pointer_pos()
    {
        locate(app, d.view.at(p.x));
    }
    let roll = d.view.rect;
    let resp = ui.interact(roll, ui.id().with(("vocal-roll", track.0)), Sense::click_and_drag());
    let hit = |p: Pos2| d.shapes.iter().rev().find(|sh| sh.rect.expand(2.0).contains(p)).map(|sh| sh.r);
    let toggle = ui.input(|i| i.modifiers.shift || i.modifiers.command || i.modifiers.ctrl);
    if resp.drag_started()
        && let Some(p) = resp.interact_pointer_pos()
    {
        app.vocal.popup = None;
        // A drag that began on a note moves pitch; anywhere else it is a rubber band.
        let origin = ui.input(|i| i.pointer.press_origin()).unwrap_or(p);
        let on_block = d.unvoiced.iter().any(|r| r.contains(origin));
        match hit(origin).filter(|_| !on_block) {
            Some(r) => super::drag::start(app, track, r, origin, toggle),
            None => app.vocal.band = Some(p),
        }
    }
    if app.vocal.drag.as_ref().is_some_and(|g| g.track == track) {
        super::drag::update(app, ui, d, resp.drag_stopped());
    } else {
        app.vocal.drag = None;
    }
    if let (Some(a), Some(b)) = (app.vocal.band, ui.ctx().pointer_latest_pos()) {
        let band = Rect::from_two_pos(a, b);
        ui.painter().with_clip_rect(roll).rect(
            band,
            0.0,
            Color32::from_rgba_unmultiplied(120, 170, 255, 40),
            Stroke::new(1.0, Color32::from_rgb(120, 170, 255)),
            StrokeKind::Inside,
        );
        if resp.drag_stopped() || !ui.input(|i| i.pointer.primary_down()) {
            let picked: BTreeSet<NoteRef> = d.shapes.iter().filter(|sh| sh.rect.intersects(band)).map(|sh| sh.r).collect();
            let picked = with_twins(app, track, &picked);
            app.vocal.whole_track = false;
            if toggle {
                app.vocal.selected.extend(picked);
            } else {
                app.vocal.selected = picked;
            }
            app.vocal.band = None;
        }
    }
    if resp.clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        app.vocal.popup = None;
        let on_block = d.unvoiced.iter().any(|r| r.contains(p));
        let r = if on_block { None } else { hit(p) };
        click_select(app, track, r, toggle && !on_block);
        match r {
            Some(r) => super::preview::request(app, r),
            // Blank space (as in Melodyne): the playhead moves there, the selection is cleared.
            None if !toggle => locate(app, d.view.at(p.x)),
            None => {}
        }
    }
    // Hover tooltip.
    if let Some(p) = resp.hover_pos()
        && app.vocal.band.is_none()
        && app.vocal.drag.is_none()
        && let Some(r) = hit(p)
        && let (Some(sh), Some(cv)) = (d.shapes.iter().find(|sh| sh.r == r), d.clips.iter().find(|c| c.clip.id == r.clip))
    {
        let tip = tooltip(cv, sh);
        store(ui.ctx(), "tooltip", tip.clone());
        resp.clone().on_hover_text_at_pointer(tip);
    }
    // Scoop handles (above the roll's own response).
    for (r, rect) in &d.scoops {
        let h = ui.interact(*rect, ui.id().with(("vocal-scoop", r.clip.0, r.note.0)), Sense::click());
        if h.clicked() {
            app.vocal.popup = if app.vocal.popup == Some(*r) { None } else { Some(*r) };
        }
        h.on_hover_text("Transition into this note: Keep, Smooth or Shorten");
    }
    popup(app, ui, d);
}

/// The transition popup of `app.vocal.popup`, under its scoop handle.
fn popup(app: &mut SoundApp, ui: &mut Ui, d: &Drawn) {
    let Some(r) = app.vocal.popup else { return };
    let (Some((_, handle)), Some(cv)) = (d.scoops.iter().find(|(x, _)| *x == r), d.clips.iter().find(|c| c.clip.id == r.clip)) else {
        app.vocal.popup = None;
        return;
    };
    let Some(n) = cv.a.notes.iter().find(|n| n.id == r.note) else { return };
    let t = Tokens::current();
    let current = cv.edits.get(&r.note).map_or(Transition::Keep, |e| e.transition);
    let title = format!("Transition into {}", note_name(target_of(cv, n)));
    let w = 250.0;
    let min = pos2((handle.right() + 8.0).min(d.view.rect.right() - w - 4.0), handle.center().y - 14.0);
    let r_box = Rect::from_min_size(min, vec2(w, 54.0));
    let p = ui.painter().clone();
    p.rect(r_box, 4.0, t.panel_bg2, Stroke::new(1.0, Color32::from_rgb(58, 92, 130)), StrokeKind::Inside);
    p.text(pos2(r_box.left() + 10.0, r_box.top() + 13.0), Align2::LEFT_CENTER, &title, regular(12.0), t.text);
    store(ui.ctx(), "popup-title", title);
    let bw = (w - 20.0) / 3.0;
    for (i, (name, label, tr)) in
        [("keep", "Keep", Transition::Keep), ("smooth", "Smooth", Transition::Smooth), ("shorten", "Shorten", Transition::Shorten)]
            .into_iter()
            .enumerate()
    {
        let b = Rect::from_min_size(pos2(r_box.left() + 8.0 + i as f32 * (bw + 2.0), r_box.top() + 26.0), vec2(bw, 22.0));
        let resp = ui.interact(b, ui.id().with(("vocal-transition", name)), Sense::click());
        let on = tr == current;
        let fill = if on {
            Color32::from_rgb(43, 90, 138)
        } else if resp.hovered() {
            t.button_hi
        } else {
            t.button
        };
        p.rect(b, 3.0, fill, Stroke::new(1.0, t.button_border), StrokeKind::Inside);
        p.text(b.center(), Align2::CENTER_CENTER, label, regular(11.5), if on { Color32::WHITE } else { t.text });
        store(ui.ctx(), name, b);
        if resp.clicked() {
            let res = app.run(crate::vocal_ids::VOCAL_EDIT, json!({"notes": [r], "transition": name}));
            match res {
                Ok(_) => super::preview::request(app, r),
                Err(e) => app.ui.status = e,
            }
            app.vocal.popup = None;
        }
    }
}

/// Drops selected notes that no longer exist (after Undo, a re-analysis or a split), or that
/// belong to another track.
pub fn prune(app: &mut SoundApp, track: TrackId) {
    if app.vocal.selected.is_empty() {
        return;
    }
    let s = checked(app);
    let st = &mut app.vocal;
    let sel = std::mem::take(&mut st.selected);
    for r in sel {
        let ok = s
            .find_clip(r.clip)
            .filter(|(t, _)| *t == track)
            .and_then(|(_, c)| st.cache.get(&s, c))
            .is_some_and(|(a, _)| a.notes.iter().any(|n| n.id == r.note));
        if ok {
            st.selected.insert(r);
        }
    }
    if st.popup.is_some_and(|p| !st.selected.contains(&p) && s.find_clip(p.clip).is_none()) {
        st.popup = None;
    }
}

//! Vocal Studio (spec 2026-11-03-vocal-studio-design.md): every vocal track's notes in one pitch
//! view. What the studio reads from the session: the tracks it lists, each track's notes as they
//! sound after tuning (session time, tuned pitch, the target they are judged by) and a status per
//! track in the Mix pitch box's colours.

use crate::vocal::{FrameMap, NoteRef, SessionScale, analysis_for, cannot_analyse, has_key, key_pc_at, key_set, note_mode, scale_at, stretched};
use soundcraft_model::{AnalysisKey, Session, TrackId, TrackKind};
use soundcraft_time::Samples;
use soundcraft_vocal::harmony::{ClashKind, HarmonyRule, chord_clash, fit_note, harmony_note, interval_clash, out_of_key};
use soundcraft_vocal::{Note, NoteEdit, NoteMode, PitchClassSet, VocalAnalysis, tuned_centre_with, tuned_target_with};
use std::collections::BTreeMap;

/// In tune: the notes are on average within this many cents (rounded) of their targets.
pub const IN_TUNE_CENTS: f32 = 10.0;
/// Drifting: up to this many cents; beyond it the track is off.
pub const DRIFT_CENTS: f32 = 17.0;

/// The tracks the studio lists, top to bottom: every audio track marked vocal (or named like
/// one) or holding a live tuning.
pub fn studio_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| t.kind == TrackKind::Audio && (t.is_vocal() || t.clips().iter().any(|c| c.tune_live().is_some())))
        .map(|t| t.id)
        .collect()
}

/// One note as the studio shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StudioNote {
    pub r: NoteRef,
    /// Session time of its first frame and of the frame after its last.
    pub start: Samples,
    pub end: Samples,
    /// Its centre after tuning (cents, MIDI note × 100).
    pub pitch: f32,
    /// The pitch it is judged by: its target, moved by its offset's whole semitones.
    pub target: f32,
}

impl StudioNote {
    /// How far it sounds from its target (cents).
    pub fn offset(&self) -> f32 {
        self.pitch - self.target
    }

    /// The semitone it sounds on (MIDI note).
    pub fn midi(&self) -> i32 {
        let m = (self.target / 100.0).round();
        if m.is_finite() { m.clamp(-1000.0, 1000.0) as i32 } else { 0 }
    }

    /// Whether it sounds at any time in `[a, b)`.
    pub fn overlaps(&self, a: Samples, b: Samples) -> bool {
        self.start < b && a < self.end
    }
}

/// Every note of `track`'s analysed, unstretched audio clips inside their windows, in time order.
pub fn studio_notes(s: &Session, track: TrackId) -> Vec<StudioNote> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let none = BTreeMap::new();
    let mut out = Vec::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && !stretched(c)) {
        let Some(a) = analysis_for(s, c) else { continue };
        let m = FrameMap::of(c, &a);
        let scale = scale_at(s, c);
        let mode = note_mode(c);
        let edits = c.tune_live().map_or(&none, |t| &t.edits);
        for n in a.notes.iter().filter(|n| m.in_window(n.start, n.end)) {
            let e = edits.get(&n.id).cloned().unwrap_or_default().clamped();
            let pitch = tuned_centre_with(&a, n, &e, &scale, mode);
            let target = tuned_target_with(&a, n, &e, &scale, mode);
            if !pitch.is_finite() || !target.is_finite() {
                continue;
            }
            out.push(StudioNote { r: NoteRef { clip: c.id, note: n.id }, start: m.time(n.start), end: m.time(n.end), pitch, target });
        }
    }
    out.sort_by_key(|n| (n.start, n.r));
    out
}

/// What the studio's status chip says about a track.
#[derive(Debug, Clone, PartialEq)]
pub struct TrackStatus {
    pub track: TrackId,
    pub notes: usize,
    /// Some audio clip of the track still waits for its analysis.
    pub analysing: bool,
    /// Every audio clip of the track can't be analysed (missing audio, too long).
    pub cannot_analyse: bool,
    /// The notes' mean distance from their targets (cents, unsigned); `None` without notes.
    pub avg_offset: Option<f32>,
}

impl TrackStatus {
    /// "in tune" (≤ 10 c), "drifting" (11–17 c), "off" (> 17 c), "analysing", "can't analyse" or
    /// "no notes".
    pub fn level(&self) -> &'static str {
        match self.avg_offset.filter(|c| c.is_finite()).map(|c| c.round()) {
            _ if self.analysing && self.notes == 0 => "analysing",
            _ if self.cannot_analyse && self.notes == 0 => "can't analyse",
            None => "no notes",
            Some(c) if c <= IN_TUNE_CENTS => "in tune",
            Some(c) if c <= DRIFT_CENTS => "drifting",
            Some(_) => "off",
        }
    }
}

/// `track`'s status (see [`TrackStatus`]).
pub fn track_status(s: &Session, track: TrackId) -> TrackStatus {
    let notes = studio_notes(s, track);
    let audio: Vec<_> = s.track(track).map(|t| t.clips().iter().filter(|c| c.is_audio()).collect()).unwrap_or_default();
    let unanalysed: Vec<_> =
        audio.iter().filter(|c| AnalysisKey::of(s, c).is_none_or(|k| !s.vocal.analyses.contains_key(&k.hash()))).copied().collect();
    let cannot = !unanalysed.is_empty() && unanalysed.iter().all(|c| cannot_analyse(s, c));
    let analysing = unanalysed.iter().any(|c| !cannot_analyse(s, c));
    let avg_offset = (!notes.is_empty()).then(|| notes.iter().map(|n| n.offset().abs()).sum::<f32>() / notes.len() as f32);
    TrackStatus { track, notes: notes.len(), analysing, cannot_analyse: cannot, avg_offset }
}

/// The lead the studio compares the other tracks with, when `editing` is edited: a listed track
/// other than it whose name says "lead", else the first listed track other than it.
pub fn lead_track(s: &Session, editing: Option<TrackId>) -> Option<TrackId> {
    let tracks: Vec<TrackId> = studio_tracks(s).into_iter().filter(|t| Some(*t) != editing).collect();
    let named = |t: &TrackId| s.track(*t).is_some_and(|tr| tr.name.to_lowercase().split(|c: char| !c.is_alphanumeric()).any(|w| w == "lead"));
    tracks.iter().find(|t| named(t)).or(tracks.first()).copied()
}

/// "G#4" for MIDI note `m`.
pub fn midi_name(m: i32) -> String {
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    format!("{}{}", NAMES.get(m.rem_euclid(12) as usize).copied().unwrap_or("C"), m.div_euclid(12) - 1)
}

/// The band chords' tones by tick span, sorted: built once, looked up per note.
pub struct Chords {
    spans: Vec<(i64, i64, [bool; 12])>,
}

impl Chords {
    pub fn of(s: &Session) -> Chords {
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
        Chords { spans }
    }

    /// The tones of the chord sounding at session time `at`.
    pub fn at(&self, s: &Session, at: Samples) -> Option<[bool; 12]> {
        let tick = s.tempo.samples_to_ticks(at, s.sample_rate);
        let k = self.spans.partition_point(|x| x.0 <= tick);
        k.checked_sub(1).and_then(|i| self.spans.get(i)).filter(|x| tick < x.1).map(|x| x.2)
    }
}

/// The key's scale at session time `at`, when the session sets a key.
pub fn scale_at_time(s: &Session, at: Samples) -> Option<PitchClassSet> {
    has_key(s).then(|| {
        let (k, sc) = key_pc_at(s, at);
        key_set(k, sc)
    })
}

/// Two notes sound together when they overlap by at least this much (seconds), or half the
/// shorter one: a lead note just starting as a BV note ends is not a clash.
const MIN_OVERLAP_S: f64 = 0.06;

/// The notes of `others` sounding with `n` (enough overlap, see [`MIN_OVERLAP_S`]).
pub fn sounding_with<'a>(s: &Session, n: &StudioNote, others: &'a [StudioNote]) -> Vec<&'a StudioNote> {
    others.iter().filter(|o| sound_together(s, n, o)).collect()
}

/// Whether `a` and `b` sound together: they overlap by at least [`MIN_OVERLAP_S`] or half the
/// shorter one.
pub fn sound_together(s: &Session, a: &StudioNote, b: &StudioNote) -> bool {
    let min_s = (MIN_OVERLAP_S * f64::from(s.sample_rate.hz())) as Samples;
    let ov = a.end.min(b.end).saturating_sub(a.start.max(b.start));
    let shorter = (a.end - a.start).min(b.end - b.start).max(1);
    ov > 0 && (ov >= min_s || ov.saturating_mul(2) >= shorter)
}

/// A clash marker: a note of the editing track that clashes with the compared track, the chord
/// or the key.
#[derive(Debug, Clone, PartialEq)]
pub struct Clash {
    pub r: NoteRef,
    pub kind: ClashKind,
    /// The compared track whose note it clashes with (`None`: the chord or the key).
    pub with: Option<TrackId>,
    /// What the hover says: "minor 2nd against Lead Vox (G#4)", "minor 2nd against the chord's
    /// G#", "out of key (E major)".
    pub text: String,
}

/// The middle of a note (session time).
fn middle(n: &StudioNote) -> Samples {
    n.start.saturating_add(n.end.saturating_sub(n.start) / 2)
}

/// The clashes of `track`'s notes: against `against`'s notes sounding with them (a minor 2nd,
/// major 7th or tritone), else against the chord at their middle (the same intervals against a
/// chord tone, the note not being one), else out of the key (not in the key's scale nor the
/// chord). `against` equal to `track` is ignored.
pub fn clashes(s: &Session, track: TrackId, against: Option<TrackId>) -> Vec<Clash> {
    let notes = studio_notes(s, track);
    let others = against.filter(|a| *a != track).map(|a| studio_notes(s, a)).unwrap_or_default();
    let other_name = against.and_then(|a| s.track(a)).map_or(String::new(), |t| t.name.clone());
    let chords = Chords::of(s);
    let mut out = Vec::new();
    for n in &notes {
        let m = n.midi();
        let hit = sounding_with(s, n, &others).into_iter().find_map(|o| interval_clash(m, o.midi()).map(|k| (k, o.midi())));
        if let Some((kind, om)) = hit {
            out.push(Clash { r: n.r, kind, with: against, text: format!("{} against {other_name} ({})", kind.name(), midi_name(om)) });
            continue;
        }
        let at = middle(n);
        let chord = chords.at(s, at);
        if let Some((kind, pc)) = chord.as_ref().and_then(|c| chord_clash(m, c)) {
            let tone = midi_name(i32::from(pc) + 60);
            let tone = tone.trim_end_matches(char::is_numeric).to_string();
            out.push(Clash { r: n.r, kind, with: None, text: format!("{} against the chord's {tone}", kind.name()) });
            continue;
        }
        if let Some(sc) = scale_at_time(s, at)
            && out_of_key(m, sc, chord.as_ref())
        {
            let (k, mode) = key_pc_at(s, at);
            let key = format!(
                "{} {}",
                midi_name(i32::from(k)).trim_end_matches(char::is_numeric).trim_end_matches('-'),
                if mode == 1 { "minor" } else { "major" }
            );
            out.push(Clash { r: n.r, kind: ClashKind::OutOfKey, with: None, text: format!("out of key ({key})") });
        }
    }
    out
}

/// The track the other listed tracks are compared with: `chosen` when it is listed, else the
/// [`lead_track`] of the session.
pub fn compared_track(s: &Session, chosen: Option<TrackId>) -> Option<TrackId> {
    let listed = studio_tracks(s);
    chosen.filter(|t| listed.contains(t)).or_else(|| lead_track(s, None))
}

/// Fit to lead: for each of `track`'s notes (only `only` when given), the semitones to move it
/// so it clashes with nothing ([`fit_note`] within a fourth: the nearest tone, chord tones
/// first, that clashes with no note of `against` sounding with it nor with the chord, and is in
/// the key). Notes that already fit, or have nowhere to go, are left out.
pub fn fit_plan(s: &Session, track: TrackId, against: Option<TrackId>, only: Option<&[NoteRef]>) -> Vec<(NoteRef, i32)> {
    let notes = studio_notes(s, track);
    let others = against.filter(|a| *a != track).map(|a| studio_notes(s, a)).unwrap_or_default();
    let chords = Chords::of(s);
    let mut out = Vec::new();
    for n in notes.iter().filter(|n| only.is_none_or(|o| o.contains(&n.r))) {
        let m = n.midi();
        let with: Vec<i32> = sounding_with(s, n, &others).iter().map(|o| o.midi()).collect();
        let at = middle(n);
        let chord = chords.at(s, at);
        if let Some(to) = fit_note(m, &with, chord.as_ref(), scale_at_time(s, at), 5)
            && to != m
        {
            out.push((n.r, to - m));
        }
    }
    out
}

/// One note of a harmony to make: the source note, where it sounds and the harmony's pitch.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct HarmonyNote {
    pub r: NoteRef,
    pub start: Samples,
    pub end: Samples,
    /// The source note's semitone and the harmony's (MIDI).
    pub from: i32,
    pub to: i32,
    /// The source note's tuned pitch (cents): the harmony sounds at it moved by `to − from`
    /// semitones, keeping the lead's own inflection.
    pub pitch: f32,
}

/// The harmony of `source` under `rule`: each note's harmony note ([`harmony_note`]) with the
/// key's scale and the band chord at the note's middle.
pub fn harmony_plan(s: &Session, source: TrackId, rule: &HarmonyRule) -> Vec<HarmonyNote> {
    let chords = Chords::of(s);
    studio_notes(s, source)
        .into_iter()
        .map(|n| {
            let at = middle(&n);
            let from = n.midi();
            let to = harmony_note(from, rule, scale_at_time(s, at), chords.at(s, at).as_ref());
            HarmonyNote { r: n.r, start: n.start, end: n.end, from, to, pitch: n.pitch }
        })
        .collect()
}

/// The name a new harmony track gets: "Harmony 3rd ↑", "Harmony octave ↓".
pub fn harmony_name(rule: &HarmonyRule) -> String {
    format!("Harmony {} {}", rule.interval.name(), if rule.up { "\u{2191}" } else { "\u{2193}" })
}

// ---- Edit All, Tune all, unison stacks (spec 2026-11-04-vocal-studio-edit-all-design.md) ----

/// One note in reach of Tune all and Match: its analysis, edit and scale, as the studio reads it.
struct NoteCtx<'a> {
    track: TrackId,
    r: NoteRef,
    a: &'a VocalAnalysis,
    n: &'a Note,
    e: NoteEdit,
    scale: &'a SessionScale,
    mode: NoteMode,
}

impl NoteCtx<'_> {
    /// The note's held pitch and target under edit `e` (cents).
    fn pitch_with(&self, e: &NoteEdit) -> (f32, f32) {
        (tuned_centre_with(self.a, self.n, e, self.scale, self.mode), tuned_target_with(self.a, self.n, e, self.scale, self.mode))
    }

    /// How far it sounds from its target now (cents).
    fn offset(&self) -> f32 {
        let (p, t) = self.pitch_with(&self.e);
        p - t
    }

    /// The edit that puts the note `goal` cents from its target: its Centre amount when that
    /// reaches (the note's own movement kept), else Centre at 100 % and the rest by the manual
    /// offset's part under a semitone. `None` when the note can't be moved there (too few voiced
    /// frames to tune).
    fn fixed(&self, goal: f32) -> Option<NoteEdit> {
        let at = |c: f32, extra: f32| {
            let mut e = self.e.clone();
            e.centre = c;
            e.offset_cents += extra;
            let e = e.clamped();
            let (p, t) = self.pitch_with(&e);
            (e, p - t)
        };
        let (_, a0) = at(0.0, 0.0);
        let (_, a1) = at(1.0, 0.0);
        let slope = a1 - a0;
        let mut cand = None;
        if slope.abs() > 0.5 && slope.is_finite() {
            let c = (goal - a0) / slope;
            if (0.0..=1.0).contains(&c) {
                cand = Some(at(c, 0.0));
            }
        }
        let (e, off) = cand.unwrap_or_else(|| at(1.0, goal - a1));
        ((off - goal).abs() <= 1.0 && off.is_finite()).then_some(e)
    }
}

/// Calls `f` with every note of `track`'s analysed, unstretched audio clips inside their windows.
fn each_note(s: &Session, track: TrackId, mut f: impl FnMut(&NoteCtx)) {
    let Some(t) = s.track(track) else { return };
    let none = BTreeMap::new();
    for c in t.clips().iter().filter(|c| c.is_audio() && !stretched(c)) {
        let Some(a) = analysis_for(s, c) else { continue };
        let m = FrameMap::of(c, &a);
        let scale = scale_at(s, c);
        let mode = note_mode(c);
        let edits = c.tune_live().map_or(&none, |t| &t.edits);
        for n in a.notes.iter().filter(|n| m.in_window(n.start, n.end)) {
            let e = edits.get(&n.id).cloned().unwrap_or_default().clamped();
            f(&NoteCtx { track, r: NoteRef { clip: c.id, note: n.id }, a: &a, n, e, scale: &scale, mode });
        }
    }
}

/// One note Tune all (or Match) corrects: the edit it gets and its offset from its target
/// before and after (cents).
#[derive(Debug, Clone, PartialEq)]
pub struct TuneStep {
    pub track: TrackId,
    pub r: NoteRef,
    pub edit: NoteEdit,
    pub before: f32,
    pub after: f32,
}

/// Whether a note `off` cents from its target is inside `tolerance` (to a twentieth of a cent:
/// a note Tune all left at the edge stays alone the next time).
pub fn within(off: f32, tolerance: f32) -> bool {
    off.is_finite() && off.abs() <= tolerance + 0.05
}

/// Tune all's plan for `tracks`: every note further than `tolerance` cents from its target gets
/// the edit that puts it on the target (`edge`: at ±tolerance, on its own side); the notes
/// inside it are left alone. Returns the steps and how many notes were left alone.
pub fn tune_all_plan(s: &Session, tracks: &[TrackId], tolerance: f32, edge: bool) -> (Vec<TuneStep>, usize) {
    let mut steps = Vec::new();
    let mut alone = 0;
    for t in tracks {
        each_note(s, *t, |c| {
            let off = c.offset();
            if !off.is_finite() || within(off, tolerance) {
                alone += 1;
                return;
            }
            let goal = if edge { tolerance.copysign(off) } else { 0.0 };
            match c.fixed(goal) {
                Some(edit) => {
                    let (p, tg) = c.pitch_with(&edit);
                    steps.push(TuneStep { track: c.track, r: c.r, edit, before: off, after: p - tg });
                }
                None => alone += 1,
            }
        });
    }
    (steps, alone)
}

/// A note of a unison stack: its track and the note as the studio shows it.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct StackNote {
    pub track: TrackId,
    pub note: StudioNote,
}

/// Notes of two or more tracks on the same row sounding together (a double, a unison BV).
#[derive(Debug, Clone, PartialEq)]
pub struct Stack {
    /// The row (MIDI note).
    pub midi: i32,
    /// In time order.
    pub notes: Vec<StackNote>,
    /// The widest gap between their held pitches (cents).
    pub apart: f32,
}

impl Stack {
    /// The held pitches differ by more than `tolerance` (whole cents): the "≠" marker.
    pub fn mismatch(&self, tolerance: f32) -> bool {
        self.apart.is_finite() && self.apart.round() > tolerance.round()
    }

    /// Whether it holds note `r`.
    pub fn holds(&self, r: NoteRef) -> bool {
        self.notes.iter().any(|n| n.note.r == r)
    }

    /// Its tracks in the order of its notes, each once.
    pub fn tracks(&self) -> Vec<TrackId> {
        let mut v: Vec<TrackId> = Vec::new();
        for n in &self.notes {
            if !v.contains(&n.track) {
                v.push(n.track);
            }
        }
        v
    }
}

/// The root of `i` in a union-find parent list (paths halved on the way).
fn root(p: &mut [usize], mut i: usize) -> usize {
    while let Some(&up) = p.get(i) {
        if up == i {
            break;
        }
        let grand = p.get(up).copied().unwrap_or(up);
        if let Some(x) = p.get_mut(i) {
            *x = grand;
        }
        i = up;
    }
    i
}

/// The unison stacks among `tracks`' notes: notes of different tracks on the same row sounding
/// together ([`sound_together`]), chained (a note sounding with two others joins them).
pub fn unison_stacks(s: &Session, tracks: &[TrackId]) -> Vec<Stack> {
    let mut all: Vec<StackNote> = tracks.iter().flat_map(|t| studio_notes(s, *t).into_iter().map(|note| StackNote { track: *t, note })).collect();
    all.sort_by_key(|n| (n.note.start, n.note.r));
    let mut parent: Vec<usize> = (0..all.len()).collect();
    for (i, a) in all.iter().enumerate() {
        for (j, b) in all.iter().enumerate().skip(i + 1) {
            if b.note.start >= a.note.end {
                break;
            }
            if a.track != b.track && a.note.midi() == b.note.midi() && sound_together(s, &a.note, &b.note) {
                let (ra, rb) = (root(&mut parent, i), root(&mut parent, j));
                if ra != rb
                    && let Some(x) = parent.get_mut(rb)
                {
                    *x = ra;
                }
            }
        }
    }
    let mut groups: BTreeMap<usize, Vec<StackNote>> = BTreeMap::new();
    for (i, n) in all.iter().enumerate() {
        groups.entry(root(&mut parent, i)).or_default().push(*n);
    }
    let mut out: Vec<Stack> = groups
        .into_values()
        .filter(|g| g.len() > 1)
        .map(|notes| {
            let hi = notes.iter().map(|n| n.note.pitch).fold(f32::NEG_INFINITY, f32::max);
            let lo = notes.iter().map(|n| n.note.pitch).fold(f32::INFINITY, f32::min);
            let midi = notes.first().map_or(0, |n| n.note.midi());
            Stack { midi, notes, apart: hi - lo }
        })
        .collect();
    out.sort_by_key(|g| g.notes.first().map(|n| (n.note.start, n.note.r)));
    out
}

/// Where Match moves notes: onto another note's held pitch, or each onto its own target.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum MatchTo {
    Note(NoteRef),
    Centre,
}

/// Match's plan: each of `notes` (found on `tracks`) with the edit that puts its held pitch on
/// `to`'s (its manual offset moved by the difference) or, for [`MatchTo::Centre`], on its own
/// target (0 c). The anchor itself, notes not found and notes that can't move are left out.
pub fn match_plan(s: &Session, tracks: &[TrackId], notes: &[NoteRef], to: MatchTo) -> Vec<TuneStep> {
    let anchor_ref = match to {
        MatchTo::Note(a) => Some(a),
        MatchTo::Centre => None,
    };
    let mut anchor = None;
    if let Some(a) = anchor_ref {
        for t in tracks {
            each_note(s, *t, |c| {
                if c.r == a {
                    anchor = Some(c.pitch_with(&c.e).0);
                }
            });
        }
        if !anchor.is_some_and(f32::is_finite) {
            return Vec::new();
        }
    }
    let mut out = Vec::new();
    for t in tracks {
        each_note(s, *t, |c| {
            if !notes.contains(&c.r) || Some(c.r) == anchor_ref {
                return;
            }
            let before = c.offset();
            let edit = match anchor {
                Some(p) => {
                    let (now, _) = c.pitch_with(&c.e);
                    let mut e = c.e.clone();
                    e.offset_cents += p - now;
                    Some(e.clamped())
                }
                None => c.fixed(0.0),
            };
            if let Some(edit) = edit {
                let (p, tg) = c.pitch_with(&edit);
                out.push(TuneStep { track: c.track, r: c.r, edit, before, after: p - tg });
            }
        });
    }
    out
}

//! Vocal tuning: the Vocal track flag (Track › Vocal Track) and the per-clip tuning commands
//! (`vocal.*`; the background work is in [`crate::vocal`]).

use super::*;
use crate::cmd;
use crate::vocal::NoteRef;
use serde_json::json;
use soundcraft_model::{AnalysisKey, Session, TrackKind, VocalTune};
use soundcraft_vocal::{
    NoteEdit, NoteId, NoteMode, ScaleAt, SegOp, Transition, VocalAnalysis, carry_edits, factory_presets, join_notes_with, selection_summary_with,
    split_note_with, target_cents, tuned_centre_with, tuned_target_with, typical_level_db,
};
use std::collections::BTreeMap;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.vocal",
            "Vocal Track",
            [],
            None,
            "{tracks?, value?: bool, auto?: true} — the Vocal flag: `value` sets it, `auto: true` makes it follow the track name again (Vox, Lead Voc, BGV…), neither toggles it (on when any targeted audio track is not vocal). Only the targeted audio tracks are changed (other kinds are left as they are; none at all is refused). Undoable. Returns {tracks: the audio tracks acted on, vocal: [their resulting flag…]}",
            has_selection,
            vocal_flag
        ),
        cmd!(
            noundo "vocal.analyse",
            "Analyse Now",
            [],
            None,
            "{clip? | clips? | track?} — analyses the targeted audio clips now (pitch track, notes, unvoiced regions of what each clip sounds like, Clean Vocal included) and renders their tuning, blocking; otherwise the background does it for clips on Vocal tracks. Default: the selected clips, else the selected tracks' audio clips. Not undoable (derived data). Returns {clips: [{clip, notes, unvoiced, message?: why the tuning plays untuned (a phrase over 60 s without a breath)}…]}",
            has_selection,
            analyse
        ),
        cmd!(
            "vocal.edit",
            "Tune Vocal Edit",
            [],
            None,
            "{notes: [{clip, note, offset?, centre?, drift?, vibrato?, raise?, lower?, gain_db?}], centre?, drift?, raise?, lower?: 0..100 (%), vibrato?: -100..100 (%), offset?: cents (-2400..2400) | offset_by?: cents (-4800..4800), gain_db?: dB (-12..12), snap_to_key?: bool, transition?: keep|smooth|shorten, reference?: selection|track, from?, to?} — sets the given fields on each note's edit (the clip gets a tuning when it has none). `offset` is the note's manual pitch move (what dragging it does), `offset_by` adds to it (the result clamped to ±2400); a note's own values in `notes` override the shared ones for that note (the note bar moves several notes by one delta in one step). `gain_db` is the note's manual level change over its body, on top of the levelling (the note bar's level field). With raise/lower, each note's levelling reference becomes the median level of the notes (reference selection, the default) or of the notes' tracks' notes (track; from/to limit it to a range), frozen per note. Undoable; the UI merges a slider drag. Returns {notes}",
            always,
            edit
        ),
        cmd!(
            "vocal.macro",
            "Tune Vocal Macro",
            [],
            None,
            "{notes: [{clip, note}], preset?: name (First pass (gentle), Like Melodyne 70 %, Firm), centre?, drift?, raise?, lower?: 0..100, vibrato?: -100..100, reference?: selection|track, from?, to?} — applies a preset (values given override it) as one edit, with the levelling reference set. Undoable. Returns {notes}",
            always,
            macro_cmd
        ),
        cmd!(
            "vocal.step",
            "Centre Note",
            [],
            None,
            "{track, at: position, kind: centre|drift} — the note under `at`: Centre note pulls 40 % further to its target (centre = 1 − (1 − centre)·0.6), Flatten drift 30 % (drift = 1 − (1 − drift)·0.7). Undoable (Centre Note / Flatten Drift). Returns the note {clip, note}",
            always,
            step
        ),
        cmd!(
            "vocal.reset",
            "Reset Tuning",
            [],
            None,
            "{notes: [{clip, note}]} — the notes' edits back to none; a clip left with no edit at all and no split/join drops its tuning. Undoable. Returns {notes}",
            always,
            reset
        ),
        cmd!(
            "vocal.split",
            "Split Vocal Note",
            [],
            None,
            "{clip, note, at: position (session time, inside the note)} — splits the note in two (both halves keep its edit); replayed after every re-analysis. Undoable. Returns {notes: [first, second]}",
            always,
            split
        ),
        cmd!(
            "vocal.join",
            "Join Vocal Notes",
            [],
            None,
            "{clip, first, second} — joins two neighbouring notes (second straight after first); the joined note keeps the first's edit. Undoable. Returns {note}",
            always,
            join
        ),
        cmd!(
            query "vocal.notes",
            "Vocal Notes",
            [],
            None,
            "{clip? | track?, from?, to?} — the analysed notes (default: the selected clips), each {clip, note, start, end (session samples), scoop?: {start, end}, centre, target, offset, tuned_offset (cents), level_db (as sung), tuned_level_db (with the edit's gain_db), edit}, the unvoiced regions [{clip, start, end}] and a summary of the notes listed, plus messages: [{clip, message}] for clips whose tuning plays untuned (a phrase over 60 s without a breath). Only the notes in each clip's window; from/to limit them to a range",
            always,
            notes_query
        ),
        cmd!(
            "vocal.note_mode",
            "Notes Classic / Musical",
            [],
            None,
            "{clip? | clips? | track?, mode?: classic|musical} — how the targeted audio clips' pitch is cut into notes (default: the selected clips, else the selected tracks' audio clips; no `mode` switches them: Musical unless all already are). musical (new tunings and untuned clips): notes by syllable and settled pitch, scoops, slides and vibrato folded into their note as its pitch curve, each note centred on its stable middle, and the correction moves whole notes with smooth glides between them. classic (tunings saved before modes existed): every pitch held for 70 ms is a note, tight on a vocal near its notes. The edits move onto the overlapping notes of the new cut; splits and joins replay where they still apply. Saved with the clip. Undoable. Returns {clips: [{clip, note_mode, notes?}…]} (notes: the count, once analysed)",
            has_selection,
            note_mode_cmd
        ),
        cmd!(
            "vocal.toggle",
            "Tuned/Original",
            [],
            None,
            "{clip? | clips? | track?} — flips the Tuned/Original switch of the targeted clips tuned by Tune Vocal (default: the selected clips; `track`: its tuned clips); Original plays the clip untuned, the edits are kept. Undoable. Returns {clips: [{clip, original}…]}",
            has_selection,
            toggle
        ),
        cmd!(
            "vocal.commit",
            "Commit Tuning",
            [],
            None,
            "{clip? | clips?} — renders each tuned clip as it plays (tuning, and Clean Vocal blended in) over its window plus 1 s handles each side into one new source (`<clip>-tuned`; the clip's offset and lyrics move with it) and drops the tuning and the Clean Vocal alternate; with Original on or no edit it only drops the tuning. Waits for the clip's analysis and render first. Undoable. Returns {clips: [{clip, source}…]}",
            has_selection,
            commit
        ),
        cmd!(
            "vocal.remove",
            "Remove Tuning",
            [],
            None,
            "{clip? | clips?} — drops the targeted clips' tuning (they play untuned). Undoable. Returns {clips}",
            has_selection,
            remove
        ),
    ]
}

const ANALYSE: &str = "vocal.analyse";

// ---- note edits ----------------------------------------------------------------------------

const EDIT: &str = "vocal.edit";
const MACRO: &str = "vocal.macro";
const STEP: &str = "vocal.step";
const RESET: &str = "vocal.reset";
/// Largest manual pitch move a note keeps (cents), and the largest relative step.
const MAX_OFFSET: f64 = soundcraft_vocal::MAX_OFFSET_CENTS as f64;
const MAX_OFFSET_BY: f64 = 2.0 * MAX_OFFSET;
/// Largest manual level change a note keeps (dB).
const MAX_GAIN: f64 = soundcraft_vocal::MAX_GAIN_DB as f64;
const SPLIT: &str = "vocal.split";
const JOIN: &str = "vocal.join";
const NOTES: &str = "vocal.notes";
const TOGGLE: &str = "vocal.toggle";
const NOTE_MODE: &str = "vocal.note_mode";
const COMMIT: &str = "vocal.commit";
const REMOVE: &str = "vocal.remove";

/// Why a note command waits: the clip's analysis isn't in yet.
const ANALYSING: &str = "analysing — try again in a moment";

/// The Centre note step (40 %) and the Flatten drift step (30 %), compounding.
const CENTRE_STEP: f32 = 0.40;
const DRIFT_STEP: f32 = 0.30;

/// Why a note command refuses a stretched clip (it plays untuned).
const STRETCHED: &str = "tuning can't follow a stretched clip — commit the stretch first";

/// Why Tuned/Original, Commit and Remove refuse.
const UNTUNED: &str = "choose a clip tuned by Tune Vocal";

/// `clip`'s analysis (its tuning's splits and joins replayed): refused when the clip is missing
/// or not audio, and asked to wait while it is being analysed.
fn analysed(e: &Engine, id: &str, clip: ClipId) -> Result<VocalAnalysis> {
    let s = e.session();
    let (_, c) = s.find_clip(clip).ok_or_else(|| bad(id, format!("no clip with id {}", clip.0)))?;
    if !c.is_audio() {
        return Err(bad(id, format!("`{}` is not an audio clip", c.name)));
    }
    crate::vocal::analysis_for(s, c).ok_or_else(|| bad(id, waiting_text(s, c)))
}

/// Why a clip has no analysis: it is being analysed, or it never will be.
fn waiting_text(s: &soundcraft_model::Session, c: &soundcraft_model::Clip) -> String {
    if crate::vocal::cannot_analyse(s, c) {
        let track = s.find_clip(c.id).and_then(|(t, _)| s.track(t)).map_or(c.name.as_str(), |t| t.name.as_str());
        crate::vocal::cannot_analyse_text(track)
    } else {
        ANALYSING.to_string()
    }
}

/// [`analysed`] for a command that edits notes: a stretched clip is refused too.
fn editable(e: &Engine, id: &str, clip: ClipId) -> Result<VocalAnalysis> {
    let a = analysed(e, id, clip)?;
    if e.session().find_clip(clip).is_some_and(|(_, c)| crate::vocal::stretched(c)) {
        return Err(bad(id, STRETCHED));
    }
    Ok(a)
}

fn note_id(id: &str, v: Option<&Value>, key: &str) -> Result<NoteId> {
    v.and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).map(NoteId).ok_or_else(|| bad(id, format!("`{key}` must be a note id")))
}

/// `notes`: every `{clip, note}`, each naming an analysed clip's note (duplicates dropped), and
/// each clip's analysis.
fn note_refs(e: &Engine, id: &str, p: &Value) -> Result<(Vec<NoteRef>, BTreeMap<ClipId, VocalAnalysis>)> {
    let arr = p.get("notes").and_then(Value::as_array).ok_or_else(|| bad(id, "`notes` must be a list of {clip, note}"))?;
    let mut refs: Vec<NoteRef> = Vec::with_capacity(arr.len());
    for v in arr {
        let clip = v.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(id, "each note is {clip, note}"))?;
        let r = NoteRef { clip, note: note_id(id, v.get("note"), "note")? };
        if !refs.contains(&r) {
            refs.push(r);
        }
    }
    if refs.is_empty() {
        return Err(bad(id, "choose a note"));
    }
    let mut analyses = BTreeMap::new();
    for r in &refs {
        if let std::collections::btree_map::Entry::Vacant(v) = analyses.entry(r.clip) {
            v.insert(editable(e, id, r.clip)?);
        }
        if !analyses.get(&r.clip).is_some_and(|a| a.notes.iter().any(|n| n.id == r.note)) {
            return Err(bad(id, format!("no note {} in clip {}", r.note.0, r.clip.0)));
        }
    }
    Ok((refs, analyses))
}

/// An optional percentage `lo..=100`, as a fraction; anything else (a string, null, NaN) refused.
fn pct(id: &str, p: &Value, key: &str, lo: f64) -> Result<Option<f32>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => match v.as_f64().filter(|m| m.is_finite() && (lo..=100.0).contains(m)) {
            Some(m) => Ok(Some((m / 100.0) as f32)),
            None => Err(bad(id, format!("`{key}` must be a number from {lo} to 100 (%)"))),
        },
    }
}

/// An optional number of cents within `±max`; anything else (a string, null, NaN) refused.
fn cents(id: &str, v: Option<&Value>, key: &str, max: f64) -> Result<Option<f32>> {
    match v {
        None => Ok(None),
        Some(v) => match v.as_f64().filter(|c| c.is_finite() && c.abs() <= max) {
            Some(c) => Ok(Some(c as f32)),
            None => Err(bad(id, format!("`{key}` must be a number of cents from -{max} to {max}"))),
        },
    }
}

/// How a command moves the notes' manual pitch offset.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Offset {
    To(f32),
    By(f32),
}

/// One note's own values in `notes: [{clip, note, …}]`, overriding the shared ones.
#[derive(Debug, Clone, Copy, Default, PartialEq)]
struct NoteValues {
    offset: Option<f32>,
    centre: Option<f32>,
    drift: Option<f32>,
    vibrato: Option<f32>,
    raise: Option<f32>,
    lower: Option<f32>,
    gain: Option<f32>,
}

impl NoteValues {
    fn is_empty(&self) -> bool {
        *self == NoteValues::default()
    }

    fn levels(&self) -> bool {
        self.raise.is_some() || self.lower.is_some()
    }
}

/// An optional `gain_db` (±12), as a number of dB; anything else refused.
fn gain_param(id: &str, p: &Value) -> Result<Option<f32>> {
    match p.get("gain_db") {
        None => Ok(None),
        Some(v) => match v.as_f64().filter(|g| g.is_finite() && g.abs() <= MAX_GAIN) {
            Some(g) => Ok(Some(g as f32)),
            None => Err(bad(id, format!("`gain_db` must be a number of dB from -{MAX_GAIN} to {MAX_GAIN}"))),
        },
    }
}

/// The edit fields a command sets (`None`: left as they are).
#[derive(Debug, Clone, Default)]
struct Fields {
    centre: Option<f32>,
    drift: Option<f32>,
    vibrato: Option<f32>,
    raise: Option<f32>,
    lower: Option<f32>,
    snap: Option<bool>,
    transition: Option<Transition>,
    offset: Option<Offset>,
    /// The manual level change (dB).
    gain: Option<f32>,
    /// Per-note values (`notes: [{clip, note, offset, centre, …}]`), overriding the shared ones.
    note_values: BTreeMap<NoteRef, NoteValues>,
}

impl Fields {
    fn parse(id: &str, p: &Value) -> Result<Fields> {
        let snap = match p.get("snap_to_key") {
            None => None,
            Some(v) => Some(v.as_bool().ok_or_else(|| bad(id, "`snap_to_key` must be true or false"))?),
        };
        let transition = match p.get("transition") {
            None => None,
            Some(v) => Some(match v.as_str() {
                Some("keep") => Transition::Keep,
                Some("smooth") => Transition::Smooth,
                Some("shorten") => Transition::Shorten,
                _ => return Err(bad(id, "`transition` must be keep, smooth or shorten")),
            }),
        };
        let offset = match (cents(id, p.get("offset"), "offset", MAX_OFFSET)?, cents(id, p.get("offset_by"), "offset_by", MAX_OFFSET_BY)?) {
            (Some(_), Some(_)) => return Err(bad(id, "pass `offset` or `offset_by`, not both")),
            (Some(c), None) => Some(Offset::To(c)),
            (None, Some(c)) => Some(Offset::By(c)),
            (None, None) => None,
        };
        let mut note_values = BTreeMap::new();
        for v in p.get("notes").and_then(Value::as_array).into_iter().flatten() {
            let nv = NoteValues {
                offset: cents(id, v.get("offset"), "offset", MAX_OFFSET)?,
                centre: pct(id, v, "centre", 0.0)?,
                drift: pct(id, v, "drift", 0.0)?,
                vibrato: pct(id, v, "vibrato", -100.0)?,
                raise: pct(id, v, "raise", 0.0)?,
                lower: pct(id, v, "lower", 0.0)?,
                gain: gain_param(id, v)?,
            };
            if nv.is_empty() {
                continue;
            }
            let clip = v.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(id, "each note is {clip, note}"))?;
            note_values.insert(NoteRef { clip, note: note_id(id, v.get("note"), "note")? }, nv);
        }
        Ok(Fields {
            gain: gain_param(id, p)?,
            centre: pct(id, p, "centre", 0.0)?,
            drift: pct(id, p, "drift", 0.0)?,
            vibrato: pct(id, p, "vibrato", -100.0)?,
            raise: pct(id, p, "raise", 0.0)?,
            lower: pct(id, p, "lower", 0.0)?,
            snap,
            transition,
            offset,
            note_values,
        })
    }

    fn is_empty(&self) -> bool {
        [self.centre, self.drift, self.vibrato, self.raise, self.lower].iter().all(Option::is_none)
            && self.snap.is_none()
            && self.transition.is_none()
            && self.offset.is_none()
            && self.gain.is_none()
            && self.note_values.is_empty()
    }

    fn levels(&self) -> bool {
        self.raise.is_some() || self.lower.is_some() || self.note_values.values().any(NoteValues::levels)
    }

    /// Sets the fields on note `r`'s edit `e`.
    fn apply(&self, r: NoteRef, e: &mut NoteEdit) {
        let own = self.note_values.get(&r).copied().unwrap_or_default();
        match (own.offset, self.offset) {
            (Some(c), _) | (None, Some(Offset::To(c))) => e.offset_cents = c,
            (None, Some(Offset::By(c))) => e.offset_cents += c,
            (None, None) => {}
        }
        let set = |dst: &mut f32, v: Option<f32>| {
            if let Some(v) = v {
                *dst = v;
            }
        };
        set(&mut e.centre, own.centre.or(self.centre));
        set(&mut e.drift, own.drift.or(self.drift));
        set(&mut e.vibrato, own.vibrato.or(self.vibrato));
        set(&mut e.raise, own.raise.or(self.raise));
        set(&mut e.lower, own.lower.or(self.lower));
        set(&mut e.gain_db, own.gain.or(self.gain));
        if let Some(b) = self.snap {
            e.snap_to_key = b;
        }
        if let Some(t) = self.transition {
            e.transition = t;
        }
    }
}

/// The levelling reference for `refs`: the median level of those notes (`reference: selection`,
/// the default) or of every note on their tracks (`reference: track`, within `from`/`to`).
fn typical_for(e: &Engine, id: &str, p: &Value, refs: &[NoteRef], analyses: &BTreeMap<ClipId, VocalAnalysis>) -> Result<f32> {
    let track = match p.get("reference") {
        None => false,
        Some(v) => match v.as_str() {
            Some("selection") => false,
            Some("track") => true,
            _ => return Err(bad(id, "`reference` must be selection or track")),
        },
    };
    let level = |a: &VocalAnalysis, n: NoteId| a.notes.iter().find(|x| x.id == n).map(|x| x.level_db);
    if !track {
        let levels: Vec<f32> = refs.iter().filter_map(|r| analyses.get(&r.clip).and_then(|a| level(a, r.note))).collect();
        return Ok(typical_level_db(&levels));
    }
    let from = position_param(e, id, p, "from")?;
    let to = position_param(e, id, p, "to")?;
    let range = match (from, to) {
        (None, None) => None,
        (a, b) => Some(Range::new(a.unwrap_or(-MAX_POSITION), b.unwrap_or(MAX_POSITION))),
    };
    let s = e.session();
    let mut tracks: Vec<TrackId> = refs.iter().filter_map(|r| s.find_clip(r.clip).map(|(t, _)| t)).collect();
    tracks.sort();
    tracks.dedup();
    let mut cache: BTreeMap<ClipId, Option<VocalAnalysis>> = BTreeMap::new();
    let mut levels = Vec::new();
    for t in tracks {
        for r in crate::vocal::track_notes(s, t, range) {
            let a = cache.entry(r.clip).or_insert_with(|| s.find_clip(r.clip).and_then(|(_, c)| crate::vocal::analysis_for(s, c)));
            if let Some(l) = a.as_ref().and_then(|a| level(a, r.note)) {
                levels.push(l);
            }
        }
    }
    Ok(typical_level_db(&levels))
}

/// `clip`'s live tuning to change: created (or, when the clip plays another source than an old
/// tuning was made on, replaced) against the clip's current source and analysis.
fn tune_mut(s: &mut Session, clip: ClipId, key: u64) -> Option<&mut VocalTune> {
    let c = s.find_clip_mut(clip)?;
    let source = c.source();
    if c.tune_live().is_none() {
        c.tune = Some(VocalTune { analysed: key, source, note_mode: NoteMode::NEW, ..VocalTune::default() });
    }
    let t = c.tune.as_mut()?;
    if t.source.is_none() {
        t.source = source;
    }
    if t.analysed == 0 {
        t.analysed = key;
    }
    Some(t)
}

fn key_of(s: &Session, clip: ClipId) -> u64 {
    s.find_clip(clip).and_then(|(_, c)| AnalysisKey::of(s, c)).map_or(0, |k| k.hash())
}

/// Changes each of `refs`' edits with `f` (clamped after), keeping the note spans current.
fn change_edits(e: &mut Engine, refs: &[NoteRef], analyses: &BTreeMap<ClipId, VocalAnalysis>, f: impl Fn(NoteRef, &mut NoteEdit)) {
    let keys: BTreeMap<ClipId, u64> = analyses.keys().map(|c| (*c, key_of(e.session(), *c))).collect();
    let s = e.session_mut();
    for (clip, a) in analyses {
        let Some(t) = tune_mut(s, *clip, keys.get(clip).copied().unwrap_or(0)) else { continue };
        for r in refs.iter().filter(|r| r.clip == *clip) {
            let ed = t.edits.entry(r.note).or_default();
            f(*r, ed);
            *ed = std::mem::take(ed).clamped();
        }
        crate::vocal::sync_note_spans(t, a);
    }
}

/// Sets `fields` on `refs`, with the levelling reference when `level` (or the fields level).
fn set_fields(e: &mut Engine, id: &str, p: &Value, fields: &Fields, level: bool) -> Result<Value> {
    let (refs, analyses) = note_refs(e, id, p)?;
    let typical = if level || fields.levels() { Some(typical_for(e, id, p, &refs, &analyses)?) } else { None };
    change_edits(e, &refs, &analyses, |r, ed| {
        fields.apply(r, ed);
        if let Some(t) = typical {
            ed.typical_db = Some(t);
        }
    });
    Ok(json!({"notes": refs}))
}

fn edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let fields = Fields::parse(EDIT, p)?;
    if fields.is_empty() {
        return Err(bad(EDIT, "pass a value to set: centre, drift, raise, lower, vibrato, offset, offset_by, gain_db, snap_to_key or transition"));
    }
    set_fields(e, EDIT, p, &fields, false)
}

fn macro_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    let per_note = |key: &str| p.get("notes").and_then(Value::as_array).is_some_and(|a| a.iter().any(|v| v.get(key).is_some()));
    for key in ["snap_to_key", "transition", "offset", "offset_by", "gain_db"] {
        if p.get(key).is_some() || per_note(key) {
            return Err(bad(MACRO, format!("a macro doesn't set `{key}`: use vocal.edit")));
        }
    }
    let given = Fields::parse(MACRO, p)?;
    let mut fields = match p.get("preset") {
        None => Fields::default(),
        Some(v) => {
            let name = v.as_str().ok_or_else(|| bad(MACRO, "`preset` must be a preset name"))?;
            let pr = factory_presets().into_iter().find(|x| x.name.eq_ignore_ascii_case(name.trim())).ok_or_else(|| {
                let names: Vec<String> = factory_presets().into_iter().map(|x| x.name).collect();
                bad(MACRO, format!("no preset `{name}` (try {})", names.join(", ")))
            })?;
            Fields {
                centre: Some(pr.centre),
                drift: Some(pr.drift),
                vibrato: Some(pr.vibrato),
                raise: Some(pr.raise),
                lower: Some(pr.lower),
                ..Fields::default()
            }
        }
    };
    let or = |a: Option<f32>, b: Option<f32>| a.or(b);
    fields.centre = or(given.centre, fields.centre);
    fields.drift = or(given.drift, fields.drift);
    fields.vibrato = or(given.vibrato, fields.vibrato);
    fields.raise = or(given.raise, fields.raise);
    fields.lower = or(given.lower, fields.lower);
    if fields.is_empty() {
        return Err(bad(MACRO, "pass a `preset` or values"));
    }
    set_fields(e, MACRO, p, &fields, true)
}

fn step(e: &mut Engine, p: &Value) -> Result<Value> {
    let track = track_param(e, STEP, p, "track")?.ok_or_else(|| bad(STEP, "`track` required"))?;
    let at = position_param(e, STEP, p, "at")?.ok_or_else(|| bad(STEP, "`at` required"))?;
    let centre = match str_param(p, "kind") {
        Some("centre") => true,
        Some("drift") => false,
        _ => return Err(bad(STEP, "`kind` must be centre or drift")),
    };
    let s = e.session();
    let Some(r) = crate::vocal::note_at(s, track, at) else {
        let waiting = s
            .track(track)
            .and_then(|t| t.clips().iter().find(|c| c.is_audio() && c.range().contains(at) && crate::vocal::analysis_for(s, c).is_none()));
        return Err(bad(STEP, waiting.map_or_else(|| "no note there".to_string(), |c| waiting_text(s, c))));
    };
    let a = editable(e, STEP, r.clip)?;
    let analyses = BTreeMap::from([(r.clip, a)]);
    change_edits(e, &[r], &analyses, |_, ed| {
        if centre {
            ed.centre = 1.0 - (1.0 - ed.centre) * (1.0 - CENTRE_STEP);
        } else {
            ed.drift = 1.0 - (1.0 - ed.drift) * (1.0 - DRIFT_STEP);
        }
    });
    e.step_label = Some(if centre { "Centre Note" } else { "Flatten Drift" });
    Ok(json!(r))
}

fn reset(e: &mut Engine, p: &Value) -> Result<Value> {
    let (refs, analyses) = note_refs(e, RESET, p)?;
    let s = e.session_mut();
    for (clip, a) in &analyses {
        let Some(c) = s.find_clip_mut(*clip) else { continue };
        if c.tune_live().is_none() {
            continue;
        }
        let Some(t) = c.tune.as_mut() else { continue };
        for r in refs.iter().filter(|r| r.clip == *clip) {
            t.edits.remove(&r.note);
        }
        crate::vocal::sync_note_spans(t, a);
        // Only an empty tuning goes: an identity-looking edit can still hold Snap to key.
        if t.edits.is_empty() && t.seg_ops.is_empty() && t.note_mode == NoteMode::NEW {
            c.tune = None;
        }
    }
    Ok(json!({"notes": refs}))
}

fn split(e: &mut Engine, p: &Value) -> Result<Value> {
    let clip = clip_id(p, SPLIT)?;
    let note = note_id(SPLIT, p.get("note"), "note")?;
    let at = position_param(e, SPLIT, p, "at")?.ok_or_else(|| bad(SPLIT, "`at` required"))?;
    let mut a = editable(e, SPLIT, clip)?;
    let s = e.session();
    let (_, c) = s.find_clip(clip).ok_or_else(|| bad(SPLIT, format!("no clip with id {}", clip.0)))?;
    let n = a.notes.iter().find(|n| n.id == note).ok_or_else(|| bad(SPLIT, format!("no note {} in clip {}", note.0, clip.0)))?;
    let frame = crate::vocal::FrameMap::of(c, &a)
        .frame(at)
        .filter(|f| n.start < *f && *f < n.end)
        .ok_or_else(|| bad(SPLIT, "`at` must be inside the note"))?;
    let key = key_of(s, clip);
    let mut edits = c.tune_live().map(|t| t.edits.clone()).unwrap_or_default();
    let mode = crate::vocal::note_mode(c);
    let (first, second) = split_note_with(&mut a, &mut edits, note, frame, mode).ok_or_else(|| bad(SPLIT, "a note already starts there"))?;
    let t = tune_mut(e.session_mut(), clip, key).ok_or_else(|| bad(SPLIT, format!("no clip with id {}", clip.0)))?;
    t.edits = edits;
    t.seg_ops.push(SegOp::Split { at: frame });
    crate::vocal::sync_note_spans(t, &a);
    Ok(json!({"notes": [NoteRef { clip, note: first }, NoteRef { clip, note: second }]}))
}

fn join(e: &mut Engine, p: &Value) -> Result<Value> {
    let clip = clip_id(p, JOIN)?;
    let first = note_id(JOIN, p.get("first"), "first")?;
    let second = note_id(JOIN, p.get("second"), "second")?;
    let mut a = editable(e, JOIN, clip)?;
    let s = e.session();
    let (_, c) = s.find_clip(clip).ok_or_else(|| bad(JOIN, format!("no clip with id {}", clip.0)))?;
    for n in [first, second] {
        if !a.notes.iter().any(|x| x.id == n) {
            return Err(bad(JOIN, format!("no note {} in clip {}", n.0, clip.0)));
        }
    }
    let key = key_of(s, clip);
    let mut edits = c.tune_live().map(|t| t.edits.clone()).unwrap_or_default();
    let joined = join_notes_with(&mut a, &mut edits, first, second, crate::vocal::note_mode(c))
        .ok_or_else(|| bad(JOIN, "only neighbouring notes join: `second` must come straight after `first`"))?;
    let t = tune_mut(e.session_mut(), clip, key).ok_or_else(|| bad(JOIN, format!("no clip with id {}", clip.0)))?;
    t.edits = edits;
    t.seg_ops.push(SegOp::Join { at: second.0 });
    crate::vocal::sync_note_spans(t, &a);
    Ok(json!({"note": NoteRef { clip, note: joined }}))
}

/// Running totals for `vocal.notes`' summary across clips.
#[derive(Default)]
struct SummaryAcc {
    count: usize,
    start: Option<Samples>,
    end: Option<Samples>,
    off_before: f64,
    off_after: f64,
    drift_before: f64,
    drift_after: f64,
    levels: Vec<f32>,
}

impl SummaryAcc {
    fn json(&self) -> Value {
        if self.count == 0 {
            return Value::Null;
        }
        let c = self.count as f64;
        let max = self.levels.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        let min = self.levels.iter().copied().fold(f32::INFINITY, f32::min);
        json!({
            "count": self.count,
            "start": self.start,
            "end": self.end,
            "avg_off_before": self.off_before / c,
            "avg_off_after": self.off_after / c,
            "drift_before": self.drift_before / c,
            "drift_after": self.drift_after / c,
            "level_spread_db": if max.is_finite() && min.is_finite() { max - min } else { 0.0 },
            "typical_db": typical_level_db(&self.levels),
        })
    }
}

fn notes_query(e: &mut Engine, p: &Value) -> Result<Value> {
    let s = e.session();
    let clips: Vec<ClipId> = if let Some(v) = p.get("clip") {
        let id = v.as_u64().map(ClipId).ok_or_else(|| bad(NOTES, "`clip` must be a clip id"))?;
        analysed(e, NOTES, id)?;
        vec![id]
    } else if let Some(t) = track_param(e, NOTES, p, "track")? {
        s.track(t).map(|tr| tr.clips().iter().filter(|c| c.is_audio()).map(|c| c.id).collect()).unwrap_or_default()
    } else if !s.edit.selected_clips.is_empty() {
        s.edit.selected_clips.clone()
    } else {
        return Err(bad(NOTES, "pass `clip` or `track`, or select clips"));
    };
    let from = position_param(e, NOTES, p, "from")?;
    let to = position_param(e, NOTES, p, "to")?;
    let range = Range::new(from.unwrap_or(-MAX_POSITION), to.unwrap_or(MAX_POSITION));
    let none = BTreeMap::new();
    let mut notes = Vec::new();
    let mut unvoiced = Vec::new();
    let mut acc = SummaryAcc::default();
    let mut messages = Vec::new();
    let mut modes = Vec::new();
    for id in clips {
        let Some((_, c)) = s.find_clip(id) else { continue };
        let Some(a) = crate::vocal::analysis_for(s, c) else { continue };
        if let Some(m) = crate::vocal::render_failed_message(s, c) {
            messages.push(json!({"clip": id, "message": m}));
        }
        let m = crate::vocal::FrameMap::of(c, &a);
        let scale = crate::vocal::scale_at(s, c);
        let edits = c.tune_live().map_or(&none, |t| &t.edits);
        let mode = crate::vocal::note_mode(c);
        modes.push(json!({"clip": id, "note_mode": mode.name()}));
        let shown = |start: u32, end: u32| m.in_window(start, end) && Range::new(m.time(start), m.time(end)).overlaps(&range);
        let mut ids = Vec::new();
        for n in a.notes.iter().filter(|n| shown(n.start, n.end)) {
            let ed = edits.get(&n.id);
            let applied = ed.cloned().unwrap_or_default().clamped();
            let centre = tuned_centre_with(&a, n, &NoteEdit::default(), &scale, mode);
            let snap = if applied.snap_to_key { scale.scale_at(n.start) } else { None };
            let target = target_cents(centre, snap);
            let tuned = tuned_centre_with(&a, n, &applied, &scale, mode);
            // A moved note is judged against the semitone it was moved to.
            let moved = tuned_target_with(&a, n, &applied, &scale, mode);
            notes.push(json!({
                "clip": id,
                "note": n.id,
                "start": m.time(n.start),
                "end": m.time(n.end),
                "scoop": n.scoop.map(|(a, b)| json!({"start": m.time(a), "end": m.time(b)})),
                "centre": centre,
                "target": target,
                "offset": centre - target,
                "tuned_offset": tuned - moved,
                "level_db": n.level_db,
                "tuned_level_db": n.level_db + applied.gain_db,
                "edit": ed,
            }));
            ids.push(n.id);
            acc.levels.push(n.level_db);
        }
        for u in a.unvoiced.iter().filter(|u| shown(u.start, u.end)) {
            unvoiced.push(json!({"clip": id, "start": m.time(u.start), "end": m.time(u.end)}));
        }
        if let Some(sm) = selection_summary_with(&a, edits, &ids, &scale, mode) {
            let w = sm.count as f64;
            acc.count += sm.count;
            let (st, en) = (m.time(sm.start_frame), m.time(sm.end_frame));
            acc.start = Some(acc.start.map_or(st, |x| x.min(st)));
            acc.end = Some(acc.end.map_or(en, |x| x.max(en)));
            acc.off_before += f64::from(sm.avg_off_before) * w;
            acc.off_after += f64::from(sm.avg_off_after) * w;
            acc.drift_before += f64::from(sm.drift_before) * w;
            acc.drift_after += f64::from(sm.drift_after) * w;
        }
    }
    let mut out = json!({"notes": notes, "unvoiced": unvoiced, "summary": acc.json(), "clips": modes});
    if !messages.is_empty()
        && let Some(o) = out.as_object_mut()
    {
        o.insert("messages".into(), json!(messages));
    }
    Ok(out)
}

/// `vocal.analyse`'s clips: `clip`/`clips`, else `track`'s audio clips, else the selection (the
/// selected clips, else the selected tracks' audio clips). Every one must be an audio clip.
fn analyse_targets(e: &Engine, p: &Value) -> Result<Vec<ClipId>> {
    audio_targets(e, ANALYSE, p)
}

/// `p`'s audio clips for command `id`, as [`analyse_targets`] picks them.
fn audio_targets(e: &Engine, id: &str, p: &Value) -> Result<Vec<ClipId>> {
    let cmd = id;
    let s = e.session();
    let ids: Vec<ClipId> = if p.get("clip").is_some() || p.get("clips").is_some() {
        if p.get("clip").is_some_and(|v| !v.is_u64()) || p.get("clips").is_some_and(|v| !v.is_array()) {
            return Err(bad(cmd, "`clip` must be a clip id, `clips` a list of them"));
        }
        clip_ids_param(e, p)
    } else if let Some(t) = track_param(e, cmd, p, "track")? {
        s.track(t).map(|tr| tr.clips().iter().filter(|c| c.is_audio()).map(|c| c.id).collect()).unwrap_or_default()
    } else if !s.edit.selected_clips.is_empty() {
        s.edit.selected_clips.clone()
    } else {
        s.tracks
            .iter()
            .filter(|t| s.edit.selected_tracks.contains(&t.id))
            .flat_map(|t| t.clips().iter().filter(|c| c.is_audio()).map(|c| c.id))
            .collect()
    };
    for id in &ids {
        match s.find_clip(*id) {
            None => return Err(bad(cmd, format!("no clip with id {}", id.0))),
            Some((_, c)) if !c.is_audio() => return Err(bad(cmd, format!("`{}` is not an audio clip", c.name))),
            Some(_) => {}
        }
    }
    if ids.is_empty() {
        return Err(bad(cmd, "choose an audio clip"));
    }
    Ok(ids)
}

fn analyse(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = analyse_targets(e, p)?;
    crate::vocal::run_until_idle(e, &ids)?;
    let s = e.session();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some((_, c)) = s.find_clip(id) else { continue };
        let a =
            crate::vocal::analysis_for(s, c).ok_or_else(|| bad(ANALYSE, format!("`{}` cannot be analysed (missing audio, or too long)", c.name)))?;
        let mut row = json!({"clip": id.0, "notes": a.notes.len(), "unvoiced": a.unvoiced.len(), "note_mode": crate::vocal::note_mode(c).name()});
        if let Some(m) = crate::vocal::render_failed_message(s, c)
            && let Some(o) = row.as_object_mut()
        {
            o.insert("message".into(), json!(m));
        }
        out.push(row);
    }
    Ok(json!({"clips": out}))
}

// ---- note detection mode -------------------------------------------------------------------

fn note_mode_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = audio_targets(e, NOTE_MODE, p)?;
    let mode = match p.get("mode") {
        Some(v) => v.as_str().and_then(NoteMode::parse).ok_or_else(|| bad(NOTE_MODE, "`mode` must be classic or musical"))?,
        // No mode: the menu's switch. Musical unless every target already is.
        None => {
            let s = e.session();
            let all = ids.iter().all(|id| s.find_clip(*id).is_some_and(|(_, c)| crate::vocal::note_mode(c) == NoteMode::Musical));
            if all { NoteMode::Classic } else { NoteMode::Musical }
        }
    };
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let s = e.session();
        let Some((_, c)) = s.find_clip(id) else { continue };
        let old_mode = crate::vocal::note_mode(c);
        if old_mode == mode {
            let notes = crate::vocal::analysis_for(s, c).map(|a| a.notes.len());
            out.push(json!({"clip": id.0, "note_mode": mode.name(), "notes": notes}));
            continue;
        }
        // The edits move onto the overlapping notes of the new cut (as after a re-analysis).
        let before = crate::vocal::analysis_for(s, c);
        let key = key_of(s, id);
        let mut clip = c.clone();
        let t = clip.tune.get_or_insert_with(|| VocalTune { note_mode: NoteMode::NEW, ..VocalTune::default() });
        t.note_mode = mode;
        let after = crate::vocal::analysis_for(s, &clip);
        let s = e.session_mut();
        let Some(t) = tune_mut(s, id, key) else { continue };
        t.note_mode = mode;
        if let (Some(b), Some(a)) = (&before, &after) {
            t.edits = carry_edits(&b.notes, &t.edits, &a.notes);
            crate::vocal::sync_note_spans(t, a);
        }
        // Back to the new-tuning mode with nothing else to keep: no tuning, as an untuned clip.
        if mode == NoteMode::NEW
            && t.edits.is_empty()
            && t.seg_ops.is_empty()
            && !t.original
            && let Some(cm) = s.find_clip_mut(id)
        {
            cm.tune = None;
        }
        out.push(json!({"clip": id.0, "note_mode": mode.name(), "notes": after.map(|a| a.notes.len())}));
    }
    Ok(json!({"clips": out}))
}

// ---- Tuned/Original, Commit, Remove --------------------------------------------------------

/// The clips with a live tuning among `p`'s targets: `clip`/`clips`, else `track`'s clips (when
/// `track` is allowed), else the selection. A named clip that is missing or untuned is refused.
fn tuned_targets(e: &Engine, id: &str, p: &Value, track: bool) -> Result<Vec<ClipId>> {
    let s = e.session();
    let named = p.get("clip").is_some() || p.get("clips").is_some();
    if p.get("clip").is_some_and(|v| !v.is_u64()) || p.get("clips").is_some_and(|v| !v.is_array()) {
        return Err(bad(id, "`clip` must be a clip id, `clips` a list of them"));
    }
    let ids: Vec<ClipId> = if !named && track && p.get("track").is_some() {
        let t = track_param(e, id, p, "track")?.ok_or_else(|| bad(id, "no such track"))?;
        s.track(t).map(|tr| tr.clips().iter().map(|c| c.id).collect()).unwrap_or_default()
    } else {
        clip_ids_param(e, p)
    };
    let mut out = Vec::new();
    for c in ids {
        match s.find_clip(c) {
            Some((_, cl)) if cl.tune_live().is_some() => out.push(c),
            Some(_) if named => return Err(bad(id, UNTUNED)),
            Some(_) => {}
            None if named => return Err(bad(id, format!("no clip with id {}", c.0))),
            None => {}
        }
    }
    if out.is_empty() {
        return Err(bad(id, UNTUNED));
    }
    Ok(out)
}

fn toggle(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = tuned_targets(e, TOGGLE, p, true)?;
    let s = e.session_mut();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        if let Some(t) = s.find_clip_mut(id).and_then(|c| c.tune.as_mut()) {
            t.original = !t.original;
            out.push(json!({"clip": id.0, "original": t.original}));
        }
    }
    Ok(json!({"clips": out}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = tuned_targets(e, REMOVE, p, false)?;
    let s = e.session_mut();
    for id in &ids {
        if let Some(c) = s.find_clip_mut(*id) {
            c.tune = None;
        }
    }
    Ok(json!({"clips": ids}))
}

fn commit(e: &mut Engine, p: &Value) -> Result<Value> {
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    use soundcraft_model::ClipContent;
    let ids = tuned_targets(e, COMMIT, p, false)?;
    // Analysed and rendered first, so the commit bakes what the tuning sounds like.
    crate::vocal::run_until_idle(e, &ids)?;
    let rate = e.session().sample_rate.hz();
    let mut out = Vec::with_capacity(ids.len());
    for id in ids {
        let Some(c) = e.session().find_clip(id).map(|(_, c)| c.clone()) else { continue };
        let Some(tune) = c.tune_live() else { continue };
        let Some(source) = c.source() else { continue };
        // Nothing to bake: Original on, no edit, or a stretched clip (it plays untuned).
        if tune.original || tune.edits.is_empty() || crate::vocal::stretched(&c) {
            if let Some(cm) = e.session_mut().find_clip_mut(id) {
                cm.tune = None;
            }
            out.push(json!({"clip": id.0, "source": source.0}));
            continue;
        }
        let s = e.session();
        if let Some(m) = crate::vocal::render_failed_message(s, &c) {
            return Err(bad(COMMIT, format!("`{}`: {m}", c.name)));
        }
        if crate::vocal::analysis_for(s, &c).is_none() {
            return Err(bad(COMMIT, format!("`{}` cannot be analysed (missing audio, or too long)", c.name)));
        }
        // The clip's window of its source, with a handle each side (as Clean Vocal's Commit).
        let total = Samples::try_from(s.pool.get(source).map_or(0, |a| a.frames())).unwrap_or(Samples::MAX);
        let handle = s.sample_rate.samples(crate::cmd::clip_clean::COMMIT_HANDLE_SECS);
        let offset = c.source_offset();
        let a = offset.saturating_sub(handle).clamp(0, total);
        let b = offset.saturating_add(c.length).saturating_add(1).saturating_add(handle).clamp(a, total);
        let len = usize::try_from(b - a).unwrap_or(0);
        crate::io::refuse_longer(s, COMMIT, &c.name, "render", b - a, crate::io::render_limit(2))?;
        let audio = soundcraft_mix::clip_source_window(s, &c, a, len)
            .ok_or_else(|| bad(COMMIT, format!("`{}`: its audio is missing, so there is nothing to commit", c.name)))?;
        let name = format!("{}-tuned", c.name);
        let s = e.session_mut();
        let src = crate::io::add_source(s, &name, AudioBuffer { sample_rate: rate, channels: audio }, None, FileFormat::Wav);
        if let Some(cm) = s.find_clip_mut(id) {
            if let ClipContent::Audio { source, offset } = &mut cm.content {
                *source = src;
                *offset -= a;
            }
            soundcraft_model::rebase_lyrics(cm, a);
            cm.tune = None;
            cm.clean = None;
        }
        out.push(json!({"clip": id.0, "source": src.0}));
    }
    Ok(json!({"clips": out}))
}

const ID: &str = "track.vocal";

fn vocal_flag(e: &mut Engine, p: &Value) -> Result<Value> {
    let tracks = tracks_required(e, ID, p)?;
    let value = match p.get("value") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| bad(ID, "`value` must be true or false"))?),
    };
    let auto = match p.get("auto") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(ID, "`auto` must be true"))?,
    };
    if auto && value.is_some() {
        return Err(bad(ID, "pass `value` or `auto`, not both"));
    }
    // Only audio tracks can be vocal: the others are neither counted nor given a flag.
    let audio: Vec<TrackId> = tracks.into_iter().filter(|t| e.session().track(*t).is_some_and(|tr| tr.kind == TrackKind::Audio)).collect();
    if audio.is_empty() {
        return Err(bad(ID, "choose an audio track"));
    }
    let s = e.session_mut();
    let set = if auto { None } else { Some(value.unwrap_or_else(|| !audio.iter().all(|t| s.track(*t).is_some_and(|tr| tr.is_vocal())))) };
    let mut out = Vec::with_capacity(audio.len());
    for t in &audio {
        if let Some(tr) = s.track_mut(*t) {
            tr.vocal = set;
            out.push(tr.is_vocal());
        }
    }
    Ok(json!({"tracks": audio, "vocal": out}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{AnalysisKey, ChannelFormat, ClipAnalysis, SourceId};
    use soundcraft_vocal::{PitchTrack, VocalAnalysis};
    use std::sync::Arc;

    #[test]
    fn track_vocal_toggles_sets_and_follows_the_name_again() {
        let mut e = Engine::default();
        let s = e.session_mut();
        let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let a1 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Audio 1"));
        let track = |e: &Engine, t| e.session().track(t).map(|t| (t.vocal, t.is_vocal()));
        assert_eq!(track(&e, vox), Some((None, true)));

        let r = e.execute(ID, &json!({"tracks": [vox.0]})).unwrap();
        assert_eq!(r["vocal"], json!([false]));
        assert_eq!(track(&e, vox), Some((Some(false), false)));
        assert!(e.undo());
        assert_eq!(track(&e, vox), Some((None, true)), "undo: follows the name again");

        let r = e.execute(ID, &json!({"tracks": ["Audio 1"], "value": true})).unwrap();
        assert_eq!(r, json!({"tracks": [a1.0], "vocal": [true]}));
        assert_eq!(track(&e, a1), Some((Some(true), true)));
        e.execute(ID, &json!({"tracks": [a1.0], "auto": true})).unwrap();
        assert_eq!(track(&e, a1), Some((None, false)), "auto: back to the name");

        assert!(e.execute(ID, &json!({"tracks": [a1.0], "value": "yes"})).is_err());
        assert!(e.execute(ID, &json!({"tracks": [a1.0], "value": true, "auto": true})).is_err());
        assert!(e.execute(ID, &json!({"tracks": [a1.0], "auto": 1})).is_err());
    }

    #[test]
    fn undo_keeps_the_vocal_cache() {
        let mut e = Engine::default();
        let vox = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        e.execute(ID, &json!({"tracks": [vox.0]})).unwrap();
        // An analysis arrives after the edit, as a background job's would.
        let key = AnalysisKey { source: SourceId(9), clean: None, rate: 48_000 };
        let analysis = VocalAnalysis {
            track: PitchTrack { sample_rate: 48_000, hop_samples: 240, frames: Vec::new() },
            notes: Vec::new(),
            unvoiced: Vec::new(),
        };
        let h = key.hash();
        e.session_mut().vocal.analyses = Arc::new([(h, Arc::new(ClipAnalysis { key, analysis }))].into_iter().collect());
        assert!(e.undo());
        assert!(e.session().vocal.analyses.contains_key(&h), "undo keeps the derived cache");
        assert_eq!(e.session().track(vox).and_then(|t| t.vocal), None, "and still undoes the edit");
        assert!(e.redo());
        assert!(e.session().vocal.analyses.contains_key(&h), "redo keeps it too");
        assert_eq!(e.session().track(vox).and_then(|t| t.vocal), Some(false));
        assert!(e.undo_discard());
        assert!(e.session().vocal.analyses.contains_key(&h), "undo_discard keeps it too");
        assert_eq!(e.session().track(vox).and_then(|t| t.vocal), None);
    }

    #[test]
    fn track_vocal_acts_on_audio_tracks_only() {
        let mut e = Engine::default();
        let s = e.session_mut();
        let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let keys = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
        let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Vox Verb"));
        let flag = |e: &Engine, t| e.session().track(t).map(|t| (t.vocal, t.is_vocal()));
        // A mixed selection toggles off and on again: the MIDI and aux tracks don't count.
        let r = e.execute(ID, &json!({"tracks": [vox.0, keys.0, aux.0]})).unwrap();
        assert_eq!(r, json!({"tracks": [vox.0], "vocal": [false]}));
        assert_eq!(flag(&e, vox), Some((Some(false), false)));
        let r = e.execute(ID, &json!({"tracks": [vox.0, keys.0, aux.0]})).unwrap();
        assert_eq!(r, json!({"tracks": [vox.0], "vocal": [true]}));
        assert_eq!(flag(&e, vox), Some((Some(true), true)));
        e.execute(ID, &json!({"tracks": [vox.0, keys.0], "value": true})).unwrap();
        e.execute(ID, &json!({"tracks": [vox.0, keys.0], "auto": true})).unwrap();
        // The others are never given a flag (it would be saved).
        assert_eq!(flag(&e, keys), Some((None, false)));
        assert_eq!(flag(&e, aux), Some((None, false)));
        // No audio track at all: refused, nothing changes.
        let err = e.execute(ID, &json!({"tracks": [keys.0, aux.0], "value": true})).unwrap_err();
        assert!(err.to_string().contains("choose an audio track"), "{err}");
        assert_eq!(flag(&e, keys), Some((None, false)));
    }

    // ---- Tuned/Original, Commit, Remove ----------------------------------------------------

    const RATE: u32 = 48_000;

    /// `secs` of a formant-shaped harmonic series at `f0`.
    fn vowel(f0: f64, secs: f64) -> Vec<f32> {
        let n = (secs * f64::from(RATE)) as usize;
        let amp = |k: usize| {
            let f = f0 * k as f64;
            let res = |c: f64, b: f64, a: f64| a / (1.0 + ((f - c) / b).powi(2));
            res(700.0, 100.0, 1.0) + res(1200.0, 110.0, 0.6) + res(2600.0, 160.0, 0.3) + 0.01 / k as f64
        };
        let ks: Vec<(usize, f64)> = (1..=60).filter(|k| f0 * (*k as f64) < f64::from(RATE) / 2.0).map(|k| (k, amp(k))).collect();
        let norm: f64 = ks.iter().map(|(_, a)| a).sum();
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                (0.3 * ks.iter().map(|(k, a)| a * (std::f64::consts::TAU * f0 * *k as f64 * t).sin()).sum::<f64>() / norm) as f32
            })
            .collect()
    }

    /// 0.5 s at 220 Hz −30 c, 0.2 s of silence, 0.5 s at 247 Hz +20 c.
    fn take() -> Vec<f32> {
        let mut x = vowel(220.0 * 2f64.powf(-30.0 / 1200.0), 0.5);
        x.extend(std::iter::repeat_n(0.0, 9_600));
        x.extend(vowel(247.0 * 2f64.powf(20.0 / 1200.0), 0.5));
        x
    }

    const START: Samples = 1_000;

    /// A "Vox" track with clip "Lead" of the whole take at `START`; Clean Vocal at 60 % when
    /// `clean`.
    fn tuned_session(clean: bool) -> (Engine, TrackId, ClipId) {
        use soundcraft_audio_io::{AudioBuffer, FileFormat};
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let x = take();
        let n = x.len() as Samples;
        let src = crate::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![x.clone()] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        let mut clip = soundcraft_model::Clip::audio(id, "Lead", src, 0, START, n);
        if clean {
            let cleaned: Vec<f32> = x.iter().map(|v| 0.5 * v + 0.2 * v * v.abs()).collect();
            let k = crate::io::add_source(&mut s, "Vox-clean", AudioBuffer { sample_rate: RATE, channels: vec![cleaned] }, None, FileFormat::Wav);
            clip.clean = Some(soundcraft_model::CleanAlt { source: k, original: src, origin: 0, mix: 0.6, bypass: false });
        }
        crate::edit::place_clip(&mut s, t, clip);
        let mut e = Engine::new(s);
        e.execute(ANALYSE, &json!({"clip": id.0})).unwrap();
        let note = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes[0].id;
        e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": note.0}], "centre": 100})).unwrap();
        e.execute(ANALYSE, &json!({"clip": id.0})).unwrap();
        (e, t, id)
    }

    fn clip_of(e: &Engine, id: ClipId) -> soundcraft_model::Clip {
        e.session().find_clip(id).map(|(_, c)| c.clone()).unwrap()
    }

    fn bounce(e: &Engine, t: TrackId) -> Vec<f32> {
        let end = START + take().len() as Samples + 2_000;
        soundcraft_mix::render_clips(e.session(), t, Range::new(0, end)).into_iter().next().unwrap()
    }

    #[test]
    fn commit_bakes_tuning_and_clean_into_one_source() {
        let (mut e, t, id) = tuned_session(true);
        let before = bounce(&e, t);
        let untuned = {
            let mut s = e.session().clone();
            s.find_clip_mut(id).unwrap().tune = None;
            soundcraft_mix::render_clips(&s, t, Range::new(0, before.len() as Samples)).into_iter().next().unwrap()
        };
        assert_ne!(before, untuned, "the clip plays tuned");
        let old = clip_of(&e, id);
        e.execute(COMMIT, &json!({"clip": id.0})).unwrap();
        let c = clip_of(&e, id);
        assert!(c.tune.is_none() && c.clean.is_none(), "tuning and Clean Vocal baked in");
        let src = c.source().unwrap();
        assert_ne!(Some(src), old.source());
        assert_eq!(e.session().sources.iter().find(|x| x.id == src).map(|x| x.name.as_str()), Some("Lead-tuned"));
        let after = bounce(&e, t);
        assert!(before.iter().zip(&after).all(|(a, b)| a.to_bits() == b.to_bits()), "the bounce is unchanged, bit for bit");
        assert_eq!(e.undo_label(), Some("Commit Tuning"));
        assert!(e.undo());
        let c = clip_of(&e, id);
        assert_eq!(c.tune, old.tune, "undo restores the tuning");
        assert_eq!(c.clean, old.clean, "and Clean Vocal");
        assert_eq!(bounce(&e, t), before, "the cached render plays again at once");
    }

    #[test]
    fn remove_drops_tuning_and_undo_restores() {
        let (mut e, t, id) = tuned_session(false);
        let tuned = bounce(&e, t);
        let old = clip_of(&e, id).tune;
        e.execute(REMOVE, &json!({"clip": id.0})).unwrap();
        assert!(clip_of(&e, id).tune.is_none());
        assert_ne!(bounce(&e, t), tuned, "plays untuned");
        assert_eq!(e.undo_label(), Some("Remove Tuning"));
        assert!(e.undo());
        assert_eq!(clip_of(&e, id).tune, old);
        assert_eq!(bounce(&e, t), tuned, "the cached render plays again at once");
    }

    #[test]
    fn toggle_flips_and_keeps_edits() {
        let (mut e, t, id) = tuned_session(false);
        let tuned = bounce(&e, t);
        let edits = clip_of(&e, id).tune.unwrap().edits;
        e.execute(TOGGLE, &json!({"clip": id.0})).unwrap();
        let tune = clip_of(&e, id).tune.unwrap();
        assert!(tune.original);
        assert_eq!(tune.edits, edits, "edits kept");
        assert_ne!(bounce(&e, t), tuned);
        assert_eq!(e.undo_label(), Some("Tuned/Original"));
        e.execute(TOGGLE, &json!({"track": "Vox"})).unwrap();
        assert!(!clip_of(&e, id).tune.unwrap().original);
        assert_eq!(bounce(&e, t), tuned, "back to Tuned, no re-render needed");
    }

    #[test]
    fn commit_on_original_only_drops_tuning() {
        let (mut e, _, id) = tuned_session(true);
        let old = clip_of(&e, id);
        e.execute(TOGGLE, &json!({"clip": id.0})).unwrap();
        e.execute(COMMIT, &json!({"clip": id.0})).unwrap();
        let c = clip_of(&e, id);
        assert!(c.tune.is_none());
        assert_eq!(c.source(), old.source(), "no new source");
        assert_eq!(c.clean, old.clean, "Clean Vocal stays as it was");
    }

    #[test]
    fn commands_refuse_untuned_clips() {
        let (mut e, _, id) = tuned_session(false);
        e.execute(REMOVE, &json!({"clip": id.0})).unwrap();
        for cmd in [TOGGLE, COMMIT, REMOVE] {
            let err = e.execute(cmd, &json!({"clip": id.0})).unwrap_err();
            assert!(err.to_string().contains("choose a clip tuned by Tune Vocal"), "{cmd}: {err}");
        }
        assert!(e.execute(TOGGLE, &json!({"clip": 9999})).is_err());
    }

    #[test]
    fn reset_keeps_a_snap_only_edit() {
        let (mut e, _, id) = tuned_session(false);
        let notes = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes;
        let n1 = notes[1].id;
        e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": n1.0}], "snap_to_key": true})).unwrap();
        let n0 = notes[0].id;
        e.execute(RESET, &json!({"notes": [{"clip": id.0, "note": n0.0}]})).unwrap();
        let tune = clip_of(&e, id).tune.expect("a Snap to key edit keeps the tuning");
        assert!(tune.edits.get(&n1).is_some_and(|ed| ed.snap_to_key));
        e.execute(RESET, &json!({"notes": [{"clip": id.0, "note": n1.0}]})).unwrap();
        assert!(clip_of(&e, id).tune.is_none(), "no edit left: the tuning goes");
    }

    #[test]
    fn note_commands_refuse_stretched_clips() {
        let (mut e, _, id) = tuned_session(false);
        let note = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes[0].id;
        e.session_mut().find_clip_mut(id).unwrap().stretch = 1.5;
        let err = e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": note.0}], "centre": 50})).unwrap_err();
        assert!(err.to_string().contains("commit the stretch first"), "{err}");
    }

    #[test]
    fn macro_refuses_snap_and_transition() {
        let (mut e, _, id) = tuned_session(false);
        let note = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes[0].id;
        for extra in [json!({"snap_to_key": true}), json!({"transition": "smooth"})] {
            let mut p = json!({"notes": [{"clip": id.0, "note": note.0}], "preset": "Firm"});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            assert!(e.execute(MACRO, &p).is_err(), "{p}");
        }
    }

    // ---- the manual pitch offset (dragging notes) ------------------------------------------

    fn offset_of(e: &Engine, id: ClipId, n: NoteId) -> f32 {
        clip_of(e, id).tune_live().and_then(|t| t.edits.get(&n).map(|ed| ed.offset_cents)).unwrap_or(0.0)
    }

    #[test]
    fn edit_sets_and_steps_the_offset() {
        let (mut e, _, id) = tuned_session(false);
        let notes = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes;
        let (n0, n1) = (notes[0].id, notes[1].id);
        let both = json!([{"clip": id.0, "note": n0.0}, {"clip": id.0, "note": n1.0}]);
        e.execute(EDIT, &json!({"notes": both, "offset": 200})).unwrap();
        assert_eq!((offset_of(&e, id, n0), offset_of(&e, id, n1)), (200.0, 200.0));
        assert_eq!(e.undo_label(), Some("Tune Vocal Edit"));
        let centre = clip_of(&e, id).tune_live().unwrap().edits[&n0].centre;
        assert_eq!(centre, 1.0, "the other fields stay as they are");
        e.execute(EDIT, &json!({"notes": both, "offset_by": -35})).unwrap();
        assert_eq!((offset_of(&e, id, n0), offset_of(&e, id, n1)), (165.0, 165.0));
        e.execute(EDIT, &json!({"notes": both, "offset_by": 4000})).unwrap();
        assert_eq!(offset_of(&e, id, n0), 2400.0, "clamped to two octaves");
        // Per-note offsets (a snapped drag of several notes) override the shared one.
        e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": n0.0, "offset": 241}, {"clip": id.0, "note": n1.0}], "offset": -20})).unwrap();
        assert_eq!((offset_of(&e, id, n0), offset_of(&e, id, n1)), (241.0, -20.0));
        e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": n1.0, "offset": 0}]})).unwrap();
        assert_eq!(offset_of(&e, id, n1), 0.0, "a per-note offset alone is a value to set");
        // Every field takes a per-note value (the note bar's multi-note drag), in one step.
        let steps = e.undo_history().len();
        e.execute(
            EDIT,
            &json!({"notes": [{"clip": id.0, "note": n0.0, "drift": 30, "gain_db": -2}, {"clip": id.0, "note": n1.0, "drift": 50, "vibrato": -40, "gain_db": 1.5}], "drift": 10}),
        )
        .unwrap();
        let ed = |e: &Engine, n: NoteId| clip_of(e, id).tune_live().unwrap().edits[&n].clone();
        assert_eq!((ed(&e, n0).drift, ed(&e, n0).gain_db, ed(&e, n0).vibrato), (0.3, -2.0, 0.0));
        assert_eq!((ed(&e, n1).drift, ed(&e, n1).gain_db, ed(&e, n1).vibrato), (0.5, 1.5, -0.4));
        assert_eq!(e.undo_history().len(), steps + 1);
        let err = e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": n0.0, "drift": 130}]})).unwrap_err();
        assert!(err.to_string().contains("`drift` must be a number from 0 to 100"), "{err}");
        e.execute(EDIT, &json!({"notes": [{"clip": id.0, "note": n0.0, "raise": 50}]})).unwrap();
        assert!(ed(&e, n0).typical_db.is_some(), "a per-note raise sets the levelling reference");
        // vocal.notes judges a moved note against the semitone it was moved to.
        let q = e.execute(NOTES, &json!({"clip": id.0})).unwrap();
        let first = &q["notes"][0];
        let (target, tuned) = (first["target"].as_f64().unwrap(), first["tuned_offset"].as_f64().unwrap());
        // Centred (100 %) on 5700, then moved 241 c: 41 c sharp of 5900.
        assert!((tuned - 41.0).abs() < 1.0 && (first["edit"]["offset_cents"].as_f64().unwrap() - 241.0).abs() < 1e-6, "{first}");
        assert_eq!(target, 5700.0, "the sung target is reported as it was");
    }

    #[test]
    fn edit_refuses_bad_offsets() {
        let (mut e, _, id) = tuned_session(false);
        let note = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes[0].id;
        let notes = json!([{"clip": id.0, "note": note.0}]);
        for (p, why) in [
            (json!({"notes": notes, "offset": 2401}), "`offset` must be a number of cents from -2400 to 2400"),
            (json!({"notes": notes, "offset": "up"}), "`offset` must be a number"),
            (json!({"notes": notes, "offset": null}), "`offset` must be a number"),
            (json!({"notes": notes, "offset_by": -4801}), "`offset_by` must be a number of cents from -4800 to 4800"),
            (json!({"notes": notes, "offset": 100, "offset_by": 100}), "not both"),
            (json!({"notes": [{"clip": id.0, "note": note.0, "offset": 9999}]}), "`offset` must be"),
        ] {
            let before = clip_of(&e, id).tune;
            let err = e.execute(EDIT, &p).unwrap_err();
            assert!(err.to_string().contains(why), "{p}: {err}");
            assert_eq!(clip_of(&e, id).tune, before, "nothing changes");
        }
        let p = json!({"notes": notes, "preset": "Firm", "offset": 100});
        assert!(e.execute(MACRO, &p).unwrap_err().to_string().contains("a macro doesn't set `offset`"));
    }

    #[test]
    fn edit_sets_the_manual_gain() {
        let (mut e, _, id) = tuned_session(false);
        let an = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap();
        let n1 = &an.notes[1];
        let notes = json!([{"clip": id.0, "note": n1.id.0}]);
        e.execute(EDIT, &json!({"notes": notes, "gain_db": 4.5})).unwrap();
        let ed = clip_of(&e, id).tune_live().unwrap().edits[&n1.id].clone();
        assert_eq!(ed.gain_db, 4.5);
        assert_eq!(ed.centre, 0.0, "the other fields stay as they are");
        // The curves carry it as a flat gain over the note, and vocal.notes reports the level.
        let c = crate::vocal::curves_for(e.session(), &clip_of(&e, id)).unwrap();
        let mid = (n1.start + n1.end) / 2;
        assert!((c.gain_db[mid as usize] - 4.5).abs() < 1e-6, "{}", c.gain_db[mid as usize]);
        let q = e.execute(NOTES, &json!({"clip": id.0})).unwrap();
        let row = &q["notes"][1];
        let (sung, tuned) = (row["level_db"].as_f64().unwrap(), row["tuned_level_db"].as_f64().unwrap());
        assert!((tuned - sung - 4.5).abs() < 1e-4, "{row}");
        assert!((row["edit"]["gain_db"].as_f64().unwrap() - 4.5).abs() < 1e-6);
        // Clamped: out of range, not a number, and macros refuse it.
        for p in [json!({"notes": notes, "gain_db": 12.5}), json!({"notes": notes, "gain_db": "loud"}), json!({"notes": notes, "gain_db": null})] {
            let err = e.execute(EDIT, &p).unwrap_err();
            assert!(err.to_string().contains("`gain_db` must be a number of dB from -12 to 12"), "{p}: {err}");
        }
        let p = json!({"notes": notes, "preset": "Firm", "gain_db": 1});
        assert!(e.execute(MACRO, &p).unwrap_err().to_string().contains("a macro doesn't set `gain_db`"));
        // It round-trips through a saved session; Reset clears it.
        let text = e.session().to_json().unwrap();
        let back = Session::from_json(&text).unwrap();
        assert_eq!(back.find_clip(id).and_then(|(_, c)| c.tune_live().cloned()).unwrap().edits[&n1.id].gain_db, 4.5);
        e.execute(RESET, &json!({"notes": notes})).unwrap();
        assert!(clip_of(&e, id).tune_live().is_none_or(|t| !t.edits.contains_key(&n1.id)));
    }

    #[test]
    fn offset_plays_saves_and_resets() {
        let (mut e, t, id) = tuned_session(false);
        let note = crate::vocal::analysis_for(e.session(), &clip_of(&e, id)).unwrap().notes[1].id;
        let notes = json!([{"clip": id.0, "note": note.0}]);
        let before = bounce(&e, t);
        e.execute(EDIT, &json!({"notes": notes, "offset": 100})).unwrap();
        e.execute(ANALYSE, &json!({"clip": id.0})).unwrap();
        assert_ne!(bounce(&e, t), before, "the moved note plays moved");
        // It round-trips through a saved session.
        let text = e.session().to_json().unwrap();
        let back = Session::from_json(&text).unwrap();
        let tune = back.find_clip(id).and_then(|(_, c)| c.tune_live().cloned()).unwrap();
        assert_eq!(tune.edits[&note].offset_cents, 100.0);
        // Reset clears it, and with no edit left the tuning goes.
        e.execute(RESET, &json!({"notes": notes})).unwrap();
        assert_eq!(offset_of(&e, id, note), 0.0);
    }
}

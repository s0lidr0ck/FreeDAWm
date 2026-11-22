//! The Vocal Editor's notes as MIDI: AI › Tune Vocal › Notes to MIDI Track (a new instrument
//! track under the vocal, one MIDI clip per tuned clip) and Export Notes as MIDI File... (the same
//! notes in a standard .mid file).
//!
//! The notes are the editor's own: its splits and joins, each note at the pitch it is tuned to
//! (its target after Snap to key and the session's key changes, moved by the whole semitones the
//! user dragged it, plus the clip's transpose) rounded to the nearest semitone, so what the
//! instrument plays is what the tuned vocal sings. MIDI numbers are true pitch (A4 = 440 Hz = 69;
//! the app names 60 C3). Times follow the clip's window, offset and varispeed onto the session's
//! tempo map, so the notes stay with the audio across tempo changes.
//!
//! Velocity follows the note's level as heard (its sung level plus the editor's level change)
//! against the median level of every note on its track: the median sings at 90, each dB above
//! or below moves it 4, kept within 40–120 (so about −12 dB … +7.5 dB spans the range). A note
//! with no measurable level gets 96.

use super::*;
use crate::cmd;
use crate::vocal::{FrameMap, NoteRef};
use serde_json::json;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{AnalysisKey, Clip, ClipId, Session, TrackId};
use soundcraft_vocal::{tuned_target_with, typical_level_db};
use std::collections::{BTreeMap, BTreeSet};

pub const TO_TRACK: &str = "vocal.to_midi_track";
pub const EXPORT: &str = "vocal.export_midi";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "vocal.to_midi_track",
            "Notes to MIDI Track",
            [],
            None,
            "{clip? | clips? | track?, notes?: [{clip, note}]} — the Vocal Editor's notes (`notes`: only those; else every note of the targeted audio clips: default the selected clips, else the selected tracks' audio clips; a track: all its clips) as MIDI on a new instrument track `<track> MIDI` directly under each vocal track (the subtractive synth), one MIDI clip per vocal clip at its position and length. A note per vocal note: its tuned pitch (target, Snap to key, dragged semitones and the clip's transpose) rounded to the nearest semitone, its start and end on the tempo map, velocity from its level against the track's median (median 90, 4 per dB, 40–120; 96 without a level). Same-pitch notes that touch or overlap are cut just before the next (a sliver under 10 ms joins its same-pitch neighbour); notes are at least 10 ms. Clips not analysed yet, or stretched in a way tuning can't follow, are skipped and listed. One undo step. Returns {tracks: [{from, track, clips, notes}], notes, skipped: [{clip, why}]}",
            has_vocal_notes,
            to_track
        ),
        cmd!(
            noundo "vocal.export_midi",
            "Export Notes as MIDI File...",
            [],
            None,
            "{path, clip? | clips? | track?, notes?: [{clip, note}]} — the same notes Notes to MIDI Track makes, written to a standard MIDI file (format 1, one track per vocal track named `<track> MIDI`, the session's tempo map, time signatures and markers), at their song position (bar 1 is the file's start). Returns {path, bytes, notes, skipped}",
            has_vocal_notes,
            export
        ),
    ]
}

/// Shortest note written (seconds): a split sliver or a same-pitch neighbour never makes a
/// zero-length or inaudible note.
const MIN_NOTE_SECS: f64 = 0.010;
/// Velocity of a note at the track's median level, the step per dB and the range.
const MEDIAN_VELOCITY: f32 = 90.0;
const VELOCITY_PER_DB: f32 = 4.0;
const VELOCITY_RANGE: (f32, f32) = (40.0, 120.0);
/// Velocity of a note without a measurable level.
const DEFAULT_VELOCITY: u8 = 96;

/// Why the commands are greyed out.
const NO_TARGET: &str = "select a sung clip or its track";
const NO_NOTES: &str = "no detected notes: open the Vocal Editor (AI › Tune Vocal) on a sung clip and let it analyse";
/// Why a clip is skipped.
const STRETCHED: &str = "tuning can't follow this stretch (warp markers, a reverse or a loop): commit the stretch first";
const NOT_ANALYSED: &str = "not analysed yet";

/// The audio clips the commands act on with nothing named: the selected clips, else the selected
/// tracks' audio clips.
fn selected_audio(s: &Session) -> Vec<&Clip> {
    let clips: Vec<&Clip> = s.edit.selected_clips.iter().filter_map(|id| s.find_clip(*id).map(|(_, c)| c)).filter(|c| c.is_audio()).collect();
    if !clips.is_empty() {
        return clips;
    }
    s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id)).flat_map(|t| t.clips().iter().filter(|c| c.is_audio())).collect()
}

/// Whether `c` has analysed notes (cheap: looks at the cached analysis, copies nothing).
fn has_notes(s: &Session, c: &Clip) -> bool {
    AnalysisKey::of(s, c).and_then(|k| s.vocal.analyses.get(&k.hash())).is_some_and(|a| !a.analysis.notes.is_empty())
}

/// Greyed out until the selection holds a clip with detected notes.
pub fn has_vocal_notes(e: &Engine) -> std::result::Result<(), String> {
    let s = e.session();
    let clips = selected_audio(s);
    if clips.is_empty() {
        Err(NO_TARGET.into())
    } else if clips.iter().any(|c| has_notes(s, c)) {
        Ok(())
    } else {
        Err(NO_NOTES.into())
    }
}

/// Whether `track` has any detected notes (the Vocal Editor's MIDI menu asks).
pub fn track_has_notes(s: &Session, track: TrackId) -> bool {
    s.track(track).is_some_and(|t| t.clips().iter().any(|c| c.is_audio() && has_notes(s, c)))
}

/// One vocal clip's notes as a MIDI clip: where it goes and what it plays.
struct PlannedClip {
    name: String,
    start: Samples,
    len: Samples,
    seq: Sequence,
}

/// Everything one vocal track gives.
struct PlannedTrack {
    from: TrackId,
    clips: Vec<PlannedClip>,
}

/// What both commands write: per vocal track (in session order) its MIDI clips, and the clips
/// skipped with why.
struct Plan {
    tracks: Vec<PlannedTrack>,
    notes: usize,
    skipped: Vec<Value>,
}

/// The clips (and, with `notes`, only those notes) `p` names.
fn targets(e: &Engine, id: &str, p: &Value) -> Result<(Vec<ClipId>, Option<BTreeSet<NoteRef>>)> {
    if let Some(arr) = p.get("notes") {
        let arr = arr.as_array().ok_or_else(|| bad(id, "`notes` must be a list of {clip, note}"))?;
        let mut refs = BTreeSet::new();
        for v in arr {
            let clip = v.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(id, "each note is {clip, note}"))?;
            let note = v
                .get("note")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .map(soundcraft_vocal::NoteId)
                .ok_or_else(|| bad(id, "each note is {clip, note}"))?;
            refs.insert(NoteRef { clip, note });
        }
        if !refs.is_empty() {
            let mut clips: Vec<ClipId> = refs.iter().map(|r| r.clip).collect();
            clips.dedup();
            for c in &clips {
                match e.session().find_clip(*c) {
                    None => return Err(bad(id, format!("no clip with id {}", c.0))),
                    Some((_, x)) if !x.is_audio() => return Err(bad(id, format!("`{}` is not an audio clip", x.name))),
                    Some(_) => {}
                }
            }
            return Ok((clips, Some(refs)));
        }
    }
    Ok((super::vocal::audio_targets(e, id, p)?, None))
}

/// The track's median note level (dB), over every analysed note of its audio clips as heard.
fn track_median_db(s: &Session, track: TrackId) -> f32 {
    let mut levels = Vec::new();
    let none = BTreeMap::new();
    for c in s.track(track).map(|t| t.clips()).unwrap_or_default() {
        let Some(a) = crate::vocal::analysis_for(s, c) else { continue };
        let m = FrameMap::of(c, &a);
        let edits = c.tune_live().map_or(&none, |t| &t.edits);
        for n in a.notes.iter().filter(|n| m.in_window(n.start, n.end)) {
            levels.push(n.level_db + edits.get(&n.id).map_or(0.0, |ed| ed.clone().clamped().gain_db));
        }
    }
    typical_level_db(&levels)
}

/// A note's velocity from its level `db` against the track's `median` (see the module docs).
pub fn velocity_of(db: f32, median: f32) -> u8 {
    if !db.is_finite() || !median.is_finite() || median <= -119.0 {
        return DEFAULT_VELOCITY;
    }
    (MEDIAN_VELOCITY + VELOCITY_PER_DB * (db - median)).round().clamp(VELOCITY_RANGE.0, VELOCITY_RANGE.1) as u8
}

/// One vocal note on the session timeline: `[start, end)` in session samples.
struct Sung {
    start: Samples,
    end: Samples,
    pitch: u8,
    velocity: u8,
}

/// `c`'s notes (those in `only` when given) on the session timeline, cut to the clip's window.
fn clip_notes(s: &Session, c: &Clip, a: &soundcraft_vocal::VocalAnalysis, only: Option<&BTreeSet<NoteRef>>, median: f32) -> Vec<Sung> {
    let none = BTreeMap::new();
    let m = FrameMap::of(c, a);
    let scale = crate::vocal::scale_at(s, c);
    let mode = crate::vocal::note_mode(c);
    let edits = c.tune_live().map_or(&none, |t| &t.edits);
    let (lo, hi) = (c.start, c.end());
    let mut out = Vec::new();
    for n in a.notes.iter().filter(|n| m.in_window(n.start, n.end)) {
        if only.is_some_and(|o| !o.contains(&NoteRef { clip: c.id, note: n.id })) {
            continue;
        }
        let applied = edits.get(&n.id).cloned().unwrap_or_default().clamped();
        // What the note sings after tuning: its target (Snap to key, the key in force) moved by
        // the whole semitones it was dragged, then the clip's transpose.
        let cents = f64::from(tuned_target_with(a, n, &applied, &scale, mode)) + f64::from(c.transpose_cents);
        if !cents.is_finite() {
            continue;
        }
        let pitch = (cents / 100.0).round().clamp(0.0, 127.0) as u8;
        let (start, end) = (m.time(n.start).max(lo), m.time(n.end).min(hi));
        if end <= start {
            continue;
        }
        let db = n.level_db + applied.gain_db;
        out.push(Sung { start, end, pitch, velocity: velocity_of(db, median) });
    }
    out
}

/// `notes` as a MIDI clip's sequence: ticks from `clip_start` on the session's tempo map, each at
/// least [`MIN_NOTE_SECS`] long; a same-pitch note that touches or overlaps the next ends a tick
/// before it, and a same-pitch neighbour within [`MIN_NOTE_SECS`] that is shorter than that (a
/// split-off sliver) or starts too close to leave room is merged into it, so no note hangs or
/// vanishes.
fn sequence_of(s: &Session, clip_start: Samples, clip_end: Samples, notes: &[Sung]) -> Sequence {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(clip_start, sr);
    let last = s.tempo.samples_to_ticks(clip_end, sr).saturating_sub(base).max(1);
    let min = sr.samples(MIN_NOTE_SECS).max(1);
    let mut out: Vec<Note> = Vec::with_capacity(notes.len());
    let mut sorted: Vec<&Sung> = notes.iter().collect();
    sorted.sort_by_key(|n| (n.pitch, n.start));
    for n in sorted {
        let a = s.tempo.samples_to_ticks(n.start, sr).saturating_sub(base).clamp(0, last - 1);
        let end = n.end.max(n.start.saturating_add(min));
        let b = s.tempo.samples_to_ticks(end, sr).saturating_sub(base).clamp(a + 1, last);
        let min_ticks = s.tempo.samples_to_ticks(n.start.saturating_add(min), sr).saturating_sub(base).saturating_sub(a).max(1);
        let sliver = n.end.saturating_sub(n.start) < min;
        if let Some(prev) = out.last_mut().filter(|p| p.pitch == n.pitch) {
            let prev_end = prev.start + prev.length;
            // Touching: less than the shortest note between them.
            let touching = prev_end.saturating_add(min_ticks) > a;
            if touching && (sliver || a - prev.start <= min_ticks) {
                // A sliver of the same pitch, or too close to separate: one note over both.
                prev.length = prev.length.max(b - prev.start);
                prev.velocity = prev.velocity.max(n.velocity);
                continue;
            }
            if prev_end >= a {
                // Ends just before the next starts (a note-off a tick ahead of the note-on).
                prev.length = (a - 1 - prev.start).max(1);
            }
        }
        out.push(Note {
            probability: 100,
            pitch: n.pitch,
            velocity: n.velocity,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: a,
            length: b - a,
        });
    }
    let mut seq = Sequence { notes: out, ..Sequence::default() };
    seq.sort();
    seq
}

/// What `p` asks for, worked out against the session: refused when it gives no notes at all.
fn plan(e: &Engine, id: &str, p: &Value) -> Result<Plan> {
    let (clips, only) = targets(e, id, p)?;
    let s = e.session();
    let mut by_track: BTreeMap<usize, PlannedTrack> = BTreeMap::new();
    let mut medians: BTreeMap<TrackId, f32> = BTreeMap::new();
    let mut skipped = Vec::new();
    let mut total = 0;
    for cid in clips {
        let Some((t, c)) = s.find_clip(cid) else { continue };
        let Some(a) = crate::vocal::analysis_for(s, c) else {
            let why = if crate::vocal::cannot_analyse(s, c) { NO_NOTES } else { NOT_ANALYSED };
            skipped.push(json!({"clip": cid, "why": why}));
            continue;
        };
        if crate::vocal::tune_blocked(s, c) {
            skipped.push(json!({"clip": cid, "why": STRETCHED}));
            continue;
        }
        let median = *medians.entry(t).or_insert_with(|| track_median_db(s, t));
        let notes = clip_notes(s, c, &a, only.as_ref(), median);
        if notes.is_empty() {
            continue;
        }
        let seq = sequence_of(s, c.start, c.end(), &notes);
        total += seq.notes.len();
        let at = s.track_index(t).unwrap_or(usize::MAX);
        let entry = by_track.entry(at).or_insert_with(|| PlannedTrack { from: t, clips: Vec::new() });
        entry.clips.push(PlannedClip { name: format!("{} MIDI", c.name), start: c.start, len: c.length, seq });
    }
    if total == 0 {
        let why = skipped
            .first()
            .and_then(|v| v["why"].as_str())
            .filter(|w| *w != NO_NOTES)
            .map_or(NO_NOTES.to_string(), |w| format!("no notes to write: {w}"));
        return Err(bad(id, why));
    }
    Ok(Plan { tracks: by_track.into_values().collect(), notes: total, skipped })
}

/// Lays `plan` into `s`: a new instrument track under each vocal track, its clips placed.
/// Returns what was made per track.
fn lay(s: &mut Session, plan: Plan) -> Vec<(TrackId, TrackId, usize, usize)> {
    let mut made = Vec::new();
    for pt in plan.tracks {
        let Some(src) = s.track(pt.from).cloned() else { continue };
        let nt = crate::ml::new_midi_track(s, &src, crate::ml::MidiModel::General);
        if let Some(t) = s.track_mut(nt) {
            t.color = src.color;
        }
        let (mut clips, mut notes) = (0, 0);
        for pc in pt.clips {
            notes += pc.seq.notes.len();
            let cid = s.new_clip_id();
            crate::edit::place_clip(s, nt, Clip::midi(cid, pc.name, pc.start, pc.len, pc.seq));
            clips += 1;
        }
        made.push((pt.from, nt, clips, notes));
    }
    made
}

fn to_track(e: &mut Engine, p: &Value) -> Result<Value> {
    let plan = plan(e, TO_TRACK, p)?;
    let (notes, skipped) = (plan.notes, plan.skipped.clone());
    let made = lay(e.session_mut(), plan);
    let tracks: Vec<Value> = made.iter().map(|(from, t, c, n)| json!({"from": from, "track": t, "clips": c, "notes": n})).collect();
    Ok(json!({"tracks": tracks, "notes": notes, "skipped": skipped}))
}

fn export(e: &mut Engine, p: &Value) -> Result<Value> {
    let path = str_param(p, "path").ok_or_else(|| bad(EXPORT, "`path` required"))?.to_string();
    let plan = plan(e, EXPORT, p)?;
    let (notes, skipped) = (plan.notes, plan.skipped.clone());
    // The notes are laid into a copy of the session and written as File › Export › MIDI writes
    // tracks: the session's tempo map, meters and markers, song position kept.
    let mut s = e.session().clone();
    let made = lay(&mut s, plan);
    let only: Vec<TrackId> = made.iter().map(|m| m.1).collect();
    let bytes = crate::io::export_midi_of(&s, Some(&only))?;
    crate::io::write_file(&path, &bytes)?;
    Ok(json!({"path": path, "bytes": bytes.len(), "notes": notes, "skipped": skipped}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::{AudioBuffer, FileFormat};
    use soundcraft_model::{ChannelFormat, ClipContent, TrackKind};

    const RATE: u32 = 48_000;
    const START: Samples = 24_000;
    /// The sung line: A3, B3, C4, G3 (MIDI 57, 59, 60, 55), each 0.4 s with 0.2 s rests.
    const LINE: [(f64, u8); 4] = [(220.0, 57), (246.94, 59), (261.63, 60), (196.0, 55)];

    /// `secs` of a formant-shaped harmonic series at `f0` (a sung "ah").
    fn vowel(f0: f64, secs: f64, amp: f64) -> Vec<f32> {
        let n = (secs * f64::from(RATE)) as usize;
        let level = |k: usize| {
            let f = f0 * k as f64;
            let res = |c: f64, b: f64, a: f64| a / (1.0 + ((f - c) / b).powi(2));
            res(700.0, 100.0, 1.0) + res(1200.0, 110.0, 0.6) + res(2600.0, 160.0, 0.3) + 0.01 / k as f64
        };
        let ks: Vec<(usize, f64)> = (1..=60).filter(|k| f0 * (*k as f64) < f64::from(RATE) / 2.0).map(|k| (k, level(k))).collect();
        let norm: f64 = ks.iter().map(|(_, a)| a).sum();
        (0..n)
            .map(|i| {
                let t = i as f64 / f64::from(RATE);
                (amp * ks.iter().map(|(k, a)| a * (std::f64::consts::TAU * f0 * *k as f64 * t).sin()).sum::<f64>() / norm) as f32
            })
            .collect()
    }

    /// The line, sung 12 c sharp and 15 c flat in turn, the third note about 6 dB louder.
    fn line() -> Vec<f32> {
        let mut x = Vec::new();
        for (i, (f, _)) in LINE.iter().enumerate() {
            let cents: f64 = if i % 2 == 0 { 12.0 } else { -15.0 };
            x.extend(vowel(*f * 2f64.powf(cents / 1200.0), 0.4, if i == 2 { 0.6 } else { 0.3 }));
            x.extend(std::iter::repeat_n(0.0, 9_600));
        }
        x
    }

    /// A "Vox" track (with an "Other" track under it) holding clip "Lead" of the line at `START`,
    /// analysed.
    fn sung() -> (Engine, TrackId, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Other"));
        let x = line();
        let n = x.len() as Samples;
        let src = crate::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![x] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "Lead", src, 0, START, n));
        let mut e = Engine::new(s);
        e.execute("vocal.analyse", &json!({"clip": id.0})).unwrap();
        (e, t, id)
    }

    /// The editor's notes of `clip` as vocal.notes lists them: (note id, start, end).
    fn vocal_notes(e: &mut Engine, clip: ClipId) -> Vec<(u64, Samples, Samples)> {
        let q = e.execute("vocal.notes", &json!({"clip": clip.0})).unwrap();
        q["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|n| (n["note"].as_u64().unwrap(), n["start"].as_i64().unwrap(), n["end"].as_i64().unwrap()))
            .collect()
    }

    /// The MIDI notes of track `t` in session samples: (pitch, start, end, velocity).
    fn midi_notes(e: &Engine, t: TrackId) -> Vec<(u8, Samples, Samples, u8)> {
        let s = e.session();
        let mut out = Vec::new();
        for c in s.track(t).unwrap().clips() {
            let ClipContent::Midi { sequence } = &c.content else { panic!("a MIDI clip") };
            let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
            for n in &sequence.notes {
                let at = |tick: i64| s.tempo.tick_to_samples(base + tick, s.sample_rate);
                out.push((n.pitch, at(n.start), at(n.start + n.length), n.velocity));
            }
        }
        out.sort_by_key(|n| n.1);
        out
    }

    fn made_track(r: &Value) -> TrackId {
        TrackId(r["tracks"][0]["track"].as_u64().unwrap())
    }

    /// 3 ms at 48 kHz.
    const NEAR: Samples = 144;

    fn near(a: Samples, b: Samples) -> bool {
        (a - b).abs() <= NEAR
    }

    fn pitches(midi: &[(u8, Samples, Samples, u8)]) -> Vec<u8> {
        midi.iter().map(|n| n.0).collect()
    }

    fn line_pitches() -> Vec<u8> {
        LINE.iter().map(|l| l.1).collect()
    }

    #[test]
    fn notes_land_as_one_midi_note_each_under_the_vocal() {
        let (mut e, vox, clip) = sung();
        let sung = vocal_notes(&mut e, clip);
        assert_eq!(sung.len(), 4, "{sung:?}");
        let before = e.session().tracks.len();
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        assert_eq!(r["notes"], 4, "{r}");
        let t = made_track(&r);
        let s = e.session();
        assert_eq!(s.tracks.len(), before + 1);
        let tr = s.track(t).unwrap();
        assert_eq!((tr.name.as_str(), tr.kind), ("Vox MIDI", TrackKind::Instrument));
        assert!(tr.instrument.is_some(), "an instrument plays it");
        assert_eq!(s.track_index(t), s.track_index(vox).map(|i| i + 1), "directly under the vocal");
        let c = &tr.clips()[0];
        let src = s.find_clip(clip).unwrap().1;
        assert_eq!((c.start, c.length), (src.start, src.length), "the MIDI clip spans the vocal clip");
        let midi = midi_notes(&e, t);
        assert_eq!(pitches(&midi), line_pitches(), "sung slightly sharp and flat, rounded to the semitone");
        for ((_, a, b), (_, x, y, _)) in sung.iter().zip(&midi) {
            assert!(near(*a, *x) && near(*b, *y), "{a}..{b} against {x}..{y}");
        }
        // The louder third note plays harder, the rest about the median (90).
        let v: Vec<u8> = midi.iter().map(|n| n.3).collect();
        assert!(v[2] > 105 && v.iter().enumerate().all(|(i, x)| i == 2 || (78..=100).contains(x)), "{v:?}");
    }

    #[test]
    fn undo_and_redo_are_one_step() {
        let (mut e, vox, _) = sung();
        let before = serde_json::to_value(&e.session().tracks).unwrap();
        let steps = e.undo_history().len();
        e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        assert_eq!(e.undo_history().len(), steps + 1);
        assert_eq!(e.undo_label(), Some("Notes to MIDI Track"));
        let after = serde_json::to_value(&e.session().tracks).unwrap();
        assert!(e.undo());
        assert_eq!(serde_json::to_value(&e.session().tracks).unwrap(), before);
        assert!(e.redo());
        assert_eq!(serde_json::to_value(&e.session().tracks).unwrap(), after);
    }

    #[test]
    fn only_the_chosen_notes() {
        let (mut e, vox, clip) = sung();
        let sung = vocal_notes(&mut e, clip);
        let pick = json!([{"clip": clip.0, "note": sung[1].0}, {"clip": clip.0, "note": sung[3].0}]);
        let r = e.execute(TO_TRACK, &json!({"track": vox.0, "notes": pick})).unwrap();
        let midi = midi_notes(&e, made_track(&r));
        assert_eq!(pitches(&midi), vec![59, 55]);
        assert!(near(midi[0].1, sung[1].1));
    }

    #[test]
    fn a_dragged_note_exports_its_new_pitch_and_level() {
        let (mut e, vox, clip) = sung();
        let sung = vocal_notes(&mut e, clip);
        // The first dragged up a whole tone and a few cents more; the second down 3 semitones
        // and 6 dB quieter.
        e.execute("vocal.edit", &json!({"notes": [{"clip": clip.0, "note": sung[0].0}], "offset": 215})).unwrap();
        e.execute("vocal.edit", &json!({"notes": [{"clip": clip.0, "note": sung[1].0}], "offset": -300, "gain_db": -6})).unwrap();
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        let midi = midi_notes(&e, made_track(&r));
        assert_eq!(pitches(&midi), vec![59, 56, 60, 55]);
        assert!(midi[1].3 + 15 < midi[3].3, "6 dB quieter plays softer: {midi:?}");
    }

    #[test]
    fn the_clip_transpose_follows() {
        let (mut e, vox, clip) = sung();
        e.session_mut().find_clip_mut(clip).unwrap().transpose_cents = -500;
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        let midi = midi_notes(&e, made_track(&r));
        assert_eq!(pitches(&midi), vec![52, 54, 55, 50]);
    }

    #[test]
    fn trimmed_and_moved_clips_keep_their_notes_on_the_audio() {
        let (mut e, vox, clip) = sung();
        // The first note trimmed off the front (the window starts 0.55 s into the source), the
        // clip moved later.
        let cut = 26_400;
        {
            let c = e.session_mut().find_clip_mut(clip).unwrap();
            if let ClipContent::Audio { offset, .. } = &mut c.content {
                *offset = cut;
            }
            c.start = 100_000;
            c.length -= cut;
        }
        e.execute("vocal.analyse", &json!({"clip": clip.0})).unwrap();
        let sung = vocal_notes(&mut e, clip);
        assert_eq!(sung.len(), 3, "{sung:?}");
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        let midi = midi_notes(&e, made_track(&r));
        assert_eq!(pitches(&midi), vec![59, 60, 55]);
        for ((_, a, b), (_, x, y, _)) in sung.iter().zip(&midi) {
            assert!(near(*a, *x) && near(*b, *y), "{a}..{b} against {x}..{y}");
        }
        assert!(midi[0].1 >= 100_000);
    }

    #[test]
    fn a_tempo_change_keeps_the_notes_with_the_audio() {
        let (mut e, vox, clip) = sung();
        // 120 → 87 bpm from beat 2 (0.5 s), before most of the line.
        e.session_mut().tempo.set_tempo(960, 87.0).unwrap();
        let sung = vocal_notes(&mut e, clip);
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        let midi = midi_notes(&e, made_track(&r));
        assert_eq!(midi.len(), 4);
        for ((_, a, b), (_, x, y, _)) in sung.iter().zip(&midi) {
            assert!(near(*a, *x) && near(*b, *y), "{a}..{b} against {x}..{y}");
        }
    }

    #[test]
    fn same_pitch_notes_that_touch_never_overlap_or_vanish() {
        let (mut e, vox, clip) = sung();
        let sung = vocal_notes(&mut e, clip);
        // The C4 split in two: two touching notes of one pitch.
        let mid = (sung[2].1 + sung[2].2) / 2;
        e.execute("vocal.split", &json!({"clip": clip.0, "note": sung[2].0, "at": mid})).unwrap();
        // ... and a 5 ms sliver cut off the end of the G3.
        e.execute("vocal.split", &json!({"clip": clip.0, "note": sung[3].0, "at": sung[3].2 - 240})).unwrap();
        let r = e.execute(TO_TRACK, &json!({"track": vox.0})).unwrap();
        let t = made_track(&r);
        let s = e.session();
        let ClipContent::Midi { sequence } = &s.track(t).unwrap().clips()[0].content else { panic!() };
        let mut cs: Vec<_> = sequence.notes.iter().filter(|n| n.pitch == 60).collect();
        cs.sort_by_key(|n| n.start);
        assert_eq!(cs.len(), 2, "{:?}", sequence.notes);
        assert!(cs[0].start + cs[0].length < cs[1].start, "the first ends before the second starts");
        let min = s.tempo.samples_to_ticks(START + 480, s.sample_rate) - s.tempo.samples_to_ticks(START, s.sample_rate);
        assert!(sequence.notes.iter().all(|n| n.length >= min - 1), "every note at least 10 ms: {:?}", sequence.notes);
        let gs: Vec<_> = sequence.notes.iter().filter(|n| n.pitch == 55).collect();
        assert_eq!(gs.len(), 1, "a sliver too short to stand alone joins its neighbour: {gs:?}");
    }

    #[test]
    fn velocity_mapping() {
        assert_eq!(velocity_of(-20.0, -20.0), 90);
        assert_eq!(velocity_of(-17.0, -20.0), 102);
        assert_eq!(velocity_of(-23.0, -20.0), 78);
        assert_eq!(velocity_of(0.0, -20.0), 120);
        assert_eq!(velocity_of(-60.0, -20.0), 40);
        assert_eq!(velocity_of(f32::NAN, -20.0), 96);
        assert_eq!(velocity_of(-20.0, -120.0), 96, "nothing measured");
    }

    #[test]
    fn greyed_out_and_refused_without_notes() {
        let (mut e, vox, _) = sung();
        e.execute("edit.select_none", &json!({})).unwrap();
        assert_eq!(has_vocal_notes(&e), Err(NO_TARGET.to_string()));
        e.execute("edit.select", &json!({"tracks": [vox.0]})).unwrap();
        assert_eq!(has_vocal_notes(&e), Ok(()));
        assert!(track_has_notes(e.session(), vox));
        let other = e.session().track_by_name("Other").unwrap().id;
        assert!(!track_has_notes(e.session(), other));
        e.execute("edit.select", &json!({"tracks": [other.0]})).unwrap();
        assert_eq!(has_vocal_notes(&e), Err(NO_TARGET.to_string()), "a track with no audio");
        // A clip of silence, analysed: nothing sung.
        let src = crate::io::add_source(
            e.session_mut(),
            "Room",
            AudioBuffer { sample_rate: RATE, channels: vec![vec![0.0; 48_000]] },
            None,
            FileFormat::Wav,
        );
        let silent = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), other, Clip::audio(silent, "Room", src, 0, 0, 48_000));
        e.execute("vocal.analyse", &json!({"clip": silent.0})).unwrap();
        e.execute("edit.select", &json!({"tracks": [other.0]})).unwrap();
        assert_eq!(has_vocal_notes(&e), Err(NO_NOTES.to_string()));
        assert!(e.execute(TO_TRACK, &json!({"track": other.0})).unwrap_err().to_string().contains("no detected notes"));
        let other = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Empty"));
        assert!(e.execute(TO_TRACK, &json!({})).is_err());
        let err = e.execute(TO_TRACK, &json!({"track": other.0})).unwrap_err();
        assert!(err.to_string().contains("choose an audio clip"), "{err}");
    }

    #[test]
    fn the_file_round_trips_through_midi_import() {
        let (mut e, vox, clip) = sung();
        e.session_mut().tempo.set_tempo(960, 100.0).unwrap();
        let sung = vocal_notes(&mut e, clip);
        let dir = std::env::temp_dir().join(format!("sc-vocal-midi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("vox.mid");
        let tracks = e.session().tracks.len();
        let r = e.execute(EXPORT, &json!({"path": path.to_string_lossy(), "track": vox.0})).unwrap();
        assert_eq!(r["notes"], 4, "{r}");
        assert_eq!(e.session().tracks.len(), tracks, "the session is left as it was");
        let bytes = std::fs::read(&path).unwrap();
        let smf = soundcraft_midi::read_smf(&bytes).unwrap();
        assert_eq!(smf.tempos.len(), 2, "the tempo map goes with it: {:?}", smf.tempos);
        assert!(!smf.meters.is_empty(), "and the time signature");
        let made = crate::io::import_midi_bytes(&mut e, &bytes, 0, false).unwrap();
        assert_eq!(made.len(), 1);
        assert_eq!(e.session().track(made[0]).unwrap().name, "Vox MIDI", "named after the vocal");
        let midi = midi_notes(&e, made[0]);
        assert_eq!(pitches(&midi), line_pitches());
        for ((_, a, b), (_, x, y, _)) in sung.iter().zip(&midi) {
            assert!(near(*a, *x) && near(*b, *y), "{a}..{b} against {x}..{y}: at its song position");
        }
        let _ = std::fs::remove_dir_all(dir);
    }
}

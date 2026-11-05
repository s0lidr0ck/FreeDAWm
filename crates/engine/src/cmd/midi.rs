//! MIDI note editing (MIDI Editor, pencil on MIDI tracks).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::ops::VelocityOp;
use soundcraft_midi::{Note, Sequence};
use soundcraft_model::{Clip, ClipContent, ClipId};
use soundcraft_time::TICKS_PER_QUARTER;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "midi.notes", "List MIDI Notes", [], None, "{clip}", always, notes),
        cmd!(
            "midi.new_clip",
            "New MIDI Clip",
            ["Clip"],
            None,
            "{track, start?, end?|length?} (default: the edit selection, or 1 bar)",
            has_tracks,
            new_clip
        ),
        cmd!("midi.note_add", "Add Note", [], None, "{clip, pitch, start_ticks, length_ticks?: 480, velocity?: 100, channel?: 0}", always, note_add),
        cmd!("midi.note_edit", "Edit Note", [], None, "{clip, index, pitch?, start_ticks?, length_ticks?, velocity?}", always, note_edit),
        cmd!("midi.note_delete", "Delete Notes", [], None, "{clip, indices: [n]}", always, note_delete),
        cmd!("midi.notes_move", "Move Notes", [], None, "{clip, indices: [n], ticks?: 0, semitones?: 0} -> {moved, indices}", always, notes_move),
        cmd!(
            "midi.notes_resize",
            "Resize Notes",
            [],
            None,
            "{clip, indices: [n], start?: ticks (moves the start edge, the end stays), length?: ticks (moves the end edge)} -> {indices}",
            always,
            notes_resize
        ),
        cmd!(
            "midi.notes_add",
            "Add Notes",
            [],
            None,
            "{clip, notes: [{pitch, start, length, velocity?: 100, channel?: 0}]} -> {indices}",
            always,
            notes_add
        ),
        cmd!(
            "midi.notes_copy",
            "Copy Notes",
            [],
            None,
            "{clip, indices: [n], ticks?: 0, semitones?: 0} -> {indices} (the copies)",
            always,
            notes_copy
        ),
        cmd!(
            "midi.notes_set",
            "Set Notes",
            [],
            None,
            "{clip, indices: [n], pitch?, start?, length?: ticks, velocity?: 1..127, channel?: 0..15} — the same value on every picked note -> {indices}",
            always,
            notes_set
        ),
        cmd!(
            "midi.notes_split",
            "Split Notes",
            [],
            None,
            "{clip, indices: [n], at: tick from the clip start} — each picked note sounding across `at` becomes two -> {indices} (both halves)",
            always,
            notes_split
        ),
        cmd!(
            "midi.notes_join",
            "Join Notes",
            [],
            None,
            "{clip, indices: [n]} — the picked notes of one pitch and channel, one after another, become one note from the first start to the last end (the first one's velocity) -> {indices}",
            always,
            notes_join
        ),
        cmd!(
            "midi.notes_velocity",
            "Change Note Velocity",
            [],
            None,
            "{clip, indices?: [n] (default every note), set? | add? | scale? | from?, to? (a ramp over the notes' starts) | values?: [v] (one per index)} -> {changed}",
            always,
            notes_velocity
        ),
        cmd!(
            "midi.snap_to_scale",
            "Snap to Scale",
            [],
            Some("Ctrl+Shift+K"),
            "{clip | clips?, indices?: [n] (one clip; default every note), key?: 'E major' (default: the Key ruler's key at each note), mode?: nearest|up|down|chord} -> {moved, indices}",
            always,
            snap_to_scale
        ),
    ]
}

fn seq_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Sequence> {
    match e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn notes(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.notes")?;
    match e.session().find_clip(id).map(|(_, c)| &c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(json!({"notes": sequence.notes})),
        _ => Err(bad("midi.notes", "not a MIDI clip")),
    }
}

fn new_clip(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = tracks_required(e, "midi.new_clip", p)?.first().copied().ok_or_else(|| bad("midi.new_clip", "no track"))?;
    if !e.session().track(t).is_some_and(|x| x.kind.is_midi()) {
        return Err(bad("midi.new_clip", "not a MIDI or instrument track"));
    }
    let mut r = range_param(e, "midi.new_clip", p)?;
    if r.is_empty() {
        let s = e.session();
        let bar = soundcraft_time::GridValue::Note { value: soundcraft_time::NoteValue::Bar, dotted: false, triplet: false };
        let start = bar.line_at_or_before(r.start, s.sample_rate, &s.tempo, s.frame_rate);
        r = soundcraft_time::Range::new(start, bar.next_line(start, s.sample_rate, &s.tempo, s.frame_rate));
    }
    let s = e.session_mut();
    let id = s.new_clip_id();
    let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
    crate::edit::place_clip(s, t, Clip::midi(id, name, r.start, r.len(), Sequence::default()));
    Ok(json!({"clip": id, "start": r.start, "end": r.end}))
}

fn note_add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_add")?;
    let n = Note {
        pitch: i64_or(p, "pitch", 60).clamp(0, 127) as u8,
        velocity: i64_or(p, "velocity", 100).clamp(1, 127) as u8,
        release_velocity: 64,
        channel: i64_or(p, "channel", 0).clamp(0, 15) as u8,
        start: i64_or(p, "start_ticks", 0).max(0),
        length: i64_or(p, "length_ticks", TICKS_PER_QUARTER / 2).max(1),
    };
    let seq = seq_mut(e, id, "midi.note_add")?;
    seq.notes.push(n);
    seq.sort();
    let index = seq.notes.iter().position(|x| *x == n).unwrap_or(0);
    Ok(json!({"index": index, "notes": seq.notes.len()}))
}

fn note_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_edit")?;
    let idx = p.get("index").and_then(Value::as_u64).ok_or_else(|| bad("midi.note_edit", "`index` required"))? as usize;
    let seq = seq_mut(e, id, "midi.note_edit")?;
    let n = seq.notes.get_mut(idx).ok_or_else(|| bad("midi.note_edit", "index out of range"))?;
    if let Some(v) = p.get("pitch").and_then(Value::as_i64) {
        n.pitch = v.clamp(0, 127) as u8;
    }
    if let Some(v) = p.get("start_ticks").and_then(Value::as_i64) {
        n.start = v.max(0);
    }
    if let Some(v) = p.get("length_ticks").and_then(Value::as_i64) {
        n.length = v.max(1);
    }
    if let Some(v) = p.get("velocity").and_then(Value::as_i64) {
        n.velocity = v.clamp(1, 127) as u8;
    }
    let edited = *n;
    seq.sort();
    Ok(json!({"note": edited, "index": seq.notes.iter().position(|x| *x == edited)}))
}

fn indices(p: &Value) -> Vec<usize> {
    p.get("indices").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_u64).map(|x| x as usize).collect()).unwrap_or_default()
}

fn note_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.note_delete")?;
    let idx = indices(p);
    let seq = seq_mut(e, id, "midi.note_delete")?;
    let before = seq.notes.len();
    let mut i = 0;
    seq.notes.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
    Ok(json!({"deleted": before - seq.notes.len()}))
}

fn notes_move(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = clip_id(p, "midi.notes_move")?;
    let idx = indices(p);
    let dt = i64_or(p, "ticks", 0);
    let ds = i64_or(p, "semitones", 0);
    let seq = seq_mut(e, id, "midi.notes_move")?;
    for (i, n) in seq.notes.iter_mut().enumerate() {
        if idx.contains(&i) {
            n.start = n.start.saturating_add(dt).max(0);
            n.pitch = i64::from(n.pitch).saturating_add(ds).clamp(0, 127) as u8;
        }
    }
    let kept = sort_tracked(seq, &idx);
    Ok(json!({"moved": kept.len(), "indices": kept}))
}

/// Sorts `seq`'s notes; returns where the notes at `marked` (old indices) ended up.
fn sort_tracked(seq: &mut Sequence, marked: &[usize]) -> Vec<usize> {
    let mut work: Vec<(Note, bool)> = seq.notes.iter().enumerate().map(|(i, n)| (*n, marked.contains(&i))).collect();
    work.sort_by_key(|(a, _)| (a.start, a.pitch, a.channel, a.length, a.velocity));
    let kept = work.iter().enumerate().filter(|(_, (_, f))| *f).map(|(i, _)| i).collect();
    seq.notes = work.into_iter().map(|(n, _)| n).collect();
    kept
}

/// `indices` (each once, in range), or every note when the parameter is missing.
fn picked(p: &Value, len: usize) -> Vec<usize> {
    if p.get("indices").is_none() {
        return (0..len).collect();
    }
    let mut v: Vec<usize> = indices(p).into_iter().filter(|i| *i < len).collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// The most notes one `midi.notes_add` takes.
const MAX_ADD: usize = 100_000;

fn notes_resize(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_resize";
    let id = clip_id(p, C)?;
    if p.get("indices").is_none() {
        return Err(bad(C, "`indices` required"));
    }
    let ds = i64_or(p, "start", 0);
    let dl = i64_or(p, "length", 0);
    let seq = seq_mut(e, id, C)?;
    let idx = picked(p, seq.notes.len());
    for i in &idx {
        if let Some(n) = seq.notes.get_mut(*i) {
            if ds != 0 {
                // The end stays: the start moves within [0, end - 1].
                let end = n.end();
                let start = n.start.saturating_add(ds).clamp(0, end.saturating_sub(1).max(0));
                n.length = end.saturating_sub(start).max(1);
                n.start = start;
            }
            if dl != 0 {
                n.length = n.length.saturating_add(dl).max(1);
            }
        }
    }
    let kept = sort_tracked(seq, &idx);
    Ok(json!({"indices": kept}))
}

/// One note from `{pitch, start, length, velocity?, channel?}` (`start_ticks`, `length_ticks` too).
fn note_of(v: &Value) -> Option<Note> {
    let int = |a: &str| v.get(a).and_then(Value::as_i64);
    Some(Note {
        pitch: int("pitch")?.clamp(0, 127) as u8,
        velocity: int("velocity").unwrap_or(100).clamp(1, 127) as u8,
        release_velocity: 64,
        channel: int("channel").unwrap_or(0).clamp(0, 15) as u8,
        start: int("start").or_else(|| int("start_ticks"))?.max(0),
        length: int("length").or_else(|| int("length_ticks")).unwrap_or(TICKS_PER_QUARTER / 2).max(1),
    })
}

fn notes_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_add";
    let id = clip_id(p, C)?;
    let list = p.get("notes").and_then(Value::as_array).ok_or_else(|| bad(C, "`notes` (an array of {pitch, start, length}) required"))?;
    if list.len() > MAX_ADD {
        return Err(bad(C, format!("at most {MAX_ADD} notes at once")));
    }
    let mut add = Vec::with_capacity(list.len());
    for v in list {
        add.push(note_of(v).ok_or_else(|| bad(C, format!("bad note {v}: want pitch, start and length")))?);
    }
    let seq = seq_mut(e, id, C)?;
    let first = seq.notes.len();
    seq.notes.extend(add);
    let marked: Vec<usize> = (first..seq.notes.len()).collect();
    let kept = sort_tracked(seq, &marked);
    Ok(json!({"indices": kept}))
}

fn notes_copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_copy";
    let id = clip_id(p, C)?;
    let dt = i64_or(p, "ticks", 0);
    let ds = i64_or(p, "semitones", 0);
    let seq = seq_mut(e, id, C)?;
    let idx = picked(p, seq.notes.len());
    let copies: Vec<Note> = idx
        .iter()
        .filter_map(|i| seq.notes.get(*i))
        .map(|n| Note { start: n.start.saturating_add(dt).max(0), pitch: i64::from(n.pitch).saturating_add(ds).clamp(0, 127) as u8, ..*n })
        .collect();
    let first = seq.notes.len();
    seq.notes.extend(copies);
    let marked: Vec<usize> = (first..seq.notes.len()).collect();
    let kept = sort_tracked(seq, &marked);
    Ok(json!({"indices": kept}))
}

fn notes_velocity(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_velocity";
    let id = clip_id(p, C)?;
    let op = if let Some(v) = p.get("set").and_then(Value::as_i64) {
        Some(VelocityOp::Set(v.clamp(1, 127) as u8))
    } else if let Some(v) = p.get("add").and_then(Value::as_i64) {
        Some(VelocityOp::Add(v.clamp(-127, 127) as i32))
    } else if let Some(v) = p.get("scale").and_then(Value::as_f64).filter(|v| v.is_finite()) {
        Some(VelocityOp::Scale((v as f32).clamp(0.0, 10.0)))
    } else if let (Some(a), Some(b)) = (p.get("from").and_then(Value::as_i64), p.get("to").and_then(Value::as_i64)) {
        Some(VelocityOp::Ramp { from: a.clamp(1, 127) as u8, to: b.clamp(1, 127) as u8 })
    } else {
        None
    };
    // `values` pairs with `indices` as given (order kept), so it is read from them directly.
    let values: Option<Vec<(usize, i64)>> = match p.get("values") {
        None => None,
        Some(v) => {
            let vals: Vec<i64> = v.as_array().map(|a| a.iter().map(|x| x.as_i64().unwrap_or(100)).collect()).unwrap_or_default();
            let idx = indices(p);
            if p.get("indices").is_none() || vals.len() != idx.len() {
                return Err(bad(C, "`values` needs `indices` of the same length"));
            }
            Some(idx.into_iter().zip(vals).collect())
        }
    };
    if op.is_none() && values.is_none() {
        return Err(bad(C, "give set, add, scale, from and to, or values"));
    }
    let seq = seq_mut(e, id, C)?;
    let mut changed = 0;
    if let Some(values) = values {
        for (i, v) in values {
            if let Some(n) = seq.notes.get_mut(i) {
                n.velocity = v.clamp(1, 127) as u8;
                changed += 1;
            }
        }
    } else if let Some(op) = op {
        // The op works on a slice: gather the picked notes, change them, put them back.
        let at = picked(p, seq.notes.len());
        let mut work: Vec<Note> = at.iter().filter_map(|i| seq.notes.get(*i).copied()).collect();
        soundcraft_midi::ops::change_velocity(&mut work, op);
        for (i, n) in at.iter().zip(work) {
            if let Some(slot) = seq.notes.get_mut(*i) {
                slot.velocity = n.velocity;
                changed += 1;
            }
        }
    }
    // No re-sort: a note's place is set by its start and pitch, so a selection stays on its notes.
    Ok(json!({"changed": changed}))
}

fn notes_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_set";
    let id = clip_id(p, C)?;
    if p.get("indices").is_none() {
        return Err(bad(C, "`indices` required"));
    }
    let int = |k: &str| p.get(k).and_then(Value::as_i64);
    let (pitch, start, length, velocity, channel) = (int("pitch"), int("start"), int("length"), int("velocity"), int("channel"));
    if [pitch, start, length, velocity, channel].iter().all(Option::is_none) {
        return Err(bad(C, "give pitch, start, length, velocity or channel"));
    }
    let seq = seq_mut(e, id, C)?;
    let idx = picked(p, seq.notes.len());
    for i in &idx {
        if let Some(n) = seq.notes.get_mut(*i) {
            if let Some(v) = pitch {
                n.pitch = v.clamp(0, 127) as u8;
            }
            if let Some(v) = start {
                n.start = v.max(0);
            }
            if let Some(v) = length {
                n.length = v.max(1);
            }
            if let Some(v) = velocity {
                n.velocity = v.clamp(1, 127) as u8;
            }
            if let Some(v) = channel {
                n.channel = v.clamp(0, 15) as u8;
            }
        }
    }
    let kept = sort_tracked(seq, &idx);
    Ok(json!({"indices": kept}))
}

fn notes_split(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_split";
    let id = clip_id(p, C)?;
    let at = p.get("at").and_then(Value::as_i64).ok_or_else(|| bad(C, "`at` (a tick from the clip start) required"))?;
    let seq = seq_mut(e, id, C)?;
    let idx = picked(p, seq.notes.len());
    let mut marked = Vec::new();
    let mut tails = Vec::new();
    for i in &idx {
        if let Some(n) = seq.notes.get_mut(*i)
            && n.start < at
            && at < n.end()
        {
            tails.push(Note { start: at, length: n.end().saturating_sub(at).max(1), ..*n });
            n.length = at.saturating_sub(n.start).max(1);
            marked.push(*i);
        }
    }
    if tails.is_empty() {
        return Err(bad(C, "no picked note sounds across the split point"));
    }
    let first = seq.notes.len();
    seq.notes.extend(tails);
    marked.extend(first..seq.notes.len());
    let kept = sort_tracked(seq, &marked);
    Ok(json!({"indices": kept}))
}

fn notes_join(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.notes_join";
    let id = clip_id(p, C)?;
    if p.get("indices").is_none() {
        return Err(bad(C, "`indices` required"));
    }
    let seq = seq_mut(e, id, C)?;
    let mut idx = picked(p, seq.notes.len());
    // By pitch and channel, then start: each run of one pitch and channel joins into its first.
    idx.sort_by_key(|i| seq.notes.get(*i).map(|n| (n.pitch, n.channel, n.start)));
    let mut gone: Vec<usize> = Vec::new();
    let mut heads: Vec<usize> = Vec::new();
    for i in idx {
        let Some(n) = seq.notes.get(i).copied() else { continue };
        let head = heads.last().copied().and_then(|h| seq.notes.get(h).copied().map(|x| (h, x)));
        match head {
            Some((h, x)) if x.pitch == n.pitch && x.channel == n.channel => {
                if let Some(x) = seq.notes.get_mut(h) {
                    x.length = n.end().max(x.end()).saturating_sub(x.start).max(1);
                }
                gone.push(i);
            }
            _ => heads.push(i),
        }
    }
    if gone.is_empty() {
        return Err(bad(C, "pick two or more notes of the same pitch"));
    }
    let mut kept_heads = Vec::new();
    let mut notes = Vec::with_capacity(seq.notes.len());
    for (i, n) in seq.notes.iter().enumerate() {
        if gone.contains(&i) {
            continue;
        }
        if heads.contains(&i) {
            kept_heads.push(notes.len());
        }
        notes.push(*n);
    }
    seq.notes = notes;
    let kept = sort_tracked(seq, &kept_heads);
    Ok(json!({"indices": kept, "joined": gone.len()}))
}

/// The pitch in `allowed` nearest to `pitch` (ties down), or the first one at or above (`up`) or
/// below (`down`); `up`/`down` fall back to the other way at the ends of the range. `None` when
/// the set is empty.
fn snapped(pitch: u8, allowed: &[bool; 12], dir: Option<bool>) -> Option<u8> {
    let ok = |p: i32| (0..=127).contains(&p) && allowed.get(p as usize % 12).copied().unwrap_or(false);
    let p = i32::from(pitch);
    let pick = |up: bool| (0..12).map(|d| if up { p + d } else { p - d }).find(|q| ok(*q));
    let r = match dir {
        Some(up) => pick(up).or_else(|| pick(!up)),
        None => {
            let (lo, hi) = (pick(false), pick(true));
            match (lo, hi) {
                (Some(l), Some(h)) => Some(if p - l <= h - p { l } else { h }),
                (l, h) => l.or(h),
            }
        }
    };
    r.map(|q| q as u8)
}

fn tones_of(set: soundcraft_vocal::PitchClassSet) -> [bool; 12] {
    std::array::from_fn(|i| set.contains(i as u8))
}

fn snap_to_scale(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.snap_to_scale";
    let mode = str_param(p, "mode").unwrap_or("nearest");
    let dir = match mode {
        "nearest" | "chord" => None,
        "up" => Some(true),
        "down" => Some(false),
        _ => return Err(bad(C, "`mode` is nearest, up, down or chord")),
    };
    let fixed = match str_param(p, "key") {
        Some(k) => Some(crate::vocal::parse_key(k).ok_or_else(|| bad(C, format!("can't read the key '{k}'")))?),
        None => None,
    };
    if fixed.is_none() && !crate::vocal::has_key(e.session()) {
        return Err(bad(C, "Set a key first"));
    }
    let ids: Vec<ClipId> = clip_ids_param(e, p)
        .into_iter()
        .filter(|id| e.session().find_clip(*id).is_some_and(|(_, c)| matches!(c.content, ClipContent::Midi { .. })))
        .collect();
    if ids.is_empty() {
        return Err(bad(C, "no MIDI clip"));
    }
    let only: Option<Vec<usize>> = p.get("indices").is_some().then(|| indices(p));
    let single = ids.len() == 1;
    let (mut moved, mut kept) = (0usize, Vec::new());
    for id in ids {
        let s = e.session();
        let Some((_, clip)) = s.find_clip(id) else { continue };
        let ClipContent::Midi { sequence } = &clip.content else { continue };
        let base = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
        let chords = if mode == "chord" { crate::midi_cleanup::chord_spans(s, clip) } else { Vec::new() };
        let mut work: Vec<(Note, bool)> = Vec::with_capacity(sequence.notes.len());
        for (i, n) in sequence.notes.iter().enumerate() {
            let mut n = *n;
            let picked = if single { only.as_ref().is_none_or(|o| o.contains(&i)) } else { true };
            if picked {
                let chord = chords.iter().rev().find(|c| c.start <= n.start && n.start < c.end).map(|c| c.tones);
                let allowed = chord.unwrap_or_else(|| {
                    let (k, sc) =
                        fixed.unwrap_or_else(|| crate::vocal::key_pc_at(s, s.tempo.tick_to_samples(base.saturating_add(n.start), s.sample_rate)));
                    tones_of(crate::vocal::key_set(k, sc))
                });
                if let Some(q) = snapped(n.pitch, &allowed, dir)
                    && q != n.pitch
                {
                    n.pitch = q;
                    moved += 1;
                }
            }
            work.push((n, picked));
        }
        work.sort_by_key(|(a, _)| (a.start, a.pitch, a.channel, a.length, a.velocity));
        if single {
            kept = work.iter().enumerate().filter(|(_, (_, f))| *f).map(|(i, _)| i).collect();
        }
        if let Some(c) = e.session_mut().find_clip_mut(id)
            && let ClipContent::Midi { sequence } = &mut c.content
        {
            sequence.notes = work.into_iter().map(|(n, _)| n).collect();
        }
    }
    Ok(json!({"moved": moved, "indices": kept}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn create_clip_add_edit_move_delete() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let r = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 0})).unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 64, "start_ticks": 480})).unwrap();
        e.execute("midi.note_edit", &json!({"clip": clip, "index": 0, "velocity": 30})).unwrap();
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [1], "semitones": 3})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["velocity"], 30);
        assert_eq!(n["notes"][1]["pitch"], 67);
        e.execute("midi.note_delete", &json!({"clip": clip, "indices": [0]})).unwrap();
        assert_eq!(e.execute("midi.notes", &json!({"clip": clip})).unwrap()["notes"].as_array().map(Vec::len), Some(1));
        assert!(e.execute("midi.note_edit", &json!({"clip": clip, "index": 99})).is_err());
    }

    #[test]
    fn notes_move_huge_params_do_not_overflow() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let r = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap();
        let clip = r["clip"].as_u64().unwrap();
        e.execute("midi.note_add", &json!({"clip": clip, "pitch": 60, "start_ticks": 480})).unwrap();
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "ticks": i64::MAX})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["start"], i64::MAX);
        e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "semitones": i64::MAX})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["pitch"], 127);
    }

    fn psl(e: &mut Engine, clip: u64) -> Vec<(u64, i64, i64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|x| (x["pitch"].as_u64().unwrap(), x["start"].as_i64().unwrap(), x["length"].as_i64().unwrap()))
            .collect()
    }

    #[test]
    fn notes_split_cuts_the_picked_notes_crossing_the_point() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (64, 0), (67, 960)]);
        let r = e.execute("midi.notes_split", &json!({"clip": clip, "indices": [0, 1, 2], "at": 240})).unwrap();
        assert_eq!(psl(&mut e, clip), vec![(60, 0, 240), (64, 0, 240), (60, 240, 240), (64, 240, 240), (67, 960, 480)]);
        assert_eq!(r["indices"], json!([0, 1, 2, 3]));
        assert!(e.execute("midi.notes_split", &json!({"clip": clip, "indices": [4], "at": 240})).is_err());
    }

    #[test]
    fn notes_join_makes_one_note_per_pitch_run() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (60, 480), (60, 1200), (64, 0)]);
        let r = e.execute("midi.notes_join", &json!({"clip": clip, "indices": [0, 1, 2, 3]})).unwrap();
        // C4 0-480, 480-960 and 1200-1680 join to 0-1680; E4 stays alone.
        assert_eq!(psl(&mut e, clip), vec![(60, 0, 1680), (64, 0, 480)]);
        assert_eq!(r["indices"], json!([0, 1]));
        assert!(e.execute("midi.notes_join", &json!({"clip": clip, "indices": [0, 1]})).is_err());
    }

    #[test]
    fn notes_set_gives_every_picked_note_the_value() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (64, 480), (67, 960)]);
        e.execute("midi.notes_set", &json!({"clip": clip, "indices": [0, 2], "length": 120, "velocity": 30, "channel": 3})).unwrap();
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][0]["length"], 120);
        assert_eq!(n["notes"][0]["channel"], 3);
        assert_eq!(n["notes"][2]["velocity"], 30);
        assert_eq!(n["notes"][1]["velocity"], 100);
        assert!(e.execute("midi.notes_set", &json!({"clip": clip, "indices": [0]})).is_err());
        assert!(e.execute("midi.notes_set", &json!({"clip": clip, "indices": [0], "channel": 99})).is_ok());
    }

    /// A MIDI clip of one bar with `(pitch, start_ticks)` notes.
    fn clip_with(e: &mut Engine, notes: &[(u8, i64)]) -> u64 {
        let t = e.session_mut().add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Keys"));
        let clip = e.execute("midi.new_clip", &json!({"track": t.0})).unwrap()["clip"].as_u64().unwrap();
        for (pitch, start) in notes {
            e.execute("midi.note_add", &json!({"clip": clip, "pitch": pitch, "start_ticks": start})).unwrap();
        }
        clip
    }

    fn pitches(e: &mut Engine, clip: u64) -> Vec<u64> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|x| x["pitch"].as_u64().unwrap()).collect()
    }

    #[test]
    fn snap_to_c_major_nearest_ties_down() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (63, 480), (65, 960), (60, 1440)]);
        let r = e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major"})).unwrap();
        assert_eq!(r["moved"], 2);
        assert_eq!(pitches(&mut e, clip), vec![60, 62, 65, 60]);
    }

    #[test]
    fn snap_up_and_down() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (63, 480)]);
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major", "mode": "up"})).unwrap();
        assert_eq!(pitches(&mut e, clip), vec![62, 64]);
        let clip = clip_with(&mut e, &[(61, 0), (63, 480)]);
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major", "mode": "down"})).unwrap();
        assert_eq!(pitches(&mut e, clip), vec![60, 62]);
        assert!(e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major", "mode": "sideways"})).is_err());
        assert!(e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "H flat"})).is_err());
    }

    #[test]
    fn snap_uses_the_key_in_force_at_each_note() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (60, 3840)]);
        let s = e.session_mut();
        let at = s.tempo.tick_to_samples(3840, s.sample_rate);
        s.key_signatures = vec![(0, "C major".into()), (at, "D major".into())];
        e.execute("midi.snap_to_scale", &json!({"clip": clip})).unwrap();
        // C# is out of C major (down to C); C is out of D major (down to B).
        assert_eq!(pitches(&mut e, clip), vec![60, 59]);
    }

    #[test]
    fn snap_without_a_key_is_refused_unless_one_is_given() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0)]);
        let err = e.execute("midi.snap_to_scale", &json!({"clip": clip})).unwrap_err().to_string();
        assert!(err.contains("Set a key first"), "{err}");
        assert_eq!(pitches(&mut e, clip), vec![61]);
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major"})).unwrap();
        assert_eq!(pitches(&mut e, clip), vec![60]);
    }

    #[test]
    fn snap_to_chord_tones_falls_back_to_the_key() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (61, 3840)]);
        // Two bars long, so the chord over bar 2 reaches the clip.
        let end = e.session().tempo.tick_to_samples(7680, e.session().sample_rate);
        e.session_mut().find_clip_mut(ClipId(clip)).unwrap().length = end;
        // G7 (degree 4, flavour 2) over bar 2 of C major.
        e.session_mut().band.chords.push(soundcraft_model::ChordEvent { id: 1, tick: 3840, length_ticks: 3840, slot: 66, bass: -1 });
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "mode": "chord"})).unwrap();
        // Bar 1 has no chord: the key (C major) puts C# on C. Bar 2: G B D F puts it on D.
        assert_eq!(pitches(&mut e, clip), vec![60, 62]);
    }

    #[test]
    fn snap_indices_keep_the_selection_after_the_resort() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (62, 0)]);
        e.execute("midi.note_edit", &json!({"clip": clip, "index": 1, "velocity": 50})).unwrap();
        let r = e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major", "mode": "up", "indices": [0]})).unwrap();
        assert_eq!(r["moved"], 1);
        // Both are D now; the quieter one sorts first, so the snapped note is second.
        assert_eq!(r["indices"], json!([1]));
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        assert_eq!(n["notes"][1]["velocity"], 100);
    }

    #[test]
    fn snap_is_one_undo_step() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(61, 0), (63, 480)]);
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "C major"})).unwrap();
        assert_eq!(e.undo_label(), Some("Snap to Scale"));
        assert!(e.undo());
        assert_eq!(pitches(&mut e, clip), vec![61, 63]);
    }

    #[test]
    fn snap_stays_inside_the_midi_range() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(127, 0), (0, 480)]);
        // Only F# major's tones: 127 (G) snaps up-mode to 126; 0 (C) down-mode to 1 (C#).
        e.execute("midi.snap_to_scale", &json!({"clip": clip, "key": "F# major", "mode": "up"})).unwrap();
        assert_eq!(pitches(&mut e, clip), vec![126, 1]);
    }

    fn velocities(e: &mut Engine, clip: u64) -> Vec<u64> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|x| x["velocity"].as_u64().unwrap()).collect()
    }

    #[test]
    fn notes_velocity_edits_only_the_picked_notes_of_a_chord() {
        let mut e = Engine::default();
        // A three-note chord, then one note later.
        let clip = clip_with(&mut e, &[(60, 0), (64, 0), (67, 0), (72, 480)]);
        e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [1, 2], "set": 40})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![100, 40, 40, 100]);
        e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [0, 1], "scale": 0.5})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![50, 20, 40, 100]);
        e.execute("midi.notes_velocity", &json!({"clip": clip, "add": 10})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![60, 30, 50, 110]);
        e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [3, 0], "values": [200, 1]})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![1, 30, 50, 127]);
        assert_eq!(e.undo_label(), Some("Change Note Velocity"));
    }

    #[test]
    fn notes_velocity_ramps_over_time_and_refuses_bad_values() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (60, 480), (60, 960)]);
        e.execute("midi.notes_velocity", &json!({"clip": clip, "from": 20, "to": 120})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![20, 70, 120]);
        assert!(e.execute("midi.notes_velocity", &json!({"clip": clip})).is_err());
        assert!(e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [0], "values": [1, 2]})).is_err());
        assert!(e.execute("midi.notes_velocity", &json!({"clip": clip, "values": [1, 2, 3]})).is_err());
        // Out-of-range indices are skipped, not a panic.
        e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [99], "set": 5})).unwrap();
        e.execute("midi.notes_velocity", &json!({"clip": clip, "indices": [99], "values": [5]})).unwrap();
        assert_eq!(velocities(&mut e, clip), vec![20, 70, 120]);
    }

    #[test]
    fn notes_move_returns_where_the_moved_notes_went() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (62, 480), (64, 960)]);
        // The first note moves past the others: it sorts last.
        let r = e.execute("midi.notes_move", &json!({"clip": clip, "indices": [0], "ticks": 1440})).unwrap();
        assert_eq!(r["indices"], json!([2]));
        assert_eq!(pitches(&mut e, clip), vec![62, 64, 60]);
    }

    fn starts_lengths(e: &mut Engine, clip: u64) -> Vec<(i64, i64)> {
        let n = e.execute("midi.notes", &json!({"clip": clip})).unwrap();
        n["notes"].as_array().unwrap().iter().map(|x| (x["start"].as_i64().unwrap(), x["length"].as_i64().unwrap())).collect()
    }

    #[test]
    fn notes_resize_moves_either_edge_of_every_picked_note() {
        let mut e = Engine::default();
        // note_add's default length is an eighth (480 ticks).
        let clip = clip_with(&mut e, &[(60, 480), (64, 960), (67, 1440)]);
        let r = e.execute("midi.notes_resize", &json!({"clip": clip, "indices": [0, 1], "length": 120})).unwrap();
        assert_eq!(r["indices"], json!([0, 1]));
        assert_eq!(starts_lengths(&mut e, clip), vec![(480, 600), (960, 600), (1440, 480)]);
        // The start edge: earlier, the ends stay.
        e.execute("midi.notes_resize", &json!({"clip": clip, "indices": [0, 1], "start": -240})).unwrap();
        assert_eq!(starts_lengths(&mut e, clip), vec![(240, 840), (720, 840), (1440, 480)]);
        // Never shorter than a tick, never before the clip start.
        e.execute("midi.notes_resize", &json!({"clip": clip, "indices": [2], "start": 10_000})).unwrap();
        assert_eq!(starts_lengths(&mut e, clip)[2], (1919, 1));
        e.execute("midi.notes_resize", &json!({"clip": clip, "indices": [0], "start": i64::MIN})).unwrap();
        assert_eq!(starts_lengths(&mut e, clip)[0], (0, 1080));
        e.execute("midi.notes_resize", &json!({"clip": clip, "indices": [0], "length": i64::MIN})).unwrap();
        assert_eq!(starts_lengths(&mut e, clip)[0], (0, 1));
        assert!(e.execute("midi.notes_resize", &json!({"clip": clip, "length": 5})).is_err());
    }

    #[test]
    fn notes_add_and_copy_return_the_new_notes() {
        let mut e = Engine::default();
        let clip = clip_with(&mut e, &[(60, 0), (64, 960)]);
        let notes = json!([{"pitch": 62, "start": 480, "length": 100, "velocity": 70}, {"pitch": 72, "start": 2000, "length": 50}]);
        let r = e.execute("midi.notes_add", &json!({"clip": clip, "notes": notes})).unwrap();
        assert_eq!(r["indices"], json!([1, 3]));
        assert_eq!(pitches(&mut e, clip), vec![60, 62, 64, 72]);
        assert_eq!(velocities(&mut e, clip), vec![100, 70, 100, 100]);
        assert!(e.execute("midi.notes_add", &json!({"clip": clip, "notes": [{"start": 1}]})).is_err());
        // Copy the first two a bar later and up an octave: the copies come back.
        let r = e.execute("midi.notes_copy", &json!({"clip": clip, "indices": [0, 1], "ticks": 1920, "semitones": 12})).unwrap();
        assert_eq!(r["indices"], json!([3, 5]));
        assert_eq!(pitches(&mut e, clip), vec![60, 62, 64, 72, 72, 74]);
        assert_eq!(e.undo_label(), Some("Copy Notes"));
    }

    #[test]
    fn rejects_audio_tracks() {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        assert!(e.execute("midi.new_clip", &json!({"track": t.0})).is_err());
    }
}

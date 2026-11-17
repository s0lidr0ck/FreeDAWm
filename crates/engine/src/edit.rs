//! Editing algorithms on playlists, independent of the command layer.
//!
//! Model rules: clips on a playlist are sorted and do not overlap, except where two clips
//! overlap exactly over a crossfade (the earlier clip fades out while the later fades in), or
//! where a clip of a higher layer covers others (the Layer overlap mode, [`Overlap`]).
//! Placing a clip overwrites whatever is under it (like the incumbent's Slip mode) unless the
//! session's overlap mode says otherwise; Shuffle mode also ripples later clips.

use soundcraft_model::{AutoParam, AutomationPoint, Clip, ClipContent, ClipId, Fade, FadeShape, Session, Track, TrackId};
use soundcraft_time::{Range, SampleRate, Samples, TempoMap};
use std::collections::{HashMap, HashSet};

/// Edit-state flag: the Edit cursor and selections snap to the grid in Shuffle, Slip and Spot.
pub const SNAP_TO_GRID: &str = "edit.snap_to_grid";
/// Edit-state flag: Shuffle mode can't be chosen.
pub const SHUFFLE_LOCK: &str = "edit.shuffle_lock";
/// Edit-state flag: the Grid mode last used was Relative, so choosing Grid again picks it.
pub const RELATIVE_GRID: &str = "edit.relative_grid";

/// Context needed to split MIDI clips in musical time.
#[derive(Clone, Copy)]
pub struct TimeCtx<'a> {
    pub tempo: &'a TempoMap,
    pub sr: SampleRate,
}

impl<'a> TimeCtx<'a> {
    pub fn of(s: &'a Session) -> Self {
        TimeCtx { tempo: &s.tempo, sr: s.sample_rate }
    }
    fn ticks(&self, s: Samples) -> i64 {
        self.tempo.samples_to_ticks(s, self.sr)
    }
}

/// Split `clip` at absolute position `at` (strictly inside). Returns (left, right) where right gets `new_id`.
pub fn split_clip(clip: &Clip, at: Samples, new_id: ClipId, t: TimeCtx<'_>) -> Option<(Clip, Clip)> {
    if at <= clip.start || at >= clip.end() {
        return None;
    }
    let mut left = clip.clone();
    let mut right = clip.clone();
    right.id = new_id;
    left.trim_end_to(at);
    right.trim_start_to(at);
    left.fade_out = Fade::default();
    right.fade_in = Fade::default();
    if let (ClipContent::Midi { sequence: ls }, ClipContent::Midi { sequence: rs }) = (&mut left.content, &mut right.content) {
        let boundary = t.ticks(at) - t.ticks(clip.start);
        ls.notes.retain(|n| n.start < boundary);
        ls.ctrls.retain(|c| c.tick < boundary);
        rs.notes.retain(|n| n.start >= boundary);
        // The right part keeps the controller state from before the cut (the program, the pedal,
        // the bend in force there): the last value of each, hidden before its start, which sets
        // the state it starts in.
        let before = last_values_before(&rs.ctrls, boundary);
        rs.ctrls.retain(|c| c.tick >= boundary || before.contains(c));
        shift_sequence(rs, boundary);
    }
    Some((left, right))
}

/// The last event of each controller (per channel and kind) before `tick`, in time order.
fn last_values_before(ctrls: &[soundcraft_midi::CtrlEvent], tick: i64) -> Vec<soundcraft_midi::CtrlEvent> {
    let mut out: Vec<soundcraft_midi::CtrlEvent> = Vec::new();
    for c in ctrls.iter().rev().filter(|c| c.tick < tick) {
        if !out.iter().any(|o| o.channel == c.channel && o.kind == c.kind) {
            out.push(*c);
        }
    }
    out.reverse();
    out
}

/// Every note and controller `delta` ticks earlier.
fn shift_sequence(seq: &mut soundcraft_midi::Sequence, delta: i64) {
    for n in &mut seq.notes {
        n.start = n.start.saturating_sub(delta);
    }
    for c in &mut seq.ctrls {
        c.tick = c.tick.saturating_sub(delta);
    }
}

/// Trim a MIDI clip's start: re-anchor the notes and controllers to the new start. Nothing is
/// dropped: what falls before it gets a negative tick, is hidden and does not play (the
/// controllers there still set the state the clip starts in), and comes back when the start is
/// dragged out again. Returns the shift in ticks (0 for other clips), which the clip's stored
/// original performance takes too ([`shift_originals`]).
fn rebase_midi_start(clip: &mut Clip, old_start: Samples, t: TimeCtx<'_>) -> i64 {
    let delta = t.ticks(clip.start) - t.ticks(old_start);
    if let ClipContent::Midi { sequence } = &mut clip.content {
        if delta != 0 {
            shift_sequence(sequence, delta);
        }
        return delta;
    }
    0
}

/// Keeps Restore Performance in step with start trims: each clip's stored original (Flatten
/// Performance, a recorded take) moves with its notes. `shifts` are (clip, ticks) as
/// [`rebase_midi_start`] returned them.
fn shift_originals(s: &mut Session, shifts: &[(ClipId, i64)]) {
    for (id, delta) in shifts.iter().filter(|(_, d)| *d != 0) {
        if let Some(orig) = s.midi_originals.get_mut(&id.0) {
            shift_sequence(orig, *delta);
        }
    }
}

/// A copied piece of a MIDI clip keeps only what can sound in it: the notes that reach into it
/// and, from before its start, the last value of each controller (the state it starts in).
fn crop_hidden_midi(clip: &mut Clip, t: TimeCtx<'_>) {
    let len = t.ticks(clip.end()) - t.ticks(clip.start);
    if let ClipContent::Midi { sequence } = &mut clip.content {
        sequence.notes.retain(|n| n.end() > 0 && n.start < len);
        let mut seen: Vec<(u8, soundcraft_midi::CtrlKind)> = Vec::new();
        let mut keep: Vec<bool> = vec![true; sequence.ctrls.len()];
        for (i, c) in sequence.ctrls.iter().enumerate().rev() {
            let earlier = c.tick < 0 && seen.contains(&(c.channel, c.kind));
            if c.tick < 0 && !earlier {
                seen.push((c.channel, c.kind));
            }
            if (c.tick >= len || earlier)
                && let Some(k) = keep.get_mut(i)
            {
                *k = false;
            }
        }
        let mut k = keep.into_iter();
        sequence.ctrls.retain(|_| k.next().unwrap_or(true));
    }
}

/// Split every clip on a track's active playlist that spans `at`.
pub fn separate_at(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let tc_tempo = s.tempo.clone();
    let sr = s.sample_rate;
    let t = TimeCtx { tempo: &tc_tempo, sr };
    let mut ids = Vec::new();
    let n_split = s.track(track).map_or(0, |tr| tr.clips().iter().filter(|c| at > c.start && at < c.end()).count());
    for _ in 0..n_split {
        ids.push(s.new_clip_id());
    }
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut out = Vec::with_capacity(pl.clips.len() + ids.len());
    let mut count = 0;
    let mut halves = Vec::new();
    for c in pl.clips.drain(..) {
        if at > c.start
            && at < c.end()
            && let Some(id) = ids.pop()
            && let Some((l, r)) = split_clip(&c, at, id, t)
        {
            if matches!(c.content, ClipContent::Midi { .. }) {
                halves.push((c.id, r.id, t.ticks(at) - t.ticks(c.start)));
            }
            out.push(l);
            out.push(r);
            count += 1;
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    pl.sort();
    // Each half keeps the stored original performance, in its own time, for Restore Performance.
    for (left, right, boundary) in halves {
        if let Some(mut orig) = s.midi_originals.get(&left.0).cloned() {
            shift_sequence(&mut orig, boundary);
            s.midi_originals.insert(right.0, orig);
        }
    }
    count
}

/// Remove material inside `range` on a track. In Shuffle mode later clips move left, up to the
/// first locked clip (see [`ripple`]). Returns the removed material as clips positioned relative
/// to `range.start`.
pub fn clear_range(s: &mut Session, track: TrackId, range: Range, shuffle: bool) -> Vec<Clip> {
    if range.is_empty() {
        return Vec::new();
    }
    separate_at(s, track, range.start);
    separate_at(s, track, range.end);
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let mut removed = Vec::new();
    pl.clips.retain(|c| {
        if c.start >= range.start && c.end() <= range.end {
            let mut r = c.clone();
            r.start -= range.start;
            removed.push(r);
            false
        } else {
            true
        }
    });
    if shuffle {
        let moved = ripple(s, track, range.end, -range.len());
        follow_automation(s, track, range.end, moved);
    }
    removed
}

/// Remove clips `ids` from a track as objects, leaving whatever lies between them alone. In
/// Shuffle mode the clips leave the Shuffle way (later clips close each gap, see [`lift_plan`];
/// time- and edit-locked clips stay); otherwise every clip in `ids` that is not edit-locked goes.
/// Returns the removed clips, in time order.
pub fn remove_clips(s: &mut Session, track: TrackId, ids: &[ClipId], shuffle: bool) -> Vec<Clip> {
    let set: HashSet<ClipId> = ids.iter().copied().collect();
    let mut out: Vec<Clip> = if shuffle {
        lift_all(s, track, &set).into_iter().map(|(c, _)| c).collect()
    } else {
        let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
        let (out, kept): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut pl.clips).into_iter().partition(|c| set.contains(&c.id) && !c.edit_locked);
        pl.clips = kept;
        out
    };
    out.sort_by_key(|c| (c.start, c.id));
    out
}

/// Copy the material inside `range` (clips trimmed to the range), relative to `range.start`.
pub fn copy_range(s: &Session, track: TrackId, range: Range) -> Vec<Clip> {
    copy_range_with(s, track, range, false)
}

/// [`copy_range`]; with `as_heard`, a MIDI clip on Cleaned gives the notes it plays (its MIDI
/// Cleanup's), not its raw ones, and leaves its cleanup behind: what Consolidate merges.
fn copy_range_with(s: &Session, track: TrackId, range: Range, as_heard: bool) -> Vec<Clip> {
    let t = TimeCtx::of(s);
    let Some(tr) = s.track(track) else { return Vec::new() };
    let mut out = Vec::new();
    for c in tr.clips() {
        let Some(isect) = c.range().intersect(&range) else { continue };
        let mut part = c.clone();
        if as_heard && let Some(heard) = c.midi_playing() {
            part.content = ClipContent::Midi { sequence: heard.clone() };
            part.midi_cleanup = None;
        }
        if isect.start > part.start {
            let old = part.start;
            part.trim_start_to(isect.start);
            rebase_midi_start(&mut part, old, t);
        }
        if isect.end < part.end() {
            part.trim_end_to(isect.end);
        }
        crop_hidden_midi(&mut part, t);
        part.start -= range.start;
        out.push(part);
    }
    out
}

/// Merge the MIDI inside `range` into one clip covering the whole range, gaps included, as it
/// sounds (muted clips stay silent). Returns false for tracks that aren't MIDI or instrument tracks.
pub fn consolidate_midi(s: &mut Session, track: TrackId, range: Range) -> bool {
    let Some(name) = s.track(track).filter(|t| t.kind.is_midi()).map(|t| t.name.clone()) else { return false };
    let mut merged = soundcraft_midi::Sequence::default();
    let channels;
    {
        let t = TimeCtx::of(s);
        let base = t.ticks(range.start);
        let parts: Vec<Clip> =
            copy_range_with(s, track, range, true).into_iter().filter(|c| !c.muted && matches!(c.content, ClipContent::Midi { .. })).collect();
        // A printed band clip among the parts keeps its channels (and its controllers-first
        // order); the other parts play on channel 1, so they go there.
        channels = parts.iter().any(|c| c.midi_channels);
        for part in parts {
            let at = t.ticks(range.start.saturating_add(part.start)).saturating_sub(base);
            let len = t.ticks(range.start.saturating_add(part.end())).saturating_sub(base).saturating_sub(at);
            let flatten = channels && !part.midi_channels;
            let ClipContent::Midi { mut sequence } = part.content else { continue };
            if flatten {
                channel_one(&mut sequence);
            }
            for mut n in sequence.notes.into_iter().filter(|n| (0..len).contains(&n.start)) {
                n.length = n.length.min(len - n.start);
                n.start += at;
                merged.notes.push(n);
            }
            // A controller a start trim hid sets the state the part starts in.
            for mut c in sequence.ctrls.into_iter().filter(|c| c.tick < len) {
                c.tick = c.tick.max(0) + at;
                merged.ctrls.push(c);
            }
        }
    }
    merged.sort();
    let id = s.new_clip_id();
    let mut clip = Clip::midi(id, name, range.start, range.len(), merged);
    clip.midi_channels = channels;
    place_clip(s, track, clip);
    true
}

/// Every note and controller of `seq` onto channel 1: how a clip without
/// [`Clip::midi_channels`] plays, kept when it joins one with them.
pub(crate) fn channel_one(seq: &mut soundcraft_midi::Sequence) {
    for n in &mut seq.notes {
        n.channel = 0;
    }
    for c in &mut seq.ctrls {
        c.channel = 0;
    }
}

/// Insert `clips` (positions relative to 0) at `at`. Overwrites what's underneath unless
/// `shuffle`, which first makes room for `length` (see [`make_room`], whose refusal it returns).
pub fn paste(s: &mut Session, track: TrackId, at: Samples, clips: &[Clip], length: Samples, shuffle: bool) -> Result<Vec<ClipId>, &'static str> {
    let length = length.max(clips.iter().map(Clip::end).max().unwrap_or(0));
    if shuffle {
        make_room(s, track, at, length)?;
    }
    let mut ids = Vec::new();
    let mut placed = Vec::with_capacity(clips.len());
    for c in clips {
        let mut nc = c.clone();
        nc.id = s.new_clip_id();
        nc.start += at;
        ids.push(nc.id);
        placed.push((track, nc));
    }
    // All at once: a crossfaded pair overwrites what is under it, never its own partner.
    put_clips(s, placed);
    Ok(ids)
}

/// Place one clip, overwriting whatever it covers.
pub fn place_clip(s: &mut Session, track: TrackId, clip: Clip) {
    clear_range(s, track, clip.range(), false);
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.push(clip);
        pl.sort();
    }
}

/// Insert silence: shift material at/after `range.start` right by the range length (Insert Time;
/// Shuffle mode edits use [`make_room`]).
pub fn insert_silence(s: &mut Session, track: TrackId, range: Range) {
    if range.is_empty() {
        return;
    }
    separate_at(s, track, range.start);
    shift_from(s, track, range.start, range.len());
}

/// Move every clip that starts at or after `from` by `by`, locked or not.
pub fn shift_from(s: &mut Session, track: TrackId, from: Samples, by: Samples) {
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        for c in pl.clips.iter_mut().filter(|c| c.start >= from) {
            c.start = c.start.saturating_add(by);
        }
    }
}

/// Commands an edit-locked clip never blocks: undo and redo, and the lock itself.
const LOCK_EXEMPT: [&str; 4] = ["edit.undo", "edit.redo", "clip.edit_lock", "track.lock"];

/// Commands that move whole stretches of the timeline: an edit-locked clip may move with them, as
/// long as nothing else about it changes.
const LOCK_MOVES: [&str; 2] = ["event.insert_time", "event.cut_time"];

/// What an edit-locked clip's lock protects: where it is, what it plays and how it sounds.
/// Names, colours, ratings, groups and the caches kept current by the engine are not edits.
fn locked_material(c: &Clip) -> impl PartialEq + '_ {
    (
        (c.length, &c.content, c.gain_db.to_bits(), &c.gain_env, c.gain_bypass),
        (c.fade_in, c.fade_out, c.muted, c.stretch.to_bits(), c.source_channel, &c.warp, c.sync_point),
        (&c.effects, c.effects_bypass),
    )
}

/// Why the command `id`, which turned `before` into `after`, must be refused: it changed or
/// removed an edit-locked clip (one the command left locked). A clip that went with its track or
/// its playlist is not refused; neither is a change of the session's sample rate, which moves
/// everything. The engine undoes a refused command.
pub fn locked_edit_refused(id: &str, before: &Session, after: &Session) -> Option<String> {
    if LOCK_EXEMPT.contains(&id) || before.sample_rate != after.sample_rate {
        return None;
    }
    let moves = LOCK_MOVES.contains(&id);
    for t in &before.tracks {
        // A deleted track takes its clips with it; so does a deleted playlist.
        let Some(nt) = after.track(t.id) else { continue };
        let playlist_gone = nt.playlists.len() < t.playlists.len();
        // A locked track (Track › Lock Track) locks every clip on it, and takes no new ones.
        let track_locked = t.edit_locked && nt.edit_locked;
        for old in t.playlists.iter().flat_map(|p| &p.clips).filter(|c| c.edit_locked || track_locked) {
            let refused = match nt.playlists.iter().flat_map(|p| &p.clips).find(|c| c.id == old.id) {
                None => !playlist_gone,
                Some(new) => {
                    (new.edit_locked || track_locked) && (locked_material(new) != locked_material(old) || (!moves && new.start != old.start))
                }
            };
            if refused && !old.edit_locked {
                return Some(format!("`{}` is locked; unlock the track to change its clips", t.name));
            }
            if refused {
                return Some(format!("clip `{}` on `{}` is edit-locked; unlock it to change it", old.name, t.name));
            }
        }
        if track_locked {
            let had = |id: ClipId| t.playlists.iter().flat_map(|p| &p.clips).any(|c| c.id == id);
            if nt.playlists.iter().flat_map(|p| &p.clips).any(|c| !had(c.id)) || (nt.mixer.record_arm && !t.mixer.record_arm) {
                return Some(format!("`{}` is locked; unlock the track to change its clips or record on it", t.name));
            }
        }
    }
    None
}

/// Time- or edit-locked: the clip doesn't move, and Shuffle stops at it.
pub fn locked(c: &Clip) -> bool {
    c.time_locked || c.edit_locked
}

/// A clip's place on its track, for working out Shuffle layouts without touching the clips.
#[derive(Clone, Copy)]
struct Span {
    id: ClipId,
    start: Samples,
    end: Samples,
    locked: bool,
}

fn spans(clips: &[Clip]) -> Vec<Span> {
    clips.iter().map(|c| Span { id: c.id, start: c.start, end: c.end(), locked: locked(c) }).collect()
}

/// Shuffle mode's ripple: the spans from `from` on move by `by`, up to the first locked one, which
/// stays put with everything after it. A move left stops at the session start, keeping their
/// spacing. Returns how far they moved and where the lock is (`Samples::MAX` when none).
fn ripple_spans(spans: &mut [Span], from: Samples, by: Samples) -> (Samples, Samples) {
    let lock = spans.iter().filter(|c| c.locked && c.start >= from).map(|c| c.start).min().unwrap_or(Samples::MAX);
    let moves = |c: &Span| c.start >= from && c.start < lock;
    let floor = spans.iter().filter(|c| moves(c)).map(|c| c.start).min().unwrap_or(0).max(0);
    let by = by.max(-floor);
    for c in spans.iter_mut().filter(|c| moves(c)) {
        c.start = c.start.saturating_add(by);
        c.end = c.end.saturating_add(by);
    }
    (by, lock)
}

/// [`ripple_spans`] on a track's clips.
fn ripple(s: &mut Session, track: TrackId, from: Samples, by: Samples) -> (Samples, Samples) {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return (0, Samples::MAX) };
    let mut sp = spans(&pl.clips);
    let moved = ripple_spans(&mut sp, from, by);
    for (c, n) in pl.clips.iter_mut().zip(&sp) {
        c.start = n.start;
    }
    pl.sort();
    moved
}

/// Automation Follows Edit for a Shuffle ripple (`moved` from [`ripple`]): the points from `from`
/// up to the lock move with the clips, and when material closes up the points in the span that
/// goes away go too.
fn follow_automation(s: &mut Session, track: TrackId, from: Samples, (by, lock): (Samples, Samples)) {
    if !s.edit.automation_follows_edit || by == 0 {
        return;
    }
    let Some(t) = s.track_mut(track) else { return };
    for l in &mut t.automation {
        if by < 0 {
            l.clear_range(from.saturating_add(by), from);
        }
        for p in l.points.iter_mut().filter(|p| p.at >= from && p.at < lock) {
            p.at = p.at.saturating_add(by);
        }
        l.points.sort_by_key(|p| p.at);
        l.points.dedup_by_key(|p| p.at);
    }
}

/// With Automation Follows Edit, take the points in `[start, end)` off a track, timed from `origin`.
fn take_automation(s: &mut Session, track: TrackId, start: Samples, end: Samples, origin: Samples) -> Vec<(AutoParam, AutomationPoint)> {
    let mut out = Vec::new();
    if !s.edit.automation_follows_edit || end <= start {
        return out;
    }
    let Some(t) = s.track_mut(track) else { return out };
    for l in &mut t.automation {
        let taken = l.points.iter().filter(|p| p.at >= start && p.at < end);
        out.extend(taken.map(|p| (l.param.clone(), AutomationPoint { at: p.at.saturating_sub(origin), ..*p })));
        l.clear_range(start, end);
    }
    out
}

fn put_automation(s: &mut Session, track: TrackId, origin: Samples, points: &[(AutoParam, AutomationPoint)]) {
    let Some(t) = s.track_mut(track) else { return };
    for (param, p) in points {
        t.lane_mut(param).set_point_curved(origin.saturating_add(p.at), p.value, Some(p.curve));
    }
}

/// How far Shuffle mode can push the material from `at` on before it runs into a locked clip
/// (`Samples::MAX` when none follows).
pub fn shuffle_room(s: &Session, track: TrackId, at: Samples) -> Samples {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let Some(lock) = clips.iter().filter(|c| locked(c) && c.end() > at).map(|c| c.start).min() else { return Samples::MAX };
    let end = clips.iter().filter(|c| c.end() > at && c.start < lock).map(Clip::end).fold(at, Samples::max);
    lock.saturating_sub(end)
}

/// Why Shuffle can't make room: a locked clip follows too closely.
pub const NO_ROOM: &str = "not enough room before a locked clip";
/// Why Shuffle can't make room: the point is inside a crossfade, which can't be pulled apart.
pub const IN_CROSSFADE: &str = "can't make room inside a crossfade";

/// Shuffle mode's room for `len` at `at`: later clips (and with Automation Follows Edit their
/// automation) move right. Refused, with nothing changed, when a locked clip is in the way or
/// `at` is inside a crossfade.
pub fn make_room(s: &mut Session, track: TrackId, at: Samples, len: Samples) -> Result<(), &'static str> {
    if len <= 0 {
        return Ok(());
    }
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    if clips.iter().filter(|c| c.start <= at && at < c.end()).count() > 1 {
        return Err(IN_CROSSFADE);
    }
    if shuffle_room(s, track, at) < len {
        return Err(NO_ROOM);
    }
    separate_at(s, track, at);
    let moved = ripple(s, track, at, len);
    follow_automation(s, track, at, moved);
    Ok(())
}

/// Trim clips to keep only material inside `range`.
pub fn trim_to(s: &mut Session, track: TrackId, range: Range) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    let mut shifts = Vec::new();
    for c in &mut pl.clips {
        if let Some(i) = c.range().intersect(&range)
            && i != c.range()
        {
            let old = c.start;
            c.trim_start_to(i.start);
            shifts.push((c.id, rebase_midi_start(c, old, tc)));
            c.trim_end_to(i.end);
            n += 1;
        }
    }
    pl.sort();
    shift_originals(s, &shifts);
    n
}

/// Trim the start of clips under `at` to `at` (Trim Start to Insertion).
pub fn trim_start_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let tempo = s.tempo.clone();
    let tc = TimeCtx { tempo: &tempo, sr: s.sample_rate };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    let mut shifts = Vec::new();
    for c in &mut pl.clips {
        if c.range().contains(at) && at > c.start {
            let old = c.start;
            c.trim_start_to(at);
            shifts.push((c.id, rebase_midi_start(c, old, tc)));
            n += 1;
        }
    }
    shift_originals(s, &shifts);
    n
}

pub fn trim_end_to(s: &mut Session, track: TrackId, at: Samples) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    let mut n = 0;
    for c in &mut pl.clips {
        if at > c.start && at < c.end() {
            c.trim_end_to(at);
            n += 1;
        }
    }
    n
}

/// Heal separations inside `range`: merge neighbouring audio clips that continue the same file,
/// and neighbouring MIDI clips with the same settings ([`midi_heals`], [`join_midi`]).
pub fn heal(s: &mut Session, track: TrackId, range: Range) -> usize {
    let (tempo, sr) = (s.tempo.clone(), s.sample_rate);
    // A clip with a stored original performance keeps it for itself (it is not joined).
    let originals: HashSet<u64> = s.midi_originals.keys().copied().collect();
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut out: Vec<Clip> = Vec::with_capacity(pl.clips.len());
    let mut healed = 0;
    for c in pl.clips.drain(..) {
        if let Some(prev) = out.last_mut()
            && prev.end() == c.start
            && c.start >= range.start
            && c.start <= range.end
            && midi_heals(prev, &c)
            && !originals.contains(&prev.id.0)
            && !originals.contains(&c.id.0)
        {
            join_midi(prev, &c, &tempo, sr);
            healed += 1;
            continue;
        }
        if let Some(prev) = out.last_mut()
            && prev.end() == c.start
            && c.start >= range.start
            && c.start <= range.end
            && heals(prev, &c)
        {
            prev.gain_env = prev.joined_env(&c);
            if prev.sync_point == 0 && c.sync_point != 0 {
                prev.sync_point = c.sync_point.saturating_add(prev.length);
            }
            prev.length += c.length;
            prev.fade_out = c.fade_out;
            healed += 1;
            continue;
        }
        out.push(c);
    }
    pl.clips = out;
    healed
}

/// Whether two MIDI clips, `next` starting where `prev` ends, can be joined into one: every
/// setting but their place, length, name, colour and notes the same, neither a pattern, a shared
/// copy or under MIDI Cleanup.
fn midi_heals(prev: &Clip, next: &Clip) -> bool {
    let both_midi = matches!((&prev.content, &next.content), (ClipContent::Midi { .. }, ClipContent::Midi { .. }));
    if !both_midi || [prev, next].iter().any(|c| c.shared.is_some() || c.pattern.is_some() || c.midi_cleanup.is_some()) {
        return false;
    }
    let neutral = |c: &Clip| {
        let mut c = c.clone();
        c.id = ClipId(0);
        c.name.clear();
        c.start = 0;
        c.length = 0;
        c.content = ClipContent::Midi { sequence: soundcraft_midi::Sequence::default() };
        c.midi_index = Default::default();
        c.sync_point = 0;
        c.rating = 0;
        c.color = None;
        c
    };
    neutral(prev) == neutral(next)
}

/// Joins `next` onto the end of `prev` (MIDI clips, see [`midi_heals`]): its notes and
/// controllers move to their place in `prev`, which grows to cover both. What either clip hid
/// (before `next`'s start, past `prev`'s end) is dropped, since it would sound in the joined clip;
/// a note of `prev` running past its end plays on, as it did before a split.
fn join_midi(prev: &mut Clip, next: &Clip, tempo: &TempoMap, sr: SampleRate) {
    let base = tempo.samples_to_ticks(prev.start, sr);
    let prev_len = tempo.samples_to_ticks(prev.end(), sr).saturating_sub(base);
    let offset = tempo.samples_to_ticks(next.start, sr).saturating_sub(base);
    if let (ClipContent::Midi { sequence: a }, ClipContent::Midi { sequence: b }) = (&mut prev.content, &next.content) {
        a.notes.retain(|n| n.start < prev_len);
        a.ctrls.retain(|c| c.tick < prev_len);
        a.notes.extend(b.notes.iter().filter(|n| n.start >= 0).map(|n| soundcraft_midi::Note { start: n.start.saturating_add(offset), ..*n }));
        a.ctrls.extend(b.ctrls.iter().filter(|c| c.tick >= 0).map(|c| soundcraft_midi::CtrlEvent { tick: c.tick.saturating_add(offset), ..*c }));
        a.sort();
    }
    prev.length = prev.length.saturating_add(next.length);
}

/// Whether `next`, starting where `prev` ends, is the rest of `prev` and sounds like it: the same
/// file continuing at the same stretch, and everything that shapes the sound but the clip-gain
/// envelope (which [`Clip::joined_env`] joins) the same, with no fade at the join. Pieces that
/// differ stay apart rather than one of them losing its settings.
fn heals(prev: &Clip, next: &Clip) -> bool {
    let (ClipContent::Audio { source: a, offset: oa }, ClipContent::Audio { source: b, offset: ob }) = (&prev.content, &next.content) else {
        return false;
    };
    a == b
        && oa.saturating_add(prev.source_span(prev.length)) == *ob
        && prev.stretch.to_bits() == next.stretch.to_bits()
        && (prev.gain_db - next.gain_db).abs() < 1e-6
        && prev.gain_bypass == next.gain_bypass
        && prev.muted == next.muted
        && prev.source_channel == next.source_channel
        && prev.effects == next.effects
        && prev.effects_bypass == next.effects_bypass
        && prev.clean == next.clean
        && prev.tune == next.tune
        && prev.warp.is_empty()
        && next.warp.is_empty()
        && prev.plays_plain()
        && next.plays_plain()
        && prev.fade_out.len == 0
        && next.fade_in.len == 0
        && prev.layer == next.layer
}

/// Move clips by `delta`, overwriting what they land on; `dest` sends whole tracks of them
/// elsewhere (see [`destinations`]).
pub fn move_clips(s: &mut Session, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> usize {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut moving: Vec<(TrackId, Clip)> = Vec::new();
    for t in &mut s.tracks {
        let tid = t.id;
        if let Some(pl) = t.playlist_mut() {
            pl.clips.retain(|c| {
                if ids.contains(&c.id) && !locked(c) {
                    moving.push((tid, c.clone()));
                    false
                } else {
                    true
                }
            });
        }
    }
    let n = moving.len();
    // A move left stops at the session start for all of them together, keeping their spacing
    // (each stopping on its own would close the gaps between them).
    let floor = moving.iter().map(|(_, c)| c.start).min().unwrap_or(0).max(0);
    let delta = delta.max(-floor);
    // Automation Follows Edit: the points under each clip go with it (empty when it's off).
    let points: Vec<Vec<(AutoParam, AutomationPoint)>> = moving.iter().map(|(tid, c)| take_automation(s, *tid, c.start, c.end(), c.start)).collect();
    let placed: Vec<(TrackId, Clip)> = moving
        .into_iter()
        .map(|(tid, mut c)| {
            c.start = c.start.saturating_add(delta).max(0);
            (lands(dest, tid), c)
        })
        .collect();
    if s.edit.automation_follows_edit {
        for (t, c) in &placed {
            if let Some(tr) = s.track_mut(*t) {
                for l in &mut tr.automation {
                    l.clear_range(c.start, c.end());
                }
            }
        }
        for ((t, c), pts) in placed.iter().zip(&points) {
            put_automation(s, *t, c.start, pts);
        }
    }
    put_clips(s, placed);
    n
}

/// Place clips together (moved, pasted or copied), as the session's [`Overlap`] mode says, never
/// cutting each other, so a crossfaded pair placed together stays a crossfade.
pub fn put_clips(s: &mut Session, placed: Vec<(TrackId, Clip)>) {
    let mode = Overlap::of(s);
    // Layer keeps audio under audio; MIDI and video clips still overwrite.
    let (layered, placed): (Vec<(TrackId, Clip)>, Vec<(TrackId, Clip)>) =
        placed.into_iter().partition(|(t, c)| mode == Overlap::Layer && c.is_audio() && s.track(*t).is_some_and(|t| !t.kind.is_midi()));
    let joins = if mode == Overlap::Crossfade { landing_edges(s, &placed) } else { Vec::new() };
    for (t, c) in &placed {
        clear_range(s, *t, c.range(), false);
    }
    let mut tracks: Vec<TrackId> = layered.iter().map(|(t, _)| *t).collect();
    tracks.dedup();
    // Clips placed together on a track share one layer, just above every clip they land on.
    let tops: Vec<(TrackId, i32)> = tracks
        .iter()
        .map(|t| {
            let top = s
                .track(*t)
                .into_iter()
                .flat_map(|tr| tr.clips())
                .filter(|x| layered.iter().any(|(lt, c)| lt == t && c.range().overlaps(&x.range())))
                .map(|x| x.layer)
                .max();
            (*t, top.map_or(0, |l| l.saturating_add(1)))
        })
        .collect();
    let layered = layered.into_iter().map(|(t, mut c)| {
        c.layer = tops.iter().find(|(x, _)| *x == t).map_or(0, |(_, l)| *l);
        (t, c)
    });
    for (t, c) in placed.into_iter().chain(layered) {
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(c);
            pl.sort();
        }
    }
    for (t, edge, room) in joins {
        crossfade_join(s, t, edge, room);
    }
}

/// Session value (`s.edit.values`): what a clip moved, pasted, duplicated or recorded onto other
/// clips does to them (Options › Overlap): see [`Overlap`].
pub const OVERLAP_KEY: &str = "edit.overlap_mode";
/// Session value: the crossfade the Crossfade overlap mode makes at each join, ms.
pub const OVERLAP_XFADE_KEY: &str = "edit.overlap_xfade_ms";
/// The Crossfade overlap mode's crossfade when none is set, ms.
pub const OVERLAP_XFADE_MS: f64 = 10.0;

/// What a clip placed over others does to them (Options › Overlap).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlap {
    /// What it covers is cut away (the default, and how sessions always worked).
    Overwrite,
    /// Audio clips stay whole underneath: the new clip goes on a higher [`Clip::layer`] and is
    /// heard where it covers them; a lower clip plays again where it is uncovered.
    Layer,
    /// Overwrite, then a crossfade at each edge where the clip landed on other audio, out of the
    /// clips' handles.
    Crossfade,
}

impl Overlap {
    pub const ALL: [Overlap; 3] = [Overlap::Overwrite, Overlap::Layer, Overlap::Crossfade];

    /// The session's mode.
    pub fn of(s: &Session) -> Overlap {
        match s.edit.value(OVERLAP_KEY, 0.0) as i64 {
            1 => Overlap::Layer,
            2 => Overlap::Crossfade,
            _ => Overlap::Overwrite,
        }
    }

    /// The value stored under [`OVERLAP_KEY`].
    pub fn value(self) -> f64 {
        match self {
            Overlap::Overwrite => 0.0,
            Overlap::Layer => 1.0,
            Overlap::Crossfade => 2.0,
        }
    }

    pub fn id(self) -> &'static str {
        match self {
            Overlap::Overwrite => "overwrite",
            Overlap::Layer => "layer",
            Overlap::Crossfade => "crossfade",
        }
    }

    pub fn from_id(id: &str) -> Option<Overlap> {
        Overlap::ALL.into_iter().find(|o| o.id() == id.trim().to_ascii_lowercase())
    }
}

/// Where audio clips about to be placed land on other audio: `(track, edge, room)` for each clip
/// edge that falls inside an audio clip already there (and not inside another clip placed with
/// it, as a crossfaded pair's inner edges do), `room` being how far the crossfade may reach into
/// the placed clip (half its length).
fn landing_edges(s: &Session, placed: &[(TrackId, Clip)]) -> Vec<(TrackId, Samples, Samples)> {
    let mut out = Vec::new();
    for (t, c) in placed.iter().filter(|(_, c)| c.is_audio()) {
        let Some(tr) = s.track(*t) else { continue };
        let inside = |x: &Clip, at: Samples| x.start < at && x.end() > at;
        for edge in [c.start, c.end()] {
            let lands = tr.clips().iter().any(|x| x.is_audio() && !placed.iter().any(|(_, p)| p.id == x.id) && inside(x, edge));
            let partner = placed.iter().any(|(pt, p)| {
                pt == t && p.id != c.id && (inside(p, edge) || p.end() == edge && edge == c.start || p.start == edge && edge == c.end())
            });
            if lands && !partner {
                out.push((*t, edge, c.length / 2));
            }
        }
    }
    out
}

/// The Crossfade overlap mode's crossfade at `edge` on track `t`, no further into either side
/// than `room`.
fn crossfade_join(s: &mut Session, t: TrackId, edge: Samples, room: Samples) {
    let ms = s.edit.value(OVERLAP_XFADE_KEY, OVERLAP_XFADE_MS).clamp(0.0, 1000.0);
    let h = ((s.sample_rate.as_f64() * ms / 1000.0 / 2.0).round() as Samples).min(room).max(0);
    if h > 0 {
        create_fades(s, t, Range::new(edge.saturating_sub(h), edge.saturating_add(h)), FadeShape::default());
    }
}

/// Place one recorded or created clip as the session's [`Overlap`] mode says (see
/// [`put_clips`]). [`place_clip`] always overwrites.
pub fn place_clip_overlapping(s: &mut Session, track: TrackId, clip: Clip) {
    put_clips(s, vec![(track, clip)]);
}

/// `ids` and the clips an active edit group brings along with them (Track › Group… with Edit
/// on, unless every group is suspended): on the group's other tracks, each clip that overlaps one
/// of `ids` in time. `ids` first, then the others in track order; no repeats.
pub fn with_edit_groups(s: &Session, ids: &[ClipId]) -> Vec<ClipId> {
    let mut out: Vec<ClipId> = Vec::with_capacity(ids.len());
    for id in ids {
        if !out.contains(id) {
            out.push(*id);
        }
    }
    if s.edit.flag(crate::cmd::GROUPS_SUSPENDED) {
        return out;
    }
    let found: Vec<(TrackId, Range)> = ids.iter().filter_map(|id| s.find_clip(*id).map(|(t, c)| (t, c.range()))).collect();
    for tr in &s.tracks {
        let spans: Vec<Range> = found
            .iter()
            .filter(|(t, _)| *t != tr.id && s.groups.iter().any(|g| g.active && g.edit && g.members.contains(t) && g.members.contains(&tr.id)))
            .map(|(_, r)| *r)
            .collect();
        if spans.is_empty() {
            continue;
        }
        for c in tr.clips().iter().filter(|c| spans.iter().any(|r| r.overlaps(&c.range()))) {
            if !out.contains(&c.id) {
                out.push(c.id);
            }
        }
    }
    out
}

/// Whether the Edit window shows track `t`: not hidden, and not inside a closed folder at any level.
pub fn shown(s: &Session, t: &Track) -> bool {
    !t.hidden && !folder_collapsed(s, t)
}

/// How many tracks move with track `t` as one block: 1, or for a folder, it and the tracks right
/// after it that sit inside it at any level.
pub fn folder_block(s: &Session, t: TrackId) -> usize {
    let Some(i) = s.track_index(t) else { return 1 };
    let inside = |tr: &Track| {
        let mut f = tr.folder;
        for _ in 0..64 {
            match f {
                Some(id) if id == t => return true,
                Some(id) => f = s.track(id).and_then(|x| x.folder),
                None => return false,
            }
        }
        false
    };
    1 + s.tracks.iter().skip(i + 1).take_while(|tr| inside(tr)).count()
}

/// Hidden because a folder at any level above this track is closed.
pub fn folder_collapsed(s: &Session, track: &Track) -> bool {
    let mut current = track.folder;
    for _ in 0..64 {
        let Some(folder) = current.and_then(|folder| s.track(folder)) else { return false };
        if !folder.folder_open {
            return true;
        }
        current = folder.folder;
    }
    false
}

/// Where the tracks of clips `ids` go when track `from` goes to `to`: each keeps its distance
/// from it among the tracks the Edit window shows, or among all tracks when a hidden one is
/// involved. None when one would land past either end, or on a track that can't hold its clips.
pub fn destinations(s: &Session, ids: &[ClipId], from: TrackId, to: TrackId) -> Option<Vec<(TrackId, TrackId)>> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let holds = |t: &Track| t.clips().iter().any(|c| ids.contains(&c.id));
    let visible = s.tracks.iter().filter(|t| t.id == from || t.id == to || holds(t)).all(|t| shown(s, t));
    let rows: Vec<&Track> = s.tracks.iter().filter(|t| !visible || shown(s, t)).collect();
    let row = |id: TrackId| rows.iter().position(|t| t.id == id).and_then(|i| isize::try_from(i).ok());
    let shift = row(to)? - row(from)?;
    s.tracks
        .iter()
        .filter(|t| holds(t))
        .map(|t| {
            let d = rows.get(usize::try_from(row(t.id)? + shift).ok()?)?;
            (d.kind.has_playlist() && (d.kind == t.kind || d.kind.is_midi() && t.kind.is_midi())).then_some((t.id, d.id))
        })
        .collect()
}

/// The track that clips on `t` land on under a [`destinations`] map.
pub fn lands(dest: &[(TrackId, TrackId)], t: TrackId) -> TrackId {
    dest.iter().find(|(from, _)| *from == t).map_or(t, |(_, d)| *d)
}

/// The earliest of clips `ids` that can move (neither time- nor edit-locked).
pub fn first_movable<'a>(s: &'a Session, ids: &[ClipId]) -> Option<&'a Clip> {
    ids.iter().filter_map(|id| s.find_clip(*id)).map(|(_, c)| c).filter(|c| !locked(c)).min_by_key(|c| c.start)
}

/// One Shuffle lift: where the hole the clip leaves starts and how long it is, how far the clips
/// after it actually close up (none past a lock, none before the session start) and where the
/// first locked clip after it is.
#[derive(Clone, Copy)]
struct Lift {
    id: ClipId,
    start: Samples,
    from: Samples,
    shift: Samples,
    by: Samples,
    lock: Samples,
}

/// Lifting the movable `moving` clips off a track's `spans` the Shuffle way, right to left, in
/// one pass: the lifts in that order, and the spans that stay at the starts they close up to.
/// A hole starts where the clips before it end (a crossfade partner keeps its material) and
/// closes up to the next clip, so silence after the lifted clip stays. Clips are ordered by
/// start, then id, so of two stacked clips the later one counts as after the other.
fn lift_plan(mut sp: Vec<Span>, moving: &HashSet<ClipId>) -> (Vec<Lift>, Vec<Span>) {
    sp.sort_by_key(|c| (c.start, c.id));
    let lifted = |c: &Span| !c.locked && moving.contains(&c.id);
    let reach: Vec<Samples> = std::iter::once(Samples::MIN)
        .chain(sp.iter().scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        }))
        .collect();
    let mut lifts = Vec::new();
    let (mut next, mut next_moves, mut lock) = (None, false, Samples::MAX);
    for (i, c) in sp.iter().enumerate().rev() {
        if !lifted(c) {
            (next, next_moves) = (Some(c.start), !c.locked);
            if c.locked {
                lock = c.start;
            }
            continue;
        }
        let from = reach.get(i).map_or(c.start, |far| c.start.max(*far));
        let shift = next.unwrap_or(c.end).min(c.end).saturating_sub(from).max(0);
        let by = if next_moves { -shift.min(next.unwrap_or(0).max(0)) } else { 0 };
        next = next.map(|n| n.saturating_add(by));
        lifts.push(Lift { id: c.id, start: c.start, from, shift, by, lock });
    }
    let (mut by, mut pending) = (0, lifts.iter().rev());
    let mut staying = Vec::with_capacity(sp.len() - lifts.len());
    for c in sp {
        if lifted(&c) {
            by += pending.next().map_or(0, |l| l.by);
        } else if c.locked {
            by = 0;
            staying.push(c);
        } else {
            staying.push(Span { start: c.start.saturating_add(by), end: c.end.saturating_add(by), ..c });
        }
    }
    (lifts, staying)
}

/// Where Shuffle can land material on `track` once the movable `moving` clips have left it (later
/// clips close each gap): the clip boundaries, never inside a clip or a crossfade, sorted. Empty
/// for an empty track.
pub fn shuffle_slots(s: &Session, track: TrackId, moving: &[ClipId]) -> Vec<Samples> {
    let clips = s.track(track).map(|t| t.clips()).unwrap_or_default();
    let (_, mut sp) = lift_plan(spans(clips), &moving.iter().copied().collect());
    sp.sort_by_key(|c| c.start);
    let reach: Vec<Samples> = sp
        .iter()
        .scan(Samples::MIN, |far, c| {
            *far = (*far).max(c.end);
            Some(*far)
        })
        .collect();
    let inside = |b: Samples| reach.get(sp.partition_point(|c| c.start < b).wrapping_sub(1)).is_some_and(|far| *far > b);
    let mut slots: Vec<Samples> = sp.iter().flat_map(|c| [c.start, c.end]).filter(|b| !inside(*b)).collect();
    slots.sort_unstable();
    slots.dedup();
    slots
}

/// The slot nearest `at` (the earlier one on a tie), or the session start when there are none.
pub fn nearest_slot(slots: &[Samples], at: Samples) -> Samples {
    let i = slots.partition_point(|b| *b < at);
    [i.checked_sub(1), Some(i)].into_iter().flatten().filter_map(|i| slots.get(i)).copied().min_by_key(|b| b.abs_diff(at)).unwrap_or(0)
}

/// Where Shuffle mode lands material dropped at `at` on `track` (see [`shuffle_slots`]).
pub fn shuffle_slot(s: &Session, track: TrackId, at: Samples, moving: &[ClipId]) -> Samples {
    nearest_slot(&shuffle_slots(s, track, moving), at)
}

/// Lift the movable `moving` clips off a track the Shuffle way (see [`lift_plan`]); with
/// Automation Follows Edit their automation leaves with them. Returns the clips, each with that
/// automation timed from its start.
fn lift_all(s: &mut Session, track: TrackId, moving: &HashSet<ClipId>) -> Vec<(Clip, Vec<(AutoParam, AutomationPoint)>)> {
    let Some(clips) = s.track(track).map(|t| spans(t.clips())) else { return Vec::new() };
    let (lifts, staying) = lift_plan(clips, moving);
    let mut points: HashMap<ClipId, Vec<(AutoParam, AutomationPoint)>> = HashMap::new();
    for l in &lifts {
        points.insert(l.id, take_automation(s, track, l.from, l.from.saturating_add(l.shift), l.start));
        follow_automation(s, track, l.from.saturating_add(l.shift), (l.by, l.lock));
    }
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return Vec::new() };
    let starts: HashMap<ClipId, Samples> = staying.iter().map(|c| (c.id, c.start)).collect();
    let (out, kept): (Vec<Clip>, Vec<Clip>) = std::mem::take(&mut pl.clips).into_iter().partition(|c| points.contains_key(&c.id));
    pl.clips = kept;
    for c in &mut pl.clips {
        if let Some(start) = starts.get(&c.id) {
            c.start = *start;
        }
    }
    pl.sort();
    out.into_iter()
        .map(|c| {
            let p = points.remove(&c.id).unwrap_or_default();
            (c, p)
        })
        .collect()
}

/// The movable clips `ids` grouped by the track they land on (per `dest`): each destination and
/// the start of its earliest clip, which Shuffle lands on a slot; the others keep their distance.
pub fn shuffle_groups(s: &Session, ids: &[ClipId], dest: &[(TrackId, TrackId)]) -> Vec<(TrackId, Samples)> {
    let ids: HashSet<ClipId> = ids.iter().copied().collect();
    let mut groups: Vec<(TrackId, Samples)> = Vec::new();
    for t in &s.tracks {
        for c in t.clips().iter().filter(|c| ids.contains(&c.id) && !locked(c)) {
            let d = lands(dest, t.id);
            match groups.iter_mut().find(|g| g.0 == d) {
                Some(g) => g.1 = g.1.min(c.start),
                None => groups.push((d, c.start)),
            }
        }
    }
    groups
}

/// Move clips in Shuffle mode: each leaves its track (later clips close the gap, see
/// [`lift_plan`]), and the clips bound for each track land together, the earliest on the
/// [`shuffle_slot`] nearest `delta` away, pushing later clips right. With Automation Follows Edit
/// their automation travels with them. Refused, with the reason, when a locked clip or a
/// crossfade leaves no room.
pub fn shuffle_clips(s: &mut Session, ids: &[ClipId], delta: Samples, dest: &[(TrackId, TrackId)]) -> Result<usize, &'static str> {
    let groups: Vec<(TrackId, Samples, Samples)> =
        shuffle_groups(s, ids, dest).into_iter().map(|(d, first)| (d, first, shuffle_slot(s, d, first.saturating_add(delta), ids))).collect();
    let moving: HashSet<ClipId> = ids.iter().copied().collect();
    let sources: Vec<TrackId> = s.tracks.iter().filter(|t| t.clips().iter().any(|c| moving.contains(&c.id) && !locked(c))).map(|t| t.id).collect();
    let mut lifted = Vec::new();
    for t in sources {
        lifted.extend(lift_all(s, t, &moving).into_iter().map(|(c, points)| (lands(dest, t), c, points)));
    }
    for (d, first, at) in groups {
        let block: Vec<&(TrackId, Clip, Vec<(AutoParam, AutomationPoint)>)> = lifted.iter().filter(|l| l.0 == d).collect();
        let len = block.iter().map(|l| l.1.end()).max().unwrap_or(first).saturating_sub(first);
        make_room(s, d, at, len)?;
        let mut placed = Vec::new();
        for (_, c, points) in block {
            let mut c = c.clone();
            c.start = at.saturating_add(c.start.saturating_sub(first));
            put_automation(s, d, c.start, points);
            placed.push(c);
        }
        if let Some(pl) = s.track_mut(d).and_then(|t| t.playlist_mut()) {
            pl.clips.extend(placed);
            pl.sort();
        }
    }
    Ok(lifted.len())
}

/// How far a Grabber drag by `raw` moves `clip` in Grid mode: Absolute puts its sync point (its
/// start when it has none inside the clip) on the nearest grid line; Relative moves it by whole
/// grid steps, keeping its offset from the grid.
pub fn grid_delta(s: &Session, clip: &Clip, raw: Samples, relative: bool) -> Samples {
    let (g, ctx) = (&s.edit.grid, s.grid_ctx());
    let sync = if (0..clip.length).contains(&clip.sync_point) { clip.sync_point } else { 0 };
    let anchor = clip.start.saturating_add(sync);
    let from = if relative { g.line_at_or_before(anchor, &ctx) } else { anchor };
    g.snap(from.saturating_add(raw), &ctx).saturating_sub(from)
}

/// How far quantizing moves `clip` (its sync point, else its start) towards the grid:
/// `strength` 0..1 of the way to the nearest line, with `swing` 0..1 delaying every second line
/// of a note grid (counted from the bar) by up to a third of a step, as MIDI quantize does.
pub fn quantize_delta(s: &Session, clip: &Clip, strength: f64, swing: f64) -> Samples {
    let (g, sr, map) = (&s.edit.grid, s.sample_rate, &s.tempo);
    let strength = if strength.is_finite() { strength.clamp(0.0, 1.0) } else { 1.0 };
    let swing = if swing.is_finite() { swing.clamp(0.0, 1.0) } else { 0.0 };
    let full = match *g {
        soundcraft_time::GridValue::Note { value, dotted, triplet } if swing > 0.0 => {
            let sync = if (0..clip.length).contains(&clip.sync_point) { clip.sync_point } else { 0 };
            let anchor = clip.start.saturating_add(sync);
            let t = map.samples_to_ticks_f(anchor, sr);
            if !t.is_finite() {
                return 0;
            }
            let bar_tick = map.bar_start_tick(map.bar_beat_at_tick(t.floor() as i64).bar);
            let step = soundcraft_time::GridValue::note_ticks(value, dotted, triplet, map, bar_tick).max(1);
            let off = swing * step as f64 / 3.0;
            let k0 = ((t - bar_tick as f64) / step as f64).floor() as i64;
            let line = |k: i64| bar_tick as f64 + (k * step) as f64 + if k.rem_euclid(2) == 1 { off } else { 0.0 };
            let best = (k0 - 1..=k0 + 2).map(line).min_by(|a, b| (a - t).abs().total_cmp(&(b - t).abs())).unwrap_or(t);
            let target = map.tick_to_samples(best.round() as i64, sr);
            target.saturating_sub(anchor)
        }
        _ => grid_delta(s, clip, 0, false),
    };
    soundcraft_time::to_samples(full as f64 * strength)
}

/// Trim a clip's edges to `start` and/or `end` like the Trim tool, never past its file. In
/// Shuffle mode the start stays put (the material slides under it) and later clips follow the
/// change, up to a locked clip; otherwise a trim stops at the neighbouring clips.
pub fn trim_clip(s: &mut Session, id: ClipId, start: Option<Samples>, end: Option<Samples>, shuffle: bool) -> Option<Clip> {
    let (track, old) = s.find_clip(id).filter(|(_, c)| !c.edit_locked).map(|(t, c)| (t, c.clone()))?;
    let (mut lo, mut hi) = (Samples::MIN, Samples::MAX);
    if shuffle {
        let room = shuffle_room(s, track, old.end()).max(0);
        lo = old.start.saturating_sub(room);
        hi = old.end().saturating_add(room);
    } else {
        let others = s.track(track).map(|t| t.clips()).unwrap_or_default().iter().filter(|c| c.id != id);
        if let Some(prev) = others.clone().filter(|c| c.start < old.start).map(Clip::end).max() {
            lo = lo.max(prev.min(old.start));
        }
        if let Some(next) = others.filter(|c| c.start > old.start).map(|c| c.start).min() {
            hi = hi.min(next.max(old.end()));
        }
        lo = lo.max(0);
    }
    let file = |c: &Clip| {
        // Where the file starts and ends on the timeline, at the clip's stretch.
        let start = c.start.saturating_sub(c.timeline_span(c.source_offset()));
        material_len(s, c).map(|n| (start.min(c.start), start.saturating_add(c.timeline_span(n)).max(c.end())))
    };
    let t = TimeCtx::of(s);
    let mut c = old.clone();
    let mut shift = 0;
    if let Some(at) = start {
        c.trim_start_to(at.max(file(&c).map_or(lo, |(f, _)| lo.max(f))));
        shift = rebase_midi_start(&mut c, old.start, t);
        if shuffle {
            c.start = old.start;
        }
    }
    if let Some(at) = end {
        c.trim_end_to(at.min(file(&c).map_or(hi, |(_, f)| hi.min(f))));
    }
    let pl = s.track_mut(track).and_then(|t| t.playlist_mut())?;
    *pl.clip_mut(id)? = c.clone();
    pl.sort();
    shift_originals(s, &[(id, shift)]);
    if shuffle {
        // The clips after this one move, a crossfade partner starting inside it included; the
        // automation moves from the same place (the partner's start, else this clip's end).
        let from = old.start.saturating_add(1);
        let first = s.track(track).map(|t| t.clips()).unwrap_or_default().iter().filter(|x| x.id != id && x.start >= from).map(|x| x.start).min();
        let moved = ripple(s, track, from, c.end().saturating_sub(old.end()));
        follow_automation(s, track, first.map_or(old.end(), |f| f.min(old.end())), moved);
    }
    Some(c)
}

/// Length of the file or movie under a clip; None for MIDI, which has no end.
pub(crate) fn material_len(s: &Session, c: &Clip) -> Option<Samples> {
    match c.content {
        ClipContent::Audio { source, .. } => s.source(source).map(|src| i64::try_from(src.frames).unwrap_or(i64::MAX)),
        ClipContent::Video { source, .. } => s.video(source).map(|v| s.sample_rate.samples(v.duration.clamp(0.0, 1e7))),
        ClipContent::Midi { .. } => None,
    }
}

/// Create fades from a selection (the incumbent's Edit › Fades › Create semantics):
/// a range across a clip boundary makes a crossfade; at a clip start a fade-in; at its end a fade-out.
pub fn create_fades(s: &mut Session, track: TrackId, range: Range, shape: FadeShape) -> usize {
    create_fades_with(s, track, range, &FadeSpec::of(shape))
}

/// The fades [`create_fades_with`] makes: the shape and bend of fade-ins (and a crossfade's
/// incoming side), of fade-outs (and its outgoing side), and, with `batch` set, a fade of that
/// length at both ends of every clip that lies wholly inside the range.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct FadeSpec {
    pub in_shape: FadeShape,
    pub in_curvature: f32,
    pub out_shape: FadeShape,
    pub out_curvature: f32,
    pub batch: Option<Samples>,
}

impl FadeSpec {
    /// One shape for every fade, unbent, no batch fades.
    pub fn of(shape: FadeShape) -> FadeSpec {
        FadeSpec { in_shape: shape, in_curvature: 0.0, out_shape: shape, out_curvature: 0.0, batch: None }
    }
    fn fade_in(&self, len: Samples) -> Fade {
        Fade { len, shape: self.in_shape, curvature: self.in_curvature }
    }
    fn fade_out(&self, len: Samples) -> Fade {
        Fade { len, shape: self.out_shape, curvature: self.out_curvature }
    }
}

/// [`create_fades`] with a shape and bend for each side, and batch fades on clips inside the range.
/// The crossfades a fade over `range` on `track` would make shorter on one side than the
/// selection asks, for want of audio beyond the cut (a clip's handle): how many.
pub fn short_crossfades(s: &Session, track: TrackId, range: Range) -> usize {
    let Some(tr) = s.track(track) else { return 0 };
    // In time order, as create_fades pairs them.
    let mut clips: Vec<&Clip> = tr.clips().iter().collect();
    clips.sort_by_key(|c| c.start);
    clips
        .windows(2)
        .filter(|w| {
            let (Some(a), Some(b)) = (w.first(), w.get(1)) else { return false };
            let boundary = a.end();
            if b.start != boundary || range.start >= boundary || range.end <= boundary {
                return false;
            }
            let (before, after) = (boundary - range.start.max(a.start), range.end.min(b.end()) - boundary);
            let a_handle = material_len(s, a).map_or(0, |n| a.timeline_span(n.saturating_sub(a.source_offset())).saturating_sub(a.length));
            let b_handle = b.timeline_span(b.source_offset());
            after > a_handle || before > b_handle
        })
        .count()
}

pub fn create_fades_with(s: &mut Session, track: TrackId, range: Range, spec: &FadeSpec) -> usize {
    if range.is_empty() {
        return 0;
    }
    let handles: Vec<(ClipId, Samples)> = {
        let Some(tr) = s.track(track) else { return 0 };
        tr.clips()
            .iter()
            .map(|c| (c.id, material_len(s, c).map_or(0, |n| c.timeline_span(n.saturating_sub(c.source_offset())).saturating_sub(c.length))))
            .collect()
    };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let mut made = 0;
    let mut xfaded: Vec<ClipId> = Vec::new();
    let n = pl.clips.len();
    // Crossfades between adjacent clips whose boundary is inside the range.
    for i in 0..n.saturating_sub(1) {
        let (Some(a), Some(b)) = (pl.clips.get(i).cloned(), pl.clips.get(i + 1).cloned()) else { continue };
        let boundary = a.end();
        if b.start == boundary && range.start < boundary && range.end > boundary {
            let before = boundary - range.start.max(a.start);
            let after = range.end.min(b.end()) - boundary;
            let a_handle = handles.iter().find(|h| h.0 == a.id).map_or(0, |h| h.1);
            let b_handle = b.timeline_span(b.source_offset());
            // Extend each clip into the other's territory using its handles.
            let ext_a = after.min(a_handle).max(0);
            let ext_b = before.min(b_handle).max(0);
            if let Some(ca) = pl.clips.get_mut(i) {
                ca.length += ext_a;
                ca.fade_out = spec.fade_out((ext_a + before).min(ca.length));
            }
            if let Some(cb) = pl.clips.get_mut(i + 1) {
                let old = cb.start;
                cb.trim_start_to(old - ext_b);
                cb.fade_in = spec.fade_in((ext_b + after).min(cb.length));
            }
            made += 1;
            xfaded.push(a.id);
            xfaded.push(b.id);
        }
    }
    for c in pl.clips.iter_mut() {
        let inside = range.start <= c.start && range.end >= c.end();
        if let Some(len) = spec.batch.filter(|l| *l > 0 && inside) {
            // Batch fades: both ends of a clip standing inside the range, where no crossfade is.
            let len = len.min(c.length / 2).max(0);
            if len > 0 {
                let (into, out_of) = (xfaded.contains(&c.id) && c.fade_in.len > 0, xfaded.contains(&c.id) && c.fade_out.len > 0);
                if !into {
                    c.fade_in = spec.fade_in(len);
                    made += 1;
                }
                if !out_of {
                    c.fade_out = spec.fade_out(len);
                    made += 1;
                }
            }
            continue;
        }
        if xfaded.contains(&c.id) {
            continue;
        }
        if range.start <= c.start && range.end > c.start && range.end < c.end() {
            c.fade_in = spec.fade_in(range.end - c.start);
            made += 1;
        } else if range.end >= c.end() && range.start > c.start && range.start < c.end() {
            c.fade_out = spec.fade_out(c.end() - range.start);
            made += 1;
        }
    }
    made
}

/// Remove the fades in `range`: each fade that lies in it (with an empty range, every fade of
/// the clip under the point). A crossfade in it is taken apart: both clips are trimmed back to
/// its midpoint and lose the fades that made it. Fades and crossfades outside the range stay.
pub fn delete_fades(s: &mut Session, track: TrackId, range: Range) -> usize {
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return 0 };
    pl.sort();
    let hit = |c: &Clip, start: Samples, end: Samples| {
        if range.is_empty() { c.range().contains(range.start) } else { Range::new(start, end).overlaps(&range) }
    };
    // Crossfades first, from the clips as they are: the overlap of each neighbouring pair.
    let mut n = 0;
    for i in 0..pl.clips.len().saturating_sub(1) {
        let (Some(a), Some(b)) = (pl.clips.get(i), pl.clips.get(i + 1)) else { continue };
        let (a_end, b_start) = (a.end(), b.start);
        if a_end <= b_start || !(hit(a, b_start, a_end) || hit(b, b_start, a_end)) {
            continue;
        }
        let mid = b_start + (a_end - b_start) / 2;
        if let Some(a) = pl.clips.get_mut(i) {
            a.trim_end_to(mid);
            a.fade_out = Fade::default();
        }
        if let Some(b) = pl.clips.get_mut(i + 1) {
            b.trim_start_to(mid);
            b.fade_in = Fade::default();
        }
        n += 1;
    }
    for c in &mut pl.clips {
        let (fi, fo) = (c.fade_in.len, c.fade_out.len);
        let fade_in = fi > 0 && hit(c, c.start, c.start.saturating_add(fi));
        let fade_out = fo > 0 && hit(c, c.end().saturating_sub(fo), c.end());
        if fade_in {
            c.fade_in = Fade::default();
        }
        if fade_out {
            c.fade_out = Fade::default();
        }
        if fade_in || fade_out {
            n += 1;
        }
    }
    // A clip trimmed from the left can now start after its next neighbour (several short
    // crossfaded clips, e.g. after Beat Detective): keep the playlist in time order.
    pl.sort();
    n
}

/// `id` and the other clips of its clip group on every track's active playlist (`id` alone when
/// it is not grouped), `id` first, and the clips an active edit group brings along with them
/// ([`with_edit_groups`]): what a grab, a trim or a fade drawn on `id` acts on.
pub fn group_members(s: &Session, id: ClipId) -> Vec<ClipId> {
    let group = s.find_clip(id).and_then(|(_, c)| c.group);
    let mut out = vec![id];
    if let Some(g) = group {
        out.extend(s.tracks.iter().flat_map(|t| t.clips()).filter(|c| c.group == Some(g) && c.id != id).map(|c| c.id));
    }
    with_edit_groups(s, &out)
}

/// Edit-state flag: edit points snap to clip edges near them (Options › Snap to Clip Edges).
pub const SNAP_TO_CLIPS: &str = "edit.snap_to_clips";
/// Edit-state flag: edit points snap to markers near them (Options › Snap to Markers).
pub const SNAP_TO_MARKERS: &str = "edit.snap_to_markers";
/// Edit-state flag: the musical grid follows the zoom (Options › Snap › Adaptive Grid), its note
/// value the one whose lines are nearest [`ADAPTIVE_GRID_PX`] apart on screen.
pub const ADAPTIVE_GRID: &str = "edit.adaptive_grid";
/// How far apart, in points, an adaptive grid's lines aim to be.
pub const ADAPTIVE_GRID_PX: f64 = 16.0;

/// The grid an adaptive grid ([`ADAPTIVE_GRID`]) takes at the current zoom: `None` when it is
/// off or the grid is not musical (time, frame and sample grids stay as chosen). Plain note
/// values only (no dotted or triplet), measured at the left edge of the view.
pub fn adapted_grid(s: &Session) -> Option<soundcraft_time::GridValue> {
    use soundcraft_time::{GridValue, NoteValue};
    let spp = s.edit.zoom.samples_per_px;
    if !s.edit.flag(ADAPTIVE_GRID) || !matches!(s.edit.grid, GridValue::Note { .. }) || !(spp.is_finite() && spp > 0.0) {
        return None;
    }
    let ctx = s.grid_ctx();
    let at = s.edit.zoom.scroll.max(0);
    NoteValue::ALL
        .into_iter()
        .map(|value| GridValue::Note { value, dotted: false, triplet: false })
        .map(|g| {
            let px = g.step_samples(at, &ctx) as f64 / spp;
            (g, (px.max(1e-9) / ADAPTIVE_GRID_PX).ln().abs())
        })
        .min_by(|a, b| a.1.total_cmp(&b.1))
        .map(|(g, _)| g)
}

/// Edit-state flag: edit points snap to the playhead near them (Options › Snap › Playhead).
pub const SNAP_TO_CURSOR: &str = "edit.snap_to_cursor";
/// Edit-state flag: a selection's edges and the insertion snap to the nearest zero crossing of
/// the audio under them (Options › Snap to Zero Crossings).
pub const SNAP_TO_ZERO: &str = "edit.snap_to_zero_crossings";

/// The clip edge, marker or playhead nearest `at` within `reach`, as [`SNAP_TO_CLIPS`],
/// [`SNAP_TO_MARKERS`] and [`SNAP_TO_CURSOR`] ask. Clip edges count on every visible track's active playlist, clips in
/// `skip` (the ones being dragged) left out; a marker counts by its start, a range by its end too.
pub fn magnet_target(s: &Session, at: Samples, reach: Samples, skip: &[ClipId]) -> Option<Samples> {
    let reach = reach.max(0);
    let mut best: Option<Samples> = None;
    let mut take = |p: Samples| {
        let d = p.saturating_sub(at).saturating_abs();
        if d <= reach && best.is_none_or(|b| d < b.saturating_sub(at).saturating_abs()) {
            best = Some(p);
        }
    };
    if s.edit.flag(SNAP_TO_CLIPS) {
        for t in s.tracks.iter().filter(|t| !t.hidden) {
            for c in t.clips().iter().filter(|c| !skip.contains(&c.id)) {
                take(c.start);
                take(c.end());
            }
        }
    }
    if s.edit.flag(SNAP_TO_CURSOR) {
        take(s.edit.playhead);
    }
    if s.edit.flag(SNAP_TO_MARKERS) {
        for m in &s.markers {
            take(m.start);
            if m.end > m.start {
                take(m.end);
            }
        }
    }
    best
}

/// The zero crossing nearest `at` within `reach` in the audio clip under `at` on `track` (its
/// first channel, or the channel it plays): where the waveform changes sign or touches zero.
pub fn zero_crossing_near(s: &Session, track: TrackId, at: Samples, reach: Samples) -> Option<Samples> {
    let t = s.track(track)?;
    let c = t.clips().iter().rev().find(|c| c.range().contains(at) && !c.muted)?;
    let ClipContent::Audio { source, offset } = &c.content else { return None };
    let audio = s.pool.get(*source)?;
    let reach = reach.clamp(1, 1 << 16);
    let file_at = offset.saturating_add(c.source_span(at.saturating_sub(c.start)));
    let from = file_at.saturating_sub(c.source_span(reach)).max(*offset).max(0);
    let to = file_at.saturating_add(c.source_span(reach)).min(offset.saturating_add(c.source_span(c.length)));
    let n = usize::try_from(to.saturating_sub(from)).ok()?.saturating_add(1);
    if n < 2 {
        return None;
    }
    let mut buf = vec![0.0f32; n];
    let got = audio.read(c.source_channel.map_or(0, usize::from), usize::try_from(from).ok()?, &mut buf);
    let buf = buf.get(..got)?;
    let best = buf
        .windows(2)
        .enumerate()
        .filter(|(_, w)| w[1] == 0.0 || (w[0] < 0.0) != (w[1] < 0.0))
        // The sample nearer zero of the two is the crossing.
        .map(|(i, w)| if w[0].abs() < w[1].abs() { i } else { i + 1 })
        .map(|i| from.saturating_add(i as Samples))
        .min_by_key(|f| f.saturating_sub(file_at).saturating_abs())?;
    let timeline = c.start.saturating_add(c.timeline_span(best.saturating_sub(*offset)));
    Some(timeline.clamp(c.start, c.end()))
}

/// An edit point at `raw` after snapping: `gridded` (the grid's answer, or `raw` when the grid is
/// off), unless a clip edge or marker within `reach` of `raw` is nearer; then, on `track` with
/// [`SNAP_TO_ZERO`] on, the nearest zero crossing within 5 ms.
pub fn snap_point(s: &Session, raw: Samples, gridded: Samples, reach: Samples, track: Option<TrackId>, skip: &[ClipId]) -> Samples {
    let mut at = gridded;
    if let Some(m) = magnet_target(s, raw, reach, skip)
        && (gridded == raw || m.saturating_sub(raw).saturating_abs() <= gridded.saturating_sub(raw).saturating_abs())
    {
        at = m;
    }
    if s.edit.flag(SNAP_TO_ZERO)
        && let Some(t) = track
        && let Some(z) = zero_crossing_near(s, t, at, s.sample_rate.samples(0.005))
    {
        at = z;
    }
    at
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Source, SourceId, TrackKind};

    fn session_with_clip() -> (Session, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        s.sources.push(Source {
            id: SourceId(500),
            name: "src".into(),
            path: "src.wav".into(),
            channels: 1,
            frames: 10_000,
            sample_rate: 48_000,
            format: soundcraft_audio_io::FileFormat::Wav,
            time_reference: 0,
            unsaved: false,
        });
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(Clip::audio(id, "a", SourceId(500), 1000, 1000, 4000));
        }
        (s, t)
    }

    fn clips(s: &Session, t: TrackId) -> Vec<(Samples, Samples, Samples)> {
        s.track(t).unwrap().clips().iter().map(|c| (c.start, c.length, c.source_offset())).collect()
    }

    #[test]
    fn separate_splits_and_heal_rejoins() {
        let (mut s, t) = session_with_clip();
        assert_eq!(separate_at(&mut s, t, 3000), 1);
        assert_eq!(clips(&s, t), vec![(1000, 2000, 1000), (3000, 2000, 3000)]);
        assert_eq!(separate_at(&mut s, t, 1000), 0);
        assert_eq!(heal(&mut s, t, Range::new(0, 10_000)), 1);
        assert_eq!(clips(&s, t), vec![(1000, 4000, 1000)]);
    }

    #[test]
    fn clear_slip_and_shuffle() {
        let (mut s, t) = session_with_clip();
        let removed = clear_range(&mut s, t, Range::new(2000, 3000), false);
        assert_eq!(removed.len(), 1);
        assert_eq!(removed[0].start, 0);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (3000, 2000, 3000)]);
        let (mut s, t) = session_with_clip();
        clear_range(&mut s, t, Range::new(2000, 3000), true);
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 2000, 3000)]);
    }

    #[test]
    fn copy_paste_overwrites() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1500, 2500));
        assert_eq!(copied.len(), 1);
        assert_eq!((copied[0].start, copied[0].length, copied[0].source_offset()), (0, 1000, 1500));
        paste(&mut s, t, 4000, &copied, 1000, false).unwrap();
        assert_eq!(clips(&s, t), vec![(1000, 3000, 1000), (4000, 1000, 1500)]);
    }

    #[test]
    fn paste_shuffle_ripples() {
        let (mut s, t) = session_with_clip();
        let copied = copy_range(&s, t, Range::new(1000, 2000));
        paste(&mut s, t, 1000, &copied, 1000, true).unwrap();
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2000, 4000, 1000)]);
    }

    #[test]
    fn insert_silence_and_trim() {
        let (mut s, t) = session_with_clip();
        insert_silence(&mut s, t, Range::new(2000, 2500));
        assert_eq!(clips(&s, t), vec![(1000, 1000, 1000), (2500, 3000, 2000)]);
        let (mut s, t) = session_with_clip();
        trim_to(&mut s, t, Range::new(1500, 2000));
        assert_eq!(clips(&s, t), vec![(1500, 500, 1500)]);
    }

    #[test]
    fn fades_and_crossfades() {
        let (mut s, t) = session_with_clip();
        assert_eq!(create_fades(&mut s, t, Range::new(500, 1500), FadeShape::Linear), 1);
        assert_eq!(s.track(t).unwrap().clips()[0].fade_in.len, 500);
        let (mut s, t) = session_with_clip();
        separate_at(&mut s, t, 3000);
        assert_eq!(create_fades(&mut s, t, Range::new(2900, 3100), FadeShape::EqualPower), 1);
        let c = s.track(t).unwrap().clips();
        assert_eq!(c[0].end(), 3100);
        assert_eq!(c[1].start, 2900);
        assert_eq!(c[0].fade_out.len, 200);
        assert_eq!(c[1].fade_in.len, 200);
        delete_fades(&mut s, t, Range::new(2900, 3100));
        let c = s.track(t).unwrap().clips();
        assert_eq!((c[0].end(), c[1].start), (3000, 3000));
    }

    fn three_clips() -> (Session, TrackId, [ClipId; 3]) {
        let (mut s, t) = session_with_clip();
        let ids = [s.new_clip_id(), s.new_clip_id(), s.new_clip_id()];
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips = vec![
                Clip::audio(ids[0], "a", SourceId(500), 0, 0, 1000),
                Clip::audio(ids[1], "b", SourceId(500), 1000, 1000, 2000),
                Clip::audio(ids[2], "c", SourceId(500), 3000, 5000, 1000),
            ];
        }
        (s, t, ids)
    }

    #[test]
    fn shuffle_moves_reorder_clips_and_keep_the_silence() {
        let (mut s, t, [_, b, _]) = three_clips();
        assert_eq!(shuffle_clips(&mut s, &[b], -900, &[]), Ok(1));
        assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
        let (mut s, t, [a, ..]) = three_clips();
        assert_eq!(shuffle_slot(&s, t, 900, &[a]), 0);
        assert_eq!(shuffle_slot(&s, t, 1100, &[a]), 2000);
        shuffle_clips(&mut s, &[a], 2500, &[]).unwrap();
        assert_eq!(clips(&s, t), vec![(0, 2000, 1000), (2000, 1000, 0), (5000, 1000, 3000)]);
    }

    #[test]
    fn shuffling_onto_an_empty_track_starts_it_at_zero() {
        let (mut s, t, [_, _, c]) = three_clips();
        let other = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        shuffle_clips(&mut s, &[c], 40_000, &[(t, other)]).unwrap();
        assert_eq!(clips(&s, other), vec![(0, 1000, 3000)]);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000)]);
    }

    #[test]
    fn shuffle_trims_move_later_clips_and_slip_trims_stop_at_neighbours() {
        let (mut s, t, [a, b, c]) = three_clips();
        trim_clip(&mut s, b, None, Some(2500), true);
        trim_clip(&mut s, b, Some(1500), None, true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 1000, 1500), (4000, 1000, 3000)]);
        let (mut s, t, _) = three_clips();
        trim_clip(&mut s, a, None, Some(2500), false);
        trim_clip(&mut s, c, Some(2000), None, false);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 2000, 1000), (3000, 3000, 1000)]);
        trim_clip(&mut s, c, None, Some(99_999), false);
        assert_eq!(clips(&s, t)[2], (3000, 9000, 1000));
    }

    #[test]
    fn shuffle_leaves_locked_clips_and_what_follows_them_in_place() {
        let (mut s, t, [a, b, c]) = three_clips();
        s.find_clip_mut(c).unwrap().time_locked = true;
        clear_range(&mut s, t, Range::new(1000, 3000), true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (5000, 1000, 3000)]);
        let (mut s, t, [a2, ..]) = three_clips();
        s.find_clip_mut(c).unwrap().edit_locked = true;
        assert_eq!(shuffle_room(&s, t, 0), 2000);
        assert_eq!(make_room(&mut s, t, 0, 2500), Err(NO_ROOM));
        assert_eq!(make_room(&mut s, t, 0, 2000), Ok(()));
        assert_eq!(clips(&s, t), vec![(2000, 1000, 0), (3000, 2000, 1000), (5000, 1000, 3000)]);
        assert_eq!(shuffle_clips(&mut s, &[a2], 2500, &[]), Ok(1));
        assert_eq!(clips(&s, t)[2], (5000, 1000, 3000));
        let (mut s, t, _) = three_clips();
        s.find_clip_mut(c).unwrap().time_locked = true;
        trim_clip(&mut s, b, None, Some(9000), true);
        assert_eq!(clips(&s, t), vec![(0, 1000, 0), (1000, 4000, 1000), (5000, 1000, 3000)]);
        assert_eq!(shuffle_slot(&s, t, 4000, &[a]), 4000);
    }

    #[test]
    fn multi_track_moves_keep_the_tracks_apart() {
        let (mut s, t1, [a, ..]) = three_clips();
        let t2 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let t3 = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
        let m = s.add_track(TrackKind::Midi, ChannelFormat::Mono, None);
        let d = s.new_clip_id();
        s.track_mut(t2).and_then(|t| t.playlist_mut()).unwrap().clips.push(Clip::audio(d, "d", SourceId(500), 0, 0, 500));
        let dest = destinations(&s, &[a, d], t1, t2);
        assert_eq!(dest, Some(vec![(t1, t2), (t2, t3)]));
        assert_eq!(destinations(&s, &[a, d], t1, m), None);
        assert_eq!(destinations(&s, &[a], t1, m), None);
        assert_eq!(move_clips(&mut s, &[a, d], 0, &dest.unwrap()), 2);
        assert_eq!(clips(&s, t2), vec![(0, 1000, 0)]);
        assert_eq!(clips(&s, t3), vec![(0, 500, 0)]);
    }

    #[test]
    fn grid_moves_snap_the_clip_start_or_keep_its_offset() {
        let (s, t) = session_with_clip();
        let step = s.edit.grid.step_samples(0, &s.grid_ctx());
        let c = s.track(t).unwrap().clips()[0].clone();
        assert_eq!(c.start + grid_delta(&s, &c, step + 200, false), step);
        assert_eq!(c.start + grid_delta(&s, &c, step + 200, true), step + 1000);
        assert_eq!(grid_delta(&s, &c, step / 3, true), 0);
    }

    #[test]
    fn moves_overwrite_and_respect_locks() {
        let (mut s, t) = session_with_clip();
        let id = s.track(t).unwrap().clips()[0].id;
        assert_eq!(move_clips(&mut s, &[id], 500, &[]), 1);
        assert_eq!(clips(&s, t), vec![(1500, 4000, 1000)]);
        s.find_clip_mut(id).unwrap().time_locked = true;
        assert_eq!(move_clips(&mut s, &[id], 500, &[]), 0);
    }
}

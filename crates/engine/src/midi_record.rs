//! Recording hardware MIDI input (a USB keyboard, an interface's MIDI In, Musical Typing) into
//! MIDI clips on the record-armed instrument and MIDI tracks whose input is that MIDI
//! ([`Route::Midi`]).
//!
//! The audio thread hands back what the mix heard while recording, each event at the timeline
//! position it arrived at (from its raw age at the callback's start, so neither the one block every
//! live event waits, `midi_in::block_offset`, nor a block cut short by a loop's end or a callback
//! split moves it) and with the loop pass it played in. When the take ends, [`record_live_midi`] places it:
//!
//! - **Latency**: the musician played what they heard, which left the speakers later than the
//!   block that carried their event was rendered. Each event moves earlier by
//!   [`MidiTake::latency`]: the output latency the record-offset system reads from the device
//!   (playback − callback) and the mix's own delay (delay compensation). The audio
//!   record offset's manual and measured parts are for the input path and don't apply. Loop
//!   recording: an event that moves before the loop's start was played at the end of the pass
//!   before. One heard from the take's start on that moves before it begins the take (notes
//!   played between the Record press and the transport rolling are heard on its first frame).
//! - **Range**: a clip covers the record range (from where recording started to where it
//!   stopped), cut to the punch range; loop recording makes a clip over the loop per pass with
//!   anything in it, the first on the track's playlist and each later one on a new playlist that
//!   starts as a copy of the track's (the comp layers are for audio takes). The first pass runs
//!   from where recording started to the loop's end: Record pressed partway through a playing
//!   loop, or before it with the transport heading into it, records the rest of that pass.
//! - Each track takes the events its input lets through (its port, or all; Omni or one channel),
//!   channels kept; notes still held at the range's end end there; notes begun before it are
//!   left out. Controllers (sustain, mod wheel, pitch bend, aftertouch, program) are kept.
//! - **Input Quantize** (Event › MIDI Operations › Input Quantize) quantizes the notes on the
//!   session's grid when it is on; the played performance is kept to restore.
//! - **Record mode** ([`RecordMode`]): Replace clears what the take covers; Merge (the MIDI Merge
//!   switch) joins the take into the MIDI clips it overlaps, loop passes adding up in one clip;
//!   New Playlist puts the take on a copy of the track's playlist, the old one kept; Keep Takes
//!   as Lanes puts each take (each loop pass) on a take lane of its own, as audio takes are
//!   ([`crate::cmd::comp::record_pass`]): the comp plays the newest, and the earlier ones stay
//!   to pick or swipe from. A track that already comps (its lanes open, a record target, take
//!   lanes) records into lanes in every mode but Merge.
//! - One undo step, "Record".

use soundcraft_midi::Sequence;
use soundcraft_model::{Clip, ClipId, Playlist, Route, Session, TrackId};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER};

use crate::Engine;

/// One recorded event: its timeline position as heard, the port it came from (by name), the
/// message, and its loop pass (0 without a loop).
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct RecordedEvent {
    pub at: Samples,
    pub port: String,
    pub msg: [u8; 3],
    pub pass: u16,
}

/// Where a MIDI take goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MidiTake {
    /// Where recording started and stopped (the record range).
    pub start: Samples,
    pub end: Samples,
    /// Loop recording: the loop each pass covers.
    pub looped: Option<Range>,
    /// Punch recording: the range kept.
    pub punch: Option<Range>,
    /// How much earlier each event belongs than where it was heard (see the module docs).
    pub latency: Samples,
}

/// The record-armed instrument and MIDI tracks whose input is MIDI (frozen and inactive ones
/// are left out).
pub fn armed_midi_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| t.kind.is_midi() && t.mixer.record_arm && matches!(t.mixer.input, Route::Midi(_)) && t.frozen_audio().is_none() && !t.inactive)
        .map(|t| t.id)
        .collect()
}

/// Places `events` as `take` says on the armed MIDI tracks (see the module docs). Returns the
/// new clips.
pub fn record_live_midi(e: &mut Engine, take: &MidiTake, events: &[RecordedEvent]) -> Vec<ClipId> {
    let tracks = armed_midi_tracks(e.session());
    record_midi_on(e, &tracks, take, events)
}

/// Where an event heard at `at` in loop pass `pass` was played: `take.latency` earlier, and one
/// heard in the first `latency` samples after a loop wrap belongs to the end of the pass before.
/// One heard from the take's start on that would move before it begins the take: a note played
/// between the Record press and the transport rolling (the player hears those on the take's first
/// frame), or a first downbeat played a little early.
pub fn played_at(take: &MidiTake, at: Samples, pass: u16) -> (Samples, u16) {
    let heard = at;
    let at = at.saturating_sub(take.latency);
    match take.looped {
        Some(l) if at < l.start && pass > 0 => (at.saturating_add(l.len()), pass - 1),
        _ if pass == 0 && heard >= take.start && at < take.start => (take.start, 0),
        _ => (at, pass),
    }
}

/// The instrument and MIDI tracks whose input is exactly the port `device` (armed or not; frozen
/// and inactive ones left out): where a take from a remote recorder lands
/// (`Remote: <PC> › <port>`).
pub fn tracks_on_port(s: &Session, device: &str) -> Vec<TrackId> {
    s.tracks
        .iter()
        .filter(|t| {
            t.kind.is_midi()
                && matches!(&t.mixer.input, Route::Midi(m) if m.device.as_deref() == Some(device))
                && t.frozen_audio().is_none()
                && !t.inactive
        })
        .map(|t| t.id)
        .collect()
}

/// [`record_live_midi`] on the given `tracks` (each still takes only what its input lets
/// through). One undo step, "Record", when anything was placed.
pub fn record_midi_on(e: &mut Engine, tracks: &[TrackId], take: &MidiTake, events: &[RecordedEvent]) -> Vec<ClipId> {
    if tracks.is_empty() || events.is_empty() {
        return Vec::new();
    }
    let before = e.session_arc();
    let out = place_take(e.session_mut(), tracks, take, events);
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
}

/// [`record_live_midi`] for a take punched out and in on the fly (QuickPunch: `punches`, the last
/// possibly open): the take is kept to each punch (as the audio takes are), what was played
/// between them left out. Without punches, the whole take.
pub fn record_punches(e: &mut Engine, take: &MidiTake, punches: &[Range], events: &[RecordedEvent]) -> Vec<ClipId> {
    if punches.is_empty() {
        return record_live_midi(e, take, events);
    }
    let mut ids = Vec::new();
    for p in punches {
        let Some(r) = Range::new(p.start, p.end.min(take.end)).intersect(&take.punch.unwrap_or(*p)).filter(|r| !r.is_empty()) else {
            continue;
        };
        ids.extend(record_live_midi(e, &MidiTake { punch: Some(r), ..*take }, events));
    }
    ids
}

/// [`record_midi_on`] without the undo step, on a session: what the loop-recording preview puts
/// on the tracks while the loop still runs (see [`LoopPreview`]).
pub fn place_take(s: &mut Session, tracks: &[TrackId], take: &MidiTake, events: &[RecordedEvent]) -> Vec<ClipId> {
    place_passes(s, tracks, take, events, None)
}

/// [`place_take`], placing only the passes before `upto` when it is given (the loop is still in
/// pass `upto`: the passes are counted to it, so the early notes at a finished pass's end start
/// the next as they will at Stop).
fn place_passes(s: &mut Session, tracks: &[TrackId], take: &MidiTake, events: &[RecordedEvent], upto: Option<u16>) -> Vec<ClipId> {
    let tracks = tracks.to_vec();
    if tracks.is_empty() || events.is_empty() {
        return Vec::new();
    }
    let mode = record_mode(s);
    let quantize = s.edit.flag("midi.input_quantize");
    let opts = crate::cmd::event_more::quantize_opts(s);
    let passes = events.iter().map(|x| x.pass).max().unwrap_or(0).max(upto.unwrap_or(0));
    let pickup = with_pickup(s, take, events);
    let take = &pickup;
    let mut out = Vec::new();
    for tid in tracks {
        let Some(Route::Midi(input)) = s.track(tid).map(|t| t.mixer.input.clone()) else { continue };
        let filter = |x: &&RecordedEvent| {
            input.device.as_ref().is_none_or(|d| *d == x.port) && (input.channel == 0 || x.msg[0] >= 0xF0 || (x.msg[0] & 0x0F) + 1 == input.channel)
        };
        // Each event where it was played: `latency` earlier, and an event heard in the first
        // `latency` samples after a loop wrap was played at the end of the pass before. A track
        // with a MIDI Track Offset plays its notes that much later: they go that much earlier,
        // so the take plays back where it was played.
        let offset = s.edit.value(&format!("midi.offset.{}", tid.0), 0.0);
        let offset = if offset.is_finite() { offset as Samples } else { 0 };
        // The track's input transform, as its instrument heard it.
        let transform = input.transform;
        let mine: Vec<(Samples, [u8; 3], u16)> = events
            .iter()
            .filter(filter)
            .filter_map(|x| {
                let msg = transform.apply(x.msg)?;
                let (at, pass) = played_at(take, x.at, x.pass);
                // (Still not before the take's start, as `played_at` keeps it.)
                let placed = at.saturating_sub(offset);
                let placed = if pass == 0 && x.at >= take.start { placed.max(take.start) } else { placed };
                Some((placed, msg, pass))
            })
            .collect();
        // The playlist as it was before this take: each loop pass after the first goes on a copy
        // of it, so the rest of the song stays on the track whichever pass is active (and with
        // New Playlist, the first too).
        // Into take lanes: each pass on a lane of its own, the comp playing the newest.
        let lanes = mode == RecordMode::TakeLanes || (mode != RecordMode::Merge && s.track(tid).is_some_and(crate::cmd::comp::comping));
        let base: Vec<Clip> = if !lanes && (take.looped.is_some() || mode == RecordMode::NewPlaylist) {
            s.track(tid).map(|t| t.clips().to_vec()).unwrap_or_default()
        } else {
            Vec::new()
        };
        let mut placed_first = false;
        // A downbeat played a little early, in the last moment of a pass, starts the next one.
        let window = (TICKS_PER_QUARTER / 8).max(if quantize { opts.grid_ticks / 2 } else { 0 });
        let mine = settle_early_notes(s, take, passes, mine, window, quantize);
        for pass in 0..=passes {
            if upto.is_some_and(|u| pass >= u) {
                break;
            }
            let Some(range) = pass_range(take, pass, passes) else { continue };
            // A pedal down when the range starts (pressed before the punch point or the take, or
            // held over a loop wrap) is down from its start.
            let mut list: Vec<(Samples, [u8; 3], bool)> =
                pedals_down_at(&mine, pass, range.start).into_iter().map(|m| (range.start, m, true)).collect();
            list.extend(mine.iter().filter(|x| x.2 == pass).map(|&(at, msg, _)| (at, msg, true)).filter(|(at, _, _)| range.contains(*at)));
            let mut seq: Sequence = crate::band_print::events_to_sequence(s, range, &list);
            // One still down when it ends comes up on its last tick (the clip plays nothing on its
            // end), or it would hold every note after the clip.
            let last = s.tempo.samples_to_ticks(range.end, s.sample_rate).saturating_sub(s.tempo.samples_to_ticks(range.start, s.sample_rate));
            lift_pedals(&mut seq, last);
            if seq.notes.is_empty() && seq.ctrls.is_empty() {
                continue;
            }
            // Merged, the clip holds what was there too: the performance kept is a new clip's.
            let performance = (quantize && mode != RecordMode::Merge).then(|| seq.clone());
            if quantize {
                // On the session's grid: the notes' ticks from the session start while quantizing.
                let base = s.tempo.samples_to_ticks(range.start, s.sample_rate);
                seq.notes.iter_mut().for_each(|n| n.start = n.start.saturating_add(base));
                soundcraft_midi::ops::quantize(&mut seq.notes, &opts);
                seq.notes.iter_mut().for_each(|n| n.start = n.start.saturating_sub(base).max(0));
                seq.sort();
            }
            // Each pass after the first that went down on this track (and with New Playlist the
            // first): a new playlist, holding what the track held before the take. Merged passes
            // add up in one clip instead.
            if !lanes && mode != RecordMode::Merge && (placed_first || mode == RecordMode::NewPlaylist) {
                let mut clips = base.clone();
                for c in &mut clips {
                    c.id = s.new_clip_id();
                }
                if let Some(t) = s.track_mut(tid) {
                    let mut pl = Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1));
                    pl.clips = clips;
                    t.playlists.push(pl);
                    t.active_playlist = t.playlists.len() - 1;
                }
            }
            let id = if mode == RecordMode::Merge {
                merge_take(s, tid, range, seq)
            } else if lanes {
                let id = s.new_clip_id();
                let name = s.track(tid).map(|t| t.name.clone()).unwrap_or_default();
                let mut clip = Clip::midi(id, name, range.start, range.len(), seq);
                clip.midi_channels = true;
                crate::cmd::comp::record_pass(s, tid, clip)
            } else {
                let id = s.new_clip_id();
                let name = s.track(tid).map(|t| t.name.clone()).unwrap_or_default();
                let mut clip = Clip::midi(id, name, range.start, range.len(), seq);
                clip.midi_channels = true;
                crate::edit::place_clip(s, tid, clip);
                id
            };
            if let Some(p) = performance {
                s.midi_originals.insert(id.0, p);
            }
            placed_first = true;
            // Merged, an earlier pass's clip went into this one (a clip on another playlist is
            // not found, so only then).
            if mode == RecordMode::Merge {
                out.retain(|c| s.find_clip(*c).is_some());
            }
            out.push(id);
        }
    }
    out
}

/// What a take does to the MIDI under it (Options › MIDI Record Mode; the transport's MIDI Merge
/// switch turns Merge on and off).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecordMode {
    /// The take's range is cleared and the take put there.
    Replace,
    /// The take joins the MIDI clips it overlaps into one clip with both: overdub. Loop passes
    /// add up in it, drum-machine style. The default for new sessions.
    Merge,
    /// The take goes on a new playlist that starts as a copy of the track's; the playlist it
    /// replaced keeps what was there.
    NewPlaylist,
    /// Keep Takes as Lanes: each take (each loop pass) goes on a take lane of its own and the
    /// comp plays the newest; the takes before stay on their lanes to pick or swipe from.
    TakeLanes,
}

impl RecordMode {
    pub const ALL: [(RecordMode, &'static str); 4] = [
        (RecordMode::Replace, "replace"),
        (RecordMode::Merge, "merge"),
        (RecordMode::NewPlaylist, "new_playlist"),
        (RecordMode::TakeLanes, "take_lanes"),
    ];

    /// Its name in commands and the session (`replace`, `merge`, `new_playlist`).
    pub fn name(self) -> &'static str {
        RecordMode::ALL.iter().find(|(m, _)| *m == self).map_or("replace", |(_, n)| n)
    }
}

/// The session's MIDI record mode: Merge while MIDI Merge is on, else the stored one
/// (`midi.record_mode`: 2 New Playlist, 3 Keep Takes as Lanes, else Replace).
pub fn record_mode(s: &Session) -> RecordMode {
    let stored = s.edit.value("midi.record_mode", 0.0);
    if s.edit.midi_merge {
        RecordMode::Merge
    } else if stored == 2.0 {
        RecordMode::NewPlaylist
    } else if stored == 3.0 {
        RecordMode::TakeLanes
    } else {
        RecordMode::Replace
    }
}

/// Sets the session's MIDI record mode (MIDI Merge on exactly for Merge).
pub fn set_record_mode(s: &mut Session, mode: RecordMode) {
    s.edit.midi_merge = mode == RecordMode::Merge;
    let v = match mode {
        RecordMode::TakeLanes => 3.0,
        RecordMode::NewPlaylist => 2.0,
        RecordMode::Merge => 1.0,
        RecordMode::Replace => 0.0,
    };
    s.edit.values.insert("midi.record_mode".into(), v);
}

/// Loop recording while the loop still runs: the passes finished so far are put on the armed
/// tracks as they will be at Stop, so the next pass plays them (Merge: they add up in one clip;
/// otherwise the last finished pass is on top). Nothing here is an undo step: before each showing,
/// and before the take is placed for good, the tracks get back the playlists they had when the
/// take began ([`LoopPreview::restore`]); only those tracks' playlists are touched, so mixing,
/// automation written meanwhile and other tracks stay. Undo Last Pass while looping leaves a
/// finished pass out of the preview and of the take ([`LoopPreview::undo_last_pass`]).
#[derive(Debug, Clone, Default)]
pub struct LoopPreview {
    /// Each armed track's playlists, active playlist, record target and open lanes before the take.
    saved: Vec<(TrackId, Vec<Playlist>, usize, Option<usize>, bool)>,
    /// Clips the preview made (their kept performances go with them).
    made: Vec<ClipId>,
    /// The passes before this one are shown.
    shown: u16,
    /// Passes left out (Undo Last Pass).
    dropped: Vec<u16>,
    /// What is on the tracks now is the preview's.
    applied: bool,
}

impl LoopPreview {
    /// For a take on `tracks` of `s` as it is now (before anything was placed).
    pub fn new(s: &Session, tracks: &[TrackId]) -> LoopPreview {
        let saved = tracks
            .iter()
            .filter_map(|t| s.track(*t).map(|tr| (*t, tr.playlists.clone(), tr.active_playlist, tr.record_layer, tr.layers_open)))
            .collect();
        LoopPreview { saved, ..LoopPreview::default() }
    }

    /// The passes before this one are on the tracks.
    pub fn shown(&self) -> u16 {
        self.shown
    }

    /// The passes Undo Last Pass left out.
    pub fn dropped(&self) -> &[u16] {
        &self.dropped
    }

    /// `events` without the passes left out.
    pub fn keep(&self, events: &[RecordedEvent]) -> Vec<RecordedEvent> {
        events.iter().filter(|x| !self.dropped.contains(&x.pass)).cloned().collect()
    }

    /// Puts the passes before `upto` of `events` (the loop runs in pass `upto`) on the tracks,
    /// as `take` would at Stop, the dropped ones left out. Returns whether the tracks changed.
    pub fn show(&mut self, s: &mut Session, take: &MidiTake, events: &[RecordedEvent], upto: u16) -> bool {
        if upto == 0 || take.looped.is_none() {
            return false;
        }
        self.restore(s);
        self.shown = upto;
        let tracks: Vec<TrackId> = self.saved.iter().map(|x| x.0).collect();
        let kept = self.keep(events);
        self.made = place_passes(s, &tracks, take, &kept, Some(upto));
        self.applied = true;
        true
    }

    /// `s` with the preview taken off, when it is on (what an autosave writes while the loop
    /// runs: a recovery places the whole take from what was spilled, once).
    pub fn lifted(&self, s: &Session) -> Option<Session> {
        if !self.applied {
            return None;
        }
        let mut s = s.clone();
        self.clone().restore(&mut s);
        Some(s)
    }

    /// Takes the preview off: the tracks as they were before the take.
    pub fn restore(&mut self, s: &mut Session) {
        if !std::mem::take(&mut self.applied) {
            return;
        }
        for (tid, playlists, active, record_layer, layers_open) in &self.saved {
            if let Some(t) = s.track_mut(*tid) {
                t.playlists = playlists.clone();
                t.active_playlist = *active;
                t.record_layer = *record_layer;
                t.layers_open = *layers_open;
            }
        }
        for id in self.made.drain(..) {
            s.midi_originals.remove(&id.0);
        }
    }

    /// Leaves the last finished pass still in (one before [`LoopPreview::shown`]) out of the
    /// preview and the take. Returns it; `None` when no finished pass is left.
    pub fn undo_last_pass(&mut self) -> Option<u16> {
        let pass = (0..self.shown).rev().find(|p| !self.dropped.contains(p))?;
        self.dropped.push(pass);
        Some(pass)
    }
}

/// Merge mode: `take` (a sequence over `range`, its channels kept) joins the unmuted MIDI clips
/// on `track` that overlap `range` in one clip over all of them, keeping the first one's name.
/// Returns its id.
fn merge_take(s: &mut Session, track: TrackId, range: Range, take: Sequence) -> ClipId {
    let sr = s.sample_rate;
    let under: Vec<&Clip> = s
        .track(track)
        .map(|t| t.clips().iter().filter(|c| !c.muted && c.midi_playing().is_some() && c.start < range.end && c.end() > range.start).collect())
        .unwrap_or_default();
    let span = under.iter().fold(range, |r, c| Range::new(r.start.min(c.start), r.end.max(c.end())));
    let name = under.first().map(|c| c.name.clone()).or_else(|| s.track(track).map(|t| t.name.clone())).unwrap_or_default();
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr);
    let base = tick(span.start);
    let mut merged = Sequence::default();
    for part in crate::edit::copy_range(s, track, span).into_iter().filter(|c| !c.muted) {
        let at = tick(span.start.saturating_add(part.start)).saturating_sub(base);
        let len = tick(span.start.saturating_add(part.end())).saturating_sub(base).saturating_sub(at);
        let flatten = !part.midi_channels;
        let soundcraft_model::ClipContent::Midi { mut sequence } = part.content else { continue };
        if flatten {
            // It played on channel 1: so it does in the merged clip, which keeps channels.
            crate::edit::channel_one(&mut sequence);
        }
        for mut n in sequence.notes.into_iter().filter(|n| (0..len).contains(&n.start)) {
            n.length = n.length.min(len - n.start);
            n.start += at;
            merged.notes.push(n);
        }
        for mut c in sequence.ctrls.into_iter().filter(|c| (0..len).contains(&c.tick)) {
            c.tick += at;
            merged.ctrls.push(c);
        }
    }
    let at = tick(range.start).saturating_sub(base);
    merged.notes.extend(take.notes.into_iter().map(|mut n| {
        n.start += at;
        n
    }));
    merged.ctrls.extend(take.ctrls.into_iter().map(|mut c| {
        c.tick += at;
        c
    }));
    merged.sort();
    let id = s.new_clip_id();
    let mut clip = Clip::midi(id, name, span.start, span.len(), merged);
    clip.midi_channels = true;
    crate::edit::place_clip(s, track, clip);
    id
}

/// Notes played during the count-off are kept as a pickup when Setup › Click/Countoff asks
/// (`midi.keep_pickup`): the take then begins at the start of the bar the first of them was
/// played in (not before the session's start), so they land where they were played. Without a
/// punch range, and only notes: a controller alone moved in the count-off does not extend it.
pub fn with_pickup(s: &Session, take: &MidiTake, events: &[RecordedEvent]) -> MidiTake {
    if !s.edit.flag("midi.keep_pickup") || take.punch.is_some() {
        return *take;
    }
    let first = events
        .iter()
        .filter(|x| x.pass == 0 && x.at < take.start && x.msg[0] & 0xF0 == 0x90 && x.msg[2] > 0)
        .map(|x| x.at.saturating_sub(take.latency))
        .min();
    let Some(first) = first.filter(|f| *f < take.start) else { return *take };
    let sr = s.sample_rate;
    let bar = s.tempo.bar_beat_at_tick(s.tempo.samples_to_ticks(first.max(0), sr)).bar;
    let start = s.tempo.tick_to_samples(s.tempo.bar_start_tick(bar), sr).clamp(0, take.start);
    MidiTake { start, ..*take }
}

/// The range pass `pass` of `passes` covers (see the module docs), cut to the punch range;
/// `None` when it is empty.
fn pass_range(take: &MidiTake, pass: u16, passes: u16) -> Option<Range> {
    let range = match take.looped {
        // The first pass runs from where recording started to the loop's end: the rest of the
        // loop when punched in partway through it, or up to it from before it. (Stopped before
        // ever reaching that loop: where it stopped.)
        Some(l) if pass == 0 && take.start < l.start && passes == 0 => Some(Range::new(take.start, take.end.clamp(take.start, l.end))),
        Some(l) if pass == 0 && take.start < l.end => Some(Range::new(take.start, l.end)),
        Some(l) => Some(l),
        None if pass == 0 => Some(Range::new(take.start, take.end.max(take.start))),
        None => None,
    };
    range.and_then(|r| r.intersect(&take.punch.unwrap_or(r))).filter(|r| !r.is_empty())
}

/// Notes begun in the last `window` ticks of a pass and still held at its end (or, with input
/// quantize on, which would snap them onto its end, released too) were played early for the next
/// downbeat: in a loop they start the next pass (their note-off with them). At the end of the
/// take, where no pass follows, they stay as played (a take whose end is its last event, as a
/// remote recorder's can be, would otherwise lose its last note). `events` are where played,
/// with their pass.
fn settle_early_notes(
    s: &Session,
    take: &MidiTake,
    passes: u16,
    mut events: Vec<(Samples, [u8; 3], u16)>,
    window: i64,
    quantize: bool,
) -> Vec<(Samples, [u8; 3], u16)> {
    let sr = s.sample_rate;
    let is_on = |m: [u8; 3]| m[0] & 0xF0 == 0x90 && m[2] > 0;
    let is_off = |m: [u8; 3]| m[0] & 0xF0 == 0x80 || (m[0] & 0xF0 == 0x90 && m[2] == 0);
    let mut order: Vec<usize> = (0..events.len()).collect();
    order.sort_by_key(|&i| events.get(i).map(|x| (x.2, x.0)));
    for (k, &i) in order.iter().enumerate() {
        let Some(&(at, msg, pass)) = events.get(i) else { continue };
        if !is_on(msg) {
            continue;
        }
        let Some(range) = pass_range(take, pass, passes) else { continue };
        let from = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(range.end, sr).saturating_sub(window), sr);
        if at < from.max(range.start) || at >= range.end {
            continue;
        }
        // Its note-off in the same pass, before the pass ends.
        let off = order
            .get(k + 1..)
            .unwrap_or_default()
            .iter()
            .copied()
            .find(|&j| events.get(j).is_some_and(|x| x.2 == pass && is_off(x.1) && x.1[0] & 0x0F == msg[0] & 0x0F && x.1[1] == msg[1]));
        let off = off.filter(|&j| events.get(j).is_some_and(|x| x.0 < range.end));
        if off.is_some() && !quantize {
            continue;
        }
        let Some(l) = take.looped.filter(|_| pass < passes) else { continue };
        if let Some(x) = events.get_mut(i) {
            *x = (l.start, msg, pass + 1);
        }
        if let Some(x) = off.and_then(|j| events.get_mut(j)) {
            *x = (l.start.saturating_add(x.0.saturating_sub(at)), x.1, pass + 1);
        }
    }
    events
}

/// The sustain and sostenuto pedals (CC 64, 66) down at `(pass, at)` by `events` (where played,
/// with their pass), as messages that press them.
fn pedals_down_at(events: &[(Samples, [u8; 3], u16)], pass: u16, at: Samples) -> Vec<[u8; 3]> {
    let mut before: Vec<&(Samples, [u8; 3], u16)> =
        events.iter().filter(|x| (x.2, x.0) < (pass, at) && x.1[0] & 0xF0 == 0xB0 && matches!(x.1[1], 64 | 66)).collect();
    before.sort_by_key(|x| (x.2, x.0));
    // By channel and pedal: the last value.
    let mut down = [[false; 2]; 16];
    for x in before {
        if let Some(d) = down.get_mut(usize::from(x.1[0] & 0x0F)).and_then(|c| c.get_mut(usize::from(x.1[1] == 66))) {
            *d = x.1[2] >= 64;
        }
    }
    let mut out = Vec::new();
    for (ch, pedals) in (0u8..).zip(down) {
        for (cc, d) in [64u8, 66].into_iter().zip(pedals) {
            if d {
                out.push([0xB0 | ch, cc, 127]);
            }
        }
    }
    out
}

/// Lifts the sustain and sostenuto pedals `seq` leaves down, at tick `len - 1` (or the pedal's
/// own tick, if later) of a clip `len` ticks long.
fn lift_pedals(seq: &mut Sequence, len: i64) {
    use soundcraft_midi::{CtrlEvent, CtrlKind};
    let mut last: Vec<(u8, u8, i64, i32)> = Vec::new();
    for c in &seq.ctrls {
        let CtrlKind::Cc(cc @ (64 | 66)) = c.kind else { continue };
        match last.iter_mut().find(|x| x.0 == c.channel && x.1 == cc) {
            Some(x) if c.tick >= x.2 => (x.2, x.3) = (c.tick, c.value),
            Some(_) => {}
            None => last.push((c.channel, cc, c.tick, c.value)),
        }
    }
    let lifts: Vec<CtrlEvent> = last
        .into_iter()
        .filter(|x| x.3 >= 64)
        .map(|(channel, cc, tick, _)| CtrlEvent { tick: len.saturating_sub(1).max(tick), channel, kind: CtrlKind::Cc(cc), value: 0 })
        .collect();
    if !lifts.is_empty() {
        seq.ctrls.extend(lifts);
        seq.sort();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use soundcraft_midi::CtrlKind;
    use soundcraft_time::TICKS_PER_QUARTER;

    /// An armed instrument track on `input` (default 120 bpm, 48 kHz: a quarter is 24000 samples).
    /// A new armed instrument track on `input`. The tests record in Replace unless they set
    /// another mode (new sessions start in Merge).
    fn armed(e: &mut Engine, input: &str, channel: u8) -> TrackId {
        set_record_mode(e.session_mut(), RecordMode::Replace);
        let id = e.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap()["tracks"][0].as_u64().unwrap();
        e.execute("track.input", &json!({"tracks": [id], "input": input, "channel": channel})).unwrap();
        e.execute("mix.record_arm", &json!({"tracks": [id], "value": true})).unwrap();
        TrackId(id)
    }

    fn ev(at: Samples, port: &str, msg: [u8; 3]) -> RecordedEvent {
        RecordedEvent { at, port: port.into(), msg, pass: 0 }
    }

    fn clip(e: &Engine, id: ClipId) -> Clip {
        e.session().find_clip(id).unwrap().1.clone()
    }

    const Q: Samples = 24_000;

    #[test]
    fn a_take_becomes_a_clip_with_its_notes_and_controllers() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        assert_eq!(e.session().sample_rate.hz(), 48_000);
        let take = MidiTake { start: 4 * Q, end: 8 * Q, looped: None, punch: None, latency: 480 };
        let events = [
            ev(4 * Q + 480, "Keys", [0x90, 60, 100]),
            ev(5 * Q + 480, "Keys", [0xB0, 64, 127]),
            ev(5 * Q + 480 + 12_000, "Keys", [0xE0, 0, 96]),
            ev(6 * Q + 480, "Keys", [0x80, 60, 30]),
            ev(6 * Q + 480, "Focusrite", [0x91, 64, 90]),
            // Still held when recording stops: it ends with the clip.
            ev(7 * Q + 480, "Keys", [0x90, 67, 80]),
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 1);
        let c = clip(&e, ids[0]);
        assert_eq!((c.start, c.length), (4 * Q, 4 * Q));
        assert!(c.midi_channels);
        let seq = c.midi_playing().unwrap();
        let notes: Vec<(u8, u8, i64, i64)> = seq.notes.iter().map(|n| (n.pitch, n.channel, n.start, n.length)).collect();
        let q = TICKS_PER_QUARTER;
        assert_eq!(notes, vec![(60, 0, 0, 2 * q), (64, 1, 2 * q, 2 * q), (67, 0, 3 * q, q)]);
        let ctrls: Vec<(i64, CtrlKind, i32)> = seq.ctrls.iter().map(|x| (x.tick, x.kind, x.value)).collect();
        // The pedal still down at the end comes up on the clip's last tick.
        assert_eq!(ctrls, vec![(q, CtrlKind::Cc(64), 127), (q + q / 2, CtrlKind::PitchBend, 96 * 128 - 8192), (4 * q - 1, CtrlKind::Cc(64), 0)]);
        assert_eq!(e.undo_label(), Some("Record"));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(ids[0]).is_none());
        let _ = t;
    }

    #[test]
    fn each_track_takes_its_port_and_channel() {
        let mut e = Engine::default();
        let din = armed(&mut e, "midi:Focusrite", 0);
        let ch2 = armed(&mut e, "midi:all", 2);
        let take = MidiTake { start: 0, end: 4 * Q, looped: None, punch: None, latency: 0 };
        let events = [ev(0, "Keys", [0x90, 60, 100]), ev(Q, "Focusrite", [0x90, 62, 100]), ev(2 * Q, "Keys", [0x91, 64, 100])];
        record_live_midi(&mut e, &take, &events);
        let pitches = |t: TrackId| -> Vec<u8> {
            let tr = e.session().track(t).unwrap();
            tr.clips()
                .iter()
                .flat_map(|c| c.midi_playing().map(|m| m.notes.iter().map(|n| n.pitch).collect::<Vec<_>>()).unwrap_or_default())
                .collect()
        };
        assert_eq!(pitches(din), vec![62]);
        assert_eq!(pitches(ch2), vec![64]);
    }

    #[test]
    fn punch_keeps_only_the_punch_range() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let take = MidiTake { start: 0, end: 8 * Q, looped: None, punch: Some(Range::new(2 * Q, 6 * Q)), latency: 0 };
        let events = [ev(Q, "K", [0x90, 50, 100]), ev(3 * Q, "K", [0x90, 60, 100]), ev(4 * Q, "K", [0x80, 60, 0]), ev(7 * Q, "K", [0x90, 70, 100])];
        let ids = record_live_midi(&mut e, &take, &events);
        let c = clip(&e, ids[0]);
        assert_eq!((c.start, c.length), (2 * Q, 4 * Q));
        let pitches: Vec<u8> = c.midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![60]);
        assert_eq!(e.session().track(t).unwrap().clips().len(), 1);
    }

    #[test]
    fn input_quantize_snaps_on_the_session_grid_and_keeps_the_performance() {
        let mut e = Engine::default();
        armed(&mut e, "midi:all", 0);
        e.execute("event.input_quantize", &json!({"enabled": true, "grid": "1/4"})).unwrap();
        // Recording from a sixteenth into bar 1: a note a little after beat 2.
        let start = Q / 4;
        let take = MidiTake { start, end: 4 * Q, looped: None, punch: None, latency: 0 };
        let ids = record_live_midi(&mut e, &take, &[ev(Q + 700, "K", [0x90, 60, 100]), ev(2 * Q, "K", [0x80, 60, 0])]);
        let c = clip(&e, ids[0]);
        let n = c.midi_playing().unwrap().notes[0];
        let q = TICKS_PER_QUARTER;
        // On beat 2 of the session, i.e. 3/4 of a quarter into the clip.
        assert_eq!(n.start, q - q / 4);
        assert!(e.session().midi_originals.contains_key(&c.id.0), "the played performance is kept");
    }

    #[test]
    fn loop_recording_makes_a_clip_per_pass_on_new_playlists() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        // The rest of the song, outside the loop: on every pass's playlist.
        let outro = Clip::midi(e.session_mut().new_clip_id(), "Outro", 6 * Q, Q, Sequence::default());
        crate::edit::place_clip(e.session_mut(), t, outro);
        let lp = Range::new(0, 4 * Q);
        let take = MidiTake { start: 0, end: 12 * Q, looped: Some(lp), punch: None, latency: 0 };
        let mut events = vec![ev(Q, "K", [0x90, 60, 100]), ev(2 * Q, "K", [0x80, 60, 0])];
        events.push(RecordedEvent { pass: 2, ..ev(Q, "K", [0x90, 64, 100]) });
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 2, "the silent pass makes nothing");
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.playlists.len(), 2);
        assert_eq!(tr.active_playlist, 1, "the last pass is on top");
        let spans: Vec<(Samples, Samples, Option<u8>)> = tr
            .playlists
            .iter()
            .flat_map(|p| p.clips.iter().map(|c| (c.start, c.length, c.midi_playing().unwrap().notes.first().map(|n| n.pitch))))
            .collect();
        assert_eq!(spans, vec![(0, 4 * Q, Some(60)), (6 * Q, Q, None), (0, 4 * Q, Some(64)), (6 * Q, Q, None)]);
        let ids: Vec<ClipId> = tr.playlists.iter().flat_map(|p| p.clips.iter().map(|c| c.id)).collect();
        assert_ne!(ids[1], ids[3], "the copy is a clip of its own");
        assert_eq!(e.undo_label(), Some("Record"));
    }

    #[test]
    fn an_event_heard_just_after_a_loop_wrap_ends_the_pass_before() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let lp = Range::new(0, 4 * Q);
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(lp), punch: None, latency: 480 };
        // Moved at the end of the first pass (3 Q + 23_700), heard 480 samples later: 180
        // samples into the second. (A note there would be an early downbeat for the second.)
        let events = vec![ev(Q + 480, "K", [0x90, 60, 100]), RecordedEvent { pass: 1, ..ev(180, "K", [0xB0, 1, 64]) }];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 1, "only the first pass has anything");
        let tr = e.session().track(t).unwrap();
        let seq = tr.playlists[0].clips[0].midi_playing().unwrap();
        let at = e.session().tempo.samples_to_ticks(4 * Q - 300, e.session().sample_rate);
        assert_eq!(seq.notes[0].start, TICKS_PER_QUARTER);
        assert_eq!(seq.ctrls.iter().map(|c| c.tick).collect::<Vec<_>>(), vec![at]);
    }

    #[test]
    fn notes_played_before_the_transport_rolled_begin_the_take_despite_latency() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        // The player hears a note played between the Record press and the roll on the take's
        // first frame; the output latency would move it before the take.
        let take = MidiTake { start: 4 * Q, end: 8 * Q, looped: None, punch: None, latency: 512 };
        let events = [ev(4 * Q, "K", [0x90, 60, 100]), ev(4 * Q + 300, "K", [0x90, 64, 100]), ev(5 * Q + 512, "K", [0x80, 60, 0])];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 1);
        let c = clip(&e, ids[0]);
        assert_eq!(c.start, 4 * Q);
        let notes: Vec<(u8, i64)> = c.midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.start)).collect();
        assert_eq!(notes, vec![(60, 0), (64, 0)], "the downbeat and an early note are kept, on the start");
        assert_eq!(c.midi_playing().unwrap().notes[0].length, TICKS_PER_QUARTER);
        // Heard before the take began (an earlier block's): still left out.
        let take2 = MidiTake { start: 12 * Q, end: 14 * Q, looped: None, punch: None, latency: 512 };
        assert!(record_live_midi(&mut e, &take2, &[ev(12 * Q - 1, "K", [0x90, 60, 100])]).is_empty());
        let _ = t;
    }

    #[test]
    fn record_pressed_before_a_playing_loop_keeps_every_pass() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        // Rolling from 2 Q into a loop over 4..8 Q: the first pass is 2..8 Q, then the loop.
        let lp = Range::new(4 * Q, 8 * Q);
        let take = MidiTake { start: 2 * Q, end: 6 * Q, looped: Some(lp), punch: None, latency: 480 };
        let events = vec![
            ev(3 * Q + 480, "K", [0x90, 50, 100]),
            ev(5 * Q + 480, "K", [0x80, 50, 0]),
            RecordedEvent { pass: 1, ..ev(5 * Q + 480, "K", [0x90, 60, 100]) },
            RecordedEvent { pass: 2, ..ev(4 * Q + 480, "K", [0x90, 64, 100]) },
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 3, "no pass after the first wrap is lost");
        let tr = e.session().track(t).unwrap();
        let first = &tr.playlists[0].clips[0];
        assert_eq!((first.start, first.length), (2 * Q, 6 * Q));
        let n = first.midi_playing().unwrap().notes[0];
        assert_eq!((n.pitch, n.start, n.length), (50, TICKS_PER_QUARTER, 2 * TICKS_PER_QUARTER));
        let later: Vec<(Samples, Samples, u8, i64)> = tr.playlists[1..]
            .iter()
            .flat_map(|p| {
                p.clips.iter().map(|c| (c.start, c.length, c.midi_playing().unwrap().notes[0].pitch, c.midi_playing().unwrap().notes[0].start))
            })
            .collect();
        let q = TICKS_PER_QUARTER;
        assert_eq!(later, vec![(4 * Q, 4 * Q, 60, q), (4 * Q, 4 * Q, 64, 0)]);
    }

    #[test]
    fn a_punch_in_inside_a_playing_loop_leaves_what_came_before_it() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let mut old = Sequence::default();
        old.notes.push(soundcraft_midi::Note { pitch: 40, velocity: 100, muted: false, release_velocity: 64, channel: 0, start: 0, length: 10 });
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::midi(id, "Old", 0, Q, old));
        // Record pressed at 2 Q while the loop 0..4 Q plays.
        let take = MidiTake { start: 2 * Q, end: 3 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        let ids = record_live_midi(&mut e, &take, &[ev(2 * Q + 100, "K", [0x90, 60, 100])]);
        let c = clip(&e, ids[0]);
        assert_eq!((c.start, c.length), (2 * Q, 2 * Q));
        assert!(e.session().find_clip(id).is_some(), "the clip before the punch point stays");
    }

    #[test]
    fn a_pedal_down_at_stop_comes_up_at_the_clip_end_and_one_pressed_before_is_down_from_the_start() {
        let mut e = Engine::default();
        armed(&mut e, "midi:all", 0);
        let take = MidiTake { start: 4 * Q, end: 8 * Q, looped: None, punch: Some(Range::new(5 * Q, 8 * Q)), latency: 480 };
        let events = [
            // Pressed in the pre-roll, before the punch point.
            ev(4 * Q + 480, "K", [0xB1, 64, 127]),
            ev(6 * Q + 480, "K", [0x91, 60, 100]),
            ev(7 * Q + 480, "K", [0x81, 60, 0]),
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        let c = clip(&e, ids[0]);
        let ctrls: Vec<(i64, u8, CtrlKind, i32)> = c.midi_playing().unwrap().ctrls.iter().map(|x| (x.tick, x.channel, x.kind, x.value)).collect();
        let len = 3 * TICKS_PER_QUARTER;
        assert_eq!(ctrls, vec![(0, 1, CtrlKind::Cc(64), 127), (len - 1, 1, CtrlKind::Cc(64), 0)]);
    }

    #[test]
    fn a_pedal_held_over_a_loop_wrap_is_down_in_the_next_pass() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        let events = vec![
            ev(3 * Q, "K", [0xB0, 64, 127]),
            RecordedEvent { pass: 1, ..ev(Q, "K", [0x90, 60, 100]) },
            RecordedEvent { pass: 1, ..ev(2 * Q, "K", [0xB0, 64, 0]) },
        ];
        record_live_midi(&mut e, &take, &events);
        let tr = e.session().track(t).unwrap();
        let ctrls =
            |p: usize| -> Vec<(i64, i32)> { tr.playlists[p].clips[0].midi_playing().unwrap().ctrls.iter().map(|x| (x.tick, x.value)).collect() };
        let q = TICKS_PER_QUARTER;
        assert_eq!(ctrls(0), vec![(3 * q, 127), (4 * q - 1, 0)]);
        assert_eq!(ctrls(1), vec![(0, 127), (2 * q, 0)]);
    }

    #[test]
    fn a_downbeat_played_early_at_a_loop_end_starts_the_next_pass() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let lp = Range::new(0, 4 * Q);
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(lp), punch: None, latency: 480 };
        // Played 1,000 samples (a 96th) before the wrap, released in the next pass.
        let events = vec![
            ev(Q + 480, "K", [0x90, 50, 100]),
            ev(2 * Q + 480, "K", [0x80, 50, 0]),
            ev(4 * Q - 1_000 + 480, "K", [0x90, 36, 100]),
            RecordedEvent { pass: 1, ..ev(Q + 480, "K", [0x80, 36, 0]) },
        ];
        record_live_midi(&mut e, &take, &events);
        let tr = e.session().track(t).unwrap();
        let notes = |p: usize| -> Vec<(u8, i64, i64)> {
            tr.playlists[p].clips.iter().flat_map(|c| c.midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.start, n.length))).collect()
        };
        let q = TICKS_PER_QUARTER;
        assert_eq!(notes(0), vec![(50, q, q)], "no sliver at the end of the first pass");
        assert_eq!(notes(1), vec![(36, 0, q)], "the downbeat of the second");
    }

    #[test]
    fn with_input_quantize_an_early_downbeat_does_not_snap_outside_the_clip() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        e.execute("event.input_quantize", &json!({"enabled": true, "grid": "1/4"})).unwrap();
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        // A short note an eighth less a bit before the wrap: quantized to a quarter it would land on the end.
        let events = vec![ev(Q, "K", [0x90, 50, 100]), ev(4 * Q - 9_000, "K", [0x90, 36, 100]), ev(4 * Q - 6_000, "K", [0x80, 36, 0])];
        let ids = record_live_midi(&mut e, &take, &[events, vec![RecordedEvent { pass: 1, ..ev(2 * Q, "K", [0x90, 40, 1]) }]].concat());
        assert_eq!(ids.len(), 2);
        let tr = e.session().track(t).unwrap();
        let pitches =
            |p: usize| -> Vec<(u8, i64)> { tr.playlists[p].clips[0].midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.start)).collect() };
        assert_eq!(pitches(0), vec![(50, TICKS_PER_QUARTER)]);
        assert_eq!(pitches(1), vec![(36, 0), (40, 2 * TICKS_PER_QUARTER)]);
    }

    #[test]
    fn a_note_begun_just_before_the_last_pass_ends_stays_as_played() {
        let mut e = Engine::default();
        armed(&mut e, "midi:all", 0);
        // A take whose end is its last event (a remote recorder's): its last note is kept.
        let take = MidiTake { start: 0, end: 4 * Q - 499, looped: None, punch: None, latency: 0 };
        let ids = record_live_midi(&mut e, &take, &[ev(Q, "K", [0x90, 50, 100]), ev(4 * Q - 500, "K", [0x90, 36, 100])]);
        let pitches: Vec<u8> = clip(&e, ids[0]).midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![50, 36]);
    }

    /// A clip with one note of `pitch` at tick `at` on `t`, from `start` for `len`.
    fn old_clip(e: &mut Engine, t: TrackId, start: Samples, len: Samples, pitch: u8, at: i64) -> ClipId {
        let mut seq = Sequence::default();
        seq.notes.push(soundcraft_midi::Note { pitch, velocity: 90, muted: false, release_velocity: 64, channel: 0, start: at, length: 100 });
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), t, Clip::midi(id, "Old", start, len, seq));
        id
    }

    #[test]
    fn new_sessions_record_midi_in_merge_mode_and_saved_ones_keep_theirs() {
        let e = Engine::default();
        assert_eq!(record_mode(e.session()), RecordMode::Merge, "a new session overdubs");
        // A session saved with MIDI Merge off (Replace) opens with it off.
        let mut saved = soundcraft_model::Session::default();
        set_record_mode(&mut saved, RecordMode::Replace);
        let json = serde_json::to_string(&saved).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&json).unwrap();
        assert_eq!(record_mode(&back), RecordMode::Replace);
        let mut v: serde_json::Value = serde_json::from_str(&json).unwrap();
        v["edit"]["midi_merge"] = json!(false);
        v["edit"]["values"] = json!({});
        let older: soundcraft_model::Session = serde_json::from_value(v).unwrap();
        assert_eq!(record_mode(&older), RecordMode::Replace, "an older session's stored switch is kept");
    }

    #[test]
    fn merge_mode_overdubs_into_the_clip_under_the_take() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let old = old_clip(&mut e, t, 0, 4 * Q, 40, 0);
        e.execute("options.midi_record_mode", &json!({"mode": "merge"})).unwrap();
        assert!(e.session().edit.midi_merge, "the MIDI Merge switch shows it");
        let take = MidiTake { start: 2 * Q, end: 6 * Q, looped: None, punch: None, latency: 480 };
        let ids = record_live_midi(&mut e, &take, &[ev(3 * Q + 480, "K", [0x91, 60, 100])]);
        assert_eq!(ids.len(), 1);
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.clips().len(), 1, "one clip");
        assert!(e.session().find_clip(old).is_none());
        let c = &tr.clips()[0];
        assert_eq!((c.start, c.length, c.name.as_str()), (0, 6 * Q, "Old"));
        let notes: Vec<(u8, u8, i64)> = c.midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.channel, n.start)).collect();
        assert_eq!(notes, vec![(40, 0, 0), (60, 1, 3 * TICKS_PER_QUARTER)]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(old).is_some(), "one undo brings the old clip back");
    }

    #[test]
    fn merged_loop_passes_add_up_in_one_clip() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        e.execute("options.midi_merge", &json!({"value": true})).unwrap();
        let take = MidiTake { start: 0, end: 12 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        let events = vec![
            ev(0, "K", [0x90, 36, 100]),
            RecordedEvent { pass: 1, ..ev(Q, "K", [0x90, 38, 100]) },
            RecordedEvent { pass: 2, ..ev(2 * Q, "K", [0x90, 42, 100]) },
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.playlists.len(), 1, "no playlist per pass");
        assert_eq!(tr.clips().len(), 1);
        assert_eq!(ids, vec![tr.clips()[0].id]);
        let pitches: Vec<u8> = tr.clips()[0].midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![36, 38, 42]);
    }

    /// Keep Takes as Lanes: each loop pass is a take lane of its own, the comp plays the newest,
    /// a swipe brings part of an earlier take into the comp (a note across the join ends there on
    /// the outgoing side and sounds from it on the incoming one), and cycling picks the take below.
    #[test]
    fn take_lanes_keep_each_pass_to_pick_and_swipe_between() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let out = e.execute("options.midi_record_mode", &json!({"mode": "take_lanes"})).unwrap();
        assert_eq!(out["mode"], "take_lanes");
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        let events = vec![
            ev(0, "K", [0x90, 36, 100]),
            ev(3 * Q, "K", [0x90, 37, 100]),
            ev(4 * Q - 100, "K", [0x80, 37, 0]),
            RecordedEvent { pass: 1, ..ev(0, "K", [0x90, 48, 100]) },
            RecordedEvent { pass: 1, ..ev(3 * Q, "K", [0x90, 49, 100]) },
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 2);
        let pitches = |c: &Clip| -> Vec<u8> { c.midi_playing().map(|m| m.notes.iter().map(|n| n.pitch).collect()).unwrap_or_default() };
        {
            let tr = e.session().track(t).unwrap();
            let layers: Vec<usize> = tr.layers().map(|(i, _)| i).collect();
            assert_eq!(layers.len(), 2, "a lane per pass");
            assert!(tr.layers_open, "the lanes are shown");
            assert_eq!(tr.clips().len(), 1, "the comp plays one take");
            assert_eq!(pitches(&tr.clips()[0]), vec![48, 49], "the newest");
            assert_eq!(tr.clips()[0].from_layer, Some(layers[1]));
            assert_eq!(pitches(&tr.playlists[layers[0]].clips[0]), vec![36, 37]);
        }
        assert_eq!(e.undo_label(), Some("Record"));
        // Swipe the last beat of the first take into the comp.
        let first = e.session().track(t).unwrap().layers().map(|(i, _)| i).next().unwrap();
        e.execute("comp.swipe", &json!({"track": t.0, "layer": first, "start": 3 * Q, "end": 4 * Q})).unwrap();
        let tr = e.session().track(t).unwrap();
        let comp: Vec<(Samples, Vec<u8>, Option<usize>)> = tr.clips().iter().map(|c| (c.start, pitches(c), c.from_layer)).collect();
        assert_eq!(comp.len(), 2);
        assert_eq!(comp[0].1, vec![48], "the second take up to the swipe");
        // The first take's last beat: its note held over the join sounds from it, then its own.
        assert_eq!((comp[1].0, comp[1].1.clone(), comp[1].2), (3 * Q, vec![36, 37], Some(first)));
        assert!(tr.clips().iter().all(|c| c.fade_in.len == 0 && c.fade_out.len == 0), "MIDI joins get no crossfade");
        // Pick the take below back over that beat.
        let back = e.execute("comp.cycle", &json!({"track": t.0, "start": 3 * Q, "end": 4 * Q, "dir": "down"})).unwrap();
        assert_ne!(back["layer"].as_u64(), Some(first as u64));
        let tr = e.session().track(t).unwrap();
        assert!(tr.clips().iter().all(|c| c.from_layer != Some(first)));
        let last: Vec<u8> = tr.clips().iter().filter(|c| c.start == 3 * Q).flat_map(pitches).collect();
        assert_eq!(last, vec![48, 49], "the second take's last beat again");
    }

    /// A MIDI track whose lanes are open records into a new lane in Replace mode too; Merge
    /// still merges into the clip under it.
    #[test]
    fn a_comping_midi_track_records_into_lanes_unless_merging() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        e.execute("comp.layers", &json!({"track": t.0, "open": true})).unwrap();
        let take = MidiTake { start: 0, end: 4 * Q, looped: None, punch: None, latency: 0 };
        record_live_midi(&mut e, &take, &[ev(0, "K", [0x90, 60, 100])]);
        record_live_midi(&mut e, &take, &[ev(Q, "K", [0x90, 62, 100])]);
        assert_eq!(e.session().track(t).unwrap().layers().count(), 2);
        set_record_mode(e.session_mut(), RecordMode::Merge);
        record_live_midi(&mut e, &take, &[ev(2 * Q, "K", [0x90, 64, 100])]);
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.layers().count(), 2, "merge adds no lane");
        assert_eq!(tr.clips().len(), 1);
    }

    #[test]
    fn new_playlist_mode_keeps_the_old_part_on_its_playlist() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let old = old_clip(&mut e, t, 0, 4 * Q, 40, 0);
        let out = e.execute("options.midi_record_mode", &json!({"mode": "new_playlist"})).unwrap();
        assert_eq!(out["mode"], "new_playlist");
        let take = MidiTake { start: 2 * Q, end: 3 * Q, looped: None, punch: None, latency: 0 };
        record_live_midi(&mut e, &take, &[ev(2 * Q, "K", [0x90, 60, 100])]);
        let tr = e.session().track(t).unwrap();
        assert_eq!((tr.playlists.len(), tr.active_playlist), (2, 1));
        assert!(tr.playlists[0].clips.iter().any(|c| c.id == old && c.length == 4 * Q), "the old part, whole");
        assert_eq!(tr.playlists[1].clips.len(), 3, "the copy cut around the take, and the take");
        // Replace clears what the take covers.
        e.execute("options.midi_record_mode", &json!({"mode": "replace"})).unwrap();
        assert_eq!(record_mode(e.session()), RecordMode::Replace);
        assert!(e.execute("options.midi_record_mode", &json!({"mode": "overdub"})).is_err());
    }

    #[test]
    fn a_track_with_a_midi_offset_records_where_it_will_play_back_right() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        // The track's notes sound 2,400 samples late.
        e.execute("event.midi_track_offsets", &json!({"track": t.0, "offset": 2_400})).unwrap();
        let take = MidiTake { start: 0, end: 4 * Q, looped: None, punch: None, latency: 480 };
        let ids = record_live_midi(&mut e, &take, &[ev(Q + 480, "K", [0x90, 60, 100]), ev(2 * Q + 480, "K", [0x80, 60, 0])]);
        let n = clip(&e, ids[0]).midi_playing().unwrap().notes[0];
        let at = e.session().tempo.samples_to_ticks(Q - 2_400, e.session().sample_rate);
        assert_eq!((n.start, n.length), (at, TICKS_PER_QUARTER));
    }

    #[test]
    fn finished_loop_passes_are_on_the_track_while_the_loop_runs_and_the_last_can_be_undone() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        set_record_mode(e.session_mut(), RecordMode::Merge);
        let old = old_clip(&mut e, t, 0, 4 * Q, 40, 0);
        let undo_before = e.undo_label().map(str::to_string);
        let lp = Range::new(0, 4 * Q);
        let mut preview = LoopPreview::new(e.session(), &armed_midi_tracks(e.session()));
        let pitches = |e: &Engine| -> Vec<u8> {
            let tr = e.session().track(t).unwrap();
            let mut v: Vec<u8> = tr.clips().iter().flat_map(|c| c.midi_playing().unwrap().notes.iter().map(|n| n.pitch)).collect();
            v.sort();
            v
        };
        // In pass 1, pass 0's kick is heard; pass 1's snare, still being played, is not yet.
        let mut events = vec![ev(0, "K", [0x90, 36, 100]), RecordedEvent { pass: 1, ..ev(Q, "K", [0x90, 38, 100]) }];
        let take = MidiTake { start: 0, end: 5 * Q, looped: Some(lp), punch: None, latency: 0 };
        assert!(preview.show(e.session_mut(), &take, &events, 1));
        assert_eq!(pitches(&e), vec![36, 40], "the old part and the first pass, merged");
        // In pass 2: both passes.
        events.push(RecordedEvent { pass: 2, ..ev(2 * Q, "K", [0x90, 42, 100]) });
        preview.show(e.session_mut(), &MidiTake { end: 9 * Q, ..take }, &events, 2);
        assert_eq!(pitches(&e), vec![36, 38, 40]);
        // Undo Last Pass: the snare's pass goes, from what plays and from the take.
        assert_eq!(preview.undo_last_pass(), Some(1));
        preview.show(e.session_mut(), &MidiTake { end: 9 * Q, ..take }, &events, 2);
        assert_eq!(pitches(&e), vec![36, 40]);
        assert_eq!(e.undo_label().map(str::to_string), undo_before, "the preview is no undo step");
        // Stop: the preview comes off, the take goes down once, one undo brings the old part back.
        preview.restore(e.session_mut());
        assert!(e.session().find_clip(old).is_some());
        let kept = preview.keep(&events);
        record_live_midi(&mut e, &MidiTake { end: 10 * Q, ..take }, &kept);
        assert_eq!(pitches(&e), vec![36, 40, 42]);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(pitches(&e), vec![40]);
        // Undo goes back no further than the passes still in.
        let mut p = LoopPreview::new(e.session(), &[t]);
        p.show(e.session_mut(), &take, &events, 1);
        assert_eq!((p.undo_last_pass(), p.undo_last_pass()), (Some(0), None));
    }

    #[test]
    fn a_previewed_pass_on_its_own_playlist_comes_off_with_its_playlist() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let take = MidiTake { start: 0, end: 9 * Q, looped: Some(Range::new(0, 4 * Q)), punch: None, latency: 0 };
        let events = vec![ev(0, "K", [0x90, 36, 100]), RecordedEvent { pass: 1, ..ev(Q, "K", [0x90, 38, 100]) }];
        let mut p = LoopPreview::new(e.session(), &[t]);
        p.show(e.session_mut(), &take, &events, 2);
        let tr = e.session().track(t).unwrap();
        assert_eq!((tr.playlists.len(), tr.active_playlist), (2, 1), "the last finished pass is on top");
        p.restore(e.session_mut());
        let tr = e.session().track(t).unwrap();
        assert_eq!((tr.playlists.len(), tr.clips().len()), (1, 0));
    }

    #[test]
    fn notes_played_in_the_count_off_are_a_pickup_when_asked() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        // Recording from bar 3 (8 Q), a pickup played on the last beat of the count-off.
        let take = MidiTake { start: 8 * Q, end: 12 * Q, looped: None, punch: None, latency: 480 };
        let events = [ev(7 * Q + 480, "K", [0x90, 55, 90]), ev(8 * Q + 480, "K", [0x80, 55, 0]), ev(9 * Q + 480, "K", [0x90, 60, 100])];
        let ids = record_live_midi(&mut e, &take, &events);
        let c = clip(&e, ids[0]);
        assert_eq!(c.start, 8 * Q, "left out unless asked");
        assert_eq!(c.midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60]);
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("setup.click_countoff", &json!({"keep_pickup": true})).unwrap();
        let ids = record_live_midi(&mut e, &take, &events);
        let c = clip(&e, ids[0]);
        assert_eq!((c.start, c.end()), (4 * Q, 12 * Q), "from the bar the pickup was played in");
        let notes: Vec<(u8, i64)> = c.midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.start)).collect();
        assert_eq!(notes, vec![(55, 3 * TICKS_PER_QUARTER), (60, 5 * TICKS_PER_QUARTER)]);
        let _ = t;
    }

    #[test]
    fn the_input_transform_shapes_the_take_and_survives_a_change_of_port() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let r = e
            .execute("track.midi_input_transform", &json!({"tracks": [t.0], "transpose": 12, "fixed_velocity": 90, "key_high": 72, "channel": 3}))
            .unwrap();
        assert_eq!(r["transform"]["transpose"], 12);
        assert_eq!(e.undo_label(), Some("MIDI Input Transform"));
        // Another port: the transform stays.
        e.execute("track.input", &json!({"tracks": [t.0], "input": "midi:Keys"})).unwrap();
        let take = MidiTake { start: 0, end: 4 * Q, looped: None, punch: None, latency: 0 };
        let events = [
            ev(0, "Keys", [0x90, 60, 30]),
            ev(Q, "Keys", [0x80, 60, 0]),
            // Above the key range: left out, its note-off too.
            ev(Q, "Keys", [0x90, 80, 100]),
            ev(2 * Q, "Keys", [0x80, 80, 0]),
        ];
        let ids = record_live_midi(&mut e, &take, &events);
        let notes: Vec<(u8, u8, u8, i64)> =
            clip(&e, ids[0]).midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.velocity, n.channel, n.length)).collect();
        assert_eq!(notes, vec![(72, 90, 2, TICKS_PER_QUARTER)]);
        e.execute("track.midi_input_transform", &json!({"tracks": [t.0], "reset": true})).unwrap();
        let Route::Midi(m) = &e.session().track(t).unwrap().mixer.input else { panic!() };
        assert!(m.transform.is_identity());
        assert_eq!(m.device.as_deref(), Some("Keys"));
    }

    #[test]
    fn unarmed_or_other_inputs_record_nothing() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        e.execute("mix.record_arm", &json!({"tracks": [t.0], "value": false})).unwrap();
        let take = MidiTake { start: 0, end: Q, looped: None, punch: None, latency: 0 };
        assert!(record_live_midi(&mut e, &take, &[ev(10, "K", [0x90, 60, 100])]).is_empty());
        assert_ne!(e.undo_label(), Some("Record"));
    }
}

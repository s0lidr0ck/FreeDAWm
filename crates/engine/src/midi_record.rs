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
//! - One undo step, "Record".

use soundcraft_midi::Sequence;
use soundcraft_model::{Clip, ClipId, Playlist, Route, Session, TrackId};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER};

use crate::Engine;

/// One recorded event: its timeline position as heard, the port it came from (by name), the
/// message, and its loop pass (0 without a loop).
#[derive(Debug, Clone, PartialEq, Eq)]
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
    let tracks = tracks.to_vec();
    if tracks.is_empty() || events.is_empty() {
        return Vec::new();
    }
    let before = e.session_arc();
    let quantize = e.session().edit.flag("midi.input_quantize");
    let opts = crate::cmd::event_more::quantize_opts(e.session());
    let passes = events.iter().map(|x| x.pass).max().unwrap_or(0);
    let mut out = Vec::new();
    for tid in tracks {
        let s = e.session_mut();
        let Some(Route::Midi(input)) = s.track(tid).map(|t| t.mixer.input.clone()) else { continue };
        let filter = |x: &&RecordedEvent| {
            input.device.as_ref().is_none_or(|d| *d == x.port) && (input.channel == 0 || x.msg[0] >= 0xF0 || (x.msg[0] & 0x0F) + 1 == input.channel)
        };
        // Each event where it was played: `latency` earlier, and an event heard in the first
        // `latency` samples after a loop wrap was played at the end of the pass before.
        let mine: Vec<(Samples, [u8; 3], u16)> = events
            .iter()
            .filter(filter)
            .map(|x| {
                let (at, pass) = played_at(take, x.at, x.pass);
                (at, x.msg, pass)
            })
            .collect();
        // The playlist as it was before this take: each loop pass after the first goes on a copy
        // of it, so the rest of the song stays on the track whichever pass is active.
        let base: Vec<Clip> = if take.looped.is_some() { s.track(tid).map(|t| t.clips().to_vec()).unwrap_or_default() } else { Vec::new() };
        let mut placed_first = false;
        // A downbeat played a little early, in the last moment of a pass, starts the next one.
        let window = (TICKS_PER_QUARTER / 8).max(if quantize { opts.grid_ticks / 2 } else { 0 });
        let mine = settle_early_notes(s, take, passes, mine, window, quantize);
        for pass in 0..=passes {
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
            let performance = quantize.then(|| seq.clone());
            if quantize {
                // On the session's grid: the notes' ticks from the session start while quantizing.
                let base = s.tempo.samples_to_ticks(range.start, s.sample_rate);
                seq.notes.iter_mut().for_each(|n| n.start = n.start.saturating_add(base));
                soundcraft_midi::ops::quantize(&mut seq.notes, &opts);
                seq.notes.iter_mut().for_each(|n| n.start = n.start.saturating_sub(base).max(0));
                seq.sort();
            }
            // Each pass after the first that went down on this track: a new playlist, holding what
            // the track held before the take.
            if placed_first {
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
            let id = s.new_clip_id();
            let name = s.track(tid).map(|t| t.name.clone()).unwrap_or_default();
            let mut clip = Clip::midi(id, name, range.start, range.len(), seq);
            clip.midi_channels = true;
            crate::edit::place_clip(s, tid, clip);
            if let Some(p) = performance {
                s.midi_originals.insert(id.0, p);
            }
            placed_first = true;
            out.push(id);
        }
    }
    if !out.is_empty() {
        e.push_undo("Record", before);
    }
    out
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
/// downbeat: in a loop they start the next pass (their note-off with them), and at the end of the
/// take, where no pass follows, they are left out instead of being cut to a sliver. `events` are
/// where played, with their pass.
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
    let mut drop = vec![false; events.len()];
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
        match take.looped.filter(|_| pass < passes) {
            Some(l) => {
                if let Some(x) = events.get_mut(i) {
                    *x = (l.start, msg, pass + 1);
                }
                if let Some(x) = off.and_then(|j| events.get_mut(j)) {
                    *x = (l.start.saturating_add(x.0.saturating_sub(at)), x.1, pass + 1);
                }
            }
            None => {
                for j in std::iter::once(i).chain(off) {
                    if let Some(d) = drop.get_mut(j) {
                        *d = true;
                    }
                }
            }
        }
    }
    events.into_iter().zip(drop).filter(|(_, d)| !d).map(|(x, _)| x).collect()
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
    fn armed(e: &mut Engine, input: &str, channel: u8) -> TrackId {
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
        old.notes.push(soundcraft_midi::Note { pitch: 40, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 10 });
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
    fn a_note_begun_just_before_stop_is_left_out() {
        let mut e = Engine::default();
        armed(&mut e, "midi:all", 0);
        let take = MidiTake { start: 0, end: 4 * Q, looped: None, punch: None, latency: 0 };
        let ids = record_live_midi(&mut e, &take, &[ev(Q, "K", [0x90, 50, 100]), ev(4 * Q - 500, "K", [0x90, 36, 100])]);
        let pitches: Vec<u8> = clip(&e, ids[0]).midi_playing().unwrap().notes.iter().map(|n| n.pitch).collect();
        assert_eq!(pitches, vec![50]);
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

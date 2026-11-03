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
//!   before.
//! - **Range**: a clip covers the record range (from where recording started to where it
//!   stopped), cut to the punch range; loop recording makes a clip over the loop per pass with
//!   anything in it, the first on the track's playlist and each later one on a new playlist (the
//!   comp layers are for audio takes).
//! - Each track takes the events its input lets through (its port, or all; Omni or one channel),
//!   channels kept; notes still held at the range's end end there; notes begun before it are
//!   left out. Controllers (sustain, mod wheel, pitch bend, aftertouch, program) are kept.
//! - **Input Quantize** (Event › MIDI Operations › Input Quantize) quantizes the notes on the
//!   session's grid when it is on; the played performance is kept to restore.
//! - One undo step, "Record".

use soundcraft_midi::Sequence;
use soundcraft_model::{Clip, ClipId, Playlist, Route, Session, TrackId};
use soundcraft_time::{Range, Samples};

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
pub fn played_at(take: &MidiTake, at: Samples, pass: u16) -> (Samples, u16) {
    let at = at.saturating_sub(take.latency);
    match take.looped {
        Some(l) if at < l.start && pass > 0 => (at.saturating_add(l.len()), pass - 1),
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
        let mut placed_first = false;
        for pass in 0..=passes {
            let range = match take.looped {
                Some(l) => Some(l),
                None if pass == 0 => Some(Range::new(take.start, take.end.max(take.start))),
                None => None,
            };
            let Some(range) = range.and_then(|r| r.intersect(&take.punch.unwrap_or(r))).filter(|r| !r.is_empty()) else { continue };
            let list: Vec<(Samples, [u8; 3], bool)> =
                mine.iter().filter(|x| x.2 == pass).map(|&(at, msg, _)| (at, msg, true)).filter(|(at, _, _)| range.contains(*at)).collect();
            let mut seq: Sequence = crate::band_print::events_to_sequence(s, range, &list);
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
            // Each pass after the first that went down on this track: a new playlist.
            if placed_first && let Some(t) = s.track_mut(tid) {
                t.playlists.push(Playlist::new(format!("{}.{:02}", t.name, t.playlists.len() + 1)));
                t.active_playlist = t.playlists.len() - 1;
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
        assert_eq!(ctrls, vec![(q, CtrlKind::Cc(64), 127), (q + q / 2, CtrlKind::PitchBend, 96 * 128 - 8192)]);
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
        let lp = Range::new(0, 4 * Q);
        let take = MidiTake { start: 0, end: 12 * Q, looped: Some(lp), punch: None, latency: 0 };
        let mut events = vec![ev(Q, "K", [0x90, 60, 100]), ev(2 * Q, "K", [0x80, 60, 0])];
        events.push(RecordedEvent { pass: 2, ..ev(Q, "K", [0x90, 64, 100]) });
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 2, "the silent pass makes nothing");
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.playlists.len(), 2);
        assert_eq!(tr.active_playlist, 1, "the last pass is on top");
        let spans: Vec<(Samples, Samples, u8)> =
            tr.playlists.iter().flat_map(|p| p.clips.iter().map(|c| (c.start, c.length, c.midi_playing().unwrap().notes[0].pitch))).collect();
        assert_eq!(spans, vec![(0, 4 * Q, 60), (0, 4 * Q, 64)]);
        assert_eq!(e.undo_label(), Some("Record"));
    }

    #[test]
    fn an_event_heard_just_after_a_loop_wrap_ends_the_pass_before() {
        let mut e = Engine::default();
        let t = armed(&mut e, "midi:all", 0);
        let lp = Range::new(0, 4 * Q);
        let take = MidiTake { start: 0, end: 8 * Q, looped: Some(lp), punch: None, latency: 480 };
        // Played at the end of the first pass (3 Q + 23_700), heard 480 samples later: 180
        // samples into the second.
        let events = vec![ev(Q + 480, "K", [0x90, 60, 100]), RecordedEvent { pass: 1, ..ev(180, "K", [0x90, 64, 100]) }];
        let ids = record_live_midi(&mut e, &take, &events);
        assert_eq!(ids.len(), 1, "only the first pass has notes");
        let tr = e.session().track(t).unwrap();
        let mut notes: Vec<(u8, i64)> = tr.playlists[0].clips[0].midi_playing().unwrap().notes.iter().map(|n| (n.pitch, n.start)).collect();
        notes.sort();
        let at = e.session().tempo.samples_to_ticks(4 * Q - 300, e.session().sample_rate);
        assert_eq!(notes, vec![(60, TICKS_PER_QUARTER), (64, at)]);
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

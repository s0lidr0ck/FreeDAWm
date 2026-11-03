//! Print the band: what the six musicians play over a range, as MIDI sequences (one per
//! musician), rendered offline exactly as playback renders it.

use std::collections::{HashMap, VecDeque};
use std::sync::Arc;

use soundcraft_band::library::LibraryData;
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::runtime::BandRuntime;
use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
use soundcraft_mix::{BAND_SLOTS, band_transport_within, prepare_band};
use soundcraft_model::{Session, TrackId};
use soundcraft_time::{Range, Samples};

/// Playback's block size for the band (the engine renders it per block, cut where the lanes start
/// and end).
const BLOCK: i64 = 512;

/// One musician's events before the range: `(sample, bytes, lower, keyswitch)`.
type Before = Vec<(Samples, [u8; 3], bool, bool)>;

/// The band's events by musician, as playback routes them to the slots: `(sample, bytes, lower)`,
/// and those before the range (for the band's state where it starts).
struct Tape<'a> {
    slots: &'a mut [Vec<(Samples, [u8; 3], bool)>; BAND_SLOTS],
    before: &'a mut [Before; BAND_SLOTS],
    at: Samples,
    range: Range,
}

impl MidiSink for Tape<'_> {
    fn event(&mut self, e: &MidiEvent) {
        if e.rendered {
            return;
        }
        // As the engine's slot routing: by musician, else by its channel (k+1) unless lower.
        let k = match usize::try_from(e.musician) {
            Ok(k) => k,
            Err(_) if !e.lower => usize::from(e.status & 0x0F).wrapping_sub(1),
            Err(_) => return,
        };
        let at = self.at.saturating_add(i64::from(e.offset.max(0)));
        if at < self.range.start {
            if let Some(b) = self.before.get_mut(k) {
                b.push((at, [e.status, e.d1, e.d2], e.lower, e.keyswitch));
            }
            return;
        }
        if !self.range.contains(at) {
            return;
        }
        if let Some(slot) = self.slots.get_mut(k) {
            slot.push((at, [e.status, e.d1, e.d2], e.lower));
        }
    }
}

/// What the band plays over `range` for the musicians in `slots` (0-based, as
/// `Route::Musician`), one sequence per musician (the others stay empty).
///
/// It renders from sample 0 in 512-frame blocks at the session rate, as playback does, and keeps
/// the events whose sample is in `range`. A range that starts mid-song starts with the band's
/// state there (see [`state_at`]): the last controllers, program and pitch bend, and the
/// keyswitches still held, on its first tick. Ticks count from `range.start`'s tick (a clip placed at
/// `range.start` plays them where the band played them). Notes pair by (channel, pitch), first on
/// with first off; a note still sounding at `range.end` is cut there. Channels are the delivered
/// ones (channel 1, the organ's lower manual its own). Controllers, pitch bend, pressure and
/// program changes become controller events; channel mode messages (CC 120–127: the band's resets
/// when it starts, stops or jumps) are the transport's and are not printed, since clip playback
/// makes its own. Keyswitches are notes.
pub fn render_print(s: &Session, lib: &Arc<LibraryData>, names: &dyn Fn(TrackId) -> Option<String>, range: Range, slots: &[u8]) -> [Sequence; 6] {
    let mut out: [Sequence; 6] = Default::default();
    if range.is_empty() {
        return out;
    }
    let Some(u) = prepare_band(s, lib, names) else { return out };
    // As the engine builds its runtime: at its `f32` rate.
    let rate = s.sample_rate.as_f64() as f32;
    let mut rt = BandRuntime::new(f64::from(rate), u.settings.clone());
    let mut events: [Vec<(Samples, [u8; 3], bool)>; BAND_SLOTS] = Default::default();
    let mut before: [Before; BAND_SLOTS] = Default::default();
    let mut playing = false;
    let mut pos: Samples = 0;
    // Past the song's end the band is stopped and silent: the render stops a block after it.
    let until = range.end.min(u.song_end.map_or(0, |e| e.saturating_add(BLOCK)));
    while pos < until {
        let block_end = pos.saturating_add(BLOCK);
        let mut cuts: Vec<Samples> = [u.start, u.song_end].into_iter().flatten().filter(|b| *b > pos && *b < block_end).collect();
        cuts.sort_unstable();
        cuts.push(block_end);
        let mut at = pos;
        for cut in cuts {
            if cut <= at {
                continue;
            }
            let t = band_transport_within(s, at, true, u.start, u.song_end);
            let mut tape = Tape { slots: &mut events, before: &mut before, at, range };
            if playing && !t.playing {
                rt.flush(&mut tape);
            }
            playing = t.playing;
            rt.process_block(&t, usize::try_from(cut - at).unwrap_or(0), &mut tape);
            at = cut;
        }
        pos = block_end;
    }
    for (k, ((seq, ev), pre)) in out.iter_mut().zip(&events).zip(&before).enumerate() {
        if slots.iter().any(|&n| usize::from(n) == k) {
            // A range that starts mid-song starts with the band's state there, as playback has it.
            let mut all = if ev.is_empty() { Vec::new() } else { state_at(range.start, pre, ev) };
            all.extend_from_slice(ev);
            *seq = events_to_sequence(s, range, &all);
        }
    }
    out
}

/// The band's state at `start` from its events before it, as events on `start`: the last value
/// of each controller per channel, the last program, a pitch bend off centre, and the keyswitch
/// notes still held (their note-offs are in `after`, the range's events). Channel mode messages
/// reset it as an instrument does: reset all controllers (121) the controllers and bend, all
/// notes off (123, and 120, 124–127) the held keyswitches.
fn state_at(start: Samples, before: &[(Samples, [u8; 3], bool, bool)], after: &[(Samples, [u8; 3], bool)]) -> Vec<(Samples, [u8; 3], bool)> {
    use std::collections::BTreeMap;
    // By the delivered channel (channel 1, the lower manual its own); each keeps its bytes.
    let chan = |status: u8, lower: bool| if lower { status & 0x0F } else { 0 };
    let mut ccs: BTreeMap<(u8, u8), ([u8; 3], bool)> = BTreeMap::new();
    let mut programs: BTreeMap<u8, ([u8; 3], bool)> = BTreeMap::new();
    let mut bends: BTreeMap<u8, ([u8; 3], bool)> = BTreeMap::new();
    let mut held: Vec<([u8; 3], bool)> = Vec::new();
    for &(_, b, lower, ks) in before {
        let [status, d1, d2] = b;
        if status >= 0xF0 {
            continue;
        }
        let ch = chan(status, lower);
        let same_note = |h: &([u8; 3], bool)| chan(h.0[0], h.1) == ch && h.0[1] == d1;
        match status & 0xF0 {
            0x90 if d2 > 0 => {
                if ks {
                    held.push((b, lower));
                }
            }
            0x80 | 0x90 => {
                if let Some(i) = held.iter().position(same_note) {
                    held.remove(i);
                }
            }
            0xB0 if d1 == 121 => {
                ccs.retain(|&(c, _), _| c != ch);
                bends.remove(&ch);
            }
            0xB0 if d1 >= 120 && d1 != 122 => held.retain(|h| chan(h.0[0], h.1) != ch),
            0xB0 if d1 >= 120 => {}
            0xB0 => {
                ccs.insert((ch, d1), (b, lower));
            }
            0xC0 => {
                programs.insert(ch, (b, lower));
            }
            0xE0 => {
                bends.insert(ch, (b, lower));
            }
            _ => {}
        }
    }
    let centre = |b: &[u8; 3]| b[1] == 0 && b[2] == 64;
    // A keyswitch released on the range's start is not held there.
    let released_at_start = |h: &([u8; 3], bool)| {
        after.iter().take_while(|e| e.0 == start).any(|&(_, b, lower)| {
            (b[0] & 0xF0 == 0x80 || (b[0] & 0xF0 == 0x90 && b[2] == 0)) && chan(b[0], lower) == chan(h.0[0], h.1) && b[1] == h.0[1]
        })
    };
    let mut out: Vec<(Samples, [u8; 3], bool)> = Vec::new();
    out.extend(programs.into_values().map(|(b, l)| (start, b, l)));
    out.extend(ccs.into_values().map(|(b, l)| (start, b, l)));
    out.extend(bends.into_values().filter(|(b, _)| !centre(b)).map(|(b, l)| (start, b, l)));
    out.extend(held.into_iter().filter(|h| !released_at_start(h)).map(|(b, l)| (start, b, l)));
    out
}

/// One musician's events, `(sample, bytes, lower)` in order as the band delivers them, as a
/// sequence whose tick 0 is `range.start` (see [`render_print`]).
pub fn events_to_sequence(s: &Session, range: Range, events: &[(Samples, [u8; 3], bool)]) -> Sequence {
    let sr = s.sample_rate;
    let base = s.tempo.samples_to_ticks(range.start, sr);
    let tick = |at: Samples| s.tempo.samples_to_ticks(at, sr).saturating_sub(base);
    let mut seq = Sequence::default();
    // Sounding notes by (channel, pitch): their (on sample, velocity), oldest first.
    let mut open: HashMap<(u8, u8), VecDeque<(Samples, u8)>> = HashMap::new();
    // Every note ends by the range's end (clip playback clamps its note-off to the clip's end).
    let last = tick(range.end);
    let close = |seq: &mut Sequence, ch: u8, pitch: u8, on: Samples, vel: u8, off: Samples, rel: u8| {
        let start = tick(on);
        let length = tick(off).min(last).saturating_sub(start).max(1);
        seq.notes.push(Note { pitch, velocity: vel.clamp(1, 127), release_velocity: rel, channel: ch, start, length });
    };
    for &(at, [status, d1, d2], lower) in events {
        if status >= 0xF0 {
            continue;
        }
        // Delivery: channel 1, the lower manual its own.
        let ch = if lower { status & 0x0F } else { 0 };
        let pitch = d1 & 0x7F;
        match status & 0xF0 {
            0x90 if d2 > 0 => open.entry((ch, pitch)).or_default().push_back((at, d2)),
            0x80 | 0x90 => {
                let rel = if status & 0xF0 == 0x80 { d2 & 0x7F } else { 64 };
                if let Some((on, vel)) = open.get_mut(&(ch, pitch)).and_then(VecDeque::pop_front) {
                    close(&mut seq, ch, pitch, on, vel, at, rel);
                }
            }
            0xB0 if d1 >= 120 => {}
            0xB0 => seq.ctrls.push(CtrlEvent { tick: tick(at), channel: ch, kind: CtrlKind::Cc(pitch), value: i32::from(d2 & 0x7F) }),
            0xE0 => {
                let v = i32::from(d1 & 0x7F) | (i32::from(d2 & 0x7F) << 7);
                seq.ctrls.push(CtrlEvent { tick: tick(at), channel: ch, kind: CtrlKind::PitchBend, value: v - 8192 });
            }
            0xD0 => seq.ctrls.push(CtrlEvent { tick: tick(at), channel: ch, kind: CtrlKind::ChannelPressure, value: i32::from(d1 & 0x7F) }),
            0xA0 => seq.ctrls.push(CtrlEvent { tick: tick(at), channel: ch, kind: CtrlKind::PolyPressure(pitch), value: i32::from(d2 & 0x7F) }),
            0xC0 => seq.ctrls.push(CtrlEvent { tick: tick(at), channel: ch, kind: CtrlKind::Program, value: i32::from(d1 & 0x7F) }),
            _ => {}
        }
    }
    // Still sounding at the range's end: cut there.
    let mut rest: Vec<((u8, u8), (Samples, u8))> = open.into_iter().flat_map(|(key, q)| q.into_iter().map(move |n| (key, n))).collect();
    rest.sort_unstable();
    for ((ch, pitch), (on, vel)) in rest {
        close(&mut seq, ch, pitch, on, vel, range.end, 64);
    }
    seq.sort();
    seq
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_state_at_a_start_is_the_last_of_each_and_the_held_keyswitches() {
        let before = [
            (0, [0x91, 24, 100], false, true), // a keyswitch, held over the start
            (0, [0x91, 25, 100], false, true), // one released before it
            (5, [0x81, 25, 0], false, true),
            (0, [0x91, 26, 100], false, true), // one released on the start
            (1, [0xB1, 1, 40], false, false),
            (2, [0xB1, 1, 50], false, false), // the last CC 1 wins
            (2, [0xB3, 1, 70], true, false),  // the lower manual's own
            (3, [0xC1, 7, 0], false, false),
            (4, [0xE1, 0, 64], false, false), // bend at centre: not sent
            (4, [0xE3, 0, 80], true, false),
            (6, [0x91, 60, 100], false, false), // a plain note held over the start: not state
        ];
        let after = [(100, [0x81, 26, 0], false), (900, [0x81, 24, 0], false)];
        let got = state_at(100, &before, &after);
        assert_eq!(
            got,
            vec![
                (100, [0xC1, 7, 0], false),
                (100, [0xB1, 1, 50], false),
                (100, [0xB3, 1, 70], true),
                (100, [0xE3, 0, 80], true),
                (100, [0x91, 24, 100], false)
            ]
        );

        // Reset all controllers clears the controllers and bend; all notes off the keyswitches.
        let mut reset = before.to_vec();
        reset.push((7, [0xB1, 121, 0], false, false));
        reset.push((7, [0xB1, 123, 0], false, false));
        assert_eq!(state_at(100, &reset, &[]), vec![(100, [0xC1, 7, 0], false), (100, [0xB3, 1, 70], true), (100, [0xE3, 0, 80], true)]);
    }
}

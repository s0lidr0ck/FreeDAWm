use super::*;
use soundcraft_model::MidiFxKind::{Arpeggiator, Chord, NoteRepeat, Scale};
use std::sync::Mutex;

type Log = Arc<Mutex<Vec<(usize, [u8; 3], bool)>>>;

/// An instrument that writes down what it is given: (absolute sample, message, raw); an All
/// Notes Off is written as `[0xFF, 0, 0]`.
struct Ear {
    base: usize,
    log: Log,
}

impl Ear {
    fn put(&mut self, offset: usize, msg: [u8; 3], raw: bool) {
        self.log.lock().unwrap().push((self.base + offset, msg, raw));
    }
}

impl Plugin for Ear {
    fn info(&self) -> &'static PluginInfo {
        &crate::track_midi::INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], frames: usize) {
        self.base += frames;
    }
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.put(offset, [0x90, note, velocity], false);
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.put(offset, [0x80, note, 0], false);
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.put(offset, msg, false);
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.put(offset, msg, true);
    }
    fn all_notes_off(&mut self) {
        self.put(0, [0xFF, 0, 0], false);
    }
}

const SR: f32 = 48_000.0;
/// 1/16 at 120 bpm, 48 kHz.
const STEP: usize = 6_000;

fn host(fx: &[MidiFx]) -> (Box<dyn Plugin>, Log) {
    let log = Log::default();
    let mut p: Box<dyn Plugin> = Box::new(Ear { base: 0, log: log.clone() });
    sync(&mut p, fx, SR);
    p.set_transport(&Transport { tempo: 120.0, ..Transport::default() });
    (p, log)
}

fn fx(kind: MidiFxKind, f: impl FnOnce(&mut MidiFx)) -> MidiFx {
    let mut x = MidiFx::new(kind);
    f(&mut x);
    x
}

fn blocks(p: &mut Box<dyn Plugin>, n: usize, size: usize) {
    let mut io = vec![vec![0.0f32; size]; 2];
    for _ in 0..n {
        p.process(&mut io, size);
    }
}

/// The notes heard: (sample, 0x90 or 0x80, pitch).
fn notes(log: &Log) -> Vec<(usize, u8, u8)> {
    log.lock().unwrap().iter().filter(|(_, m, _)| matches!(m[0] & 0xF0, 0x80 | 0x90)).map(|(t, m, _)| (*t, m[0] & 0xF0, m[1])).collect()
}

fn take(log: &Log) -> Vec<(usize, u8, u8)> {
    let n = notes(log);
    log.lock().unwrap().clear();
    n
}

#[test]
fn a_track_without_midi_effects_is_left_alone_and_an_empty_list_passes_through() {
    let mut p: Box<dyn Plugin> = Box::new(Ear { base: 0, log: Log::default() });
    sync(&mut p, &[], SR);
    assert!(p.host_wrapper().is_none(), "never wrapped while it has no effect");
    // Wrapped, then emptied or bypassed: every call arrives as it was made, at once.
    let (mut p, log) = host(&[MidiFx::new(Chord)]);
    sync(&mut p, &[fx(Chord, |f| f.bypass = true)], SR);
    p.note_on(5, 60, 90);
    p.midi_raw(7, [0x93, 62, 80]);
    p.midi(9, [0xB0, 1, 30]);
    assert_eq!(*log.lock().unwrap(), vec![(5, [0x90, 60, 90], false), (7, [0x93, 62, 80], true), (9, [0xB0, 1, 30], false)]);
    sync(&mut p, &[], SR);
    log.lock().unwrap().clear();
    p.note_off(1, 60);
    assert_eq!(*log.lock().unwrap(), vec![(1, [0x80, 60, 0], false)]);
}

#[test]
fn chord_plays_the_shape_and_shared_notes_end_with_the_last_key() {
    let (mut p, log) = host(&[MidiFx::new(Chord)]);
    p.note_on(10, 60, 100);
    p.note_on(20, 64, 100);
    blocks(&mut p, 1, 512);
    // C major on C, then on E: E is already sounding (from C's chord), G#, B added.
    assert_eq!(take(&log), vec![(10, 0x90, 60), (10, 0x90, 64), (10, 0x90, 67), (20, 0x90, 68), (20, 0x90, 71)]);
    p.note_off(0, 60);
    blocks(&mut p, 1, 512);
    // C's chord ends but E (still held by the E key's root) stays.
    assert_eq!(take(&log), vec![(512, 0x80, 60), (512, 0x80, 67)]);
    p.note_off(0, 64);
    blocks(&mut p, 1, 512);
    assert_eq!(take(&log), vec![(1024, 0x80, 64), (1024, 0x80, 68), (1024, 0x80, 71)]);
}

#[test]
fn scale_moves_notes_to_the_nearest_in_key_or_leaves_them_out() {
    assert_eq!(snap(61, 0, SCALES[0].1, false), Some(60), "C# in C major: the lower of C and D");
    assert_eq!(snap(66, 0, SCALES[0].1, false), Some(65), "F# goes to F");
    assert_eq!(snap(66, 7, SCALES[0].1, false), Some(66), "F# is in G major");
    assert_eq!(snap(61, 0, SCALES[0].1, true), None);
    let (mut p, log) = host(&[fx(Scale, |f| f.root = 2)]);
    // D major: C (60) is out, C# (61) in.
    p.midi_raw(0, [0x91, 60, 90]);
    p.midi_raw(5, [0x91, 60, 0]);
    blocks(&mut p, 1, 64);
    let got = log.lock().unwrap().clone();
    assert_eq!(got, vec![(0, [0x91, 59, 90], true), (5, [0x91, 59, 0], true)], "B, on its channel, raw kept");
    log.lock().unwrap().clear();
    sync(&mut p, &[fx(Scale, |f| (f.root, f.drop) = (2, true))], SR);
    p.note_on(0, 60, 90);
    p.note_on(0, 62, 90);
    blocks(&mut p, 1, 64);
    assert_eq!(take(&log), vec![(64, 0x90, 62)]);
}

#[test]
fn the_arpeggiator_steps_through_the_held_keys_on_the_tempo() {
    let (mut p, log) = host(&[MidiFx::new(Arpeggiator)]);
    // Stopped: it starts on the first key, 1/16 steps at 120 bpm, half gate.
    p.note_on(100, 64, 90);
    p.note_on(100, 60, 80);
    p.note_on(100, 67, 70);
    blocks(&mut p, 50, 512); // 25600 samples: steps at 100, 6100, …, 24100
    let got = take(&log);
    let ons: Vec<(usize, u8)> = got.iter().filter(|n| n.1 == 0x90).map(|n| (n.0, n.2)).collect();
    assert_eq!(ons, vec![(100, 60), (100 + STEP, 64), (100 + 2 * STEP, 67), (100 + 3 * STEP, 60), (100 + 4 * STEP, 64)]);
    let offs: Vec<usize> = got.iter().filter(|n| n.1 == 0x80).map(|n| n.0).collect();
    assert_eq!(offs[..3], [100 + STEP / 2, 100 + STEP + STEP / 2, 100 + 2 * STEP + STEP / 2]);
    // Keys up: no more steps (the last note still ends at its gate).
    for k in [60, 64, 67] {
        p.note_off(0, k);
    }
    blocks(&mut p, 30, 512);
    assert!(take(&log).iter().all(|n| n.1 == 0x80), "nothing new once the keys are up");
}

#[test]
fn the_arpeggiator_follows_the_song_grid_patterns_and_octaves() {
    let (mut p, log) = host(&[fx(Arpeggiator, |f| (f.pattern, f.octaves, f.rate) = (2, 2, 1))]);
    // Playing, a tenth of a quarter in: 1/8 steps fall on 0.5, 1.0, 1.5 … quarters.
    let spq = 24_000.0;
    p.set_transport(&Transport { tempo: 120.0, playing: true, quarters: 0.1, ..Transport::default() });
    p.note_on(0, 60, 90);
    p.note_on(0, 62, 90);
    let mut io = vec![vec![0.0f32; 512]; 2];
    let mut q = 0.1;
    for _ in 0..200 {
        p.set_transport(&Transport { tempo: 120.0, playing: true, quarters: q, ..Transport::default() });
        p.process(&mut io, 512);
        q += 512.0 / spq;
    }
    let ons: Vec<(usize, u8)> = take(&log).iter().filter(|n| n.1 == 0x90).map(|n| (n.0, n.2)).collect();
    let first = ((0.5 - 0.1) * spq) as usize;
    assert_eq!(ons.first().map(|x| x.0), Some(first), "the first step on the grid, not at the key");
    // Up/Down over two octaves of C D: C D C' D' C' D (no repeat at the ends), then again.
    let pitches: Vec<u8> = ons.iter().map(|x| x.1).take(8).collect();
    assert_eq!(pitches, vec![60, 62, 72, 74, 72, 62, 60, 62]);
    let gaps: Vec<usize> = ons.windows(2).map(|w| w[1].0 - w[0].0).collect();
    assert!(gaps.iter().all(|g| g.abs_diff(12_000) <= 1), "an eighth apart: {gaps:?}");
}

#[test]
fn latch_keeps_the_keys_until_new_ones_are_pressed() {
    let (mut p, log) = host(&[fx(Arpeggiator, |f| f.latch = true)]);
    p.note_on(0, 60, 90);
    p.note_off(10, 60);
    blocks(&mut p, 30, 512);
    let ons = take(&log).iter().filter(|n| n.1 == 0x90).count();
    assert!(ons >= 2, "still playing after the key is up: {ons}");
    p.note_on(0, 70, 90);
    blocks(&mut p, 30, 512);
    let pitches: Vec<u8> = take(&log).iter().filter(|n| n.1 == 0x90).map(|n| n.2).collect();
    assert!(pitches.iter().all(|x| *x == 70), "the new key replaces the latched one: {pitches:?}");
}

#[test]
fn note_repeat_plays_the_key_again_with_its_pressure() {
    let (mut p, log) = host(&[MidiFx::new(NoteRepeat)]);
    p.note_on(100, 38, 100);
    blocks(&mut p, 12, 512); // 6144 samples
    p.midi(0, [0xD0, 40, 0]);
    blocks(&mut p, 12, 512);
    let got = log.lock().unwrap().clone();
    let ons: Vec<(usize, u8)> = got.iter().filter(|(_, m, _)| m[0] == 0x90).map(|(t, m, _)| (*t, m[2])).collect();
    assert_eq!(ons, vec![(100, 100), (100 + STEP, 100), (100 + 2 * STEP, 40)], "the third hit at the pressure");
    let offs = got.iter().filter(|(_, m, _)| m[0] == 0x80).count();
    assert_eq!(offs, 2, "each hit ends at its gate");
    log.lock().unwrap().clear();
    p.note_off(0, 38);
    blocks(&mut p, 30, 512);
    assert_eq!(notes(&log), vec![(24 * 512, 0x80, 38)], "the hit sounding ends with the key, and nothing follows");
}

#[test]
fn a_changed_setting_lets_go_and_plays_the_held_keys_through_the_new_list() {
    let (mut p, log) = host(&[MidiFx::new(Chord)]);
    p.note_on(0, 60, 100);
    blocks(&mut p, 1, 64);
    take(&log);
    sync(&mut p, &[fx(Chord, |f| f.chord = 1)], SR);
    blocks(&mut p, 1, 64);
    let got = take(&log);
    let offs: Vec<u8> = got.iter().filter(|n| n.1 == 0x80).map(|n| n.2).collect();
    let ons: Vec<u8> = got.iter().filter(|n| n.1 == 0x90).map(|n| n.2).collect();
    assert_eq!((offs.len(), ons), (3, vec![60, 63, 67]), "the major chord let go, the minor one played");
    // Unchanged settings change nothing; removing the effect plays the key as it is.
    sync(&mut p, &[fx(Chord, |f| f.chord = 1)], SR);
    blocks(&mut p, 1, 64);
    assert!(take(&log).is_empty());
    sync(&mut p, &[], SR);
    let got = take(&log);
    assert_eq!(got.iter().filter(|n| n.1 == 0x80).count(), 3);
    assert_eq!(got.iter().filter(|n| n.1 == 0x90).map(|n| n.2).collect::<Vec<_>>(), vec![60]);
    // All Notes Off forgets the keys: a later change plays nothing again.
    p.all_notes_off();
    sync(&mut p, &[MidiFx::new(Chord)], SR);
    blocks(&mut p, 1, 64);
    assert!(take(&log).is_empty());
}

#[test]
fn effects_run_in_list_order() {
    // Scale (C major) then a power chord: C# → C → C G. The other way round: C# G# → C G.
    let (mut p, log) = host(&[MidiFx::new(Scale), fx(Chord, |f| f.chord = 9)]);
    p.note_on(0, 61, 90);
    blocks(&mut p, 1, 64);
    assert_eq!(take(&log), vec![(0, 0x90, 60), (0, 0x90, 67)]);
    let (mut p, log) = host(&[fx(Chord, |f| f.chord = 9), MidiFx::new(Scale)]);
    p.note_on(0, 61, 90);
    blocks(&mut p, 1, 64);
    assert_eq!(take(&log), vec![(0, 0x90, 60), (0, 0x90, 67)]);
    // An arpeggiator before a chord arpeggiates chords.
    let (mut p, log) = host(&[MidiFx::new(Arpeggiator), MidiFx::new(Chord)]);
    p.note_on(0, 60, 90);
    blocks(&mut p, 1, 64);
    assert_eq!(take(&log), vec![(0, 0x90, 60), (0, 0x90, 64), (0, 0x90, 67)]);
}

#[test]
fn a_flood_is_dropped_and_counted_and_zero_frames_are_safe() {
    let (mut p, _log) = host(&[MidiFx::new(Chord)]);
    for i in 0..3 * CAP {
        p.midi(i % 64, [0xB0, 1, (i % 128) as u8]);
    }
    blocks(&mut p, 1, 0);
    blocks(&mut p, 1, 64);
    let h = p.host_wrapper().and_then(|a| a.downcast_mut::<MidiFxHost>()).unwrap();
    assert!(h.dropped() > 0);
    let (mut p, _log) = host(&[MidiFx::new(Arpeggiator), MidiFx::new(NoteRepeat)]);
    p.note_on(0, 60, 90);
    blocks(&mut p, 3, 0);
    blocks(&mut p, 3, 1);
}

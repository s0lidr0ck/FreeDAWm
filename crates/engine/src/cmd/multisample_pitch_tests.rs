//! A captured Pantry plays each key at the pitch its source plays it: captured from a sine of
//! known pitch, every sample is measured against the note it is filed under, and the Pantry is
//! played key by key and measured against the source. Covers the capture dialog's ranges and
//! spacings (a low note moved off the default, every 2, 3, 4 and 6 semitones), session rates
//! (and a session converted after capture), mono, layers and takes, a stand-in for a hosted
//! instrument (reported latency, a slow attack, a long release), and the source track's
//! transposes (the global transpose track, MIDI Real-Time Properties).

use super::*;
use crate::Engine;
use soundcraft_dsp::{Category, Plugin, PluginInfo};
use soundcraft_model::Session;
use soundcraft_time::SampleRate;

/// Equal temperament, A (69) at 440 Hz.
fn hz(note: u8) -> f64 {
    440.0 * 2f64.powf((f64::from(note) - 69.0) / 12.0)
}

/// The fundamental of a sine: its rising zero crossings, interpolated, over the part of `x`
/// within 20 dB of its peak (a fade or a decay near silence is left out).
fn measure(x: &[f32], sr: f64) -> f64 {
    let peak = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
    let loud = |v: &f32| v.abs() > 0.1 * peak;
    let (Some(a), Some(b)) = (x.iter().position(loud), x.iter().rposition(loud)) else { return 0.0 };
    crossings(&x[a..=b], sr)
}

fn crossings(x: &[f32], sr: f64) -> f64 {
    let (mut first, mut last, mut count) = (None, 0.0, 0usize);
    for (i, w) in x.windows(2).enumerate() {
        let (a, b) = (w[0], w[1]);
        if a < 0.0 && b >= 0.0 {
            let at = i as f64 + f64::from(a / (a - b));
            if first.is_some() {
                count += 1;
            } else {
                first = Some(at);
            }
            last = at;
        }
    }
    match first {
        Some(f) if count > 0 => count as f64 * sr / (last - f),
        _ => 0.0,
    }
}

fn cents(measured: f64, expected: f64) -> f64 {
    1200.0 * (measured / expected).log2()
}

/// Well inside a semitone (100 cents): the zero-crossing count over a short window.
const CLOSE: f64 = 3.0;

fn engine(rate: u32) -> Engine {
    Engine::new(Session::new("pitch", SampleRate::new(rate).unwrap()))
}

/// Burley playing a plain sine: one oscillator, the filter open, no modulation.
fn sine_track(e: &mut Engine) -> TrackId {
    let v = e.execute("track.new", &json!({"kind": "instrument", "instrument": "subtractive_synth"})).unwrap();
    let t = TrackId(v["tracks"][0].as_u64().unwrap());
    for (param, value) in [
        ("osc1_wave", 2.0),
        ("osc2_wave", 2.0),
        ("osc2_detune", 0.0),
        ("osc_mix", 0.0),
        ("cutoff", 20000.0),
        ("resonance", 0.0),
        ("filter_env_amount", 0.0),
        ("amp_sustain", 100.0),
    ] {
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": param, "value": value})).unwrap();
    }
    t
}

/// A stand-in for a hosted piano (an id under a host prefix): it reports latency and sounds that
/// late, swells in over a quarter second, rings on long after its key comes up, and tunes itself
/// from the rate it is prepared at.
struct SlowSine {
    sr: f32,
    /// (note, phase, level, held, frames until it starts)
    voices: Vec<(u8, f64, f32, bool, usize)>,
}

static SLOW_INFO: PluginInfo = PluginInfo {
    id: "clap:test.slowsine",
    name: "Slow Sine",
    short_name: "Slow",
    category: Category::Instrument,
    params: &[],
    is_instrument: true,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};
const SLOW_LATENCY: usize = 1500;

impl Plugin for SlowSine {
    fn info(&self) -> &'static PluginInfo {
        &SLOW_INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.sr = sample_rate;
    }
    fn reset(&mut self) {
        self.voices.clear();
    }
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn latency(&self) -> usize {
        SLOW_LATENCY
    }
    fn tail_samples(&self) -> usize {
        (self.sr * 6.0) as usize
    }
    fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
        self.voices.retain(|v| v.0 != note);
        self.voices.push((note, 0.0, 0.0, true, offset + SLOW_LATENCY));
    }
    fn note_off(&mut self, _offset: usize, note: u8) {
        for v in self.voices.iter_mut().filter(|v| v.0 == note) {
            v.3 = false;
        }
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let sr = f64::from(self.sr);
        let (rise, fall) = (1.0 / (0.25 * self.sr), (-1.0 / (0.15 * self.sr)).exp());
        for f in 0..frames {
            let mut out = 0.0f32;
            for v in &mut self.voices {
                if v.4 > 0 {
                    v.4 -= 1;
                    continue;
                }
                v.2 = if v.3 { (v.2 + rise).min(1.0) } else { v.2 * fall };
                v.1 = (v.1 + hz(v.0) / sr).fract();
                out += 0.3 * v.2 * (v.1 * std::f64::consts::TAU).sin() as f32;
            }
            for c in io.iter_mut() {
                if let Some(x) = c.get_mut(f) {
                    *x = out;
                }
            }
        }
        self.voices.retain(|v| v.3 || v.2 > 1.0e-6);
    }
}

fn slow_factory(id: &str) -> Option<Box<dyn Plugin>> {
    (id == SLOW_INFO.id).then(|| Box::new(SlowSine { sr: 48_000.0, voices: Vec::with_capacity(256) }) as Box<dyn Plugin>)
}

fn slow_track(e: &mut Engine) -> TrackId {
    soundcraft_mix::set_test_plugin_factory(Some(slow_factory));
    let t = sine_track(e);
    let ins = e.session_mut().track_mut(t).unwrap().instrument.as_mut().unwrap();
    ins.plugin = SLOW_INFO.id.into();
    ins.params.clear();
    t
}

/// Plays `notes` on `track`, each held a while, and measures each from the middle of the note.
fn play(e: &Engine, track: TrackId, notes: &[u8]) -> Vec<f64> {
    let mut s = e.session().clone();
    let sr = s.sample_rate;
    let (len, gap) = (sr.samples(0.6), sr.samples(1.6));
    let mut seq = Sequence::default();
    let mut at = sr.samples(0.2);
    let mut starts = Vec::new();
    for &n in notes {
        let (on, off) = (s.tempo.samples_to_ticks(at, sr), s.tempo.samples_to_ticks(at + len, sr));
        seq.notes.push(Note {
            pitch: n,
            velocity: 100,
            release_velocity: 64,
            channel: 0,
            muted: false,
            probability: 100,
            start: on,
            length: off - on,
        });
        starts.push(s.tempo.tick_to_samples(on, sr));
        at += gap;
    }
    seq.sort();
    let clip = Clip::midi(s.new_clip_id(), "Play", 0, at, seq);
    s.track_mut(track).unwrap().playlist_mut().unwrap().clips = vec![clip];
    let (audio, _) = soundcraft_mix::TrackRender::frozen(&s, track, at, None, None).run();
    starts
        .iter()
        .map(|&st| {
            // Past a slow attack and inside the shortest sample (a capture holds its notes 0.25 s
            // at the least).
            let a = usize::try_from(st + sr.samples(0.08)).unwrap();
            let b = usize::try_from(st + sr.samples(0.24)).unwrap().min(audio[0].len());
            measure(&audio[0][a..b], sr.as_f64())
        })
        .collect()
}

fn capture(e: &mut Engine, track: TrackId, extra: Value) -> TrackId {
    let mut p = json!({"track": track.0, "low": 48, "high": 72, "step": 3, "layers": 1, "hold": 0.4, "hold_high": 0.4, "tail": 0.1});
    for (k, v) in extra.as_object().unwrap() {
        p[k] = v.clone();
    }
    let v = e.execute("track.capture_multisample", &p).unwrap();
    assert_eq!(v["silent"], 0, "{p}");
    TrackId(v["track"].as_u64().unwrap())
}

/// Every key sample of `pantry` is the note it is filed under (and so plays where it should);
/// returns the notes captured.
fn check_samples(label: &str, e: &Engine, pantry: TrackId) -> Vec<u8> {
    let s = e.session();
    let ins = s.track(pantry).unwrap().instrument.as_ref().unwrap();
    let mut notes = BTreeSet::new();
    for (key, src) in &ins.samples {
        let Some((note, _, _)) = parse_sample_key(key) else { continue };
        // (decoded from its file when it plays from disk)
        let audio = s.pool.get(*src).unwrap().buffer().into_owned();
        let ch = &audio.channels[0];
        let rate = f64::from(audio.sample_rate);
        // Past the attack (and any latency), before the fade.
        let (a, b) = ((rate * 0.1) as usize, ch.len().saturating_sub((rate * 0.05) as usize));
        let got = measure(&ch[a..b], rate);
        let c = cents(got, hz(note as u8));
        assert!(c.abs() < CLOSE, "{label}: sample {key} sounds {got:.2} Hz, its note is {:.2} Hz ({c:+.1} cents)", hz(note as u8));
        notes.insert(note as u8);
    }
    notes.into_iter().collect()
}

/// The Pantry plays every key from `keys` as the source track does, and at equal temperament
/// shifted by `shift` semitones (the global transpose both follow).
fn check_keys(label: &str, e: &Engine, source: TrackId, pantry: TrackId, keys: &[u8], shift: i8) {
    let (want, got) = (play(e, source, keys), play(e, pantry, keys));
    for ((k, w), g) in keys.iter().zip(&want).zip(&got) {
        let tuned = hz(k.saturating_add_signed(shift));
        assert!(cents(*w, tuned).abs() < CLOSE, "{label}: the source plays key {k} at {w:.2} Hz, not {tuned:.2}");
        let c = cents(*g, *w);
        assert!(c.abs() < CLOSE, "{label}: key {k}: the Pantry plays {g:.2} Hz, the source {w:.2} Hz ({c:+.1} cents)");
    }
}

/// The capture dialog's range moved off its default (the owner's report: the low note from A-1,
/// MIDI 21, to C0, MIDI 24, every 3 semitones), and other spacings: each sample is the note it is
/// filed under, the notes are the ones asked for, and every key plays at its own pitch.
#[test]
fn captured_notes_are_filed_under_the_notes_played_whatever_the_range_and_spacing() {
    for (low, high, step) in [(24, 108, 3), (21, 108, 3), (22, 100, 2), (25, 101, 4), (24, 96, 6), (23, 23, 3)] {
        let mut e = engine(48_000);
        let src = sine_track(&mut e);
        let label = format!("{low}..{high} every {step}");
        let p = capture(&mut e, src, json!({"low": low, "high": high, "step": step, "hold": 0.25, "hold_high": 0.25, "tail": 0.05}));
        let notes = check_samples(&label, &e, p);
        assert_eq!(notes, (low..=high).step_by(step).collect::<Vec<u8>>(), "{label}");
    }
    // The owner's capture, played key by key over a stretch of it (on and between the notes).
    let mut e = engine(48_000);
    let src = sine_track(&mut e);
    let p = capture(&mut e, src, json!({"low": 24, "high": 108, "step": 3}));
    check_keys("C0 to C8 every 3", &e, src, p, &(20..=40).chain(58..=62).chain(100..=112).collect::<Vec<u8>>(), 0);
}

/// At 44.1, 48 and 96 kHz, after the session is converted to another rate, in mono, with layers
/// and takes: every sample its own note, every key its own pitch.
#[test]
fn a_captured_pantry_plays_in_tune_at_any_session_rate() {
    let keys: Vec<u8> = (46..=74).collect();
    for rate in [44_100, 48_000, 96_000] {
        let mut e = engine(rate);
        let src = sine_track(&mut e);
        let p = capture(&mut e, src, json!({}));
        check_samples(&format!("{rate} Hz"), &e, p);
        check_keys(&format!("{rate} Hz"), &e, src, p, &keys, 0);
    }
    let mut e = engine(48_000);
    let src = sine_track(&mut e);
    let p = capture(&mut e, src, json!({"mono": true, "layers": 3, "robins": 2}));
    e.execute("session.set_sample_rate", &json!({"rate": 44_100})).unwrap();
    check_samples("captured at 48 kHz, converted to 44.1", &e, p);
    check_keys("captured at 48 kHz, converted to 44.1", &e, src, p, &keys, 0);
}

/// A stand-in for a hosted piano: latency, a slow attack, a release that rings into the next
/// strike's slot, layers, takes and release samples. Nothing of one note lands in another's
/// sample.
#[test]
fn a_slow_instrument_with_latency_captures_in_tune() {
    for rate in [44_100, 48_000] {
        let mut e = engine(rate);
        let src = slow_track(&mut e);
        let label = format!("slow, latency, {rate} Hz");
        let p = capture(
            &mut e,
            src,
            json!({"low": 45, "high": 75, "step": 3, "layers": 2, "robins": 2, "release": true, "hold": 0.6, "hold_high": 0.6, "tail": 0.1}),
        );
        check_samples(&label, &e, p);
        check_keys(&label, &e, src, p, &(44..=76).collect::<Vec<u8>>(), 0);
    }
}

/// The global transpose track (the Key ruler's "T+2") and the track's Real-Time Properties are
/// how the source track plays its clips: the capture plays each note to the instrument as
/// written, and the Pantry follows the global transpose itself, as the source does. Before the
/// fix the transpose was heard twice (the Pantry 200 cents above the source at every key).
#[test]
fn the_source_tracks_transposes_are_not_captured_into_the_samples() {
    let keys: Vec<u8> = (46..=74).collect();
    let mut e = engine(48_000);
    let src = sine_track(&mut e);
    e.execute("song.transpose", &json!({"at": 0, "semitones": 2})).unwrap();
    // A change partway through a note's render (each render is a note's strikes, one after another).
    e.execute("song.transpose", &json!({"at": 96_000, "semitones": 5})).unwrap();
    let p = capture(&mut e, src, json!({"layers": 3, "hold": 0.5, "hold_high": 0.5}));
    check_samples("global transpose", &e, p);
    e.session_mut().transpose_track = vec![(0, 2)];
    check_keys("global transpose +2", &e, src, p, &keys, 2);

    let mut e = engine(48_000);
    let src = sine_track(&mut e);
    e.execute("event.midi_rtp", &json!({"tracks": [src.0], "transpose": 12, "velocity": -40, "delay": 240})).unwrap();
    let p = capture(&mut e, src, json!({"velocities": [40, 100]}));
    check_samples("real-time properties", &e, p);
    let s = e.session();
    let ins = s.track(p).unwrap().instrument.as_ref().unwrap();
    assert!(ins.samples.keys().all(|k| k.contains(".v40.") || k.contains(".v100.")));
}

/// Saved with its session and opened again (its samples then play from the session's files),
/// and that session converted to another rate: still in tune.
#[test]
fn a_captured_pantry_plays_in_tune_from_its_saved_session() {
    let dir = std::env::temp_dir().join(format!("sc-pantry-pitch-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Song.scraft").to_string_lossy().into_owned();
    let mut e = engine(48_000);
    let src = sine_track(&mut e);
    let p = capture(&mut e, src, json!({"layers": 2}));
    e.execute("session.save_as", &json!({"path": path})).unwrap();
    let mut e = Engine::default();
    e.execute("session.open", &json!({"path": path})).unwrap();
    assert_eq!(e.session().sample_rate.hz(), 48_000);
    let keys: Vec<u8> = (46..=74).collect();
    check_samples("reopened", &e, p);
    check_keys("reopened", &e, src, p, &keys, 0);
    e.execute("session.set_sample_rate", &json!({"rate": 44_100})).unwrap();
    check_keys("reopened, converted to 44.1 kHz", &e, src, p, &keys, 0);
    // Saved to the Library as SFZ and loaded into another Pantry; its files imported by name
    // (the note names capture and Save write: 60 is C3).
    let sfz = dir.join("Lib").join("Keys.sfz").to_string_lossy().into_owned();
    e.execute("mix.multisample_save", &json!({"track": p.0, "path": sfz})).unwrap();
    for (label, cmd, args) in [
        ("loaded from SFZ", "mix.multisample_load", json!({"path": sfz})),
        ("imported by name", "mix.multisample_import", json!({"folder": dir.join("Lib").join("Keys").to_string_lossy()})),
        ("session files imported by name", "mix.multisample_import", json!({"folder": dir.join("Audio Files").to_string_lossy()})),
    ] {
        let v = e.execute("track.new", &json!({"kind": "instrument", "instrument": MULTISAMPLER})).unwrap();
        let other = TrackId(v["tracks"][0].as_u64().unwrap());
        let mut args = args;
        args["track"] = json!(other.0);
        e.execute(cmd, &args).unwrap();
        check_keys(label, &e, src, other, &keys, 0);
    }
    let _ = std::fs::remove_dir_all(&dir);
}

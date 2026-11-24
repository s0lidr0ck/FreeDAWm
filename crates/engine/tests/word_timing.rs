//! Timing typed words on a vocal: Time Words to Vocal on synthetic sung lines with known word
//! boundaries (accuracy per kind of line, printed with `--nocapture`), taps with latency and
//! snapping, each hand edit and its undo, the neighbour-edge rules, typed-versus-transcribed
//! protection, comps, moved and duplicated clips, and the selection helpers.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, ClipId, LyricLine, LyricWord, Session, TrackId, TrackKind, WordTiming};
use soundcraft_time::Samples;
use std::f64::consts::TAU;

const RATE: u32 = 48_000;
const R: f64 = RATE as f64;

// ---- A synthetic singer ------------------------------------------------------------------------

/// Formant centres of a few vowels (Hz).
const VOWELS: [[f64; 3]; 4] = [[700.0, 1200.0, 2600.0], [300.0, 2300.0, 3000.0], [500.0, 900.0, 2400.0], [400.0, 1900.0, 2600.0]];

/// Builds a sung line sound by sound, remembering where each word starts (its first sound) and
/// ends (where its voice stops): the truth the aligner is measured against.
struct Singer {
    out: Vec<f32>,
    phase: f64,
    words: Vec<(String, f64, f64)>,
    open: Option<(String, f64)>,
    voiced_end: f64,
    seed: u32,
}

fn hz(midi: f64) -> f64 {
    440.0 * 2f64.powf((midi - 69.0) / 12.0)
}

impl Singer {
    fn new() -> Singer {
        Singer { out: Vec::new(), phase: 0.0, words: Vec::new(), open: None, voiced_end: 0.0, seed: 0x9E37_79B9 }
    }

    fn now(&self) -> f64 {
        self.out.len() as f64 / R
    }

    fn silence(&mut self, secs: f64) -> &mut Self {
        self.out.extend(std::iter::repeat_n(0.0, (secs * R) as usize));
        self
    }

    /// Unvoiced noise: a consonant (about -32 dB) or a breath (about -42 dB).
    fn noise(&mut self, secs: f64, db: f64) -> &mut Self {
        let g = 10f64.powf(db / 20.0) * 3f64.sqrt();
        for _ in 0..(secs * R) as usize {
            self.seed ^= self.seed << 13;
            self.seed ^= self.seed >> 17;
            self.seed ^= self.seed << 5;
            self.out.push(((f64::from(self.seed) / f64::from(u32::MAX) * 2.0 - 1.0) * g) as f32);
        }
        self
    }

    /// A consonant (50 ms of noise).
    fn cons(&mut self) -> &mut Self {
        self.noise(0.05, -32.0)
    }

    /// A word begins with the next sound.
    fn word(&mut self, text: &str) -> &mut Self {
        self.close();
        self.open = Some((text.to_string(), self.now()));
        self
    }

    fn close(&mut self) {
        if let Some((t, a)) = self.open.take() {
            self.words.push((t, a, self.voiced_end));
        }
    }

    /// Voiced sound over `notes` (MIDI pitch, seconds), legato between them (a 30 ms glide), on
    /// vowel `v`; `attack`/`release` fade in and out (off: it joins the sound before or after with
    /// no dip). `vib` adds 5.5 Hz vibrato of that many cents.
    fn sing(&mut self, notes: &[(f64, f64)], v: usize, attack: bool, release: bool, vib: f64) -> &mut Self {
        let total: f64 = notes.iter().map(|n| n.1).sum();
        let n = (total * R) as usize;
        let (att, rel) = ((0.012 * R) as usize, (0.02 * R) as usize);
        let mut bounds = Vec::new();
        let mut acc = 0.0;
        for (p, d) in notes {
            bounds.push((acc, *p));
            acc += d;
        }
        let pitch_at = |t: f64| -> f64 {
            let i = bounds.iter().rposition(|(s, _)| *s <= t).unwrap_or(0);
            let (s, p) = bounds[i];
            if i > 0 && t - s < 0.03 {
                let prev = bounds[i - 1].1;
                prev + (p - prev) * ((t - s) / 0.03)
            } else {
                p
            }
        };
        let f = VOWELS[v % VOWELS.len()];
        let mut amps: Vec<f64> = Vec::new();
        for i in 0..n {
            let t = i as f64 / R;
            let f0 = hz(pitch_at(t) + vib / 100.0 * (TAU * 5.5 * t).sin());
            if i % 64 == 0 {
                amps = (1..=30)
                    .map(|k| {
                        let fk = f0 * k as f64;
                        if fk > R / 2.0 {
                            return 0.0;
                        }
                        let res = |j: usize, a: f64| a / (1.0 + ((fk - f[j]) / (80.0 + 0.05 * f[j])).powi(2));
                        res(0, 1.0) + res(1, 0.6) + res(2, 0.3) + 0.02 / k as f64
                    })
                    .collect();
            }
            self.phase += TAU * f0 / R;
            let x: f64 = amps.iter().enumerate().map(|(k, a)| a * (self.phase * (k + 1) as f64).sin()).sum();
            let mut env = 1.0;
            if attack && i < att {
                env *= i as f64 / att as f64;
            }
            if release && n - i < rel {
                env *= (n - i) as f64 / rel as f64;
            }
            self.out.push((0.12 * env * x) as f32);
        }
        self.phase %= TAU;
        self.voiced_end = self.now();
        self
    }

    /// One syllable on one note, faded in and out.
    fn note(&mut self, midi: f64, secs: f64, v: usize) -> &mut Self {
        self.sing(&[(midi, secs)], v, true, true, 0.0)
    }

    fn finish(mut self) -> Fixture {
        self.close();
        self.silence(0.4);
        // A room's noise floor under everything (-58 dB).
        let n = self.out.len();
        let floor = {
            let mut f = Singer::new();
            f.seed = 0x1234_5677;
            f.noise(n as f64 / R + 0.01, -58.0);
            f.out
        };
        for (x, f) in self.out.iter_mut().zip(floor) {
            *x += f;
        }
        Fixture { audio: self.out, words: self.words }
    }
}

struct Fixture {
    audio: Vec<f32>,
    /// (word, start s, end s), the truth.
    words: Vec<(String, f64, f64)>,
}

/// Syllabic: one note per syllable, consonants between words.
fn syllabic() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("Lord").cons().note(62.0, 0.35, 2).silence(0.04);
    s.word("you").cons().note(64.0, 0.25, 1).silence(0.03);
    s.word("are").note(65.0, 0.3, 0).silence(0.05);
    s.word("my").cons().note(67.0, 0.3, 0).silence(0.03);
    s.word("shepherd").cons().note(65.0, 0.28, 3).cons().note(64.0, 0.45, 2);
    s.finish()
}

/// Melisma: a word spread over 3 to 6 legato notes.
fn melisma() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("Glory").cons().note(60.0, 0.3, 2).cons();
    s.sing(&[(64.0, 0.18), (65.0, 0.16), (67.0, 0.16), (65.0, 0.16), (64.0, 0.3)], 1, true, true, 0.0);
    s.silence(0.05);
    s.word("to").cons().note(62.0, 0.2, 1).silence(0.03);
    s.word("the").cons().note(62.0, 0.15, 3).silence(0.03);
    s.word("King").cons().sing(&[(64.0, 0.25), (62.0, 0.2), (60.0, 0.45)], 1, true, true, 0.0);
    s.finish()
}

/// Two words on one note (no break between them), then a two-note word.
fn two_on_one() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("I").sing(&[(64.0, 0.25)], 0, true, false, 0.0);
    s.word("am").sing(&[(64.0, 0.3)], 3, false, true, 0.0).silence(0.05);
    s.word("here").cons().note(67.0, 0.4, 1).silence(0.04);
    s.word("today").cons().note(65.0, 0.22, 3).cons().note(64.0, 0.5, 0);
    s.finish()
}

/// Pickup words: short words before a long downbeat word.
fn pickup() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("and").note(60.0, 0.1, 0).silence(0.02);
    s.word("the").cons().note(62.0, 0.09, 3).silence(0.02);
    s.word("light").cons().note(67.0, 0.8, 0).silence(0.05);
    s.word("shines").cons().note(65.0, 0.45, 0).silence(0.04);
    s.word("on").note(64.0, 0.5, 2);
    s.finish()
}

/// A held last word with vibrato.
fn held_last() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("hold").cons().note(62.0, 0.3, 2).silence(0.04);
    s.word("on").note(64.0, 0.3, 2).silence(0.04);
    s.word("forever").cons().note(65.0, 0.2, 2).cons().note(67.0, 0.2, 3).silence(0.02);
    s.sing(&[(69.0, 2.2)], 3, true, true, 40.0);
    s.finish()
}

/// A breath in the middle of the line.
fn breath() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("Amen").note(64.0, 0.25, 0).note(62.0, 0.35, 3).silence(0.04);
    s.word("we").cons().note(60.0, 0.25, 1).silence(0.04);
    s.word("say").cons().note(62.0, 0.5, 3);
    s.noise(0.4, -42.0);
    s.word("Amen").note(64.0, 0.25, 0).note(65.0, 0.35, 3).silence(0.04);
    s.word("we").cons().note(64.0, 0.25, 1).silence(0.04);
    s.word("pray").cons().note(62.0, 0.6, 3);
    s.finish()
}

/// A gospel "Amen": "A" run over five legato notes, "men" after a voiced m (a level dip, no
/// gap), then "Lord".
fn amen_run() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("Amen").sing(&[(64.0, 0.2), (67.0, 0.15), (69.0, 0.15), (67.0, 0.15), (64.0, 0.25)], 0, true, true, 15.0);
    s.sing(&[(62.0, 0.45)], 3, true, true, 15.0).silence(0.06);
    s.word("Lord").cons().note(60.0, 0.6, 2);
    s.finish()
}

/// Words joined legato with no consonant or gap between them, and a melisma among them: the
/// notes alone cannot say where one word ends and the next starts (the method's weak spot).
fn legato_run() -> Fixture {
    let mut s = Singer::new();
    s.silence(0.3);
    s.word("you").sing(&[(60.0, 0.3)], 1, true, false, 0.0);
    s.word("are").sing(&[(62.0, 0.3)], 0, false, false, 0.0);
    s.word("my").sing(&[(64.0, 0.2), (65.0, 0.2), (64.0, 0.2)], 0, false, false, 0.0);
    s.word("love").sing(&[(62.0, 0.3), (60.0, 0.5)], 2, false, true, 0.0);
    s.finish()
}

// ---- Sessions ----------------------------------------------------------------------------------

/// A "Lead Vox" track with `audio` as one clip at `at` (samples), the sheet holding `lines`.
fn session_with(audio: Vec<f32>, at: Samples, sheet: &str) -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Lead Vox"));
    let len = audio.len() as Samples;
    let src = soundcraft_engine::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: RATE, channels: vec![audio] }, None, FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "Vox", src, 0, at, len));
    let mut e = Engine::new(s);
    e.execute("lyrics.sheet_set", &json!({"text": sheet})).unwrap();
    (e, t, id)
}

fn secs(x: f64) -> Samples {
    (x * R).round() as Samples
}

/// The fixture's line placed loosely on the ruler (100 ms either side, as by hand), timed.
fn timed(f: &Fixture, at: Samples) -> (Engine, TrackId, Vec<(String, f64, f64)>) {
    let text: Vec<&str> = f.words.iter().map(|w| w.0.as_str()).collect();
    let (mut e, t, _) = session_with(f.audio.clone(), at, &text.join(" "));
    let (a, b) = (f.words.first().unwrap().1, f.words.last().unwrap().2);
    e.execute("lyrics.place", &json!({"number": 1, "start": at + secs(a - 0.1), "end": at + secs(b + 0.1)})).unwrap();
    let v = e.execute("lyrics.time_words", &json!({"track": t.0})).unwrap();
    assert_eq!(v["lines"][0]["words"].as_u64(), Some(f.words.len() as u64), "{v}");
    let got = words(&e, t)
        .iter()
        .map(|w| {
            (w["text"].as_str().unwrap().to_string(), (w["start"].as_i64().unwrap() - at) as f64 / R, (w["end"].as_i64().unwrap() - at) as f64 / R)
        })
        .collect();
    (e, t, got)
}

fn words(e: &Engine, t: TrackId) -> Vec<Value> {
    let mut e2 = Engine::new(e.session().clone());
    e2.execute("lyrics.words", &json!({"track": t.0})).unwrap()["words"].as_array().unwrap().clone()
}

/// Median, 90th percentile, share within 50 ms and within 120 ms of absolute errors (ms).
fn stats(errs: &[f64]) -> (f64, f64, f64, f64) {
    let mut v: Vec<f64> = errs.iter().map(|e| e.abs() * 1000.0).collect();
    v.sort_by(f64::total_cmp);
    let q = |p: f64| v[((v.len() - 1) as f64 * p).round() as usize];
    let within = |ms: f64| 100.0 * v.iter().filter(|e| **e <= ms).count() as f64 / v.len() as f64;
    (q(0.5), q(0.9), within(50.0), within(120.0))
}

#[test]
fn time_words_accuracy_on_synthetic_lines() {
    let at = secs(1.0);
    let mut report = String::from("kind         | starts: median  p90  <=50ms <=120ms | ends: median  p90  <=50ms <=120ms\n");
    let mut all_s = Vec::new();
    let mut all_e = Vec::new();
    for (name, f, (max_start_med, max_end_med)) in [
        ("syllabic", syllabic(), (30.0, 30.0)),
        ("melisma", melisma(), (40.0, 40.0)),
        ("two on one", two_on_one(), (60.0, 60.0)),
        ("pickup", pickup(), (40.0, 40.0)),
        ("held last", held_last(), (40.0, 40.0)),
        ("breath", breath(), (40.0, 40.0)),
        ("amen run", amen_run(), (40.0, 40.0)),
        // Known weak: words joined legato inside a melisma (measured, not hidden).
        ("legato run", legato_run(), (400.0, 400.0)),
    ] {
        let (e, t, got) = timed(&f, at);
        assert_eq!(got.iter().map(|g| g.0.clone()).collect::<Vec<_>>(), f.words.iter().map(|w| w.0.clone()).collect::<Vec<_>>());
        let se: Vec<f64> = got.iter().zip(&f.words).map(|(g, w)| g.1 - w.1).collect();
        let ee: Vec<f64> = got.iter().zip(&f.words).map(|(g, w)| g.2 - w.2).collect();
        let (sm, s9, s50, s120) = stats(&se);
        let (em, e9, e50, e120) = stats(&ee);
        report += &format!("{name:12} | {sm:6.0} {s9:6.0} {s50:5.0}% {s120:5.0}%  | {em:6.0} {e9:6.0} {e50:5.0}% {e120:5.0}%\n");
        if std::env::var("WORDTIME_DEBUG").is_ok() {
            for (g, w) in got.iter().zip(&f.words) {
                eprintln!("  {:10} got {:.3}-{:.3} truth {:.3}-{:.3}", g.0, g.1, g.2, w.1, w.2);
            }
            let (segs, breaths) = soundcraft_engine::word_time::track_segs(e.session(), t);
            for g in segs {
                let x = |v: i64| (v - at) as f64 / R;
                eprintln!("    seg {:.3}-{:.3} lead {:.3} {:?} gap {:.3}", x(g.start), x(g.end), x(g.lead), g.onset, g.gap_before as f64 / R);
            }
            eprintln!("    breaths {:?}", breaths.iter().map(|(a, b)| ((a - at) as f64 / R, (b - at) as f64 / R)).collect::<Vec<_>>());
        }
        assert!(sm <= max_start_med && em <= max_end_med, "{name}: start median {sm} ms, end median {em} ms\n{report}");
        // Never in a breath: no word starts inside an unvoiced stretch longer than a consonant.
        let (_, breaths) = soundcraft_engine::word_time::track_segs(e.session(), t);
        for g in &got {
            let s = at + secs(g.1);
            assert!(!breaths.iter().any(|(a, b)| s > *a + secs(0.01) && s < *b - secs(0.01)), "{name}: `{}` starts in a breath", g.0);
        }
        // All guesses.
        assert!(words(&e, t).iter().all(|w| w["timing"] == "guess"));
        all_s.extend(se);
        all_e.extend(ee);
    }
    let (sm, s9, s50, s120) = stats(&all_s);
    let (em, e9, e50, e120) = stats(&all_e);
    report += &format!("{:12} | {sm:6.0} {s9:6.0} {s50:5.0}% {s120:5.0}%  | {em:6.0} {e9:6.0} {e50:5.0}% {e120:5.0}%\n", "all");
    println!("{report}");
}

// ---- Hand edits on words without audio (a MIDI clip holds them) --------------------------------

/// A track with one MIDI clip at 0 holding one line of words at the given (start, end) ms.
fn hand_session(ws: &[(&str, i64, i64)]) -> (Engine, TrackId, ClipId) {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("Vox"));
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "Vox", 0, secs(20.0), Default::default());
    let words: Vec<LyricWord> = ws.iter().map(|(w, a, b)| LyricWord::new(secs(*a as f64 / 1000.0), secs(*b as f64 / 1000.0), *w)).collect();
    c.lyrics = vec![LyricLine { start: words[0].start, end: words.last().unwrap().end, words, sheet_line: None }];
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    (Engine::new(s), t, id)
}

fn ms(v: &Value) -> (i64, i64) {
    (v["start"].as_i64().unwrap() * 1000 / RATE as i64, v["end"].as_i64().unwrap() * 1000 / RATE as i64)
}

fn spans(e: &Engine, t: TrackId) -> Vec<(String, i64, i64)> {
    words(e, t).iter().map(|w| (w["text"].as_str().unwrap().to_string(), ms(w).0, ms(w).1)).collect()
}

fn r(clip: ClipId, word: usize) -> Value {
    json!({"clip": clip.0, "line": 0, "word": word})
}

fn with(mut a: Value, b: Value) -> Value {
    for (k, v) in b.as_object().unwrap() {
        a[k] = v.clone();
    }
    a
}

#[test]
fn edges_move_touching_neighbours_unless_alone_and_stop_at_others() {
    let (mut e, t, c) = hand_session(&[("Lord", 1000, 1500), ("what", 1500, 1800), ("ever", 2000, 2400)]);
    // Touching: dragging "what"'s start earlier pulls "Lord"'s end with it.
    let v = e.execute("lyrics.word_set", &with(r(c, 1), json!({"start": secs(1.4)}))).unwrap();
    assert_eq!(ms(&v), (1400, 1800));
    assert_eq!(spans(&e, t)[0], ("Lord".into(), 1000, 1400));
    // Not touching: "what"'s end stops at "ever"'s start.
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"end": secs(2.3)}))).unwrap();
    assert_eq!(spans(&e, t)[1], ("what".into(), 1400, 2000));
    assert_eq!(spans(&e, t)[2], ("ever".into(), 2000, 2400));
    // Now touching "ever": alone moves only this edge (a gap opens), without alone both move.
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"end": secs(1.9), "alone": true}))).unwrap();
    assert_eq!((spans(&e, t)[1].2, spans(&e, t)[2].1), (1900, 2000));
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"end": secs(1.95)}))).unwrap();
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"end": secs(2.0)}))).unwrap();
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"end": secs(2.2)}))).unwrap();
    assert_eq!((spans(&e, t)[1].2, spans(&e, t)[2].1), (2200, 2200), "touching: the neighbour follows");
    // A word keeps 20 ms and never crosses its neighbour.
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"start": secs(3.0)}))).unwrap();
    let w = &spans(&e, t)[1];
    assert_eq!((w.1, w.2), (2180, 2200));
    // Edited words are confirmed; untouched neighbours keep what they were.
    let ws = words(&e, t);
    assert_eq!(ws[1]["timing"], "confirmed");
    assert_eq!(ws[0]["timing"], "heard");
    // Each edit is one undo step.
    for _ in 0..7 {
        e.execute("edit.undo", &json!({})).unwrap();
    }
    assert_eq!(spans(&e, t), vec![("Lord".into(), 1000, 1500), ("what".into(), 1500, 1800), ("ever".into(), 2000, 2400)]);
}

#[test]
fn move_split_merge_retype_delete_insert_and_undo() {
    let (mut e, t, c) = hand_session(&[("Amen", 1000, 1600), ("Lord", 2000, 2500)]);
    // Move (nudge) the whole word: stops at the next word.
    e.execute("lyrics.word_move", &with(r(c, 0), json!({"by_ms": 300.0}))).unwrap();
    assert_eq!(spans(&e, t)[0], ("Amen".into(), 1300, 1900));
    e.execute("lyrics.word_move", &with(r(c, 0), json!({"by_ms": 500.0}))).unwrap();
    assert_eq!(spans(&e, t)[0], ("Amen".into(), 1400, 2000), "stops against Lord");
    // Split by syllable at the middle.
    let v = e.execute("lyrics.word_split", &r(c, 0)).unwrap();
    assert_eq!((ms(&v["first"]), ms(&v["second"])), ((1400, 1700), (1700, 2000)));
    assert_eq!(spans(&e, t).iter().map(|w| w.0.as_str()).collect::<Vec<_>>(), vec!["A-", "men", "Lord"]);
    // Merge back.
    e.execute("lyrics.word_merge", &r(c, 0)).unwrap();
    assert_eq!(spans(&e, t)[0], ("Amen".into(), 1400, 2000));
    // Retype.
    e.execute("lyrics.word_set", &with(r(c, 1), json!({"text": "Lord,"}))).unwrap();
    assert_eq!(spans(&e, t)[1].0, "Lord,");
    assert!(e.execute("lyrics.word_set", &with(r(c, 1), json!({"text": "two words"}))).is_err());
    // Insert at the playhead (into the gap), then delete it.
    e.session_mut().edit.playhead = secs(3.0);
    let v = e.execute("lyrics.word_insert", &json!({"track": t.0, "text": "yes"})).unwrap();
    assert_eq!(ms(&v), (3000, 3300));
    assert!(e.execute("lyrics.word_insert", &json!({"track": t.0, "text": "no", "at": secs(2.1)})).is_err(), "inside a word");
    e.execute("lyrics.word_delete", &json!({"clip": c.0, "line": v["line"], "word": v["word"]})).unwrap();
    assert_eq!(spans(&e, t).len(), 2);
    // Every step undoes on its own, back to the start.
    for _ in 0..7 {
        e.execute("edit.undo", &json!({})).unwrap();
    }
    assert_eq!(spans(&e, t), vec![("Amen".into(), 1000, 1600), ("Lord".into(), 2000, 2500)]);
    assert!(words(&e, t).iter().all(|w| w["timing"] == "heard"));
}

#[test]
fn split_text_by_syllable() {
    use soundcraft_engine::word_time::split_text;
    assert_eq!(split_text("Amen"), ("A-".into(), "men".into()));
    assert_eq!(split_text("glory"), ("glo-".into(), "ry".into()));
    assert_eq!(split_text("glo-ry"), ("glo-".into(), "ry".into()));
    assert_eq!(split_text("I"), ("I".into(), "-".into()));
}

#[test]
fn selection_helpers_pick_first_last_same_and_find() {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("Vox"));
    let id = s.new_clip_id();
    let mut c = Clip::midi(id, "Vox", 0, secs(30.0), Default::default());
    let line = |ws: &[(&str, f64)]| {
        let words: Vec<LyricWord> = ws.iter().map(|(w, a)| LyricWord::new(secs(*a), secs(a + 0.3), *w)).collect();
        LyricLine { start: words[0].start, end: words.last().unwrap().end, words, sheet_line: None }
    };
    c.lyrics = vec![
        line(&[("Amen,", 1.0), ("Amen", 2.0)]),
        line(&[("Lord", 4.0), ("whatever", 5.0), ("you", 6.0), ("say", 7.0)]),
        line(&[("I'll", 9.0), ("say", 10.0), ("Amen!", 11.0)]),
    ];
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    let mut e = Engine::new(s);
    let pick = |e: &mut Engine, which: &str, text: &str| -> Vec<String> {
        let v = e.execute("lyrics.select_words", &json!({"track": t.0, "which": which, "text": text})).unwrap();
        v["words"].as_array().unwrap().iter().map(|w| w["text"].as_str().unwrap().to_string()).collect()
    };
    assert_eq!(pick(&mut e, "first_of_line", ""), vec!["Amen,", "Lord", "I'll"]);
    assert_eq!(pick(&mut e, "last_of_line", ""), vec!["Amen", "say", "Amen!"]);
    assert_eq!(pick(&mut e, "same_word", "amen"), vec!["Amen,", "Amen", "Amen!"]);
    assert_eq!(pick(&mut e, "find", "you say"), vec!["you", "say"]);
    // The selection spans the matches and the playhead goes to the first.
    assert_eq!(e.session().edit.selection, soundcraft_time::Range::new(secs(6.0), secs(7.3)));
    assert_eq!(e.session().edit.playhead, secs(6.0));
    assert!(e.execute("lyrics.select_words", &json!({"track": t.0, "which": "odd"})).is_err());
    // Confirm all, then nothing is guessed.
    e.execute("lyrics.words_confirm", &json!({"track": t.0, "all": true})).unwrap();
    assert_eq!(e.execute("lyrics.words", &json!({"track": t.0})).unwrap()["confirmed"], 9);
}

// ---- With audio: taps, snapping, protection, comps, moves, duplicates ---------------------------

#[test]
fn taps_take_off_latency_and_snap_to_note_onsets() {
    let f = syllabic();
    let at = secs(1.0);
    let text: Vec<&str> = f.words.iter().map(|w| w.0.as_str()).collect();
    let (mut e, t, _) = session_with(f.audio.clone(), at, &text.join(" "));
    let line = e.session().lyric_sheet.line_numbered(1).unwrap().id;
    // Taps 80 ms late (the latency) and up to 40 ms off either way.
    let jitter = [30.0, -25.0, 40.0, -10.0, 20.0];
    let taps: Vec<Value> = f.words.iter().zip(jitter).map(|(w, j)| json!([at + secs(w.1 + 0.08 + j / 1000.0)])).collect();
    let v = e.execute("lyrics.tap_words", &json!({"track": t.0, "line": line, "taps": taps, "latency_ms": 80.0})).unwrap();
    assert_eq!(v["words"], 5, "{v}");
    let ws = words(&e, t);
    for (w, truth) in ws.iter().zip(&f.words) {
        let s = (w["start"].as_i64().unwrap() - at) as f64 / R;
        assert!((s - truth.1).abs() < 0.03, "{}: tapped {s:.3}, truth {:.3}", truth.0, truth.1);
        assert_eq!(w["timing"], "confirmed");
        assert_eq!(w["sheet_line"], line);
    }
    // Raw taps (snap off) keep the tap times.
    let v = e
        .execute("lyrics.tap_words", &json!({"track": t.0, "line": line, "taps": [[at + secs(0.5)], [at + secs(0.9), at + secs(1.0)]], "snap_ms": 0}))
        .unwrap();
    assert_eq!(v["words"], 2);
    let ws = words(&e, t);
    assert_eq!(ws.len(), 2, "tapping a line again replaces its words");
    assert_eq!(ws[0]["start"].as_i64(), Some(at + secs(0.5)));
    assert_eq!((ws[1]["start"].as_i64(), ws[1]["end"].as_i64()), (Some(at + secs(0.9)), Some(at + secs(1.0))), "held: key up is the end");
    e.execute("edit.undo", &json!({})).unwrap();
    assert_eq!(words(&e, t).len(), 5, "one undo step per tapped line");
}

#[test]
fn tap_line_starts_then_spread_words() {
    let f = breath();
    let at = secs(0.5);
    let (mut e, t, _) = session_with(f.audio.clone(), at, "Amen we say\nAmen we pray");
    let (l1, l2) = (e.session().lyric_sheet.line_numbered(1).unwrap().id, e.session().lyric_sheet.line_numbered(2).unwrap().id);
    let taps = json!([{"line": l1, "at": at + secs(f.words[0].1 + 0.02)}, {"line": l2, "at": at + secs(f.words[3].1 - 0.03)}]);
    let v = e.execute("lyrics.tap_lines", &json!({"track": t.0, "taps": taps})).unwrap();
    assert_eq!(v["guessed"], 6, "{v}");
    let ws = words(&e, t);
    for (w, truth) in ws.iter().zip(&f.words) {
        let s = (w["start"].as_i64().unwrap() - at) as f64 / R;
        assert!((s - truth.1).abs() < 0.06, "{}: {s:.3} vs {:.3}", truth.0, truth.1);
    }
    assert!(e.session().lyric_sheet.place_of(l2).is_some());
}

#[test]
fn snapping_hand_edits_to_notes() {
    let f = syllabic();
    let at = secs(1.0);
    let (mut e, t, c) = session_with(f.audio.clone(), at, "x");
    soundcraft_engine::word_time::analyse_track(&mut e, t);
    let truth_you = at + secs(f.words[1].1);
    {
        let s = e.session_mut();
        let clip = s.find_clip_mut(c).unwrap();
        clip.lyrics = vec![LyricLine {
            start: 0,
            end: secs(3.0),
            words: vec![LyricWord::new(secs(0.2), secs(0.5), "Lord"), LyricWord::new(secs(1.0), secs(1.3), "you")],
            sheet_line: None,
        }];
    }
    // Drop "you"'s start 35 ms off its consonant: snap brings it back.
    let v = e.execute("lyrics.word_set", &json!({"clip": c.0, "line": 0, "word": 1, "start": truth_you + secs(0.035), "snap": true})).unwrap();
    assert!((v["start"].as_i64().unwrap() - truth_you).abs() < secs(0.015), "{v} vs {truth_you}");
    // Without snap it stays where it was dropped.
    let v = e.execute("lyrics.word_set", &json!({"clip": c.0, "line": 0, "word": 1, "start": truth_you + secs(0.035)})).unwrap();
    assert_eq!(v["start"].as_i64(), Some(truth_you + secs(0.035)));
}

#[test]
fn transcription_must_ask_before_replacing_typed_words() {
    let f = syllabic();
    let (mut e, t, _) = timed_engine(&f);
    let err = soundcraft_engine::ml::prepare(&e, "ml.lyrics", &json!({"track": t.0})).err().unwrap().to_string();
    assert!(err.contains("typed or hand-timed"), "{err}");
    assert!(soundcraft_engine::ml::prepare(&e, "ml.lyrics", &json!({"track": t.0, "replace_typed": true})).is_ok());
    // A track with only heard words transcribes without asking.
    let n = soundcraft_model::track_hand_words(e.session().track(t).unwrap());
    assert_eq!(n, 5);
    e.execute("lyrics.clear", &json!({"track": t.0})).unwrap();
    assert!(soundcraft_engine::ml::prepare(&e, "ml.lyrics", &json!({"track": t.0})).is_ok());
}

fn timed_engine(f: &Fixture) -> (Engine, TrackId, Vec<(String, f64, f64)>) {
    timed(f, secs(1.0))
}

#[test]
fn timing_again_keeps_hand_work_unless_forced() {
    let f = pickup();
    let (mut e, t, _) = timed_engine(&f);
    let ws = words(&e, t);
    let light = ws.iter().position(|w| w["text"] == "light").unwrap();
    e.execute("lyrics.word_set", &json!({"clip": ws[light]["clip"], "line": ws[light]["line"], "word": ws[light]["word"], "text": "Light"})).unwrap();
    let v = e.execute("lyrics.time_words", &json!({"track": t.0})).unwrap();
    assert!(v["lines"][0]["skipped"].as_str().unwrap().contains("hand-timed"), "{v}");
    assert!(words(&e, t).iter().any(|w| w["text"] == "Light"));
    let v = e.execute("lyrics.time_words", &json!({"track": t.0, "force": true})).unwrap();
    assert_eq!(v["lines"][0]["words"], 5);
    assert!(words(&e, t).iter().all(|w| w["timing"] == "guess" && w["text"] != "Light"));
}

#[test]
fn words_follow_moved_clips_duplicates_and_comps() {
    let f = syllabic();
    let (mut e, t, _) = timed_engine(&f);
    let before: Vec<i64> = words(&e, t).iter().map(|w| w["start"].as_i64().unwrap()).collect();
    // Moving the clip moves its words (they live in its source time).
    let clip = e.session().track(t).unwrap().clips()[0].id;
    e.execute("edit.move_clips", &json!({"clips": [clip.0], "to": secs(3.0)})).unwrap();
    let moved = e.session().track(t).unwrap().clips()[0].start;
    let shift = moved - secs(1.0);
    assert_eq!(shift, secs(2.0));
    let after: Vec<i64> = words(&e, t).iter().map(|w| w["start"].as_i64().unwrap()).collect();
    assert_eq!(after, before.iter().map(|b| b + shift).collect::<Vec<_>>());
    // A duplicate track carries the same words at the same times, and is timed on its own.
    e.execute("track.duplicate", &json!({"tracks": [t.0]})).unwrap();
    let dup = e.session().tracks.iter().find(|x| x.id != t && x.kind == TrackKind::Audio).unwrap().id;
    assert_eq!(words(&e, dup).iter().map(|w| w["start"].as_i64().unwrap()).collect::<Vec<_>>(), after);
    // A comp: the playing playlist's pieces hold the words sung in them; another take's clips
    // (not playing) are left alone.
    let s = e.session_mut();
    let tr = s.track_mut(t).unwrap();
    let mut take = tr.playlists[0].clone();
    take.name = "Take 2".into();
    for c in &mut take.clips {
        c.lyrics.clear();
    }
    tr.playlists.push(take);
    let n = soundcraft_engine::word_time::track_words(e.session(), t).len();
    assert_eq!(n, 5);
    assert!(e.session().track(t).unwrap().playlists[1].clips.iter().all(|c| c.lyrics.is_empty()));
    let _ = WordTiming::Guess;
}

/// A real take, kept off the repo: `FREEDAWM_WORDTIME_SESSION` names a session whose sheet has
/// placed lines and whose vocal track (the first vocal track, or `FREEDAWM_WORDTIME_TRACK`) holds
/// the take; `FREEDAWM_WORDTIME_OUT` is where the word list goes. Reports what can be told without
/// an answer key. Run with `--ignored --nocapture`.
#[test]
#[ignore = "needs a private take named by FREEDAWM_WORDTIME_SESSION"]
fn real_take_report() {
    let Ok(path) = std::env::var("FREEDAWM_WORDTIME_SESSION") else { return };
    let mut e = Engine::default();
    soundcraft_engine::io::open_session(&mut e, &path).unwrap();
    let want = std::env::var("FREEDAWM_WORDTIME_TRACK").ok();
    let t = e
        .session()
        .tracks
        .iter()
        .find(|t| want.as_deref().map_or(t.is_vocal() && !t.clips().is_empty(), |w| t.name == w))
        .map(|t| t.id)
        .expect("a vocal track");
    let rate = e.session().sample_rate.as_f64();
    let t0 = std::time::Instant::now();
    let v = e.execute("lyrics.time_words", &json!({"track": t.0})).unwrap();
    let took = t0.elapsed().as_secs_f64();
    let s = e.session();
    let (segs, breaths) = soundcraft_engine::word_time::track_segs(s, t);
    let ws = soundcraft_engine::word_time::track_words(s, t);
    let mut out = String::new();
    let mut lens: Vec<f64> = Vec::new();
    let mut in_breath = 0;
    let m = |x: f64| format!("{}:{:06.3}", (x / 60.0) as u32, x % 60.0);
    for l in v["lines"].as_array().unwrap() {
        let id = l["line"].as_u64().unwrap();
        let text = s.lyric_sheet.lines.iter().find(|x| x.id == id).map(|x| x.text.clone()).unwrap_or_default();
        let n = s.lyric_sheet.number_of(id).unwrap_or(0);
        let skipped = l["skipped"].as_str().map(|w| format!(" (skipped: {w})")).unwrap_or_default();
        out += &format!("\nLine {n}: {text}\n  {} words placed on {} notes, {} sharing a note{skipped}\n", l["words"], l["notes"], l["shared"]);
        if std::env::var("WORDTIME_DEBUG").is_ok()
            && let Some(p) = s.lyric_sheet.place_of(id)
        {
            for g in segs.iter().filter(|g| g.start >= p.start - (0.2 * rate) as i64 && g.start < p.end) {
                out +=
                    &format!("    note {} - {} {:?} lead {}\n", m(g.start as f64 / rate), m(g.end as f64 / rate), g.onset, m(g.lead as f64 / rate));
            }
        }
        for w in ws.iter().filter(|w| w.sheet_line == Some(id)) {
            let (a, b) = (w.start as f64 / rate, w.end as f64 / rate);
            out += &format!("  {:>10} to {:>10}  ({:4.0} ms)  {}\n", m(a), m(b), (b - a) * 1000.0, w.text);
            lens.push((b - a) * 1000.0);
            if breaths.iter().any(|(x, y)| w.start > *x + (0.01 * rate) as i64 && w.start < *y - (0.01 * rate) as i64) {
                in_breath += 1;
            }
        }
    }
    lens.sort_by(f64::total_cmp);
    let q = |p: f64| lens.get(((lens.len().max(1) - 1) as f64 * p).round() as usize).copied().unwrap_or(0.0);
    let shared: u64 = v["lines"].as_array().unwrap().iter().map(|l| l["shared"].as_u64().unwrap_or(0)).sum();
    let name = s.track(t).map(|t| t.name.clone()).unwrap_or_default();
    let head = format!(
        "Words timed from the typed lines (first guess, no speech model)\ntrack: {name}\nlines: {}  words: {}  notes on the track: {}  breaths or long unvoiced stretches: {}\nwords starting inside a breath: {in_breath}\nwords that had to share a note: {shared}\nword length ms: shortest {:.0}, 10% {:.0}, median {:.0}, 90% {:.0}, longest {:.0}\ntook {took:.1} s (analysis included)\n",
        v["lines"].as_array().unwrap().len(),
        ws.len(),
        segs.len(),
        breaths.len(),
        q(0.0),
        q(0.1),
        q(0.5),
        q(0.9),
        q(1.0)
    );
    println!("{head}{out}");
    if let Ok(dest) = std::env::var("FREEDAWM_WORDTIME_OUT") {
        std::fs::write(dest, format!("{head}{out}")).unwrap();
    }
}

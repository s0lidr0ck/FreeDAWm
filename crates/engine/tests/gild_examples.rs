#![allow(clippy::field_reassign_with_default)]
//! Makes Gild's listening examples (not run by default): `GILD_EXAMPLES_DIR=<dir> cargo test -p
//! soundcraft-engine --test gild_examples -- --ignored --nocapture`. Writes WAVs of a synthetic
//! sung voice (dry, each effect, a throw on the last word of each line) and a session to take
//! screenshots from. With `GILD_TAKE=<wav>` and `GILD_TAKE_SESSION=<fdawm>` too (a take of your
//! own and the session holding its typed lyric, placed on the Lyrics ruler), it times the words
//! and gilds the take as well. Nothing is written into the repository.

use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, LyricLine, LyricWord, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples, TempoMap};
use std::f64::consts::TAU;
use std::path::Path;

const R: f64 = 48_000.0;

fn secs(x: f64) -> Samples {
    (x * R).round() as Samples
}

fn sing(out: &mut [f32], a: f64, b: f64, midi: f64, vowel: usize) {
    const V: [[f64; 2]; 4] = [[700.0, 1200.0], [300.0, 2300.0], [500.0, 900.0], [400.0, 1900.0]];
    let f = V[vowel % 4];
    let f0 = 440.0 * 2f64.powf((midi - 69.0) / 12.0);
    let (i0, i1) = (secs(a) as usize, (secs(b) as usize).min(out.len()));
    let mut ph = 0.0f64;
    for (k, o) in out.iter_mut().enumerate().take(i1).skip(i0) {
        let t = (k - i0) as f64 / R;
        let env = (t / 0.02).min(1.0) * ((b - a - t) / 0.04).clamp(0.0, 1.0);
        ph += TAU * f0 * (1.0 + 0.003 * (TAU * 5.3 * t).sin()) / R;
        let x: f64 = (1..=24)
            .map(|h| {
                let fk = f0 * h as f64;
                let res = |c: f64, w: f64| 1.0 / (1.0 + ((fk - c) / w).powi(2));
                (res(f[0], 110.0) + 0.6 * res(f[1], 180.0) + 0.02) * (ph * h as f64).sin()
            })
            .sum();
        *o = (0.12 * env * x) as f32;
    }
}

fn write_wav(dir: &Path, name: &str, ch: Vec<Vec<f32>>) {
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: true, bwf: None, bitrate_kbps: 0 };
    let bytes = soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: ch }, &opts).unwrap();
    std::fs::write(dir.join(name), bytes).unwrap();
    eprintln!("wrote {name}");
}

/// Three synthetic lines (text, start, end s, MIDI, vowel).
const LINES: [&[(&str, f64, f64, f64, usize)]; 3] = [
    &[("Lord", 0.5, 0.9, 62.0, 2), ("you", 1.0, 1.25, 64.0, 1), ("are", 1.3, 1.6, 65.0, 0), ("good", 1.7, 2.6, 67.0, 2)],
    &[("Amen", 4.5, 5.2, 64.0, 0), ("we", 5.3, 5.55, 62.0, 1), ("pray", 5.6, 6.6, 60.0, 3)],
    &[("Glory", 8.5, 9.1, 67.0, 2), ("to", 9.2, 9.4, 65.0, 1), ("the", 9.45, 9.6, 64.0, 3), ("King", 9.7, 10.8, 62.0, 1)],
];
const LEN: f64 = 14.0;

fn synthetic() -> (Engine, TrackId, soundcraft_model::ClipId) {
    let mut audio = vec![0.0f32; secs(LEN) as usize];
    for l in LINES {
        for (_, a, b, m, v) in l {
            sing(&mut audio, *a, *b, *m, *v);
        }
    }
    let mut s = Session::default();
    s.tempo = TempoMap::new(96.0, 4, 4).unwrap();
    s.edit.click = false;
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Lead Vox"));
    let src = soundcraft_engine::io::add_source(
        &mut s,
        "Synthetic voice",
        AudioBuffer { sample_rate: 48_000, channels: vec![audio.clone(), audio] },
        None,
        FileFormat::Wav,
    );
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "Synthetic voice", src, 0, 0, secs(LEN));
    c.lyrics = LINES
        .iter()
        .map(|l| {
            let words: Vec<LyricWord> = l.iter().map(|(w, a, b, _, _)| LyricWord::new(secs(*a), secs(*b), *w)).collect();
            LyricLine { start: words[0].start, end: words.last().unwrap().end, words, sheet_line: None }
        })
        .collect();
    soundcraft_engine::edit::place_clip(&mut s, t, c);
    (Engine::new(s), t, id)
}

fn refs(v: &Value) -> Vec<Value> {
    v["words"].as_array().unwrap().iter().map(|w| json!({"clip": w["clip"], "line": w["line"], "word": w["word"]})).collect()
}

fn render(e: &mut Engine, end: Samples) -> Vec<Vec<f32>> {
    e.execute("gild.render", &json!({})).unwrap();
    soundcraft_mix::render_range(e.session(), Range::new(0, end), 1024)
}

/// Gilds `word` with `fx`, or adds `fx` to the gilding already on it.
fn gild_word(e: &mut Engine, word: &Value, fx: &Value) {
    if e.execute("gild.add", &json!({"words": [word], "fx": fx})).is_ok() {
        return;
    }
    let (_, clip) = e.session().find_clip(soundcraft_model::ClipId(word["clip"].as_u64().unwrap())).unwrap();
    let line = word["line"].as_u64().unwrap() as usize;
    let w = &clip.lyrics[line].words[word["word"].as_u64().unwrap() as usize];
    let g = clip.gild.iter().find(|g| g.words.contains(&w.id) || (w.start < g.end && w.end > g.start)).unwrap().id;
    e.execute("gild.set", &json!({"clip": clip.id.0, "id": g, "fx": fx})).unwrap();
}

#[test]
#[ignore]
fn make_gild_examples() {
    let Ok(dir) = std::env::var("GILD_EXAMPLES_DIR") else { return };
    let dir = Path::new(&dir);
    std::fs::create_dir_all(dir).unwrap();
    let end = secs(LEN);
    let (e0, t, c) = synthetic();
    write_wav(dir, "synthetic-00-dry.wav", render(&mut Engine::new(e0.session().clone()), end));
    let word = |line: usize, w: usize| json!({"clip": c.0, "line": line, "word": w});
    let one = |name: &str, words: Vec<Value>, fx: Value| {
        let mut e = Engine::new(e0.session().clone());
        e.execute("gild.add", &json!({"words": words, "fx": fx})).unwrap();
        write_wav(dir, name, render(&mut e, end));
    };
    one("synthetic-01-delay-throw-on-good.wav", vec![word(0, 3)], json!({"delay": {"note": "1/4", "repeats": 0.45}}));
    one("synthetic-02-reverb-throw-on-good.wav", vec![word(0, 3)], json!({"reverb": {"space": "hall", "decay_s": 4.0}}));
    one("synthetic-03-double-on-glory.wav", vec![word(2, 0)], json!({"double": true}));
    one("synthetic-04-swell-into-amen.wav", vec![word(1, 0)], json!({"swell": {"length_s": 1.8}}));
    one("synthetic-05-shift-octave-below-layer-on-pray.wav", vec![word(1, 2)], json!({"shift": true}));
    one("synthetic-06-grit-on-king.wav", vec![word(2, 3)], json!({"grit": {"drive": 0.8, "blend": 0.8}}));
    one(
        "synthetic-07-harmony-third-and-fifth-on-king.wav",
        vec![word(2, 3)],
        json!({"harmony": {"voices": [{"steps": 2}, {"steps": 4, "pan": 0.4}]}}),
    );
    // A throw on the last word of each line.
    let mut e = Engine::new(e0.session().clone());
    let last = e.execute("lyrics.select_words", &json!({"track": t.0, "which": "last_of_line"})).unwrap();
    e.execute("gild.add", &json!({"words": refs(&last), "preset": "Last-word throw"})).unwrap();
    write_wav(dir, "synthetic-08-throw-on-last-word-of-each-line.wav", render(&mut e, end));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    write_wav(dir, "synthetic-09-same-dry-off-only-gild.wav", render(&mut e, end));
    e.execute("gild.track", &json!({"track": t.0, "dry": true})).unwrap();
    // A session for the screenshots.
    let sess = dir.join("screens").join("Gild demo.fdawm");
    std::fs::create_dir_all(sess.parent().unwrap()).unwrap();
    e.execute("gild.add", &json!({"words": [word(1, 0)], "fx": {"swell": true, "double": true}})).unwrap();
    soundcraft_engine::io::save_session(&mut e, sess.to_str().unwrap()).unwrap();
    eprintln!("session {}", sess.display());

    // The owner's take, when given.
    let (Ok(take), Ok(sp)) = (std::env::var("GILD_TAKE"), std::env::var("GILD_TAKE_SESSION")) else { return };
    let text = std::fs::read_to_string(&sp).unwrap();
    let theirs: Session = serde_json::from_str(&text).unwrap();
    let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&take).unwrap(), Some("wav")).unwrap();
    let frames = buf.channels.first().map_or(0, Vec::len) as Samples;
    let mut s = Session::new("Gild on the take", theirs.sample_rate);
    s.tempo = theirs.tempo.clone();
    s.key_signatures = theirs.key_signatures.clone();
    s.lyric_sheet = theirs.lyric_sheet.clone();
    s.edit.click = false;
    let fmt = if buf.channels.len() > 1 { ChannelFormat::Stereo } else { ChannelFormat::Mono };
    let t = s.add_track(TrackKind::Audio, fmt, Some("Vocal"));
    let src = soundcraft_engine::io::add_source(&mut s, "take", buf, None, FileFormat::Wav);
    let id = s.new_clip_id();
    // As the take sits in the session: its first 374 samples trimmed off, starting at 0.
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(id, "take", src, 374, 0, frames - 374));
    let mut e = Engine::new(s);
    let timed = e.execute("lyrics.time_words", &json!({"track": t.0})).unwrap();
    eprintln!("timed {} words ({} lines)", timed["words"], timed["lines"].as_array().map_or(0, Vec::len));
    let end = frames + secs(6.0);
    write_wav(dir, "take-00-dry.wav", render(&mut e, end));
    // A double on the third line, a swell into the first "Amen", a throw on every line's last word.
    let words = e.execute("lyrics.words", &json!({"track": t.0})).unwrap();
    let all = words["words"].as_array().unwrap().clone();
    let mut lines: Vec<(u64, u64)> = Vec::new();
    for w in &all {
        let k = (w["clip"].as_u64().unwrap(), w["line"].as_u64().unwrap());
        if !lines.contains(&k) {
            lines.push(k);
        }
    }
    let mut notes = Vec::new();
    if let Some(k) = lines.get(2) {
        let third: Vec<Value> = all
            .iter()
            .filter(|w| (w["clip"].as_u64().unwrap(), w["line"].as_u64().unwrap()) == *k)
            .map(|w| json!({"clip": w["clip"], "line": w["line"], "word": w["word"]}))
            .collect();
        let text: Vec<&str> =
            all.iter().filter(|w| (w["clip"].as_u64().unwrap(), w["line"].as_u64().unwrap()) == *k).map(|w| w["text"].as_str().unwrap()).collect();
        e.execute("gild.add", &json!({"words": third, "fx": {"double": true}})).unwrap();
        notes.push(format!("double on line 3: \"{}\"", text.join(" ")));
    }
    let amen = e.execute("lyrics.select_words", &json!({"track": t.0, "which": "same_word", "text": "amen"})).unwrap();
    // The first "Amen" with room for its swell before it (the very first starts at the top of
    // the song, where a swell would fall before time zero and not be heard).
    let aw = amen["words"].as_array().unwrap();
    if let Some(first) = aw.iter().find(|w| w["start"].as_i64().unwrap_or(0) >= secs(2.2)) {
        let r = json!({"clip": first["clip"], "line": first["line"], "word": first["word"]});
        gild_word(&mut e, &r, &json!({"swell": {"length_s": 2.0}}));
        notes.push(format!("swell into the first \"Amen\" with room before it, at {:.2} s", first["start"].as_f64().unwrap_or(0.0) / R));
    }
    let last = e.execute("lyrics.select_words", &json!({"track": t.0, "which": "last_of_line"})).unwrap();
    let throw = soundcraft_engine::gild_presets::find("Last-word throw").unwrap().fx;
    let throw = serde_json::to_value(&throw).unwrap();
    for w in refs(&last) {
        gild_word(&mut e, &w, &throw);
    }
    notes.push(format!("Last-word throw on {} last words", refs(&last).len()));
    write_wav(dir, "take-01-gilded.wav", render(&mut e, end));
    e.execute("gild.track", &json!({"track": t.0, "dry": false})).unwrap();
    write_wav(dir, "take-02-gild-only-dry-off.wav", render(&mut e, end));
    let list = e.execute("gild.list", &json!({"track": t.0})).unwrap();
    let summary: Vec<String> = list["gildings"]
        .as_array()
        .unwrap()
        .iter()
        .map(|g| format!("{:7.2} s  {:<28} {}", g["seconds"].as_f64().unwrap_or(0.0), g["text"].as_str().unwrap_or(""), g["kinds"]))
        .collect();
    std::fs::write(dir.join("take-gildings.txt"), format!("{}\n\n{}\n", notes.join("\n"), summary.join("\n"))).unwrap();
}

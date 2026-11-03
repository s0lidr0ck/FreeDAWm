//! A bounce of an instrument track must come out at the level playback plays it: the offline
//! render (File › Bounce Mix) and the realtime mix engine run the same code, so their RMS over
//! the same span must agree within 0.5 dB.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use serde_json::json;
use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_engine::io::Bounce;
use soundcraft_midi::{Note, Sequence};
use soundcraft_mix::MixEngine;
use soundcraft_model::{ChannelFormat, Clip, Insert, Session, TrackKind};
use soundcraft_time::Range;
use std::path::{Path, PathBuf};

fn scratch(name: &str) -> PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// The voicing Convert to MIDI (piano) gives the subtractive synth (`soundcraft_engine::ml`).
const KEYS_VOICE: [(&str, f32); 12] = [
    ("osc1_wave", 3.0),
    ("osc2_wave", 2.0),
    ("osc2_semitones", 12.0),
    ("osc2_detune", 0.0),
    ("osc_mix", 30.0),
    ("cutoff", 3500.0),
    ("filter_env_amount", 35.0),
    ("amp_attack", 2.0),
    ("amp_decay", 2500.0),
    ("amp_sustain", 0.0),
    ("amp_release", 450.0),
    ("velocity", 90.0),
];

/// One instrument track (the synth voiced as keys) playing a few chords of varied velocity.
fn piano_session() -> Session {
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, Some("Piano MIDI"));
    let q = soundcraft_time::TICKS_PER_QUARTER;
    let mut seq = Sequence::default();
    for (i, (pitch, vel)) in [(60u8, 100u8), (64, 80), (67, 110), (72, 60), (65, 90), (69, 70)].into_iter().enumerate() {
        seq.notes.push(Note { pitch, velocity: vel, release_velocity: 64, channel: 0, start: i as i64 * q, length: q * 3 / 2 });
        seq.notes.push(Note { pitch: pitch - 12, velocity: vel, release_velocity: 64, channel: 0, start: i as i64 * q, length: q });
    }
    let cid = s.new_clip_id();
    let len = s.sample_rate.samples(5.0);
    let tr = s.track_mut(t).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::midi(cid, "piano", 0, len, seq));
    let mut ins = Insert::new("subtractive_synth");
    if let Some(info) = soundcraft_dsp::plugin_info("subtractive_synth") {
        for pi in info.params {
            ins.params.insert(pi.id.to_string(), pi.default);
        }
    }
    for (k, v) in KEYS_VOICE {
        ins.params.insert(k.to_string(), v);
    }
    ins.preset = "Keys".into();
    tr.instrument = Some(ins);
    s
}

fn rms(ch: &[Vec<f32>]) -> f32 {
    let n: usize = ch.iter().map(Vec::len).sum();
    (ch.iter().flatten().map(|x| x * x).sum::<f32>() / n.max(1) as f32).sqrt()
}

fn db(x: f32) -> f32 {
    20.0 * x.max(1e-12).log10()
}

/// Renders `range` the way the realtime player does: external instances, metronome on (the
/// click is off in the session), a Play at the range start (`reset_at`), device-sized blocks.
fn live(s: &Session, range: Range, block: usize) -> Vec<Vec<f32>> {
    let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, block);
    eng.set_external_instances(true);
    eng.metronome = true;
    eng.ensure_synced(s);
    eng.reset_at(s, range.start);
    let len = usize::try_from(range.len()).unwrap();
    let nch = soundcraft_mix::main_channels(s);
    let mut out = vec![vec![0.0f32; len]; nch];
    let mut tmp = vec![vec![0.0f32; block]; nch];
    let mut done = 0usize;
    while done < len {
        let n = (len - done).min(block);
        eng.render(s, range.start + done as i64, n, &mut tmp);
        for (o, t) in out.iter_mut().zip(tmp.iter()) {
            o[done..done + n].copy_from_slice(&t[..n]);
        }
        done += n;
    }
    // Playback has the mix's latency in front; the bounce removes it. Drop it here too.
    let lat = eng.latency;
    out.iter_mut().for_each(|c| {
        c.drain(..lat.min(len));
    });
    out
}

fn bounce(e: &Engine, dir: &Path, name: &str, range: Range) -> Vec<Vec<f32>> {
    let path = dir.join(name);
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    Bounce::new(e.session_arc(), &path, range, &opts, None, false, false).unwrap().run().unwrap();
    let bytes = std::fs::read(&path).unwrap();
    let (_, file) = soundcraft_audio_io::decode(&bytes, None).unwrap();
    file.channels
}

fn assert_same_level(live: &[Vec<f32>], bounced: &[Vec<f32>], what: &str) {
    let n = live[0].len().min(bounced[0].len());
    let l: Vec<Vec<f32>> = live.iter().map(|c| c[..n].to_vec()).collect();
    let b: Vec<Vec<f32>> = bounced.iter().map(|c| c[..n].to_vec()).collect();
    let (ld, bd) = (db(rms(&l)), db(rms(&b)));
    assert!(ld > -40.0, "{what}: live is silent ({ld} dBFS)");
    assert!((ld - bd).abs() < 0.5, "{what}: live {ld:.2} dBFS RMS vs bounce {bd:.2} dBFS RMS (diff {:.2} dB)", ld - bd);
}

#[test]
fn an_instrument_track_bounces_at_its_playback_level() {
    let s = piano_session();
    let range = Range::new(0, s.sample_rate.samples(5.0));
    let e = Engine::new(s.clone());
    let dir = scratch("bounce-level");
    let bounced = bounce(&e, &dir, "piano.wav", range);
    for block in [128usize, 480, 1024] {
        let played = live(e.session(), range, block);
        assert_same_level(&played, &bounced, &format!("piano, live block {block}"));
    }
    // The command path (what File › Bounce Mix runs).
    let mut e = Engine::new(s);
    let path = dir.join("cmd.wav");
    e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "start": 0, "end": range.end, "bit_depth": "32f", "dither": false}))
        .unwrap();
    let (_, file) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
    let played = live(e.session(), range, 480);
    assert_same_level(&played, &file.channels, "piano, file.bounce_mix");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn the_demo_keys_bounce_at_their_playback_level() {
    let e = soundcraft_engine::demo::demo_engine();
    let s = e.session();
    let keys = s.tracks.iter().find(|t| t.name == "Keys").unwrap();
    let from = keys.clips().iter().map(|c| c.start).min().unwrap();
    let range = Range::new(from, from + s.sample_rate.samples(4.0));
    // Solo the Keys so only the instrument is measured.
    let mut s2 = s.clone();
    for t in &mut s2.tracks {
        t.mixer.solo = t.name == "Keys";
    }
    let e = Engine::new(s2);
    let dir = scratch("bounce-level-demo");
    let bounced = bounce(&e, &dir, "keys.wav", range);
    let played = live(e.session(), range, 480);
    assert_same_level(&played, &bounced, "demo Keys");
    let _ = std::fs::remove_dir_all(&dir);
}

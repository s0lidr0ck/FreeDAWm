//! Detect Tempo on a real recording (local only): set `FREEDAWM_TEMPO_AUDIO` to an audio file.
//! Prints the average BPM and how steady the per-bar tempo is; skipped when the file is missing
//! (`FIDELITY_STRICT=1` makes that a failure).

use serde_json::json;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};

#[test]
fn detects_a_stable_tempo_in_a_real_recording() {
    let path = std::env::var_os("FREEDAWM_TEMPO_AUDIO").map(std::path::PathBuf::from);
    let Some(bytes) = path.as_ref().and_then(|p| std::fs::read(p).ok()) else {
        assert!(std::env::var_os("FIDELITY_STRICT").is_none(), "FREEDAWM_TEMPO_AUDIO is not set or unreadable");
        eprintln!("SKIPPED: set FREEDAWM_TEMPO_AUDIO to an audio file");
        return;
    };
    let ext = path.as_ref().and_then(|p| p.extension()).and_then(|e| e.to_str()).map(str::to_ascii_lowercase);
    let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).expect("decodes");
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Song"));
    let len = buf.channels.first().map_or(0, Vec::len) as i64;
    let file_rate = i64::from(buf.sample_rate.max(1));
    let src = soundcraft_engine::io::add_source(&mut s, "song", buf, None, soundcraft_audio_io::FileFormat::Wav);
    let len = len * i64::from(s.sample_rate.hz()) / file_rate;
    let cid = s.new_clip_id();
    soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(cid, "song", src, 0, 0, len.max(1)));
    let mut e = Engine::new(s);
    let started = std::time::Instant::now();
    let r = e.execute("event.tempo_detect", &json!({"source": "session"})).expect("detects");
    let bpm = r["bpm"].as_f64().unwrap_or(0.0);
    let bars: Vec<f64> = r["bars"].as_array().map(|a| a.iter().filter_map(serde_json::Value::as_f64).collect()).unwrap_or_default();
    let n = bars.len().max(1) as f64;
    let mean = bars.iter().sum::<f64>() / n;
    let sd = (bars.iter().map(|b| (b - mean).powi(2)).sum::<f64>() / n).sqrt();
    let mut sorted = bars.clone();
    sorted.sort_by(f64::total_cmp);
    let pct = |q: f64| sorted.get(((sorted.len() as f64 - 1.0) * q).round() as usize).copied().unwrap_or(0.0);
    let within = |d: f64| bars.iter().filter(|b| (**b - mean).abs() <= d).count() as f64 / n * 100.0;
    eprintln!(
        "tempo: {bpm:.2} BPM average; {} beats, {} bars; per-bar mean {mean:.2}, sd {sd:.2}, p5 {:.2}, median {:.2}, p95 {:.2}, min {:.2}, max {:.2}; {:.0}% of bars within ±1 BPM, {:.0}% within ±2; {:.1} s",
        r["beats"].as_array().map_or(0, Vec::len),
        bars.len(),
        pct(0.05),
        pct(0.5),
        pct(0.95),
        sorted.first().copied().unwrap_or(0.0),
        sorted.last().copied().unwrap_or(0.0),
        within(1.0),
        within(2.0),
        started.elapsed().as_secs_f64()
    );
    assert!((50.0..=220.0).contains(&bpm), "implausible tempo {bpm}");
    assert!(bars.len() >= 8, "only {} bars", bars.len());
    assert!(within(4.0) >= 80.0, "per-bar tempo is not stable");
}

//! The built-in Sampler's sample: `mix.sampler_load` decodes an audio file (WAV, AIFF, FLAC, MP3,
//! OGG, anything `file.import_audio` reads) into a session source, resampled to the session rate,
//! and makes it the sample an instrument track's Sampler plays; `mix.sampler_clear` removes it.
//! The source is saved in `Audio Files/` with the session like any other audio, so a session that
//! uses a sample reopens with it. Sampler parameters (root note, key range, ADSR, loop…) are set
//! with `mix.instrument_param`, or all at once with `params` here.
//!
//! `mix.sampler_from_selection` takes the sample from the session instead: the selected range of
//! an audio track, rendered as it plays (clip gain and fades included, the track's plugins not).
//! `mix.sampler_detect` reports the pitch the sample sounds at, for its root note.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Insert, TrackKind};
use std::path::Path;

const SAMPLER: &str = "sampler";

/// Largest sample accepted (frames × channels): ten minutes of 192 kHz stereo.
const MAX_SAMPLE_VALUES: usize = 192_000 * 600 * 2;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.sampler_load",
            "Load Sampler Sample",
            [],
            None,
            "{track?, path, params?: {root_note, key_low, key_high, attack, loop_mode, …}}: an instrument track plays the file through the built-in Sampler (made its instrument if it is not already) → {track, source, name, frames, channels, sample_rate}",
            has_selection,
            load
        ),
        cmd!(
            "mix.sampler_from_selection",
            "Sampler Sample from Selection",
            [],
            None,
            "{track?, from?, start?, end?, params?}: the range (default the edit selection) of audio track `from` (default the first selected audio or aux track), rendered as its clips play, becomes the sample of instrument track `track`'s Sampler (default the first selected instrument track) → {track, source, name, frames, channels, sample_rate}",
            has_selection,
            from_selection
        ),
        cmd!(
            "mix.sampler_detect",
            "Detect Sampler Root Note",
            [],
            None,
            "{track?}: the pitch the track's Sampler sample sounds at → {track, root: MIDI note (fractional) or null, note, cents, name}; null for drums, noise and silence",
            has_selection,
            detect
        ),
        cmd!("mix.sampler_clear", "Clear Sampler Sample", [], None, "{track?}: the track's Sampler stops playing its sample", has_selection, clear),
    ]
}

/// The instrument track a sampler command acts on: `track`/`tracks`, else the first selected
/// instrument track.
fn instrument_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let named = p.get("track").is_some() || p.get("tracks").is_some();
    let ts = tracks_required(e, id, p)?;
    let s = e.session();
    let is_inst = |t: &TrackId| s.track(*t).map(|tr| tr.kind) == Some(TrackKind::Instrument);
    let t = if named { ts.first().copied() } else { ts.iter().copied().find(is_inst).or_else(|| ts.first().copied()) };
    let t = t.ok_or_else(|| bad(id, "no track"))?;
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad(id, "only instrument tracks have a sampler"));
    }
    Ok(t)
}

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_load";
    let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let t = instrument_track(e, id, p)?;
    let values = param_values(id, p)?;
    let bytes = crate::io::read_file(&path)?;
    let name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
    attach(e, t, &name, &bytes, Some(&path), values)
}

/// Sampler parameters passed in `params`, validated and clamped, before anything changes.
fn param_values(id: &str, p: &Value) -> Result<Vec<(String, f32)>> {
    let info = soundcraft_dsp::plugin_info(SAMPLER).ok_or_else(|| bad(id, "the sampler is not available"))?;
    let mut values = Vec::new();
    if let Some(v) = p.get("params") {
        let obj = v.as_object().ok_or_else(|| bad(id, "`params` must be an object"))?;
        for (k, v) in obj {
            let pi = info.param(k).ok_or_else(|| bad(id, format!("the sampler has no parameter `{k}`")))?;
            let x = v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(id, format!("`{k}` must be a number")))?;
            values.push((k.clone(), pi.clamp(x as f32)));
        }
    }
    Ok(values)
}

fn from_selection(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_from_selection";
    let t = instrument_track(e, id, p)?;
    let values = param_values(id, p)?;
    let s = e.session();
    let audio_kind = |k: TrackKind| k.has_playlist() && !k.is_midi();
    let from = match track_param(e, id, p, "from")? {
        Some(f) => f,
        None => {
            let sel = e.target_tracks();
            sel.into_iter()
                .find(|x| s.track(*x).is_some_and(|tr| audio_kind(tr.kind)))
                .ok_or_else(|| bad(id, "select a range on an audio track, or pass `from`"))?
        }
    };
    let tr = s.track(from).ok_or_else(|| bad(id, "no source track"))?;
    if !audio_kind(tr.kind) {
        return Err(bad(id, format!("`{}` has no audio clips to sample", tr.name)));
    }
    let r = range_param(e, id, p)?;
    let start = r.start.max(0);
    let len = r.end.saturating_sub(start);
    if len <= 0 {
        return Err(bad(id, "the selection is empty: select a range to sample"));
    }
    let max = MAX_SAMPLE_VALUES / tr.channels().max(1);
    crate::io::refuse_longer(s, id, &tr.name, "sample", len, max.min(crate::io::render_limit(tr.channels())))?;
    let r = soundcraft_time::Range::new(start, start + len);
    let audio = soundcraft_mix::render_clips(s, from, r);
    if audio.iter().all(|ch| ch.iter().all(|x| *x == 0.0)) {
        return Err(bad(id, format!("the selection on `{}` is silent", tr.name)));
    }
    let name = format!("{} sample", tr.name);
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    attach_buffer(e, t, &name, buf, None, soundcraft_audio_io::FileFormat::Wav, values)
}

fn detect(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_detect";
    let t = instrument_track(e, id, p)?;
    let s = e.session();
    let ins = s.track(t).and_then(|tr| tr.instrument.as_ref()).filter(|i| i.plugin == SAMPLER);
    let ins = ins.ok_or_else(|| bad(id, "the track's instrument is not the sampler"))?;
    let src = ins.sample.ok_or_else(|| bad(id, "the sampler has no sample"))?;
    let audio = s.pool.get(src).ok_or_else(|| bad(id, "the sample's audio is missing"))?;
    let root = sample_root(audio);
    let (note, cents) = root.map_or((Value::Null, Value::Null), |m| {
        let n = m.round();
        (json!(n as u8), json!(((m - n) * 100.0).round()))
    });
    let name = root.map(|m| soundcraft_midi::note_name(m.round().clamp(0.0, 127.0) as u8));
    Ok(json!({"track": t, "root": root, "note": note, "cents": cents, "name": name}))
}

/// The pitch a source sounds at (a fractional MIDI note), from the first seconds of its first
/// channel (or its two channels mixed).
pub fn sample_root(audio: &soundcraft_model::SourceAudio) -> Option<f32> {
    let rate = audio.sample_rate();
    let n = audio.frames().min(rate as usize * 4);
    let w = audio.window(0, n);
    let mono: Vec<f32> = match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect(),
        (Some(a), None) => a.clone(),
        _ => return None,
    };
    soundcraft_dsp::pitch_detect::sample_root(&mono, rate as f32)
}

/// Decodes `bytes` (a file called `name`, for the format hint) into a new session source and
/// makes it the sample of track `t`'s Sampler, which becomes its instrument if it is not
/// already; `values` are sampler parameters to set. Fails before touching the session.
pub(crate) fn attach(e: &mut Engine, t: TrackId, name: &str, bytes: &[u8], path: Option<&str>, values: Vec<(String, f32)>) -> Result<Value> {
    let id = "mix.sampler_load";
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad(id, "only instrument tracks have a sampler"));
    }
    let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (format, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    attach_buffer(e, t, name, buf, path, format.format, values)
}

/// Makes decoded audio the sample of track `t`'s Sampler (see [`attach`]).
fn attach_buffer(
    e: &mut Engine,
    t: TrackId,
    name: &str,
    buf: soundcraft_audio_io::AudioBuffer,
    path: Option<&str>,
    format: soundcraft_audio_io::FileFormat,
    values: Vec<(String, f32)>,
) -> Result<Value> {
    let id = "mix.sampler_load";
    if e.session().track(t).map(|tr| tr.kind) != Some(TrackKind::Instrument) {
        return Err(bad(id, "only instrument tracks have a sampler"));
    }
    let info = soundcraft_dsp::plugin_info(SAMPLER).ok_or_else(|| bad(id, "the sampler is not available"))?;
    if buf.frames() == 0 || buf.num_channels() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    let buf = crate::io::at_rate(buf, e.session().sample_rate.hz(), MAX_SAMPLE_VALUES)
        .ok_or_else(|| EngineError::Io(format!("{name}: too long for a sample (ten minutes of 192 kHz stereo at most)")))?;
    let channels = buf.num_channels();
    let s = e.session_mut();
    let src = crate::io::add_source(s, name, buf, path, format);
    let (frames, rate, stem) = s.source(src).map_or((0, 0, String::new()), |x| (x.frames, x.sample_rate, x.name.clone()));
    let tr = s.track_mut(t).ok_or_else(|| bad(id, "no track"))?;
    let keep = tr.instrument.as_ref().is_some_and(|i| i.plugin == SAMPLER);
    if !keep {
        let mut ins = Insert::new(SAMPLER);
        for pi in info.params {
            ins.params.insert(pi.id.to_string(), pi.default);
        }
        tr.instrument = Some(ins);
    }
    if let Some(ins) = tr.instrument.as_mut() {
        ins.sample = Some(src);
        for (k, v) in values {
            ins.params.insert(k, v);
        }
    }
    Ok(json!({"track": t, "source": src, "name": stem, "frames": frames, "channels": channels, "sample_rate": rate}))
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.sampler_clear";
    let t = instrument_track(e, id, p)?;
    let ins = e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).filter(|i| i.plugin == SAMPLER);
    let ins = ins.ok_or_else(|| bad(id, "the track's instrument is not the sampler"))?;
    let had = ins.sample.take();
    Ok(json!({"track": t, "cleared": had}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{Clip, TrackId};
    use soundcraft_time::{Range, TICKS_PER_QUARTER};
    use std::path::PathBuf;

    /// A scratch folder for one test.
    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-sampler-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Writes a generated tone (no recorded or third-party audio) as a 16-bit WAV.
    fn tone_wav(path: &std::path::Path, freq: f32, rate: u32, secs: f32, channels: usize) {
        let n = (rate as f32 * secs) as usize;
        let one: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * freq * i as f32 / rate as f32).sin()).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![one; channels] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
        std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    }

    fn instrument_track(e: &mut Engine) -> TrackId {
        e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
        e.session().tracks.last().unwrap().id
    }

    /// One MIDI note (pitch, start in quarter notes, length in quarters) on the track at 120 bpm.
    fn add_notes(e: &mut Engine, t: TrackId, notes: &[(u8, i64, i64)]) {
        let q = TICKS_PER_QUARTER;
        let mut seq = Sequence::default();
        for &(pitch, start, len) in notes {
            seq.notes.push(Note { pitch, velocity: 127, release_velocity: 64, channel: 0, start: start * q, length: len * q });
        }
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 8, seq));
    }

    fn render(e: &Engine, secs: i64) -> Vec<Vec<f32>> {
        soundcraft_mix::render_range(e.session(), Range::new(0, 48_000 * secs), 512)
    }

    fn peak(x: &[f32]) -> f32 {
        x.iter().fold(0.0f32, |m, v| m.max(v.abs()))
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn loads_a_sample_plays_it_and_undoes() {
        let d = dir("play");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 44_100, 2.0, 1);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        add_notes(&mut e, t, &[(60, 0, 1), (72, 2, 1)]);
        assert!(peak(&render(&e, 2)[0]) > 0.0, "the default synth plays the notes first");
        let r = e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy()})).unwrap();
        assert_eq!(r["frames"], 96_000, "resampled to the 48 kHz session: {r}");
        assert_eq!(r["sample_rate"], 48_000);
        assert_eq!(r["channels"], 1);
        assert_eq!(r["name"], "tone");
        let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(ins.plugin, "sampler");
        assert_eq!(ins.sample.map(|s| s.0), r["source"].as_u64());
        let out = render(&e, 2);
        // Middle C plays the tone at its pitch for the first quarter (0.5 s), key 72 an octave up.
        let first = &out[0][2400..21_600];
        let second = &out[0][50_400..69_600];
        assert!(peak(first) > 0.05, "the sample plays");
        let (a, b) = (crossings(first), crossings(second));
        assert!((a as f32 - 176.0).abs() <= 2.0, "440 Hz over 0.4 s: {a}");
        assert!((b as f32 - 352.0).abs() <= 3.0, "an octave up: {b}");
        assert!(peak(&out[0][36_000..48_000]) < 1e-3, "released between the notes");
        e.execute("edit.undo", &json!({})).unwrap();
        let back = e.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(back.plugin, "subtractive_synth");
        assert!(back.sample.is_none());
        assert!(e.session().sources.is_empty(), "undo removes the source too");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn loads_dropped_bytes_as_one_undo_step() {
        let d = dir("bytes");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 48_000, 0.5, 1);
        let bytes = std::fs::read(&wav).unwrap();
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        let steps = e.undo_history().len();
        let r = crate::io::load_sampler_bytes(&mut e, t, "dropped.wav", &bytes).unwrap();
        assert_eq!(r["name"], "dropped");
        assert_eq!(r["frames"], 24_000);
        assert_eq!(e.undo_label(), Some("Load Sampler Sample"));
        assert_eq!(e.undo_history().len(), steps + 1);
        let src = e.session().sources.first().cloned().unwrap();
        assert!(src.unsaved, "written into Audio Files on the next save");
        assert!(e.undo());
        assert!(e.session().sources.is_empty());
        assert_eq!(e.session().track(t).unwrap().instrument.as_ref().map(|i| i.plugin.as_str()), Some("subtractive_synth"));
        assert!(crate::io::load_sampler_bytes(&mut e, t, "junk.wav", b"not audio").is_err());
        e.execute("track.new", &json!({"kind": "audio"})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        assert!(crate::io::load_sampler_bytes(&mut e, audio, "tone.wav", &bytes).is_err(), "an audio track");
        assert!(e.session().sources.is_empty());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn loads_flac_and_aiff_too() {
        let d = dir("formats");
        let n = 4800;
        let tone: Vec<f32> = (0..n).map(|i| 0.5 * (std::f32::consts::TAU * 440.0 * i as f32 / 96_000.0).sin()).collect();
        let buf = AudioBuffer { sample_rate: 96_000, channels: vec![tone.clone(), tone] };
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        for (ext, format) in [("flac", FileFormat::Flac), ("aif", FileFormat::Aiff)] {
            let path = d.join(format!("tone.{ext}"));
            let opts = EncodeOptions { format, bit_depth: BitDepth::Int24, dither: false, bwf: None };
            std::fs::write(&path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
            let r = e.execute("mix.sampler_load", &json!({"track": t.0, "path": path.to_string_lossy()})).unwrap();
            assert_eq!(r["channels"], 2, "{ext}");
            assert_eq!(r["frames"], 2400, "{ext}: 96 kHz resampled to the 48 kHz session");
        }
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn instrument_params_reach_the_sampler() {
        let d = dir("params");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 48_000, 2.0, 2);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        add_notes(&mut e, t, &[(60, 0, 1)]);
        e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy(), "params": {"key_low": 61}})).unwrap();
        assert_eq!(peak(&render(&e, 1)[0]), 0.0, "key 60 is below the key range");
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "key_low", "value": 0})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "root_note", "value": 48})).unwrap();
        let out = render(&e, 1);
        assert!((crossings(&out[0][2400..21_600]) as f32 - 352.0).abs() <= 3.0, "root note 48: key 60 plays an octave up");
        assert!(peak(&out[1]) > 0.05, "a stereo sample plays on both sides");
        assert!(e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy(), "params": {"nope": 1}})).is_err());
        assert!(e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy(), "params": {"root_note": "C3"}})).is_err());
        assert!(e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy(), "params": 3})).is_err());
        assert_eq!(e.session().sources.len(), 1, "rejected loads add nothing");
        // Reloading keeps the sampler's settings and swaps the sample.
        let r = e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy()})).unwrap();
        let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(ins.params.get("root_note"), Some(&48.0));
        assert_eq!(ins.sample.map(|s| s.0), r["source"].as_u64());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn clear_silences_the_sampler() {
        let d = dir("clear");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 48_000, 1.0, 1);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        add_notes(&mut e, t, &[(60, 0, 1)]);
        e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy()})).unwrap();
        assert!(peak(&render(&e, 1)[0]) > 0.05);
        let r = e.execute("mix.sampler_clear", &json!({"track": t.0})).unwrap();
        assert!(r["cleared"].is_u64());
        assert_eq!(peak(&render(&e, 1)[0]), 0.0);
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": "drum_synth"})).unwrap();
        assert!(e.execute("mix.sampler_clear", &json!({"track": t.0})).is_err(), "not a sampler");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_sampled_session_saves_and_reopens_with_its_sample() {
        let d = dir("save");
        let wav = d.join("pluck.wav");
        tone_wav(&wav, 330.0, 22_050, 1.0, 1);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        add_notes(&mut e, t, &[(64, 0, 1)]);
        e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy(), "params": {"root_note": 64, "loop_mode": 1}})).unwrap();
        let before = render(&e, 1);
        let path = d.join("song.scraft");
        e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
        std::fs::remove_file(&wav).unwrap();
        let mut e2 = Engine::default();
        e2.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        let ins = e2.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(ins.plugin, "sampler");
        assert_eq!(ins.params.get("loop_mode"), Some(&1.0));
        let after = render(&e2, 1);
        assert!(peak(&after[0]) > 0.05, "the saved copy in Audio Files plays");
        let diff = before[0].iter().zip(&after[0]).fold(0.0f32, |m, (a, b)| m.max((a - b).abs()));
        assert!(diff < 1e-5, "same audio after reopening: {diff}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn refuses_bad_input() {
        let d = dir("bad");
        let junk = d.join("junk.wav");
        std::fs::write(&junk, b"definitely not audio").unwrap();
        let empty = d.join("empty.wav");
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
        std::fs::write(&empty, soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![vec![]] }, &opts).unwrap()).unwrap();
        let wav = d.join("ok.wav");
        tone_wav(&wav, 440.0, 48_000, 0.1, 1);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        e.execute("track.new", &json!({"kind": "audio"})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        let load = |e: &mut Engine, track: u64, path: &std::path::Path| {
            e.execute("mix.sampler_load", &json!({"track": track, "path": path.to_string_lossy()}))
        };
        assert!(load(&mut e, t.0, &junk).is_err(), "not audio");
        assert!(load(&mut e, t.0, &empty).is_err(), "no frames");
        assert!(load(&mut e, t.0, &d.join("missing.wav")).is_err(), "missing file");
        assert!(load(&mut e, audio.0, &wav).is_err(), "an audio track");
        assert!(load(&mut e, 999_999, &wav).is_err(), "no such track");
        assert!(e.execute("mix.sampler_load", &json!({"track": t.0})).is_err(), "no path");
        assert!(e.execute("mix.sampler_clear", &json!({"track": audio.0})).is_err());
        assert!(e.session().sources.is_empty());
        assert_eq!(e.session().track(t).unwrap().instrument.as_ref().map(|i| i.plugin.as_str()), Some("subtractive_synth"));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn missing_media_plays_silence() {
        let d = dir("missing");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 48_000, 1.0, 1);
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        add_notes(&mut e, t, &[(60, 0, 1)]);
        let r = e.execute("mix.sampler_load", &json!({"track": t.0, "path": wav.to_string_lossy()})).unwrap();
        let src = soundcraft_model::SourceId(r["source"].as_u64().unwrap());
        e.session_mut().pool.remove(src);
        assert_eq!(peak(&render(&e, 1)[0]), 0.0);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn samples_a_selection_from_an_audio_track_and_detects_its_root() {
        let d = dir("from-sel");
        let wav = d.join("a3.wav");
        tone_wav(&wav, 220.0, 48_000, 3.0, 2);
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "audio", "format": "stereo"})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": audio.0, "at": 0})).unwrap();
        let t = instrument_track(&mut e);
        let undo_before = e.session().sources.len();
        let r = e.execute("mix.sampler_from_selection", &json!({"track": t.0, "from": audio.0, "start": 24_000, "end": 72_000})).unwrap();
        assert_eq!(r["frames"], 48_000, "the selection's length: {r}");
        assert_eq!(r["channels"], 2);
        let name = r["name"].as_str().unwrap().to_string();
        assert!(name.ends_with("sample"), "{name}");
        let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
        assert_eq!(ins.plugin, "sampler");
        assert_eq!(ins.sample.map(|s| s.0), r["source"].as_u64());
        let found = e.execute("mix.sampler_detect", &json!({"track": t.0})).unwrap();
        assert_eq!(found["note"], 57, "A3: {found}");
        assert_eq!(found["cents"], 0.0);
        assert_eq!(found["name"], soundcraft_midi::note_name(57));
        // Played at its root (A3), it sounds as recorded.
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "root_note", "value": 57})).unwrap();
        add_notes(&mut e, t, &[(57, 0, 1)]);
        let out = render(&e, 1);
        let held = &out[0][2400..21_600];
        assert!((crossings(held) as f32 - 88.0).abs() <= 2.0, "220 Hz over 0.4 s: {}", crossings(held));
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().sources.len(), undo_before, "undo removes the sampled source");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn sampling_uses_the_selection_and_refuses_what_it_cannot_sample() {
        let d = dir("from-sel-bad");
        let wav = d.join("tone.wav");
        tone_wav(&wav, 440.0, 48_000, 1.0, 1);
        let mut e = Engine::default();
        e.execute("track.new", &json!({"kind": "audio", "format": "mono"})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy(), "track": audio.0, "at": 0})).unwrap();
        let t = instrument_track(&mut e);
        // Nothing selected: no range.
        let err = e.execute("mix.sampler_from_selection", &json!({"track": t.0, "from": audio.0})).unwrap_err();
        assert!(err.to_string().contains("empty"), "{err}");
        // The edit selection and the selected tracks pick the range and both tracks.
        {
            let s = e.session_mut();
            s.edit.selection = Range::new(0, 12_000);
            s.edit.selected_tracks = vec![audio, t].into_iter().collect();
        }
        let r = e.execute("mix.sampler_from_selection", &json!({})).unwrap();
        assert_eq!((r["track"].as_u64(), r["frames"].as_u64()), (Some(t.0), Some(12_000)), "{r}");
        // Silence, a MIDI source and a non-instrument target are refused.
        let err = e.execute("mix.sampler_from_selection", &json!({"track": t.0, "from": audio.0, "start": 96_000, "end": 120_000})).unwrap_err();
        assert!(err.to_string().contains("silent"), "{err}");
        let err = e.execute("mix.sampler_from_selection", &json!({"track": t.0, "from": t.0, "start": 0, "end": 100})).unwrap_err();
        assert!(err.to_string().contains("no audio clips"), "{err}");
        let err = e.execute("mix.sampler_from_selection", &json!({"track": audio.0, "from": audio.0, "start": 0, "end": 100})).unwrap_err();
        assert!(err.to_string().contains("instrument"), "{err}");
        let err = e
            .execute("mix.sampler_from_selection", &json!({"track": t.0, "from": audio.0, "start": 0, "end": 100, "params": {"nope": 1}}))
            .unwrap_err();
        assert!(err.to_string().contains("nope"), "{err}");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn detect_needs_a_sample_and_finds_no_root_in_noise() {
        let mut e = Engine::default();
        let t = instrument_track(&mut e);
        assert!(e.execute("mix.sampler_detect", &json!({"track": t.0})).is_err(), "not a sampler yet");
        let mut x = 0x2468_ACE1u32;
        let noise: Vec<f32> = (0..48_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                (x as f32 / u32::MAX as f32) - 0.5
            })
            .collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![noise] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
        let bytes = soundcraft_audio_io::encode(&buf, &opts).unwrap();
        crate::io::load_sampler_bytes(&mut e, t, "noise.wav", &bytes).unwrap();
        let r = e.execute("mix.sampler_detect", &json!({"track": t.0})).unwrap();
        assert!(r["root"].is_null() && r["note"].is_null(), "{r}");
    }
}

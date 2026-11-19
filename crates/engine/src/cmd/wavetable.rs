//! The Wavetable Synth's user tables: a table made from the user's own audio, stored in the
//! synth's insert (`Insert::state`: the waves themselves, see
//! [`soundcraft_dsp::plugins::wavetable::user`]), so a session never depends on the file it came
//! from. Whenever the state changes, and when a session opens, the table is band-limited here,
//! off the audio thread, into `Insert::prepared` ([`prepared`]); the mix engine hands it to the
//! running synth by reference between blocks.
//!
//! The commands: `mix.wavetable_import` reads an audio file (through the engine's file access
//! rules), cuts it into waves in one of three ways ([`import`]: fixed frames, pitch-tracked
//! cycles with the Vocal Editor's tracker, spectral resynthesis) and stores the table: one undo
//! step. `mix.wavetable_preview` shows what an import would make without changing anything;
//! `mix.wavetable_clear` goes back to the built-in tables; `mix.wavetable_save` and
//! `mix.wavetable_load` write and read a plain table WAV other synths read too.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::plugins::wavetable::{import, user};
use soundcraft_model::{Insert, PreparedState, TrackKind};
use std::path::Path;
use std::sync::Arc;

/// The Wavetable Synth's plugin id.
pub const WAVETABLE: &str = "wavetable_synth";
/// The largest file read for an import.
pub const MAX_FILE_BYTES: u64 = 256 << 20;
/// Audio longer than this is refused.
pub const MAX_FILE_SECONDS: f64 = 600.0;
/// Audio longer than this is cut to it (the first minute is what an import reads).
pub const MAX_READ_SECONDS: f64 = 60.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.wavetable_import",
            "Import Wavetable",
            [],
            None,
            "{track?, path, mode?: fixed|pitch|spectral (default fixed), frame_size?: 256|512|1024|2048|4096|8192 (fixed: default from the file's `clm ` chunk, else guessed from its length), waves?: 1..32 (pitch, spectral: default 32), name?} — makes the track's Cicada (wavetable synth) play a table made from an audio file: `fixed` reads the file as a row of single-cycle frames (as wavetable files are; more than 32: 32 spread through them), `pitch` cuts one cycle at each of up to 32 points at the pitch tracked there, `spectral` makes one clean cycle from the spectrum at each point (any sound: noise, speech, drums). The table (not the file) is kept in the session. Files up to 10 minutes; only the first minute is read → {track, name, waves, mode, frame_size, notes: [what was guessed, spread or cut]}",
            always,
            import_cmd
        ),
        cmd!(
            query "mix.wavetable_preview",
            "Preview Wavetable Import",
            [],
            None,
            "{the import's params, points?: 16..2048 (default 256)} — the waves `mix.wavetable_import` would make, each as `points` samples (the whole table scaled to peak 1); nothing changes → {name, mode, frame_size, waves: [[…]], notes}",
            always,
            preview_cmd
        ),
        cmd!(
            "mix.wavetable_clear",
            "Clear Wavetable",
            [],
            None,
            "{track?} — the track's Cicada (wavetable synth) goes back to its built-in tables (the Table control), its imported table removed from the session → {track}",
            always,
            clear_cmd
        ),
        cmd!(
            noundo "mix.wavetable_save",
            "Save Wavetable",
            [],
            None,
            "{track?, path} — writes the track's imported table as a plain 32-bit float mono WAV of 32 frames × 2048 samples (with a `clm ` chunk naming the frame size, so other wavetable synths read it as a table) → {track, path, frames}",
            always,
            save_cmd
        ),
        cmd!(
            "mix.wavetable_load",
            "Load Wavetable",
            [],
            None,
            "{track?, path, name?} — a wavetable WAV (frames of 2048 samples, or the size its `clm ` chunk names) becomes the track's Cicada (wavetable synth)'s table: `mix.wavetable_import` with mode fixed → as that",
            always,
            load_cmd
        ),
        cmd!(
            query "mix.wavetable_info",
            "Wavetable Info",
            [],
            None,
            "{track?} — the table the track's Cicada (wavetable synth) plays → {track, user: bool (an imported table), name, waves, bank (the built-in table), error?: what is wrong with a damaged imported table}",
            always,
            info_cmd
        ),
    ]
}

/// The band-limited table a Wavetable Synth insert's stored state makes (`None` when it has none
/// or is another plugin; an error naming what is wrong with a damaged one). Tables built lately
/// are kept by the hash of the state that made them, so preparing an insert again (a session
/// reopened in the same run, its modulation edited) does not build its table again. Allocates:
/// never on the audio thread.
pub(crate) fn built_table(ins: &Insert) -> std::result::Result<Option<Arc<dyn soundcraft_dsp::SampleSource>>, String> {
    use std::hash::{Hash, Hasher};
    static RECENT: std::sync::Mutex<Vec<(u64, Arc<dyn soundcraft_dsp::SampleSource>)>> = std::sync::Mutex::new(Vec::new());
    if ins.plugin != WAVETABLE {
        return Ok(None);
    }
    let Some(text) = ins.state.as_ref() else { return Ok(None) };
    let mut h = std::collections::hash_map::DefaultHasher::new();
    text.hash(&mut h);
    let key = h.finish();
    if let Ok(recent) = RECENT.lock()
        && let Some((_, t)) = recent.iter().find(|(k, _)| *k == key)
    {
        return Ok(Some(Arc::clone(t)));
    }
    let bytes = ins.state_bytes().ok_or("the wavetable is not valid base64")?;
    let built = user::prepare(&bytes)?;
    if let Ok(mut recent) = RECENT.lock() {
        if recent.len() >= 8 {
            recent.remove(0);
        }
        recent.push((key, Arc::clone(&built)));
    }
    Ok(Some(built))
}

/// What a Wavetable Synth insert's stored state makes ready for it: `data` (numbers made ready
/// from its other settings, empty when none) with its user table band-limited beside them.
pub(crate) fn prepared_with(ins: &Insert, data: Option<PreparedState>) -> std::result::Result<Option<PreparedState>, String> {
    let Some(table) = built_table(ins)? else { return Ok(data) };
    let shared: Arc<dyn std::any::Any + Send + Sync> = Arc::new(table);
    Ok(Some(match data {
        Some(d) => PreparedState::with_shared(d.data, shared),
        None => PreparedState::new_shared(shared),
    }))
}

/// [`prepared_with`] with nothing else made ready.
#[cfg(test)]
pub(crate) fn prepared(ins: &Insert) -> std::result::Result<Option<PreparedState>, String> {
    prepared_with(ins, None)
}

/// Adds a Wavetable Synth's user table to what `ins.prepared` already holds (call it after
/// whatever else prepares the insert): a damaged table is left out (the synth plays its built-in
/// tables) and logged. Other plugins are left alone.
pub(crate) fn attach_table(ins: &mut Insert) {
    if ins.plugin != WAVETABLE {
        return;
    }
    let data = ins.prepared.take();
    ins.prepared = prepared_with(ins, data.clone()).unwrap_or_else(|err| {
        log::warn!("{WAVETABLE}: its user wavetable is damaged and was left out: {err}");
        data
    });
}

/// The user table a Wavetable Synth insert holds, decoded (`None`: none, or another plugin).
pub fn user_table(ins: &Insert) -> Option<std::result::Result<user::UserTable, String>> {
    if ins.plugin != WAVETABLE {
        return None;
    }
    ins.state_bytes().map(|b| user::UserTable::decode(&b))
}

/// The instrument track a command acts on (`track`, else the first selected instrument track)
/// and its Wavetable Synth; errors when it has another instrument.
fn synth_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let named = p.get("track").is_some() || p.get("tracks").is_some();
    let ts = tracks_required(e, id, p)?;
    let s = e.session();
    let is_synth = |t: &TrackId| s.track(*t).and_then(|tr| tr.instrument.as_ref()).is_some_and(|i| i.plugin == WAVETABLE);
    let t = if named { ts.first().copied() } else { ts.iter().copied().find(is_synth).or_else(|| ts.first().copied()) };
    let t = t.ok_or_else(|| bad(id, "no track"))?;
    let tr = s.track(t).ok_or_else(|| bad(id, "no such track"))?;
    if tr.kind != TrackKind::Instrument {
        return Err(bad(id, format!("`{}` is not an instrument track", tr.name)));
    }
    if !is_synth(&t) {
        return Err(bad(id, format!("`{}` does not play Cicada (the wavetable synth)", tr.name)));
    }
    Ok(t)
}

fn synth_insert<'a>(e: &'a Engine, id: &str, t: TrackId) -> Result<&'a Insert> {
    e.session().track(t).and_then(|tr| tr.instrument.as_ref()).ok_or_else(|| bad(id, "the track has no instrument"))
}

/// How an import cuts the audio.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Mode {
    Fixed,
    Pitch,
    Spectral,
}

impl Mode {
    fn name(self) -> &'static str {
        match self {
            Mode::Fixed => "fixed",
            Mode::Pitch => "pitch",
            Mode::Spectral => "spectral",
        }
    }
}

/// An import's parameters, checked before anything is read.
struct Options {
    path: String,
    mode: Mode,
    frame_size: Option<usize>,
    waves: usize,
    name: Option<String>,
}

fn options(id: &str, p: &Value) -> Result<Options> {
    let path =
        str_param(p, "path").filter(|s| !s.is_empty()).ok_or_else(|| bad(id, "`path` required: the audio file to make the table from"))?.to_string();
    let mode = match p.get("mode") {
        None | Some(Value::Null) => Mode::Fixed,
        Some(Value::String(m)) => match m.to_ascii_lowercase().as_str() {
            "fixed" => Mode::Fixed,
            "pitch" | "pitched" => Mode::Pitch,
            "spectral" => Mode::Spectral,
            _ => return Err(bad(id, format!("no mode `{m}`: fixed, pitch or spectral"))),
        },
        Some(_) => return Err(bad(id, "`mode` must be fixed, pitch or spectral")),
    };
    let frame_size = match p.get("frame_size") {
        None | Some(Value::Null) => None,
        Some(v) => Some(
            v.as_u64()
                .and_then(|n| usize::try_from(n).ok())
                .filter(|n| import::FRAME_SIZES.contains(n))
                .ok_or_else(|| bad(id, "`frame_size` must be 256, 512, 1024, 2048, 4096 or 8192"))?,
        ),
    };
    let waves = match p.get("waves") {
        None | Some(Value::Null) => user::MAX_WAVES,
        Some(v) => v
            .as_u64()
            .and_then(|n| usize::try_from(n).ok())
            .filter(|n| (1..=user::MAX_WAVES).contains(n))
            .ok_or_else(|| bad(id, format!("`waves` must be 1..{}", user::MAX_WAVES)))?,
    };
    let name = match p.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(s)) => Some(s.trim().to_string()).filter(|s| !s.is_empty()),
        Some(_) => return Err(bad(id, "`name` must be text")),
    };
    Ok(Options { path, mode, frame_size, waves, name })
}

/// A file read for an import: mono, at its own rate, at most [`MAX_READ_SECONDS`].
struct Audio {
    mono: Vec<f32>,
    rate: u32,
    clm: Option<usize>,
    notes: Vec<String>,
}

fn read_audio(id: &str, path: &str) -> Result<Audio> {
    let bytes = crate::io::read_file_max(path, MAX_FILE_BYTES)?;
    let ext = Path::new(path).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let clm = import::clm_frame_size(&bytes);
    let info = soundcraft_audio_io::probe(&bytes, ext.as_deref()).map_err(|err| bad(id, format!("{path}: not audio this build reads ({err})")))?;
    let rate = info.sample_rate;
    if rate == 0 {
        return Err(bad(id, format!("{path}: no sample rate")));
    }
    if info.frames as f64 / f64::from(rate) > MAX_FILE_SECONDS {
        return Err(bad(
            id,
            format!("{path}: too long to make a wavetable from (at most {} minutes; only the first minute is used)", MAX_FILE_SECONDS / 60.0),
        ));
    }
    let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| bad(id, format!("{path}: {err}")))?;
    drop(bytes);
    let frames = buf.frames();
    if frames == 0 || buf.channels.is_empty() {
        return Err(bad(id, format!("{path}: no audio")));
    }
    let keep = frames.min((MAX_READ_SECONDS * f64::from(rate)) as usize);
    let mut notes = Vec::new();
    if keep < frames {
        notes.push(format!("Only the first {MAX_READ_SECONDS:.0} seconds of the file were read ({:.0} s long).", frames as f64 / f64::from(rate)));
    }
    let ch = buf.channels.len() as f32;
    let mono = (0..keep)
        .map(|i| {
            let v = buf.channels.iter().map(|c| c.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0)).sum::<f32>() / ch;
            if v.is_finite() { v } else { 0.0 }
        })
        .collect();
    if buf.channels.len() > 1 {
        notes.push(format!("The file's {} channels were mixed to one.", buf.channels.len()));
    }
    Ok(Audio { mono, rate, clm, notes })
}

/// The pitch through `a` (Hz per frame of the vocal tracker, `None` unpitched) and its hop.
fn pitch_of(id: &str, a: &Audio) -> Result<(Vec<Option<f32>>, usize)> {
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let t = soundcraft_vocal::track_pitch(&a.mono, a.rate, &cancel).map_err(|err| bad(id, format!("no pitch could be tracked: {err}")))?;
    let hz = t.frames.iter().map(|f| f.cents.filter(|c| c.is_finite()).map(|c| 440.0 * 2f32.powf((c / 100.0 - 69.0) / 12.0))).collect();
    Ok((hz, t.hop_samples as usize))
}

/// The table an import makes, and its notes and frame size.
fn slice(id: &str, o: &Options) -> Result<(user::UserTable, usize, Vec<String>)> {
    let a = read_audio(id, &o.path)?;
    let mut notes = a.notes.clone();
    let mut frame = 0;
    let sliced = match o.mode {
        Mode::Fixed => {
            let (size, guessed) = match o.frame_size {
                Some(n) => (n, false),
                None => import::guess_frame_size(a.mono.len(), a.clm),
            };
            if guessed {
                notes.push(format!("Frames of {size} samples (guessed from the length: choose another size if the waves look wrong)."));
            }
            frame = size;
            import::fixed(&a.mono, size)
        }
        Mode::Pitch => {
            let (pitch, hop) = pitch_of(id, &a)?;
            import::pitched(&a.mono, a.rate as f32, &pitch, hop, o.waves)
        }
        Mode::Spectral => {
            // The recording's own pitch as the reference when at least half of it has one.
            let (pitch, _) = pitch_of(id, &a).unwrap_or_default();
            let mut voiced: Vec<f32> = pitch.iter().flatten().copied().collect();
            let f_ref = (!voiced.is_empty() && voiced.len() * 2 >= pitch.len()).then(|| {
                voiced.sort_by(f32::total_cmp);
                voiced.get(voiced.len() / 2).copied().unwrap_or(0.0)
            });
            match f_ref {
                Some(f) => notes.push(format!("Harmonics of {f:.1} Hz, the recording's pitch.")),
                None => notes.push("No steady pitch: each harmonic is one band of the spectrum.".into()),
            }
            import::spectral(&a.mono, a.rate as f32, o.waves, f_ref)
        }
    };
    notes.extend(sliced.notes.iter().cloned());
    // Silent once the constant level is taken out (as band-limiting does): nothing to play.
    let audible = sliced.waves.iter().any(|w| {
        let mean = w.iter().sum::<f32>() / w.len().max(1) as f32;
        w.iter().any(|v| (v - mean).abs() > 1e-5)
    });
    if sliced.waves.is_empty() {
        return Err(bad(id, notes.join(" ")));
    }
    if !audible {
        return Err(bad(id, "nothing to make a table from: the audio is silent (or a constant level)"));
    }
    let name = o.name.clone().unwrap_or_else(|| Path::new(&o.path).file_stem().and_then(|n| n.to_str()).unwrap_or("Imported").to_string());
    let table = user::UserTable::new(&name, &sliced.waves).ok_or_else(|| bad(id, "no waves"))?;
    Ok((table, frame, notes))
}

/// Puts `table` into track `t`'s synth (made ready off the audio thread here).
fn store(e: &mut Engine, id: &str, t: TrackId, table: &user::UserTable) -> Result<()> {
    let s = e.session_mut();
    let ins = s.track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(id, "the track has no instrument"))?;
    ins.set_state_bytes(&table.encode());
    ins.prepared = None;
    super::module_ceiling::prepare_insert(ins);
    if ins.prepared.as_ref().is_none_or(|p| p.shared.is_none()) {
        return Err(bad(id, "the table could not be made ready"));
    }
    Ok(())
}

fn import_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_import";
    let t = synth_track(e, C, p)?;
    let o = options(C, p)?;
    let (table, frame, notes) = slice(C, &o)?;
    store(e, C, t, &table)?;
    Ok(
        json!({"track": t, "name": table.name, "waves": table.wave_count(), "mode": o.mode.name(), "frame_size": (frame > 0).then_some(frame), "notes": notes}),
    )
}

fn preview_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_preview";
    let o = options(C, p)?;
    let points = match p.get("points") {
        None | Some(Value::Null) => 256,
        Some(v) => {
            v.as_u64().and_then(|n| usize::try_from(n).ok()).filter(|n| (16..=2048).contains(n)).ok_or_else(|| bad(C, "`points` must be 16..2048"))?
        }
    };
    let _ = e;
    let (table, frame, notes) = slice(C, &o)?;
    let n = table.wave_count();
    let waves: Vec<Vec<f32>> = (0..n)
        .map(|i| {
            let mut out = vec![0.0f32; points];
            table.draw(if n > 1 { i as f32 / (n - 1) as f32 } else { 0.0 }, &mut out);
            out
        })
        .collect();
    Ok(json!({"name": table.name, "mode": o.mode.name(), "frame_size": (frame > 0).then_some(frame), "waves": waves, "notes": notes}))
}

fn clear_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_clear";
    let t = synth_track(e, C, p)?;
    if synth_insert(e, C, t)?.state.is_none() {
        return Err(bad(C, "the synth already plays its built-in tables"));
    }
    let s = e.session_mut();
    let ins = s.track_mut(t).and_then(|tr| tr.instrument.as_mut()).ok_or_else(|| bad(C, "the track has no instrument"))?;
    ins.state = None;
    super::module_ceiling::prepare_insert(ins);
    Ok(json!({"track": t}))
}

/// The frames a saved table file holds.
pub const SAVE_FRAMES: usize = 32;

/// `table` as a WAV other wavetable synths read: [`SAVE_FRAMES`] frames of 2048 samples (the
/// table read at 32 even positions, as the synth crossfades between its waves), 32-bit float,
/// mono, with a `clm ` chunk naming the frame size.
pub fn table_wav(table: &user::UserTable) -> Result<Vec<u8>> {
    let mut samples = Vec::with_capacity(SAVE_FRAMES * user::WAVE_LEN);
    let n = table.wave_count();
    for i in 0..SAVE_FRAMES {
        let pos = i as f32 / (SAVE_FRAMES - 1) as f32 * (n.saturating_sub(1)) as f32;
        let (a, b) = (pos.floor() as usize, (pos.floor() as usize + 1).min(n.saturating_sub(1)));
        let f = pos - a as f32;
        let (wa, wb) = (table.waves.get(a), table.waves.get(b));
        for k in 0..user::WAVE_LEN {
            let x = wa.and_then(|w| w.get(k)).copied().unwrap_or(0.0);
            let y = wb.and_then(|w| w.get(k)).copied().unwrap_or(x);
            samples.push(x + (y - x) * f);
        }
    }
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![samples] };
    let opts = soundcraft_audio_io::EncodeOptions {
        format: soundcraft_audio_io::FileFormat::Wav,
        bit_depth: soundcraft_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
        bitrate_kbps: 0,
    };
    let wav = soundcraft_audio_io::encode(&buf, &opts).map_err(|err| EngineError::Io(err.to_string()))?;
    Ok(with_clm_chunk(&wav, user::WAVE_LEN))
}

/// `wav` with a `clm ` chunk naming `frame` (`<!>2048 …`) put before its `data` chunk and the
/// RIFF size made to match. Unchanged if it is not a RIFF/WAVE.
fn with_clm_chunk(wav: &[u8], frame: usize) -> Vec<u8> {
    let (Some(b"RIFF"), Some(b"WAVE")) = (wav.get(..4), wav.get(8..12)) else { return wav.to_vec() };
    let mut text = format!("<!>{frame} 00000000 wavetable").into_bytes();
    if text.len() % 2 == 1 {
        text.push(b' ');
    }
    let mut chunk = Vec::with_capacity(8 + text.len());
    chunk.extend_from_slice(b"clm ");
    chunk.extend_from_slice(&(text.len() as u32).to_le_bytes());
    chunk.extend_from_slice(&text);
    // Find `data`; the chunk goes in front of it.
    let mut at = 12usize;
    let mut data_at = None;
    while let Some(head) = wav.get(at..at + 8) {
        if head.get(..4) == Some(b"data".as_slice()) {
            data_at = Some(at);
            break;
        }
        let len = head.get(4..8).and_then(|b| b.try_into().ok()).map_or(0, u32::from_le_bytes) as usize;
        at = at.saturating_add(8).saturating_add(len).saturating_add(len & 1);
    }
    let Some(at) = data_at else { return wav.to_vec() };
    let mut out = Vec::with_capacity(wav.len() + chunk.len());
    out.extend_from_slice(wav.get(..at).unwrap_or(&[]));
    out.extend_from_slice(&chunk);
    out.extend_from_slice(wav.get(at..).unwrap_or(&[]));
    let riff = (out.len() - 8) as u32;
    if let Some(sz) = out.get_mut(4..8) {
        sz.copy_from_slice(&riff.to_le_bytes());
    }
    out
}

fn save_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_save";
    let t = synth_track(e, C, p)?;
    let path = str_param(p, "path").filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "`path` required: where to write the table"))?.to_string();
    let table = match user_table(synth_insert(e, C, t)?) {
        Some(Ok(table)) => table,
        Some(Err(err)) => return Err(bad(C, format!("the synth's imported table is damaged: {err}"))),
        None => return Err(bad(C, "the synth plays its built-in tables: import a table first")),
    };
    let bytes = table_wav(&table)?;
    crate::io::write_file(&path, &bytes)?;
    Ok(json!({"track": t, "path": path, "frames": SAVE_FRAMES}))
}

fn load_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_load";
    let t = synth_track(e, C, p)?;
    let mut o = options(C, p)?;
    o.mode = Mode::Fixed;
    let (table, frame, notes) = slice(C, &o)?;
    store(e, C, t, &table)?;
    Ok(
        json!({"track": t, "name": table.name, "waves": table.wave_count(), "mode": o.mode.name(), "frame_size": (frame > 0).then_some(frame), "notes": notes}),
    )
}

fn info_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.wavetable_info";
    let t = synth_track(e, C, p)?;
    let ins = synth_insert(e, C, t)?;
    let bank = soundcraft_dsp::plugins::wavetable::tables::BANKS
        .get(ins.params.get("table").copied().filter(|v| v.is_finite()).map_or(0, |v| v.round().max(0.0) as usize))
        .copied()
        .unwrap_or("");
    Ok(match user_table(ins) {
        Some(Ok(table)) => json!({"track": t, "user": true, "name": table.name, "waves": table.wave_count(), "bank": bank}),
        Some(Err(err)) => json!({"track": t, "user": false, "name": bank, "waves": 32, "bank": bank, "error": err}),
        None => json!({"track": t, "user": false, "name": bank, "waves": 32, "bank": bank}),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_midi::{Note, Sequence};
    use soundcraft_model::{Clip, Session, TrackId};
    use soundcraft_time::{Range, TICKS_PER_QUARTER};

    pub(super) fn synth_track(e: &mut Engine) -> TrackId {
        e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        e.execute("mix.instrument", &json!({"track": t.0, "plugin": WAVETABLE, "params": {"cutoff": 20000.0}})).unwrap();
        let q = TICKS_PER_QUARTER;
        let mut seq = Sequence::default();
        for (pitch, start) in [(57u8, 0i64), (69, 2)] {
            seq.notes.push(Note {
                probability: 100,
                pitch,
                velocity: 120,
                muted: false,
                release_velocity: 64,
                channel: 0,
                start: start * q,
                length: q,
            });
        }
        let s = e.session_mut();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "notes", 0, 48_000 * 4, seq));
        t
    }

    pub(super) fn render(s: &Session) -> Vec<f32> {
        soundcraft_mix::render_range(s, Range::new(0, 48_000), 512).swap_remove(0)
    }

    pub(super) fn odd_table() -> user::UserTable {
        let w: Vec<f32> = (0..user::WAVE_LEN).map(|i| (std::f32::consts::TAU * 3.0 * i as f32 / user::WAVE_LEN as f32).sin()).collect();
        user::UserTable::new("third", &[w]).unwrap()
    }

    fn crossings(x: &[f32]) -> usize {
        x.windows(2).filter(|w| w[0] < 0.0 && w[1] >= 0.0).count()
    }

    #[test]
    fn a_user_table_in_the_session_plays_saves_and_reopens() {
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let plain = render(e.session());
        // A table whose only wave is the third harmonic: the note sounds an octave and a fifth up.
        let mut s = e.session().clone();
        let ins = s.tracks.iter_mut().find(|x| x.id == t).unwrap().instrument.as_mut().unwrap();
        ins.set_state_bytes(&odd_table().encode());
        let e2 = Engine::new(s);
        assert!(e2.session().track(t).unwrap().instrument.as_ref().unwrap().prepared.is_some());
        let with = render(e2.session());
        let (a, b) = (crossings(&plain[2_400..24_000]), crossings(&with[2_400..24_000]));
        eprintln!("crossings: built-in {a}, user table {b}");
        assert!((b as f32 / a as f32 - 3.0).abs() < 0.1, "{a} {b}");
        // Saved and opened again (JSON): the table travels in the session and sounds the same.
        let text = serde_json::to_string(e2.session()).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        let e3 = Engine::new(back);
        assert_eq!(render(e3.session()), with);
    }

    #[test]
    fn a_damaged_table_is_left_out_and_never_panics() {
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let plain = render(e.session());
        let mut good = odd_table().encode();
        for damage in [vec![0xff, 0x00], good.split_off(100), b"FDWT\x01\x20\x00\x08\x00".to_vec()] {
            let mut s = e.session().clone();
            let ins = s.tracks.iter_mut().find(|x| x.id == t).unwrap().instrument.as_mut().unwrap();
            ins.set_state_bytes(&damage);
            assert!(prepared(ins).is_err());
            assert!(user_table(ins).is_some_and(|r| r.is_err()));
            let e2 = Engine::new(s);
            assert!(e2.session().track(t).unwrap().instrument.as_ref().unwrap().prepared.is_none());
            assert_eq!(render(e2.session()), plain, "the built-in bank plays");
        }
    }

    fn dir(name: &str) -> std::path::PathBuf {
        let d = std::env::temp_dir().join(format!("sc-wavetable-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Writes generated audio (no recorded or third-party sound) as a 32-bit float WAV.
    fn wav(path: &std::path::Path, rate: u32, channels: Vec<Vec<f32>>) {
        use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
        let buf = AudioBuffer { sample_rate: rate, channels };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 };
        std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    }

    fn tone(f0: f32, rate: u32, secs: f32) -> Vec<f32> {
        (0..(rate as f32 * secs) as usize)
            .map(|i| {
                let t = i as f32 / rate as f32;
                (1..=6).map(|k| (std::f32::consts::TAU * f0 * k as f32 * t).sin() / k as f32).sum::<f32>() * 0.4
            })
            .collect()
    }

    fn state(e: &Engine, t: TrackId) -> Option<String> {
        e.session().track(t).unwrap().instrument.as_ref().unwrap().state.clone()
    }

    #[test]
    fn each_mode_imports_in_one_undo_step_and_plays() {
        let d = dir("modes");
        let file = d.join("hum.wav");
        wav(&file, 44_100, vec![tone(196.0, 44_100, 1.5), tone(196.0, 44_100, 1.5)]);
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let plain = render(e.session());
        for (mode, waves) in [("pitch", 16), ("spectral", 8), ("fixed", 32)] {
            let r = e.execute("mix.wavetable_import", &json!({"track": t.0, "path": file.to_string_lossy(), "mode": mode, "waves": waves})).unwrap();
            eprintln!("{mode}: {r}");
            assert_eq!(r["mode"], mode);
            assert_eq!(r["name"], "hum");
            assert_eq!(r["waves"].as_u64().unwrap() as usize, waves, "{r}");
            assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("mixed to one")));
            let ins = e.session().track(t).unwrap().instrument.clone().unwrap();
            assert!(ins.prepared.is_some());
            assert_ne!(render(e.session()), plain, "{mode}: the table plays");
            e.run("edit.undo").unwrap();
            assert_eq!(state(&e, t), None, "{mode}: undo takes it out again");
            assert_eq!(render(e.session()), plain);
            e.run("edit.redo").unwrap();
            assert!(state(&e, t).is_some());
            e.execute("mix.wavetable_clear", &json!({"track": t.0})).unwrap();
            assert_eq!(state(&e, t), None);
            assert_eq!(render(e.session()), plain, "{mode}: cleared, the built-in table plays");
        }
        assert!(e.execute("mix.wavetable_clear", &json!({"track": t.0})).is_err(), "nothing to clear");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_preview_changes_nothing_and_info_reports_the_table() {
        let d = dir("preview");
        let file = d.join("Lead Tone.wav");
        wav(&file, 48_000, vec![tone(220.0, 48_000, 1.0)]);
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let before = e.session().clone();
        let r = e
            .execute("mix.wavetable_preview", &json!({"track": t.0, "path": file.to_string_lossy(), "mode": "spectral", "waves": 4, "points": 64}))
            .unwrap();
        let waves = r["waves"].as_array().unwrap();
        assert_eq!(waves.len(), 4);
        assert!(waves.iter().all(|w| w.as_array().unwrap().len() == 64));
        assert_eq!(*e.session(), before);
        let info = e.execute("mix.wavetable_info", &json!({"track": t.0})).unwrap();
        assert_eq!((info["user"].as_bool(), info["name"].as_str()), (Some(false), Some("Classic")));
        e.execute("mix.wavetable_import", &json!({"track": t.0, "path": file.to_string_lossy(), "mode": "pitch", "waves": 5, "name": "Mine"}))
            .unwrap();
        let info = e.execute("mix.wavetable_info", &json!({"track": t.0})).unwrap();
        assert_eq!((info["user"].as_bool(), info["name"].as_str(), info["waves"].as_u64()), (Some(true), Some("Mine"), Some(5)));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_saved_table_is_a_plain_wav_and_loads_back_the_same() {
        let d = dir("save");
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let out = d.join("table.wav");
        assert!(e.execute("mix.wavetable_save", &json!({"track": t.0, "path": out.to_string_lossy()})).is_err(), "no table to save yet");
        // 32 frames of a table written frame by frame, brighter along it.
        let frames: Vec<f32> = (0..32)
            .flat_map(|f| {
                (0..2048).map(move |i| {
                    let x = i as f32 / 2048.0;
                    (1..=(1 + f / 4)).map(|k| (std::f32::consts::TAU * k as f32 * x).sin() / k as f32).sum::<f32>() * 0.5
                })
            })
            .collect();
        let src = d.join("frames.wav");
        wav(&src, 44_100, vec![frames]);
        let r = e.execute("mix.wavetable_load", &json!({"track": t.0, "path": src.to_string_lossy()})).unwrap();
        assert_eq!((r["waves"].as_u64(), r["frame_size"].as_u64()), (Some(32), Some(2048)), "{r}");
        let table = user_table(e.session().track(t).unwrap().instrument.as_ref().unwrap()).unwrap().unwrap();
        e.execute("mix.wavetable_save", &json!({"track": t.0, "path": out.to_string_lossy()})).unwrap();
        let bytes = std::fs::read(&out).unwrap();
        assert_eq!(import::clm_frame_size(&bytes), Some(2048), "the clm chunk names the frame size");
        let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).unwrap();
        assert_eq!((info.channels, buf.frames()), (1, 32 * 2048));
        // Read back: the same waves, sample for sample.
        e.execute("mix.wavetable_clear", &json!({"track": t.0})).unwrap();
        e.execute("mix.wavetable_load", &json!({"track": t.0, "path": out.to_string_lossy()})).unwrap();
        let back = user_table(e.session().track(t).unwrap().instrument.as_ref().unwrap()).unwrap().unwrap();
        assert_eq!(back.waves, table.waves);
        // A table of fewer waves is saved as 32 frames read through it as the synth plays it.
        let few = user::UserTable::new("few", &[vec![0.0; 2048], vec![1.0; 2048]]).unwrap();
        let w = table_wav(&few).unwrap();
        let (_, buf) = soundcraft_audio_io::decode(&w, Some("wav")).unwrap();
        let mid = buf.channels[0][16 * 2048];
        assert!((mid - 16.0 / 31.0).abs() < 1e-6, "{mid}");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn hostile_requests_are_refused_and_change_nothing() {
        let d = dir("hostile");
        let ok = d.join("ok.wav");
        wav(&ok, 48_000, vec![tone(300.0, 48_000, 0.5)]);
        let silent = d.join("silent.wav");
        wav(&silent, 48_000, vec![vec![0.0; 48_000]]);
        let dc = d.join("dc.wav");
        wav(&dc, 48_000, vec![vec![0.25; 48_000]]);
        let junk = d.join("junk.wav");
        std::fs::write(&junk, b"RIFF\x10\0\0\0WAVEfmt nonsense").unwrap();
        let noise = d.join("noise.wav");
        let mut x = 1u32;
        let hiss: Vec<f32> = (0..24_000)
            .map(|_| {
                x ^= x << 13;
                x ^= x >> 17;
                x ^= x << 5;
                x as f32 / u32::MAX as f32 - 0.5
            })
            .collect();
        wav(&noise, 48_000, vec![hiss]);
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        e.execute("track.new", &json!({"kind": "instrument", "format": "stereo"})).unwrap();
        let other = e.session().tracks.last().unwrap().id;
        e.execute("track.new", &json!({"kind": "audio", "format": "mono"})).unwrap();
        let audio = e.session().tracks.last().unwrap().id;
        let before = e.session().clone();
        let p = |extra: Value| {
            let mut v = json!({"track": t.0, "path": ok.to_string_lossy()});
            for (k, x) in extra.as_object().unwrap() {
                v[k] = x.clone();
            }
            v
        };
        for (params, why) in [
            (json!({"track": other.0, "path": ok.to_string_lossy()}), "another instrument"),
            (json!({"track": audio.0, "path": ok.to_string_lossy()}), "not an instrument track"),
            (json!({"track": t.0}), "no path"),
            (p(json!({"path": d.join("missing.wav").to_string_lossy()})), "missing file"),
            (p(json!({"path": junk.to_string_lossy()})), "not audio"),
            (p(json!({"path": silent.to_string_lossy()})), "silence"),
            (p(json!({"path": dc.to_string_lossy()})), "a constant level"),
            (p(json!({"path": silent.to_string_lossy(), "mode": "pitch"})), "no pitch"),
            (p(json!({"path": noise.to_string_lossy(), "mode": "pitch"})), "noise has no pitch"),
            (p(json!({"mode": "granular"})), "unknown mode"),
            (p(json!({"mode": 3})), "mode not text"),
            (p(json!({"frame_size": 300})), "frame size"),
            (p(json!({"frame_size": -1})), "negative frame size"),
            (p(json!({"waves": 0})), "no waves"),
            (p(json!({"waves": 33})), "too many waves"),
            (p(json!({"name": 5})), "name not text"),
        ] {
            let r = e.execute("mix.wavetable_import", &params);
            assert!(r.is_err(), "{why}: {r:?}");
            eprintln!("{why}: {}", r.unwrap_err());
            assert_eq!(*e.session(), before, "{why}");
        }
        assert!(e.execute("mix.wavetable_preview", &p(json!({"points": 5}))).is_err());
        assert!(e.execute("mix.wavetable_save", &json!({"track": t.0})).is_err());
        // Noise in spectral mode is fine.
        let r = e.execute("mix.wavetable_import", &p(json!({"path": noise.to_string_lossy(), "mode": "spectral"}))).unwrap();
        assert_eq!(r["waves"], 32);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_file_too_long_is_refused_and_a_long_one_cut_with_a_note() {
        let d = dir("long");
        let long = d.join("long.wav");
        // 90 seconds at 8 kHz: read for its first minute.
        wav(&long, 8_000, vec![tone(200.0, 8_000, 90.0)]);
        let mut e = Engine::default();
        let t = synth_track(&mut e);
        let r = e.execute("mix.wavetable_import", &json!({"track": t.0, "path": long.to_string_lossy(), "mode": "spectral", "waves": 2})).unwrap();
        assert!(r["notes"].as_array().unwrap().iter().any(|n| n.as_str().unwrap().contains("first 60 seconds")), "{r}");
        // More than ten minutes (here of silence at 8 kHz, 16-bit) is refused, from its header.
        let too = d.join("too_long.wav");
        {
            use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
            let buf = AudioBuffer { sample_rate: 8_000, channels: vec![vec![0.0; 8_000 * 601]] };
            let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None, bitrate_kbps: 0 };
            std::fs::write(&too, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        }
        let r = e.execute("mix.wavetable_import", &json!({"track": t.0, "path": too.to_string_lossy()}));
        assert!(r.as_ref().is_err_and(|e| e.to_string().contains("too long")), "{r:?}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

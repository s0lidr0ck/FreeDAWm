//! Module assets: files a plugin works from (FireStation Amp's cabinet impulse response per amp).
//! `mix.module_ir` reads and decodes the file here, on the engine's thread, makes it ready for the
//! plugin at the session rate (`ImpulseAsset`: the response trimmed, resampled, normalised and
//! partitioned) and keeps it in `Session::assets`; the module stores only the reference
//! (`Insert::assets`: `bundled:<name>` or the file's path), saved with the session and made ready
//! again when it opens. The mix engine hands the prepared asset to the plugin; the audio thread
//! only swaps it in.
//!
//! `mix.module_reference` does the same for reference audio (FireStation LevelUp's reference
//! tracks): a whole file decoded and resampled to the session rate, kept under the slot's key
//! (`ref0`..`ref3`, [`soundcraft_model::REFERENCE_KEYS`]) apart from prepared assets
//! (`AssetPool::insert_for`). The plugin reads it at the transport position (`Transport::samples`).

use super::*;
use crate::cmd;
use firestation::amp::amp_engine_dsp::{Role, param as amp_param_index};
use firestation::amp::{AMP_NAMES, AMP_PARAMS, IR_KEYS, ImpulseAsset, NUM_AMPS};
use serde_json::json;
use soundcraft_dsp::SampleSource;
use soundcraft_dsp::plugins::fs as firestation;
use soundcraft_model::{AssetData, Insert, ModuleId, REFERENCE_KEYS, Session, is_reference_key};
use std::path::Path;
use std::sync::Arc;

/// Prefix of a reference to an IR bundled with SoundCraft.
pub const BUNDLED: &str = "bundled:";
/// The largest IR file read (a few seconds of 192 kHz stereo float is under 10 MB).
const MAX_IR_FILE_BYTES: u64 = 32 << 20;
/// The largest reference file read (a long mix as 32-bit float WAV at 96 kHz fits).
const MAX_REFERENCE_FILE_BYTES: u64 = 2 << 30;
/// The most samples (frames x channels) a reference holds at the session rate: 30 minutes of
/// stereo at 96 kHz.
const MAX_REFERENCE_SAMPLES: usize = 30 * 60 * 96_000 * 2;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.module_reference",
            "Load Module Reference",
            [],
            None,
            "{track?, module: id, slot: 0..3, file: path | null}: reference audio for a module whose plugin has reference slots (FireStation LevelUp), read, decoded and resampled to the session rate here (never on the audio thread); the plugin plays it at the transport position. null clears the slot. A missing, corrupt or too-long file is an error and changes nothing → {track, module, slot, key, file, frames, channels, sample_rate}",
            has_selection,
            module_reference
        ),
        cmd!(
            query "mix.module_action",
            "Module Action",
            [],
            None,
            "{track?, module: id, action: name}: a one-off action on the module's live plugin, outside the session (not undoable, nothing saved): FireStation LevelUp's `reset_delivery` (its Deliver tab's Reset: integrated loudness and the true-peak hold start again). The app sends it to the playing instance → {track, module, action}",
            has_selection,
            module_action
        ),
        cmd!(
            "mix.module_ir",
            "Load Module IR",
            [],
            None,
            "{track?, module: id, ir: bundled name (\"Fender/….wav\") | {path} | null, amp?: index or name (default: the module's current amp)}: the Amp module's cabinet impulse response for that amp, read and made ready here (resampled to the session rate); loading one sets that amp's Cabinet to IR, as the engine's editor does; null clears it → {track, module, amp, key, ir, frames, sample_rate}",
            has_selection,
            module_ir
        ),
    ]
}

fn module_reference(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_reference";
    let t = super::module::first_track(e, C, p)?;
    let id = super::module::module_id(C, p)?;
    let ins = e.session().track(t).and_then(|tr| tr.mixer.module(id)).cloned().ok_or_else(|| bad(C, format!("no module {}", id.0)))?;
    let slot = p
        .get("slot")
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
        .filter(|n| *n < REFERENCE_KEYS.len())
        .ok_or_else(|| bad(C, "`slot` must be 0..3"))?;
    let key = REFERENCE_KEYS[slot];
    let file = match p.get("file") {
        None => return Err(bad(C, "`file` required: a file path, or null to clear the slot")),
        Some(Value::Null) => None,
        Some(Value::String(path)) if !path.is_empty() => Some(path.as_str()),
        Some(Value::Object(o)) => {
            Some(o.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "`file.path` must be a file path"))?)
        }
        Some(_) => return Err(bad(C, "`file` must be a file path or null")),
    };
    // A plugin this build doesn't know (a missing third-party one) can still have a slot cleared.
    if file.is_some() {
        let slots = super::clap::plugin_info(&ins.plugin).map_or(0, |i| usize::from(i.references));
        if slot >= slots {
            return Err(bad(C, format!("`{}` has {slots} reference slot(s); no slot {slot}", ins.plugin)));
        }
    }
    let reference = file.map(|f| std::path::absolute(f).map_or_else(|_| f.to_string(), |p| p.display().to_string()));
    let rate = e.session().sample_rate.hz();
    // Read and make it ready before touching the session: a bad file changes nothing.
    let made = match &reference {
        Some(r) => Some(make_asset(key, r, rate, None).map_err(|err| EngineError::Io(format!("{C}: {err}")))?),
        None => None,
    };
    let (frames, channels, sample_rate) = made.as_ref().map_or((0, 0, 0.0), |a| (a.frames(), a.num_channels(), a.sample_rate()));
    let s = e.session_mut();
    if let (Some(r), Some(asset)) = (&reference, made) {
        s.assets.insert_for(key, r, rate, AssetData(Arc::new(asset)));
    }
    let ins = super::module::find_mut(e, C, t, id)?;
    match &reference {
        Some(r) => ins.assets.insert(key.to_string(), r.clone()),
        None => ins.assets.remove(key),
    };
    Ok(
        json!({"track": t, "module": id.0, "slot": slot, "key": key, "file": reference, "frames": frames, "channels": channels, "sample_rate": sample_rate}),
    )
}

fn module_action(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_action";
    let t = super::module::first_track(e, C, p)?;
    let id = super::module::module_id(C, p)?;
    let ins = e.session().track(t).and_then(|tr| tr.mixer.module(id)).ok_or_else(|| bad(C, format!("no module {}", id.0)))?;
    let action = p.get("action").and_then(Value::as_str).ok_or_else(|| bad(C, "`action` required"))?;
    let known = soundcraft_dsp::plugin_actions(&ins.plugin);
    if !known.contains(&action) {
        return Err(bad(
            C,
            format!("`{}` has no action `{action}` (it has: {})", ins.plugin, if known.is_empty() { "none".to_string() } else { known.join(", ") }),
        ));
    }
    Ok(json!({"track": t, "module": id.0, "action": action}))
}

fn module_ir(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_ir";
    let t = super::module::first_track(e, C, p)?;
    let id = p
        .get("module")
        .and_then(Value::as_u64)
        .and_then(|n| u32::try_from(n).ok())
        .map(ModuleId)
        .ok_or_else(|| bad(C, "`module` must be a module id"))?;
    let ins = e.session().track(t).and_then(|tr| tr.mixer.module(id)).cloned().ok_or_else(|| bad(C, format!("no module {}", id.0)))?;
    if ins.plugin != "fs_amp" {
        return Err(bad(C, format!("`{}` takes no impulse response", ins.plugin)));
    }
    let amp = amp_param(C, &ins, p.get("amp"))?;
    let key = IR_KEYS[amp];
    let rate = e.session().sample_rate.hz();
    let reference = match p.get("ir") {
        None => return Err(bad(C, "`ir` required: a bundled IR name, {path} or null")),
        Some(Value::Null) => None,
        Some(Value::String(name)) => Some(format!("{BUNDLED}{name}")),
        Some(Value::Object(o)) => {
            let path = o.get("path").and_then(Value::as_str).filter(|s| !s.is_empty()).ok_or_else(|| bad(C, "`ir.path` must be a file path"))?;
            Some(std::path::absolute(path).map_or_else(|_| path.to_string(), |p| p.display().to_string()))
        }
        Some(_) => return Err(bad(C, "`ir` must be a bundled IR name, {path} or null")),
    };
    // Read and make it ready before touching the session: a bad file changes nothing.
    let made = match &reference {
        Some(r) => Some(make_asset(key, r, rate, None).map_err(|err| EngineError::Io(format!("{C}: {err}")))?),
        None => None,
    };
    let s = e.session_mut();
    let (frames, sample_rate) = made.as_ref().map_or((0, 0.0), |a| (a.frames(), a.sample_rate()));
    if let (Some(r), Some(asset)) = (&reference, made) {
        s.assets.insert(r, rate, AssetData(Arc::new(asset)));
    }
    let tr = s.track_mut(t).ok_or_else(|| bad(C, "no track"))?;
    let ins = tr.mixer.module_mut(id).ok_or_else(|| bad(C, format!("no module {}", id.0)))?;
    match &reference {
        Some(r) => {
            ins.assets.insert(key.to_string(), r.clone());
            // the editor's Load IR picks the IR cabinet for that amp
            let cab = amp_param_index(amp, Role::Cab);
            if let Some(cab) = cab.and_then(|i| AMP_PARAMS.get(i)) {
                ins.params.insert(cab.id.to_string(), 1.0);
            }
        }
        None => {
            ins.assets.remove(key);
        }
    }
    Ok(json!({"track": t, "module": id.0, "amp": amp, "key": key, "ir": reference, "frames": frames, "sample_rate": sample_rate}))
}

/// The amp an `amp` parameter names (an index or a name), else the module's current amp.
fn amp_param(cmd: &str, ins: &Insert, v: Option<&Value>) -> Result<usize> {
    let current = || ins.params.get("amp").map_or(0, |a| if a.is_finite() { a.round().clamp(0.0, (NUM_AMPS - 1) as f32) as usize } else { 0 });
    match v {
        None | Some(Value::Null) => Ok(current()),
        Some(Value::Number(n)) => {
            n.as_u64().and_then(|i| usize::try_from(i).ok()).filter(|i| *i < NUM_AMPS).ok_or_else(|| bad(cmd, "`amp` must be 0..6"))
        }
        Some(Value::String(name)) => AMP_NAMES
            .iter()
            .position(|n| n.eq_ignore_ascii_case(name))
            .ok_or_else(|| bad(cmd, format!("no amp `{name}` (one of {})", AMP_NAMES.join(", ")))),
        Some(_) => Err(bad(cmd, "`amp` must be an index or a name")),
    }
}

/// The bytes a reference names: a bundled IR, else a file (read only up to a sane size). A path
/// stored in a session (`session_file`) is resolved and checked as the session's other media are.
fn read_reference(reference: &str, session_file: Option<&Path>, max_bytes: u64) -> std::result::Result<(Vec<u8>, Option<String>), String> {
    if let Some(name) = reference.strip_prefix(BUNDLED) {
        let bytes = crate::amp_irs::bundled_ir(name).ok_or_else(|| format!("no bundled IR `{name}`"))?;
        return Ok((bytes.to_vec(), Some("wav".to_string())));
    }
    let path = match session_file {
        Some(session) => crate::io::stored_path(reference, session)?,
        None => std::path::PathBuf::from(reference),
    };
    crate::file_access::check_read(&path)?;
    let bytes = crate::io::read_file_max(&path, max_bytes).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    Ok((bytes, ext))
}

/// Reads, decodes and makes ready the asset a reference names for module asset key `key`, for a
/// session at `rate`: reference audio for a reference slot, else an impulse response. Errors name
/// what is wrong (missing, not audio, too long, unusable).
fn make_asset(key: &str, reference: &str, rate: u32, session_file: Option<&Path>) -> std::result::Result<Arc<dyn SampleSource>, String> {
    if is_reference_key(key) {
        return make_reference_audio(reference, rate, session_file);
    }
    let (bytes, ext) = read_reference(reference, session_file, MAX_IR_FILE_BYTES)?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| format!("{reference}: {err}"))?;
    let first = buf.channels.first().ok_or_else(|| format!("{reference}: no audio"))?;
    if first.is_empty() {
        return Err(format!("{reference}: no audio"));
    }
    let ir = ImpulseAsset::new(first, f64::from(info.sample_rate), f64::from(rate)).map_err(|err| format!("{reference}: {err}"))?;
    Ok(Arc::new(ir))
}

/// A reference audio file, whole, at the session rate (every channel; at most
/// `MAX_REFERENCE_SAMPLES` once converted).
fn make_reference_audio(reference: &str, rate: u32, session_file: Option<&Path>) -> std::result::Result<Arc<dyn SampleSource>, String> {
    if reference.starts_with(BUNDLED) {
        return Err(format!("{reference}: no bundled reference audio"));
    }
    let (bytes, ext) = read_reference(reference, session_file, MAX_REFERENCE_FILE_BYTES)?;
    // The header says how long it is: refuse one too long before decoding it all (a compressed
    // file, or one at a very low rate, can be small on disk and huge decoded).
    let too_long = || format!("{reference}: too long for a reference (at most 30 minutes of stereo at 96 kHz)");
    if let Ok(info) = soundcraft_audio_io::probe(&bytes, ext.as_deref()) {
        let samples = info.frames as f64 * f64::from(info.channels);
        let at_rate = samples * f64::from(rate) / f64::from(info.sample_rate.max(1));
        if samples.max(at_rate) > MAX_REFERENCE_SAMPLES as f64 {
            return Err(too_long());
        }
    }
    let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| format!("{reference}: {err}"))?;
    drop(bytes);
    if buf.frames() == 0 || buf.channels.is_empty() {
        return Err(format!("{reference}: no audio"));
    }
    let buf = crate::io::at_rate(buf, rate, MAX_REFERENCE_SAMPLES).ok_or_else(too_long)?;
    if buf.channels.iter().flatten().any(|v| !v.is_finite()) {
        return Err(format!("{reference}: the audio holds non-finite samples"));
    }
    // LevelUp's reference job (the only plugin with reference slots): at most 8 minutes, measured
    // here (its integrated loudness, which it matches to the mix), and refused when too short or
    // too quiet to measure, as the C++ loader refuses it.
    let r = firestation::levelup::ReferenceAudio::new(rate as f32, buf.channels).map_err(|err| format!("{reference}: {err}"))?;
    Ok(Arc::new(r))
}

/// Makes ready every module asset a session names that it does not have at its rate (a session
/// just opened). Returns what could not be made, for the "missing media" report: the module keeps
/// its reference (so the name shows what is missing) and plays without it.
pub fn load_session_assets(s: &mut Session, session_file: Option<&Path>) -> Vec<String> {
    let wanted: Vec<(String, String)> =
        s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).flat_map(|i| i.assets.iter().map(|(k, r)| (k.clone(), r.clone()))).collect();
    load_assets(s, wanted, session_file)
}

/// A session file opened as a new, folderless session (a template): its modules' file
/// references, and its audio sources' paths, are made absolute (they may be relative to the file),
/// as [`import_track_assets`] does for imported tracks. A reference that doesn't resolve stays as
/// it is: loading the file has already reported it missing.
pub(crate) fn absolutize_paths(s: &mut Session, file: &Path) {
    for t in &mut s.tracks {
        for ins in &mut t.mixer.inserts {
            for r in ins.assets.values_mut() {
                if !r.starts_with(BUNDLED)
                    && let Ok(abs) = crate::io::stored_path(r, file)
                {
                    *r = abs.to_string_lossy().into_owned();
                }
            }
        }
    }
    for src in &mut s.sources {
        if let Ok(abs) = crate::io::stored_path(&src.path, file) {
            src.path = abs.to_string_lossy().into_owned();
        }
    }
}

/// Tracks brought in from another session file (`donor`): their modules' file references are
/// made absolute (they were relative to the donor), then their assets are made ready. Returns
/// what could not be, for the "missing media" report.
pub(crate) fn import_track_assets(s: &mut Session, tracks: &[soundcraft_model::TrackId], donor: &Path) -> Vec<String> {
    let mut wanted = Vec::new();
    let mut missing = Vec::new();
    for t in s.tracks.iter_mut().filter(|t| tracks.contains(&t.id)) {
        for ins in &mut t.mixer.inserts {
            for (k, r) in &mut ins.assets {
                if !r.starts_with(BUNDLED) {
                    match crate::io::stored_path(r, donor) {
                        Ok(abs) => *r = abs.to_string_lossy().into_owned(),
                        Err(why) => {
                            missing.push(format!("{r}: {why}"));
                            continue;
                        }
                    }
                }
                wanted.push((k.clone(), r.clone()));
            }
        }
    }
    missing.extend(load_assets(s, wanted, None));
    missing
}

fn load_assets(s: &mut Session, wanted: Vec<(String, String)>, session_file: Option<&Path>) -> Vec<String> {
    let rate = s.sample_rate.hz();
    let mut missing = Vec::new();
    // (kind, reference) pairs that failed: one report each, however many modules name them
    let mut failed: Vec<(bool, String)> = Vec::new();
    for (k, r) in wanted {
        let kind = is_reference_key(&k);
        if s.assets.contains_for(&k, &r, rate) || failed.iter().any(|(fk, fr)| *fk == kind && *fr == r) {
            continue;
        }
        match make_asset(&k, &r, rate, session_file) {
            Ok(a) => s.assets.insert_for(&k, &r, rate, AssetData(Arc::new(a))),
            Err(err) => {
                missing.push(if err.starts_with(&r) { err } else { format!("{r}: {err}") });
                failed.push((kind, r));
            }
        }
    }
    missing
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
    use soundcraft_model::TrackId;
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-module-ir-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// Writes a generated decaying response (no recorded audio) as a WAV.
    fn ir_wav(path: &std::path::Path, rate: u32, secs: f32) {
        let n = (rate as f32 * secs) as usize;
        let one: Vec<f32> = (1..=n).map(|i| 0.6 * (i as f32 * 0.3).sin() * (-(i as f32) / (rate as f32 * 0.02)).exp()).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![one] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    }

    fn amp_track(e: &mut Engine) -> (TrackId, u64) {
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_amp"})).unwrap()["module"].as_u64().unwrap();
        (t, m)
    }

    fn render(e: &Engine) -> Vec<Vec<f32>> {
        let s = e.session();
        let range = soundcraft_time::Range::new(0, 24_000);
        soundcraft_mix::render_range(s, range, 512)
    }

    #[test]
    fn a_bundled_ir_loads_for_the_current_amp_and_sets_its_cabinet() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let name = "Marshall/gb sm57.wav";
        let r = e.execute("mix.module_ir", &json!({"track": t, "module": m, "amp": "JTM45", "ir": name})).unwrap();
        assert_eq!(r["key"], "irPathJtm");
        assert_eq!(r["sample_rate"].as_f64(), Some(48_000.0));
        let ins = e.session().tracks.last().unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap().clone();
        assert_eq!(ins.assets["irPathJtm"], format!("bundled:{name}"));
        assert_eq!(ins.params["jtmCab"], 1.0);
        assert!(e.session().assets.contains(&format!("bundled:{name}"), 48_000));
        // undo puts the reference (and the Cabinet) back
        e.run("edit.undo").unwrap();
        let ins = e.session().tracks.last().unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap().clone();
        assert!(ins.assets.is_empty());
        // clearing removes the reference only
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": name})).unwrap();
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": null})).unwrap();
        let ins = e.session().tracks.last().unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap().clone();
        assert!(ins.assets.is_empty() && ins.params["cab"] == 1.0);
    }

    #[test]
    fn an_ir_file_outside_the_project_is_copied_into_it_on_save() {
        let d = dir("copied");
        std::fs::create_dir_all(d.join("Elsewhere")).unwrap();
        let ir = d.join("Elsewhere").join("cab.wav");
        ir_wav(&ir, 48_000, 0.1);
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let r = e.execute("mix.module_ir", &json!({"track": t, "module": m, "amp": "JTM45", "ir": {"path": ir.to_string_lossy()}})).unwrap();
        let key = r["key"].as_str().unwrap().to_string();
        let path = d.join("Song").join("Song.fdawm");
        e.execute("session.save", &json!({"path": path.to_string_lossy()})).unwrap();
        let module = |e: &Engine| e.session().tracks.last().unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap().clone();
        assert_eq!(module(&e).assets[&key], "Audio Files/cab.wav");
        assert!(d.join("Song").join("Audio Files").join("cab.wav").is_file());
        // Still ready under its new name (nothing reloads), and found again on open.
        assert!(e.session().assets.contains("Audio Files/cab.wav", 48_000));
        std::fs::remove_file(&ir).unwrap();
        let mut r = Engine::default();
        assert_eq!(crate::io::open_session(&mut r, &path.to_string_lossy()).unwrap(), Vec::<String>::new());
        assert!(r.session().assets.contains("Audio Files/cab.wav", 48_000));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn the_ir_reaches_the_sound() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        // something to play: a generated tone clip
        let d = dir("sound");
        let wav = d.join("tone.wav");
        let n = 48_000usize;
        let tone: Vec<f32> = (0..n).map(|i| 0.4 * (i as f32 * 0.03).sin() * (-((i % 6000) as f32) / 3000.0).exp()).collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone.clone(), tone] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        std::fs::write(&wav, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        e.execute("file.import_audio", &json!({"path": wav.display().to_string(), "track": t, "at": 0})).unwrap();
        e.execute("mix.module_params", &json!({"track": t, "module": m, "params": {"cab": 1.0}})).unwrap();
        let builtin = render(&e);
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cap.wav"}))
            .unwrap();
        let ir = render(&e);
        assert!(ir.iter().flatten().all(|v| v.is_finite()));
        let diff: f32 = builtin[0].iter().zip(&ir[0]).map(|(a, b)| (a - b).abs()).sum();
        assert!(diff > 1.0, "the IR cabinet sounds different from the built-in one ({diff})");
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn bad_file_inputs() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let d = dir("bad");
        let before = e.session().clone();
        let fails =
            |e: &mut Engine, ir: serde_json::Value| e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": ir})).unwrap_err().to_string();
        // missing
        assert!(fails(&mut e, json!({"path": d.join("nope.wav").display().to_string()})).contains("nope.wav"));
        // corrupt
        std::fs::write(d.join("junk.wav"), b"RIFF\x10\x00\x00\x00WAVEjunkjunkjunk").unwrap();
        assert!(!fails(&mut e, json!({"path": d.join("junk.wav").display().to_string()})).is_empty());
        std::fs::write(d.join("text.wav"), b"hello").unwrap();
        assert!(!fails(&mut e, json!({"path": d.join("text.wav").display().to_string()})).is_empty());
        // huge: past the size limit, and too long once trimmed
        let big = d.join("big.wav");
        std::fs::write(&big, vec![0u8; (super::MAX_IR_FILE_BYTES + 1) as usize]).unwrap();
        assert!(fails(&mut e, json!({"path": big.display().to_string()})).contains("large"));
        let long = d.join("long.wav");
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.3; 48_000 * 6]] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
        std::fs::write(&long, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        assert!(fails(&mut e, json!({"path": long.display().to_string()})).contains("too long"));
        // an unknown bundled name, a non-amp module, nonsense
        assert!(fails(&mut e, json!("Fender/none.wav")).contains("no bundled IR"));
        assert!(fails(&mut e, json!(42)).contains("`ir`"));
        // nothing changed
        assert_eq!(e.session().tracks, before.tracks);
        assert!(e.session().assets.is_empty());
        // another sample rate: resampled to the session's
        let other = d.join("ir96.wav");
        ir_wav(&other, 96_000, 0.25);
        let r = e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": {"path": other.display().to_string()}})).unwrap();
        assert_eq!(r["sample_rate"].as_f64(), Some(48_000.0));
        let frames = r["frames"].as_u64().unwrap();
        // (0.25 s at 96 kHz, trimmed at -80 dB: about 0.17 s, halved in samples)
        assert!((7_000..=12_000).contains(&frames), "{frames}");
        // and the module keeps working
        assert!(render(&e).iter().flatten().all(|v| v.is_finite()));
        let comp = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        assert!(e.execute("mix.module_ir", &json!({"track": t, "module": comp, "ir": "Marshall/gb sm57.wav"})).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_saved_session_reopens_with_its_irs_and_reports_missing_ones() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let d = dir("reopen");
        let file_ir = d.join("mine.wav");
        ir_wav(&file_ir, 44_100, 0.2);
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": {"path": file_ir.display().to_string()}})).unwrap();
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "amp": 1, "ir": "Marshall/gb md421.wav"})).unwrap();
        let path = d.join("s.scraft");
        crate::io::save_session(&mut e, &path.display().to_string()).unwrap();

        let mut f = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_session(&mut f, &path.display().to_string()).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(f.session().assets.len(), 2);
        // the user's file is gone: the bundled one still loads, the other is reported
        std::fs::remove_file(&file_ir).unwrap();
        let mut g = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_session(&mut g, &path.display().to_string()).unwrap();
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("mine.wav"));
        assert_eq!(g.session().assets.len(), 1);
        assert!(render(&g).iter().flatten().all(|v| v.is_finite()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn imported_tracks_bring_their_irs_or_report_them_missing() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let d = dir("import");
        let file_ir = d.join("mine.wav");
        ir_wav(&file_ir, 48_000, 0.2);
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": {"path": file_ir.display().to_string()}})).unwrap();
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "amp": 1, "ir": "Marshall/gb md421.wav"})).unwrap();
        let path = d.join("donor.scraft");
        crate::io::save_session(&mut e, &path.display().to_string()).unwrap();

        let mut f = Engine::new(soundcraft_model::Session::default());
        let r = f.execute("file.import_session_data", &json!({"path": path.display().to_string()})).unwrap();
        assert!(r["missing"].as_array().unwrap().is_empty(), "{r}");
        assert_eq!(f.session().assets.len(), 2);
        let s = f.session();
        let refs: Vec<&String> = s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).flat_map(|i| i.assets.values()).collect();
        assert!(refs.iter().all(|r| s.assets.contains(r, s.sample_rate.hz())), "{refs:?}");
        assert!(render(&f).iter().flatten().all(|v| v.is_finite()));

        std::fs::remove_file(&file_ir).unwrap();
        let mut g = Engine::new(soundcraft_model::Session::default());
        let r = g.execute("file.import_session_data", &json!({"path": path.display().to_string()})).unwrap();
        let missing = r["missing"].as_array().unwrap();
        assert_eq!(missing.len(), 1, "{r}");
        assert!(missing[0].as_str().unwrap().contains("mine.wav"));
        assert_eq!(g.session().assets.len(), 1);
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_template_keeps_its_irs_with_absolute_paths_or_reports_them_missing() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = amp_track(&mut e);
        let d = dir("template");
        let file_ir = d.join("mine.wav");
        ir_wav(&file_ir, 48_000, 0.2);
        e.execute("mix.module_ir", &json!({"track": t, "module": m, "ir": {"path": file_ir.display().to_string()}})).unwrap();
        // A session file used as a template (saved sessions store paths relative to themselves).
        let path = d.join("tpl.scraft");
        crate::io::save_session(&mut e, &path.display().to_string()).unwrap();
        // The IR named relative to the template file (as a template copied with its files has it).
        let text = std::fs::read_to_string(&path).unwrap();
        let abs = serde_json::to_string(&file_ir.display().to_string()).unwrap();
        let rel = text.replace(&abs, "\"mine.wav\"");
        assert_ne!(rel, text, "the IR was stored by its full path");
        std::fs::write(&path, rel).unwrap();

        let mut f = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_template(&mut f, &path.display().to_string(), "New").unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        let s = f.session();
        let refs: Vec<&String> = s.tracks.iter().flat_map(|t| t.mixer.inserts.iter()).flat_map(|i| i.assets.values()).collect();
        assert!(!refs.is_empty());
        for r in &refs {
            assert!(r.starts_with(super::BUNDLED) || std::path::Path::new(r.as_str()).is_absolute(), "{r} stays relative with no folder");
            assert!(s.assets.contains(r, s.sample_rate.hz()), "{r}");
        }

        // The IR moved away: the template still opens, and says what is missing.
        std::fs::rename(&file_ir, d.join("moved.wav")).unwrap();
        let mut g = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_template(&mut g, &path.display().to_string(), "New").unwrap();
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("mine.wav"), "{missing:?}");
        assert!(g.path.is_none() && g.session().folder.is_none());
        let _ = std::fs::remove_dir_all(d);
    }

    // ---- reference audio (`mix.module_reference`) ------------------------------------------

    fn ref_track(e: &mut Engine) -> (TrackId, u64) {
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        // the probe plays reference slot 0 at the transport position with Role Off
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "test_link"})).unwrap()["module"].as_u64().unwrap();
        (t, m)
    }

    /// A stereo signal (generated) as a WAV at `rate`.
    fn song_wav(path: &std::path::Path, rate: u32, frames: usize, bits: BitDepth) {
        let l: Vec<f32> = (0..frames).map(|i| 0.5 * (i as f32 * 0.002).sin()).collect();
        let r: Vec<f32> = l.iter().map(|x| -x).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![l, r] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: bits, dither: false, bwf: None };
        std::fs::write(path, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
    }

    fn refs(e: &Engine, t: TrackId, m: u64) -> std::collections::BTreeMap<String, String> {
        e.session().track(t).unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap().assets.clone()
    }

    #[test]
    fn a_reference_loads_at_the_session_rate_and_plays_at_the_transport_position() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = ref_track(&mut e);
        let d = dir("ref-play");
        let wav = d.join("song.wav");
        song_wav(&wav, 48_000, 48_000, BitDepth::Float32);
        let r = e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": wav.display().to_string()})).unwrap();
        assert_eq!((r["key"].as_str(), r["frames"].as_u64(), r["channels"].as_u64()), (Some("ref0"), Some(48_000), Some(2)));
        assert_eq!(refs(&e, t, m)["ref0"], r["file"].as_str().unwrap());
        // kept apart from prepared assets, under the slot's kind
        let file = r["file"].as_str().unwrap().to_string();
        assert!(e.session().assets.contains_for("ref0", &file, 48_000));
        assert!(!e.session().assets.contains(&file, 48_000));
        // the plugin reads it where the transport is: rendering from 10 000 gives the reference
        // from 10 000 (scaled by the strip's pan law), not from 0
        let start = 10_000i64;
        let out = soundcraft_mix::render_range(e.session(), soundcraft_time::Range::new(start, start + 2048), 512);
        let want: Vec<f32> = (0..2048).map(|i| 0.5 * ((start as f32 + i as f32) * 0.002).sin()).collect();
        let k = out[0][100] / want[100];
        assert!(k.is_finite() && k > 0.3, "{k}");
        for i in 0..2048 {
            assert!((out[0][i] - k * want[i]).abs() < 1e-4, "sample {i}: {} vs {}", out[0][i], k * want[i]);
        }
        // undo, redo, clear
        e.run("edit.undo").unwrap();
        assert!(refs(&e, t, m).is_empty());
        e.run("edit.redo").unwrap();
        assert!(refs(&e, t, m).contains_key("ref0"));
        e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": null})).unwrap();
        assert!(refs(&e, t, m).is_empty());
        // the probe has two slots; a plugin without any takes none
        assert!(e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 1, "file": wav.display().to_string()})).is_ok());
        let err = e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 2, "file": wav.display().to_string()})).unwrap_err();
        assert!(err.to_string().contains("no slot 2"), "{err}");
        assert!(e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 4, "file": null})).is_err());
        let comp = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        assert!(e.execute("mix.module_reference", &json!({"track": t, "module": comp, "slot": 0, "file": wav.display().to_string()})).is_err());
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn reference_bad_file_inputs() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = ref_track(&mut e);
        let d = dir("ref-bad");
        let before = e.session().clone();
        let fails = |e: &mut Engine, f: &std::path::Path| {
            e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": f.display().to_string()})).unwrap_err().to_string()
        };
        // missing
        assert!(fails(&mut e, &d.join("nope.wav")).contains("nope.wav"));
        // corrupt, not audio, empty
        std::fs::write(d.join("junk.wav"), b"RIFF\x10\x00\x00\x00WAVEjunkjunkjunk").unwrap();
        assert!(!fails(&mut e, &d.join("junk.wav")).is_empty());
        std::fs::write(d.join("text.wav"), b"hello").unwrap();
        assert!(!fails(&mut e, &d.join("text.wav")).is_empty());
        std::fs::write(d.join("empty.wav"), b"").unwrap();
        assert!(!fails(&mut e, &d.join("empty.wav")).is_empty());
        // huge: a small file at a very low rate that would be far too long at the session rate
        let huge = d.join("huge.wav");
        song_wav(&huge, 100, 800_000, BitDepth::Int16);
        let err = fails(&mut e, &huge);
        assert!(err.contains("too long"), "{err}");
        // LevelUp's loader refuses a reference too short to use or too quiet to measure (a tenth
        // of a second is under the 400 ms its integrated loudness needs)
        let short = d.join("short.wav");
        song_wav(&short, 48_000, 900, BitDepth::Int16);
        assert!(fails(&mut e, &short).contains("too short"));
        let tenth = d.join("tenth.wav");
        song_wav(&tenth, 48_000, 4_800, BitDepth::Int16);
        assert!(fails(&mut e, &tenth).contains("too quiet"));
        let silent = d.join("silent.wav");
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.0; 48_000]; 2] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None };
        std::fs::write(&silent, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        assert!(fails(&mut e, &silent).contains("silent"));
        // non-finite samples
        let nan = d.join("nan.wav");
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.1, f32::NAN, 0.2, f32::INFINITY]] };
        let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
        std::fs::write(&nan, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();
        // (the decoder keeps such a file playable: what loads holds only finite samples)
        if let Ok(r) = e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": nan.display().to_string()})) {
            let file = r["file"].as_str().unwrap();
            let a = e
                .session()
                .assets
                .get_for("ref0", file, 48_000)
                .unwrap()
                .0
                .downcast_ref::<std::sync::Arc<dyn soundcraft_dsp::SampleSource>>()
                .unwrap()
                .clone();
            assert!((0..a.num_channels()).filter_map(|c| a.channel(c)).flatten().all(|v| v.is_finite()));
            e.run("edit.undo").unwrap();
        }
        // a bundled name is no reference
        let err =
            e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": "bundled:Marshall/gb sm57.wav"})).unwrap_err();
        assert!(!err.to_string().is_empty());
        // nonsense parameters
        for p in [
            json!({"track": t, "module": m, "slot": 0}),
            json!({"track": t, "module": m, "slot": "a", "file": null}),
            json!({"track": t, "module": m, "slot": 0, "file": 5}),
        ] {
            assert!(e.execute("mix.module_reference", &p).is_err(), "{p}");
        }
        // nothing changed
        assert_eq!(e.session().tracks, before.tracks);
        assert!(e.session().assets.is_empty());
        // another rate: resampled to the session's
        let other = d.join("song96.wav");
        song_wav(&other, 96_000, 48_000, BitDepth::Int24);
        let r = e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": other.display().to_string()})).unwrap();
        assert_eq!(r["sample_rate"].as_f64(), Some(48_000.0));
        assert!((23_990..=24_010).contains(&r["frames"].as_u64().unwrap()), "{r}");
        assert!(render(&e).iter().flatten().all(|v| v.is_finite()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_saved_session_reopens_with_its_references_and_reports_missing_ones() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = ref_track(&mut e);
        let d = dir("ref-reopen");
        let (one, two) = (d.join("one.wav"), d.join("two.wav"));
        song_wav(&one, 44_100, 22_050, BitDepth::Int16);
        song_wav(&two, 48_000, 24_000, BitDepth::Int16);
        e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": one.display().to_string()})).unwrap();
        e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 1, "file": two.display().to_string()})).unwrap();
        // the same file as an Amp IR is a separate, prepared asset
        let amp = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_amp"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_ir", &json!({"track": t, "module": amp, "ir": {"path": two.display().to_string()}})).unwrap();
        assert_eq!(e.session().assets.len(), 3);
        let path = d.join("s.scraft");
        crate::io::save_session(&mut e, &path.display().to_string()).unwrap();

        let mut f = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_session(&mut f, &path.display().to_string()).unwrap();
        assert!(missing.is_empty(), "{missing:?}");
        assert_eq!(f.session().assets.len(), 3);
        let s = f.session();
        let ins = s.track(t).unwrap().mixer.module(soundcraft_model::ModuleId(m as u32)).unwrap();
        assert!(ins.assets.iter().all(|(k, r)| s.assets.contains_for(k, r, 48_000)));
        std::fs::remove_file(&one).unwrap();
        let mut g = Engine::new(soundcraft_model::Session::default());
        let missing = crate::io::open_session(&mut g, &path.display().to_string()).unwrap();
        assert_eq!(missing.len(), 1, "{missing:?}");
        assert!(missing[0].contains("one.wav"));
        assert!(render(&g).iter().flatten().all(|v| v.is_finite()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn a_sample_rate_change_remakes_irs_and_references_and_reports_missing_ones() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let (t, m) = ref_track(&mut e);
        let d = dir("ref-rate");
        let (song, ir) = (d.join("song.wav"), d.join("ir.wav"));
        song_wav(&song, 48_000, 24_000, BitDepth::Int16);
        ir_wav(&ir, 48_000, 0.2);
        e.execute("mix.module_reference", &json!({"track": t, "module": m, "slot": 0, "file": song.display().to_string()})).unwrap();
        // a second module naming the same file in a reference slot, and the same file as an IR
        let m2 = e.execute("mix.module_add", &json!({"track": t, "plugin": "test_link"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_reference", &json!({"track": t, "module": m2, "slot": 1, "file": song.display().to_string()})).unwrap();
        let amp = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_amp"})).unwrap()["module"].as_u64().unwrap();
        e.execute("mix.module_ir", &json!({"track": t, "module": amp, "ir": {"path": song.display().to_string()}})).unwrap();
        // the rate changes: everything is remade at 96 kHz (the reference twice as long)
        e.session_mut().sample_rate = soundcraft_time::SampleRate::new(96_000).unwrap();
        assert!(e.remake_assets_at_rate().is_empty());
        let file = refs(&e, t, m)["ref0"].clone();
        let a = e
            .session()
            .assets
            .get_for("ref0", &file, 96_000)
            .unwrap()
            .0
            .downcast_ref::<std::sync::Arc<dyn soundcraft_dsp::SampleSource>>()
            .unwrap()
            .clone();
        assert_eq!((a.frames(), a.sample_rate()), (48_000, 96_000.0));
        assert!(e.session().assets.contains_for("irPathTwin", &file, 96_000));
        assert!(render(&e).iter().flatten().all(|v| v.is_finite()));
        // the file goes, the rate changes again: one report per kind (reference, IR), not per module
        std::fs::remove_file(&song).unwrap();
        e.session_mut().sample_rate = soundcraft_time::SampleRate::new(44_100).unwrap();
        let missing = e.remake_assets_at_rate();
        assert_eq!(missing.len(), 2, "{missing:?}");
        assert!(missing.iter().all(|x| x.contains("song.wav")));
        assert!(render(&e).iter().flatten().all(|v| v.is_finite()));
        let _ = std::fs::remove_dir_all(d);
    }

    #[test]
    fn module_action_names_a_known_action_and_changes_nothing() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_levelup"})).unwrap()["module"].as_u64().unwrap();
        let rev = e.revision;
        let before = e.session().tracks.clone();
        let r = e.execute("mix.module_action", &json!({"track": t, "module": m, "action": "reset_delivery"})).unwrap();
        assert_eq!(r["action"], "reset_delivery");
        assert_eq!(e.session().tracks, before);
        assert_eq!(e.revision, rev, "not a document edit");
        assert!(e.execute("mix.module_action", &json!({"track": t, "module": m, "action": "nope"})).is_err());
        assert!(e.execute("mix.module_action", &json!({"track": t, "module": m})).is_err());
        assert!(e.execute("mix.module_action", &json!({"track": t, "module": 999, "action": "reset_delivery"})).is_err());
        let comp = e.execute("mix.module_add", &json!({"track": t, "plugin": "compressor"})).unwrap()["module"].as_u64().unwrap();
        let err = e.execute("mix.module_action", &json!({"track": t, "module": comp, "action": "reset_delivery"})).unwrap_err();
        assert!(err.to_string().contains("none"), "{err}");
    }

    #[test]
    fn a_link_role_takes_no_automation_lane() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        e.execute("track.new", &json!({"count": 1, "format": "stereo"})).unwrap();
        let t = e.session().tracks.last().unwrap().id;
        let m = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_levelup"})).unwrap()["module"].as_u64().unwrap();
        let lane = |e: &mut Engine, param: &str| {
            e.execute(
                "automation.write_range",
                &json!({"track": t, "param": format!("plugin:m{m}:{param}"), "start": 0, "end": 48_000, "value": 0.5}),
            )
        };
        let err = lane(&mut e, "link").unwrap_err();
        assert!(err.to_string().contains("not automatable"), "{err}");
        assert!(e.session().track(t).unwrap().automation.is_empty());
        // nor through a view on it and Write to Current
        let view = e.execute("track.view", &json!({"tracks": [t], "view": format!("plugin:m{m}:link")})).unwrap_err();
        assert!(view.to_string().contains("not automatable"), "{view}");
        // (a view left on it by an old session: Write to Current skips it)
        e.session_mut().track_mut(t).unwrap().view = format!("plugin:m{m}:link");
        e.execute("automation.write_to_current", &json!({"tracks": [t], "start": 0, "end": 48_000})).unwrap();
        assert!(e.session().track(t).unwrap().automation.is_empty());
        // its other parameters are, the Pair included
        assert!(lane(&mut e, "linkGroup").is_ok());
        assert!(lane(&mut e, "target").is_ok());
    }
}

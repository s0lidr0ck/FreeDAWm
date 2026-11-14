//! FireStation CopyQ: measuring a profile without a module (`mix.copyq_measure`) and what a
//! CopyQ module matches to (`mix.module_copyq`).
//!
//! A profile is CopyQ's own JSON (`"format": "copyq-profile"`). The profiles a module matches to
//! and its frozen source live in the module's state (`Insert::state`: a `copyq-state`), which is
//! made ready for the audio thread here, off it (`Insert::prepared`), as CleanQ's ceiling is.

use super::module::{find_mut, first_track, module_id};
use super::module_ceiling::prepare_insert;
use super::*;
use crate::cmd;
use serde_json::json;
// (Named without its literal path: `file_access` keeps any `fs` + `::` out of the engine's code.)
use firestation::copyq::measure::{MAX_MEASURE_FRAMES, measure_audio};
use firestation::copyq::profile::{ALL, CopyQState, LOUD, MAX_TARGETS, Profile, QUIET, factory_profiles};
use soundcraft_dsp::plugins::fs as firestation;
use std::path::Path;

/// The plugin these commands are for.
const COPYQ: &str = "fs_copyq";
/// The largest file measured (an hour of 96 kHz stereo 32-bit audio is about 2.8 GB; this is
/// what a long reference mix in any common format stays under).
const MAX_FILE_BYTES: u64 = 1 << 30;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "mix.copyq_measure",
            "Measure CopyQ Profile",
            [],
            None,
            "one of: {path: an audio file} | {from?: track, start?, end?} (the range, default the edit selection, of an audio track, default the first selected one, as its clips play) | {factory: name} (a made-up profile: a straight spectral slope); name?: what to call it, gate?: 6..60 dB (frames this far under the programme level are left out as silence or bleed, default 24) → {profile: a CopyQ profile, name, seconds, stereo, loud: bool, quiet: bool, factory: [names]}. Nothing in the session changes; give the profile to `mix.module_copyq`",
            always,
            measure
        ),
        cmd!(
            "mix.module_copyq",
            "Set CopyQ Match",
            [],
            None,
            "{track?, module: id (a FireStation CopyQ), any of: targets: [{profile, weight?: 0..1 (default 1)}] | null (what it matches to: up to 4 profiles blended by weight; null or [] for none), source: profile | null (its own sound, frozen: used when Follow is off; null to measure it live)} → {targets: [{name, weight}], source: name | null}. What is not given is kept",
            has_selection,
            module_copyq
        ),
    ]
}

/// A profile from a JSON value, or an error naming what was wrong with `what`.
fn profile_param(cmd: &str, v: &Value, what: &str) -> Result<Profile> {
    let p = Profile::from_value(v).ok_or_else(|| bad(cmd, format!("{what} is not a CopyQ profile (as `mix.copyq_measure` returns it)")))?;
    if !p.is_measured() {
        return Err(bad(cmd, format!("{what} was measured from no sound")));
    }
    Ok(p)
}

fn measure(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.copyq_measure";
    let gate = f32_or(p, "gate", 24.0).clamp(6.0, 60.0);
    let factory: Vec<String> = factory_profiles().into_iter().map(|f| f.name).collect();
    let given = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty());
    let profile = if let Some(want) = str_param(p, "factory") {
        factory_profiles()
            .into_iter()
            .find(|f| f.name.eq_ignore_ascii_case(want.trim()))
            .ok_or_else(|| bad(C, format!("no factory profile `{want}` (one of: {})", factory.join(", "))))?
    } else if let Some(path) = str_param(p, "path") {
        let bytes = crate::io::read_file_max(path, MAX_FILE_BYTES)?;
        let file = Path::new(path);
        let ext = file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let shown = file.file_name().and_then(|n| n.to_str()).unwrap_or("file");
        let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{shown}: {err}")))?;
        let stem = file.file_stem().and_then(|n| n.to_str()).unwrap_or("Profile");
        let channels: Vec<&[f32]> = buf.channels.iter().map(Vec::as_slice).collect();
        let prof = measure_audio(&channels, f64::from(buf.sample_rate), 0, MAX_MEASURE_FRAMES, gate, given.unwrap_or(stem));
        if !prof.is_measured() {
            return Err(bad(C, format!("`{shown}` is silent or too short to measure")));
        }
        prof
    } else {
        let s = e.session();
        let audio_kind = |k: soundcraft_model::TrackKind| k.has_playlist() && !k.is_midi();
        let from = match track_param(e, C, p, "from")? {
            Some(f) => f,
            None => e
                .target_tracks()
                .into_iter()
                .find(|x| s.track(*x).is_some_and(|tr| audio_kind(tr.kind)))
                .ok_or_else(|| bad(C, "give `path`, `factory`, or select a range on an audio track (or pass `from`)"))?,
        };
        let tr = s.track(from).ok_or_else(|| bad(C, "no source track"))?;
        if !audio_kind(tr.kind) {
            return Err(bad(C, format!("`{}` has no audio clips to measure", tr.name)));
        }
        let r = range_param(e, C, p)?;
        let start = r.start.max(0);
        let len = r.end.saturating_sub(start);
        if len <= 0 {
            return Err(bad(C, "the selection is empty: select a range to measure"));
        }
        crate::io::refuse_longer(s, C, &tr.name, "measure", len, crate::io::render_limit(tr.channels()))?;
        let audio = soundcraft_mix::render_clips(s, from, soundcraft_time::Range::new(start, start + len));
        let channels: Vec<&[f32]> = audio.iter().map(Vec::as_slice).collect();
        let prof = measure_audio(&channels, s.sample_rate.as_f64(), 0, MAX_MEASURE_FRAMES, gate, given.unwrap_or(&tr.name));
        if !prof.is_measured() {
            return Err(bad(C, format!("the selection on `{}` is silent or too short to measure", tr.name)));
        }
        prof
    };
    let mut profile = profile;
    if let Some(n) = given {
        profile.name = firestation::copyq::profile::clean_name(n);
    }
    Ok(json!({
        "profile": profile.to_value(),
        "name": profile.name,
        "seconds": profile.seconds.get(ALL).copied().unwrap_or(0.0),
        "stereo": profile.stereo,
        "loud": profile.has_level(LOUD),
        "quiet": profile.has_level(QUIET),
        "factory": factory,
    }))
}

fn module_copyq(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.module_copyq";
    let t = first_track(e, C, p)?;
    let id = module_id(C, p)?;
    // Everything is read before anything changes.
    let targets = match p.get("targets") {
        None => None,
        Some(Value::Null) => Some(Vec::new()),
        Some(Value::Array(list)) => {
            if list.len() > MAX_TARGETS {
                return Err(bad(C, format!("at most {MAX_TARGETS} targets")));
            }
            let mut out = Vec::with_capacity(list.len());
            for (i, item) in list.iter().enumerate() {
                let what = format!("target {}", i + 1);
                let prof = profile_param(C, item.get("profile").ok_or_else(|| bad(C, format!("{what} has no `profile`")))?, &what)?;
                let weight = match item.get("weight") {
                    None | Some(Value::Null) => 1.0,
                    Some(w) => w
                        .as_f64()
                        .filter(|w| w.is_finite())
                        .map(|w| w.clamp(0.0, 1.0) as f32)
                        .ok_or_else(|| bad(C, format!("{what}'s `weight` must be a number")))?,
                };
                out.push((weight, prof));
            }
            Some(out)
        }
        Some(_) => return Err(bad(C, "`targets` must be an array of {profile, weight?}, or null")),
    };
    let source = match p.get("source") {
        None => None,
        Some(Value::Null) => Some(None),
        Some(v) => Some(Some(profile_param(C, v, "`source`")?)),
    };
    if targets.is_none() && source.is_none() {
        return Err(bad(C, "give `targets`, `source`, or both"));
    }
    let ins = find_mut(e, C, t, id)?;
    if ins.plugin != COPYQ {
        return Err(bad(C, format!("`{}` matches no profile (a FireStation CopyQ does)", ins.plugin)));
    }
    let mut st = ins.state_bytes().and_then(|b| CopyQState::read(&b)).unwrap_or_default();
    if let Some(list) = targets {
        st.targets = list;
    }
    if let Some(src) = source {
        st.source = src;
    }
    ins.set_state_bytes(&st.write());
    prepare_insert(ins);
    Ok(json!({
        "targets": st.targets.iter().map(|(w, p)| json!({"name": p.name, "weight": w})).collect::<Vec<_>>(),
        "source": st.source.as_ref().map(|p| p.name.clone()),
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use firestation::copyq::profile::PREPARED_LEN;
    use soundcraft_model::{ChannelFormat, Insert, TrackKind};

    fn engine() -> (Engine, u64, u32) {
        let mut e = Engine::default();
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
        let r = e.execute("mix.module_add", &json!({"track": t.0, "plugin": COPYQ})).unwrap();
        (e, t.0, r["module"].as_u64().unwrap() as u32)
    }

    fn ins(e: &Engine) -> &Insert {
        &e.session().tracks[0].mixer.inserts[0]
    }

    fn state(e: &Engine) -> CopyQState {
        CopyQState::read(&ins(e).state_bytes().unwrap()).unwrap()
    }

    fn factory(e: &mut Engine, name: &str) -> Value {
        e.execute("mix.copyq_measure", &json!({"factory": name})).unwrap()["profile"].clone()
    }

    #[test]
    fn factory_profiles_are_measured_by_name() {
        let mut e = Engine::default();
        let r = e.execute("mix.copyq_measure", &json!({"factory": "pink (-3 dB/oct)", "name": "  Mine "})).unwrap();
        assert_eq!(r["name"], "Mine");
        assert_eq!((r["stereo"].as_bool(), r["loud"].as_bool()), (Some(false), Some(false)));
        assert_eq!(r["factory"].as_array().unwrap().len(), 5);
        assert!(Profile::from_value(&r["profile"]).is_some_and(|p| p.name == "Mine" && p.is_measured()));
        let err = e.execute("mix.copyq_measure", &json!({"factory": "nope"})).unwrap_err().to_string();
        assert!(err.contains("Pink (-3 dB/oct)"), "{err}");
        // With nothing to measure it says what it takes.
        let err = e.execute("mix.copyq_measure", &json!({})).unwrap_err().to_string();
        assert!(err.contains("`path`"), "{err}");
        assert!(e.execute("mix.copyq_measure", &json!({"path": "/no/such/file.wav"})).is_err());
        assert!(!e.can_undo(), "measuring changes nothing");
    }

    #[test]
    fn a_file_and_a_range_of_a_track_are_measured() {
        // A second of noise as a WAV file.
        let mut seed = 7u32;
        let noise: Vec<f32> = (0..48_000)
            .map(|_| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                ((seed >> 8) as f32 / (1u32 << 23) as f32 - 1.0) * 0.2
            })
            .collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![noise.clone(), noise.iter().map(|v| -v * 0.5).collect()] };
        let bytes = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).unwrap();
        let dir = std::env::temp_dir().join(format!("copyq-measure-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let file = dir.join("Snare Top.wav");
        std::fs::write(&file, bytes).unwrap();
        let mut e = Engine::default();
        let r = e.execute("mix.copyq_measure", &json!({"path": file.to_str().unwrap()})).unwrap();
        assert_eq!(r["name"], "Snare Top");
        assert_eq!(r["stereo"], true);
        assert!((r["seconds"].as_f64().unwrap() - 0.9).abs() < 0.2, "{}", r["seconds"]);
        // The same file on a track, measured from a range of it.
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Kit"));
        e.execute("file.import_audio", &json!({"path": file.to_str().unwrap(), "track": t.0, "at": 0})).unwrap();
        let from_track = e.execute("mix.copyq_measure", &json!({"from": t.0, "start": 0, "end": 48_000, "name": "Range"})).unwrap();
        assert_eq!(from_track["name"], "Range");
        let (a, b) = (Profile::from_value(&r["profile"]).unwrap(), Profile::from_value(&from_track["profile"]).unwrap());
        let worst = a.curve(ALL, 0).iter().zip(b.curve(ALL, 0)).skip(24).map(|(x, y)| (x - y).abs()).fold(0.0f32, f32::max);
        assert!(worst < 0.5, "the clip measures as its file does: {worst}");
        // An empty range, a silent one and a track with no audio are refused.
        assert!(e.execute("mix.copyq_measure", &json!({"from": t.0, "start": 100, "end": 100})).is_err());
        let err = e.execute("mix.copyq_measure", &json!({"from": t.0, "start": 96_000, "end": 192_000})).unwrap_err().to_string();
        assert!(err.contains("silent"), "{err}");
        let midi = e.session_mut().add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("Keys"));
        assert!(e.execute("mix.copyq_measure", &json!({"from": midi.0, "start": 0, "end": 48_000})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn targets_and_source_are_stored_prepared_and_undoable() {
        let (mut e, t, m) = engine();
        assert!(ins(&e).prepared.is_none());
        let (dark, pink) = (factory(&mut e, "Dark (-6 dB/oct)"), factory(&mut e, "Pink (-3 dB/oct)"));
        let r = e
            .execute("mix.module_copyq", &json!({"track": t, "module": m, "targets": [{"profile": dark, "weight": 0.25}, {"profile": pink}]}))
            .unwrap();
        assert_eq!(r["targets"].as_array().unwrap().len(), 2);
        assert_eq!(r["targets"][0]["name"], "Dark (-6 dB/oct)");
        assert_eq!(r["source"], Value::Null);
        assert_eq!(e.undo_label(), Some("Set CopyQ Match"));
        let st = state(&e);
        assert_eq!((st.targets[0].0, st.targets[1].0), (0.25, 1.0));
        assert_eq!(ins(&e).prepared.as_ref().unwrap().data.len(), PREPARED_LEN, "made ready for the audio thread");
        // The source alone keeps the targets; null takes it away again.
        e.execute("mix.module_copyq", &json!({"track": t, "module": m, "source": pink})).unwrap();
        assert_eq!(state(&e).targets.len(), 2);
        assert_eq!(state(&e).source.unwrap().name, "Pink (-3 dB/oct)");
        let rev = ins(&e).prepared.as_ref().unwrap().rev;
        e.execute("mix.module_copyq", &json!({"track": t, "module": m, "source": null})).unwrap();
        assert!(state(&e).source.is_none() && state(&e).targets.len() == 2);
        assert_ne!(ins(&e).prepared.as_ref().unwrap().rev, rev, "a new preparation each time");
        e.undo();
        assert!(state(&e).source.is_some());
        // No targets.
        e.execute("mix.module_copyq", &json!({"track": t, "module": m, "targets": null})).unwrap();
        assert!(state(&e).targets.is_empty());
        // Refused: too many, not a profile, a weight that is not a number, nothing given, the
        // wrong plugin. Nothing changes.
        let before = ins(&e).state.clone();
        let five: Vec<Value> = (0..5).map(|_| json!({"profile": dark})).collect();
        for bad in [
            json!({"track": t, "module": m, "targets": five}),
            json!({"track": t, "module": m, "targets": [{"profile": {"format": "copyq-profile"}}]}),
            json!({"track": t, "module": m, "targets": [{"weight": 1}]}),
            json!({"track": t, "module": m, "targets": [{"profile": dark, "weight": "x"}]}),
            json!({"track": t, "module": m, "targets": 5}),
            json!({"track": t, "module": m, "source": "x"}),
            json!({"track": t, "module": m}),
            json!({"track": t, "module": 99, "targets": null}),
        ] {
            assert!(e.execute("mix.module_copyq", &bad).is_err(), "{bad}");
        }
        assert_eq!(ins(&e).state, before);
        let other = e.execute("mix.module_add", &json!({"track": t, "plugin": "fs_manners"})).unwrap()["module"].as_u64().unwrap();
        let err = e.execute("mix.module_copyq", &json!({"track": t, "module": other, "targets": null})).unwrap_err().to_string();
        assert!(err.contains("CopyQ"), "{err}");
    }

    #[test]
    fn a_session_opened_with_a_copyq_makes_its_state_ready() {
        let (mut e, t, m) = engine();
        let dark = factory(&mut e, "Dark (-6 dB/oct)");
        e.execute("mix.module_copyq", &json!({"track": t, "module": m, "targets": [{"profile": dark}]})).unwrap();
        let mut s = e.session().clone();
        s.tracks[0].mixer.inserts[0].prepared = None;
        super::super::module_ceiling::prepare_states(&mut s);
        assert_eq!(s.tracks[0].mixer.inserts[0].prepared.as_ref().unwrap().data.len(), PREPARED_LEN);
        // The generic state commands prepare it too.
        let b64 = ins(&e).state.clone().unwrap();
        e.execute("mix.module_copyq", &json!({"track": t, "module": m, "targets": null})).unwrap();
        e.execute("mix.module_state_edit", &json!({"track": t, "module": m, "state": b64})).unwrap();
        assert_eq!(state(&e).targets.len(), 1);
        assert!(ins(&e).prepared.is_some());
    }
}

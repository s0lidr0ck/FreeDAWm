//! `scout.profile_*`: Scout's sound-profile library (spec 2026-10-29-scout-profiles §1). The
//! library lives outside the session (`scout_profiles::default_dir`, or `dir`), so none of these
//! change the session or the undo history.

use super::*;
use crate::cmd;
use crate::scout_profiles as lib;
use serde_json::json;
use soundcraft_model::{ProfileKind, ProfileSource, ScoutProfile};
use soundcraft_time::Range;
use std::path::PathBuf;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "scout.profile_capture", "Capture Sound Profile", [], None,
            "{from: \"strip\"|\"clip\"|\"range\"|\"file\"|\"stems\", name?, role?, tags?: [text], kind?: \"track\"|\"mix\", dir?, save?: bool (default true)}: measures a sound profile and saves it in the library. strip: {capture: id, track: id or name (\"Master\"), position?: \"base\"|\"post\" (default post)}; clip: {clip?: id (default the first selected clip)} as it plays; range: {track?, start?, end? (samples; default the selection)} pre-fader; file: {path} a reference audio file, measured without adding it to the session (kind defaults to mix); stems: {track? | clip?} separates the audio with the session's stem separation (default model) and saves one profile per stem plus one for the mix, named \"<song> · <stem>\". Names are made unique. → {profiles: [profile], paths: [file]}",
            always, capture),
        cmd!(
            query "scout.profile_list",
            "Sound Profiles",
            [],
            None,
            "{query?, role?, kind?, dir?}: the library's profiles matching every word of `query` (name, role, tags, source) and `role` → {dir, profiles: [profile + {file}]}",
            always,
            list
        ),
        cmd!(noundo "scout.profile_rename", "Rename Sound Profile", [], None, "{name, to, dir?}: renames a profile (refused if `to` is taken) → {name}", always, rename),
        cmd!(
            noundo "scout.profile_duplicate",
            "Duplicate Sound Profile",
            [],
            None,
            "{name, to?, dir?}: copies a profile (default name \"<name> copy\", made unique) → {name, file}",
            always,
            duplicate
        ),
        cmd!(noundo "scout.profile_delete", "Delete Sound Profile", [], None, "{name, dir?}: deletes a profile's file → {name}", always, delete),
        cmd!(
            noundo "scout.profile_edit",
            "Edit Sound Profile",
            [],
            None,
            "{name, role?, tags?: [text], kind?: \"track\"|\"mix\", dir?}: changes a profile's role, tags or kind → {profile}",
            always,
            edit
        ),
        cmd!(noundo "scout.profile_export", "Export Sound Profile", [], None, "{name, path, dir?}: writes a profile to a file → {path}", always, export),
        cmd!(
            noundo "scout.profile_import",
            "Import Sound Profile",
            [],
            None,
            "{path, dir?}: adds a profile file to the library (its name made unique) → {name, file}",
            always,
            import
        ),
        cmd!(
            "scout.target_set",
            "Set Scout Target",
            [],
            None,
            "{capture: id, track: id or name (\"Master\"), profile: name | profile object | null, dir?}: gives a captured strip a target sound profile (a copy from the library; a mix target on the master is a mastering target); null clears it. The package then carries the target's numbers and the differences → {capture, track, target: name | null}",
            always,
            target_set
        ),
    ]
}

fn target_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.target_set";
    let cap_id = p.get("capture").and_then(Value::as_u64).ok_or_else(|| bad(C, "`capture` must be a capture id"))?;
    let target: Option<ScoutProfile> = match p.get("profile") {
        None | Some(Value::Null) => None,
        Some(Value::String(name)) => Some(find(C, &dir_param(C, p)?, name)?.1),
        Some(v @ Value::Object(_)) => Some(ScoutProfile::from_json(&v.to_string()).map_err(|m| bad(C, m))?),
        Some(_) => return Err(bad(C, "`profile` must be a profile name, a profile or null")),
    };
    let want = p.get("track").cloned().unwrap_or(Value::Null);
    let s = e.session_mut();
    let c = s.scout.captures.iter_mut().find(|c| u64::from(c.id) == cap_id).ok_or_else(|| bad(C, format!("no capture {cap_id}")))?;
    let only = c.strips.len() == 1;
    let st = c
        .strips
        .iter_mut()
        .find(|st| match &want {
            Value::Number(n) => n.as_u64() == Some(st.track.0),
            Value::String(n) => st.name.eq_ignore_ascii_case(n) || (n.eq_ignore_ascii_case("master") && st.channel_type == "master"),
            _ => only,
        })
        .ok_or_else(|| bad(C, "`track` must name a strip of the capture"))?;
    let name = target.as_ref().map(|t| t.name.clone());
    st.target = target.map(Box::new);
    Ok(json!({"capture": cap_id, "track": st.track.0, "target": name}))
}

fn dir_param(cmd: &str, p: &Value) -> Result<PathBuf> {
    match str_param(p, "dir") {
        Some(d) if !d.trim().is_empty() => Ok(PathBuf::from(d)),
        _ => lib::default_dir().ok_or_else(|| bad(cmd, "no settings folder for the profile library")),
    }
}

fn name_param<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` must name a profile"))
}

fn find(cmd: &str, dir: &std::path::Path, name: &str) -> Result<(PathBuf, ScoutProfile)> {
    lib::find(dir, name).ok_or_else(|| bad(cmd, format!("no profile named `{name}`")))
}

fn io(cmd: &str, e: String) -> EngineError {
    EngineError::Io(format!("{cmd}: {e}"))
}

fn tags_param(p: &Value) -> Option<Vec<String>> {
    p.get("tags")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_str).map(|t| t.trim().to_string()).filter(|t| !t.is_empty()).collect())
}

fn kind_param(cmd: &str, p: &Value) -> Result<Option<ProfileKind>> {
    match str_param(p, "kind") {
        None => Ok(None),
        Some(k) => ProfileKind::parse(k).map(Some).ok_or_else(|| bad(cmd, "`kind` must be \"track\" or \"mix\"")),
    }
}

fn entry(path: &std::path::Path, p: &ScoutProfile) -> Value {
    let mut v = serde_json::to_value(p).unwrap_or(Value::Null);
    if let Some(o) = v.as_object_mut() {
        o.insert("file".into(), json!(path.to_string_lossy()));
    }
    v
}

fn secs(s: &soundcraft_model::Session, at: i64) -> f64 {
    let sr = s.sample_rate.as_f64();
    if sr > 0.0 { at as f64 / sr } else { 0.0 }
}

/// The audio and source of a clip (as it plays) or a track range (pre-fader), and a default name.
fn session_audio(e: &Engine, cmd: &str, p: &Value, from: &str) -> Result<(Vec<Vec<f32>>, ProfileSource, String)> {
    let s = e.session();
    let clip = if from == "clip" || p.get("clip").is_some() {
        let id = match p.get("clip").and_then(Value::as_u64) {
            Some(n) => Some(soundcraft_model::ClipId(n)),
            None => s.edit.selected_clips.first().copied(),
        };
        if from == "clip" && id.is_none() {
            return Err(bad(cmd, "select a clip first"));
        }
        id
    } else {
        None
    };
    if let Some(id) = clip {
        let Some((t, c)) = s.find_clip(id).map(|(t, c)| (t, c.clone())) else { return Err(bad(cmd, format!("no clip with id {}", id.0))) };
        if !c.is_audio() {
            return Err(bad(cmd, "choose an audio clip"));
        }
        let channels = s.track(t).map_or(1, |x| x.channels());
        crate::io::refuse_longer(s, cmd, &c.name, "measure", c.length, crate::io::render_limit(channels))?;
        let mut tmp = s.clone();
        if let Some(pl) = tmp.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.retain(|x| x.id == c.id);
        }
        let audio = soundcraft_mix::render_clips(&tmp, t, c.range());
        let src =
            ProfileSource { file: format!("{} · {}", s.name, c.name), start_s: secs(s, c.start), end_s: secs(s, c.end()), ..Default::default() };
        return Ok((audio, src, c.name.clone()));
    }
    let t = match track_param(e, cmd, p, "track")? {
        Some(t) => t,
        None => e.target_tracks().into_iter().next().ok_or_else(|| bad(cmd, "select a track first"))?,
    };
    let Some(tr) = s.track(t) else { return Err(bad(cmd, "no such track")) };
    let num = |k: &str| p.get(k).and_then(Value::as_i64);
    let range = match (num("start"), num("end")) {
        (Some(a), Some(b)) => Range::new(a, b),
        _ if from == "range" && !s.edit.selection.is_empty() => s.edit.selection,
        _ => {
            let start = tr.clips().iter().map(|c| c.start).min().unwrap_or(0);
            let end = tr.clips().iter().map(soundcraft_model::Clip::end).max().unwrap_or(0);
            Range::new(start, end)
        }
    };
    if range.is_empty() || range.start < 0 {
        return Err(bad(cmd, "choose a time range with audio"));
    }
    crate::io::refuse_longer(s, cmd, &tr.name, "measure", range.len(), crate::io::render_limit(tr.channels().max(2)))?;
    let audio = soundcraft_mix::render_track_pre_fader(s, t, range);
    let src =
        ProfileSource { file: format!("{} · {}", s.name, tr.name), start_s: secs(s, range.start), end_s: secs(s, range.end), ..Default::default() };
    Ok((audio, src, tr.name.clone()))
}

fn capture(e: &mut Engine, p: &Value) -> Result<Value> {
    #[cfg(not(target_arch = "wasm32"))]
    {
        // Per-stem capture separates with the best model installed, as Separate Stems does.
        let model = lib::best_separation_model();
        capture_with_model(e, p, &|a, r| lib::separate_with(model, a, r), model.id())
    }
    #[cfg(target_arch = "wasm32")]
    {
        capture_with(e, p, &|_, _| Err("stem separation is not available on the web build".to_string()))
    }
}

/// `scout.profile_capture` with the separator given (tests pass a fake one).
pub fn capture_with(e: &mut Engine, p: &Value, separate: lib::Separator<'_>) -> Result<Value> {
    capture_with_model(e, p, separate, lib::DEFAULT_SEPARATION_MODEL)
}

/// [`capture_with`], the stem profiles recording `model` as the separator's model.
pub fn capture_with_model(e: &mut Engine, p: &Value, separate: lib::Separator<'_>, model: &str) -> Result<Value> {
    const C: &str = "scout.profile_capture";
    let dir = dir_param(C, p)?;
    let from = str_param(p, "from").unwrap_or("strip");
    let created = super::scout::now_rfc3339();
    let kind = kind_param(C, p)?;
    let role = str_param(p, "role").unwrap_or("").to_string();
    let tags = tags_param(p).unwrap_or_default();
    let name = str_param(p, "name").map(str::trim).filter(|n| !n.is_empty());
    let mut made: Vec<ScoutProfile> = match from {
        "strip" => {
            let s = e.session();
            let cap_id = p.get("capture").and_then(Value::as_u64).ok_or_else(|| bad(C, "`capture` must be a capture id"))?;
            let c = s.scout.captures.iter().find(|c| u64::from(c.id) == cap_id).ok_or_else(|| bad(C, format!("no capture {cap_id}")))?;
            let want = p.get("track");
            let st = c
                .strips
                .iter()
                .find(|st| match want {
                    Some(Value::Number(n)) => n.as_u64() == Some(st.track.0),
                    Some(Value::String(n)) => st.name.eq_ignore_ascii_case(n) || (n.eq_ignore_ascii_case("master") && st.channel_type == "master"),
                    _ => c.strips.len() == 1,
                })
                .ok_or_else(|| bad(C, "`track` must name a strip of the capture"))?;
            let pos = str_param(p, "position").unwrap_or("post");
            let m = match pos {
                "base" | "pre" => st.base.clone(),
                "post" => st.post.clone(),
                _ => return Err(bad(C, "`position` must be \"base\" or \"post\"")),
            };
            let master = st.channel_type == "master";
            let k = kind.unwrap_or(if master { ProfileKind::Mix } else { ProfileKind::Track });
            let mut prof = lib::make(name.unwrap_or(&format!("{} · {}", c.name, st.name)), k, &role, tags, m, created);
            prof.bands_version = c.bands_version;
            let played_end = c.played.iter().map(|r| r.end).max().unwrap_or(0);
            let played_start = c.played.iter().map(|r| r.start).min().unwrap_or(0);
            let sr = if c.sample_rate > 0.0 { c.sample_rate } else { 1.0 };
            prof.source = ProfileSource {
                file: format!("{} · {} · {}", s.name, c.name, st.name),
                start_s: played_start as f64 / sr,
                end_s: played_end as f64 / sr,
                position: Some(if pos == "post" { "post".into() } else { "base".into() }),
                ..Default::default()
            };
            vec![prof]
        }
        "clip" | "range" => {
            let (audio, src, default) = session_audio(e, C, p, from)?;
            let m = lib::measure(&audio, e.session().sample_rate.as_f64());
            let mut prof = lib::make(name.unwrap_or(&default), kind.unwrap_or(ProfileKind::Track), &role, tags, m, created);
            prof.source = src;
            vec![prof]
        }
        "file" => {
            let path = str_param(p, "path").ok_or_else(|| bad(C, "`path` must name an audio file"))?;
            let path = std::path::Path::new(path);
            let (audio, rate) = lib::read_audio(path).map_err(|m| bad(C, m))?;
            let m = lib::measure(&audio, f64::from(rate));
            let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
            let mut prof = lib::make(name.unwrap_or(&stem), kind.unwrap_or(ProfileKind::Mix), &role, tags, m, created);
            let frames = audio.first().map_or(0, Vec::len);
            prof.source = ProfileSource {
                file: path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default(),
                start_s: 0.0,
                end_s: frames as f64 / f64::from(rate.max(1)),
                ..Default::default()
            };
            vec![prof]
        }
        "stems" => {
            let (audio, src, default) = if let Some(path) = str_param(p, "path") {
                let path = std::path::Path::new(path);
                let (audio, rate) = lib::read_audio(path).map_err(|m| bad(C, m))?;
                if rate != e.session().sample_rate.hz() {
                    return Err(bad(C, "the file's sample rate differs from the session's; import it first"));
                }
                let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                let file = path.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
                (audio, ProfileSource { file, ..Default::default() }, stem)
            } else {
                session_audio(e, C, p, "track")?
            };
            let song = name.unwrap_or(&default).to_string();
            let mut v = lib::stem_profiles(&song, audio, e.session().sample_rate.hz(), separate, model, &src, &created).map_err(|m| io(C, m))?;
            for prof in &mut v {
                prof.tags = tags.clone();
            }
            v
        }
        other => return Err(bad(C, format!("unknown source `{other}` (strip, clip, range, file, stems)"))),
    };
    let mut paths = Vec::new();
    if bool_or(p, "save", true) {
        for prof in &mut made {
            paths.push(json!(lib::save_new(&dir, prof).map_err(|m| io(C, m))?.to_string_lossy()));
        }
    }
    Ok(json!({"profiles": made, "paths": paths}))
}

fn list(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_list";
    let dir = dir_param(C, p)?;
    let q = str_param(p, "query").unwrap_or("");
    let role = str_param(p, "role").unwrap_or("");
    let kind = kind_param(C, p)?;
    let v: Vec<Value> =
        lib::list(&dir).iter().filter(|(_, x)| x.matches(q, role) && kind.is_none_or(|k| x.kind == k)).map(|(f, x)| entry(f, x)).collect();
    Ok(json!({"dir": dir.to_string_lossy(), "profiles": v}))
}

fn rename(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_rename";
    let dir = dir_param(C, p)?;
    let (path, mut prof) = find(C, &dir, name_param(C, p)?)?;
    let to = str_param(p, "to").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`to` must be a new name"))?;
    if !to.eq_ignore_ascii_case(&prof.name) && lib::find(&dir, to).is_some() {
        return Err(bad(C, format!("a profile named `{to}` exists")));
    }
    prof.name = to.to_string();
    prof.sanitize();
    lib::write(&path, &prof).map_err(|m| io(C, m))?;
    Ok(json!({"name": prof.name}))
}

fn duplicate(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_duplicate";
    let dir = dir_param(C, p)?;
    let (_, mut prof) = find(C, &dir, name_param(C, p)?)?;
    prof.name = str_param(p, "to").map_or_else(|| format!("{} copy", prof.name), str::to_string);
    let file = lib::save_new(&dir, &mut prof).map_err(|m| io(C, m))?;
    Ok(json!({"name": prof.name, "file": file.to_string_lossy()}))
}

fn delete(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_delete";
    let dir = dir_param(C, p)?;
    let (path, prof) = find(C, &dir, name_param(C, p)?)?;
    crate::io::remove_path(&path)?;
    Ok(json!({"name": prof.name}))
}

fn edit(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_edit";
    let dir = dir_param(C, p)?;
    let (path, mut prof) = find(C, &dir, name_param(C, p)?)?;
    if let Some(r) = str_param(p, "role") {
        prof.role = r.trim().to_string();
    }
    if let Some(t) = tags_param(p) {
        prof.tags = t;
    }
    if let Some(k) = kind_param(C, p)? {
        prof.kind = k;
    }
    prof.sanitize();
    lib::write(&path, &prof).map_err(|m| io(C, m))?;
    Ok(json!({"profile": prof}))
}

fn export(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_export";
    let dir = dir_param(C, p)?;
    let (_, prof) = find(C, &dir, name_param(C, p)?)?;
    let to = str_param(p, "path").ok_or_else(|| bad(C, "`path` must name the file to write"))?;
    crate::io::write_path(to, prof.to_json())?;
    Ok(json!({"path": to}))
}

fn import(_e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "scout.profile_import";
    let dir = dir_param(C, p)?;
    let from = str_param(p, "path").ok_or_else(|| bad(C, "`path` must name a profile file"))?;
    let mut prof = lib::read(std::path::Path::new(from)).map_err(|m| bad(C, m))?;
    let file = lib::save_new(&dir, &mut prof).map_err(|m| io(C, m))?;
    Ok(json!({"name": prof.name, "file": file.to_string_lossy()}))
}

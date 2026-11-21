//! File menu commands.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::Session;
use soundcraft_model::TrackKind;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "session.new", "New...", ["File"], Some("Cmd+N"), "{name?: 'Untitled', sample_rate?: 48000, bit_depth?: 24, template?: blank|demo|<path of a template file>}", always, new_session),
        cmd!(noundo "session.open", "Open Session...", ["File"], Some("Cmd+O"), "{path}", always, |e, p| open_through_job(e, "session.open", p)),
        cmd!(noundo "session.close", "Close Session", ["File"], Some("Cmd+Shift+W"), "{}", always, |e, p| { open_through_job(e, "session.close", p)?; Ok(json!({})) }),
        cmd!(noundo "session.save", "Save Session", ["File"], Some("Cmd+S"), "{path?, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").map(str::to_string).or_else(|| e.path.clone()).ok_or_else(|| bad("session.save", "no path yet: pass `path` (Save As)"))?;
            refuse_replacing(e, "session.save", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_as", "Save Session As...", ["File"], Some("Cmd+Shift+S"), "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_as", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_as", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_copy", "Save Session Copy In...", ["File"], None, "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_copy", "`path` required"))?.to_string();
            refuse_replacing(e, "session.save_copy", &crate::io::session_file(&path), p)?;
            let written = crate::io::save_session_copy(e, &path)?;
            Ok(json!({"path": path, "audio_files_written": written}))
        }),
        cmd!(noundo "session.save_template", "Save As Template...", ["File"], None, "{path, replace?: false}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("session.save_template", "`path` required"))?.to_string();
            let file = crate::io::session_file(&path);
            refuse_replacing(e, "session.save_template", &file, p)?;
            let mut s = e.session().clone();
            for t in &mut s.tracks { for pl in &mut t.playlists { pl.clips.clear() } }
            s.sources.clear();
            s.videos.clear();
            let text = s.to_json().map_err(|err| EngineError::Io(err.to_string()))?;
            crate::io::replace_file(&file.with_extension(crate::io::SESSION_TMP_EXTENSION), &file, text.as_bytes())?;
            Ok(json!({"path": file}))
        }),
        cmd!(noundo "session.revert", "Revert Session to Saved...", ["File"], None, "{}", |e| if e.path.is_some() { Ok(()) } else { Err("the session has not been saved".into()) }, |e, p| open_through_job(e, "session.revert", p)),
        cmd!(
            "file.import_audio",
            "Audio...",
            ["File", "Import"],
            Some("Cmd+Shift+I"),
            "{path | paths: [..], track?: target track, at?: position, new_tracks?: true (default; false uses `track` or the selected audio track)}",
            always,
            import_audio
        ),
        cmd!("file.import_midi", "MIDI...", ["File", "Import"], Some("Cmd+Alt+I"), "{path, at?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.import_midi", "`path` required"))?.to_string();
            let at = position_param(e, "file.import_midi", p, "at")?.unwrap_or(0);
            let bytes = crate::io::read_file(&path)?;
            let tracks = crate::io::import_midi_bytes(e, &bytes, at, bool_or(p, "tempo_map", true))?;
            Ok(json!({"tracks": tracks}))
        }),
        cmd!(
            "file.import_musicxml",
            "MusicXML...",
            ["File", "Import"],
            None,
            "{path, at?} — a partwise MusicXML score: a track per part, its meters and tempo marks on the tempo map, its keys on the Key ruler, its words as lyrics",
            always,
            |e, p| {
                const C: &str = "file.import_musicxml";
                let path = str_param(p, "path").ok_or_else(|| bad(C, "`path` required"))?.to_string();
                let at = position_param(e, C, p, "at")?.unwrap_or(0);
                let bytes = crate::io::read_file(&path)?;
                let text = String::from_utf8(bytes).map_err(|_| bad(C, "a MusicXML file is UTF-8 text (a compressed .mxl is not read)"))?;
                let tracks = crate::musicxml::import(e, &text, at)?;
                Ok(json!({"tracks": tracks}))
            }
        ),
        cmd!(
            noundo "file.import_dawproject",
            "DAWproject...",
            ["File", "Import"],
            None,
            "{path} — opens a .dawproject file (Studio One, Bitwig, Cubase) as a new unsaved session, loading in the background like Open; returns {name, tracks, report: {file, application, carried, plugins, items: [{fate: placeholder|changed|dropped, track, what, why}]}, report_text}; the report is also saved in the Notes tab",
            always,
            |e, p| open_through_job(e, "file.import_dawproject", p)
        ),
        cmd!(
            "file.import_video",
            "Video...",
            ["File", "Import"],
            Some("Cmd+Ctrl+I"),
            "{path, at?} — adds a Video track clip for the movie's picture (H.264, ProRes, Motion JPEG) and imports its audio onto a new track",
            always,
            |e, p| {
                let path = str_param(p, "path").ok_or_else(|| bad("file.import_video", "`path` required"))?.to_string();
                let at = position_param(e, "file.import_video", p, "at")?.unwrap_or(0).max(0);
                let bytes = crate::io::read_file(&path)?;
                crate::io::import_video_bytes(e, &path, bytes, at)
            }
        ),
        cmd!(noundo "file.bounce_mix", "Bounce Mix...", ["File"], Some("Cmd+Alt+B"), "{path, format?: wav|aiff|flac|mp3|opus (default: the path's extension), bit_depth?: 16|24|32f, bitrate?: kbps for mp3 (32-320, default 320) or opus (6-510, default 160), start?, end? (default: the loop when one exists, else the whole session), source?: main|bus name|hardware output name (I/O Setup; `output:name` when a bus has that name), normalize?: false (peak to -0.1 dBFS), loudness?: target LUFS (-40..0; integrated, BS.1770: gain only, held back so the true peak stays under true_peak), true_peak?: dBTP ceiling for loudness (-9..0, default -1), dither?: true, fold_down?: stereo (mp3 and opus hold mono or stereo), tail?: seconds past the range (0-30), sample_rate?: Hz to convert to (8000-384000), dither?: tpdf|shaped|off (true/false), channels?: as_mixed|mono|split (a mono file per channel), also?: [more formats written in the same pass], tags?: {title, artist, album, date, comment} (in every format), import?: false (add the bounce to the session on new tracks), realtime?: false (the app plays the range through the audio device and prints it as heard, hardware inserts included; without a device, and outside the app, it renders offline)} → {path, files, imported, ...}", always, bounce),
        cmd!(noundo "file.bounce_stems", "Bounce Stems...", [], None, "{dir, format?: wav|aiff|flac|mp3|opus, bit_depth?: 16|24|32f, bitrate?: kbps for mp3 or opus, start?, end?, fold_down?: stereo, tracks?: [track] (default: every unmuted audio, instrument and MIDI track), busses?: true (every bus) | [bus name]: a stem of each bus as summed}", has_tracks, |e, p| {
            let files = stems_job(e, p)?.run()?;
            Ok(json!({"files": files}))
        }),
        cmd!(noundo "file.export_midi", "MIDI...", ["File", "Export"], None, "{path, tracks?}", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_midi", "`path` required"))?.to_string();
            let only = if p.get("tracks").is_some() || p.get("track").is_some() { Some(tracks_param(e, "file.export_midi", p)?) } else { None };
            let bytes = crate::io::export_midi(e, only.as_deref())?;
            crate::io::write_file(&path, &bytes)?;
            Ok(json!({"path": path, "bytes": bytes.len()}))
        }),
        cmd!(noundo "file.export_clip_midi", "Export Clip as MIDI...", [], None, "{path, clips?} — the selected MIDI clips alone as a .mid file, starting at the earliest one's start, the clips keeping their spacing", has_selection, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_clip_midi", "`path` required"))?.to_string();
            let clips = clip_ids_param(e, p);
            let bytes = crate::io::export_clips_midi(e, &clips)?;
            crate::io::write_file(&path, &bytes)?;
            Ok(json!({"path": path, "bytes": bytes.len()}))
        }),
        cmd!(
            noundo "file.export_dawproject",
            "DAWproject...",
            ["File", "Export"],
            None,
            "{path, replace?: false} — writes the session as a .dawproject file (tracks, busses, sends, clips with their audio, MIDI, automation, markers, tempo map, plugins with their state) for Studio One, Bitwig or Cubase; returns {path, tracks, clips, audio_files, plugin_states, notes: [what other applications won't get exactly]}",
            always,
            export_dawproject
        ),
        cmd!(noundo "file.export_session_text", "Session Info as Text...", ["File", "Export"], None, "{path?}", always, |e, p| {
            let text = crate::inspect::session_text(e);
            if let Some(path) = str_param(p, "path") {
                crate::io::write_file(path, text.as_bytes())?;
            }
            Ok(json!({"text": text}))
        }),
        cmd!(noundo "file.export_clips", "Export Clips as Files...", [], Some("Cmd+Shift+K"), "{dir, clips?, format?: wav}", always, |e, p| {
            let dir = str_param(p, "dir").ok_or_else(|| bad("file.export_clips", "`dir` required"))?.to_string();
            let ids = clip_ids_param(e, p);
            if ids.is_empty() {
                return Err(bad("file.export_clips", "no clips selected"));
            }
            for (t, c) in ids.iter().filter_map(|id| e.session().find_clip(*id)) {
                let channels = e.session().track(t).map_or(1, |x| x.channels());
                crate::io::refuse_longer(e.session(), "file.export_clips", &c.name, "export", c.length, crate::io::render_limit(channels))?;
            }
            let n = crate::io::export_clips(e, &ids, &dir)?;
            Ok(json!({"written": n, "dir": dir}))
        }),
        cmd!(noundo "file.export_selected_tracks_as_session", "Selected Tracks as New Session...", ["File", "Export"], None, "{path, tracks?, replace?: false}", has_selection, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_selected_tracks_as_session", "`path` required"))?.to_string();
            refuse_replacing(e, "file.export_selected_tracks_as_session", &crate::io::session_file(&path), p)?;
            let keep = tracks_required(e, "file.export_selected_tracks_as_session", p)?;
            let mut s = e.session().clone();
            s.tracks.retain(|t| keep.contains(&t.id) || t.kind == soundcraft_model::TrackKind::Master);
            let mut tmp = crate::Engine::new(s);
            crate::io::save_session(&mut tmp, &path)?;
            Ok(json!({"path": path}))
        }),
        cmd!(noundo "file.export_sibelius", "Sibelius...", ["File", "Export"], None, "{path, tracks?} — MIDI tracks as MusicXML (.musicxml)", always, |e, p| {
            write_score(e, p, "file.export_sibelius", false)
        }),
        cmd!(noundo "file.send_to_sibelius", "Sibelius", ["File", "Send To..."], None, "{path?, tracks?} — writes MusicXML for a notation program (to the temp folder when no `path`)", always, |e, p| {
            write_score(e, p, "file.send_to_sibelius", false)
        }),
        cmd!(noundo "file.print_score", "Print Score...", ["File"], Some("Cmd+P"), "{path?, tracks?} — engraves the MIDI tracks as a printable SVG page", always, |e, p| {
            write_score(e, p, "file.print_score", true)
        }),
        cmd!(
            "file.score_setup",
            "Score Setup...",
            ["File"],
            None,
            "{title?, composer?, bars_per_system?: 1..16, show_track_names?: bool, staff_percent?: 70..150, part_pages?: bool (Print Score writes a page per track), page?: continuous|a4|letter, section_breaks?: bool (a new system at each section)} — returns the current setup",
            always,
            |e, p| {
                let mut setup = crate::score::ScoreSetup::from_session(e.session());
                if let Some(t) = str_param(p, "title") {
                    setup.title = t.to_string();
                }
                if let Some(c) = str_param(p, "composer") {
                    setup.composer = c.to_string();
                }
                if let Some(b) = p.get("bars_per_system").and_then(serde_json::Value::as_f64) {
                    setup.bars_per_system = if b.is_finite() { b.clamp(1.0, 16.0) as u32 } else { 4 };
                }
                if let Some(b) = p.get("show_track_names").and_then(serde_json::Value::as_bool) {
                    setup.show_track_names = b;
                }
                if let Some(v) = p.get("staff_percent").and_then(serde_json::Value::as_f64) {
                    setup.staff_percent = if v.is_finite() { v.clamp(70.0, 150.0) as u32 } else { 100 };
                }
                if let Some(b) = p.get("part_pages").and_then(serde_json::Value::as_bool) {
                    setup.part_pages = b;
                }
                if let Some(v) = p.get("page") {
                    let name = v.as_str().map(str::to_ascii_lowercase);
                    setup.page = match name.as_deref() {
                        Some("continuous") => 0,
                        Some("a4") => 1,
                        Some("letter" | "us letter") => 2,
                        _ => return Err(bad("file.score_setup", "`page` is continuous, a4 or letter")),
                    };
                }
                if let Some(b) = p.get("section_breaks").and_then(serde_json::Value::as_bool) {
                    setup.section_breaks = b;
                }
                setup.store(e.session_mut());
                let page = ["continuous", "a4", "letter"].get(usize::from(setup.page)).copied().unwrap_or("continuous");
                Ok(
                    json!({"title": setup.title, "composer": setup.composer, "bars_per_system": setup.bars_per_system, "show_track_names": setup.show_track_names, "staff_percent": setup.staff_percent, "part_pages": setup.part_pages, "page": page, "section_breaks": setup.section_breaks}),
                )
            }
        ),
        cmd!(noundo "file.export_clip_groups", "Export Clip Groups...", [], None, "{path, clips?} — writes a .scgrp clip group file with its audio", always, |e, p| {
            let path = str_param(p, "path").ok_or_else(|| bad("file.export_clip_groups", "`path` required"))?.to_string();
            let ids = clip_ids_param(e, p);
            let n = crate::clip_group_file::export(e, &ids, &path)?;
            Ok(json!({"path": path, "clips": n}))
        }),
        cmd!("file.import_clip_groups", "Clip Groups...", ["File", "Import"], None, "{path, track?: first target track, at?}", always, |e, p| {
            let id = "file.import_clip_groups";
            let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
            let at = position_param(e, id, p, "at")?.unwrap_or(e.session().edit.selection.start).max(0);
            let first = tracks_param(e, id, p)?.first().copied();
            let placed = crate::clip_group_file::import(e, &path, first, at)?;
            e.session_mut().edit.selected_clips.clone_from(&placed);
            Ok(json!({"clips": placed.iter().map(|c| c.0).collect::<Vec<_>>()}))
        }),
        cmd!(query "file.get_info", "Get Info...", ["File"], None, "{}", always, |e, _| Ok(crate::inspect::session(e, false))),
        cmd!(noundo "app.quit", "Quit", [], Some("Cmd+Q"), "{}", always, |e, _| {
            // Undo ends here: takes nothing names any more go.
            crate::io::clean_unused_takes(e, true);
            Ok(json!({"quit": true}))
        }),
    ]
}

/// Writing a session over an existing file needs `replace: true` (see `io::replaces`).
fn refuse_replacing(e: &Engine, id: &str, file: &std::path::Path, p: &Value) -> Result<()> {
    // First, so a path outside the write root is refused as such and never reported as existing.
    crate::file_access::check_write(file).map_err(EngineError::Io)?;
    if crate::io::replaces(e, id, file) && !bool_or(p, "replace", false) {
        return Err(bad(id, format!("{} already exists: pass `replace: true` to replace it", file.display())));
    }
    Ok(())
}

fn export_dawproject(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "file.export_dawproject";
    let mut path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    if std::path::Path::new(&path).extension().is_none_or(|x| !x.eq_ignore_ascii_case(crate::dawproject::EXTENSION)) {
        path.push('.');
        path.push_str(crate::dawproject::EXTENSION);
    }
    let file = std::path::PathBuf::from(&path);
    crate::file_access::check_write(&file).map_err(EngineError::Io)?;
    if crate::io::path_taken(&file) && !bool_or(p, "replace", false) {
        return Err(bad(id, format!("{path} already exists: pass `replace: true` to replace it")));
    }
    let session = e.session_arc();
    let done = write_dawproject(&session, &file)?;
    Ok(json!({
        "path": path,
        "tracks": done.tracks,
        "clips": done.clips,
        "audio_files": done.audio_files,
        "plugin_states": done.plugin_states,
        "notes": done.notes,
    }))
}

/// Writes the archive beside `file` and renames it into place.
fn write_dawproject(s: &Session, file: &std::path::Path) -> Result<crate::dawproject::export::Exported> {
    crate::io::write_streamed(file, |w| crate::dawproject::export::export(s, w))
}

fn new_session(e: &mut Engine, p: &Value) -> Result<Value> {
    open_through_job(e, "session.new", p)
}

/// The session-replacing commands run both halves of [`crate::open_job`] in one go (a UI host
/// runs them a phase at a time behind its progress window instead).
fn open_through_job(e: &mut Engine, id: &str, p: &Value) -> Result<Value> {
    let req = crate::open_job::OpenRequest::from_command(e, id, p).ok_or_else(|| bad(id, "not a session-replacing command"))??;
    crate::open_job::open_now(e, req)
}

fn import_audio(e: &mut Engine, p: &Value) -> Result<Value> {
    let mut paths: Vec<String> =
        p.get("paths").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default();
    if let Some(one) = str_param(p, "path") {
        paths.push(one.to_string());
    }
    if paths.is_empty() {
        return Err(bad("file.import_audio", "`path` or `paths` required"));
    }
    let at = position_param(e, "file.import_audio", p, "at")?.unwrap_or(0).max(0);
    let target = import_audio_target(e, p)?;
    let mut out = Vec::new();
    for path in &paths {
        out.push(crate::io::import_audio_file(e, path, target, at)?);
    }
    Ok(json!({"imported": out}))
}

/// Destination for `file.import_audio`: an explicit `track` wins; otherwise `new_tracks`
/// (default true) creates a new track per file, and `new_tracks: false` uses the selected
/// audio track or errors when none is selected.
fn import_audio_target(e: &Engine, p: &Value) -> Result<Option<TrackId>> {
    if let Some(t) = track_param(e, "file.import_audio", p, "track")? {
        return Ok(Some(t));
    }
    if bool_or(p, "new_tracks", true) {
        return Ok(None);
    }
    let s = e.session();
    let selected = s.edit.selected_tracks.iter().find_map(|id| s.track(*id).filter(|t| t.kind == TrackKind::Audio).map(|t| t.id));
    selected.map(Some).ok_or_else(|| bad("file.import_audio", "`new_tracks: false` needs `track` or a selected audio track"))
}

/// `source` omitted or `main` is the main mix. Any other name is a bus, else a hardware output
/// of the I/O Setup (by its name, or `output:` and its name when a bus has the same name); an
/// unknown name is an error. The first output is the main mix.
fn bounce_bus(e: &Engine, p: &Value) -> Result<soundcraft_mix::Tap> {
    use soundcraft_mix::Tap;
    let Some(name) = str_param(p, "source").map(str::trim).filter(|s| !s.is_empty()) else {
        return Ok(Tap::Main);
    };
    if name.eq_ignore_ascii_case("main") {
        return Ok(Tap::Main);
    }
    let s = e.session();
    let output = |n: &str| s.outputs.iter().position(|o| o.name.eq_ignore_ascii_case(n.trim())).map(Tap::Output);
    if let Some(n) = name.strip_prefix("output:") {
        return output(n).ok_or_else(|| bad("file.bounce_mix", format!("unknown output `{}`", n.trim())));
    }
    s.bus_by_name(name)
        .map(|b| Tap::Bus(b.id))
        .or_else(|| output(name))
        .ok_or_else(|| bad("file.bounce_mix", format!("unknown bus or output `{name}`")))
}

/// `fold_down: "stereo"` bounces an ITU stereo fold-down of a surround mix.
fn fold_down_param(p: &Value, id: &str) -> Result<bool> {
    match str_param(p, "fold_down").map(str::trim) {
        None | Some("") | Some("none") => Ok(false),
        Some(f) if f.eq_ignore_ascii_case("stereo") => Ok(true),
        Some(f) => Err(bad(id, format!("unknown fold_down `{f}` (use \"stereo\")"))),
    }
}

fn bounce(e: &mut Engine, p: &Value) -> Result<Value> {
    let job = bounce_job(e, p)?;
    let (path, seconds) = (job.path().to_string_lossy().into_owned(), e.session().sample_rate.seconds(job.range().len()));
    let at = job.range().start;
    let done = job.run()?;
    let files: Vec<String> = done.files.iter().map(|f| f.to_string_lossy().into_owned()).collect();
    let imported = if bool_or(p, "import", false) { import_bounce(e, &files, at)? } else { Value::Null };
    Ok(json!({
        "path": path,
        "files": files,
        "imported": imported,
        "bytes": done.bytes,
        "seconds": seconds,
        "peak_db": done.peak_db,
        "lufs": done.lufs,
        "true_peak_db": done.true_peak_db,
        "measured": done.measured.map(|m| json!({"lufs": m.lufs, "true_peak_db": m.true_peak_db, "gain_db": m.gain_db, "held_by_ceiling": m.held_by_ceiling})),
    }))
}

/// Bounce Mix's Add to Session: the main file (every file of a split bounce, as one track each)
/// imported onto new tracks at the bounced range's start.
pub fn import_bounce(e: &mut Engine, files: &[String], at: soundcraft_time::Samples) -> Result<Value> {
    let main_ext = files.first().and_then(|f| std::path::Path::new(f).extension().map(|x| x.to_os_string()));
    let paths: Vec<&String> = files.iter().filter(|f| std::path::Path::new(f.as_str()).extension().map(|x| x.to_os_string()) == main_ext).collect();
    e.execute("file.import_audio", &json!({"paths": paths, "at": at, "new_tracks": true}))
}

/// The format a bounce writes for a format name or extension.
fn format_param(id: &str, name: &str) -> Result<soundcraft_audio_io::FileFormat> {
    soundcraft_audio_io::encode_format_for(name)
        .ok_or_else(|| bad(id, format!("cannot write `{name}` files (supported: {})", soundcraft_audio_io::ENCODE_EXTENSIONS)))
}

/// `bit_depth`: 16, 24 (the default) or 32 (float).
fn bit_depth_param(p: &Value) -> soundcraft_audio_io::BitDepth {
    match p.get("bit_depth").map(|b| b.to_string().trim_matches('"').to_string()).as_deref() {
        Some("16") => soundcraft_audio_io::BitDepth::Int16,
        Some("32") | Some("32f") => soundcraft_audio_io::BitDepth::Float32,
        _ => soundcraft_audio_io::BitDepth::Int24,
    }
}

/// `bitrate` in kbps for an MP3 or Opus bounce (0, the format's default, when not given).
fn bitrate_param(id: &str, p: &Value, format: soundcraft_audio_io::FileFormat) -> Result<u32> {
    let Some(v) = p.get("bitrate") else { return Ok(0) };
    let kbps = v
        .as_u64()
        .or_else(|| v.as_str().and_then(|s| s.trim().trim_end_matches("kbps").trim_end_matches('k').trim().parse().ok()))
        .and_then(|k| u32::try_from(k).ok())
        .ok_or_else(|| bad(id, "`bitrate` must be a whole number of kbps"))?;
    if !soundcraft_audio_io::is_lossy(format) {
        return Ok(0);
    }
    soundcraft_audio_io::check_kbps(format, kbps).map_err(|err| bad(id, err.to_string()))?;
    Ok(kbps)
}

/// `loudness` (a LUFS target, with `true_peak` its ceiling) or `normalize` (peak).
fn normalize_param(p: &Value) -> Result<crate::io::Normalize> {
    let id = "file.bounce_mix";
    let num = |k: &str| {
        p.get(k).filter(|v| !v.is_null()).map(|v| v.as_f64().filter(|x| x.is_finite()).ok_or_else(|| bad(id, format!("`{k}` must be a number"))))
    };
    match num("loudness").transpose()? {
        Some(l) => {
            if !(-40.0..=0.0).contains(&l) {
                return Err(bad(id, "`loudness` is a target from -40 to 0 LUFS"));
            }
            let ceiling = num("true_peak").transpose()?.unwrap_or(-1.0);
            if !(-9.0..=0.0).contains(&ceiling) {
                return Err(bad(id, "`true_peak` is a ceiling from -9 to 0 dBTP"));
            }
            Ok(crate::io::Normalize::Loudness { lufs: l as f32, ceiling_db: ceiling as f32 })
        }
        None if bool_or(p, "normalize", false) => Ok(crate::io::Normalize::Peak),
        None => Ok(crate::io::Normalize::Off),
    }
}

/// The export `file.bounce_stems` runs for `p`, as a job a host steps behind a progress window
/// ([`crate::io::StemsJob`]); the command itself runs it to the end.
pub fn stems_job(e: &Engine, p: &Value) -> Result<crate::io::StemsJob> {
    let dir = str_param(p, "dir").ok_or_else(|| bad("file.bounce_stems", "`dir` required"))?.to_string();
    let r = range_param(e, "file.bounce_stems", p)?;
    let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
    let format = format_param("file.bounce_stems", str_param(p, "format").unwrap_or("wav"))?;
    let opts = soundcraft_audio_io::EncodeOptions {
        format,
        bit_depth: bit_depth_param(p),
        dither: true,
        bwf: None,
        bitrate_kbps: bitrate_param("file.bounce_stems", p, format)?,
    };
    let fold = fold_down_param(p, "file.bounce_stems")?;
    let tracks = if p.get("tracks").is_some_and(|v| !v.is_null()) { Some(tracks_param(e, "file.bounce_stems", p)?) } else { None };
    let busses: Vec<soundcraft_model::BusId> = match p.get("busses") {
        None | Some(Value::Null) | Some(Value::Bool(false)) => Vec::new(),
        Some(Value::Bool(true)) => e.session().busses.iter().map(|b| b.id).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|v| {
                let name = v.as_str().unwrap_or_default();
                e.session().bus_by_name(name).map(|b| b.id).ok_or_else(|| bad("file.bounce_stems", format!("unknown bus `{name}`")))
            })
            .collect::<Result<_>>()?,
        Some(_) => return Err(bad("file.bounce_stems", "`busses` is true or a list of bus names")),
    };
    if tracks.as_ref().is_some_and(Vec::is_empty) && busses.is_empty() {
        return Err(bad("file.bounce_stems", "no tracks or busses to bounce"));
    }
    crate::io::StemsJob::choose(e, &dir, r, &opts, fold, tracks.as_deref(), &busses)
}

/// The longest tail a bounce takes past its range (`file.bounce_mix`'s `tail`), seconds.
pub const MAX_TAIL_SECS: f64 = 30.0;

/// The bounce `file.bounce_mix` runs for `p`, not rendered yet.
pub fn bounce_job(e: &Engine, p: &Value) -> Result<crate::io::Bounce> {
    let path = str_param(p, "path").ok_or_else(|| bad("file.bounce_mix", "`path` required"))?.to_string();
    // The range given; else the loop when there is one; else the whole session. The edit
    // selection is not used: the selection and the loop are one thing (the sticky loop spec).
    let r = match (p.get("start").is_some() || p.get("end").is_some() || p.get("length").is_some(), e.session().edit.loop_range) {
        (true, _) => range_param(e, "file.bounce_mix", p)?,
        (false, Some(lp)) => lp,
        (false, None) => soundcraft_time::Range::default(),
    };
    let r = if r.is_empty() { soundcraft_time::Range::new(0, e.session().content_end().max(1)) } else { r };
    // `tail` seconds more (up to 30), so a reverb or delay rings out past the range.
    let tail = p.get("tail").and_then(Value::as_f64).filter(|t| t.is_finite()).unwrap_or(0.0).clamp(0.0, MAX_TAIL_SECS);
    let r = soundcraft_time::Range::new(r.start, r.end.saturating_add(e.session().sample_rate.samples(tail)));
    let ext = std::path::Path::new(&path).extension().and_then(|x| x.to_str()).unwrap_or("wav").to_ascii_lowercase();
    let format = format_param("file.bounce_mix", str_param(p, "format").unwrap_or(ext.as_str()))?;
    let (dither, noise_shaping) = dither_param(p)?;
    let opts = soundcraft_audio_io::EncodeOptions {
        format,
        bit_depth: bit_depth_param(p),
        dither,
        bwf: None,
        bitrate_kbps: bitrate_param("file.bounce_mix", p, format)?,
    };
    let normalize = normalize_param(p)?;
    let bus = bounce_bus(e, p)?;
    let layout = layout_param(p)?;
    // A mono bounce of a surround source averages its stereo fold-down (not the LFE and heights).
    let fold =
        fold_down_param(p, "file.bounce_mix")? || (layout == crate::io::FileLayout::Mono && soundcraft_mix::source_channels(e.session(), bus) > 2);
    let export = crate::io::Export {
        sample_rate: sample_rate_param(p)?,
        noise_shaping,
        layout,
        also: also_param(p, &opts)?,
        bwf: true,
        tags: tags_param(p)?,
        import: bool_or(p, "import", false),
    };
    crate::io::Bounce::export(e.session_arc(), std::path::Path::new(&path), r, &opts, bus, normalize, fold, &export)
        .map(|b| b.with_band(e.band_library()))
}

/// `dither`: true or "tpdf" (the default), "shaped" (TPDF with noise shaping), false or "off".
/// Returns the encoder's dither switch and whether the bounce shapes its own.
fn dither_param(p: &Value) -> Result<(bool, bool)> {
    match p.get("dither") {
        None | Some(Value::Null) => Ok((true, false)),
        Some(Value::Bool(b)) => Ok((*b, false)),
        Some(Value::String(s)) => match s.trim().to_ascii_lowercase().as_str() {
            "tpdf" | "on" => Ok((true, false)),
            "shaped" | "noise_shaped" => Ok((true, true)),
            "off" | "none" => Ok((false, false)),
            other => Err(bad("file.bounce_mix", format!("unknown dither `{other}` (tpdf, shaped or off)"))),
        },
        Some(_) => Err(bad("file.bounce_mix", "`dither` is true, false, \"tpdf\", \"shaped\" or \"off\"")),
    }
}

/// `channels`: "mono" (one averaged file), "split" (a mono file per channel) or as mixed.
fn layout_param(p: &Value) -> Result<crate::io::FileLayout> {
    match str_param(p, "channels").map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        None | Some("" | "as_mixed" | "all" | "interleaved") => Ok(crate::io::FileLayout::AsMixed),
        Some("mono") => Ok(crate::io::FileLayout::Mono),
        Some("split" | "multiple_mono" | "multi_mono") => Ok(crate::io::FileLayout::Split),
        Some(other) => Err(bad("file.bounce_mix", format!("unknown `channels` `{other}` (mono, split or as_mixed)"))),
    }
}

/// `sample_rate`: the rate the files are written at (converted as they are written).
fn sample_rate_param(p: &Value) -> Result<Option<u32>> {
    match p.get("sample_rate") {
        None | Some(Value::Null) => Ok(None),
        Some(v) => v
            .as_u64()
            .and_then(|r| u32::try_from(r).ok())
            .filter(|r| (8_000..=crate::io::MAX_EXPORT_RATE).contains(r))
            .map(Some)
            .ok_or_else(|| bad("file.bounce_mix", format!("`sample_rate` is a rate in Hz from 8000 to {}", crate::io::MAX_EXPORT_RATE))),
    }
}

/// `also`: more formats written in the same pass, beside the main file (the main file's bit
/// depth for the lossless ones, the format's default bitrate for MP3 and Opus).
fn also_param(p: &Value, main: &soundcraft_audio_io::EncodeOptions) -> Result<Vec<soundcraft_audio_io::EncodeOptions>> {
    let names: Vec<&str> = match p.get("also") {
        None | Some(Value::Null) => return Ok(Vec::new()),
        Some(Value::String(s)) => vec![s.as_str()],
        Some(Value::Array(a)) => {
            a.iter().map(|v| v.as_str().ok_or_else(|| bad("file.bounce_mix", "`also` is a list of formats"))).collect::<Result<_>>()?
        }
        Some(_) => return Err(bad("file.bounce_mix", "`also` is a list of formats")),
    };
    let mut out = Vec::new();
    for n in names {
        let format = format_param("file.bounce_mix", n.trim())?;
        if format == main.format || out.iter().any(|o: &soundcraft_audio_io::EncodeOptions| o.format == format) {
            continue;
        }
        // FLAC holds 16 or 24 bits: a float main file gives a 24-bit FLAC.
        let bit_depth = match (format, main.bit_depth) {
            (soundcraft_audio_io::FileFormat::Flac, soundcraft_audio_io::BitDepth::Float32 | soundcraft_audio_io::BitDepth::Int32) => {
                soundcraft_audio_io::BitDepth::Int24
            }
            (_, d) => d,
        };
        out.push(soundcraft_audio_io::EncodeOptions { format, bit_depth, dither: main.dither, bwf: None, bitrate_kbps: 0 });
    }
    Ok(out)
}

/// `tags`: {title, artist, album, date, comment}, written as ID3v2 in an MP3, `LIST`/`INFO` (and
/// the `bext` description) in a WAV, Vorbis comments in a FLAC or Opus file and text chunks in an
/// AIFF.
fn tags_param(p: &Value) -> Result<Option<soundcraft_audio_io::export::Tags>> {
    match p.get("tags") {
        None | Some(Value::Null) => Ok(None),
        Some(v @ Value::Object(_)) => {
            let t: soundcraft_audio_io::export::Tags =
                serde_json::from_value(v.clone()).map_err(|e| bad("file.bounce_mix", format!("`tags`: {e}")))?;
            Ok((!t.is_empty()).then_some(t))
        }
        Some(_) => Err(bad("file.bounce_mix", "`tags` is {title, artist, album, date, comment}")),
    }
}

/// Write the score of the chosen MIDI tracks as MusicXML (or SVG when `svg`).
fn write_score(e: &mut Engine, p: &Value, id: &str, svg: bool) -> Result<Value> {
    let tracks = if p.get("tracks").is_some() || p.get("track").is_some() { tracks_param(e, id, p)? } else { Vec::new() };
    let parts = crate::score::layout(e.session(), &tracks);
    if parts.is_empty() {
        return Err(bad(id, "no MIDI or instrument tracks to notate"));
    }
    let setup = crate::score::ScoreSetup::from_session(e.session());
    if svg && setup.part_pages && parts.len() > 1 {
        // A page per part: `<name>-<n> <part>.svg` beside the path asked for.
        let base = match str_param(p, "path") {
            Some(p) => p.trim_end_matches(".svg").to_string(),
            None => std::env::temp_dir().join(crate::io::sanitize_name(&setup.title)).to_string_lossy().to_string(),
        };
        let mut paths = Vec::new();
        for (k, part) in parts.iter().enumerate() {
            let one = crate::score::ScoreSetup { title: format!("{} — {}", setup.title, part.name), ..setup.clone() };
            let path = format!("{base}-{} {}.svg", k + 1, crate::io::sanitize_name(&part.name));
            crate::io::write_file(&path, crate::score::svg(std::slice::from_ref(part), &one).as_bytes())?;
            paths.push(path);
        }
        let measures = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
        return Ok(json!({"path": paths.first(), "paths": paths, "parts": parts.len(), "measures": measures}));
    }
    let (text, ext) = if svg { (crate::score::svg(&parts, &setup), "svg") } else { (crate::score::musicxml(&parts, &setup), "musicxml") };
    let path = match str_param(p, "path") {
        Some(p) => p.to_string(),
        None => std::env::temp_dir().join(format!("{}.{ext}", crate::io::sanitize_name(&setup.title))).to_string_lossy().to_string(),
    };
    crate::io::write_file(&path, text.as_bytes())?;
    let measures = parts.iter().map(|p| p.measures.len()).max().unwrap_or(0);
    Ok(json!({"path": path, "parts": parts.len(), "measures": measures}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::{AudioBuffer, EncodeOptions};
    use soundcraft_model::ChannelFormat;

    fn tone_wav(dir: &std::path::Path, name: &str) -> String {
        let tone: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.1).sin() * 0.2).collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let path = dir.join(name);
        std::fs::write(&path, bytes).unwrap();
        path.to_string_lossy().into_owned()
    }

    /// Parts: with Score Setup's part pages on, Print Score writes a page per track, each titled
    /// with its name; the staff size scales the page.
    #[test]
    fn print_score_writes_a_page_per_part() {
        let mut e = Engine::default();
        for name in ["Flute", "Cello"] {
            e.execute("track.new", &json!({"name": name, "kind": "midi"})).unwrap();
            let c = e.execute("midi.new_clip", &json!({"track": name, "start": 0, "end": 96_000})).unwrap()["clip"].clone();
            e.execute("midi.note_add", &json!({"clip": c, "pitch": 60, "start_ticks": 0, "length_ticks": 960})).unwrap();
        }
        let dir = std::env::temp_dir().join(format!("sc-parts-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("song.svg").to_string_lossy().into_owned();
        let one = e.execute("file.print_score", &json!({"path": path})).unwrap();
        assert_eq!(one["path"], json!(path));
        let page = std::fs::read_to_string(&path).unwrap();
        assert!(page.contains("width=\"794\""), "100 %");
        let r = e.execute("file.score_setup", &json!({"part_pages": true, "staff_percent": 125})).unwrap();
        assert_eq!((r["part_pages"].as_bool(), r["staff_percent"].as_u64()), (Some(true), Some(125)));
        let r = e.execute("file.print_score", &json!({"path": path})).unwrap();
        let paths: Vec<String> = r["paths"].as_array().unwrap().iter().map(|p| p.as_str().unwrap().to_string()).collect();
        assert_eq!(paths.len(), 2);
        let cello = std::fs::read_to_string(&paths[1]).unwrap();
        assert!(paths[1].ends_with("-2 Cello.svg"), "{paths:?}");
        assert!(cello.contains("Cello") && !cello.contains("Flute"), "one part a page");
        assert!(cello.contains("width=\"99"), "125 %: {}", &cello[..200]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Tier 2: a bounce can take a tail past its range (a reverb rings out), at most 30 s.
    #[test]
    fn a_bounce_takes_its_tail() {
        let e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("sc-bounce-tail-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("t.wav").to_string_lossy().into_owned();
        let job = bounce_job(&e, &json!({"path": path, "start": 0, "end": 48_000, "tail": 2.0})).unwrap();
        assert_eq!(job.range().end, 48_000 + e.session().sample_rate.samples(2.0));
        let job = bounce_job(&e, &json!({"path": path, "start": 0, "end": 48_000, "tail": 1e9})).unwrap();
        assert_eq!(job.range().end, 48_000 + e.session().sample_rate.samples(MAX_TAIL_SECS));
        let job = bounce_job(&e, &json!({"path": path, "start": 0, "end": 48_000})).unwrap();
        assert_eq!(job.range().end, 48_000, "no tail unless asked");
        drop(job);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Bounce Mix's export options: another rate, mono and one file per channel, more formats in
    /// the same pass, tags and BWF, noise-shaped dither, and Add to Session.
    #[test]
    fn a_bounce_converts_splits_tags_and_comes_back() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("sc-bounce-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let p = |n: &str| dir.join(n).to_string_lossy().into_owned();
        let probe = |path: &str| {
            let bytes = std::fs::read(path).unwrap();
            (soundcraft_audio_io::probe(&bytes, std::path::Path::new(path).extension().and_then(|x| x.to_str())).unwrap(), bytes)
        };
        let (start, end) = (48_000, 96_000);
        // Converted to 44.1 kHz, with an MP3 beside it, tagged, written in one pass.
        let r = e
            .execute(
                "file.bounce_mix",
                &json!({"path": p("a.wav"), "start": start, "end": end, "sample_rate": 44_100, "also": ["mp3"],
                    "tags": {"title": "Song", "artist": "Band"}, "dither": "shaped", "bit_depth": 16}),
            )
            .unwrap();
        assert_eq!(r["files"].as_array().unwrap().len(), 2, "{r}");
        let (info, bytes) = probe(&p("a.wav"));
        assert_eq!((info.sample_rate, info.channels), (44_100, 2));
        assert!((info.frames as i64 - 44_100).abs() <= 2, "{} frames", info.frames);
        let bwf = info.bwf.expect("a bext chunk");
        assert_eq!(bwf.time_reference, start as u64);
        assert!(bwf.description.contains("Song"), "{bwf:?}");
        assert!(bytes.windows(4).any(|w| w == b"INFO") && bytes.windows(4).any(|w| w == b"INAM"), "LIST/INFO tags");
        let (mp3, mbytes) = probe(&p("a.mp3"));
        assert!(mbytes.starts_with(b"ID3"), "the MP3 is tagged");
        assert_eq!(mp3.channels, 2);
        // FLAC, AIFF and Opus carry the tags in the file too.
        e.execute(
            "file.bounce_mix",
            &json!({"path": p("t.flac"), "start": start, "end": end, "also": ["aif", "opus"], "tags": {"title": "Song", "artist": "Band"}}),
        )
        .unwrap();
        for (f, title) in [("t.flac", &b"TITLE=Song"[..]), ("t.opus", b"TITLE=Song"), ("t.aif", b"NAME\0\0\0\x04Song")] {
            let (info, bytes) = probe(&p(f));
            assert_eq!(info.channels, 2, "{f}");
            assert!(bytes.windows(title.len()).any(|w| w == title), "{f} is tagged");
        }
        // Mono, and one file per channel.
        e.execute("file.bounce_mix", &json!({"path": p("m.wav"), "start": start, "end": end, "channels": "mono"})).unwrap();
        assert_eq!(probe(&p("m.wav")).0.channels, 1);
        let r = e.execute("file.bounce_mix", &json!({"path": p("s.wav"), "start": start, "end": end, "channels": "split"})).unwrap();
        let files: Vec<String> = r["files"].as_array().unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
        assert_eq!(files, vec![p("s.L.wav"), p("s.R.wav")]);
        assert!(files.iter().all(|f| probe(f).0.channels == 1));
        // A mono bounce is the average of the split sides.
        let dec = |f: &str| soundcraft_audio_io::decode(&std::fs::read(f).unwrap(), Some("wav")).unwrap().1.channels;
        let (m, l, rr) = (dec(&p("m.wav")), dec(&files[0]), dec(&files[1]));
        let worst = m[0].iter().zip(l[0].iter().zip(&rr[0])).map(|(m, (l, r))| (m - (l + r) / 2.0).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-3, "mono is the average: {worst}");
        // Add to Session: a new track with the bounce at the range's start.
        let before: Vec<_> = e.session().tracks.iter().map(|t| t.id).collect();
        let r = e.execute("file.bounce_mix", &json!({"path": p("i.wav"), "start": start, "end": end, "import": true})).unwrap();
        assert!(!r["imported"].is_null());
        assert_eq!(e.session().tracks.len(), before.len() + 1);
        let t = e.session().tracks.iter().find(|t| !before.contains(&t.id)).unwrap();
        let first = t.playlists.iter().flat_map(|pl| pl.clips.iter()).map(|c| c.start).min();
        assert_eq!(first, Some(start), "{r}");
        // Bad options are refused before anything is written.
        for bad in [json!({"sample_rate": 1}), json!({"channels": "quad"}), json!({"dither": "loud"}), json!({"also": ["xyz"]})] {
            let mut q = bad.clone();
            q["path"] = json!(p("bad.wav"));
            assert!(e.execute("file.bounce_mix", &q).is_err(), "{bad}");
        }
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Stems of chosen tracks and of busses: only those files, a bus stem as the bus sums.
    #[test]
    fn stems_take_a_choice_of_tracks_and_busses() {
        let mut e = crate::demo::demo_engine();
        let bus = e.session_mut().add_bus("Drums", ChannelFormat::Stereo);
        let first = e.session().tracks.iter().find(|t| t.kind == soundcraft_model::TrackKind::Audio).unwrap().clone();
        e.session_mut().track_mut(first.id).unwrap().mixer.output = soundcraft_model::Route::Bus(bus);
        let dir = std::env::temp_dir().join(format!("sc-stems-choice-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let d = dir.to_string_lossy().into_owned();
        let files =
            e.execute("file.bounce_stems", &json!({"dir": d, "start": 0, "end": 48_000, "tracks": [first.id.0], "busses": ["Drums"]})).unwrap();
        let names: Vec<String> =
            files.as_array().or_else(|| files["files"].as_array()).unwrap().iter().map(|f| f.as_str().unwrap().to_string()).collect();
        assert_eq!(names.len(), 2, "{names:?}");
        assert!(names[0].ends_with(&format!(" - {}.wav", first.name)) && names[1].ends_with(" - Drums.wav"), "{names:?}");
        // The bus stem is the track through the bus: as loud as the track's own stem here.
        let dec = |f: &str| soundcraft_audio_io::decode(&std::fs::read(f).unwrap(), Some("wav")).unwrap().1.channels;
        let peak = |c: &[Vec<f32>]| c.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!(peak(&dec(&names[1])) > 0.0, "the bus stem has the track in it");
        assert!(e.execute("file.bounce_stems", &json!({"dir": d, "busses": ["Nope"]})).is_err());
        assert!(e.execute("file.bounce_stems", &json!({"dir": d, "tracks": []})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_audio_honours_new_tracks() {
        let dir = std::env::temp_dir().join(format!("sc-import-new-tracks-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = tone_wav(&dir, "tone.wav");

        // Default / new_tracks:true: create a new track even when one is selected.
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
        let target = e.session().tracks[0].id;
        e.session_mut().edit.selected_tracks = vec![target];
        e.execute("file.import_audio", &json!({"path": wav, "new_tracks": true})).unwrap();
        assert_eq!(e.session().tracks.len(), 2);
        assert_eq!(e.session().track(target).unwrap().clips().len(), 0);
        assert_eq!(e.session().tracks[1].clips().len(), 1);

        // new_tracks:false with a selected audio track: import onto it.
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
        let target = e.session().tracks[0].id;
        e.session_mut().edit.selected_tracks = vec![target];
        e.execute("file.import_audio", &json!({"path": wav, "new_tracks": false})).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(e.session().track(target).unwrap().clips().len(), 1);

        // new_tracks:false with no selection and no track: clear error.
        let mut e = Engine::default();
        let err = e.execute("file.import_audio", &json!({"path": wav, "new_tracks": false})).unwrap_err();
        assert!(err.to_string().contains("new_tracks: false"), "{err}");

        // Explicit track wins over new_tracks:true.
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 1, "name": "Target", "format": "mono"})).unwrap();
        e.execute("file.import_audio", &json!({"path": wav, "track": "Target", "new_tracks": true})).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(e.session().tracks[0].clips().len(), 1);

        // Omitted new_tracks still creates a track on a blank session.
        let mut e = Engine::default();
        e.execute("file.import_audio", &json!({"path": wav})).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(e.session().tracks[0].kind, TrackKind::Audio);
        assert_eq!(e.session().tracks[0].format, ChannelFormat::Mono);

        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn bounce_reaches_a_loudness_target_under_its_true_peak_ceiling() {
        let dir = std::env::temp_dir().join(format!("sc-bounce-loudness-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        // Three seconds of tone: integrated loudness needs more than a few gating blocks.
        let tone: Vec<f32> = (0..144_000).map(|i| (i as f32 * 0.0577).sin() * 0.2).collect();
        let wav = dir.join("tone.wav");
        std::fs::write(
            &wav,
            soundcraft_audio_io::encode(&AudioBuffer { sample_rate: 48_000, channels: vec![tone] }, &EncodeOptions::default()).unwrap(),
        )
        .unwrap();
        let mut e = Engine::default();
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy()})).unwrap();
        let bounce = |e: &mut Engine, name: &str, extra: Value| {
            let mut p = json!({"path": dir.join(name).to_string_lossy(), "start": 0, "end": 144_000, "bit_depth": "32f", "dither": false});
            p.as_object_mut().unwrap().extend(extra.as_object().unwrap().clone());
            e.execute("file.bounce_mix", &p)
        };
        let plain = bounce(&mut e, "plain.wav", json!({})).unwrap();
        assert!(plain["measured"].is_null());
        let as_mixed = plain["lufs"].as_f64().unwrap();
        // Quieter than the mix: reached exactly, the ceiling not in the way.
        let quiet = bounce(&mut e, "quiet.wav", json!({"loudness": -30.0})).unwrap();
        assert!((quiet["lufs"].as_f64().unwrap() + 30.0).abs() < 0.3, "{quiet}");
        assert!((quiet["measured"]["lufs"].as_f64().unwrap() - as_mixed).abs() < 0.3, "{quiet}");
        assert_eq!(quiet["measured"]["held_by_ceiling"], json!(false));
        // Louder than the ceiling allows: held at the ceiling, short of the target, and said so.
        let loud = bounce(&mut e, "loud.wav", json!({"loudness": -0.5, "true_peak": -1.0})).unwrap();
        assert_eq!(loud["measured"]["held_by_ceiling"], json!(true), "{loud}");
        assert!(loud["true_peak_db"].as_f64().unwrap() < -0.9, "{loud}");
        assert!(loud["lufs"].as_f64().unwrap() < -1.0, "{loud}");
        // Out of range.
        assert!(bounce(&mut e, "bad.wav", json!({"loudness": -60.0})).is_err());
        assert!(bounce(&mut e, "bad.wav", json!({"loudness": -14.0, "true_peak": 3.0})).is_err());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bounce_refuses_formats_it_cannot_write() {
        let mut e = Engine::default();
        let dir = std::env::temp_dir().join(format!("sc-bounce-ext-{}", std::process::id()));
        assert!(std::fs::create_dir_all(&dir).is_ok());
        for name in ["mix.ogg", "mix.m4a", "mix.xyz"] {
            let path = dir.join(name);
            let err = e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "start": 0, "end": 480})).map(|_| ()).unwrap_err();
            assert!(err.to_string().contains("supported: wav"), "{name}: {err}");
            assert!(!path.exists(), "{name} must not be written");
        }
        for ok in ["mix.flac", "mix.mp3", "mix.opus"] {
            let ok = dir.join(ok);
            assert!(e.execute("file.bounce_mix", &json!({"path": ok.to_string_lossy(), "start": 0, "end": 480})).is_ok(), "{}", ok.display());
        }
        assert!(std::fs::remove_dir_all(&dir).is_ok());
    }

    #[test]
    fn a_bounce_into_the_projects_bounced_files_makes_the_folder() {
        let mut e = Engine::default();
        let dir = std::env::temp_dir().join(format!("sc-bounce-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(dir.join("Song")).unwrap();
        let path = dir.join("Song").join("Bounced Files").join("Song Bounce.wav");
        e.execute("file.bounce_mix", &json!({"path": path.to_string_lossy(), "start": 0, "end": 480})).unwrap();
        assert!(path.is_file());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn bounce_source_renders_the_named_bus() {
        let dir = std::env::temp_dir().join(format!("sc-bounce-source-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let wav = tone_wav(&dir, "tone.wav");
        let mut e = Engine::default();
        e.execute("file.import_audio", &json!({"path": wav})).unwrap();
        let stem = e.session_mut().add_bus("Stem", ChannelFormat::Stereo);
        e.session_mut().add_bus("Empty", ChannelFormat::Stereo);
        let track = e.session().tracks[0].id;
        {
            let tr = e.session_mut().track_mut(track).unwrap();
            let mut send = soundcraft_model::SendSlot::new(soundcraft_model::Route::Bus(stem));
            send.level_db = -12.0;
            tr.mixer.sends[0] = Some(send);
        }
        let bounce = |e: &mut Engine, name: &str, source: Option<&str>| {
            let path = dir.join(name);
            let mut params = json!({"path": path.to_string_lossy(), "start": 0, "end": 4800, "bit_depth": "32f", "dither": false});
            if let Some(source) = source {
                params["source"] = json!(source);
            }
            let value = e.execute("file.bounce_mix", &params).unwrap();
            let peak = value.get("peak_db").and_then(serde_json::Value::as_f64).unwrap_or(0.0);
            // The samples only: the file's header carries the time of day (BWF `bext`), which
            // differs between two bounces made either side of a second's tick.
            let bytes = std::fs::read(&path).unwrap();
            let data = bytes.windows(4).position(|w| w == b"data").expect("a data chunk");
            (bytes[data..].to_vec(), peak)
        };
        let (main_bytes, main_peak) = bounce(&mut e, "main.wav", None);
        let (named_main, named_peak) = bounce(&mut e, "named-main.wav", Some("main"));
        let (stem_bytes, stem_peak) = bounce(&mut e, "stem.wav", Some("STEM"));
        let (empty_bytes, empty_peak) = bounce(&mut e, "empty.wav", Some("Empty"));
        assert_eq!(main_bytes, named_main);
        assert!((main_peak - named_peak).abs() < 1e-4, "{main_peak} vs {named_peak}");
        assert!(main_peak > -20.0, "{main_peak}");
        assert_ne!(stem_bytes, main_bytes);
        assert!(main_peak - stem_peak > 6.0 && main_peak - stem_peak < 18.0, "main {main_peak} stem {stem_peak}");
        assert_ne!(empty_bytes, main_bytes);
        assert!(empty_peak < -100.0, "{empty_peak}");
        // A hardware output by name (nothing routed to it here: silent), or `output:` and its name.
        e.session_mut().outputs.push(soundcraft_model::OutputPath {
            name: "Cue".into(),
            first_channel: 2,
            format: soundcraft_model::ChannelFormat::Stereo,
        });
        let (cue_bytes, cue_peak) = bounce(&mut e, "cue.wav", Some("Cue"));
        assert_eq!(cue_bytes, bounce(&mut e, "cue2.wav", Some("output:Cue")).0);
        assert!(cue_peak < -100.0, "{cue_peak}");
        let err = e.execute("file.bounce_mix", &json!({"path": dir.join("x.wav").to_string_lossy(), "source": "output:Nope"})).unwrap_err();
        assert!(err.to_string().contains("unknown output"), "{err}");
        let missing = dir.join("missing.wav");
        let err = e
            .execute("file.bounce_mix", &json!({"path": missing.to_string_lossy(), "source": "NoSuchBus", "start": 0, "end": 4800, "dither": false}))
            .unwrap_err();
        assert!(err.to_string().contains("unknown bus"), "{err}");
        assert!(!missing.exists());
        assert!(std::fs::remove_dir_all(&dir).is_ok());
    }

    fn remove_media(dir: &std::path::Path) {
        for entry in std::fs::read_dir(dir).into_iter().flatten().flatten() {
            let p = entry.path();
            if p.is_dir() {
                remove_media(&p);
            } else if !soundcraft_model::is_session_path(&p) {
                let _ = std::fs::remove_file(&p);
            }
        }
    }

    #[test]
    fn session_open_reports_missing_media() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-open-missing-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Gone.scraft").to_string_lossy().into_owned();
        e.execute("session.save_as", &json!({"path": path})).unwrap();
        let mut ok = Engine::default();
        assert!(ok.execute("session.open", &json!({"path": path})).unwrap()["missing"].as_array().is_some_and(Vec::is_empty));
        remove_media(&dir);
        let mut r = Engine::default();
        let res = r.execute("session.open", &json!({"path": path})).unwrap();
        assert!(res["missing"].as_array().is_some_and(|m| !m.is_empty()), "{res}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn saving_a_session_without_audio_creates_its_folder() {
        let dir = std::env::temp_dir().join(format!("soundcraft-new-folder-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let path = dir.join("Song").join("Song.scraft");
        Engine::default().execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
        assert!(path.exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    fn tone_engine() -> (Engine, u64) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 1, "name": "Tone", "format": "mono"})).unwrap();
        let track = e.session().tracks[0].id;
        let tone: Vec<f32> = (0..4_800).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let src = crate::io::add_source(e.session_mut(), "tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let id = e.session_mut().new_clip_id();
        crate::edit::place_clip(e.session_mut(), track, soundcraft_model::Clip::audio(id, "tone", src, 0, 0, 4_800));
        e.session_mut().edit.selected_tracks = vec![track];
        (e, track.0)
    }

    #[test]
    fn same_named_clips_export_as_separate_files() {
        let (mut e, track) = tone_engine();
        let first = e.session().tracks[0].clips()[0].id;
        let src = e.session().tracks[0].clips()[0].source().unwrap();
        let second = e.session_mut().new_clip_id();
        crate::edit::place_clip(
            e.session_mut(),
            soundcraft_model::TrackId(track),
            soundcraft_model::Clip::audio(second, "tone", src, 0, 10_000, 4_800),
        );
        for id in [first, second] {
            e.session_mut().find_clip_mut(id).unwrap().name = "Take".into();
        }
        let dir = std::env::temp_dir().join(format!("sc-export-clips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let n = e.execute("file.export_clips", &json!({"dir": dir.to_string_lossy(), "clips": [first.0, second.0]})).unwrap();
        assert_eq!(n["written"], 2);
        let mut wavs: Vec<_> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
        wavs.sort();
        assert_eq!(wavs, vec!["Take-2.wav".to_string(), "Take.wav".to_string()]);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn exports_the_selected_clips_and_refuses_an_empty_selection() {
        let mut e = crate::demo::demo_engine();
        let dir = std::env::temp_dir().join(format!("soundcraft-clips-{}", std::process::id()));
        let p = json!({"dir": dir.to_string_lossy()});
        e.execute("edit.select_none", &json!({})).unwrap();
        assert!(e.execute("file.export_clips", &p).is_err());
        e.execute("edit.select", &json!({"tracks": ["Kick"], "start": {"seconds": 10.0}, "end": {"seconds": 11.0}})).unwrap();
        let r = e.execute("file.export_clips", &p).unwrap();
        let files = std::fs::read_dir(&dir).map_or(0, Iterator::count);
        let _ = std::fs::remove_dir_all(&dir);
        assert!(files >= 1 && r["written"] == files, "{r}, {files} files");
    }

    fn midi_engine() -> Engine {
        use soundcraft_midi::{CtrlEvent, CtrlKind, Note, Sequence};
        use soundcraft_model::{ChannelFormat, Clip, TrackKind};
        let mut s = Session::default();
        let keys = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
        let drums = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Drums"));
        let _audio = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Room"));
        let mut keys_seq = Sequence::default();
        keys_seq.notes.push(Note {
            probability: 100,
            pitch: 60,
            velocity: 100,
            muted: false,
            release_velocity: 64,
            channel: 0,
            start: 0,
            length: 480,
        });
        keys_seq.ctrls.push(CtrlEvent { tick: 10, channel: 0, kind: CtrlKind::Cc(7), value: 90, shape: Default::default() });
        let mut drum_seq = Sequence::default();
        drum_seq.notes.push(Note {
            probability: 100,
            pitch: 36,
            velocity: 110,
            muted: false,
            release_velocity: 64,
            channel: 9,
            start: 0,
            length: 120,
        });
        drum_seq.ctrls.push(CtrlEvent { tick: 20, channel: 9, kind: CtrlKind::Cc(11), value: 40, shape: Default::default() });
        let a = s.new_clip_id();
        let b = s.new_clip_id();
        crate::edit::place_clip(&mut s, keys, Clip::midi(a, "Keys", 0, 48_000, keys_seq));
        crate::edit::place_clip(&mut s, drums, Clip::midi(b, "Drums", 0, 48_000, drum_seq));
        Engine::new(s)
    }

    fn cc_numbers(bytes: &[u8]) -> Vec<(String, u8, i32)> {
        let smf = soundcraft_midi::read_smf(bytes).unwrap();
        let mut out = Vec::new();
        for t in &smf.tracks {
            for c in &t.sequence.ctrls {
                if let soundcraft_midi::CtrlKind::Cc(n) = c.kind {
                    out.push((t.name.clone(), n, c.value));
                }
            }
        }
        out
    }

    #[test]
    fn midi_export_keeps_controllers_and_honours_tracks() {
        let mut e = midi_engine();
        let dir = std::env::temp_dir().join(format!("sc-midi-export-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let all = dir.join("all.mid");
        e.execute("file.export_midi", &json!({"path": all.to_string_lossy()})).unwrap();
        let ctrls = cc_numbers(&std::fs::read(&all).unwrap());
        assert!(ctrls.iter().any(|(n, cc, v)| n == "Keys" && *cc == 7 && *v == 90), "{ctrls:?}");
        assert!(ctrls.iter().any(|(n, cc, v)| n == "Drums" && *cc == 11 && *v == 40), "{ctrls:?}");
        assert!(ctrls.iter().all(|(n, _, _)| n != "Room"));

        let keys = dir.join("keys.mid");
        e.execute("file.export_midi", &json!({"path": keys.to_string_lossy(), "tracks": ["Keys"]})).unwrap();
        let ctrls = cc_numbers(&std::fs::read(&keys).unwrap());
        assert_eq!(ctrls.len(), 1, "{ctrls:?}");
        assert_eq!(ctrls[0].0, "Keys");

        let err = e.execute("file.export_midi", &json!({"path": dir.join("nope.mid").to_string_lossy(), "tracks": ["Room"]})).unwrap_err();
        assert!(err.to_string().contains("none of the requested tracks hold MIDI"), "{err}");

        // Export Clip as MIDI: the Keys clip alone.
        let keys_clip = e.session().tracks.iter().find(|t| t.name == "Keys").and_then(|t| t.clips().first()).map(|c| c.id.0).unwrap();
        let one = dir.join("clip.mid");
        e.execute("file.export_clip_midi", &json!({"path": one.to_string_lossy(), "clips": [keys_clip]})).unwrap();
        let ctrls = cc_numbers(&std::fs::read(&one).unwrap());
        assert_eq!(ctrls.iter().map(|c| c.0.as_str()).collect::<Vec<_>>(), vec!["Keys"], "{ctrls:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn export_clip_as_midi_starts_at_the_earliest_clip() {
        let mut e = midi_engine();
        let ids: Vec<u64> = e.session().tracks.iter().filter_map(|t| t.clips().first().map(|c| c.id.0)).collect();
        // Keys moved to bar 3 (two bars of 4/4 at 120 bpm: 4 s), Drums to bar 5.
        let sr = e.session().sample_rate.as_f64() as i64;
        let (keys, drums) = (ids[0], ids[1]);
        e.session_mut().find_clip_mut(soundcraft_model::ClipId(keys)).unwrap().start = 4 * sr;
        e.session_mut().find_clip_mut(soundcraft_model::ClipId(drums)).unwrap().start = 8 * sr;
        let q = i64::from(soundcraft_midi::PPQ);
        let dir = std::env::temp_dir().join(format!("sc-clip-midi-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let first_note = |e: &mut Engine, clips: Vec<u64>| {
            let path = dir.join("clip.mid");
            e.execute("file.export_clip_midi", &json!({"path": path.to_string_lossy(), "clips": clips})).unwrap();
            let smf = soundcraft_midi::read_smf(&std::fs::read(&path).unwrap()).unwrap();
            smf.tracks.iter().map(|t| (t.name.clone(), t.sequence.notes.first().map(|n| n.start))).collect::<Vec<_>>()
        };
        assert_eq!(first_note(&mut e, vec![drums]), vec![("Drums".to_string(), Some(0))], "one clip starts the file");
        assert_eq!(
            first_note(&mut e, vec![keys, drums]),
            vec![("Keys".to_string(), Some(0)), ("Drums".to_string(), Some(8 * q))],
            "several keep their spacing from the earliest"
        );
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn import_surround_creates_a_matching_track() {
        let channels: Vec<Vec<f32>> = (0..6).map(|ch| vec![0.1 * (ch as f32 + 1.0); 32]).collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let mut e = Engine::default();
        let v = crate::io::import_audio_bytes(&mut e, "bed.wav", &bytes, None, None, 0).unwrap();
        assert_eq!(v["channels"], 6);
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(e.session().tracks[0].format, ChannelFormat::Surround51);
        let src = e.session().sources[0].id;
        let audio = e.session().pool.get(src).unwrap();
        assert_eq!(audio.buffer().channels.len(), 6);
        assert!((audio.buffer().channels[5][0] - 0.6).abs() < 1e-3);

        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Bed", "format": "5.1"})).unwrap();
        let id = e.session().tracks[0].id;
        crate::io::import_audio_bytes(&mut e, "bed.wav", &bytes, None, Some(id), 0).unwrap();
        assert_eq!(e.session().tracks.len(), 1);
        assert_eq!(e.session().tracks[0].format, ChannelFormat::Surround51);
        assert_eq!(e.session().tracks[0].clips().len(), 1);
    }

    /// A loop that says its tempo and key in its name is stretched to the session tempo (pitch
    /// kept) and transposed to the session key; with the option off it comes in as it is, its
    /// tempo still recorded for Conform to Tempo.
    #[test]
    fn an_imported_loop_follows_the_session_tempo_and_key() {
        let tone: Vec<f32> = (0..96_000).map(|i| (std::f32::consts::TAU * 440.0 * i as f32 / 48_000.0).sin() * 0.5).collect();
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![tone] };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let mut e = Engine::default();
        assert!(e.session().edit.flag(crate::io::LOOPS_FOLLOW), "on in a new session");
        e.session_mut().key_signatures.push((0, "D minor".into()));
        let now = e.session().tempo.tempo_at_tick(0);
        let v = crate::io::import_audio_bytes(&mut e, "Keys 60bpm Am.wav", &bytes, None, None, 0).unwrap();
        assert_eq!(v["loop"]["bpm"], 60.0);
        assert_eq!(v["loop"]["semitones"], 5, "A minor up a fourth to D minor");
        let cid = soundcraft_model::ClipId(v["clip"].as_u64().unwrap());
        let c = e.session().find_clip(cid).unwrap().1.clone();
        let want = 96_000.0 * 60.0 / now;
        assert!((c.length as f64 - want).abs() < 4.0, "{} vs {want}", c.length);
        assert_eq!(c.transpose_cents, 500);
        // Off: as recorded, the tempo kept for later.
        e.execute("options.loops_follow", &json!({"value": false})).unwrap();
        let v = crate::io::import_audio_bytes(&mut e, "Keys 60bpm Am.wav", &bytes, None, None, 0).unwrap();
        let cid = soundcraft_model::ClipId(v["clip"].as_u64().unwrap());
        let c = e.session().find_clip(cid).unwrap().1.clone();
        assert_eq!((c.length, c.transpose_cents), (96_000, 0));
        assert_eq!(e.session().edit.value(&format!("clip.bpm.{}", cid.0), 0.0), 60.0);
        // A file that says nothing (and has no beat to find).
        e.execute("options.loops_follow", &json!({"value": true})).unwrap();
        let v = crate::io::import_audio_bytes(&mut e, "Take 1.wav", &bytes, None, None, 0).unwrap();
        assert!(v["loop"].is_null(), "{v}");
    }

    /// A short file that says no tempo has it detected at import and recorded on the clip,
    /// which keeps its length (nothing is stretched on a guess).
    #[test]
    fn a_short_import_has_its_tempo_detected_and_recorded() {
        let sr = 48_000usize;
        let beat = sr * 60 / 100;
        let positions: Vec<usize> = (0..16).map(|k| k * beat + 2_000).collect();
        let clicks = soundcraft_dsp::beat::click_track(17 * beat, &positions, sr as f32, 4);
        let len = clicks.len() as i64;
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![clicks] };
        let bytes = soundcraft_audio_io::encode(&buf, &EncodeOptions::default()).unwrap();
        let mut e = Engine::default();
        let v = crate::io::import_audio_bytes(&mut e, "Take 2.wav", &bytes, None, None, 0).unwrap();
        let bpm = v["loop"]["detected_bpm"].as_f64().unwrap_or(0.0);
        assert!((bpm - 100.0).abs() < 1.5, "{v}");
        let cid = v["clip"].as_u64().unwrap();
        assert_eq!(e.session().edit.value(&format!("clip.bpm.{cid}"), 0.0), bpm);
        assert_eq!(e.session().find_clip(soundcraft_model::ClipId(cid)).unwrap().1.length, len);
    }

    #[test]
    fn repeated_consolidate_keeps_distinct_audio_files() {
        let (mut e, track) = tone_engine();
        e.execute("edit.consolidate", &json!({"track": track, "start": 0, "end": 4800})).unwrap();
        let cid = e.session().tracks[0].clips()[0].id;
        e.execute("clip.gain", &json!({"clip": cid.0, "db": -24.0})).unwrap();
        e.execute("edit.consolidate", &json!({"track": track, "start": 0, "end": 4800})).unwrap();
        let consolidated: Vec<_> = e.session().sources.iter().filter(|s| s.name.contains("consolidated")).map(|s| s.path.clone()).collect();
        assert_eq!(consolidated.len(), 2, "{consolidated:?}");
        assert_ne!(consolidated[0], consolidated[1]);

        let dir = std::env::temp_dir().join(format!("sc-consolidate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("Tone.scraft");
        e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
        let mut opened = Engine::default();
        opened.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        let files: Vec<_> = opened.session().sources.iter().filter(|s| s.name.contains("consolidated")).map(|s| dir.join(&s.path)).collect();
        assert_eq!(files.len(), 2, "{:?}", opened.session().sources);
        let a = std::fs::read(&files[0]).unwrap();
        let b = std::fs::read(&files[1]).unwrap();
        assert_ne!(a, b, "the second consolidation replaced the first file");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn save_copy_leaves_the_open_session_alone() {
        let (mut e, _) = tone_engine();
        let dir = std::env::temp_dir().join(format!("sc-save-copy-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let original = dir.join("Original.scraft");
        e.execute("session.save_as", &json!({"path": original.to_string_lossy()})).unwrap();
        e.execute("track.new", &json!({"count": 1, "name": "Extra", "format": "mono"})).unwrap();
        assert!(e.is_dirty());
        let copy_dir = dir.join("copy");
        std::fs::create_dir_all(&copy_dir).unwrap();
        let copy = copy_dir.join("Copy.scraft");
        e.execute("session.save_copy", &json!({"path": copy.to_string_lossy()})).unwrap();
        assert_eq!(e.path.as_deref(), Some(original.to_string_lossy().as_ref()));
        assert_eq!(e.session().name, "Original");
        assert!(e.is_dirty());
        assert_eq!(e.session().tracks.len(), 2);

        let mut from_original = Engine::default();
        from_original.execute("session.open", &json!({"path": original.to_string_lossy()})).unwrap();
        assert_eq!(from_original.session().tracks.len(), 1);
        let mut from_copy = Engine::default();
        from_copy.execute("session.open", &json!({"path": copy.to_string_lossy()})).unwrap();
        assert_eq!(from_copy.session().tracks.len(), 2);
        assert!(from_copy.session().track_by_name("Extra").is_some());
        let _ = std::fs::remove_dir_all(&dir);
    }
}

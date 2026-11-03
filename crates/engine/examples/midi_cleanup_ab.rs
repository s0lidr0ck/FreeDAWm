//! MIDI Cleanup on real material: transcribes a piano excerpt (PianoKong, with the sustain pedal),
//! detects its chords, cleans the MIDI at the default settings and renders an A/B pair through the
//! same instrument (the original transcription and the cleaned one), plus the source audio of the
//! same excerpt, with a report of what the cleanup did.
//!
//! `cargo run --release -p soundcraft-engine --example midi_cleanup_ab -- <wav> [start_s] [dur_s] <outdir>`
//! (or set `MIDI_CLEANUP_WAV` and `MIDI_CLEANUP_OUT`; there are no defaults).
//!
//! The instrument is a real piano when the VST3 scan cache (`<config>/PluginScan/vst3.json`) lists
//! one (Pianoteq, Keyscape, a grand...), else the Subtractive "Keys" voice Convert to MIDI puts on
//! the track. With no scan cache yet, `FREEDAWM_SCAN_HELPER=<target>/release/freedawm-scan.exe`
//! makes the tool scan first (out of process, as the app does), which writes the cache.

#[cfg(target_arch = "wasm32")]
fn main() {
    println!("midi_cleanup_ab: desktop only");
}

#[cfg(not(target_arch = "wasm32"))]
fn main() -> std::process::ExitCode {
    match desktop::run() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("midi_cleanup_ab: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
mod desktop {
    use serde_json::{Value, json};
    use soundcraft_engine::Engine;
    use soundcraft_midi::cleanup::{ChordSpan, EditKind, NoteEdit};
    use soundcraft_midi::{Note, PPQ};
    use soundcraft_model::{ClipContent, ClipId};
    use std::fmt::Write as _;
    use std::path::{Path, PathBuf};

    const USAGE: &str = "usage: midi_cleanup_ab <wav> [start_s] [dur_s] <outdir> (or set MIDI_CLEANUP_WAV and MIDI_CLEANUP_OUT)";
    const NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    /// Real pianos, best first, matched case-insensitively against VST3 instrument names.
    const PIANOS: [&str; 9] = ["pianoteq", "keyscape", "noire", "una corda", "grand", "piano", "ivory", "rhodes", "keys"];

    fn exec(e: &mut Engine, id: &str, p: Value) -> Result<Value, String> {
        e.execute(id, &p).map_err(|err| format!("{id}: {err}"))
    }

    fn pitch_name(p: u8) -> String {
        format!("{}{}", NAMES.get(usize::from(p % 12)).copied().unwrap_or("?"), i32::from(p) / 12 - 1)
    }

    /// `m:ss.s` of a time in seconds.
    fn clock(secs: f64) -> String {
        let m = (secs / 60.0).floor();
        format!("{m:.0}:{:04.1}", secs - m * 60.0)
    }

    fn tones(t: &[bool; 12]) -> String {
        NAMES.iter().zip(t).filter(|(_, on)| **on).map(|(n, _)| *n).collect::<Vec<_>>().join(" ")
    }

    /// `<config>/PluginScan`, as the app and the CLI keep it.
    fn plugin_scan_dir() -> Option<PathBuf> {
        let config = match std::env::var_os("SOUNDCRAFT_CONFIG_DIR") {
            Some(d) => (!d.is_empty()).then(|| PathBuf::from(d)),
            None if cfg!(target_os = "macos") => std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Library/Application Support/SoundCraft")),
            None if cfg!(windows) => std::env::var_os("APPDATA").map(|a| PathBuf::from(a).join("SoundCraft")),
            None => std::env::var_os("XDG_CONFIG_HOME")
                .map(PathBuf::from)
                .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
                .map(|c| c.join("soundcraft")),
        };
        config.map(|c| c.join("PluginScan"))
    }

    /// The best real piano among the scanned VST3 instruments: (id, name).
    fn find_piano() -> Option<(String, String)> {
        let dir = plugin_scan_dir();
        println!("VST3 scan cache: {}", dir.as_ref().map_or("(none)".into(), |d| d.join("vst3.json").display().to_string()));
        soundcraft_engine::set_plugin_scan_cache_dir(dir);
        soundcraft_engine::set_blocking_plugin_lookups(Some(|done, total| eprintln!("  plugin scan {done}/{total}")));
        let list = match soundcraft_vst3_host::cached() {
            Some(l) => l,
            None => {
                let helper = std::env::var_os("FREEDAWM_SCAN_HELPER")
                    .or_else(|| std::env::var_os("SOUNDCRAFT_SCAN_HELPER"))
                    .map(PathBuf::from)
                    .filter(|p| p.is_file());
                match helper {
                    Some(h) => {
                        println!("no VST3 scan cache yet: scanning through {} (writes the cache)", h.display());
                        soundcraft_vst3_host::scan()
                    }
                    None => {
                        println!("no VST3 scan cache yet (set FREEDAWM_SCAN_HELPER to the freedawm-scan helper to make one)");
                        Vec::new()
                    }
                }
            }
        };
        let instruments: Vec<_> = list.iter().filter(|d| d.is_instrument).collect();
        PIANOS.iter().find_map(|pat| {
            instruments.iter().find(|d| d.name.to_ascii_lowercase().contains(pat)).map(|d| (d.id.clone(), format!("{} ({})", d.name, d.vendor)))
        })
    }

    /// The `edit` kind as the report names it.
    fn kind_name(k: EditKind) -> &'static str {
        match k {
            EditKind::WrongNote => "wrong note",
            EditKind::HeldOver => "held over",
            EditKind::LegatoTrim => "legato trim",
            EditKind::LegatoClose => "legato close",
            EditKind::Extended => "extended",
            EditKind::PedalCatch => "pedal catch",
        }
    }

    /// The clip's original notes and its preview edits.
    fn notes_and_edits(e: &Engine, clip: ClipId) -> (Vec<Note>, Vec<NoteEdit>) {
        let Some((_, c)) = e.session().find_clip(clip) else { return (Vec::new(), Vec::new()) };
        let notes = match &c.content {
            ClipContent::Midi { sequence } => sequence.notes.clone(),
            _ => Vec::new(),
        };
        let edits = c.midi_cleanup.as_ref().and_then(|m| m.preview.as_ref()).map(|p| p.out.edits.clone()).unwrap_or_default();
        (notes, edits)
    }

    fn chord_at(chords: &[ChordSpan], t: i64) -> Option<&ChordSpan> {
        chords.iter().find(|c| c.start <= t && t < c.end)
    }

    /// The `report` object of a `midi.cleanup` result.
    fn report_of(v: &Value) -> Value {
        v.pointer("/clips/0/report").cloned().unwrap_or(Value::Null)
    }

    fn count(r: &Value, k: &str) -> u64 {
        r.get(k).and_then(Value::as_u64).unwrap_or(0)
    }

    /// Prints a line and keeps it for report.txt.
    fn line(rep: &mut String, s: String) {
        println!("{s}");
        rep.push_str(&s);
        rep.push('\n');
    }

    pub fn run() -> Result<(), String> {
        let a: Vec<String> = std::env::args().skip(1).collect();
        // No default source or output: the WAV and the folder come from the arguments or
        // `MIDI_CLEANUP_WAV` / `MIDI_CLEANUP_OUT`.
        let from = |i: usize, var: &str| a.get(i).cloned().or_else(|| std::env::var(var).ok()).filter(|s| !s.is_empty());
        let wav = from(0, "MIDI_CLEANUP_WAV").ok_or(USAGE)?;
        let start_s: f64 = a.get(1).map_or(Ok(410.0), |s| s.parse().map_err(|_| format!("bad start `{s}`")))?;
        let dur_s: f64 = a.get(2).map_or(Ok(30.0), |s| s.parse().map_err(|_| format!("bad duration `{s}`")))?;
        let out = PathBuf::from(from(3, "MIDI_CLEANUP_OUT").ok_or(USAGE)?);
        if !(start_s.is_finite() && start_s >= 0.0 && dur_s.is_finite() && dur_s > 0.0) {
            return Err("start must be ≥ 0 and the duration > 0".into());
        }
        std::fs::create_dir_all(&out).map_err(|e| format!("{}: {e}", out.display()))?;
        let mut rep = String::new();

        line(&mut rep, format!("source: {wav}, {} to {} ({start_s} s + {dur_s} s)", clock(start_s), clock(start_s + dur_s)));
        let piano = find_piano();
        let instrument = match &piano {
            Some((id, name)) => format!("{name} [{id}] (VST3, real piano)"),
            None => "Subtractive Synth, Keys voice (built in; no real piano in the VST3 scan cache)".to_string(),
        };
        line(&mut rep, format!("instrument: {instrument}"));

        let mut e = Engine::default();
        exec(&mut e, "session.new", json!({"name": "MIDI Cleanup A/B", "sample_rate": 48000}))?;
        let bpm = e.session().tempo.tempo_at_tick(0);
        line(
            &mut rep,
            format!(
                "tempo: {bpm:.0} bpm (the session default; the chord grid and the Legato 8th / Extend 16th limits assume it, the take is not beat-mapped)"
            ),
        );
        let rate = e.session().sample_rate;
        exec(&mut e, "file.import_audio", json!({"path": wav, "at": 0}))?;
        let audio_track = e.session().tracks.first().map(|t| t.id).ok_or("the import made no track")?;
        let (a0, a1) = (rate.samples(start_s), rate.samples(start_s + dur_s));
        exec(&mut e, "edit.trim_to_selection", json!({"tracks": [audio_track.0], "start": a0, "end": a1}))?;
        let audio_clip = e.session().track(audio_track).and_then(|t| t.clips().first()).map(|c| c.id).ok_or("the trim left no clip")?;

        let t0 = std::time::Instant::now();
        let m = exec(&mut e, "ml.midi", json!({"clip": audio_clip.0, "model": "piano"}))?;
        let made = m.pointer("/made/0").cloned().ok_or("Convert to MIDI made nothing")?;
        let midi_track = made.get("track").and_then(Value::as_u64).ok_or("no MIDI track")?;
        let midi_clip = ClipId(made.get("clip").and_then(Value::as_u64).ok_or("no MIDI clip")?);
        line(
            &mut rep,
            format!(
                "transcribed (piano, pedal on) in {:.1} s: {} notes, {} pedal events",
                t0.elapsed().as_secs_f64(),
                count(&made, "notes"),
                count(&made, "pedals")
            ),
        );

        let c = exec(&mut e, "ml.chords", json!({"clip": audio_clip.0}))?;
        let c = c.pointer("/made/0").cloned().ok_or("Detect Chords placed nothing")?;
        let placed = c.get("chords").and_then(Value::as_array).cloned().unwrap_or_default();
        let key = c.get("key").and_then(Value::as_str).unwrap_or("?").to_string();
        // `~`: the lane could not hold the detected chord exactly in the key (the nearest is used).
        let names: Vec<String> = placed
            .iter()
            .filter_map(|x| {
                let exact = x.get("exact").and_then(Value::as_bool).unwrap_or(true);
                x.get("name").and_then(Value::as_str).map(|n| if exact { n.to_string() } else { format!("~{n}") })
            })
            .collect();
        line(&mut rep, format!("chords: {} found, key {key} (set: {})", placed.len(), c.get("key_set").and_then(Value::as_bool).unwrap_or(false)));
        line(&mut rep, format!("  {}", names.join(" | ")));
        if names.iter().any(|n| n.starts_with('~')) {
            line(&mut rep, "  (~ = not exact in the key: the lane holds the nearest chord, which the cleanup judges against)".into());
        }

        // The clip's view of the chord track and the tempo, for the per-note listing.
        let (chords, tpm, key_tones) = {
            let s = e.session();
            let clip = s.find_clip(midi_clip).map(|(_, c)| c).ok_or("the MIDI clip is gone")?;
            (
                soundcraft_engine::midi_cleanup::chord_spans(s, clip),
                soundcraft_engine::midi_cleanup::ticks_per_ms(s, clip),
                soundcraft_engine::midi_cleanup::key_tones(s),
            )
        };
        let changes = chords.windows(2).filter(|w| matches!(w, [p, n] if p.end == n.start)).count();
        line(&mut rep, format!("  chord changes in the excerpt: {changes}; key tones: {}", tones(&key_tones)));
        let secs_at = |tick: i64| start_s + tick as f64 / tpm / 1000.0;
        let lane: Vec<String> = chords.iter().map(|c| format!("{} [{}]", clock(secs_at(c.start.max(0))), tones(&c.tones))).collect();
        line(&mut rep, format!("  lane as the cleanup reads it: {}", lane.join(", ")));
        let bar_ticks = i64::from(PPQ) * 4;

        // The cleanup at the defaults.
        let r = exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0}))?;
        let settings = r.pointer("/clips/0/settings").cloned().unwrap_or(Value::Null);
        let report = report_of(&r);
        let summary = r.pointer("/clips/0/summary").and_then(Value::as_str).unwrap_or("").to_string();
        line(&mut rep, format!("settings: {settings}"));
        for k in ["total", "removed", "held_trimmed", "legato_trimmed", "legato_closed", "extended", "pedal_changes"] {
            line(&mut rep, format!("  {k}: {}", count(&report, k)));
        }
        line(&mut rep, format!("summary: {summary}"));

        // Listening proxies: removals per bar, held-over trims per chord change, and each removal.
        let (notes, edits) = notes_and_edits(&e, midi_clip);
        let total = count(&report, "total").max(1);
        let removed = count(&report, "removed");
        let bars = (dur_s * bpm / 60.0 / 4.0).max(1.0);
        line(
            &mut rep,
            format!(
                "per bar ({bars:.1} bars at {bpm:.0} bpm): {:.2} notes, {:.2} removed; held-over trims per chord change: {:.2}; pedal changes per chord change: {:.2}",
                total as f64 / bars,
                removed as f64 / bars,
                count(&report, "held_trimmed") as f64 / changes.max(1) as f64,
                count(&report, "pedal_changes") as f64 / changes.max(1) as f64,
            ),
        );
        let mut per_bar = vec![0usize; bars.ceil() as usize];
        let describe = |n: &Note| {
            let ch = chord_at(&chords, n.start);
            format!(
                "{} {:<4} vel {:>3} len {:>4.0} ms  over {}{}",
                clock(secs_at(n.start)),
                pitch_name(n.pitch),
                n.velocity,
                n.length as f64 / tpm,
                ch.map_or("(no chord)".to_string(), |c| format!("[{}]", tones(&c.tones))),
                if key_tones[usize::from(n.pitch % 12)] { "" } else { "  (out of key)" }
            )
        };
        line(&mut rep, "removed notes:".into());
        for ed in edits.iter().filter(|x| x.new_end.is_none()) {
            if let Some(n) = notes.get(ed.index) {
                if let Some(b) = per_bar.get_mut(usize::try_from(n.start / bar_ticks).unwrap_or(0)) {
                    *b += 1;
                }
                line(&mut rep, format!("  {}", describe(n)));
            }
        }
        line(&mut rep, format!("  removals per bar: {per_bar:?}"));
        let mut kinds: Vec<(&str, usize)> = Vec::new();
        for ed in &edits {
            let k = kind_name(ed.kind);
            match kinds.iter_mut().find(|(n, _)| *n == k) {
                Some((_, c)) => *c += 1,
                None => kinds.push((k, 1)),
            }
        }
        line(&mut rep, format!("edits by stage (first stage to touch a note): {kinds:?}"));
        let held_cut_ms: Vec<f64> =
            edits.iter().filter(|x| x.kind == EditKind::HeldOver).filter_map(|x| x.new_end.map(|n| (x.old_end - n) as f64 / tpm)).collect();
        if !held_cut_ms.is_empty() {
            let mean = held_cut_ms.iter().sum::<f64>() / held_cut_ms.len() as f64;
            line(&mut rep, format!("held-over cuts: mean {mean:.0} ms shorter, longest {:.0} ms", held_cut_ms.iter().copied().fold(0.0, f64::max)));
        }

        // What Protect melody spares: the same settings without it.
        let off = exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "protect_melody": false}))?;
        let (_, edits_off) = notes_and_edits(&e, midi_clip);
        line(&mut rep, format!("without Protect melody: removed {} (with: {removed})", count(&report_of(&off), "removed")));
        let removed_on: Vec<usize> = edits.iter().filter(|x| x.new_end.is_none()).map(|x| x.index).collect();
        for ed in edits_off.iter().filter(|x| x.new_end.is_none() && !removed_on.contains(&x.index)) {
            if let Some(n) = notes.get(ed.index) {
                line(&mut rep, format!("  spared by protection: {}", describe(n)));
            }
        }
        // How the counts move with Wrong notes and Held over (the rest at the defaults).
        let mut sweep = String::from("Wrong notes sweep (removed):");
        let mut next_step = Vec::new();
        for w in [0, 20, 40, 60, 80, 100] {
            let v = exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "reset": true, "wrong": w}))?;
            let _ = write!(sweep, " {w}%={}", count(&report_of(&v), "removed"));
            if w == 60 {
                next_step = notes_and_edits(&e, midi_clip).1;
            }
        }
        line(&mut rep, sweep);
        for ed in next_step.iter().filter(|x| x.new_end.is_none() && !removed_on.contains(&x.index)) {
            if let Some(n) = notes.get(ed.index) {
                line(&mut rep, format!("  also removed at 60 %: {}", describe(n)));
            }
        }
        let mut sweep = String::from("Held over sweep (held_trimmed/pedal_changes):");
        for h in [0, 25, 50, 70, 100] {
            let v = exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "reset": true, "held": h}))?;
            let r = report_of(&v);
            let _ = write!(sweep, " {h}%={}/{}", count(&r, "held_trimmed"), count(&r, "pedal_changes"));
        }
        line(&mut rep, sweep);
        let back = exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "reset": true}))?;
        if report_of(&back) != report {
            return Err("the cleanup at the defaults changed between calls".into());
        }

        // Tuning gate (spec: "If lots of notes disappear, it has failed").
        let removed_ok = removed * 100 <= total * 5;
        let held_ok = changes == 0 || count(&report, "held_trimmed") + count(&report, "pedal_changes") > 0;
        line(
            &mut rep,
            format!(
                "gate: removed {removed} of {total} = {:.1} % (≤ 5 %: {}); held_trimmed + pedal_changes = {} with {changes} chord changes ({}) → {}",
                removed as f64 * 100.0 / total as f64,
                if removed_ok { "ok" } else { "FAIL" },
                count(&report, "held_trimmed") + count(&report, "pedal_changes"),
                if held_ok { "ok" } else { "FAIL" },
                if removed_ok && held_ok { "PASS" } else { "FAIL" }
            ),
        );

        // The renders: the piano track alone, then the source audio alone.
        if let Some((id, _)) = &piano {
            exec(&mut e, "mix.instrument", json!({"track": midi_track, "plugin": id}))?;
        }
        exec(&mut e, "mix.mute", json!({"tracks": [audio_track.0], "value": true}))?;
        let bounce = |e: &mut Engine, path: &Path| -> Result<(), String> {
            exec(
                e,
                "file.bounce_mix",
                json!({"path": path.to_string_lossy(), "format": "wav", "bit_depth": 24, "start": a0, "end": a1, "normalize": false, "dither": false}),
            )?;
            println!("wrote {}", path.display());
            Ok(())
        };
        let original = out.join("piano-original.wav");
        let cleaned = out.join("piano-cleaned.wav");
        let excerpt = out.join("source-excerpt.wav");
        exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "original": true}))?;
        bounce(&mut e, &original)?;
        exec(&mut e, "midi.cleanup", json!({"clip": midi_clip.0, "original": false}))?;
        bounce(&mut e, &cleaned)?;
        exec(&mut e, "mix.mute", json!({"tracks": [audio_track.0], "value": false}))?;
        exec(&mut e, "mix.mute", json!({"tracks": [midi_track], "value": true}))?;
        bounce(&mut e, &excerpt)?;
        line(&mut rep, format!("renders: {}, {}, {}", original.display(), cleaned.display(), excerpt.display()));

        let report_path = out.join("report.txt");
        std::fs::write(&report_path, rep).map_err(|e| format!("{}: {e}", report_path.display()))?;
        println!("wrote {}", report_path.display());
        drop(e);
        soundcraft_engine::shutdown_plugin_hosts();
        Ok(())
    }
}

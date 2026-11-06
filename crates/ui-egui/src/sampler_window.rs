//! The Sampler in its instrument window, all on one page: the sample (Load Sample, From
//! Selection, Clear, the detected root note), the waveform with its start, end and loop markers
//! (which drag), the Pitch, Filter, Amp Envelope, LFO and Output knob panels, and a keyboard
//! with the key range and root note; and audio files dropped onto the window or onto an
//! instrument track. Every control goes through `mix.instrument_param` or a sampler command, so
//! it undoes and scripts like any edit.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Rect, Stroke, StrokeKind, Ui};
use serde_json::json;
use soundcraft_model::{Insert, TrackId};
use std::path::Path;

/// The built-in Sampler's plugin id.
pub const SAMPLER: &str = "sampler";

/// File extensions a sample can come from (what the audio decoder reads).
pub(crate) const AUDIO_EXTS: &[&str] = &["wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "m4a", "aac", "mp4", "caf"];

/// Markers on the sample strip: the parameter each one sets, and whether it needs a loop.
pub(crate) const MARKERS: [(&str, bool); 4] = [("start", false), ("end", false), ("loop_start", true), ("loop_end", true)];

/// Whether a file name looks like audio the sampler can load.
pub fn is_audio_name(name: &str) -> bool {
    Path::new(name).extension().and_then(|x| x.to_str()).is_some_and(|x| AUDIO_EXTS.contains(&x.to_ascii_lowercase().as_str()))
}

/// Sampler parameters that are MIDI notes.
const NOTE_PARAMS: [&str; 3] = ["root_note", "key_low", "key_high"];

/// Whether a built-in instrument's parameter only takes whole numbers (notes and velocities).
pub fn whole_number(plugin: &str, param: &str) -> bool {
    plugin == SAMPLER && (NOTE_PARAMS.contains(&param) || param == "vel_low" || param == "vel_high")
}

/// A built-in instrument's parameter as its window shows it: the sampler's notes by name.
pub fn format_param(plugin: &str, p: &soundcraft_dsp::ParamInfo, value: f32) -> String {
    if plugin == SAMPLER && NOTE_PARAMS.contains(&p.id) {
        let n = value.round().clamp(0.0, 127.0) as u8;
        return format!("{} ({n})", soundcraft_midi::note_name(n));
    }
    if whole_number(plugin, p.id) {
        return format!("{:.0}", value.round());
    }
    p.format(value)
}

pub(crate) fn param(ins: &Insert, id: &str) -> f32 {
    let default = soundcraft_dsp::plugin_info(SAMPLER).and_then(|i| i.param(id)).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().unwrap_or(default)
}

/// Remembers `r` as a place where a dropped audio file loads into track `t`'s sampler, and
/// outlines it while files are dragged over it.
pub fn drop_target(app: &mut SoundApp, ctx: &egui::Context, r: Rect, t: TrackId) {
    app.sample_drop_targets.push((r, t));
    let over = ctx.input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p)));
    if over {
        ctx.layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new(("sample_drop", t.0)))).rect_stroke(
            r,
            2.0,
            Stroke::new(2.0, Tokens::current().accent),
            StrokeKind::Inside,
        );
    }
}

/// An audio file dropped on a sampler target whose bytes the shell has to read itself (in the
/// browser, files arrive by name and are read asynchronously). Hand the bytes to
/// [`SoundApp::load_sample_bytes`].
#[derive(Debug, Clone)]
pub struct SampleRead {
    pub track: TrackId,
    pub file: egui::DroppedFileHandle,
}

impl SampleRead {
    /// The dropped file's name.
    pub fn name(&self) -> String {
        self.file.path().file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string()
    }
}

impl SoundApp {
    /// Takes this frame's dropped files: audio dropped on an instrument track or a Sampler window
    /// loads into that track's sampler (the sampler becomes its instrument). A file with a full
    /// path loads straight away; one without (the browser) waits in [`SoundApp::take_sample_reads`]
    /// for the shell to read. Everything else is returned for the shell to open or import.
    pub fn take_dropped_files(&mut self, ctx: &egui::Context) -> Vec<egui::DroppedFileHandle> {
        let (files, pos) = ctx.input(|i| (i.raw.dropped_files.clone(), i.pointer.latest_pos()));
        if files.is_empty() {
            return files;
        }
        let Some(track) = self.drop_track(pos) else { return files };
        let mut rest = Vec::new();
        let mut claimed = false;
        for f in files {
            if claimed || !is_audio_name(&f.path().to_string_lossy()) {
                rest.push(f);
                continue;
            }
            claimed = true;
            if f.path().is_absolute() {
                let path = f.path().to_string_lossy().into_owned();
                let r = self.run("mix.sampler_load", json!({"track": track.0, "path": path}));
                self.sample_loaded(track, &path, r);
            } else {
                self.sample_reads.push(SampleRead { track, file: f });
            }
        }
        rest
    }

    /// The track a drop at `pos` loads into. Without a position (some platforms report none
    /// during a drag), a single open Sampler window takes it.
    fn drop_track(&self, pos: Option<egui::Pos2>) -> Option<TrackId> {
        if let Some(p) = pos {
            return self.sample_drop_targets.iter().rev().find(|(r, _)| r.contains(p)).map(|(_, t)| *t);
        }
        let s = self.engine.session();
        let mut open = self.ui.plugin_windows.iter().filter(|(t, slot)| {
            *slot == soundcraft_mix::SlotKey::Instrument && s.track(*t).and_then(|x| x.instrument.as_ref()).is_some_and(|i| i.plugin == SAMPLER)
        });
        match (open.next(), open.next()) {
            (Some((t, _)), None) => Some(*t),
            _ => None,
        }
    }

    /// Dropped files waiting for the shell to read their bytes.
    pub fn take_sample_reads(&mut self) -> Vec<SampleRead> {
        std::mem::take(&mut self.sample_reads)
    }

    /// Loads a dropped file's bytes (or reports why they could not be read) into its track's
    /// sampler, as one undo step.
    pub fn load_sample_bytes(&mut self, read: &SampleRead, bytes: Result<Vec<u8>, String>) {
        let name = read.name();
        let r = bytes.and_then(|b| soundcraft_engine::io::load_sampler_bytes(&mut self.engine, read.track, &name, &b).map_err(|e| e.to_string()));
        self.sample_loaded(read.track, &name, r);
    }

    fn sample_loaded(&mut self, track: TrackId, what: &str, r: Result<serde_json::Value, String>) {
        match r {
            Ok(v) => {
                if !self.ui.plugin_windows.contains(&(track, soundcraft_mix::SlotKey::Instrument)) {
                    self.ui.plugin_windows.push((track, soundcraft_mix::SlotKey::Instrument));
                }
                let name = v.get("name").and_then(|n| n.as_str()).unwrap_or(what);
                self.ui.status = format!("{name} loaded into the sampler");
            }
            Err(e) => self.ui.status = format!("{what}: {e}"),
        }
    }
}

/// Width the Sampler's window opens at: Lift's editor.
pub const EDITOR_WIDTH: f32 = crate::lift::W;

/// Lift - Sampler's editor (see [`crate::lift`]).
pub fn editor(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    crate::lift::editor(app, ui, tid, ins);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn notes_show_by_name() {
        let info = soundcraft_dsp::plugin_info(SAMPLER).unwrap();
        let root = info.param("root_note").unwrap();
        assert_eq!(format_param(SAMPLER, root, 60.0), format!("{} (60)", soundcraft_midi::note_name(60)));
        assert_eq!(format_param(SAMPLER, info.param("vel_high").unwrap(), 126.6), "127");
        assert_eq!(format_param(SAMPLER, info.param("attack").unwrap(), 1.0), "1.0 ms");
        assert!(whole_number(SAMPLER, "key_low") && !whole_number(SAMPLER, "attack") && !whole_number("subtractive_synth", "root_note"));
    }

    #[test]
    fn audio_names() {
        for n in ["a.wav", "B.FLAC", "x.mp3", "y.ogg", "z.aiff", "/tmp/k.Wav"] {
            assert!(is_audio_name(n), "{n}");
        }
        for n in ["song.scraft", "beat.mid", "noext", "", "pic.png"] {
            assert!(!is_audio_name(n), "{n}");
        }
    }
}

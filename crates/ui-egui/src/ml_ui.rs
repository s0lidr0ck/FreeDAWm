//! Local ML audio in the app (desktop only): the small dialog per job (Separate Stems, Make
//! Karaoke, Split Drums, Clean Vocal, Convert to MIDI, Detect Chords, Transcribe Lyrics), the progress window with Cancel while the job runs on a
//! worker thread, and Setup › ML Models (installed models, licences, the model folder, GPU/CPU).
//!
//! The source is rendered on the UI thread when the job starts (`ml::prepare`), inference runs on a
//! worker (`MlJob::run`), and the results are applied on the UI thread as one undo step
//! (`ml::apply`), so the app stays live and the session is only touched by the UI thread.

use crate::SoundApp;
use crate::theme::{Tokens, bold};
use serde_json::{Value, json};
use soundcraft_engine::ml::{self, Cancel, Ctx, Device, Isolate, MidiModel, MlError, ModelStatus, ProgressCell, Quality, SepModel};
use std::sync::mpsc::{Receiver, TryRecvError};

/// The job a dialog sets up.
#[derive(Debug, Clone, PartialEq)]
pub struct JobDialog {
    pub command: &'static str,
    /// What it runs on: `{"track": id}` or `{"clip": id}`.
    pub target: Value,
    pub target_name: String,
    pub keep_backing: bool,
    pub from_mix: bool,
    pub mute_original: bool,
    pub denoise: bool,
    pub dereverb: bool,
    pub amount_db: f32,
    /// Clean Vocal: Vocal (singing) or Speech.
    pub clean_mode: soundcraft_engine::ml::CleanMode,
    /// Clean Vocal (Vocal mode): the clip's starting Amount, % cleaned.
    pub clean_mix: f32,
    /// Separate Stems: the model (HTDemucs or BS-RoFormer SW).
    pub sep_model: SepModel,
    /// Separate Stems: which of the model's stems (`sep_model.stems()` order) to keep.
    pub stems: [bool; 6],
    /// Separate Stems, Make Karaoke, Split Drums (from a mix): the HTDemucs step's quality.
    pub quality: Quality,
    /// The source's length, seconds (for the time estimate).
    pub seconds: f64,
    /// Convert to MIDI: the model and its options.
    pub midi: MidiDialog,
    /// Detect Chords' options (`ml_text_ui`).
    pub chords: crate::ml_text_ui::ChordDialog,
    /// Transcribe Lyrics' options (`ml_text_ui`).
    pub lyrics: crate::ml_text_ui::LyricsDialog,
}

/// Convert to MIDI's settings in the dialog.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MidiDialog {
    pub model: MidiModel,
    /// Onset threshold 0..1 (the model's default when it opens or the model changes).
    pub onset: f32,
    /// Shortest note, ms.
    pub min_ms: f32,
    pub pitch_bends: bool,
    pub pedal: bool,
    /// Drums: one threshold per class (kick, snare, toms, hi-hat, cymbals).
    pub drum_thresholds: [f32; 5],
    /// Separate this stem out first and transcribe it.
    pub isolate: Isolate,
}

impl MidiDialog {
    pub fn new(model: MidiModel) -> Self {
        Self::for_source(model, None)
    }

    /// The defaults for `model` on a source named `track` (the isolation follows the name).
    pub fn for_source(model: MidiModel, track: Option<&str>) -> Self {
        MidiDialog {
            model,
            onset: model.default_onset(),
            min_ms: model.default_min_note_ms(),
            pitch_bends: false,
            pedal: true,
            drum_thresholds: ml::adtof::scaled_thresholds(model.default_onset()),
            isolate: Isolate::default_for(model, track),
        }
    }
}

/// How long the source of `target` is, in seconds.
fn source_seconds(app: &SoundApp, target: &Value) -> f64 {
    let s = app.engine.session();
    let frames = if let Some(c) = target.get("clip").and_then(Value::as_u64) {
        s.find_clip(soundcraft_model::ClipId(c)).map_or(0, |(_, c)| c.length)
    } else if let Some(t) = target.get("track").and_then(Value::as_u64).and_then(|t| s.track(soundcraft_model::TrackId(t))) {
        let a = t.clips().iter().map(|c| c.start).min().unwrap_or(0);
        let b = t.clips().iter().map(|c| c.end()).max().unwrap_or(0);
        (b - a).max(0)
    } else {
        0
    };
    frames as f64 / s.sample_rate.as_f64()
}

/// The karaoke step's speed, × real time (export lane): DirectML about 1.3 (1.2 s per 8 s chunk at
/// 4 overlaps), the 16-core CPU about 0.13.
const KARAOKE_GPU_SPEED: f64 = 1.3;
const KARAOKE_CPU_SPEED: f64 = 0.13;

/// Whether the karaoke model will run on the GPU with `device` (its card may rule DirectML out).
fn karaoke_on_gpu(models: &[ml::ModelEntry], device: Device) -> bool {
    device != Device::Cpu
        && models.iter().any(|m| {
            m.model.id == "karaoke_melroformer"
                && matches!(&m.status, ModelStatus::Installed(c) if c.extra.get("dml_supported").and_then(Value::as_bool).unwrap_or(true))
        })
}

/// How a licence status reads: dimmed when free to use, the warning colour when the author states
/// no licence or forbids commercial use (personal use only).
fn licence_color(status: &str, t: &Tokens) -> egui::Color32 {
    if status.contains("personal use") { t.rec } else { t.text_dim }
}

/// "Licence: Model — status; …" for the models a job runs.
fn licence_line(models: &[ml::ModelEntry], ids: &[&str]) -> String {
    let parts: Vec<String> = ids
        .iter()
        .map(|id| match models.iter().find(|m| m.model.id == *id) {
            Some(m) => format!("{} — {}", m.model.name, m.licence()),
            None => format!("{id} — {}", ml::known_licence(id)),
        })
        .collect();
    format!("Licence: {}", parts.join("; "))
}

/// The model list, read from disk once (when a dialog or the Models window opens, or on Refresh),
/// not every frame.
fn models(app: &mut SoundApp) -> Vec<ml::ModelEntry> {
    app.ml.models.get_or_insert_with(|| ml::list_models(&ml::models_dir())).clone()
}

/// Opens the model folder in the file manager (made first, so there is something to open).
fn open_folder(dir: &std::path::Path) -> Result<(), String> {
    std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
    let program = if cfg!(windows) {
        "explorer"
    } else if cfg!(target_os = "macos") {
        "open"
    } else {
        "xdg-open"
    };
    std::process::Command::new(program).arg(dir).spawn().map(|_| ()).map_err(|e| format!("{program}: {e}"))
}

/// Separate Stems' choices, in the dialog's order.
const SEP_MODELS: [(SepModel, &str); 3] =
    [(SepModel::Best, "Best of each (6 stems)"), (SepModel::Sw6, "BS-RoFormer SW (6 stems)"), (SepModel::Demucs, "HTDemucs (4 stems)")];

/// The Model choice Separate Stems opens with: the best the installed models allow
/// ([`SepModel::best_installed`]).
pub fn default_sep_model(installed: impl Fn(&str) -> bool) -> SepModel {
    SepModel::best_installed(installed)
}

/// A job running on its worker thread.
pub struct Running {
    pub title: String,
    job: ml::MlJob,
    cancel: Cancel,
    progress: ProgressCell,
    rx: Receiver<Result<ml::MlResult, MlError>>,
    started: std::time::Instant,
}

/// The ML windows' state.
#[derive(Default)]
pub struct MlUi {
    pub dialog: Option<JobDialog>,
    pub running: Option<Running>,
    /// Convert to MIDI's Analyse on its worker.
    pub analysing: Option<crate::ml_audition::Analysing>,
    /// Convert to MIDI's analysed source, decoded with the dialog's settings.
    pub audition: Option<crate::ml_audition::Audition>,
    pub show_models: bool,
    /// The model list as last read (refreshed when the window opens or on Refresh).
    models: Option<Vec<ml::ModelEntry>>,
}

/// The UI-layer commands: menu items that open a job's dialog, and the Models window.
pub const MENU_ITEMS: [(&str, &str, &str); 8] = [
    ("ui.ml.separate", "Separate Stems…", "ml.separate"),
    ("ui.ml.karaoke", "Make Karaoke…", "ml.karaoke"),
    ("ui.ml.drums", "Split Drums…", "ml.drums"),
    ("ui.ml.clean", "Clean Vocal…", "ml.clean"),
    ("ui.ml.midi", "Convert to MIDI…", "ml.midi"),
    ("ui.ml.chords", "Detect Chords…", "ml.chords"),
    ("ui.ml.lyrics", "Transcribe Lyrics…", "ml.lyrics"),
    ("window.ml_models", "ML Models…", ""),
];

fn title_of(command: &str) -> &'static str {
    match command {
        "ml.separate" => "Separate Stems",
        "ml.karaoke" => "Make Karaoke",
        "ml.drums" => "Split Drums",
        "ml.midi" => "Convert to MIDI",
        "ml.chords" => "Detect Chords",
        "ml.lyrics" => "Transcribe Lyrics",
        _ => "Clean Vocal",
    }
}

/// The selected clip (first), else the first selected track, as a job target.
fn selection_target(app: &SoundApp) -> Option<(Value, String)> {
    let s = app.engine.session();
    if let Some(c) = s.edit.selected_clips.first().and_then(|c| s.find_clip(*c)).map(|(_, c)| c) {
        return Some((json!({"clip": c.id.0}), c.name.clone()));
    }
    let t = app.engine.target_tracks().into_iter().next().and_then(|t| s.track(t))?;
    Some((json!({"track": t.id.0}), t.name.clone()))
}

/// Opens `command`'s dialog for `target` (`{"track": id}` / `{"clip": id}`), or for the selection.
pub fn open(app: &mut SoundApp, command: &str, target: Option<(Value, String)>) -> Result<Value, String> {
    if app.ml.running.is_some() {
        return Err("an ML job is already running".into());
    }
    let command = match command {
        "ml.separate" | "ui.ml.separate" => "ml.separate",
        "ml.karaoke" | "ui.ml.karaoke" => "ml.karaoke",
        "ml.drums" | "ui.ml.drums" => "ml.drums",
        "ml.clean" | "ui.ml.clean" => "ml.clean",
        "ml.midi" | "ui.ml.midi" => "ml.midi",
        "ml.chords" | "ui.ml.chords" => "ml.chords",
        "ml.lyrics" | "ui.ml.lyrics" => "ml.lyrics",
        other => return Err(format!("unknown ML job `{other}`")),
    };
    let Some((target, target_name)) = target.or_else(|| selection_target(app)) else {
        return Err("select an audio track or clip first".into());
    };
    let seconds = source_seconds(app, &target);
    let full_mix = crate::ml_text_ui::full_mix(app.engine.session(), &target);
    let grid = crate::ml_text_ui::default_grid(app.engine.session());
    let midi = MidiDialog::for_source(MidiModel::General, Some(&target_name));
    app.ml.models = None;
    // A Convert to MIDI audition belongs to the dialog this one replaces.
    crate::ml_audition::close(app);
    app.ml.dialog = Some(JobDialog {
        command,
        target,
        target_name,
        keep_backing: false,
        from_mix: false,
        mute_original: command == "ml.karaoke",
        denoise: true,
        dereverb: true,
        amount_db: ml::DEFAULT_AMOUNT_DB,
        clean_mode: soundcraft_engine::ml::CleanMode::Vocal,
        clean_mix: 100.0,
        sep_model: {
            let models = models(app);
            default_sep_model(|id| models.iter().any(|m| m.model.id == id && matches!(m.status, ModelStatus::Installed(_))))
        },
        stems: [true; 6],
        quality: Quality::High,
        seconds,
        midi,
        chords: crate::ml_text_ui::ChordDialog::new(full_mix, grid),
        lyrics: crate::ml_text_ui::LyricsDialog::default(),
    });
    Ok(json!({"dialog": command}))
}

/// The parameters the dialog runs its command with.
pub fn params(d: &JobDialog, device: Device) -> Value {
    let mut p = d.target.clone();
    if let Some(o) = p.as_object_mut() {
        o.insert("device".into(), json!(device.id()));
        match d.command {
            "ml.separate" => {
                let keep: Vec<&str> = d.sep_model.stems().iter().zip(d.stems).filter(|(_, on)| *on).map(|(s, _)| *s).collect();
                o.insert("model".into(), json!(d.sep_model.id()));
                o.insert("stems".into(), json!(keep));
                o.insert("mute_original".into(), json!(d.mute_original));
                o.insert("quality".into(), json!(d.quality.id()));
            }
            "ml.karaoke" => {
                o.insert("keep_backing".into(), json!(d.keep_backing));
                o.insert("mute_original".into(), json!(d.mute_original));
                o.insert("quality".into(), json!(d.quality.id()));
            }
            "ml.drums" => {
                o.insert("from_mix".into(), json!(d.from_mix));
                o.insert("mute_original".into(), json!(d.mute_original));
                o.insert("quality".into(), json!(d.quality.id()));
            }
            "ml.midi" => {
                let m = &d.midi;
                o.insert("model".into(), json!(m.model.id()));
                o.insert("onset_threshold".into(), json!(m.onset));
                o.insert("min_note_ms".into(), json!(m.min_ms));
                o.insert("pitch_bends".into(), json!(m.pitch_bends));
                o.insert("pedal".into(), json!(m.pedal));
                if m.model == MidiModel::Drums {
                    o.insert("drum_thresholds".into(), json!(m.drum_thresholds));
                }
                o.insert("isolate".into(), json!(m.isolate.id()));
                o.insert("quality".into(), json!(d.quality.id()));
            }
            "ml.chords" => crate::ml_text_ui::chord_params(o, &d.chords),
            "ml.lyrics" => crate::ml_text_ui::lyrics_params(o, &d.lyrics),
            _ => {
                o.insert("mode".into(), json!(d.clean_mode.id()));
                o.insert("mix".into(), json!(d.clean_mix));
                if d.clean_mode == soundcraft_engine::ml::CleanMode::Speech {
                    o.insert("dereverb".into(), json!(d.dereverb));
                    o.insert("amount_db".into(), json!(d.amount_db));
                }
            }
        }
    }
    p
}

/// The device the person chose in Setup › ML Models.
pub fn device(app: &SoundApp) -> Device {
    Device::parse(&app.ui.ml_device).unwrap_or_default()
}

/// Starts `command` with `p`: renders the source now, runs the models on a worker thread.
pub fn start(app: &mut SoundApp, command: &str, p: &Value) -> Result<(), String> {
    if app.ml.running.is_some() {
        return Err("an ML job is already running".into());
    }
    // Third-party plugins keep state the session can't see: store it before rendering a track.
    app.capture_plugin_states();
    let mut worker = ml::prepare(&app.engine, command, p).map_err(|e| e.to_string())?;
    // The UI keeps the targets only; the worker owns (and frees) the source audio.
    let job = worker.without_audio();
    let cancel = Cancel::new();
    let progress = ProgressCell::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = Ctx::with_cell(cancel.clone(), progress.clone());
    std::thread::Builder::new()
        .name("ml-job".into())
        .spawn(move || {
            let _ = tx.send(worker.run(&ctx));
        })
        .map_err(|e| format!("could not start the ML job: {e}"))?;
    app.ml.running = Some(Running { title: job.title(), job, cancel, progress, rx, started: std::time::Instant::now() });
    Ok(())
}

/// Picks up a finished job and applies it (one undo step). Called every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &app.ml.running else { return };
    let got = match run.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        Err(TryRecvError::Disconnected) => Err(MlError::Runtime("the ML job stopped unexpectedly".into())),
    };
    let Some(run) = app.ml.running.take() else { return };
    let secs = run.started.elapsed().as_secs_f64();
    // Cancel wins even when the worker finished just before it saw the flag: nothing is applied.
    let got = if run.cancel.is_cancelled() { Err(MlError::Cancelled) } else { got };
    app.ui.status = match got {
        Ok(result) => match ml::apply(&mut app.engine, &run.job, result) {
            Ok(v) if crate::ml_text_ui::summary(&run.title, &v, secs).is_some() => {
                crate::ml_text_ui::summary(&run.title, &v, secs).unwrap_or_default()
            }
            Ok(v) => {
                let made = v.get("made").and_then(Value::as_array).map_or(0, Vec::len);
                let notes: Vec<&str> =
                    v.get("notes").and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).collect()).unwrap_or_default();
                format!("{}: {made} result{} in {secs:.1} s ({})", run.title, if made == 1 { "" } else { "s" }, notes.join("; "))
            }
            Err(e) => format!("{}: {e}", run.title),
        },
        Err(MlError::Cancelled) => format!("{} cancelled", run.title),
        Err(e) => format!("{}: {e}", run.title),
    };
}

/// Refuses to replace the session while a job holds its source: its results would land in the
/// wrong session. True when refused.
pub fn refuses_session_switch(app: &mut SoundApp) -> bool {
    if app.ml.running.is_some() {
        app.ui.status = "Finish or cancel the ML job first".into();
    }
    app.ml.running.is_some()
}

/// The dialog, the progress window and the Models window.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    progress_window(app, ctx);
    dialog(app, ctx);
    models_window(app, ctx);
}

fn progress_window(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(run) = &app.ml.running else { return };
    let mut cancel = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let (f, what) = (run.progress.get(), run.progress.what());
    let elapsed = run.started.elapsed().as_secs_f64();
    let title = run.title.clone();
    egui::Modal::new(egui::Id::new("ml_progress")).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 8.0);
        ui.set_width(360.0);
        ui.label(egui::RichText::new(&title).font(bold(13.0)));
        ui.add(egui::ProgressBar::new(f));
        if !what.is_empty() {
            ui.label(egui::RichText::new(&what).color(Tokens::current().text_dim));
        }
        ui.horizontal(|ui| {
            // Cancel is seen at the job's next check (a chunk, or whisper.cpp's next step).
            if run.cancel.is_cancelled() {
                ui.add_enabled(false, egui::Button::new("Cancelling…"));
            } else {
                cancel |= ui.button("Cancel").clicked();
            }
            ui.label(egui::RichText::new(format!("{:.0}%  ·  {elapsed:.0} s", f * 100.0)).color(Tokens::current().text_dim));
        });
    });
    if cancel {
        // The worker stops at its next chunk; `step` reports it.
        run.cancel.cancel();
    }
}

/// What the Convert to MIDI dialog's audition buttons asked for.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Audit {
    Analyse,
    Play,
    Stop,
    Mix(ml::AuditionMix),
    Convert,
    CancelAnalysis,
}

fn dialog(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(mut d) = app.ml.dialog.take() else { return };
    let mut keep = true;
    let mut run = false;
    let mut audit: Option<Audit> = None;
    let mut device = device(app);
    let title = title_of(d.command);
    let models = models(app);
    let installed = |id: &str| models.iter().any(|m| m.model.id == id && matches!(m.status, ModelStatus::Installed(_)));
    let melband = installed("vocals_melroformer");
    let needs: Vec<&str> = match d.command {
        "ml.separate" => d.sep_model.model_ids(),
        // The vocals come from Mel-Band RoFormer vocals when installed, else HTDemucs.
        "ml.karaoke" if melband => Vec::new(),
        "ml.karaoke" => vec!["htdemucs"],
        "ml.drums" if d.from_mix => vec!["htdemucs", "drumsep_mdx23c"],
        "ml.drums" => vec!["drumsep_mdx23c"],
        // Isolation takes the first installed of its sources (HTDemucs is the fallback).
        "ml.midi" if !ml::isolation_sources(d.midi.isolate).iter().any(|(s, _)| installed(s.model_id())) => {
            let mut v = vec![d.midi.model.model_id()];
            if d.midi.isolate != Isolate::None {
                v.insert(0, "htdemucs");
            }
            v
        }
        "ml.midi" => vec![d.midi.model.model_id()],
        "ml.chords" => vec!["btc_chords"],
        "ml.lyrics" => vec!["whisper_lyrics"],
        _ if d.clean_mode == soundcraft_engine::ml::CleanMode::Vocal || d.dereverb => vec!["srs_small"],
        _ => Vec::new(),
    };
    let band_key = {
        let b = &app.engine.session().band;
        const K: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
        format!("{} {}", K.get(usize::from(b.key)).copied().unwrap_or("C"), if b.scale == 1 { "minor" } else { "major" })
    };
    // Every model the job runs (for the licence line), not only the missing ones.
    let uses: Vec<&str> = match d.command {
        "ml.karaoke" => vec![if melband { "vocals_melroformer" } else { "htdemucs" }, "karaoke_melroformer"],
        "ml.midi" => vec![d.midi.model.model_id()],
        "ml.clean" if d.clean_mode == soundcraft_engine::ml::CleanMode::Speech => {
            let mut v = vec!["deepfilternet3"];
            v.extend(needs.iter().copied());
            v
        }
        _ => needs.clone(),
    };
    let missing: Vec<&str> =
        needs.into_iter().filter(|id| models.iter().any(|m| m.model.id == *id && !matches!(m.status, ModelStatus::Installed(_)))).collect();
    egui::Window::new(egui::RichText::new(title).font(bold(13.0))).collapsible(false).resizable(false).default_width(340.0).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        ui.label(format!("Source: {}", d.target_name));
        match d.command {
            "ml.separate" => {
                ui.horizontal(|ui| {
                    ui.label("Model:");
                    for (m, label) in SEP_MODELS {
                        ui.radio_value(&mut d.sep_model, m, label);
                    }
                });
                ui.label("Stems, as new tracks under the source:");
                ui.horizontal_wrapped(|ui| {
                    for (i, s) in d.sep_model.stems().iter().enumerate() {
                        let mut on = d.stems[i];
                        if ui.checkbox(&mut on, crate::ml_ui::cap(s)).changed() {
                            d.stems[i] = on;
                        }
                    }
                });
                if d.sep_model == SepModel::Best {
                    ui.label("Vocals from Mel-Band RoFormer; drums, bass, guitar, piano and other from BS-RoFormer SW.");
                }
                if d.sep_model.uses_quality() {
                    quality_row(ui, &mut d.quality);
                }
                ui.checkbox(&mut d.mute_original, "Mute original");
            }
            "ml.karaoke" => {
                ui.label(if melband {
                    "Lead vocal and an instrumental (Mel-Band RoFormer vocals, then Mel-Band RoFormer Karaoke)."
                } else {
                    "Lead vocal and an instrumental (HTDemucs, then Mel-Band RoFormer Karaoke)."
                });
                let gpu = karaoke_on_gpu(&models, device);
                let est = d.seconds / if gpu { KARAOKE_GPU_SPEED } else { KARAOKE_CPU_SPEED } / 60.0;
                ui.label(
                    egui::RichText::new(format!(
                        "The karaoke step takes about {} for this {:.0} s source on the {}.",
                        if est < 1.0 { "a minute".to_string() } else { format!("{est:.0} min") },
                        d.seconds,
                        if gpu { "GPU" } else { "CPU (much slower than the GPU)" }
                    ))
                    .color(Tokens::current().text_dim),
                );
                ui.checkbox(&mut d.keep_backing, "Keep backing vocals in the instrumental")
                    .on_hover_text("Off: the backing vocals get their own track");
                quality_row(ui, &mut d.quality);
                ui.checkbox(&mut d.mute_original, "Mute original");
            }
            "ml.drums" => {
                ui.label("Kick, snare, toms, hi-hat, ride, crash (MDX23C DrumSep).");
                ui.checkbox(&mut d.from_mix, "The source is a full mix: separate the drums first");
                if d.from_mix {
                    quality_row(ui, &mut d.quality);
                }
                ui.checkbox(&mut d.mute_original, "Mute original");
            }
            "ml.midi" => {
                let full_mix = crate::ml_text_ui::full_mix(app.engine.session(), &d.target);
                midi_options(ui, &mut d.midi, &models, &d.target_name, full_mix);
                if d.midi.isolate != Isolate::None {
                    quality_row(ui, &mut d.quality);
                }
                audit = audition_panel(app, ui, &d, device);
            }
            "ml.chords" => crate::ml_text_ui::chord_options(ui, &mut d.chords, &band_key),
            "ml.lyrics" => crate::ml_text_ui::lyrics_options(ui, &mut d.lyrics, d.seconds),
            _ => {
                use soundcraft_engine::ml::CleanMode;
                ui.label("Keeps the original: the clip plays a mix of it and the cleaned audio (Amount, Original/Cleaned on the clip).");
                ui.horizontal(|ui| {
                    ui.label("Mode");
                    for m in [CleanMode::Vocal, CleanMode::Speech] {
                        ui.selectable_value(&mut d.clean_mode, m, m.label());
                    }
                });
                if d.clean_mode == CleanMode::Vocal {
                    ui.label(
                        egui::RichText::new("Smule Renaissance (trained on singing): noise and reverb off together, level matched.")
                            .color(Tokens::current().text_dim),
                    );
                } else {
                    ui.horizontal(|ui| {
                        ui.label("De-noise");
                        ui.add(egui::Slider::new(&mut d.amount_db, 3.0..=100.0).suffix(" dB").logarithmic(true)).on_hover_text(
                            "DeepFilterNet 3: the most the noise is reduced (100 dB is unlimited). A speech model: it can pump sustained singing.",
                        );
                    });
                    ui.checkbox(&mut d.dereverb, "Then de-reverb (Smule Renaissance)");
                }
                ui.horizontal(|ui| {
                    ui.label("Amount");
                    ui.add(egui::Slider::new(&mut d.clean_mix, 0.0..=100.0).suffix(" %"))
                        .on_hover_text("How much of the cleaned audio the clip plays (the rest is the original). Change it later on the clip.");
                });
            }
        }
        // whisper.cpp runs on the CPU here (no Vulkan SDK or CUDA toolkit to build its GPU backends).
        if d.command != "ml.lyrics" {
            ui.horizontal(|ui| {
                ui.label("Run on");
                egui::ComboBox::from_id_salt("ml_device").selected_text(device.label()).show_ui(ui, |ui| {
                    for dv in [Device::Auto, Device::Gpu, Device::Cpu] {
                        ui.selectable_value(&mut device, dv, dv.label());
                    }
                });
            });
        }
        if !uses.is_empty() {
            let line = licence_line(&models, &uses);
            let c = licence_color(&line, &Tokens::current());
            ui.label(egui::RichText::new(line).small().color(c));
        }
        if !missing.is_empty() {
            ui.colored_label(Tokens::current().rec, format!("Not installed: {}. See Setup › ML Models.", missing.join(", ")));
        }
        ui.separator();
        ui.horizontal(|ui| {
            let can = missing.is_empty() && (d.command != "ml.separate" || d.stems.iter().take(d.sep_model.stems().len()).any(|s| *s));
            if d.command == "ml.midi" {
                let analysing = app.ml.analysing.is_some();
                let analysed = app.ml.audition.is_some();
                if ui
                    .add_enabled(can && !analysing, egui::Button::new(if analysed { "Analyse again" } else { "Analyse" }))
                    .on_hover_text("Runs the model once; then every setting re-decodes at once, and Play auditions the notes")
                    .clicked()
                {
                    audit = Some(Audit::Analyse);
                }
                if analysed {
                    if ui.add_enabled(!analysing, egui::Button::new("Convert")).on_hover_text("Commits the notes on screen").clicked() {
                        audit = Some(Audit::Convert);
                    }
                } else if ui.add_enabled(can && !analysing, egui::Button::new("Run")).on_hover_text("Analyse and convert in one go").clicked() {
                    run = true;
                }
            } else if ui.add_enabled(can, egui::Button::new("Run")).clicked() {
                run = true;
            }
            if ui.button("Cancel").clicked() || ui.input(|i| i.key_pressed(egui::Key::Escape)) {
                keep = false;
            }
        });
    });
    if let Some(a) = audit {
        let r = match a {
            Audit::Analyse => crate::ml_audition::start(app, &params(&d, device)),
            Audit::Play => crate::ml_audition::play(app),
            Audit::Stop => {
                crate::ml_audition::stop(app);
                Ok(())
            }
            Audit::Mix(m) => {
                if let Some(x) = app.ml.audition.as_mut() {
                    x.mix = m;
                }
                crate::ml_audition::refresh(app);
                Ok(())
            }
            Audit::CancelAnalysis => {
                if let Some(x) = &app.ml.analysing {
                    x.cancel();
                }
                Ok(())
            }
            Audit::Convert => match crate::ml_audition::commit(app) {
                Ok(v) => {
                    let made = v.get("made").and_then(Value::as_array).map_or(0, Vec::len);
                    let notes: usize =
                        v.get("made").and_then(Value::as_array).map_or(0, |a| a.iter().filter_map(|m| m["notes"].as_u64()).sum::<u64>() as usize);
                    app.ui.status = format!("Convert to MIDI: {made} clip{} with {notes} notes", if made == 1 { "" } else { "s" });
                    keep = false;
                    Ok(())
                }
                Err(e) => Err(e),
            },
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
    if d.command == "ml.midi" && (!keep || run) {
        crate::ml_audition::close(app);
    }
    if device.id() != app.ui.ml_device {
        app.ui.ml_device = device.id().to_string();
    }
    if run {
        let p = params(&d, device);
        if let Err(e) = start(app, d.command, &p) {
            app.ui.status = e;
        }
        return;
    }
    if keep {
        app.ml.dialog = Some(d);
    }
}

/// Convert to MIDI's audition inside the dialog: the analysis' progress, or the mini piano roll
/// with Play / Stop and the Source / Notes / Both toggle. Returns what a button asked for.
fn audition_panel(app: &mut SoundApp, ui: &mut egui::Ui, d: &JobDialog, device: Device) -> Option<Audit> {
    let t = Tokens::current();
    let mut out = None;
    if let Some(run) = &app.ml.analysing {
        let (f, what) = run.progress();
        ui.add(egui::ProgressBar::new(f).text(if what.is_empty() { "Analysing…".to_string() } else { what }));
        if run.cancelled() {
            ui.add_enabled(false, egui::Button::new("Cancelling…"));
        } else if ui.small_button("Cancel analysis").clicked() {
            out = Some(Audit::CancelAnalysis);
        }
        return out;
    }
    let key = crate::ml_audition::key_of(&ml::midi_options("ml.midi", &params(d, device)).ok()?, d.quality);
    let stale = app.ml.audition.as_ref().is_some_and(|a| a.key != key);
    if stale {
        // The model, isolation or quality changed: this analysis is for other settings.
        crate::ml_audition::close(app);
        ui.label(egui::RichText::new("Settings changed: Analyse again to audition.").color(t.text_dim));
        return None;
    }
    let opts = ml::midi_options("ml.midi", &params(d, device)).ok()?;
    let changed = app.ml.audition.as_mut().is_some_and(|a| a.redecode(&opts));
    if changed {
        crate::ml_audition::refresh(app);
    }
    let playing = app.audition_session.is_some() && app.is_playing();
    let playhead = app.ml.audition.as_ref().and_then(|a| crate::ml_audition::playhead_secs(app, a));
    let a = app.ml.audition.as_ref()?;
    ui.separator();
    ui.label(
        egui::RichText::new(format!(
            "{} note{}{}{}",
            a.note_count(),
            if a.note_count() == 1 { "" } else { "s" },
            if a.job.parts.len() > 1 { format!(" in {} clips (the first shown)", a.job.parts.len()) } else { String::new() },
            if a.notes.is_empty() { String::new() } else { format!(" · {}", a.notes.join("; ")) }
        ))
        .small()
        .color(t.text_dim),
    );
    crate::ml_audition::roll(ui, a, playhead);
    if playing {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    let mut mix = a.mix;
    ui.horizontal(|ui| {
        if playing {
            if ui.button("Stop").clicked() {
                out = Some(Audit::Stop);
            }
        } else if ui
            .button("Play")
            .on_hover_text("Loops the selection (or the clip): the notes through the target instrument, the source at −12 dB")
            .clicked()
        {
            out = Some(Audit::Play);
        }
        for m in ml::AuditionMix::ALL {
            if ui.selectable_value(&mut mix, m, m.label()).clicked() {
                out = Some(Audit::Mix(m));
            }
        }
    });
    out
}

/// The HTDemucs step's Quality choice (Fast: one pass; High: shifted passes averaged).
fn quality_row(ui: &mut egui::Ui, q: &mut Quality) {
    ui.horizontal(|ui| {
        ui.label("Quality");
        egui::ComboBox::from_id_salt("ml_quality").selected_text(q.label()).show_ui(ui, |ui| {
            for v in Quality::ALL {
                ui.selectable_value(q, v, v.label());
            }
        });
    })
    .response
    .on_hover_text("High runs the separation twice on inputs shifted by up to half a second and averages the passes (Demucs' shift trick): fewer watery transients and spectral holes, about twice the time");
}

/// Convert to MIDI's part of the dialog: the model and its options. `source` is the source's
/// name (the isolation default follows it); `full_mix` says the source looks like a whole mix.
fn midi_options(ui: &mut egui::Ui, m: &mut MidiDialog, models: &[ml::ModelEntry], source: &str, full_mix: bool) {
    let t = Tokens::current();
    ui.label("A new instrument track under the source, one MIDI clip per audio clip.");
    let installed = |id: &str| models.iter().any(|e| e.model.id == id && matches!(e.status, ModelStatus::Installed(_)));
    ui.horizontal(|ui| {
        ui.label("Model");
        let mut pick = m.model;
        egui::ComboBox::from_id_salt("ml_midi_model").selected_text(m.model.label()).show_ui(ui, |ui| {
            for mm in MidiModel::ALL {
                let label = if installed(mm.model_id()) { mm.label().to_string() } else { format!("{} (not installed)", mm.label()) };
                ui.selectable_value(&mut pick, mm, label);
            }
        });
        if pick != m.model {
            *m = MidiDialog { pitch_bends: m.pitch_bends, pedal: m.pedal, ..MidiDialog::for_source(pick, Some(source)) };
        }
    });
    ui.horizontal(|ui| {
        ui.label("Isolate first");
        egui::ComboBox::from_id_salt("ml_midi_isolate").selected_text(m.isolate.label()).show_ui(ui, |ui| {
            for i in Isolate::ALL {
                ui.selectable_value(&mut m.isolate, i, i.label());
            }
        });
    })
    .response
    .on_hover_text(
        "Separates that stem out of the source first and transcribes the stem: a piano model fed a band mix hears bass, guitar and voice as keys.          Vocals come from Mel-Band RoFormer, piano, drums and bass from BS-RoFormer SW when installed, else from HTDemucs",
    );
    if full_mix && m.isolate == Isolate::None {
        ui.colored_label(t.meter_yellow, "This looks like a full mix: isolate the instrument first for a cleaner transcription.");
    }
    let blurb = match m.model {
        MidiModel::General => "Any pitched instrument, several notes at once (Spotify Basic Pitch).",
        MidiModel::Piano => "Solo piano: notes, velocities and the sustain pedal (Kong et al.).",
        MidiModel::Drums => "A drum kit: kick, snare, toms, hi-hat, cymbals as GM drum notes (ADTOF; CC BY-NC-SA, private use only).",
        MidiModel::Vocal => "A singing voice, one note at a time (openvpi GAME).",
    };
    ui.label(egui::RichText::new(blurb).color(t.text_dim));
    if m.model == MidiModel::Drums {
        ui.label(egui::RichText::new("Thresholds per drum (lower hears more hits):").color(t.text_dim));
        for (i, name) in ml::adtof::CLASS_NAMES.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.add_sized([56.0, 18.0], egui::Label::new(*name));
                if let Some(v) = m.drum_thresholds.get_mut(i) {
                    ui.add(egui::Slider::new(v, 0.05..=0.95).fixed_decimals(2));
                }
            });
        }
        ui.label(egui::RichText::new("Each hit's velocity follows its level in the source (the loudest per drum is 120).").small().color(t.text_dim));
    } else if m.model != MidiModel::Vocal {
        ui.horizontal(|ui| {
            ui.label("Onset threshold");
            ui.add(egui::Slider::new(&mut m.onset, 0.05..=0.95).fixed_decimals(2))
                .on_hover_text("Lower hears more notes (and more false ones); higher keeps only clear onsets");
        });
    }
    if m.model != MidiModel::Drums {
        ui.horizontal(|ui| {
            ui.label("Shortest note");
            ui.add(egui::Slider::new(&mut m.min_ms, 0.0..=500.0).suffix(" ms").fixed_decimals(0));
        });
        if m.model == MidiModel::General && m.min_ms > 0.0 {
            // A 16th note lasts 15000 / bpm ms: shorter than the shortest note above this tempo.
            let bpm = 15_000.0 / m.min_ms;
            ui.label(
                egui::RichText::new(format!(
                    "Notes shorter than {:.0} ms are dropped: 16th notes above about {bpm:.0} bpm. Lower it for fast parts.",
                    m.min_ms
                ))
                .small()
                .color(t.text_dim),
            );
        }
    }
    match m.model {
        MidiModel::General | MidiModel::Vocal => {
            ui.checkbox(&mut m.pitch_bends, "Pitch bends");
        }
        MidiModel::Piano => {
            ui.checkbox(&mut m.pedal, "Sustain pedal (CC64)");
        }
        MidiModel::Drums => {}
    }
    if ui.small_button("Defaults").clicked() {
        *m = MidiDialog::for_source(m.model, Some(source));
    }
}

/// "vocals" → "Vocals".
pub(crate) fn cap(s: &str) -> String {
    let mut c = s.chars();
    c.next().map(|f| f.to_uppercase().chain(c).collect()).unwrap_or_default()
}

fn models_window(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ml.show_models {
        return;
    }
    let dir = ml::models_dir();
    if app.ml.models.is_none() {
        app.ml.models = Some(ml::list_models(&dir));
    }
    let mut open = true;
    let mut refresh = false;
    let mut folder_error: Option<String> = None;
    let mut device = device(app);
    let t = Tokens::current();
    let models = app.ml.models.clone().unwrap_or_default();
    egui::Window::new(egui::RichText::new("ML Models").font(bold(13.0))).open(&mut open).default_width(560.0).resizable(true).show(ctx, |ui| {
        ui.spacing_mut().item_spacing = egui::vec2(8.0, 6.0);
        ui.horizontal(|ui| {
            ui.label("Run models on");
            egui::ComboBox::from_id_salt("ml_models_device").selected_text(device.label()).show_ui(ui, |ui| {
                for dv in [Device::Auto, Device::Gpu, Device::Cpu] {
                    ui.selectable_value(&mut device, dv, dv.label());
                }
            });
        })
        .response
        .on_hover_text("Auto and GPU use DirectML (any DirectX 12 GPU, no CUDA needed) and fall back to the CPU when it can't run");
        ui.horizontal(|ui| {
            ui.label("Model folder:");
            ui.monospace(dir.display().to_string());
            if ui.small_button("Open folder").clicked() {
                folder_error = open_folder(&dir).err();
            }
            if ui.small_button("Copy").clicked() {
                ui.ctx().copy_text(dir.display().to_string());
            }
        });
        ui.label(
            egui::RichText::new(format!(
                "Install or update the models with `cargo xtask ml-models` (exports them and copies them here). Set {} to use another folder.",
                soundcraft_ml_env()
            ))
            .color(t.text_dim),
        );
        ui.separator();
        egui::Grid::new("ml_models_grid").striped(true).num_columns(5).show(ui, |ui| {
            for h in ["Model", "Job", "Status", "Licence", "Notes / source"] {
                ui.label(egui::RichText::new(h).font(bold(12.0)));
            }
            ui.end_row();
            for m in &models {
                ui.label(m.model.name);
                ui.label(m.model.job);
                let (text, color) = match &m.status {
                    ModelStatus::Installed(_) => (format!("installed, {:.0} MB", m.size as f64 / 1e6), t.text),
                    ModelStatus::Builtin => ("built in".to_string(), t.text),
                    ModelStatus::Missing => ("not installed".to_string(), t.rec),
                    ModelStatus::Broken(w) => (format!("broken: {w}"), t.rec),
                };
                ui.colored_label(color, text);
                let status = m.licence();
                ui.colored_label(licence_color(&status, &t), &status);
                let lic = match &m.status {
                    ModelStatus::Installed(c) => format!("{}\n{}", c.license, c.source),
                    ModelStatus::Builtin => "MIT OR Apache-2.0\nhttps://github.com/Rikorose/DeepFilterNet".to_string(),
                    _ => String::new(),
                };
                // One line here; the whole licence note and source on hover.
                let first: String = lic.split(['.', '\n']).next().unwrap_or("").chars().take(48).collect();
                let short = if first.is_empty() { String::new() } else { format!("{first}…") };
                let r = ui.add(egui::Label::new(egui::RichText::new(short).small().color(t.text_dim)).truncate());
                if !lic.is_empty() {
                    r.on_hover_text(lic);
                }
                ui.end_row();
            }
        });
        if ui.button("Refresh").clicked() {
            refresh = true;
        }
    });
    if device.id() != app.ui.ml_device {
        app.ui.ml_device = device.id().to_string();
    }
    if refresh || !open {
        app.ml.models = None;
    }
    if let Some(e) = folder_error {
        app.ui.status = e;
    }
    app.ml.show_models = open;
}

fn soundcraft_ml_env() -> &'static str {
    "FREEDAWM_MODELS_DIR"
}

/// UI commands: `ui.ml.*` open a job's dialog (on the selection, or `track`/`clip` in `p`);
/// `window.ml_models` shows the Models window. None when `id` is not one of them.
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    if id == "window.ml_models" {
        app.ml.show_models = p.get("value").and_then(Value::as_bool).unwrap_or(true);
        app.ml.models = None;
        return Some(Ok(json!({"value": app.ml.show_models})));
    }
    if !id.starts_with("ui.ml.") {
        return None;
    }
    let s = app.engine.session();
    let target = if let Some(c) = p.get("clip").and_then(Value::as_u64) {
        s.find_clip(soundcraft_model::ClipId(c)).map(|(_, x)| (json!({"clip": c}), x.name.clone()))
    } else if let Some(t) = p.get("track").and_then(Value::as_u64) {
        s.track(soundcraft_model::TrackId(t)).map(|x| (json!({"track": t}), x.name.clone()))
    } else {
        None
    };
    Some(open(app, id, target))
}

/// What the status line says when an edit is refused during a job.
pub const BLOCKED: &str = "An ML job is running: edits wait until it finishes or is cancelled (playback still works)";

/// Whether `id` would change the session (or start another job) while a job runs.
pub fn blocks(id: &str) -> bool {
    if matches!(id, "edit.undo" | "edit.redo" | "transport.record") || (id.starts_with("ml.") && id != "ml.models") {
        return true;
    }
    soundcraft_engine::find_command(id).is_some_and(|c| c.undoable && !c.view)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;

    #[test]
    fn separate_stems_opens_on_best_when_both_roformers_are_installed() {
        let with = |ids: &'static [&'static str]| move |id: &str| ids.contains(&id);
        assert_eq!(default_sep_model(with(&["htdemucs", "bs_roformer_sw", "vocals_melroformer"])), SepModel::Best);
        assert_eq!(default_sep_model(with(&["bs_roformer_sw", "vocals_melroformer"])), SepModel::Best);
        assert_eq!(default_sep_model(with(&["htdemucs", "bs_roformer_sw"])), SepModel::Sw6);
        assert_eq!(default_sep_model(with(&["htdemucs", "vocals_melroformer"])), SepModel::Demucs);
        assert_eq!(default_sep_model(with(&[])), SepModel::Demucs);
        // The dialog offers all three; Best's stems are six, vocals first.
        assert!(SEP_MODELS.iter().any(|(m, _)| *m == SepModel::Best));
        assert_eq!(SepModel::Best.stems(), ["vocals", "drums", "bass", "guitar", "piano", "other"]);
    }

    fn app() -> (SoundApp, u64) {
        let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let s = app.engine.session();
        let clip = s.tracks.iter().find(|t| t.name == "Bass").and_then(|t| t.clips().first()).map(|c| c.id.0).unwrap();
        (app, clip)
    }

    /// Lets the worker finish and `step` pick it up.
    fn settle(app: &mut SoundApp) {
        let ctx = egui::Context::default();
        for _ in 0..600 {
            step(app, &ctx);
            if app.ml.running.is_none() {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(50));
        }
        panic!("the job did not finish");
    }

    #[test]
    fn the_palette_and_menus_open_the_dialog_instead_of_running_the_job() {
        let (mut app, _) = app();
        app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
        crate::menus::invoke_menu(&mut app, "ml.separate", "");
        assert_eq!(app.ml.dialog.as_ref().map(|d| d.command), Some("ml.separate"));
        assert!(app.ml.running.is_none(), "nothing runs until Run is pressed");
    }

    #[test]
    fn separation_dialogs_default_to_high_quality_and_send_it() {
        let (mut app, clip) = app();
        for cmd in ["ui.ml.separate", "ui.ml.karaoke", "ui.ml.drums"] {
            app.ml.dialog = None;
            assert!(app.run(cmd, json!({"clip": clip})).is_ok());
            let d = app.ml.dialog.clone().unwrap();
            assert_eq!(d.quality, Quality::High, "{cmd}");
            let p = params(&d, Device::Cpu);
            assert_eq!(p["quality"].as_str(), Some("high"), "{cmd}");
            let job = ml::prepare(&app.engine, d.command, &p).unwrap();
            assert_eq!(job.quality, Quality::High);
        }
        // Headless, the default stays Fast (and a bad value is refused).
        let job = ml::prepare(&app.engine, "ml.separate", &json!({"clip": clip})).unwrap();
        assert_eq!(job.quality, Quality::Fast);
        assert!(ml::prepare(&app.engine, "ml.separate", &json!({"clip": clip, "quality": "ultra"})).is_err());
    }

    #[test]
    fn convert_to_midi_opens_its_dialog_with_the_model_defaults() {
        let (mut app, clip) = app();
        crate::menus::invoke_menu(&mut app, "ml.midi", "");
        let Some(d) = app.ml.dialog.clone() else { panic!("no dialog") };
        assert_eq!(d.command, "ml.midi");
        assert_eq!(d.midi, MidiDialog::new(MidiModel::General));
        assert!(app.ml.running.is_none());
        // From the clip context menu, on that clip; the piano model's settings go to the command.
        app.ml.dialog = None;
        assert!(app.run("ui.ml.midi", json!({"clip": clip})).is_ok());
        let mut d = app.ml.dialog.clone().unwrap();
        assert_eq!(d.target, json!({"clip": clip}));
        d.midi = MidiDialog::new(MidiModel::Piano);
        let p = params(&d, Device::Cpu);
        assert_eq!((p["model"].as_str(), p["pedal"].as_bool(), p["pitch_bends"].as_bool()), (Some("piano"), Some(true), Some(false)));
        // Piano isolates the piano stem by default; General on the Bass track, the bass stem.
        assert_eq!(p["isolate"].as_str(), Some("piano"));
        assert_eq!(ml::midi_options("ml.midi", &p).unwrap().isolate, Isolate::Piano);
        let general = MidiDialog::for_source(MidiModel::General, Some("Bass"));
        assert_eq!(general.isolate, Isolate::Bass);
        let job = ml::prepare(&app.engine, "ml.midi", &json!({"clip": clip, "model": "general"})).unwrap();
        assert!(matches!(job.kind, ml::JobKind::Midi(o) if o.isolate == Isolate::Bass), "{:?}", job.kind);
        let job = ml::prepare(&app.engine, "ml.midi", &json!({"clip": clip, "model": "general", "isolate": "none"})).unwrap();
        assert!(matches!(job.kind, ml::JobKind::Midi(o) if o.isolate == Isolate::None));
        assert!((p["onset_threshold"].as_f64().unwrap() - 0.3).abs() < 1e-6);
        assert!(ml::midi_options("ml.midi", &p).is_ok());
        // Drums: the five per-drum thresholds start at the published values and go to the command.
        d.midi = MidiDialog::new(MidiModel::Drums);
        assert_eq!(d.midi.drum_thresholds, ml::adtof::THRESHOLDS);
        d.midi.drum_thresholds[1] = 0.5;
        let p = params(&d, Device::Cpu);
        let o = ml::midi_options("ml.midi", &p).unwrap();
        assert_eq!(o.drum_thresholds(), [0.22, 0.5, 0.32, 0.22, 0.30]);
    }

    #[test]
    fn browser_drops_and_module_swaps_wait_while_a_job_runs() {
        use crate::browser::catalog::Kind;
        use crate::browser::drop::{DropTarget, PluginDrag, apply_drop};
        let (mut app, clip) = app();
        let bass = app.engine.session().track_by_name("Bass").map(|t| t.id).unwrap();
        app.engine.execute("mix.module_add", &json!({"track": "Bass", "plugin": "eq_7band"})).unwrap();
        let module = app.engine.session().track(bass).and_then(|t| t.mixer.inserts.last()).map(|i| i.id).unwrap();
        let before = (app.engine.session().clone(), app.engine.undo_history());
        start(&mut app, "ml.clean", &json!({"clip": clip, "dereverb": false})).unwrap();
        let effect = PluginDrag { plugin: "eq_1band".into(), kind: Kind::Effect, preset: None };
        assert_eq!(apply_drop(&mut app, &effect, DropTarget::NewTrack), Err(BLOCKED.to_string()));
        assert_eq!(apply_drop(&mut app, &effect, DropTarget::InsertSlot(bass, 0)), Err(BLOCKED.to_string()));
        assert_eq!(crate::mix_window::replace_module(&mut app, bass, module, "eq_1band"), Err(BLOCKED.to_string()));
        assert_eq!((app.engine.session().clone(), app.engine.undo_history()), before);
        app.ml.running.as_ref().unwrap().cancel.cancel();
        settle(&mut app);
    }

    #[test]
    fn edits_wait_while_a_job_runs_and_cancel_applies_nothing() {
        let (mut app, clip) = app();
        let before = (app.engine.session().clone(), app.engine.undo_history());
        start(&mut app, "ml.clean", &json!({"clip": clip, "dereverb": false})).unwrap();
        for id in ["edit.undo", "track.delete", "mix.volume", "ml.separate", "transport.record"] {
            let r = app.run(id, json!({"tracks": ["Bass"], "db": -6.0}));
            assert_eq!(r, Err(BLOCKED.to_string()), "{id}");
        }
        // Views and windows still work.
        assert!(app.run("window.mix", json!({})).is_ok());
        app.ml.running.as_ref().unwrap().cancel.cancel();
        settle(&mut app);
        assert!(app.ui.status.contains("cancelled"), "{}", app.ui.status);
        assert_eq!((app.engine.session().clone(), app.engine.undo_history()), before);
        // Once it's done, edits work again; a job run to the end applies as one undo step.
        start(&mut app, "ml.clean", &json!({"clip": clip, "dereverb": false})).unwrap();
        settle(&mut app);
        assert_eq!(app.engine.undo_label(), Some("Clean Vocal"), "{}", app.ui.status);
        assert!(app.run("edit.undo", json!({})).is_ok());
    }
}

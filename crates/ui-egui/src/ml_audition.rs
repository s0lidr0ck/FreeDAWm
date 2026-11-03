//! Convert to MIDI's audition: **Analyse** runs the network once (on a worker, with the isolation
//! step when asked) and keeps the model's raw output in the dialog; every slider change then
//! re-decodes it in milliseconds and redraws a mini piano roll over the source's waveform.
//! **Play** loops the selection (or the clip) through the instrument track Convert would make,
//! soloed with the source at −12 dB, with a Source / Notes / Both toggle; it stops when the
//! dialog closes. **Convert** commits the notes on screen as one undo step.
//!
//! Playback goes through the Vocal Editor's preview path: the player is handed a copy of the
//! session ([`soundcraft_engine::ml::audition_session`]) and the document is untouched.

use crate::SoundApp;
use crate::theme::Tokens;
use serde_json::Value;
use soundcraft_engine::ml::{
    self, Analysed, AuditionMix, Cancel, Ctx, Isolate, MidiModel, MidiOptions, MlError, MlJob, ProgressCell, Quality, Transcription,
};
use soundcraft_time::Range;
use std::sync::mpsc::{Receiver, TryRecvError};

/// Columns of the waveform kept for the roll.
pub const WAVE_COLUMNS: usize = 320;
/// The mini piano roll's height, points.
pub const ROLL_HEIGHT: f32 = 120.0;

/// An analysis running on its worker thread.
pub struct Analysing {
    job: MlJob,
    cancel: Cancel,
    progress: ProgressCell,
    rx: Receiver<Result<Vec<(Analysed, Vec<String>)>, MlError>>,
    waveform: Vec<(f32, f32)>,
    key: (MidiModel, Isolate, Quality),
}

impl Analysing {
    pub fn progress(&self) -> (f32, String) {
        (self.progress.get(), self.progress.what())
    }

    pub fn cancel(&self) {
        self.cancel.cancel();
    }

    pub fn cancelled(&self) -> bool {
        self.cancel.is_cancelled()
    }
}

/// An analysed source, decoded with the dialog's current settings.
#[derive(Debug, Clone)]
pub struct Audition {
    /// The job (targets only) Convert commits through.
    pub job: MlJob,
    pub parts: Vec<Analysed>,
    /// Where it ran, which stem was transcribed.
    pub notes: Vec<String>,
    /// The first part's waveform, `WAVE_COLUMNS` (min, max) columns.
    pub waveform: Vec<(f32, f32)>,
    /// One transcription per part, from `opts`.
    pub decoded: Vec<Transcription>,
    pub opts: MidiOptions,
    pub mix: AuditionMix,
    /// What the analysis was made with; a change needs Analyse again.
    pub key: (MidiModel, Isolate, Quality),
}

impl Audition {
    /// Decodes every part again when `opts` differ from the last decode (milliseconds; the
    /// network is not run). True when something changed.
    pub fn redecode(&mut self, opts: &MidiOptions) -> bool {
        if *opts == self.opts && self.decoded.len() == self.parts.len() {
            return false;
        }
        self.opts = *opts;
        self.decoded = self.parts.iter().map(|a| a.decode(opts)).collect();
        true
    }

    /// How many notes are on screen.
    pub fn note_count(&self) -> usize {
        self.decoded.iter().map(|t| t.notes.len()).sum()
    }

    /// The first part's span in session time.
    fn span(&self) -> Option<Range> {
        self.job.parts.first().and_then(|p| match &p.target {
            ml::Target::Midi { start, len, .. } => Some(Range::new(*start, start.saturating_add(*len))),
            _ => None,
        })
    }
}

/// `audio` (`[channel][frame]`) as `columns` (min, max) pairs of its channel average.
pub fn waveform(audio: &[Vec<f32>], columns: usize) -> Vec<(f32, f32)> {
    let n = audio.iter().map(Vec::len).max().unwrap_or(0);
    if n == 0 || columns == 0 || audio.is_empty() {
        return Vec::new();
    }
    let g = 1.0 / audio.len() as f32;
    (0..columns)
        .map(|c| {
            let a = c * n / columns;
            let b = ((c + 1) * n / columns).max(a + 1).min(n);
            let (mut lo, mut hi) = (f32::MAX, f32::MIN);
            for i in a..b {
                let v: f32 = audio.iter().map(|ch| ch.get(i).copied().unwrap_or(0.0)).sum::<f32>() * g;
                lo = lo.min(v);
                hi = hi.max(v);
            }
            if lo > hi { (0.0, 0.0) } else { (lo, hi) }
        })
        .collect()
}

/// The analysis key of `opts` at `quality` (which only matters when a stem is isolated).
pub fn key_of(opts: &MidiOptions, quality: Quality) -> (MidiModel, Isolate, Quality) {
    (opts.model, opts.isolate, if opts.isolate == Isolate::None { Quality::Fast } else { quality })
}

/// Starts Analyse for the `ml.midi` params `p`: renders the source now, runs the isolation and
/// the network on a worker thread. Any audition on screen goes.
pub fn start(app: &mut SoundApp, p: &Value) -> Result<(), String> {
    if app.ml.running.is_some() || app.ml.analysing.is_some() {
        return Err("an ML job is already running".into());
    }
    close(app);
    app.capture_plugin_states();
    let mut worker = ml::prepare(&app.engine, "ml.midi", p).map_err(|e| e.to_string())?;
    let ml::JobKind::Midi(opts) = worker.kind else { return Err("not a Convert to MIDI job".into()) };
    let waveform = worker.parts.first().map(|part| waveform(&part.audio, WAVE_COLUMNS)).unwrap_or_default();
    let job = worker.without_audio();
    let cancel = Cancel::new();
    let progress = ProgressCell::new();
    let (tx, rx) = std::sync::mpsc::channel();
    let ctx = Ctx::with_cell(cancel.clone(), progress.clone());
    std::thread::Builder::new()
        .name("ml-analyse".into())
        .spawn(move || {
            let _ = tx.send(worker.analyse(&ctx));
        })
        .map_err(|e| format!("could not start the analysis: {e}"))?;
    let key = key_of(&opts, job.quality);
    app.ml.analysing = Some(Analysing { job, cancel, progress, rx, waveform, key });
    Ok(())
}

/// Picks up a finished analysis; stops an audition whose playback ended elsewhere. Called every
/// frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    if app.audition_session.is_some() && app.engine.transport.recording {
        // Recording started over an audition: the player hears (and records against) the
        // document, never the audition's copy. The transport is left running.
        app.audition_session = None;
        if let Some(p) = &app.player {
            p.update_session(app.player_session());
        }
    }
    if app.audition_session.is_some() && !app.is_playing() {
        // Stopped from the transport: the player hears the document again.
        stop(app);
    }
    let Some(run) = &app.ml.analysing else { return };
    let got = match run.rx.try_recv() {
        Ok(r) => r,
        Err(TryRecvError::Empty) => {
            ctx.request_repaint_after(std::time::Duration::from_millis(100));
            return;
        }
        Err(TryRecvError::Disconnected) => Err(MlError::Runtime("the analysis stopped unexpectedly".into())),
    };
    let Some(run) = app.ml.analysing.take() else { return };
    let got = if run.cancel.is_cancelled() { Err(MlError::Cancelled) } else { got };
    match got {
        Ok(parts) => {
            let ml::JobKind::Midi(opts) = run.job.kind else { return };
            let mut notes: Vec<String> = Vec::new();
            let mut analysed = Vec::with_capacity(parts.len());
            for (a, n) in parts {
                for n in n {
                    if !notes.contains(&n) {
                        notes.push(n);
                    }
                }
                analysed.push(a);
            }
            let mut a = Audition {
                job: run.job,
                parts: analysed,
                notes,
                waveform: run.waveform,
                decoded: Vec::new(),
                opts,
                mix: AuditionMix::Both,
                key: run.key,
            };
            a.redecode(&opts);
            app.ui.status = format!("Analysed: {} notes ({})", a.note_count(), a.notes.join("; "));
            app.ml.audition = Some(a);
        }
        Err(MlError::Cancelled) => app.ui.status = "Analysis cancelled".into(),
        Err(e) => app.ui.status = format!("Analyse: {e}"),
    }
}

/// The loop the audition plays: the edit selection where it overlaps the source, else the
/// whole source.
pub fn loop_range(app: &SoundApp, a: &Audition) -> Option<Range> {
    let span = a.span()?;
    let sel = app.engine.session().edit.selection;
    let inter = Range::new(sel.start.max(span.start), sel.end.min(span.end));
    Some(if !sel.is_empty() && inter.end > inter.start { inter } else { span })
}

/// Plays the audition: the notes on screen through the target instrument, looped. Refused while
/// recording, or while the transport plays something else.
pub fn play(app: &mut SoundApp) -> Result<(), String> {
    if app.engine.transport.recording {
        return Err("Stop recording before auditioning".into());
    }
    if app.is_playing() && app.audition_session.is_none() {
        return Err("Stop the transport before auditioning".into());
    }
    let Some(a) = &app.ml.audition else { return Err("Analyse first".into()) };
    let Some(range) = loop_range(app, a) else { return Err("nothing to play".into()) };
    let Some((s, _)) = ml::audition_session(app.engine.session(), &a.job, &a.decoded, a.mix) else {
        return Err("the source is gone".into());
    };
    if app.is_playing() {
        app.stop_play();
    }
    app.audition_session = Some(std::sync::Arc::new(s));
    app.start_play(range.start, None, Some(range));
    Ok(())
}

/// Stops the audition; the player hears the document again.
pub fn stop(app: &mut SoundApp) {
    if app.audition_session.take().is_none() {
        return;
    }
    if app.is_playing() {
        app.stop_play();
    }
    if let Some(p) = &app.player {
        p.update_session(app.player_session());
    }
}

/// The notes changed (a re-decode, or the Source / Notes / Both toggle) while playing: the
/// player gets the new copy without a restart.
pub fn refresh(app: &mut SoundApp) {
    if app.audition_session.is_none() {
        return;
    }
    let Some(a) = &app.ml.audition else { return };
    if let Some((s, _)) = ml::audition_session(app.engine.session(), &a.job, &a.decoded, a.mix) {
        app.audition_session = Some(std::sync::Arc::new(s));
        if let Some(p) = &app.player {
            p.update_session(app.player_session());
        }
    }
}

/// Convert: the notes on screen into the session as one undo step; the audition is over.
pub fn commit(app: &mut SoundApp) -> Result<Value, String> {
    stop(app);
    let Some(a) = app.ml.audition.take() else { return Err("Analyse first".into()) };
    ml::commit_midi(&mut app.engine, &a.job, a.decoded, a.notes).map_err(|e| e.to_string())
}

/// The dialog closed (or Analyse starts again): playback stops, the analysis and any worker go.
pub fn close(app: &mut SoundApp) {
    stop(app);
    if let Some(run) = app.ml.analysing.take() {
        run.cancel.cancel();
    }
    app.ml.audition = None;
}

/// Where the audition's playhead is, in seconds from the first part's start, while it plays.
pub fn playhead_secs(app: &SoundApp, a: &Audition) -> Option<f64> {
    if app.audition_session.is_none() || !app.is_playing() {
        return None;
    }
    let span = a.span()?;
    let pos = app.engine.transport.position;
    (pos >= span.start && pos <= span.end).then(|| (pos - span.start) as f64 / app.engine.session().sample_rate.as_f64())
}

/// The mini piano roll: the first part's waveform with its decoded notes over it (drums: one lane
/// per class; else the pitches found, at least an octave), and the playhead while it plays.
pub fn roll(ui: &mut egui::Ui, a: &Audition, playhead: Option<f64>) {
    let t = Tokens::current();
    let width = ui.available_width().max(120.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, ROLL_HEIGHT), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, t.playlist_bg);
    // The waveform, centred.
    let mid = rect.center().y;
    let half = rect.height() * 0.48;
    let cols = a.waveform.len().max(1) as f32;
    for (i, (lo, hi)) in a.waveform.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) / cols * rect.width();
        let (y0, y1) = (mid - hi.clamp(-1.0, 1.0) * half, mid - lo.clamp(-1.0, 1.0) * half);
        p.line_segment([egui::pos2(x, y0.min(y1 - 1.0)), egui::pos2(x, y1)], egui::Stroke::new(1.0, t.text_dim.gamma_multiply(0.45)));
    }
    let Some(first) = a.decoded.first() else { return };
    let secs = a.parts.first().map_or(0.0, |p| p.secs).max(1e-6);
    let x_of = |s: f64| rect.left() + (s / secs).clamp(0.0, 1.0) as f32 * rect.width();
    if first.drums {
        let lanes = ml::adtof::LABELS;
        let row_h = rect.height() / lanes.len() as f32;
        for (i, name) in ml::adtof::CLASS_NAMES.iter().enumerate() {
            let y = rect.top() + i as f32 * row_h;
            p.text(egui::pos2(rect.left() + 3.0, y + 2.0), egui::Align2::LEFT_TOP, *name, egui::FontId::proportional(9.0), t.text_dim);
        }
        for n in &first.notes {
            let Some(i) = lanes.iter().position(|l| *l == n.pitch) else { continue };
            let y = rect.top() + i as f32 * row_h;
            let x = x_of(n.start);
            let h = row_h * (0.3 + 0.7 * f32::from(n.velocity) / 127.0);
            p.rect_filled(egui::Rect::from_min_size(egui::pos2(x - 1.0, y + row_h - h), egui::vec2(3.0, h)), 0.0, t.accent);
        }
    } else {
        let (lo, hi) = first.notes.iter().fold((127u8, 0u8), |(lo, hi), n| (lo.min(n.pitch), hi.max(n.pitch)));
        let (lo, hi) = if first.notes.is_empty() { (48, 72) } else { (lo.saturating_sub(1), hi.saturating_add(1).max(lo.saturating_add(12))) };
        let rows = f32::from(hi - lo) + 1.0;
        let row_h = rect.height() / rows;
        for n in &first.notes {
            let y = rect.bottom() - (f32::from(n.pitch - lo) + 1.0) * row_h;
            let x0 = x_of(n.start);
            let x1 = x_of(n.end).max(x0 + 2.0);
            let c = t.accent.gamma_multiply(0.4 + 0.6 * f32::from(n.velocity) / 127.0);
            p.rect_filled(egui::Rect::from_min_max(egui::pos2(x0, y), egui::pos2(x1, y + row_h.max(2.0) - 0.5)), 1.0, c);
        }
        for (s, e) in &first.pedals {
            p.line_segment(
                [egui::pos2(x_of(*s), rect.bottom() - 1.5), egui::pos2(x_of(*e), rect.bottom() - 1.5)],
                egui::Stroke::new(3.0, t.text_dim.gamma_multiply(0.6)),
            );
        }
        let name = |m: u8| {
            const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
            format!("{}{}", N[usize::from(m % 12)], i32::from(m / 12) - 1)
        };
        p.text(egui::pos2(rect.left() + 3.0, rect.top() + 2.0), egui::Align2::LEFT_TOP, name(hi), egui::FontId::proportional(9.0), t.text_dim);
        p.text(egui::pos2(rect.left() + 3.0, rect.bottom() - 2.0), egui::Align2::LEFT_BOTTOM, name(lo), egui::FontId::proportional(9.0), t.text_dim);
    }
    if let Some(s) = playhead {
        let x = x_of(s);
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.0, t.playhead));
    }
}

/// A synthetic audition on the clip `clip` for renders and tests (no model needed): a General
/// analysis holding a rising line of four notes over the clip, and its waveform.
pub fn demo(app: &mut SoundApp, clip: soundcraft_model::ClipId) -> Result<(), String> {
    use ml::basicpitch::{N_PITCH, Raw};
    let s = app.engine.session();
    let Some((track, c)) = s.find_clip(clip).map(|(t, c)| (t, c.clone())) else { return Err("no such clip".into()) };
    let secs = c.length as f64 / s.sample_rate.as_f64();
    let audio = soundcraft_mix::render_clips(s, track, c.range());
    let frames = (secs * 22_050.0 / 256.0) as usize;
    let mut note = vec![0.0f32; frames * N_PITCH];
    let mut onset = vec![0.0f32; frames * N_PITCH];
    for (i, pitch) in [40usize, 43, 47, 52].into_iter().enumerate() {
        let (a, b) = (frames * i / 4 + 2, frames * (i + 1) / 4 - 4);
        for r in a..b.max(a + 1) {
            if let Some(v) = note.get_mut(r * N_PITCH + pitch - 21) {
                *v = 0.9 - 0.15 * i as f32;
            }
        }
        if let Some(v) = onset.get_mut(a * N_PITCH + pitch - 21) {
            *v = 0.95;
        }
    }
    let analysed = Analysed { analysis: ml::Analysis::General(Raw { frames, note, onset, contour: Vec::new() }), secs };
    let job = ml::MlJob {
        command: "ml.midi",
        kind: ml::JobKind::Midi(MidiOptions::new(MidiModel::General)),
        parts: vec![ml::Part { target: ml::Target::Midi { track, clip, start: c.start, len: c.length, name: c.name.clone() }, audio: Vec::new() }],
        rate: s.sample_rate.hz(),
        device: ml::Device::Cpu,
        models_dir: ml::models_dir(),
        quality: Quality::Fast,
        keep: Vec::new(),
        mute_original: false,
        lane: Default::default(),
    };
    let opts = MidiOptions::new(MidiModel::General);
    let mut a = Audition {
        job,
        parts: vec![analysed],
        notes: vec!["demo".into()],
        waveform: waveform(&audio, WAVE_COLUMNS),
        decoded: Vec::new(),
        opts,
        mix: AuditionMix::Both,
        key: key_of(&opts, Quality::Fast),
    };
    a.redecode(&opts);
    app.ml.audition = Some(a);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use serde_json::json;

    fn app() -> (SoundApp, soundcraft_model::ClipId) {
        let app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        let s = app.engine.session();
        let clip = s.tracks.iter().find(|t| t.name == "Bass").and_then(|t| t.clips().first()).map(|c| c.id).unwrap();
        (app, clip)
    }

    #[test]
    fn play_refuses_while_recording_and_stops_with_the_dialog() {
        let (mut app, clip) = app();
        demo(&mut app, clip).unwrap();
        app.engine.transport.recording = true;
        assert!(play(&mut app).is_err());
        assert!(app.audition_session.is_none());
        app.engine.transport.recording = false;
        play(&mut app).unwrap();
        assert!(app.audition_session.is_some() && app.is_playing());
        // The player's copy has the notes soloed with the source; the document is untouched.
        let pv = app.audition_session.clone().unwrap();
        assert!(pv.tracks.iter().any(|t| t.name == "Bass MIDI" && t.mixer.solo));
        assert!(app.engine.session().tracks.iter().all(|t| t.name != "Bass MIDI"));
        close(&mut app);
        assert!(app.audition_session.is_none() && !app.is_playing() && app.ml.audition.is_none());
    }

    #[test]
    fn recording_over_an_audition_hands_the_player_the_document() {
        let (mut app, clip) = app();
        demo(&mut app, clip).unwrap();
        play(&mut app).unwrap();
        app.engine.transport.recording = true;
        step(&mut app, &egui::Context::default());
        assert!(app.audition_session.is_none());
        assert!(app.player_session().tracks.iter().all(|t| t.name != "Bass MIDI"));
    }

    #[test]
    fn opening_another_ml_dialog_ends_the_audition() {
        let (mut app, clip) = app();
        demo(&mut app, clip).unwrap();
        play(&mut app).unwrap();
        crate::ml_ui::open(&mut app, "ml.separate", Some((json!({"clip": clip}), "Bass".into()))).unwrap();
        assert!(app.audition_session.is_none() && app.ml.audition.is_none() && !app.is_playing());
    }

    #[test]
    fn a_change_redecodes_without_the_network_and_convert_commits_what_is_shown() {
        let (mut app, clip) = app();
        demo(&mut app, clip).unwrap();
        let a = app.ml.audition.as_mut().unwrap();
        assert_eq!(a.note_count(), 4);
        let strict = MidiOptions { frame_threshold: Some(0.85), ..a.opts };
        assert!(a.redecode(&strict));
        assert!(!a.redecode(&strict), "nothing to do twice");
        assert_eq!(a.note_count(), 1, "{:?}", a.decoded);
        assert!(app.ml.analysing.is_none(), "no worker was started");
        let shown = app.ml.audition.as_ref().unwrap().decoded.clone();
        let before = app.engine.session().tracks.len();
        let v = commit(&mut app).unwrap();
        assert_eq!(v["made"][0]["notes"], json!(1));
        let s = app.engine.session();
        assert_eq!(s.tracks.len(), before + 1);
        let made = s.tracks.iter().find(|t| t.name == "Bass MIDI").unwrap();
        let c = made.clips().first().unwrap();
        let want = ml::midi_sequence(s, c.start, c.length, &shown[0]);
        assert_eq!(c.midi_playing().cloned(), Some(want));
        assert_eq!(app.engine.undo_label(), Some("Convert to MIDI"));
        assert!(app.ml.audition.is_none());
    }

    #[test]
    fn the_dialog_keeps_an_audition_whose_quality_does_not_apply() {
        // Without isolation the dialog's Quality (High by default) is not part of the analysis.
        let mut o = MidiOptions::new(MidiModel::General);
        o.isolate = Isolate::None;
        assert_eq!(key_of(&o, Quality::High), key_of(&o, Quality::Fast));
        o.isolate = Isolate::Other;
        assert_ne!(key_of(&o, Quality::High), key_of(&o, Quality::Fast));
    }

    #[test]
    fn waveform_columns_cover_the_audio() {
        let audio = vec![(0..1000).map(|i| if i < 500 { 0.5 } else { -0.25 }).collect::<Vec<f32>>()];
        let w = waveform(&audio, 4);
        assert_eq!(w, vec![(0.5, 0.5), (0.5, 0.5), (-0.25, -0.25), (-0.25, -0.25)]);
        assert!(waveform(&[], 4).is_empty());
    }
}

//! Event › Detect Tempo…: the dialog. Opening it starts the analysis on a worker (a progress line
//! shows while it runs); the result shows the average tempo and a beat preview strip over the
//! source's waveform (beat ticks, downbeats bold). **Play** loops the source soloed with a click
//! on the detected beats; **Apply** writes the tempo map through `event.tempo_detect` with the
//! beats on screen, as one undo step.

use crate::SoundApp;
use crate::theme::Tokens;
use serde_json::{Value, json};
use soundcraft_engine::tempo_detect::{self as td, Analysis, TempoMode, TempoSource};
use std::sync::mpsc::{Receiver, TryRecvError};

/// The preview strip's height, points.
pub const STRIP_HEIGHT: f32 = 110.0;

/// The dialog's state.
pub struct DetectDialog {
    pub source: TempoSource,
    /// "Selected clips (2)", "Selection on 3 tracks", "Whole session (5 tracks)".
    pub source_label: String,
    pub mode: TempoMode,
    /// Index into [`td::RANGES`].
    pub range: usize,
    pub set_bar1: bool,
    /// After detecting: Straighten (conform the audio to a steady tempo) instead of Follow.
    pub straighten: bool,
    /// Straighten's tempo (the detected average until changed).
    pub straighten_bpm: f64,
    /// Keep feel, 0..100 %: 0 = locked to the grid, 100 = unchanged.
    pub keep_feel: f64,
    /// The tracks Straighten stretches (all with the same warp), with their names.
    pub tracks: Vec<(soundcraft_model::TrackId, String, bool)>,
    pub analysis: Option<Analysis>,
    /// A worker's result, while it runs.
    running: Option<Receiver<Analysis>>,
    /// Play started from this dialog (the audition copy is playing).
    pub playing: bool,
    pub error: Option<String>,
}

#[derive(Default)]
pub struct TempoUi {
    pub dialog: Option<DetectDialog>,
}

fn source_label(app: &SoundApp, src: &TempoSource, p: &Value) -> String {
    let n = src.tracks.len();
    let tracks = if n == 1 { "1 track".to_string() } else { format!("{n} tracks") };
    if !src.clips.is_empty() {
        let c = src.clips.len();
        return if c == 1 { "Selected clip".into() } else { format!("Selected clips ({c})") };
    }
    let whole = src.range.start == 0 && src.range.end >= app.engine.session().content_end() && p.get("start").is_none();
    if whole && app.engine.session().edit.selection.is_empty() { format!("Whole session ({tracks})") } else { format!("Selection on {tracks}") }
}

/// Open the dialog on the source `p` names (else the selected clips, the selection or the whole
/// session) and start the analysis.
pub fn open(app: &mut SoundApp, p: &Value) -> Result<(), String> {
    let source = soundcraft_engine::cmd::tempo::detect_source(&app.engine, p).map_err(|e| e.to_string())?;
    let source_label = source_label(app, &source, p);
    // Straighten's tracks: those the detection hears, and every audio track with audio in range.
    let s = app.engine.session();
    let tracks = s
        .tracks
        .iter()
        .filter(|t| {
            source.tracks.contains(&t.id)
                || (t.kind.has_playlist() && !t.kind.is_midi() && t.clips().iter().any(|c| c.is_audio() && c.range().overlaps(&source.range)))
        })
        .map(|t| (t.id, t.name.clone(), true))
        .collect();
    close(app);
    app.tempo.dialog = Some(DetectDialog {
        source,
        source_label,
        mode: TempoMode::Follow,
        range: 0,
        set_bar1: true,
        straighten: false,
        straighten_bpm: 0.0,
        keep_feel: 0.0,
        tracks,
        analysis: None,
        running: None,
        playing: false,
        error: None,
    });
    start(app);
    Ok(())
}

/// (Re)run the analysis with the dialog's range.
fn start(app: &mut SoundApp) {
    stop(app);
    let Some(d) = app.tempo.dialog.as_mut() else { return };
    let (_, lo, hi) = td::RANGES.get(d.range).copied().unwrap_or(td::RANGES[0]);
    let session = app.engine.session().clone();
    let source = d.source.clone();
    d.analysis = None;
    d.error = None;
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (tx, rx) = std::sync::mpsc::channel();
        let spawned = std::thread::Builder::new().name("tempo-detect".into()).spawn(move || {
            let _ = tx.send(td::analyse(&session, source, (lo, hi)));
        });
        match spawned {
            Ok(_) => d.running = Some(rx),
            Err(e) => d.error = Some(format!("could not start the analysis: {e}")),
        }
    }
    #[cfg(target_arch = "wasm32")]
    {
        d.analysis = Some(td::analyse(&session, source, (lo, hi)));
    }
}

/// The parameters Apply sends: the source, mode and the beats on screen.
pub fn apply_params(d: &DetectDialog) -> Option<Value> {
    let a = d.analysis.as_ref()?;
    let mut p = json!({
        "mode": d.mode.id(),
        "range": td::RANGES.get(d.range).map_or("auto", |r| r.0),
        "set_bar1": d.set_bar1,
        "apply": true,
        "beats": a.detection.beats,
        "downbeats": a.detection.downbeats,
    });
    if d.source.clips.is_empty() {
        p["tracks"] = json!(d.source.tracks.iter().map(|t| t.0).collect::<Vec<_>>());
        p["start"] = json!(d.source.range.start);
        p["end"] = json!(d.source.range.end);
    } else {
        p["clips"] = json!(d.source.clips.iter().map(|c| c.0).collect::<Vec<_>>());
    }
    Some(p)
}

/// The parameters Straighten sends: the beats on screen, the tempo, keep feel and the tracks.
pub fn straighten_params(d: &DetectDialog) -> Option<Value> {
    let a = d.analysis.as_ref()?;
    let tracks: Vec<u64> = d.tracks.iter().filter(|t| t.2).map(|t| t.0.0).collect();
    Some(json!({
        "beats": a.detection.beats,
        "downbeats": a.detection.downbeats,
        "bpm": d.straighten_bpm,
        "keep_feel": d.keep_feel / 100.0,
        "tracks": tracks,
    }))
}

fn apply(app: &mut SoundApp) {
    stop(app);
    if app.tempo.dialog.as_ref().is_some_and(|d| d.straighten) {
        let Some(p) = app.tempo.dialog.as_ref().and_then(straighten_params) else { return };
        match app.run("event.tempo_conform", p) {
            Ok(v) => {
                let warn = v["warnings"].as_array().and_then(|w| w.first()).and_then(Value::as_str).map(|w| format!(" ({w})")).unwrap_or_default();
                app.ui.status = format!("Straightened {} clip(s) to {:.1} BPM{warn}", v["clips"], v["bpm"].as_f64().unwrap_or(0.0));
                let _ = app.run("view.ruler", json!({"ruler": "tempo", "visible": true}));
                app.tempo.dialog = None;
            }
            Err(e) => {
                if let Some(d) = app.tempo.dialog.as_mut() {
                    d.error = Some(e);
                }
            }
        }
        return;
    }
    let Some(p) = app.tempo.dialog.as_ref().and_then(apply_params) else { return };
    match app.run("event.tempo_detect", p) {
        Ok(v) => {
            let n = v["points"].as_array().map_or(0, Vec::len);
            app.ui.status =
                format!("Tempo map written: {:.1} BPM average, {n} tempo point{}", v["bpm"].as_f64().unwrap_or(0.0), if n == 1 { "" } else { "s" });
            let _ = app.run("view.ruler", json!({"ruler": "tempo", "visible": true}));
            app.tempo.dialog = None;
        }
        Err(e) => {
            if let Some(d) = app.tempo.dialog.as_mut() {
                d.error = Some(e);
            }
        }
    }
}

/// Play: the source soloed with a click on the detected beats, looped.
pub fn play(app: &mut SoundApp) -> Result<(), String> {
    if app.engine.transport.recording {
        return Err("Stop recording before auditioning".into());
    }
    if app.is_playing() && app.audition_session.is_none() {
        return Err("Stop the transport before auditioning".into());
    }
    let Some(a) = app.tempo.dialog.as_ref().and_then(|d| d.analysis.as_ref()) else { return Err("Wait for the analysis".into()) };
    let Some((s, range)) = td::audition_session(app.engine.session(), a) else { return Err("nothing to play".into()) };
    if app.is_playing() {
        app.stop_play();
    }
    app.audition_session = Some(std::sync::Arc::new(s));
    app.start_play(range.start, None, Some(range));
    if let Some(d) = app.tempo.dialog.as_mut() {
        d.playing = true;
    }
    Ok(())
}

/// Stop a Play started here; the player hears the document again.
pub fn stop(app: &mut SoundApp) {
    let Some(d) = app.tempo.dialog.as_mut() else { return };
    if !std::mem::take(&mut d.playing) {
        return;
    }
    if app.audition_session.take().is_some() && app.is_playing() {
        app.stop_play();
    }
    if let Some(p) = &app.player {
        p.update_session(app.player_session());
    }
}

/// The dialog closes: playback stops; a running worker's result is dropped.
pub fn close(app: &mut SoundApp) {
    stop(app);
    app.tempo.dialog = None;
}

/// Picks up a finished analysis and notices playback stopped elsewhere. Every frame.
pub fn step(app: &mut SoundApp, ctx: &egui::Context) {
    let playing = app.is_playing();
    let Some(d) = app.tempo.dialog.as_mut() else { return };
    if d.playing && (!playing || app.audition_session.is_none()) {
        stop(app);
        return;
    }
    let Some(rx) = &d.running else { return };
    match rx.try_recv() {
        Ok(a) => {
            d.running = None;
            d.straighten_bpm = (a.detection.bpm * 10.0).round() / 10.0;
            if a.detection.beats.len() < 2 {
                d.error = Some("No steady beat found in this audio".into());
            }
            d.analysis = Some(a);
        }
        Err(TryRecvError::Empty) => ctx.request_repaint_after(std::time::Duration::from_millis(100)),
        Err(TryRecvError::Disconnected) => {
            d.running = None;
            d.error = Some("the analysis stopped unexpectedly".into());
        }
    }
}

/// The largest bar stretch Straighten would need with the dialog's settings.
fn drift_of(app: &SoundApp, d: &DetectDialog) -> Option<f64> {
    let a = d.analysis.as_ref()?;
    let s = app.engine.session();
    let first = *a.detection.beats.first()?;
    let (bpb, q) = td::meter_at(s, first);
    let near = s.sample_rate.samples(0.02);
    let anchor = a.detection.downbeats.first().and_then(|db| a.detection.beats.iter().position(|b| (b - db).abs() <= near)).unwrap_or(0);
    let period = q * 60.0 * s.sample_rate.as_f64() / d.straighten_bpm.max(5.0);
    Some(td::max_bar_drift(&td::conform_pairs(&a.detection.beats, anchor, period, d.keep_feel / 100.0), bpb))
}

/// Per-bar tempo range of an analysis (Follow points): (min, max, bars).
fn bar_stats(app: &SoundApp, a: &Analysis) -> Option<(f64, f64, usize)> {
    let pts = td::tempo_points(app.engine.session(), &a.detection.downbeats, TempoMode::Follow);
    let lo = pts.iter().map(|p| p.1).fold(f64::INFINITY, f64::min);
    let hi = pts.iter().map(|p| p.1).fold(f64::NEG_INFINITY, f64::max);
    (!pts.is_empty()).then_some((lo, hi, pts.len()))
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    step(app, ctx);
    if app.tempo.dialog.is_none() {
        return;
    }
    let mut open = true;
    let mut action: Option<&str> = None;
    egui::Window::new("Detect Tempo")
        .id(egui::Id::new("detect_tempo_dialog"))
        .collapsible(false)
        .resizable(false)
        .default_width(560.0)
        .anchor(egui::Align2::CENTER_CENTER, egui::vec2(0.0, 0.0))
        .open(&mut open)
        .show(ctx, |ui| action = body(app, ui));
    match action {
        Some("apply") => apply(app),
        Some("play") => {
            if let Err(e) = play(app)
                && let Some(d) = app.tempo.dialog.as_mut()
            {
                d.error = Some(e);
            }
        }
        Some("stop") => stop(app),
        Some("redetect") => start(app),
        Some("cancel") => close(app),
        _ => {}
    }
    if !open {
        close(app);
    }
}

fn body(app: &mut SoundApp, ui: &mut egui::Ui) -> Option<&'static str> {
    let t = Tokens::current();
    let mut action = None;
    let stats = app.tempo.dialog.as_ref().and_then(|d| d.analysis.as_ref()).and_then(|a| bar_stats(app, a));
    let sr = app.engine.session().sample_rate.as_f64();
    let playhead = app.is_playing().then_some(app.engine.transport.position);
    let drift = app.tempo.dialog.as_ref().and_then(|d| drift_of(app, d));
    let d = app.tempo.dialog.as_mut()?;
    ui.set_width(560.0);
    egui::Grid::new("detect_tempo_grid").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("Source");
        ui.label(&d.source_label);
        ui.end_row();
        ui.label("Range");
        let before = d.range;
        ui.horizontal(|ui| {
            for (i, (id, _, _)) in td::RANGES.iter().enumerate() {
                let label = if *id == "auto" { "Auto".to_string() } else { id.replace('-', "–") };
                ui.selectable_value(&mut d.range, i, label);
            }
        });
        if d.range != before {
            action = Some("redetect");
        }
        ui.end_row();
    });
    ui.add_space(6.0);
    match (&d.analysis, &d.running) {
        (_, Some(_)) => {
            ui.horizontal(|ui| {
                ui.spinner();
                ui.label(format!("Detecting tempo… ({:.0} s of audio)", d.source.range.len() as f64 / sr));
            });
            ui.add_space(STRIP_HEIGHT - 18.0);
        }
        (Some(a), None) => {
            ui.horizontal(|ui| {
                ui.label(egui::RichText::new(format!("{:.1} BPM", a.detection.bpm)).size(26.0).strong());
                ui.vertical(|ui| {
                    ui.label(format!("{} beats, {} downbeats", a.detection.beats.len(), a.detection.downbeats.len()));
                    if let Some((lo, hi, bars)) = stats {
                        ui.label(egui::RichText::new(format!("{bars} bars from {lo:.1} to {hi:.1} BPM")).color(t.text_dim));
                    }
                });
            });
            strip(ui, a, playhead);
        }
        (None, None) => {
            ui.add_space(STRIP_HEIGHT);
        }
    }
    if let Some(e) = &d.error {
        ui.colored_label(t.rec, e);
    }
    ui.add_space(6.0);
    egui::Grid::new("detect_tempo_opts").num_columns(2).spacing([12.0, 6.0]).show(ui, |ui| {
        ui.label("After detecting");
        ui.horizontal(|ui| {
            ui.radio_value(&mut d.straighten, false, "Follow").on_hover_text("Write the tempo map; the audio is untouched");
            ui.radio_value(&mut d.straighten, true, "Straighten to").on_hover_text("Stretch the audio so every beat lands on a steady tempo");
            ui.add_enabled(d.straighten, egui::DragValue::new(&mut d.straighten_bpm).speed(0.1).range(5.0..=1000.0).suffix(" BPM"));
        });
        ui.end_row();
        if d.straighten {
            ui.label("Keep feel");
            ui.add(egui::Slider::new(&mut d.keep_feel, 0.0..=100.0).suffix(" %").fixed_decimals(0))
                .on_hover_text("0 % locks every beat to the grid, 100 % leaves it as played; in between, each beat moves that far toward the grid");
            ui.end_row();
            ui.label("Tracks");
            ui.vertical(|ui| {
                egui::ScrollArea::vertical().max_height(90.0).show(ui, |ui| {
                    for (_, name, on) in &mut d.tracks {
                        ui.checkbox(on, name.as_str());
                    }
                });
                ui.label(egui::RichText::new("Every track gets exactly the same stretch, so they stay in phase").color(t.text_dim).small());
            });
            ui.end_row();
            if let Some(w) = drift.filter(|w| *w > td::LARGE_DRIFT) {
                ui.label("");
                ui.colored_label(t.meter_yellow, format!("Large drift: a bar is stretched {:.0} %, cymbals may sound stretched", w * 100.0));
                ui.end_row();
            }
            return;
        }
        ui.label("Mode");
        ui.horizontal(|ui| {
            ui.radio_value(&mut d.mode, TempoMode::Constant, "Constant").on_hover_text("One tempo: the average");
            ui.radio_value(&mut d.mode, TempoMode::Follow, "Follow").on_hover_text("A tempo point per bar, for drifting live takes");
            ui.radio_value(&mut d.mode, TempoMode::Smooth, "Smooth").on_hover_text("Per bar, smoothed over five bars");
        });
        ui.end_row();
        ui.label("");
        ui.checkbox(&mut d.set_bar1, "Set bar 1 at the first downbeat");
        ui.end_row();
    });
    ui.separator();
    let ready = d.analysis.as_ref().is_some_and(|a| a.detection.downbeats.len() >= 2);
    ui.horizontal(|ui| {
        if d.playing {
            if ui.button("■ Stop").clicked() {
                action = Some("stop");
            }
        } else if ui.add_enabled(ready, egui::Button::new("▶ Play")).on_hover_text("Loop the source with a click on the detected beats").clicked() {
            action = Some("play");
        }
        ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
            if ui.add_enabled(ready, egui::Button::new("Apply")).clicked() {
                action = Some("apply");
            }
            if ui.button("Cancel").clicked() {
                action = Some("cancel");
            }
        });
    });
    action
}

/// The beat preview strip: the waveform with a tick per beat (downbeats bold) and the playhead.
pub fn strip(ui: &mut egui::Ui, a: &Analysis, playhead: Option<soundcraft_time::Samples>) {
    let t = Tokens::current();
    let width = ui.available_width().max(200.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(width, STRIP_HEIGHT), egui::Sense::hover());
    let p = ui.painter_at(rect);
    p.rect_filled(rect, 2.0, t.playlist_bg);
    let r = a.source.range;
    let len = r.len().max(1) as f32;
    let x_of = |s: soundcraft_time::Samples| rect.left() + (s - r.start) as f32 / len * rect.width();
    let mid = rect.center().y;
    let half = rect.height() * 0.42;
    let peak = a.waveform.iter().map(|(lo, hi)| lo.abs().max(hi.abs())).fold(1e-6f32, f32::max);
    let cols = a.waveform.len().max(1) as f32;
    for (i, (lo, hi)) in a.waveform.iter().enumerate() {
        let x = rect.left() + (i as f32 + 0.5) / cols * rect.width();
        p.line_segment(
            [egui::pos2(x, mid - hi / peak * half), egui::pos2(x, mid - lo / peak * half)],
            egui::Stroke::new(1.0, t.text_dim.gamma_multiply(0.45)),
        );
    }
    for b in &a.detection.beats {
        let x = x_of(*b);
        p.line_segment([egui::pos2(x, rect.top() + 4.0), egui::pos2(x, rect.top() + 14.0)], egui::Stroke::new(1.0, t.text_dim));
    }
    for (i, b) in a.detection.downbeats.iter().enumerate() {
        let x = x_of(*b);
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(2.0, t.accent.gamma_multiply(0.85)));
        if i % 4 == 0 && rect.width() / a.detection.downbeats.len().max(1) as f32 * 4.0 > 22.0 {
            p.text(
                egui::pos2(x + 2.0, rect.bottom() - 2.0),
                egui::Align2::LEFT_BOTTOM,
                format!("{}", i + 1),
                egui::FontId::proportional(10.0),
                t.text_dim,
            );
        }
    }
    if let Some(ph) = playhead.filter(|ph| *ph >= r.start && *ph <= r.end) {
        let x = x_of(ph);
        p.line_segment([egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())], egui::Stroke::new(1.5, t.playhead));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Services;
    use soundcraft_model::{ChannelFormat, Clip, Session, TrackKind};

    /// An app whose session holds one click track at 120 BPM (accent every 4th click).
    fn app() -> SoundApp {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Click"));
        let pos: Vec<usize> = (0..40).map(|i| 9_600 + i * 24_000).collect();
        let len = pos.last().copied().unwrap_or(0) + 48_000;
        let audio = soundcraft_dsp::beat::click_track(len, &pos, 48_000.0, 4);
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![audio] };
        let src = soundcraft_engine::io::add_source(&mut s, "click", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        soundcraft_engine::edit::place_clip(&mut s, t, Clip::audio(cid, "click", src, 0, 0, len as i64));
        SoundApp::new(soundcraft_engine::Engine::new(s), None, Services::default())
    }

    fn wait(app: &mut SoundApp) {
        let ctx = egui::Context::default();
        for _ in 0..600 {
            step(app, &ctx);
            if app.tempo.dialog.as_ref().is_some_and(|d| d.running.is_none()) {
                return;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        panic!("the analysis did not finish");
    }

    #[test]
    fn menu_opens_the_dialog_analyses_and_apply_writes_the_map() {
        let mut app = app();
        crate::menus::invoke_menu(&mut app, "event.tempo_detect", "Event > Detect Tempo...");
        assert!(app.tempo.dialog.is_some(), "the menu opens the dialog");
        assert_eq!(app.engine.session().tempo, soundcraft_time::TempoMap::default(), "nothing written yet");
        wait(&mut app);
        let d = app.tempo.dialog.as_ref().unwrap();
        assert_eq!(d.source_label, "Whole session (1 track)");
        let a = d.analysis.as_ref().unwrap();
        assert!((a.detection.bpm - 120.0).abs() < 0.5, "{}", a.detection.bpm);
        assert_eq!(a.detection.downbeats.first().copied(), Some(9_600 + 3), "accented first click, refined to its attack");
        // Play: the source soloed with the click copy; the document keeps its tracks.
        play(&mut app).unwrap();
        assert!(app.audition_session.as_ref().is_some_and(|s| s.tracks.iter().any(|t| t.name == td::AUDITION_TRACK && t.mixer.solo)));
        assert_eq!(app.engine.session().tracks.len(), 1);
        apply(&mut app);
        assert!(app.tempo.dialog.is_none() && app.audition_session.is_none() && !app.is_playing());
        let s = app.engine.session();
        assert_eq!(s.tempo.origin(), 9_603, "bar 1 at the first downbeat");
        assert!((s.tempo.tempo_at_tick(0) - 120.0).abs() < 0.1);
        assert!(s.edit.rulers.iter().any(|r| r == "tempo"), "the Tempo ruler shows the result");
        app.run("edit.undo", json!({})).unwrap();
        assert_eq!(app.engine.session().tempo, soundcraft_time::TempoMap::default(), "one undo step");
    }

    #[test]
    fn straighten_from_the_dialog_conforms_every_listed_track_in_one_step() {
        let mut app = app();
        open(&mut app, &json!({"source": "session"})).unwrap();
        wait(&mut app);
        let d = app.tempo.dialog.as_mut().unwrap();
        assert_eq!(d.straighten_bpm, 120.0, "defaults to the detected average");
        assert_eq!(d.tracks.len(), 1);
        assert!(d.tracks[0].2);
        d.straighten = true;
        d.straighten_bpm = 115.0;
        assert!(drift_of(&app, app.tempo.dialog.as_ref().unwrap()).unwrap() > 0.03);
        apply(&mut app);
        assert!(app.tempo.dialog.is_none(), "{}", app.ui.status);
        let s = app.engine.session();
        assert_eq!(s.tempo.tempo_at_tick(0), 115.0);
        let clip = s.tracks[0].clips()[0].clone();
        assert!(!clip.warp.is_empty());
        assert!(clip.length > 40 * 24_000, "slower: longer");
        app.run("edit.undo", json!({})).unwrap();
        assert!(app.engine.session().tracks[0].clips()[0].warp.is_empty(), "one undo step");
    }

    #[test]
    fn range_change_reanalyses_and_close_stops_playback() {
        let mut app = app();
        open(&mut app, &json!({"source": "session"})).unwrap();
        wait(&mut app);
        app.tempo.dialog.as_mut().unwrap().range = 1; // 60–90
        start(&mut app);
        wait(&mut app);
        let bpm = app.tempo.dialog.as_ref().unwrap().analysis.as_ref().unwrap().detection.bpm;
        assert!((bpm - 60.0).abs() < 0.5, "half tempo inside 60–90: {bpm}");
        play(&mut app).unwrap();
        close(&mut app);
        assert!(app.audition_session.is_none() && !app.is_playing() && app.tempo.dialog.is_none());
    }
}

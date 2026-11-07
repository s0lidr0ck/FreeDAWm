//! Lift - Sampler's editor, in the Juke family's look (`docs/juke.md` §5): a walnut ground, tobacco
//! cards, cream knobs whose value rings light in amber, curves in dark inset wells with cream
//! handles, and one amber accent for values and active states.
//!
//! The page: a title bar (the Lift wordmark, the sample, Load Sample… / From Selection / Clear); the
//! main card (a header band with the root note and Use as Root, the waveform well with its start,
//! end and loop markers, the amp envelope row and the filter row, each with a hero knob, its curve
//! and its stage knobs); a side card (Pitch, LFO with a preview of its shape, Output, Playback);
//! and the keyboard with the key range and root. Every control runs `mix.instrument_param` or a
//! sampler command, so it undoes and scripts like any edit.

use crate::SoundApp;
use crate::sampler_window::{AUDIO_EXTS, MARKERS, SAMPLER, format_param, param, whole_number};
use crate::theme::{bold, mono, regular};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::ParamInfo;
use soundcraft_model::{Insert, SourceAudio, TrackId};

/// The editor's size: the window opens at this width.
pub const W: f32 = 1100.0;
const H: f32 = 860.0;

// The Juke family's palette.
const WALNUT: Color32 = Color32::from_rgb(0x2A, 0x24, 0x1F);
const TOBACCO: Color32 = Color32::from_rgb(0x4A, 0x40, 0x37);
const TOBACCO_HI: Color32 = Color32::from_rgb(0x55, 0x4A, 0x40);
const WELL: Color32 = Color32::from_rgb(0x1E, 0x1A, 0x16);
const WELL_GRID: Color32 = Color32::from_rgb(0x30, 0x2A, 0x24);
const AMBER: Color32 = Color32::from_rgb(0xF2, 0xB5, 0x44);
const CREAM: Color32 = Color32::from_rgb(0xEE, 0xE6, 0xD3);
const CREAM_SHADE: Color32 = Color32::from_rgb(0xC9, 0xBF, 0xA8);
const INK: Color32 = Color32::from_rgb(0x2A, 0x21, 0x18);
const TEXT: Color32 = Color32::from_rgb(0xF3, 0xED, 0xE4);
const DIM: Color32 = Color32::from_rgb(0xB5, 0xAA, 0x9B);
const PILL: Color32 = Color32::from_rgb(0x2F, 0x28, 0x22);
const LAMP: Color32 = Color32::from_rgb(0x7C, 0xC4, 0x6A);

fn info_of(id: &str) -> Option<&'static ParamInfo> {
    soundcraft_dsp::plugin_info(SAMPLER).and_then(|i| i.param(id))
}

/// Sets one parameter (merged into one undo step while dragging).
fn set(app: &mut SoundApp, tid: TrackId, id: &str, value: f32) {
    let v = if whole_number(SAMPLER, id) { value.round() } else { value };
    if let Err(e) = app.engine.execute_merged(
        "mix.instrument_param",
        &json!({"track": tid.0, "param": id, "value": v}),
        &format!("instrument_param:{}:{id}", tid.0),
    ) {
        app.ui.status = e.to_string();
    }
}

/// Parameters whose knob lights its ring from the centre.
fn bipolar(id: &str) -> bool {
    matches!(id, "transpose" | "fine_tune" | "pan" | "filter_env")
}

/// The whole editor.
pub fn editor(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert) {
    let (outer, _) = ui.allocate_exact_size(vec2(W - 16.0, H), Sense::hover());
    let p = ui.painter_at(outer);
    p.rect_filled(outer, CornerRadius::same(6), WALNUT);
    let x0 = outer.min.x + 12.0;
    let x1 = outer.max.x - 12.0;
    let title = Rect::from_min_max(pos2(x0, outer.min.y + 8.0), pos2(x1, outer.min.y + 50.0));
    title_bar(app, ui, &p, title, tid, ins);
    let side_w = 360.0;
    let top = title.max.y + 10.0;
    let bottom = outer.max.y - 84.0;
    let main = Rect::from_min_max(pos2(x0, top), pos2(x1 - side_w - 12.0, bottom));
    let side = Rect::from_min_max(pos2(x1 - side_w, top), pos2(x1, bottom));
    main_card(app, ui, &p, main, tid, ins);
    side_card(app, ui, &p, side, tid, ins);
    let keys = Rect::from_min_max(pos2(x0, bottom + 12.0), pos2(x1, outer.max.y - 10.0));
    keyboard(app, ui, &p, keys, tid, ins);
}

/// A rounded button drawn on the painter; returns whether it was clicked.
fn button(ui: &mut Ui, p: &Painter, r: Rect, text: &str, enabled: bool, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Response {
    let sense = if enabled { Sense::click() } else { Sense::hover() };
    let resp = ui.interact(r, ui.id().with(("lift-btn", id)), sense);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, enabled, text));
    let fill = if !enabled {
        PILL.gamma_multiply(0.6)
    } else if resp.hovered() {
        TOBACCO_HI
    } else {
        Color32::from_rgb(0x3A, 0x32, 0x2B)
    };
    p.rect_filled(r, CornerRadius::same(8), fill);
    p.rect_stroke(r, CornerRadius::same(8), Stroke::new(1.0, Color32::from_rgb(0x5A, 0x4F, 0x45)), StrokeKind::Inside);
    p.text(r.center(), Align2::CENTER_CENTER, text, regular(13.0), if enabled { TEXT } else { DIM });
    if resp.hovered() && enabled {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// A pill (amber when on); returns the response.
fn pill(ui: &mut Ui, p: &Painter, r: Rect, text: &str, on: bool, id: impl std::hash::Hash + std::fmt::Debug) -> egui::Response {
    let resp = ui.interact(r, ui.id().with(("lift-pill", id)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, on, text));
    let fill = if on {
        AMBER
    } else if resp.hovered() {
        TOBACCO_HI
    } else {
        PILL
    };
    p.rect_filled(r, CornerRadius::same((r.height() * 0.5) as u8), fill);
    p.text(r.center(), Align2::CENTER_CENTER, text, if on { bold(12.5) } else { regular(12.5) }, if on { INK } else { TEXT });
    if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::PointingHand);
    }
    resp
}

/// A row of pills for a choice parameter, `labels` shortening the choices.
fn choice_pills(app: &mut SoundApp, ui: &mut Ui, p: &Painter, at: Pos2, tid: TrackId, ins: &Insert, id: &str, labels: &[&str], w: f32) {
    let now = param(ins, id).round().max(0.0) as usize;
    for (i, l) in labels.iter().enumerate() {
        let r = Rect::from_min_size(pos2(at.x + i as f32 * (w + 6.0), at.y), vec2(w, 26.0));
        if pill(ui, p, r, l, i == now, (id, i)).clicked() && i != now {
            set(app, tid, id, i as f32);
        }
    }
}

/// The top bar: the wordmark, the sample, and its buttons.
fn title_bar(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    let g = p.layout_no_wrap("Lift".into(), bold(30.0), AMBER);
    let w = g.size().x;
    p.galley(pos2(r.min.x, r.center().y - g.size().y * 0.5), g, AMBER);
    p.text(pos2(r.min.x + w + 10.0, r.center().y + 4.0), Align2::LEFT_CENTER, "Sampler", regular(14.0), DIM);
    // The sample, in the centre pill.
    let s = app.engine.session();
    let source = ins.sample.and_then(|id| s.source(id)).cloned();
    let audio = ins.sample.and_then(|id| s.pool.get(id)).cloned();
    let (name, facts) = match (&source, &audio) {
        (Some(src), Some(a)) => {
            let rate = a.sample_rate().max(1);
            let secs = a.frames() as f64 / f64::from(rate);
            let width = match a.num_channels() {
                1 => "mono".to_string(),
                2 => "stereo".to_string(),
                n => format!("{n} ch"),
            };
            (src.name.clone(), format!("{secs:.2} s · {width} · {:.1} kHz", f64::from(rate) / 1000.0))
        }
        (Some(src), None) => (src.name.clone(), "missing".to_string()),
        _ => ("No sample".to_string(), "drop audio here".to_string()),
    };
    let mid = Rect::from_center_size(pos2(r.center().x - 60.0, r.center().y), vec2(300.0, 44.0));
    p.rect_filled(mid, CornerRadius::same(22), Color32::from_rgb(0x22, 0x1D, 0x19));
    p.rect_stroke(mid, CornerRadius::same(22), Stroke::new(1.0, Color32::from_rgb(0x48, 0x3F, 0x37)), StrokeKind::Inside);
    p.text(pos2(mid.center().x, mid.min.y + 15.0), Align2::CENTER_CENTER, name, bold(15.0), TEXT);
    p.text(pos2(mid.center().x, mid.min.y + 32.0), Align2::CENTER_CENTER, facts, regular(11.5), DIM);
    // Buttons, right to left.
    let selection = !s.edit.selection.is_empty();
    let mut x = r.max.x;
    let mut next = |w: f32| {
        let b = Rect::from_min_max(pos2(x - w, r.center().y - 17.0), pos2(x, r.center().y + 17.0));
        x -= w + 8.0;
        b
    };
    let clear_r = next(70.0);
    if ins.sample.is_some() && button(ui, p, clear_r, "Clear", true, "clear").clicked() {
        let _ = app.run("mix.sampler_clear", json!({"track": tid.0}));
    }
    let from_r = next(130.0);
    let from = button(ui, p, from_r, "From Selection", selection, "from")
        .on_hover_text("Load the selected range of an audio track, as it plays")
        .on_disabled_hover_text("Select a range on an audio track first");
    if from.clicked() {
        match app.run("mix.sampler_from_selection", json!({"track": tid.0})) {
            Ok(v) => {
                let n = v.get("name").and_then(|n| n.as_str()).unwrap_or("The selection").to_string();
                app.ui.status = format!("{n} loaded into the sampler");
            }
            Err(e) => app.ui.status = e,
        }
    }
    let can_pick = app.services.pick_open.is_some();
    let load_r = next(130.0);
    let load = button(ui, p, load_r, "Load Sample…", can_pick, "load").on_disabled_hover_text("Drop an audio file on this window");
    if load.clicked()
        && let Some(pick) = &app.services.pick_open
        && let Some(path) = pick("mix.sampler_load", AUDIO_EXTS)
    {
        let _ = app.run("mix.sampler_load", json!({"track": tid.0, "path": path}));
    }
}

/// A card: a tobacco panel with rounded corners.
fn card(p: &Painter, r: Rect) {
    p.rect_filled(r.translate(vec2(0.0, 2.0)), CornerRadius::same(10), Color32::from_black_alpha(70));
    p.rect_filled(r, CornerRadius::same(10), TOBACCO);
}

/// A dark inset well (for curves and the waveform).
fn well(p: &Painter, r: Rect) {
    p.rect_filled(r, CornerRadius::same(6), WELL);
    for i in 1..4 {
        let x = r.min.x + r.width() * i as f32 / 4.0;
        p.line_segment([pos2(x, r.min.y + 2.0), pos2(x, r.max.y - 2.0)], Stroke::new(1.0, WELL_GRID));
    }
    p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
}

/// The main card: header band, waveform, amp and filter rows.
fn main_card(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    card(p, r);
    // Header band in amber.
    let band = Rect::from_min_size(r.min, vec2(r.width(), 46.0));
    p.rect_filled(band, CornerRadius { nw: 10, ne: 10, sw: 0, se: 0 }, AMBER);
    let current = param(ins, "root_note").round().clamp(0.0, 127.0) as u8;
    p.text(pos2(band.min.x + 16.0, band.center().y), Align2::LEFT_CENTER, "Sample", bold(19.0), INK);
    p.text(
        pos2(band.min.x + 100.0, band.center().y + 1.0),
        Align2::LEFT_CENTER,
        format!("Root {}", soundcraft_midi::note_name(current)),
        regular(14.0),
        INK.gamma_multiply(0.8),
    );
    root_detection(app, ui, p, band, tid, ins, current);
    // Waveform well.
    let wave = Rect::from_min_max(pos2(r.min.x + 14.0, band.max.y + 14.0), pos2(r.max.x - 14.0, band.max.y + 14.0 + 150.0));
    waveform(app, ui, p, wave, tid, ins);
    let tools = Rect::from_min_max(pos2(wave.min.x, wave.max.y + 8.0), pos2(wave.max.x, wave.max.y + 44.0));
    sample_tools(app, ui, p, tools, tid, ins);
    // The two envelope rows.
    let rows_top = tools.max.y + 8.0;
    let row_h = (r.max.y - rows_top) / 2.0;
    let amp = Rect::from_min_max(pos2(r.min.x, rows_top), pos2(r.max.x, rows_top + row_h));
    let filt = Rect::from_min_max(pos2(r.min.x, amp.max.y), pos2(r.max.x, r.max.y));
    p.line_segment([pos2(r.min.x + 1.0, amp.min.y), pos2(r.max.x - 1.0, amp.min.y)], Stroke::new(1.0, Color32::from_black_alpha(90)));
    p.line_segment([pos2(r.min.x + 1.0, filt.min.y), pos2(r.max.x - 1.0, filt.min.y)], Stroke::new(1.0, Color32::from_black_alpha(90)));
    amp_row(app, ui, p, amp, tid, ins);
    filter_row(app, ui, p, filt, tid, ins);
}

/// Under the waveform: Trim, Normalize and Reverse (each a new sample, one undo step), and the
/// loop's crossfade.
fn sample_tools(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    let has = ins.sample.is_some();
    let ops = [
        ("Trim", "trim", "Cut the silence off both ends"),
        ("Normalize", "normalize", "Bring the peak to -0.3 dBFS"),
        ("Reverse", "reverse", "Play the sample backwards"),
    ];
    for (i, (label, op, help)) in ops.iter().enumerate() {
        let b = Rect::from_min_size(pos2(r.min.x + i as f32 * 92.0, r.min.y + 4.0), vec2(86.0, 28.0));
        let resp = button(ui, p, b, label, has, ("edit", *op)).on_hover_text(*help);
        if resp.clicked() {
            match app.run("mix.sampler_edit", json!({"track": tid.0, "op": op})) {
                Ok(_) => app.ui.status = format!("{label}: done"),
                Err(e) => app.ui.status = e,
            }
        }
    }
    if !slicing(ins) {
        small_knob(app, ui, p, pos2(r.min.x + 292.0, r.min.y - 4.0), tid, ins, "loop_xfade", "Loop Crossfade");
        let b = Rect::from_min_size(pos2(r.max.x - 110.0, r.min.y + 4.0), vec2(110.0, 28.0));
        let resp = button(ui, p, b, "Slice…", has, "slice-start").on_hover_text("Cut the sample at its hits and play a slice on each key");
        if resp.clicked() {
            run_status(app, "mix.sampler_slice", json!({"track": tid.0, "by": "hits"}));
        }
        return;
    }
    // Slice mode: re-slice by hits, beats or equal parts, write the slices into a MIDI clip, or
    // play the picked slice up and down the keyboard.
    let x = r.min.x + 282.0;
    let tips = ["Slice at each hit", "Slice on the beat grid (the sample's tempo)", "Slice into 16 equal parts"];
    for (i, ((label, by), tip)) in [("Hits", "hits"), ("Beats", "beats"), ("Equal", "equal")].iter().zip(tips).enumerate() {
        let pr = Rect::from_min_size(pos2(x + i as f32 * 58.0, r.min.y + 5.0), vec2(54.0, 26.0));
        if pill(ui, p, pr, label, false, ("slice-by", *by)).on_hover_text(tip).clicked() {
            run_status(app, "mix.sampler_slice", json!({"track": tid.0, "by": by}));
        }
    }
    let midi = Rect::from_min_size(pos2(r.max.x - 192.0, r.min.y + 4.0), vec2(78.0, 28.0));
    if button(ui, p, midi, "To MIDI", true, "slice-midi").on_hover_text("A MIDI clip that plays the slices in order, to rearrange").clicked() {
        run_status(app, "mix.sampler_slice_midi", json!({"track": tid.0}));
    }
    let picked = picked_slice(ui, tid);
    let keys = Rect::from_min_size(pos2(r.max.x - 106.0, r.min.y + 4.0), vec2(106.0, 28.0));
    let resp = button(ui, p, keys, "Slice → Keys", picked.is_some(), "slice-keys")
        .on_hover_text("Play the picked slice up and down the keyboard")
        .on_disabled_hover_text("Click a slice in the waveform first");
    if resp.clicked()
        && let Some(k) = picked
    {
        run_status(app, "mix.sampler_slice_play", json!({"track": tid.0, "slice": k}));
    }
}

/// Runs a command, putting its error (or nothing) in the status line.
fn run_status(app: &mut SoundApp, id: &str, params: serde_json::Value) {
    if let Err(e) = app.run(id, params) {
        app.ui.status = e;
    }
}

/// Whether the Sampler is in Slice mode.
fn slicing(ins: &Insert) -> bool {
    param(ins, "play_mode").round() as i32 == 2
}

/// The slices in use: (number from 1 in order, parameter id, start percent), in order.
fn slices(ins: &Insert) -> Vec<(usize, String, f32)> {
    let n = (param(ins, "slice_count").round().max(0.0) as usize).min(soundcraft_dsp::slices::MAX_SLICES);
    let mut v: Vec<(String, f32)> = (1..=n)
        .map(|k| format!("slice_{k}"))
        .map(|id| {
            let at = param(ins, &id);
            (id, at)
        })
        .collect();
    v.sort_by(|a, b| a.1.total_cmp(&b.1));
    v.into_iter().enumerate().map(|(i, (id, at))| (i + 1, id, at)).collect()
}

/// The slice picked in the waveform (numbered from 1), if any.
fn picked_slice(ui: &Ui, tid: TrackId) -> Option<usize> {
    ui.ctx().data(|d| d.get_temp::<usize>(egui::Id::new(("lift-slice-pick", tid.0))))
}

/// The detected root note and Use as Root, at the band's right.
fn root_detection(app: &mut SoundApp, ui: &mut Ui, p: &Painter, band: Rect, tid: TrackId, ins: &Insert, current: u8) {
    let audio = ins.sample.and_then(|id| app.engine.session().pool.get(id)).cloned();
    let Some((src, audio)) = ins.sample.zip(audio) else { return };
    let key = ui.id().with(("lift-root", tid.0, src.0));
    let root = match ui.ctx().data(|d| d.get_temp::<Option<f32>>(key)) {
        Some(r) => r,
        None => {
            let r = detect_root(&audio);
            ui.ctx().data_mut(|d| d.insert_temp(key, r));
            r
        }
    };
    let right = band.max.x - 14.0;
    match root {
        Some(m) => {
            let n = m.round().clamp(0.0, 127.0) as u8;
            let cents = ((m - m.round()) * 100.0).round() as i32;
            let off = if cents == 0 { String::new() } else { format!(" {cents:+}¢") };
            let text = format!("detected {}{off}", soundcraft_midi::note_name(n));
            if n != current {
                let b = Rect::from_min_max(pos2(right - 112.0, band.center().y - 15.0), pos2(right, band.center().y + 15.0));
                let resp = ui.interact(b, ui.id().with(("lift-use-root", tid.0)), Sense::click());
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Use as Root"));
                p.rect_filled(b, CornerRadius::same(15), if resp.hovered() { Color32::from_rgb(0x12, 0x0E, 0x0B) } else { INK });
                p.text(b.center(), Align2::CENTER_CENTER, "Use as Root", bold(12.5), TEXT);
                let resp = resp.on_hover_text("Play the sample in tune: set its root note to the detected pitch");
                if resp.clicked() {
                    set(app, tid, "root_note", f32::from(n));
                }
                p.text(pos2(b.min.x - 12.0, band.center().y), Align2::RIGHT_CENTER, text, regular(13.5), INK.gamma_multiply(0.8));
            } else {
                p.text(pos2(right, band.center().y), Align2::RIGHT_CENTER, format!("{text} · in tune"), regular(13.5), INK.gamma_multiply(0.8));
            }
        }
        None => {
            p.text(pos2(right, band.center().y), Align2::RIGHT_CENTER, "no clear pitch", regular(13.5), INK.gamma_multiply(0.8));
        }
    }
}

/// The pitch a sample sounds at, from its first seconds (both sides mixed).
fn detect_root(audio: &SourceAudio) -> Option<f32> {
    let rate = audio.sample_rate();
    let n = audio.frames().min(rate as usize * 4);
    let w = audio.window(0, n);
    let mono: Vec<f32> = match (w.first(), w.get(1)) {
        (Some(a), Some(b)) => a.iter().zip(b).map(|(x, y)| 0.5 * (x + y)).collect(),
        (Some(a), None) => a.clone(),
        _ => return None,
    };
    soundcraft_dsp::pitch_detect::sample_root(&mono, rate as f32)
}

/// The waveform well: amber where a note plays, dim outside; the loop shaded; S and E flags on
/// top and the loop's brackets at the bottom, all of which drag.
fn waveform(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    p.rect_filled(r, CornerRadius::same(6), WELL);
    let resp = ui.interact(r, ui.id().with(("lift-wave", tid.0)), Sense::click_and_drag());
    let audio = ins.sample.and_then(|id| app.engine.session().pool.get(id)).cloned();
    let Some(audio) = audio else {
        p.text(
            r.center(),
            Align2::CENTER_CENTER,
            "Drop an audio file here, or select a range on an audio track and press From Selection",
            regular(13.0),
            DIM,
        );
        p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
        return;
    };
    let frames = audio.frames().max(1);
    let x_of = |pct: f32| r.min.x + r.width() * (pct / 100.0).clamp(0.0, 1.0);
    let looping = param(ins, "loop_mode") >= 0.5;
    let (ls, le) = {
        let (a, b) = (param(ins, "loop_start"), param(ins, "loop_end"));
        (a.min(b), a.max(b))
    };
    let (st, en) = {
        let (a, b) = (param(ins, "start"), param(ins, "end"));
        (a.min(b), a.max(b))
    };
    if looping {
        p.rect_filled(Rect::from_x_y_ranges(x_of(ls)..=x_of(le), r.y_range()), 0.0, AMBER.gamma_multiply(0.12));
    }
    let lanes = audio.peaks.len().clamp(1, 2);
    let lane_h = r.height() / lanes as f32;
    let cols = r.width().max(1.0) as usize;
    let played = |x: f32| x >= x_of(st) && (looping || x <= x_of(en));
    for (c, peaks) in audio.peaks.iter().take(lanes).enumerate() {
        let mid = r.min.y + lane_h * (c as f32 + 0.5);
        let half = lane_h * 0.42;
        let samples = audio.channel(c).unwrap_or_default();
        for i in 0..cols {
            let a = frames.saturating_mul(i) / cols;
            let b = (frames.saturating_mul(i + 1) / cols).max(a + 1);
            let (lo, hi) = peaks.minmax(samples, a, b);
            let x = r.min.x + i as f32 + 0.5;
            let col = if played(x) { AMBER } else { DIM.gamma_multiply(0.35) };
            p.line_segment([pos2(x, mid - hi.clamp(-1.0, 1.0) * half), pos2(x, mid - lo.clamp(-1.0, 1.0) * half)], Stroke::new(1.0, col));
        }
    }
    p.rect_stroke(r, CornerRadius::same(6), Stroke::new(1.0, Color32::from_black_alpha(120)), StrokeKind::Inside);
    if slicing(ins) {
        slice_lines(app, ui, p, r, &resp, tid, ins);
        return;
    }
    // Markers: cream lines with tabs.
    let shown: Vec<(&str, f32, &str, bool)> = MARKERS
        .iter()
        .filter(|(_, needs_loop)| !needs_loop || looping)
        .map(|(id, _)| {
            let (tag, bottom) = match *id {
                "start" => ("S", false),
                "end" => ("E", false),
                "loop_start" => ("[", true),
                _ => ("]", true),
            };
            (*id, param(ins, id), tag, bottom)
        })
        .collect();
    for (_, pct, tag, bottom) in &shown {
        let x = x_of(*pct);
        let col = if *bottom { AMBER } else { CREAM };
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, col));
        let tab = if *bottom {
            Rect::from_min_size(pos2(x - 7.0, r.max.y - 15.0), vec2(14.0, 15.0))
        } else {
            Rect::from_min_size(pos2(x - 7.0, r.min.y), vec2(14.0, 15.0))
        };
        p.rect_filled(tab, CornerRadius::same(3), col);
        p.text(tab.center(), Align2::CENTER_CENTER, *tag, bold(10.0), INK);
    }
    let grab_id = ui.id().with(("lift-marker", tid.0));
    let nearest = |pos: Pos2| {
        let lower = pos.y > r.center().y;
        shown
            .iter()
            .map(|(id, pct, _, bottom)| (*id, (x_of(*pct) - pos.x).abs() + if *bottom == lower { 0.0 } else { 6.0 }))
            .filter(|(_, d)| *d <= 10.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    };
    if let Some(h) = resp.hover_pos()
        && nearest(h).is_some()
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.drag_started()
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let grabbed = nearest(pos).map(str::to_string);
        ui.ctx().data_mut(|d| d.insert_temp(grab_id, grabbed));
    }
    let grabbed = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab_id)).flatten();
    if resp.dragged()
        && let (Some(id), Some(pos)) = (grabbed.as_deref(), resp.interact_pointer_pos())
    {
        set(app, tid, id, ((pos.x - r.min.x) / r.width().max(1.0) * 100.0).clamp(0.0, 100.0));
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab_id));
    }
}

/// Slice mode's waveform overlay: a numbered line at each slice start (drag to move it), the
/// picked slice shaded (click to pick).
fn slice_lines(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, resp: &egui::Response, tid: TrackId, ins: &Insert) {
    let x_of = |pct: f32| r.min.x + r.width() * (pct / 100.0).clamp(0.0, 1.0);
    let list = slices(ins);
    let picked = picked_slice(ui, tid);
    for (i, (k, _, at)) in list.iter().enumerate() {
        let next = list.get(i + 1).map_or(100.0, |n| n.2);
        if picked == Some(*k) {
            p.rect_filled(Rect::from_x_y_ranges(x_of(*at)..=x_of(next), r.y_range()), 0.0, CREAM.gamma_multiply(0.12));
        }
        let x = x_of(*at);
        p.line_segment([pos2(x, r.min.y), pos2(x, r.max.y)], Stroke::new(1.5, CREAM));
        let label = k.to_string();
        let tab = Rect::from_min_size(pos2(x, r.min.y), vec2(8.0 + 7.0 * label.len() as f32, 15.0));
        p.rect_filled(tab, CornerRadius::same(3), if picked == Some(*k) { AMBER } else { CREAM });
        p.text(tab.center(), Align2::CENTER_CENTER, label, bold(10.0), INK);
    }
    if list.is_empty() {
        p.text(pos2(r.center().x, r.min.y + 14.0), Align2::CENTER_CENTER, "No slices yet: Slice by Hits, Beats or Equal", regular(12.0), DIM);
    }
    let grab_id = ui.id().with(("lift-slice-grab", tid.0));
    let nearest = |x: f32| {
        list.iter()
            .map(|(_, id, at)| (id.clone(), (x_of(*at) - x).abs()))
            .filter(|(_, d)| *d <= 6.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(id, _)| id)
    };
    if let Some(h) = resp.hover_pos()
        && nearest(h.x).is_some()
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.clicked()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let pct = (at.x - r.min.x) / r.width().max(1.0) * 100.0;
        let k = list.iter().rev().find(|(_, _, s)| *s <= pct).map(|(k, _, _)| *k);
        let key = egui::Id::new(("lift-slice-pick", tid.0));
        ui.ctx().data_mut(|d| {
            if let Some(k) = k {
                d.insert_temp(key, k);
            } else {
                d.remove::<usize>(key);
            }
        });
    }
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let g = nearest(at.x);
        ui.ctx().data_mut(|d| d.insert_temp(grab_id, g));
    }
    let grabbed = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab_id)).flatten();
    if resp.dragged()
        && let (Some(id), Some(at)) = (grabbed.as_deref(), resp.interact_pointer_pos())
    {
        set(app, tid, id, ((at.x - r.min.x) / r.width().max(1.0) * 100.0).clamp(0.0, 100.0));
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab_id));
    }
}

/// Paints a cream knob at `c` of radius `rad` showing `norm` (0..1); `hero` adds the segmented
/// value ring, lit in amber from the start (or from the centre when `bipolar`).
fn paint_knob(p: &Painter, c: Pos2, rad: f32, norm: f32, hero: bool, bipolar: bool, hot: bool) {
    let norm = if norm.is_finite() { norm.clamp(0.0, 1.0) } else { 0.0 };
    let angle = |t: f32| (-135.0 + 270.0 * t).to_radians();
    let dir = |a: f32| vec2(a.sin(), -a.cos());
    // Ticks around the cap.
    let n = if hero { 31 } else { 11 };
    let (lo, hi) = if bipolar { (norm.min(0.5), norm.max(0.5)) } else { (0.0, norm) };
    for i in 0..n {
        let t = i as f32 / (n - 1) as f32;
        let lit = hero && t >= lo - 1e-3 && t <= hi + 1e-3;
        let (r0, r1, w) = if hero { (rad + 4.0, rad + 10.0, 2.0) } else { (rad + 3.0, rad + 6.0, 1.2) };
        let col = if lit { AMBER } else { Color32::from_rgb(0x8A, 0x7E, 0x70) };
        let d = dir(angle(t));
        p.line_segment([c + d * r0, c + d * r1], Stroke::new(w, col));
    }
    // The cap: a soft shadow, a brushed cream face, a highlight and a rim.
    p.circle_filled(c + vec2(0.0, 2.5), rad + 1.0, Color32::from_black_alpha(90));
    p.circle_filled(c, rad, CREAM_SHADE);
    p.circle_filled(c + vec2(-rad * 0.06, -rad * 0.08), rad * 0.92, if hot { Color32::from_rgb(0xF6, 0xEF, 0xDF) } else { CREAM });
    for k in 1..5 {
        p.circle_stroke(c, rad * (0.2 * k as f32), Stroke::new(0.6, CREAM_SHADE.gamma_multiply(0.45)));
    }
    p.circle_filled(c + vec2(-rad * 0.3, -rad * 0.35), rad * 0.22, Color32::from_white_alpha(70));
    p.circle_stroke(c, rad, Stroke::new(1.0, Color32::from_rgb(0x8F, 0x84, 0x70)));
    let d = dir(angle(norm));
    p.line_segment([c + d * rad * 0.2, c + d * rad * 0.82], Stroke::new(if hero { 3.5 } else { 2.2 }, INK));
}

/// A knob's interaction over `r`: drag up and down (Shift for fine), wheel, double-click resets.
fn knob_input(app: &mut SoundApp, ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, id: &str, name: &str) -> bool {
    let Some(info) = info_of(id) else { return false };
    let v = param(ins, id);
    let resp = ui.interact(r, ui.id().with(("lift-knob", tid.0, id)), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(v), name));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeVertical);
    }
    let fine = ui.input(|i| i.modifiers.shift);
    if resp.double_clicked() {
        set(app, tid, id, info.default);
    } else if resp.dragged() {
        let dy = resp.drag_delta().y;
        if dy != 0.0 {
            set(app, tid, id, crate::widgets::knob_drag(info, v, dy, fine));
        }
    } else if resp.hovered() {
        let scroll = ui.input(|i| i.smooth_scroll_delta.y);
        if scroll.abs() > 0.5 {
            set(app, tid, id, crate::widgets::knob_scroll(info, v, scroll / 50.0));
        }
    }
    resp.hovered() || resp.dragged()
}

fn value_text(ins: &Insert, id: &str) -> String {
    let Some(info) = info_of(id) else { return String::new() };
    let v = param(ins, id);
    // Long times read as whole milliseconds, or seconds, so the knobs' values stay short.
    if info.unit == soundcraft_dsp::Unit::Ms && v >= 1000.0 {
        return format!("{:.2} s", v / 1000.0);
    }
    if info.unit == soundcraft_dsp::Unit::Ms && v >= 100.0 {
        return format!("{v:.0} ms");
    }
    if id == "sample_bpm" {
        return format!("{v:.1} bpm");
    }
    if id == "lfo_pitch" || id == "fine_tune" && v.fract() != 0.0 {
        return format!("{v:.1} {}", if id == "lfo_pitch" { "st" } else { "ct" });
    }
    format_param(SAMPLER, info, v)
}

/// A hero knob centred in `r`: the knob, its value in bold, its name.
fn hero_knob(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert, id: &str, name: &str) {
    let Some(info) = info_of(id) else { return };
    let rad = 30.0;
    let c = pos2(r.center().x, r.min.y + rad + 14.0);
    let hot = knob_input(app, ui, Rect::from_center_size(c, vec2(rad * 2.0 + 20.0, rad * 2.0 + 20.0)), tid, ins, id, name);
    paint_knob(p, c, rad, info.to_normalized(param(ins, id)), true, bipolar(id), hot);
    p.text(pos2(c.x, c.y + rad + 22.0), Align2::CENTER_CENTER, value_text(ins, id), bold(16.0), TEXT);
    p.text(pos2(c.x, c.y + rad + 40.0), Align2::CENTER_CENTER, name, regular(12.5), DIM);
}

/// A small knob with its name and value to its right, in a `w`-wide cell at `at`.
fn small_knob(app: &mut SoundApp, ui: &mut Ui, p: &Painter, at: Pos2, tid: TrackId, ins: &Insert, id: &str, name: &str) {
    let Some(info) = info_of(id) else { return };
    let rad = 15.0;
    let c = pos2(at.x + rad + 6.0, at.y + 22.0);
    let hot = knob_input(app, ui, Rect::from_center_size(c, vec2(rad * 2.0 + 10.0, rad * 2.0 + 10.0)), tid, ins, id, name);
    paint_knob(p, c, rad, info.to_normalized(param(ins, id)), false, bipolar(id), hot);
    p.text(pos2(c.x + rad + 10.0, c.y - 8.0), Align2::LEFT_CENTER, name, regular(12.0), DIM);
    p.text(pos2(c.x + rad + 10.0, c.y + 8.0), Align2::LEFT_CENTER, value_text(ins, id), bold(13.5), TEXT);
}

/// A row's title: an optional lamp, a bold name and a dim note.
fn row_title(p: &Painter, at: Pos2, lamp: Option<bool>, name: &str, note: &str) -> f32 {
    let mut x = at.x;
    if let Some(on) = lamp {
        let r = Rect::from_min_size(pos2(x, at.y - 11.0), vec2(40.0, 22.0));
        p.rect_filled(r, CornerRadius::same(11), if on { LAMP } else { PILL });
        p.text(r.center(), Align2::CENTER_CENTER, if on { "ON" } else { "OFF" }, bold(11.0), if on { INK } else { DIM });
        x = r.max.x + 10.0;
    }
    let g = p.layout_no_wrap(name.into(), bold(15.5), TEXT);
    let w = g.size().x;
    p.galley(pos2(x, at.y - g.size().y * 0.5), g, TEXT);
    p.text(pos2(x + w + 10.0, at.y + 1.0), Align2::LEFT_CENTER, note, regular(12.5), DIM);
    x + w
}

/// Amp envelope: Level as the hero knob, the ADSR shape (its points drag), and the stage knobs.
fn amp_row(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    hero_knob(app, ui, p, Rect::from_min_size(pos2(r.min.x + 10.0, r.min.y + 12.0), vec2(110.0, 120.0)), tid, ins, "level", "Level");
    let x = r.min.x + 134.0;
    row_title(p, pos2(x, r.min.y + 20.0), None, "Amp envelope", "how each note rises and dies away");
    let curve = Rect::from_min_max(pos2(x, r.min.y + 38.0), pos2(r.max.x - 16.0, r.max.y - 58.0));
    adsr(app, ui, p, curve, tid, ins);
    let knobs = [("attack", "Attack"), ("decay", "Decay"), ("sustain", "Sustain"), ("release", "Release"), ("velocity", "Velocity")];
    let cell = (r.max.x - 16.0 - x) / knobs.len() as f32;
    for (i, (id, name)) in knobs.iter().enumerate() {
        small_knob(app, ui, p, pos2(x + i as f32 * cell, r.max.y - 52.0), tid, ins, id, name);
    }
}

/// The ADSR curve in its well; drag the attack peak, the decay corner (sideways for decay, up and
/// down for sustain) and the release end.
fn adsr(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    well(p, r);
    let resp = ui.interact(r, ui.id().with(("lift-adsr", tid.0)), Sense::click_and_drag());
    let norm = |id: &str| info_of(id).map_or(0.0, |i| i.to_normalized(param(ins, id)));
    let seg = r.width() * 0.26;
    let (top, bottom) = (r.min.y + 10.0, r.max.y - 6.0);
    let sus = (param(ins, "sustain") / 100.0).clamp(0.0, 1.0);
    let x0 = r.min.x + 6.0;
    let xa = x0 + seg * norm("attack");
    let xd = xa + seg * norm("decay");
    let xs = xd + r.width() * 0.14;
    let xr = xs + seg * norm("release");
    let ys = bottom - (bottom - top) * sus;
    let pts = [pos2(x0, bottom), pos2(xa, top), pos2(xd, ys), pos2(xs, ys), pos2(xr, bottom)];
    curve_fill(p, &pts, bottom);
    for i in [1usize, 2, 4] {
        if let Some(q) = pts.get(i) {
            p.circle_filled(*q, 6.0, CREAM);
            p.circle_stroke(*q, 6.0, Stroke::new(1.0, CREAM_SHADE));
        }
    }
    let grab = ui.id().with(("lift-adsr-grab", tid.0));
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let near = [1usize, 2, 4]
            .into_iter()
            .filter_map(|i| pts.get(i).map(|q| (i, q.distance(at))))
            .filter(|(_, d)| *d < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        ui.ctx().data_mut(|d| d.insert_temp(grab, near));
    }
    let held = ui.ctx().data(|d| d.get_temp::<Option<usize>>(grab)).flatten();
    if resp.dragged()
        && let Some(i) = held
    {
        let d = resp.drag_delta();
        let nudge = |app: &mut SoundApp, id: &str, by: f32| {
            if let Some(info) = info_of(id) {
                set(app, tid, id, info.from_normalized(info.to_normalized(param(ins, id)) + by));
            }
        };
        match i {
            1 => nudge(app, "attack", d.x / seg),
            2 => {
                nudge(app, "decay", d.x / seg);
                set(app, tid, "sustain", (sus - d.y / (bottom - top)).clamp(0.0, 1.0) * 100.0);
            }
            _ => nudge(app, "release", d.x / seg),
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<usize>>(grab));
    }
}

/// An amber curve through `pts` with a soft fill down to `floor`.
fn curve_fill(p: &Painter, pts: &[Pos2], floor: f32) {
    // Fill as vertical strips between consecutive points (the shapes are not convex).
    for w in pts.windows(2) {
        if let [a, b] = w {
            let quad = vec![*a, *b, pos2(b.x, floor), pos2(a.x, floor)];
            p.add(Shape::convex_polygon(quad, AMBER.gamma_multiply(0.14), Stroke::NONE));
        }
    }
    p.add(Shape::line(pts.to_vec(), Stroke::new(2.5, AMBER)));
}

/// Filter: Cutoff as the hero knob, an ON lamp and the type pills, the envelope's shape (its points
/// drag), and Resonance with the envelope's Amount, Attack and Decay.
fn filter_row(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    hero_knob(app, ui, p, Rect::from_min_size(pos2(r.min.x + 10.0, r.min.y + 12.0), vec2(110.0, 120.0)), tid, ins, "cutoff", "Cutoff");
    let x = r.min.x + 134.0;
    let kind = param(ins, "filter_type").round() as i32;
    let on = kind != 0;
    let title_y = r.min.y + 20.0;
    let lamp = Rect::from_min_size(pos2(x, title_y - 11.0), vec2(40.0, 22.0));
    let resp = ui.interact(lamp, ui.id().with(("lift-filter-on", tid.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, "Filter on"));
    if resp.clicked() {
        // Off goes back to the type it had, else low-pass.
        let last = ui.ctx().data(|d| d.get_temp::<i32>(ui.id().with(("lift-filter-last", tid.0)))).unwrap_or(1);
        if on {
            ui.ctx().data_mut(|d| d.insert_temp(ui.id().with(("lift-filter-last", tid.0)), kind));
        }
        set(app, tid, "filter_type", if on { 0.0 } else { last.clamp(1, 3) as f32 });
    }
    let end = row_title(p, pos2(x, title_y), Some(on), "Filter", "");
    let types = [("Low-pass", 1), ("High-pass", 2), ("Band-pass", 3)];
    for (i, (label, k)) in types.iter().enumerate() {
        let pr = Rect::from_min_size(pos2(end + 16.0 + i as f32 * 92.0, title_y - 13.0), vec2(86.0, 26.0));
        if pill(ui, p, pr, label, kind == *k, ("ftype", tid.0, *k)).clicked() {
            set(app, tid, "filter_type", *k as f32);
        }
    }
    let curve = Rect::from_min_max(pos2(x, r.min.y + 38.0), pos2(r.max.x - 16.0, r.max.y - 58.0));
    filter_env(app, ui, p, curve, tid, ins, on);
    let knobs = [("resonance", "Resonance"), ("filter_env", "Amount"), ("filter_attack", "Attack"), ("filter_decay", "Decay")];
    let cell = (r.max.x - 16.0 - x) / knobs.len() as f32;
    for (i, (id, name)) in knobs.iter().enumerate() {
        small_knob(app, ui, p, pos2(x + i as f32 * cell, r.max.y - 52.0), tid, ins, id, name);
    }
}

/// The filter envelope's shape: up by its amount over the attack, back over the decay (down
/// first when the amount is negative). The peak drags sideways for the attack and up and down for
/// the amount; the end drags sideways for the decay.
fn filter_env(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert, on: bool) {
    well(p, r);
    let resp = ui.interact(r, ui.id().with(("lift-fenv", tid.0)), Sense::click_and_drag());
    let norm = |id: &str| info_of(id).map_or(0.0, |i| i.to_normalized(param(ins, id)));
    let amount = (param(ins, "filter_env") / 72.0).clamp(-1.0, 1.0);
    let mid = r.center().y;
    let span = r.height() * 0.42;
    let seg = r.width() * 0.4;
    let x0 = r.min.x + 6.0;
    let xa = x0 + seg * norm("filter_attack");
    let xd = xa + seg * norm("filter_decay");
    let peak = mid - amount * span;
    // The decay is exponential: a few points along it.
    let mut pts = vec![pos2(x0, mid), pos2(xa, peak)];
    for k in 1..=12 {
        let t = k as f32 / 12.0;
        let level = (0.001f32.ln() * t).exp();
        pts.push(pos2(xa + (xd - xa) * t, mid - amount * span * level));
    }
    pts.push(pos2(r.max.x - 6.0, mid));
    p.line_segment([pos2(r.min.x + 4.0, mid), pos2(r.max.x - 4.0, mid)], Stroke::new(1.0, WELL_GRID.gamma_multiply(1.6)));
    let col_mul = if on { 1.0 } else { 0.4 };
    for w in pts.windows(2) {
        if let [a, b] = w {
            let quad = if amount >= 0.0 { vec![*a, *b, pos2(b.x, mid), pos2(a.x, mid)] } else { vec![pos2(a.x, mid), pos2(b.x, mid), *b, *a] };
            p.add(Shape::convex_polygon(quad, AMBER.gamma_multiply(0.14 * col_mul), Stroke::NONE));
        }
    }
    p.add(Shape::line(pts.clone(), Stroke::new(2.5, AMBER.gamma_multiply(col_mul))));
    let handles = [pos2(xa, peak), pos2(xd, mid - amount * span * 0.001)];
    for q in handles {
        p.circle_filled(q, 6.0, CREAM);
        p.circle_stroke(q, 6.0, Stroke::new(1.0, CREAM_SHADE));
    }
    if !on {
        p.text(pos2(r.max.x - 12.0, r.min.y + 12.0), Align2::RIGHT_CENTER, "filter off", regular(12.0), DIM);
    }
    let grab = ui.id().with(("lift-fenv-grab", tid.0));
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let near = handles
            .iter()
            .enumerate()
            .map(|(i, q)| (i, q.distance(at)))
            .filter(|(_, d)| *d < 14.0)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i);
        ui.ctx().data_mut(|d| d.insert_temp(grab, near));
    }
    let held = ui.ctx().data(|d| d.get_temp::<Option<usize>>(grab)).flatten();
    if resp.dragged()
        && let Some(i) = held
    {
        let d = resp.drag_delta();
        let nudge = |app: &mut SoundApp, id: &str, by: f32| {
            if let Some(info) = info_of(id) {
                set(app, tid, id, info.from_normalized(info.to_normalized(param(ins, id)) + by));
            }
        };
        if i == 0 {
            nudge(app, "filter_attack", d.x / seg);
            set(app, tid, "filter_env", ((amount - d.y / span) * 72.0).clamp(-72.0, 72.0));
        } else {
            nudge(app, "filter_decay", d.x / seg);
        }
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<usize>>(grab));
    }
}

/// A section title in the side card.
fn section(p: &Painter, at: Pos2, name: &str) {
    p.text(at, Align2::LEFT_CENTER, name, bold(15.5), TEXT);
}

/// The side card: Pitch, LFO, Output and Playback.
fn side_card(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    card(p, r);
    let x = r.min.x + 16.0;
    let col = (r.width() - 32.0) / 2.0;
    let divider = |y: f32| p.line_segment([pos2(r.min.x + 12.0, y), pos2(r.max.x - 12.0, y)], Stroke::new(1.0, Color32::from_black_alpha(90)));
    // Pitch: Transpose as the hero, Root, Fine and Glide small.
    let mut y = r.min.y + 22.0;
    section(p, pos2(x, y), "Pitch");
    hero_knob(app, ui, p, Rect::from_min_size(pos2(x - 6.0, y + 8.0), vec2(110.0, 120.0)), tid, ins, "transpose", "Tune");
    small_knob(app, ui, p, pos2(x + 116.0, y + 8.0), tid, ins, "root_note", "Root");
    small_knob(app, ui, p, pos2(x + 116.0, y + 56.0), tid, ins, "fine_tune", "Fine");
    small_knob(app, ui, p, pos2(x + 116.0 + 110.0, y + 56.0), tid, ins, "glide", "Glide");
    y += 128.0;
    divider(y);
    // LFO: shape pills, a preview of the shape, and its four knobs.
    y += 20.0;
    section(p, pos2(x, y), "LFO");
    choice_pills(app, ui, p, pos2(x + 50.0, y - 13.0), tid, ins, "lfo_shape", &["Sine", "Tri", "Sq", "Saw", "Rnd"], 48.0);
    let prev = Rect::from_min_size(pos2(x, y + 18.0), vec2(r.width() - 32.0, 46.0));
    let synced = param(ins, "lfo_sync") >= 0.5;
    let rate = if synced { value_text(ins, "lfo_division") } else { value_text(ins, "lfo_rate") };
    lfo_preview(p, prev, param(ins, "lfo_shape").round() as usize, &rate);
    let sync_r = Rect::from_min_size(pos2(prev.max.x - 58.0, prev.min.y + 4.0), vec2(52.0, 20.0));
    let sync = pill(ui, p, sync_r, "Sync", synced, ("lfo-sync", tid.0)).on_hover_text("Lock the LFO to the song's tempo");
    if sync.clicked() {
        set(app, tid, "lfo_sync", if synced { 0.0 } else { 1.0 });
    }
    let ky = prev.max.y + 4.0;
    if synced {
        small_knob(app, ui, p, pos2(x, ky), tid, ins, "lfo_division", "Division");
    } else {
        small_knob(app, ui, p, pos2(x, ky), tid, ins, "lfo_rate", "Rate");
    }
    small_knob(app, ui, p, pos2(x + col, ky), tid, ins, "lfo_pitch", "To Pitch");
    small_knob(app, ui, p, pos2(x, ky + 46.0), tid, ins, "lfo_filter", "To Filter");
    small_knob(app, ui, p, pos2(x + col, ky + 46.0), tid, ins, "lfo_volume", "To Volume");
    y = ky + 96.0;
    divider(y);
    // Output: pan and the velocity range, in one row.
    y += 20.0;
    section(p, pos2(x, y), "Output");
    let third = (r.width() - 32.0) / 3.0;
    small_knob(app, ui, p, pos2(x, y + 8.0), tid, ins, "pan", "Pan");
    small_knob(app, ui, p, pos2(x + third, y + 8.0), tid, ins, "vel_low", "Vel Low");
    small_knob(app, ui, p, pos2(x + 2.0 * third, y + 8.0), tid, ins, "vel_high", "Vel High");
    y += 60.0;
    divider(y);
    // Playback.
    y += 20.0;
    section(p, pos2(x, y), "Playback");
    let row = |k: f32| y + 22.0 + k * 34.0;
    let label = |yy: f32, t: &str| p.text(pos2(x, yy + 13.0), Align2::LEFT_CENTER, t, regular(12.5), DIM);
    label(row(0.0), "Play");
    choice_pills(app, ui, p, pos2(x + 64.0, row(0.0)), tid, ins, "play_mode", &["Gate", "One-shot", "Slice"], 84.0);
    label(row(1.0), "Loop");
    choice_pills(app, ui, p, pos2(x + 64.0, row(1.0)), tid, ins, "loop_mode", &["Off", "Forward", "Ping-pong"], 84.0);
    label(row(2.0), "Voices");
    choice_pills(app, ui, p, pos2(x + 64.0, row(2.0)), tid, ins, "voice_mode", &["Poly", "Mono"], 84.0);
    // Warp: the sample at the song's tempo, its pitch free of its speed.
    label(row(3.0), "Warp");
    choice_pills(app, ui, p, pos2(x + 64.0, row(3.0)), tid, ins, "warp", &["Off", "On"], 50.0);
    small_knob(app, ui, p, pos2(x + 182.0, row(3.0) - 8.0), tid, ins, "sample_bpm", "Sample Tempo");
    let yy = row(4.0) + 10.0;
    let found = detected_bpm(app, ui, tid, ins);
    if found.is_none() && ins.sample.is_some() {
        p.text(pos2(x + 64.0, yy + 3.0), Align2::LEFT_CENTER, "no steady beat found: set the tempo by hand", regular(12.0), DIM);
    }
    if let Some(bpm) = found {
        p.text(pos2(x + 64.0, yy + 3.0), Align2::LEFT_CENTER, format!("detected {bpm:.1} bpm"), regular(12.0), DIM);
        if (f64::from(param(ins, "sample_bpm")) - bpm).abs() > 0.05 {
            let b = Rect::from_min_size(pos2(x + 200.0, yy - 10.0), vec2(96.0, 26.0));
            if pill(ui, p, b, "Use tempo", false, ("use-bpm", tid.0)).on_hover_text("Set the sample's tempo to the detected one").clicked() {
                set(app, tid, "sample_bpm", bpm as f32);
            }
        }
    }
}

/// The sample's tempo (its first 30 seconds), worked out once per sample; `None` for a sample
/// too short or without a steady beat backed by its hits (a single note, a pad).
fn detected_bpm(app: &SoundApp, ui: &Ui, tid: TrackId, ins: &Insert) -> Option<f64> {
    let src = ins.sample?;
    let key = ui.id().with(("lift-bpm", tid.0, src.0));
    if let Some(found) = ui.ctx().data(|d| d.get_temp::<Option<f64>>(key)) {
        return found;
    }
    let found = app.engine.session().pool.get(src).and_then(|audio| {
        let rate = audio.sample_rate();
        let n = audio.frames().min(rate as usize * 30);
        if n < rate as usize * 2 {
            return None;
        }
        let mono = soundcraft_dsp::beat::downmix(&audio.window(0, n));
        soundcraft_dsp::slices::steady_bpm(&mono, rate as f32)
    });
    ui.ctx().data_mut(|d| d.insert_temp(key, found));
    found
}

/// Two cycles of the LFO's shape in a well (the random shape as steps).
fn lfo_preview(p: &Painter, r: Rect, shape: usize, rate: &str) {
    well(p, r);
    let n = 160usize;
    let mid = r.center().y;
    let amp = r.height() * 0.36;
    let steps = [0.3f32, -0.7, 0.9, -0.2, 0.6, -0.9, 0.1, 0.75];
    let pts: Vec<Pos2> = (0..=n)
        .map(|i| {
            let t = i as f32 / n as f32 * 2.0;
            let ph = t.fract();
            let y = match shape {
                1 => 1.0 - 4.0 * (ph - 0.5).abs(),
                2 => {
                    if ph < 0.5 {
                        1.0
                    } else {
                        -1.0
                    }
                }
                3 => 2.0 * ph - 1.0,
                4 => steps.get(((t * 2.0) as usize) % steps.len()).copied().unwrap_or(0.0),
                _ => (std::f32::consts::TAU * ph).sin(),
            };
            pos2(r.min.x + 6.0 + (r.width() - 12.0) * i as f32 / n as f32, mid - y * amp)
        })
        .collect();
    p.add(Shape::line(pts, Stroke::new(2.0, AMBER)));
    p.text(pos2(r.min.x + 8.0, r.min.y + 9.0), Align2::LEFT_CENTER, rate, mono(10.0), DIM);
}

/// The keyboard: keys in the playable range lit cream, the others dark; the root in amber with
/// its name. Drag a range edge to move it; double-click a key to make it the root.
fn keyboard(app: &mut SoundApp, ui: &mut Ui, p: &Painter, r: Rect, tid: TrackId, ins: &Insert) {
    p.text(
        pos2(r.min.x, r.min.y + 6.0),
        Align2::LEFT_CENTER,
        "Keys · drag an amber edge to set the range, double-click a key for the root",
        regular(12.0),
        DIM,
    );
    let r = Rect::from_min_max(pos2(r.min.x, r.min.y + 16.0), r.max);
    let resp = ui.interact(r, ui.id().with(("lift-keys", tid.0)), Sense::click_and_drag());
    let lo = param(ins, "key_low").round().clamp(0.0, 127.0) as u8;
    let hi = param(ins, "key_high").round().clamp(0.0, 127.0) as u8;
    let (lo, hi) = (lo.min(hi), lo.max(hi));
    let root = param(ins, "root_note").round().clamp(0.0, 127.0) as u8;
    let black = |n: u8| matches!(n % 12, 1 | 3 | 6 | 8 | 10);
    let whites = (0..=127u8).filter(|n| !black(*n)).count() as f32;
    let ww = r.width() / whites;
    let mut keys: Vec<(u8, Rect)> = Vec::with_capacity(128);
    let mut wi = 0.0f32;
    for n in 0..=127u8 {
        if black(n) {
            let x = r.min.x + wi * ww - ww * 0.32;
            keys.push((n, Rect::from_min_size(pos2(x, r.min.y), vec2(ww * 0.64, r.height() * 0.6))));
        } else {
            keys.push((n, Rect::from_min_size(pos2(r.min.x + wi * ww, r.min.y), vec2(ww, r.height()))));
            wi += 1.0;
        }
    }
    let slice_keys = slicing(ins).then(|| (root, root.saturating_add(slices(ins).len().min(127) as u8).saturating_sub(1)));
    let in_range = |n: u8| match slice_keys {
        Some((a, b)) => n >= a && n <= b,
        None => n >= lo && n <= hi,
    };
    for (n, k) in keys.iter().filter(|(n, _)| !black(*n)) {
        let fill = if *n == root {
            AMBER
        } else if in_range(*n) {
            CREAM
        } else {
            Color32::from_rgb(0x5A, 0x50, 0x46)
        };
        p.rect_filled(k.shrink2(vec2(0.6, 0.0)), CornerRadius { nw: 0, ne: 0, sw: 3, se: 3 }, fill);
        if n % 12 == 0 {
            p.text(pos2(k.center().x, k.max.y - 3.0), Align2::CENTER_BOTTOM, soundcraft_midi::note_name(*n), regular(9.0), INK.gamma_multiply(0.8));
        }
    }
    for (n, k) in keys.iter().filter(|(n, _)| black(*n)) {
        let fill = if *n == root {
            AMBER
        } else if in_range(*n) {
            Color32::from_rgb(0x1A, 0x15, 0x11)
        } else {
            Color32::from_rgb(0x3A, 0x32, 0x2B)
        };
        p.rect_filled(*k, CornerRadius { nw: 0, ne: 0, sw: 2, se: 2 }, fill);
    }
    let edge = |n: u8, upper: bool| keys.iter().find(|(k, _)| *k == n).map_or(r.min.x, |(_, k)| if upper { k.max.x } else { k.min.x });
    let (xl, xh) = (edge(lo, false), edge(hi, true));
    if slice_keys.is_none() {
        for x in [xl, xh] {
            p.line_segment([pos2(x, r.min.y - 2.0), pos2(x, r.max.y + 2.0)], Stroke::new(3.0, AMBER));
        }
    }
    let key_at = |x: f32, y: f32| {
        keys.iter()
            .filter(|(n, k)| black(*n) && k.contains(pos2(x, y)))
            .map(|(n, _)| *n)
            .next()
            .or_else(|| keys.iter().find(|(n, k)| !black(*n) && x >= k.min.x && x < k.max.x).map(|(n, _)| *n))
    };
    if resp.double_clicked()
        && let Some(at) = resp.interact_pointer_pos()
        && let Some(n) = key_at(at.x, at.y)
    {
        set(app, tid, "root_note", f32::from(n));
    }
    let grab = ui.id().with(("lift-keyrange", tid.0));
    if resp.drag_started()
        && let Some(at) = resp.interact_pointer_pos()
    {
        let which = if (at.x - xl).abs() <= (at.x - xh).abs() { "key_low" } else { "key_high" };
        let near = (at.x - xl).abs().min((at.x - xh).abs()) < 10.0;
        ui.ctx().data_mut(|d| d.insert_temp(grab, near.then(|| which.to_string())));
    }
    let held = ui.ctx().data(|d| d.get_temp::<Option<String>>(grab)).flatten();
    if let Some(h) = resp.hover_pos()
        && ((h.x - xl).abs() < 10.0 || (h.x - xh).abs() < 10.0)
    {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    if resp.dragged()
        && let (Some(id), Some(at)) = (held.as_deref(), resp.interact_pointer_pos())
        && let Some(n) = key_at(at.x.clamp(r.min.x, r.max.x - 1.0), r.max.y - 2.0)
    {
        set(app, tid, id, f32::from(n));
    }
    if resp.drag_stopped() {
        ui.ctx().data_mut(|d| d.remove::<Option<String>>(grab));
    }
}

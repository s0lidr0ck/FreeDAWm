//! Clip › Sample Editor: one audio clip's samples in a window of their own, with its own zoom.
//! Drag to select samples (Silence, Reverse, Invert, Gain act on them, in a copy of the file the
//! clip then plays: `clip.sample_edit`); with Pencil on and zoomed in to the samples, a drag
//! redraws them (`clip.draw_samples`). One undo step per edit or stroke.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::{Color32, Pos2, Rect, Sense, Shape, Stroke, pos2, vec2};
use serde_json::json;
use soundcraft_model::{Clip, ClipId};
use soundcraft_time::Samples;

/// The window's view and what is being done in it (kept in egui memory, not saved).
#[derive(Debug, Clone, Default)]
struct View {
    clip: Option<ClipId>,
    /// Samples per point and the first sample shown (in the clip, from its start).
    spp: f64,
    scroll: f64,
    pencil: bool,
    /// Selected samples (in the clip).
    sel: Option<(Samples, Samples)>,
    /// A selection drag's anchor, or a stroke's points (in the clip, value) and channel.
    anchor: Option<Samples>,
    stroke: Vec<(Samples, f32)>,
    stroke_ch: usize,
    gain_db: f32,
}

/// The audio clip the editor shows: the first selected one.
fn target(app: &SoundApp) -> Option<Clip> {
    let s = app.engine.session();
    s.edit.selected_clips.iter().filter_map(|id| s.find_clip(*id)).map(|(_, c)| c).find(|c| c.is_audio()).cloned()
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_sample_editor;
    if !open {
        return;
    }
    let key = egui::Id::new("sample_editor_view");
    let mut v: View = ctx.memory(|m| m.data.get_temp(key)).unwrap_or_default();
    egui::Window::new("Sample Editor").open(&mut open).default_size(vec2(640.0, 300.0)).show(ctx, |ui| {
        let Some(clip) = target(app) else {
            ui.label("Select an audio clip to edit its samples.");
            return;
        };
        if v.clip != Some(clip.id) {
            // A new clip: shown whole.
            v = View { clip: Some(clip.id), spp: clip.length as f64 / 600.0, gain_db: -3.0, ..View::default() };
        }
        let plain = clip.stretch == 1.0 && clip.warp.is_empty() && clip.loop_len == 0 && !clip.reversed;
        ui.horizontal(|ui| {
            ui.label(egui::RichText::new(&clip.name).strong());
            if ui.small_button("−").on_hover_text("Zoom out").clicked() {
                v.spp *= 2.0;
            }
            if ui.small_button("+").on_hover_text("Zoom in").clicked() {
                v.spp /= 2.0;
            }
            if ui.small_button("Whole Clip").clicked() {
                (v.spp, v.scroll) = (clip.length as f64 / ui.available_width().max(100.0) as f64, 0.0);
            }
            crate::widgets::checkbox(ui, &mut v.pencil, "Pencil");
        });
        if !plain {
            ui.label("This clip is stretched, looped or reversed: consolidate it to edit its samples.");
        }
        let (resp, painter) = ui.allocate_painter(vec2(ui.available_width(), (ui.available_height() - 30.0).max(80.0)), Sense::click_and_drag());
        let r = resp.rect;
        v.spp = v.spp.clamp(1.0 / 64.0, (clip.length.max(1) as f64 / f64::from(r.width().max(1.0))).max(1.0 / 64.0));
        v.scroll = v.scroll.clamp(0.0, (clip.length as f64 - v.spp * f64::from(r.width())).max(0.0));
        if resp.hovered() {
            // Wheel scrolls; Ctrl/Cmd + wheel zooms about the pointer.
            let (dy, zoom) = ui.input(|i| (i.smooth_scroll_delta, i.modifiers.command || i.modifiers.ctrl));
            if zoom && dy.y != 0.0 {
                let at = resp.hover_pos().map_or(0.0, |p| f64::from(p.x - r.min.x));
                let pos = v.scroll + at * v.spp;
                v.spp *= if dy.y > 0.0 { 0.8 } else { 1.25 };
                v.scroll = pos - at * v.spp;
            } else if dy.x != 0.0 || dy.y != 0.0 {
                v.scroll -= f64::from(dy.x + dy.y) * v.spp;
            }
        }
        let t = Tokens::current();
        painter.rect_filled(r, 2.0, t.panel_bg);
        let x_of = |i: f64| r.min.x + ((i - v.scroll) / v.spp) as f32;
        let at_x = |x: f32| (v.scroll + f64::from(x - r.min.x) * v.spp).round() as Samples;
        if let Some((a, b)) = v.sel {
            let sr = Rect::from_x_y_ranges(x_of(a as f64).max(r.min.x)..=x_of(b as f64).min(r.max.x), r.y_range());
            painter.rect_filled(sr, 0.0, Color32::from_rgba_unmultiplied(120, 160, 230, 50));
        }
        let s = app.engine.session_arc();
        let audio = clip.source().and_then(|src| s.pool.get(src)).cloned();
        let channels = audio.as_ref().map_or(1, |a| a.num_channels()).max(1);
        let h = r.height() / channels as f32;
        let band = |ch: usize| Rect::from_min_size(pos2(r.min.x, r.min.y + h * ch as f32), vec2(r.width(), h));
        let y_of = |ch: usize, val: f32| band(ch).center().y - val.clamp(-1.0, 1.0) * h * 0.46;
        if let Some(a) = &audio {
            let off = usize::try_from(clip.source_offset().max(0)).unwrap_or(0);
            let first = v.scroll.floor().max(0.0) as usize;
            let last = ((v.scroll + v.spp * f64::from(r.width())).ceil() as usize).min(usize::try_from(clip.length.max(0)).unwrap_or(0));
            for ch in 0..channels {
                let b = band(ch);
                painter.hline(r.x_range(), b.center().y, Stroke::new(1.0, t.border));
                let mut buf = vec![0.0f32; last.saturating_sub(first) + 1];
                a.read(ch, off + first, &mut buf);
                if v.spp <= 1.0 {
                    let pts: Vec<Pos2> = buf.iter().enumerate().map(|(i, x)| pos2(x_of((first + i) as f64), y_of(ch, *x))).collect();
                    painter.add(Shape::line(pts.clone(), Stroke::new(1.0, t.accent)));
                    if v.spp <= 1.0 / 8.0 {
                        for p in pts {
                            painter.circle_filled(p, 2.0, t.accent);
                        }
                    }
                } else {
                    // One column per point: the samples' lowest and highest there.
                    let per = v.spp.max(1.0) as usize;
                    for (k, chunk) in buf.chunks(per.max(1)).enumerate() {
                        let (lo, hi) = chunk.iter().fold((0f32, 0f32), |(l, h), x| (l.min(*x), h.max(*x)));
                        let x = x_of((first + k * per) as f64);
                        painter.vline(x, y_of(ch, hi)..=y_of(ch, lo), Stroke::new(1.0, t.accent));
                    }
                }
            }
        }
        // The stroke being drawn.
        if v.stroke.len() > 1 {
            let pts: Vec<Pos2> = v.stroke.iter().map(|(i, x)| pos2(x_of(*i as f64), y_of(v.stroke_ch, *x))).collect();
            painter.add(Shape::line(pts, Stroke::new(1.5, Color32::from_rgb(240, 200, 80))));
        }
        let can_draw = plain && v.pencil && v.spp <= crate::waveform::PENCIL_SAMPLES_PER_POINT;
        if v.pencil && !can_draw && resp.hovered() {
            resp.clone().on_hover_text("Zoom in to the samples to draw them");
        }
        if let Some(p) = resp.interact_pointer_pos() {
            let i = at_x(p.x).clamp(0, clip.length.saturating_sub(1).max(0));
            if resp.drag_started() || (resp.is_pointer_button_down_on() && v.anchor.is_none() && v.stroke.is_empty()) {
                if can_draw {
                    v.stroke_ch = (((p.y - r.min.y) / h).floor().max(0.0) as usize).min(channels - 1);
                    v.stroke.clear();
                } else {
                    v.anchor = Some(i);
                }
            }
            if can_draw && resp.is_pointer_button_down_on() {
                let b = band(v.stroke_ch);
                let val = ((b.center().y - p.y) / (b.height() * 0.46)).clamp(-1.0, 1.0);
                v.stroke.push((i, val));
            } else if let Some(a) = v.anchor {
                v.sel = (a != i).then(|| (a.min(i), a.max(i)));
            }
        }
        if resp.drag_stopped() || resp.clicked() || (!resp.is_pointer_button_down_on() && (!v.stroke.is_empty() || v.anchor.is_some())) {
            v.anchor = None;
            let stroke = std::mem::take(&mut v.stroke);
            if let Some(values) = stroke_values(&stroke) {
                let (from, vals) = values;
                let p = json!({"clip": clip.id.0, "at": clip.start + from, "values": vals, "channel": v.stroke_ch});
                if let Err(e) = app.run("clip.draw_samples", p) {
                    app.ui.status = e;
                }
            }
        }
        ui.horizontal(|ui| {
            let sel = v.sel.filter(|_| plain);
            ui.add_enabled_ui(sel.is_some(), |ui| {
                let mut op = None;
                for (name, id) in [("Silence", "silence"), ("Reverse", "reverse"), ("Invert", "invert")] {
                    if ui.button(name).clicked() {
                        op = Some(json!({"op": id}));
                    }
                }
                ui.add(egui::DragValue::new(&mut v.gain_db).range(-48.0..=24.0).suffix(" dB").speed(0.1));
                if ui.button("Gain").clicked() {
                    op = Some(json!({"op": "gain", "db": v.gain_db}));
                }
                if let (Some(mut p), Some((a, b))) = (op, sel) {
                    p["clip"] = json!(clip.id.0);
                    p["start"] = json!(clip.start + a);
                    p["end"] = json!(clip.start + b);
                    if let Err(e) = app.run("clip.sample_edit", p) {
                        app.ui.status = e;
                    }
                }
            });
            if let Some((a, b)) = v.sel {
                ui.label(format!("{} samples selected", b - a));
            }
        });
    });
    ctx.memory_mut(|m| m.data.insert_temp(key, v));
    app.ui.show_sample_editor = open;
}

/// A stroke's points (position, value; in drawing order) as one value per sample from its
/// first position: later points win, gaps are joined by straight lines. `None` when empty.
pub fn stroke_values(stroke: &[(Samples, f32)]) -> Option<(Samples, Vec<f32>)> {
    let lo = stroke.iter().map(|p| p.0).min()?;
    let hi = stroke.iter().map(|p| p.0).max()?;
    let n = usize::try_from(hi - lo + 1).ok()?.min(soundcraft_engine::cmd::MAX_DRAWN_SAMPLES);
    let mut vals: Vec<Option<f32>> = vec![None; n];
    for w in stroke.windows(2) {
        let (Some(&(a, va)), Some(&(b, vb))) = (w.first(), w.get(1)) else { continue };
        let (lo2, hi2) = (a.min(b), a.max(b));
        for i in lo2..=hi2 {
            let t = if b == a { 1.0 } else { (i - a) as f32 / (b - a) as f32 };
            if let Some(x) = usize::try_from(i - lo).ok().and_then(|k| vals.get_mut(k)) {
                *x = Some(va + (vb - va) * t);
            }
        }
    }
    if let (Some(&(a, va)), true) = (stroke.first(), stroke.len() == 1)
        && let Some(x) = usize::try_from(a - lo).ok().and_then(|k| vals.get_mut(k))
    {
        *x = Some(va);
    }
    // Any sample not reached keeps a straight line from its neighbours (none should be missed).
    let mut last = 0.0;
    Some((lo, vals.into_iter().map(|x| x.inspect(|v| last = *v).unwrap_or(last)).collect()))
}

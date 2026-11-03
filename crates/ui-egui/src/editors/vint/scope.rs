//! `VintScope.h`: the curve. For the five EQs, the exact response of the circuit the audio runs
//! through (`VintEngineDSP::responseDb`, 240 points at 192 kHz); for the three flavour circuits,
//! a private engine measured at two levels (quiet −40 dBFS, loud −12 dBFS), one sine per point at
//! 24 points, 150 ms after the last change. Both run on the curve worker (off the UI thread); the
//! last curve stays drawn until the new one is in.

use crate::curve_worker::CurveWorker;
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, paint_panel};
use crate::editor_kit::palette::{VstPalette, al};
use crate::theme::regular;
use egui::{Align2, Id, Pos2, Rect, Shape, Stroke, Ui, pos2};
use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp::{self as d, NUM_PARAMS, Values, VintEngineDsp};
use std::f64::consts::PI;
use std::sync::Arc;

const K_CURVE_POINTS: usize = 240;
const K_MEASURED_POINTS: usize = 24;

fn freq_at(i: usize, n: usize) -> f64 {
    20.0 * 1000f64.powf(i as f64 / (n - 1) as f64)
}
fn x_for(f: f64, r: Rect) -> f32 {
    r.left() + ((f / 20.0).ln() / 1000f64.ln()) as f32 * r.width()
}
fn y_for(db: f64, r: Rect) -> f32 {
    r.top() + ((20.0 - db) / 40.0).clamp(0.0, 1.0) as f32 * r.height()
}

fn circuit_of(v: &[f32]) -> i32 {
    let c = v.get(d::P_CIRCUIT).copied().unwrap_or(0.0);
    if c.is_finite() { (c.round() as i32).clamp(0, d::NUM_CIRCUITS - 1) } else { 0 }
}
/// The five EQs: a fixed curve.
fn is_eq(c: i32) -> bool {
    c <= d::C_BRITISH || c == d::C_PASSIVE_MID
}

fn values_of(v: &[f32]) -> Values {
    let mut out = [0.0f32; NUM_PARAMS];
    for (i, o) in out.iter_mut().enumerate() {
        *o = v.get(i).copied().filter(|x| x.is_finite()).unwrap_or_else(|| d::default_of(i));
    }
    out
}

/// `computeCurve`: the EQ's response at 240 points.
fn compute_curve(v: &Values) -> Vec<f32> {
    let c = circuit_of(v);
    (0..K_CURVE_POINTS).map(|i| d::response_db(c, v, freq_at(i, K_CURVE_POINTS), 192_000.0) as f32).collect()
}

/// `measure`: the gain at `f` of a sine of amplitude `amp` through a private engine (no stage,
/// fully wet, output 0 dB, Eco).
fn measure_at(v: &Values, f: f64, amp: f64) -> f64 {
    let mut v = *v;
    v[d::P_STAGE] = 0.0;
    v[d::P_MIX] = 1.0;
    v[d::P_BYPASS] = 0.0;
    v[d::P_OUTPUT] = 0.5;
    v[d::P_QUALITY] = 0.0;
    let mut e = Box::new(VintEngineDsp::new());
    for (i, x) in v.iter().enumerate() {
        e.set_parameter(i, *x);
    }
    let sr = 48_000.0;
    e.prepare(sr);
    let settle = ((0.12f64).max(6.0 / f) * sr) as usize;
    let window = ((0.06f64).max(8.0 / f) * sr) as usize;
    let mut buf: Vec<f32> = (0..settle + window).map(|n| (amp * (2.0 * PI * f * n as f64 / sr).sin()) as f32).collect();
    let mut block = vec![vec![0.0f32; 512]];
    let mut done = 0;
    while done < buf.len() {
        let n = 512.min(buf.len() - done);
        block[0][..n].copy_from_slice(&buf[done..done + n]);
        e.process(&mut block, 1, n);
        buf[done..done + n].copy_from_slice(&block[0][..n]);
        done += 512;
    }
    let (mut re, mut im, mut ws) = (0.0, 0.0, 0.0);
    for i in 0..window {
        let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (window - 1) as f64).cos();
        let ph = 2.0 * PI * f * i as f64 / sr;
        let x = f64::from(buf[settle + i]);
        re += w * x * ph.cos();
        im += w * x * ph.sin();
        ws += w;
    }
    20.0 * (2.0 * (re * re + im * im).sqrt() / ws / amp).max(1.0e-9).log10()
}

/// The worker's job: the EQ curve (240 values) or the two measured curves (24 quiet, 24 loud).
pub(crate) fn compute(values: &[f32]) -> Vec<f32> {
    let v = values_of(values);
    if is_eq(circuit_of(&v)) {
        compute_curve(&v)
    } else {
        let mut out = vec![0.0f32; 2 * K_MEASURED_POINTS];
        for i in 0..K_MEASURED_POINTS {
            let f = freq_at(i, K_MEASURED_POINTS);
            out[i] = measure_at(&v, f, 0.01) as f32;
            out[K_MEASURED_POINTS + i] = measure_at(&v, f, 0.25) as f32;
        }
        out
    }
}

fn worker(ctx: &egui::Context) -> Arc<CurveWorker> {
    let id = Id::new("vint-scope-worker");
    if let Some(w) = ctx.data(|d| d.get_temp::<Arc<CurveWorker>>(id)) {
        return w;
    }
    let repaint = ctx.clone();
    let w = Arc::new(CurveWorker::new(Box::new(|v, _sr, _f| compute(v)), Box::new(move || repaint.request_repaint())));
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&w)));
    w
}

/// The scope's state, kept by the editor.
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    want: Option<(u64, f64)>,
    requested: Option<u64>,
    /// (eq, the curve) of the latest result.
    result: Option<(bool, Arc<Vec<f32>>)>,
}

/// Paints the scope in `r`; `values`: every parameter by index.
pub(crate) fn draw(ui: &mut Ui, r: Rect, pal: &VstPalette, st: &mut View, values: &Values, key: Id) {
    let c = circuit_of(values);
    let eq = is_eq(c);
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        values.iter().for_each(|v| v.to_bits().hash(&mut h));
        h.finish()
    };
    let now = ui.input(|i| i.time);
    if st.want.is_none_or(|(h, _)| h != hash) {
        st.want = Some((hash, now));
    }
    let w = worker(ui.ctx());
    // The EQs' curve is "instant" in the C++ (no timer); the flavours wait 150 ms.
    let wait = if eq { 0.0 } else { 0.15 };
    if let Some((h, at)) = st.want
        && st.requested != Some(h)
    {
        if now - at >= wait {
            let len = if eq { K_CURVE_POINTS } else { 2 * K_MEASURED_POINTS };
            w.request(key, h, values, 48_000.0, &vec![0.0; len]);
            st.requested = Some(h);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((wait - (now - at)).max(0.0)));
        }
    }
    if let Some((h, curve)) = w.take(key)
        && Some(h) == st.requested
    {
        st.result = Some((eq, curve));
    }

    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);
    let b = r.shrink2(egui::vec2(12.0, 10.0));
    let caption = Rect::from_min_max(pos2(b.left(), b.bottom() - 14.0), b.max);
    let labels = Rect::from_min_max(pos2(b.left(), caption.top() - 14.0), pos2(b.right(), caption.top()));
    let plot = Rect::from_min_max(pos2(b.left() + 30.0, b.top()), pos2(b.right(), labels.top()));

    for db in (-18..=18).rev().step_by(6) {
        let y = y_for(f64::from(db), plot).round();
        p.line_segment(
            [pos2(plot.left(), y + 0.5), pos2(plot.right(), y + 0.5)],
            Stroke::new(1.0, al(pal.text_faint, if db == 0 { 0.35 } else { 0.13 })),
        );
        let t = format!("{}{db}", if db > 0 { "+" } else { "" });
        p.text(pos2(plot.left() - 4.0, y), Align2::RIGHT_CENTER, t, regular(TINY_SIZE), pal.text_faint);
    }
    for f in [30.0, 100.0, 300.0, 1000.0, 3000.0, 10000.0] {
        let x = x_for(f, plot).round();
        p.line_segment([pos2(x + 0.5, plot.top()), pos2(x + 0.5, plot.bottom())], Stroke::new(1.0, al(pal.text_faint, 0.12)));
        let t = if f >= 1000.0 { format!("{}k", (f / 1000.0) as i32) } else { format!("{}", f as i32) };
        p.text(pos2(x, labels.center().y), Align2::CENTER_CENTER, t, regular(CAPTION_SIZE), pal.text_faint);
    }
    let stroke = |db: &[f32], col: egui::Color32, w: f32| {
        if db.len() < 2 {
            return;
        }
        let pts: Vec<Pos2> = db.iter().enumerate().map(|(i, v)| pos2(x_for(freq_at(i, db.len()), plot), y_for(f64::from(*v), plot))).collect();
        p.with_clip_rect(r).add(Shape::line(pts, Stroke::new(w, col)));
    };
    match &st.result {
        Some((true, curve)) if eq && curve.len() == K_CURVE_POINTS => stroke(curve, pal.accent, 2.0),
        Some((false, curve)) if !eq && curve.len() == 2 * K_MEASURED_POINTS => {
            stroke(&curve[..K_MEASURED_POINTS], al(pal.text_body, 0.75), 1.5);
            stroke(&curve[K_MEASURED_POINTS..], pal.accent, 2.0);
        }
        _ => {}
    }
    let cap = if eq {
        "The exact response of the circuit the audio runs through"
    } else {
        "Measured: quiet (-40 dBFS, grey) and loud (-12 dBFS). The gap is the level-dependence"
    };
    // ends in "…" where it does not fit (the slim drawer); the whole line on hover
    let mut job = egui::text::LayoutJob::simple_singleline(cap.to_string(), regular(CAPTION_SIZE), pal.text_faint);
    job.wrap = egui::text::TextWrapping::truncate_at_width(caption.width());
    let g = ui.fonts_mut(|f| f.layout_job(job));
    let cut = g.size().x + 1.0 < p.layout_no_wrap(cap.to_string(), regular(CAPTION_SIZE), pal.text_faint).size().x;
    p.with_clip_rect(caption).galley(pos2(caption.left(), caption.center().y - g.size().y / 2.0), g, pal.text_faint);
    if cut {
        let _ = ui.interact(caption, key.with("caption"), egui::Sense::hover()).on_hover_text(cap);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn eq_curve_and_flavour_measurement_are_finite() {
        let mut v = d::DEFAULTS;
        let c = compute(&v);
        assert_eq!(c.len(), K_CURVE_POINTS);
        assert!(c.iter().all(|x| x.is_finite()));
        v[d::P_CIRCUIT] = d::C_AIR as f32;
        let m = compute(&v);
        assert_eq!(m.len(), 2 * K_MEASURED_POINTS);
        assert!(m.iter().all(|x| x.is_finite()));
        assert!(compute(&[f32::NAN; NUM_PARAMS]).iter().all(|x| x.is_finite()));
    }
}

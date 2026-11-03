//! `TapeScope.h` (`fs::tape::TapeScope`): the editor's measured display. Private TapeEngines
//! (no transport, no hiss, fully wet, output at 0 dB) are measured whenever a setting changes:
//! Response (a −20 dB third-octave multitone and a 0 VU sine per point, each against its own
//! 1 kHz) or Harmonics (H2..H9 of a 0 VU sine). The measurements run on the curve worker, 120 ms
//! after the last change (`schedule`); a newer request replaces one still waiting.

use crate::curve_worker::CurveWorker;
use crate::editor_kit::controls::{SelectionStyle, segmented};
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, VALUE_SIZE, font, paint_panel};
use crate::editor_kit::palette::{VstPalette, al};
use crate::theme::regular;
use egui::{Align2, CornerRadius, Id, Pos2, Rect, Shape, Stroke, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::tape::tape_engine_dsp::{NUM_PARAMS, P_BYPASS, P_FLUTTER, P_HISS, P_MIX, P_OUTPUT, TapeEngineDsp};
use std::f64::consts::PI;
use std::sync::Arc;

const K_TONES: usize = 31;
const K_N: usize = 16384;
const K_RESP_SR: f64 = 48_000.0;
/// 20 · 2^(17/3) = 1016 Hz.
const K_REF: usize = 17;
const K_TOP_DB: f64 = 6.0;
const K_BOTTOM_DB: f64 = -15.0;

/// The worker's output: Response = `[quiet; 31] + [hot; 31]`; Harmonics = `[thd, level 2..=9,
/// reachable 2..=9]`.
const OUT_LEN: usize = 2 * K_TONES;

fn tone_hz(i: usize) -> f64 {
    20.0 * 2f64.powf(i as f64 / 3.0)
}
fn tone_bin(i: usize) -> f64 {
    (tone_hz(i) * K_N as f64 / K_RESP_SR).round()
}
/// The hot curve's points: every other third octave, and the reference.
fn is_hot_point(t: usize) -> bool {
    t.is_multiple_of(2) || t == K_REF
}

/// `makeEngine`.
fn make_engine(params: &[f32], sr: f64) -> Box<TapeEngineDsp> {
    let mut d = Box::new(TapeEngineDsp::new());
    for i in 0..NUM_PARAMS {
        let v = params.get(i).copied().unwrap_or(0.0);
        d.set_parameter(i, if v.is_finite() { v } else { 0.0 });
    }
    d.set_parameter(P_BYPASS, 0.0);
    d.set_parameter(P_FLUTTER, 0.0);
    d.set_parameter(P_HISS, 0.0);
    d.set_parameter(P_MIX, 1.0);
    d.set_parameter(P_OUTPUT, 0.5);
    d.prepare(sr);
    d
}

/// `run`: 512-sample blocks, in place.
fn run(d: &mut TapeEngineDsp, buf: &mut [f32]) {
    let mut block = vec![vec![0.0f32; 512]];
    let mut done = 0;
    while done < buf.len() {
        let n = 512.min(buf.len() - done);
        block[0][..n].copy_from_slice(&buf[done..done + n]);
        d.process(&mut block, 1, n);
        buf[done..done + n].copy_from_slice(&block[0][..n]);
        done += 512;
    }
}

fn hann_amplitude(buf: &[f32], from: usize, n: usize, f: f64, sr: f64) -> f64 {
    let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let w = 0.5 - 0.5 * (2.0 * PI * i as f64 / (n - 1) as f64).cos();
        let ph = 2.0 * PI * f * i as f64 / sr;
        let x = f64::from(buf.get(from + i).copied().unwrap_or(0.0));
        re += w * x * ph.cos();
        im += w * x * ph.sin();
        wsum += w;
    }
    2.0 * (re * re + im * im).sqrt() / wsum
}

/// `measureResponse`: `[quiet; 31]` then `[hot; 31]` (dB re 1 kHz).
pub(crate) fn measure_response(params: &[f32]) -> Vec<f32> {
    let mut out = vec![0.0f32; OUT_LEN];
    // ---- quiet: the multitone (Schroeder phases).
    {
        let settle = (0.35 * K_RESP_SR) as usize;
        let total = settle + K_N;
        let a = 0.0125893 * (2.0 / K_TONES as f64).sqrt();
        let mut buf = vec![0.0f32; total];
        for t in 0..K_TONES {
            let w = 2.0 * PI * tone_bin(t) / K_N as f64;
            let ph = PI * (t * t) as f64 / K_TONES as f64;
            for (n, b) in buf.iter_mut().enumerate() {
                *b += (a * (w * n as f64 + ph).sin()) as f32;
            }
        }
        let mut d = make_engine(params, K_RESP_SR);
        run(&mut d, &mut buf);
        let mut g = [0.0f64; K_TONES];
        for (t, gt) in g.iter_mut().enumerate() {
            let w = 2.0 * PI * tone_bin(t) / K_N as f64;
            let (mut re, mut im) = (0.0, 0.0);
            for n in 0..K_N {
                let x = f64::from(buf[settle + n]);
                re += x * (w * (settle + n) as f64).cos();
                im += x * (w * (settle + n) as f64).sin();
            }
            *gt = 2.0 * (re * re + im * im).sqrt() / K_N as f64 / a;
        }
        for t in 0..K_TONES {
            out[t] = (20.0 * (g[t] / g[K_REF].max(1.0e-12)).max(1.0e-6).log10()) as f32;
        }
    }
    // ---- hot: one sine at 0 VU per point, a fresh machine each time.
    {
        let amp = 0.1778;
        let mut g = [0.0f64; K_TONES];
        for (t, gt) in g.iter_mut().enumerate() {
            if !is_hot_point(t) {
                continue;
            }
            let f = tone_hz(t);
            let settle = ((0.12f64).max(6.0 / f) * K_RESP_SR) as usize;
            let window = ((0.06f64).max(8.0 / f) * K_RESP_SR) as usize;
            let mut buf: Vec<f32> = (0..settle + window).map(|n| (amp * (2.0 * PI * f * n as f64 / K_RESP_SR).sin()) as f32).collect();
            let mut d = make_engine(params, K_RESP_SR);
            run(&mut d, &mut buf);
            *gt = hann_amplitude(&buf, settle, window, f, K_RESP_SR) / amp;
        }
        for t in 0..K_TONES {
            out[K_TONES + t] = if is_hot_point(t) { (20.0 * (g[t] / g[K_REF].max(1.0e-12)).max(1.0e-6).log10()) as f32 } else { 0.0 };
        }
    }
    out
}

/// `measureHarmonics`: `[thd, level 2..=9, reachable 2..=9]`.
pub(crate) fn measure_harmonics(params: &[f32], freq_index: usize) -> Vec<f32> {
    const SR: f64 = 96_000.0;
    let f = [100.0, 1000.0, 5000.0][freq_index.min(2)];
    let mut d = make_engine(params, SR);
    let total = (0.6 * SR) as usize;
    let window = (0.2 * SR) as usize;
    let mut buf: Vec<f32> = (0..total).map(|i| (0.1778 * (2.0 * PI * f * i as f64 / SR).sin()) as f32).collect();
    run(&mut d, &mut buf);
    let from = total - window;
    let fund = hann_amplitude(&buf, from, window, f, SR);
    let mut sum = 0.0;
    let mut out = vec![0.0f32; OUT_LEN];
    for k in 2..=9usize {
        let reachable = f * (k as f64) < 0.45 * SR;
        out[1 + k - 2] = -200.0;
        out[9 + k - 2] = if reachable { 1.0 } else { 0.0 };
        if !reachable || fund <= 0.0 {
            continue;
        }
        let a = hann_amplitude(&buf, from, window, f * k as f64, SR);
        sum += a * a;
        out[1 + k - 2] = (20.0 * (a / fund).max(1.0e-12).log10()) as f32;
    }
    out[0] = if fund > 0.0 { (sum.sqrt() / fund) as f32 } else { 0.0 };
    out
}

/// The worker's job: `values` = the parameters, then the view, then the tone.
fn measure(values: &[f32]) -> Vec<f32> {
    let get = |i: usize| values.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0);
    if get(NUM_PARAMS) < 0.5 { measure_response(values) } else { measure_harmonics(values, get(NUM_PARAMS + 1).round().clamp(0.0, 2.0) as usize) }
}

fn worker(ctx: &egui::Context) -> Arc<CurveWorker> {
    let id = Id::new("tape-scope-worker");
    if let Some(w) = ctx.data(|d| d.get_temp::<Arc<CurveWorker>>(id)) {
        return w;
    }
    let repaint = ctx.clone();
    let w = Arc::new(CurveWorker::new(Box::new(|v, _sr, _f| measure(v)), Box::new(move || repaint.request_repaint())));
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&w)));
    w
}

/// The scope's state (kept by the editor).
#[derive(Clone, Debug, Default)]
pub(crate) struct View {
    /// 0 Response, 1 Harmonics.
    pub view: usize,
    /// 1 kHz at open (set by the editor's init).
    pub freq_index: usize,
    want: Option<(u64, f64)>,
    requested: Option<u64>,
    response: Option<Arc<Vec<f32>>>,
    harmonics: Option<Arc<Vec<f32>>>,
}

fn x_for(f: f64, plot: Rect) -> f32 {
    let t = (f / 20.0).ln() / (20000.0f64 / 20.0).ln();
    plot.left() + t.clamp(0.0, 1.0) as f32 * plot.width()
}
fn y_for(db: f64, plot: Rect) -> f32 {
    let t = ((K_TOP_DB - db) / (K_TOP_DB - K_BOTTOM_DB)).clamp(0.0, 1.0);
    plot.top() + t as f32 * plot.height()
}

fn format_percent(t: f64) -> String {
    let pct = 100.0 * t;
    if pct < 0.01 {
        "< 0.01 %".into()
    } else if pct < 1.0 {
        format!("{pct:.2} %")
    } else if pct < 10.0 {
        format!("{pct:.1} %")
    } else {
        format!("{} %", pct.round() as i64)
    }
}

/// The scope's inner area in `r` (the slim layout puts its display toggle at its top-left).
pub(crate) fn inner(r: Rect) -> Rect {
    r.shrink2(vec2(12.0, 10.0))
}

/// Paints the scope in `r` and handles its tabs. `params`: every engine parameter by index.
/// `slim`: the slim layout's display toggle (drawn by it over the tab row) stands in for the view
/// tabs, and the tone tabs get a row of their own under it.
pub(crate) fn draw_ex(ui: &mut Ui, r: Rect, pal: &VstPalette, st: &mut View, params: &[f32; NUM_PARAMS], key: Id, slim: bool) {
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);

    // resized()
    let mut b = inner(r);
    let head = Rect::from_min_size(b.min, vec2(b.width(), 22.0));
    let view_w = 170f32.min((head.width() / 2.0).trunc());
    let view_r = Rect::from_min_size(head.min, vec2(view_w, 22.0));
    let rest = head.width() - view_w - 8.0;
    let freq_r = if slim {
        // The second row is the view's own head (the legend, THD) with the tone tabs on its left.
        let row = Rect::from_min_size(pos2(head.left(), head.bottom() + 6.0), vec2(126f32.min(head.width() / 2.0), 22.0));
        b.min.y = row.top();
        row
    } else {
        Rect::from_min_size(pos2(view_r.right() + 8.0, head.top()), vec2(170f32.min(rest - 90.0).max(0.0), 22.0))
    };
    let head = Rect::from_min_size(b.min, vec2(b.width(), 22.0));
    if !slim && let Some(i) = segmented(ui, view_r, ("tape-view", key), pal, &["Response", "Harmonics"], 1, Some(st.view), SelectionStyle::Filled) {
        st.view = i;
    }
    if st.view == 1
        && let Some(i) = segmented(ui, freq_r, ("tape-tone", key), pal, &["100 Hz", "1 kHz", "5 kHz"], 1, Some(st.freq_index), SelectionStyle::Filled)
    {
        st.freq_index = i;
    }

    // setParams + schedule + the worker.
    let mut values = [0.0f32; NUM_PARAMS + 2];
    values[..NUM_PARAMS].copy_from_slice(params);
    values[NUM_PARAMS] = st.view as f32;
    values[NUM_PARAMS + 1] = if st.view == 1 { st.freq_index as f32 } else { 0.0 };
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
    let job_key = key.with(st.view);
    if let Some((h, at)) = st.want
        && st.requested != Some(h)
    {
        if now - at >= 0.12 {
            w.request(job_key, h, &values, 0.0, &[0.0; OUT_LEN]);
            st.requested = Some(h);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((0.12 - (now - at)).max(0.0)));
        }
    }
    // A result is drawn only if it is the latest request's (a stale one is dropped).
    for v in 0..2 {
        if let Some((h, c)) = w.take(key.with(v))
            && Some(h) == st.requested
        {
            if v == 0 {
                st.response = Some(c);
            } else {
                st.harmonics = Some(c);
            }
        }
    }

    let content = Rect::from_min_max(pos2(b.left(), b.top() + 22.0 + 6.0), b.max);
    if st.view == 0 {
        paint_response(&p, head, content, pal, st.response.as_deref());
    } else {
        paint_harmonics(&p, head, content, pal, st.harmonics.as_deref());
    }
}

fn paint_response(p: &egui::Painter, head: Rect, content: Rect, pal: &VstPalette, resp: Option<&Vec<f32>>) {
    let caption = Rect::from_min_max(pos2(content.left(), content.bottom() - 14.0), content.max);
    let labels = Rect::from_min_max(pos2(content.left(), caption.top() - 14.0), pos2(content.right(), caption.top()));
    let plot = Rect::from_min_max(pos2(content.left() + 30.0, content.top()), pos2(content.right(), labels.top()));

    // The legend on the header row, right-aligned.
    let mut lx = head.right() - 150.0;
    for (c, t) in [(al(pal.text_body, 0.7), "\u{2212}20 dB"), (pal.accent, "0 VU")] {
        let sw = Rect::from_center_size(pos2(lx + 7.0, head.center().y), vec2(12.0, 3.0));
        p.rect_filled(sw, CornerRadius::same(1), c);
        p.text(pos2(lx + 19.0, head.center().y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
        lx += 75.0;
    }

    for db in [6, 0, -6, -12] {
        let y = y_for(f64::from(db), plot).round();
        p.line_segment(
            [pos2(plot.left(), y + 0.5), pos2(plot.right(), y + 0.5)],
            Stroke::new(1.0, al(pal.text_faint, if db == 0 { 0.35 } else { 0.15 })),
        );
        let t = format!("{}{db}", if db > 0 { "+" } else { "" });
        p.text(pos2(plot.left() - 4.0, y), Align2::RIGHT_CENTER, t, regular(TINY_SIZE), pal.text_faint);
    }
    for f in [50.0, 100.0, 500.0, 1000.0, 5000.0, 10000.0] {
        let x = x_for(f, plot).round();
        p.line_segment([pos2(x + 0.5, plot.top()), pos2(x + 0.5, plot.bottom())], Stroke::new(1.0, al(pal.text_faint, 0.12)));
        let t = if f >= 1000.0 { format!("{}k", (f / 1000.0) as i32) } else { format!("{}", f as i32) };
        p.text(pos2(x, labels.center().y), Align2::CENTER_CENTER, t, regular(CAPTION_SIZE), pal.text_faint);
    }

    if let Some(c) = resp.filter(|c| c.len() >= OUT_LEN) {
        for pass in 0..2 {
            let pts: Vec<Pos2> = (0..K_TONES)
                .filter(|t| pass == 0 || is_hot_point(*t))
                .map(|t| pos2(x_for(tone_hz(t), plot), y_for(f64::from(c[pass * K_TONES + t]), plot)))
                .collect();
            let stroke = if pass == 0 { Stroke::new(1.5, al(pal.text_body, 0.7)) } else { Stroke::new(2.0, pal.accent) };
            p.add(Shape::line(pts, stroke));
        }
    }

    p.with_clip_rect(caption).text(
        pos2(caption.left(), caption.center().y),
        Align2::LEFT_CENTER,
        "Each curve against its own 1 kHz: the gap is what your level compresses",
        regular(CAPTION_SIZE),
        pal.text_faint,
    );
}

fn paint_harmonics(p: &egui::Painter, head: Rect, content: Rect, pal: &VstPalette, h: Option<&Vec<f32>>) {
    let measured = h.filter(|c| c.len() >= 17);
    let thd = measured.map_or_else(|| "...".to_string(), |c| format!("THD {}", format_percent(f64::from(c[0]))));
    p.text(pos2(head.right(), head.center().y), Align2::RIGHT_CENTER, thd, font(VALUE_SIZE, true), pal.text_bright);

    let caption = Rect::from_min_max(pos2(content.left(), content.bottom() - 14.0), content.max);
    let labels = Rect::from_min_max(pos2(content.left(), caption.top() - 14.0), pos2(content.right(), caption.top()));
    p.with_clip_rect(caption).text(
        pos2(caption.left(), caption.center().y),
        Align2::LEFT_CENTER,
        "Sine at 0 VU through your Drive, each harmonic against the fundamental",
        regular(CAPTION_SIZE),
        pal.text_faint,
    );
    let plot = Rect::from_min_max(content.min, pos2(content.right(), labels.top()));
    for db in (0..=100i32).step_by(20).map(|d| -d) {
        let y = (plot.top() + (f64::from(-db) / 100.0) as f32 * plot.height()).round();
        p.line_segment(
            [pos2(plot.left() + 30.0, y + 0.5), pos2(plot.right(), y + 0.5)],
            Stroke::new(1.0, al(pal.text_faint, if db == 0 { 0.35 } else { 0.15 })),
        );
        p.text(pos2(plot.left() + 26.0, y), Align2::RIGHT_CENTER, db.to_string(), regular(TINY_SIZE), pal.text_faint);
    }
    let bars = Rect::from_min_max(pos2(plot.left() + 34.0, plot.top()), plot.max);
    let slot = bars.width() / 8.0;
    let bar_w = 26f32.min(slot * 0.55);
    for k in 2..=9usize {
        let cx = bars.left() + slot * ((k - 2) as f32 + 0.5);
        p.text(pos2(cx, labels.center().y), Align2::CENTER_CENTER, format!("H{k}"), regular(CAPTION_SIZE), pal.text_faint);
        let Some(c) = measured else { continue };
        let dv = f64::from(c[1 + k - 2]);
        if dv <= -100.0 || c[9 + k - 2] < 0.5 {
            continue;
        }
        let y = plot.top() + (-dv / 100.0).clamp(0.0, 1.0) as f32 * plot.height();
        let colour = if k % 2 == 0 { pal.accent } else { pal.text_body };
        p.rect_filled(Rect::from_min_max(pos2(cx - bar_w * 0.5, y), pos2(cx + bar_w * 0.5, plot.bottom())), CornerRadius::same(2), al(colour, 0.85));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn defaults() -> Vec<f32> {
        vec![1.0, 0.0, 1.0, 0.3, 0.5, 0.0, 0.0, 1.0, 0.5, 1.0, 1.0, 0.0]
    }

    #[test]
    fn response_is_flat_at_1k_and_finite() {
        let out = measure_response(&defaults());
        assert_eq!(out.len(), OUT_LEN);
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(out[K_REF].abs() < 1e-4 && out[K_TONES + K_REF].abs() < 1e-4, "each curve against its own 1 kHz");
    }

    #[test]
    fn harmonics_and_hostile_values() {
        let out = measure_harmonics(&defaults(), 1);
        assert!(out[0] > 0.0 && out[0].is_finite());
        let v = vec![f32::NAN; NUM_PARAMS + 2];
        assert!(measure(&v).iter().all(|x| x.is_finite()));
    }
}

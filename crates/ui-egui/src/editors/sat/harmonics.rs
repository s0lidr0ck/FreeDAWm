//! `SatHarmonics.h` (`fs::sat::HarmonicsView`): the editor's harmonic signature. A private
//! SatEngine renders a test tone whenever a setting changes, and H2..H9 are drawn against the
//! fundamental. The measurement runs on the curve worker (off the UI thread), 120 ms after the
//! last change (`schedule`).

use crate::curve_worker::CurveWorker;
use crate::editor_kit::controls::{SelectionStyle, segmented};
use crate::editor_kit::look::{CAPTION_SIZE, TINY_SIZE, VALUE_SIZE, font, paint_panel};
use crate::editor_kit::palette::{VstPalette, al};
use crate::theme::regular;
use egui::{Align2, CornerRadius, Id, Rect, Stroke, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::sat::sat_engine_dsp::{NUM_PARAMS, P_BYPASS, P_CHAN_MODE, SatEngineDsp};
use std::sync::Arc;

/// The signature's caption (the slim layout puts it on its display toggle's hover).
pub(super) const CAPTION: &str = "Sine at \u{2212}12 dBFS  \u{00B7}  each harmonic against the fundamental";

/// The three test tones (`freqs`).
const FREQS: [f64; 3] = [100.0, 1000.0, 5000.0];
/// What a measurement returns: THD, then H2..H9 levels (dB), then H2..H9 reachable (0/1).
const OUT_LEN: usize = 17;

/// `measure()`: `values` are the engine's parameters (by index) and the tone's index last.
/// Returns `[thd, level 2..=9, reachable 2..=9]`.
pub(crate) fn measure(values: &[f32]) -> Vec<f32> {
    const SR: f64 = 96_000.0;
    let fi = values.get(NUM_PARAMS).copied().unwrap_or(1.0);
    let fi = if fi.is_finite() { (fi.round() as i64).clamp(0, 2) as usize } else { 1 };
    let f = FREQS[fi];

    let mut d = Box::new(SatEngineDsp::new());
    for i in 0..NUM_PARAMS {
        let v = values.get(i).copied().unwrap_or(0.0);
        d.set_parameter(i, if v.is_finite() { v } else { 0.0 });
    }
    d.set_parameter(P_BYPASS, 0.0);
    d.set_parameter(P_CHAN_MODE, 0.0); // one channel: M/S is irrelevant here
    d.prepare(SR);

    let total = (0.6 * SR) as usize;
    let window = (0.2 * SR) as usize;
    let mut buffer: Vec<f32> = (0..total).map(|i| (0.25 * (2.0 * std::f64::consts::PI * f * i as f64 / SR).sin()) as f32).collect();
    let mut block = vec![vec![0.0f32; 512]];
    let mut done = 0;
    while done < total {
        let n = 512.min(total - done);
        block[0][..n].copy_from_slice(&buffer[done..done + n]);
        d.process(&mut block, 1, n);
        buffer[done..done + n].copy_from_slice(&block[0][..n]);
        done += 512;
    }

    let from = total - window;
    let fund = amplitude(&buffer, from, window, f, SR);
    let mut sum = 0.0;
    let mut out = vec![0.0f32; OUT_LEN];
    for k in 2..=9usize {
        let reachable = f * k as f64 > 0.0 && f * (k as f64) < 0.45 * SR;
        out[1 + k - 2] = -200.0;
        out[9 + k - 2] = if reachable { 1.0 } else { 0.0 };
        if !reachable || fund <= 0.0 {
            continue;
        }
        let a = amplitude(&buffer, from, window, f * k as f64, SR);
        sum += a * a;
        out[1 + k - 2] = (20.0 * (a / fund).max(1.0e-12).log10()) as f32;
    }
    out[0] = if fund > 0.0 { (sum.sqrt() / fund) as f32 } else { 0.0 };
    out
}

/// A Hann-windowed DFT bin's amplitude at `f`.
fn amplitude(buffer: &[f32], from: usize, n: usize, f: f64, sr: f64) -> f64 {
    let (mut re, mut im, mut wsum) = (0.0, 0.0, 0.0);
    for i in 0..n {
        let w = 0.5 - 0.5 * (2.0 * std::f64::consts::PI * i as f64 / (n - 1) as f64).cos();
        let ph = 2.0 * std::f64::consts::PI * f * i as f64 / sr;
        let x = f64::from(buffer.get(from + i).copied().unwrap_or(0.0));
        re += w * x * ph.cos();
        im += w * x * ph.sin();
        wsum += w;
    }
    2.0 * (re * re + im * im).sqrt() / wsum
}

/// `formatPercent`.
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

/// `yFor`: 0 dB at the top, −100 at the floor.
fn y_for(db: f64, plot: Rect) -> f32 {
    let t = (-db / 100.0).clamp(0.0, 1.0);
    plot.top() + t as f32 * plot.height()
}

/// The worker that measures Sat's harmonics, one per context.
fn worker(ctx: &egui::Context) -> Arc<CurveWorker> {
    let id = Id::new("sat-harmonics-worker");
    if let Some(w) = ctx.data(|d| d.get_temp::<Arc<CurveWorker>>(id)) {
        return w;
    }
    let repaint = ctx.clone();
    let w = Arc::new(CurveWorker::new(Box::new(|v, _sr, _f| measure(v)), Box::new(move || repaint.request_repaint())));
    ctx.data_mut(|d| d.insert_temp(id, Arc::clone(&w)));
    w
}

/// The view's state (kept by the editor).
#[derive(Clone, Debug)]
pub(crate) struct View {
    pub freq_index: usize,
    /// The inputs now, and since when (`schedule`: measure 120 ms after the last change).
    want: Option<(u64, f64)>,
    requested: Option<u64>,
    result: Option<Arc<Vec<f32>>>,
}

impl Default for View {
    fn default() -> Self {
        View { freq_index: 1, want: None, requested: None, result: None }
    }
}

/// The view's inner area in `r` (the slim layout puts its display toggle at its top-left).
pub(crate) fn inner(r: Rect) -> Rect {
    r.shrink2(vec2(12.0, 10.0))
}

/// Paints the view in `r` and handles its tone tabs. `params`: every engine parameter by index.
/// `lead`: the slim layout's display toggle takes that much of the tab row's left; the tone tabs
/// follow it and THD moves to the caption line.
pub(crate) fn draw_ex(ui: &mut Ui, r: Rect, pal: &VstPalette, view: &mut View, params: &[f32; NUM_PARAMS], key: Id, lead: Option<f32>) {
    let p = ui.painter().clone();
    paint_panel(&p, r, 10.0);

    // resized(): the tabs.
    let b = inner(r);
    let top = Rect::from_min_size(b.min, vec2(b.width(), 22.0));
    let seg_r = match lead {
        Some(w) => Rect::from_min_max(pos2(top.left() + w + 8.0, top.top()), top.max),
        None => Rect::from_min_size(top.min, vec2(180f32.min((top.width() / 2.0).trunc()), 22.0)),
    };
    if let Some(i) = segmented(ui, seg_r, ("sat-tone", key), pal, &["100 Hz", "1 kHz", "5 kHz"], 1, Some(view.freq_index), SelectionStyle::Filled) {
        view.freq_index = i;
    }

    // setParams + schedule + the worker.
    let mut values = [0.0f32; NUM_PARAMS + 1];
    values[..NUM_PARAMS].copy_from_slice(params);
    values[NUM_PARAMS] = view.freq_index as f32;
    let hash = {
        use std::hash::{Hash, Hasher};
        let mut h = std::hash::DefaultHasher::new();
        values.iter().for_each(|v| v.to_bits().hash(&mut h));
        h.finish()
    };
    let now = ui.input(|i| i.time);
    if view.want.is_none_or(|(h, _)| h != hash) {
        view.want = Some((hash, now));
    }
    let w = worker(ui.ctx());
    if let Some((h, at)) = view.want
        && view.requested != Some(h)
    {
        if now - at >= 0.12 {
            w.request(key, h, &values, 96_000.0, &[0.0; OUT_LEN]);
            view.requested = Some(h);
        } else {
            ui.ctx().request_repaint_after(std::time::Duration::from_secs_f64((0.12 - (now - at)).max(0.0)));
        }
    }
    if let Some((_, c)) = w.take(key) {
        view.result = Some(c);
    }

    // paint()
    let measured = view.result.as_ref().filter(|c| c.len() >= OUT_LEN);
    let thd = measured.map_or_else(|| "...".to_string(), |c| format!("THD {}", format_percent(f64::from(c[0]))));
    if lead.is_none() {
        let thd_r = Rect::from_min_max(pos2(top.right() - 120.0, top.top()), top.max);
        p.text(pos2(thd_r.right(), thd_r.center().y), Align2::RIGHT_CENTER, &thd, font(VALUE_SIZE, true), pal.text_bright);
    }

    let body = Rect::from_min_max(pos2(b.left(), top.bottom() + 6.0), b.max);
    let caption = Rect::from_min_max(pos2(body.left(), body.bottom() - 14.0), body.max);
    let labels = Rect::from_min_max(pos2(body.left(), caption.top() - 14.0), pos2(body.right(), caption.top()));
    let plot = Rect::from_min_max(body.min, pos2(body.right(), labels.top()));

    // Legend, right-aligned on the caption line.
    let mut lx = caption.right() - 100.0;
    for (c, t) in [(pal.accent, "even"), (pal.text_body, "odd")] {
        let sw = Rect::from_center_size(pos2(lx + 5.0, caption.center().y), vec2(9.0, 9.0));
        p.rect_filled(sw, CornerRadius::same(2), c);
        lx += 14.0;
        p.text(pos2(lx, caption.center().y), Align2::LEFT_CENTER, t, regular(CAPTION_SIZE), pal.text_body);
        lx += 34.0;
    }
    let cap_text = Rect::from_min_max(caption.min, pos2(caption.right() - 100.0, caption.bottom()));
    if lead.is_some() {
        p.with_clip_rect(cap_text).text(
            pos2(cap_text.left(), cap_text.center().y),
            Align2::LEFT_CENTER,
            &thd,
            font(VALUE_SIZE, true),
            pal.text_bright,
        );
    } else {
        p.with_clip_rect(cap_text).text(
            pos2(cap_text.left(), cap_text.center().y),
            Align2::LEFT_CENTER,
            CAPTION,
            regular(CAPTION_SIZE),
            pal.text_faint,
        );
    }

    // Scale: 0 dB at the top, −100 at the floor, a faint line every 20 dB.
    for db in (0..=100i32).step_by(20).map(|d| -d) {
        let y = y_for(f64::from(db), plot).round();
        p.line_segment(
            [pos2(plot.left() + 30.0, y + 0.5), pos2(plot.right(), y + 0.5)],
            Stroke::new(1.0, al(pal.text_faint, if db == 0 { 0.35 } else { 0.15 })),
        );
        let lr = Rect::from_min_size(pos2(plot.left(), y - 6.0), vec2(26.0, 12.0));
        p.text(pos2(lr.right(), lr.center().y), Align2::RIGHT_CENTER, db.to_string(), regular(TINY_SIZE), pal.text_faint);
    }

    let bars = Rect::from_min_max(pos2(plot.left() + 34.0, plot.top()), plot.max);
    let slot = bars.width() / 8.0;
    let bar_w = 26f32.min(slot * 0.55);
    for k in 2..=9usize {
        let cx = bars.left() + slot * ((k - 2) as f32 + 0.5);
        let colour = if k % 2 == 0 { pal.accent } else { pal.text_body };
        p.text(pos2(cx, labels.center().y), Align2::CENTER_CENTER, format!("H{k}"), regular(CAPTION_SIZE), pal.text_faint);
        let Some(c) = measured else { continue };
        let d = f64::from(c[1 + k - 2]);
        if d <= -100.0 || c[9 + k - 2] < 0.5 {
            continue;
        }
        let y = y_for(d, plot);
        let bar = Rect::from_min_max(pos2(cx - bar_w * 0.5, y), pos2(cx + bar_w * 0.5, plot.bottom()));
        p.rect_filled(bar, CornerRadius::same(2), al(colour, 0.85));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn valve_triode_is_mostly_even() {
        // The defaults (Valve, Triode, drive 0.4) at 1 kHz.
        let mut v = vec![1.0, 0.0, 0.4, 0.5, 0.5, 0.0, 1.0, 0.5, 1.0, 0.0, 1.0, 0.0];
        v.push(1.0);
        let out = measure(&v);
        assert_eq!(out.len(), OUT_LEN);
        assert!(out.iter().all(|x| x.is_finite()));
        assert!(out[0] > 0.0 && out[0] < 1.0, "THD {}", out[0]);
        assert!(out[1] > out[2] - 30.0, "H2 is present: {out:?}");
        // 5 kHz: H9 is out of reach at 96 kHz (45 kHz > 43.2 kHz).
        v[NUM_PARAMS] = 2.0;
        let out = measure(&v);
        assert_eq!(out[9 + 9 - 2], 0.0);
        assert_eq!(out[9 + 8 - 2], 1.0);
    }

    #[test]
    fn hostile_values_measure_finite() {
        let v = vec![f32::NAN; NUM_PARAMS + 1];
        assert!(measure(&v).iter().all(|x| x.is_finite()));
    }
}

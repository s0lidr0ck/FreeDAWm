//! `CompCrossover.h` / `TwineCrossover.h` `CrossoverStrip` (the same component in both engines,
//! with its own band names and defaults, [`Spec`]): where Multiband splits the signal, and the
//! handles to move it. 20 Hz .. 20 kHz on a log scale, the three bands shaded, a divider at each crossover (drag,
//! wheel, double-click), the input's spectrum behind them and each band's gain reduction hanging
//! from the top. Dimmed but live outside Multiband.

use super::palette::{al, argb};
use super::{VstPalette, consume_wheel, juce_wheel_delta_y};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};

/// What differs between the engines' strips.
#[derive(Clone, Copy, Debug)]
pub struct Spec {
    pub names: [&'static str; 3],
    pub default_lo: f32,
    pub default_hi: f32,
}

/// `knobToXoverLo`: 40 Hz .. 1 kHz (both engines).
fn knob_to_xover_lo(v: f32) -> f32 {
    40.0 * 25f32.powf(v.clamp(0.0, 1.0))
}
/// `knobToXoverHi`: 800 Hz .. 12 kHz (both engines).
fn knob_to_xover_hi(v: f32) -> f32 {
    800.0 * 15f32.powf(v.clamp(0.0, 1.0))
}
const F_MIN: f32 = 20.0;
const F_MAX: f32 = 20000.0;
/// The spectrum's FFT size (`updateSpectrum`).
pub const N: usize = 2048;

fn xover_lo_to_knob(hz: f32) -> f32 {
    (hz / 40.0).ln() / 25f32.ln()
}
fn xover_hi_to_knob(hz: f32) -> f32 {
    (hz / 800.0).ln() / 15f32.ln()
}

/// `hzText`.
pub fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 {
        let k = hz / 1000.0;
        if k < 10.0 { format!("{k:.1}k") } else { format!("{k:.0}k") }
    } else {
        format!("{}", hz.round() as i32)
    }
}

/// The input's spectrum (`scopeBuf`, `fftBuf`, `spectrumDb`), kept between frames.
#[derive(Clone, Default)]
pub struct Spectrum {
    scope: Vec<f32>,
    re: Vec<f32>,
    im: Vec<f32>,
    pub db: Vec<f32>,
    pub bin_hz: f64,
}

impl Spectrum {
    /// `updateSpectrum`: the last 2048 input samples, Hann-windowed, peaks held and falling back
    /// 1 dB a tick.
    pub fn update(&mut self, ring: &soundcraft_dsp::analysis::AnalysisRing, sr: f64) {
        self.scope.resize(N, 0.0);
        self.re.resize(N, 0.0);
        self.im.resize(N, 0.0);
        ring.latest(&mut self.scope);
        for i in 0..N {
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / N as f32).cos();
            self.re[i] = self.scope[i] * w;
            self.im[i] = 0.0;
        }
        fft(&mut self.re, &mut self.im);
        if self.db.len() != N / 2 {
            self.db = vec![-200.0; N / 2];
        }
        let norm = 4.0 / N as f32; // Hann's coherent gain, and one side
        for k in 1..N / 2 {
            let mag = (self.re[k] * self.re[k] + self.im[k] * self.im[k]).sqrt();
            let db = 20.0 * (mag * norm + 1.0e-9).log10();
            let held = &mut self.db[k];
            *held = if db > *held { db } else { *held - 1.0 };
        }
        self.bin_hz = sr / N as f64;
    }
}

/// `CrossoverStrip::fft`: radix-2, in place.
fn fft(re: &mut [f32], im: &mut [f32]) {
    let n = re.len().min(im.len());
    let mut j = 0usize;
    for i in 1..n {
        let mut bit = n >> 1;
        while j & bit != 0 {
            j ^= bit;
            bit >>= 1;
        }
        j ^= bit;
        if i < j {
            re.swap(i, j);
            im.swap(i, j);
        }
    }
    let mut len = 2;
    while len <= n {
        let ang = -2.0 * std::f32::consts::PI / len as f32;
        let (wr, wi) = (ang.cos(), ang.sin());
        let mut i = 0;
        while i < n {
            let (mut cr, mut ci) = (1.0f32, 0.0f32);
            for k in 0..len / 2 {
                let (a, b) = (i + k, i + k + len / 2);
                let (vr, vi) = (re[b] * cr - im[b] * ci, re[b] * ci + im[b] * cr);
                let (ur, ui) = (re[a], im[a]);
                re[a] = ur + vr;
                im[a] = ui + vi;
                re[b] = ur - vr;
                im[b] = ui - vi;
                let t = cr * wr - ci * wi;
                ci = cr * wi + ci * wr;
                cr = t;
            }
            i += len;
        }
        len <<= 1;
    }
}

/// What the strip shows.
pub struct View<'a> {
    pub spec: Spec,
    pub lo_knob: f32,
    pub hi_knob: f32,
    pub active: bool,
    pub band_gr: [f32; 3],
    pub spectrum: &'a Spectrum,
}

/// The strip's gestures this frame (`onGestureStart`, `onValue`, `onGestureEnd`).
#[derive(Default, Debug)]
pub struct Out {
    pub gesture_start: Option<usize>,
    pub value: Option<(usize, f32)>,
    pub gesture_end: Option<usize>,
}

struct Geo {
    plot: Rect,
}
impl Geo {
    fn x_of(&self, hz: f32) -> f32 {
        self.plot.left() + self.plot.width() * (hz / F_MIN).ln() / (F_MAX / F_MIN).ln()
    }
    fn hz_at(&self, x: f32) -> f32 {
        let t = ((x - self.plot.left()) / self.plot.width().max(1.0)).clamp(0.0, 1.0);
        F_MIN * (F_MAX / F_MIN).powf(t)
    }
}

fn lo_hz(lo: f32) -> f32 {
    knob_to_xover_lo(lo)
}
fn hi_hz(lo: f32, hi: f32) -> f32 {
    knob_to_xover_hi(hi).max(lo_hz(lo) * 2.0)
}

/// The strip in `r`; `dragging` is the divider being dragged (kept by the editor).
pub fn crossover(ui: &mut Ui, r: Rect, pal: &VstPalette, v: &View, dragging: &mut Option<usize>) -> Out {
    let mut out = Out::default();
    let lo = if v.lo_knob.is_finite() { v.lo_knob.clamp(0.0, 1.0) } else { v.spec.default_lo };
    let hi = if v.hi_knob.is_finite() { v.hi_knob.clamp(0.0, 1.0) } else { v.spec.default_hi };
    let g = Geo { plot: r.shrink2(vec2(4.0, 3.0)) };
    let (x1, x2) = (g.x_of(lo_hz(lo)), g.x_of(hi_hz(lo, hi)));
    let divider_at = |x: f32| {
        let (d0, d1) = ((x - x1).abs(), (x - x2).abs());
        if d0.min(d1) > 6.0 { None } else { Some(if d0 <= d1 { 0 } else { 1 }) }
    };
    let nearest = |x: f32| if (x - x1).abs() <= (x - x2).abs() { 0 } else { 1 };
    let drag_to = |d: usize, x: f32| {
        let hz = g.hz_at(x);
        let v = if d == 0 { xover_lo_to_knob(hz.clamp(40.0, 1000.0)) } else { xover_hi_to_knob(hz.clamp(800.0, 12000.0)) };
        v.clamp(0.0, 1.0)
    };

    let resp = ui.interact(r, ui.id().with("vst-crossover"), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Crossover"));
    let hover = resp.hover_pos().and_then(|p| divider_at(p.x));
    if hover.is_some() || dragging.is_some() {
        ui.ctx().set_cursor_icon(CursorIcon::ResizeHorizontal);
    }
    let (pressed, released, pointer) = ui.input(|i| (i.pointer.primary_pressed(), i.pointer.primary_released(), i.pointer.interact_pos()));
    if pressed
        && resp.hovered()
        && let Some(p) = pointer
    {
        // grab the nearer divider even when the click missed it
        let d = divider_at(p.x).unwrap_or_else(|| nearest(p.x));
        *dragging = Some(d);
        out.gesture_start = Some(d);
        out.value = Some((d, drag_to(d, p.x)));
    } else if let Some(d) = *dragging
        && ui.input(|i| i.pointer.primary_down())
        && let Some(p) = pointer
    {
        out.value = Some((d, drag_to(d, p.x)));
    }
    if released && let Some(d) = dragging.take() {
        out.gesture_end = Some(d);
    }
    if resp.double_clicked()
        && let Some(p) = resp.interact_pointer_pos()
    {
        let d = nearest(p.x);
        out.gesture_start = Some(d);
        out.value = Some((d, if d == 0 { v.spec.default_lo } else { v.spec.default_hi }));
        out.gesture_end = Some(d);
    }
    if resp.hovered()
        && let Some(p) = resp.hover_pos()
    {
        let dy = juce_wheel_delta_y(ui);
        if dy != 0.0 {
            consume_wheel(ui);
            let d = nearest(p.x);
            let now = if d == 0 { lo } else { hi };
            let step = (if dy > 0.0 { 1.0 } else { -1.0 }) * if ui.input(|i| i.modifiers.shift) { 0.004 } else { 0.02 };
            out.gesture_start = Some(d);
            out.value = Some((d, (now + step).clamp(0.0, 1.0)));
            out.gesture_end = Some(d);
        }
    }

    // ---- paint (with the value just set, as the attachment repaints at once)
    let (lo, hi) = match out.value {
        Some((0, nv)) => (nv, hi),
        Some((_, nv)) => (lo, nv),
        None => (lo, hi),
    };
    let (x1, x2) = (g.x_of(lo_hz(lo)), g.x_of(hi_hz(lo, hi)));
    let p = ui.painter();
    let plot = g.plot;
    let dim = if v.active { 1.0 } else { 0.45 };
    let accent = pal.accent;
    p.rect_filled(r, CornerRadius::same(6), al(Color32::BLACK, 0.22));
    let edges = [plot.left(), x1, x2, plot.right()];
    let names = v.spec.names;
    for i in 0..3 {
        let b = Rect::from_min_max(pos2(edges[i], plot.top()), pos2(edges[i + 1], plot.bottom()));
        p.rect_filled(b, 0.0, al(accent, (0.06 + 0.06 * i as f32) * dim));
        if b.width() > 30.0 {
            let t = Rect::from_min_size(pos2(b.left(), b.top() + 2.0), vec2(b.width(), 12.0));
            p.text(t.center(), Align2::CENTER_CENTER, names[i], regular(9.5), al(pal.text_dim, dim));
        }
    }
    // ---- the input's spectrum, behind everything else
    let s = v.spectrum;
    if !s.db.is_empty() && s.bin_hz > 0.0 {
        let top = s.db.iter().copied().fold(-200.0f32, f32::max);
        let (reference, range) = (top.max(-60.0), 54.0);
        let last = s.db.len() - 1;
        let mut pts: Vec<Pos2> = Vec::with_capacity(plot.width() as usize + 2);
        let mut x = plot.left();
        while x <= plot.right() {
            let (f0, f1) = (f64::from(g.hz_at(x)), f64::from(g.hz_at(x + 1.0)));
            let k0 = ((f0 / s.bin_hz).floor() as usize).clamp(1, last);
            let k1 = ((f1 / s.bin_hz).ceil() as usize).clamp(k0, last);
            let v = s.db[k0..=k1].iter().copied().fold(-200.0f32, f32::max);
            let t = ((v - (reference - range)) / range).clamp(0.0, 1.0);
            pts.push(pos2(x, plot.bottom() - t * plot.height() * 0.9));
            x += 1.0;
        }
        crate::editor_kit::icons::fill_strip(p, &pts, |_| plot.bottom(), al(pal.text_bright, 0.10));
        let mut line = Vec::with_capacity(pts.len() + 2);
        line.push(pos2(plot.left(), plot.bottom()));
        line.extend_from_slice(&pts);
        line.push(pos2(plot.right(), plot.bottom()));
        p.add(egui::Shape::line(line, Stroke::new(1.0, al(pal.text_bright, 0.30))));
    }
    // ---- each band's gain reduction, hanging from the top (Multiband only)
    if v.active {
        for i in 0..3 {
            let gr = v.band_gr[i];
            if !gr.is_finite() || gr < 0.1 {
                continue;
            }
            let b = Rect::from_min_size(
                pos2(edges[i] + 1.0, plot.top()),
                vec2(edges[i + 1] - edges[i] - 2.0, plot.height() * (gr / 12.0).clamp(0.0, 1.0)),
            );
            p.rect_filled(b, 0.0, al(accent, 0.35));
            if b.width() > 28.0 {
                let t = Rect::from_min_size(pos2(b.left(), plot.center().y - 6.0), vec2(b.width(), 12.0));
                p.text(t.center(), Align2::CENTER_CENTER, format!("-{gr:.1}"), bold(9.0), pal.text_bright);
            }
        }
    }
    // ---- octave ticks
    for f in [50.0, 100.0, 200.0, 500.0, 1000.0, 2000.0, 5000.0, 10000.0] {
        let x = g.x_of(f).round() + 0.5;
        p.line_segment([pos2(x, plot.bottom() - 4.0), pos2(x, plot.bottom())], Stroke::new(1.0, al(Color32::WHITE, 0.06)));
    }
    // ---- the dividers, with their frequency
    for d in 0..2 {
        let x = if d == 0 { x1 } else { x2 };
        let hot = *dragging == Some(d) || hover == Some(d);
        let w = if hot { 3.0 } else { 2.0 };
        p.rect_filled(
            Rect::from_min_size(pos2(x - w * 0.5, plot.top()), vec2(w, plot.height())),
            0.0,
            al(if hot { pal.text_bright } else { accent }, dim),
        );
        let t = hz_text(if d == 0 { lo_hz(lo) } else { hi_hz(lo, hi) });
        let lw = 34.0;
        let lx = (x - lw * 0.5).clamp(plot.left(), plot.right() - lw);
        let label = Rect::from_min_size(pos2(lx, plot.bottom() - 13.0), vec2(lw, 12.0));
        p.rect_filled(label, CornerRadius::same(3), al(argb(0xFF1C1D20), 0.9));
        p.text(label.center(), Align2::CENTER_CENTER, t, bold(9.5), al(if hot { pal.text_bright } else { pal.text_body }, dim));
    }
    p.rect_stroke(r.shrink(0.5), CornerRadius::same(6), Stroke::new(1.0, al(Color32::WHITE, 0.05)), StrokeKind::Middle);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fft_finds_a_sine_bin() {
        let mut re: Vec<f32> = (0..64).map(|i| (2.0 * std::f32::consts::PI * 4.0 * i as f32 / 64.0).sin()).collect();
        let mut im = vec![0.0; 64];
        fft(&mut re, &mut im);
        let mags: Vec<f32> = re.iter().zip(&im).map(|(r, i)| (r * r + i * i).sqrt()).collect();
        let peak = mags[..32].iter().enumerate().fold((0, 0.0f32), |a, (k, m)| if *m > a.1 { (k, *m) } else { a }).0;
        assert_eq!(peak, 4);
        assert!((mags[4] - 32.0).abs() < 1e-3);
    }

    #[test]
    fn hz_text_as_the_cpp() {
        assert_eq!(hz_text(180.4), "180");
        assert_eq!(hz_text(2500.0), "2.5k");
        assert_eq!(hz_text(12000.0), "12k");
    }
}

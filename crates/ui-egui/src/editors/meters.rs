//! The meter plug-ins' editors (FreeDAWm's own designs, `soundcraft_dsp::plugins::meters`):
//! Spectrum, Loudness Meter, Phase Meter and Tuner. Each is a header, its few settings as
//! segmented switches (every change goes through `mix.module_params`) and the display, drawn
//! from the module's streams and live values while the session plays.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, segmented_ex};
use crate::editor_kit::look::{font, paint_background, paint_panel, paint_title};
use crate::editor_kit::{Host, VstEditor, VstPalette, action, argb, end_gesture, live, param, set_params, set_params_merged};
use crate::module_graphs::freq_to_x;
use egui::{Align2, Color32, Pos2, Rect, Stroke, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::analysis::Analysis;
use soundcraft_dsp::plugins::meters as dsp;
use soundcraft_dsp::spectrum::{Analyzer, bin_hz};
use soundcraft_mix::StripMeter;
use soundcraft_model::{ModuleId, Track};
use std::sync::Arc;

/// The meters' accent: instrument teal.
pub const ACCENT: u32 = 0xFF5F_C8C0;
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(ACCENT));

const W: f32 = 640.0;
const PAD: f32 = 16.0;
const HEADER_H: f32 = 26.0;
const ROW_H: f32 = 22.0;

/// The module's streams while it plays.
fn analysis(host: &Host) -> Option<Arc<Analysis>> {
    host.meters?.analysis.iter().find(|(id, _)| *id == host.ins.id).map(|(_, a)| a.clone())
}

/// A choice parameter as a segmented switch in `r`.
fn choice_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str) {
    let Some(p) = host.info.params.iter().find(|p| p.id == id) else { return };
    let opts: Vec<&str> = p.choices.to_vec();
    let sel = param(host, id).round().max(0.0) as usize;
    if let Some(i) = segmented_ex(ui, r, id, &PALETTE, &opts, 1, Some(sel.min(opts.len().saturating_sub(1))), SelectionStyle::Filled, false) {
        set_params(app, host, &[(id, i as f32)]);
    }
}

/// A toggle parameter as a pill in `r`.
fn toggle_pill(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str, text: &str) {
    let on = param(host, id) > 0.5;
    if pill_ex(ui, r, id, &PALETTE, text, on, PillStyle::Small, PillLook::default()).clicked() {
        set_params(app, host, &[(id, if on { 0.0 } else { 1.0 })]);
    }
}

/// A plain push button in `r`.
fn button(ui: &mut Ui, r: Rect, salt: &str, text: &str) -> bool {
    pill_ex(ui, r, salt, &PALETTE, text, false, PillStyle::Small, PillLook::default()).clicked()
}

/// The editor's frame: background and title; returns the area under the header.
fn frame(ui: &Ui, host: &Host, first: &str, rest: &str) -> Rect {
    let p = ui.painter();
    paint_background(p, host.rect);
    let head = Rect::from_min_size(host.rect.min + vec2(PAD, PAD), vec2(host.rect.width() - 2.0 * PAD, HEADER_H));
    paint_title(p, head, first, rest, &PALETTE);
    Rect::from_min_max(pos2(host.rect.left() + PAD, head.bottom() + 10.0), host.rect.max - vec2(PAD, PAD))
}

fn label(ui: &Ui, at: Pos2, align: Align2, text: &str, size: f32, c: Color32) {
    ui.painter().text(at, align, text, font(size, false), c);
}

fn lufs_text(v: f32) -> String {
    if v.is_finite() && v > -69.95 { format!("{v:.1}") } else { "-inf".into() }
}

// ============================================================================ Spectrum

const SPEC_H: f32 = 300.0;
/// FFT size: 8192 points resolve 6 Hz at 48 kHz.
const FFT: usize = 8192;

#[derive(Clone, Default)]
pub(crate) struct SpectrumEditor {
    an: Analyzer,
    l: Vec<f32>,
    r: Vec<f32>,
    mix: Vec<f32>,
    db: Vec<f32>,
    db2: Vec<f32>,
    /// The smoothed curves and their peak hold, per shown channel.
    smooth: [Vec<f32>; 2],
    hold: [Vec<f32>; 2],
}

pub(crate) fn draw_spectrum(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<SpectrumEditor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for SpectrumEditor {
    const PLUGIN: &'static str = "spectrum";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        vec2(W, PAD * 2.0 + HEADER_H + 10.0 + 2.0 * (ROW_H + 8.0) + SPEC_H)
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        let body = frame(ui, host, "Spectrum", "Analyser");
        let row1 = Rect::from_min_size(body.min, vec2(body.width(), ROW_H));
        choice_row(app, ui, host, row1, "view");
        let row2 = row1.translate(vec2(0.0, ROW_H + 8.0));
        let third = (row2.width() - 16.0) / 3.0;
        choice_row(app, ui, host, Rect::from_min_size(row2.min, vec2(third, ROW_H)), "slope");
        choice_row(app, ui, host, Rect::from_min_size(row2.min + vec2(third + 8.0, 0.0), vec2(third, ROW_H)), "speed");
        toggle_pill(app, ui, host, Rect::from_min_size(row2.min + vec2(2.0 * (third + 8.0), 0.0), vec2(third, ROW_H)), "hold", "Peak hold");
        let plot = Rect::from_min_max(pos2(body.left(), row2.bottom() + 8.0), body.max);
        paint_panel(ui.painter(), plot, 6.0);
        let inner = plot.shrink2(vec2(30.0, 14.0));
        self.paint(ui, host, inner);
    }
}

impl SpectrumEditor {
    #[allow(clippy::needless_range_loop)] // `c` picks the curve, its buffers and its colour
    fn paint(&mut self, ui: &Ui, host: &Host, plot: Rect) {
        let p = ui.painter();
        let (top_db, range_db) = (0.0f32, 96.0f32);
        let y_of = |db: f32| plot.top() + ((top_db - db) / range_db).clamp(0.0, 1.0) * plot.height();
        for db in (0..=96).step_by(12) {
            let y = y_of(-(db as f32));
            p.line_segment([pos2(plot.left(), y), pos2(plot.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(14)));
            label(ui, pos2(plot.left() - 4.0, y), Align2::RIGHT_CENTER, &format!("-{db}"), 9.0, PALETTE.text_faint);
        }
        for hz in [30.0f32, 100.0, 300.0, 1000.0, 3000.0, 10000.0] {
            let x = freq_to_x(plot, hz);
            p.line_segment([pos2(x, plot.top()), pos2(x, plot.bottom())], Stroke::new(1.0, Color32::from_white_alpha(14)));
            let t = if hz >= 1000.0 { format!("{}k", hz / 1000.0) } else { format!("{hz}") };
            label(ui, pos2(x, plot.bottom() + 2.0), Align2::CENTER_TOP, &t, 9.0, PALETTE.text_faint);
        }
        let Some(a) = analysis(host) else {
            label(ui, plot.center(), Align2::CENTER_CENTER, "Play to see the spectrum", 12.0, PALETTE.text_faint);
            return;
        };
        self.l.resize(FFT, 0.0);
        self.r.resize(FFT, 0.0);
        a.pre.latest(&mut self.l);
        a.post.latest(&mut self.r);
        let view = param(host, "view").round() as i32;
        let slope = [0.0f32, 3.0, 4.5][(param(host, "slope").round() as usize).min(2)];
        let keep = [0.6f32, 0.82, 0.93][(param(host, "speed").round() as usize).min(2)];
        let hold = param(host, "hold") > 0.5;
        let sr = host.sr as f32;
        let curves = if view == 5 { 2 } else { 1 };
        for c in 0..curves {
            self.mix.clear();
            let pick = |l: f32, r: f32| match (view, c) {
                (1, _) | (5, 0) => l,
                (2, _) | (5, _) => r,
                (3, _) => 0.5 * (l + r),
                (4, _) => 0.5 * (l - r),
                _ => 0.5 * (l + r),
            };
            self.mix.extend(self.l.iter().zip(&self.r).map(|(l, r)| pick(*l, *r)));
            let out = if c == 0 { &mut self.db } else { &mut self.db2 };
            self.an.magnitude_db_into(&self.mix, FFT, out);
            let sm = &mut self.smooth[c];
            if sm.len() != out.len() {
                sm.clone_from(out);
            }
            for (s, v) in sm.iter_mut().zip(out.iter()) {
                // fall slowly, rise at once
                *s = if *v > *s { *v } else { keep * *s + (1.0 - keep) * v };
            }
            let hd = &mut self.hold[c];
            if hd.len() != sm.len() || !hold {
                hd.clone_from(sm);
            }
            for (h, v) in hd.iter_mut().zip(sm.iter()) {
                *h = h.max(*v);
            }
        }
        let colours = [PALETTE.accent, Color32::from_rgb(0xF0, 0xA0, 0x60)];
        for c in 0..curves {
            for (vals, stroke) in [(&self.smooth[c], Stroke::new(1.5, colours[c])), (&self.hold[c], Stroke::new(1.0, colours[c].gamma_multiply(0.4)))]
            {
                if std::ptr::eq(vals, &self.hold[c]) && !hold {
                    continue;
                }
                let mut pts = Vec::with_capacity(400);
                let mut last_x = f32::NEG_INFINITY;
                for (k, v) in vals.iter().enumerate().skip(1) {
                    let hz = bin_hz(k, FFT, sr);
                    if !(20.0..=20_000.0).contains(&hz) {
                        continue;
                    }
                    let x = freq_to_x(plot, hz);
                    let db = v + slope * (hz / 1000.0).log2();
                    if x - last_x < 1.0 {
                        if let Some(pt) = pts.last_mut() {
                            let pt: &mut Pos2 = pt;
                            pt.y = pt.y.min(y_of(db));
                        }
                        continue;
                    }
                    last_x = x;
                    pts.push(pos2(x, y_of(db)));
                }
                if pts.len() > 1 {
                    p.add(egui::Shape::line(pts, stroke));
                }
            }
        }
        ui.ctx().request_repaint();
    }
}

// ============================================================================ Loudness

const HIST_H: f32 = 200.0;

#[derive(Clone, Default)]
pub(crate) struct LoudnessEditor {
    hist: Vec<f32>,
}

pub(crate) fn draw_loudness(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<LoudnessEditor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for LoudnessEditor {
    const PLUGIN: &'static str = "loudness";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        vec2(W, PAD * 2.0 + HEADER_H + 10.0 + ROW_H + 8.0 + 92.0 + 8.0 + HIST_H)
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        let body = frame(ui, host, "Loudness", "Meter");
        let row = Rect::from_min_size(body.min, vec2(body.width() - 90.0, ROW_H));
        choice_row(app, ui, host, row, "target");
        if button(ui, Rect::from_min_size(pos2(body.right() - 80.0, body.top()), vec2(80.0, ROW_H)), "reset", "Reset") {
            action(app, host, "reset");
        }
        let target = dsp::LOUDNESS_TARGET_LUFS.get(param(host, "target").round().max(0.0) as usize).copied().unwrap_or(-14.0);
        let ceiling = param(host, "tp_ceiling");
        let v = |i: usize| live(host).map_or(f32::NEG_INFINITY, |l| l.get(i));
        let nums = Rect::from_min_size(pos2(body.left(), row.bottom() + 8.0), vec2(body.width(), 92.0));
        paint_panel(ui.painter(), nums, 6.0);
        let integrated = v(dsp::LIVE_INTEGRATED);
        let tp = v(dsp::LIVE_TRUE_PEAK);
        let cells: [(&str, String, Color32); 6] = [
            (
                "Integrated",
                lufs_text(integrated),
                if integrated.is_finite() && (integrated - target).abs() > 1.0 { Color32::from_rgb(0xF0, 0xB0, 0x50) } else { PALETTE.text_bright },
            ),
            ("Short-term", lufs_text(v(dsp::LIVE_SHORT_TERM)), PALETTE.text_bright),
            ("Momentary", lufs_text(v(dsp::LIVE_MOMENTARY)), PALETTE.text_bright),
            ("Max momentary", lufs_text(v(dsp::LIVE_MAX_MOMENTARY)), PALETTE.text_body),
            ("Range (LU)", format!("{:.1}", v(dsp::LIVE_RANGE).max(0.0)), PALETTE.text_body),
            ("True peak (dBTP)", lufs_text(tp), if tp > ceiling { Color32::from_rgb(0xF0, 0x60, 0x50) } else { PALETTE.text_body }),
        ];
        let cw = nums.width() / 6.0;
        for (i, (name, val, c)) in cells.iter().enumerate() {
            let x = nums.left() + cw * (i as f32 + 0.5);
            label(ui, pos2(x, nums.top() + 18.0), Align2::CENTER_CENTER, name, 10.0, PALETTE.text_faint);
            ui.painter().text(pos2(x, nums.top() + 50.0), Align2::CENTER_CENTER, val, font(24.0, true), *c);
        }
        label(
            ui,
            pos2(nums.center().x, nums.bottom() - 10.0),
            Align2::CENTER_CENTER,
            &format!("Target {target:.0} LUFS · ceiling {ceiling:.1} dBTP"),
            10.0,
            PALETTE.text_faint,
        );

        let plot = Rect::from_min_max(pos2(body.left(), nums.bottom() + 8.0), body.max);
        paint_panel(ui.painter(), plot, 6.0);
        let inner = plot.shrink2(vec2(30.0, 10.0));
        let (lo, hi) = (-50.0f32, 0.0f32);
        let y_of = |db: f32| inner.bottom() - ((db - lo) / (hi - lo)).clamp(0.0, 1.0) * inner.height();
        let p = ui.painter();
        for db in (-50..=0).step_by(10) {
            let y = y_of(db as f32);
            p.line_segment([pos2(inner.left(), y), pos2(inner.right(), y)], Stroke::new(1.0, Color32::from_white_alpha(14)));
            label(ui, pos2(inner.left() - 4.0, y), Align2::RIGHT_CENTER, &db.to_string(), 9.0, PALETTE.text_faint);
        }
        let ty = y_of(target);
        p.line_segment([pos2(inner.left(), ty), pos2(inner.right(), ty)], Stroke::new(1.0, PALETTE.accent.gamma_multiply(0.6)));
        if let Some(h) = analysis(host).and_then(|a| a.extra.clone()).and_then(|e| e.downcast::<dsp::LoudnessHistory>().ok()) {
            h.read(&mut self.hist);
        }
        // the last minute, or all of it when there is more, squeezed to the width
        let shown = self.hist.len().max(600);
        let pts: Vec<Pos2> = self
            .hist
            .iter()
            .enumerate()
            .filter(|(_, v)| v.is_finite() && **v > lo - 10.0)
            .map(|(i, v)| pos2(inner.left() + inner.width() * i as f32 / shown as f32, y_of(*v)))
            .collect();
        if pts.len() > 1 {
            p.add(egui::Shape::line(pts, Stroke::new(1.5, PALETTE.accent)));
        }
        label(ui, pos2(inner.right(), inner.top()), Align2::RIGHT_TOP, "Short-term history", 9.0, PALETTE.text_faint);
        ui.ctx().request_repaint();
    }
}

// ============================================================================ Phase

const SCOPE: f32 = 260.0;
const SCOPE_POINTS: usize = 2048;

#[derive(Clone, Default)]
pub(crate) struct PhaseEditor {
    l: Vec<f32>,
    r: Vec<f32>,
}

pub(crate) fn draw_phase(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<PhaseEditor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for PhaseEditor {
    const PLUGIN: &'static str = "phase_meter";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        vec2(W, PAD * 2.0 + HEADER_H + 10.0 + ROW_H + 8.0 + SCOPE + 8.0 + 60.0)
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        let body = frame(ui, host, "Phase", "Meter");
        choice_row(app, ui, host, Rect::from_min_size(body.min, vec2(200.0, ROW_H)), "scope");
        if button(ui, Rect::from_min_size(pos2(body.right() - 110.0, body.top()), vec2(110.0, ROW_H)), "reset", "Reset hold") {
            action(app, host, "reset");
        }
        let scope = Rect::from_center_size(pos2(body.center().x, body.top() + ROW_H + 8.0 + SCOPE / 2.0), vec2(SCOPE, SCOPE));
        paint_panel(ui.painter(), scope, 6.0);
        let p = ui.painter();
        let c = scope.center();
        let rad = SCOPE / 2.0 - 10.0;
        let grid = Stroke::new(1.0, Color32::from_white_alpha(20));
        // M up, S across (mono is vertical), with the L and R diagonals
        p.line_segment([pos2(c.x, c.y - rad), pos2(c.x, c.y + rad)], grid);
        p.line_segment([pos2(c.x - rad, c.y), pos2(c.x + rad, c.y)], grid);
        let d = rad * std::f32::consts::FRAC_1_SQRT_2;
        p.line_segment([pos2(c.x - d, c.y - d), pos2(c.x + d, c.y + d)], grid);
        p.line_segment([pos2(c.x + d, c.y - d), pos2(c.x - d, c.y + d)], grid);
        label(ui, pos2(c.x - d - 4.0, c.y - d - 4.0), Align2::RIGHT_BOTTOM, "L", 10.0, PALETTE.text_faint);
        label(ui, pos2(c.x + d + 4.0, c.y - d - 4.0), Align2::LEFT_BOTTOM, "R", 10.0, PALETTE.text_faint);
        let polar = param(host, "scope").round() as i32 == 1;
        if let Some(a) = analysis(host) {
            self.l.resize(SCOPE_POINTS, 0.0);
            self.r.resize(SCOPE_POINTS, 0.0);
            a.pre.latest(&mut self.l);
            a.post.latest(&mut self.r);
            let peak = self.l.iter().chain(&self.r).fold(1e-4f32, |m, v| m.max(v.abs()));
            let k = rad / (peak * std::f32::consts::SQRT_2).max(0.05);
            let dot = PALETTE.accent.gamma_multiply(0.5);
            for (l, r) in self.l.iter().zip(&self.r) {
                let (m, s) = ((l + r) * std::f32::consts::FRAC_1_SQRT_2, (r - l) * std::f32::consts::FRAC_1_SQRT_2);
                let (x, y) = if polar {
                    // polar: the angle of the point, its distance, folded into the upper half
                    let (rr, th) = ((m * m + s * s).sqrt(), s.atan2(m.abs()));
                    (rr * th.sin(), -(rr * th.cos()))
                } else {
                    (s, -m)
                };
                p.circle_filled(pos2(c.x + x * k, c.y + y * k), 0.8, dot);
            }
        } else {
            label(ui, c, Align2::CENTER_CENTER, "Play to see the scope", 12.0, PALETTE.text_faint);
        }
        // the correlation bar
        let bar = Rect::from_min_size(pos2(body.left() + 40.0, scope.bottom() + 20.0), vec2(body.width() - 80.0, 14.0));
        paint_panel(p, bar, 4.0);
        let x_of = |v: f32| bar.left() + (v.clamp(-1.0, 1.0) + 1.0) * 0.5 * bar.width();
        let corr = live(host).map_or(1.0, |l| l.get(dsp::LIVE_CORRELATION));
        let min = live(host).map_or(1.0, |l| l.get(dsp::LIVE_MIN_CORRELATION));
        let col = if corr < 0.0 { Color32::from_rgb(0xF0, 0x60, 0x50) } else { PALETTE.accent };
        let from = x_of(0.0);
        p.rect_filled(Rect::from_x_y_ranges(from.min(x_of(corr))..=from.max(x_of(corr)), bar.y_range()), 2.0, col);
        p.line_segment([pos2(x_of(min), bar.top() - 3.0), pos2(x_of(min), bar.bottom() + 3.0)], Stroke::new(1.5, Color32::WHITE));
        for (v, t) in [(-1.0f32, "-1"), (0.0, "0"), (1.0, "+1")] {
            label(ui, pos2(x_of(v), bar.bottom() + 3.0), Align2::CENTER_TOP, t, 9.0, PALETTE.text_faint);
        }
        let bal = live(host).map_or(0.0, |l| l.get(dsp::LIVE_BALANCE));
        label(
            ui,
            pos2(body.center().x, bar.top() - 4.0),
            Align2::CENTER_BOTTOM,
            &format!("Correlation {corr:+.2} · lowest {min:+.2} · balance {bal:+.1} dB"),
            10.0,
            PALETTE.text_body,
        );
        ui.ctx().request_repaint();
    }
}

// ============================================================================ Tuner

const TUNER_H: f32 = 170.0;
const TUNER_SAMPLES: usize = 4096;

#[derive(Clone, Default)]
pub(crate) struct TunerEditor {
    buf: Vec<f32>,
    /// The needle, smoothed (cents), and the last note read.
    cents: f32,
    last: Option<dsp::TunerReading>,
}

pub(crate) fn draw_tuner(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<TunerEditor>(app, ui, track, module, meters, |_, _| {});
}

impl VstEditor for TunerEditor {
    const PLUGIN: &'static str = "tuner";

    fn size(&self, _ins: &soundcraft_model::Insert) -> Vec2 {
        vec2(W, PAD * 2.0 + HEADER_H + 10.0 + 70.0 + 8.0 + TUNER_H)
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        let body = frame(ui, host, "Tuner", "");
        let a4 = param(host, "reference");
        let knob = Rect::from_min_size(body.min, vec2(70.0, 70.0));
        let text = format!("{a4:.1} Hz");
        let o = DialOpts {
            label: "A4",
            value_text: &text,
            default: (440.0 - 415.0) / 51.0,
            small: true,
            info: host.info.param("reference"),
            ..DialOpts::default()
        };
        let out = dial(ui, knob, "reference", &PALETTE, (a4 - 415.0) / 51.0, &o);
        if let Some(v) = out.value {
            set_params_merged(app, host, &[("reference", 415.0 + 51.0 * v.clamp(0.0, 1.0))], "tuner-ref");
        }
        if out.ended {
            end_gesture(app);
        }
        toggle_pill(app, ui, host, Rect::from_min_size(pos2(body.right() - 90.0, body.top()), vec2(90.0, ROW_H)), "mute", "Mute");
        let panel = Rect::from_min_max(pos2(body.left(), knob.bottom() + 8.0), body.max);
        paint_panel(ui.painter(), panel, 6.0);
        let reading = analysis(host).and_then(|a| {
            self.buf.resize(TUNER_SAMPLES, 0.0);
            a.post.latest(&mut self.buf);
            dsp::tuner_reading(&self.buf, host.sr as f32, a4)
        });
        if let Some(r) = reading {
            self.cents = if self.last.is_some_and(|l| l.note == r.note) { 0.7 * self.cents + 0.3 * r.cents } else { r.cents };
            self.last = Some(r);
        }
        let p = ui.painter();
        let scale = Rect::from_center_size(pos2(panel.center().x, panel.bottom() - 40.0), vec2(panel.width() - 80.0, 10.0));
        let x_of = |c: f32| scale.center().x + c.clamp(-50.0, 50.0) / 50.0 * scale.width() / 2.0;
        for c in (-50..=50).step_by(10) {
            let h = if c == 0 { 16.0 } else { 8.0 };
            p.line_segment(
                [pos2(x_of(c as f32), scale.center().y - h), pos2(x_of(c as f32), scale.center().y)],
                Stroke::new(1.0, PALETTE.text_faint),
            );
        }
        match (reading, self.last) {
            (Some(_), Some(r)) => {
                let name = dsp::NOTE_NAMES[r.note.rem_euclid(12) as usize];
                let octave = r.note.div_euclid(12) - 1;
                let in_tune = self.cents.abs() < 3.0;
                let col = if in_tune { Color32::from_rgb(0x70, 0xE0, 0x80) } else { PALETTE.text_bright };
                p.text(pos2(panel.center().x, panel.top() + 44.0), Align2::CENTER_CENTER, format!("{name}{octave}"), font(44.0, true), col);
                let x = x_of(self.cents);
                p.line_segment([pos2(x, scale.center().y - 26.0), pos2(x, scale.center().y + 4.0)], Stroke::new(3.0, col));
                label(
                    ui,
                    pos2(panel.center().x, panel.bottom() - 14.0),
                    Align2::CENTER_CENTER,
                    &format!("{:+.0} cents · {:.2} Hz", self.cents, r.hz),
                    11.0,
                    PALETTE.text_body,
                );
                ui.ctx().request_repaint();
            }
            _ => {
                label(ui, pos2(panel.center().x, panel.top() + 44.0), Align2::CENTER_CENTER, "Play a note", 16.0, PALETTE.text_faint);
                if host.meters.is_some() {
                    ui.ctx().request_repaint();
                }
            }
        }
    }
}

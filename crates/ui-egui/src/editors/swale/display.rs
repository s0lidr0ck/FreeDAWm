//! `SwaleDisplay.h`, translated: the screen. Left (40 %) one cycle of the envelope as the settings
//! describe it (the beat marked in Rhythmic, the pick otherwise); right (60 %) about five seconds
//! of history (the VCA opening in front, the detector behind, the trigger / re-arm lines); the mode
//! name, the stage lengths, the mode's line along the bottom and the swell lamp.
//!
//! The history is `swale_ui::SwaleHistory` (the engine's `envOut` / `detOut` readouts at the
//! editor's 30 Hz poll, the ring the strip's graph shares); the shape goes through the dsp port's
//! `shape` and unit maps, so the picture cannot disagree with the sound.

use crate::editor_kit::look::{font, tracked_text};
use crate::editor_kit::palette::{VstPalette, al, rgb};
use crate::swale_ui::{HISTORY_LEN, SwaleHistory};
use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Shape, Stroke, StrokeKind, pos2, vec2};
use soundcraft_dsp::plugins::fs::swale::{swale_config as sc, swale_dsp as sd};

const SCREEN: Color32 = rgb(0x0B0E10);
const SCREEN_LINE: Color32 = rgb(0x1F252B);
const RULE: Color32 = rgb(0x171C21);
const LABEL: Color32 = rgb(0x8A96A3);
const ROW_LABEL: Color32 = rgb(0x5D6874);
const DETECTOR: Color32 = rgb(0x4D5763);
const LAMP_OFF: Color32 = rgb(0x20262C);
/// Narrower than this (the slim editor), the screen drops its mode name.
const NARROW_W: f32 = 400.0;

/// `SwaleDisplay::Settings`.
#[derive(Clone, Copy, Debug, Default)]
pub struct Settings {
    pub mode: i32,
    pub curve: i32,
    pub peak_beat: bool,
    pub phrase: bool,
    pub bypassed: bool,
    pub slow_gear: bool,
    pub sens: f32,
    pub retrig: f32,
    pub floor_amt: f32,
    pub times: sc::StageTimes,
}

impl Settings {
    /// `restingGain`: Floor if it is dialled in, and the Slow Gear FET never quite shuts.
    fn resting_gain(&self) -> f32 {
        let f = if self.floor_amt < 0.005 { 0.0 } else { 10f32.powf(sd::knob_to_floor_db(self.floor_amt) / 20.0) };
        if self.slow_gear { f.max(0.006) } else { f }
    }

    /// `gainAt`: one traversal of the envelope, `t` seconds from the start of the shape.
    fn gain_at(&self, t: f64, sustain: f64) -> f64 {
        let tm = &self.times;
        let (a, h, r) = (tm.attack, tm.hold, tm.release);
        if t < a {
            return sd::shape(self.curve, t / a.max(1.0e-6));
        }
        if t < a + sustain + h {
            return 1.0;
        }
        if t < a + sustain + h + r {
            return sd::shape(self.curve, 1.0 - (t - a - sustain - h) / r.max(1.0e-6));
        }
        0.0
    }
}

/// `drawSingleLineText` at a baseline.
fn single_line(p: &Painter, x: f32, baseline: f32, text: &str, size: f32, c: Color32) {
    p.text(pos2(x, baseline + size * 0.25), Align2::LEFT_BOTTOM, text, font(size, false), c);
}

fn fill_under(p: &Painter, pts: &[Pos2], x0: f32, x1: f32, floor: f32, c: Color32) {
    if pts.len() < 2 {
        return;
    }
    let mut mesh = egui::Mesh::default();
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let i = mesh.vertices.len() as u32;
        mesh.colored_vertex(a, c);
        mesh.colored_vertex(b, c);
        mesh.colored_vertex(pos2(b.x, floor), c);
        mesh.colored_vertex(pos2(a.x, floor), c);
        mesh.add_triangle(i, i + 1, i + 2);
        mesh.add_triangle(i, i + 2, i + 3);
    }
    let _ = (x0, x1);
    p.add(Shape::mesh(mesh));
}

/// `paint`: the screen in `r`. `scratch` is reused for the paths (no per-frame allocation once
/// grown); `lamp`: the swell lamp is lit.
pub fn paint(p: &Painter, r: Rect, pal: &VstPalette, s: &Settings, hist: &SwaleHistory, lamp: bool, scratch: &mut Vec<Pos2>) {
    p.rect_filled(r, CornerRadius::same(4), SCREEN);
    p.rect_stroke(r.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, SCREEN_LINE), StrokeKind::Middle);
    let (w, h) = (r.width(), r.height());
    if w < 40.0 || h < 30.0 {
        return;
    }
    let p = p.with_clip_rect(r.intersect(p.clip_rect()));
    let bot_k = if w < NARROW_W { 0.64 } else { 0.74 };
    let (top, bot, split, pad) = (r.top() + h * 0.2, r.top() + h * bot_k, r.left() + w * 0.4, w * 0.015);
    paint_shape(&p, r, pal, s, top, bot, split, pad, scratch);
    paint_history(&p, r, pal, s, hist, top, bot, split, pad, scratch);
    paint_labels(&p, r, pal, s, lamp);
}

#[allow(clippy::too_many_arguments)]
fn paint_shape(p: &Painter, r: Rect, pal: &VstPalette, s: &Settings, top: f32, bot: f32, split: f32, pad: f32, pts: &mut Vec<Pos2>) {
    let tm = s.times;
    let floor_g = f64::from(s.resting_gain());
    let y_of = |g: f64| bot - ((floor_g + (1.0 - floor_g) * g) as f32) * (bot - top);
    let (x0, x1) = (r.left() + pad, split - pad);
    // Pick holds open for as long as the note rings, so the picture shows some of that.
    let sustain = if s.mode == 0 { (0.25 * (tm.attack + tm.hold + tm.release)).max(0.05) } else { 0.0 };
    let total = if s.mode == 2 { tm.cycle } else { tm.attack + sustain + tm.hold + tm.release };
    if !(total > 0.0 && total.is_finite()) || x1 <= x0 {
        return;
    }
    let x_of = |t: f64| x0 + (t / total) as f32 * (x1 - x0);
    p.line_segment([pos2(x0, bot), pos2(x1, bot)], Stroke::new(1.0, RULE));
    p.line_segment([pos2(x0, top), pos2(x1, top)], Stroke::new(1.0, RULE));
    const STEPS: usize = 200;
    pts.clear();
    if s.mode == 2 {
        // One cycle starting half a cycle before the beat, so the beat sits mid-panel.
        let offset = if s.peak_beat { tm.attack } else { 0.0 };
        let shift = ((offset - 0.5 * tm.cycle) % tm.cycle + tm.cycle) % tm.cycle;
        for i in 0..=STEPS {
            let t = i as f64 / STEPS as f64 * tm.cycle;
            pts.push(pos2(x_of(t), y_of(s.gain_at((t + shift) % tm.cycle, 0.0))));
        }
    } else {
        for i in 0..=STEPS {
            let t = i as f64 / STEPS as f64 * total;
            pts.push(pos2(x_of(t), y_of(s.gain_at(t, sustain))));
        }
    }
    fill_under(p, pts, x0, x1, bot, al(pal.accent, 0.11));
    p.add(Shape::line(pts.clone(), Stroke::new(2.0, pal.accent)));
    if s.mode == 2 {
        let xb = x_of(0.5 * tm.cycle);
        p.extend(Shape::dashed_line(&[pos2(xb, top - 6.0), pos2(xb, bot)], Stroke::new(1.0, pal.text_bright), 3.0, 3.0));
        single_line(p, (xb + 4.0).round(), (top - 2.0).round(), "beat", 9.0, LABEL);
    } else {
        single_line(p, x0.round(), (top - 2.0).round(), "pick", 9.0, LABEL);
        if s.mode == 0 {
            single_line(p, (x_of(tm.attack) + 4.0).round(), (top - 2.0).round(), "note rings", 9.0, ROW_LABEL);
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn paint_history(
    p: &Painter,
    r: Rect,
    pal: &VstPalette,
    s: &Settings,
    hist: &SwaleHistory,
    top: f32,
    bot: f32,
    split: f32,
    pad: f32,
    pts: &mut Vec<Pos2>,
) {
    let (rx0, rx1) = (split + pad, r.right() - pad);
    if rx1 <= rx0 {
        return;
    }
    p.line_segment([pos2(rx0, bot), pos2(rx1, bot)], Stroke::new(1.0, RULE));
    p.line_segment([pos2(rx0, top), pos2(rx1, top)], Stroke::new(1.0, RULE));
    p.line_segment([pos2(split, r.top() + r.height() * 0.12), pos2(split, bot + 4.0)], Stroke::new(1.0, SCREEN_LINE));
    let n = hist.len();
    let hx = |i: usize| rx1 - (n - 1 - i) as f32 / HISTORY_LEN as f32 * (rx1 - rx0);
    let dy = |db: f32| bot - ((db + 80.0) / 80.0).clamp(0.0, 1.0) * (bot - top);
    // The two lines the detector is working to. Rhythmic has no trigger, so it has neither.
    if s.mode != 2 {
        let th = sd::knob_to_sens_db(s.sens);
        let re = th + sd::knob_to_rearm_db(s.retrig);
        p.extend(Shape::dashed_line(&[pos2(rx0, dy(th)), pos2(rx1, dy(th))], Stroke::new(1.0, ROW_LABEL), 4.0, 4.0));
        p.extend(Shape::dashed_line(&[pos2(rx0, dy(re)), pos2(rx1, dy(re))], Stroke::new(1.0, ROW_LABEL), 1.0, 4.0));
        single_line(p, (rx0 + 2.0).round(), (dy(th) - 3.0).round(), "trigger", 9.0, ROW_LABEL);
        let word = if s.mode == 0 && s.phrase { "lets go" } else { "re-arm" };
        single_line(p, (rx0 + 2.0).round(), (dy(re) - 3.0).round(), word, 9.0, ROW_LABEL);
    }
    if n < 2 || s.bypassed {
        return;
    }
    if s.mode != 2 {
        pts.clear();
        pts.extend((0..n).filter_map(|i| hist.point(i).map(|q| pos2(hx(i), dy(q.1)))));
        p.add(Shape::line(pts.clone(), Stroke::new(1.2, DETECTOR)));
    }
    pts.clear();
    pts.extend((0..n).filter_map(|i| hist.point(i).map(|q| pos2(hx(i), bot - q.0 * (bot - top)))));
    fill_under(p, pts, rx0, rx1, bot, al(pal.accent, 0.11));
    p.add(Shape::line(pts.clone(), Stroke::new(2.0, pal.accent)));
}

fn paint_labels(p: &Painter, r: Rect, pal: &VstPalette, s: &Settings, lamp: bool) {
    let (w, h) = (r.width(), r.height());
    let tm = s.times;
    // A narrow screen (the slim editor) leaves the mode's name to the mode row right above it, and
    // gives the stage times and the blurb the room.
    let narrow = w < NARROW_W;
    let mode = sc::MODES.get(s.mode.clamp(0, 2) as usize).copied().unwrap_or("");
    if !narrow {
        tracked_text(
            p,
            Rect::from_min_size(r.min + vec2(8.0, 4.0), vec2(w * 0.4, 14.0)),
            Align2::LEFT_CENTER,
            mode,
            10.0,
            0.16,
            true,
            false,
            ROW_LABEL,
        );
    }
    let dot = "  \u{b7}  ";
    let mut text = format!(
        "A {}{dot}{}{}{dot}R {}",
        sc::format_seconds(tm.attack),
        if s.mode == 0 { "hang " } else { "H " },
        sc::format_seconds(tm.hold),
        sc::format_seconds(tm.release)
    );
    if s.mode == 2 {
        text += &format!("{dot}every {}{}", sc::format_seconds(tm.cycle), if tm.fitted { " (fitted)" } else { "" });
    }
    let tr = Rect::from_min_size(r.min + vec2(w * 0.35, 3.0), vec2(w * 0.65 - 8.0, 15.0));
    p.text(pos2(tr.right(), tr.center().y), Align2::RIGHT_CENTER, text, font(if narrow { 10.0 } else { 11.0 }, false), pal.accent);
    // What the mode does, along the bottom, clear of the lamp.
    let blurb = if s.mode == 0 {
        format!("{} Hang is how long it waits after the playing drops away.", sc::phrase_blurb(i32::from(s.phrase)))
    } else {
        sc::mode_blurb(s.mode).to_string()
    };
    let bh = if narrow { 40.0 } else { 26.0 };
    let br = Rect::from_min_size(pos2(r.left() + 8.0, r.bottom() - bh - 4.0), vec2(w - 36.0, bh));
    let mut job = egui::text::LayoutJob::simple(blurb, font(10.0, false), ROW_LABEL, br.width());
    job.wrap.max_rows = if narrow { 3 } else { 2 };
    let g = p.ctx().fonts_mut(|f| f.layout_job(job));
    p.with_clip_rect(br).galley(pos2(br.left(), br.bottom() - g.size().y), g, ROW_LABEL);
    // The lamp: one flash per swell.
    let led = Rect::from_min_size(pos2(r.right() - 19.0, r.bottom() - 17.0), vec2(9.0, 9.0));
    if lamp {
        for i in (1..=4).rev() {
            p.circle_filled(led.center(), 4.5 + i as f32 * 1.5, al(pal.accent, 0.16 / i as f32));
        }
        p.circle_filled(led.center(), 4.5, pal.accent);
    } else {
        p.circle_filled(led.center(), 4.5, LAMP_OFF);
    }
    let _ = h;
}

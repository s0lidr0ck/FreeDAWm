//! Burley, the subtractive synth, as an instrument: cream panel, walnut cheeks, amber displays.
//!
//! Top: the badge and the preset display. Then the displays: the two oscillators' waves (the
//! scope), the filter's response at its cutoff and resonance (drag its point: sideways the
//! cutoff, up and down the resonance, one undo step per drag) and the two envelopes to scale.
//! Then the sections in signal order: the oscillators and the mixer with the amp envelope's
//! faders; the filter and the LFO with envelope 2's (the filter envelope's); unison, how it
//! plays, and the output. Every parameter of the synth is on the panel.

use super::kit::{self, Ctx, Share, adsr, section, section_cells};
use crate::SoundApp;
use crate::synth_panel::{envelope_points, response_db};
use egui::{Pos2, Rect, Sense, Ui, pos2, vec2};

/// The subtractive synth's plugin id.
pub const ID: &str = "subtractive_synth";

/// The front's height: the displays and three rows of sections.
pub const BODY_H: f32 = DISPLAY_H + 10.0 + 3.0 * (kit::SECTION_H + kit::GAP);
const DISPLAY_H: f32 = 104.0;

const LO_HZ: f32 = 20.0;
const HI_HZ: f32 = 20_000.0;
const FLOOR_DB: f32 = -36.0;
const TOP_DB: f32 = 24.0;

/// Draws Burley's front (its displays and sections) into `area`.
pub fn body(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, area: Rect) {
    let face = cx.face;
    let p = ui.painter().clone();
    let disp = Rect::from_min_size(area.min, vec2(area.width(), DISPLAY_H));
    let parts = kit::row(disp, &[Share::Fixed(150.0), Share::Knobs(1), Share::Fixed(200.0)]);
    if cx.back {
        for (r, tag) in parts.iter().zip(["Scope", "Filter", "Envelopes"]) {
            kit::rear_plate(&p, *r, tag, face);
        }
    } else if let [scope, filter, envs] = parts.as_slice() {
        draw_scope(&p, *scope, cx);
        draw_filter(app, ui, *filter, cx);
        draw_envelopes(&p, *envs, cx);
    }

    // The sections, three rows in signal order.
    let row_h = kit::SECTION_H;
    let mut y = disp.bottom() + 10.0;
    let mut next_row = |shares: &[Share]| {
        let rr = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), row_h));
        y += row_h + kit::GAP;
        kit::row(rr, shares)
    };
    let fw = kit::ENV_W;
    let env1 = kit::SOURCE_COLOURS[0];
    let env2 = kit::SOURCE_COLOURS[1];

    let r1 = next_row(&[Share::Knobs(5), Share::Knobs(3), Share::Knobs(3), Share::Fixed(fw)]);
    if let [osc, xm, mix, amp] = r1.as_slice() {
        let note = format!("2: {:+.0} st", cx.value("osc2_semitones"));
        section(&p, *osc, "Oscillators", &note, face);
        let c = section_cells(*osc, 5);
        kit::knob(app, ui, cx, c[0], "osc1_wave", "Wave 1");
        kit::knob(app, ui, cx, c[1], "osc2_wave", "Wave 2");
        kit::knob(app, ui, cx, c[2], "osc2_semitones", "Pitch 2");
        kit::knob(app, ui, cx, c[3], "osc2_detune", "Detune");
        kit::switch(app, ui, cx, c[4], "osc_sync", "Sync");
        section(&p, *xm, "X-Mod", "audio rate", face);
        let c = section_cells(*xm, 3);
        kit::knob(app, ui, cx, c[0], "xmod_source", "From");
        kit::knob(app, ui, cx, c[1], "xmod_pitch", "Pitch");
        kit::knob(app, ui, cx, c[2], "xmod_cutoff", "Cutoff");
        section(&p, *mix, "Mixer", "", face);
        let c = section_cells(*mix, 3);
        kit::knob(app, ui, cx, c[0], "osc_mix", "Osc Mix");
        kit::knob(app, ui, cx, c[1], "sub_level", "Sub");
        kit::knob(app, ui, cx, c[2], "noise_level", "Noise");
        section(&p, *amp, "Amp Env", "", face);
        adsr(app, ui, cx, *amp, ["amp_attack", "amp_decay", "amp_sustain", "amp_release"], env1);
        kit::outputs_on(ui, cx, *amp, &[0]);
    }

    let r2 = next_row(&[Share::Knobs(5), Share::Knobs(5), Share::Fixed(fw)]);
    if let [filt, lfo, fenv] = r2.as_slice() {
        let note = filter_short(cx.value("filter_type"));
        section(&p, *filt, "Filter", note, face);
        let c = section_cells(*filt, 5);
        kit::knob(app, ui, cx, c[0], "filter_type", "Type");
        kit::knob(app, ui, cx, c[1], "cutoff", "Cutoff");
        kit::knob(app, ui, cx, c[2], "resonance", "Reso");
        kit::knob(app, ui, cx, c[3], "filter_env_amount", "Env Amt");
        kit::knob(app, ui, cx, c[4], "key_track", "Key Trk");
        let shape = choice_name(cx, "lfo_wave");
        section(&p, *lfo, "LFO", &shape, face);
        kit::outputs_on(ui, cx, *lfo, &[2]);
        let c = section_cells(*lfo, 5);
        kit::knob(app, ui, cx, c[0], "lfo_wave", "Shape");
        kit::knob(app, ui, cx, c[1], "lfo_rate", "Rate");
        kit::knob(app, ui, cx, c[2], "lfo_pitch", "Pitch");
        kit::knob(app, ui, cx, c[3], "lfo_cutoff", "Cutoff");
        kit::knob(app, ui, cx, c[4], "lfo_amp", "Level");
        section(&p, *fenv, "Env 2", if cx.back { "" } else { "filter" }, face);
        adsr(app, ui, cx, *fenv, ["filter_attack", "filter_decay", "filter_sustain", "filter_release"], env2);
        kit::outputs_on(ui, cx, *fenv, &[1]);
    }

    let r3 = next_row(&[Share::Knobs(3), Share::Knobs(5), Share::Knobs(2)]);
    if let [uni, play, out] = r3.as_slice() {
        let voices = choice_name(cx, "unison");
        section(&p, *uni, "Unison", if voices == "Off" { "off" } else { "" }, face);
        let c = section_cells(*uni, 3);
        kit::knob(app, ui, cx, c[0], "unison", "Voices");
        kit::knob(app, ui, cx, c[1], "unison_detune", "Detune");
        kit::knob(app, ui, cx, c[2], "unison_spread", "Spread");
        let mode = choice_name(cx, "voice_mode");
        section(&p, *play, "Play", &mode, face);
        kit::outputs_on(ui, cx, *play, &[3, 4, 5, 6]);
        let c = section_cells(*play, 5);
        kit::knob(app, ui, cx, c[0], "voice_mode", "Mode");
        kit::knob(app, ui, cx, c[1], "glide", "Glide");
        kit::knob(app, ui, cx, c[2], "bend_range", "Bend");
        kit::knob(app, ui, cx, c[3], "mod_wheel", "Vibrato");
        kit::knob(app, ui, cx, c[4], "velocity", "Vel Sens");
        section(&p, *out, "Output", "", face);
        let c = section_cells(*out, 2);
        kit::knob(app, ui, cx, c[0], "drive", "Drive");
        kit::knob(app, ui, cx, c[1], "level", "Level");
    }
}

/// The name of choice parameter `id`'s current choice.
fn choice_name(cx: &Ctx, id: &str) -> String {
    cx.param_info(id).map(|i| i.format(cx.value(id))).unwrap_or_default()
}

/// The filter type, short.
fn filter_short(v: f32) -> &'static str {
    ["LP 12", "LP 24", "HP 12", "BP 12", "Notch"].get(v.round().clamp(0.0, 4.0) as usize).copied().unwrap_or("")
}

/// A wave's shape at phase `t` (0..1): saw, square, sine, triangle.
pub fn wave_at(wave: usize, t: f32) -> f32 {
    let t = t.rem_euclid(1.0);
    match wave {
        1 => {
            if t < 0.5 {
                1.0
            } else {
                -1.0
            }
        }
        2 => (std::f32::consts::TAU * t).sin(),
        3 => 1.0 - 4.0 * ((t + 0.25).fract() - 0.5).abs(),
        _ => 2.0 * t - 1.0,
    }
}

/// The scope: oscillator 1's wave bright over two cycles, oscillator 2's fainter under it at its
/// pitch, the sub when it is up.
fn draw_scope(p: &egui::Painter, r: Rect, cx: &Ctx) {
    let face = cx.face;
    kit::display(p, r, "Scope", face);
    let plot = r.shrink2(vec2(12.0, 22.0)).translate(vec2(0.0, 6.0));
    let w1 = cx.value("osc1_wave").round() as usize;
    let w2 = cx.value("osc2_wave").round() as usize;
    let ratio = 2f32.powf((cx.value("osc2_semitones") + cx.value("osc2_detune") / 100.0) / 12.0);
    let line = |f: &dyn Fn(f32) -> f32, mid: f32, amp: f32| -> Vec<Pos2> {
        (0..=160).map(|i| i as f32 / 160.0).map(|t| pos2(plot.left() + plot.width() * t, mid - f(t) * amp)).collect()
    };
    let h = plot.height();
    kit::trace(p, line(&|t| wave_at(w2, t * 2.0 * ratio), plot.top() + h * 0.74, h * 0.18), kit::alpha(face.well_ink, 0.55), 1.4);
    kit::trace(p, line(&|t| wave_at(w1, t * 2.0), plot.top() + h * 0.30, h * 0.24), face.glow, 2.0);
    let names = ["Saw", "Sqr", "Sine", "Tri"];
    let n = |w: usize| names.get(w).copied().unwrap_or("");
    kit::display_note(p, r, &format!("{} + {} {:+.0}", n(w1), n(w2), cx.value("osc2_semitones")), face);
}

/// The filter's response at its cutoff and resonance, the sweep of the filter envelope shaded;
/// dragging moves the cutoff (sideways) and the resonance (up and down), one undo step a drag.
fn draw_filter(app: &mut SoundApp, ui: &mut Ui, r: Rect, cx: &Ctx) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::display(&p, r, "Filter", face);
    let kind = cx.value("filter_type").round().clamp(0.0, 4.0) as usize;
    let cutoff = cx.value("cutoff").clamp(LO_HZ, HI_HZ);
    let res = cx.value("resonance").clamp(0.0, 100.0);
    let k = 2.0 - 1.95 * (res / 100.0);
    let plot = Rect::from_min_max(r.min + vec2(4.0, 20.0), r.max - vec2(4.0, 8.0));
    let x_of = |hz: f32| plot.min.x + plot.width() * ((hz / LO_HZ).ln() / (HI_HZ / LO_HZ).ln());
    let hz_of = |x: f32| LO_HZ * (HI_HZ / LO_HZ).powf(((x - plot.min.x) / plot.width()).clamp(0.0, 1.0));
    let y_of = |db: f32| plot.max.y - plot.height() * ((db.clamp(FLOOR_DB, TOP_DB) - FLOOR_DB) / (TOP_DB - FLOOR_DB));
    let env_oct = cx.value("filter_env_amount") / 100.0 * 6.0;
    let (a, b) = (cutoff, (cutoff * 2f32.powf(env_oct)).clamp(LO_HZ, HI_HZ));
    p.rect_filled(Rect::from_x_y_ranges(x_of(a.min(b))..=x_of(a.max(b)), plot.y_range()), 0.0, kit::alpha(face.glow, 0.045));
    // Where modulation has the filter now (the knobs' settings when nothing moves it): the curve
    // and its point follow it, a faint ring stays at the setting.
    let live_cut = cx.live_value("cutoff").map_or(cutoff, |c| c.clamp(LO_HZ, HI_HZ));
    let live_k = cx.live_value("resonance").map_or(k, |r| 2.0 - 1.95 * (r.clamp(0.0, 100.0) / 100.0));
    let curve: Vec<Pos2> = (0..=200)
        .map(|i| {
            let x = plot.min.x + plot.width() * i as f32 / 200.0;
            pos2(x, y_of(response_db(hz_of(x), live_cut, live_k, kind)))
        })
        .collect();
    // The area under the curve, faintly lit.
    let mut fill = egui::Mesh::default();
    let under = kit::alpha(face.glow, 0.13);
    for (i, q) in curve.iter().enumerate() {
        fill.colored_vertex(*q, under);
        fill.colored_vertex(pos2(q.x, plot.max.y), kit::alpha(face.glow, 0.02));
        if i > 0 {
            let j = (i as u32) * 2;
            fill.add_triangle(j - 2, j - 1, j + 1);
            fill.add_triangle(j - 2, j + 1, j);
        }
    }
    p.add(egui::Shape::mesh(fill));
    kit::trace(&p, curve, face.glow, 2.4);
    let dot = pos2(x_of(live_cut), y_of(response_db(live_cut, live_cut, live_k, kind)));
    let set_at = pos2(x_of(cutoff), y_of(response_db(cutoff, cutoff, k, kind)));
    if set_at.distance(dot) > 1.5 {
        p.circle_stroke(set_at, 4.0, egui::Stroke::new(1.0, kit::alpha(face.glow, 0.5)));
    }
    let resp = ui.interact(plot, egui::Id::new(("burley-filter", cx.track.0)), Sense::click_and_drag());
    let hot = resp.hovered() || resp.dragged();
    p.circle_filled(dot, if hot { 6.5 } else { 5.0 }, face.glow);
    p.circle_stroke(dot, 8.0, egui::Stroke::new(1.0, kit::alpha(face.glow, if hot { 0.6 } else { 0.25 })));
    let resp = resp.on_hover_text(format!("Cutoff {cutoff:.0} Hz · Resonance {res:.0}% · drag sideways: cutoff, up and down: resonance"));
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Filter curve"));
    if hot {
        ui.ctx().set_cursor_icon(egui::CursorIcon::Move);
    }
    if resp.dragged() {
        let d = resp.drag_delta();
        let new_cut = (cutoff * 2f32.powf(d.x / plot.width() * (HI_HZ / LO_HZ).log2())).clamp(LO_HZ, HI_HZ);
        let new_res = (res - d.y * 0.5).clamp(0.0, 100.0);
        if (new_cut - cutoff).abs() > 1e-3 {
            kit::set_param(app, cx.track, "cutoff", new_cut, "filter-curve");
        }
        if (new_res - res).abs() > 1e-3 {
            kit::set_param(app, cx.track, "resonance", new_res, "filter-curve");
        }
    }
    if resp.drag_stopped() {
        app.engine.end_merge();
    }
    let names = ["LP 12", "LP 24", "HP 12", "BP 12", "Notch"];
    let hz = if cutoff >= 1000.0 { format!("{:.1} kHz", cutoff / 1000.0) } else { format!("{cutoff:.0} Hz") };
    kit::display_note(&p, r, &format!("{}   {hz}", names.get(kind).copied().unwrap_or("")), face);
}

/// The amp envelope and envelope 2 side by side, each to its own scale, in the sources' colours.
fn draw_envelopes(p: &egui::Painter, r: Rect, cx: &Ctx) {
    let face = cx.face;
    kit::display(p, r, "Envelopes", face);
    let half = (r.width() - 36.0) / 2.0;
    for (i, (prefix, col)) in [("amp", kit::SOURCE_COLOURS[0]), ("filter", kit::SOURCE_COLOURS[1])].into_iter().enumerate() {
        let x0 = r.left() + 12.0 + i as f32 * (half + 12.0);
        let plot = Rect::from_min_size(pos2(x0, r.top() + 26.0), vec2(half, r.height() - 48.0));
        draw_adsr(p, plot, cx, prefix, col);
    }
    kit::display_note(p, r, &format!("{}   {}", kit::short_value_of(cx, "amp_release"), kit::short_value_of(cx, "filter_release")), face);
}

/// One ADSR drawn to scale in `plot` from the parameters `<prefix>_attack` … `_release`, with
/// dots at its corners.
pub fn draw_adsr(p: &egui::Painter, plot: Rect, cx: &Ctx, prefix: &str, col: egui::Color32) {
    let pts = envelope_points(
        cx.value(&format!("{prefix}_attack")),
        cx.value(&format!("{prefix}_decay")),
        cx.value(&format!("{prefix}_sustain")) / 100.0,
        cx.value(&format!("{prefix}_release")),
    );
    let total = pts.last().map_or(1.0, |q| q.0).max(1e-3);
    let to = |(x, y): (f32, f32)| pos2(plot.min.x + plot.width() * x / total, plot.max.y - plot.height() * y);
    let drawn: Vec<Pos2> = pts.iter().copied().map(to).collect();
    kit::trace(p, drawn, col, 2.0);
    // The corners: the peak, the start of the sustain, the end of the release.
    if let (Some(peak), Some(last)) = (pts.get(1), pts.last()) {
        p.circle_filled(to(*peak), 3.2, col);
        let sustain = pts.iter().rev().nth(25).copied().unwrap_or(*peak);
        p.circle_filled(to(sustain), 3.2, col);
        p.circle_filled(to(*last), 3.2, col);
    }
}

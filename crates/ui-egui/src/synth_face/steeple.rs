//! Steeple, the FM synth, as an instrument: galvanised steel panel, pine cheeks, blue displays.
//!
//! The algorithm display draws the four operators as boxes (the ones heard on the bottom row over
//! the output line, each modulator above what it modulates, a loop on an operator with feedback,
//! each box showing its ratio or fixed frequency); clicking it steps to the next algorithm,
//! right-clicking to the one before (one undo step). Beside it the four operators' envelopes to
//! scale. Then one framed row per operator (its frequency, level, response and envelope), and the
//! globals: the LFO, the mod wheel and bend, the output. Every parameter is on the panel.

use super::kit::{self, Ctx, Share, adsr, section, section_cells};
use crate::SoundApp;
use crate::fm_panel::layout;
use crate::theme::{bold, mono};
use egui::{Align2, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::fm::{ALGORITHMS, CARRIERS, MODULATORS};

/// The FM synth's plugin id.
pub const ID: &str = "fm_synth";

const DISPLAY_H: f32 = 190.0;

/// The front's height: the displays, four operator rows and the globals.
pub const BODY_H: f32 = DISPLAY_H + 18.0 + 5.0 * (kit::SECTION_H + 16.0);

/// Draws Steeple's front (its displays and sections) into `area`.
pub fn body(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, area: Rect) {
    let face = cx.face;
    let p = ui.painter().clone();
    let disp = Rect::from_min_size(area.min, vec2(area.width(), DISPLAY_H));
    let parts = kit::row(disp, &[Share::Fixed(330.0), Share::Knobs(1)]);
    if let [alg, envs] = parts.as_slice() {
        draw_algorithm(app, ui, *alg, cx);
        draw_op_envelopes(&p, *envs, cx);
    }

    let row_h = kit::SECTION_H;
    let mut y = disp.bottom() + 18.0;
    let mut next_row = |shares: &[Share]| {
        let rr = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), row_h));
        y += row_h + 16.0;
        kit::row(rr, shares)
    };
    let fw = 4.0 * 34.0 + 16.0;
    let alg = algorithm(cx);
    let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
    for n in 1..=4 {
        let parts = next_row(&[Share::Knobs(5), Share::Knobs(4), Share::Fixed(fw)]);
        let [freq, lvl, env] = parts.as_slice() else { continue };
        let role = if carriers & (1 << (n - 1)) != 0 { "carrier" } else { "modulator" };
        section(&p, *freq, &format!("Operator {n}"), role, face);
        let c = section_cells(*freq, 5);
        let id = |s: &str| format!("op{n}_{s}");
        kit::knob(app, ui, cx, c[0], &id("coarse"), "Ratio");
        kit::knob(app, ui, cx, c[1], &id("fine"), "Fine");
        kit::knob(app, ui, cx, c[2], &id("detune"), "Detune");
        kit::switch(app, ui, cx, c[3], &id("fixed"), "Fixed");
        kit::knob(app, ui, cx, c[4], &id("freq"), "Freq");
        section(&p, *lvl, "Level", "", face);
        let c = section_cells(*lvl, 4);
        kit::knob(app, ui, cx, c[0], &id("level"), "Level");
        kit::knob(app, ui, cx, c[1], &id("velocity"), "Velocity");
        kit::knob(app, ui, cx, c[2], &id("key_scale"), "Key Scl");
        kit::knob(app, ui, cx, c[3], &id("feedback"), "Feedbk");
        section(&p, *env, &format!("Op {n} Env"), "", face);
        let col = if n == 1 { kit::SOURCE_COLOURS[0] } else { face.accent };
        adsr(app, ui, cx, *env, [&id("attack"), &id("decay"), &id("sustain"), &id("release")], col);
    }

    let g = next_row(&[Share::Knobs(5), Share::Knobs(3), Share::Knobs(1), Share::Fixed(fw)]);
    if let [lfo, wheel, out, e2] = g.as_slice() {
        let shape = kit::short_value_of(cx, "lfo_wave");
        section(&p, *lfo, "LFO", &shape, face);
        let c = section_cells(*lfo, 5);
        kit::knob(app, ui, cx, c[0], "lfo_wave", "Shape");
        kit::knob(app, ui, cx, c[1], "lfo_rate", "Rate");
        kit::knob(app, ui, cx, c[2], "lfo_pitch", "Pitch");
        kit::knob(app, ui, cx, c[3], "lfo_amp", "Amp");
        kit::knob(app, ui, cx, c[4], "lfo_mod", "Mods");
        section(&p, *wheel, "Wheel & Bend", "", face);
        let c = section_cells(*wheel, 3);
        kit::knob(app, ui, cx, c[0], "mod_wheel", "Vibrato");
        kit::knob(app, ui, cx, c[1], "wheel_mod", "Mods");
        kit::knob(app, ui, cx, c[2], "bend_range", "Bend");
        section(&p, *out, "Output", "", face);
        let c = section_cells(*out, 1);
        kit::knob(app, ui, cx, c[0], "level", "Level");
        section(&p, *e2, "Env 2", "a source", face);
        adsr(app, ui, cx, *e2, ["env2_attack", "env2_decay", "env2_sustain", "env2_release"], kit::SOURCE_COLOURS[1]);
    }
}

fn algorithm(cx: &Ctx) -> usize {
    (cx.value("algorithm").round().max(0.0) as usize).min(ALGORITHMS.len() - 1)
}

/// Operator `n`'s ratio ("×3.00") or fixed frequency.
fn ratio_text(cx: &Ctx, n: usize) -> String {
    if cx.value(&format!("op{n}_fixed")) >= 0.5 {
        return format!("{:.0} Hz", cx.value(&format!("op{n}_freq")));
    }
    let coarse = cx.value(&format!("op{n}_coarse")).round().max(0.0) as usize;
    let r = soundcraft_dsp::plugins::fm::ratio_of(coarse, cx.value(&format!("op{n}_fine")), 0.0);
    format!("×{r:.2}")
}

/// The algorithm as boxes and arrows; a click steps to the next algorithm, a right-click back.
fn draw_algorithm(app: &mut SoundApp, ui: &mut Ui, r: Rect, cx: &Ctx) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::display(&p, r, "Algorithm", face);
    let alg = algorithm(cx);
    let plot = Rect::from_min_max(r.min + vec2(20.0, 24.0), r.max - vec2(20.0, 24.0));
    let at = layout(alg, plot);
    let size = vec2(56.0, 26.0);
    let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
    let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
    let wire = Stroke::new(1.6, kit::alpha(face.well_ink, 0.55));
    let out_y = r.bottom() - 18.0;
    let xs: Vec<f32> = (0..4).filter(|n| carriers & (1 << n) != 0).map(|n| at[n].x).collect();
    let (lo, hi) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
    for n in (0..4).filter(|n| carriers & (1 << n) != 0) {
        p.line_segment([at[n] + vec2(0.0, size.y / 2.0), pos2(at[n].x, out_y)], Stroke::new(1.8, face.glow));
    }
    p.hline(lo.min(hi) - 1.0..=hi.max(lo) + 1.0, out_y, Stroke::new(1.8, face.glow));
    for (target, mask) in mods.iter().enumerate() {
        for m in (0..4).filter(|m| mask & (1 << m) != 0) {
            let (a, b) = (at[m] + vec2(0.0, size.y / 2.0), at[target] - vec2(0.0, size.y / 2.0));
            p.line_segment([a, b], wire);
            let d = (b - a).normalized();
            let side = vec2(-d.y, d.x) * 3.5;
            p.add(egui::Shape::convex_polygon(vec![b, b - d * 7.0 + side, b - d * 7.0 - side], wire.color, Stroke::NONE));
        }
    }
    for (n, centre) in at.iter().enumerate() {
        let b = Rect::from_center_size(*centre, size);
        let heard = carriers & (1 << n) != 0;
        if heard {
            p.rect_filled(b.expand(3.0), 7.0, kit::alpha(face.glow, 0.15));
            p.rect_filled(b, 5.0, face.glow);
        } else {
            p.rect_filled(b, 5.0, face.well);
            p.rect_stroke(b, 5.0, wire, egui::StrokeKind::Inside);
        }
        let ink = if heard { face.well } else { face.well_ink };
        p.text(b.center() - vec2(0.0, 5.0), Align2::CENTER_CENTER, format!("OP {}", n + 1), bold(12.0), ink);
        p.text(b.center() + vec2(0.0, 7.5), Align2::CENTER_CENTER, ratio_text(cx, n + 1), mono(9.0), kit::alpha(ink, 0.8));
        if cx.value(&format!("op{}_feedback", n + 1)) > 0.0 {
            let c = pos2(b.right() + 4.0, b.center().y);
            p.add(egui::Shape::line(
                (0..=12).map(|i| c + 9.0 * vec2((-1.1 + 2.2 * i as f32 / 12.0).cos(), (-1.1 + 2.2 * i as f32 / 12.0).sin())).collect::<Vec<Pos2>>(),
                wire,
            ));
        }
    }
    let resp = ui.interact(r, egui::Id::new(("steeple-algorithm", cx.track.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Algorithm"));
    let resp = resp.on_hover_text(format!("Algorithm {}: click for the next, right-click for the one before", alg + 1));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let n = ALGORITHMS.len();
    let step = if resp.clicked() {
        Some((alg + 1) % n)
    } else if resp.secondary_clicked() {
        Some((alg + n - 1) % n)
    } else {
        None
    };
    if let Some(to) = step
        && let Err(e) = app.run("mix.instrument_param", json!({"track": cx.track.0, "param": "algorithm", "value": to}))
    {
        app.ui.status = e;
    }
    kit::display_note(&p, r, &format!("ALGORITHM {}", alg + 1), face);
}

/// The four operators' envelopes side by side, operator 1's bright.
fn draw_op_envelopes(p: &egui::Painter, r: Rect, cx: &Ctx) {
    let face = cx.face;
    kit::display(p, r, "Operator envelopes", face);
    let w = (r.width() - 24.0) / 4.0;
    for n in 1..=4 {
        let x0 = r.left() + 12.0 + w * (n - 1) as f32;
        let plot = Rect::from_min_size(pos2(x0 + 4.0, r.top() + 26.0), vec2(w - 16.0, r.height() - 52.0));
        let col = if n == 1 { face.glow } else { kit::alpha(face.well_ink, 0.6) };
        super::burley::draw_adsr(p, plot, cx, &format!("op{n}"), col);
        p.text(pos2(x0 + 4.0, r.bottom() - 8.0), Align2::LEFT_BOTTOM, format!("OP {n}"), bold(10.0), kit::alpha(face.well_ink, 0.6));
    }
}

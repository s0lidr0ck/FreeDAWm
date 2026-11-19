//! Steeple, the FM synth, as an instrument: galvanised steel panel, pine cheeks, blue displays.
//!
//! The algorithm display draws the four operators as boxes (the ones heard on the bottom row over
//! the output line, each modulator above what it modulates, a loop on an operator with feedback,
//! each box showing its ratio or fixed frequency and a bar for its level). Clicking a box (or the
//! OP 1-4 keys beside the picture) shows that operator's controls below; clicking the picture
//! elsewhere steps to the next algorithm, right-clicking to the one before (one undo step). Beside
//! it the four operators' envelopes to scale, so the whole voice is in view at once. Then the
//! chosen operator's frequency, level, response and envelope; and the globals: the LFO, the mod
//! wheel and bend, the output and envelope 2.

use super::kit::{self, Ctx, Share, adsr, section, section_cells};
use crate::SoundApp;
use crate::fm_panel::layout;
use crate::theme::{bold, mono};
use egui::{Align2, Id, Pos2, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::fm::{ALGORITHMS, CARRIERS, MODULATORS};
use soundcraft_model::TrackId;

/// The FM synth's plugin id.
pub const ID: &str = "fm_synth";

const DISPLAY_H: f32 = 118.0;

/// The front's height: the displays, the chosen operator's row and the globals.
pub const BODY_H: f32 = DISPLAY_H + 10.0 + 2.0 * (kit::SECTION_H + kit::GAP);

fn op_key(track: TrackId) -> Id {
    Id::new(("steeple-op", track.0))
}

/// The operator whose controls show (1..4) on `track`'s Steeple.
pub fn selected_op(ctx: &egui::Context, track: TrackId) -> usize {
    ctx.data(|d| d.get_temp::<usize>(op_key(track))).unwrap_or(1).clamp(1, 4)
}

fn select_op(ctx: &egui::Context, track: TrackId, n: usize) {
    ctx.data_mut(|d| d.insert_temp(op_key(track), n.clamp(1, 4)));
}

/// Draws Steeple's front (its displays and sections) into `area`.
pub fn body(app: &mut SoundApp, ui: &mut Ui, cx: &Ctx, area: Rect) {
    let face = cx.face;
    let p = ui.painter().clone();
    let disp = Rect::from_min_size(area.min, vec2(area.width(), DISPLAY_H));
    let parts = kit::row(disp, &[Share::Fixed(290.0), Share::Knobs(1)]);
    if let [alg, envs] = parts.as_slice() {
        if cx.back {
            kit::rear_plate(&p, *alg, "Algorithm", face);
            kit::rear_plate(&p, *envs, "Operator envelopes", face);
            // Operator 1's envelope is the amp envelope source.
            kit::outputs_on(ui, cx, *envs, &[0]);
            op_keys(ui, *alg, cx);
        } else {
            draw_algorithm(app, ui, *alg, cx);
            draw_op_envelopes(&p, *envs, cx);
        }
    }

    let row_h = kit::SECTION_H;
    let mut y = disp.bottom() + 10.0;
    let mut next_row = |shares: &[Share]| {
        let rr = Rect::from_min_size(pos2(area.left(), y), vec2(area.width(), row_h));
        y += row_h + kit::GAP;
        kit::row(rr, shares)
    };
    let fw = kit::ENV_W;
    let alg = algorithm(cx);
    let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
    let n = selected_op(ui.ctx(), cx.track);
    let parts = next_row(&[Share::Knobs(5), Share::Knobs(4), Share::Fixed(fw)]);
    if let [freq, lvl, env] = parts.as_slice() {
        let role = if carriers & (1 << (n - 1)) != 0 { "carrier" } else { "modulator" };
        section(&p, *freq, &format!("Operator {n}"), role, face);
        let c = section_cells(*freq, 5);
        let id = |s: &str| format!("op{n}_{s}");
        kit::knob(app, ui, cx, c[0], &id("coarse"), "Ratio");
        kit::knob(app, ui, cx, c[1], &id("fine"), "Fine");
        kit::knob(app, ui, cx, c[2], &id("detune"), "Detune");
        kit::switch(app, ui, cx, c[3], &id("fixed"), "Fixed");
        kit::knob(app, ui, cx, c[4], &id("freq"), "Freq");
        section(&p, *lvl, &format!("Op {n} Level"), "", face);
        let c = section_cells(*lvl, 4);
        kit::knob(app, ui, cx, c[0], &id("level"), "Level");
        kit::knob(app, ui, cx, c[1], &id("velocity"), "Vel");
        kit::knob(app, ui, cx, c[2], &id("key_scale"), "Key Scl");
        kit::knob(app, ui, cx, c[3], &id("feedback"), "Feedbk");
        section(&p, *env, &format!("Op {n} Env"), "", face);
        let col = if n == 1 { kit::SOURCE_COLOURS[0] } else { face.accent };
        adsr(app, ui, cx, *env, [&id("attack"), &id("decay"), &id("sustain"), &id("release")], col);
    }

    let g = next_row(&[Share::Knobs(5), Share::Knobs(5), Share::Knobs(1), Share::Fixed(fw)]);
    if let [lfo, wheel, out, e2] = g.as_slice() {
        let shape = kit::short_value_of(cx, "lfo_wave");
        section(&p, *lfo, "LFO", &shape, face);
        kit::outputs_on(ui, cx, *lfo, &[2]);
        let c = section_cells(*lfo, 5);
        kit::knob(app, ui, cx, c[0], "lfo_wave", "Shape");
        kit::knob(app, ui, cx, c[1], "lfo_rate", "Rate");
        kit::knob(app, ui, cx, c[2], "lfo_pitch", "Pitch");
        kit::knob(app, ui, cx, c[3], "lfo_amp", "Amp");
        kit::knob(app, ui, cx, c[4], "lfo_mod", "Mods");
        section(&p, *wheel, if cx.back { "Keys" } else { "Wheel" }, "", face);
        kit::outputs_on(ui, cx, *wheel, &[3, 4, 5, 6]);
        let c = section_cells(*wheel, 3);
        kit::knob(app, ui, cx, c[0], "mod_wheel", "Vibrato");
        kit::knob(app, ui, cx, c[1], "wheel_mod", "Mods");
        kit::knob(app, ui, cx, c[2], "bend_range", "Bend");
        section(&p, *out, "Out", "", face);
        let c = section_cells(*out, 1);
        kit::knob(app, ui, cx, c[0], "level", "Level");
        section(&p, *e2, "Env 2", "", face);
        adsr(app, ui, cx, *e2, ["env2_attack", "env2_decay", "env2_sustain", "env2_release"], kit::SOURCE_COLOURS[1]);
        kit::outputs_on(ui, cx, *e2, &[1]);
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

/// The algorithm as boxes and arrows, each box with its ratio and a level bar, and the OP 1-4
/// keys. A click on a box or key shows that operator; on the picture elsewhere it steps to the
/// next algorithm, a right-click to the one before.
fn draw_algorithm(app: &mut SoundApp, ui: &mut Ui, r: Rect, cx: &Ctx) {
    let face = cx.face;
    let p = ui.painter().clone();
    kit::display(&p, r, "Algorithm", face);
    let alg = algorithm(cx);
    let chosen = selected_op(ui.ctx(), cx.track);
    // The picture on the left, the operator keys on the right.
    let keys_w = 44.0;
    let pic = Rect::from_min_max(r.min, pos2(r.right() - keys_w, r.bottom()));
    let plot = Rect::from_min_max(pic.min + vec2(16.0, 14.0), pic.max - vec2(16.0, 18.0));
    let at = layout(alg, plot);
    let size = vec2(50.0, 18.0);
    let mods = MODULATORS.get(alg).copied().unwrap_or([0; 4]);
    let carriers = CARRIERS.get(alg).copied().unwrap_or(1);
    let wire = Stroke::new(1.4, kit::alpha(face.well_ink, 0.55));
    let out_y = r.bottom() - 12.0;
    let xs: Vec<f32> = (0..4).filter(|n| carriers & (1 << n) != 0).map(|n| at[n].x).collect();
    let (lo, hi) = (xs.iter().copied().fold(f32::MAX, f32::min), xs.iter().copied().fold(f32::MIN, f32::max));
    for n in (0..4).filter(|n| carriers & (1 << n) != 0) {
        p.line_segment([at[n] + vec2(0.0, size.y / 2.0), pos2(at[n].x, out_y)], Stroke::new(1.6, face.glow));
    }
    p.hline(lo.min(hi) - 1.0..=hi.max(lo) + 1.0, out_y, Stroke::new(1.6, face.glow));
    for (target, mask) in mods.iter().enumerate() {
        for m in (0..4).filter(|m| mask & (1 << m) != 0) {
            let (a, b) = (at[m] + vec2(0.0, size.y / 2.0), at[target] - vec2(0.0, size.y / 2.0));
            p.line_segment([a, b], wire);
            let d = (b - a).normalized();
            let side = vec2(-d.y, d.x) * 3.0;
            p.add(egui::Shape::convex_polygon(vec![b, b - d * 6.0 + side, b - d * 6.0 - side], wire.color, Stroke::NONE));
        }
    }
    let mut boxes = [Rect::NOTHING; 4];
    for (n, centre) in at.iter().enumerate() {
        let b = Rect::from_center_size(*centre, size);
        boxes[n] = b;
        let heard = carriers & (1 << n) != 0;
        if heard {
            p.rect_filled(b, 4.0, face.glow);
        } else {
            p.rect_filled(b, 4.0, face.well);
            p.rect_stroke(b, 4.0, wire, egui::StrokeKind::Inside);
        }
        // The level as a bar along the box's foot.
        let level = (cx.value(&format!("op{}_level", n + 1)) / 100.0).clamp(0.0, 1.0);
        let bar = Rect::from_min_size(pos2(b.left() + 3.0, b.bottom() - 4.0), vec2((b.width() - 6.0) * level, 2.0));
        p.rect_filled(bar, 1.0, if heard { kit::alpha(face.well, 0.7) } else { face.glow });
        if n + 1 == chosen {
            p.rect_stroke(b.expand(2.5), 6.0, Stroke::new(1.5, face.well_ink), egui::StrokeKind::Outside);
        }
        let ink = if heard { face.well } else { face.well_ink };
        p.text(pos2(b.left() + 5.0, b.center().y - 2.0), Align2::LEFT_CENTER, format!("{}", n + 1), bold(11.0), ink);
        p.text(pos2(b.right() - 4.0, b.center().y - 2.0), Align2::RIGHT_CENTER, ratio_text(cx, n + 1), mono(9.0), kit::alpha(ink, 0.85));
        if cx.value(&format!("op{}_feedback", n + 1)) > 0.0 {
            let c = pos2(b.right() + 3.0, b.center().y);
            p.add(egui::Shape::line(
                (0..=12).map(|i| c + 7.0 * vec2((-1.1 + 2.2 * i as f32 / 12.0).cos(), (-1.1 + 2.2 * i as f32 / 12.0).sin())).collect::<Vec<Pos2>>(),
                wire,
            ));
        }
    }
    let resp = ui.interact(pic, Id::new(("steeple-algorithm", cx.track.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Algorithm"));
    let resp = resp.on_hover_text(format!(
        "Algorithm {}: click an operator to show its controls; click elsewhere for the next algorithm, right-click for the one before",
        alg + 1
    ));
    if resp.hovered() {
        ui.ctx().set_cursor_icon(egui::CursorIcon::PointingHand);
    }
    let hit = resp.interact_pointer_pos().and_then(|q| boxes.iter().position(|b| b.expand(3.0).contains(q)));
    let n = ALGORITHMS.len();
    let step = if resp.clicked() {
        match hit {
            Some(op) => {
                select_op(ui.ctx(), cx.track, op + 1);
                None
            }
            None => Some((alg + 1) % n),
        }
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
    op_keys(ui, r, cx);
    kit::display_note(&p, pic, &format!("ALG {}", alg + 1), face);
}

/// The OP 1-4 keys down the right of `r`: a click shows that operator's controls. On the Back
/// each key is also where the cables into an operator that is not shown end (`@opN`).
fn op_keys(ui: &mut Ui, r: Rect, cx: &Ctx) {
    let face = cx.face;
    let p = ui.painter().clone();
    let chosen = selected_op(ui.ctx(), cx.track);
    let keys_w = 44.0;
    for k in 1..=4usize {
        let kr = Rect::from_min_size(pos2(r.right() - keys_w + 4.0, r.top() + 8.0 + (k - 1) as f32 * 26.0), vec2(keys_w - 10.0, 22.0));
        let on = k == chosen;
        let kresp = ui.interact(kr, Id::new(("steeple-op-key", cx.track.0, k)), Sense::click());
        kresp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, format!("OP {k}")));
        let fill = if on {
            face.glow
        } else if kresp.hovered() {
            kit::alpha(face.well_ink, 0.2)
        } else {
            kit::alpha(face.well_ink, 0.08)
        };
        p.rect_filled(kr, 3.0, fill);
        p.text(kr.center(), Align2::CENTER_CENTER, format!("OP {k}"), bold(9.5), if on { face.well } else { face.well_ink });
        if kresp.on_hover_text(format!("Show operator {k}'s controls")).clicked() {
            select_op(ui.ctx(), cx.track, k);
        }
        if cx.back && k != chosen {
            cx.centres.borrow_mut().push((format!("@op{k}"), pos2(kr.left() - 2.0, kr.center().y)));
        }
    }
}

/// The four operators' envelopes side by side, the one shown below bright.
fn draw_op_envelopes(p: &egui::Painter, r: Rect, cx: &Ctx) {
    let face = cx.face;
    kit::display(p, r, "Operator envelopes", face);
    let chosen = selected_op(p.ctx(), cx.track);
    let w = (r.width() - 20.0) / 4.0;
    for n in 1..=4 {
        let x0 = r.left() + 10.0 + w * (n - 1) as f32;
        let plot = Rect::from_min_size(pos2(x0 + 4.0, r.top() + 22.0), vec2(w - 14.0, r.height() - 42.0));
        let col = if n == chosen { face.glow } else { kit::alpha(face.well_ink, 0.55) };
        super::burley::draw_adsr(p, plot, cx, &format!("op{n}"), col);
        p.text(pos2(x0 + 4.0, r.bottom() - 6.0), Align2::LEFT_BOTTOM, format!("OP {n}"), bold(9.5), kit::alpha(face.well_ink, 0.6));
    }
}

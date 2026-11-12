//! Vint in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]). The header
//! (title and preset strip; Undo, A, B, Bypass), one circuit menu (each entry with its
//! description), the circuit's own faceplate turned on its side — the panel's left-to-right runs
//! top to bottom, its rows become columns, while every printed label, scale and knob stays
//! upright — then the curve and the output section. The face is scaled to the drawer's width at
//! its natural height; the drawer column scrolls when the editor is taller than the strip.

use super::config::{CircuitDef, Column, Control, Face, Glyph, Kind, PanelLine, PanelText, Placed, ToggleStyle};
use super::{Editor, FAMILY, PALETTE, circuit, face_bounds, face_controls, knob, paint_face, pv, scope};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, pill_height, segmented};
use crate::editor_kit::family::{self, FamilyState, HeaderRects, small_pill_w};
use crate::editor_kit::look::{CAPTION_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::slim::{self, GAP, MODEL_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H};
use crate::theme::{bold, regular};
use egui::{Align2, Color32, Id, Pos2, Rect, Stroke, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::vint::vint_engine_dsp::{self as d, NUM_PARAMS};

const SCOPE_H: f32 = 150.0;
const FOOT_KNOB: f32 = 48.0;
/// The faceplate's margin round its controls.
const FACE_MARGIN: f32 = 14.0;

pub use crate::editor_kit::slim::size;

/// Each circuit's hardware, for the menu ("British · SSL 4000 E").
fn tag(c: i32) -> &'static str {
    match c {
        d::C_PASSIVE => "EQP-1A",
        d::C_PASSIVE_MID => "MEQ-5",
        d::C_CONSOLE => "1073",
        d::C_PROPORTIONAL => "550A",
        d::C_BRITISH => "SSL 4000 E",
        d::C_ENHANCER => "Exciter",
        d::C_AIR => "Dolby A trick",
        _ => "12-bit",
    }
}

// ============================================================================ the turned face

/// The panel turned on its side: a point (x, y) of the 1000 x 300 panel lands at (y, x), so the
/// top row becomes the left column and the panel's left edge the top.
fn turn(x: f32, y: f32) -> (f32, f32) {
    (y, x)
}

/// The face turned: controls, the meter and the lamp move; printed words that belong to a control
/// (its unit, its name) keep their place beside it; everything stays upright. A tall key lies down.
fn turned(def: &CircuitDef, c: i32) -> Face {
    let src = &def.face;
    let mut f = src.clone();
    for (pc, o) in f.controls.iter_mut().zip(&src.controls) {
        (pc.x, pc.y) = turn(o.x, o.y);
        if matches!(o.toggle, ToggleStyle::SslTall | ToggleStyle::SslTallRed) {
            pc.toggle = if o.toggle == ToggleStyle::SslTall { ToggleStyle::Ssl } else { ToggleStyle::SslRed };
            pc.w = (o.w * 2.6).round();
        }
        // a pointer switch's figures crowd first as the face shrinks: give it room to print them
        if o.style == super::config::FaceStyle::Chicken {
            pc.w = (o.w * 1.15).round();
        }
    }
    // a word within reach of a control is that control's: it keeps its offset from it
    let owner = |x: f32, y: f32, reach: f32| -> Option<usize> {
        src.controls
            .iter()
            .enumerate()
            .map(|(i, pc)| (i, (pc.x - x).hypot(pc.y - y)))
            .filter(|(_, dd)| *dd < reach)
            .min_by(|a, b| a.1.total_cmp(&b.1))
            .map(|(i, _)| i)
    };
    f.texts = src
        .texts
        .iter()
        .map(|t| {
            let reach = if t.text.chars().count() == 1 { 80.0 } else { 60.0 };
            let (x, y) = match owner(t.x, t.y, reach) {
                Some(i) => {
                    let (o, n) = (&src.controls[i], &f.controls[i]);
                    (n.x + t.x - o.x, n.y + t.y - o.y)
                }
                None => turn(t.x, t.y),
            };
            PanelText { x, y, vertical: false, ..*t }
        })
        .collect();
    f.lines = src
        .lines
        .iter()
        .map(|l| {
            let (x0, y0) = turn(l.x0, l.y0);
            let (x1, y1) = turn(l.x1, l.y1);
            PanelLine { x0, y0, x1, y1 }
        })
        .collect();
    for g in &mut f.glyphs {
        (g.x, g.y) = turn(g.x, g.y);
    }
    if src.meter_w > 0.0 {
        (f.meter_x, f.meter_y) = turn(src.meter_x, src.meter_y);
    }
    if src.lamp_x >= 0.0 {
        (f.lamp_x, f.lamp_y) = turn(src.lamp_x, src.lamp_y);
    }
    tweak(&mut f, c);
    f
}

fn text_mut<'a>(f: &'a mut Face, s: &str) -> impl Iterator<Item = &'a mut PanelText> {
    let s = s.to_string();
    f.texts.iter_mut().filter(move |t| t.text == s)
}

fn control_mut(f: &mut Face, param: usize) -> Option<&mut Placed> {
    f.controls.iter_mut().find(|p| p.param == param)
}

/// Where a turned panel needs a hand: the name plate across the top, a meter that fits its column.
fn tweak(f: &mut Face, c: i32) {
    match c {
        d::C_PASSIVE => {
            // the name plate across the top, over both columns
            for t in text_mut(f, "PROGRAM EQUALIZER") {
                (t.x, t.y) = (150.0, -14.0);
            }
            for t in text_mut(f, "EQP-1A") {
                (t.x, t.y) = (150.0, 8.0);
            }
            // High Frequency (and its KCS) down a little: room for SHARP / BROAD above it
            if let Some(k) = control_mut(f, d::P_PAS_HIGH_FREQ) {
                k.y += 26.0;
            }
            for t in text_mut(f, "KCS") {
                t.y += 26.0;
            }
            // SHARP / BROAD: under Bandwidth's 0 and 10, as printed either side of its foot
            if let Some(bw) = control_mut(f, d::P_PAS_BANDWIDTH).map(|p| (p.x, p.y)) {
                for (s, dx) in [("SHARP", -40.0), ("BROAD", 40.0)] {
                    for t in text_mut(f, s) {
                        (t.x, t.y, t.size) = (bw.0 + dx, bw.1 + 82.0, 8.5);
                    }
                }
            }
            f.meter_w = 132.0;
            f.meter_h = 80.0;
            f.meter_x = 92.0;
        }
        d::C_PASSIVE_MID => {
            for t in text_mut(f, "MIDRANGE EQUALIZER  MEQ-5") {
                (t.x, t.y) = (150.0, -10.0);
            }
            f.meter_w = 120.0;
            f.meter_h = 74.0;
        }
        d::C_CONSOLE => {
            // the curve symbols head each band's row, at the left
            let rows = [(347.0, Glyph::HighShelf), (517.0, Glyph::Peak), (686.0, Glyph::LowShelf), (839.0, Glyph::HighPass)];
            for g in &mut f.glyphs {
                if let Some((x, _)) = rows.iter().find(|(_, k)| *k == g.kind) {
                    (g.x, g.y) = (26.0, x - 70.0);
                }
            }
            // the preamp switch alone in its row: room for its 22 figures
            if let Some(k) = control_mut(f, d::P_CON_PRE) {
                k.w = 190.0;
            }
        }
        d::C_PROPORTIONAL => {
            // the push-buttons' names just under them
            for n in ["IN", "H/F", "L/F"] {
                for t in text_mut(f, n) {
                    t.y = 948.0;
                }
            }
            for k in [d::P_PRO_HIGH_FREQ, d::P_PRO_MID_FREQ, d::P_PRO_LOW_FREQ] {
                if let Some(k) = control_mut(f, k) {
                    k.w = 150.0;
                }
            }
        }
        d::C_BRITISH => {
            // FILTERS reads across the strip's top rule
            for t in text_mut(f, "FILTERS") {
                t.size = 11.0;
            }
            // BYP over BLACK, as the keys stack in their box
            if let Some(k) = control_mut(f, d::P_BYPASS) {
                (k.x, k.y) = (72.0, 556.0);
            }
            if let Some(k) = control_mut(f, d::P_BRI_BLACK) {
                (k.x, k.y) = (72.0, 602.0);
            }
        }
        _ => {}
    }
}

/// The turned face's extent in points, drawn at `scale` from (0, 0).
fn extent(f: &Face, scale: f32) -> Rect {
    let mut r = Rect::NOTHING;
    for pc in &f.controls {
        let b = face_bounds(pc, scale, Pos2::ZERO);
        // a knob's printed scale reaches past its box by about half a figure
        r = r.union(if matches!(pc.kind, Kind::Knob | Kind::Step) { b.expand2(vec2(8.0, 0.0)) } else { b });
    }
    for t in &f.texts {
        let w = t.text.chars().count() as f32 * t.size * 0.62;
        r = r.union(Rect::from_center_size(pos2(t.x * scale, t.y * scale), vec2(w, t.size + 2.0)));
    }
    if f.meter_w > 0.0 {
        r = r.union(Rect::from_center_size(pos2(f.meter_x * scale, f.meter_y * scale), vec2(f.meter_w * scale, f.meter_h * scale)));
    }
    if f.lamp_x >= 0.0 {
        r = r.union(Rect::from_center_size(pos2(f.lamp_x * scale, f.lamp_y * scale), Vec2::splat(30.0 * scale)));
    }
    r
}

/// The scale at which the turned face is `w` wide.
fn fit_scale(f: &Face, w: f32) -> f32 {
    let mut s = w / 300.0;
    for _ in 0..4 {
        let e = extent(f, s);
        if e.width() > 1.0 {
            s *= w / e.width();
        }
    }
    s.clamp(0.2, 2.0)
}

// ============================================================================ the editor

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
        st.needle = (-22.0, ui.input(|i| i.time));
    }
    // `ui.mix.drawer {tab: "Models"}` (scripts): open the circuit menu
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("Models") {
        st.circuits_open = true;
    }
    let pal = &PALETTE;
    let ed = host.rect;
    let cur = (pv(host, d::P_CIRCUIT).round() as i32).clamp(0, d::NUM_CIRCUITS - 1);
    if family::browser(app, ui, host, pal, &mut st.fam, &FAMILY, circuit(cur).name) {
        return;
    }
    let p = ui.painter().clone();
    paint_background(&p, ed);
    let def = circuit(cur);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: the title and the preset strip; Undo, A, B, Bypass
    let btn_h = pill_height(PillStyle::Small);
    let head = row(y, ROW_H + 2.0);
    let title_w = paint_title(&p, head, "Vint", "Engine", pal);
    let (prev, name, next) = slim::preset_strip(head, title_w);
    y += ROW_H + 2.0 + 6.0;
    let widths = [small_pill_w(ui, "Undo").max(52.0), small_pill_w(ui, "A").max(34.0), small_pill_w(ui, "B").max(34.0), small_pill_w(ui, "Bypass")];
    let cells = slim::pill_row(x0, w, y, &widths, &[1]);
    let rects = HeaderRects { prev, name, next, undo: cells[0], a: cells[1], b: cells[2], bypass: cells[3] };
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);
    y += btn_h + GAP + 2.0;

    // ---- 2. the circuit: one menu, each entry with its description
    let order = super::config::CIRCUIT_ORDER;
    let entries: Vec<MenuEntry> =
        order.iter().map(|&c| MenuEntry { name: circuit(c).name, tag: tag(c), blurb: circuit(c).blurb, credit: circuit(c).credit }).collect();
    let selected = order.iter().position(|&c| c == cur).unwrap_or(0);
    let look = MenuLook { max_height: 640.0, ..MenuLook::CENTRED };
    let open_now = std::mem::take(&mut st.circuits_open);
    if let Some(c) =
        slim::model_menu_ex(ui, row(y, MODEL_H), "vint-circuit", "Circuit", pal, &entries, selected, open_now, look).and_then(|i| order.get(i))
    {
        family::set(app, host, &mut st.fam, &FAMILY, "circuit", *c as f32);
    }
    y += MODEL_H + GAP;

    // ---- the rest's height, for the face that must fit
    let has_stage = d::stage_for(cur) != soundcraft_dsp::plugins::fs::vint::vint_stages::ST_NONE;
    let foot_h = 16.0 + 26.0 + 6.0 + FOOT_KNOB + 30.0 + 6.0 + 24.0;

    // ---- 3. the faceplate, turned on its side
    if def.face.empty() {
        y = rows(app, ui, host, &mut st.fam, row(y, 0.0), cur);
    } else {
        let face = turned(def, cur);
        let scale = fit_scale(&face, w - 2.0 * FACE_MARGIN);
        let e = extent(&face, scale);
        let panel_w = (e.width() + 2.0 * FACE_MARGIN).min(w);
        let panel = Rect::from_min_size(pos2((x0 + (w - panel_w) / 2.0).round(), y), vec2(panel_w, (e.height() + 2.0 * FACE_MARGIN).ceil()));
        let origin = pos2(panel.center().x - e.center().x, panel.top() + FACE_MARGIN - e.top());
        paint_face(ui, &p, host, panel, &face, scale, origin);
        face_controls(app, ui, host, &mut st.fam, &face, scale, origin);
        if face.meter_w > 0.0 {
            let vr = Rect::from_center_size(
                pos2(origin.x + face.meter_x * scale, origin.y + face.meter_y * scale),
                vec2(face.meter_w * scale, face.meter_h * scale),
            );
            let now = ui.input(|i| i.time);
            if now - st.needle.1 >= 1.0 / 30.0 || now < st.needle.1 {
                let rms = crate::editor_kit::live(host).map_or(0.0, |l| l.take(soundcraft_dsp::plugins::fs::vint::LIVE_OUT_LEVEL));
                let db = if rms.is_finite() && rms > 0.0 { (20.0 * rms.log10()).max(-60.0) } else { -60.0 };
                knob::vu_set_level(&mut st.needle.0, db, 1.0 / 30.0);
                st.needle.1 = now;
            }
            knob::paint_vu(ui.painter(), Rect::from_min_size(vr.min.round(), vr.size().round()), st.needle.0);
            ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
        }
        y = panel.bottom();
    }
    y += GAP;

    // ---- 4. the curve, full width
    let mut values = [0.0f32; NUM_PARAMS];
    for (i, v) in values.iter_mut().enumerate() {
        *v = pv(host, i);
    }
    scope::draw(ui, row(y, SCOPE_H), pal, &mut st.scope, &values, Id::new(("vint-slim-scope", host.track.0, host.ins.id.0)));
    y += SCOPE_H + GAP;

    // ---- 5. the output section: Output Stage, Drive / Mix / Output, Quality
    let foot = row(y, foot_h);
    paint_panel(&p, foot, 10.0);
    let f = foot.shrink2(vec2(10.0, 8.0));
    let stage_on = pv(host, d::P_STAGE) > 0.5;
    let plain = PillLook { tracked: false, ..PillLook::default() };
    let stage_w = small_pill_w(ui, "Output Stage").max(104.0);
    let stage_r = Rect::from_min_size(pos2(f.left(), f.top() + (26.0 - btn_h) / 2.0), vec2(stage_w, btn_h));
    if has_stage {
        if pill_ex(ui, stage_r, "vint-stage", pal, "Output Stage", stage_on, PillStyle::Small, plain).clicked() {
            family::pill_set(app, host, &mut st.fam, &FAMILY, "stage", if stage_on { 0.0 } else { 1.0 });
        }
    } else {
        ui.scope(|ui| {
            ui.multiply_opacity(0.35);
            let _ = ui.add_enabled_ui(false, |ui| pill_ex(ui, stage_r, "vint-stage", pal, "Output Stage", stage_on, PillStyle::Small, plain));
        });
    }
    let note = Rect::from_min_max(pos2(stage_r.right() + 8.0, f.top()), pos2(f.right(), f.top() + 26.0));
    let mut job = egui::text::LayoutJob::simple_singleline(def.stage.to_string(), regular(CAPTION_SIZE), pal.text_faint);
    job.wrap = egui::text::TextWrapping::truncate_at_width(note.width());
    let g = ui.fonts_mut(|fo| fo.layout_job(job));
    p.galley(pos2(note.left(), note.center().y - g.size().y / 2.0), g, pal.text_faint);

    let knobs = Rect::from_min_size(pos2(f.left(), f.top() + 26.0 + 6.0), vec2(f.width(), FOOT_KNOB + 30.0));
    let kw = (knobs.width() / 3.0).floor();
    let cell = |i: usize| Rect::from_min_size(pos2(knobs.left() + kw * i as f32, knobs.top()), vec2(kw, knobs.height()));
    let drives = has_stage && cur != d::C_CONSOLE;
    let drive_text = super::format_knob(d::P_DRIVE, pv(host, d::P_DRIVE));
    let mix_text = format!("{} %", (pv(host, d::P_MIX) * 100.0).round() as i32);
    let out_text = super::format_knob(d::P_OUTPUT, pv(host, d::P_OUTPUT));
    let default_of = |id: &str| family::default01(host, id);
    if drives {
        let o = DialOpts { label: "Drive", value_text: &drive_text, default: default_of("drive"), small: true, ..DialOpts::default() };
        family::dial_param(app, ui, host, &mut st.fam, &FAMILY, cell(0), pal, "drive", &o);
    } else {
        let v = pv(host, d::P_DRIVE);
        ui.scope(|ui| {
            ui.multiply_opacity(0.35);
            let o = DialOpts { label: "Drive", value_text: &drive_text, small: true, ..DialOpts::default() };
            let _ = dial(ui, cell(0), "vint-drive-off", pal, v, &o);
        });
    }
    let o = DialOpts { label: "Mix", value_text: &mix_text, default: default_of("mix"), small: true, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, cell(1), pal, "mix", &o);
    let o = DialOpts { label: "Output", value_text: &out_text, default: default_of("output"), small: true, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, cell(2), pal, "output", &o);
    let q = Rect::from_min_size(pos2(f.left(), knobs.bottom() + 6.0), vec2(f.width(), 24.0));
    p.text(pos2(q.left(), q.center().y), Align2::LEFT_CENTER, "Quality", regular(11.0), pal.text_body);
    let qsel = (pv(host, d::P_QUALITY).round() as i32).clamp(0, 2) as usize;
    let seg = Rect::from_min_max(pos2(q.left() + 54.0, q.top()), q.max);
    if let Some(i) = segmented(ui, seg, "vint-quality", pal, &["Eco", "Normal", "Insane"], 1, Some(qsel), SelectionStyle::Filled)
        && i != qsel
    {
        family::set(app, host, &mut st.fam, &FAMILY, "quality", i as f32);
    }
    y = foot.bottom() + PAD_Y;
    slim::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// A run of a column's controls laid out across a row: its knobs side by side, a selector as one
/// segmented row, a switch as a pill.
enum Run {
    Knobs(Vec<Control>),
    Seg(Control),
    Toggle(Control),
}

fn runs(col: &Column) -> Vec<Run> {
    let mut out: Vec<Run> = Vec::new();
    for c in &col.controls {
        match c.kind {
            Kind::Knob | Kind::Step => match out.last_mut() {
                Some(Run::Knobs(v)) if v.len() < 3 => v.push(*c),
                _ => out.push(Run::Knobs(vec![*c])),
            },
            Kind::Seg => out.push(Run::Seg(*c)),
            Kind::Toggle => out.push(Run::Toggle(*c)),
        }
    }
    out
}

const TITLE_H: f32 = 18.0;
const ROW_KNOB_H: f32 = 56.0 + 28.0;
const ROW_SEG_H: f32 = 24.0;

fn run_h(r: &Run) -> f32 {
    match r {
        Run::Knobs(_) => ROW_KNOB_H + 4.0,
        Run::Seg(_) | Run::Toggle(_) => ROW_SEG_H + 8.0,
    }
}

/// The flavour circuits' panel turned as the EQs' faces are: each column of the full editor a row
/// here (its title, then its controls left to right). `r`: the top. Returns the bottom.
fn rows(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut FamilyState, r: Rect, c: i32) -> f32 {
    let pal = &PALETTE;
    let enh_type = (pv(host, d::P_ENH_TYPE).round() as i32).clamp(0, 2);
    let cols = super::columns_for(c, enh_type);
    let all: Vec<(&Column, Vec<Run>)> = cols.iter().map(|col| (col, runs(col))).collect();
    let inner_h: f32 = all.iter().map(|(_, rs)| TITLE_H + rs.iter().map(run_h).sum::<f32>() + 6.0).sum();
    let panel = Rect::from_min_size(r.min, vec2(r.width(), inner_h + 16.0));
    let p = ui.painter().clone();
    paint_panel(&p, panel, 10.0);
    let inner = panel.shrink2(vec2(10.0, 8.0));
    let mut y = inner.top();
    let variable = (pv(host, d::P_SMP_MACHINE).round() as i32) == 2;
    for (ci, (col, rs)) in all.iter().enumerate() {
        if ci > 0 {
            p.hline(inner.x_range(), y - 3.0, Stroke::new(1.0, al(Color32::WHITE, 0.06)));
        }
        p.text(pos2(inner.left(), y + TITLE_H / 2.0), Align2::LEFT_CENTER, col.title.to_uppercase(), bold(CAPTION_SIZE), pal.text_body);
        y += TITLE_H;
        for run in rs {
            match run {
                Run::Knobs(ks) => {
                    let cw = (inner.width() / 3.0).floor();
                    let x0 = inner.left() + ((inner.width() - cw * ks.len() as f32) / 2.0).round();
                    for (k, ctl) in ks.iter().enumerate() {
                        let cell = Rect::from_min_size(pos2(x0 + cw * k as f32, y), vec2(cw, ROW_KNOB_H));
                        let id = super::id_of(ctl.param);
                        let text = super::format_knob_for(ctl.param, pv(host, ctl.param), enh_type);
                        let o = DialOpts {
                            label: ctl.label,
                            value_text: &text,
                            default: family::default01(host, id),
                            small: true,
                            ..DialOpts::default()
                        };
                        // the variable rate only means something on the Variable machine
                        if ctl.param == d::P_SMP_RATE && !variable {
                            let v = pv(host, ctl.param);
                            ui.scope(|ui| {
                                ui.multiply_opacity(0.35);
                                let _ = dial(ui, cell, "vint-rate-off", pal, v, &o);
                            });
                        } else {
                            family::dial_param(app, ui, host, st, &FAMILY, cell, pal, id, &o);
                        }
                    }
                }
                Run::Seg(ctl) => {
                    let opts = super::choices(host, ctl.param);
                    let rr = Rect::from_min_size(pos2(inner.left(), y + 4.0), vec2(inner.width(), ROW_SEG_H));
                    let sel = (pv(host, ctl.param).round() as i32).clamp(0, opts.len().saturating_sub(1) as i32) as usize;
                    if let Some(i) = segmented(ui, rr, ("vint-col-seg", ctl.param), pal, opts, 1, Some(sel), SelectionStyle::Filled)
                        && i != sel
                    {
                        family::set(app, host, st, &FAMILY, super::id_of(ctl.param), i as f32);
                    }
                }
                Run::Toggle(ctl) => {
                    let on = pv(host, ctl.param) > 0.5;
                    let pw = small_pill_w(ui, ctl.label).max(100.0);
                    let rr = Rect::from_min_size(pos2((inner.center().x - pw / 2.0).round(), y + 4.0), vec2(pw, ROW_SEG_H));
                    let look = PillLook { tracked: false, ..PillLook::default() };
                    if pill_ex(ui, rr, ("vint-col-toggle", ctl.param), pal, ctl.label, on, PillStyle::Small, look).clicked() {
                        family::pill_set(app, host, st, &FAMILY, super::id_of(ctl.param), if on { 0.0 } else { 1.0 });
                    }
                }
            }
            y += run_h(run);
        }
        y += 6.0;
    }
    panel.bottom()
}

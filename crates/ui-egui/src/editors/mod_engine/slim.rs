//! ModEngine's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a
//! mixer strip tall: the full editor's sections, names and order, folded to fit. Header (title
//! and preset strip; Undo, A, B, Bypass), the circuit (a menu of the fourteen, each with its
//! description), the first dial row three to a row, Link with its set points and Calibrate (the
//! wizard's panel under them while it runs), the trims three to a row, and a More section with the
//! switch rows (Variant, Input, Shape, Sync, Div) and the LFO readout. The gestures are the full
//! editor's own code (`super::dial_ui`, `super::link_ui`, `super::switch_seg`, ...).

use super::{
    Editor, FAMILY, PALETTE, ROW1_DIALS, ROW2_DIALS, SWITCH_ROWS, cal_spec, calibrate_ui, cfg, choices, d, dial_ui, lfo_text, link_text, link_ui,
    pick_circuit, prelude, switch_def, switch_seg,
};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{PillStyle, pill_height, segmented_id, tip};
use crate::editor_kit::family;
use crate::editor_kit::link_cal;
use crate::editor_kit::look::{self, CAPTION_SIZE, LABEL_SIZE, font};
use crate::editor_kit::slim::{self as sp, GAP, MODEL_H, MORE_H, MenuEntry, PAD_X, PAD_Y, ROW_H};
use egui::{Align2, Rect, Ui, pos2, vec2};

const BIG_KNOB: f32 = 56.0;
const SMALL_KNOB: f32 = 40.0;
const LABEL_W: f32 = 46.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let Some((circuit, c, synced)) = prelude(st, app, ui, host) else { return };
    let ed = host.rect;
    let pal = &PALETTE;
    let ins = host.ins;
    let p = ui.painter().clone();
    look::paint_background(&p, ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = sp::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Mod", "Engine"), (x0, w, y), sp::AbPlace::Spread);

    // ---- 2. the circuit: one menu of the fourteen (the full editor's two-row selector and blurb)
    let names = choices(d::P_CIRCUIT);
    let entries: Vec<MenuEntry> = (0..cfg::CIRCUIT_BLURBS.len())
        .map(|i| MenuEntry {
            name: names.get(i).copied().unwrap_or(""),
            tag: if names.get(i).copied() == Some(cfg::circuit(i as i32).name) { "" } else { cfg::circuit(i as i32).name },
            blurb: cfg::CIRCUIT_BLURBS[i],
            credit: super::CIRCUIT_CREDITS.get(i).copied().unwrap_or(""),
        })
        .collect();
    let open_now = std::mem::take(&mut st.models_open);
    if let Some(i) = sp::model_menu(ui, row(y, MODEL_H), "circuit", "Circuit", pal, &entries, circuit.clamp(0, 13) as usize, open_now) {
        pick_circuit(st, app, host, i);
    }
    y += MODEL_H + GAP;

    // ---- 3. Input, Depth, Rate / Mix, Width, Level
    let cells = sp::grid(x0, w, y, ROW1_DIALS.len(), 3, vec2((w / 3.0).floor(), BIG_KNOB + 30.0), 2.0);
    for (spec, r) in ROW1_DIALS.iter().zip(&cells) {
        let r = if spec.2 { shrink_knob(*r, BIG_KNOB, (BIG_KNOB * 0.78).round()) } else { *r };
        dial_ui(st, app, ui, host, r, *spec, c, synced);
    }
    y = sp::bottom_of(&cells, y) + 4.0;

    // ---- 4. Link, its set points, Calibrate (beside Level in the full editor)
    let btn_h = pill_height(PillStyle::Small);
    let link_w = family::small_pill_w(ui, "Link").trunc().max(56.0);
    let cal_w = family::small_pill_w(ui, "Calibrate").trunc().max(78.0);
    let r = row(y, btn_h);
    link_ui(st, app, ui, host, Rect::from_min_size(r.min, vec2(link_w, btn_h)));
    calibrate_ui(st, app, ui, host, Rect::from_min_max(pos2(r.right() - cal_w, r.top()), r.max));
    let mid = pos2((r.left() + link_w + r.right() - cal_w) / 2.0, r.center().y);
    p.text(mid, Align2::CENTER_CENTER, link_text(ins), font(CAPTION_SIZE, false), pal.text_body);
    y += btn_h + GAP;
    if st.cal.active {
        let spec = cal_spec();
        let cal_h = link_cal::panel_height(ui, &st.cal, &spec, w);
        link_cal::panel(app, ui, host, pal, &mut st.cal, &spec, row(y, cal_h));
        y += cal_h + GAP;
    }

    // ---- 5. the trims: Regen, Drive, Noise / Bias, Tone, Tempo
    let cells = sp::grid(x0, w, y, ROW2_DIALS.len(), 3, vec2((w / 3.0).floor(), SMALL_KNOB + 30.0), 6.0);
    for (spec, r) in ROW2_DIALS.iter().zip(&cells) {
        let r = if spec.2 { shrink_knob(*r, SMALL_KNOB, (SMALL_KNOB * 0.85).round()) } else { *r };
        dial_ui(st, app, ui, host, r, (spec.0, spec.1, false), c, synced);
    }
    y = sp::bottom_of(&cells, y) + GAP;

    // ---- 6. More: the switch rows and the LFO readout
    let holds: Vec<&str> = SWITCH_ROWS.iter().map(|(l, _)| *l).collect();
    if sp::more_header(ui, row(y, MORE_H), "mod", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        for (label, i) in SWITCH_ROWS {
            let (opts, disabled, desc) = switch_def(st, ins, c, i, synced);
            let rows = if opts.len() > 4 { 2 } else { 1 };
            let r = row(y, ROW_H * rows as f32);
            let ink = if disabled { pal.text_faint } else { pal.text_body };
            p.text(pos2(r.left(), r.top() + ROW_H / 2.0), Align2::LEFT_CENTER, label, font(LABEL_SIZE, false), ink);
            let seg = Rect::from_min_max(pos2(r.left() + LABEL_W, r.top()), r.max);
            switch_seg(app, ui, host, st, seg, i, opts, disabled, rows);
            y = r.bottom() + 2.0;
            if !desc.is_empty() {
                tip(ui, segmented_id(ui, ("row", i)), desc);
                let dr = Rect::from_min_size(pos2(seg.left(), y), vec2(seg.width(), 13.0));
                one_line(ui, dr, desc, pal.text_faint);
                y += 13.0;
            }
            y += 5.0;
        }
        let text = lfo_text(ins, circuit, synced);
        let g = p.layout(text, font(CAPTION_SIZE, false), pal.text_faint, w);
        let gh = g.size().y;
        p.galley(pos2(x0, y + 2.0), g, pal.text_faint);
        y += gh + 4.0;
    }
    y += PAD_Y;
    sp::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// A smaller knob centred in the cell `r` laid out for `knob` (the full editor's small dials).
fn shrink_knob(r: Rect, knob: f32, small: f32) -> Rect {
    Rect::from_min_size(pos2(r.left(), r.top() + ((knob - small) / 2.0).trunc()), vec2(r.width(), small + 30.0))
}

/// `text` on one line in `r`, cut with an ellipsis where it does not fit.
fn one_line(ui: &Ui, r: Rect, text: &str, c: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font(CAPTION_SIZE, false), c);
    job.wrap = egui::text::TextWrapping::truncate_at_width(r.width());
    let g = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).galley(r.left_top(), g, c);
}

//! FilterEngine's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about
//! a mixer strip tall: the full editor's sections, names and order, folded to fit. Header (title
//! and preset strip; Undo, A, B, Bypass), the circuit (a menu of the nine, each with the real box
//! and what it says about itself), the circuit's switch rows, Cutoff / Reso / Drive / Mix two to a
//! row, the mod source with Depth, the modulation panel for that source, and a More section with
//! the setup row (Input, Level, BPM, Use Manual BPM and the tempo line). The gestures are the full
//! editor's own code (`super::*_ui`, `super::dial`, `super::switch_seg`).

use super::{
    Editor, FAMILY, Frame, MANUAL_NOTE, PALETTE, SETUP_DIALS, cfg, depth_ui, dial, env_src_ui, lfo_shape_ui, lfo_sync_ui, main_dials, mod_src_ui,
    panel_dials, panel_label, pick_circuit, prelude, steps_ui, switch_description, switch_seg, tempo_line, tempo_src_ui,
};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{LabelLayout, segmented_id, tip};
use crate::editor_kit::look::{self, CAPTION_SIZE, LABEL_SIZE, font};
use crate::editor_kit::slim::{self as sp, GAP, MODEL_H, MORE_H, MenuEntry, PAD_X, PAD_Y, ROW_H};
use egui::{Align2, Rect, Ui, pos2, vec2};

const BIG_KNOB: f32 = 56.0;
const SMALL_KNOB: f32 = 36.0;
const LABEL_W: f32 = 50.0;
const DEPTH_W: f32 = 70.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let Some(Frame { circuit, c, mod_src, lfo_sync, a, bpm, host_bpm, live }) = prelude(st, app, ui, host) else { return };
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
    y = sp::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Filter", "Engine"), (x0, w, y), sp::AbPlace::Spread);

    // ---- 2. the circuit: one menu of the nine (the full editor's circuit row and blurb)
    let entries: Vec<MenuEntry> = cfg::CIRCUITS.iter().map(|k| MenuEntry { name: k.name, tag: k.full, blurb: k.blurb, credit: "" }).collect();
    let open_now = std::mem::take(&mut st.models_open);
    if let Some(i) = sp::model_menu(ui, row(y, MODEL_H), "circuit", "Circuit", pal, &entries, circuit.clamp(0, 8) as usize, open_now) {
        pick_circuit(st, app, host, i);
    }
    y += MODEL_H + GAP;

    // ---- 3. the circuit's switch rows: name, Segmented, the option's description
    for sw in c.switches {
        let r = row(y, ROW_H);
        p.text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, sw.label, font(LABEL_SIZE, false), pal.text_body);
        let seg = Rect::from_min_max(pos2(r.left() + LABEL_W, r.top()), r.max);
        switch_seg(app, ui, host, st, seg, sw);
        let desc = switch_description(host, sw);
        tip(ui, segmented_id(ui, ("row", sw.param)), desc);
        let dr = Rect::from_min_size(pos2(seg.left(), r.bottom() + 2.0), vec2(seg.width(), 13.0));
        one_line(ui, dr, desc, pal.text_faint);
        y += ROW_H + 2.0 + 13.0 + 6.0;
    }
    if !c.switches.is_empty() {
        y += 2.0;
    }

    // ---- 4. Cutoff, Reso, Drive, Mix, two to a row
    let main = main_dials(st, ui, ins, c);
    let cells = sp::grid(x0, w, y, main.len(), 2, vec2((w / 2.0).floor(), BIG_KNOB + 32.0), 2.0);
    for ((i, label, text), r) in main.into_iter().zip(&cells) {
        dial(app, ui, host, st, *r, i, label, false, LabelLayout::Below, text, bpm);
    }
    y = sp::bottom_of(&cells, y) + GAP;

    // ---- 5. the mod source and Depth
    let src_row = row(y, 26.0);
    let seg = Rect::from_min_max(src_row.min, pos2(src_row.right() - if mod_src != 0 { DEPTH_W + 4.0 } else { 0.0 }, src_row.top() + ROW_H));
    mod_src_ui(st, app, ui, host, seg, mod_src);
    if mod_src != 0 {
        depth_ui(st, app, ui, host, Rect::from_min_max(pos2(src_row.right() - DEPTH_W, src_row.top()), src_row.max), bpm);
    }
    y += 26.0 + 6.0;

    // ---- 6. the modulation panel for the source
    let top = y;
    let inner = |y: f32, h: f32| Rect::from_min_size(pos2(x0 + 8.0, y), vec2(w - 16.0, h));
    let bg = p.add(egui::Shape::Noop);
    y += 8.0;
    let knob_cell = vec2(((w - 16.0) / 3.0).floor(), SMALL_KNOB + 32.0);
    let knobs = |y: f32, list: &[usize]| sp::grid(x0 + 8.0, w - 16.0, y, list.len(), 3, knob_cell, 2.0);
    match mod_src {
        1 => {
            let list = panel_dials(mod_src, a);
            let cells = knobs(y, &list);
            for (i, r) in list.iter().zip(&cells) {
                dial(app, ui, host, st, *r, *i, panel_label(*i), true, LabelLayout::Below, None, bpm);
            }
            y = sp::bottom_of(&cells, y) + 4.0;
            let seg = Rect::from_center_size(inner(y, ROW_H).center(), vec2((w - 16.0).min(160.0), ROW_H));
            env_src_ui(st, app, ui, host, seg);
            y += ROW_H;
        }
        2 => {
            let strip = inner(y, 2.0 * ROW_H);
            let shape = Rect::from_min_max(strip.min, pos2(strip.right() - 60.0, strip.bottom()));
            lfo_shape_ui(st, app, ui, host, shape, 2);
            let sync_r = Rect::from_center_size(pos2(strip.right() - 26.0, strip.center().y), vec2(52.0, 21.0));
            lfo_sync_ui(st, app, ui, host, sync_r, lfo_sync);
            y += 2.0 * ROW_H + 6.0;
            let list = panel_dials(mod_src, a);
            let cells = knobs(y, &list);
            for (i, r) in list.iter().zip(&cells) {
                dial(app, ui, host, st, *r, *i, panel_label(*i), true, LabelLayout::Below, None, bpm);
            }
            y = sp::bottom_of(&cells, y);
        }
        3 => {
            let list = panel_dials(mod_src, a);
            let cells = knobs(y, &list);
            for (i, r) in list.iter().zip(&cells) {
                dial(app, ui, host, st, *r, *i, panel_label(*i), true, LabelLayout::Below, None, bpm);
            }
            y = sp::bottom_of(&cells, y) + 4.0;
            steps_ui(st, app, ui, host, inner(y, 64.0), live);
            y += 64.0;
        }
        _ => {
            let r = inner(y, 40.0);
            let g = p.layout(MANUAL_NOTE.to_string(), font(CAPTION_SIZE + 0.5, false), pal.text_faint, r.width());
            let h = g.size().y;
            p.galley(pos2(r.center().x - g.size().x / 2.0, r.top()), g, pal.text_faint);
            y += h;
        }
    }
    y += 8.0;
    let panel = Rect::from_min_max(pos2(x0, top), pos2(x0 + w, y));
    p.set(bg, egui::Shape::rect_filled(panel, look::PANEL_CORNER, egui::Color32::from_black_alpha(102)));
    p.rect_stroke(panel, look::PANEL_CORNER, egui::Stroke::new(1.0, egui::Color32::from_white_alpha(18)), egui::StrokeKind::Inside);
    y += GAP + 2.0;

    // ---- 7. More: the setup row
    let holds = "Input \u{00B7} Level \u{00B7} BPM \u{00B7} Use Manual BPM";
    if sp::more_header(ui, row(y, MORE_H), "filter", pal, st.slim_more, holds) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cells = sp::grid(x0, w, y, SETUP_DIALS.len(), 3, vec2((w / 3.0).floor(), SMALL_KNOB + 32.0), 2.0);
        for ((i, label), r) in SETUP_DIALS.into_iter().zip(&cells) {
            dial(app, ui, host, st, *r, i, label, true, LabelLayout::Below, None, bpm);
        }
        y = sp::bottom_of(&cells, y) + 4.0;
        let pill = Rect::from_center_size(pos2(x0 + w / 2.0, y + 10.5), vec2(124.0, 21.0));
        tempo_src_ui(st, app, ui, host, pill);
        y += 21.0 + 4.0;
        p.text(pos2(x0 + w / 2.0, y + 7.0), Align2::CENTER_CENTER, tempo_line(ins, bpm, host_bpm), font(CAPTION_SIZE, false), pal.text_faint);
        y += 14.0;
    }
    y += PAD_Y;
    sp::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// `text` on one line in `r`, cut with an ellipsis where it does not fit.
fn one_line(ui: &Ui, r: Rect, text: &str, c: egui::Color32) {
    let mut job = egui::text::LayoutJob::simple_singleline(text.to_string(), font(CAPTION_SIZE, false), c);
    job.wrap = egui::text::TextWrapping::truncate_at_width(r.width());
    let g = ui.fonts_mut(|f| f.layout_job(job));
    ui.painter().with_clip_rect(r.intersect(ui.clip_rect())).galley(r.left_top(), g, c);
}

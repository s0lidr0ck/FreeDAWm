//! Amp's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a mixer
//! strip tall: the full editor's sections, names and order, folded to fit. Header (the wordmark
//! and the preset strip; "tube amps", Undo, Bypass), the amp (a menu of the seven, each with the
//! line about it), the face (its metal strip folded to two lines, the front panel's controls in
//! their order three to a row), the rig (Input, Output with Auto Level, the four meters), and a
//! More section with the cabinet (Load IR...) and the oversampling. The gestures are the full
//! editor's own code (`super::knob`, `super::switch`, `super::toggle_pill`, `super::cabinet_ui`,
//! `super::header_controls`).

use super::{
    CHANNELS, Editor, HeaderRects, Part, amp_credit, amp_line, cab_text, cabinet_ui, face, face_parts, greyed, header_controls, knob, meters,
    normal_channel, panel, pick_amp, prelude, rig, switch, toggle_pill,
};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::editor_kit::slim::{self as sp, GAP, MODEL_H, MORE_H, MenuEntry, PAD_X, PAD_Y};
use egui::{Align2, CornerRadius, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::amp as dsp;

const STRIP_H: f32 = 48.0;
const CELL_H: f32 = 84.0;
const PILL_H: f32 = 21.0;
const RIG_H: f32 = 196.0;

/// Each amp's power and tubes, the menu's subtitle.
const TAGS: [&str; dsp::NUM_AMPS] = [
    "135 W \u{00B7} 6L6s",
    "KT66s \u{00B7} tube rectifier",
    "22 W \u{00B7} 6V6s",
    "45 W \u{00B7} 6L6s",
    "12 W \u{00B7} 6V6s",
    "30 W \u{00B7} EL84s",
    "120 W \u{00B7} EL34s",
];

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let Some((amp, pal)) = prelude(st, app, ui, host) else { return };
    let ed = host.rect;
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, super::ROOT_BACK);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: the wordmark and the preset strip; "tube amps", Undo and Bypass
    let head = row(y, 24.0);
    let title_w = tracked_width(&p, "AMP ENGINE", 13.0, 0.24, true);
    tracked_text(
        &p,
        Rect::from_min_size(head.min, vec2(title_w + 4.0, head.height())),
        Align2::LEFT_CENTER,
        "AMP ENGINE",
        13.0,
        0.24,
        true,
        true,
        pal.text_bright,
    );
    let strip = Rect::from_min_max(pos2(head.left() + title_w + 12.0, head.top()), head.max);
    let prev = Rect::from_min_size(strip.min, vec2(24.0, head.height()));
    let next = Rect::from_min_max(pos2(strip.right() - 24.0, strip.top()), strip.max);
    let name = Rect::from_min_max(pos2(prev.right() + 4.0, strip.top() + 1.0), pos2(next.left() - 4.0, strip.bottom() - 1.0));
    y += 24.0 + 6.0;
    let r2 = row(y, 24.0);
    tracked_text(&p, Rect::from_min_size(r2.min, vec2(90.0, r2.height())), Align2::LEFT_CENTER, "tube amps", 10.0, 0.12, true, false, pal.accent);
    let bypass = Rect::from_min_max(pos2(r2.right() - 62.0, r2.top()), r2.max);
    let undo = Rect::from_min_max(pos2(bypass.left() - 6.0 - 52.0, r2.top()), pos2(bypass.left() - 6.0, r2.bottom()));
    header_controls(st, app, ui, host, &pal, &HeaderRects { bypass, undo, prev, next, name });
    y += 24.0 + GAP;

    // ---- 2. the amp: one menu of the seven (the full editor's select and its foot line)
    let entries: Vec<MenuEntry> =
        dsp::AMPS.iter().enumerate().map(|(i, n)| MenuEntry { name: n, tag: TAGS[i], blurb: amp_line(i), credit: amp_credit(i) }).collect();
    let open_now = std::mem::take(&mut st.models_open);
    if let Some(i) = sp::model_menu(ui, row(y, MODEL_H), "amp", "Amp", &pal, &entries, amp, open_now) {
        pick_amp(st, app, host, i);
    }
    y += MODEL_H + GAP;

    // ---- 3. the face: the metal strip, then the front panel in its order, three to a row
    let normal = normal_channel(host, amp);
    let parts = face_parts(amp);
    let rows = parts.len().div_ceil(3) as f32;
    let pull = parts.iter().any(|q| matches!(q, Part::Dial("master", _)));
    let face_h = STRIP_H + 8.0 + rows * (CELL_H + 4.0) - 4.0 + if pull { PILL_H + 4.0 } else { 0.0 } + 6.0;
    panel(&p, row(y, face_h));
    face::paint_strip_slim(&p, Rect::from_min_size(pos2(x0 + 1.0, y + 1.0), vec2(w - 2.0, STRIP_H)), amp, normal);
    let face_bottom = y + face_h;
    y += STRIP_H + 8.0;
    let cells = sp::grid(x0 + 4.0, w - 8.0, y, parts.len(), 3, vec2(((w - 8.0) / 3.0).floor(), CELL_H), 4.0);
    for (part, cell) in parts.iter().zip(&cells) {
        match *part {
            Part::Dial(id, label) => {
                knob(st, app, ui, host, &pal, *cell, id, label, greyed(id, normal));
                // the Pull Boost pulls from the Master knob, so it sits under it
                if id == "master" {
                    let br = Rect::from_center_size(pos2(cell.center().x, cell.bottom() + 2.0 + PILL_H / 2.0), vec2(cell.width() - 4.0, PILL_H));
                    toggle_pill(st, app, ui, host, &pal, br, "pullBoost", "Pull Boost");
                }
            }
            Part::ChannelBright(ch, bright) => {
                let cr = Rect::from_min_size(pos2(cell.left() + 3.0, cell.top() + 4.0), vec2(cell.width() - 6.0, 40.0));
                switch(st, app, ui, host, &pal, cr, ch, CHANNELS, 2);
                let br = Rect::from_min_size(pos2(cell.left() + 6.0, cr.bottom() + 8.0), vec2(cell.width() - 12.0, PILL_H + 2.0));
                toggle_pill(st, app, ui, host, &pal, br, bright, "Bright");
            }
            Part::Switch(id, opts, rows, _, _) => {
                let h = (rows as f32 * 20.0).min(CELL_H - 20.0);
                let sr = Rect::from_min_size(pos2(cell.left() + 3.0, cell.top() + 4.0), vec2(cell.width() - 6.0, h));
                switch(st, app, ui, host, &pal, sr, id, opts, rows);
                if id == "orFac" {
                    // the FAC switch's label, drawn like a knob's
                    let lr = Rect::from_min_size(pos2(cell.left(), cell.bottom() - 16.0), vec2(cell.width(), 14.0));
                    tracked_text(&p, lr, Align2::CENTER_CENTER, "FAC", 9.0, 0.12, true, false, pal.text_label);
                }
            }
        }
    }
    y = face_bottom;
    y += GAP;

    // ---- 4. the rig: Input, Output and Auto Level, the meters
    let rig_r = row(y, RIG_H);
    panel(&p, rig_r);
    let [input, output, _, auto] = rig(amp);
    let g = rig_r.shrink2(vec2(6.0, 8.0));
    // Input, Output, Auto Level across; the meters under them, the whole width
    let third = (g.width() / 3.0).floor();
    knob(st, app, ui, host, &pal, Rect::from_min_size(g.min, vec2(third, 80.0)), input, "Input", false);
    let out_r = Rect::from_min_size(pos2(g.left() + third, g.top()), vec2(third, 80.0));
    knob(st, app, ui, host, &pal, out_r, output, "Output", false);
    let auto_r = Rect::from_center_size(pos2(g.left() + 2.5 * third, g.top() + 25.0), vec2(third - 4.0, 22.0));
    toggle_pill(st, app, ui, host, &pal, auto_r, auto, "Auto Level");
    let meter_area = Rect::from_min_max(pos2(g.left(), g.top() + 80.0 + 8.0), g.max);
    meters(&p, st, &pal, meter_area, amp);
    y += RIG_H + GAP;

    // ---- 5. More: the cabinet and the oversampling
    if sp::more_header(ui, row(y, MORE_H), "amp", &pal, st.slim_more, "Cabinet \u{00B7} Load IR... \u{00B7} Oversampling") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, "Cabinet", 9.5, 0.1, true, false, pal.text_label);
        y += 14.0 + 2.0;
        let cab_r = Rect::from_min_size(pos2(x0, y), vec2(w - 100.0, 26.0));
        let ir_r = Rect::from_min_size(pos2(x0 + w - 92.0, y + 2.0), vec2(92.0, 22.0));
        cabinet_ui(st, app, ui, host, &pal, cab_r, ir_r, amp);
        y += 26.0 + 2.0;
        let name_r = row(y, 18.0);
        let text = cab_text(host, amp);
        let mut job = egui::text::LayoutJob::simple_singleline(text, crate::theme::regular(10.5), pal.text_dim);
        job.wrap = egui::text::TextWrapping::truncate_at_width(w);
        let gl = ui.fonts_mut(|f| f.layout_job(job));
        p.galley(pos2(x0, name_r.center().y - gl.size().y / 2.0), gl, pal.text_dim);
        y += 18.0 + 8.0;
        tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, "Oversampling", 9.5, 0.1, true, false, pal.text_label);
        y += 14.0 + 2.0;
        switch(st, app, ui, host, &pal, Rect::from_min_size(pos2(x0, y), vec2(130.0, 26.0)), "quality", &["2x", "4x"], 1);
        y += 26.0 + 4.0;
    }
    y += PAD_Y;
    sp::settle(ui, ed.top(), y, &mut st.slim_h);
}

//! Shale's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. Header (the title with the trigger lamp and the preset strip;
//! Undo and Bypass), the mic as one menu (each mic with what its detector listens for), the
//! display with its threshold line, the five main dials (three, then two), the behaviour row
//! folded onto four rows, the Kit Link panel (Kit Link and the group; the role, Align To and
//! Reject; the status; Phase Ref, Align, Line Up, Align Offset and Flip; the phase status), the
//! footer, and a More section holding Shale's own More drawer (the ten dials, Auto Band, Trigger
//! From, SC Listen, MIDI Out). The gestures are the full editor's own code.

use super::{
    Editor, HeaderRects, K_BYPASS_RED, K_LINK_BLUE, K_PANEL_FILL, K_PANEL_LINE, LinkRects, MORE_DIALS, PALETTE, PhaseRects, behaviour_ui, display_ui,
    fit_tracked, header_controls, link_status, link_switches, live_text, main_dials, more_controls, phase_row, prelude, set_choice, tick, timer,
    title_ui,
};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, pill_height};
use crate::editor_kit::look::tracked_text;
use crate::editor_kit::palette::al;
use crate::editor_kit::slim::{self as kit, GAP, MODEL_H, MORE_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H};
use crate::editor_kit::{Host, param};
use crate::shale_ui;
use egui::{Align2, CornerRadius, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use soundcraft_dsp::analysis::REPORT_LEN;
use soundcraft_dsp::plugins::fs::shale::{shale_config as sc, shale_dsp as sd};

const DISPLAY_H: f32 = 120.0;
const MAIN_KNOB: f32 = 50.0;
const MORE_KNOB: f32 = 34.0;
const ROW_GAP: f32 = 6.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let ed = host.rect;
    let ins = host.ins;
    let p = ui.painter().clone();
    let report: Option<[f32; REPORT_LEN]> = host.meters.and_then(|m| m.analysis.iter().find(|(id, _)| *id == ins.id)).map(|(_, a)| a.report());
    let now = ui.input(|i| i.time);
    let (ro, defers) = tick(st, host, now);
    let link_on = param(host, "link") > 0.5;
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let btn_h = pill_height(PillStyle::Small);
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: SHALE and its lamp, the preset name, < >; then Undo and Bypass
    let head = row(y, ROW_H + 2.0);
    title_ui(st, &p, head, now);
    let mid = head.center().y;
    let next = Rect::from_min_size(pos2(head.right() - 26.0, mid - btn_h / 2.0), vec2(26.0, btn_h));
    let prev = Rect::from_min_size(pos2(next.left() - 3.0 - 26.0, mid - btn_h / 2.0), vec2(26.0, btn_h));
    let preset = Rect::from_min_max(pos2(head.left() + 88.0, mid - btn_h / 2.0), pos2(prev.left() - 6.0, mid + btn_h / 2.0));
    y += ROW_H + 2.0 + 6.0;
    let undo = Rect::from_min_size(pos2(x0, y), vec2(52.0, btn_h));
    let bypass = Rect::from_min_size(pos2(x0 + w - 62.0, y), vec2(62.0, btn_h));
    header_controls(st, app, ui, host, &HeaderRects { preset, prev, next, undo, more: None, bypass });
    y += btn_h + GAP + 2.0;

    // ---- 2. the mic: one menu of the eleven, each with what its detector listens for
    let items: Vec<MenuEntry> =
        sc::INSTRUMENTS.iter().enumerate().map(|(i, name)| MenuEntry { name, tag: "Mic", blurb: sd::mode(i as i32).why, credit: "" }).collect();
    let mic = param(host, "instrument").round().clamp(0.0, (sc::INSTRUMENTS.len() - 1) as f32) as usize;
    let open = std::mem::take(&mut st.mic_open);
    let mic_r = row(y, MODEL_H);
    if let Some(i) = kit::model_menu_ex(ui, mic_r, "mic", "Mic", pal, &items, mic, open, MenuLook::CENTRED) {
        set_choice(&mut st.fam, app, host, "instrument", i as f32);
    }
    y += MODEL_H + GAP;

    // ---- 3. the display
    display_ui(st, app, ui, host, row(y, DISPLAY_H));
    y += DISPLAY_H + GAP;

    // ---- 4. the five main dials, three then two
    let cells = kit::grid(x0, w, y, 5, 3, vec2((w / 3.0).floor(), MAIN_KNOB + 32.0), 0.0);
    let dials = [cells[0], cells[1], cells[2], cells[3], cells[4]];
    main_dials(st, app, ui, host, &dials);
    y = kit::bottom_of(&cells, y) + GAP;

    // ---- 5. the behaviour row: Ghost Notes and Close; DeBleed mode; Lookahead; Monitor
    let ghost = Rect::from_min_size(pos2(x0, y), vec2(100.0, ROW_H));
    let close = Rect::from_min_max(pos2(ghost.right() + ROW_GAP, y), pos2(x0 + w, y + ROW_H));
    let full = |k: f32| row(y + k * (ROW_H + ROW_GAP), ROW_H);
    behaviour_ui(st, app, ui, host, [ghost, close, full(1.0), full(2.0), full(3.0)]);
    y += 4.0 * ROW_H + 3.0 * ROW_GAP + GAP + 2.0;

    // ---- 6. the Kit Link panel
    let panel_bg = p.add(egui::Shape::Noop);
    let top = y;
    let ix = x0 + 8.0;
    let iw = w - 16.0;
    y += 8.0;
    let link_pill = Rect::from_min_size(pos2(ix, y), vec2(86.0, ROW_H));
    let group = Rect::from_min_max(pos2(link_pill.right() + ROW_GAP, y), pos2(ix + iw, y + ROW_H));
    y += ROW_H + ROW_GAP;
    let reject_w = 76.0;
    let left_w = iw - reject_w - ROW_GAP;
    let role = Rect::from_min_size(pos2(ix, y), vec2(left_w, ROW_H));
    let align_to = Rect::from_min_size(pos2(ix, y + ROW_H + ROW_GAP), vec2(left_w, ROW_H));
    let reject = Rect::from_min_size(pos2(ix + iw - reject_w, y - 2.0), vec2(reject_w, MORE_KNOB + 30.0));
    link_switches(st, app, ui, host, &LinkRects { link_pill, group, role, align_to, reject });
    y = (align_to.bottom()).max(reject.bottom()) + 4.0;
    let (status, unavailable) = link_status(host, report);
    y += kit::wrapped(ui, Rect::from_min_size(pos2(ix, y), vec2(iw, 0.0)), &status, 9.5, if unavailable { K_BYPASS_RED } else { pal.text_dim }, 3)
        + ROW_GAP;
    let measures = shale_ui::measures(ins);
    let phase_ref = Rect::from_min_size(pos2(ix, y), vec2(86.0, ROW_H));
    let align = Rect::from_min_size(pos2(phase_ref.right() + ROW_GAP, y), vec2(64.0, ROW_H));
    let line_up = Rect::from_min_max(pos2(align.right() + ROW_GAP, y), pos2(ix + iw, y + ROW_H));
    y += ROW_H + ROW_GAP;
    let flip = Rect::from_min_size(pos2(ix + iw - 54.0, y), vec2(54.0, ROW_H));
    let offset = Rect::from_min_max(pos2(ix, y + 2.0), pos2(flip.left() - ROW_GAP, y + ROW_H - 2.0));
    if measures {
        y += ROW_H + ROW_GAP;
    }
    let (phase_text, measured) = phase_row(st, app, ui, host, report, &PhaseRects { phase_ref, align, line_up, offset, flip });
    y += kit::wrapped(ui, Rect::from_min_size(pos2(ix, y), vec2(iw, 0.0)), &phase_text, 9.5, if measured { K_LINK_BLUE } else { pal.text_dim }, 3)
        + 8.0;
    let panel = Rect::from_min_max(pos2(x0, top), pos2(x0 + w, y));
    p.set(panel_bg, egui::Shape::rect_filled(panel, CornerRadius::same(5), K_PANEL_FILL));
    let line = if link_on { al(K_LINK_BLUE, 0.35) } else { K_PANEL_LINE };
    p.rect_stroke(panel.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, line), StrokeKind::Middle);
    y += GAP;

    // ---- 7. the footer: the mic's note, and the live numbers
    let why = sd::mode(mic as i32).why;
    let fitted = fit_tracked(&p, why, w, 10.0, 0.03);
    tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, &fitted, 10.0, 0.03, false, false, pal.text_faint);
    y += 14.0;
    let (live, flashing) = live_text(st, host, ro, defers, now);
    if !live.is_empty() {
        tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, &live, 10.0, 0.03, false, false, if flashing { K_LINK_BLUE } else { pal.text_faint });
        y += 14.0;
    }
    y += GAP;

    // ---- 8. More: Shale's More drawer
    let holds: Vec<&str> = MORE_DIALS.iter().map(|d| d.1).collect();
    if kit::more_header(ui, row(y, MORE_H), "shale", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 4.0;
        // The drawer's two rows of five, each as three then two.
        let cell = vec2((w / 3.0).floor(), MORE_KNOB + 32.0);
        let first = kit::grid(x0, w, y, 5, 3, cell, 0.0);
        y = kit::bottom_of(&first, y) + 4.0;
        let second = kit::grid(x0, w, y, 5, 3, cell, 0.0);
        y = kit::bottom_of(&second, y) + 4.0;
        if param(host, "autoBand") > 0.5 {
            y += kit::wrapped(
                ui,
                Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)),
                "Det Low, Det High and Crossover: linked to Auto Band",
                9.5,
                pal.accent,
                2,
            ) + 4.0;
        }
        y += 2.0;
        let mut dials = [Rect::NOTHING; 10];
        for (d, c) in dials.iter_mut().zip(first.iter().chain(&second)) {
            *d = *c;
        }
        let half = ((w - ROW_GAP) / 2.0).floor();
        let auto_band = Rect::from_min_size(pos2(x0, y), vec2(100.0, ROW_H));
        let source = Rect::from_min_max(pos2(auto_band.right() + ROW_GAP, y), pos2(x0 + w, y + ROW_H));
        let listen = Rect::from_min_size(pos2(x0, y + ROW_H + ROW_GAP), vec2(half, ROW_H));
        let midi = Rect::from_min_size(pos2(x0 + w - half, y + ROW_H + ROW_GAP), vec2(half, ROW_H));
        more_controls(st, app, ui, host, &dials, &[auto_band, source, listen, midi], true);
        y += 2.0 * ROW_H + ROW_GAP;
    }

    timer(st, ui, ro);
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

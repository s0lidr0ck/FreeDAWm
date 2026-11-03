//! ChamberMade's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]): the
//! full editor's sections, names and order, folded to fit. Header (the wordmark and the preset
//! field; More, the LED and Bypass), one display that flips between the room plan (with its
//! readouts and Add rows folded to two lines each) and More's echogram, the inspector (Room,
//! Speaker, Microphone A / B or an object) in tighter measures, Mix and Decay, and More (the
//! plugin's own: Sound paths, ft / m and the four groups of knobs) as a section under them. The
//! gestures are the full editor's own code (`super::plan_ui`, `super::inspector`,
//! `super::header_controls`, `super::knob_param`, `super::more_toggles`).

use super::controls::{self, ButtonLook, KnobOpts, cm, ui_font};
use super::{
    Editor, HeaderRects, MORE_GROUPS, Metrics, header_controls, inspector, knob_param, mix_decay, more_toggles, plan, prelude, toggle_more,
    valid_target, values,
};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::slim::{self as sp, GAP, MORE_H, PAD_X, PAD_Y};
use egui::{Align2, Id, Rect, Ui, pos2, vec2};

const HEAD_H: f32 = 28.0;
const DISPLAY_H: f32 = 250.0;
const MIX_H: f32 = 118.0;
const HELP_H: f32 = 54.0;
const MORE_CELL_H: f32 = 86.0;

/// The inspector's slim measures.
const SLIM: Metrics = Metrics {
    title_h: 32.0,
    row_h: 26.0,
    row_pitch: 30.0,
    label_w: 58.0,
    cell_h: 96.0,
    label_font: 12.5,
    box_font: 12.5,
    title_font: 14.0,
    help_font: 11.5,
    inset: 10.0,
    back_y: 4.0,
};

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let Some(mut ui_state) = prelude(st, app, ui, host) else { return };
    let ed = host.rect;
    let p = ui.painter().clone();
    p.rect_filled(ed, egui::CornerRadius::ZERO, cm::BACKGROUND);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;
    let knobs = values(host);
    let ch = crate::chamber_plan::chamber_for(ui.ctx(), Id::new(("chamber-editor", host.track.0, host.ins.id.0)), host.ins);
    let metric = ui_state.metric;

    // ---- 1. header: the wordmark, ◀ preset ▶; then More, the LED and Bypass
    let head = row(y, HEAD_H);
    let w1 = controls::text_width(&p, "Chamber", ui_font(18.0, true));
    let w2 = controls::text_width(&p, "Made", ui_font(18.0, false));
    p.text(pos2(head.left(), head.center().y), Align2::LEFT_CENTER, "Chamber", ui_font(18.0, true), cm::TEXT);
    p.text(pos2(head.left() + w1 + 1.0, head.center().y), Align2::LEFT_CENTER, "Made", ui_font(18.0, false), crate::editor_kit::argb(0xFF8FA0B0));
    let strip_l = head.left() + w1 + w2 + 10.0;
    let prev = Rect::from_min_size(pos2(strip_l, head.top()), vec2(26.0, HEAD_H));
    let next = Rect::from_min_max(pos2(head.right() - 26.0, head.top()), head.max);
    let field = Rect::from_min_max(pos2(prev.right() + 4.0, head.top()), pos2(next.left() - 4.0, head.bottom()));
    y += HEAD_H + 6.0;
    let r2 = row(y, HEAD_H);
    let more = Rect::from_min_size(r2.min, vec2(61.0, HEAD_H));
    let bypass = Rect::from_min_max(pos2(r2.right() - 71.0, r2.top()), r2.max);
    let led = Rect::from_min_max(pos2(bypass.left() - 6.0 - 18.0, r2.top()), pos2(bypass.left() - 6.0, r2.bottom()));
    header_controls(st, app, ui, host, &HeaderRects { prev, next, field, more, led, bypass }, &mut ui_state, 12.0);
    y += HEAD_H + GAP;

    // ---- 2. one display: the plan, or More's echogram
    let display = row(y, DISPLAY_H);
    let target = valid_target(host, ui_state.inspect);
    if st.slim_echo {
        controls::draw_panel(&p, display, cm::PANEL, cm::PANEL_LINE, 6.0);
        plan::echo(&p, Rect::from_min_max(display.min + vec2(8.0, 28.0), display.max - vec2(8.0, 8.0)), &ch);
        sp::display_label(ui, display, "chamber-echo", "Echogram");
    } else {
        super::plan_ui(st, app, ui, host, &ch, display, target, &knobs, &mut ui_state, false, true);
    }
    if let Some(i) = sp::display_toggle(
        ui,
        sp::toggle_rect(display, &["Plan", "Echoes"]),
        "chamber-display",
        &super::PALETTE,
        &["Plan", "Echoes"],
        usize::from(st.slim_echo),
    ) {
        st.slim_echo = i == 1;
    }
    y += DISPLAY_H + GAP;

    // ---- 3. the inspector
    let target = valid_target(host, ui_state.inspect);
    let area = row(y, inspector_height(target));
    controls::draw_panel(&p, area, cm::PANEL, cm::PANEL_LINE, 6.0);
    inspector(st, app, ui, host, &ch, area, target, &mut ui_state, metric, &SLIM);
    y = area.bottom() + GAP;

    // ---- 4. Mix and Decay
    let mix_area = row(y, MIX_H);
    controls::draw_panel(&p, mix_area, cm::PANEL, cm::PANEL_LINE, 6.0);
    let mx = mix_area.shrink2(vec2(5.0, 6.0));
    let cw = (mx.width() / 2.0).trunc();
    mix_decay(
        st,
        app,
        ui,
        host,
        &ch,
        metric,
        Rect::from_min_size(mx.min, vec2(cw, mx.height())),
        Rect::from_min_max(pos2(mx.left() + cw, mx.top()), mx.max),
    );
    y += MIX_H + GAP;

    // ---- 5. More: Sound paths, ft / m, and the four groups of knobs
    let holds: Vec<&str> = MORE_GROUPS.iter().map(|(t, _)| *t).collect();
    if sp::more_header(
        ui,
        row(y, MORE_H),
        "chamber",
        &super::PALETTE,
        ui_state.more,
        &format!("Sound paths \u{00B7} ft / m \u{00B7} {}", holds.join(" \u{00B7} ")),
    ) {
        toggle_more(app, host, &mut ui_state);
    }
    y += MORE_H;
    if ui_state.more {
        y += 6.0;
        let h = 30.0;
        let rays = Rect::from_min_size(pos2(x0, y), vec2(106.0, h));
        let m_r = Rect::from_min_max(pos2(x0 + w - 37.0, y), pos2(x0 + w, y + h));
        let ft = Rect::from_min_max(pos2(m_r.left() - 37.0, y), pos2(m_r.left(), y + h));
        more_toggles(app, ui, host, &mut ui_state, rays, ft, m_r, ButtonLook { font_h: 12.0, ..ButtonLook::default() });
        y += h + GAP;
        for (g, (title, list)) in MORE_GROUPS.iter().enumerate() {
            let per_row = if list.len() == 4 { 2 } else { 3 };
            let rows = list.len().div_ceil(per_row) as f32;
            let gh = 24.0 + rows * (MORE_CELL_H + 2.0) + 4.0;
            let gr = row(y, gh);
            controls::draw_panel(&p, gr, cm::GROUP, cm::GROUP_LINE, 5.0);
            let tc = if g == 3 { cm::TAPE } else { cm::LABEL };
            p.text(pos2(gr.left() + 10.0, gr.top() + 13.0), Align2::LEFT_CENTER, *title, ui_font(12.5, false), tc);
            let cells = sp::grid(
                gr.left() + 2.0,
                gr.width() - 4.0,
                gr.top() + 24.0,
                list.len(),
                per_row,
                vec2(((gr.width() - 4.0) / 3.0).floor(), MORE_CELL_H),
                2.0,
            );
            for ((i, label, _), cell) in list.iter().zip(&cells) {
                let arc = if g == 3 { cm::TAPE } else { cm::ARC };
                knob_param(st, app, ui, host, *cell, *i, &ch, metric, KnobOpts { label, arc, ..KnobOpts::default() });
            }
            y += gh + 6.0;
        }
    }
    y += PAD_Y;
    sp::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// The inspector's height for `target` in the slim measures: the title, its dropdown rows, the
/// knob row, the help (and Remove for an object).
fn inspector_height(target: i32) -> f32 {
    let rows = match target {
        plan::SEL_ROOM => 4.0,
        plan::SEL_SPEAKER => 1.0,
        plan::SEL_MIC_A | plan::SEL_MIC_B => 2.0,
        _ => 1.0,
    };
    let object = target >= plan::SEL_OBJECT0;
    SLIM.title_h + rows * SLIM.row_pitch + 4.0 + SLIM.cell_h + 4.0 + 10.0 + HELP_H + if object { SLIM.row_h + 10.0 } else { 0.0 }
}

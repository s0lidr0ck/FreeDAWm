//! LevelUp's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections, names and order, folded to fit. The LINK / REF routing rows stay above it (the
//! frame's, as for the full editor). Header (LEVEL UP and the preset name; the Level / Reference /
//! Deliver tabs and Bypass), the pairing row (Role, Pair) and its status line, then the open tab:
//! Level (the scope, the correction, the five dials 3 + 2, the foot in two rows), Reference (Mix /
//! Reference and the four slots, compact) or Deliver (the three readings, Reset, the platforms two
//! lines each, Preview / Shaped / Dither). The tabs already fold the editor, so there is no More.
//! The gestures are the full editor's own (`super::knob`, `super::foot_ui`, `super::ref_row`,
//! `super::platform_click`, `super::deliver_foot` …).

use super::{
    ACCENT, BAD, BG, DIM, Editor, LABEL, PANEL, TEXT, bypass_ui, d, deliver_foot, file_drop, foot_ui, idx, knob, listen_ui, name_ui,
    paint_correction, paint_scope, pairing_ui, panel_sel, platform_click, platform_target, platform_verdict, prelude, preview_text, ref_row,
    ref_status, reset_ui, status_line, tabs_ui,
};
use crate::SoundApp;
use crate::editor_kit::controls::{PillStyle, pill_width};
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::editor_kit::palette::al;
use crate::editor_kit::slim::{self as sk, PAD_X, PAD_Y};
use crate::editor_kit::{Host, param};
use crate::levelup_ui::lufs_text;
use crate::theme::regular;
use egui::{Align2, CornerRadius, Rect, Stroke, StrokeKind, Ui, pos2, vec2};

const GAP: f32 = 8.0;
const ROW_H: f32 = 24.0;
const SCOPE_H: f32 = 150.0;
const KNOB: f32 = 50.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    let Some((rd, history)) = prelude(st, app, ui, host) else { return };
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, BG);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header: LEVEL UP and the preset name; the tabs and Bypass
    let head = row(y, ROW_H);
    let w1 = tracked_width(&p, "Level", 15.0, 0.22, true);
    let w2 = tracked_width(&p, "Up", 15.0, 0.22, true);
    tracked_text(&p, Rect::from_min_size(head.min, vec2(w1 + 2.0, head.height())), Align2::LEFT_CENTER, "Level", 15.0, 0.22, true, false, TEXT);
    let up = Rect::from_min_size(pos2(head.left() + w1 + 6.0, head.top()), vec2(w2 + 2.0, head.height()));
    tracked_text(&p, up, Align2::LEFT_CENTER, "Up", 15.0, 0.22, true, true, ACCENT);
    name_ui(st, app, ui, host, Rect::from_min_max(pos2(up.right() + 10.0, head.top()), head.max));
    y += ROW_H + 6.0;
    let bw = pill_width(ui, "Bypass", PillStyle::Small);
    tabs_ui(st, ui, Rect::from_min_max(pos2(x0, y), pos2(x0 + w - bw - 8.0, y + ROW_H)));
    bypass_ui(app, ui, host, Rect::from_min_size(pos2(x0 + w - bw, y + 1.0), vec2(bw, 22.0)));
    y += ROW_H + GAP;

    // ---- 2. the pairing row and what this instance is doing in its pair
    let role_r = Rect::from_min_size(pos2(x0, y), vec2((w * 0.54).round(), 22.0));
    let pair_r = Rect::from_min_max(pos2(role_r.right() + 8.0, y), pos2(x0 + w, y + 22.0));
    pairing_ui(app, ui, host, role_r, pair_r);
    y += 22.0 + 4.0;
    let (text, colour) = status_line(host, &rd);
    y += sk::wrapped(ui, row(y, 50.0), &text, 10.5, colour, 3) + GAP;

    // ---- 3. the open tab
    y = match st.tab {
        0 => level_tab(st, app, ui, host, row(y, 0.0), &rd, &history),
        1 => reference_tab(st, app, ui, host, row(y, 0.0), &rd),
        _ => deliver_tab(app, ui, host, row(y, 0.0), &rd),
    };
    let rows = st.slim_rows.unwrap_or([Rect::NOTHING; 4]);
    file_drop(app, ui, host, st, ed, rows);
    y += PAD_Y;
    sk::settle(ui, ed.top(), y, &mut st.slim_h);
}

/// The Level tab from `r`'s top; returns its bottom.
fn level_tab(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    r: Rect,
    rd: &super::Readings,
    history: &crate::levelup_ui::LevelUpHistory,
) -> f32 {
    let p = ui.painter().clone();
    let (x0, w) = (r.left(), r.width());
    let mut y = r.top();
    paint_scope(st, &p, Rect::from_min_size(pos2(x0, y), vec2(w, SCOPE_H)), host, rd, history, 22.0);
    y += SCOPE_H + GAP;
    paint_correction(&p, Rect::from_min_size(pos2(x0, y), vec2(w, 44.0)), host, rd, 104.0, 26.0);
    y += 44.0 + GAP;
    // the five dials: Target, Attack, Release; Gate, Max Gain
    let cell = vec2((w / 3.0).floor(), KNOB + 32.0);
    let cells = sk::grid(x0, w, y, 5, 3, cell, 2.0);
    let dials = [("target", "Target"), ("attack", "Attack"), ("release", "Release"), ("gate", "Gate"), ("maxGain", "Max Gain")];
    for ((id, label), c) in dials.into_iter().zip(&cells) {
        knob(app, ui, host, *c, id, label);
    }
    y = sk::bottom_of(&cells, y) + GAP;
    // the foot: Learn, Auto / Static, Normalize; Freeze, Ceiling
    let h = 26.0;
    let third = ((w - 2.0 * 6.0) / 3.0).floor();
    let at = |col: f32, row: f32, cw: f32| Rect::from_min_size(pos2(x0 + col, y + row * (h + 6.0)), vec2(cw, h));
    let half = ((w - 6.0) / 2.0).floor();
    let cells =
        [at(0.0, 0.0, third), at(third + 6.0, 0.0, third), at(2.0 * (third + 6.0), 0.0, third), at(0.0, 1.0, half), at(half + 6.0, 1.0, half)];
    foot_ui(st, app, ui, host, cells);
    y + 2.0 * h + 6.0
}

/// The Reference tab from `r`'s top; returns its bottom.
fn reference_tab(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, rd: &super::Readings) -> f32 {
    let (x0, w) = (r.left(), r.width());
    let mut y = r.top();
    listen_ui(app, ui, host, Rect::from_min_size(pos2(x0, y), vec2(w, 26.0)));
    y += 26.0 + GAP;
    let mut rows = [Rect::NOTHING; 4];
    for (s, slot) in rows.iter_mut().enumerate() {
        *slot = Rect::from_min_size(pos2(x0, y), vec2(w, 50.0));
        ref_row(app, ui, host, *slot, s, true);
        y += 50.0 + 6.0;
    }
    st.slim_rows = Some(rows);
    y += 4.0;
    y + sk::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 120.0)), &ref_status(host, rd), 11.0, DIM, 7)
}

/// The Deliver tab from `r`'s top; returns its bottom.
fn deliver_tab(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, rd: &super::Readings) -> f32 {
    let p = ui.painter().clone();
    let (x0, w) = (r.left(), r.width());
    let mut y = r.top();
    let (integ, tp, st_out) = (rd.or(3, -100.0), rd.or(4, -120.0), rd.or(2, -100.0));
    // the three readings
    let cw = (w / 3.0).floor();
    let big = |i: usize, value: String, label: &str, c| {
        let at = pos2(x0 + cw * i as f32, y);
        p.text(at + vec2(0.0, 12.0), Align2::LEFT_CENTER, value, regular(22.0), c);
        tracked_text(&p, Rect::from_min_size(at + vec2(0.0, 26.0), vec2(cw - 4.0, 12.0)), Align2::LEFT_CENTER, label, 7.5, 0.12, true, false, LABEL);
    };
    big(0, lufs_text(integ), "LUFS integ.", TEXT);
    big(1, if tp <= -119.0 { "--".into() } else { format!("{tp:.1}") }, "dBTP true pk", if tp > -1.0 { BAD } else { TEXT });
    big(2, lufs_text(st_out), "LUFS short", DIM);
    y += 42.0;
    let reset_w = 70.0;
    reset_ui(app, ui, host, Rect::from_min_size(pos2(x0 + w - reset_w, y), vec2(reset_w, 22.0)));
    if tp > -1.0 {
        let warn = "True peak above -1 dBTP: MP3/AAC encoding can push this over and clip.";
        sk::wrapped(ui, Rect::from_min_size(pos2(x0, y), vec2(w - reset_w - 8.0, 40.0)), warn, 10.0, BAD, 3);
        y += 40.0;
    } else {
        y += 22.0 + 6.0;
    }
    // the platforms: name and target, then what it would do
    let chosen = idx(host, "platform") as i32;
    let row_h = 36.0;
    for i in 1..d::NUM_PLATFORMS as i32 {
        let pf = d::platform(i);
        let rr = Rect::from_min_size(pos2(x0, y), vec2(w, row_h));
        platform_click(app, ui, host, rr, i);
        let b = rr.shrink2(vec2(0.0, 1.5));
        let sel = i == chosen;
        p.rect_filled(b, CornerRadius::same(3), if sel { panel_sel() } else { PANEL });
        if sel {
            p.rect_stroke(b.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, al(ACCENT, 0.7)), StrokeKind::Middle);
        }
        let t = b.shrink2(vec2(10.0, 0.0));
        p.text(pos2(t.left(), t.top() + 10.0), Align2::LEFT_CENTER, pf.name, regular(12.0), if sel { ACCENT } else { TEXT });
        p.text(pos2(t.right(), t.top() + 10.0), Align2::RIGHT_CENTER, platform_target(i), regular(10.0), DIM);
        let (verdict, vc) = platform_verdict(i, integ, tp);
        p.with_clip_rect(t.intersect(p.clip_rect())).text(pos2(t.left(), t.top() + 24.5), Align2::LEFT_CENTER, verdict, regular(10.5), vc);
        y += row_h;
    }
    y += GAP;
    // the foot: Preview and Shaped; Dither under them
    let half = ((w - 8.0) / 2.0).floor();
    let preview_r = Rect::from_min_size(pos2(x0, y), vec2(half, 26.0));
    let shaping_r = Rect::from_min_size(pos2(x0 + half + 8.0, y), vec2(half, 26.0));
    let dither_r = Rect::from_min_size(pos2(x0, y + 26.0 + 6.0), vec2(w, 26.0));
    deliver_foot(app, ui, host, preview_r, dither_r, shaping_r);
    y += 2.0 * 26.0 + 6.0;
    if param(host, "preview") > 0.5 && chosen > 0 {
        let g = if rd.preview_gain.is_finite() { rd.preview_gain } else { 0.0 };
        p.text(pos2(x0, y + 10.0), Align2::LEFT_CENTER, preview_text(chosen, g), regular(11.0), ACCENT);
        y += 20.0;
    }
    y
}

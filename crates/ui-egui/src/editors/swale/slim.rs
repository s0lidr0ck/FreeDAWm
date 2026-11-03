//! Swale's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections, names and order, folded to fit. Header (SWALE and the preset strip; Undo, A/B,
//! Bypass), the mode row, the screen, the stages (Attack, Hold or Hang, Release with their Sync
//! pills), Sens or Rate, Bloom, Mix, the curve row, and More: Swale's own More panel (Retrig, Low
//! Cut, Floor, Alternate, Input, Level, the VCA) and the tempo foot. More is Swale's `showMore`, so
//! the full editor and the slim one open and close it together. The gestures are the full
//! editor's own (`super::header_controls`, `super::dial_ctl`, the family's attachments).

use super::{
    Editor, FAM, Header, K_ROW_LABEL, PALETTE, dial_ctl, effective_bpm, header_controls, screen_settings, tempo_runs, toggle_more, tracked_look,
};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillStyle, SelectionStyle, pill_height, pill_width};
use crate::editor_kit::family::{pill_param, segmented_param};
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::editor_kit::slim::{self as sk, PAD_X, PAD_Y};
use crate::editor_kit::{param, readouts};
use egui::{Align2, CornerRadius, Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::fs::swale::swale_config as sc;

const GAP: f32 = 8.0;
const ROW_H: f32 = 22.0;
const SCREEN_H: f32 = 160.0;
const STAGE_KNOB: f32 = 48.0;
const SMALL_KNOB: f32 = 36.0;
const SYNC_H: f32 = 19.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &PALETTE;
    let ed = host.rect;
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, super::K_ROOT_BACK);
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => crate::editor_kit::presets::open(app, host, &mut st.fam.presets),
            "More" if !st.more => toggle_more(st, app, host),
            _ => {}
        }
    }
    if crate::editor_kit::family::browser(app, ui, host, pal, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return;
    }
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;
    let ro = readouts(host);
    let bpm = effective_bpm(host, ro);
    let manual = param(host, "tempoSrc") > 0.5;
    let a = sc::what_applies(
        param(host, "mode").round() as i32,
        param(host, "atkSync") > 0.5,
        param(host, "holdSync") > 0.5,
        param(host, "relSync") > 0.5,
        manual,
        true,
    );

    // ---- 1. header: SWALE and the preset strip; then Undo, A/B, Bypass
    let head = row(y, ROW_H + 2.0);
    let tw = tracked_width(&p, "Swale", 17.0, 0.22, true);
    tracked_text(&p, Rect::from_min_size(head.min, vec2(tw + 4.0, head.height())), Align2::LEFT_CENTER, "Swale", 17.0, 0.22, true, true, pal.accent);
    let (prev, name, next) = sk::preset_strip(head, tw);
    y += ROW_H + 2.0 + 6.0;
    let widths = [pill_width(ui, "Undo", PillStyle::Small).floor(), 30.0, pill_width(ui, "Bypass", PillStyle::Small).floor().max(66.0)];
    let cells = sk::pill_row(x0, w, y, &widths, &[0]);
    let hd = Header { title: head, bypass: cells[2], prev, name, next, undo: cells[0], ab: cells[1], more: None };
    header_controls(st, app, ui, host, &hd);
    y += pill_height(PillStyle::Small) + GAP + 2.0;

    // ---- 2. the mode (three: stays a segmented row)
    segmented_param(app, ui, host, &mut st.fam, &FAM, row(y, 26.0), pal, "mode", Some(&sc::MODES), 1, SelectionStyle::Outlined, true);
    y += 26.0 + GAP;

    // ---- 3. the screen
    let live = ro.map(|r| crate::module_graphs::LiveValues { readouts: r, ..Default::default() });
    let hist = crate::swale_ui::history(ui.ctx(), crate::module_graphs::module_key(host.track.0, host.ins.id.0), live.as_ref());
    let now = ui.input(|i| i.time);
    let settings = screen_settings(st, host, ro, bpm, now);
    super::display::paint(&p, row(y, SCREEN_H), pal, &settings, &hist, now < st.led_until, &mut st.scratch);
    y += SCREEN_H + GAP;

    // ---- 4. the stages with their Sync pills, then Sens or Rate, Bloom, Mix
    let cell = vec2((w / 3.0).floor(), STAGE_KNOB + 32.0);
    let stages = sk::grid(x0, w, y, 3, 3, cell, 0.0);
    let stage_rows = [
        ("attack", "atkNote", "atkSync", a.attack_note, "Attack"),
        ("hold", "holdNote", "holdSync", a.hold_note, a.hold_label),
        ("release", "relNote", "relSync", a.release_note, "Release"),
    ];
    for (slot, (time_id, note_id, sync_id, synced, label)) in stages.iter().zip(stage_rows) {
        dial_ctl(app, ui, host, &mut st.fam, *slot, if synced { note_id } else { time_id }, label, DialOpts::default());
        let sync_r = Rect::from_center_size(pos2(slot.center().x, slot.bottom() + 2.0 + SYNC_H * 0.5), vec2(54.0, SYNC_H));
        pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, sync_id, "Sync", tracked_look(None));
    }
    y = sk::bottom_of(&stages, y) + 2.0 + SYNC_H + GAP;
    let dials = sk::grid(x0, w, y, 3, 3, cell, 0.0);
    if a.sens {
        dial_ctl(app, ui, host, &mut st.fam, dials[0], "sens", "Sens", DialOpts::default());
    }
    if a.rate {
        dial_ctl(app, ui, host, &mut st.fam, dials[0], "rate", "Rate", DialOpts::default());
    }
    dial_ctl(app, ui, host, &mut st.fam, dials[1], "bloom", "Bloom", DialOpts::default());
    dial_ctl(app, ui, host, &mut st.fam, dials[2], "mix", "Mix", DialOpts::default());
    y = sk::bottom_of(&dials, y) + GAP;

    // ---- 5. the curve row: the four curves; Peak on Beat (Rhythmic), Per Note / Phrase (Pick)
    let label_w = 44.0;
    tracked_text(&p, Rect::from_min_size(pos2(x0, y), vec2(label_w, 24.0)), Align2::LEFT_CENTER, "Curve", 9.0, 0.18, true, false, K_ROW_LABEL);
    let cr = Rect::from_min_max(pos2(x0 + label_w, y), pos2(x0 + w, y + 24.0));
    segmented_param(app, ui, host, &mut st.fam, &FAM, cr, pal, "curve", None, 1, SelectionStyle::Outlined, true);
    y += 24.0 + 6.0;
    if a.peak_beat {
        let pr = Rect::from_min_size(pos2(x0 + label_w, y), vec2(98.0, pill_height(PillStyle::Small)));
        pill_param(app, ui, host, &mut st.fam, &FAM, pr, pal, "peakBeat", "Peak on Beat", tracked_look(None));
        y += pr.height() + 6.0;
    }
    if a.phrase {
        let pr = Rect::from_min_max(pos2(x0 + label_w, y), pos2(x0 + w, y + 24.0));
        segmented_param(app, ui, host, &mut st.fam, &FAM, pr, pal, "phrase", None, 1, SelectionStyle::Outlined, true);
        y += 24.0 + 6.0;
    }
    let blurb = sc::curve_blurb(param(host, "curve").round() as i32);
    y += sk::wrapped(ui, Rect::from_min_size(pos2(x0, y), vec2(w, 60.0)), blurb, 10.0, pal.text_dim, 3) + GAP;

    // ---- 6. More: Swale's More panel and the tempo foot
    let summary = "Retrig \u{00B7} Low Cut \u{00B7} Floor \u{00B7} Alternate \u{00B7} Input \u{00B7} Level \u{00B7} VCA \u{00B7} Tempo";
    if sk::more_header(ui, row(y, 20.0), "swale", pal, st.more, summary) {
        toggle_more(st, app, host);
    }
    y += 20.0;
    if st.more {
        y += 6.0;
        let top = y;
        let slot = sk::panel_slot(&p);
        y += 8.0;
        let dim = a.dim_detector;
        let more_dials = [
            ("retrig", "Retrig", dim),
            ("detHp", "Low Cut", dim),
            ("floorAmt", "Floor", false),
            ("alternate", "Alternate", false),
            ("trim", "Input", false),
            ("level", "Level", false),
        ];
        // two to a row: Retrig and Low Cut with their readouts under them (they carry "· linked"
        // while the detector is off), the rest with the readout beside the knob
        let below = vec2((w / 2.0).floor(), SMALL_KNOB + 32.0);
        let top_cells = sk::grid(x0, w, y, 2, 2, below, 0.0);
        y = sk::bottom_of(&top_cells, y) + 4.0;
        let cell = vec2((w / 2.0).floor() - 4.0, SMALL_KNOB + 4.0);
        let mut cells = sk::grid(x0 + 4.0, w - 8.0, y, 4, 2, cell, 4.0);
        cells.splice(0..0, top_cells);
        for (i, ((id, label, locked), r)) in more_dials.into_iter().zip(&cells).enumerate() {
            let layout = if i < 2 { LabelLayout::Below } else { LabelLayout::Right };
            let o = DialOpts { locked, small: true, layout, ..DialOpts::default() };
            dial_ctl(app, ui, host, &mut st.fam, *r, id, label, o);
        }
        y = sk::bottom_of(&cells, y) + 6.0;
        let inner = Rect::from_min_max(pos2(x0 + 8.0, y), pos2(x0 + w - 8.0, y));
        tracked_text(&p, Rect::from_min_size(inner.min, vec2(40.0, 24.0)), Align2::LEFT_CENTER, "VCA", 9.0, 0.18, true, false, K_ROW_LABEL);
        let seg = Rect::from_min_max(pos2(inner.left() + 40.0, y), pos2(inner.right(), y + 24.0));
        segmented_param(app, ui, host, &mut st.fam, &FAM, seg, pal, "vca", None, 1, SelectionStyle::Outlined, true);
        y += 24.0 + 4.0;
        let vb = sc::vca_blurb(param(host, "vca").round() as i32);
        y += sk::wrapped(ui, Rect::from_min_size(pos2(inner.left(), y), vec2(inner.width(), 60.0)), vb, 10.0, pal.text_dim, 4) + 8.0;
        sk::panel_under(&p, slot, Rect::from_min_max(pos2(x0, top), pos2(x0 + w, y)), 4.0);
        y += GAP;

        // the tempo foot: source, the BPM knob when it applies, the readout, the advice
        tracked_text(&p, Rect::from_min_size(pos2(x0, y), vec2(44.0, 26.0)), Align2::LEFT_CENTER, "Tempo", 9.0, 0.18, true, false, K_ROW_LABEL);
        let ts = Rect::from_min_size(pos2(x0 + 50.0, y), vec2(116.0, 26.0));
        segmented_param(app, ui, host, &mut st.fam, &FAM, ts, pal, "tempoSrc", None, 1, SelectionStyle::Outlined, true);
        if a.bpm_knob {
            let br = Rect::from_min_max(pos2(ts.right() + 8.0, y - 4.0), pos2(x0 + w, y + 30.0));
            dial_ctl(app, ui, host, &mut st.fam, br, "bpm", "BPM", DialOpts { layout: LabelLayout::Right, ..DialOpts::default() });
        }
        y += 26.0 + 8.0;
        let mut rx = x0 + 50.0;
        for (text, c) in tempo_runs(host, bpm) {
            let tw = tracked_width(&p, &text, 10.0, 0.12, true);
            tracked_text(&p, Rect::from_min_size(pos2(rx, y), vec2(tw, 14.0)), Align2::LEFT_CENTER, &text, 10.0, 0.12, true, false, c);
            rx += tw;
        }
        y += 14.0 + 6.0;
        tracked_text(&p, row(y, 14.0), Align2::LEFT_CENTER, "Put me before delay and reverb", 10.0, 0.12, true, false, pal.text_faint);
        y += 14.0;
    }
    y += PAD_Y;

    if ro.is_some() || now < st.led_until || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
    sk::settle(ui, ed.top(), y, &mut st.slim_h);
}

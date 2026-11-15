//! CopyQ's editor in the classic drawer's width: the full editor's sections stacked. Header, the
//! display (its range edges drag here too), Amount with Detail and Max Gain, Match To and the
//! library, Capture and Save, and a More section holding the other dials, the switches, the
//! link, Follow / Freeze / Reset, a profile from a file or the selection, and Send to CleanQ.
//! The gestures are the full editor's own code.

use super::{
    AUDIO_EXTS, Editor, FAMILY, PALETTE, button, dial_ui, display_ui, freeze, frozen, library_ui, measure_and_use, measured, name_box, pill_ui,
    pill_w, prelude, save_measured, send_to_cleanq, state_of, targets_ui, unfreeze,
};
use crate::SoundApp;
use crate::editor_kit::controls::{Btn, PillStyle, SelectionStyle, card_button, pill_height};
use crate::editor_kit::family;
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, paint_background, paint_panel};
use crate::editor_kit::slim::{self as kit, GAP, MORE_H, PAD_X, PAD_Y, ROW_H};
use crate::editor_kit::{Host, param};
use crate::theme::regular;
use egui::{Align2, Rect, Sense, Ui, pos2, vec2};
use serde_json::json;

const DISPLAY_H: f32 = 170.0;
const BIG_KNOB: f32 = 64.0;
const SMALL_KNOB: f32 = 40.0;
/// A dial's label and readout under its knob.
const DIAL_TEXT: f32 = 30.0;
const LIBRARY_H: f32 = 132.0;

/// The dials in More: (parameter, label).
const MORE_DIALS: [(&str, &str); 8] = [
    ("dynamics", "Dynamics"),
    ("window", "Window"),
    ("gate", "Gate"),
    ("output", "Output"),
    ("amtLow", "Low"),
    ("amtLowMid", "Low Mid"),
    ("amtHighMid", "High Mid"),
    ("amtHigh", "High"),
];

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    paint_background(ui.painter(), ed);
    let state = state_of(app, ui, host);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let btn_h = pill_height(PillStyle::Small);
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("CopyQ", ""), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. the display
    display_ui(st, app, ui, host, &state, row(y, DISPLAY_H), true);
    y += DISPLAY_H + GAP;

    // ---- 3. Amount, with Detail and Max Gain beside it
    let panel_h = 8.0 + BIG_KNOB + DIAL_TEXT + 8.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let third = (w / 3.0).floor();
    for (i, (id, label, big)) in [("amount", "Amount", true), ("detail", "Detail", false), ("limit", "Max Gain", false)].into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(x0 + third * i as f32, y + 8.0), vec2(third, BIG_KNOB + DIAL_TEXT));
        let knob = if big { BIG_KNOB } else { SMALL_KNOB };
        let cell = Rect::from_center_size(col.center(), vec2((third - 4.0).min(knob + 40.0), knob + DIAL_TEXT));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, !big);
    }
    y += panel_h + GAP;

    // ---- 4. Match To and the library
    let targets_h = 16.0 + 22.0 * state.targets.len().max(1) as f32;
    let side_h = 8.0 + targets_h + 6.0 + 16.0 + LIBRARY_H + 8.0;
    let side = row(y, side_h);
    paint_panel(ui.painter(), side, 10.0);
    let s = side.shrink(8.0);
    let ty = targets_ui(st, app, ui, host, &state, s);
    library_ui(st, app, ui, host, &state, Rect::from_min_max(pos2(s.left(), ty + 6.0), s.max));
    y += side_h + GAP;

    // ---- 5. Capture, the name and Save
    let have = measured(host, "").is_some();
    let cap_w = pill_w(ui, "Capture");
    pill_ui(app, ui, host, &mut st.fam, Rect::from_min_size(pos2(x0, y + (ROW_H - btn_h) / 2.0), vec2(cap_w, btn_h)), "capture", "Capture")
        .on_hover_text("Measure everything that plays from now on. The audio passes untouched while it is on");
    let save_w = 46.0;
    name_box(st, ui, Rect::from_min_max(pos2(x0 + cap_w + 6.0, y), pos2(x0 + w - save_w - 6.0, y + ROW_H)));
    if button(
        ui,
        Rect::from_min_size(pos2(x0 + w - save_w, y), vec2(save_w, ROW_H)),
        "save",
        "Save",
        have,
        "Save what was measured as a profile in the library",
    ) {
        save_measured(st, app, ui, host);
    }
    y += ROW_H + GAP;

    // ---- 6. More
    if kit::more_header(ui, row(y, MORE_H), "copyq", pal, st.slim_more, "Ranges \u{00B7} Stereo \u{00B7} Phase \u{00B7} Link \u{00B7} Freeze") {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cells = kit::grid(x0, w, y, MORE_DIALS.len(), 4, vec2((w / 4.0).floor(), SMALL_KNOB + DIAL_TEXT + 2.0), 2.0);
        for ((id, label), r) in MORE_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, true);
        }
        y = kit::bottom_of(&cells, y) + GAP;
        family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, row(y, ROW_H), pal, "channels", None, 1, SelectionStyle::Filled, false);
        y += ROW_H + 6.0;
        family::segmented_param(
            app,
            ui,
            host,
            &mut st.fam,
            &FAMILY,
            row(y, ROW_H),
            pal,
            "phase",
            Some(&["Linear Phase", "Minimum Phase"]),
            1,
            SelectionStyle::Filled,
            false,
        );
        y += ROW_H + 6.0;
        // Match Level, Delta, Follow
        let mut x = x0;
        for (id, text, tip) in [
            ("autoGain", "Match Level", "Give back the level the match changes"),
            ("delta", "Delta", "Play only what the match changes"),
            ("follow", "Follow", "Keep measuring this track as it plays. Off holds the match where it is"),
        ] {
            let pw = pill_w(ui, text);
            pill_ui(app, ui, host, &mut st.fam, Rect::from_min_size(pos2(x, y + (ROW_H - btn_h) / 2.0), vec2(pw, btn_h)), id, text)
                .on_hover_text(tip);
            x += pw + 6.0;
        }
        y += ROW_H + 6.0;
        // Link and its group
        ui.painter().text(pos2(x0, y + ROW_H / 2.0), Align2::LEFT_CENTER, "Link", regular(LABEL_SIZE), pal.text_body);
        let group_w = 70.0;
        family::segmented_param(
            app,
            ui,
            host,
            &mut st.fam,
            &FAMILY,
            Rect::from_min_max(pos2(x0 + 30.0, y), pos2(x0 + w - group_w - 6.0, y + ROW_H)),
            pal,
            "link",
            None,
            1,
            SelectionStyle::Filled,
            false,
        );
        let group_text = host.ins.link_group.map_or_else(|| "No group".to_string(), |g| format!("Group {g}"));
        let group_r = Rect::from_min_size(pos2(x0 + w - group_w, y), vec2(group_w, ROW_H));
        let resp = card_button(ui, group_r, "copyq-group", pal, Btn::new(&group_text).on(host.ins.link_group.is_some()), Sense::click())
            .on_hover_text("The link group this module is in: modules in one group (on any track) share what they measure");
        egui::Popup::menu(&resp).show(|ui| crate::link_picker::link_menu(app, ui, host.track, host.ins, true));
        y += ROW_H + 6.0;
        // Freeze / Unfreeze, Reset
        let half = ((w - 6.0) / 2.0).floor();
        let (left, right) = (Rect::from_min_size(pos2(x0, y), vec2(half, ROW_H)), Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, ROW_H)));
        if frozen(&state) {
            if button(ui, left, "unfreeze", "Unfreeze", true, "Drop the frozen sound and follow this track as it plays again") {
                unfreeze(st, app, ui, host);
            }
        } else if button(ui, left, "freeze", "Freeze", have, "Keep this track's sound as measured so far, so the match holds still") {
            freeze(st, app, ui, host);
        }
        if button(ui, right, "reset", "Reset", true, "Forget what was measured of this track and start over") {
            crate::editor_kit::action(app, host, "reset_measure");
        }
        y += ROW_H + 6.0;
        // A profile from elsewhere
        let (left, right) = (Rect::from_min_size(pos2(x0, y), vec2(half, ROW_H)), Rect::from_min_size(pos2(x0 + half + 6.0, y), vec2(half, ROW_H)));
        let can_pick = app.services.pick_open.is_some();
        if button(ui, left, "file", "From File\u{2026}", can_pick, "Measure an audio file as a profile and match to it")
            && let Some(open) = &app.services.pick_open
            && let Some(path) = open("mix.copyq_measure", &AUDIO_EXTS)
        {
            measure_and_use(st, app, ui, host, &state, json!({"path": path}));
        }
        if button(
            ui,
            right,
            "selection",
            "From Selection",
            true,
            "Measure the edit selection on the selected audio track as a profile and match to it",
        ) {
            measure_and_use(st, app, ui, host, &state, json!({}));
        }
        y += ROW_H + 6.0;
        if button(
            ui,
            row(y, ROW_H),
            "cleanq",
            "Send to CleanQ",
            true,
            "Turn the response being applied right now into a CleanQ with editable bells after this module, and bypass CopyQ",
        ) {
            send_to_cleanq(st, app, ui, host);
        }
        y += ROW_H + 6.0;
        let (lo, hi) = (param(host, "rangeLo"), param(host, "rangeHi"));
        let (lo, hi) = if lo <= hi { (lo, hi) } else { (hi, lo) };
        let text = format!("Works from {} to {}: drag the display's edges", super::hz_text(lo), super::hz_text(hi));
        y += kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)), &text, CAPTION_SIZE, pal.text_faint, 2);
    }
    // The last message, for a while.
    let now = ui.input(|i| i.time);
    if let Some((m, at)) = &st.message
        && now - *at < super::MESSAGE_SECONDS
    {
        y += 4.0;
        y += kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)), m, CAPTION_SIZE, pal.accent, 3);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

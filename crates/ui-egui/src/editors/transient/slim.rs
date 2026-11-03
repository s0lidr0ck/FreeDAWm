//! Transient's editor in the classic drawer's width, about a mixer strip tall: the full editor's
//! sections in its order, stacked. Header (title and preset strip; Undo, A, B, Bypass), Source as
//! one menu (each source with its description), the scope, the knob panel with Attack and Sustain
//! and their Focus selectors, the focus note, and a More section holding the four small dials
//! (Attack Time, Sustain Time, Mix, Output). The gestures are the full editor's own code.

use super::{Editor, FAMILY, FOCUS_NOTE, PALETTE, SMALL_DIALS, SOURCES, dial_ui, focus_ui, prelude, scope_ui, source_of};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::family;
use crate::editor_kit::look::{CAPTION_SIZE, paint_background, paint_panel};
use crate::editor_kit::slim::{self as kit, GAP, MODEL_H, MORE_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H};
use egui::{Rect, Ui, pos2, vec2};

const SCOPE_H: f32 = 150.0;
const BIG_KNOB: f32 = 70.0;
const SMALL_KNOB: f32 = 44.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let ed = host.rect;
    let pal = &PALETTE;
    paint_background(ui.painter(), ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = kit::family_header(app, ui, host, pal, &mut st.fam, &FAMILY, ("Transient", "Engine"), (x0, w, y), kit::AbPlace::Centred(4.0));

    // ---- 2. Source: one menu, each source with its description
    let items: Vec<MenuEntry> = SOURCES.iter().map(|(name, blurb)| MenuEntry { name, tag: "", blurb, credit: "" }).collect();
    let open = std::mem::take(&mut st.source_open);
    let source = source_of(host);
    if let Some(i) = kit::model_menu_ex(ui, row(y, MODEL_H), "source", "Source", pal, &items, source, open, MenuLook::CENTRED) {
        family::set(app, host, &mut st.fam, &FAMILY, "source", i as f32);
    }
    y += MODEL_H + GAP;

    // ---- 3. the scope
    scope_ui(ui, host, row(y, SCOPE_H));
    y += SCOPE_H + GAP;

    // ---- 4. the knob panel: Attack and Sustain side by side, each with its Focus
    let half = (w / 2.0).floor();
    let panel_h = 8.0 + BIG_KNOB + 30.0 + 6.0 + ROW_H + 10.0;
    paint_panel(ui.painter(), row(y, panel_h), 10.0);
    let top = y + 8.0;
    for (i, (id, label, focus)) in [("attack", "Attack", "attackFocus"), ("sustain", "Sustain", "sustainFocus")].into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(x0 + half * i as f32, top), vec2(half, BIG_KNOB + 30.0));
        let cell = Rect::from_center_size(col.center(), vec2(BIG_KNOB + 40.0, BIG_KNOB + 30.0));
        dial_ui(app, ui, host, &mut st.fam, cell, id, label, false);
        let seg = Rect::from_min_size(pos2(col.left() + 8.0, col.bottom() + 6.0), vec2(half - 16.0, ROW_H));
        focus_ui(app, ui, host, &mut st.fam, seg, focus);
    }
    y += panel_h + 6.0;

    // ---- 5. the focus note
    y += kit::wrapped(ui, Rect::from_min_size(pos2(x0 + 2.0, y), vec2(w - 4.0, 0.0)), FOCUS_NOTE, CAPTION_SIZE, pal.text_faint, 3) + GAP;

    // ---- 6. More: the four small dials
    let holds: Vec<&str> = SMALL_DIALS.iter().map(|(_, l)| *l).collect();
    if kit::more_header(ui, row(y, MORE_H), "transient", pal, st.slim_more, &holds.join(" \u{00B7} ")) {
        st.slim_more = !st.slim_more;
    }
    y += MORE_H;
    if st.slim_more {
        y += 6.0;
        let cells = kit::grid(x0, w, y, SMALL_DIALS.len(), 2, vec2((w / 2.0).floor(), SMALL_KNOB + 32.0), 2.0);
        for ((id, label), r) in SMALL_DIALS.iter().zip(&cells) {
            dial_ui(app, ui, host, &mut st.fam, *r, id, label, true);
        }
        y = kit::bottom_of(&cells, y);
    }
    kit::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

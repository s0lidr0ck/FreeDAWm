//! Sat's editor in the classic drawer's width ([`crate::module_drawer::DRAWER_W`]), about a
//! mixer strip tall: the full editor's sections, names and order, folded to fit. Header (title and
//! preset strip; Undo, A, B, Bypass), Circuit (a menu of the five circuits, each with its
//! description) and Variant, one display that flips between the harmonic signature and the in /
//! out meters, the dials (Drive and Mix, then Bias, Focus, Air, Output), and a More section
//! holding the foot: Auto Gain, Channels, Quality. The gestures are the full editor's own code
//! (`super::*_ui`, `super::seg_param`, the family header).

use super::{CIRCUITS, Editor, PALETTE, SMALL_DIALS, circuit, index, paint_meters, prelude};
use crate::SoundApp;
use crate::editor_kit::Host;
use crate::editor_kit::controls::{LabelLayout, PillStyle, pill_height};
use crate::editor_kit::look::{paint_background, paint_panel};
use crate::editor_kit::slim::{
    self, AbPlace, BIG_KNOB, GAP, MODEL_H, MenuEntry, MenuLook, PAD_X, PAD_Y, ROW_H, SMALL_KNOB, caption, display_label, display_toggle,
    family_header, grid, model_menu_ex, more_header, row_label,
};
use egui::{Rect, Ui, pos2, vec2};

const LABEL_W: f32 = 56.0;
const DISPLAY_H: f32 = 156.0;
const TOGGLE_W: f32 = 118.0;

pub(super) fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let ed = host.rect;
    let p = ui.painter().clone();
    paint_background(&p, ed);
    let x0 = ed.left() + PAD_X;
    let w = ed.width() - 2.0 * PAD_X;
    let row = |y: f32, h: f32| Rect::from_min_size(pos2(x0, y), vec2(w, h));
    let labelled = |y: f32, h: f32| Rect::from_min_max(pos2(x0 + LABEL_W, y), pos2(x0 + w, y + h));
    let mut y = ed.top() + PAD_Y;

    // ---- 1. header
    y = family_header(app, ui, host, pal, &mut st.fam, &super::FAMILY, ("Sat", "Engine"), (x0, w, y), AbPlace::Centred(3.0));

    // ---- 2. Circuit: one menu of the five (the full editor's segmented row and its blurb)
    row_label(ui, row(y, MODEL_H), "Circuit", pal, false);
    let entries: Vec<MenuEntry> = CIRCUITS.iter().map(|c| MenuEntry { name: c.name, tag: "", blurb: c.blurb, credit: "" }).collect();
    let cur = index(host, "circuit").clamp(0, 4) as usize;
    if let Some(i) = model_menu_ex(
        ui,
        labelled(y, MODEL_H),
        "sat-circuit",
        "Circuit",
        pal,
        &entries,
        cur,
        std::mem::take(&mut st.circuits_open),
        MenuLook::BASELINE_10,
    ) {
        crate::editor_kit::family::set(app, host, &mut st.fam, &super::FAMILY, "circuit", i as f32);
    }
    y += MODEL_H + 6.0;

    // ---- 3. Variant and its blurb
    row_label(ui, row(y, ROW_H + 2.0), "Variant", pal, false);
    super::variant_ui(app, ui, host, &mut st.fam, labelled(y, ROW_H + 2.0));
    y += ROW_H + 2.0 + 3.0;
    let c = circuit(index(host, "circuit"));
    let blurb = c.variants.get(index(host, "variant").clamp(0, 2) as usize).map_or("", |v| v.1);
    y += caption(ui, labelled(y, 0.0), blurb, pal.text_faint, 2) + GAP;

    // ---- 4. one display: the harmonic signature or the meters
    let display = row(y, DISPLAY_H);
    if st.slim_meters {
        paint_panel(&p, display, 10.0);
        super::tick_meters(st, ui, host);
        let inner = super::harmonics::inner(display);
        let m = Rect::from_center_size(pos2(inner.center().x, (inner.top() + 30.0 + inner.bottom()) / 2.0), vec2(96.0, inner.height() - 30.0));
        paint_meters(&p, m, pal, st.meter.0, st.meter.1);
    } else {
        super::tick_meters(st, ui, host);
        super::harmonics_ui(st, ui, host, display, Some(TOGGLE_W));
    }
    display_label(ui, display, "sat", if st.slim_meters { "In / out meters" } else { "Harmonic signature" });
    let toggle = Rect::from_min_size(super::harmonics::inner(display).min, vec2(TOGGLE_W, 22.0));
    if let Some(i) = display_toggle(ui, toggle, "sat", pal, &["Harmonics", "Meters"], usize::from(st.slim_meters)) {
        st.slim_meters = i == 1;
    }
    // the full editor's caption, which the slim display has no room for
    let _ = ui.interact(toggle, ui.id().with("sat-slim-toggle-tip"), egui::Sense::hover()).on_hover_text(super::harmonics::CAPTION);
    y += DISPLAY_H + GAP;

    // ---- 5. the dials, in their panel: Drive and Mix, then the four small ones
    let big = vec2((w / 3.0).floor(), BIG_KNOB + 32.0);
    let small = vec2((w / 4.0).floor(), SMALL_KNOB + 32.0);
    let panel_h = 6.0 + big.y + 4.0 + small.y + 4.0;
    paint_panel(&p, row(y, panel_h), 10.0);
    let bigs = grid(x0, w, y + 6.0, 2, 3, big, 0.0);
    for ((id, label), r) in [("drive", "Drive"), ("mix", "Mix")].into_iter().zip(bigs) {
        super::dial_ui(app, ui, host, &mut st.fam, r, id, label, false, LabelLayout::Below);
    }
    let smalls = grid(x0, w, y + 6.0 + big.y + 4.0, SMALL_DIALS.len(), 4, small, 0.0);
    for ((id, label), r) in SMALL_DIALS.into_iter().zip(smalls) {
        super::dial_ui(app, ui, host, &mut st.fam, r, id, label, true, LabelLayout::Below);
    }
    y += panel_h + GAP;

    // ---- 6. More: the foot (Auto Gain, Channels, Quality)
    if more_header(ui, row(y, 20.0), "sat", pal, st.slim_more, "Auto Gain \u{00B7} Channels \u{00B7} Quality") {
        st.slim_more = !st.slim_more;
    }
    y += 20.0;
    if st.slim_more {
        y += 6.0;
        let ph = pill_height(PillStyle::Small);
        let ag_w = 84f32.max(crate::editor_kit::family::small_pill_w(ui, "Auto Gain").trunc());
        super::auto_gain_ui(app, ui, host, &mut st.fam, Rect::from_min_size(pos2(x0, y), vec2(ag_w, ph)));
        y += ph + 8.0;
        row_label(ui, row(y, 24.0), "Channels", pal, false);
        super::channels_ui(app, ui, host, &mut st.fam, labelled(y, 24.0));
        y += 24.0 + 6.0;
        row_label(ui, row(y, 24.0), "Quality", pal, false);
        super::quality_ui(app, ui, host, &mut st.fam, labelled(y, 24.0));
        y += 24.0 + 3.0;
        y += caption(ui, labelled(y, 0.0), &super::quality_note(host), pal.text_faint, 1);
    }

    slim::settle(ui, ed.top(), y + PAD_Y, &mut st.slim_h);
}

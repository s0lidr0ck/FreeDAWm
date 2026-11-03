//! Tape's VST editor (`Engine 9 - Tape/Source/PluginEditor.cpp`), translated: `resized()` and
//! `paint()` constant for constant, the family header, the machine row and the tape / speed row
//! with their blurbs (`TapeConfig.h`), the six dials, the measured display ([`scope`],
//! `TapeScope.h`), the in / out meters (`tape::Meters`) and the foot. Palette: `look::apply
//! (kAccent)` (Lavender), the same in both DAW themes. Its interaction inventory is
//! `docs/superpowers/editors/tape.md`.

mod scope;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, pill_width_ex, segmented};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, run_with};
use crate::theme::regular;
use egui::{Align2, CornerRadius, Id, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::tape::tape_engine_dsp::{
    K_LATENCY_MS, M_CASSETTE, NUM_PARAMS, flutter_percent, knob_to_bias, knob_to_drive_db, knob_to_hiss_db, knob_to_out_db,
};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 14.0;
const K_HEADER_H: f32 = 26.0;
const K_ROW_H: f32 = 28.0;
const K_BLURB_H: f32 = 16.0;
const K_LABEL_W: f32 = 62.0;
const K_LABEL_GAP: f32 = 12.0;
const K_BIG_KNOB: f32 = 104.0;
const K_SMALL_KNOB: f32 = 62.0;
const K_MIDDLE_H: f32 = K_BIG_KNOB + 30.0 + 16.0 + K_SMALL_KNOB + 28.0;
const K_FOOT_H: f32 = 26.0;
const K_METER_W: f32 = 44.0;
/// `kDefaultH`.
const K_DEFAULT_H: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + 2.0 * (K_ROW_H + 3.0 + K_BLURB_H) + 8.0 + K_GAP + K_MIDDLE_H + K_GAP + K_FOOT_H;

/// `setSize (860, kDefaultH)`.
pub const SIZE: Vec2 = vec2(860.0, K_DEFAULT_H);

/// `kAccent`: Lavender.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFB9A2FF));

/// `PresetCatalogue`: the storage key, and what `getPresetParams` leaves out besides `bypass`.
const FAMILY: Family = Family::new("TapeEngine", &["output", "autoGain", "quality"]);

/// The engine's parameter ids by index (`Parameters.h`).
const IDS: [&str; NUM_PARAMS] = ["machine", "formula", "speed", "drive", "bias", "flutter", "hiss", "mix", "output", "autoGain", "quality", "bypass"];

// ============================================================================ TapeConfig.h

const REEL_FORMULAS: [(&str, &str); 3] = [
    ("Standard", "The classic mastering tape. Saturates smoothly from about +9 dB."),
    ("High Output", "Three more dB of headroom: stays cleaner when you hit it hard."),
    ("Vintage", "Older, lower-output oxide. A wider loop, an earlier ceiling, more hiss."),
];

struct MachineDef {
    name: &'static str,
    blurb: &'static str,
    formulas: [(&'static str, &'static str); 3],
}

const MACHINES: [MachineDef; 4] = [
    MachineDef {
        name: "Two-Track",
        blurb: "Half-inch mastering deck, NAB curves, wide tracks: the quietest and cleanest here.",
        formulas: REEL_FORMULAS,
    },
    MachineDef {
        name: "Multitrack",
        blurb: "Two-inch 24-track, CCIR curves, narrow tracks: the biggest head bump, the glue.",
        formulas: REEL_FORMULAS,
    },
    MachineDef {
        name: "Valve",
        blurb: "A 1950s deck with valve record electronics: second harmonic, a softer top, a looser transport.",
        formulas: REEL_FORMULAS,
    },
    MachineDef {
        name: "Cassette",
        blurb: "1 7/8 ips. Highs saturate long before lows, the top rolls off early, and it hisses.",
        formulas: [
            ("Type I", "Ferric, 120 us: dull, noisy, and the top end compresses first."),
            ("Type II", "Chrome, 70 us: brighter and quieter."),
            ("Type IV", "Metal, 70 us: the most headroom a cassette ever had."),
        ],
    },
];

/// `tape::machine (i)`: clamped to 0..3.
fn machine(i: i32) -> &'static MachineDef {
    &MACHINES[i.clamp(0, 3) as usize]
}

// ============================================================================ formatting

/// `signedDb`: never "-0.0 dB".
fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}
fn format_percent(v: f32) -> String {
    format!("{} %", (v * 100.0).round() as i32)
}
fn format_bias(v: f32) -> String {
    let b = (knob_to_bias(v) * 100.0).round() as i32;
    if b == 0 {
        return "Aligned".into();
    }
    format!("{}{} %", if b < 0 { "Under " } else { "Over " }, b.abs())
}
fn format_hiss(v: f32) -> String {
    if v < 0.001 {
        return "Off".into();
    }
    let db = knob_to_hiss_db(v);
    if db.abs() < 0.25 { "Real".into() } else { signed_db(db) }
}
/// Wow & flutter in the machine's own peak speed error.
fn format_flutter(machine: i32, v: f32) -> String {
    if v < 0.001 { "Off".into() } else { format!("{:.2} %", flutter_percent(machine, v)) }
}

// ============================================================================ the editor

/// The editor's own state (what the VST keeps in its editor object).
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    scope: scope::View,
    /// `Meters::in` / `out`, and when they last fell.
    meter: (f32, f32, f64),
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's one display shows the in / out meters (else the scope, in its view).
    slim_meters: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// Scripts: open the slim layout's machine menu on the next frame.
    machines_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_tape";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { SIZE }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

/// JUCE `withSizeKeepingCentre` on integer rectangles.
fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(r.left() + ((r.width() - w) / 2.0).trunc(), r.top() + ((r.height() - h) / 2.0).trunc()), vec2(w, h))
}

/// A left-aligned, vertically centred line of text in `r`.
fn text_left(p: &egui::Painter, r: Rect, text: &str, size: f32, c: egui::Color32) {
    p.with_clip_rect(r.intersect(p.clip_rect())).text(pos2(r.left(), r.center().y), Align2::LEFT_CENTER, text, regular(size), c);
}

/// A choice parameter's index.
fn index(host: &Host, id: &str) -> i32 {
    param(host, id).round() as i32
}

/// A Segmented bound to choice parameter `id`: a click is one undo step.
#[allow(clippy::too_many_arguments)]
fn seg_param(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    r: Rect,
    pal: &VstPalette,
    id: &str,
    options: &[&str],
    selected: usize,
) {
    if let Some(i) = segmented(ui, r, ("tape-seg", id), pal, options, 1, Some(selected), SelectionStyle::Filled)
        && i != selected
    {
        family::set(app, host, st, &FAMILY, id, i as f32);
    }
}

/// What both layouts do first: the scripted drawer tabs and the preset browser, which covers
/// the whole editor while it is open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
        st.scope.freq_index = 1;
    }
    // `ui.mix.drawer {tab}`: the slim layout's "Machine" (its menu), "More" and its display.
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Machine") if st.slim => st.machines_open = true,
        Some("More") if st.slim => st.slim_more = true,
        Some("Meters") if st.slim => st.slim_meters = true,
        Some("Response") if st.slim => (st.slim_meters, st.scope.view) = (false, 0),
        Some("Harmonics") if st.slim => (st.slim_meters, st.scope.view) = (false, 1),
        _ => {}
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, machine(index(host, "machine")).name)
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let full = host.rect;
    let p = ui.painter().clone();
    paint_background(&p, full);

    // ---- resized()
    let mut b = full.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let header = Rect::from_min_size(b.min, vec2(b.width(), K_HEADER_H));
    b.min.y += K_HEADER_H + K_GAP;
    // machine
    let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
    let machine_label = Rect::from_min_size(row.min, vec2(K_LABEL_W, K_ROW_H));
    let machine_seg = Rect::from_min_max(pos2(row.left() + K_LABEL_W + K_LABEL_GAP, row.top()), row.max);
    b.min.y += K_ROW_H + 3.0;
    let machine_blurb = Rect::from_min_max(pos2(b.left() + K_LABEL_W + K_LABEL_GAP, b.top()), pos2(b.right(), b.top() + K_BLURB_H));
    b.min.y += K_BLURB_H + 8.0;
    // tape and speed side by side
    let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
    let formula_label = Rect::from_min_size(row.min, vec2(K_LABEL_W, K_ROW_H));
    let rest = Rect::from_min_max(pos2(row.left() + K_LABEL_W + K_LABEL_GAP, row.top()), row.max);
    let speed_w = 220f32.max((rest.width() * 36.0 / 100.0).trunc());
    let speed_part = Rect::from_min_max(pos2(rest.right() - speed_w, rest.top()), rest.max);
    let formula_seg = Rect::from_min_max(rest.min, pos2(speed_part.left() - 18.0, rest.bottom()));
    let speed_label = Rect::from_min_size(speed_part.min, vec2(50.0, K_ROW_H));
    let speed_seg = Rect::from_min_max(pos2(speed_part.left() + 50.0, speed_part.top()), speed_part.max);
    b.min.y += K_ROW_H + 3.0;
    let formula_blurb = Rect::from_min_max(pos2(b.left() + K_LABEL_W + K_LABEL_GAP, b.top()), pos2(b.right(), b.top() + K_BLURB_H));
    b.min.y += K_BLURB_H + K_GAP;

    let mid_h = K_MIDDLE_H.max(b.height() - K_GAP - K_FOOT_H);
    let mut mid = Rect::from_min_size(b.min, vec2(b.width(), mid_h));
    b.min.y += mid_h;
    let meters_r = Rect::from_min_max(pos2(mid.right() - K_METER_W, mid.top() + 6.0), pos2(mid.right(), mid.bottom() - 6.0));
    mid.max.x -= K_METER_W + K_GAP;
    let knob_panel = Rect::from_min_size(mid.min, vec2(330f32.max((mid.width() * 45.0 / 100.0).trunc()), mid.height()));
    mid.min.x += knob_panel.width() + K_GAP;
    let scope_r = mid;

    let k = knob_panel.shrink(10.0);
    let top = Rect::from_min_size(k.min, vec2(k.width(), K_BIG_KNOB + 30.0));
    let half_w = (top.width() / 2.0).trunc();
    let drive_r = keep_centre(Rect::from_min_size(top.min, vec2(half_w, top.height())), K_BIG_KNOB, K_BIG_KNOB + 30.0);
    let mix_r = keep_centre(Rect::from_min_max(pos2(top.left() + half_w, top.top()), top.max), K_BIG_KNOB, K_BIG_KNOB + 30.0);
    // `removeFromTop` never takes more than is left: the row is shorter than the dials, which
    // centre on it and overhang (as in the VST).
    let bottom_top = top.bottom() + 16.0;
    let bottom = Rect::from_min_size(pos2(k.left(), bottom_top), vec2(k.width(), (K_SMALL_KNOB + 28.0).min(k.bottom() - bottom_top).max(0.0)));
    let cell_w = (bottom.width() / 4.0).trunc();

    b.min.y += K_GAP;
    let foot = Rect::from_min_size(b.min, vec2(b.width(), K_FOOT_H));
    let btn_h = pill_height(PillStyle::Small);
    let ag_w = 84f32.max(pill_width_ex(ui, "Auto Gain", PillStyle::Small, false).trunc());
    let auto_gain_r = keep_centre(Rect::from_min_size(foot.min, vec2(ag_w, K_FOOT_H)), ag_w, btn_h);
    let fx = foot.left() + ag_w + 22.0;
    let quality_label = Rect::from_min_size(pos2(fx, foot.top()), vec2(50.0, K_FOOT_H));
    let quality_seg = keep_centre(Rect::from_min_size(pos2(fx + 50.0, foot.top()), vec2(200.0, K_FOOT_H)), 200.0, 24.0);

    // ---- paint()
    let cached_machine = index(host, "machine");
    let cassette = cached_machine == M_CASSETTE;
    for (r, t) in [(machine_label, "Machine"), (formula_label, "Tape"), (quality_label, "Quality")] {
        text_left(&p, r, t, LABEL_SIZE, pal.text_body);
    }
    text_left(&p, speed_label, "Speed", LABEL_SIZE, if cassette { pal.text_faint } else { pal.text_body });
    let m = machine(cached_machine);
    text_left(&p, machine_blurb, m.blurb, CAPTION_SIZE, pal.text_faint);
    text_left(&p, formula_blurb, &formula_text(host), CAPTION_SIZE, pal.text_faint);
    paint_panel(&p, knob_panel, 10.0);
    {
        let nr = Rect::from_min_max(pos2(quality_seg.right() + 16.0, foot.top()), foot.max);
        text_left(&p, nr, &quality_note(host), CAPTION_SIZE, pal.text_faint);
    }

    // ---- the header
    paint_title(&p, header, "Tape", "Engine", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- machine, tape, speed
    let names: Vec<&str> = MACHINES.iter().map(|m| m.name).collect();
    seg_param(app, ui, host, &mut st.fam, machine_seg, pal, "machine", &names, cached_machine.clamp(0, 3) as usize);
    formula_ui(app, ui, host, &mut st.fam, formula_seg);
    speed_ui(app, ui, host, &mut st.fam, speed_seg);

    // ---- dials
    let mut cells: Vec<(Rect, &str, &str, bool)> = vec![(drive_r, "drive", "Drive", false), (mix_r, "mix", "Mix", false)];
    for (i, (id, label)) in SMALL_DIALS.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(bottom.left() + cell_w * i as f32, bottom.top()), vec2(cell_w, bottom.height()));
        // Wider than the knob, so "Wow & Flutter" fits under it.
        cells.push((keep_centre(cell, (cell_w - 4.0).min(96.0), K_SMALL_KNOB + 28.0), id, label, true));
    }
    for (r, id, label, small) in cells {
        dial_ui(app, ui, host, &mut st.fam, r, id, label, small, LabelLayout::Below);
    }

    // ---- the measured display
    scope_ui(st, ui, host, scope_r, false);

    // ---- the meters
    tick_meters(st, ui, host);
    paint_meters(&p, meters_r, pal, st.meter.0, st.meter.1);

    // ---- the foot
    auto_gain_ui(app, ui, host, &mut st.fam, auto_gain_r);
    quality_ui(app, ui, host, &mut st.fam, quality_seg);
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// The four small dials, in the full editor's order.
const SMALL_DIALS: [(&str, &str); 4] = [("bias", "Bias"), ("flutter", "Wow & Flutter"), ("hiss", "Hiss"), ("output", "Output")];

/// The tape's blurb (and, on the Cassette, that its speed is fixed).
fn formula_text(host: &Host) -> String {
    let m = machine(index(host, "machine"));
    let mut text = m.formulas[index(host, "formula").clamp(0, 2) as usize].1.to_string();
    if index(host, "machine") == M_CASSETTE {
        text.push_str("  \u{00B7}  Speed is fixed at 1 7/8 ips.");
    }
    text
}

/// What Quality buys, and the latency that never changes.
fn quality_note(host: &Host) -> String {
    let os = match index(host, "quality") {
        0 => 2,
        1 => 4,
        _ => 8,
    };
    format!("{os}x oversampling  \u{00B7}  {K_LATENCY_MS:.1} ms latency, always")
}

/// The Tape switch: the machine's three tapes.
fn formula_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    let m = machine(index(host, "machine"));
    let formulas: Vec<&str> = m.formulas.iter().map(|f| f.0).collect();
    seg_param(app, ui, host, fam, r, &PALETTE, "formula", &formulas, index(host, "formula").clamp(0, 2) as usize);
}

/// The Speed switch. Cassette runs at 1 7/8 ips only: the switch keeps its position but is
/// disabled at 35 %.
fn speed_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    let speed = index(host, "speed").clamp(0, 2) as usize;
    const SPEEDS: [&str; 3] = ["7.5 ips", "15 ips", "30 ips"];
    if index(host, "machine") == M_CASSETTE {
        ui.scope(|ui| {
            ui.multiply_opacity(0.35);
            let _ = segmented(ui, r, ("tape-seg", "speed-off"), &PALETTE, &SPEEDS, 1, Some(speed), SelectionStyle::Filled);
        });
    } else {
        seg_param(app, ui, host, fam, r, &PALETTE, "speed", &SPEEDS, speed);
    }
}

/// The Quality switch (Eco, Normal, Insane).
fn quality_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    seg_param(app, ui, host, fam, r, &PALETTE, "quality", &["Eco", "Normal", "Insane"], index(host, "quality").clamp(0, 2) as usize);
}

/// A dial's readout (`formatValue`).
fn dial_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "drive" => signed_db(knob_to_drive_db(v)),
        "output" => signed_db(knob_to_out_db(v)),
        "mix" => format_percent(v),
        "bias" => format_bias(v),
        "flutter" => format_flutter(index(host, "machine"), v),
        _ => format_hiss(v),
    }
}

/// One of the six dials (`attachDial`).
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, small: bool, layout: LabelLayout) {
    let value_text = dial_text(host, id);
    let default = host.info.param(id).map_or(0.5, |p| p.to_normalized(p.default));
    let o = DialOpts { label, value_text: &value_text, default, small, layout, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// The measured display in `r`; `slim`: the slim layout's display toggle stands in for its view
/// tabs.
fn scope_ui(st: &mut Editor, ui: &mut Ui, host: &Host, r: Rect, slim: bool) {
    let mut params = [0.0f32; NUM_PARAMS];
    for (v, id) in params.iter_mut().zip(IDS) {
        *v = param(host, id);
    }
    scope::draw_ex(ui, r, &PALETTE, &mut st.scope, &params, Id::new(("tape-scope", host.track.0, host.ins.id.0)), slim);
}

/// The meters' 30 Hz fall (`Meters::update`), from the module's in / out peaks.
fn tick_meters(st: &mut Editor, ui: &Ui, host: &Host) {
    let now = ui.input(|i| i.time);
    let (in_pk, out_pk) =
        host.meters.and_then(|m| m.modules.iter().find(|mm| mm.module == host.ins.id)).map_or((0.0, 0.0), |mm| (mm.in_peak, mm.out_peak));
    let fall = 0.82f32.powf(((now - st.meter.2) * 30.0).clamp(0.0, 100.0) as f32);
    let clean = |x: f32| if x.is_finite() { x.max(0.0) } else { 0.0 };
    st.meter = (clean(in_pk).max(st.meter.0 * fall), clean(out_pk).max(st.meter.1 * fall), now);
    if st.meter.0 > 1e-5 || st.meter.1 > 1e-5 {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
}

/// The Auto Gain pill.
fn auto_gain_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    let auto_gain = param(host, "autoGain") > 0.5;
    let plain = PillLook { tracked: false, ..PillLook::default() };
    if pill_ex(ui, r, "tape-autogain", &PALETTE, "Auto Gain", auto_gain, PillStyle::Small, plain).clicked() {
        family::pill_set(app, host, fam, &FAMILY, "autoGain", if auto_gain { 0.0 } else { 1.0 });
    }
}

/// `tape::Meters::paint`: IN and OUT, −48..0 dBFS.
fn paint_meters(p: &egui::Painter, r: Rect, pal: &VstPalette, input: f32, output: f32) {
    let labels_top = r.bottom() - 14.0;
    let body = Rect::from_min_max(r.min, pos2(r.right(), labels_top));
    let w = (body.width() - 6.0) * 0.5;
    for (i, (peak, name)) in [(input, "IN"), (output, "OUT")].into_iter().enumerate() {
        let m = Rect::from_min_size(pos2(body.left() + i as f32 * (w + 6.0), body.top()), vec2(w, body.height()));
        p.rect_filled(m, CornerRadius::same(3), pal.panel_alt);
        let db = if peak > 0.0 { 20.0 * peak.log10() } else { -100.0 };
        let t = ((db + 48.0) / 48.0).clamp(0.0, 1.0);
        let fill = Rect::from_min_max(pos2(m.left(), m.bottom() - m.height() * t), m.max);
        if t > 0.0 {
            p.rect_filled(fill, CornerRadius::same(3), if db > -0.5 { pal.bypass_red } else { al(pal.accent, 0.8) });
        }
        let lr = Rect::from_min_size(pos2(m.left() - 4.0, labels_top), vec2(m.width() + 8.0, 14.0));
        p.text(lr.center(), Align2::CENTER_CENTER, name, regular(TINY_SIZE), pal.text_faint);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_height_is_the_vsts() {
        assert_eq!(SIZE, vec2(860.0, 466.0));
    }

    #[test]
    fn readouts_match_the_cpp() {
        assert_eq!(format_bias(0.5), "Aligned");
        assert_eq!(format_bias(0.25), "Under 50 %");
        assert_eq!(format_hiss(0.0), "Off");
        assert_eq!(format_flutter(1, 0.0), "Off");
        assert_eq!(machine(9).name, "Cassette");
    }
}

//! Sat's VST editor (`Engine 8 - Sat/Source/PluginEditor.cpp`), translated: `resized()` and
//! `paint()` constant for constant, the family header, the circuit and variant rows with their
//! blurbs (`SatConfig.h`), the six dials, the harmonic signature ([`harmonics`], `SatHarmonics.h`),
//! the in / out meters (`sat::Meters`) and the foot. Palette: `look::apply (kAccent)` (Rose), the
//! same in both DAW themes. Its interaction inventory is `docs/superpowers/editors/sat.md`.

mod harmonics;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, pill_width_ex, segmented};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, run_with};
use crate::theme::regular;
use egui::{Align2, CornerRadius, Id, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::sat::sat_engine_dsp::{NUM_PARAMS, knob_to_bias, knob_to_drive_db, knob_to_out_db, knob_to_tilt_db};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 14.0;
const K_HEADER_H: f32 = 26.0;
const K_ROW_H: f32 = 28.0;
const K_BLURB_H: f32 = 16.0;
const K_LABEL_W: f32 = 58.0;
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

/// `kAccent`: Rose.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFF08CA8));

/// `PresetCatalogue`: the storage key, and what `getPresetParams` leaves out besides `bypass`.
const FAMILY: Family = Family::new("SatEngine", &["output", "autoGain", "quality"]);

/// The engine's parameter ids by index (`Parameters.h`).
const IDS: [&str; NUM_PARAMS] = ["circuit", "variant", "drive", "bias", "focus", "air", "mix", "output", "autoGain", "chanMode", "quality", "bypass"];

// ============================================================================ SatConfig.h

struct CircuitDef {
    name: &'static str,
    blurb: &'static str,
    variants: [(&'static str, &'static str); 3],
}

const CIRCUITS: [CircuitDef; 5] = [
    CircuitDef {
        name: "Iron",
        blurb: "Transformer. Hysteresis driven by flux, so it distorts more the lower the note -- weight and glue.",
        variants: [
            ("Nickel", "Saturates early and softly. The thickest low end."),
            ("Steel", "A wider loop: more grit, odd harmonics first."),
            ("Ferrite", "Narrow loop, the cleanest of the three. Iron you barely hear."),
        ],
    },
    CircuitDef {
        name: "Valve",
        blurb: "Tube stage whose bias moves: it sags after a hit and recovers, so the colour follows your playing.",
        variants: [
            ("Triode", "Even-order and soft. Warmth rather than edge."),
            ("Pentode", "Clean until it isn't, then odd-order and hard. More bite."),
            ("Starved", "Low plate voltage: breaks up early, sags the most."),
        ],
    },
    CircuitDef {
        name: "Desk",
        blurb: "Console line amp. Slew-limited, so loud highs soften first -- the difference between fast and slow desks.",
        variants: [
            ("Clean", "A modern op-amp. Only bites when you really lean on the top."),
            ("Vintage", "A slow op-amp: audibly rounds off loud highs."),
            ("Driven", "Slow and pushed. The console run hot."),
        ],
    },
    CircuitDef {
        name: "Bridge",
        blurb: "Diode bridge inside iron, solved exactly. Firm and forward, with a soft knee.",
        variants: [
            ("Germanium", "Low threshold, rounded. Conducts early and gently."),
            ("Silicon", "Stays clean longer, then clamps harder."),
            ("Mixed", "One of each: strongly lopsided, mostly second harmonic."),
        ],
    },
    CircuitDef {
        name: "FET",
        blurb: "Square-law JFET. Pure even-order colour with no ceiling in reach -- the lightest touch here.",
        variants: [
            ("Cold", "Biased near pinch-off: the most second harmonic."),
            ("Stock", "The middle of the curve."),
            ("Hot", "Biased high: the least colour of the three."),
        ],
    },
];

/// `sat::circuit (i)`: clamped to 0..4.
fn circuit(i: i32) -> &'static CircuitDef {
    &CIRCUITS[i.clamp(0, 4) as usize]
}

// ============================================================================ formatting

/// `signedDb`: never "-0.0 dB".
pub(crate) fn signed_db(db: f32) -> String {
    let db = if db.abs() < 0.05 { 0.0 } else { db };
    format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
}
fn format_percent(v: f32) -> String {
    format!("{} %", (v * 100.0).round() as i32)
}
fn format_air(v: f32) -> String {
    if v < 0.005 { "Off".into() } else { format_percent(v) }
}
fn format_bias(v: f32) -> String {
    let b = (knob_to_bias(v) * 100.0).round() as i32;
    if b == 0 { "0".into() } else { format!("{}{b}", if b > 0 { "+" } else { "" }) }
}
fn format_focus(v: f32) -> String {
    let t = knob_to_tilt_db(v);
    if t.abs() < 0.25 {
        return "Flat".into();
    }
    format!("{}{:.1} dB", if t > 0.0 { "Lows +" } else { "Highs +" }, t.abs())
}

// ============================================================================ the editor

/// The editor's own state (what the VST keeps in its editor object).
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    harmonics: harmonics::View,
    /// `Meters::in` / `out`, and when they last fell.
    meter: (f32, f32, f64),
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's one display: the in / out meters (true) or the harmonic signature.
    slim_meters: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// Scripts: open the slim layout's circuit menu on the next frame.
    circuits_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_sat";

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
    if let Some(i) = segmented(ui, r, ("sat-seg", id), pal, options, 1, Some(selected), SelectionStyle::Filled)
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
    }
    // `ui.mix.drawer {tab}`: the slim layout's "Circuit" (its menu), "More" and its display.
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Circuit") if st.slim => st.circuits_open = true,
        Some("More") if st.slim => st.slim_more = true,
        Some("Meters") if st.slim => st.slim_meters = true,
        Some("Harmonics") if st.slim => st.slim_meters = false,
        _ => {}
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, circuit(index(host, "circuit")).name)
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
    let selector_row = |b: &mut Rect| {
        let row = Rect::from_min_size(b.min, vec2(b.width(), K_ROW_H));
        let label = Rect::from_min_size(row.min, vec2(K_LABEL_W, K_ROW_H));
        let seg = Rect::from_min_max(pos2(row.left() + K_LABEL_W + K_LABEL_GAP, row.top()), row.max);
        b.min.y += K_ROW_H + 3.0;
        let blurb = Rect::from_min_max(pos2(b.left() + K_LABEL_W + K_LABEL_GAP, b.top()), pos2(b.right(), b.top() + K_BLURB_H));
        b.min.y += K_BLURB_H;
        (label, seg, blurb)
    };
    let (circuit_label, circuit_seg, circuit_blurb) = selector_row(&mut b);
    b.min.y += 8.0;
    let (variant_label, variant_seg, variant_blurb) = selector_row(&mut b);
    b.min.y += K_GAP;

    let mid_h = K_MIDDLE_H.max(b.height() - K_GAP - K_FOOT_H);
    let mut mid = Rect::from_min_size(b.min, vec2(b.width(), mid_h));
    b.min.y += mid_h;
    let meters_r = Rect::from_min_max(pos2(mid.right() - K_METER_W, mid.top() + 6.0), pos2(mid.right(), mid.bottom() - 6.0));
    mid.max.x -= K_METER_W + K_GAP;
    let knob_panel = Rect::from_min_size(mid.min, vec2(330f32.max((mid.width() * 45.0 / 100.0).trunc()), mid.height()));
    mid.min.x += knob_panel.width() + K_GAP;
    let harmonics_r = mid;

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
    let mut fx = foot.left() + ag_w + 22.0;
    let chan_label = Rect::from_min_size(pos2(fx, foot.top()), vec2(56.0, K_FOOT_H));
    fx += 56.0;
    let chan_seg = keep_centre(Rect::from_min_size(pos2(fx, foot.top()), vec2(130.0, K_FOOT_H)), 130.0, 24.0);
    fx += 130.0 + 22.0;
    let quality_label = Rect::from_min_size(pos2(fx, foot.top()), vec2(50.0, K_FOOT_H));
    fx += 50.0;
    let quality_seg = keep_centre(Rect::from_min_size(pos2(fx, foot.top()), vec2(200.0, K_FOOT_H)), 200.0, 24.0);

    // ---- paint()
    let cached_circuit = index(host, "circuit");
    let cached_variant = index(host, "variant").clamp(0, 2);
    for (r, t) in [(circuit_label, "Circuit"), (variant_label, "Variant"), (chan_label, "Channels"), (quality_label, "Quality")] {
        text_left(&p, r, t, LABEL_SIZE, pal.text_body);
    }
    let c = circuit(cached_circuit);
    text_left(&p, circuit_blurb, c.blurb, CAPTION_SIZE, pal.text_faint);
    text_left(&p, variant_blurb, c.variants[cached_variant as usize].1, CAPTION_SIZE, pal.text_faint);
    paint_panel(&p, knob_panel, 10.0);
    {
        let nr = Rect::from_min_max(pos2(quality_seg.right() + 16.0, foot.top()), foot.max);
        text_left(&p, nr, &quality_note(host), CAPTION_SIZE, pal.text_faint);
    }

    // ---- the header
    paint_title(&p, header, "Sat", "Engine", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- circuit and variant
    let circuit_names: Vec<&str> = CIRCUITS.iter().map(|c| c.name).collect();
    seg_param(app, ui, host, &mut st.fam, circuit_seg, pal, "circuit", &circuit_names, cached_circuit.clamp(0, 4) as usize);
    variant_ui(app, ui, host, &mut st.fam, variant_seg);

    // ---- dials
    let mut cells: Vec<(Rect, &str, &str, bool)> = vec![(drive_r, "drive", "Drive", false), (mix_r, "mix", "Mix", false)];
    for (i, (id, label)) in SMALL_DIALS.into_iter().enumerate() {
        let cell = Rect::from_min_size(pos2(bottom.left() + cell_w * i as f32, bottom.top()), vec2(cell_w, bottom.height()));
        cells.push((keep_centre(cell, K_SMALL_KNOB, K_SMALL_KNOB + 28.0), id, label, true));
    }
    for (r, id, label, small) in cells {
        dial_ui(app, ui, host, &mut st.fam, r, id, label, small, LabelLayout::Below);
    }

    // ---- the harmonic signature
    harmonics_ui(st, ui, host, harmonics_r, None);

    // ---- the meters
    tick_meters(st, ui, host);
    paint_meters(&p, meters_r, pal, st.meter.0, st.meter.1);

    // ---- the foot
    auto_gain_ui(app, ui, host, &mut st.fam, auto_gain_r);
    channels_ui(app, ui, host, &mut st.fam, chan_seg);
    quality_ui(app, ui, host, &mut st.fam, quality_seg);
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// The four small dials, in the full editor's order.
const SMALL_DIALS: [(&str, &str); 4] = [("bias", "Bias"), ("focus", "Focus"), ("air", "Air"), ("output", "Output")];

/// What Quality actually buys; Iron never runs below 4x.
fn quality_note(host: &Host) -> String {
    let os = match index(host, "quality") {
        0 => 2,
        1 => 4,
        _ => 8,
    };
    if index(host, "circuit") == 0 && os < 4 { "4x oversampling \u{2014} Iron's floor".to_string() } else { format!("{os}x oversampling") }
}

/// The variant switch: the circuit's three variants.
fn variant_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    let c = circuit(index(host, "circuit"));
    let names: Vec<&str> = c.variants.iter().map(|v| v.0).collect();
    seg_param(app, ui, host, fam, r, &PALETTE, "variant", &names, index(host, "variant").clamp(0, 2) as usize);
}

/// The Channels switch (Stereo, M/S).
fn channels_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    seg_param(app, ui, host, fam, r, &PALETTE, "chanMode", &["Stereo", "M/S"], index(host, "chanMode").clamp(0, 1) as usize);
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
        "air" => format_air(v),
        "bias" => format_bias(v),
        _ => format_focus(v),
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

/// The harmonic signature in `r`; `lead`: the width the slim layout's display toggle takes in
/// its corner.
fn harmonics_ui(st: &mut Editor, ui: &mut Ui, host: &Host, r: Rect, lead: Option<f32>) {
    let mut params = [0.0f32; NUM_PARAMS];
    for (v, id) in params.iter_mut().zip(IDS) {
        *v = param(host, id);
    }
    harmonics::draw_ex(ui, r, &PALETTE, &mut st.harmonics, &params, Id::new(("sat-harmonics", host.track.0, host.ins.id.0)), lead);
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
    if pill_ex(ui, r, "sat-autogain", &PALETTE, "Auto Gain", auto_gain, PillStyle::Small, plain).clicked() {
        family::pill_set(app, host, fam, &FAMILY, "autoGain", if auto_gain { 0.0 } else { 1.0 });
    }
}

/// `sat::Meters::paint`: IN and OUT, −48..0 dBFS.
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
        assert_eq!(signed_db(0.01), "0.0 dB");
        assert_eq!(signed_db(-0.04), "0.0 dB");
        assert_eq!(signed_db(6.0), "+6.0 dB");
        assert_eq!(signed_db(-3.3), "-3.3 dB");
        assert_eq!(format_percent(1.0), "100 %");
        assert_eq!(format_air(0.0), "Off");
        assert_eq!(format_bias(0.5), "0");
        assert_eq!(format_focus(0.5), "Flat");
        assert_eq!(circuit(99).name, "FET");
        assert_eq!(circuit(-1).name, "Iron");
    }
}

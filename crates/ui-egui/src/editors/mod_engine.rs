//! ModEngine's VST editor (`fs_mod`), a translation of `Engine 4 - Mod/Source/PluginEditor.cpp`
//! (+ `PluginEditor.h`'s `SwitchRow`, `ModConfig.h`) on the editor kit: the family look tinted
//! #8FD3C6 at `setSize (900, requiredHeight())` = 424 + the five switch rows (669), the header,
//! the two-row Circuit selector and its blurb, the two dial rows (dials a circuit does not use are
//! greyed, not hidden), Link / the set points / Calibrate beside Level, the switch rows with their
//! descriptions, the LFO readout, and the four-step calibration wizard.
//!
//! Calibration: the C++ suspends Link while the wizard runs (`setCalibrationActive`, so Level can
//! be set by hand). Here Link's derivation runs in the command layer, so the wizard takes Link off
//! while it runs and puts it back after (on when it completes, as `calEnd (true)`), the pill
//! showing the state it will return to.

mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, segmented_ex};
use crate::editor_kit::family::{self, Family, FamilyState, HeaderRects};
use crate::editor_kit::link_cal::{self, CalSpec};
use crate::editor_kit::look::{self, CAPTION_SIZE, LABEL_SIZE, font};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb};
use egui::{Align2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::mod_engine::MOD_PARAMS;
use soundcraft_dsp::plugins::fs::mod_engine::mod_config as cfg;
use soundcraft_dsp::plugins::fs::mod_engine::mod_engine_dsp as d;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ---- resized()'s constants, by name
const PAD_X: f32 = 24.0;
const PAD_Y: f32 = 15.0;
const GAP: f32 = 14.0;
const HEADER_H: f32 = 26.0;
/// Two rows of seven, so the circuit names stay readable.
const CIRCUIT_ROW_H: f32 = 58.0;
const BLURB_H: f32 = 16.0;
/// Link / readout / Calibrate, beside the Level knob.
const LINK_COL_W: f32 = 128.0;
const KNOB_MIN: f32 = 66.0;
const KNOB_MAX: f32 = 104.0;
const KNOB_DEFAULT: f32 = 82.0;
/// The second row is deliberately smaller: these are trims.
const ROW2_KNOB: f32 = 62.0;
const ROW2_H: f32 = ROW2_KNOB + 28.0;
const FOOT_H: f32 = 18.0;
const DEFAULT_WIDTH: f32 = 900.0;

/// `SwitchRow`.
const ROW_H: f32 = 26.0;
const DESC_H: f32 = 16.0;
const LABEL_W: f32 = 52.0;
const LABEL_GAP: f32 = 12.0;
const SWITCH_ROW_PREF_H: f32 = ROW_H + 3.0 + DESC_H;

/// `kAccent`: the original UI's PEDAL.accent.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFF8FD3C6));

/// `getPresetParams`: circuit, variant, the performance dials, the trims and the switches.
const FAMILY: Family = Family::new("ModEngine", &["level", "trim", "link", "linkLo", "linkHi", "tempo"]);

/// The switch rows below the dials (`getSwitches()` without the Circuit row): label, parameter.
const SWITCH_ROWS: [(&str, usize); 5] =
    [("Variant", d::P_VARIANT), ("Input", d::P_STEREO), ("Shape", d::P_SHAPE), ("Sync", d::P_SYNC), ("Div", d::P_DIVISION)];

/// The Shape row's descriptions (`getSwitches()`).
/// Each circuit's hardware credit line (`crate::hardware_credit`), in `CIRCUIT_BLURBS` order:
/// ModEngine's fourteen circuits are component-level models (`soundcraft_dsp` `fs::mod_engine`).
const CIRCUIT_CREDITS: [&str; 14] = [
    "Component-level model of the Roland SDD-320 Dimension D. No affiliation; Roland is a trademark of its owner.",
    "Component-level model of the Boss CE-1 Chorus Ensemble. No affiliation; Boss is a trademark of its owner.",
    "Component-level model of the Boss CE-2 Chorus. No affiliation; Boss is a trademark of its owner.",
    "Component-level model of the Electro-Harmonix Small Clone. No affiliation; Electro-Harmonix and Small Clone are trademarks of their owner.",
    "Component-level model of the Roland JC-120's chorus. No affiliation; Roland and Jazz Chorus are trademarks of their owner.",
    "",
    "Component-level model of the Electro-Harmonix Electric Mistress. No affiliation; Electro-Harmonix and Electric Mistress are trademarks of their owner.",
    "Component-level model of the A/DA Flanger. No affiliation; A/DA is a trademark of its owner.",
    "Component-level model of the MXR Phase 90. No affiliation; MXR and Phase 90 are trademarks of their owner.",
    "Component-level model of the Electro-Harmonix Small Stone. No affiliation; Electro-Harmonix and Small Stone are trademarks of their owner.",
    "Component-level model of the Shin-ei Uni-Vibe. No affiliation; Uni-Vibe is a trademark of its owner.",
    "Component-level model of the Fender blackface tremolo. No affiliation; Fender is a trademark of its owner.",
    "Component-level model of the Fender brownface harmonic tremolo. No affiliation; Fender is a trademark of its owner.",
    "Component-level model of the Magnatone varistor vibrato. No affiliation; Magnatone is a trademark of its owner.",
];

const SHAPE_DESCRIPTIONS: [&str; 5] = [
    "Whatever the selected circuit shipped with.",
    "A plain sine.",
    "Triangle \u{2014} a linear sweep, the classic BBD chorus shape.",
    "Trapezoid \u{2014} a triangle with flattened peaks, as in the Dimension D.",
    "Square \u{2014} hard switching between the two extremes.",
];

/// `kDriveName`, `kCalInputText`.
const DRIVE_NAME: &str = "Depth";
const CAL_INPUT_TEXT: &str = "Depth is at 5 and the pedal is on. On a bucket-brigade circuit, turn Input until the compander breathes a little on hard chords and not at all on clean picking. Around 0 dB is right for most guitars.";

#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    cal: link_cal::Cal,
    /// The Variant row's options: the last circuit's that had variants (`seg.setOptions`), the
    /// parameter's own labels until then.
    variants: Option<&'static [&'static str]>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// Scripts: open the slim layout's circuit menu on the next frame.
    models_open: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
}

/// Draws ModEngine's editor for `module` on `track`.
pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn id(i: usize) -> &'static str {
    MOD_PARAMS.get(i).map_or("", |p| p.id)
}

fn pv(ins: &Insert, i: usize) -> f32 {
    MOD_PARAMS.get(i).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

fn choices(i: usize) -> &'static [&'static str] {
    MOD_PARAMS.get(i).map_or(&[], |p| p.choices)
}

/// `requiredHeight()`.
pub fn required_height() -> f32 {
    let rows = SWITCH_ROWS.len() as f32;
    let rows_h = rows * SWITCH_ROW_PREF_H + (rows - 1.0) * 5.0;
    PAD_Y * 2.0 + HEADER_H + GAP + CIRCUIT_ROW_H + 4.0 + BLURB_H + GAP + (KNOB_DEFAULT + 30.0) + GAP + ROW2_H + GAP + rows_h + GAP + FOOT_H
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_mod";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(DEFAULT_WIDTH, required_height()) }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

fn take_top(b: &mut Rect, h: f32) -> Rect {
    let r = Rect::from_min_size(b.min, vec2(b.width(), h));
    b.min.y += h;
    r
}

fn plain() -> PillLook {
    PillLook { tracked: false, ..PillLook::default() }
}

/// What both layouts do first: the scripted drawer tabs, the preset browser (which covers the
/// whole editor while it is open: then `None`, paint nothing else) and the Variant row's memory.
/// Returns the circuit and whether the LFO is synced.
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> Option<(i32, &'static cfg::CircuitDef, bool)> {
    let ins = host.ins;
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Calibrate") if !st.cal.active => link_cal::start(app, host, &mut st.cal, &cal_spec()),
        Some("Models") if st.slim => st.models_open = true,
        Some("More") if st.slim => st.slim_more = true,
        _ => {}
    }
    let circuit = pv(ins, d::P_CIRCUIT).round() as i32;
    let c = cfg::circuit(circuit);
    if family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, c.name) {
        return None;
    }
    if !c.variants.is_empty() {
        st.variants = Some(c.variants);
    }
    Some((circuit, c, pv(ins, d::P_SYNC) >= 0.5))
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let pal = &PALETTE;
    let ins = host.ins;
    let Some((circuit, c, synced)) = prelude(st, app, ui, host) else { return };
    let p = ui.painter().clone();
    look::paint_background(&p, host.rect);

    // ---- resized()
    let mut b = host.rect.shrink2(vec2(PAD_X, PAD_Y));
    let rows = SWITCH_ROWS.len() as f32;
    let rows_h = rows * SWITCH_ROW_PREF_H + (rows - 1.0) * 5.0;
    let fixed_h = HEADER_H + GAP + CIRCUIT_ROW_H + 4.0 + BLURB_H + GAP + GAP + ROW2_H + GAP + rows_h + GAP + FOOT_H;
    let knob = (b.height() - fixed_h - 30.0).clamp(KNOB_MIN, KNOB_MAX);

    let header_area = take_top(&mut b, HEADER_H);
    let btn_h = pill_height(PillStyle::Small);
    let header = {
        let mut right = header_area;
        let mut place_right = |w: f32| {
            let r = Rect::from_min_size(pos2(right.right() - w, right.center().y - btn_h * 0.5), vec2(w, btn_h));
            right.max.x -= w + 6.0;
            r
        };
        let bypass = place_right(family::small_pill_w(ui, "Bypass").trunc());
        let b_r = place_right(family::small_pill_w(ui, "B").trunc().max(28.0));
        let a_r = place_right(family::small_pill_w(ui, "A").trunc().max(28.0));
        let undo_w = family::small_pill_w(ui, "Undo").trunc();
        right.max.x -= 2.0;
        let undo = Rect::from_min_size(pos2(right.right() - undo_w, right.center().y - btn_h * 0.5), vec2(undo_w, btn_h));
        right.max.x -= undo_w + 6.0;
        let arrow_w = 24.0;
        let name_w = (right.width() - 2.0 * arrow_w - 12.0).trunc().clamp(140.0, 230.0);
        let strip = Rect::from_center_size(right.center(), vec2(name_w + 2.0 * arrow_w + 12.0, HEADER_H));
        let prev = Rect::from_center_size(pos2(strip.left() + arrow_w * 0.5, strip.center().y), vec2(arrow_w, 22.0));
        let next = Rect::from_center_size(pos2(strip.right() - arrow_w * 0.5, strip.center().y), vec2(arrow_w, 22.0));
        let name = Rect::from_min_max(pos2(prev.right() + 6.0, strip.center().y - 11.0), pos2(next.left() - 6.0, strip.center().y + 11.0));
        HeaderRects { prev, name, next, undo, a: a_r, b: b_r, bypass }
    };
    b.min.y += GAP;
    let mut circuit_row = take_top(&mut b, CIRCUIT_ROW_H);
    let circuit_label = Rect::from_min_size(circuit_row.min, vec2(LABEL_W, circuit_row.height()));
    circuit_row.min.x += LABEL_W + LABEL_GAP;
    b.min.y += 4.0;
    let mut blurb_area = take_top(&mut b, BLURB_H);
    blurb_area.min.x += LABEL_W + LABEL_GAP;
    b.min.y += GAP;

    // dial row one, with the Link column beside Level
    let mut row1 = take_top(&mut b, knob + 30.0);
    let link_col = Rect::from_min_max(pos2(row1.right() - LINK_COL_W, row1.top()), row1.max);
    row1.max.x = link_col.left();
    let cell_w = (row1.width() as i32 / 6) as f32;
    let small_knob = (knob * 0.78).round();
    let row1_dials = ROW1_DIALS;
    let row1_rects: Vec<Rect> = (0..6)
        .map(|i| {
            let k = if row1_dials[i].2 { small_knob } else { knob };
            let cx = row1.left() + cell_w * i as f32 + cell_w * 0.5;
            Rect::from_min_size(pos2((cx - k * 0.5).trunc(), row1.top() + ((knob - k) / 2.0).trunc()), vec2(k, k + 30.0))
        })
        .collect();
    let stack_h = btn_h + 6.0 + 14.0 + 8.0 + btn_h;
    let stack =
        Rect::from_min_size(pos2(link_col.left() + 4.0, row1.top() + ((knob - stack_h) / 2.0).trunc()), vec2(link_col.width() - 8.0, stack_h));
    let link_w = family::small_pill_w(ui, "Link").trunc().max(64.0);
    let link_r = Rect::from_min_size(stack.min, vec2(link_w, btn_h));
    let link_readout = Rect::from_min_size(pos2(stack.left(), stack.top() + btn_h + 4.0), vec2(stack.width(), 14.0));
    let cal_w = family::small_pill_w(ui, "Calibrate").trunc().max(78.0);
    let cal_r = Rect::from_min_size(pos2(stack.left(), link_readout.bottom() + 6.0), vec2(cal_w, btn_h));
    b.min.y += GAP;

    // dial row two: the trims, smaller, over the same six columns
    let mut row2 = take_top(&mut b, ROW2_H);
    row2.max.x -= LINK_COL_W;
    let cell2 = (row2.width() as i32 / 6) as f32;
    let row2_dials = ROW2_DIALS;
    let row2_rects: Vec<Rect> = (0..6)
        .map(|i| {
            let cx = row2.left() + cell2 * i as f32 + cell2 * 0.5;
            Rect::from_min_size(pos2((cx - ROW2_KNOB * 0.5).trunc(), row2.top()), vec2(ROW2_KNOB, ROW2_KNOB + 28.0))
        })
        .collect();
    b.min.y += GAP;
    let switch_rects: Vec<Rect> = SWITCH_ROWS
        .iter()
        .map(|_| {
            let r = take_top(&mut b, SWITCH_ROW_PREF_H);
            b.min.y += 5.0;
            r
        })
        .collect();
    let rate_readout = take_top(&mut b, FOOT_H);

    // ---- paint()
    look::paint_title(&p, header_area, "Mod", "Engine", pal);
    p.text(pos2(circuit_label.left(), circuit_label.center().y), Align2::LEFT_CENTER, "Circuit", font(LABEL_SIZE, false), pal.text_body);
    p.text(
        pos2(blurb_area.left(), blurb_area.center().y),
        Align2::LEFT_CENTER,
        cfg::CIRCUIT_BLURBS[circuit.clamp(0, 13) as usize],
        font(CAPTION_SIZE, false),
        pal.text_faint,
    );
    let ci = circuit.clamp(0, 13) as usize;
    crate::hardware_credit::hover(ui, blurb_area, "mod_circuit", cfg::CIRCUIT_BLURBS[ci], CIRCUIT_CREDITS.get(ci).copied().unwrap_or(""));
    let text = lfo_text(ins, circuit, synced);
    p.text(pos2(rate_readout.left(), rate_readout.center().y), Align2::LEFT_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);
    p.text(pos2(link_readout.left(), link_readout.center().y), Align2::LEFT_CENTER, link_text(ins), font(CAPTION_SIZE, false), pal.text_body);

    // ---- the header
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &header);

    // ---- circuit selector, two rows of seven
    if let Some(i) =
        segmented_ex(ui, circuit_row, "circuit", pal, choices(d::P_CIRCUIT), 2, Some(circuit.clamp(0, 13) as usize), SelectionStyle::Filled, false)
    {
        pick_circuit(st, app, host, i);
    }

    // ---- dials
    for (r, spec) in row1_rects.iter().chain(&row2_rects).zip(row1_dials.iter().chain(&row2_dials)) {
        dial_ui(st, app, ui, host, *r, *spec, c, synced);
    }

    // ---- Link and Calibrate
    link_ui(st, app, ui, host, link_r);
    calibrate_ui(st, app, ui, host, cal_r);

    // ---- switch rows
    for (r, (label, i)) in switch_rects.iter().zip(SWITCH_ROWS) {
        let (opts, disabled, desc) = switch_def(st, ins, c, i, synced);
        switch_row(app, ui, host, st, *r, label, i, opts, disabled, desc);
    }

    // ---- calibration panel, floating over the second dial row and the switches
    if st.cal.active {
        let panel = host.rect.shrink2(vec2(PAD_X, PAD_Y));
        let top = panel.top().max(cal_r.bottom() + GAP);
        let panel = Rect::from_min_size(pos2(panel.left(), top), vec2(panel.width(), (panel.bottom() - top).clamp(72.0, 100.0)));
        link_cal::panel(app, ui, host, pal, &mut st.cal, &cal_spec(), panel);
    }
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// The first dial row (parameter, name, small).
const ROW1_DIALS: [(usize, &str, bool); 6] = [
    (d::P_TRIM, "Input", true),
    (d::P_DEPTH, "Depth", false),
    (d::P_RATE, "Rate", false),
    (d::P_MIX, "Mix", false),
    (d::P_WIDTH, "Width", false),
    (d::P_LEVEL, "Level", false),
];

/// The second dial row: the trims.
const ROW2_DIALS: [(usize, &str, bool); 6] = [
    (d::P_REGEN, "Regen", false),
    (d::P_DRIVE, "Drive", false),
    (d::P_NOISE, "Noise", false),
    (d::P_BIAS, "Bias", false),
    (d::P_TONE, "Tone", false),
    (d::P_TEMPO, "Tempo", true),
];

/// The circuit picker's pick (a complete gesture after `pushUndo`).
fn pick_circuit(st: &mut Editor, app: &mut SoundApp, host: &Host, i: usize) {
    family::set(app, host, &mut st.fam, &FAMILY, "circuit", i as f32);
}

/// The LFO readout: its rate, and what it is synced to.
fn lfo_text(ins: &Insert, circuit: i32, synced: bool) -> String {
    let hz = d::lfo_rate_hz(
        circuit,
        pv(ins, d::P_VARIANT).round() as i32,
        pv(ins, d::P_RATE),
        pv(ins, d::P_SYNC).round() as i32,
        pv(ins, d::P_DIVISION).round() as i32,
        pv(ins, d::P_TEMPO),
    );
    let value = if hz >= 10.0 {
        format!("{hz:.1}")
    } else if hz >= 1.0 {
        format!("{hz:.2}")
    } else {
        format!("{hz:.3}")
    };
    let mut text = format!("LFO  {value} Hz");
    if synced {
        let div = choices(d::P_DIVISION).get(pv(ins, d::P_DIVISION).round() as usize).copied().unwrap_or("");
        text += &format!("   \u{00B7}   synced to {div} at {} bpm", d::knob_to_bpm(pv(ins, d::P_TEMPO)).round() as i32);
    }
    text
}

/// Link's set points.
fn link_text(ins: &Insert) -> String {
    format!("0 \u{2192} {}   \u{00B7}   10 \u{2192} {}", (pv(ins, d::P_LINK_LO) * 10.0).round() as i32, (pv(ins, d::P_LINK_HI) * 10.0).round() as i32)
}

/// A dial of the two rows (`applyCircuitHook`: greyed where unused; Level locked while Linked;
/// the wizard's focus).
#[allow(clippy::too_many_arguments)]
fn dial_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, spec: (usize, &str, bool), c: &cfg::CircuitDef, synced: bool) {
    let (i, label, small) = spec;
    let linked = pv(host.ins, d::P_LINK) >= 0.5 && !st.cal.active;
    let inactive = match i {
        d::P_TEMPO => !synced,
        d::P_RATE => synced,
        d::P_REGEN => !c.regen,
        d::P_NOISE => !c.noise,
        d::P_BIAS => !c.bias,
        d::P_TONE => !c.tone,
        _ => false,
    };
    let cal_focus = (i == d::P_TRIM && st.cal.input_focus()) || (i == d::P_LEVEL && st.cal.level_focus());
    dial(app, ui, host, st, r, i, label, small, inactive, i == d::P_LEVEL && linked, cal_focus);
}

/// The Link pill in `r` (while the wizard runs it sets what Link returns to).
fn link_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let link_on = if st.cal.active { st.cal.saved_link >= 0.5 } else { pv(host.ins, d::P_LINK) >= 0.5 };
    if pill_ex(ui, r, "link", &PALETTE, "Link", link_on, PillStyle::Small, plain()).clicked() {
        if st.cal.active {
            st.cal.saved_link = if link_on { 0.0 } else { 1.0 };
        } else {
            family::pill_set(app, host, &mut st.fam, &FAMILY, "link", if link_on { 0.0 } else { 1.0 });
        }
    }
}

/// The Calibrate pill in `r`: starts the wizard.
fn calibrate_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    if pill_ex(ui, r, "calibrate", &PALETTE, "Calibrate", false, PillStyle::Small, plain()).clicked() && !st.cal.active {
        link_cal::start(app, host, &mut st.cal, &cal_spec());
    }
}

/// A switch row's options, whether it is disabled, and its description.
fn switch_def(st: &Editor, ins: &Insert, c: &'static cfg::CircuitDef, i: usize, synced: bool) -> (&'static [&'static str], bool, &'static str) {
    match i {
        d::P_VARIANT if c.variants.is_empty() => (st.variants.unwrap_or(choices(i)), true, "This circuit has no variants."),
        d::P_VARIANT => (c.variants, false, ""),
        d::P_DIVISION => (choices(i), !synced, if synced { "" } else { "Set Sync to lock the LFO to this note value." }),
        d::P_SHAPE => (choices(i), false, SHAPE_DESCRIPTIONS[(pv(ins, i).round() as usize).min(4)]),
        _ => (choices(i), false, ""),
    }
}

/// `SwitchRow`'s Segmented in `seg`, its options in `rows` rows. `seg.setEnabled (false)` when disabled: drawn the same, but
/// a click does nothing.
#[allow(clippy::too_many_arguments)]
fn switch_seg(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, seg: Rect, i: usize, opts: &[&str], disabled: bool, rows: usize) {
    let sel = (pv(host.ins, i).round().max(0.0) as usize).min(opts.len().saturating_sub(1));
    let picked = segmented_ex(ui, seg, ("row", i), &PALETTE, opts, rows, Some(sel), SelectionStyle::Filled, false);
    if let Some(n) = picked.filter(|_| !disabled) {
        family::set(app, host, &mut st.fam, &FAMILY, id(i), n as f32);
    }
}

/// One `fs::Dial` in the family style, bound to parameter `i`.
#[allow(clippy::too_many_arguments)]
fn dial(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut Editor,
    r: Rect,
    i: usize,
    label: &str,
    small: bool,
    inactive: bool,
    locked: bool,
    cal_focus: bool,
) {
    let pid = id(i);
    let v = family::value01(host, pid);
    let text = MOD_PARAMS.get(i).and_then(|p| p.display).map_or_else(|| format!("{v:.2}"), |f| f(v));
    let o = DialOpts { label, value_text: &text, default: family::default01(host, pid), small, inactive, locked, cal_focus, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, r, &PALETTE, pid, &o);
}

/// `SwitchRow`: the name at the left (faint when disabled), the Segmented after it, the current
/// option's description under it.
#[allow(clippy::too_many_arguments)]
fn switch_row(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut Editor,
    r: Rect,
    label: &str,
    i: usize,
    opts: &[&str],
    disabled: bool,
    desc: &str,
) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    let row = Rect::from_min_size(r.min, vec2(r.width(), ROW_H));
    p.text(
        pos2(row.left(), row.center().y),
        Align2::LEFT_CENTER,
        label,
        font(LABEL_SIZE, false),
        if disabled { pal.text_faint } else { pal.text_body },
    );
    let desc_r = Rect::from_min_max(pos2(r.left() + LABEL_W + LABEL_GAP, row.bottom() + 3.0), r.max);
    p.with_clip_rect(desc_r.intersect(p.clip_rect())).text(desc_r.left_top(), Align2::LEFT_TOP, desc, font(CAPTION_SIZE, false), pal.text_faint);
    let seg = Rect::from_min_max(pos2(row.left() + LABEL_W + LABEL_GAP, row.top()), row.max);
    switch_seg(app, ui, host, st, seg, i, opts, disabled, 1);
}

/// `currentCircuit()`'s Link facts for the wizard: Depth stands in for the drive role, pinned at
/// 0 and 10, with one pair of set points for the whole plugin.
fn cal_spec() -> CalSpec<'static> {
    CalSpec {
        drive: "depth",
        drive_name: DRIVE_NAME,
        anchor_lo: 0.0,
        anchor_hi: 1.0,
        link_lo: "linkLo",
        link_hi: "linkHi",
        input_text: CAL_INPUT_TEXT.into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_height_is_424_plus_the_rows() {
        assert_eq!(required_height(), 424.0 + 5.0 * 45.0 + 4.0 * 5.0);
        assert_eq!(link_cal::texts(4, &cal_spec()).0, "Set your Depth 10 level");
    }
}

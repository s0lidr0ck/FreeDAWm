//! FilterEngine's VST editor (`fs_filter`), a translation of `Engine 5 - Filter/Source/
//! PluginEditor.cpp` (+ `PluginEditor.h`'s `SwitchRow` and `StepStrip`, `FilterConfig.h`) on the
//! editor kit: the family look tinted #35E0C2 at `setSize (920, requiredHeight())` = 522, plus
//! 13 + n x 50 + (n - 1) x 6 for the circuit's n switch rows; the header, the circuit row and what
//! it says about itself, the switch rows, Cutoff (its readout the frequency the filter is at) /
//! Reso / Drive / Mix, the mod source and Depth, the modulation panel for that source (Envelope,
//! LFO, Steps with its step strip, or Manual's note), and the setup row with the tempo line.
//!
//! Controls the source does not use are hidden, not greyed (`refreshAll`).

mod slim;
mod step_strip;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, pill_ex, pill_height, segmented_ex};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{self, BLURB_SIZE, CAPTION_SIZE, LABEL_SIZE, font};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, consume_wheel, end_gesture, juce_wheel_delta_y, set_params_merged};
use egui::{Align2, Rect, Sense, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::filter::FILTER_PARAMS;
use soundcraft_dsp::plugins::fs::filter::filter_config as cfg;
use soundcraft_dsp::plugins::fs::filter::filter_engine_dsp as d;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ---- resized()'s constants, by name
const PAD_X: f32 = 22.0;
const PAD_Y: f32 = 14.0;
const GAP: f32 = 13.0;
const HEADER_H: f32 = 26.0;
const CIRCUIT_ROW_H: f32 = 30.0;
/// Two lines: these circuits have a lot to say.
const BLURB_H: f32 = 30.0;
const MAIN_KNOB: f32 = 104.0;
const MAIN_ROW_H: f32 = MAIN_KNOB + 32.0;
const MOD_ROW_H: f32 = 26.0;
const MOD_PANEL_H: f32 = 120.0;
const SETUP_H: f32 = 62.0;
const DEFAULT_WIDTH: f32 = 920.0;

/// `SwitchRow`.
const ROW_H: f32 = 26.0;
const DESC_H: f32 = 20.0;
const LABEL_W: f32 = 62.0;
const LABEL_GAP: f32 = 12.0;
const SWITCH_ROW_PREF_H: f32 = ROW_H + 4.0 + DESC_H;

/// `kAccent`: the original UI's ACCENT.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFF35E0C2));

/// `getPresetParams`: everything but Level, Input, the tempo source and BPM.
const FAMILY: Family = Family::new("FilterEngine", &["level", "trim", "tempoSrc", "bpm"]);

#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    /// The last live cutoff the engine reported (shown while the transport is stopped).
    last_hz: Option<f32>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// Scripts: open the slim layout's circuit menu on the next frame.
    models_open: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
}

/// Draws FilterEngine's editor for `module` on `track`.
pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn id(i: usize) -> &'static str {
    FILTER_PARAMS.get(i).map_or("", |p| p.id)
}

fn pv(ins: &Insert, i: usize) -> f32 {
    FILTER_PARAMS.get(i).map_or(0.0, |p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)))
}

fn choices(i: usize) -> &'static [&'static str] {
    FILTER_PARAMS.get(i).map_or(&[], |p| p.choices)
}

fn circuit_of(ins: &Insert) -> (i32, &'static cfg::CircuitDef) {
    let i = (pv(ins, d::P_CIRCUIT).round() as i32).clamp(0, 8);
    (i, cfg::circuit(i))
}

/// `requiredHeight()` for a circuit with `rows` switch rows.
pub fn required_height(rows: usize) -> f32 {
    let n = rows as f32;
    let rows_h = if rows == 0 { 0.0 } else { n * SWITCH_ROW_PREF_H + (n - 1.0) * 6.0 };
    PAD_Y
        + HEADER_H
        + GAP
        + CIRCUIT_ROW_H
        + 4.0
        + BLURB_H
        + if rows > 0 { GAP + rows_h } else { 0.0 }
        + GAP
        + MAIN_ROW_H
        + GAP
        + MOD_ROW_H
        + 8.0
        + MOD_PANEL_H
        + GAP
        + SETUP_H
        + PAD_Y
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_filter";

    fn size(&self, ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(DEFAULT_WIDTH, required_height(circuit_of(ins).1.switches.len())) }
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

fn take_left(b: &mut Rect, w: f32) -> Rect {
    let r = Rect::from_min_size(b.min, vec2(w, b.height()));
    b.min.x += w;
    r
}

/// `layVisible`: the shown items evenly across `area` (integer widths, from the left).
fn lay(area: Rect, items: &[usize]) -> Vec<(usize, Rect)> {
    if items.is_empty() {
        return Vec::new();
    }
    let w = (area.width() as i32 / items.len() as i32) as f32;
    items.iter().enumerate().map(|(k, i)| (*i, Rect::from_min_size(pos2(area.left() + w * k as f32, area.top()), vec2(w, area.height())))).collect()
}

fn plain() -> PillLook {
    PillLook { tracked: false, ..PillLook::default() }
}

/// The module's live readouts (`[2]` cutoff Hz, `[4]` the tempo in use, `[5]` the step).
fn readouts(host: &Host) -> Option<[f32; 8]> {
    host.meters.and_then(|m| m.modules.iter().find(|x| x.module == host.ins.id)).map(|m| m.readouts)
}

/// `getEffectiveBpm`, and the host's tempo (`getHostBpm`): the session's tempo is the host's
/// (SoundCraft always gives one); the engine's own `bpmOut` once it has played.
fn tempo(host: &Host) -> (f32, f32) {
    let want_host = pv(host.ins, d::P_TEMPO_SRC) < 0.5;
    let live = readouts(host).map(|r| r[4]).filter(|v| v.is_finite() && *v != 0.0);
    let host_bpm = match live {
        Some(v) if want_host => v.max(0.0),
        _ => host.session.tempo.tempo_at_tick(0) as f32,
    };
    let eff = if want_host && host_bpm >= 20.0 { host_bpm } else { d::knob_to_bpm(pv(host.ins, d::P_BPM)) };
    (eff, host_bpm)
}

/// What both layouts read before they paint.
struct Frame {
    circuit: i32,
    c: &'static cfg::CircuitDef,
    mod_src: i32,
    lfo_sync: bool,
    a: cfg::Applicable,
    bpm: f32,
    host_bpm: f32,
    live: Option<[f32; 8]>,
}

/// What both layouts do first: the scripted drawer tabs, the preset browser (which covers the
/// whole editor while it is open: then `None`, paint nothing else), and the live readouts.
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> Option<Frame> {
    let ins = host.ins;
    match family::take_drawer_tab(app, host, &mut st.fam).as_deref() {
        Some("Models") if st.slim => st.models_open = true,
        Some("More") if st.slim => st.slim_more = true,
        _ => {}
    }
    let (circuit, c) = circuit_of(ins);
    if family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, c.name) {
        return None;
    }
    let mod_src = (pv(ins, d::P_MOD_SRC).round() as i32).clamp(0, 3);
    let lfo_sync = pv(ins, d::P_LFO_SYNC) > 0.5;
    let a = cfg::what_applies(mod_src, lfo_sync);
    let (bpm, host_bpm) = tempo(host);
    let live = readouts(host);
    if let Some(hz) = live.map(|r| r[2]).filter(|v| v.is_finite() && *v > 0.0) {
        st.last_hz = Some(hz);
    }
    // The live cutoff and the running step (`startTimerHz (20)`).
    if live.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    Some(Frame { circuit, c, mod_src, lfo_sync, a, bpm, host_bpm, live })
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let pal = &PALETTE;
    let ins = host.ins;
    let Some(Frame { circuit, c, mod_src, lfo_sync, a, bpm, host_bpm, live }) = prelude(st, app, ui, host) else { return };
    let p = ui.painter().clone();
    look::paint_background(&p, host.rect);

    // ---- resized()
    let mut b = host.rect.shrink2(vec2(PAD_X, PAD_Y));
    let header_area = take_top(&mut b, HEADER_H);
    let pill_h = pill_height(PillStyle::Small);
    let header = {
        let mut row = header_area;
        let mut centred = Rect::from_center_size(row.center(), vec2((row.width() - 360.0).min(420.0), pill_h));
        let mut from_right = |w: f32, gap: f32| {
            let r = Rect::from_min_size(pos2(row.right() - w, row.center().y - pill_h * 0.5), vec2(w, pill_h));
            row.max.x -= w + gap;
            r
        };
        let bypass = from_right(62.0, 6.0);
        let b_r = from_right(26.0, 3.0);
        let a_r = from_right(26.0, 6.0);
        let undo = from_right(48.0, 0.0);
        let prev = take_left(&mut centred, 24.0);
        centred.min.x += 5.0;
        let next = Rect::from_min_max(pos2(centred.right() - 24.0, centred.top()), centred.max);
        centred.max.x -= 24.0 + 5.0;
        family::HeaderRects { prev, name: centred, next, undo, a: a_r, b: b_r, bypass }
    };
    b.min.y += GAP;
    let circuit_r = take_top(&mut b, CIRCUIT_ROW_H);
    b.min.y += 4.0;
    let blurb_area = take_top(&mut b, BLURB_H);
    let mut switch_rects = Vec::with_capacity(c.switches.len());
    if !c.switches.is_empty() {
        b.min.y += GAP;
        for _ in c.switches {
            switch_rects.push(take_top(&mut b, SWITCH_ROW_PREF_H));
            b.min.y += 6.0;
        }
        b.min.y -= 6.0;
    }
    b.min.y += GAP;
    let main_row = take_top(&mut b, MAIN_ROW_H);
    b.min.y += GAP;
    let mut mod_row = take_top(&mut b, MOD_ROW_H);
    let w0 = (mod_row.width() as i32 / 2).min(300) as f32;
    let mod_src_r = take_left(&mut mod_row, w0);
    mod_row.min.x += 14.0;
    let w1 = mod_row.width().min(150.0);
    let depth_r = take_left(&mut mod_row, w1);
    b.min.y += 8.0;
    let mod_panel = take_top(&mut b, MOD_PANEL_H);
    b.min.y += GAP;
    let setup_row = take_top(&mut b, SETUP_H);

    // ---- paint()
    look::paint_title(&p, header_area, "Filter", "Engine", pal);
    {
        let full_w = p.layout_no_wrap(c.full.to_string(), font(LABEL_SIZE, true), pal.text_dim).size().x.trunc() + 14.0;
        let mut area = blurb_area;
        let full_r = take_left(&mut area, full_w.min((blurb_area.width() as i32 / 3) as f32));
        p.text(full_r.left_top(), Align2::LEFT_TOP, c.full, font(LABEL_SIZE, true), pal.text_dim);
        fitted(&p, area, c.blurb, BLURB_SIZE, pal.text_body);
    }
    look::paint_panel(&p, mod_panel, look::PANEL_CORNER);
    if mod_src == 0 {
        p.text(mod_panel.center(), Align2::CENTER_CENTER, MANUAL_NOTE, font(BLURB_SIZE, false), pal.text_faint);
    }
    {
        let text = tempo_line(ins, bpm, host_bpm);
        let r = Rect::from_min_max(pos2(setup_row.left(), setup_row.bottom() - 14.0), setup_row.max);
        p.text(pos2(r.right(), r.center().y), Align2::RIGHT_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);
    }

    // ---- the header
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &header);

    // ---- circuit
    if let Some(i) = segmented_ex(ui, circuit_r, "circuit", pal, choices(d::P_CIRCUIT), 1, Some(circuit as usize), SelectionStyle::Filled, false) {
        pick_circuit(st, app, host, i);
    }
    for (r, sw) in switch_rects.iter().zip(c.switches) {
        switch_row(app, ui, host, st, *r, sw);
    }

    // ---- main dials (Cutoff's readout is the frequency the filter is at)
    let main = main_dials(st, ui, ins, c);
    let w = (main_row.width() as i32 / 4) as f32;
    for (k, (i, label, text)) in main.into_iter().enumerate() {
        let r = if k < 3 {
            Rect::from_min_size(pos2(main_row.left() + w * k as f32, main_row.top()), vec2(w, main_row.height()))
        } else {
            Rect::from_min_max(pos2(main_row.left() + w * 3.0, main_row.top()), main_row.max)
        };
        dial(app, ui, host, st, r, i, label, false, LabelLayout::Below, text, bpm);
    }

    // ---- mod source and Depth
    mod_src_ui(st, app, ui, host, mod_src_r, mod_src);
    if mod_src != 0 {
        depth_ui(st, app, ui, host, depth_r, bpm);
    }

    // ---- the modulation panel
    let panel = mod_panel.shrink2(vec2(16.0, 12.0));
    match mod_src {
        1 => {
            let strip = Rect::from_min_max(pos2(panel.left(), panel.bottom() - 26.0), panel.max);
            let seg = Rect::from_center_size(strip.center(), vec2(strip.width().min(220.0), 24.0));
            env_src_ui(st, app, ui, host, seg);
            let knobs = Rect::from_min_max(panel.min, pos2(panel.right(), strip.top() - 6.0));
            for (i, r) in lay(knobs, &panel_dials(mod_src, a)) {
                dial(app, ui, host, st, r, i, panel_label(i), true, LabelLayout::Below, None, bpm);
            }
        }
        2 => {
            let mut strip = Rect::from_min_size(panel.min, vec2(panel.width(), 26.0));
            let sw = (strip.width() - 70.0).min(380.0);
            let shape = take_left(&mut strip, sw);
            lfo_shape_ui(st, app, ui, host, shape, 1);
            strip.min.x += 10.0;
            let sync_r = Rect::from_center_size(pos2(strip.left() + 28.0, strip.center().y), vec2(56.0, 21.0));
            lfo_sync_ui(st, app, ui, host, sync_r, lfo_sync);
            let knobs = Rect::from_min_max(pos2(panel.left(), panel.top() + 26.0 + 8.0), panel.max);
            for (i, r) in lay(knobs, &panel_dials(mod_src, a)) {
                dial(app, ui, host, st, r, i, panel_label(i), true, LabelLayout::Below, None, bpm);
            }
        }
        3 => {
            let mut rest = panel;
            let knobs = take_left(&mut rest, (panel.width() as i32 / 3).min(230) as f32);
            for (i, r) in lay(knobs, &panel_dials(mod_src, a)) {
                dial(app, ui, host, st, r, i, panel_label(i), true, LabelLayout::Below, None, bpm);
            }
            rest.min.x += 14.0;
            steps_ui(st, app, ui, host, rest.shrink2(vec2(0.0, 6.0)), live);
        }
        _ => {}
    }

    // ---- setup row: Input, Level, BPM and the tempo source
    let w = (setup_row.width() as i32 / 4) as f32;
    for (k, (i, label)) in SETUP_DIALS.into_iter().enumerate() {
        let r = Rect::from_min_size(pos2(setup_row.left() + w * k as f32, setup_row.top()), vec2(w, setup_row.height()));
        dial(app, ui, host, st, r, i, label, true, LabelLayout::Right, None, bpm);
    }
    let last = Rect::from_min_max(pos2(setup_row.left() + w * 3.0, setup_row.top()), setup_row.max);
    tempo_src_ui(st, app, ui, host, Rect::from_center_size(last.center(), vec2(124.0, 21.0)));
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// Manual's note in the modulation panel.
const MANUAL_NOTE: &str = "Manual \u{2014} Cutoff is the whole control. Automate it, or map an expression pedal to it.";

/// The setup row's dials.
const SETUP_DIALS: [(usize, &str); 3] = [(d::P_TRIM, "Input"), (d::P_LEVEL, "Level"), (d::P_BPM, "BPM")];

/// The tempo line: where the tempo comes from, and the tempo in force.
fn tempo_line(ins: &Insert, bpm: f32, host_bpm: f32) -> String {
    let want_host = pv(ins, d::P_TEMPO_SRC) < 0.5;
    if want_host && host_bpm >= 20.0 {
        format!("HOST  {bpm:.1}")
    } else if want_host {
        format!("NO HOST TEMPO  \u{00B7}  {bpm:.1}")
    } else {
        format!("MANUAL  {bpm:.1}")
    }
}

/// The circuit picker's pick (a complete gesture after `pushUndo`).
fn pick_circuit(st: &mut Editor, app: &mut SoundApp, host: &Host, i: usize) {
    family::set(app, host, &mut st.fam, &FAMILY, "circuit", i as f32);
}

/// The four main dials: parameter, the circuit's name for it, and Cutoff's live readout.
fn main_dials(st: &Editor, ui: &Ui, ins: &Insert, c: &cfg::CircuitDef) -> [(usize, &'static str, Option<String>); 4] {
    let hz = st.last_hz.unwrap_or_else(|| crate::module_graphs::filter_display_cached(ui.ctx(), ins).knob_hz);
    [
        (d::P_CUTOFF, c.dial_cutoff, Some(cfg::format_cutoff_hz(hz))),
        (d::P_RESO, c.dial_reso, None),
        (d::P_DRIVE, "Drive", None),
        (d::P_MIX, "Mix", None),
    ]
}

/// The modulation source switch in `r`.
fn mod_src_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, mod_src: i32) {
    if let Some(i) = segmented_ex(ui, r, "modSrc", &PALETTE, choices(d::P_MOD_SRC), 1, Some(mod_src as usize), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "modSrc", i as f32);
    }
}

/// Depth in `r`. A 26 px row leaves the knob no room (`jmin (w, h - 30)`): the readout and name
/// only; the wheel still turns it (`mouseWheelMove` has no bounds check).
fn depth_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, bpm: f32) {
    dial(app, ui, host, st, r, d::P_DEPTH, "Depth", true, LabelLayout::Below, None, bpm);
    let resp = ui.interact(r, ui.id().with("depth-wheel"), Sense::hover());
    if resp.hovered() {
        let dy = juce_wheel_delta_y(ui);
        let v = pv(host.ins, d::P_DEPTH);
        let nv = (v + dy * 0.25).clamp(0.0, 1.0);
        if dy != 0.0 && (nv - v).abs() > f32::EPSILON {
            consume_wheel(ui);
            family::set(app, host, &mut st.fam, &FAMILY, "depth", nv);
        }
    }
}

/// The modulation panel's dials for the source (`refreshAll`: what the source does not use is
/// hidden).
fn panel_dials(mod_src: i32, a: cfg::Applicable) -> Vec<usize> {
    match mod_src {
        1 => vec![d::P_ENV_SENS, d::P_ENV_ATK, d::P_ENV_REL],
        2 => {
            let mut items = Vec::with_capacity(3);
            if a.note {
                items.push(d::P_NOTE);
            }
            if a.free_rate {
                items.push(d::P_LFO_RATE);
            }
            items.push(d::P_LFO_STEREO);
            items
        }
        3 => vec![d::P_SEQ_STEPS, d::P_NOTE, d::P_SEQ_GLIDE],
        _ => Vec::new(),
    }
}

/// A modulation dial's name.
fn panel_label(i: usize) -> &'static str {
    match i {
        d::P_ENV_SENS => "Sens",
        d::P_ENV_ATK => "Attack",
        d::P_ENV_REL => "Release",
        d::P_NOTE => "Note",
        d::P_LFO_RATE => "Rate",
        d::P_LFO_STEREO => "Stereo",
        d::P_SEQ_STEPS => "Steps",
        _ => "Glide",
    }
}

/// The envelope's source switch in `r`.
fn env_src_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let sel = Some(pv(host.ins, d::P_ENV_SRC).round() as usize);
    if let Some(i) = segmented_ex(ui, r, "envSrc", &PALETTE, choices(d::P_ENV_SRC), 1, sel, SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "envSrc", i as f32);
    }
}

/// The LFO's shape switch in `r`, its options in `rows` rows.
fn lfo_shape_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, rows: usize) {
    let sel = Some(pv(host.ins, d::P_LFO_SHAPE).round() as usize);
    if let Some(i) = segmented_ex(ui, r, "lfoShape", &PALETTE, choices(d::P_LFO_SHAPE), rows, sel, SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, "lfoShape", i as f32);
    }
}

/// The LFO's Sync pill in `r`.
fn lfo_sync_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, lfo_sync: bool) {
    if pill_ex(ui, r, "lfoSync", &PALETTE, "Sync", lfo_sync, PillStyle::Small, plain()).clicked() {
        family::pill_set(app, host, &mut st.fam, &FAMILY, "lfoSync", if lfo_sync { 0.0 } else { 1.0 });
    }
}

/// The step strip in `r` and its gesture (one undo step per drag).
fn steps_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, live: Option<[f32; 8]>) {
    let ins = host.ins;
    let mut values = [0.0f32; 16];
    for (k, v) in values.iter_mut().enumerate() {
        *v = pv(ins, d::P_STEP1 + k);
    }
    let count = d::knob_to_step_count(pv(ins, d::P_SEQ_STEPS)) as usize;
    let current = live.map(|r| r[5]).filter(|v| v.is_finite() && *v >= 0.0).map(|v| v as usize);
    let out = step_strip::step_strip(ui, r, &PALETTE, &values, count, current);
    if out.started {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if let Some((k, v)) = out.changed
        && (values[k] - v).abs() > 1e-6
    {
        set_params_merged(app, host, &[(id(d::P_STEP1 + k), v)], "steps");
    }
    if out.ended {
        end_gesture(app);
    }
}

/// The Use Manual BPM pill in `r`.
fn tempo_src_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let manual = pv(host.ins, d::P_TEMPO_SRC) > 0.5;
    if pill_ex(ui, r, "tempoSrc", &PALETTE, "Use Manual BPM", manual, PillStyle::Small, plain()).clicked() {
        family::pill_set(app, host, &mut st.fam, &FAMILY, "tempoSrc", if manual { 0.0 } else { 1.0 });
    }
}

/// `drawFittedText (.., topLeft, 2)`: the type shrinks until the text fits two lines.
fn fitted(p: &egui::Painter, r: Rect, text: &str, size: f32, c: egui::Color32) {
    let mut s = size;
    let mut g = p.layout(text.to_string(), font(s, false), c, r.width().max(1.0));
    while g.rows.len() > 2 && s > 8.0 {
        s -= 0.5;
        g = p.layout(text.to_string(), font(s, false), c, r.width().max(1.0));
    }
    p.with_clip_rect(r.intersect(p.clip_rect())).galley(r.left_top(), g, c);
}

/// One `fs::Dial` in the family style (`setSmall` for the modulation and setup dials), printed
/// by `formatValue` with the tempo in force, or by `text` (Cutoff's live frequency).
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
    layout: LabelLayout,
    text: Option<String>,
    bpm: f32,
) {
    let pid = id(i);
    let stepped = i == d::P_NOTE || i == d::P_SEQ_STEPS;
    // `attachSteppedDial`: the readout prints the option the knob rounds to.
    let raw = pv(host.ins, i);
    let text = text.unwrap_or_else(|| cfg::format_value(i, if stepped { raw.round() } else { raw }, bpm));
    let o = DialOpts { label, value_text: &text, default: family::default01(host, pid), small, layout, ..DialOpts::default() };
    family::dial_param(app, ui, host, &mut st.fam, &FAMILY, r, &PALETTE, pid, &o);
}

/// `SwitchRow`: name, Segmented, the option's description (textFaint, two lines).
fn switch_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, r: Rect, sw: &cfg::SwitchDef) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    let row = Rect::from_min_size(r.min, vec2(r.width(), ROW_H));
    p.text(pos2(row.left(), row.center().y), Align2::LEFT_CENTER, sw.label, font(LABEL_SIZE, false), pal.text_body);
    let desc_r = Rect::from_min_max(pos2(r.left() + LABEL_W + LABEL_GAP, row.bottom() + 4.0), r.max);
    fitted(&p, desc_r, switch_description(host, sw), CAPTION_SIZE, pal.text_faint);
    let seg = Rect::from_min_max(pos2(row.left() + LABEL_W + LABEL_GAP, row.top()), row.max);
    switch_seg(app, ui, host, st, seg, sw);
}

/// What the option of switch `sw` in force says about itself.
fn switch_description(host: &Host, sw: &cfg::SwitchDef) -> &'static str {
    let sel = (pv(host.ins, sw.param).round().max(0.0) as usize).min(choices(sw.param).len().saturating_sub(1));
    sw.descriptions.get(sel).copied().unwrap_or("")
}

/// `SwitchRow`'s Segmented in `seg`: a pick pushes Undo and is one undo step.
fn switch_seg(app: &mut SoundApp, ui: &mut Ui, host: &Host, st: &mut Editor, seg: Rect, sw: &cfg::SwitchDef) {
    let opts = choices(sw.param);
    let sel = (pv(host.ins, sw.param).round().max(0.0) as usize).min(opts.len().saturating_sub(1));
    if let Some(n) = segmented_ex(ui, seg, ("row", sw.param), &PALETTE, opts, 1, Some(sel), SelectionStyle::Filled, false) {
        family::set(app, host, &mut st.fam, &FAMILY, id(sw.param), n as f32);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn required_heights_follow_the_circuit() {
        assert_eq!(required_height(0), 522.0);
        assert_eq!(required_height(1), 522.0 + 13.0 + 50.0);
        assert_eq!(required_height(2), 522.0 + 13.0 + 2.0 * 50.0 + 6.0);
    }
}

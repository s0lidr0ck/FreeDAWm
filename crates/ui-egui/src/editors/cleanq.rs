//! CleanQ's VST editor (`CleanQ/Source/PluginEditor.cpp` and its components), translated: the
//! layout, the palette (`CleanQConfig.h`, the VST's own, in both DAW themes, as the VST has no
//! light skin), the paint code of `CleanQGraph.h`, `CleanQBandCard.h`, `CleanQGuideCard.h`,
//! `CleanQCeilingCard.h` and the glyphs of `CleanQIcons.h`, and every mouse and key gesture.
//! Its interaction inventory is `docs/superpowers/editors/cleanq.md`.
//!
//! Parameters go through `mix.module_param(s)`; the ceiling through `mix.module_ceiling` (shape,
//! undoable) and `mix.module_state` (the panel's settings and the guides, `uiState/ceiling` and
//! `uiState/guides`). Each band's live dynamic offset and the dynamic ceiling's cut come from the
//! plugin's live values (`cleanq::LIVE_*`).
//!
//! Parts: [`graph`] (`CleanQGraph`), [`band_card`] (`CleanQBandCard`), [`guide_card`]
//! (`CleanQGuideCard`), [`ceiling_strip`] (`CleanQCeilingCard`), [`amorph`] (Copy / Paste,
//! `CleanQAmorphEq.h`, and file drop).

mod amorph;
mod band_card;
mod ceiling_strip;
mod graph;
mod guide_card;
mod guide_info;
mod slim;

use crate::SoundApp;
use crate::cleanq_ceiling as cc;
use crate::editor_kit::controls::{Btn, Edge, ValueBoxState, ValueSpec, card_button, h_slider, ink_of, pref_w};
use crate::editor_kit::icons::{cross, tri_down, turn_arrow};
use crate::editor_kit::presets::{self, PresetState, RECALL_GESTURE};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, rgb};
use crate::editor_kit::{end_gesture, set_params, set_params_merged, set_state};
use crate::module_graphs::cq_param_id;
use crate::theme::{bold, regular};
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::Ceiling;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling_state::CeilingState;
use soundcraft_dsp::plugins::fs::cleanq::clean_q_guides::{Guide, Guides};
use soundcraft_dsp::plugins::fs::cleanq::{self as cq, clean_q_dsp as q};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};
use std::sync::Arc;

// ============================================================================ CleanQConfig.h

/// `kDefaultWidth` x `kDefaultHeight` (`setSize`).
pub const SIZE: Vec2 = vec2(1010.0, 560.0);

/// CleanQ's palette. CleanQ is not one of the Engine family (it keeps its Amorph build's look and
/// does not call `fs::look::apply`): what `CleanQConfig.h` sets, the theme's defaults elsewhere.
pub const PALETTE: VstPalette = VstPalette {
    accent: rgb(0x5ED1FF),
    background: rgb(0x101317),
    panel: rgb(0x12161B),
    border: rgb(0x2A313A),
    border_hover: rgb(0x3B4652),
    text_bright: rgb(0xEEF1F4),
    text_body: rgb(0xC9D1D9),
    text_dim: rgb(0x8FA1B3),
    text_faint: rgb(0x6F7A86),
    on_accent: rgb(0x08121A),
    bypass_red: rgb(0xFF6B5E),
    warn: rgb(0xFFD166),
    field: rgb(0x0E1115),
    well: rgb(0x0B0E12),
    ..VstPalette::THEME_DEFAULTS
};

/// The editor's ground (`background`).
pub const BACKGROUND: Color32 = PALETTE.background;
const ACCENT: Color32 = PALETTE.accent;
const BORDER: Color32 = PALETTE.border;
const TEXT_BRIGHT: Color32 = PALETTE.text_bright;
const TEXT_DIM: Color32 = PALETTE.text_dim;
const TEXT_FAINT: Color32 = PALETTE.text_faint;
const WARN: Color32 = PALETTE.warn;
const TOP_HI: Color32 = rgb(0x1C2128);
const TOP_LO: Color32 = rgb(0x161A1F);
const PANEL: Color32 = PALETTE.panel;
/// The cards' ground.
const CARD_BG: Color32 = rgb(0x14181D);
/// The dynamic ceiling's cut (`paintDynCut`).
pub const CUT_COLOUR: Color32 = argb(0xFFE06CF0);

fn band_colour(slot: usize) -> Color32 {
    const C: [u32; 8] = [0x5ED1FF, 0xFF7A59, 0x8DFF8A, 0xFFD166, 0xD38CFF, 0xFF6FB1, 0x5EFFD4, 0xFFA94D];
    rgb(C[slot % 8])
}
fn place_colour(place: i32) -> Color32 {
    const C: [u32; 5] = [0xF3F6F8, 0x5EA8FF, 0xFF6B6B, 0x7BE0B0, 0xC9A2FF];
    rgb(C[place.clamp(0, 4) as usize])
}
fn guide_colour(i: usize) -> Color32 {
    rgb(Guides::colour_for(i))
}
use crate::editor_kit::palette::al;

const FREQ_MIN: f64 = 10.0;
const FREQ_MAX: f64 = 30_000.0;
fn freq_norm(f: f64) -> f64 {
    let f = if f.is_finite() { f.clamp(FREQ_MIN, FREQ_MAX) } else { 1000.0 };
    (f / FREQ_MIN).ln() / (FREQ_MAX / FREQ_MIN).ln()
}
fn norm_freq(x: f64) -> f64 {
    FREQ_MIN * (FREQ_MAX / FREQ_MIN).powf(if x.is_finite() { x.clamp(0.0, 1.0) } else { 0.5 })
}
fn db_step(range: i32) -> i32 {
    if range <= 6 {
        1
    } else if range <= 18 {
        3
    } else {
        6
    }
}
fn format_freq(hz: f64) -> String {
    if hz >= 10_000.0 {
        format!("{:.1} kHz", hz / 1000.0)
    } else if hz >= 1000.0 {
        format!("{:.2} kHz", hz / 1000.0)
    } else if hz >= 100.0 {
        format!("{} Hz", hz.round() as i64)
    } else {
        format!("{hz:.1} Hz")
    }
}
fn format_gain(db: f64) -> String {
    if db.abs() < 0.05 { "0.0 dB".into() } else { format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" }) }
}
fn note_name(hz: f64) -> String {
    const N: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
    if hz <= 0.0 || !hz.is_finite() {
        return String::new();
    }
    let n = 12.0 * (hz / 440.0).log2() + 69.0;
    let r = n.round() as i32;
    let cents = ((n - f64::from(r)) * 100.0).round() as i32;
    // JUCE's `r / 12 - 1` truncates toward zero.
    let mut s = format!("{}{}", N[r.rem_euclid(12) as usize], r / 12 - 1);
    if cents != 0 {
        s += &format!("{}{cents}c", if cents > 0 { " +" } else { " " });
    }
    s
}

// ============================================================================ the band view
#[derive(Clone, Copy, Default, Debug)]
struct BandV {
    slot: usize,
    on: i32,
    kind: i32,
    place: i32,
    slope: i32,
    dyn_: bool,
    freq: f64,
    gain: f64,
    q: f64,
    /// The live dynamic offset (dB) the detector is applying now (0 when not playing).
    dyn_off: f64,
}
impl BandV {
    fn used(&self) -> bool {
        self.on >= q::ON_BYPASSED
    }
    fn active(&self) -> bool {
        self.on == q::ON_ACTIVE
    }
    fn has_gain(&self) -> bool {
        self.kind <= q::TYPE_HIGH_SHELF
    }
    /// `gainNow`: the gain the band is applying right now, its dynamic offset included. The
    /// live offset is taken to 0.05 dB (a display step) so a moving dynamic band's curve is not
    /// recomputed for changes nobody can see.
    fn gain_now(&self) -> f64 {
        self.gain + if self.dyn_ { (self.dyn_off * 20.0).round() / 20.0 } else { 0.0 }
    }
}

fn pv(ins: &Insert, id: &str) -> f32 {
    cq::CLEANQ_PARAMS.iter().find(|p| p.id == id).map_or(0.0, |p| p.clamp(ins.params.get(id).copied().unwrap_or(p.default)))
}
fn bp(slot: usize, field: usize) -> &'static str {
    cq_param_id(slot, field)
}
fn bands_of(ins: &Insert, live: Option<&soundcraft_dsp::analysis::LiveValues>) -> [BandV; q::NUM_BANDS] {
    std::array::from_fn(|s| {
        let g = |f: usize| pv(ins, bp(s, f));
        let off = live.map_or(0.0, |l| l.get(cq::LIVE_DYN.start + s));
        BandV {
            slot: s,
            on: g(cq::F_ON).round() as i32,
            kind: g(cq::F_TYPE).round() as i32,
            place: g(cq::F_PLACE).round() as i32,
            slope: g(cq::F_SLOPE).round() as i32,
            dyn_: g(cq::F_DYN) > 0.5,
            freq: q::knob_to_freq(g(cq::F_FREQ)),
            gain: q::knob_to_gain_db(g(cq::F_GAIN)),
            q: q::knob_to_q(g(cq::F_Q)),
            dyn_off: if off.is_finite() { f64::from(off).clamp(-60.0, 60.0) } else { 0.0 },
        }
    })
}

// ============================================================================ value boxes
#[derive(Clone, Copy, PartialEq, Eq, Debug, Hash)]
enum Field {
    Freq,
    Gain,
    Q,
    Thr,
    Range,
    Att,
    Rel,
    GCentre,
    GFrom,
    GTo,
    GGain,
}

fn spec(f: Field) -> ValueSpec {
    let mk = |label, lo, hi, log, fine, decimals, fmt: fn(f64) -> String| ValueSpec {
        label,
        lo,
        hi,
        log,
        fine,
        decimals,
        unit: "",
        default: lo,
        fmt: Some(fmt),
    };
    let f60 = 2f64.powf(1.0 / 60.0);
    let f40 = 2f64.powf(1.0 / 40.0);
    match f {
        Field::Freq => mk("Freq", FREQ_MIN, 20_000.0, true, f60, 1, format_freq),
        Field::Gain => mk("Gain", -30.0, 30.0, false, 0.1, 1, format_gain),
        Field::Q => mk("Q", 0.025, 40.0, true, f40, 3, |v| if v < 1.0 { format!("{v:.3}") } else { format!("{v:.2}") }),
        Field::Thr => mk("Thresh", -60.0, 0.0, false, 0.25, 1, |v| format!("{v:.1} dB")),
        Field::Range => mk("Range", -30.0, 30.0, false, 0.1, 1, format_gain),
        Field::Att => mk("Attack", 0.1, 500.0, true, f40, 1, |v| if v < 10.0 { format!("{v:.1} ms") } else { format!("{} ms", v.round() as i64) }),
        Field::Rel => {
            mk(
                "Release",
                5.0,
                5000.0,
                true,
                f40,
                0,
                |v| if v >= 1000.0 { format!("{:.2} s", v / 1000.0) } else { format!("{} ms", v.round() as i64) },
            )
        }
        Field::GCentre => mk("Centre", FREQ_MIN, FREQ_MAX, true, f60, 1, format_freq),
        Field::GFrom => mk("From", FREQ_MIN, FREQ_MAX, true, f60, 1, format_freq),
        Field::GTo => mk("To", FREQ_MIN, FREQ_MAX, true, f60, 1, format_freq),
        Field::GGain => mk("Proposes", -30.0, 30.0, false, 0.1, 1, format_gain),
    }
}

// ============================================================================ editor state
#[derive(Clone, Debug)]
struct GuideEdit {
    index: Option<usize>,
    g: Guide,
    focus_note: bool,
}

#[derive(Clone, Copy, Debug)]
struct NodeDrag {
    slot: usize,
    start: Pos2,
    freq: f64,
    gain: f64,
    axis: u8,
    gain_only: bool,
    has_gain: bool,
}

#[derive(Clone, Copy, Default, Debug)]
struct Sweep {
    active: bool,
    q: f32,
}

/// One band's curve, cached by what it depends on (`key`).
#[derive(Clone, Default)]
struct BandCurve {
    key: u64,
    used: bool,
    db: Vec<f64>,
}

/// The per-band curves (one slot each, each recomputed only when its own inputs change) and
/// their sum.
#[derive(Clone, Default)]
struct Curves {
    per: Vec<BandCurve>,
    total: Vec<f64>,
}

/// CleanQ's editor: everything the C++ editor holds that is not a parameter or the module state.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    selected: Option<usize>,
    range: i32,
    analyser: bool,
    pre: bool,
    post: bool,
    hold: bool,
    held_pre: Vec<f32>,
    held_post: Vec<f32>,
    ceiling_open: bool,
    ceiling_node: Option<usize>,
    conform_result: String,
    guides_on: bool,
    placing: bool,
    guide_edit: Option<GuideEdit>,
    /// The Templates picker replaces the guides (else it adds to them).
    template_replace: bool,
    /// The guide chips as drawn, one per guide (`Rect::NOTHING` for one not drawn).
    chips: Vec<Rect>,
    /// Scout's guide chips for this track as drawn (read-only: presses on them are swallowed).
    scout_chips: Vec<Rect>,
    /// Scout's guides for this track this frame, and their columns (a press in one is swallowed).
    scout: Vec<(usize, Rect, Color32, String)>,
    /// Scratch, reused every frame: chip rows, a dynamic band's ring.
    row_ends: Vec<f32>,
    ring: Vec<Pos2>,
    ab_other: Option<Vec<(&'static str, f32)>>,
    on_b: bool,
    sweeps: [Sweep; 24],
    drag: Option<NodeDrag>,
    soloing: Option<(usize, f32)>,
    shortcut: Option<(i32, Pos2)>,
    wheel_at: f64,
    fields: ValueBoxState,
    ceil_drag: Option<(usize, Ceiling, Pos2)>,
    noise_drag: Option<usize>,
    noise_levels: Option<[f64; soundcraft_dsp::plugins::fs::cleanq::clean_q_ceiling::NUM_NOISES]>,
    awaiting_paste: bool,
    message: Option<(String, f64)>,
    file_hover: bool,
    last_press: Option<(f64, Pos2)>,
    curves: Curves,
    trace: Vec<f64>,
    presets: PresetState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
    /// The slim layout's Ceiling strip's height as last laid out.
    slim_ceil_h: f32,
}

/// What every part of the editor needs to know this frame.
struct Cx<'a> {
    host: &'a Host<'a>,
    bands: [BandV; q::NUM_BANDS],
    geo: Geo,
    graph: Rect,
    ceil: Arc<CeilingState>,
    live: Option<&'a soundcraft_dsp::analysis::LiveValues>,
}
impl Cx<'_> {
    fn ins(&self) -> &Insert {
        self.host.ins
    }
    fn sr(&self) -> f64 {
        self.host.sr
    }
    fn band(&self, slot: usize) -> Option<BandV> {
        self.bands.get(slot).copied().filter(BandV::used)
    }
    fn first_free(&self) -> Option<usize> {
        self.bands.iter().find(|b| b.on == q::ON_FREE).map(|b| b.slot)
    }
    fn guides(&self) -> &[Guide] {
        &self.ceil.guides.list
    }
    /// The dynamic ceiling is on and cutting (`dynCutting`).
    fn cutting(&self) -> bool {
        pv(self.ins(), "ceilOn") > 0.5 && self.live.is_some_and(|l| l.get(cq::LIVE_CUTTING) > 0.5)
    }
}

#[derive(Clone, Copy)]
struct Geo {
    plot: Rect,
    range: f64,
}
impl Geo {
    fn fx(&self, hz: f64) -> f32 {
        self.plot.left() + (freq_norm(hz) * f64::from(self.plot.width())) as f32
    }
    fn xf(&self, x: f32) -> f64 {
        norm_freq(f64::from(x - self.plot.left()) / f64::from(self.plot.width().max(1.0)))
    }
    fn gy(&self, db: f64) -> f32 {
        let t = (0.5 - db / (2.0 * self.range)).clamp(-0.2, 1.2);
        self.plot.top() + (t * f64::from(self.plot.height())) as f32
    }
    fn yg(&self, y: f32) -> f64 {
        let t = f64::from(y - self.plot.top()) / f64::from(self.plot.height().max(1.0));
        (0.5 - t) * 2.0 * self.range
    }
    fn sy(&self, db: f64) -> f32 {
        self.plot.bottom() - (((db + 96.0) / 96.0).clamp(0.0, 1.0) as f32) * self.plot.height()
    }
    fn ys(&self, y: f32) -> f64 {
        f64::from(self.plot.bottom() - y) / f64::from(self.plot.height().max(1.0)) * 96.0 - 96.0
    }
    fn node(&self, b: &BandV) -> Pos2 {
        pos2(self.fx(b.freq), self.gy(if b.has_gain() { b.gain } else { 0.0 }))
    }
}

// ============================================================================ layout constants
const PAD_X: f32 = 10.0;
const TOP_BAR_H: f32 = 40.0;
const BTN_H: f32 = 24.0;
const BTN_GAP: f32 = 6.0;
const SCALE_H: f32 = 14.0;
const EDGE_INSET: f32 = 9.0;
/// `BandCard::cardWidth`, `GuideCard::cardWidth`.
const CARD_W: f32 = 250.0;
const GUIDE_W: f32 = 258.0;
/// The guide templates' button.
const TEMPLATES_LABEL: &str = "Templates";
const CEIL_STRIP_H: f32 = 8.0 * 2.0 + 4.0 * 24.0 + 3.0 * 6.0;

/// Draws CleanQ's editor for `module` on `track`.
pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_cleanq";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { SIZE }
    }

    /// `keyPressed`: Delete / Backspace, D, S on the selected band, Esc ends guide placing, and
    /// Cmd+C / Cmd+V (Copy / Paste). Cmd+Z and friends stay the DAW's (its undo is the VST's).
    fn owns_key(key: egui::Key, m: egui::Modifiers) -> bool {
        use egui::Key;
        matches!(key, Key::D | Key::S | Key::Delete | Key::Backspace | Key::Escape) && !m.command && !m.ctrl && !m.alt
            || m.command && matches!(key, Key::C | Key::V)
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        editor_ui(self, app, ui, host);
    }
}

// ============================================================================ commands
fn params(app: &mut SoundApp, cx: &Cx, list: &[(&str, f32)]) {
    set_params(app, cx.host, list);
}
fn params_merged(app: &mut SoundApp, cx: &Cx, list: &[(&str, f32)], key: &str) {
    set_params_merged(app, cx.host, list, key);
}
fn write_state(app: &mut SoundApp, cx: &Cx, st: &CeilingState) {
    set_state(app, cx.host, &st.write());
}
fn guides_params(host: &Host, guides: &Guides) -> Value {
    json!({"track": host.track.0, "module": host.ins.id.0, "guides": guides.to_json()})
}
/// The guides changed: `mix.module_guides` (they live in the module state next to the ceiling,
/// `uiState/guides`), one undo step.
fn write_guides(app: &mut SoundApp, cx: &Cx, guides: Guides) {
    if let Err(e) = app.run("mix.module_guides", guides_params(cx.host, &guides)) {
        app.ui.status = e;
    }
}
/// A guide template (`mix.module_guides {template, replace}`): its guides added to the ones there
/// (skipping those already there) or replacing them, one undo step. Guides only; no band, no gain.
fn apply_template(app: &mut SoundApp, cx: &Cx, st: &mut Editor, id: &str, replace: bool) {
    let p = json!({"track": cx.host.track.0, "module": cx.ins().id.0, "template": id, "replace": replace});
    match app.run("mix.module_guides", p) {
        Ok(r) => {
            st.guides_on = true;
            st.guide_edit = None;
            let n = |k: &str| r.get(k).and_then(Value::as_u64).unwrap_or(0);
            let mut m = format!("{} guide{} placed", n("added"), if n("added") == 1 { "" } else { "s" });
            if n("skipped") > 0 {
                m += &format!(", {} already there", n("skipped"));
            }
            if n("full") > 0 {
                m += &format!(", {} left out (24 at most)", n("full"));
            }
            st.message = Some((m, 0.0));
        }
        Err(e) => app.ui.status = e,
    }
}

/// The Templates picker: Add or Replace, then a source per section.
fn templates_menu(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    use soundcraft_dsp::plugins::fs::cleanq::clean_q_guide_templates::TEMPLATES;
    ui.set_min_width(190.0);
    ui.label(egui::RichText::new("Place a source's typical problem areas").weak());
    if ui.add(egui::Button::selectable(!st.template_replace, "Add to the guides")).clicked() {
        st.template_replace = false;
    }
    if ui.add(egui::Button::selectable(st.template_replace, "Replace guides")).clicked() {
        st.template_replace = true;
    }
    let mut group = "";
    for t in TEMPLATES {
        if t.group != group {
            group = t.group;
            ui.separator();
            ui.label(egui::RichText::new(group).small().weak());
        }
        if ui.button(t.name).clicked() {
            let replace = st.template_replace;
            apply_template(app, cx, st, t.id, replace);
            ui.close();
        }
    }
}

/// [`write_guides`] merged into `gesture` (with the band it makes, a pasted EQ, a preset).
fn write_guides_merged(app: &mut SoundApp, host: &Host, guides: &Guides, gesture: &str) {
    if let Err(e) = app.engine.execute_merged("mix.module_guides", &guides_params(host, guides), &host.key(gesture)) {
        app.ui.status = e.to_string();
    }
}
fn ceiling_points(c: &Ceiling) -> Value {
    if c.pts.is_empty() { Value::Null } else { json!(c.pts.iter().map(|p| [p.hz, p.db]).collect::<Vec<_>>()) }
}
fn set_ceiling(app: &mut SoundApp, cx: &Cx, c: &Ceiling, merged: bool) {
    let p = json!({"track": cx.host.track.0, "module": cx.ins().id.0, "points": ceiling_points(c)});
    let r = if merged {
        app.engine.execute_merged("mix.module_ceiling", &p, &cx.host.key("ceiling")).map(|_| ()).map_err(|e| e.to_string())
    } else {
        app.run("mix.module_ceiling", p).map(|_| ())
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// `addBandAt` (+ `addBandOfTypeAt`): Q 1, Stereo, 12 dB/oct, active, selected.
fn add_band(app: &mut SoundApp, cx: &Cx, st: &mut Editor, kind: i32, hz: f64, gain: f64) -> Option<usize> {
    let slot = cx.first_free()?;
    let mut list = vec![
        (bp(slot, cq::F_FREQ), q::freq_to_knob(hz).clamp(0.0, 1.0)),
        (bp(slot, cq::F_GAIN), q::gain_to_knob(gain).clamp(0.0, 1.0)),
        (bp(slot, cq::F_Q), q::q_to_knob(1.0).clamp(0.0, 1.0)),
        (bp(slot, cq::F_TYPE), kind as f32),
        (bp(slot, cq::F_PLACE), q::PLACE_STEREO as f32),
        (bp(slot, cq::F_SLOPE), 1.0),
        (bp(slot, cq::F_DYN), 0.0),
        (bp(slot, cq::F_ON), q::ON_ACTIVE as f32),
    ];
    if kind == q::TYPE_LOW_CUT || kind == q::TYPE_HIGH_CUT {
        list[1].1 = q::gain_to_knob(0.0);
    }
    params(app, cx, &list);
    select(st, Some(slot));
    Some(slot)
}

fn select(st: &mut Editor, slot: Option<usize>) {
    st.selected = slot;
    if slot.is_some() {
        st.fields.cancel();
    }
}

fn remove_band(app: &mut SoundApp, cx: &Cx, st: &mut Editor, slot: usize) {
    params(app, cx, &[(bp(slot, cq::F_ON), q::ON_FREE as f32)]);
    if let Some(s) = st.sweeps.get_mut(slot) {
        *s = Sweep::default();
    }
    if st.selected == Some(slot) {
        select(st, None);
    }
}

fn toggle_dyn(app: &mut SoundApp, cx: &Cx, slot: usize) {
    let on = pv(cx.ins(), bp(slot, cq::F_DYN)) > 0.5;
    params(app, cx, &[(bp(slot, cq::F_DYN), if on { 0.0 } else { 1.0 })]);
}

fn toggle_solo(app: &mut SoundApp, cx: &Cx, slot: usize) {
    let cur = pv(cx.ins(), "solo").round() as usize;
    params(app, cx, &[("solo", if cur == slot + 1 { 0.0 } else { (slot + 1) as f32 })]);
}

/// `toggleSweep`: park a narrow +12 dB boost and solo it; again: turn it into a -6 dB cut.
fn toggle_sweep(app: &mut SoundApp, cx: &Cx, st: &mut Editor, slot: usize) {
    let Some(b) = cx.band(slot) else { return };
    if !b.has_gain() {
        return;
    }
    let Some(sw) = st.sweeps.get_mut(slot) else { return };
    if !sw.active {
        sw.active = true;
        sw.q = pv(cx.ins(), bp(slot, cq::F_Q));
        params(
            app,
            cx,
            &[
                (bp(slot, cq::F_GAIN), q::gain_to_knob(12.0).clamp(0.0, 1.0)),
                (bp(slot, cq::F_Q), q::q_to_knob(12.0).clamp(0.0, 1.0)),
                ("solo", (slot + 1) as f32),
            ],
        );
    } else {
        sw.active = false;
        let qv = q::knob_to_q(sw.q).max(6.0);
        params(
            app,
            cx,
            &[(bp(slot, cq::F_GAIN), q::gain_to_knob(-6.0).clamp(0.0, 1.0)), (bp(slot, cq::F_Q), q::q_to_knob(qv).clamp(0.0, 1.0)), ("solo", 0.0)],
        );
    }
}

/// The parameters a snapshot holds (A/B): everything but the engine bypass and the solo.
fn snapshot(ins: &Insert) -> Vec<(&'static str, f32)> {
    cq::CLEANQ_PARAMS.iter().filter(|p| p.id != "bypass" && p.id != "solo").map(|p| (p.id, pv(ins, p.id))).collect()
}

/// `guidesAsVar`: the block a preset carries, or nothing when there are no guides.
fn guides_as_var(g: &Guides) -> Option<Value> {
    (!g.is_empty()).then(|| json!({"guides": g.to_json()}))
}

// ============================================================================ the editor
fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    let ins = host.ins;
    let track = host.track;
    let module = ins.id;
    if !st.init {
        st.init = true;
        st.range = 18;
        st.analyser = true;
        st.post = true;
        st.guides_on = true;
        st.presets.extra_key = "cleanq";
        st.presets.category = "All".into();
    }
    let streams = crate::chain_view::streams_for(ui, track, ins, host.meters);
    let readouts = host.meters.and_then(|m| m.modules.iter().find(|x| x.module == module)).map(crate::chain_view::live_values);
    let mut pending_paste = None;
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == track && *m == module) {
        if tab.trim_start().starts_with('{') {
            // `ui.mix.drawer {tab: <amorph-eq JSON>}`: Paste by command (scripts, Scout).
            pending_paste = Some(tab);
        } else if tab == "Ceiling" {
            st.ceiling_open = true;
            select(st, None);
        } else if tab == "More" && st.slim {
            st.slim_more = true;
        } else if tab == "Presets" {
            presets::open(app, host, &mut st.presets);
        } else if let Some(n) = tab.strip_prefix("Band ").and_then(|n| n.trim().parse::<usize>().ok()) {
            select(st, n.checked_sub(1));
        }
    }
    let ceil = cc::state_of(ui.ctx(), track, ins);

    // ---- layout (`resized`)
    let (bar, graph, strip) = if st.slim {
        slim::layout(st, ed)
    } else {
        let bar = Rect::from_min_size(ed.min, vec2(ed.width(), TOP_BAR_H));
        let mut body = Rect::from_min_max(pos2(ed.min.x, bar.bottom()), ed.max);
        let strip = st.ceiling_open.then(|| {
            let s = Rect::from_min_max(pos2(body.min.x, body.max.y - CEIL_STRIP_H), body.max);
            body.max.y = s.min.y;
            s
        });
        (bar, body, strip)
    };
    let plot = Rect::from_min_max(pos2(graph.min.x, graph.min.y + EDGE_INSET), pos2(graph.max.x, graph.max.y - SCALE_H - EDGE_INSET));
    let live = crate::editor_kit::live(host);
    let cx = Cx { host, bands: bands_of(ins, live), geo: Geo { plot, range: f64::from(st.range) }, graph, ceil, live };
    if st.selected.is_some_and(|s| cx.band(s).is_none()) && st.drag.is_none() {
        st.selected = None;
    }
    if let Some(t) = pending_paste {
        amorph::paste_text(app, &cx, st, &t);
    }
    let hovered = ui.rect_contains_pointer(ed);

    // ---- the preset browser covers the whole editor while it is open
    if st.presets.open {
        let p = ui.painter().clone();
        let n = cx.bands.iter().filter(|b| b.on != q::ON_FREE).count();
        st.presets.tags_out = format!("{n} {}", if n == 1 { "band" } else { "bands" });
        st.presets.extra_out = guides_as_var(&cx.ceil.guides);
        let out = presets::preset_browser(app, ui, host, ed, &PALETTE, &mut st.presets);
        apply_browser(app, &cx, st, ui, out);
        p.rect_stroke(ed, CornerRadius::ZERO, Stroke::new(1.0, BORDER), StrokeKind::Inside);
        st.message = None;
        return;
    }

    // ---- the analyser: Hold freezes it, and the capture with it (`refreshAnalyser`)
    if st.analyser && !st.hold {
        if let Some(s) = streams.as_deref() {
            hold_peaks(&mut st.held_pre, &s.pre);
            hold_peaks(&mut st.held_post, &s.post);
        }
        let analysis = host.meters.and_then(|m| m.analysis.iter().find(|(m, _)| *m == module)).map(|(_, a)| a);
        app.ui.cq_learn.entry((track, module)).or_default();
        cc::feed(app, ui, track, ins, analysis, host.sr as f32);
    }
    st.trace.clear();
    if let Some(l) = app.ui.cq_learn.get(&(track, module)) {
        st.trace.extend_from_slice(l.trace());
    }
    prune_generated(app, &cx);

    // ---- paint the frame and the top bar
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, BACKGROUND);
    if st.slim {
        slim::top_bar(app, ui, &cx, st, bar);
    } else {
        top_bar(app, ui, &cx, st, bar);
    }

    // ---- the graph
    graph::paint_graph(app, ui, &cx, st);
    graph::graph_mouse(app, ui, &cx, st);
    graph_edges(app, ui, &cx, st);
    if st.slim {
        slim::under_graph(app, ui, &cx, st, strip);
    } else {
        if let Some(slot) = st.selected
            && let Some(b) = cx.band(slot)
        {
            band_card::band_card(app, ui, &cx, st, b);
        }
        if st.guide_edit.is_some() {
            guide_card::guide_card(app, ui, &cx, st);
        }
        if let Some(s) = strip {
            ceiling_strip::ceiling_strip(app, ui, &cx, st, s, false);
        }
    }

    // ---- paintOverChildren
    overlays(ui, &cx, st, ed, readouts.map(|l| l.readouts[0]));
    p.rect_stroke(ed, CornerRadius::ZERO, Stroke::new(1.0, BORDER), StrokeKind::Inside);

    if hovered {
        keys(app, ui, &cx, st);
    }
    if st.awaiting_paste {
        let pasted = ui.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }));
        if let Some(t) = pasted {
            st.awaiting_paste = false;
            amorph::paste_text(app, &cx, st, &t);
        }
    }
    amorph::file_drop(app, ui, &cx, st, ed);
    // A wheel burst on a node is one undo step.
    let now = ui.input(|i| i.time);
    if st.wheel_at > 0.0 && now - st.wheel_at > 0.4 {
        st.wheel_at = 0.0;
        end_gesture(app);
    }
    if let Some((_, at)) = st.message.as_mut().filter(|(_, at)| *at == 0.0) {
        *at = now.max(1e-6);
    }
    if st.message.as_ref().is_some_and(|(_, at)| now - at > 2.5) {
        st.message = None;
    }
    if streams.is_some() || st.message.is_some() || live.is_some() {
        ui.ctx().request_repaint();
    }
}

/// What the preset browser asked: a recall's guides join its undo step (`loadPreset`: a preset
/// that carries none leaves yours alone), Copy and Paste.
fn apply_browser(app: &mut SoundApp, cx: &Cx, st: &mut Editor, ui: &Ui, out: presets::BrowserOut) {
    if let Some(extra) = out.recalled {
        if let Some(v) = extra {
            let guides = Guides::from_json(v.get("guides").unwrap_or(&Value::Null));
            write_guides_merged(app, cx.host, &guides, RECALL_GESTURE);
        }
        end_gesture(app);
        select(st, None);
        st.guide_edit = None;
    }
    if out.copy {
        amorph::copy_text(ui, cx, st);
        st.presets.message = st.message.take();
    }
    if out.paste {
        st.awaiting_paste = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
    if st.awaiting_paste {
        let pasted = ui.input(|i| i.events.iter().find_map(|e| if let egui::Event::Paste(t) = e { Some(t.clone()) } else { None }));
        if let Some(t) = pasted {
            st.awaiting_paste = false;
            amorph::paste_text(app, cx, st, &t);
            st.presets.message = st.message.take();
        }
    }
}

/// `held = v > held ? v : held + (v - held) * 0.35`, on the analyser's 0..1 scale.
fn hold_peaks(held: &mut Vec<f32>, db: &[f32]) {
    if db.is_empty() {
        held.clear();
        return;
    }
    held.resize(db.len(), 0.0);
    for (h, d) in held.iter_mut().zip(db) {
        let v = if d.is_finite() { ((d + 96.0) / 96.0).clamp(0.0, 1.0) } else { 0.0 };
        *h = if v > *h { v } else { *h + (v - *h) * 0.35 };
    }
}

/// `pruneGenerated`: a slot Conform wrote that has since been freed is no longer Conform's.
fn prune_generated(app: &mut SoundApp, cx: &Cx) {
    if cx.ceil.generated.iter().any(|s| cx.bands.get(*s as usize).is_none_or(|b| b.on == q::ON_FREE)) {
        let mut s = (*cx.ceil).clone();
        s.generated.retain(|g| cx.bands.get(*g as usize).is_some_and(|b| b.on != q::ON_FREE));
        // Bookkeeping, not an edit (the C++ pushes no undo step): the plain, non-undoable
        // `mix.module_state`, so undoing the band's removal brings the list back with it.
        let b64 = soundcraft_model::b64::encode(&s.write());
        if let Err(e) = app.run("mix.module_state", json!({"track": cx.host.track.0, "module": cx.ins().id.0, "state": b64})) {
            app.ui.status = e;
        }
    }
}

// ============================================================================ the top bar
fn top_bar(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, bar: Rect) {
    let p = ui.painter().clone();
    paint_bar(&p, bar);

    let row = Rect::from_min_max(
        pos2(bar.left() + PAD_X, bar.top() + (TOP_BAR_H - BTN_H) * 0.5),
        pos2(bar.right() - PAD_X, bar.bottom() - (TOP_BAR_H - BTN_H) * 0.5),
    );
    let mut x = row.left();
    let mut take = |w: f32| {
        let r = Rect::from_min_size(pos2(x, row.top()), vec2(w, BTN_H));
        x += w;
        r
    };

    let title = take(title_w(&p));
    paint_cq_title(&p, title);
    add_band_button(app, ui, cx, st, take(pref_w(ui, "+ Band", 18.0)));
    take(BTN_GAP);
    let (wp, wo, wh) = (pref_w(ui, "Pre", 16.0), pref_w(ui, "Post", 16.0), pref_w(ui, "Hold", 16.0));
    let analyser = [take(wp), take(wo), take(wh)];
    analyser_buttons(ui, st, analyser);
    take(BTN_GAP);
    let (ur, rr) = (take(26.0), take(26.0));
    undo_redo(app, ui, ur, rr);
    take(BTN_GAP);
    let (ab, abc) = (take(28.0), take(38.0));
    ab_buttons(app, ui, cx, st, ab, abc);

    // ---- the right-hand end, laid out backwards (the drawer's fold first: not the VST's)
    let mut right = row.right();
    let mut take_r = |w: f32| {
        right -= w;
        Rect::from_min_size(pos2(right, row.top()), vec2(w, BTN_H))
    };
    fold_button(app, ui, cx, take_r(20.0));
    take_r(BTN_GAP + 4.0);
    bypass_button(app, ui, cx, take_r(pref_w(ui, "Byp", 18.0)));
    take_r(BTN_GAP);
    invert_button(app, ui, cx, take_r(28.0));
    take_r(BTN_GAP);
    auto_button(app, ui, cx, take_r(pref_w(ui, "Auto", 18.0)));
    take_r(BTN_GAP);
    out_box(app, ui, cx, take_r(176.0));
    take_r(BTN_GAP);
    let (wc, wpa, wcl) = (pref_w(ui, "Copy", 18.0), pref_w(ui, "Paste", 18.0), pref_w(ui, "Clear", 18.0));
    let clear = take_r(wcl);
    let paste = take_r(wpa);
    let copy = take_r(wc);
    clipboard_buttons(app, ui, cx, st, [copy, paste, clear]);
    take_r(BTN_GAP);
    presets_button(app, ui, cx, st, take_r(pref_w(ui, PRESETS_LABEL, 18.0)));
}

// ---------------------------------------------------------------------------- its pieces
// Both layouts place these (the slim one folds the bar onto two rows and a More section).

/// The bar's ground: the gradient and its two edges.
fn paint_bar(p: &egui::Painter, bar: Rect) {
    crate::widgets::vgradient(p, bar, TOP_HI, TOP_LO);
    p.line_segment([pos2(bar.left(), bar.top() + 0.5), pos2(bar.right(), bar.top() + 0.5)], Stroke::new(1.0, rgb(0x262C34)));
    p.line_segment([pos2(bar.left(), bar.bottom() - 0.5), pos2(bar.right(), bar.bottom() - 0.5)], Stroke::new(1.0, rgb(0x06080A)));
}

/// The title's width.
fn title_w(p: &egui::Painter) -> f32 {
    p.layout_no_wrap("CLEANQ".into(), bold(13.0), TEXT_BRIGHT).size().x + 14.0
}

/// Title: "CLEAN" plain, "Q" accented.
fn paint_cq_title(p: &egui::Painter, title: Rect) {
    let g1 = p.layout_no_wrap("CLEAN".into(), bold(13.0), TEXT_BRIGHT);
    let w1 = g1.size().x;
    p.galley(pos2(title.left(), title.center().y - g1.size().y * 0.5), g1, TEXT_BRIGHT);
    p.text(pos2(title.left() + w1 + 1.0, title.center().y), Align2::LEFT_CENTER, "Q", bold(13.0), ACCENT);
}

/// "+ Band": a bell at 1 kHz, selected.
fn add_band_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, r: Rect) {
    if card_button(ui, r, "add", &PALETTE, Btn::new("+ Band"), Sense::click()).clicked() {
        add_band(app, cx, st, q::TYPE_BELL, 1000.0, 0.0);
    }
}

/// Pre | Post | Hold: what the analyser shows, and its freeze.
fn analyser_buttons(ui: &mut Ui, st: &mut Editor, r: [Rect; 3]) {
    let pal = &PALETTE;
    if card_button(ui, r[0], "pre", pal, Btn::new("Pre").on(st.pre).edge(Edge::Left), Sense::click()).clicked() {
        st.pre = !st.pre;
    }
    if card_button(ui, r[1], "post", pal, Btn::new("Post").on(st.post).edge(Edge::Middle), Sense::click()).clicked() {
        st.post = !st.post;
    }
    if card_button(ui, r[2], "hold", pal, Btn::new("Hold").on(st.hold).edge(Edge::Right), Sense::click()).clicked() {
        st.hold = !st.hold;
    }
}

/// Undo and Redo (the DAW's: the VST's undo is the session's).
fn undo_redo(app: &mut SoundApp, ui: &mut Ui, ur: Rect, rr: Rect) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    let u = card_button(ui, ur, "undo", pal, Btn::new("").edge(Edge::Left), Sense::click());
    turn_arrow(&p, ur, false, ink_of(pal, &u, false));
    if u.clicked() {
        let _ = app.run("edit.undo", json!({}));
    }
    let rd = card_button(ui, rr, "redo", pal, Btn::new("").edge(Edge::Right), Sense::click());
    turn_arrow(&p, rr, true, ink_of(pal, &rd, false));
    if rd.clicked() {
        let _ = app.run("edit.redo", json!({}));
    }
}

/// A / A->B: the editor's two snapshots (`toggleAb`, `copyAbToOther`).
fn ab_buttons(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, ab: Rect, abr: Rect) {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    if card_button(ui, ab, "ab", pal, Btn::new(if st.on_b { "B" } else { "A" }).on(true).edge(Edge::Left), Sense::click()).clicked() {
        let current = snapshot(cx.ins());
        if let Some(other) = st.ab_other.take() {
            params(app, cx, &other);
        }
        st.ab_other = Some(current);
        st.on_b = !st.on_b;
    }
    let abc = card_button(ui, abr, "abcopy", pal, Btn::new("").edge(Edge::Right), Sense::click());
    {
        let ink = ink_of(pal, &abc, false);
        let c = abr.center();
        p.text(c - vec2(9.0, 0.0), Align2::CENTER_CENTER, "A", regular(11.0), ink);
        p.text(c + vec2(9.0, 0.0), Align2::CENTER_CENTER, "B", regular(11.0), ink);
        p.line_segment([c - vec2(4.0, 0.0), c + vec2(3.5, 0.0)], Stroke::new(1.1, ink));
        p.add(Shape::convex_polygon(vec![c + vec2(4.5, 0.0), c + vec2(1.5, -2.5), c + vec2(1.5, 2.5)], ink, Stroke::NONE));
    }
    if abc.clicked() {
        st.ab_other = Some(snapshot(cx.ins()));
        st.message = Some(("A copied to the other slot".into(), 0.0));
    }
}

/// The drawer's fold (the DAW's, not the VST's).
fn fold_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, fold: Rect) {
    let fresp = ui.interact(fold, ui.id().with("cqw-fold"), Sense::click());
    fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Fold the drawer"));
    cross(ui.painter(), fold.center(), 4.0, if fresp.hovered() { TEXT_BRIGHT } else { TEXT_FAINT });
    if fresp.clicked() {
        app.ui.drawers.remove(&cx.host.track);
    }
}

/// Byp: the engine's bypass.
fn bypass_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, r: Rect) {
    let byp = pv(cx.ins(), "bypass") > 0.5;
    if card_button(ui, r, "byp", &PALETTE, Btn::new("Byp").on(byp).warn(), Sense::click()).clicked() {
        params(app, cx, &[("bypass", if byp { 0.0 } else { 1.0 })]);
    }
}

/// The polarity (a slashed O).
fn invert_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, r: Rect) {
    let inv = pv(cx.ins(), "invert") > 0.5;
    if card_button(ui, r, "inv", &PALETTE, Btn::new("\u{00D8}").on(inv).warn(), Sense::click()).clicked() {
        params(app, cx, &[("invert", if inv { 0.0 } else { 1.0 })]);
    }
}

/// Auto: the auto gain.
fn auto_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, r: Rect) {
    let auto = pv(cx.ins(), "autoGain") > 0.5;
    if card_button(ui, r, "auto", &PALETTE, Btn::new("Auto").on(auto), Sense::click()).clicked() {
        params(app, cx, &[("autoGain", if auto { 0.0 } else { 1.0 })]);
    }
}

/// The Out box: border, OUT, the slider (a drag is one undo step), the readout.
fn out_box(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, out_box: Rect) {
    let p = ui.painter().clone();
    p.rect_filled(out_box, CornerRadius::same(5), PANEL);
    p.rect_stroke(out_box, CornerRadius::same(5), Stroke::new(1.0, BORDER), StrokeKind::Inside);
    let inner = out_box.shrink2(vec2(6.0, 0.0));
    p.text(pos2(inner.left(), inner.center().y), Align2::LEFT_CENTER, "OUT", regular(10.0), TEXT_FAINT);
    let out_v = pv(cx.ins(), "outGain");
    let db = q::knob_to_out_db(out_v);
    let sl = Rect::from_min_max(pos2(inner.left() + 26.0, inner.center().y - 8.0), pos2(inner.right() - 50.0, inner.center().y + 8.0));
    let so = h_slider(ui, sl, "out", &PALETTE, ((db + 24.0) / 48.0) as f32, 0.5, false);
    let shown = so.prop.map_or(db, |pr| (f64::from(pr) * 48.0 - 24.0).clamp(-24.0, 24.0));
    p.text(pos2(inner.right(), inner.center().y), Align2::RIGHT_CENTER, format_gain((shown * 10.0).round() / 10.0), regular(11.0), TEXT_BRIGHT);
    if let Some(pr) = so.prop {
        let d = ((f64::from(pr) * 48.0 - 24.0) * 10.0).round() / 10.0;
        params_merged(app, cx, &[("outGain", q::out_db_to_knob(d).clamp(0.0, 1.0))], "out");
    }
    if so.ended {
        end_gesture(app);
    }
}

/// Copy | Paste | Clear (the Amorph-format clipboard, and every band freed).
fn clipboard_buttons(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, r: [Rect; 3]) {
    let pal = &PALETTE;
    // Laid out right to left in the full bar: Clear, Paste, Copy.
    let clear = card_button(ui, r[2], "clear", pal, Btn::new("Clear").edge(Edge::Right), Sense::click());
    let paste = card_button(ui, r[1], "paste", pal, Btn::new("Paste").edge(Edge::Middle), Sense::click());
    let copy = card_button(ui, r[0], "copy", pal, Btn::new("Copy").edge(Edge::Left), Sense::click());
    if clear.clicked() {
        let list: Vec<(&str, f32)> = cx.bands.iter().filter(|b| b.on != q::ON_FREE).map(|b| (bp(b.slot, cq::F_ON), q::ON_FREE as f32)).collect();
        params(app, cx, &list);
        st.sweeps = Default::default();
        select(st, None);
    }
    if copy.clicked() {
        amorph::copy_text(ui, cx, st);
    }
    if paste.clicked() {
        // The clipboard is read by the platform: it arrives as a Paste event.
        st.awaiting_paste = true;
        ui.ctx().send_viewport_cmd(egui::ViewportCommand::RequestPaste);
    }
}

/// The presets button's label (its triangle is drawn in the spaces).
const PRESETS_LABEL: &str = "Presets    ";

/// "Presets" with its triangle opens FireStation's preset browser over the editor.
fn presets_button(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor, pr: Rect) {
    let presp = card_button(ui, pr, "presets", &PALETTE, Btn::new(PRESETS_LABEL), Sense::click());
    tri_down(ui.painter(), pos2(pr.right() - 12.0, pr.center().y), ink_of(&PALETTE, &presp, false));
    if presp.clicked() {
        presets::open(app, cx.host, &mut st.presets);
    }
}

// ============================================================================ graph edge buttons
fn graph_edges(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    let pal = &PALETTE;
    let g = cx.graph;
    let h = 18.0;
    let p = ui.painter().clone();
    // Range, at the left.
    let rr = Rect::from_min_size(pos2(g.left() + 4.0, g.center().y - h * 0.5), vec2(42.0, h));
    let range_label = format!("\u{00B1}{}", st.range);
    let rresp = card_button(ui, rr, "range", pal, Btn::new(&range_label), Sense::click());
    egui::Popup::menu(&rresp).show(|ui| {
        for r in [6, 12, 18, 24, 30] {
            if ui.add(egui::Button::selectable(st.range == r, format!("\u{00B1}{r} dB"))).clicked() {
                st.range = r;
                ui.close();
            }
        }
    });
    // The analyser, at the right.
    let ar = Rect::from_min_size(pos2(g.right() - 54.0, g.center().y - h * 0.5), vec2(50.0, h));
    let aresp = card_button(ui, ar, "rta", pal, Btn::new("RTA   "), Sense::click());
    tri_down(&p, pos2(ar.right() - 11.0, ar.center().y), ink_of(pal, &aresp, false));
    egui::Popup::menu(&aresp).show(|ui| {
        if ui.add(egui::Button::selectable(st.analyser, "Analyser on")).clicked() {
            st.analyser = !st.analyser;
            ui.close();
        }
    });
    // Ceiling, centred along the top.
    let cw = pref_w(ui, "Ceiling", 18.0).max(60.0);
    let cr = Rect::from_min_size(pos2(g.center().x - cw * 0.5, g.top() + 8.0), vec2(cw, h));
    if card_button(ui, cr, "ceiling", pal, Btn::new("Ceiling").on(st.ceiling_open), Sense::click()).clicked() {
        st.ceiling_open = !st.ceiling_open;
        if st.ceiling_open {
            select(st, None);
            st.guide_edit = None;
        }
    }
    // Guides and its +, centred along the bottom, above the frequency scale.
    let guides = cx.guides();
    let left = guides.iter().filter(|g| !g.done).count();
    let glabel = if left > 0 { format!("Guides {left}") } else { "Guides".into() };
    let gw = pref_w(ui, &glabel, 16.0).max(56.0);
    let pw = 22.0;
    // Templates, beside them (not in the C++ editor).
    let tw = pref_w(ui, TEMPLATES_LABEL, 16.0) + 10.0;
    let gx = (g.center().x - (gw + pw + BTN_GAP + tw) * 0.5).round();
    let gy = g.bottom() - 14.0 - 6.0 - h;
    let gr = Rect::from_min_size(pos2(gx, gy), vec2(gw, h));
    let gresp = card_button(ui, gr, "guides", pal, Btn::new(&glabel).on(st.guides_on && !guides.is_empty()).edge(Edge::Left), Sense::click());
    if gresp.clicked() {
        st.guides_on = !st.guides_on;
        if !st.guides_on {
            st.guide_edit = None;
        }
    }
    let mut delete_all = false;
    egui::Popup::context_menu(&gresp).show(|ui| {
        let any = !guides.is_empty();
        if ui.button("Add a guide (then click where)").clicked() {
            if !st.placing {
                select(st, None);
                st.guide_edit = None;
                st.placing = true;
            }
            ui.close();
        }
        if ui.add_enabled(any, egui::Button::new(if st.guides_on { "Hide guides" } else { "Show guides" })).clicked() {
            st.guides_on = !st.guides_on;
            ui.close();
        }
        ui.separator();
        if ui.add_enabled(any, egui::Button::new("Delete all guides")).clicked() {
            delete_all = true;
            ui.close();
        }
    });
    if delete_all {
        write_guides(app, cx, Guides::default());
        st.guide_edit = None;
    }
    let plus = Rect::from_min_size(pos2(gx + gw, gy), vec2(pw, h));
    if card_button(ui, plus, "addguide", pal, Btn::new("+").on(st.placing).edge(Edge::Right), Sense::click()).clicked() {
        let on = !st.placing;
        if on {
            select(st, None);
            st.guide_edit = None;
        }
        st.placing = on;
    }
    let tr = Rect::from_min_size(pos2(plus.right() + BTN_GAP, gy), vec2(tw, h));
    let tresp = card_button(ui, tr, "guide-templates", pal, Btn::new(&format!("{TEMPLATES_LABEL}   ")), Sense::click());
    tri_down(&p, pos2(tr.right() - 10.0, tr.center().y), ink_of(pal, &tresp, false));
    egui::Popup::menu(&tresp).show(|ui| templates_menu(app, ui, cx, st));

    // The four corner shortcuts: click drops one at a default, drag puts it where you let go.
    let (cw, chh, inset) = (34.0, 26.0, 10.0);
    let corners = [
        (q::TYPE_LOW_CUT, pos2(g.left() + inset, g.top() + inset)),
        (q::TYPE_HIGH_CUT, pos2(g.right() - inset - cw, g.top() + inset)),
        (q::TYPE_LOW_SHELF, pos2(g.left() + inset, g.bottom() - inset - chh - 14.0)),
        (q::TYPE_HIGH_SHELF, pos2(g.right() - inset - cw, g.bottom() - inset - chh - 14.0)),
    ];
    for (kind, at) in corners {
        let r = Rect::from_min_size(at, vec2(cw, chh));
        let resp = card_button(ui, r, ("corner", kind), pal, Btn::new(""), Sense::click_and_drag());
        band_card::draw_type(&p, kind, r.shrink2(vec2(r.width() * 0.14, r.height() * 0.22)), ink_of(pal, &resp, false));
        let low = kind == q::TYPE_LOW_CUT || kind == q::TYPE_LOW_SHELF;
        if resp.dragged()
            && resp.total_drag_delta().is_some_and(|d| d.length() >= 4.0)
            && let Some(pp) = resp.interact_pointer_pos()
        {
            st.shortcut = Some((kind, pp));
        }
        if resp.drag_stopped() {
            if let Some((k, at)) = st.shortcut.take()
                && g.contains(at)
            {
                add_band(app, cx, st, k, cx.geo.xf(at.x), cx.geo.yg(at.y).clamp(-30.0, 30.0));
            }
        } else if resp.clicked() {
            add_band(app, cx, st, kind, if low { 80.0 } else { 8000.0 }, if low { -3.0 } else { 3.0 });
        }
    }
}

// ============================================================================ paintOverChildren
fn overlays(ui: &mut Ui, cx: &Cx, st: &mut Editor, ed: Rect, auto_db: Option<f32>) {
    let clip = ed.intersect(ui.clip_rect());
    let fg = ui
        .ctx()
        .layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new(("cqw-over", cx.host.track.0, cx.ins().id.0))))
        .with_clip_rect(clip);
    let ga = cx.graph;
    if st.file_hover {
        fg.rect_stroke(ed.shrink(3.0), CornerRadius::same(6), Stroke::new(2.0, al(ACCENT, 0.6)), StrokeKind::Middle);
        let b = Rect::from_center_size(ed.center(), vec2(160.0, 26.0));
        fg.rect_filled(b, CornerRadius::same(4), al(rgb(0x0D1013), 0.92));
        fg.text(b.center(), Align2::CENTER_CENTER, "Drop to load preset", regular(12.0), ACCENT);
    }
    if let Some((kind, p)) = st.shortcut {
        let over = ga.contains(p);
        let col = if over { band_colour(cx.first_free().unwrap_or(0)) } else { TEXT_FAINT };
        if over {
            fg.line_segment([pos2(p.x.round(), ga.top()), pos2(p.x.round(), ga.bottom())], Stroke::new(1.0, al(col, 0.35)));
            fg.text(pos2(p.x, p.y + 23.0), Align2::CENTER_CENTER, format_freq(cx.geo.xf(p.x)), regular(10.0), col);
        }
        let ghost = Rect::from_min_size(pos2(p.x - 17.0, p.y - 13.0), vec2(34.0, 26.0));
        fg.rect_filled(ghost, CornerRadius::same(5), al(CARD_BG, 0.92));
        fg.rect_stroke(ghost.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, al(col, if over { 0.9 } else { 0.4 })), StrokeKind::Middle);
        band_card::draw_type(&fg, kind, ghost.shrink2(vec2(5.0, 6.0)), al(col, if over { 1.0 } else { 0.5 }));
        if cx.first_free().is_none() {
            fg.text(pos2(p.x, p.y - 23.0), Align2::CENTER_CENTER, "all 24 bands in use", regular(10.0), WARN);
        }
    }
    if pv(cx.ins(), "autoGain") > 0.5 {
        let db = auto_db.filter(|v| v.is_finite()).unwrap_or(0.0);
        ui.painter().text(
            pos2(ga.left() + 52.0, ga.top() + 23.0),
            Align2::LEFT_CENTER,
            format!("auto {}", format_gain(f64::from(db))),
            regular(10.0),
            WARN,
        );
    }
    if let Some((m, _)) = &st.message {
        let g = fg.layout_no_wrap(m.clone(), regular(11.0), TEXT_BRIGHT);
        let b = Rect::from_center_size(pos2(ga.center().x, ga.top() + 44.0), g.size() + vec2(16.0, 8.0));
        fg.rect_filled(b, CornerRadius::same(4), al(rgb(0x0D1013), 0.92));
        fg.rect_stroke(b, CornerRadius::same(4), Stroke::new(1.0, BORDER), StrokeKind::Inside);
        fg.galley(b.min + vec2(8.0, 4.0), g, TEXT_BRIGHT);
    }
}

// ============================================================================ keyPressed
fn keys(app: &mut SoundApp, ui: &mut Ui, cx: &Cx, st: &mut Editor) {
    if ui.ctx().egui_wants_keyboard_input() {
        return;
    }
    let events: Vec<egui::Event> = ui.input(|i| i.events.clone());
    for e in events {
        match e {
            egui::Event::Copy => amorph::copy_text(ui, cx, st),
            egui::Event::Paste(t) if !st.awaiting_paste => amorph::paste_text(app, cx, st, &t),
            egui::Event::Key { key, pressed: true, modifiers: m, .. } => {
                if key == egui::Key::Escape && st.placing {
                    st.placing = false;
                    continue;
                }
                if m.alt || m.command || m.ctrl {
                    continue;
                }
                let Some(slot) = st.selected else { continue };
                match key {
                    egui::Key::Delete | egui::Key::Backspace => remove_band(app, cx, st, slot),
                    egui::Key::D => toggle_dyn(app, cx, slot),
                    egui::Key::S => toggle_sweep(app, cx, st, slot),
                    _ => {}
                }
            }
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests;

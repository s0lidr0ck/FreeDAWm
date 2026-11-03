//! Shale's VST editor (`Shale/Source/PluginEditor.cpp/.h`, `ShaleDisplay.h`), translated. Shale
//! keeps its own palette (a repair tool, not one of the Engine pedals): the theme's defaults with
//! its pale clay accent, Kit Link's blue, the same in both DAW themes.
//!
//! Layout and paint follow `resized()` / `paint()`: the header (title, the preset name, `<` `>`,
//! Undo, More, Bypass, the trigger lamp), the eleven mics in two rows, the display
//! ([`display`]), the five main dials, the behaviour row, the Kit Link strip (switches, status,
//! the phase row with Align Offset's box, Reject), the More drawer and the footer (the mic's note,
//! the live numbers). `setSize (940, max (640, requiredHeight()))`: 940 x 640, 940 x 896 with More.
//!
//! Kit Link is the DAW's (the Kit Link task): the status is `shale_ui::link_status` (who shares
//! this mic's link group and its group A-D, from the session), the phase row reads the module's
//! analysis report (`shale_ui::phase_status`) and Line Up is `shale_ui::line_up` (one undo step).
//! The timer's numbers are the engine's readouts: `[3]` dB out of the highs, `[4]` the trigger
//! count (the lamp), `[7]` the hits handed to another mic. Inventory:
//! `docs/superpowers/editors/shale.md`.

mod display;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, segmented_ex, segmented_id, tip};
use crate::editor_kit::family::{self, Family, FamilyState, preset_name};
use crate::editor_kit::look::{font, tracked_text, tracked_width};
use crate::editor_kit::palette::{VstPalette, al, argb, rgb};
use crate::editor_kit::{Host, VstEditor, consume_wheel, end_gesture, juce_wheel_delta_y, param, presets, readouts, set_params, set_params_merged};
use crate::shale_ui;
use egui::{Align2, Color32, CornerRadius, CursorIcon, Pos2, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::analysis::REPORT_LEN;
use soundcraft_dsp::plugins::fs::shale::{shale_align, shale_config as sc, shale_dsp as sd};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};
use std::collections::VecDeque;

const K_ACCENT: Color32 = rgb(0xC8A06A);
const K_BYPASS_RED: Color32 = rgb(0xFF6B5E);
const K_LINK_BLUE: Color32 = rgb(0x7FD4FF);
const K_PANEL_FILL: Color32 = rgb(0x12161A);
const K_PANEL_LINE: Color32 = rgb(0x1F252B);
const K_ROOT_BACK: Color32 = rgb(0x101316);

const K_PAD_X: f32 = 16.0;
const K_PAD_Y: f32 = 11.0;
const K_GAP: f32 = 9.0;
const K_HEADER_H: f32 = 25.0;
/// Two rows of mic buttons.
const K_MIC_H: f32 = 50.0;
const K_SCOPE_H: f32 = 190.0;
const K_DIAL_H: f32 = 118.0;
const K_ROW_H: f32 = 26.0;
/// Switches, status, and the phase row.
const K_LINK_H: f32 = 130.0;
const K_MORE_H: f32 = 250.0;
const K_FOOT_H: f32 = 22.0;
const K_DEFAULT_WIDTH: f32 = 940.0;
const K_DEFAULT_HEIGHT: f32 = 640.0;

/// The theme's defaults with Shale's accent, its label grey and its Bypass red.
pub const PALETTE: VstPalette =
    VstPalette { accent: K_ACCENT, text_label: argb(0xFF8A_96A3), bypass_red: K_BYPASS_RED, ..VstPalette::THEME_DEFAULTS };

/// `getPresetParams()`: everything but Bypass, Threshold, the gain staging, the listening tools,
/// the routing and this session's mic positions.
pub const PRESET_PARAMS: [&str; 19] = [
    "instrument",
    "sens",
    "ghost",
    "reduction",
    "length",
    "closeMode",
    "lookahead",
    "hold",
    "retrig",
    "debleed",
    "debleedMode",
    "xover",
    "velTrack",
    "autoBand",
    "detLow",
    "detHigh",
    "mix",
    "reject",
    "midiNote",
];

/// The preset facts: `storageKey`, and what `getPresetParams` leaves out besides Bypass.
const FAM: Family = Family::new(
    "Shale",
    &[
        "threshold",
        "trim",
        "output",
        "monitor",
        "scListen",
        "scSource",
        "link",
        "linkGroup",
        "linkRole",
        "midiOut",
        "phaseRef",
        "align",
        "alignOffset",
        "alignFlip",
        "alignTo",
    ],
);

/// The editor's own state.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    fam: FamilyState,
    /// `showMore`.
    pub more: bool,
    /// The display's span (`getDisplaySeconds`; 0: the default 5).
    seconds: f32,
    /// Freeze's copy of the bins, while frozen.
    frozen: Option<VecDeque<[f32; 3]>>,
    /// The threshold line is being dragged.
    dragging_line: bool,
    /// Counts last seen and when their flashes end (`flashTicks`, `deferFlashTicks`).
    last_trig: Option<f32>,
    trig_at: f64,
    last_defer: Option<f32>,
    defer_until: f64,
    /// Align Offset's box: typing (text), and the drag's start value (samples).
    offset_edit: Option<(String, bool)>,
    offset_drag: Option<f64>,
    cols: Vec<[f32; 3]>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open (the full editor's `more` is its own).
    slim_more: bool,
    /// Scripts: open the slim layout's Mic menu on the next frame.
    mic_open: bool,
}

impl std::fmt::Debug for Editor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("ShaleEditor").field("more", &self.more).finish()
    }
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

/// `requiredHeight()`: 637, or 896 with More.
pub fn required_height(more: bool) -> f32 {
    let h = K_PAD_Y
        + K_HEADER_H
        + K_GAP
        + K_MIC_H
        + K_GAP
        + K_SCOPE_H
        + K_GAP
        + K_DIAL_H
        + K_GAP
        + K_ROW_H
        + K_GAP
        + K_LINK_H
        + K_GAP
        + K_FOOT_H
        + K_PAD_Y;
    if more { h + K_MORE_H + K_GAP } else { h }
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_shale";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(K_DEFAULT_WIDTH, K_DEFAULT_HEIGHT.max(required_height(self.more))) }
    }

    /// `showMore = processor.isMorePanelOpen()`, and the display's `getDisplaySeconds`.
    fn opened(&mut self, ins: &Insert) {
        let ui_state = crate::editor_kit::ui_state(ins);
        self.more = ui_state.get("more").and_then(serde_json::Value::as_bool).unwrap_or(false);
        self.seconds = ui_state
            .get("displaySeconds")
            .and_then(serde_json::Value::as_f64)
            .map(|s| s as f32)
            .filter(|s| [2.5, 5.0, 10.0].contains(s))
            .unwrap_or(0.0);
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

// ---------------------------------------------------------------------------- resized()

/// `resized()`'s areas.
pub struct Layout {
    pub header: Rect,
    pub bypass: Rect,
    pub more: Rect,
    pub undo: Rect,
    pub next: Rect,
    pub prev: Rect,
    pub preset: Rect,
    pub mic: Rect,
    pub scope: Rect,
    /// Threshold, Onset, Length, Reduction, DeBleed.
    pub dials: [Rect; 5],
    pub ghost: Rect,
    pub close: Rect,
    pub debleed_mode: Rect,
    pub lookahead: Rect,
    pub monitor: Rect,
    pub link: Rect,
    pub reject: Rect,
    pub phase_ref: Rect,
    pub align: Rect,
    pub line_up: Rect,
    pub offset: Rect,
    pub flip: Rect,
    pub align_status: Rect,
    pub link_pill: Rect,
    pub group: Rect,
    pub role: Rect,
    pub align_to: Rect,
    pub link_status: Rect,
    /// The More drawer: its area, its ten dials (Hold, Retrigger, Vel Track, Mix, MIDI Note; Det
    /// Low, Det High, Crossover, Input, Output) and its row (Auto Band, Trigger From, SC Listen,
    /// MIDI Out).
    pub more_area: Option<Rect>,
    pub more_dials: [Rect; 10],
    pub more_row: [Rect; 4],
    pub foot: Rect,
}

fn split_left(r: &mut Rect, w: f32) -> Rect {
    let t = Rect::from_min_size(r.min, vec2(w.min(r.width()).max(0.0), r.height()));
    r.min.x = t.right();
    t
}

fn split_right(r: &mut Rect, w: f32) -> Rect {
    let w = w.min(r.width()).max(0.0);
    let t = Rect::from_min_max(pos2(r.right() - w, r.top()), r.max);
    r.max.x = t.left();
    t
}

fn split_top(r: &mut Rect, h: f32) -> Rect {
    let t = Rect::from_min_size(r.min, vec2(r.width(), h.min(r.height()).max(0.0)));
    r.min.y = t.bottom();
    t
}

fn split_bottom(r: &mut Rect, h: f32) -> Rect {
    let h = h.min(r.height()).max(0.0);
    let t = Rect::from_min_max(pos2(r.left(), r.bottom() - h), r.max);
    r.max.y = t.top();
    t
}

/// `n` columns of `w` (`row.getWidth() / n`, the last taking the rest).
fn fifths(row: Rect, n: usize) -> Vec<Rect> {
    let w = (row.width() / n as f32).floor();
    let mut r = row;
    (0..n).map(|i| if i + 1 == n { r } else { split_left(&mut r, w) }).collect()
}

pub fn layout(frame: Rect, more: bool) -> Layout {
    let mut r = frame.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let header = split_top(&mut r, K_HEADER_H);
    let mut h = header;
    let bypass = split_right(&mut h, 62.0);
    split_right(&mut h, 5.0);
    let more_r = split_right(&mut h, 52.0);
    split_right(&mut h, 5.0);
    let undo = split_right(&mut h, 52.0);
    split_right(&mut h, 12.0);
    let next = split_right(&mut h, 26.0);
    split_right(&mut h, 3.0);
    let prev = split_right(&mut h, 26.0);
    split_right(&mut h, 6.0);
    split_left(&mut h, 92.0);
    let preset = h.shrink2(vec2(0.0, 1.0));
    split_top(&mut r, K_GAP);
    let mic = split_top(&mut r, K_MIC_H);
    split_top(&mut r, K_GAP);
    let scope = split_top(&mut r, K_SCOPE_H);
    split_top(&mut r, K_GAP);
    let main = split_top(&mut r, K_DIAL_H);
    let d = fifths(main, 5);
    let dials = [d[0], d[1], d[2], d[3], d[4]];
    split_top(&mut r, K_GAP);
    let mut b = split_top(&mut r, K_ROW_H);
    let ghost = split_left(&mut b, 104.0);
    split_left(&mut b, K_GAP);
    let close = split_left(&mut b, 150.0);
    split_left(&mut b, K_GAP);
    let debleed_mode = split_left(&mut b, 140.0);
    split_left(&mut b, K_GAP);
    let lookahead = split_left(&mut b, 200.0);
    split_left(&mut b, K_GAP);
    let monitor = split_left(&mut b, 190.0);
    split_top(&mut r, K_GAP);
    let link = split_top(&mut r, K_LINK_H);
    let mut l = link.shrink2(vec2(10.0, 8.0));
    let reject = split_right(&mut l, 120.0);
    split_right(&mut l, K_GAP);
    let strip = l;
    let mut phase = split_bottom(&mut l, K_ROW_H);
    let phase_ref = split_left(&mut phase, 86.0);
    split_left(&mut phase, K_GAP);
    let align = split_left(&mut phase, 64.0);
    split_left(&mut phase, 6.0);
    let line_up = split_left(&mut phase, 74.0);
    split_left(&mut phase, 6.0);
    let offset = split_left(&mut phase, 132.0).shrink2(vec2(0.0, 2.0));
    split_left(&mut phase, 6.0);
    let flip = split_left(&mut phase, 54.0);
    split_left(&mut phase, K_GAP + 4.0);
    let align_status = phase;
    let mut top = split_top(&mut l, K_ROW_H);
    let link_pill = split_left(&mut top, 86.0);
    split_left(&mut top, K_GAP);
    let group = split_left(&mut top, 118.0);
    split_left(&mut top, K_GAP);
    let role = split_left(&mut top, 150.0);
    split_left(&mut top, K_GAP);
    let align_to = split_left(&mut top, 190.0);
    // paint(): the status sits between the switch row and the phase row
    let mut st = strip;
    split_top(&mut st, K_ROW_H + 6.0);
    split_bottom(&mut st, K_ROW_H + 4.0);
    let link_status = st;
    let mut more_dials = [Rect::NOTHING; 10];
    let mut more_row = [Rect::NOTHING; 4];
    let more_area = more.then(|| {
        split_top(&mut r, K_GAP);
        let area = split_top(&mut r, K_MORE_H);
        let mut m = area.shrink2(vec2(10.0, 8.0));
        let row1 = split_top(&mut m, 100.0);
        for (i, c) in fifths(row1, 5).into_iter().enumerate() {
            more_dials[i] = c;
        }
        split_top(&mut m, 6.0);
        let row2 = split_top(&mut m, 100.0);
        for (i, c) in fifths(row2, 5).into_iter().enumerate() {
            more_dials[5 + i] = c;
        }
        split_top(&mut m, 6.0);
        let mut row3 = split_top(&mut m, K_ROW_H);
        more_row[0] = split_left(&mut row3, 100.0);
        split_left(&mut row3, K_GAP);
        more_row[1] = split_left(&mut row3, 170.0);
        split_left(&mut row3, K_GAP);
        more_row[2] = split_left(&mut row3, 94.0);
        split_left(&mut row3, K_GAP);
        more_row[3] = split_left(&mut row3, 94.0);
        area
    });
    split_top(&mut r, K_GAP);
    let foot = r;
    Layout {
        header,
        bypass,
        more: more_r,
        undo,
        next,
        prev,
        preset,
        mic,
        scope,
        dials,
        ghost,
        close,
        debleed_mode,
        lookahead,
        monitor,
        link,
        reject,
        phase_ref,
        align,
        line_up,
        offset,
        flip,
        align_status,
        link_pill,
        group,
        role,
        align_to,
        link_status,
        more_area,
        more_dials,
        more_row,
        foot,
    }
}

// ---------------------------------------------------------------------------- helpers

/// `fitTracked`: the first sentence ("a full stop followed by a space"), cut with an ellipsis to
/// `width`.
fn fit_tracked(p: &egui::Painter, text: &str, width: f32, size: f32, tracking: f32) -> String {
    let mut s = match text.find(". ") {
        Some(stop) if stop > 0 => text[..=stop].to_string(),
        _ => text.to_string(),
    };
    if tracked_width(p, &s, size, tracking, false) <= width {
        return s;
    }
    while s.chars().count() > 4 && tracked_width(p, &format!("{s}\u{2026}"), size, tracking, false) > width {
        s.pop();
    }
    format!("{}\u{2026}", s.trim())
}

/// A Shale pill: `Pill::Small`, letter-spaced caps (Common's default).
fn look(active: Option<Color32>, dimmed: bool) -> PillLook {
    PillLook { active_colour: active, tracked: true, dimmed }
}

/// `setInactive`: drawn at 42 % and taking no clicks.
fn inactive_scope<R>(ui: &mut Ui, inactive: bool, body: impl FnOnce(&mut Ui) -> R) -> R {
    if inactive {
        ui.scope(|ui| {
            ui.multiply_opacity(0.42);
            body(ui)
        })
        .inner
    } else {
        body(ui)
    }
}

/// The value box's text (`AlignOffsetBox::paint`).
pub fn offset_text(samples: f64, sr: f64) -> String {
    let smp = samples.round() as i64;
    format!("{}{smp} smp   {:.2} ms", if smp > 0 { "+" } else { "" }, samples / sr * 1000.0)
}

/// `AlignOffsetBox::commit`: samples, or "1.2 ms"; `None` for text with no digits.
pub fn parse_offset(text: &str, sr: f64) -> Option<f64> {
    let t = text.trim().to_lowercase();
    if t.is_empty() || !t.chars().any(|c| c.is_ascii_digit()) {
        return None;
    }
    // `getDoubleValue`: the leading number.
    let num: String = t.trim_start().chars().take_while(|c| c.is_ascii_digit() || matches!(c, '.' | '-' | '+')).collect();
    let v = num.parse::<f64>().unwrap_or(0.0);
    Some(if t.contains("ms") { (v * 0.001 * sr).round() } else { v.round() })
}

// ---------------------------------------------------------------------------- the editor

/// `moreButton.onClick`: `showMore = ! showMore; processor.setMorePanelOpen (showMore)`.
fn toggle_more(st: &mut Editor, app: &mut SoundApp, host: &Host) {
    st.more = !st.more;
    crate::editor_kit::set_ui_state(app, host, &[("more", serde_json::json!(st.more))]);
}

/// The Kit Link panel's frame (blue-edged while Link is on) and the More drawer's.
fn panel(p: &egui::Painter, a: Rect, line: Color32) {
    p.rect_filled(a, CornerRadius::same(5), K_PANEL_FILL);
    p.rect_stroke(a.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, line), StrokeKind::Middle);
}

/// `attachSegmented` (letter-spaced): a pick pushes the editor's Undo and is one DAW step.
#[allow(clippy::too_many_arguments)]
fn seg(fam: &mut FamilyState, app: &mut SoundApp, ui: &mut Ui, host: &Host, rect: Rect, id: &str, rows: usize, help: &str) {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let sel = param(host, id).round().max(0.0) as usize;
    if let Some(i) = segmented_ex(ui, rect, id, &PALETTE, info.choices, rows, Some(sel), SelectionStyle::Filled, true) {
        set_choice(fam, app, host, id, info.clamp(i as f32));
    }
    tip(ui, segmented_id(ui, id), help);
}

/// A choice set from a pick: the editor's Undo, then one DAW step.
fn set_choice(fam: &mut FamilyState, app: &mut SoundApp, host: &Host, id: &str, v: f32) {
    family::push_undo(fam, host, &FAM);
    end_gesture(app);
    set_params(app, host, &[(id, v)]);
}

/// `attachPill`: a click flips toggle `id` (the editor's Undo, then one DAW step).
#[allow(clippy::too_many_arguments)]
fn pill(
    fam: &mut FamilyState,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    rect: Rect,
    id: &str,
    text: &str,
    active: Option<Color32>,
    dimmed: bool,
    help: &str,
) -> bool {
    let on = param(host, id) > 0.5;
    let resp = pill_ex(ui, rect, ("pill", id), &PALETTE, text, on, PillStyle::Small, look(active, dimmed));
    let clicked = resp.clicked();
    if clicked {
        set_choice(fam, app, host, id, if on { 0.0 } else { 1.0 });
    }
    let _ = resp.on_hover_text(help);
    clicked
}

/// `attachDial`: the editor's Undo on the drag start, one merged DAW step per gesture.
#[allow(clippy::too_many_arguments)]
fn knob(fam: &mut FamilyState, app: &mut SoundApp, ui: &mut Ui, host: &Host, rect: Rect, id: &str, label: &str, o: DialOpts, help: &str) {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let v = param(host, id);
    let text = shale_ui::value_text(host.ins, info, v);
    let o = DialOpts { label, value_text: &text, tracked: true, default: info.to_normalized(info.default), ..o };
    let out = dial(ui, rect, id, &PALETTE, info.to_normalized(v), &o);
    if out.started {
        end_gesture(app);
        family::push_undo(fam, host, &FAM);
    }
    if let Some(n) = out.value {
        set_params_merged(app, host, &[(id, info.clamp(info.from_normalized(n)))], &format!("dial-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
    let _ = out.response.on_hover_text(help);
}

const MIC_HELP: &str = "Which mic this is. It picks the band the detector listens in -- where this drum is loud and its neighbours are not -- and sensible starting points for everything else.";

/// The five main dials: (param, label, help).
const MAIN: [(&str, &str, &str); 5] = [
    (
        "threshold",
        "Threshold",
        "How loud a hit has to be to open the gate. Easiest set on the display: drag the orange line so it sits between the hits and the bleed.",
    ),
    (
        "sens",
        "Onset",
        "How sudden a hit has to be. Shale also checks for a sharp jump in level, so a cymbal that is loud but ringing steadily does not open it. Higher = only clear, sharp hits.",
    ),
    (
        "length",
        "Length",
        "How far the gate follows each hit down before closing, in dB below that hit's own peak. More negative = longer ring. A hard hit automatically rings longer than a soft one.",
    ),
    (
        "reduction",
        "Reduction",
        "How much quieter the drum gets while the gate is shut. Mute = silent; less keeps some of the room/bleed under it for a more natural sound.",
    ),
    (
        "debleed",
        "DeBleed",
        "Removes cymbal and hat bleed that gets through WHILE the gate is open. Real hits put lows and highs in at the same instant; bleed is highs on their own, and that is what gets ducked.",
    ),
];

/// The More drawer's ten dials: (param, label, locked while Auto Band is on, help).
const MORE_DIALS: [(&str, &str, bool, &str); 10] = [
    ("hold", "Hold", false, "The shortest time the gate stays open once it opens, however fast the hit decays."),
    ("retrig", "Retrigger", false, "The shortest gap before it can re-trigger, so one hit's rattle is not counted as two hits."),
    ("velTrack", "Vel Track", false, "Eases DeBleed on quiet hits, which have less top end of their own, so ghost notes are not gutted."),
    ("mix", "Mix", false, "Blend the gated drum with the original (parallel gating)."),
    ("midiNote", "MIDI Note", false, "The note sent for each hit when MIDI Out is on -- for triggering samples."),
    ("detLow", "Det Low", true, "Bottom of the band the detector listens in. Locked while Auto Band is on."),
    ("detHigh", "Det High", true, "Top of the band the detector listens in. Locked while Auto Band is on."),
    ("xover", "Crossover", true, "Where DeBleed's 'highs' start: above this frequency is what gets ducked."),
    ("trim", "Input", false, "Gain into the detector and the gate."),
    ("output", "Output", false, "Output level."),
];

/// What both layouts do first: the editor's Undo setup, the display span, the scripted drawer
/// tabs, and the preset browser, which covers the whole editor while it is open (then `true`).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) -> bool {
    st.fam.undo_cap = Some(32);
    st.fam.snapshot_all = true;
    st.fam.undo_on_load = true;
    st.fam.presets.quiet = true;
    ui.painter().rect_filled(host.rect, CornerRadius::ZERO, K_ROOT_BACK);
    let module_key = crate::module_graphs::module_key(host.track.0, host.ins.id.0);
    if st.seconds <= 0.0 {
        st.seconds = shale_ui::span(ui.ctx(), module_key);
    }
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.fam.presets),
            "More" if st.slim => st.slim_more = true,
            "More" => toggle_more(st, app, host),
            "Mic" if st.slim => st.mic_open = true,
            _ => {}
        }
    }
    if family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return true;
    }
    false
}

/// The timer's counts: the trigger lamp's and the hand-off flash's. Returns the readouts and the
/// hand-off count.
fn tick(st: &mut Editor, host: &Host, now: f64) -> (Option<[f32; 8]>, Option<f32>) {
    let ro = readouts(host);
    if let Some(t) = ro.map(|r| r[4]).filter(|t| t.is_finite()) {
        if st.last_trig.is_some_and(|l| l != t) {
            st.trig_at = now;
        }
        st.last_trig = Some(t);
    }
    let defers = ro.map(|r| r[7]).filter(|d| d.is_finite());
    if let Some(d) = defers {
        if st.last_defer.is_some_and(|l| l != d) {
            st.defer_until = now + 20.0 / 30.0;
        }
        st.last_defer = Some(d);
    }
    (ro, defers)
}

/// The title "SHALE" in `title` and the trigger lamp beside it.
fn title_ui(st: &Editor, p: &egui::Painter, title: Rect, now: f64) {
    tracked_text(
        p,
        Rect::from_min_size(title.min, vec2(90.0, title.height())),
        Align2::LEFT_CENTER,
        "SHALE",
        15.0,
        0.24,
        true,
        true,
        PALETTE.text_bright,
    );
    // The trigger lamp, on the header, so a hit is visible even when the scope is still.
    let flash = 1.0 - ((now - st.trig_at) / 0.2) as f32;
    if st.last_trig.is_some() && flash > 0.0 && now >= st.trig_at {
        let dot = Rect::from_min_size(pos2(title.left() + 74.0, title.center().y - 3.0), vec2(6.0, 6.0));
        p.circle_filled(dot.center(), 3.0, al(PALETTE.accent, flash.clamp(0.0, 1.0)));
    }
}

/// Where the header's controls go (each layout places them; the slim one has no More pill).
pub struct HeaderRects {
    pub preset: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub undo: Rect,
    pub more: Option<Rect>,
    pub bypass: Rect,
}

/// The header's controls: the preset name, `<` `>`, Undo, More, Bypass.
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, h: &HeaderRects) {
    let pal = &PALETTE;
    st.fam.presets.not_in_presets = FAM.not_in_presets;
    let dirty = presets::is_dirty(app, host, &mut st.fam.presets);
    let shown = presets::shown_name(host);
    if preset_name(ui, h.preset, pal, shown, dirty).clicked() {
        presets::open(app, host, &mut st.fam.presets);
    }
    let mut out = presets::BrowserOut::default();
    for (rect, text, dir) in [(h.prev, "<", -1), (h.next, ">", 1)] {
        if pill_ex(ui, rect, text, pal, text, false, PillStyle::Small, look(None, false)).clicked() {
            family::push_undo(&mut st.fam, host, &FAM);
            out = presets::step_all(app, host, &mut st.fam.presets, dir);
        }
    }
    let undo_r = pill_ex(ui, h.undo, "undo", pal, "Undo", false, PillStyle::Small, look(None, !st.fam.can_undo()));
    if undo_r.clicked() {
        family::do_undo(app, host, &mut st.fam);
    }
    let _ = undo_r.on_hover_text("Undo the last change.");
    if let Some(more) = h.more {
        let more_r = pill_ex(ui, more, "more", pal, "More", st.more, PillStyle::Small, look(None, false));
        if more_r.clicked() {
            toggle_more(st, app, host);
        }
        let _ = more_r.on_hover_text("Show the rest of the controls.");
    }
    let bypassed = param(host, "bypass") > 0.5;
    if pill_ex(ui, h.bypass, "bypass", pal, "Bypass", bypassed, PillStyle::Small, look(Some(K_BYPASS_RED), false)).clicked() {
        family::push_undo(&mut st.fam, host, &FAM);
        set_params(app, host, &[("bypass", if bypassed { 0.0 } else { 1.0 })]);
    }
    if out.recalled.is_some() {
        end_gesture(app);
    }
}

/// The display in `r`: the scope, its span and Freeze buttons, and the threshold line (the same
/// parameter as the knob).
fn display_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let module_key = crate::module_graphs::module_key(host.track.0, host.ins.id.0);
    let streams = crate::chain_view::streams_for(ui, host.track, host.ins, host.meters);
    let empty = VecDeque::new();
    let live_bins = streams.as_deref().map_or(&empty, |s| &s.shale);
    let thresh_db = sd::knob_to_thresh_db(param(host, "threshold"));
    let span_b = display::span_button(r);
    let freeze_b = display::freeze_button(r);
    let resp = ui.interact(r, ui.id().with("shale-display"), Sense::click_and_drag());
    let over_button = resp.hover_pos().is_some_and(|q| span_b.contains(q) || freeze_b.contains(q));
    if resp.hovered() || resp.dragged() {
        ui.ctx().set_cursor_icon(if over_button && !st.dragging_line { CursorIcon::PointingHand } else { CursorIcon::ResizeVertical });
    }
    let press = ui.input(|i| i.pointer.press_origin());
    if resp.is_pointer_button_down_on()
        && ui.input(|i| i.pointer.primary_pressed())
        && let Some(q) = press
    {
        if span_b.contains(q) {
            st.seconds = display::next_span(st.seconds);
            ui.ctx().data_mut(|d| d.insert_temp(module_key.with("shale-span"), st.seconds));
            // `setDisplaySeconds`: kept in the module's `uiState`.
            crate::editor_kit::set_ui_state(app, host, &[("displaySeconds", serde_json::json!(st.seconds))]);
        } else if freeze_b.contains(q) {
            st.frozen = match st.frozen.take() {
                Some(_) => None,
                None => Some(live_bins.clone()),
            };
        } else {
            st.dragging_line = true;
            end_gesture(app);
            family::push_undo(&mut st.fam, host, &FAM);
        }
    }
    if st.dragging_line
        && let Some(q) = resp.interact_pointer_pos().or(press)
        && let Some(v) = display::threshold_at(r, q.y)
        && (v - param(host, "threshold")).abs() > 1e-6
    {
        set_params_merged(app, host, &[("threshold", v)], "threshold-line");
    }
    if st.dragging_line && !ui.input(|i| i.pointer.primary_down()) {
        st.dragging_line = false;
        end_gesture(app);
    }
    let bins = st.frozen.as_ref().unwrap_or(live_bins);
    let view = display::View { bins, seconds: st.seconds, frozen: st.frozen.is_some(), thresh_db, dragging: st.dragging_line };
    display::paint(ui.painter(), r, &PALETTE, &view, &mut st.cols);
}

/// The five main dials in `rects`, in [`MAIN`]'s order.
fn main_dials(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, rects: &[Rect; 5]) {
    for (rect, (id, label, help)) in rects.iter().zip(MAIN) {
        knob(&mut st.fam, app, ui, host, *rect, id, label, DialOpts::default(), help);
    }
}

/// The behaviour row's controls: Ghost Notes, Close, DeBleed mode, Lookahead, Monitor.
fn behaviour_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: [Rect; 5]) {
    let fam = &mut st.fam;
    pill(
        fam,
        app,
        ui,
        host,
        r[0],
        "ghost",
        "Ghost Notes",
        None,
        false,
        "Catch quiet ghost notes. Lets a sharp enough hit open the gate even under the threshold, instead of lowering the threshold and letting the hats in too.",
    );
    seg(
        fam,
        app,
        ui,
        host,
        r[1],
        "closeMode",
        1,
        "How the gate shuts. Decay Match follows the drum's own natural decay (no suck, no bleed tail). Fixed is a plain time -- for sample-replaced drums.",
    );
    seg(
        fam,
        app,
        ui,
        host,
        r[2],
        "debleedMode",
        1,
        "Shelf: ducks everything up top by a set amount. Learned: listens to the bleed while the gate is shut and removes just that -- keeps more of the drum's own sizzle. Give Learned a few bars of playing first.",
    );
    seg(
        fam,
        app,
        ui,
        host,
        r[3],
        "lookahead",
        1,
        "Lets the gate open slightly BEFORE the hit, so the very front of the attack is never clipped. Adds that much latency, which the host compensates.",
    );
    seg(
        fam,
        app,
        ui,
        host,
        r[4],
        "monitor",
        1,
        "What you hear. Output: the gated drum. Bleed: only what is being removed -- if you can hear the drum in it, back off. Detector: what the detector is listening to.",
    );
}

/// Where Kit Link's switch row goes.
pub struct LinkRects {
    pub link_pill: Rect,
    pub group: Rect,
    pub role: Rect,
    pub align_to: Rect,
    pub reject: Rect,
}

/// Kit Link's switch row: Kit Link, the group, the role, Align To (a menu of the twelve names),
/// and Reject.
fn link_switches(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: &LinkRects) {
    let pal = &PALETTE;
    let link_on = param(host, "link") > 0.5;
    pill(
        &mut st.fam,
        app,
        ui,
        host,
        r.link_pill,
        "link",
        "Kit Link",
        Some(K_LINK_BLUE),
        false,
        "Kit Link: Shale instances on different mics talk to each other. When another mic heard the same hit louder, this one knows the hit was not its drum and stays shut.",
    );
    inactive_scope(ui, !link_on, |ui| {
        if link_on {
            seg(
                &mut st.fam,
                app,
                ui,
                host,
                r.group,
                "linkGroup",
                1,
                "Which kit this mic belongs to. Only instances in the same group (A-D) listen to each other.",
            );
            seg(
                &mut st.fam,
                app,
                ui,
                host,
                r.role,
                "linkRole",
                1,
                "Lead: decides for itself and tells the others. Follow: opens when the Lead opens (e.g. a room mic following the snare). Pair: two mics that must open together, like snare top and bottom.",
            );
        } else {
            // inactive: drawn, no clicks
            for (rect, id) in [(r.group, "linkGroup"), (r.role, "linkRole")] {
                let opts = host.info.params.iter().find(|p| p.id == id).map_or(&[][..], |p| p.choices);
                let _ = segmented_ex(
                    ui,
                    rect,
                    ("inactive", id),
                    pal,
                    opts,
                    1,
                    Some(param(host, id).round().max(0.0) as usize),
                    SelectionStyle::Filled,
                    true,
                );
            }
        }
    });
    // Align To: a menu of the twelve names
    let align_to = param(host, "alignTo").round().max(0.0) as usize;
    let at_text = format!("Align To: {}", sc::ALIGN_TOS.get(align_to).copied().unwrap_or("?"));
    let at_resp = inactive_scope(ui, !link_on, |ui| {
        pill_ex(ui, r.align_to, "alignTo", pal, &at_text, align_to > 0, PillStyle::Small, look(Some(K_LINK_BLUE), false))
    });
    if link_on {
        let mut chosen = None;
        egui::Popup::menu(&at_resp).show(|ui| {
            for (i, name) in sc::ALIGN_TOS.iter().enumerate() {
                if i == 1 {
                    ui.separator();
                }
                let label = if i == 0 { "Group Ref (the mic with Phase Ref on, in this group)" } else { name };
                if ui.add(egui::Button::selectable(i == align_to, label)).clicked() {
                    chosen = Some(i);
                    ui.close();
                }
            }
        });
        if let Some(i) = chosen {
            set_choice(&mut st.fam, app, host, "alignTo", i as f32);
        }
        let _ = at_resp.on_hover_text("Which mic this one lines up to. Group Ref: the mic with Phase Ref on in this group. Or pick a mic by name, in any group -- Kick Out to Kick In, Snare Btm and Overheads to Snare Top -- so each pair gets its own reference without splitting the kit. That mic needs Kit Link on.");
    }
    knob(
        &mut st.fam,
        app,
        ui,
        host,
        r.reject,
        "reject",
        "Reject",
        DialOpts { small: true, inactive: !link_on, ..DialOpts::default() },
        "How much louder another mic must hear a hit before this one decides it was bleed. 0 = never defer.",
    );
}

/// The link status line (`shale_ui::link_status`) and whether it is the "unavailable" warning.
fn link_status(host: &Host, report: Option<[f32; REPORT_LEN]>) -> (String, bool) {
    use soundcraft_dsp::plugins::fs::shale as s;
    let no_slot = report.is_some_and(|r| r[s::REPORT_MARK] == s::REPORT_VALID && r[s::REPORT_NO_SLOT] > 0.5);
    let (status, _warn) = shale_ui::link_status(&host.session, host.track, host.ins, no_slot);
    let unavailable = status.starts_with("Kit Link unavailable");
    (status, unavailable)
}

/// Where the phase row's controls go.
pub struct PhaseRects {
    pub phase_ref: Rect,
    pub align: Rect,
    pub line_up: Rect,
    pub offset: Rect,
    pub flip: Rect,
}

/// The phase row: Phase Ref, and where this mic measures, Align, Line Up, Align Offset's box and
/// Flip. Returns the phase status and whether it is a fresh measurement.
fn phase_row(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, report: Option<[f32; REPORT_LEN]>, r: &PhaseRects) -> (String, bool) {
    let pal = &PALETTE;
    let ins = host.ins;
    let link_on = param(host, "link") > 0.5;
    let sr = host.sr;
    let is_ref = param(host, "phaseRef") > 0.5;
    inactive_scope(ui, !link_on, |ui| {
        if link_on {
            pill(
                &mut st.fam,
                app,
                ui,
                host,
                r.phase_ref,
                "phaseRef",
                "Phase Ref",
                Some(K_LINK_BLUE),
                false,
                "Make this mic the phase reference for its group -- normally the overheads. Every other mic in the group measures how far it is from this one.",
            );
        } else {
            let _ = pill_ex(ui, r.phase_ref, ("inactive", "phaseRef"), pal, "Phase Ref", is_ref, PillStyle::Small, look(Some(K_LINK_BLUE), false));
        }
    });
    let measures = shale_ui::measures(ins);
    let align_on = param(host, "align") > 0.5;
    let (phase_text, can_line_up) = shale_ui::phase_status(ins, report, sr);
    if measures {
        pill(
            &mut st.fam,
            app,
            ui,
            host,
            r.align,
            "align",
            "Align",
            Some(K_LINK_BLUE),
            false,
            "Delay (or advance) this mic by Align Offset, and flip it if Align Polarity says so. Adds 10 ms of latency while on, which the host compensates for.",
        );
        // Line Up: enabled with four agreeing hits measured; 45 % otherwise.
        let lu = ui.scope(|ui| {
            if !can_line_up {
                ui.multiply_opacity(0.45);
            }
            pill_ex(ui, r.line_up, "lineUp", pal, "Line Up", false, PillStyle::Small, look(None, false))
        });
        if lu.inner.clicked()
            && can_line_up
            && let Some(rep) = report
        {
            family::push_undo(&mut st.fam, host, &FAM);
            if let Err(e) = shale_ui::line_up(&mut app.engine, host.track, ins.id, rep) {
                app.ui.status = e;
            }
        }
        let _ =
            lu.inner.on_hover_text("Take the measured offset and polarity, and switch Align on. Play a few hits first: it needs four that agree.");
        offset_box(st, app, ui, host, r.offset, align_on, sr);
        // Flip: switching it also switches Align on.
        let flip_on = param(host, "alignFlip") > 0.5;
        let fr = pill_ex(ui, r.flip, ("pill", "alignFlip"), pal, "Flip", flip_on, PillStyle::Small, look(Some(K_LINK_BLUE), !align_on));
        if fr.clicked() {
            family::push_undo(&mut st.fam, host, &FAM);
            end_gesture(app);
            let mut list = vec![("alignFlip", if flip_on { 0.0 } else { 1.0 })];
            if !align_on {
                list.push(("align", 1.0));
            }
            set_params(app, host, &list);
        }
        let _ = fr.on_hover_text("Align Polarity: flip this mic upside down. Line Up sets it from the measurement.");
    }
    use soundcraft_dsp::plugins::fs::shale as s;
    let measured = report.is_some_and(|r| r[s::REPORT_STATE] as i32 == s::AL_MEASURED && r[s::REPORT_MARK] == s::REPORT_VALID);
    (phase_text, measured)
}

/// The More drawer's controls: the ten dials and Auto Band, Trigger From, SC Listen, MIDI Out.
/// `narrow` (the slim layout): a dial Auto Band locks is drawn inactive, without the "· LINKED"
/// its label carries in the full editor (the layout says it once instead).
#[allow(clippy::too_many_arguments)]
fn more_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, dials: &[Rect; 10], row: &[Rect; 4], narrow: bool) {
    let auto_band = param(host, "autoBand") > 0.5;
    for (rect, (id, label, lockable, help)) in dials.iter().zip(MORE_DIALS) {
        let held = lockable && auto_band;
        let o = DialOpts { small: true, locked: held && !narrow, inactive: held && narrow, ..DialOpts::default() };
        knob(&mut st.fam, app, ui, host, *rect, id, label, o, help);
    }
    let fam = &mut st.fam;
    pill(
        fam,
        app,
        ui,
        host,
        row[0],
        "autoBand",
        "Auto Band",
        None,
        false,
        "Let the Mic setting choose the detector band and the DeBleed crossover. Turn off to set them yourself.",
    );
    seg(fam, app, ui, host, row[1], "scSource", 1, "What the detector listens to: this track, or the host's sidechain input.");
    pill(
        fam,
        app,
        ui,
        host,
        row[2],
        "scListen",
        "SC Listen",
        None,
        false,
        "Hear the detector's signal instead of the drum, to check what it is keying from.",
    );
    pill(fam, app, ui, host, row[3], "midiOut", "MIDI Out", None, false, "Send a MIDI note on every detected hit, for triggering samples.");
}

/// The footer's live numbers ("x dB out of the highs - n handed to another mic"), and whether a
/// hand-off is flashing.
fn live_text(st: &Editor, host: &Host, ro: Option<[f32; 8]>, defers: Option<f32>, now: f64) -> (String, bool) {
    let mut live = String::new();
    let cut = ro.map_or(f32::NAN, |r| r[3]);
    if param(host, "debleed") > 0.0005 && cut.is_finite() && cut > 0.2 {
        live.push_str(&format!("{cut:.1} dB out of the highs"));
    }
    if let Some(d) = defers.filter(|d| *d > 0.0) {
        if !live.is_empty() {
            live.push_str("   -   ");
        }
        live.push_str(&format!("{} handed to another mic", d as i64));
    }
    (live, now < st.defer_until)
}

/// The timer: the scope, the lamp and the link strip (30 Hz).
fn timer(st: &Editor, ui: &Ui, ro: Option<[f32; 8]>) {
    if ro.is_some() || st.fam.presets.message.is_some() || st.dragging_line {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
    }
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let p = ui.painter().clone();
    let ins = host.ins;
    let l = layout(host.rect, st.more);
    let report: Option<[f32; REPORT_LEN]> = host.meters.and_then(|m| m.analysis.iter().find(|(id, _)| *id == ins.id)).map(|(_, a)| a.report());
    let now = ui.input(|i| i.time);
    let (ro, defers) = tick(st, host, now);
    let link_on = param(host, "link") > 0.5;

    // ---- paint(): the title, the link and More panels, the lamp
    title_ui(st, &p, l.header, now);
    panel(&p, l.link, if link_on { al(K_LINK_BLUE, 0.35) } else { K_PANEL_LINE });
    if let Some(m) = l.more_area {
        panel(&p, m, K_PANEL_LINE);
    }

    // ---- header
    let h = HeaderRects { preset: l.preset, prev: l.prev, next: l.next, undo: l.undo, more: Some(l.more), bypass: l.bypass };
    header_controls(st, app, ui, host, &h);

    // ---- the mic: eleven of them, two rows
    seg(&mut st.fam, app, ui, host, l.mic, "instrument", 2, MIC_HELP);

    // ---- the display: the threshold line and the knob are the same parameter
    display_ui(st, app, ui, host, l.scope);

    // ---- the four that matter (and DeBleed)
    main_dials(st, app, ui, host, &l.dials);

    // ---- behaviour row
    behaviour_ui(st, app, ui, host, [l.ghost, l.close, l.debleed_mode, l.lookahead, l.monitor]);

    // ---- Kit Link: the switch row
    let lr = LinkRects { link_pill: l.link_pill, group: l.group, role: l.role, align_to: l.align_to, reject: l.reject };
    link_switches(st, app, ui, host, &lr);

    // ---- the link status
    let (status, unavailable) = link_status(host, report);
    let fitted = fit_tracked(&p, &status, l.link_status.width(), 10.0, 0.05);
    tracked_text(&p, l.link_status, Align2::LEFT_CENTER, &fitted, 10.0, 0.05, false, false, if unavailable { K_BYPASS_RED } else { pal.text_dim });

    // ---- the phase row
    let pr = PhaseRects { phase_ref: l.phase_ref, align: l.align, line_up: l.line_up, offset: l.offset, flip: l.flip };
    let (phase_text, measured) = phase_row(st, app, ui, host, report, &pr);
    let fitted = fit_tracked(&p, &phase_text, l.align_status.width(), 10.0, 0.03);
    tracked_text(&p, l.align_status, Align2::LEFT_CENTER, &fitted, 10.0, 0.03, false, false, if measured { K_LINK_BLUE } else { pal.text_dim });

    // ---- the More drawer
    if l.more_area.is_some() {
        more_controls(st, app, ui, host, &l.more_dials, &l.more_row, false);
    }

    // ---- the footer: the mic's note, and the two live numbers worth seeing
    let f = l.foot;
    let right = Rect::from_min_max(pos2(f.right() - 240.0, f.top()), f.max);
    let left = Rect::from_min_max(f.min, pos2(right.left(), f.bottom()));
    let why = sd::mode(param(host, "instrument").round() as i32).why;
    let fitted = fit_tracked(&p, why, left.width() - 12.0, 10.0, 0.03);
    tracked_text(&p, left, Align2::LEFT_CENTER, &fitted, 10.0, 0.03, false, false, pal.text_faint);
    let (live, flashing) = live_text(st, host, ro, defers, now);
    if !live.is_empty() {
        tracked_text(&p, right, Align2::RIGHT_CENTER, &live, 10.0, 0.03, false, false, if flashing { K_LINK_BLUE } else { pal.text_faint });
    }

    timer(st, ui, ro);
}

/// `AlignOffsetBox`: Align Offset in samples. Drag (up and right both later; 1 sample per 3 px,
/// 12 with Shift) as one gesture, the wheel one sample per step, double-click to type samples or
/// "1.2 ms" (Enter or losing focus commits, Esc cancels). Setting it switches Align on.
#[allow(clippy::too_many_arguments)]
fn offset_box(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, rect: Rect, align_on: bool, sr: f64) {
    let pal = &PALETTE;
    let applied = shale_align::knob_to_offset_ms(f64::from(param(host, "alignOffset"))) * 0.001 * sr;
    let id = ui.id().with("shale-offset");
    let resp = ui.interact(rect, id, Sense::click_and_drag());
    let to_knob = |samples: f64| shale_align::offset_ms_to_knob(samples / sr * 1000.0) as f32;
    let set = |app: &mut SoundApp, samples: f64, merged: bool| {
        let mut list = vec![("alignOffset", to_knob(samples))];
        if param(host, "align") < 0.5 {
            list.push(("align", 1.0));
        }
        if merged {
            set_params_merged(app, host, &list, "align-offset");
        } else {
            set_params(app, host, &list);
        }
    };
    let alpha = if align_on { 1.0 } else { 0.45 };
    let hot = resp.hovered() || resp.dragged();
    let p = ui.painter().clone();
    let b = rect.shrink(0.5);
    p.rect_filled(b, CornerRadius::same(4), al(pal.panel, alpha));
    p.rect_stroke(b, CornerRadius::same(4), Stroke::new(1.0, al(if hot { pal.border_hover } else { pal.border }, alpha)), StrokeKind::Middle);
    if let Some((text, fresh)) = st.offset_edit.as_mut() {
        let te = egui::TextEdit::singleline(text)
            .id(id.with("text"))
            .font(font(12.0, false))
            .horizontal_align(egui::Align::Center)
            .frame(egui::Frame::NONE);
        let tr = ui.put(rect.shrink(2.0), te);
        if *fresh {
            tr.request_focus();
            *fresh = false;
        }
        if tr.lost_focus() {
            let esc = ui.input(|i| i.key_pressed(egui::Key::Escape));
            let typed = st.offset_edit.take().map(|(t, _)| t).unwrap_or_default();
            if !esc && let Some(s) = parse_offset(&typed, sr) {
                family::push_undo(&mut st.fam, host, &FAM);
                end_gesture(app);
                set(app, s, false);
            }
        }
        return;
    }
    if resp.drag_started() {
        st.offset_drag = Some(applied);
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAM);
    }
    if resp.dragged()
        && let Some(start) = st.offset_drag
    {
        let d = resp.total_drag_delta().unwrap_or_default();
        let px = if ui.input(|i| i.modifiers.shift) { 12.0 } else { 3.0 };
        let moved = f64::from(d.x - d.y) / px;
        let target = start + moved.round();
        if (target - applied).abs() > 0.25 {
            set(app, target, true);
        }
    }
    if resp.drag_stopped() {
        st.offset_drag = None;
        end_gesture(app);
    }
    if resp.hovered() {
        let wheel = juce_wheel_delta_y(ui);
        let wheel = if wheel != 0.0 || crate::widgets::wheel_scrolls(ui.ctx()) { wheel } else { ui.input(|i| i.smooth_scroll_delta.x) };
        if wheel != 0.0 {
            consume_wheel(ui);
            family::push_undo(&mut st.fam, host, &FAM);
            end_gesture(app);
            set(app, applied.round() + if wheel > 0.0 { 1.0 } else { -1.0 }, false);
        }
    }
    if resp.double_clicked() {
        st.offset_edit = Some((format!("{}", applied.round() as i64), true));
    }
    let ink = al(if hot { pal.text_bright } else { pal.text_body }, alpha);
    tracked_text(&p, b, Align2::CENTER_CENTER, &offset_text(applied, sr), 9.0, 0.06, false, false, ink);
    let _ = resp.on_hover_text("Align Offset, in samples: + delays this mic, - moves it earlier (up to 10 ms either way). Drag, Shift-drag for fine, scroll one sample at a time, or double-click and type samples or \"1.2 ms\". Changing it switches Align on.");
}

/// A point in an editor at `frame` for tests and scripts: a main dial's knob ("threshold",
/// "sens", "length", "reduction", "debleed"), the display at `db`, or a mic segment ("mic:<i>").
pub fn spot(frame: Rect, more: bool, which: &str) -> Option<Pos2> {
    let l = layout(frame, more);
    let knob = |r: Rect| crate::editor_kit::controls::dial_knob_bounds(r, Default::default()).center();
    if let Some(i) = ["threshold", "sens", "length", "reduction", "debleed"].iter().position(|x| *x == which) {
        return Some(knob(l.dials[i]));
    }
    if let Some(db) = which.strip_prefix("db:").and_then(|d| d.parse::<f32>().ok()) {
        return Some(display::point_at_db(l.scope, db));
    }
    if let Some(i) = which.strip_prefix("mic:").and_then(|d| d.parse::<usize>().ok()) {
        let per_row = sc::INSTRUMENTS.len().div_ceil(2);
        let (col, row) = (i % per_row, i / per_row);
        let w = l.mic.width() / per_row as f32;
        return Some(pos2(l.mic.left() + w * (col as f32 + 0.5), l.mic.top() + l.mic.height() * (0.25 + 0.5 * row as f32)));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_and_offset_text() {
        assert_eq!(required_height(false), 637.0);
        assert_eq!(required_height(true), 896.0);
        assert_eq!(offset_text(12.0, 48_000.0), "+12 smp   0.25 ms");
        assert_eq!(offset_text(-3.4, 48_000.0), "-3 smp   -0.07 ms");
        assert_eq!(parse_offset("1.2 ms", 48_000.0), Some(58.0));
        assert_eq!(parse_offset(" -7 ", 48_000.0), Some(-7.0));
        assert_eq!(parse_offset("abc", 48_000.0), None);
    }
}

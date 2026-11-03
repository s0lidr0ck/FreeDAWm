//! Amp's VST editor (`Engine 12 - Amp/Source/PluginEditor.cpp`), translated: the layout of
//! `resized()` (each amp's front panel in its own order), the paint of `paint()` (the face strip,
//! [`face`]; the rig and its meters), and every gesture. Like Twine it keeps the theme's defaults
//! (`textLabel = 0xFF8A96A3`), tinted by the amp in use (`accentFor`), the same in both DAW themes.
//! Its interaction inventory is `docs/superpowers/editors/amp.md`.
//!
//! The meters are the engine's (`amp::LIVE_*`: running maxima taken at 30 Hz as the C++
//! `exchange (0)`, and the supply volts). Load IR... goes through `mix.module_ir` (which puts the
//! amp's Cabinet on IR, as `chooseImpulse` does). The VST's own Undo stack is kept in the editor.

mod face;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{
    DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, preset_name_button_ex, segmented_ex, segmented_id, tip,
};
use crate::editor_kit::look::tracked_text;
use crate::editor_kit::palette::al;
use crate::editor_kit::presets::{self, PresetState};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, set_params, set_params_merged};
use egui::{Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::fs::amp as dsp;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

const PAD_X: f32 = 16.0;
const PAD_Y: f32 = 11.0;
const GAP: f32 = 9.0;
const HEADER_H: f32 = 25.0;
const SELECT_H: f32 = 28.0;
const FACE_H: f32 = 214.0;
const RIG_H: f32 = 150.0;
const FOOT_H: f32 = 20.0;
const WIDTH: f32 = 900.0;
const HEIGHT: f32 = PAD_Y + HEADER_H + GAP + SELECT_H + GAP + FACE_H + GAP + RIG_H + GAP + FOOT_H + PAD_Y;
/// `setSize (kWidth, kHeight)`.
pub const SIZE: Vec2 = vec2(WIDTH, HEIGHT);

pub(crate) const AMP_TWIN: usize = 0;
pub(crate) const AMP_MARSHALL: usize = 1;
pub(crate) const AMP_DELUXE: usize = 2;
pub(crate) const AMP_SUPER: usize = 3;
pub(crate) const AMP_PRINCETON: usize = 4;
pub(crate) const AMP_AC30: usize = 5;
pub(crate) const AMP_ORANGE: usize = 6;

const BYPASS_RED: Color32 = argb(0xFFFF6B5E);
const PANEL_FILL: Color32 = argb(0xFF12161A);
const PANEL_LINE: Color32 = argb(0xFF1F252B);
const ROOT_BACK: Color32 = argb(0xFF101316);

/// The theme's defaults with `textLabel = 0xFF8A96A3`; the accent is the amp's (`accentFor`).
pub const PALETTE: VstPalette =
    VstPalette { text_label: argb(0xFF8A96A3), bypass_red: BYPASS_RED, ..VstPalette::THEME_DEFAULTS.with_accent(argb(0xFF8FC9D6)) };

/// `accentFor`: each amp's own colour.
fn accent_for(amp: usize) -> Color32 {
    argb([0xFF8FC9D6, 0xFFD9B45A, 0xFFE4DDCF, 0xFFA9C2DE, 0xFFD9C9A8, 0xFFD7925A, 0xFFF2994A][amp.min(6)])
}

/// `toDb`.
fn to_db(lin: f32) -> f32 {
    if lin.is_finite() && lin > 1.0e-6 { 20.0 * lin.log10() } else { -120.0 }
}

/// A control on an amp's face.
#[derive(Clone, Copy)]
enum Part {
    Dial(&'static str, &'static str),
    /// A channel switch over a Bright pill (Twin, Super): (channel, bright).
    ChannelBright(&'static str, &'static str),
    /// A switch: (param, options, rows, size, lift).
    Switch(&'static str, &'static [&'static str], usize, (f32, f32), f32),
}

const CHANNELS: &[&str] = &["Vibrato", "Normal"];
const AC_CHANNELS: &[&str] = &["Top Boost", "Normal", "Both"];
const FAC: &[&str] = &["1", "2", "3", "4", "5", "6"];

/// The front panel's order, left to right (`resized`).
fn face_parts(amp: usize) -> &'static [Part] {
    use Part::*;
    match amp {
        AMP_TWIN => &[
            ChannelBright("channel", "bright"),
            Dial("volume", "Volume"),
            Dial("treble", "Treble"),
            Dial("middle", "Middle"),
            Dial("bass", "Bass"),
            Dial("reverb", "Reverb"),
            Dial("speed", "Speed"),
            Dial("intensity", "Intensity"),
            Dial("master", "Master"),
        ],
        AMP_DELUXE => &[
            Switch("dlxChannel", CHANNELS, 2, (92.0, 44.0), -14.0),
            Dial("dlxVolume", "Volume"),
            Dial("dlxTreble", "Treble"),
            Dial("dlxBass", "Bass"),
            Dial("dlxReverb", "Reverb"),
            Dial("dlxSpeed", "Speed"),
            Dial("dlxIntensity", "Intensity"),
        ],
        AMP_SUPER => &[
            ChannelBright("supChannel", "supBright"),
            Dial("supVolume", "Volume"),
            Dial("supTreble", "Treble"),
            Dial("supMiddle", "Middle"),
            Dial("supBass", "Bass"),
            Dial("supReverb", "Reverb"),
            Dial("supSpeed", "Speed"),
            Dial("supIntensity", "Intensity"),
        ],
        AMP_PRINCETON => &[
            Dial("prnVolume", "Volume"),
            Dial("prnTreble", "Treble"),
            Dial("prnBass", "Bass"),
            Dial("prnReverb", "Reverb"),
            Dial("prnSpeed", "Speed"),
            Dial("prnIntensity", "Intensity"),
        ],
        AMP_AC30 => &[
            Switch("acChannel", AC_CHANNELS, 3, (100.0, 66.0), -12.0),
            Dial("acNormal", "Normal"),
            Dial("acBrilliant", "Brilliant"),
            Dial("acTreble", "Treble"),
            Dial("acBass", "Bass"),
            Dial("acCut", "Cut"),
        ],
        AMP_ORANGE => &[
            Dial("orGain", "Gain"),
            Switch("orFac", FAC, 2, (120.0, 48.0), -12.0),
            Dial("orBass", "Bass"),
            Dial("orTreble", "Treble"),
            Dial("orHfDrive", "HF Drive"),
        ],
        _ => &[
            Dial("presence", "Presence"),
            Dial("jtmBass", "Bass"),
            Dial("jtmMiddle", "Middle"),
            Dial("jtmTreble", "Treble"),
            Dial("jtmVolume", "Volume I"),
            Dial("volume2", "Volume II"),
            Dial("jtmSpeed", "Speed"),
            Dial("jtmIntensity", "Intensity"),
        ],
    }
}

/// Each amp's rig: input, output, cabinet, Auto Level (`rigs()`).
fn rig(amp: usize) -> [&'static str; 4] {
    match amp {
        AMP_MARSHALL => ["jtmInput", "jtmOutput", "jtmCab", "jtmAutoLevel"],
        AMP_DELUXE => ["dlxInput", "dlxOutput", "dlxCab", "dlxAutoLevel"],
        AMP_SUPER => ["supInput", "supOutput", "supCab", "supAutoLevel"],
        AMP_PRINCETON => ["prnInput", "prnOutput", "prnCab", "prnAutoLevel"],
        AMP_AC30 => ["acInput", "acOutput", "acCab", "acAutoLevel"],
        AMP_ORANGE => ["orInput", "orOutput", "orCab", "orAutoLevel"],
        _ => ["input", "output", "cab", "autoLevel"],
    }
}

/// The channel switch whose Normal greys reverb, speed and intensity (`setInactive`).
fn normal_channel(host: &Host, amp: usize) -> bool {
    let ch = match amp {
        AMP_TWIN => "channel",
        AMP_DELUXE => "dlxChannel",
        AMP_SUPER => "supChannel",
        _ => return false,
    };
    param(host, ch).round() as i32 == 1
}

/// A meter with a peak hold.
#[derive(Clone, Copy, Debug)]
struct Meter {
    now: f32,
    hold: f32,
    hold_ticks: i32,
}

impl Default for Meter {
    fn default() -> Self {
        Meter { now: -120.0, hold: -120.0, hold_ticks: 0 }
    }
}

impl Meter {
    fn feed(&mut self, v: f32, fall: f32) {
        self.now = if v > self.now { v } else { self.now - fall };
        let reset = v >= self.hold || {
            self.hold_ticks -= 1;
            self.hold_ticks <= 0
        };
        if reset {
            self.hold = v;
            self.hold_ticks = 45;
        }
    }
}

type Snapshot = Vec<(&'static str, f32)>;

fn snapshot(host: &Host) -> Snapshot {
    host.info.params.iter().map(|p| (p.id, param(host, p.id))).collect()
}

/// Amp's editor: everything the C++ editor holds that is not a parameter.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    undo: Vec<Snapshot>,
    in_meter: Meter,
    out_meter: Meter,
    watts_shown: f32,
    watts_hold: f32,
    watts_hold_ticks: i32,
    sag_shown: f32,
    tick_at: f64,
    presets: PresetState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// Scripts: open the slim layout's amp menu on the next frame.
    models_open: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
}

/// Draws Amp's editor for `module` on `track`.
/// What `getPresetParams` leaves out besides `bypass` (Amp's `Presets.h`).
const NOT_IN_PRESETS: [&str; 15] = [
    "quality",
    "input",
    "jtmInput",
    "dlxInput",
    "supInput",
    "prnInput",
    "acInput",
    "orInput",
    "autoLevel",
    "jtmAutoLevel",
    "dlxAutoLevel",
    "supAutoLevel",
    "prnAutoLevel",
    "acAutoLevel",
    "orAutoLevel",
];

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_amp";

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

impl Editor {
    /// `pushUndo` (32 deep).
    fn push_undo(&mut self, s: Snapshot) {
        self.undo.push(s);
        if self.undo.len() > 32 {
            self.undo.remove(0);
        }
    }

    /// `timerCallback`.
    fn tick(&mut self, host: &Host) {
        let live = crate::editor_kit::live(host);
        let take = |i: usize| live.map_or(0.0, |l| l.take(i));
        let get = |i: usize| live.map_or(0.0, |l| l.get(i));
        let input = to_db(take(dsp::LIVE_IN_PEAK));
        let out = to_db(take(dsp::LIVE_OUT_PEAK));
        let w = take(dsp::LIVE_WATTS);
        let w = if w.is_finite() { w } else { 0.0 };
        let (bp, bp_idle) = (get(dsp::LIVE_B_PLUS), get(dsp::LIVE_B_PLUS_IDLE));
        self.in_meter.feed(input, 0.8);
        self.out_meter.feed(out, 0.8);
        self.watts_shown = if w > self.watts_shown { w } else { self.watts_shown * 0.9 };
        let reset = w >= self.watts_hold || {
            self.watts_hold_ticks -= 1;
            self.watts_hold_ticks <= 0
        };
        if reset {
            self.watts_hold = w;
            self.watts_hold_ticks = 45;
        }
        let sag = if bp.is_finite() && bp_idle.is_finite() && bp > 1.0 { (bp_idle - bp).max(0.0) } else { 0.0 };
        self.sag_shown = if sag > self.sag_shown { sag } else { self.sag_shown + (sag - self.sag_shown) * 0.15 };
    }
}

fn panel(p: &egui::Painter, a: Rect) {
    p.rect_filled(a, CornerRadius::same(5), PANEL_FILL);
    p.rect_stroke(a.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, PANEL_LINE), StrokeKind::Middle);
}

fn pill(ui: &mut Ui, pal: &VstPalette, r: Rect, salt: &str, text: &str, on: bool, dimmed: bool, lit: Option<Color32>) -> egui::Response {
    pill_ex(ui, r, salt, pal, text, on, PillStyle::Small, PillLook { active_colour: lit, tracked: true, dimmed })
}

/// The amp in use (`currentAmp`).
fn amp_of(host: &Host) -> usize {
    let a = param(host, "amp");
    if a.is_finite() { (a.round() as i64).clamp(0, 6) as usize } else { 0 }
}

// ============================================================================ the editor

/// What both layouts do first: the scripted drawer tabs, the 30 Hz timer, and the preset browser,
/// which covers the whole editor while it is open (then `None`: paint nothing else). Returns the
/// amp in use and the palette tinted by it.
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> Option<(usize, VstPalette)> {
    let ed = host.rect;
    if !st.init {
        st.init = true;
        st.presets.category = "All".into();
    }
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.presets),
            "Models" if st.slim => st.models_open = true,
            "More" if st.slim => st.slim_more = true,
            _ => {}
        }
    }
    let now = ui.input(|i| i.time);
    if now - st.tick_at >= 1.0 / 30.0 || now < st.tick_at {
        st.tick_at = now;
        st.tick(host);
    }
    if host.meters.is_some() {
        ui.ctx().request_repaint();
    }
    let amp = amp_of(host);
    let pal = VstPalette { accent: accent_for(amp), ..PALETTE };

    st.presets.not_in_presets = &NOT_IN_PRESETS;
    if st.presets.open {
        let before = snapshot(host);
        let out = presets::preset_browser(app, ui, host, ed, &pal, &mut st.presets);
        let pasted = presets::clipboard(app, ui, host, &mut st.presets, &out);
        if out.recalled.is_some() {
            end_gesture(app);
        }
        if out.recalled.is_some() || pasted.is_some() {
            st.push_undo(before);
        }
        return None;
    }
    Some((amp, pal))
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    let Some((amp, pal)) = prelude(st, app, ui, host) else { return };
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, ROOT_BACK);
    let r = ed.shrink2(vec2(PAD_X, PAD_Y));
    let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H));
    let select = Rect::from_min_size(pos2(r.left(), header.bottom() + GAP), vec2(r.width(), SELECT_H));
    let face_area = Rect::from_min_size(pos2(r.left(), select.bottom() + GAP), vec2(r.width(), FACE_H));
    let rig_area = Rect::from_min_size(pos2(r.left(), face_area.bottom() + GAP), vec2(r.width(), RIG_H));
    let foot = Rect::from_min_size(pos2(r.left(), rig_area.bottom() + GAP), vec2(r.width(), FOOT_H));

    header_ui(st, app, ui, host, &pal, header);

    // ---- the amp select
    let sel_r = Rect::from_center_size(select.center(), vec2(select.width().min(860.0), SELECT_H));
    if let Some(i) = segmented_ex(ui, sel_r, "amp", &pal, &dsp::AMPS, 1, Some(amp), SelectionStyle::Filled, true) {
        pick_amp(st, app, host, i);
    }
    if let Some(t) = tooltip("amp") {
        tip(ui, segmented_id(ui, "amp"), t);
    }

    // ---- the face
    panel(&p, face_area);
    let normal = normal_channel(host, amp);
    let strip = Rect::from_min_size(face_area.min + vec2(1.0, 1.0), vec2(face_area.width() - 2.0, 44.0));
    face::paint_strip(&p, strip, amp, normal);
    let f = face_area.shrink2(vec2(14.0, 0.0));
    let row = Rect::from_min_size(pos2(f.left(), f.top() + 44.0 + 10.0), vec2(f.width(), 130.0));
    let parts = face_parts(amp);
    let slot_w = (row.width() / parts.len() as f32).trunc();
    for (i, part) in parts.iter().enumerate() {
        let slot = Rect::from_min_size(pos2(row.left() + slot_w * i as f32, row.top()), vec2(slot_w, row.height()));
        match *part {
            Part::Dial(id, label) => {
                let inactive = greyed(id, normal);
                let dr = Rect::from_center_size(slot.center(), vec2(slot_w.min(96.0), 122.0));
                knob(st, app, ui, host, &pal, dr, id, label, inactive);
                // the Pull Boost pulls from the Master knob, so it sits under it
                if id == "master" {
                    let br = Rect::from_min_size(pos2(dr.center().x - 44.0, dr.bottom() - 2.0), vec2(88.0, 22.0));
                    toggle_pill(st, app, ui, host, &pal, br, "pullBoost", "Pull Boost");
                }
            }
            Part::ChannelBright(ch, bright) => {
                let w = (slot_w - 6.0).min(92.0);
                let cr = Rect::from_center_size(slot.center() + vec2(0.0, -26.0), vec2(w, 44.0));
                switch(st, app, ui, host, &pal, cr, ch, CHANNELS, 2);
                let br = Rect::from_center_size(slot.center() + vec2(0.0, 22.0), vec2((slot_w - 8.0).min(84.0), 24.0));
                toggle_pill(st, app, ui, host, &pal, br, bright, "Bright");
            }
            Part::Switch(id, opts, rows, (w, h), lift) => {
                let sr = Rect::from_center_size(slot.center() + vec2(0.0, lift), vec2((slot_w - 6.0).min(w), h));
                switch(st, app, ui, host, &pal, sr, id, opts, rows);
                if id == "orFac" {
                    // the FAC switch's label, drawn like a knob's
                    let lr = Rect::from_min_size(pos2(sr.left(), sr.bottom() + 36.0), vec2(sr.width(), 14.0));
                    tracked_text(&p, lr, Align2::CENTER_CENTER, "FAC", 9.0, 0.12, true, false, pal.text_label);
                }
            }
        }
    }

    // ---- the rig
    panel(&p, rig_area);
    rig_ui(st, app, ui, host, &pal, rig_area, amp);

    // ---- a line about the amp
    tracked_text(&p, foot, Align2::LEFT_CENTER, amp_line(amp), 10.0, 0.03, false, false, pal.text_faint);
    crate::hardware_credit::hover(ui, foot, "amp_line", amp_line(amp), amp_credit(amp));
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// A line about the amp (the full editor's foot; the slim menu's descriptions).
fn amp_line(amp: usize) -> &'static str {
    match amp {
        AMP_TWIN => "Loud and clean to the top of the dial; the reverb and the vibrato are the amp's own circuits.",
        AMP_DELUXE => "Two 6V6s and a GZ34: clean at 3, singing by 6 -- and it sags when you dig in.",
        AMP_SUPER => "Two 6L6s into four 10s: more headroom than a Deluxe, less than a Twin -- the blues club amp.",
        AMP_PRINCETON => "Twelve watts and a 10: edge of breakup at a volume you can record at.",
        AMP_AC30 => "Four EL84s biasing themselves: chime at low volume, and it blooms and compresses as you dig in.",
        AMP_ORANGE => "120 W of EL34s: a thick, warm, mid-forward clean with a long way to go before it breaks.",
        _ => "Breaks up early and sweetly: the cathode follower clips one side, the KT66s and the tube rectifier sag.",
    }
}

/// The amp's hardware credit line (`crate::hardware_credit`): every amp is wired from its
/// schematic (`amp_models`), so each is a component-level model.
pub(crate) fn amp_credit(amp: usize) -> &'static str {
    match amp {
        AMP_TWIN => "Component-level model of the 1977 Fender Twin Reverb. No affiliation; Fender and Twin Reverb are trademarks of their owner.",
        AMP_DELUXE => {
            "Component-level model of the 1965 Fender Deluxe Reverb. No affiliation; Fender and Deluxe Reverb are trademarks of their owner."
        }
        AMP_SUPER => "Component-level model of the 1965 Fender Super Reverb. No affiliation; Fender and Super Reverb are trademarks of their owner.",
        AMP_PRINCETON => {
            "Component-level model of the 1965 Fender Princeton Reverb. No affiliation; Fender and Princeton are trademarks of their owner."
        }
        AMP_AC30 => "Component-level model of the 1964 Vox AC30 Top Boost. No affiliation; Vox and AC30 are trademarks of their owner.",
        AMP_ORANGE => "Component-level model of the Orange OR120 Graphic. No affiliation; Orange is a trademark of its owner.",
        _ => "Component-level model of the 1966 Marshall JTM45 (model 1962). No affiliation; Marshall and JTM45 are trademarks of their owner.",
    }
}

/// The amp select's pick: a complete change after `pushUndo`.
fn pick_amp(st: &mut Editor, app: &mut SoundApp, host: &Host, i: usize) {
    st.push_undo(snapshot(host));
    set_params(app, host, &[("amp", i as f32)]);
}

/// The Normal channel greys reverb, speed and intensity (`setInactive`).
fn greyed(id: &str, normal: bool) -> bool {
    normal && (id.ends_with("everb") || id.ends_with("peed") || id.ends_with("ntensity"))
}

fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, h: Rect) {
    let p = ui.painter().clone();
    tracked_text(&p, Rect::from_min_size(h.min, vec2(126.0, h.height())), Align2::LEFT_CENTER, "AMP ENGINE", 15.0, 0.24, true, true, pal.text_bright);
    let sub = Rect::from_min_size(pos2(h.left() + 130.0, h.top()), vec2(70.0, h.height()));
    tracked_text(&p, sub, Align2::LEFT_CENTER, "tube amps", 10.0, 0.12, true, false, pal.accent);
    let mut right = h.right();
    let mut take = |w: f32, gap: f32| {
        let r = Rect::from_min_max(pos2(right - w, h.top()), pos2(right, h.bottom()));
        right -= w + gap;
        r
    };
    let bypass_r = take(62.0, 5.0);
    let undo_r = take(52.0, 12.0);
    let next_r = take(26.0, 3.0);
    let prev_r = take(26.0, 6.0);
    let name_r = Rect::from_min_max(pos2(h.left() + 206.0, h.top() + 1.0), pos2(right, h.bottom() - 1.0));
    header_controls(st, app, ui, host, pal, &HeaderRects { bypass: bypass_r, undo: undo_r, prev: prev_r, next: next_r, name: name_r });
}

/// Where the header's controls go (each layout places them).
struct HeaderRects {
    bypass: Rect,
    undo: Rect,
    prev: Rect,
    next: Rect,
    name: Rect,
}

/// The header's controls and their gestures: Bypass, Undo, the preset steppers and the name.
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, r: &HeaderRects) {
    let (bypass_r, undo_r, prev_r, next_r, name_r) = (r.bypass, r.undo, r.prev, r.next, r.name);
    let byp = param(host, "bypass") > 0.5;
    if pill(ui, pal, bypass_r, "bypass", "Bypass", byp, false, Some(BYPASS_RED)).clicked() {
        set_params(app, host, &[("bypass", if byp { 0.0 } else { 1.0 })]);
    }
    if pill(ui, pal, undo_r, "undo", "Undo", false, st.undo.is_empty(), None).clicked()
        && let Some(v) = st.undo.pop()
    {
        end_gesture(app);
        set_params(app, host, &v);
    }
    for (r, dir, text) in [(prev_r, -1, "<"), (next_r, 1, ">")] {
        if pill(ui, pal, r, if dir < 0 { "prev" } else { "next" }, text, false, false, None).clicked() {
            let before = snapshot(host);
            let cat = std::mem::replace(&mut st.presets.category, "All".into());
            let out = presets::step(app, host, &mut st.presets, dir);
            st.presets.category = cat;
            if out.recalled.is_some() {
                end_gesture(app);
                st.push_undo(before);
            }
        }
    }
    let dirty = presets::is_dirty(app, host, &mut st.presets);
    if preset_name_button_ex(ui, name_r, "name", pal, presets::shown_name(host), dirty).clicked() {
        presets::open(app, host, &mut st.presets);
    }
}

// ---------------------------------------------------------------------------- addTooltips

const TIP_VOLUME: &str = "The channel volume, between the first gain stage and the second -- the knob that decides how hard the rest of the \
     amp is pushed. On the JTM45 this is Volume I, the bright channel.";
const TIP_REVERB: &str = "The Twin's spring reverb: a 12AT7 driving a long-decay tank, recovered by a 7025.";
const TIP_SPEED: &str = "How fast the vibrato (Twin) or tremolo (JTM45) throbs.";
const TIP_INTENSITY: &str = "How deep. The Twin's 'vibrato' is an optical tremolo -- a neon bulb and a light-dependent resistor -- so it chops \
     rather than sways.";
const TIP_INPUT: &str = "Match your interface. 0 dB treats a full-scale DI as a 2 V guitar; if your DI peaks near -18 dBFS, that is a quiet \
     single coil and the amp will stay clean longer.";
const TIP_CAB: &str = "Built-in: a stand-in shaped after each amp's own speakers. IR: your cabinet impulse response (load one). Off: the \
     speaker voltage, for an external cab simulator.";
const TIP_OUTPUT: &str = "Level out of the plugin -- a trim on top of Auto Level when that is on.";
const TIP_AUTO: &str = "Auto Level: the output follows your guitar's level, not the amp's. A clean Twin and a dimed JTM45 come out about as \
     loud as each other, so Volume changes the tone and the drive, not the level. Playing softer still comes out softer. Each amp has \
     its own.";

/// `addTooltips`, by parameter (the amps' shared dials take the Twin's text, as `getTooltip()`
/// copies it).
fn tooltip(id: &str) -> Option<&'static str> {
    Some(match id {
        "amp" => "Which amp. Each is modelled from its own schematic, tube by tube.",
        "volume" | "jtmVolume" => TIP_VOLUME,
        "reverb" | "dlxReverb" | "supReverb" | "prnReverb" => TIP_REVERB,
        "speed" | "jtmSpeed" | "dlxSpeed" | "supSpeed" => TIP_SPEED,
        "intensity" | "jtmIntensity" | "dlxIntensity" | "supIntensity" => TIP_INTENSITY,
        "master" => "The Twin's Master, after the preamp: loud-and-clean at the top, or the preamp pushed harder at a lower level.",
        "pullBoost" => "The Twin's Pull Boost on the Master: more gain into the power amp.",
        "channel" => {
            "The Twin's two channels. Vibrato has the reverb and the vibrato, and a treble-rich mixer in front of them; Normal is one \
             gain stage shorter, fuller and darker, with neither."
        }
        "bright" => "The Twin's Bright switch: a 120 pF cap across the volume pot. Most obvious at low Volume.",
        "volume2" => "The JTM45's Normal channel, jumpered in with the bright one as everybody plays them. Turn it up for more body and low end.",
        "presence" => "The JTM45's Presence: takes treble out of the negative feedback, so the power amp adds it back.",
        "input" | "jtmInput" | "dlxInput" | "supInput" | "prnInput" | "acInput" | "orInput" => TIP_INPUT,
        "cab" | "jtmCab" | "dlxCab" | "supCab" | "prnCab" | "acCab" | "orCab" => TIP_CAB,
        "quality" => {
            "How far the amp is oversampled. 2x is clean and light on the CPU; 4x is cleaner still at about twice the cost -- for \
             mixdown."
        }
        "output" | "jtmOutput" | "dlxOutput" | "supOutput" | "prnOutput" | "acOutput" | "orOutput" => TIP_OUTPUT,
        "autoLevel" | "jtmAutoLevel" | "dlxAutoLevel" | "supAutoLevel" | "prnAutoLevel" | "acAutoLevel" | "orAutoLevel" => TIP_AUTO,
        "dlxVolume" => {
            "The Deluxe's Volume. There is no Master: this is the knob that pushes the 6V6s, and past about 4 the amp starts to break \
             up -- the Deluxe sound."
        }
        "dlxChannel" => "The Deluxe's two channels. Vibrato has the reverb and the vibrato; Normal has neither.",
        "acChannel" => {
            "Which input the guitar is in. Top Boost: the Brilliant channel through the Top Boost's gain, cathode follower and \
             Treble/Bass -- the chime. Normal: plainer and darker, only the Cut shapes it. Both: the two jumpered, as many players run \
             it -- blend them with the two volumes."
        }
        "acNormal" => "The Normal channel's volume.",
        "acBrilliant" => {
            "The Brilliant (Top Boost) channel's volume. It feeds a whole extra gain stage: the AC30 gets gritty sooner here than on \
             Normal."
        }
        "acTreble" => "The Top Boost's Treble. It works on the Brilliant channel only, and it interacts with Bass.",
        "acBass" => "The Top Boost's Bass. Brilliant channel only.",
        "acCut" => "Vox's Cut: a treble cut right at the power tubes, on every channel. Turn it UP to take the top off.",
        "orGain" => "The Orange's Gain, between its first and second stages. 120 W of EL34s behind it: loud and clean a long way up.",
        "orFac" => "FAC: a six-way switch down a chain of capacitors. 1 is full; each step takes more low end away -- 6 is thin and cutting.",
        "orBass" => "A Baxandall bass control: boost or cut around a flat centre, unlike the Fender and Marshall stacks.",
        "orTreble" => "A Baxandall treble control.",
        "orHfDrive" => {
            "HF Drive: a tuned network (2 mH, 0.47 uF) in the driver's cathode that takes the negative feedback away around 5 kHz -- \
             a presence control. Its wiring on the drawing is not certain; here it is gentle."
        }
        "supVolume" => "The Super's Volume (no Master). Clean to about 4, singing by 6 -- between a Deluxe and a Twin.",
        "supChannel" => "The Super's two channels. Vibrato has the reverb and the vibrato; Normal has neither.",
        "supBright" => "The Super's Bright switch: 120 pF across the Volume pot. Most obvious at low Volume.",
        "prnVolume" => "The Princeton's Volume. It is small: it breaks up early and at a level you can record at.",
        "prnSpeed" => "How fast the tremolo throbs.",
        "prnIntensity" => {
            "How deep. The Princeton's 'vibrato' moves the power tubes' bias: a smoother throb than the Deluxe's optical kind, and the \
             tubes' gain and grit pulse with it."
        }
        _ => return None,
    })
}

/// `attachDial`: label, `formatParam` readout, `pushUndo` on the drag start.
#[allow(clippy::too_many_arguments)]
fn knob(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, r: Rect, id: &'static str, label: &str, inactive: bool) {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let value = param(host, id);
    let text = info.format(value);
    let o = DialOpts { label, value_text: &text, default: info.to_normalized(info.default), tracked: true, inactive, ..DialOpts::default() };
    let out = dial(ui, r, id, pal, info.to_normalized(value), &o);
    if let Some(t) = tooltip(id) {
        let _ = out.response.clone().on_hover_text(t);
    }
    if out.started {
        st.push_undo(snapshot(host));
        end_gesture(app);
    }
    if let Some(v) = out.value {
        set_params_merged(app, host, &[(id, info.clamp(info.from_normalized(v)))], &format!("dial-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
}

/// `attachSegmented`: a pick pushes Undo.
#[allow(clippy::too_many_arguments)]
fn switch(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    pal: &VstPalette,
    r: Rect,
    id: &'static str,
    opts: &[&str],
    rows: usize,
) -> Option<usize> {
    let sel = usize::try_from(param(host, id).round() as i64).ok();
    let picked = segmented_ex(ui, r, id, pal, opts, rows, sel, SelectionStyle::Filled, true);
    if let Some(t) = tooltip(id) {
        tip(ui, segmented_id(ui, id), t);
    }
    if let Some(i) = picked {
        st.push_undo(snapshot(host));
        set_params(app, host, &[(id, i as f32)]);
    }
    picked
}

/// `attachPill`.
#[allow(clippy::too_many_arguments)]
fn toggle_pill(_st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, r: Rect, id: &'static str, text: &str) {
    let on = param(host, id) > 0.5;
    let resp = pill(ui, pal, r, id, text, on, false, None);
    if resp.clicked() {
        set_params(app, host, &[(id, if on { 0.0 } else { 1.0 })]);
    }
    if let Some(t) = tooltip(id) {
        let _ = resp.on_hover_text(t);
    }
}

/// `chooseImpulse`: a file through `mix.module_ir` (the amp's Cabinet goes on IR).
fn choose_impulse(app: &mut SoundApp, host: &Host, amp: usize) {
    let path = app.services.pick_open.as_ref().and_then(|open| open("mix.module_ir", &["wav", "wave", "aif", "aiff"]));
    if let Some(path) = path {
        let r = app.run("mix.module_ir", json!({"track": host.track.0, "module": host.ins.id.0, "amp": amp, "ir": {"path": path}}));
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

/// The rig: input, cabinet (+ Load IR...), oversampling, output + Auto Level, the meters.
fn rig_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, area: Rect, amp: usize) {
    let p = ui.painter().clone();
    let [input, output, _, auto] = rig(amp);
    let g = area.shrink2(vec2(12.0, 10.0));
    let in_r = Rect::from_min_size(g.min, vec2(96.0, 126.0));
    knob(st, app, ui, host, pal, in_r, input, "Input", false);
    let meter_area = Rect::from_min_max(pos2(g.right() - 250.0, g.top()), g.max);
    let out_col = Rect::from_min_max(pos2(meter_area.left() - 10.0 - 96.0, g.top()), pos2(meter_area.left() - 10.0, g.bottom()));
    let out_r = Rect::from_min_size(out_col.min, vec2(96.0, 112.0));
    knob(st, app, ui, host, pal, out_r, output, "Output", false);
    let auto_r = Rect::from_center_size(pos2(out_col.center().x, out_r.bottom() + 12.0), vec2(88.0, 22.0));
    toggle_pill(st, app, ui, host, pal, auto_r, auto, "Auto Level");

    let col = Rect::from_min_max(pos2(in_r.right() + 10.0, g.top()), pos2(out_col.left() - 14.0, g.bottom()));
    let cab_label = Rect::from_min_size(col.min, vec2(col.width(), 14.0));
    tracked_text(&p, cab_label, Align2::LEFT_CENTER, "Cabinet", 9.5, 0.1, true, false, pal.text_label);
    let cab_row = Rect::from_min_size(pos2(col.left(), cab_label.bottom()), vec2(col.width(), 26.0));
    let cab_r = Rect::from_min_size(cab_row.min, vec2(230f32.min(cab_row.width() - 100.0), 26.0));
    let ir_r = Rect::from_center_size(pos2(cab_r.right() + 8.0 + 46.0, cab_row.center().y), vec2(92.0, 22.0));
    cabinet_ui(st, app, ui, host, pal, cab_r, ir_r, amp);
    let name_r = Rect::from_min_size(pos2(col.left(), cab_row.bottom()), vec2(col.width(), 18.0));
    p.with_clip_rect(name_r.intersect(p.clip_rect())).text(
        pos2(name_r.left(), name_r.center().y),
        Align2::LEFT_CENTER,
        cab_text(host, amp),
        crate::theme::regular(10.5),
        pal.text_dim,
    );
    let q_label = Rect::from_min_size(pos2(col.left(), name_r.bottom() + 12.0), vec2(col.width(), 14.0));
    tracked_text(&p, q_label, Align2::LEFT_CENTER, "Oversampling", 9.5, 0.1, true, false, pal.text_label);
    let q_r = Rect::from_min_size(pos2(col.left(), q_label.bottom()), vec2(130.0, 26.0));
    switch(st, app, ui, host, pal, q_r, "quality", &["2x", "4x"], 1);

    // ---- meters: in, watts into the speaker, the sag, out
    meters(&p, st, pal, meter_area, amp);
}

/// The Cabinet switch in `cab_r` (picking IR with none loaded asks for one) and Load IR... in
/// `ir_r`.
#[allow(clippy::too_many_arguments)]
fn cabinet_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, cab_r: Rect, ir_r: Rect, amp: usize) {
    let [_, _, cab, _] = rig(amp);
    let impulse = host.ins.assets.get(dsp::IR_KEYS[amp]).cloned();
    if switch(st, app, ui, host, pal, cab_r, cab, &["Built-in", "IR", "Off"], 1) == Some(1) && impulse.is_none() {
        // picking IR with no IR loaded for this amp asks for one
        choose_impulse(app, host, amp);
    }
    if pill(ui, pal, ir_r, "loadIr", "Load IR...", false, false, None).clicked() {
        choose_impulse(app, host, amp);
    }
}

/// What the cabinet is: the built-in's speakers, the IR's name, or none.
fn cab_text(host: &Host, amp: usize) -> String {
    let [_, _, cab, _] = rig(amp);
    let impulse = host.ins.assets.get(dsp::IR_KEYS[amp]).cloned();
    let cab_v = param(host, cab).round() as i32;
    match cab_v {
        1 => impulse
            .map_or_else(|| "no IR loaded -- using the built-in cabinet".to_string(), |r| format!("IR: {}", crate::module_drawer::ir_label(&r))),
        2 => "speaker out, no cabinet".to_string(),
        _ => match amp {
            AMP_TWIN => "built-in: open-back 2 x 12",
            AMP_DELUXE => "built-in: open-back 1 x 12",
            AMP_SUPER => "built-in: open-back 4 x 10",
            AMP_PRINCETON => "built-in: open-back 1 x 10",
            AMP_AC30 => "built-in: open-back 2 x 12, alnico Blues",
            AMP_ORANGE => "built-in: closed-back 4 x 12, Celestion",
            _ => "built-in: open-back 2 x 12, alnico",
        }
        .to_string(),
    }
}

/// The meters in `meter_area`: in, watts into the speaker, the sag, out.
fn meters(p: &egui::Painter, st: &Editor, pal: &VstPalette, meter_area: Rect, amp: usize) {
    let watts_max = [150.0, 60.0, 40.0, 90.0, 30.0, 45.0, 150.0][amp.min(6)];
    let sag_max = [60.0, 90.0, 70.0, 80.0, 90.0, 90.0, 90.0][amp.min(6)];
    let db_text = |db: f32| if db <= -119.0 { "-inf".to_string() } else { format!("{db:.1}") };
    let watts = if st.watts_hold < 10.0 { format!("{:.1}", st.watts_hold) } else { format!("{:.0}", st.watts_hold) };
    let sag = if st.sag_shown < 0.5 { "0 V".to_string() } else { format!("-{:.0} V", st.sag_shown) };
    let darker = |c: Color32, a: f32| {
        let k = 1.0 - a / (1.0 + a);
        Color32::from_rgb((f32::from(c.r()) * k) as u8, (f32::from(c.g()) * k) as u8, (f32::from(c.b()) * k) as u8)
    };
    let bars = [
        ("In", ((st.in_meter.now + 48.0) / 48.0).clamp(0.0, 1.0), db_text(st.in_meter.hold), pal.text_body),
        ("Watts", (st.watts_shown / watts_max).max(0.0).sqrt().clamp(0.0, 1.0), watts, pal.accent),
        ("Sag", (st.sag_shown / sag_max).clamp(0.0, 1.0), sag, darker(pal.accent, 0.3)),
        ("Out", ((st.out_meter.now + 48.0) / 48.0).clamp(0.0, 1.0), db_text(st.out_meter.hold), pal.text_bright),
    ];
    let bw = (meter_area.width() / 4.0).floor();
    for (i, (name, frac, value, colour)) in bars.into_iter().enumerate() {
        let col =
            Rect::from_min_size(pos2(meter_area.left() + bw * i as f32, meter_area.top()), vec2(bw, meter_area.height())).shrink2(vec2(5.0, 0.0));
        let label = Rect::from_min_max(pos2(col.left(), col.bottom() - 14.0), col.max);
        let value_r = Rect::from_min_size(col.min, vec2(col.width(), 14.0));
        let body = Rect::from_min_max(pos2(col.left(), value_r.bottom() + 4.0), pos2(col.right(), label.top()));
        let track = Rect::from_center_size(body.center(), vec2(12.0, body.height()));
        p.rect_filled(track, CornerRadius::same(3), al(Color32::BLACK, 0.35));
        let frac = if frac.is_finite() { frac } else { 0.0 };
        if frac > 0.0 {
            p.rect_filled(Rect::from_min_max(pos2(track.left(), track.bottom() - track.height() * frac), track.max), CornerRadius::same(3), colour);
        }
        p.text(value_r.center(), Align2::CENTER_CENTER, value, crate::theme::regular(10.5), pal.text_bright);
        tracked_text(p, label, Align2::CENTER_CENTER, name, 9.0, 0.08, true, false, pal.text_label);
    }
}

//! Twine's VST editor (`Twine/Source/PluginEditor.cpp`), translated: the layout of `resized()`,
//! the paint of `paint()`, `TwineDisplay` ([`display`]) and `TwineCrossover.h` (the kit's
//! [`crossover`] strip with Twine's names and defaults), and every gesture. Twine is not an Engine
//! family editor: it keeps the theme's defaults with its own straw accent (`setAccent`,
//! `textLabel = 0xFF8A96A3`), the same in both DAW themes. Its interaction inventory is
//! `docs/superpowers/editors/twine.md`.
//!
//! The meters are the engine's running maxima, taken (reset) at 30 Hz as the C++ timer's
//! `exchange (0)` (`twine::LIVE_*`); the spectrum is its input scope (`analysis().post`). The
//! VST's own Undo stack is kept in the editor; a restore is one `mix.module_params`.

mod display;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{
    DialOpts, PillLook, PillStyle, SelectionStyle, crossover_id, dial, pill_ex, preset_name_button_ex, segmented_ex, segmented_id, tip,
};
use crate::editor_kit::crossover;
use crate::editor_kit::look::tracked_text;
use crate::editor_kit::palette::al;
use crate::editor_kit::presets::{self, PresetState};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, set_params, set_params_merged};
use egui::{Align2, Color32, CornerRadius, Rect, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::twine as dsp;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

const PAD_X: f32 = 16.0;
const PAD_Y: f32 = 11.0;
const GAP: f32 = 9.0;
const HEADER_H: f32 = 25.0;
const VIEW_H: f32 = 290.0;
const CONTROLS_H: f32 = 150.0;
const FOOT_H: f32 = 20.0;
/// The crossover strip, under the display.
const STRIP_H: f32 = 58.0;
const METER_W: f32 = 190.0;
const WIDTH: f32 = 860.0;
const HEIGHT: f32 = PAD_Y + HEADER_H + GAP + VIEW_H + GAP + CONTROLS_H + GAP + FOOT_H + PAD_Y;
/// `setSize (kWidth, kHeight)`.
pub const SIZE: Vec2 = vec2(WIDTH, HEIGHT);

/// Twine's own colour: dry straw.
const ACCENT: Color32 = argb(0xFFD8B25C);
const BYPASS_RED: Color32 = argb(0xFFFF6B5E);
/// The clipper's share.
const CLIP_COL: Color32 = argb(0xFFE8894A);
/// The limiter's share.
const LIM_COL: Color32 = argb(0xFF7FB4FF);
const IN_COL: Color32 = argb(0xFF5C6670);
const PANEL_FILL: Color32 = argb(0xFF12161A);
const PANEL_LINE: Color32 = argb(0xFF1F252B);
const ROOT_BACK: Color32 = argb(0xFF101316);

/// `theme::setAccent (kAccent)` and `textLabel = 0xFF8A96A3` over the theme's defaults.
pub const PALETTE: VstPalette = VstPalette { text_label: argb(0xFF8A96A3), bypass_red: BYPASS_RED, ..VstPalette::THEME_DEFAULTS.with_accent(ACCENT) };

const XOVER: crossover::Spec = crossover::Spec { names: ["low", "mid", "high"], default_lo: 0.4106, default_hi: 0.4881 };

/// `juce::Colour::brighter (amount)`.
fn brighter(c: Color32, amount: f32) -> Color32 {
    let k = 1.0 / (1.0 + amount);
    let ch = |v: u8| (255.0 - k * (255.0 - f32::from(v))).round().clamp(0.0, 255.0) as u8;
    Color32::from_rgba_unmultiplied(ch(c.r()), ch(c.g()), ch(c.b()), c.a())
}

/// `toDb`.
fn to_db(lin: f32) -> f32 {
    if lin.is_finite() && lin > 1.0e-6 { 20.0 * lin.log10() } else { -120.0 }
}

/// `characterNote`.
fn character_note(c: i32) -> &'static str {
    match c {
        1 => "Punchy: lets go fast between hits, so the drums keep their bounce.",
        2 => "Loud: a later attack and a quick release. Louder, and dirtier when pushed.",
        _ => "Transparent: 5 ms lookahead and your release. The limiter you do not hear.",
    }
}

/// A meter: what it shows (falling slowly) and a peak hold for the number.
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
        let v = if v.is_finite() { v } else { self.now };
        self.now = if v > self.now { v } else { self.now - fall };
        // the number holds the highest for a second and a half, then follows again
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

/// `snapshot`: every parameter.
fn snapshot(host: &Host) -> Snapshot {
    host.info.params.iter().map(|p| (p.id, param(host, p.id))).collect()
}

/// Twine's editor: everything the C++ editor holds that is not a parameter.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    undo: Vec<Snapshot>,
    history: display::History,
    meters: [Meter; 4],
    band_gr: [f32; 3],
    tick_at: f64,
    spectrum: crossover::Spectrum,
    xover_drag: Option<usize>,
    presets: PresetState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
}

/// Draws Twine's editor for `module` on `track`.
/// What `getPresetParams` leaves out besides `bypass` (Twine's `Presets.h`).
const NOT_IN_PRESETS: [&str; 2] = ["match", "chaff"];

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_twine";

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

    /// The 30 Hz `timerCallback`: take the meters, push a display column, feed the bars, the band
    /// reductions and the spectrum.
    fn tick(&mut self, host: &Host) {
        let live = crate::editor_kit::live(host);
        let take = |i: usize| live.map_or(0.0, |l| l.take(i));
        let input = to_db(take(dsp::LIVE_IN_PEAK));
        let out = to_db(take(dsp::LIVE_OUT_TP));
        let clip = take(dsp::LIVE_CLIP_GR);
        let lim = take(dsp::LIVE_LIM_GR);
        // the display shows the input AFTER Drive: what Twine was handed
        let driven = input + dsp::knob_to_drive_db(param(host, "drive"));
        self.history.push(display::Column { in_db: driven, out_db: out, clip_db: clip, lim_db: lim });
        self.meters[0].feed(input, 0.8);
        self.meters[3].feed(out, 0.8);
        // gain reduction meters show dB taken off, so "rising" is more reduction
        self.meters[1].feed(clip, 0.6);
        self.meters[2].feed(lim, 0.6);
        for (i, g) in self.band_gr.iter_mut().enumerate() {
            let v = take(dsp::LIVE_BAND_GR + i);
            *g = if v.is_finite() { v } else { 0.0 };
        }
        let ring = host.meters.and_then(|m| m.analysis.iter().find(|(id, _)| *id == host.ins.id)).map(|(_, a)| a.clone());
        if let Some(a) = ring
            && a.post.written() > 0
        {
            self.spectrum.update(&a.post, host.sr);
        }
    }
}

fn pill(ui: &mut Ui, r: Rect, salt: &str, text: &str, on: bool, dimmed: bool, lit: Option<Color32>) -> egui::Response {
    pill_ex(ui, r, salt, &PALETTE, text, on, PillStyle::Small, PillLook { active_colour: lit, tracked: true, dimmed })
}

fn panel(p: &egui::Painter, a: Rect) {
    p.rect_filled(a, CornerRadius::same(5), PANEL_FILL);
    p.rect_stroke(a.shrink(0.5), CornerRadius::same(5), Stroke::new(1.0, PANEL_LINE), StrokeKind::Middle);
}

// ============================================================================ the editor
/// What both layouts do before they paint: the scripted drawer tabs, the 30 Hz timer, and the
/// preset browser, which covers the whole editor while it is open (then `true`).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    let ed = host.rect;
    if !st.init {
        st.init = true;
        st.presets.category = "All".into();
    }
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.presets),
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

    // ---- the preset browser covers the whole editor while it is open; a recall is undoable
    // with the VST's Undo (`loadPreset` pushes first)
    st.presets.not_in_presets = &NOT_IN_PRESETS;
    if st.presets.open {
        let before = snapshot(host);
        let out = presets::preset_browser(app, ui, host, ed, &PALETTE, &mut st.presets);
        let pasted = presets::clipboard(app, ui, host, &mut st.presets, &out);
        if out.recalled.is_some() {
            end_gesture(app);
        }
        if out.recalled.is_some() || pasted.is_some() {
            st.push_undo(before);
        }
        return true;
    }

    false
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    if prelude(st, app, ui, host) {
        return;
    }
    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, ROOT_BACK);
    let r = ed.shrink2(vec2(PAD_X, PAD_Y));
    let header = Rect::from_min_size(r.min, vec2(r.width(), HEADER_H));
    let view = Rect::from_min_size(pos2(r.left(), header.bottom() + GAP), vec2(r.width(), VIEW_H));
    let controls = Rect::from_min_size(pos2(r.left(), view.bottom() + GAP), vec2(r.width(), CONTROLS_H));
    let foot = Rect::from_min_size(pos2(r.left(), controls.bottom() + GAP), vec2(r.width(), FOOT_H));

    header_ui(st, app, ui, host, header);

    // ---- the view: display, crossover strip, meters
    let meter_area = Rect::from_min_max(pos2(view.right() - METER_W, view.top()), view.max);
    let left = Rect::from_min_max(view.min, pos2(meter_area.left() - GAP, view.bottom()));
    let strip = Rect::from_min_max(pos2(left.left(), left.bottom() - STRIP_H), left.max);
    let disp = Rect::from_min_max(left.min, pos2(left.right(), strip.top() - 6.0));
    let ceiling_db = dsp::knob_to_ceiling_db(param(host, "ceiling"));
    display::paint(&p, disp, &mut st.history, ceiling_db);
    crossover_ui(st, app, ui, host, strip);
    meters_paint(st, &p, meter_area, ceiling_db);

    // ---- controls
    panel(&p, controls);
    let c = controls.shrink2(vec2(12.0, 10.0));
    let dial_w = 112.0;
    for (i, id) in ["drive", "clip", "ceiling", "release"].into_iter().enumerate() {
        let dr = Rect::from_min_size(pos2(c.left() + dial_w * i as f32, c.top()), vec2(dial_w, 126.0));
        if let Some(resp) = knob(st, app, ui, host, dr, id) {
            let _ = resp.on_hover_text(tooltip(id));
        }
    }
    let col = Rect::from_min_max(pos2(c.left() + 4.0 * dial_w + 18.0, c.top()), c.max);
    let seg_w = col.width().min(270.0);
    let shape_label = Rect::from_min_size(col.min, vec2(col.width(), 14.0));
    let shape_r = Rect::from_min_size(pos2(col.left(), shape_label.bottom()), vec2(seg_w, 26.0));
    let char_label = Rect::from_min_size(pos2(col.left(), shape_r.bottom() + 10.0), vec2(col.width(), 14.0));
    let char_r = Rect::from_min_size(pos2(col.left(), char_label.bottom()), vec2(seg_w, 26.0));
    let pills = Rect::from_min_size(pos2(col.left(), char_r.bottom() + 14.0), vec2(col.width(), 24.0));
    tracked_text(&p, shape_label, Align2::LEFT_CENTER, "Clip shape", 9.5, 0.1, true, false, PALETTE.text_label);
    tracked_text(&p, char_label, Align2::LEFT_CENTER, "Limiter", 9.5, 0.1, true, false, PALETTE.text_label);
    for (id, rr) in [("shape", shape_r), ("character", char_r)] {
        seg_ui(st, app, ui, host, rr, id);
    }
    let mut x = pills.left();
    for (id, w) in [("match", 80.0), ("chaff", 80.0), ("multi", 96.0)] {
        toggle_ui(app, ui, host, Rect::from_min_size(pos2(x, pills.top()), vec2(w, 24.0)), id);
        x += w + 8.0;
    }

    // ---- what the limiter character does, in a line
    tracked_text(
        &p,
        foot,
        Align2::LEFT_CENTER,
        character_note(param(host, "character").round() as i32),
        10.0,
        0.03,
        false,
        false,
        PALETTE.text_faint,
    );
}

/// The header: TWINE, "clip + limit", the preset name, < >, Undo, Bypass.
fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, h: Rect) {
    let p = ui.painter().clone();
    tracked_text(&p, Rect::from_min_size(h.min, vec2(70.0, h.height())), Align2::LEFT_CENTER, "TWINE", 15.0, 0.24, true, true, PALETTE.text_bright);
    let sub = Rect::from_min_size(pos2(h.left() + 74.0, h.top()), vec2(110.0, h.height()));
    tracked_text(&p, sub, Align2::LEFT_CENTER, "clip + limit", 10.0, 0.12, true, false, ACCENT);
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
    let name_r = Rect::from_min_max(pos2(h.left() + 190.0, h.top() + 1.0), pos2(right, h.bottom() - 1.0));
    header_controls(st, app, ui, host, &HeaderRects { bypass: bypass_r, undo: undo_r, prev: prev_r, next: next_r, name: name_r });
}

/// Where the header's controls go (each layout places them).
struct HeaderRects {
    bypass: Rect,
    undo: Rect,
    prev: Rect,
    next: Rect,
    name: Rect,
}

/// The header's gestures: Bypass, Undo, < > (`step`, pushing the VST's Undo) and the name.
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: &HeaderRects) {
    let (bypass_r, undo_r, prev_r, next_r, name_r) = (r.bypass, r.undo, r.prev, r.next, r.name);

    let byp = param(host, "bypass") > 0.5;
    if pill(ui, bypass_r, "bypass", "Bypass", byp, false, Some(BYPASS_RED)).clicked() {
        set_params(app, host, &[("bypass", if byp { 0.0 } else { 1.0 })]);
    }
    if pill(ui, undo_r, "undo", "Undo", false, st.undo.is_empty(), None).clicked()
        && let Some(v) = st.undo.pop()
    {
        end_gesture(app);
        set_params(app, host, &v);
    }
    for (r, dir, text) in [(prev_r, -1, "<"), (next_r, 1, ">")] {
        if pill(ui, r, if dir < 0 { "prev" } else { "next" }, text, false, false, None).clicked() {
            // `step` walks every preset (`search ({})`), whatever the browser's category
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
    if preset_name_button_ex(ui, name_r, "name", &PALETTE, presets::shown_name(host), dirty).clicked() {
        presets::open(app, host, &mut st.presets);
    }
}

/// The crossover strip (`TwineCrossover`) in `r` and its drags.
fn crossover_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, strip: Rect) {
    let xv = crossover::View {
        spec: XOVER,
        lo_knob: param(host, "xoverLo"),
        hi_knob: param(host, "xoverHi"),
        active: param(host, "multi") > 0.5,
        band_gr: st.band_gr,
        spectrum: &st.spectrum,
    };
    let mut drag = st.xover_drag;
    let out = crossover::crossover(ui, strip, &PALETTE, &xv, &mut drag);
    tip(ui, crossover_id(ui), "Multiband crossovers. Drag a divider to move it, scroll to nudge, double-click to reset.");
    st.xover_drag = drag;
    if out.gesture_start.is_some() {
        st.push_undo(snapshot(host));
        end_gesture(app);
    }
    if let Some((d, v)) = out.value {
        set_params_merged(app, host, &[(if d == 0 { "xoverLo" } else { "xoverHi" }, v)], &format!("xover-{d}"));
    }
    if out.gesture_end.is_some() {
        end_gesture(app);
    }
}

/// Clip shape (`shape`) or the limiter's character (`character`) in `r`: a pick pushes the VST's
/// Undo and is one step.
fn seg_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str) {
    let opts: &[&str] = if id == "shape" { &["Hard", "Soft", "Tape"] } else { &["Transparent", "Punchy", "Loud"] };
    let sel = usize::try_from(param(host, id).round() as i64).ok();
    if let Some(i) = segmented_ex(ui, r, id, &PALETTE, opts, 1, sel, SelectionStyle::Filled, true) {
        st.push_undo(snapshot(host));
        set_params(app, host, &[(id, i as f32)]);
    }
    tip(ui, segmented_id(ui, id), tooltip(id));
}

/// Match, Chaff (lit in the clipper's colour) or Multiband in `r`.
fn toggle_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str) {
    let (text, lit) = match id {
        "match" => ("Match", None),
        "chaff" => ("Chaff", Some(CLIP_COL)),
        _ => ("Multiband", None),
    };
    let on = param(host, id) > 0.5;
    let resp = pill(ui, r, id, text, on, false, lit);
    if resp.clicked() {
        set_params(app, host, &[(id, if on { 0.0 } else { 1.0 })]);
    }
    let _ = resp.on_hover_text(tooltip(id));
}

/// The editor's `setTooltip`s, by parameter.
fn tooltip(id: &str) -> &'static str {
    match id {
        "drive" => "How hard the master is pushed in. Everything above the ceiling is what Twine has to take off.",
        "clip" => {
            "How the work is shared. 0: the limiter does it all. Higher: the clipper shaves the tips first, with no ducking, and the              limiter catches what is left. 100 %: the clipper sits at the ceiling."
        }
        "ceiling" => {
            "The highest the output may go, as a TRUE peak -- between the samples too. -1 dBTP is the usual limit for streaming;              lower still if it will be encoded to MP3 or AAC."
        }
        "release" => "How fast the limiter lets go. Short is louder and can pump; long is smoother.",
        "shape" => {
            "The clipper's corner. Hard: the most level off for the least distortion on short peaks. Soft: rounds into it. Tape:              rounder still, and warmer when pushed."
        }
        "character" => "How the limiter behaves. Transparent, Punchy (fast between hits) or Loud (late attack, quick release).",
        "match" => "Turn the output down by the Drive, so you hear what the processing does rather than just 'louder'.",
        "multi" => {
            "Split the limiter into three bands, so a loud kick is limited in the lows and does not pull the vocal and the cymbals              down with it. The full-band limiter still guarantees the ceiling. Adds about 5 ms of latency."
        }
        "chaff" => "Hear only what is being knocked off. If you can hear the music in it, it is doing too much.",
        _ => "",
    }
}

/// A dial (`attachDial`): `pushUndo` on the drag start, the parameter's own readout.
fn knob(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str) -> Option<egui::Response> {
    let info = host.info.params.iter().find(|p| p.id == id)?;
    let value = param(host, id);
    let text = info.format(value);
    let o = DialOpts { label: info.name, value_text: &text, default: info.to_normalized(info.default), tracked: true, ..DialOpts::default() };
    let out = dial(ui, r, id, &PALETTE, info.to_normalized(value), &o);
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
    Some(out.response)
}

/// The four bars: In, Clip, Limit, Out TP.
fn meters_paint(st: &Editor, p: &egui::Painter, area: Rect, ceiling_db: f32) {
    panel(p, area);
    let m = area.shrink2(vec2(12.0, 10.0));
    let bars = [("In", brighter(IN_COL, 0.6), false), ("Clip", CLIP_COL, true), ("Limit", LIM_COL, true), ("Out TP", ACCENT, false)];
    let bw = (m.width() / 4.0).floor();
    for (i, (name, colour, reduction)) in bars.into_iter().enumerate() {
        let col = Rect::from_min_size(pos2(m.left() + bw * i as f32, m.top()), vec2(bw, m.height())).shrink2(vec2(6.0, 0.0));
        let meter = st.meters[i];
        let label = Rect::from_min_max(pos2(col.left(), col.bottom() - 14.0), col.max);
        let value = Rect::from_min_size(col.min, vec2(col.width(), 14.0));
        let body = Rect::from_min_max(pos2(col.left(), value.bottom() + 4.0), pos2(col.right(), label.top()));
        let track = Rect::from_center_size(body.center(), vec2(12.0, body.height()));
        p.rect_filled(track, CornerRadius::same(3), al(Color32::BLACK, 0.35));
        let frac = if reduction { (meter.now / 12.0).clamp(0.0, 1.0) } else { ((meter.now + 48.0) / 48.0).clamp(0.0, 1.0) };
        let fill = if reduction {
            Rect::from_min_size(track.min, vec2(track.width(), track.height() * frac))
        } else {
            Rect::from_min_max(pos2(track.left(), track.bottom() - track.height() * frac), track.max)
        };
        if frac > 0.0 {
            p.rect_filled(fill, CornerRadius::same(3), colour);
        }
        if i == 3 {
            // the ceiling, on the output meter
            let y = (track.bottom() - track.height() * ((ceiling_db + 48.0) / 48.0).clamp(0.0, 1.0)).round() + 0.5;
            p.line_segment([pos2(track.left() - 4.0, y), pos2(track.right() + 4.0, y)], Stroke::new(1.0, ACCENT));
        }
        let h = meter.hold;
        let txt = if reduction {
            if h < 0.05 { "0.0".to_string() } else { format!("-{h:.1}") }
        } else if h <= -119.0 {
            "-inf".to_string()
        } else {
            format!("{h:.1}")
        };
        let ink = if i == 3 && h > ceiling_db + 0.05 { BYPASS_RED } else { PALETTE.text_bright };
        p.text(value.center(), Align2::CENTER_CENTER, txt, crate::theme::regular(10.5), ink);
        tracked_text(p, label, Align2::CENTER_CENTER, name, 9.0, 0.08, true, false, PALETTE.text_label);
    }
}

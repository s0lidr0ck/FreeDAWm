//! ChamberMade's VST editor (`ChamberMade/Source/PluginEditor.cpp`), translated: the shipping
//! build's layout at 1190 x 648 — the wordmark and preset header, the plan on the left with its
//! Add row ([`plan`], `ChamberPlan.h`), the inspector (Room, Speaker, Microphone A / B or an
//! object) on the right with Mix and Decay under it, and More as a card over the plan (sound
//! paths, ft / m, the echogram and fifteen more knobs). ChamberMade's own look (`cm::`), the same
//! in both DAW themes; its knobs wear the DAW's knob face (the owner's rule). Its interaction
//! inventory is `docs/superpowers/editors/chamber.md`.
//!
//! The editor's UI state (`uiState`: More open, sound paths, ft / m, the inspector's target) is
//! kept in the module's state, as the plugin's state keeps it; Undo (Cmd+Z) is the editor's own
//! 32-step stack of every parameter.

mod controls;
mod plan;
mod slim;

use crate::SoundApp;
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::{Host, VstEditor, VstPalette, end_gesture, param, presets, set_params, set_params_merged};
use controls::{ButtonLook, KnobOpts, chamber_button, chamber_knob, cm, dropdown, fitted_text, led, preset_field, tri_side, ui_font};
use egui::{Align2, Color32, Id, Rect, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::fs::chamber::chamber_geometry::{Chamber, object_is_flat};
use soundcraft_dsp::plugins::fs::chamber::chamber_made_dsp::{self as d, NUM_OBJECTS, NUM_PARAMS};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

// ============================================================================ constants

const K_DEFAULT_WIDTH: f32 = 1190.0;
const K_DEFAULT_HEIGHT: f32 = 648.0;
const K_PAD: f32 = 14.0;
const K_HEADER_Y: f32 = 16.0;
const K_HEADER_H: f32 = 40.0;
const K_BODY_Y: f32 = 66.0;
const K_GAP: f32 = 14.0;
const K_RIGHT_W: f32 = 274.0;
const K_MIX_H: f32 = 137.0;
const K_ROW_H: f32 = 36.0;
const K_ROW_PITCH: f32 = 46.0;
const K_LABEL_W: f32 = 68.0;
const K_CELL_W: f32 = 72.0;
const K_CELL_H: f32 = 100.0;

/// `setSize (kDefaultWidth, kDefaultHeight)`.
pub const SIZE: Vec2 = vec2(K_DEFAULT_WIDTH, K_DEFAULT_HEIGHT);

/// The kit palette (the browser draws in it): `cm::`'s navy and orange.
pub const PALETTE: VstPalette = VstPalette {
    accent: cm::ACCENT,
    background: cm::BACKGROUND,
    panel: cm::PANEL,
    panel_alt: cm::GROUP,
    border: cm::PANEL_LINE,
    border_hover: cm::FIELD_LINE_HOVER,
    text_bright: cm::TEXT,
    text_body: cm::LABEL,
    text_dim: cm::LABEL,
    text_label: cm::LABEL,
    text_faint: cm::FAINT,
    on_accent: cm::ON_ACCENT,
    field: cm::FIELD,
    ..VstPalette::THEME_DEFAULTS
};

/// `PresetCatalogue`: a preset carries the room and its voicing, not Mix, Level or Bypass.
const FAMILY: Family = Family::new("ChamberMade", &["mix", "level"]);

/// The undo stack's cap (`undoStack.size() > 32`).
const UNDO_DEPTH: usize = 32;

pub(crate) fn obj_type_param(o: usize) -> usize {
    d::P_OBJ1_TYPE + 4 * o
}
pub(crate) fn obj_x_param(o: usize) -> usize {
    d::P_OBJ1_TYPE + 4 * o + 1
}
pub(crate) fn obj_y_param(o: usize) -> usize {
    d::P_OBJ1_TYPE + 4 * o + 2
}
pub(crate) fn obj_size_param(o: usize) -> usize {
    d::P_OBJ1_TYPE + 4 * o + 3
}
pub(crate) fn obj_rotate_param(o: usize) -> usize {
    d::P_OBJ1_ROT + o
}

fn id_of(i: usize) -> &'static str {
    soundcraft_dsp::plugins::fs::chamber::CHAMBER_PARAMS.get(i).map_or("", |p| p.id)
}

fn choices(i: usize) -> &'static [&'static str] {
    soundcraft_dsp::plugins::fs::chamber::CHAMBER_PARAMS.get(i).map_or(&[], |p| p.choices)
}

// ============================================================================ formatParam

/// `formatParam`: what a control says, from the room the parameters make.
fn format_param(index: usize, v: f32, ch: &Chamber, metric: bool) -> String {
    let round = |x: f32| (x + 0.5).floor() as i32;
    let pct = |x: f32| format!("{} %", round(x * 100.0));
    let decibels = |g: f32| {
        let db = if g > 1.0e-6 { 20.0 * g.log10() } else { -120.0 };
        if db <= -90.0 {
            "off".to_string()
        } else if db.abs() < 0.05 {
            "0.0 dB".to_string()
        } else {
            format!("{}{db:.1} dB", if db > 0.0 { "+" } else { "" })
        }
    };
    let len = |m: f32| plan::length_string(m, false, metric);
    match index {
        d::P_MIX | d::P_DRIVE | d::P_DRIFT | d::P_TAPER | d::P_TAPE_SEND => pct(v),
        d::P_WIDTH => format!("{} %", round(v * 200.0)),
        d::P_TAPE_REPEATS => format!("{} %", round(v * 90.0)),
        d::P_DECAY => format!("\u{d7}{:.2} \u{b7} {:.1} s", d::knob_to_decay_mul(v), ch.acoustics.rt60.mid),
        d::P_PRE_DELAY => format!("{} ms", round(d::knob_to_pre_delay_ms(v))),
        d::P_EARLY | d::P_TAIL => decibels(d::knob_to_early_gain(v)),
        d::P_LEVEL => decibels(d::knob_to_level_gain(v)),
        d::P_LOW_CUT => format!("{} Hz", round(d::knob_to_low_cut_hz(v))),
        d::P_HIGH_CUT => {
            let f = d::knob_to_high_cut_hz(v);
            if f >= 10000.0 { format!("{:.1} k", f / 1000.0) } else { format!("{:.2} k", f / 1000.0) }
        }
        d::P_ROOM_W => len(ch.room.width_m),
        d::P_ROOM_L => len(ch.room.length_m),
        d::P_ROOM_H => len(ch.room.height_m),
        d::P_CEIL_SLOPE => {
            if v < 0.01 {
                "flat".into()
            } else {
                format!("{}\u{2192}{}", plan::length_string(ch.room.ceiling_at(0.0), true, metric), len(ch.room.ceiling_at(ch.room.length_m)))
            }
        }
        d::P_SPK_X => len(ch.placement.speaker.x),
        d::P_SPK_Y => len(ch.placement.speaker.y),
        d::P_SPK_Z => format!("{} up", len(ch.placement.speaker.z)),
        d::P_MIC_AX => len(ch.placement.mic_a.x),
        d::P_MIC_AY => len(ch.placement.mic_a.y),
        d::P_MIC_BX => len(ch.placement.mic_b.x),
        d::P_MIC_BY => len(ch.placement.mic_b.y),
        d::P_MIC_Z => format!("{} up", len(ch.placement.mic_a.z)),
        d::P_SPK_AIM | d::P_MIC_A_AIM | d::P_MIC_B_AIM => format!("{}\u{b0}", round(v * 360.0) % 360),
        d::P_TAPE_TIME => format!("{} ms", round(d::knob_to_tape_ms(v))),
        d::P_TAPE_TONE => format!("{:.1} k", d::knob_to_tape_tone_hz(v) / 1000.0),
        _ => {
            for o in 0..NUM_OBJECTS {
                let obj = &ch.objects[o];
                if index == obj_size_param(o) {
                    return if obj.on { format!("\u{d8} {}", len(obj.radius * 2.0)) } else { pct(v) };
                }
                if index == obj_rotate_param(o) {
                    return format!("{}\u{b0}", round(v * 180.0) % 180);
                }
                if index == obj_x_param(o) || index == obj_y_param(o) {
                    return if obj.on { len(if index == obj_x_param(o) { obj.pos.x } else { obj.pos.y }) } else { pct(v) };
                }
            }
            let c = choices(index);
            if !c.is_empty() { c.get(v.round().max(0.0) as usize).copied().unwrap_or("").to_string() } else { format!("{v:.2}") }
        }
    }
}

// ============================================================================ the UI state

/// The plugin's `uiState`: More open, sound paths, ft / m, the inspector's target.
#[derive(Clone, Copy, Debug, PartialEq)]
struct UiState {
    more: bool,
    rays: bool,
    metric: bool,
    inspect: i32,
}

impl Default for UiState {
    fn default() -> Self {
        UiState { more: false, rays: true, metric: false, inspect: plan::SEL_ROOM }
    }
}

fn read_ui_state(ins: &Insert) -> UiState {
    let def = UiState::default();
    let v = crate::editor_kit::ui_state(ins);
    let b = |k: &str, d: bool| v.get(k).and_then(Value::as_bool).unwrap_or(d);
    let inspect = v.get("inspect").and_then(Value::as_i64).map_or(def.inspect, |i| i.clamp(-1, 2 + NUM_OBJECTS as i64) as i32);
    UiState { more: b("more", def.more), rays: b("rays", def.rays), metric: b("metric", def.metric), inspect }
}

/// `uiState.setProperty`: kept in the module's state, outside the undo history (as the C++).
fn write_ui_state(app: &mut SoundApp, host: &Host, s: UiState) {
    let entries = [("more", json!(s.more)), ("rays", json!(s.rays)), ("metric", json!(s.metric)), ("inspect", json!(s.inspect))];
    crate::editor_kit::set_ui_state(app, host, &entries);
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    plan: plan::PlanState,
    undo: Vec<Vec<f32>>,
    /// The Add counter flashes until then (`addWarnTicks`).
    add_warn_until: f64,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's one display shows the echogram (More's) rather than the plan.
    slim_echo: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_chamber";

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

    /// `keyPressed`: Cmd+Z is the editor's undo.
    fn owns_key(key: egui::Key, mods: egui::Modifiers) -> bool {
        key == egui::Key::Z && mods.command && !mods.shift
    }
}

fn values(host: &Host) -> Vec<f32> {
    (0..NUM_PARAMS).map(|i| param(host, id_of(i))).collect()
}

/// `pushUndo`: every parameter.
fn push_undo(st: &mut Editor, host: &Host) {
    st.undo.push(values(host));
    if st.undo.len() > UNDO_DEPTH {
        st.undo.remove(0);
    }
}

/// `undo` / `restore`: one change.
fn undo(st: &mut Editor, app: &mut SoundApp, host: &Host) {
    if let Some(v) = st.undo.pop() {
        let list: Vec<(&str, f32)> = v.iter().enumerate().map(|(i, x)| (id_of(i), *x)).collect();
        end_gesture(app);
        set_params(app, host, &list);
    }
}

/// `setInspectorTarget` + `plan->setSelected`.
fn select(app: &mut SoundApp, host: &Host, ui_state: &mut UiState, t: i32) {
    if ui_state.inspect != t {
        ui_state.inspect = t;
        write_ui_state(app, host, *ui_state);
    }
}

/// `showInspector`'s check: an object target whose slot is empty falls back to the Room.
fn valid_target(host: &Host, t: i32) -> i32 {
    let slot = t - plan::SEL_OBJECT0;
    if slot >= 0 && (slot as usize) < NUM_OBJECTS && param(host, id_of(obj_type_param(slot as usize))) < 0.5 { plan::SEL_ROOM } else { t }
}

/// What both layouts do first: the scripted drawer tabs, Cmd+Z, and the preset browser, which
/// covers the whole editor while it is open (then `None`: paint nothing else). Returns the
/// editor's UI state.
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> Option<UiState> {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
        st.fam.presets.quiet = true;
        st.fam.presets.not_in_presets = FAMILY.not_in_presets;
    }
    let mut ui_state = read_ui_state(host.ins);
    // `ui.mix.drawer {tab: "More"}` opens More in the slim layout (the full editor's More stays
    // its header button's).
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") && st.slim && !ui_state.more {
        ui_state.more = true;
        write_ui_state(app, host, ui_state);
    }
    let full = host.rect;

    // Cmd+Z (`keyPressed`), while the editor is under the pointer.
    if ui.rect_contains_pointer(full) && ui.input_mut(|i| i.consume_key(egui::Modifiers::COMMAND, egui::Key::Z)) {
        undo(st, app, host);
    }

    // The browser over the whole editor (a recall is pushed on the undo stack first, as
    // `loadPreset` does).
    if st.fam.presets.open {
        let before = values(host);
        let preset_before = host.ins.preset.clone();
        let _ = family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "");
        if app.engine.session().track(host.track).and_then(|t| t.module(host.ins.id)).is_some_and(|i| i.preset != preset_before) {
            st.undo.push(before);
        }
        return None;
    }
    Some(ui_state)
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let Some(mut ui_state) = prelude(st, app, ui, host) else { return };
    let full = host.rect;
    let o = full.min;
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(o.x + x, o.y + y), vec2(w, h));
    let metric = ui_state.metric;
    let knobs = values(host);
    let ch = crate::chamber_plan::chamber_for(ui.ctx(), Id::new(("chamber-editor", host.track.0, host.ins.id.0)), host.ins);
    let target = valid_target(host, ui_state.inspect);
    let p = ui.painter().clone();
    p.rect_filled(full, egui::CornerRadius::ZERO, cm::BACKGROUND);

    // ---- resized()
    let (w, h) = (full.width(), full.height());
    let header = at(K_PAD, K_HEADER_Y, w - 2.0 * K_PAD, K_HEADER_H);
    let bypass_r = Rect::from_min_max(pos2(header.right() - 71.0, header.top()), header.max);
    let led_r = Rect::from_min_max(pos2(bypass_r.left() - 6.0 - 18.0, header.top()), pos2(bypass_r.left() - 6.0, header.bottom()));
    let more_r = Rect::from_min_max(pos2(led_r.left() - 4.0 - 61.0, header.top()), pos2(led_r.left() - 4.0, header.bottom()));
    let next_r = Rect::from_min_max(pos2(more_r.left() - 10.0 - 42.0, header.top()), pos2(more_r.left() - 10.0, header.bottom()));
    let prev_r = Rect::from_min_size(pos2(header.left() + 179.0, header.top()), vec2(42.0, K_HEADER_H));
    let field_r = Rect::from_min_max(pos2(prev_r.right() + 10.0, header.top()), pos2(next_r.left() - 10.0, header.bottom()));
    let body = at(K_PAD, K_BODY_Y, w - 2.0 * K_PAD, h - K_BODY_Y - 7.0);
    let right = Rect::from_min_max(pos2(body.right() - K_RIGHT_W, body.top()), body.max);
    let plan_r = Rect::from_min_max(body.min, pos2(right.left() - K_GAP, body.bottom()));
    let mix_area = Rect::from_min_max(pos2(right.left(), right.bottom() - K_MIX_H), right.max);
    let inspect_area = Rect::from_min_max(right.min, pos2(right.right(), mix_area.top() - 10.0));
    let mx = mix_area.shrink2(vec2(5.0, 6.0));
    let cw = (mx.width() / 2.0).trunc();
    let mix_r = Rect::from_min_size(mx.min, vec2(cw, mx.height()));
    let decay_r = Rect::from_min_max(pos2(mx.left() + cw, mx.top()), mx.max);

    // ---- paint()
    controls::draw_panel(&p, inspect_area, cm::PANEL, cm::PANEL_LINE, 6.0);
    controls::draw_panel(&p, mix_area, cm::PANEL, cm::PANEL_LINE, 6.0);
    {
        let w1 = controls::text_width(&p, "Chamber", ui_font(24.0, true));
        p.text(pos2(header.left(), header.center().y), Align2::LEFT_CENTER, "Chamber", ui_font(24.0, true), cm::TEXT);
        p.text(
            pos2(header.left() + w1 + 1.0, header.center().y),
            Align2::LEFT_CENTER,
            "Made",
            ui_font(24.0, false),
            crate::editor_kit::argb(0xFF8FA0B0),
        );
    }

    // ---- the header
    let rects = HeaderRects { prev: prev_r, next: next_r, field: field_r, more: more_r, led: led_r, bypass: bypass_r };
    header_controls(st, app, ui, host, &rects, &mut ui_state, 13.0);

    // ---- the plan (its Add row and the flutter readout)
    let blocked = ui_state.more;
    plan_ui(st, app, ui, host, &ch, plan_r, target, &knobs, &mut ui_state, blocked, false);

    // ---- the inspector
    let target = valid_target(host, ui_state.inspect);
    inspector(st, app, ui, host, &ch, inspect_area, target, &mut ui_state, metric, &FULL);

    // ---- Mix and Decay
    mix_decay(st, app, ui, host, &ch, metric, mix_r, decay_r);

    // ---- More, over the plan
    if ui_state.more {
        more_card(st, app, ui, host, &ch, plan_r, &mut ui_state, metric);
    }
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// Where the header's controls go (each layout places them).
struct HeaderRects {
    prev: Rect,
    next: Rect,
    field: Rect,
    more: Rect,
    led: Rect,
    bypass: Rect,
}

/// The header's controls and their gestures: the preset steppers and field, More, the LED and
/// Bypass (`font_h`: the buttons' type).
#[allow(clippy::too_many_arguments)]
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: &HeaderRects, ui_state: &mut UiState, font_h: f32) {
    let p = ui.painter().clone();
    for (rect, left) in [(r.prev, true), (r.next, false)] {
        let resp = chamber_button(ui, rect, ("chamber-step", left, host.ins.id.0), "", ButtonLook { font_h, ..ButtonLook::default() });
        tri_side(&p, rect.center(), left, cm::LABEL_BRIGHT);
        if resp.clicked() {
            let before = values(host);
            let out = presets::step_all(app, host, &mut st.fam.presets, if left { -1 } else { 1 });
            if out.recalled.is_some() {
                end_gesture(app);
                st.undo.push(before);
            }
        }
    }
    let dirty = presets::is_dirty(app, host, &mut st.fam.presets);
    if preset_field(ui, r.field, presets::shown_name(host), dirty).clicked() {
        presets::open(app, host, &mut st.fam.presets);
    }
    let more_look = ButtonLook { active: ui_state.more, ..ButtonLook::default() };
    if chamber_button(ui, r.more, ("chamber-more", host.ins.id.0), "More", more_look).clicked() {
        toggle_more(app, host, ui_state);
    }
    let bypassed = param(host, "bypass") > 0.5;
    led(&p, r.led, bypassed);
    if chamber_button(
        ui,
        r.bypass,
        ("chamber-bypass", host.ins.id.0),
        "Bypass",
        ButtonLook { always_filled: true, fill: cm::BYPASS, ..ButtonLook::default() },
    )
    .clicked()
    {
        set_params(app, host, &[("bypass", if bypassed { 0.0 } else { 1.0 })]);
    }
}

/// More opens or closes (`uiState`).
fn toggle_more(app: &mut SoundApp, host: &Host, ui_state: &mut UiState) {
    ui_state.more = !ui_state.more;
    write_ui_state(app, host, *ui_state);
}

/// The plan in `r` and what its gestures do (`blocked`: something lies over it).
#[allow(clippy::too_many_arguments)]
fn plan_ui(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    ch: &Chamber,
    plan_r: Rect,
    target: i32,
    knobs: &[f32],
    ui_state: &mut UiState,
    blocked: bool,
    compact: bool,
) {
    let flutter = host.meters.and_then(|m| m.modules.iter().find(|mm| mm.module == host.ins.id)).map_or(0.0, |mm| mm.readouts[4]);
    let now = ui.input(|i| i.time);
    st.plan.add_warn = now < st.add_warn_until;
    let pout = plan::plan(
        ui,
        plan_r,
        ch,
        &mut st.plan,
        target,
        ui_state.rays,
        ui_state.metric,
        if flutter.is_finite() { flutter } else { 0.0 },
        knobs,
        blocked,
        compact,
    );
    if pout.gesture_start {
        end_gesture(app);
        push_undo(st, host);
    }
    if !pout.set.is_empty() {
        let list: Vec<(&str, f32)> = pout.set.iter().map(|(i, v)| (id_of(*i), *v)).collect();
        set_params_merged(app, host, &list, "chamber-plan");
    }
    if pout.gesture_end {
        end_gesture(app);
    }
    if let Some(s) = pout.select {
        select(app, host, ui_state, s);
    }
    if let Some(ty) = pout.add_object {
        add_object(st, app, host, ui_state, ty, now);
    }
    if st.plan.add_warn {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
    // The flutter readout comes off the DSP (`timerCallback`, 20 Hz).
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
}

/// Mix and Decay, the large knobs under the inspector.
#[allow(clippy::too_many_arguments)]
fn mix_decay(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, ch: &Chamber, metric: bool, mix_r: Rect, decay_r: Rect) {
    for (r, i, label) in [(mix_r, d::P_MIX, "Mix"), (decay_r, d::P_DECAY, "Decay")] {
        knob_param(
            st,
            app,
            ui,
            host,
            r,
            i,
            ch,
            metric,
            KnobOpts { label, arc: cm::ACCENT, label_colour: cm::LABEL_BLUE, large: true, ..KnobOpts::default() },
        );
    }
}

/// `addObject`: the first free slot, a small offset per slot so new things do not stack; all
/// eight in use: the counter flashes.
fn add_object(st: &mut Editor, app: &mut SoundApp, host: &Host, ui_state: &mut UiState, ty: i32, now: f64) {
    let Some(k) = (0..NUM_OBJECTS).find(|o| param(host, id_of(obj_type_param(*o))) < 0.5) else {
        st.add_warn_until = now + 18.0 / 20.0;
        return;
    };
    push_undo(st, host);
    end_gesture(app);
    set_params(
        app,
        host,
        &[
            (id_of(obj_x_param(k)), 0.5 + ((k % 3) as f32 - 1.0) * 0.12),
            (id_of(obj_y_param(k)), 0.5 + ((k % 2) as f32 - 0.5) * 0.14),
            (id_of(obj_size_param(k)), 0.5),
            (id_of(obj_rotate_param(k)), 0.0),
            (id_of(obj_type_param(k)), ty as f32),
        ],
    );
    select(app, host, ui_state, plan::SEL_OBJECT0 + k as i32);
}

/// An attached `ChamberKnob`: a drag one change, pushed on the undo stack as it starts.
#[allow(clippy::too_many_arguments)]
fn knob_param(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, i: usize, ch: &Chamber, metric: bool, o: KnobOpts) {
    let id = id_of(i);
    let v = param(host, id);
    let text = format_param(i, v, ch, metric);
    let default = host.info.param(id).map_or(0.5, |p| p.to_normalized(p.default));
    let out = chamber_knob(ui, r, (id, host.track.0, host.ins.id.0), v, &KnobOpts { value_text: &text, default, ..o });
    if out.started {
        end_gesture(app);
        push_undo(st, host);
    }
    if let Some(nv) = out.value {
        set_params_merged(app, host, &[(id, nv)], &format!("chamber-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
}

/// The inspector's measures (`layoutInspector`'s, or the slim layout's).
pub(crate) struct Metrics {
    /// From the panel's top to the first row.
    pub title_h: f32,
    pub row_h: f32,
    pub row_pitch: f32,
    pub label_w: f32,
    pub cell_h: f32,
    pub label_font: f32,
    pub box_font: f32,
    pub title_font: f32,
    pub help_font: f32,
    pub inset: f32,
    /// The × (back to the Room) button's top, from the panel's top.
    pub back_y: f32,
}

/// `layoutInspector`'s measures.
const FULL: Metrics = Metrics {
    title_h: 42.0,
    row_h: K_ROW_H,
    row_pitch: K_ROW_PITCH,
    label_w: K_LABEL_W,
    cell_h: K_CELL_H,
    label_font: 14.0,
    box_font: 14.5,
    title_font: 15.0,
    help_font: 13.5,
    inset: 13.0,
    back_y: 11.0,
};

/// `knobs (...)` in `layoutInspector`: a row of cells centred, at most 124 wide.
fn knob_row(a: &mut Rect, n: usize, cell_h: f32) -> (Vec<Rect>, f32) {
    let r = Rect::from_min_size(pos2(a.left(), a.top() + 4.0), vec2(a.width(), cell_h));
    a.min.y += cell_h + 4.0;
    let cw = 124f32.min((r.width() / n.max(1) as f32).trunc());
    let mut x = (r.center().x - cw * n as f32 / 2.0).trunc();
    let mut out = Vec::with_capacity(n);
    for _ in 0..n {
        out.push(Rect::from_min_size(pos2(x, r.top()), vec2(cw, cell_h)));
        x += cw;
    }
    (out, r.bottom() + 10.0)
}

/// The right-hand panel (`showInspector`, `layoutInspector` and its part of `paint`).
#[allow(clippy::too_many_arguments)]
fn inspector(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    ch: &Chamber,
    area: Rect,
    target: i32,
    ui_state: &mut UiState,
    metric: bool,
    m: &Metrics,
) {
    let p = ui.painter().clone();
    let room = target == plan::SEL_ROOM;
    let spk = target == plan::SEL_SPEAKER;
    let mic = target == plan::SEL_MIC_A || target == plan::SEL_MIC_B;
    let slot = target - plan::SEL_OBJECT0;
    let object = !room && slot >= 0 && (slot as usize) < NUM_OBJECTS;
    let slot = slot.max(0) as usize;

    let mut a = area.shrink2(vec2(m.inset, 0.0));
    a.min.y += m.title_h;
    let bs = (m.title_h - 14.0).min(28.0);
    let back_r = Rect::from_min_size(pos2(area.right() - (m.inset - 1.0) - bs, area.top() + m.back_y), vec2(bs, bs));
    if !room && chamber_button(ui, back_r, ("chamber-back", host.ins.id.0), "\u{d7}", ButtonLook { font_h: 16.0, ..ButtonLook::default() }).clicked()
    {
        select(app, host, ui_state, plan::SEL_ROOM);
    }

    // the title
    let title = if room {
        "Room".to_string()
    } else if spk {
        "Speaker".into()
    } else if target == plan::SEL_MIC_A {
        "Microphone A".into()
    } else if target == plan::SEL_MIC_B {
        "Microphone B".into()
    } else {
        let names = choices(obj_type_param(slot));
        names.get(param(host, id_of(obj_type_param(slot))).round().max(0.0) as usize).copied().unwrap_or("").to_string()
    };
    p.text(pos2(area.left() + m.inset, area.top() + m.title_h / 2.0 + 2.0), Align2::LEFT_CENTER, title, ui_font(m.title_font, true), cm::TEXT);

    // a dropdown row, its label in the column on the left
    let row = |st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, a: &mut Rect, label: &str, i: usize, swatch: Option<fn(usize) -> Color32>| {
        let line = Rect::from_min_size(a.min, vec2(a.width(), m.row_h));
        a.min.y += m.row_pitch;
        let bx = Rect::from_min_max(pos2(line.left() + m.label_w, line.top()), line.max);
        p.text(pos2(bx.left() - m.label_w, bx.center().y), Align2::LEFT_CENTER, label, ui_font(m.label_font, false), cm::LABEL);
        let sel = param(host, id_of(i)).round().max(0.0) as usize;
        if let Some(n) = dropdown(ui, bx, (i, host.ins.id.0), choices(i), sel, swatch, m.box_font) {
            push_undo(st, host);
            end_gesture(app);
            set_params(app, host, &[(id_of(i), n as f32)]);
        }
    };
    let help;
    let mut help_top = a.top();
    let mut remove_r = None;
    if room {
        row(st, app, ui, &mut a, "Shape", d::P_SHAPE, None);
        row(st, app, ui, &mut a, "Walls", d::P_WALL_MAT, Some(plan::material));
        row(st, app, ui, &mut a, "Floor", d::P_FLOOR_MAT, Some(plan::material));
        row(st, app, ui, &mut a, "Ceiling", d::P_CEIL_MAT, Some(plan::material));
        let (cells, top) = knob_row(&mut a, 2, m.cell_h);
        help_top = top;
        for (r, (i, l)) in cells.into_iter().zip([(d::P_ROOM_H, "Height"), (d::P_CEIL_SLOPE, "Ceiling slope")]) {
            knob_param(st, app, ui, host, r, i, ch, metric, KnobOpts { label: l, ..KnobOpts::default() });
        }
        help = "Drag the square handles on the plan to resize. Click the speaker, a mic or an object to edit it here.".to_string();
    } else if spk {
        row(st, app, ui, &mut a, "Type", d::P_SPK_TYPE, None);
        let (cells, top) = knob_row(&mut a, 3, m.cell_h);
        help_top = top;
        for (r, (i, l)) in cells.into_iter().zip([(d::P_SPK_Z, "Height"), (d::P_SPK_AIM, "Aim"), (d::P_DRIVE, "Amp drive")]) {
            knob_param(st, app, ui, host, r, i, ch, metric, KnobOpts { label: l, ..KnobOpts::default() });
        }
        help = "Drag the speaker to move it, and the handle on its stalk to aim it.".to_string();
    } else if mic {
        row(st, app, ui, &mut a, "Pattern", d::P_MIC_PAT, None);
        row(st, app, ui, &mut a, "Type", d::P_MIC_TYPE, None);
        let (cells, top) = knob_row(&mut a, 2, m.cell_h);
        help_top = top;
        let aim = if target == plan::SEL_MIC_A { d::P_MIC_A_AIM } else { d::P_MIC_B_AIM };
        for (r, (i, l)) in cells.into_iter().zip([(d::P_MIC_Z, "Height"), (aim, "Aim")]) {
            knob_param(st, app, ui, host, r, i, ch, metric, KnobOpts { label: l, ..KnobOpts::default() });
        }
        help = "Pattern, type and height apply to both mics.".to_string();
    } else if object {
        // the type box: "Remove" in place of the empty slot's name
        let line = Rect::from_min_size(a.min, vec2(a.width(), m.row_h));
        a.min.y += m.row_pitch;
        let bx = Rect::from_min_max(pos2(line.left() + m.label_w, line.top()), line.max);
        p.text(pos2(bx.left() - m.label_w, bx.center().y), Align2::LEFT_CENTER, "Type", ui_font(m.label_font, false), cm::LABEL);
        let mut names: Vec<&str> = choices(obj_type_param(slot)).to_vec();
        if let Some(n) = names.first_mut() {
            *n = "Remove";
        }
        let ty = param(host, id_of(obj_type_param(slot))).round().max(0.0) as i32;
        if let Some(n) = dropdown(ui, bx, ("objtype", host.ins.id.0), &names, ty as usize, None, m.box_font) {
            push_undo(st, host);
            end_gesture(app);
            set_params(app, host, &[(id_of(obj_type_param(slot)), n as f32)]);
            if n == 0 {
                select(app, host, ui_state, plan::SEL_ROOM);
            }
        }
        // Rotate is only offered for things with a face.
        let flat = object_is_flat(ty);
        let list: Vec<(usize, &str)> =
            if flat { vec![(obj_size_param(slot), "Size"), (obj_rotate_param(slot), "Rotate")] } else { vec![(obj_size_param(slot), "Size")] };
        let (cells, top) = knob_row(&mut a, list.len(), m.cell_h);
        help_top = top;
        for (r, (i, l)) in cells.into_iter().zip(list) {
            knob_param(st, app, ui, host, r, i, ch, metric, KnobOpts { label: l, ..KnobOpts::default() });
        }
        let rh = if m.row_h < K_ROW_H { m.row_h } else { 32.0 };
        let rr = Rect::from_min_size(pos2(area.left() + m.inset, area.bottom() - m.inset - rh), vec2(84.0, rh));
        remove_r = Some(rr);
        if chamber_button(ui, rr, ("chamber-remove", host.ins.id.0), "Remove", ButtonLook { font_h: 13.5, ..ButtonLook::default() }).clicked() {
            push_undo(st, host);
            end_gesture(app);
            set_params(app, host, &[(id_of(obj_type_param(slot)), 0.0)]);
            select(app, host, ui_state, plan::SEL_ROOM);
        }
        let o = &ch.objects[slot];
        let mut t = plan::tip(o.ty).to_string();
        let f0 = plan::resonance_hz(o);
        if f0 > 0.0 {
            t.push_str(&format!(" Tuned to {} Hz.", (f0 + 0.5).floor() as i32));
        }
        help = t;
    } else {
        help = String::new();
    }
    if !help.is_empty() {
        let bottom = remove_r.map_or(area.bottom() - 10.0, |r| r.top() - 8.0);
        let hr = Rect::from_min_max(pos2(area.left() + m.inset, help_top), pos2(area.right() - m.inset, bottom));
        fitted_text(&p, hr, &help, m.help_font, cm::LABEL);
    }
}

/// The More card (`MoreCard` and its children): Sound paths, ft / m, Close, the echogram and
/// the four groups of knobs. It swallows clicks so they never reach the plan.
#[allow(clippy::too_many_arguments)]
fn more_card(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, ch: &Chamber, body: Rect, ui_state: &mut UiState, metric: bool) {
    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("chamber-more", host.ins.id.0)).max_rect(body));
    let ui = &mut cui;
    let _ = ui.interact(body, ui.id().with("swallow"), egui::Sense::click_and_drag());
    let p = ui.painter().clone();
    controls::draw_panel(&p, body, crate::editor_kit::palette::al(cm::PANEL, 0.93), cm::PANEL_LINE, 6.0);
    let at = |x: f32, y: f32, w: f32, h: f32| Rect::from_min_size(pos2(body.left() + x, body.top() + y), vec2(w, h));
    p.text(pos2(body.left() + 16.0, body.top() + 31.0), Align2::LEFT_CENTER, "More", ui_font(16.0, true), cm::TEXT);
    let cw = body.width();
    let close = at(cw - 14.0 - 62.0, 13.0, 62.0, 34.0);
    let m_r = at(close.left() - body.left() - 11.0 - 37.0, 13.0, 37.0, 34.0);
    let ft_r = at(m_r.left() - body.left() - 37.0, 13.0, 37.0, 34.0);
    let rays_r = at(ft_r.left() - body.left() - 10.0 - 106.0, 13.0, 106.0, 34.0);
    let look = ButtonLook::default();
    more_toggles(app, ui, host, ui_state, rays_r, ft_r, m_r, look);
    if chamber_button(ui, close, ("chamber-close", host.ins.id.0), "Close", look).clicked() {
        ui_state.more = false;
        write_ui_state(app, host, *ui_state);
    }
    plan::echo(&p, at(16.0, 58.0, cw - 32.0, 94.0), ch);

    let groups = [
        ("Blend", at(16.0, 163.0, 374.0, 132.0), cm::LABEL),
        ("Tone", at(400.0, 163.0, 229.0, 132.0), cm::LABEL),
        ("Room, exact", at(16.0, 305.0, 260.0, 132.0), cm::LABEL),
        ("Tape machine before the speaker", at(286.0, 305.0, 301.0, 132.0), cm::TAPE),
    ];
    for (title, r, c) in groups {
        controls::draw_panel(&p, r, cm::GROUP, cm::GROUP_LINE, 5.0);
        p.text(pos2(r.left() + 12.0, r.top() + 17.0), Align2::LEFT_CENTER, title, ui_font(13.5, false), c);
    }
    for (g, (_, list)) in MORE_GROUPS.iter().enumerate() {
        let r = groups[g].1;
        let mut x = r.left() + 6.0;
        for (i, label, w) in *list {
            let cell = Rect::from_min_size(pos2(x, r.top() + 28.0), vec2(*w, K_CELL_H));
            x += w;
            let arc = if g == 3 { cm::TAPE } else { cm::ARC };
            knob_param(st, app, ui, host, cell, *i, ch, metric, KnobOpts { label, arc, ..KnobOpts::default() });
        }
    }
}

/// More's knob groups: the title and the knobs (parameter, name, the card's cell width).
#[allow(clippy::type_complexity)]
const MORE_GROUPS: [(&str, &[(usize, &str, f32)]); 4] = [
    (
        "Blend",
        &[
            (d::P_PRE_DELAY, "Pre-delay", K_CELL_W),
            (d::P_EARLY, "Early", K_CELL_W),
            (d::P_TAIL, "Tail", K_CELL_W),
            (d::P_WIDTH, "Width", K_CELL_W),
            (d::P_LEVEL, "Level", K_CELL_W),
        ],
    ),
    ("Tone", &[(d::P_LOW_CUT, "Low cut", K_CELL_W), (d::P_HIGH_CUT, "High cut", K_CELL_W), (d::P_DRIFT, "Drift", K_CELL_W)]),
    ("Room, exact", &[(d::P_ROOM_W, "Width", 76.0), (d::P_ROOM_L, "Length", 74.0), (d::P_TAPER, "Shape amount", 98.0)]),
    (
        "Tape machine before the speaker",
        &[
            (d::P_TAPE_SEND, "Send", K_CELL_W),
            (d::P_TAPE_TIME, "Time", K_CELL_W),
            (d::P_TAPE_REPEATS, "Repeats", K_CELL_W),
            (d::P_TAPE_TONE, "Tone", K_CELL_W),
        ],
    ),
];

/// More's Sound paths and ft / m buttons (`uiState`).
#[allow(clippy::too_many_arguments)]
fn more_toggles(app: &mut SoundApp, ui: &mut Ui, host: &Host, ui_state: &mut UiState, rays_r: Rect, ft_r: Rect, m_r: Rect, look: ButtonLook) {
    let metric = ui_state.metric;
    if chamber_button(ui, rays_r, ("chamber-rays", host.ins.id.0), "Sound paths", ButtonLook { active: ui_state.rays, ..look }).clicked() {
        ui_state.rays = !ui_state.rays;
        write_ui_state(app, host, *ui_state);
    }
    if chamber_button(ui, ft_r, ("chamber-ft", host.ins.id.0), "ft", ButtonLook { active: !metric, join: (false, true), ..look }).clicked() {
        ui_state.metric = false;
        write_ui_state(app, host, *ui_state);
    }
    if chamber_button(ui, m_r, ("chamber-m", host.ins.id.0), "m", ButtonLook { active: metric, join: (true, false), ..look }).clicked() {
        ui_state.metric = true;
        write_ui_state(app, host, *ui_state);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn size_and_readouts() {
        assert_eq!(SIZE, vec2(1190.0, 648.0));
        let ch = soundcraft_dsp::plugins::fs::chamber::build_chamber_for(
            &soundcraft_dsp::plugins::fs::chamber::CHAMBER_PARAMS.iter().map(|p| p.default).collect::<Vec<_>>(),
        );
        assert_eq!(format_param(d::P_MIX, 0.35, &ch, false), "35 %");
        assert!(format_param(d::P_ROOM_H, 0.5, &ch, false).ends_with(" ft"));
        assert!(format_param(d::P_ROOM_H, 0.5, &ch, true).ends_with(" m"));
        assert_eq!(format_param(d::P_SPK_AIM, 0.25, &ch, false), "90\u{b0}");
        assert_eq!(obj_type_param(7), d::P_OBJ8_SIZE - 3);
        assert_eq!(obj_rotate_param(7), d::P_OBJ8_ROT);
    }

    #[test]
    fn ui_state_reads_hostile_bytes_as_defaults() {
        let mut ins = Insert::new("fs_chamber");
        assert_eq!(read_ui_state(&ins), UiState::default());
        ins.set_state_bytes(&[0xFF, 0x00, 0x13]);
        assert_eq!(read_ui_state(&ins), UiState::default());
        ins.set_state_bytes(br#"{"more": true, "inspect": 999}"#);
        let s = read_ui_state(&ins);
        assert!(s.more && s.rays && s.inspect == 2 + NUM_OBJECTS as i32);
    }
}

//! LevelUp's VST editor (`LevelUp/Source/PluginEditor.cpp`), translated: the header (title, the
//! Level / Reference / Deliver tabs, the preset name, Bypass), the pairing row (Role, Pair, the
//! status line), and the three tabs' layout, paint and gestures. LevelUp keeps its Amorph look
//! (`applyPalette`: dark, one cyan accent), the same in both DAW themes. Its interaction
//! inventory is `docs/superpowers/editors/levelup.md`.
//!
//! Reused from `levelup_ui` (the same translation): the 10 Hz short-term history, the pairing view
//! of the host's link group, `lufsText` / `signedDb`; Learn's write-back onto Target runs there for
//! every LevelUp. Everything shown is the engine's own readouts (`fs_levelup`'s `readouts`) plus
//! the preview gain (`levelup::LIVE_PREVIEW_GAIN`); references go through
//! `mix.module_reference`, Deliver's Reset through `mix.module_action {reset_delivery}`.

use crate::SoundApp;
use crate::editor_kit::controls::{
    DialOpts, PillLook, PillStyle, SelectionStyle, dial, pill_ex, pill_width, preset_name_button_ex, segmented_ex, segmented_id, tip,
};
use crate::editor_kit::look::{tracked_text, tracked_width};
use crate::editor_kit::palette::al;
use crate::editor_kit::presets::{self, PresetState};
use crate::editor_kit::{Host, VstEditor, VstPalette, action, argb, end_gesture, param, set_params, set_params_merged};
use crate::levelup_ui::{self as lu, lufs_text, signed_db};
use crate::theme::regular;
use egui::{Align2, Color32, CornerRadius, Id, Pos2, Rect, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::plugins::fs::levelup::{self as dsp, level_up_dsp as d};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, REFERENCE_KEYS, Track};

mod slim;

const W: f32 = 820.0;
const H: f32 = 580.0;
const PAD: f32 = 18.0;
/// `setSize (kW, kH)`.
pub const SIZE: Vec2 = vec2(W, H);

// The Amorph build's palette (LevelUpUI.js).
const BG: Color32 = argb(0xFF0E1114);
const PANEL: Color32 = argb(0xFF13171B);
const LINE: Color32 = argb(0xFF1F252B);
const BORDER: Color32 = argb(0xFF262C33);
const TEXT: Color32 = argb(0xFFDFE6EC);
const BODY: Color32 = argb(0xFFB8C2CC);
const DIM: Color32 = argb(0xFF7D8894);
const LABEL: Color32 = argb(0xFF6F7B86);
const ACCENT: Color32 = argb(0xFF3DD6FF);
const WARN: Color32 = argb(0xFFFFB454);
const BAD: Color32 = argb(0xFFFF6B5E);

/// `applyPalette`.
pub const PALETTE: VstPalette = VstPalette {
    accent: ACCENT,
    background: BG,
    panel: PANEL,
    panel_alt: PANEL,
    border: BORDER,
    border_hover: argb(0xFF3A444F),
    text_bright: TEXT,
    text_body: BODY,
    text_dim: DIM,
    text_label: LABEL,
    text_faint: argb(0xFF4D5763),
    on_accent: argb(0xFF0B0E11),
    bypass_red: BAD,
    ..VstPalette::THEME_DEFAULTS
};

/// `Colour::brighter (0.06f)` of the panel.
fn panel_sel() -> Color32 {
    let k = 1.0 / 1.06;
    let ch = |v: u8| (255.0 - k * (255.0 - f32::from(v))).round() as u8;
    Color32::from_rgb(ch(PANEL.r()), ch(PANEL.g()), ch(PANEL.b()))
}

/// How long Normalize and Learn are held at 1 (`callAfterDelay (150)`).
const MOMENTARY_S: f64 = 0.15;

/// LevelUp's editor: what the C++ editor holds that is not a parameter.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    /// `tab` (the processor keeps it in the VST; here the editor does).
    tab: usize,
    /// A momentary press in progress: the parameter and when it was pressed.
    held: Option<(&'static str, f64)>,
    presets: PresetState,
    scope: Vec<Pos2>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's reference rows as last laid out (a dropped file lands on the one under it).
    slim_rows: Option<[Rect; 4]>,
}

/// Draws LevelUp's editor for `module` on `track`.
/// What `getPresetParams` leaves out besides `bypass` (LevelUp's `Presets.h`).
const NOT_IN_PRESETS: [&str; 6] = ["linkGroup", "reference", "refSlot", "preview", "normalize", "learn"];

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_levelup";

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

/// The engine's readouts (NaN: no reading).
struct Readings {
    r: [f32; 8],
    preview_gain: f32,
}
impl Readings {
    fn get(&self, i: usize) -> f32 {
        self.r.get(i).copied().unwrap_or(f32::NAN)
    }
    fn or(&self, i: usize, d: f32) -> f32 {
        let v = self.get(i);
        if v.is_finite() { v } else { d }
    }
}

/// The editor's `setTooltip`s, by control (its parameter, or the pill's or segment's name).
fn tooltip(id: &str) -> Option<&'static str> {
    Some(match id {
        "role" => {
            "IN: put before the chain you are testing. OUT: after it -- it matches the IN's loudness, so what you hear change is              the chain, not the level. Off: a leveler on its own."
        }
        "pair" => "Which IN an OUT follows, when there is more than one A/B going.",
        "target" => "The loudness to ride to. On an OUT that has found its IN, the IN's loudness is used instead.",
        "attack" => "How fast it turns DOWN when the signal gets louder than the target.",
        "release" => "How fast it turns UP when the signal gets quieter.",
        "gate" => "Below this far under the target it stops riding, so a pause or a fade is not pulled up.",
        "maxGain" => "The most it will ever add or take away.",
        "learn" => "Take what is playing now as the Target.",
        "normalize" => "Jump straight to the target, once -- for Static mode.",
        "freeze" => "Hold the current correction.",
        "ceiling" => "A soft ceiling at -1 dBFS, so a boost can never clip.",
        "mode" => "Auto rides continuously. Static holds one gain; set it with Normalize.",
        "listen" => "Flip between your mix and the selected reference, played at your mix's loudness.",
        "reset" => "Start measuring again -- do this at the top of a full play-through.",
        "preview" => "Hear the master at the level the selected platform would play it.",
        "dither" => "Dither for the final bounce: 16-bit for a CD-quality file, 24-bit otherwise. Last in the chain.",
        "shaping" => "Push the dither noise up the spectrum, where it is less audible.",
        _ => return None,
    })
}

fn pill(ui: &mut Ui, r: Rect, salt: &str, text: &str, on: bool, lit: Option<Color32>) -> egui::Response {
    let resp = pill_ex(ui, r, salt, &PALETTE, text, on, PillStyle::Small, PillLook { active_colour: lit, tracked: true, dimmed: false });
    match tooltip(salt) {
        Some(t) => resp.on_hover_text(t),
        None => resp,
    }
}

fn seg(ui: &mut Ui, r: Rect, salt: &str, opts: &[&str], sel: usize) -> Option<usize> {
    let picked = segmented_ex(ui, r, salt, &PALETTE, opts, 1, Some(sel), SelectionStyle::Filled, true);
    if let Some(t) = tooltip(salt) {
        tip(ui, segmented_id(ui, salt), t);
    }
    picked
}

fn idx(host: &Host, id: &str) -> usize {
    let v = param(host, id);
    if v.is_finite() { v.round().max(0.0) as usize } else { 0 }
}

/// `setValueAsCompleteGesture` on a choice or toggle.
fn set_one(app: &mut SoundApp, host: &Host, id: &str, v: f32) {
    set_params(app, host, &[(id, v)]);
}

// ============================================================================ the editor
/// What both layouts do before they paint: the scripted drawer tabs, the momentary presses, the
/// readouts and the short-term history; `None` while the preset browser covers the editor.
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> Option<(Readings, std::sync::Arc<lu::LevelUpHistory>)> {
    let ed = host.rect;
    if !st.init {
        st.init = true;
        st.presets.category = "All".into();
    }
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.presets),
            "Level" => st.tab = 0,
            "Reference" => st.tab = 1,
            "Deliver" => st.tab = 2,
            _ => {}
        }
    }
    let now = ui.input(|i| i.time);
    // the momentary press lets go after 150 ms (one undo step for the press)
    if let Some((id, t0)) = st.held {
        if now - t0 >= MOMENTARY_S || now < t0 {
            set_params_merged(app, host, &[(id, 0.0)], &format!("momentary-{id}"));
            end_gesture(app);
            st.held = None;
        }
        ui.ctx().request_repaint();
    }
    let meter = host.meters.and_then(|m| m.modules.iter().find(|x| x.module == host.ins.id));
    let chain_live = meter.map(crate::chain_view::live_values);
    let rd = Readings {
        r: meter.map_or([f32::NAN; 8], |m| m.readouts),
        preview_gain: crate::editor_kit::live(host).map_or(f32::NAN, |l| l.get(dsp::LIVE_PREVIEW_GAIN)),
    };
    let history = lu::history(ui.ctx(), Id::new(("levelup-editor", host.track.0, host.ins.id.0)), chain_live.as_ref());
    if host.meters.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }

    st.presets.not_in_presets = &NOT_IN_PRESETS;
    if st.presets.open {
        let out = presets::preset_browser(app, ui, host, ed, &PALETTE, &mut st.presets);
        presets::clipboard(app, ui, host, &mut st.presets, &out);
        if out.recalled.is_some() {
            end_gesture(app);
        }
        return None;
    }
    Some((rd, history))
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    let ed = host.rect;
    let Some((rd, history)) = prelude(st, app, ui, host) else { return };

    let p = ui.painter().clone();
    p.rect_filled(ed, CornerRadius::ZERO, BG);
    let b = ed.shrink2(vec2(PAD, 12.0));
    let header = Rect::from_min_size(b.min, vec2(b.width(), 30.0));
    let status_area = Rect::from_min_size(pos2(b.left(), header.bottom() + 8.0), vec2(b.width(), 26.0));
    let body = Rect::from_min_max(pos2(b.left(), status_area.bottom() + 8.0), b.max);

    // ---- header
    {
        // title: "Level" plain, "Up" in the accent, tracked uppercase
        let t = Rect::from_min_size(header.min, vec2(120.0, header.height()));
        let w1 = tracked_width(&p, "Level", 15.0, 0.22, true);
        tracked_text(&p, Rect::from_min_size(t.min, vec2(w1 + 2.0, t.height())), Align2::LEFT_CENTER, "Level", 15.0, 0.22, true, false, TEXT);
        tracked_text(&p, Rect::from_min_max(pos2(t.left() + w1 + 6.0, t.top()), t.max), Align2::LEFT_CENTER, "Up", 15.0, 0.22, true, true, ACCENT);
        let mut h = Rect::from_min_max(pos2(header.left() + 120.0, header.top()), header.max);
        let bw = pill_width(ui, "Bypass", PillStyle::Small);
        let bypass_r = Rect::from_center_size(pos2(h.right() - bw / 2.0, h.center().y), vec2(bw, 22.0));
        h.max.x -= bw + 10.0;
        bypass_ui(app, ui, host, bypass_r);
        let tab_r = Rect::from_min_size(pos2(h.left(), h.center().y - 12.0), vec2(250.0, 24.0));
        tabs_ui(st, ui, tab_r);
        let name_r = Rect::from_min_max(pos2(tab_r.right() + 12.0, h.center().y - 12.0), pos2(h.right(), h.center().y + 12.0));
        name_ui(st, app, ui, host, name_r);
    }

    // ---- status: Role, Pair, and what this instance is doing in its pair
    {
        let pair_r = Rect::from_center_size(pos2(status_area.right() - 66.0, status_area.center().y), vec2(132.0, 22.0));
        let role_r = Rect::from_center_size(pos2(pair_r.left() - 8.0 - 75.0, status_area.center().y), vec2(150.0, 22.0));
        pairing_ui(app, ui, host, role_r, pair_r);
        let (text, colour) = status_line(host, &rd);
        let sr = Rect::from_min_max(status_area.min, pos2(status_area.right() - 300.0, status_area.bottom()));
        p.with_clip_rect(sr.intersect(p.clip_rect())).text(pos2(sr.left(), sr.center().y), Align2::LEFT_CENTER, text, regular(11.5), colour);
    }

    match st.tab {
        0 => level_tab(st, app, ui, host, body, &rd, &history),
        1 => reference_tab(app, ui, host, body, &rd),
        _ => deliver_tab(app, ui, host, body, &rd),
    }
    let body_rows = ref_rows(body);
    file_drop(app, ui, host, st, ed, body_rows);
}

/// The Level / Reference / Deliver tabs in `r` (editor state: no command).
fn tabs_ui(st: &mut Editor, ui: &mut Ui, r: Rect) {
    if let Some(t) = seg(ui, r, "tab", &["Level", "Reference", "Deliver"], st.tab) {
        st.tab = t.min(2);
    }
}

/// Bypass in `r`, lit red.
fn bypass_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let byp = param(host, "bypass") > 0.5;
    if pill(ui, r, "bypass", "Bypass", byp, Some(BAD)).clicked() {
        set_one(app, host, "bypass", if byp { 0.0 } else { 1.0 });
    }
}

/// The preset name in `r`: a click opens the browser.
fn name_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let dirty = presets::is_dirty(app, host, &mut st.presets);
    if preset_name_button_ex(ui, r, "name", &PALETTE, presets::shown_name(host), dirty).clicked() {
        presets::open(app, host, &mut st.presets);
    }
}

/// Role (Off / IN / OUT) and Pair (A..D).
fn pairing_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, role_r: Rect, pair_r: Rect) {
    if let Some(i) = seg(ui, role_r, "role", &dsp::ROLES, idx(host, "link").min(2)) {
        set_one(app, host, "link", i as f32);
    }
    if let Some(i) = seg(ui, pair_r, "pair", &dsp::PAIRS, idx(host, "linkGroup").min(3)) {
        set_one(app, host, "linkGroup", i as f32);
    }
}

/// The status line (`paint`): the C++'s words; the host's link group adds what the VST cannot be
/// (not in a group, a group it cannot pair in), as `levelup_ui::status` says.
fn status_line(host: &Host, rd: &Readings) -> (String, Color32) {
    let role = idx(host, "link") as i32;
    let pair = dsp::PAIRS.get(idx(host, "linkGroup").min(3)).copied().unwrap_or("A");
    if role == d::LINK_OFF {
        return ("Leveling on its own. Set IN before a chain and OUT after it to A/B the chain at equal loudness.".into(), DIM);
    }
    let info = lu::pair_info(&host.session, host.track, host.ins);
    if !info.linked || info.unavailable.is_some() {
        let lv = crate::module_graphs::LiveValues { readouts: rd.r, ..Default::default() };
        let (s, _) = lu::status(host.ins, Some(&lv), Some(&info));
        return (s, WARN);
    }
    if role == d::LINK_IN {
        return (format!("IN of pair {pair} -- an OUT in pair {pair} will match this loudness"), DIM);
    }
    let following = rd.get(7);
    if following.is_finite() {
        (format!("OUT of pair {pair} -- matching its IN at {} LUFS", lufs_text(following)), ACCENT)
    } else {
        (format!("OUT of pair {pair} -- no IN found yet; using the Target knob"), WARN)
    }
}

/// `attachDial` (tracked labels, `formatParam` readouts; no undo push in LevelUp).
fn knob(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, id: &'static str, label: &str) {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let value = param(host, id);
    let text = info.format(value);
    let o = DialOpts { label, value_text: &text, default: info.to_normalized(info.default), tracked: true, ..DialOpts::default() };
    let out = dial(ui, r, id, &PALETTE, info.to_normalized(value), &o);
    if let Some(t) = tooltip(id) {
        let _ = out.response.clone().on_hover_text(t);
    }
    if out.started {
        end_gesture(app);
    }
    if let Some(v) = out.value {
        set_params_merged(app, host, &[(id, info.clamp(info.from_normalized(v)))], &format!("dial-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
}

#[allow(clippy::too_many_arguments)]
fn level_tab(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, body: Rect, rd: &Readings, history: &lu::LevelUpHistory) {
    let p = ui.painter().clone();
    let foot = Rect::from_min_max(pos2(body.left(), body.bottom() - 32.0), body.max);
    let dials = Rect::from_min_max(pos2(body.left(), foot.top() - 10.0 - 104.0), pos2(body.right(), foot.top() - 10.0));
    let delta = Rect::from_min_max(pos2(body.left(), dials.top() - 8.0 - 54.0), pos2(body.right(), dials.top() - 8.0));
    let r = Rect::from_min_max(body.min, pos2(body.right(), delta.top() - 8.0));

    paint_scope(st, &p, r, host, rd, history, 26.0);

    paint_correction(&p, delta, host, rd, 170.0, 34.0);

    // ---- the five dials
    let w = (dials.width() / 5.0).trunc();
    for (i, (id, label)) in
        [("target", "Target"), ("attack", "Attack"), ("release", "Release"), ("gate", "Gate"), ("maxGain", "Max Gain")].into_iter().enumerate()
    {
        let slot = Rect::from_min_size(pos2(dials.left() + w * i as f32, dials.top()), vec2(w, dials.height()));
        knob(app, ui, host, Rect::from_center_size(slot.center(), vec2(w.min(110.0), dials.height())), id, label);
    }

    // ---- the foot: Learn, Auto / Static, Normalize, Freeze, Ceiling
    let fw = ((foot.width() - 4.0 * 8.0) / 5.0).trunc();
    let cell = |i: usize| {
        Rect::from_min_size(
            pos2(foot.left() + (fw + 8.0) * i as f32, foot.top()),
            vec2(if i == 4 { foot.right() - (foot.left() + (fw + 8.0) * 4.0) } else { fw }, foot.height()),
        )
    };
    foot_ui(st, app, ui, host, [cell(0), cell(1), cell(2), cell(3), cell(4)]);
}

/// The scope in `r`: short-term LUFS over 30 s against the target (dashed) and the gate (the
/// shaded floor), and the reading top right at `big` points.
#[allow(clippy::too_many_arguments)]
fn paint_scope(st: &mut Editor, p: &egui::Painter, r: Rect, host: &Host, rd: &Readings, history: &lu::LevelUpHistory, big: f32) {
    p.rect_filled(r, CornerRadius::same(4), PANEL);
    p.rect_stroke(r.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, LINE), StrokeKind::Middle);
    let (lo, hi) = (-70.0f32, 0.0f32);
    let y_of = |v: f32| r.bottom() - (v.clamp(lo, hi) - lo) / (hi - lo) * r.height();
    let mut v = lo;
    while v <= hi {
        let y = y_of(v).round() + 0.5;
        p.line_segment([pos2(r.left(), y), pos2(r.right(), y)], Stroke::new(1.0, al(Color32::WHITE, 0.06)));
        p.text(pos2(r.left() + 4.0, y - 6.5), Align2::LEFT_CENTER, format!("{}", v as i32), regular(9.0), al(Color32::WHITE, 0.28));
        v += 10.0;
    }
    let following = rd.get(7);
    let target = if following.is_finite() { following } else { d::knob_to_target(param(host, "target")) };
    let gate = d::knob_to_gate(param(host, "gate"));
    p.rect_filled(Rect::from_min_max(pos2(r.left(), y_of(target - gate)), r.max), 0.0, al(Color32::WHITE, 0.035));
    p.extend(Shape::dashed_line(&[pos2(r.left(), y_of(target)), pos2(r.right(), y_of(target))], Stroke::new(1.0, al(ACCENT, 0.55)), 4.0, 4.0));
    // 300 points, the oldest first; before the history fills, the C++'s -100 start
    let n = lu::HISTORY_LEN;
    let missing = n - history.len().min(n);
    st.scope.clear();
    for i in 0..n {
        let v = if i < missing { -100.0 } else { history.point(i - missing).map_or(-100.0, |q| q.1) };
        st.scope.push(pos2(r.left() + r.width() * i as f32 / (n - 1) as f32, y_of(v)));
    }
    crate::editor_kit::icons::fill_strip(p, &st.scope, |_| r.bottom(), al(ACCENT, 0.08));
    for i in (1..=3).rev() {
        p.add(Shape::line(st.scope.clone(), Stroke::new(2.0 + i as f32 * 2.0, al(ACCENT, 0.12 / i as f32))));
    }
    p.add(Shape::line(st.scope.clone(), Stroke::new(2.0, ACCENT)));
    // readout, top right
    let ro = Rect::from_min_max(pos2(r.right() - 12.0 - 160.0, r.top() + 8.0), pos2(r.right() - 12.0, r.top() + 8.0 + 48.0));
    p.text(pos2(ro.right(), ro.top() + 15.0), Align2::RIGHT_CENTER, lufs_text(rd.or(0, -100.0)), regular(big), TEXT);
    let lab = Rect::from_min_max(pos2(ro.left(), ro.top() + 30.0), ro.max);
    tracked_text(p, lab, Align2::RIGHT_CENTER, "LUFS short-term", 9.0, 0.18, true, false, LABEL);
}

/// The correction in `delta`: the gain in dB (`left_w` wide, at `big` points) and the bar of it
/// against Max Gain.
fn paint_correction(p: &egui::Painter, delta: Rect, host: &Host, rd: &Readings, left_w: f32, big: f32) {
    let gain = rd.or(1, 0.0);
    let left = Rect::from_min_size(delta.min, vec2(left_w, delta.height()));
    p.text(pos2(left.left(), left.top() + big * 0.56), Align2::LEFT_CENTER, signed_db(gain), regular(big), ACCENT);
    let lab = Rect::from_min_max(pos2(left.left(), left.top() + big * 1.12), left.max);
    tracked_text(p, lab, Align2::LEFT_CENTER, "Gain correction dB", 9.0, 0.18, true, false, LABEL);
    let rest = Rect::from_min_max(pos2(left.right(), delta.top()), delta.max);
    let bar = Rect::from_center_size(rest.center(), vec2(rest.width(), 6.0));
    p.rect_filled(bar, CornerRadius::same(3), argb(0xFF1A1F24));
    let cx = bar.center().x.round() + 0.5;
    p.line_segment([pos2(cx, bar.top()), pos2(cx, bar.bottom())], Stroke::new(1.0, argb(0xFF3A434C)));
    let mg = d::knob_to_max_gain(param(host, "maxGain")).max(1.0);
    let pct = (gain / mg).clamp(-1.0, 1.0) * 0.5;
    let fill = if pct < 0.0 {
        Rect::from_min_max(pos2(bar.center().x + pct * bar.width(), bar.top()), pos2(bar.center().x, bar.bottom()))
    } else {
        Rect::from_min_max(pos2(bar.center().x, bar.top()), pos2(bar.center().x + pct * bar.width(), bar.bottom()))
    };
    if fill.width() > 0.0 {
        p.rect_filled(fill, CornerRadius::same(3), ACCENT);
    }
}

/// The foot's controls in `cells`: Learn, Auto / Static, Normalize, Freeze, Ceiling.
fn foot_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, cells: [Rect; 5]) {
    let cell = |i: usize| cells[i.min(4)];
    for (i, id, text) in [(0, "learn", "Learn"), (2, "normalize", "Normalize")] {
        let on = st.held.is_some_and(|(h, _)| h == id);
        if pill(ui, cell(i), id, text, on, None).clicked() && st.held.is_none() {
            end_gesture(app);
            set_params_merged(app, host, &[(id, 1.0)], &format!("momentary-{id}"));
            st.held = Some((id, ui.input(|i| i.time)));
        }
    }
    if let Some(i) = seg(ui, cell(1), "mode", &dsp::MODES, idx(host, "mode").min(1)) {
        set_one(app, host, "mode", i as f32);
    }
    for (i, id, text) in [(3, "freeze", "Freeze"), (4, "ceiling", "Ceiling")] {
        let on = param(host, id) > 0.5;
        if pill(ui, cell(i), id, text, on, None).clicked() {
            set_one(app, host, id, if on { 0.0 } else { 1.0 });
        }
    }
}

/// A reference slot's info: name, loudness, true peak, seconds, truncated (`getRefInfo`).
struct RefInfo {
    name: String,
    loaded: Option<(f32, f32, f32, bool)>,
    error: Option<String>,
}

fn ref_info(host: &Host, slot: usize) -> RefInfo {
    let Some(key) = REFERENCE_KEYS.get(slot) else { return RefInfo { name: String::new(), loaded: None, error: None } };
    let Some(r) = host.ins.assets.get(*key) else { return RefInfo { name: String::new(), loaded: None, error: None } };
    let name = std::path::Path::new(r).file_name().and_then(|n| n.to_str()).unwrap_or(r.as_str()).to_string();
    let s = &host.session;
    let asset =
        s.assets.get_for(key, r, s.sample_rate.hz()).and_then(|a| a.0.downcast_ref::<std::sync::Arc<dyn soundcraft_dsp::SampleSource>>()).cloned();
    match asset.as_deref().and_then(dsp::as_reference) {
        Some(a) => RefInfo { name, loaded: Some((a.lufs(), a.true_peak_dbtp(), a.seconds(), a.truncated())), error: None },
        None => RefInfo { name, loaded: None, error: Some("missing: the file could not be read".into()) },
    }
}

fn choose_reference(app: &mut SoundApp, host: &Host, slot: usize) {
    let path = app.services.pick_open.as_ref().and_then(|open| open("mix.module_reference", &["wav", "wave", "aif", "aiff", "flac", "mp3"]));
    if let Some(path) = path {
        load_reference(app, host, slot, &path);
    }
}

fn load_reference(app: &mut SoundApp, host: &Host, slot: usize, path: &str) {
    match app.run("mix.module_reference", json!({"track": host.track.0, "module": host.ins.id.0, "slot": slot, "file": path})) {
        Ok(_) => set_one(app, host, "refSlot", slot as f32),
        Err(e) => app.ui.status = e,
    }
}

fn ref_rows(body: Rect) -> [Rect; 4] {
    let top = body.top() + 34.0 + 10.0;
    std::array::from_fn(|s| Rect::from_min_size(pos2(body.left(), top + (58.0 + 8.0) * s as f32), vec2(body.width(), 58.0)))
}

fn reference_tab(app: &mut SoundApp, ui: &mut Ui, host: &Host, body: Rect, rd: &Readings) {
    let p = ui.painter().clone();
    let listen_r = Rect::from_min_size(pos2(body.left(), body.top() + 4.0), vec2(220.0, 26.0));
    listen_ui(app, ui, host, listen_r);
    let rows = ref_rows(body);
    for (s, r) in rows.iter().enumerate() {
        ref_row(app, ui, host, *r, s, false);
    }
    // what is happening
    let big = Rect::from_min_max(pos2(body.left() + 4.0, rows[3].bottom() + 8.0 + 6.0), pos2(body.right() - 4.0, body.bottom() - 6.0));
    let mut job = egui::text::LayoutJob::simple(ref_status(host, rd), regular(12.0), DIM, big.width());
    job.wrap.max_rows = 3;
    let g = ui.fonts_mut(|f| f.layout_job(job));
    p.galley(big.min, g, DIM);
}

/// Mix / Reference: what is heard.
fn listen_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let listening = param(host, "reference") > 0.5;
    if let Some(i) = seg(ui, r, "listen", &["Mix", "Reference"], usize::from(listening)) {
        set_one(app, host, "reference", i as f32);
    }
}

/// Reference slot `s` in `r`: its number, name and readings, Load and the clear button; a click
/// selects it. `compact` (the slim layout) uses smaller type and a shorter readings line.
fn ref_row(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, s: usize, compact: bool) {
    let p = ui.painter().clone();
    let selected = idx(host, "refSlot").min(3);
    let r = &r;
    {
        let info = ref_info(host, s);
        let sel = s == selected;
        // a click on the row selects it (`mouseUp`)
        let resp = ui.interact(*r, ui.id().with(("ref-row", s)), egui::Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Reference {}", s + 1)));
        p.rect_filled(*r, CornerRadius::same(4), if sel { panel_sel() } else { PANEL });
        p.rect_stroke(r.shrink(0.5), CornerRadius::same(4), Stroke::new(1.0, if sel { al(ACCENT, 0.7) } else { LINE }), StrokeKind::Middle);
        let (pad, buttons, num_w) = if compact { (8.0, 80.0, 18.0) } else { (12.0, 110.0, 26.0) };
        let t = Rect::from_min_max(r.min + vec2(pad, 8.0), pos2(r.right() - pad - buttons, r.bottom() - 8.0));
        let num = if compact { 15.0 } else { 18.0 };
        p.text(pos2(t.left(), t.center().y), Align2::LEFT_CENTER, format!("{}", s + 1), regular(num), if sel { ACCENT } else { DIM });
        let tt = Rect::from_min_max(pos2(t.left() + num_w, t.top()), t.max);
        let pc = p.with_clip_rect(tt.intersect(p.clip_rect()));
        let empty = info.name.is_empty();
        pc.text(
            pos2(tt.left(), tt.top() + 10.0),
            Align2::LEFT_CENTER,
            if empty && compact {
                "Empty"
            } else if empty {
                "Empty -- Load, or drop a file here"
            } else {
                info.name.as_str()
            },
            regular(if compact { 12.0 } else { 13.0 }),
            if empty { LABEL } else { TEXT },
        );
        let (sub, sub_col) = match (&info.loaded, &info.error) {
            (_, Some(e)) => (e.clone(), BAD),
            (Some((lufs, tp, secs, trunc)), None) => {
                let secs = secs.max(0.0) as i64;
                let mut s = if compact {
                    format!("{} LUFS \u{b7} {tp:.1} dBTP \u{b7} {}:{:02}", lufs_text(*lufs), secs / 60, secs % 60)
                } else {
                    format!("{} LUFS integrated   {tp:.1} dBTP   {}:{:02}", lufs_text(*lufs), secs / 60, secs % 60)
                };
                if *trunc {
                    s += if compact { " (8 min)" } else { "   (first 8 minutes)" };
                }
                (s, DIM)
            }
            _ => (String::new(), DIM),
        };
        let sub_size = if compact { 9.5 } else { 11.0 };
        pc.text(pos2(tt.left(), tt.top() + 20.0 + (tt.height() - 20.0) / 2.0), Align2::LEFT_CENTER, sub, regular(sub_size), sub_col);
        let rr = r.shrink2(vec2(pad, 0.0));
        let (clear_w, load_w) = if compact { (24.0, 46.0) } else { (28.0, 64.0) };
        let clear_r = Rect::from_center_size(pos2(rr.right() - clear_w / 2.0, rr.center().y), vec2(clear_w, 22.0));
        let load_r = Rect::from_center_size(pos2(clear_r.left() - 6.0 - load_w / 2.0, rr.center().y), vec2(load_w, 22.0));
        if pill(ui, load_r, &format!("load-{s}"), "Load", false, None).clicked() {
            choose_reference(app, host, s);
        } else if pill(ui, clear_r, &format!("clear-{s}"), "\u{d7}", false, None).clicked() {
            if let Err(e) = app.run("mix.module_reference", json!({"track": host.track.0, "module": host.ins.id.0, "slot": s, "file": null})) {
                app.ui.status = e;
            }
        } else if resp.clicked() {
            set_one(app, host, "refSlot", s as f32);
        }
    }
}

/// What is happening: the reference tab's last lines.
fn ref_status(host: &Host, rd: &Readings) -> String {
    let listening = param(host, "reference") > 0.5;
    let selected = idx(host, "refSlot").min(3);
    let info = ref_info(host, selected);
    if info.loaded.is_none() {
        "Load a reference, click its row to select it, and flip Mix / Reference. It plays at your mix's loudness, and follows your song position while the transport runs, so you compare the same bar.".to_string()
    } else if listening {
        let mix = if rd.or(3, -100.0) > -69.0 { rd.or(3, -100.0) } else { rd.or(2, -100.0) };
        format!(
            "Playing reference {} at {} dB, to match your mix at {} LUFS. Your mix is still being measured underneath.",
            selected + 1,
            signed_db(rd.or(5, 0.0)),
            lufs_text(mix)
        )
    } else {
        format!("Listening to your mix. Flip to Reference to hear reference {} at the same loudness.", selected + 1)
    }
}

fn deliver_tab(app: &mut SoundApp, ui: &mut Ui, host: &Host, body: Rect, rd: &Readings) {
    let p = ui.painter().clone();
    let big_area = Rect::from_min_size(body.min, vec2(body.width(), 110.0));
    let (integ, tp, st_out) = (rd.or(3, -100.0), rd.or(4, -120.0), rd.or(2, -100.0));
    {
        let a = Rect::from_min_max(big_area.min, pos2(big_area.right() - 110.0, big_area.bottom()));
        let w = a.width() / 3.0;
        let big = |i: usize, value: String, label: &str, c: Color32| {
            let r = Rect::from_min_size(pos2(a.left() + w * i as f32, a.top()), vec2(w, a.height()));
            p.text(pos2(r.left(), r.top() + 25.0), Align2::LEFT_CENTER, value, regular(40.0), c);
            tracked_text(
                &p,
                Rect::from_min_size(pos2(r.left(), r.top() + 50.0), vec2(w, 16.0)),
                Align2::LEFT_CENTER,
                label,
                9.0,
                0.18,
                true,
                false,
                LABEL,
            );
        };
        big(0, lufs_text(integ), "LUFS integrated", TEXT);
        big(1, if tp <= -119.0 { "--".into() } else { format!("{tp:.1}") }, "dBTP true peak", if tp > -1.0 { BAD } else { TEXT });
        big(2, lufs_text(st_out), "LUFS short-term", DIM);
        if tp > -1.0 {
            p.text(
                pos2(big_area.left(), big_area.bottom() - 10.0),
                Align2::LEFT_CENTER,
                "True peak above -1 dBTP: MP3/AAC encoding can push this over and clip.",
                regular(11.0),
                BAD,
            );
        }
    }
    // Reset (`resetDelivery`) through `mix.module_action`
    let reset_r = Rect::from_center_size(pos2(big_area.right() - 50.0, big_area.center().y - 30.0), vec2(80.0, 22.0));
    reset_ui(app, ui, host, reset_r);
    // the platforms
    let a = Rect::from_min_max(pos2(body.left(), big_area.bottom() + 10.0), pos2(body.right(), body.bottom() - 32.0 - 10.0));
    let n = d::NUM_PLATFORMS - 1;
    let row_h = (a.height() / n as f32).trunc().min(30.0);
    let chosen = idx(host, "platform") as i32;
    for i in 1..d::NUM_PLATFORMS as i32 {
        let pf = d::platform(i);
        let row = Rect::from_min_size(pos2(a.left(), a.top() + row_h * (i - 1) as f32), vec2(a.width(), row_h));
        let r = row.shrink2(vec2(0.0, 2.0));
        platform_click(app, ui, host, row, i);
        let sel = i == chosen;
        p.rect_filled(r, CornerRadius::same(3), if sel { panel_sel() } else { PANEL });
        if sel {
            p.rect_stroke(r.shrink(0.5), CornerRadius::same(3), Stroke::new(1.0, al(ACCENT, 0.7)), StrokeKind::Middle);
        }
        let t = r.shrink2(vec2(12.0, 0.0));
        p.text(pos2(t.left(), t.center().y), Align2::LEFT_CENTER, pf.name, regular(13.0), if sel { ACCENT } else { TEXT });
        let target = platform_target(i);
        p.text(pos2(t.left() + 140.0, t.center().y), Align2::LEFT_CENTER, target, regular(11.0), DIM);
        let (verdict, vc) = platform_verdict(i, integ, tp);
        p.text(pos2(t.left() + 290.0, t.center().y), Align2::LEFT_CENTER, verdict, regular(13.0), vc);
    }
    // the foot: Preview, Dither, Shaped
    let foot = Rect::from_min_max(pos2(body.left(), body.bottom() - 32.0), body.max);
    let preview_r = Rect::from_min_size(foot.min, vec2(110.0, foot.height()));
    let dither_r = Rect::from_min_size(pos2(preview_r.right() + 24.0, foot.top()), vec2(260.0, foot.height()));
    let shaping_r = Rect::from_min_size(pos2(dither_r.right() + 8.0, foot.top()), vec2(90.0, foot.height()));
    deliver_foot(app, ui, host, preview_r, dither_r, shaping_r);
    if param(host, "preview") > 0.5 && chosen > 0 {
        let at = preview_r.translate(vec2(0.0, -22.0));
        let g = if rd.preview_gain.is_finite() { rd.preview_gain } else { 0.0 };
        p.text(pos2(at.left(), at.center().y), Align2::LEFT_CENTER, preview_text(chosen, g), regular(11.0), ACCENT);
    }
}

/// "Previewing Spotify: -3.2 dB".
fn preview_text(chosen: i32, gain: f32) -> String {
    format!("Previewing {}: {} dB", d::platform(chosen).name, signed_db(gain))
}

/// A platform's target: "-14 LUFS", ", down only" where it never turns up.
fn platform_target(i: i32) -> String {
    let pf = d::platform(i);
    format!("{} LUFS{}", pf.target_lufs as i32, if pf.turns_up { "" } else { ", down only" })
}

/// What platform `i` would do to the song at `integ` LUFS and `tp` dBTP, and its colour.
fn platform_verdict(i: i32, integ: f32, tp: f32) -> (String, Color32) {
    let pf = d::platform(i);
    if integ <= -69.0 {
        ("play the song through to measure".to_string(), LABEL)
    } else {
        let gdb = d::platform_gain_db(i, integ, tp);
        let wanted = pf.target_lufs - integ;
        if gdb < -0.05 {
            (format!("turned down {:.1} dB", -gdb), if gdb < -3.0 { WARN } else { BODY })
        } else if gdb > 0.05 {
            (format!("turned up {gdb:.1} dB{}", if gdb < wanted - 0.05 { " (peak-limited)" } else { "" }), BODY)
        } else if wanted > 0.05 && !pf.turns_up {
            (format!("left {wanted:.1} dB quieter than the rest"), WARN)
        } else {
            ("played as is".to_string(), BODY)
        }
    }
}

/// Platform `i`'s row `row`: a click picks it (or puts it back to none).
fn platform_click(app: &mut SoundApp, ui: &mut Ui, host: &Host, row: Rect, i: i32) {
    let chosen = idx(host, "platform") as i32;
    let resp = ui.interact(row, ui.id().with(("platform", i)), egui::Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, d::platform(i).name));
    if resp.clicked() {
        set_one(app, host, "platform", if chosen == i { 0.0 } else { i as f32 });
    }
}

/// Reset (`resetDelivery`) through `mix.module_action`.
fn reset_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    if pill(ui, r, "reset", "Reset", false, None).clicked() {
        action(app, host, "reset_delivery");
    }
}

/// The Deliver foot: Preview, Dither, Shaped.
fn deliver_foot(app: &mut SoundApp, ui: &mut Ui, host: &Host, preview_r: Rect, dither_r: Rect, shaping_r: Rect) {
    for (r, id, text) in [(preview_r, "preview", "Preview"), (shaping_r, "shaping", "Shaped")] {
        let on = param(host, id) > 0.5;
        if pill(ui, r, id, text, on, None).clicked() {
            set_one(app, host, id, if on { 0.0 } else { 1.0 });
        }
    }
    if let Some(i) = seg(ui, dither_r, "dither", &dsp::DITHERS, idx(host, "dither").min(2)) {
        set_one(app, host, "dither", i as f32);
    }
}

/// `filesDropped`: an audio file onto a reference row loads that slot; anywhere else the first
/// empty slot, then the selected one; then the Reference tab shows. (No file drop on the web.)
fn file_drop(app: &mut SoundApp, ui: &Ui, host: &Host, st: &mut Editor, ed: Rect, rows: [Rect; 4]) {
    #[cfg(not(target_arch = "wasm32"))]
    {
        let (files, at) = ui.input(|i| (i.raw.dropped_files.clone(), i.pointer.hover_pos()));
        let Some(at) = at.filter(|p| ed.contains(*p)) else { return };
        let audio = |p: &std::path::Path| {
            p.extension().and_then(|e| e.to_str()).is_some_and(|e| ["wav", "aif", "aiff", "flac", "mp3"].contains(&e.to_ascii_lowercase().as_str()))
        };
        let Some(path) = files.iter().map(|f| f.path().to_path_buf()).find(|p| audio(p)) else { return };
        let mut slot = if st.tab == 1 { rows.iter().position(|r| r.contains(at)) } else { None };
        if slot.is_none() {
            slot = (0..4).find(|s| ref_info(host, *s).name.is_empty());
        }
        let slot = slot.unwrap_or_else(|| idx(host, "refSlot").min(3));
        load_reference(app, host, slot, &path.display().to_string());
        st.tab = 1;
    }
    #[cfg(target_arch = "wasm32")]
    {
        let _ = (app, ui, host, st, ed, rows);
    }
}

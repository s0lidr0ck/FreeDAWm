//! Comp's VST editor (`Engine 1 - Comp/Source/PluginEditor.cpp`), translated: the layout of
//! `resized()`, the paint of `paint()` and of its components, `CompControls.h` ([`style_strip`],
//! [`gr_meter`], [`curve`]), `CompLaneMixer.h` ([`lane_mixer`]), `CompCrossover.h`
//! ([`crossover`]) and `CompConfig` ([`model`]), and every gesture. The palette is the family's
//! (`fs::look::apply (0xFF79D2BE)`), the same in both DAW themes. Its interaction inventory is
//! `docs/superpowers/editors/comp.md`.
//!
//! Every lane-bound control edits the lane picked in the mixer (`laneParam`). The meters are the
//! engine's own 30 Hz meters (`comp::LIVE_*`) and the crossover's spectrum is its input scope
//! (`analysis().pre`), both polled at 30 Hz as the C++ timer does. The VST's own Undo stack and
//! its 1 / 2 slots are kept in the editor; a restore is one `mix.module_params` (one DAW undo
//! step).

mod curve;
mod gr_meter;
mod lane_mixer;
mod model;
mod slim;
mod style_strip;

use crate::SoundApp;
use crate::editor_kit::controls::{
    DialOpts, LabelLayout, PillLook, PillStyle, SelectionStyle, crossover_id, dial, glyph_pill, pill_ex, pill_height, pill_width_ex,
    preset_name_button_ex, segmented_ex, segmented_id, tip,
};
use crate::editor_kit::crossover;
use crate::editor_kit::look::{TITLE_SIZE, font, paint_background, paint_title};
use crate::editor_kit::presets::{self, PresetState, RECALL_GESTURE};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, set_params, set_params_merged};
use egui::{Align2, Color32, Painter, Pos2, Rect, Stroke, Ui, Vec2, pos2, vec2};
use model::CompValues;
use soundcraft_dsp::plugins::fs::comp as dsp;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

/// `setSize (960, 620)`.
pub const SIZE: Vec2 = vec2(960.0, 620.0);
/// `kAccent`: this engine's one colour.
const ACCENT: Color32 = argb(0xFF79D2BE);
/// `look::apply (kAccent)`.
pub const PALETTE: VstPalette = VstPalette::engine_look(ACCENT);

const PAD_X: f32 = 22.0;
const PAD_Y: f32 = 12.0;
const GAP: f32 = 10.0;
const HEADER_H: f32 = 30.0;
const STRIP_H: f32 = 34.0;
const BLURB_H: f32 = 30.0;
const MIXER_W: f32 = 214.0;
const VIEW_MIN: f32 = 170.0;
const VIEW_MAX: f32 = 330.0;
const BIG_KNOB: f32 = 76.0;
const BIG_CELL: f32 = 122.0;
const BIG_ROW_H: f32 = 118.0;
const SMALL_KNOB: f32 = 46.0;
const SMALL_CELL: f32 = 150.0;
const SIDE_ROW_H: f32 = 54.0;

const BIG_IDS: [&str; 5] = ["amount", "ratio", "attack", "release", "makeup"];
const SMALL_IDS: [&str; 4] = ["scHpf", "color", "knee", "link"];

/// `CompEngineDSP::laneParam`: a lane-A field's parameter on lane 0, 1, 2.
const LANE_IDS: [(&str, [&str; 3]); 14] = [
    ("style", ["style", "bStyle", "cStyle"]),
    ("amount", ["amount", "bAmount", "cAmount"]),
    ("ratio", ["ratio", "bRatio", "cRatio"]),
    ("attack", ["attack", "bAttack", "cAttack"]),
    ("release", ["release", "bRelease", "cRelease"]),
    ("makeup", ["makeup", "bMakeup", "cMakeup"]),
    ("knee", ["knee", "bKnee", "cKnee"]),
    ("scHpf", ["scHpf", "bScHpf", "cScHpf"]),
    ("color", ["color", "bColor", "cColor"]),
    ("link", ["link", "bLink", "cLink"]),
    ("autoGain", ["autoGain", "bAutoGain", "cAutoGain"]),
    ("variant", ["variant", "bVariant", "cVariant"]),
    ("detMode", ["detMode", "bDetMode", "cDetMode"]),
    ("level", ["aLevel", "bLevel", "cLevel"]),
];
const LANE_ON: [&str; 3] = ["aOn", "bOn", "cOn"];

fn lane_param(base: &str, lane: usize) -> &'static str {
    LANE_IDS.iter().find(|(b, _)| *b == base).and_then(|(_, ids)| ids.get(lane.min(2)).copied()).unwrap_or("style")
}

/// The circuit name as the mixer strip shows it (`styleShortName`).
fn style_short_name(style: i32) -> &'static str {
    ["Clean", "1176", "LA-2A", "160", "G Bus", "Vari-Mu", "Ross", "EL8", "2500", "33609"][style.clamp(0, 9) as usize]
}

fn style_of(v: f32) -> i32 {
    if v.is_finite() { (v.round() as i32).clamp(0, 9) } else { 0 }
}

/// The cache of the lane on screen (`cached`, reseeded from the selected lane).
fn values(host: &Host, lane: usize) -> CompValues {
    let g = |b: &str| param(host, lane_param(b, lane));
    CompValues {
        style: style_of(g("style")),
        amount: g("amount"),
        ratio: g("ratio"),
        attack: g("attack"),
        release: g("release"),
        makeup: g("makeup"),
        knee: g("knee"),
        sc_hpf: g("scHpf"),
        color: g("color"),
        link: g("link"),
        variant: g("variant").round() as i32,
        det_mode: g("detMode").round() as i32,
        chan_mode: param(host, "chanMode").round() as i32,
    }
}

/// A snapshot of every preset parameter (`snapshot`: all but the bypass).
type Snapshot = Vec<(&'static str, f32)>;

fn snapshot(host: &Host) -> Snapshot {
    host.info.params.iter().filter(|p| p.id != "bypass").map(|p| (p.id, param(host, p.id))).collect()
}

/// Comp's editor: everything the C++ editor holds that is not a parameter.
#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    /// `selectedLane`, 0..2.
    lane: usize,
    undo: Vec<Snapshot>,
    slot_a: Option<Snapshot>,
    slot_b: Option<Snapshot>,
    /// `abActive == "B"`.
    on_b: bool,
    meter: gr_meter::Meter,
    lane_gr: [f32; 3],
    tick_at: f64,
    spectrum: crossover::Spectrum,
    curve: curve::CurveCache,
    mixer_drag: Option<usize>,
    xover_drag: Option<usize>,
    presets: PresetState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's one display: the transfer curve (true) or the gain-reduction meter.
    slim_curve: bool,
    /// Scripts: open the slim layout's model menu on the next frame.
    models_open: bool,
    /// The slim layout's More section is open.
    slim_more: bool,
}

/// Draws Comp's editor for `module` on `track`.
/// What `getPresetParams` leaves out besides `bypass` (CompEngine's `Presets.h`).
const NOT_IN_PRESETS: [&str; 0] = [];

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_comp";

    fn size(&self, _ins: &Insert) -> Vec2 {
        if self.slim { slim::size(self.slim_h) } else { SIZE }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

// ============================================================================ helpers

/// `drawSingleLineText`: `text` with its baseline at `at.y`, from `at.x`.
pub(super) fn baseline_text(p: &Painter, at: Pos2, text: &str, f: egui::FontId, c: Color32) {
    let size = f.size;
    p.text(pos2(at.x, at.y - size * 0.95), Align2::LEFT_TOP, text, f, c);
}

/// A stroked open path.
pub(super) fn polyline(p: &Painter, pts: Vec<Pos2>, stroke: Stroke) {
    if pts.len() >= 2 {
        p.add(egui::Shape::line(pts, stroke));
    }
}

fn small_pill(ui: &mut Ui, r: Rect, salt: &str, text: &str, on: bool, dimmed: bool, lit: Option<Color32>) -> egui::Response {
    let look = PillLook { active_colour: lit, tracked: false, dimmed };
    pill_ex(ui, r, salt, &PALETTE, text, on, PillStyle::Small, look)
}

fn small_w(ui: &Ui, text: &str) -> f32 {
    pill_width_ex(ui, text, PillStyle::Small, false)
}

impl Editor {
    /// `pushUndo`.
    fn push_undo(&mut self, host: &Host) {
        self.undo.push(snapshot(host));
        if self.undo.len() > 64 {
            self.undo.remove(0);
        }
    }

    /// `restore`: style first in the C++; here one command sets them all.
    fn restore(app: &mut SoundApp, host: &Host, v: &Snapshot) {
        end_gesture(app);
        set_params(app, host, v);
    }
}

// ============================================================================ the editor

/// What both layouts do before they paint: the scripted drawer tabs, the 30 Hz timer, and the
/// preset browser, which covers the whole editor while it is open (then `true`: paint nothing
/// else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    let ed = host.rect;
    if !st.init {
        st.init = true;
        st.presets.category = "All".into();
        st.meter = gr_meter::Meter::default();
    }
    // `ui.mix.drawer {tab}` (scripts, tests): "Presets" opens the browser; "Lane A".."Lane C"
    // selects the lane on screen, as a press on its mixer header does.
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id) {
        match tab.as_str() {
            "Presets" => presets::open(app, host, &mut st.presets),
            "Lane A" => st.lane = 0,
            "Lane B" => st.lane = 1,
            "Lane C" => st.lane = 2,
            "Models" if st.slim => st.models_open = true,
            "More" if st.slim => st.slim_more = true,
            _ => {}
        }
    }

    // ---- the 30 Hz timer: the meters, the lane meters and the spectrum
    let now = ui.input(|i| i.time);
    let live = crate::editor_kit::live(host);
    if now - st.tick_at >= 1.0 / 30.0 || now < st.tick_at {
        st.tick_at = now;
        if let Some(l) = live {
            st.meter.set_values(l.get(dsp::LIVE_GR), l.get(dsp::LIVE_IN), l.get(dsp::LIVE_OUT));
            for (i, g) in st.lane_gr.iter_mut().enumerate() {
                let v = l.get(dsp::LIVE_LANE_GR + i);
                *g = if v.is_finite() { v } else { 0.0 };
            }
        }
        let ring = host.meters.and_then(|m| m.analysis.iter().find(|(id, _)| *id == host.ins.id)).map(|(_, a)| a.clone());
        if let Some(a) = ring
            && a.pre.written() > 0
        {
            st.spectrum.update(&a.pre, host.sr);
        }
    }
    if live.is_some() {
        ui.ctx().request_repaint();
    }

    // ---- the preset browser covers the whole editor while it is open
    st.presets.not_in_presets = &NOT_IN_PRESETS;
    if st.presets.open {
        let out = presets::preset_browser(app, ui, host, ed, &PALETTE, &mut st.presets);
        let pasted = presets::clipboard(app, ui, host, &mut st.presets, &out);
        after_recall(app, host, out.params.as_ref().or(pasted.as_ref()), out.recalled.is_some() || pasted.is_some());
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
    paint_background(&p, ed);

    let mut b = ed.shrink2(vec2(PAD_X, PAD_Y));
    let take_top = |b: &mut Rect, h: f32| {
        let r = Rect::from_min_size(b.min, vec2(b.width(), h));
        b.min.y += h;
        r
    };
    let header = take_top(&mut b, HEADER_H);
    header_ui(st, app, ui, host, header);
    b.min.y += GAP;
    let strip = take_top(&mut b, STRIP_H);
    b.min.y += 6.0;
    let blurb = take_top(&mut b, BLURB_H);
    b.min.y += GAP;

    let v = values(host, st.lane);

    // ---- style strip
    if let Some(i) = style_strip::style_strip(ui, strip, &PALETTE, v.style as usize) {
        pick_style(st, app, host, i);
    }
    // ---- style blurb, centred under the strip
    paint_blurb(ui, blurb, v.style, 2);

    // ---- meter, curve and the lane mixer, side by side
    let view_h = (b.height() - (GAP + BIG_ROW_H + GAP + SIDE_ROW_H)).clamp(VIEW_MIN, VIEW_MAX);
    let view = take_top(&mut b, view_h);
    {
        let column = Rect::from_min_max(pos2(view.right() - MIXER_W, view.top()), view.max);
        let split_r = Rect::from_min_size(column.min, vec2(column.width(), 22.0));
        let xover_r = Rect::from_min_max(pos2(column.left(), column.bottom() - 56.0), column.max);
        let mixer_r = Rect::from_min_max(pos2(column.left(), split_r.bottom() + 4.0), pos2(column.right(), xover_r.top() - 4.0));
        let rest = Rect::from_min_max(view.min, pos2(column.left() - GAP, view.bottom()));
        let meter_w = (rest.width() * 1.15 / 2.15).round();
        let meter_r = Rect::from_min_size(rest.min, vec2(meter_w, rest.height()));
        let curve_r = Rect::from_min_max(pos2(meter_r.right() + GAP, rest.top()), rest.max);

        gr_meter::paint(&p, meter_r, ACCENT, &st.meter);
        curve::paint(&p, curve_r, ACCENT, &v, &mut st.curve, st.meter.input, st.meter.output, st.meter.gr);

        let split = split_ui(st, app, ui, host, split_r);
        mixer(st, app, ui, host, mixer_r, split);
        crossover_ui(st, app, ui, host, xover_r, split);
    }
    b.min.y += GAP;

    // ---- big knob row, centred as a group
    let area = take_top(&mut b, BIG_ROW_H);
    let bigs: Vec<&str> = BIG_IDS.iter().copied().filter(|id| model::visible(id, v.style)).collect();
    let mut x = area.center().x - (bigs.len() as f32 * BIG_CELL / 2.0).trunc();
    for id in bigs {
        knob(st, app, ui, host, Rect::from_min_size(pos2(x, area.top()), vec2(BIG_CELL, BIG_KNOB + 36.0)), id, &v, false);
        x += BIG_CELL;
    }
    b.min.y += GAP;

    // ---- side row: style switches, the small knobs, Auto gain, centred as one group
    let area = take_top(&mut b, SIDE_ROW_H);
    let smalls: Vec<&str> = SMALL_IDS.iter().copied().filter(|id| model::visible(id, v.style)).collect();
    let (_, vopts) = model::switch_def("variant", v.style);
    let (_, dopts) = model::switch_def("detMode", v.style);
    let seg_w = |n: usize, visible: bool| if visible { (n as f32 * 66.0).clamp(90.0, 300.0) } else { 0.0 };
    let variant_w = seg_w(3, vopts.len() > 1);
    let detector_w = seg_w(4, dopts.len() > 1);
    let auto_w = small_w(ui, "Auto gain").max(92.0);
    let mut total = smalls.len() as f32 * SMALL_CELL + auto_w;
    if variant_w > 0.0 {
        total += variant_w + GAP;
    }
    if detector_w > 0.0 {
        total += detector_w + GAP;
    }
    total += GAP;
    let mut x = area.left().max(area.center().x - (total / 2.0).trunc());
    let seg_h = 24.0;
    for (base, w) in [("variant", variant_w), ("detMode", detector_w)] {
        if w == 0.0 {
            continue;
        }
        switch_ui(st, app, ui, host, Rect::from_min_size(pos2(x, area.top() + 4.0), vec2(w, seg_h)), base, &v);
        x += w + GAP;
    }
    for id in smalls {
        knob(st, app, ui, host, Rect::from_min_size(pos2(x, area.top()), vec2(SMALL_CELL, SMALL_KNOB + 6.0)), id, &v, true);
        x += SMALL_CELL;
    }
    x += GAP;
    let ph = pill_height(PillStyle::Small);
    auto_gain_ui(st, app, ui, host, Rect::from_min_size(pos2(x, area.top() + 4.0), vec2(auto_w, ph)));
}

// ============================================================================ shared sections
// The pieces both layouts draw, each in the rectangle its layout gives it.

/// The style picker's pick: the selected lane's circuit, after an Undo snapshot.
fn pick_style(st: &mut Editor, app: &mut SoundApp, host: &Host, i: usize) {
    st.push_undo(host);
    set_params(app, host, &[(lane_param("style", st.lane), i as f32)]);
}

/// The hardware credit line of the style named `name` (`crate::hardware_credit`).
pub(crate) fn style_credit(name: &str) -> &'static str {
    model::STYLES.iter().find(|s| s.name == name).map_or("", |s| s.credit)
}

/// The style's blurb, centred in `r`, at most `max_rows` lines.
fn paint_blurb(ui: &Ui, r: Rect, style: i32, max_rows: usize) {
    let def = model::STYLES.get(style.clamp(0, 9) as usize);
    let text = def.map_or("", |s| s.blurb);
    crate::hardware_credit::hover(ui, r, "comp_style", text, def.map_or("", |s| s.credit));
    let mut job = egui::text::LayoutJob::simple(text.to_string(), font(11.5, false), PALETTE.text_body, r.width());
    job.halign = egui::Align::Center;
    job.wrap.max_rows = max_rows;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let p = ui.painter();
    p.with_clip_rect(r.intersect(p.clip_rect())).galley(pos2(r.center().x, r.top()), galley, PALETTE.text_body);
}

/// The Parallel / Multiband / Serial switch in `r`; returns the split shown.
fn split_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) -> i64 {
    let split = param(host, "split").round() as i64;
    if let Some(i) =
        segmented_ex(ui, r, "split", &PALETTE, &["Parallel", "Multiband", "Serial"], 1, usize::try_from(split).ok(), SelectionStyle::Filled, false)
    {
        st.push_undo(host);
        set_params(app, host, &[("split", i as f32)]);
    }
    tip(
        ui,
        segmented_id(ui, "split"),
        "Parallel: every lane hears the whole signal, and they are mixed. Multiband: A gets the lows, B the mids, C the highs,              split at the crossovers below. Serial: A feeds B feeds C, one after the other.",
    );
    split
}

/// The crossover bar (A low / B mid / C high) in `r` and its drags.
fn crossover_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, split: i64) {
    let xv = crossover::View {
        spec: crossover::Spec { names: ["A  low", "B  mid", "C  high"], default_lo: 0.467, default_hi: 0.42 },
        lo_knob: param(host, "xoverLo"),
        hi_knob: param(host, "xoverHi"),
        active: split == dsp::comp_engine_dsp::SPLIT_MULTIBAND,
        band_gr: st.lane_gr,
        spectrum: &st.spectrum,
    };
    let mut drag = st.xover_drag;
    let out = crossover::crossover(ui, r, &PALETTE, &xv, &mut drag);
    tip(ui, crossover_id(ui), "Multiband crossovers. Drag a divider to move it, scroll to nudge, double-click to reset.");
    st.xover_drag = drag;
    let id = |d: usize| if d == 0 { "xoverLo" } else { "xoverHi" };
    if out.gesture_start.is_some() {
        st.push_undo(host);
        end_gesture(app);
    }
    if let Some((d, nv)) = out.value {
        set_params_merged(app, host, &[(id(d), nv)], &format!("xover-{d}"));
    }
    if out.gesture_end.is_some() {
        end_gesture(app);
    }
}

/// One of the style's switches (`variant`, `detMode`) in `r`, its caption under it; nothing when
/// the style has none.
fn switch_ui(st: &Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, base: &'static str, v: &CompValues) {
    let (label, opts) = model::switch_def(base, v.style);
    if opts.is_empty() {
        return;
    }
    let sel = if base == "variant" { v.variant } else { v.det_mode };
    let selected = usize::try_from(sel.clamp(0, opts.len() as i32 - 1)).ok();
    if let Some(i) = segmented_ex(ui, r, base, &PALETTE, opts, 1, selected, SelectionStyle::Filled, false) {
        set_params(app, host, &[(lane_param(base, st.lane), i as f32)]);
    }
    let cap = Rect::from_min_size(pos2(r.left(), r.bottom() + 2.0), vec2(r.width(), 12.0));
    ui.painter().text(cap.center(), Align2::CENTER_CENTER, label, font(10.0, false), PALETTE.text_body);
}

/// The Auto gain pill in `r`, "matches loudness" under it.
fn auto_gain_ui(st: &Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let auto_id = lane_param("autoGain", st.lane);
    let on = param(host, auto_id) > 0.5;
    if small_pill(ui, r, "autoGain", "Auto gain", on, false, None).clicked() {
        set_params(app, host, &[(auto_id, if on { 0.0 } else { 1.0 })]);
    }
    let cap = Rect::from_min_size(pos2(r.left(), r.bottom() + 2.0), vec2(r.width(), 12.0));
    ui.painter().text(cap.center(), Align2::CENTER_CENTER, "matches loudness", font(9.0, false), PALETTE.text_faint);
}

/// After a recall (`loadPreset`): presets from before Multiband do not mention it; they load in
/// Parallel at the default crossovers, joining the recall's undo step.
fn after_recall(app: &mut SoundApp, host: &Host, params: Option<&serde_json::Map<String, serde_json::Value>>, recalled: bool) {
    if let Some(m) = params {
        let missing: Vec<(&str, f32)> = ["split", "xoverLo", "xoverHi"]
            .into_iter()
            .filter(|k| !m.contains_key(*k))
            .filter_map(|k| host.info.params.iter().find(|p| p.id == k).map(|p| (p.id, p.default)))
            .collect();
        if !missing.is_empty() {
            set_params_merged(app, host, &missing, RECALL_GESTURE);
        }
    }
    if recalled {
        end_gesture(app);
    }
}

/// The header: title, preset strip, Channels, Undo, 1, 2, Bypass.
fn header_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, header: Rect) {
    let p = ui.painter().clone();
    paint_title(&p, header, "Comp", "Engine", &PALETTE);
    let btn_h = pill_height(PillStyle::Small);
    let mut row = header;
    let place_right = |row: &mut Rect, w: f32| {
        let r = Rect::from_min_size(pos2(row.right() - w, row.center().y - btn_h / 2.0), vec2(w, btn_h));
        row.max.x -= w + 6.0;
        r
    };
    let bypass_r = place_right(&mut row, small_w(ui, "Bypass"));
    let b_r = place_right(&mut row, small_w(ui, "2").max(28.0));
    let a_r = place_right(&mut row, small_w(ui, "1").max(28.0));
    row.max.x -= 2.0;
    let undo_r = place_right(&mut row, small_w(ui, "Undo"));
    let chan = param(host, "chanMode").round() as i32;
    let chan_text = model::channel_mode_name(chan);
    let chan_r = place_right(&mut row, small_w(ui, chan_text).max(74.0));

    // the title claims its own width on the left, the strip centres in what is left
    let title_w = p.layout_no_wrap("Comp Engine".into(), font(TITLE_SIZE, true), Color32::WHITE).size().x + 14.0;
    row.min.x += (title_w + 12.0).trunc();
    let arrow_w = 24.0;
    let name_w = (row.width() - 2.0 * arrow_w - 24.0).trunc().clamp(140.0, 260.0);
    let strip = Rect::from_center_size(row.center(), vec2(name_w + 2.0 * arrow_w + 12.0, HEADER_H));
    let prev = Rect::from_center_size(pos2(strip.left() + arrow_w / 2.0, strip.center().y), vec2(arrow_w, btn_h));
    let next = Rect::from_center_size(pos2(strip.right() - arrow_w / 2.0, strip.center().y), vec2(arrow_w, btn_h));
    let name_r = Rect::from_min_max(
        pos2(prev.right() + 6.0, strip.center().y - (btn_h + 2.0) / 2.0),
        pos2(next.left() - 6.0, strip.center().y + (btn_h + 2.0) / 2.0),
    );
    let rects = HeaderRects { bypass: bypass_r, slot_1: a_r, slot_2: b_r, undo: undo_r, chan: chan_r, prev, next, name: name_r };
    header_controls(st, app, ui, host, &rects);
}

/// Where the header's controls go (each layout places them).
pub(super) struct HeaderRects {
    pub bypass: Rect,
    pub slot_1: Rect,
    pub slot_2: Rect,
    pub undo: Rect,
    pub chan: Rect,
    pub prev: Rect,
    pub next: Rect,
    pub name: Rect,
}

/// The header's controls and their gestures: Bypass, the 1 / 2 slots, Undo, Channels and the
/// preset strip.
fn header_controls(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, h: &HeaderRects) {
    let (bypass_r, a_r, b_r, undo_r, chan_r, prev, next, name_r) = (h.bypass, h.slot_1, h.slot_2, h.undo, h.chan, h.prev, h.next, h.name);
    let chan = param(host, "chanMode").round() as i32;
    let chan_text = model::channel_mode_name(chan);
    let byp = param(host, "bypass") > 0.5;
    if small_pill(ui, bypass_r, "bypass", "Bypass", byp, false, Some(PALETTE.bypass_red)).clicked() {
        set_params(app, host, &[("bypass", if byp { 0.0 } else { 1.0 })]);
    }
    // `selectAbSlot`: stash what is on screen into the slot being left, recall the other.
    for (r, b) in [(a_r, false), (b_r, true)] {
        if small_pill(ui, r, if b { "slot-2" } else { "slot-1" }, if b { "2" } else { "1" }, st.on_b == b, false, None).clicked() && st.on_b != b {
            let here = snapshot(host);
            if st.on_b {
                st.slot_b = Some(here);
            } else {
                st.slot_a = Some(here);
            }
            st.on_b = b;
            if let Some(v) = if b { st.slot_b.clone() } else { st.slot_a.clone() } {
                Editor::restore(app, host, &v);
            }
        }
    }
    if small_pill(ui, undo_r, "undo", "Undo", false, st.undo.is_empty(), None).clicked()
        && let Some(v) = st.undo.pop()
    {
        Editor::restore(app, host, &v);
    }
    if small_pill(ui, chan_r, "chanMode", chan_text, chan > 0, false, None).clicked() {
        set_params(app, host, &[("chanMode", ((chan.clamp(0, 3) + 1) % 4) as f32)]);
    }

    for (r, left) in [(prev, true), (next, false)] {
        if glyph_pill(ui, r, ("step", left), &PALETTE, left, PillStyle::Small).clicked() {
            let out = presets::step(app, host, &mut st.presets, if left { -1 } else { 1 });
            after_recall(app, host, out.params.as_ref(), out.recalled.is_some());
        }
    }
    let shown = presets::shown_name(host);
    let dirty = presets::is_dirty(app, host, &mut st.presets);
    if preset_name_button_ex(ui, name_r, "name", &PALETTE, shown, dirty).clicked() {
        presets::open(app, host, &mut st.presets);
    }
}

/// A knob (`addKnob` / `attachKnob`): bound to the selected lane, its readout and label from the
/// model, `pushUndo` on the drag start, stepped knobs snapped to their positions.
#[allow(clippy::too_many_arguments)]
fn knob(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, base: &'static str, v: &CompValues, small: bool) {
    knob_laid(st, app, ui, host, r, base, v, small, if small { LabelLayout::Right } else { LabelLayout::Below });
}

/// [`knob`] with its readout placed by `layout` (the slim layout stacks even the small ones).
#[allow(clippy::too_many_arguments)]
fn knob_laid(
    st: &mut Editor,
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    r: Rect,
    base: &'static str,
    v: &CompValues,
    small: bool,
    layout: LabelLayout,
) {
    let id = lane_param(base, st.lane);
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let text = model::format(base, v);
    let o = DialOpts {
        label: model::label(base, v.style),
        value_text: &text,
        default: info.to_normalized(info.default),
        layout,
        small,
        ..DialOpts::default()
    };
    let out = dial(ui, r, base, &PALETTE, info.to_normalized(param(host, id)), &o);
    if out.started {
        st.push_undo(host);
        end_gesture(app);
    }
    if let Some(mut nv) = out.value {
        let n = model::steps(base, v.style);
        if n > 1 {
            nv = (nv * (n - 1) as f32).round() / (n - 1) as f32;
        }
        set_params_merged(app, host, &[(id, info.clamp(info.from_normalized(nv)))], &format!("dial-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
}

/// The lane mixer (`laneMixer`) and what its gestures do.
fn mixer(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect, split: i64) {
    let strip =
        |level: &str, on: &str, gr: f32, style: &'static str| lane_mixer::Strip { level: param(host, level), gr, on: param(host, on) > 0.5, style };
    let view = lane_mixer::View {
        strips: [
            strip("dry", "dryOn", 0.0, "no comp"),
            strip("aLevel", "aOn", st.lane_gr[0], style_short_name(style_of(param(host, "style")))),
            strip("bLevel", "bOn", st.lane_gr[1], style_short_name(style_of(param(host, "bStyle")))),
            strip("cLevel", "cOn", st.lane_gr[2], style_short_name(style_of(param(host, "cStyle")))),
        ],
        selected: st.lane + 1,
        multiband: split == dsp::comp_engine_dsp::SPLIT_MULTIBAND,
        serial: split == dsp::comp_engine_dsp::SPLIT_SERIAL,
    };
    let mut drag = st.mixer_drag;
    let out = lane_mixer::lane_mixer(ui, r, &PALETTE, &view, &mut drag);
    st.mixer_drag = drag;
    let level_id = |s: usize| if s == 0 { "dry" } else { lane_param("level", s - 1) };
    if let Some(s) = out.select
        && s > 0
    {
        st.lane = (s - 1).min(2);
    }
    if let Some(s) = out.toggle_on {
        let id = if s == 0 { "dryOn" } else { LANE_ON[(s - 1).min(2)] };
        st.push_undo(host);
        set_params(app, host, &[(id, if param(host, id) > 0.5 { 0.0 } else { 1.0 })]);
    }
    if out.gesture_start.is_some() {
        st.push_undo(host);
        end_gesture(app);
    }
    if let Some((s, lv)) = out.level {
        set_params_merged(app, host, &[(level_id(s), lv.clamp(0.0, 1.0))], &format!("lane-level-{s}"));
    }
    if out.gesture_end.is_some() {
        end_gesture(app);
    }
}

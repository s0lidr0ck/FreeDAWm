//! DelayEngine's VST editor (`Engine 6 - Delay/Source/PluginEditor.cpp`, `DelayConfig.h`),
//! translated: `resized()`'s geometry (constants by name), `paint()`, the family header with its
//! own Undo and A/B, the style row and its blurb, the Heads row (Tape and Drum), the four main
//! dials (Sync swaps Time for the Note), the six drawer tabs, the foot (Sync, Tap, the tempo
//! readout, Reverse, Freeze) and FireStation's preset browser. The palette is the family's
//! (`fs::look::apply (0xFFFFB454)`), the same in both DAW themes.
//!
//! What the VST's 20 Hz timer reads comes from the engine's readouts (`delay/mod.rs`): `[2]` the
//! tempo in force, `[3]` the gate lamp, `[4]`/`[5]` the tap sequence and tempo (the tap
//! follow-up, Tempo Src Manual / Manual BPM / Sync on, is the app's `delay_tap_watch`, which runs
//! whether or not an editor is open). Interaction inventory: `docs/superpowers/editors/delay.md`.

use crate::SoundApp;
use crate::editor_kit::controls::{DialOpts, led};
use crate::editor_kit::controls::{PillLook, PillStyle, SelectionStyle, pill_ex};
use crate::editor_kit::family::{self, Family, FamilyState, dial_attached, pill_param, segmented_param};
use crate::editor_kit::look::{BLURB_SIZE, CAPTION_SIZE, font, paint_background, paint_panel};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, readouts, set_params_merged};
use egui::{Align2, Pos2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::delay::delay_config as dc;
use soundcraft_dsp::plugins::fs::delay::delay_engine_dsp as dd;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

mod slim;

// ---------------------------------------------------------------------------- constants

const K_PAD_X: f32 = 22.0;
const K_PAD_Y: f32 = 14.0;
const K_GAP: f32 = 13.0;
const K_HEADER_H: f32 = 26.0;
const K_STYLE_ROW_H: f32 = 30.0;
const K_BLURB_H: f32 = 16.0;
const K_HEADS_ROW_H: f32 = 28.0;
const K_MAIN_KNOB: f32 = 104.0;
/// Knob, then value and name under it.
const K_MAIN_ROW_H: f32 = K_MAIN_KNOB + 32.0;
const K_TABS_H: f32 = 24.0;
const K_PANEL_H: f32 = 146.0;
const K_FOOT_H: f32 = 24.0;
const K_DEFAULT_WIDTH: f32 = 900.0;

/// `fs::look::apply (kAccent)`, the original UI's `PEDAL.accent`.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFFF_B454));

/// `getPresetParams()` (`Presets.h`): what Undo, A/B, Copy and the dirty mark cover.
pub const PRESET_PARAMS: [&str; 28] = [
    "style",
    "time",
    "feedback",
    "mix",
    "routing",
    "sync",
    "division",
    "lowCut",
    "highCut",
    "character",
    "modRate",
    "modDepth",
    "offset",
    "heads",
    "diffusion",
    "duck",
    "swing",
    "pitch",
    "freeze",
    "width",
    "threshold",
    "focus",
    "reverse",
    "noise",
    "filtMod",
    "pattern",
    "modSync",
    "divisionR",
];

/// The family's preset facts: `storageKey`, and what `getPresetParams` leaves out.
const FAM: Family = Family::new("DelayEngine", &["trim", "level", "tempo", "tempoSrc", "tap", "trails"]);

/// The family's plain (untracked) `Pill::Small`.
const PLAIN: PillLook = PillLook { active_colour: None, tracked: false, dimmed: false };

/// The drawer's tabs (`Tab`).
pub const TABS: [&str; 6] = ["Tone", "Dynamics", "Motion", "Rhythm", "Stereo", "Setup"];
const TAB_TONE: usize = 0;
const TAB_DYNAMICS: usize = 1;
const TAB_MOTION: usize = 2;
const TAB_RHYTHM: usize = 3;
const TAB_STEREO: usize = 4;
const TAB_SETUP: usize = 5;

/// How long `tap()` holds Tap at 1 (`callAfterDelay (60)`).
const TAP_HOLD_S: f64 = 0.06;

// ---------------------------------------------------------------------------- state

/// The editor's own state: the open tab (`activeTab`, pure UI state), the family header's, and
/// a Tap being held.
#[derive(Clone, Default, Debug)]
pub(crate) struct Editor {
    pub tab: usize,
    fam: FamilyState,
    tap_since: Option<f64>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section (the drawer's tabs) is open.
    slim_more: bool,
    /// Scripts: open the slim layout's style menu on the next frame.
    styles_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn model(ins: &Insert, id: &str) -> i32 {
    crate::module_drawer::model_index(ins, id)
}

/// `whatApplies` for the module's current values.
fn applies(ins: &Insert) -> dc::Applicable {
    dc::what_applies(
        model(ins, "style"),
        model(ins, "routing"),
        model(ins, "sync") == 1,
        model(ins, "pattern"),
        model(ins, "reverse") == 1,
        model(ins, "divisionR"),
    )
}

/// `requiredHeight()`: 496, or 533 with the Heads row.
pub fn required_height(ins: &Insert) -> f32 {
    let mut h = K_PAD_Y + K_HEADER_H + K_GAP + K_STYLE_ROW_H + 4.0 + K_BLURB_H;
    if applies(ins).heads {
        h += 9.0 + K_HEADS_ROW_H;
    }
    h + K_GAP + K_MAIN_ROW_H + K_GAP + K_TABS_H + 10.0 + K_PANEL_H + K_GAP + K_FOOT_H + K_PAD_Y
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_delay";

    fn size(&self, ins: &Insert) -> Vec2 {
        if self.slim { crate::editor_kit::slim::size(self.slim_h) } else { vec2(K_DEFAULT_WIDTH, required_height(ins)) }
    }

    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
        if self.slim {
            slim::editor_ui(self, app, ui, host);
        } else {
            editor_ui(self, app, ui, host);
        }
    }
}

// ---------------------------------------------------------------------------- the tempo

/// `processor.getHostBpm()`: the session's tempo (`0` when it has none worth the name).
fn host_bpm(host: &Host) -> f32 {
    let t = host.session.tempo.tempo_at_tick(0) as f32;
    if t.is_finite() { t } else { 0.0 }
}

/// `processor.getEffectiveBpm()`: the engine's tempo in force (readout `[2]`) while it plays,
/// else what it would take: the session's on Host (20 BPM or more), else the Manual BPM.
fn effective_bpm(host: &Host, ro: Option<[f32; 8]>) -> f32 {
    if let Some(b) = ro.map(|r| r[2]).filter(|b| b.is_finite() && *b > 0.0) {
        return b;
    }
    let hb = host_bpm(host);
    if param(host, "tempoSrc") < 0.5 && hb >= 20.0 { hb } else { dd::knob_to_bpm(param(host, "tempo")) }
}

/// `formatValue (index, v, bpm, modSyncOn)` for parameter `id` at its real value `v`.
fn value_text(host: &Host, id: &str, v: f32, bpm: f32) -> String {
    match id {
        "division" => dc::division_text(v, bpm),
        "divisionR" => dc::DIVISIONS_R.get(v.round().clamp(0.0, 16.0) as usize).copied().unwrap_or_default().to_string(),
        "modRate" if param(host, "modSync") > 0.5 => dc::mod_rate_sync_text(v),
        _ => host.info.params.iter().find(|p| p.id == id).map_or_else(String::new, |p| p.format(v)),
    }
}

// ---------------------------------------------------------------------------- resized + paint

/// `resized()`'s areas.
struct Layout {
    header: Rect,
    style: Rect,
    blurb: Rect,
    heads: Option<Rect>,
    main: Rect,
    tabs: Rect,
    drawer: Rect,
    /// `drawerArea.reduced (16, 12)` less the help line and its gap.
    panel: Rect,
    help: Rect,
    foot: Rect,
}

fn layout(r: Rect, heads: bool) -> Layout {
    let mut b = r.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let mut take = |h: f32| {
        let t = Rect::from_min_size(b.min, vec2(b.width(), h));
        b.min.y += h;
        t
    };
    let header = take(K_HEADER_H);
    take(K_GAP);
    let style = take(K_STYLE_ROW_H);
    take(4.0);
    let blurb = take(K_BLURB_H);
    let heads = heads.then(|| {
        take(9.0);
        let row = take(K_HEADS_ROW_H);
        Rect::from_center_size(row.center(), vec2(row.width().min(420.0), K_HEADS_ROW_H))
    });
    take(K_GAP);
    let main = take(K_MAIN_ROW_H);
    take(K_GAP);
    let tabs = take(K_TABS_H);
    take(10.0);
    let drawer = take(K_PANEL_H);
    take(K_GAP);
    let foot = take(K_FOOT_H);
    let inner = drawer.shrink2(vec2(16.0, 12.0));
    let help = Rect::from_min_max(pos2(inner.left(), inner.bottom() - 32.0), inner.max);
    let panel = Rect::from_min_max(inner.min, pos2(inner.right(), help.top() - 6.0));
    Layout { header, style, blurb, heads, main, tabs, drawer, panel, help, foot }
}

/// `n` equal columns of `row`, the last taking the remainder (`removeFromLeft (w)`).
pub(crate) fn columns(row: Rect, n: usize) -> Vec<Rect> {
    let w = (row.width() / n.max(1) as f32).floor();
    (0..n)
        .map(|i| {
            let x0 = row.left() + w * i as f32;
            let x1 = if i + 1 == n { row.right() } else { x0 + w };
            Rect::from_min_max(pos2(x0, row.top()), pos2(x1, row.bottom()))
        })
        .collect()
}

/// The help line along the bottom of a tab's panel (`paint`).
fn help_text(tab: usize) -> &'static str {
    match tab {
        TAB_TONE => {
            "Filter Sweep moves High Cut with the Motion LFO.\nNoise is hiss inside the loop: it recirculates, and on BBD the compander makes it breathe."
        }
        TAB_DYNAMICS => {
            "Duck turns the echoes down while you play.  Threshold only echoes notes you dig into (lamp = getting through).\nAttack Focus echoes just the pick attack, so chords stay clear."
        }
        TAB_RHYTHM => {
            "Extra taps inside one delay time. Set Time (or Note) to the length of the whole figure: a half note or a bar works best.\nOnly the last tap feeds back, so the figure repeats cleanly."
        }
        _ => "",
    }
}

/// `paint`'s tempo readout: "HOST  120.0", "NO HOST TEMPO  ·  120.0" or "MANUAL  96.0".
pub fn tempo_readout(want_host: bool, host_bpm: f32, bpm: f32) -> String {
    let bpm = format!("{bpm:.1}");
    if want_host && host_bpm >= 20.0 {
        format!("HOST  {bpm}")
    } else if want_host {
        format!("NO HOST TEMPO  \u{b7}  {bpm}")
    } else {
        format!("MANUAL  {bpm}")
    }
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &PALETTE;
    let r = host.rect;
    let p = ui.painter().clone();
    paint_background(&p, r);
    st.tab = st.tab.min(TABS.len() - 1);
    // `ui.mix.drawer {tab}` (scripts, tests): "Presets" opens the browser, a tab's name shows it.
    if let Some(tab) = family::take_drawer_tab(app, host, &mut st.fam)
        && let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(&tab))
    {
        st.tab = i;
    }

    // The browser covers the whole editor (`browser->setBounds (getLocalBounds())`).
    if family::browser(app, ui, host, pal, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return;
    }

    let ins = host.ins;
    let a = applies(ins);
    let style = model(ins, "style");
    let l = layout(r, a.heads);
    let ro = readouts(host);
    let bpm = effective_bpm(host, ro);

    // ---- paint(): the title (drawn by the header), the blurb, the panel, the help, the tempo.
    p.text(pos2(l.blurb.left(), l.blurb.center().y), Align2::LEFT_CENTER, dc::style_blurb(style), font(BLURB_SIZE, false), pal.text_body);
    paint_panel(&p, l.drawer, crate::editor_kit::look::PANEL_CORNER);
    let help = help_text(st.tab);
    if !help.is_empty() {
        p.with_clip_rect(l.help).text(l.help.min, Align2::LEFT_TOP, help, font(CAPTION_SIZE, false), pal.text_faint);
    }

    // ---- header
    crate::editor_kit::look::paint_title(&p, l.header, "Delay", "Engine", pal);
    family::header(app, ui, host, pal, &mut st.fam, &FAM, &family::engine_header_rects(l.header));

    // ---- style, heads
    segmented_param(app, ui, host, &mut st.fam, &FAM, l.style, pal, "style", Some(&dc::STYLES), 1, SelectionStyle::Filled, false);
    if let Some(hr) = l.heads {
        segmented_param(app, ui, host, &mut st.fam, &FAM, hr, pal, "heads", None, 1, SelectionStyle::Filled, false);
    }

    // ---- main dials: Time and Note share the first slot.
    let dial = |app: &mut SoundApp, ui: &mut Ui, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, small: bool| {
        dial_ctl(app, ui, host, fam, rect, id, label, small, bpm);
    };
    let cols = columns(l.main, 4);
    if a.time {
        dial(app, ui, &mut st.fam, cols[0], "time", "Time", false);
    }
    if a.division {
        dial(app, ui, &mut st.fam, cols[0], "division", "Note", false);
    }
    dial(app, ui, &mut st.fam, cols[1], "feedback", "Feedback", false);
    dial(app, ui, &mut st.fam, cols[2], "mix", "Mix", false);
    dial(app, ui, &mut st.fam, cols[3], "character", dc::character_label(style), false);

    // ---- the drawer's tabs: pure UI state (no parameter, no undo).
    if let Some(i) = crate::editor_kit::controls::segmented(ui, l.tabs, "tabs", pal, &TABS, 1, Some(st.tab), Default::default()) {
        st.tab = i;
    }
    let panel = l.panel;
    match st.tab {
        TAB_TONE => {
            for (c, (id, label)) in
                columns(panel, 4).into_iter().zip([("lowCut", "Low Cut"), ("highCut", "High Cut"), ("filtMod", "Filter Sweep"), ("noise", "Noise")])
            {
                dial(app, ui, &mut st.fam, c, id, label, true);
            }
        }
        TAB_DYNAMICS => {
            let c = columns(panel, 3);
            dial(app, ui, &mut st.fam, c[0], "duck", "Duck", true);
            dial(app, ui, &mut st.fam, c[1], "threshold", "Threshold", true);
            // `gateLed.setBounds (thr.getCentreX() + 30, thr.getY() + 6, 10, 10)`, lit by the
            // engine's gate lamp (the timer's `getGateLamp() > 0.5`).
            let lamp = Rect::from_min_size(pos2(c[1].center().x.floor() + 30.0, c[1].top() + 6.0), vec2(10.0, 10.0));
            let lit = ro.is_some_and(|r| r[3] > 0.5);
            led(&p, lamp, pal, lit, pal.accent);
            dial(app, ui, &mut st.fam, c[2], "focus", "Attack Focus", true);
        }
        TAB_MOTION => {
            let c = columns(panel, 4);
            let rate = c[0];
            dial(app, ui, &mut st.fam, Rect::from_min_max(rate.min, pos2(rate.right(), rate.bottom() - 22.0)), "modRate", "Mod Rate", true);
            let sync_r = Rect::from_center_size(pos2(rate.center().x, rate.bottom() - 10.0), vec2(52.0, 20.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, "modSync", "Sync", PLAIN);
            dial(app, ui, &mut st.fam, c[1], "modDepth", "Mod Depth", true);
            dial(app, ui, &mut st.fam, c[2], "diffusion", "Diffusion", true);
            let pr = Rect::from_center_size(c[3].center(), vec2(c[3].width() - 8.0, 26.0));
            segmented_param(app, ui, host, &mut st.fam, &FAM, pr, pal, "pitch", None, 1, SelectionStyle::Filled, false);
        }
        TAB_RHYTHM => {
            let pr = Rect::from_min_size(panel.min, vec2(panel.width(), 28.0));
            segmented_param(app, ui, host, &mut st.fam, &FAM, pr, pal, "pattern", None, 1, SelectionStyle::Filled, false);
        }
        TAB_STEREO => {
            let rr = Rect::from_min_size(panel.min, vec2(panel.width(), 26.0));
            segmented_param(app, ui, host, &mut st.fam, &FAM, rr, pal, "routing", None, 1, SelectionStyle::Filled, false);
            let row = Rect::from_min_max(pos2(panel.left(), rr.bottom() + 8.0), panel.max);
            let mut shown: Vec<(&str, &str)> = vec![("width", "Width")];
            if a.offset {
                shown.push(("offset", "R Offset"));
            }
            if a.division_r {
                shown.push(("divisionR", "R Note"));
            }
            if a.swing {
                shown.push(("swing", "Swing"));
            }
            let n = shown.len();
            for (c, (id, label)) in columns(row, n).into_iter().zip(shown) {
                dial(app, ui, &mut st.fam, c, id, label, true);
            }
        }
        TAB_SETUP => {
            let c = columns(panel, 4);
            dial(app, ui, &mut st.fam, c[0], "trim", "Input", true);
            dial(app, ui, &mut st.fam, c[1], "level", "Level", true);
            dial(app, ui, &mut st.fam, c[2], "tempo", "Manual BPM", true);
            let toggles = c[3].shrink2(vec2(8.0, 0.0));
            let src = Rect::from_center_size(pos2(toggles.center().x, toggles.top() + 11.0), vec2(124.0, 21.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, src, pal, "tempoSrc", "Use Manual BPM", PLAIN);
            let trails = Rect::from_center_size(pos2(toggles.center().x, toggles.top() + 22.0 + 8.0 + 11.0), vec2(70.0, 21.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, trails, pal, "trails", "Trails", PLAIN);
        }
        _ => {}
    }

    // ---- foot: Sync and Tap left, the tempo readout beside them, Reverse / Freeze right.
    let pill_h = crate::editor_kit::controls::pill_height(crate::editor_kit::controls::PillStyle::Small);
    let f = l.foot;
    let at = |x: f32, w: f32| Rect::from_min_size(pos2(x, f.center().y - pill_h * 0.5), vec2(w, pill_h));
    pill_param(app, ui, host, &mut st.fam, &FAM, at(f.left(), 54.0), pal, "sync", "Sync", PLAIN);
    tap_ui(st, app, ui, host, at(f.left() + 60.0, 46.0));
    pill_param(app, ui, host, &mut st.fam, &FAM, at(f.right() - 62.0, 62.0), pal, "freeze", "Freeze", PLAIN);
    pill_param(app, ui, host, &mut st.fam, &FAM, at(f.right() - 62.0 - 6.0 - 70.0, 70.0), pal, "reverse", "Reverse", PLAIN);
    let readout = Rect::from_min_max(pos2(f.left() + 54.0 + 6.0 + 46.0 + 12.0, f.top()), pos2(f.right() - 62.0 - 6.0 - 70.0 - 12.0, f.bottom()));
    let text = tempo_readout(param(host, "tempoSrc") < 0.5, host_bpm(host), bpm);
    p.with_clip_rect(readout).text(pos2(readout.left(), readout.center().y), Align2::LEFT_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);

    // The timer (gate lamp, tempo readouts) runs while the engine plays.
    if ro.is_some() || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// A dial bound to `id` (`attachDial` / `attachSteppedDial`) with its `formatValue` readout.
#[allow(clippy::too_many_arguments)]
fn dial_ctl(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, small: bool, bpm: f32) {
    let text = value_text(host, id, param(host, id), bpm);
    let o = DialOpts { label, value_text: &text, small, ..DialOpts::default() };
    dial_attached(app, ui, host, fam, &FAM, rect, &PALETTE, id, &o);
}

/// The Tap pill in `r` (`tap()`): Tap at 1, back to 0 after 60 ms (a parameter is only seen per
/// block), one undo step.
fn tap_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, r: Rect) {
    let held = st.tap_since.is_some();
    if pill_ex(ui, r, "tap", &PALETTE, "Tap", false, PillStyle::Small, PLAIN).clicked() && !held {
        end_gesture(app);
        set_params_merged(app, host, &[("tap", 1.0)], "tap");
        st.tap_since = Some(ui.input(|i| i.time));
    }
    if let Some(t0) = st.tap_since {
        if ui.input(|i| i.time) - t0 >= TAP_HOLD_S {
            set_params_merged(app, host, &[("tap", 0.0)], "tap");
            end_gesture(app);
            st.tap_since = None;
        } else {
            ui.ctx().request_repaint();
        }
    }
}

/// Where a parameter's dial sits in an editor at `frame` with `tab` open, for tests and scripts:
/// the knob's centre. `None` when that dial is not showing.
pub fn dial_centre(ins: &Insert, frame: Rect, tab: usize, id: &str) -> Option<Pos2> {
    let a = applies(ins);
    let l = layout(frame, a.heads);
    let knob = |r: Rect| crate::editor_kit::controls::dial_knob_bounds(r, Default::default()).center();
    let main = columns(l.main, 4);
    match id {
        "time" if a.time => return Some(knob(main[0])),
        "division" if a.division => return Some(knob(main[0])),
        "feedback" => return Some(knob(main[1])),
        "mix" => return Some(knob(main[2])),
        "character" => return Some(knob(main[3])),
        _ => {}
    }
    let c4 = columns(l.panel, 4);
    let c3 = columns(l.panel, 3);
    match (tab, id) {
        (TAB_TONE, "lowCut") => Some(knob(c4[0])),
        (TAB_TONE, "highCut") => Some(knob(c4[1])),
        (TAB_TONE, "filtMod") => Some(knob(c4[2])),
        (TAB_TONE, "noise") => Some(knob(c4[3])),
        (TAB_DYNAMICS, "duck") => Some(knob(c3[0])),
        (TAB_DYNAMICS, "threshold") => Some(knob(c3[1])),
        (TAB_DYNAMICS, "focus") => Some(knob(c3[2])),
        (TAB_SETUP, "trim") => Some(knob(c4[0])),
        (TAB_SETUP, "level") => Some(knob(c4[1])),
        (TAB_SETUP, "tempo") => Some(knob(c4[2])),
        _ => None,
    }
}

/// The style row, the tab row and the foot, for tests and scripts.
pub fn rows(ins: &Insert, frame: Rect) -> (Rect, Rect, Rect) {
    let l = layout(frame, applies(ins).heads);
    (l.style, l.tabs, l.foot)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_match_required_height() {
        let mut ins = Insert::new("fs_delay");
        assert_eq!(required_height(&ins), 496.0);
        ins.params.insert("style".into(), 3.0);
        assert_eq!(required_height(&ins), 533.0, "Tape has heads");
        ins.params.insert("reverse".into(), 1.0);
        assert_eq!(required_height(&ins), 496.0, "Reverse rewrites the taps");
    }

    #[test]
    fn the_tempo_readout_reads_as_the_vst() {
        assert_eq!(tempo_readout(true, 120.0, 120.0), "HOST  120.0");
        assert_eq!(tempo_readout(true, 0.0, 96.0), "NO HOST TEMPO  \u{b7}  96.0");
        assert_eq!(tempo_readout(false, 120.0, 96.0), "MANUAL  96.0");
    }
}

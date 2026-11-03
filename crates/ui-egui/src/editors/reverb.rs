//! ReverbEngine's VST editor (`Engine 7 - Reverb/Source/PluginEditor.cpp`, `ReverbConfig.h`),
//! translated: `resized()`'s geometry, `paint()`, the family header with its own Undo and A/B, the
//! machine row, the era (Color) row (every machine but the Spring), the blurb, the four main dials,
//! the five drawer tabs, the foot (the tempo readout, Freeze) and FireStation's preset browser.
//! The palette is the family's (`fs::look::apply (0xFF6FD3E8)`), the same in both DAW themes.
//!
//! The VST's 20 Hz timer reads `getEffectiveBpm()`: the engine's readout `[3]` here.
//! Interaction inventory: `docs/superpowers/editors/reverb.md`.

mod slim;

use super::delay::{columns, tempo_readout};
use crate::SoundApp;
use crate::editor_kit::controls::DialOpts;
use crate::editor_kit::controls::{PillLook, SelectionStyle};
use crate::editor_kit::family::{self, Family, FamilyState, dial_attached, pill_param, segmented_param};
use crate::editor_kit::look::{BLURB_SIZE, CAPTION_SIZE, PANEL_CORNER, font, paint_background, paint_panel};
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, param, readouts};
use egui::{Align2, Pos2, Rect, Ui, Vec2, pos2, vec2};
use soundcraft_dsp::plugins::fs::reverb::reverb_config as rc;
use soundcraft_dsp::plugins::fs::reverb::reverb_engine_dsp as rd;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};

const K_PAD_X: f32 = 22.0;
const K_PAD_Y: f32 = 14.0;
const K_GAP: f32 = 13.0;
const K_HEADER_H: f32 = 26.0;
const K_MACHINE_ROW_H: f32 = 30.0;
const K_COLOR_ROW_H: f32 = 24.0;
const K_BLURB_H: f32 = 16.0;
const K_MAIN_KNOB: f32 = 104.0;
/// Knob, then value and name under it.
const K_MAIN_ROW_H: f32 = K_MAIN_KNOB + 32.0;
const K_TABS_H: f32 = 24.0;
const K_PANEL_H: f32 = 146.0;
const K_FOOT_H: f32 = 24.0;
const K_DEFAULT_WIDTH: f32 = 900.0;

/// `fs::look::apply (kAccent)`, the original UI's `PEDAL.accent`.
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFF6F_D3E8));

/// `getPresetParams()` (`Presets.h`).
pub const PRESET_PARAMS: [&str; 22] = [
    "machine",
    "decay",
    "mix",
    "size",
    "preDelay",
    "lowCut",
    "highCut",
    "damping",
    "bass",
    "character",
    "diffusion",
    "modRate",
    "modDepth",
    "width",
    "duck",
    "swell",
    "freeze",
    "interval",
    "sync",
    "division",
    "color",
    "attack",
];

/// The family's preset facts: `storageKey`, and what `getPresetParams` leaves out.
const FAM: Family = Family::new("ReverbEngine", &["level", "tempo", "tempoSrc", "trails"]);

/// The family's plain (untracked) `Pill::Small`.
const PLAIN: PillLook = PillLook { active_colour: None, tracked: false, dimmed: false };

/// The drawer's tabs (`Tab`).
pub const TABS: [&str; 5] = ["Tone", "Time", "Motion", "Dynamics", "Setup"];
const TAB_TONE: usize = 0;
const TAB_TIME: usize = 1;
const TAB_MOTION: usize = 2;
const TAB_DYNAMICS: usize = 3;
const TAB_SETUP: usize = 4;

/// The editor's own state: the open tab (pure UI state) and the family header's.
#[derive(Clone, Default, Debug)]
pub(crate) struct Editor {
    pub tab: usize,
    fam: FamilyState,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// Scripts: open the slim layout's machine menu on the next frame.
    machines_open: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    crate::editor_kit::run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

fn model(ins: &Insert, id: &str) -> i32 {
    crate::module_drawer::model_index(ins, id)
}

fn applies(ins: &Insert) -> rc::Applicable {
    rc::what_applies(model(ins, "machine"), model(ins, "sync") == 1)
}

/// `requiredHeight()`: 496, or 527 with the era row.
pub fn required_height(ins: &Insert) -> f32 {
    let mut h = K_PAD_Y + K_HEADER_H + K_GAP + K_MACHINE_ROW_H;
    if applies(ins).color {
        h += 7.0 + K_COLOR_ROW_H;
    }
    h + 4.0 + K_BLURB_H + K_GAP + K_MAIN_ROW_H + K_GAP + K_TABS_H + 10.0 + K_PANEL_H + K_GAP + K_FOOT_H + K_PAD_Y
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_reverb";

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

fn host_bpm(host: &Host) -> f32 {
    let t = host.session.tempo.tempo_at_tick(0) as f32;
    if t.is_finite() { t } else { 0.0 }
}

/// `getEffectiveBpm()`: the engine's (readout `[3]`) while it plays, else the session's on Host
/// (20 BPM or more), else the Manual BPM.
fn effective_bpm(host: &Host, ro: Option<[f32; 8]>) -> f32 {
    if let Some(b) = ro.map(|r| r[3]).filter(|b| b.is_finite() && *b > 0.0) {
        return b;
    }
    let hb = host_bpm(host);
    if param(host, "tempoSrc") < 0.5 && hb >= 20.0 { hb } else { rd::knob_to_bpm(param(host, "tempo")) }
}

/// `formatValue (index, v, bpm)`.
fn value_text(host: &Host, id: &str, v: f32, bpm: f32) -> String {
    match id {
        "division" => rc::division_text(v.round() as i32, bpm),
        _ => host.info.params.iter().find(|p| p.id == id).map_or_else(String::new, |p| p.format(v)),
    }
}

struct Layout {
    header: Rect,
    machine: Rect,
    color: Option<Rect>,
    blurb: Rect,
    main: Rect,
    tabs: Rect,
    drawer: Rect,
    panel: Rect,
    help: Rect,
    foot: Rect,
}

fn layout(r: Rect, color: bool) -> Layout {
    let mut b = r.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let mut take = |h: f32| {
        let t = Rect::from_min_size(b.min, vec2(b.width(), h));
        b.min.y += h;
        t
    };
    let header = take(K_HEADER_H);
    take(K_GAP);
    let machine = take(K_MACHINE_ROW_H);
    let color = color.then(|| {
        take(7.0);
        let row = take(K_COLOR_ROW_H);
        Rect::from_center_size(row.center(), vec2(row.width().min(300.0), K_COLOR_ROW_H))
    });
    take(4.0);
    let blurb = take(K_BLURB_H);
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
    Layout { header, machine, color, blurb, main, tabs, drawer, panel, help, foot }
}

fn help_text(tab: usize) -> &'static str {
    match tab {
        TAB_TONE => {
            "Damping and Low Decay work inside the loop: they change how long highs and lows ring,\nnot just how bright the output is. Low Cut and High Cut are plain EQ on the wet."
        }
        TAB_TIME => {
            "Pre-Delay keeps the note in front of the reverb.\nAttack spreads what goes into the tail, so the reverb swells in behind the note."
        }
        TAB_DYNAMICS => {
            "Duck turns the reverb down while you play.\nSwell shuts the send on every new pick attack and fades it back up: instant bowed pad."
        }
        _ => "",
    }
}

/// The Tone tab's dials that show (`layVisible`): Low Decay and Diffusion not on the Spring.
fn tone_dials(a: &rc::Applicable) -> Vec<(&'static str, &'static str)> {
    let mut v = vec![("lowCut", "Low Cut"), ("highCut", "High Cut"), ("damping", "Damping")];
    if a.low_decay {
        v.push(("bass", "Low Decay"));
    }
    if a.diffusion {
        v.push(("diffusion", "Diffusion"));
    }
    v
}

/// What both layouts do first: the scripted drawer tabs and the preset browser, which covers
/// the whole editor while it is open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) -> bool {
    st.tab = st.tab.min(TABS.len() - 1);
    if let Some(tab) = family::take_drawer_tab(app, host, &mut st.fam) {
        if let Some(i) = TABS.iter().position(|t| t.eq_ignore_ascii_case(&tab)) {
            st.tab = i;
        } else if tab == "Machine" && st.slim {
            st.machines_open = true;
        }
    }
    if family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAM, "") {
        ui.ctx().request_repaint();
        return true;
    }
    false
}

/// One of the editor's dials (`attachDial`), its readout at `bpm`.
#[allow(clippy::too_many_arguments)]
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, small: bool, bpm: f32) {
    let text = value_text(host, id, param(host, id), bpm);
    let o = DialOpts { label, value_text: &text, small, ..DialOpts::default() };
    dial_attached(app, ui, host, fam, &FAM, rect, &PALETTE, id, &o);
}

/// The foot's Freeze pill.
fn freeze_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect) {
    pill_param(app, ui, host, fam, &FAM, r, &PALETTE, "freeze", "Freeze", PLAIN);
}

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host) {
    let pal = &PALETTE;
    let r = host.rect;
    if prelude(st, app, ui, host) {
        return;
    }
    let p = ui.painter().clone();
    paint_background(&p, r);

    let ins = host.ins;
    let a = applies(ins);
    let machine = model(ins, "machine");
    let l = layout(r, a.color);
    let ro = readouts(host);
    let bpm = effective_bpm(host, ro);

    // ---- paint()
    let blurb = rc::header_blurb(machine, model(ins, "color"));
    p.text(pos2(l.blurb.left(), l.blurb.center().y), Align2::LEFT_CENTER, blurb, font(BLURB_SIZE, false), pal.text_body);
    paint_panel(&p, l.drawer, PANEL_CORNER);
    let help = help_text(st.tab);
    if !help.is_empty() {
        p.with_clip_rect(l.help).text(l.help.min, Align2::LEFT_TOP, help, font(CAPTION_SIZE, false), pal.text_faint);
    }

    crate::editor_kit::look::paint_title(&p, l.header, "Reverb", "Engine", pal);
    family::header(app, ui, host, pal, &mut st.fam, &FAM, &family::engine_header_rects(l.header));

    segmented_param(app, ui, host, &mut st.fam, &FAM, l.machine, pal, "machine", Some(&rc::MACHINES), 1, SelectionStyle::Filled, false);
    if let Some(cr) = l.color {
        segmented_param(app, ui, host, &mut st.fam, &FAM, cr, pal, "color", None, 1, SelectionStyle::Filled, false);
    }

    let dial = |app: &mut SoundApp, ui: &mut Ui, fam: &mut FamilyState, rect: Rect, id: &str, label: &str, small: bool| {
        dial_ui(app, ui, host, fam, rect, id, label, small, bpm);
    };
    let cols = columns(l.main, 4);
    dial(app, ui, &mut st.fam, cols[0], "decay", "Decay", false);
    dial(app, ui, &mut st.fam, cols[1], "size", "Size", false);
    dial(app, ui, &mut st.fam, cols[2], "mix", "Mix", false);
    dial(app, ui, &mut st.fam, cols[3], "character", rc::character_label(machine), false);

    if let Some(i) = crate::editor_kit::controls::segmented(ui, l.tabs, "tabs", pal, &TABS, 1, Some(st.tab), Default::default()) {
        st.tab = i;
    }
    let panel = l.panel;
    match st.tab {
        TAB_TONE => {
            let shown = tone_dials(&a);
            for (c, (id, label)) in columns(panel, shown.len()).into_iter().zip(shown) {
                dial(app, ui, &mut st.fam, c, id, label, true);
            }
        }
        TAB_TIME => {
            let c = columns(panel, 3);
            let first = Rect::from_min_max(c[0].min, pos2(c[0].right(), c[0].bottom() - 22.0));
            if a.pre_delay {
                dial(app, ui, &mut st.fam, first, "preDelay", "Pre-Delay", true);
            }
            if a.division {
                dial(app, ui, &mut st.fam, first, "division", "Pre-Delay Note", true);
            }
            let sync_r = Rect::from_center_size(pos2(c[0].center().x, c[0].bottom() - 10.0), vec2(52.0, 20.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, sync_r, pal, "sync", "Sync", PLAIN);
            if a.attack {
                dial(app, ui, &mut st.fam, c[1], "attack", "Attack", true);
            }
        }
        TAB_MOTION => {
            let c = columns(panel, 3);
            dial(app, ui, &mut st.fam, c[0], "modRate", "Mod Rate", true);
            dial(app, ui, &mut st.fam, c[1], "modDepth", "Mod Depth", true);
            if a.interval {
                let ir = Rect::from_center_size(c[2].center(), vec2((c[2].width() - 8.0).min(200.0), 26.0));
                segmented_param(app, ui, host, &mut st.fam, &FAM, ir, pal, "interval", None, 1, SelectionStyle::Filled, false);
            }
        }
        TAB_DYNAMICS => {
            let c = columns(panel, 3);
            dial(app, ui, &mut st.fam, c[0], "duck", "Duck", true);
            dial(app, ui, &mut st.fam, c[1], "swell", "Swell", true);
        }
        TAB_SETUP => {
            let c = columns(panel, 4);
            dial(app, ui, &mut st.fam, c[0], "width", "Width", true);
            dial(app, ui, &mut st.fam, c[1], "level", "Level", true);
            dial(app, ui, &mut st.fam, c[2], "tempo", "Manual BPM", true);
            let toggles = c[3].shrink2(vec2(8.0, 0.0));
            let src = Rect::from_center_size(pos2(toggles.center().x, toggles.top() + 11.0), vec2(124.0, 21.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, src, pal, "tempoSrc", "Use Manual BPM", PLAIN);
            let trails = Rect::from_center_size(pos2(toggles.center().x, toggles.top() + 41.0), vec2(70.0, 21.0));
            pill_param(app, ui, host, &mut st.fam, &FAM, trails, pal, "trails", "Trails", PLAIN);
        }
        _ => {}
    }

    // ---- foot: the tempo readout left, Freeze right.
    let pill_h = crate::editor_kit::controls::pill_height(crate::editor_kit::controls::PillStyle::Small);
    let f = l.foot;
    let fr = Rect::from_min_size(pos2(f.right() - 62.0, f.center().y - pill_h * 0.5), vec2(62.0, pill_h));
    freeze_ui(app, ui, host, &mut st.fam, fr);
    let readout = Rect::from_min_max(f.min, pos2(fr.left() - 12.0, f.bottom()));
    let text = tempo_readout(param(host, "tempoSrc") < 0.5, host_bpm(host), bpm);
    p.with_clip_rect(readout).text(pos2(readout.left(), readout.center().y), Align2::LEFT_CENTER, text, font(CAPTION_SIZE, false), pal.text_faint);

    if ro.is_some() || st.fam.presets.message.is_some() {
        ui.ctx().request_repaint_after(std::time::Duration::from_millis(50));
    }
}

/// A dial's knob centre in an editor at `frame` with `tab` open (tests, scripts).
pub fn dial_centre(ins: &Insert, frame: Rect, tab: usize, id: &str) -> Option<Pos2> {
    let a = applies(ins);
    let l = layout(frame, a.color);
    let knob = |r: Rect| crate::editor_kit::controls::dial_knob_bounds(r, Default::default()).center();
    let main = columns(l.main, 4);
    if let Some(i) = ["decay", "size", "mix", "character"].iter().position(|x| *x == id) {
        return Some(knob(main[i]));
    }
    match tab {
        TAB_TONE => {
            let shown = tone_dials(&a);
            let i = shown.iter().position(|(x, _)| *x == id)?;
            Some(knob(columns(l.panel, shown.len())[i]))
        }
        TAB_DYNAMICS => ["duck", "swell"].iter().position(|x| *x == id).map(|i| knob(columns(l.panel, 3)[i])),
        _ => None,
    }
}

/// The machine row and the tab row (tests, scripts).
pub fn rows(ins: &Insert, frame: Rect) -> (Rect, Rect) {
    let l = layout(frame, applies(ins).color);
    (l.machine, l.tabs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heights_match_required_height() {
        let mut ins = Insert::new("fs_reverb");
        assert_eq!(required_height(&ins), 527.0, "Hall has the era row");
        ins.params.insert("machine".into(), 3.0);
        assert_eq!(required_height(&ins), 496.0, "the Spring has none");
    }
}

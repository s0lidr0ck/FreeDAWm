//! CopyQ's wide editor. CopyQ is Rust-native, so like Manners' this is laid out for this app
//! with the family look ([`crate::editor_kit`]), not translated from a JUCE editor:
//!
//! - the header (presets, Undo, A/B, Bypass);
//! - the display ([`display`]: this track's sound, the target's and the response applied; the
//!   working range's edges and the three splits drag);
//! - Match To: the profiles the module matches to, each with its weight in the blend, and the
//!   library to pick them from ([`library`]: factory profiles and the user's own files);
//! - the source row: Capture (measure what plays and save it as a profile), a profile from a
//!   file or from the edit selection, Follow / Freeze (hold this track's own sound), Reset;
//! - the switches (Channels, Phase, Match Level, Delta, Link and its group, Send to CleanQ);
//! - the dials.
//!
//! What a module matches to is its stored state (`mix.module_copyq`, undoable); everything else
//! is a parameter. [`slim`] is the same editor in the classic drawer's width.

mod display;
pub(crate) mod library;
mod slim;

use crate::SoundApp;
use crate::editor_kit::controls::{Btn, DialOpts, PillLook, PillStyle, SelectionStyle, card_button, h_slider, pill_height};
use crate::editor_kit::family::{self, Family, FamilyState};
use crate::editor_kit::look::{CAPTION_SIZE, LABEL_SIZE, TINY_SIZE, paint_background, paint_panel, paint_title};
use crate::editor_kit::palette::al;
use crate::editor_kit::{Host, VstEditor, VstPalette, argb, end_gesture, param, run_with, set_params_merged};
use crate::theme::{bold, regular};
use egui::{Align2, CornerRadius, Rect, Sense, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::fs::cleanq::clean_q_dsp as cleanq;
use soundcraft_dsp::plugins::fs::copyq::bells::fit_bells;
use soundcraft_dsp::plugins::fs::copyq::profile::{ALL, CURVES, Curve, FLOOR_DB, LV, MAX_TARGETS, SUM, clean_name};
use soundcraft_dsp::plugins::fs::copyq::{
    self as dsp, CopyQState, GRID_N, LIVE_COMP, LIVE_GROUP, LIVE_LEVEL, LIVE_LINKED, LIVE_MATCHING, LIVE_MEASURED, LIVE_REV, LIVE_SECONDS,
    LIVE_SENDERS, LIVE_SPLIT, LIVE_STEREO, Profile, live_shown,
};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track};
use std::sync::Arc;

// ============================================================================ constants

const K_PAD_X: f32 = 24.0;
const K_PAD_Y: f32 = 15.0;
const K_GAP: f32 = 12.0;
const K_HEADER_H: f32 = 26.0;
const K_TOP_H: f32 = 330.0;
const K_SIDE_W: f32 = 290.0;
const K_ROW_H: f32 = 24.0;
const K_BIG_KNOB: f32 = 96.0;
const K_SMALL_KNOB: f32 = 52.0;
/// A dial's label and readout under its knob.
const K_DIAL_TEXT: f32 = 30.0;
const K_PANEL_PAD: f32 = 10.0;
const K_PANEL_H: f32 = K_BIG_KNOB + K_DIAL_TEXT + K_PANEL_PAD * 2.0;
const K_FOOT_H: f32 = 16.0;
const K_HEIGHT: f32 = K_PAD_Y * 2.0 + K_HEADER_H + K_GAP + K_TOP_H + K_GAP + K_ROW_H + K_GAP + K_ROW_H + K_GAP + K_PANEL_H + K_GAP + K_FOOT_H;

/// The editor's size.
pub const SIZE: Vec2 = vec2(1040.0, K_HEIGHT);

/// The accent: the module's own (`engines/copyq.rs`).
pub const PALETTE: VstPalette = VstPalette::engine_look(argb(0xFFC9_9BFF));

/// The preset catalogue's key. Capture and Link are not a preset's business.
const FAMILY: Family = Family::new("CopyQ", &["capture", "link"]);

const FOOT_NOTE: &str = "Capture a sound and save it as a profile \u{00B7} pick profiles to match to \u{00B7} drag the display's edges and its three splits to set where, and how much";

/// The dials, left to right: (parameter, label, big).
const DIALS: [(&str, &str, bool); 11] = [
    ("amount", "Amount", true),
    ("detail", "Detail", false),
    ("limit", "Max Gain", false),
    ("dynamics", "Dynamics", false),
    ("window", "Window", false),
    ("gate", "Gate", false),
    ("amtLow", "Low", false),
    ("amtLowMid", "Low Mid", false),
    ("amtHighMid", "High Mid", false),
    ("amtHigh", "High", false),
    ("output", "Output", false),
];

/// A profile heard for less than this, in seconds, is not saved or frozen.
const MIN_SECONDS: f32 = 0.5;
/// How long a message stays in the foot, seconds.
const MESSAGE_SECONDS: f64 = 6.0;
/// Audio files a profile is measured from.
const AUDIO_EXTS: [&str; 9] = ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "opus", "m4a"];

// ============================================================================ formatting

pub(crate) fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 { format!("{:.1} kHz", hz / 1000.0) } else { format!("{} Hz", hz.round() as i32) }
}

fn seconds_text(s: f32) -> String {
    if s >= 60.0 { format!("{}:{:02}", (s / 60.0) as u32, (s % 60.0) as u32) } else { format!("{s:.1} s") }
}

/// A dial's readout.
fn value_text(host: &Host, id: &str) -> String {
    let v = param(host, id);
    match id {
        "limit" => format!("{v:.1} dB"),
        "gate" => format!("{v:.0} dB"),
        "window" => format!("{v:.1} s"),
        "output" => format!("{v:+.1} dB"),
        "amount" if v < 0.0 => format!("\u{2212}{} %", (-v).round() as i32),
        _ => format!("{} %", v.round() as i32),
    }
}

// ============================================================================ the editor

#[derive(Clone, Default)]
pub(crate) struct Editor {
    init: bool,
    fam: FamilyState,
    /// The display handle being dragged ([`display::EDGE_LO`] ..).
    dragging: Option<usize>,
    /// The library as last scanned.
    lib: Arc<Vec<library::Entry>>,
    lib_scanned: bool,
    /// The name the next capture is saved under.
    name: String,
    /// A line for the foot and when it was said (UI time).
    message: Option<(String, f64)>,
    /// Drawn in the slim layout (`UiState::slim_editors`), and its height as last laid out.
    slim: bool,
    slim_h: f32,
    /// The slim layout's More section is open.
    slim_more: bool,
}

pub(crate) fn draw(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<Editor>(app, ui, track, module, meters, |st, app| st.slim = app.ui.slim_editors);
}

impl VstEditor for Editor {
    const PLUGIN: &'static str = "fs_copyq";

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

fn keep_centre(r: Rect, w: f32, h: f32) -> Rect {
    Rect::from_min_size(pos2(r.left() + ((r.width() - w) / 2.0).trunc(), r.top() + ((r.height() - h) / 2.0).trunc()), vec2(w, h))
}

// ============================================================================ state and live values

/// What the module matches to, read once per document revision.
fn state_of(app: &SoundApp, ui: &Ui, host: &Host) -> Arc<CopyQState> {
    let id = egui::Id::new(("copyq-state", host.track.0, host.ins.id.0));
    crate::sidechain_picker::cached(ui, id, app.engine.revision, || {
        Arc::new(host.ins.state_bytes().and_then(|b| CopyQState::read(&b)).unwrap_or_default())
    })
}

/// A curve out of the live values.
fn live_curve(live: &soundcraft_dsp::analysis::LiveValues, at: usize) -> Curve {
    let mut c = [FLOOR_DB; GRID_N];
    for (i, v) in c.iter_mut().enumerate() {
        let x = live.get(at + i);
        *v = if x.is_finite() { x } else { FLOOR_DB };
    }
    c
}

/// What the module has measured of its input, as a profile called `name` (`None` before it has
/// heard [`MIN_SECONDS`] of sound, or while nothing plays).
pub(crate) fn measured(host: &Host, name: &str) -> Option<Profile> {
    let live = crate::editor_kit::live(host)?;
    if live.len() < dsp::LIVE_LEN || live.get(LIVE_SECONDS).is_nan() || live.get(LIVE_SECONDS) < MIN_SECONDS {
        return None;
    }
    let mut p = Profile::silent();
    p.name = clean_name(name);
    for (i, c) in p.curves.iter_mut().enumerate().take(CURVES) {
        *c = live_curve(live, LIVE_MEASURED + i * GRID_N);
    }
    for lv in 0..LV {
        if let (Some(l), Some(s)) = (p.level.get_mut(lv), p.seconds.get_mut(lv)) {
            *l = Some(live.get(LIVE_LEVEL + lv)).filter(|v| v.is_finite()).unwrap_or(FLOOR_DB);
            *s = Some(live.get(LIVE_SECONDS + lv)).filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0);
        }
    }
    p.stereo = live.get(LIVE_STEREO) > 0.5;
    p.is_measured().then_some(p)
}

/// The names of the two curves when they are matched apart.
fn pair_names(host: &Host) -> (&'static str, &'static str) {
    if param(host, "channels").round() as i32 == 2 { ("Left", "Right") } else { ("Mid", "Side") }
}

/// What the display draws: the engine's own curves while it plays, else the stored target (and
/// frozen source) so the editor shows what it will match to before anything plays.
fn shown(host: &Host, state: &CopyQState) -> display::Shown {
    let mut out = display::Shown::default();
    let live = crate::editor_kit::live(host).filter(|l| l.len() >= dsp::LIVE_LEN && l.get(LIVE_REV) > 0.0);
    let capturing = param(host, "capture") > 0.5;
    let Some(live) = live else {
        let target = state.target();
        let source = state.source.as_ref().filter(|p| p.is_measured());
        // The target at the source's level, as the engine shows it.
        let offset = match (&target, source) {
            (Some(t), Some(s)) => {
                let (a, b) = (t.curve(ALL, SUM), s.curve(ALL, SUM));
                a.iter().zip(b).map(|(x, y)| x - y).sum::<f32>() / GRID_N as f32
            }
            _ => 0.0,
        };
        out.target = target.as_ref().map(|t| {
            let mut c = *t.curve(ALL, SUM);
            c.iter_mut().for_each(|v| *v -= offset);
            [c, c]
        });
        out.source = source.map(|s| [*s.curve(ALL, SUM), *s.curve(ALL, SUM)]);
        out.status = match (capturing, target.is_some()) {
            (true, _) => "Capture is on: play to measure".into(),
            (false, true) => "Play to match".into(),
            (false, false) => "No target yet: pick a profile".into(),
        };
        return out;
    };
    out.source = Some([live_curve(live, live_shown(0, 0)), live_curve(live, live_shown(1, 0))]);
    out.target = Some([live_curve(live, live_shown(0, 1)), live_curve(live, live_shown(1, 1))]);
    out.applied = Some([live_curve(live, live_shown(0, 2)), live_curve(live, live_shown(1, 2))]);
    if live.get(LIVE_SPLIT) > 0.5 {
        let (a, b) = pair_names(host);
        out.first = a;
        out.second = Some(b);
    }
    let seconds = live.get(LIVE_SECONDS).max(0.0);
    let linked = live.get(LIVE_LINKED) > 0.5;
    let role = param(host, "link").round() as i32;
    out.status = if capturing {
        format!("Capturing \u{00B7} {}", seconds_text(seconds))
    } else if role == 1 {
        format!("Sending to its link group \u{00B7} {}", seconds_text(seconds))
    } else if live.get(LIVE_MATCHING) > 0.5 {
        let r = crate::editor_kit::readouts(host).unwrap_or([f32::NAN; 8]);
        let reading = |i: usize| r.get(i).copied().filter(|v| v.is_finite()).unwrap_or(0.0).max(0.0);
        let from = if linked {
            let (s, g) = (live.get(LIVE_SENDERS) as i32, live.get(LIVE_GROUP) as i32);
            if s > 0 { format!("Group target ({s} sending) \u{00B7} ") } else { format!("Group average ({g}) \u{00B7} ") }
        } else {
            String::new()
        };
        format!("{from}\u{2212}{:.1} / +{:.1} dB \u{00B7} level {:+.1} dB", reading(0), reading(1), live.get(LIVE_COMP))
    } else if state.targets.is_empty() && !linked {
        if role >= 2 { "Waiting for its link group".into() } else { "No target yet: pick a profile".into() }
    } else {
        "Listening\u{2026}".into()
    };
    out
}

// ============================================================================ what the buttons do

/// Runs an engine command for this module; an error goes to the status line. The result on
/// success.
fn run(app: &mut SoundApp, host: &Host, cmd: &str, mut p: Value) -> Option<Value> {
    if let Some(o) = p.as_object_mut() {
        o.insert("track".into(), json!(host.track.0));
        o.insert("module".into(), json!(host.ins.id.0));
    }
    match app.run(cmd, p) {
        Ok(v) => Some(v),
        Err(e) => {
            app.ui.status = e;
            None
        }
    }
}

fn targets_json(list: &[(f32, Arc<Profile>)]) -> Value {
    Value::Array(list.iter().map(|(w, p)| json!({"weight": w, "profile": p.to_value()})).collect())
}

fn say(st: &mut Editor, ui: &Ui, text: impl Into<String>) {
    st.message = Some((text.into(), ui.input(|i| i.time)));
}

fn rescan(st: &mut Editor, app: &SoundApp) {
    st.lib = Arc::new(library::scan(library::dir(app.preset_dir.as_deref()).as_deref()));
    st.lib_scanned = true;
}

/// Matches to `profile`: alone, or (`add`) blended with what is there (a profile of the same
/// name is replaced, and the oldest makes room when there are four).
fn use_profile(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host, state: &CopyQState, profile: &Profile, add: bool) {
    family::push_undo(&mut st.fam, host, &FAMILY);
    let mut list: Vec<(f32, Arc<Profile>)> = if add { state.targets.iter().map(|(w, p)| (*w, Arc::new(p.clone()))).collect() } else { Vec::new() };
    list.retain(|(_, p)| p.name != profile.name);
    while list.len() >= MAX_TARGETS {
        list.remove(0);
    }
    list.push((1.0, Arc::new(profile.clone())));
    if run(app, host, "mix.module_copyq", json!({"targets": targets_json(&list)})).is_some() {
        say(
            st,
            ui,
            if list.len() > 1 {
                format!("Blending in \u{201C}{}\u{201D}", profile.name)
            } else {
                format!("Matching to \u{201C}{}\u{201D}", profile.name)
            },
        );
    }
}

fn remove_target(app: &mut SoundApp, host: &Host, state: &CopyQState, index: usize) {
    let list: Vec<(f32, Arc<Profile>)> =
        state.targets.iter().enumerate().filter(|(i, _)| *i != index).map(|(_, (w, p))| (*w, Arc::new(p.clone()))).collect();
    run(app, host, "mix.module_copyq", json!({"targets": targets_json(&list)}));
}

/// A target's weight while its slider is dragged: one undo step for the whole drag.
fn set_weight(app: &mut SoundApp, host: &Host, state: &CopyQState, index: usize, weight: f32) {
    let list: Vec<(f32, Arc<Profile>)> =
        state.targets.iter().enumerate().map(|(i, (w, p))| (if i == index { weight.clamp(0.0, 1.0) } else { *w }, Arc::new(p.clone()))).collect();
    let p = json!({"track": host.track.0, "module": host.ins.id.0, "targets": targets_json(&list)});
    if let Err(e) = app.engine.execute_merged("mix.module_copyq", &p, &host.key(&format!("weight-{index}"))) {
        app.ui.status = e.to_string();
    }
}

/// Saves what the module has measured to the library, under the name in the box.
fn save_measured(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host) {
    let name = if st.name.trim().is_empty() {
        host.session.track(host.track).map_or_else(|| "Profile".to_string(), |t| t.name.clone())
    } else {
        st.name.clone()
    };
    let Some(profile) = measured(host, &name) else {
        return say(st, ui, "Nothing measured yet: switch Capture on and play the sound");
    };
    save_to_library(st, app, ui, &profile);
}

/// A profile into the library; says what happened. The name it is listed under.
fn save_to_library(st: &mut Editor, app: &mut SoundApp, ui: &Ui, profile: &Profile) -> Option<String> {
    let Some(dir) = library::dir(app.preset_dir.as_deref()) else {
        say(st, ui, "Profiles are not saved here (no preset folder)");
        return None;
    };
    match library::save(&dir, profile) {
        Ok(name) => {
            rescan(st, app);
            say(st, ui, format!("Saved \u{201C}{name}\u{201D} ({} measured)", seconds_text(profile.seconds.first().copied().unwrap_or(0.0))));
            Some(name)
        }
        Err(e) => {
            say(st, ui, e);
            None
        }
    }
}

/// A profile measured by the engine (`mix.copyq_measure` with `params`): saved to the library
/// and matched to.
fn measure_and_use(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host, state: &CopyQState, params: Value) {
    let result = match app.run("mix.copyq_measure", params) {
        Ok(v) => v,
        Err(e) => return say(st, ui, e),
    };
    let Some(mut profile) = result.get("profile").and_then(Profile::from_value) else { return say(st, ui, "That could not be measured") };
    if let Some(name) = save_to_library(st, app, ui, &profile) {
        profile.name = name;
    }
    use_profile(st, app, ui, host, state, &profile, false);
}

/// Freeze: this track's own sound, as measured so far, becomes the module's stored source and
/// Follow goes off, so the match holds still. One undo step.
fn freeze(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host) {
    let name = host.session.track(host.track).map_or_else(|| "This track".to_string(), |t| t.name.clone());
    let Some(profile) = measured(host, &name) else {
        return say(st, ui, "Nothing measured yet: play the track with Follow on first");
    };
    family::push_undo(&mut st.fam, host, &FAMILY);
    let key = host.key("freeze");
    crate::editor_kit::one_step(app, |app| {
        let p = json!({"track": host.track.0, "module": host.ins.id.0, "source": profile.to_value()});
        let r = app.engine.execute_merged("mix.module_copyq", &p, &key).map(|_| ());
        let q = json!({"track": host.track.0, "module": host.ins.id.0, "params": {"follow": 0.0}});
        let r = r.and_then(|()| app.engine.execute_merged("mix.module_params", &q, &key).map(|_| ()));
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    });
    say(st, ui, "Frozen: the match now holds still");
}

/// Unfreeze: the stored source goes and Follow comes back on. One undo step.
fn unfreeze(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host) {
    family::push_undo(&mut st.fam, host, &FAMILY);
    let key = host.key("unfreeze");
    crate::editor_kit::one_step(app, |app| {
        let p = json!({"track": host.track.0, "module": host.ins.id.0, "source": null});
        let r = app.engine.execute_merged("mix.module_copyq", &p, &key).map(|_| ());
        let q = json!({"track": host.track.0, "module": host.ins.id.0, "params": {"follow": 1.0}});
        let r = r.and_then(|()| app.engine.execute_merged("mix.module_params", &q, &key).map(|_| ()));
        if let Err(e) = r {
            app.ui.status = e.to_string();
        }
    });
    say(st, ui, "Following this track again");
}

/// The CleanQ parameters that make `curves` (one per place): up to 24 bells, shared out evenly.
/// `places` are CleanQ's Place choices (0 Stereo, 1 Left, 2 Right, 3 Mid, 4 Side).
pub(crate) fn cleanq_params(curves: &[(&Curve, usize)], out_db: f32) -> serde_json::Map<String, Value> {
    let mut m = serde_json::Map::new();
    let per = (cleanq::NUM_BANDS / curves.len().max(1)).min(16);
    let mut slot = 0usize;
    for (curve, place) in curves {
        for b in fit_bells(curve, per) {
            if slot >= cleanq::NUM_BANDS {
                break;
            }
            let tag = format!("b{:02}", slot + 1);
            m.insert(format!("{tag}_on"), json!(2.0));
            m.insert(format!("{tag}_type"), json!(0.0));
            m.insert(format!("{tag}_place"), json!(*place as f32));
            m.insert(format!("{tag}_freq"), json!(cleanq::freq_to_knob(f64::from(b.hz)).clamp(0.0, 1.0)));
            m.insert(format!("{tag}_gain"), json!(cleanq::gain_to_knob(f64::from(b.gain_db)).clamp(0.0, 1.0)));
            m.insert(format!("{tag}_q"), json!(cleanq::q_to_knob(f64::from(b.q)).clamp(0.0, 1.0)));
            slot += 1;
        }
    }
    if out_db.is_finite() && out_db.abs() > 0.05 {
        m.insert("outGain".into(), json!(cleanq::out_db_to_knob(f64::from(out_db.clamp(-24.0, 24.0))).clamp(0.0, 1.0)));
    }
    m
}

/// Send to CleanQ: the response being applied right now becomes a CleanQ after this module, as
/// bells that can be edited, and this module is bypassed. One undo step.
fn send_to_cleanq(st: &mut Editor, app: &mut SoundApp, ui: &Ui, host: &Host) {
    let live = crate::editor_kit::live(host).filter(|l| l.len() >= dsp::LIVE_LEN && l.get(LIVE_MATCHING) > 0.5);
    let Some(live) = live else {
        return say(st, ui, "Nothing is being matched yet: play the track with a target first");
    };
    let (a, b) = (live_curve(live, live_shown(0, 2)), live_curve(live, live_shown(1, 2)));
    let split = live.get(LIVE_SPLIT) > 0.5;
    let places: Vec<(&Curve, usize)> = match (split, param(host, "channels").round() as i32) {
        (true, 2) => vec![(&a, 1), (&b, 2)],
        (true, _) => vec![(&a, 3), (&b, 4)],
        _ => vec![(&a, 0)],
    };
    let comp = Some(live.get(LIVE_COMP)).filter(|v| v.is_finite()).unwrap_or(0.0) + param(host, "output");
    let params = cleanq_params(&places, comp);
    let bands = params.keys().filter(|k| k.ends_with("_on")).count();
    if bands == 0 {
        return say(st, ui, "The match is flat: there is nothing to send");
    }
    let at = host.session.track(host.track).and_then(|t| t.mixer.inserts.iter().position(|i| i.id == host.ins.id)).map(|i| i + 1);
    let key = host.key("to-cleanq");
    let mut ok = true;
    crate::editor_kit::one_step(app, |app| {
        let add = json!({"track": host.track.0, "plugin": "fs_cleanq", "at": at, "params": params});
        let r = app.engine.execute_merged("mix.module_add", &add, &key).map(|_| ());
        let off = json!({"track": host.track.0, "module": host.ins.id.0, "value": true});
        let r = r.and_then(|()| app.engine.execute_merged("mix.module_bypass", &off, &key).map(|_| ()));
        if let Err(e) = r {
            app.ui.status = e.to_string();
            ok = false;
        }
    });
    if ok {
        say(st, ui, format!("Sent to a CleanQ after this module as {bands} bells; CopyQ is bypassed"));
    }
}

// ============================================================================ shared sections

/// What both layouts do first: the scripted drawer tab ("More" opens the slim layout's section),
/// the library's first scan, and the preset browser, which covers the whole editor while it is
/// open (then `true`: paint nothing else).
fn prelude(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) -> bool {
    if !st.init {
        st.init = true;
        st.fam.presets.category = "All".into();
    }
    if !st.lib_scanned {
        rescan(st, app);
    }
    if family::take_drawer_tab(app, host, &mut st.fam).as_deref() == Some("More") && st.slim {
        st.slim_more = true;
    }
    family::browser(app, ui, host, &PALETTE, &mut st.fam, &FAMILY, "Match")
}

/// The display in `r` and its handles' gestures (a drag is one undo step).
fn display_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>, state: &CopyQState, r: Rect, compact: bool) {
    let knobs = display::Knobs {
        range: (param(host, "rangeLo"), param(host, "rangeHi")),
        splits: [param(host, "split1"), param(host, "split2"), param(host, "split3")],
        amounts: [param(host, "amtLow"), param(host, "amtLowMid"), param(host, "amtHighMid"), param(host, "amtHigh")],
        scale_db: param(host, "limit").max(12.0),
    };
    let out = display::draw(ui, r, &PALETTE, &shown(host, state), knobs, &mut st.dragging, compact);
    if out.started {
        end_gesture(app);
        family::push_undo(&mut st.fam, host, &FAMILY);
    }
    if let Some((which, hz)) = out.moved {
        let id = match which {
            display::EDGE_LO => "rangeLo",
            display::EDGE_HI => "rangeHi",
            w => ["split1", "split2", "split3"].get(w - display::SPLIT).copied().unwrap_or("split2"),
        };
        if let Some(info) = host.info.param(id) {
            set_params_merged(app, host, &[(id, info.clamp(hz))], &format!("handle-{which}"));
        }
    }
    if out.ended {
        end_gesture(app);
    }
    ui.ctx().request_repaint_after(std::time::Duration::from_millis(33));
}

/// One dial in `r`.
fn dial_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, label: &str, small: bool) {
    let value_text = value_text(host, id);
    let o = DialOpts { label, value_text: &value_text, default: family::default01(host, id), small, ..DialOpts::default() };
    family::dial_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, &o);
}

/// A small switch for toggle `id` in `r`.
fn pill_ui(app: &mut SoundApp, ui: &mut Ui, host: &Host, fam: &mut FamilyState, r: Rect, id: &str, text: &str) -> egui::Response {
    family::pill_param(app, ui, host, fam, &FAMILY, r, &PALETTE, id, text, PillLook { tracked: false, ..PillLook::default() })
}

fn pill_w(ui: &Ui, text: &str) -> f32 {
    family::small_pill_w(ui, text).max(56.0)
}

/// The Match To list in `r`: one row per target (its name, its weight in the blend when there is
/// more than one, a button to drop it). Returns the y under it.
fn targets_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, state: &CopyQState, r: Rect) -> f32 {
    let pal = &PALETTE;
    let p = ui.painter().clone();
    p.text(r.left_top(), Align2::LEFT_TOP, "MATCH TO", bold(TINY_SIZE), pal.text_faint);
    let mut y = r.top() + 16.0;
    if state.targets.is_empty() {
        p.text(pos2(r.left(), y + 10.0), Align2::LEFT_CENTER, "Nothing yet: pick a profile below", regular(CAPTION_SIZE), pal.text_faint);
        return y + 22.0;
    }
    let many = state.targets.len() > 1;
    for (i, (w, prof)) in state.targets.iter().enumerate() {
        let row = Rect::from_min_size(pos2(r.left(), y), vec2(r.width(), 20.0));
        let drop_r = Rect::from_min_size(pos2(row.right() - 20.0, row.top()), vec2(20.0, 20.0));
        let slider_r = Rect::from_min_max(pos2(row.right() - 20.0 - 6.0 - 84.0, row.top()), pos2(row.right() - 26.0, row.bottom()));
        let name_r = Rect::from_min_max(row.min, pos2(if many { slider_r.left() - 30.0 } else { drop_r.left() - 4.0 }, row.bottom()));
        p.with_clip_rect(name_r.intersect(p.clip_rect())).text(
            pos2(name_r.left(), row.center().y),
            Align2::LEFT_CENTER,
            &prof.name,
            regular(LABEL_SIZE),
            pal.text_bright,
        );
        if many {
            p.text(pos2(slider_r.left() - 4.0, row.center().y), Align2::RIGHT_CENTER, format!("{:.0}", w * 100.0), regular(TINY_SIZE), pal.text_body);
            let out = h_slider(ui, slider_r, ("copyq-weight", i), pal, *w, 1.0, false);
            if let Some(v) = out.prop {
                set_weight(app, host, state, i, v);
            }
            if out.ended {
                end_gesture(app);
            }
        }
        let resp = card_button(ui, drop_r, ("copyq-drop", i), pal, Btn::new("\u{00D7}").danger(), Sense::click());
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Remove target {}", prof.name)));
        if resp.clicked() {
            family::push_undo(&mut st.fam, host, &FAMILY);
            remove_target(app, host, state, i);
        }
        y += 22.0;
    }
    y
}

/// The library in `r`: every profile, scrolling. A click matches to it alone; its `+` blends it
/// in; a user profile's `×` deletes its file.
fn library_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host, state: &CopyQState, r: Rect) {
    let pal = &PALETTE;
    ui.painter().text(r.left_top(), Align2::LEFT_TOP, "LIBRARY", bold(TINY_SIZE), pal.text_faint);
    let list_r = Rect::from_min_max(pos2(r.left(), r.top() + 16.0), r.max);
    ui.painter().rect_filled(list_r, CornerRadius::same(3), pal.well);
    ui.painter().rect_stroke(list_r, CornerRadius::same(3), Stroke::new(1.0, pal.border), StrokeKind::Inside);
    let inner = list_r.shrink(3.0);
    let lib = st.lib.clone();
    let mut pick: Option<(usize, bool)> = None;
    let mut delete: Option<usize> = None;
    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt("copyq-library").max_rect(inner));
    cui.set_clip_rect(inner.intersect(ui.clip_rect()));
    egui::ScrollArea::vertical().id_salt("copyq-library-scroll").auto_shrink([false, false]).show(&mut cui, |ui| {
        ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
        let row_h = 20.0;
        let mut was_factory = true;
        for (i, e) in lib.iter().enumerate() {
            if was_factory && !e.factory {
                let (sep, _) = ui.allocate_exact_size(vec2(inner.width() - 10.0, 7.0), Sense::hover());
                ui.painter()
                    .line_segment([pos2(sep.left() + 4.0, sep.center().y), pos2(sep.right() - 4.0, sep.center().y)], Stroke::new(1.0, pal.border));
            }
            was_factory = e.factory;
            let (row, resp) = ui.allocate_exact_size(vec2(inner.width() - 10.0, row_h), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Profile {}", e.name)));
            let in_use = state.targets.iter().any(|(_, p)| p.name == e.name);
            let pp = ui.painter().clone();
            if resp.hovered() {
                pp.rect_filled(row, CornerRadius::same(3), pal.panel);
            }
            let ink = if in_use {
                pal.accent
            } else if resp.hovered() {
                pal.text_bright
            } else {
                pal.text_body
            };
            let add_r = Rect::from_min_size(pos2(row.right() - if e.factory { 20.0 } else { 40.0 }, row.top() + 1.0), vec2(18.0, 18.0));
            let name_clip = Rect::from_min_max(row.min, pos2(add_r.left() - 4.0, row.bottom()));
            pp.with_clip_rect(name_clip.intersect(pp.clip_rect())).text(
                pos2(row.left() + 6.0, row.center().y),
                Align2::LEFT_CENTER,
                &e.name,
                if in_use { bold(LABEL_SIZE) } else { regular(LABEL_SIZE) },
                ink,
            );
            let add = card_button(ui, add_r, ("copyq-lib-add", i), pal, Btn::new("+").enabled(!in_use), Sense::click());
            add.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !in_use, format!("Blend in {}", e.name)));
            let add = add.on_hover_text("Blend this profile in with the ones already matched to");
            if add.clicked() {
                pick = Some((i, true));
            } else if resp.clicked() {
                pick = Some((i, false));
            }
            if !e.factory {
                let del_r = Rect::from_min_size(pos2(row.right() - 20.0, row.top() + 1.0), vec2(18.0, 18.0));
                let del = card_button(ui, del_r, ("copyq-lib-del", i), pal, Btn::new("\u{00D7}").danger(), Sense::click());
                del.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Delete profile {}", e.name)));
                if del.on_hover_text("Delete this profile's file (modules already matching to it keep their copy)").clicked() {
                    delete = Some(i);
                }
            }
        }
    });
    if let Some((i, add)) = pick
        && let Some(e) = lib.get(i)
    {
        use_profile(st, app, ui, host, state, &e.profile, add);
    }
    if let Some(e) = delete.and_then(|i| lib.get(i))
        && let Some(dir) = library::dir(app.preset_dir.as_deref())
    {
        library::delete(&dir, &e.name);
        rescan(st, app);
        say(st, ui, format!("Deleted \u{201C}{}\u{201D}", e.name));
    }
}

/// A plain button in `r`.
fn button(ui: &mut Ui, r: Rect, salt: &str, text: &str, enabled: bool, tip: &str) -> bool {
    let resp = card_button(ui, r, ("copyq-btn", salt), &PALETTE, Btn::new(text).enabled(enabled), Sense::click());
    resp.on_hover_text(tip).clicked()
}

/// The name box in `r` (what the next capture is saved as).
fn name_box(st: &mut Editor, ui: &mut Ui, r: Rect) {
    let pal = &PALETTE;
    let id = ui.id().with("copyq-name");
    let focused = ui.memory(|m| m.has_focus(id));
    let p = ui.painter();
    p.rect_filled(r, CornerRadius::same(3), pal.panel);
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, if focused { pal.accent } else { pal.border }), StrokeKind::Inside);
    let te = egui::TextEdit::singleline(&mut st.name)
        .id(id)
        .char_limit(60)
        .font(regular(11.0))
        .text_color(pal.text_bright)
        .hint_text(egui::RichText::new("Profile name").color(pal.text_faint))
        .frame(egui::Frame::NONE)
        .vertical_align(egui::Align::Center);
    ui.put(r.shrink2(vec2(6.0, 2.0)), te);
}

/// Whether the module holds a frozen source.
fn frozen(state: &CopyQState) -> bool {
    state.source.as_ref().is_some_and(Profile::is_measured)
}

// ============================================================================ the wide layout

fn editor_ui(st: &mut Editor, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>) {
    if prelude(st, app, ui, host) {
        return;
    }
    let pal = &PALETTE;
    let p = ui.painter().clone();
    paint_background(&p, host.rect);
    let state = state_of(app, ui, host);

    // ---- layout
    let mut b = host.rect.shrink2(vec2(K_PAD_X, K_PAD_Y));
    let mut take = |h: f32| {
        let r = Rect::from_min_size(b.min, vec2(b.width(), h));
        b.min.y += h + K_GAP;
        r
    };
    let header = take(K_HEADER_H);
    let top = take(K_TOP_H);
    let source_row = take(K_ROW_H);
    let switch_row = take(K_ROW_H);
    let panel = take(K_PANEL_H);
    let foot = take(K_FOOT_H);
    let display_r = Rect::from_min_max(top.min, pos2(top.right() - K_SIDE_W - K_GAP, top.bottom()));
    let side = Rect::from_min_max(pos2(top.right() - K_SIDE_W, top.top()), top.max);

    // ---- the header
    paint_title(&p, header, "CopyQ", "", pal);
    let rects = family::right_aligned_header(ui, header);
    family::header(app, ui, host, pal, &mut st.fam, &FAMILY, &rects);

    // ---- the display
    display_ui(st, app, ui, host, &state, display_r, false);

    // ---- Match To and the library
    paint_panel(&p, side, 10.0);
    let s = side.shrink(10.0);
    let y = targets_ui(st, app, ui, host, &state, s);
    library_ui(st, app, ui, host, &state, Rect::from_min_max(pos2(s.left(), y + 6.0), s.max));

    // ---- the source row: capture and save, a profile from elsewhere, follow / freeze / reset
    let btn_h = pill_height(PillStyle::Small);
    let cy = source_row.center().y;
    let mut x = source_row.left();
    let mut place = |w: f32, h: f32| {
        let r = Rect::from_min_size(pos2(x, cy - h / 2.0), vec2(w, h));
        x += w + 6.0;
        r
    };
    let capturing = param(host, "capture") > 0.5;
    pill_ui(app, ui, host, &mut st.fam, place(pill_w(ui, "Capture"), btn_h), "capture", "Capture")
        .on_hover_text("Measure everything that plays from now on. The audio passes untouched while it is on");
    let have = measured(host, "").is_some();
    name_box(st, ui, place(170.0, 22.0));
    if button(ui, place(48.0, 22.0), "save", "Save", have, "Save what was measured as a profile in the library") {
        save_measured(st, app, ui, host);
    }
    // (a little room before the profiles from elsewhere)
    place(4.0, 0.0);
    let can_pick = app.services.pick_open.is_some();
    if button(ui, place(82.0, 22.0), "file", "From File\u{2026}", can_pick, "Measure an audio file as a profile and match to it")
        && let Some(open) = &app.services.pick_open
        && let Some(path) = open("mix.copyq_measure", &AUDIO_EXTS)
    {
        measure_and_use(st, app, ui, host, &state, json!({"path": path}));
    }
    if button(
        ui,
        place(104.0, 22.0),
        "selection",
        "From Selection",
        true,
        "Measure the edit selection on the selected audio track as a profile and match to it",
    ) {
        measure_and_use(st, app, ui, host, &state, json!({}));
    }
    // From the right: Reset, Freeze / Unfreeze, Follow.
    let mut rx = source_row.right();
    let mut place_r = |w: f32, h: f32| {
        rx -= w;
        let r = Rect::from_min_size(pos2(rx, cy - h / 2.0), vec2(w, h));
        rx -= 6.0;
        r
    };
    if button(ui, place_r(52.0, 22.0), "reset", "Reset", true, "Forget what was measured of this track and start over") {
        crate::editor_kit::action(app, host, "reset_measure");
    }
    let is_frozen = frozen(&state);
    let freeze_r = place_r(70.0, 22.0);
    if is_frozen {
        if button(ui, freeze_r, "unfreeze", "Unfreeze", true, "Drop the frozen sound and follow this track as it plays again") {
            unfreeze(st, app, ui, host);
        }
    } else if button(
        ui,
        freeze_r,
        "freeze",
        "Freeze",
        have,
        "Keep this track's sound as measured so far, so the match holds still (and is the same every time the session opens)",
    ) {
        freeze(st, app, ui, host);
    }
    pill_ui(app, ui, host, &mut st.fam, place_r(pill_w(ui, "Follow"), btn_h), "follow", "Follow")
        .on_hover_text("Keep measuring this track as it plays. Off holds the match where it is");
    let note = if capturing {
        "audio passes untouched"
    } else if is_frozen {
        "this track: frozen"
    } else {
        "this track: live"
    };
    p.text(pos2(rx - 4.0, cy), Align2::RIGHT_CENTER, note, regular(CAPTION_SIZE), pal.text_faint);

    // ---- the switches
    let cy = switch_row.center().y;
    let mut x = switch_row.left();
    let mut place = |w: f32, h: f32, gap: f32| {
        let r = Rect::from_min_size(pos2(x, cy - h / 2.0), vec2(w, h));
        x += w + gap;
        r
    };
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, place(210.0, 22.0, 8.0), pal, "channels", None, 1, SelectionStyle::Filled, false);
    family::segmented_param(
        app,
        ui,
        host,
        &mut st.fam,
        &FAMILY,
        place(150.0, 22.0, 8.0),
        pal,
        "phase",
        Some(&["Linear", "Minimum"]),
        1,
        SelectionStyle::Filled,
        false,
    );
    pill_ui(app, ui, host, &mut st.fam, place(pill_w(ui, "Match Level"), btn_h, 6.0), "autoGain", "Match Level")
        .on_hover_text("Give back the level the match changes");
    pill_ui(app, ui, host, &mut st.fam, place(pill_w(ui, "Delta"), btn_h, 14.0), "delta", "Delta").on_hover_text("Play only what the match changes");
    let link_label = place(26.0, 22.0, 2.0);
    p.text(pos2(link_label.left(), cy), Align2::LEFT_CENTER, "Link", regular(LABEL_SIZE), pal.text_body);
    family::segmented_param(app, ui, host, &mut st.fam, &FAMILY, place(190.0, 22.0, 6.0), pal, "link", None, 1, SelectionStyle::Filled, false);
    let group_r = place(78.0, 22.0, 8.0);
    let group_text = host.ins.link_group.map_or_else(|| "No group".to_string(), |g| format!("Group {g}"));
    let resp = card_button(ui, group_r, "copyq-group", pal, Btn::new(&group_text).on(host.ins.link_group.is_some()), Sense::click())
        .on_hover_text("The link group this module is in: modules in one group (on any track) share what they measure");
    egui::Popup::menu(&resp).show(|ui| crate::link_picker::link_menu(app, ui, host.track, host.ins, true));
    let send_r = Rect::from_min_size(pos2(switch_row.right() - 112.0, cy - 11.0), vec2(112.0, 22.0));
    if button(
        ui,
        send_r,
        "cleanq",
        "Send to CleanQ",
        true,
        "Turn the response being applied right now into a CleanQ with editable bells after this module, and bypass CopyQ",
    ) {
        send_to_cleanq(st, app, ui, host);
    }

    // ---- the dials
    paint_panel(&p, panel, 10.0);
    let k = panel.shrink(K_PANEL_PAD);
    let big_w = 130.0;
    let cell_w = ((k.width() - big_w) / (DIALS.len() - 1) as f32).trunc();
    let mut x = k.left();
    for (id, label, big) in DIALS {
        let w = if big { big_w } else { cell_w };
        let cell = Rect::from_min_size(pos2(x, k.top()), vec2(w, k.height()));
        x += w;
        let knob = if big { K_BIG_KNOB } else { K_SMALL_KNOB };
        let r = keep_centre(cell, if big { knob } else { (cell_w - 4.0).min(100.0) }, knob + K_DIAL_TEXT);
        dial_ui(app, ui, host, &mut st.fam, r, id, label, !big);
    }
    // The four ranges' dials are the ones the display's splits divide.
    let range_cells = Rect::from_min_size(pos2(k.left() + big_w + cell_w * 5.0, k.top() - 2.0), vec2(cell_w * 4.0, 12.0));
    p.text(range_cells.center_top(), Align2::CENTER_TOP, "AMOUNT BY RANGE", bold(TINY_SIZE), pal.text_faint);

    // ---- the foot: the last message for a while, else the hint
    let now = ui.input(|i| i.time);
    let (text, ink) = match &st.message {
        Some((m, at)) if now - *at < MESSAGE_SECONDS => (m.as_str(), al(pal.accent, 0.9)),
        _ => (FOOT_NOTE, pal.text_faint),
    };
    p.with_clip_rect(foot.intersect(p.clip_rect())).text(pos2(foot.left(), foot.center().y), Align2::LEFT_CENTER, text, regular(CAPTION_SIZE), ink);
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::plugins::fs::copyq::bells::response;

    #[test]
    fn size_and_readouts() {
        assert_eq!(SIZE, vec2(1040.0, 662.0));
        assert_eq!(hz_text(20.0), "20 Hz");
        assert_eq!(hz_text(2_500.0), "2.5 kHz");
        assert_eq!(seconds_text(4.26), "4.3 s");
        assert_eq!(seconds_text(75.0), "1:15");
        assert!(FAMILY.not_in_presets.iter().all(|id| dsp::PRESET_SKIPS.contains(id)), "a preset leaves Capture and Link alone");
    }

    #[test]
    fn a_curve_becomes_cleanq_bands() {
        let mut tilt = [0.0f32; GRID_N];
        for (i, v) in tilt.iter_mut().enumerate() {
            *v = (i as f32 - 60.0) * 0.08;
        }
        let m = cleanq_params(&[(&tilt, 0)], 1.5);
        let bands: Vec<&String> = m.keys().filter(|k| k.ends_with("_on")).collect();
        assert!(!bands.is_empty() && bands.len() <= 16);
        // Every key is a CleanQ parameter and every value in its range.
        let info = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        for (k, v) in &m {
            let p = info.param(k).unwrap_or_else(|| panic!("{k}"));
            let v = v.as_f64().unwrap() as f32;
            assert!((p.min..=p.max).contains(&v), "{k} = {v}");
        }
        assert_eq!(m["b01_on"], json!(2.0));
        assert_eq!(m["b01_place"], json!(0.0));
        assert!((cleanq::knob_to_out_db(m["outGain"].as_f64().unwrap() as f32) - 1.5).abs() < 0.01);
        // The knobs read back as bells that follow the curve.
        let back: Vec<soundcraft_dsp::plugins::fs::copyq::bells::Bell> = (1..=bands.len())
            .map(|n| {
                let get = |f: &str| m[&format!("b{n:02}_{f}")].as_f64().unwrap() as f32;
                soundcraft_dsp::plugins::fs::copyq::bells::Bell {
                    hz: cleanq::knob_to_freq(get("freq")) as f32,
                    gain_db: cleanq::knob_to_gain_db(get("gain")) as f32,
                    q: cleanq::knob_to_q(get("q")) as f32,
                }
            })
            .collect();
        let r = response(&back);
        let err = (tilt.iter().zip(&r).map(|(a, b)| (a - b).powi(2)).sum::<f32>() / GRID_N as f32).sqrt();
        assert!(err < 1.0, "{err} dB RMS");
        // Two curves share the 24 bands and go to their own places; a flat one asks for nothing.
        let two = cleanq_params(&[(&tilt, 3), (&tilt, 4)], 0.0);
        assert!(two.keys().filter(|k| k.ends_with("_on")).count() <= 24);
        assert!(two.values().any(|v| *v == json!(4.0)));
        assert!(!two.contains_key("outGain"));
        assert!(cleanq_params(&[(&[0.0; GRID_N], 0)], 0.0).is_empty());
    }
}

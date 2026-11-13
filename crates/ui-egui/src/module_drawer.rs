//! The drawer: a built-in module opened sideways in the Mix window, right of its strip.
//!
//! One drawer per strip (opening another module of the same strip replaces it); several strips
//! may each have one. It shows the track and module, the preset menu, bypass and fold, the
//! module's large graph (a compressor adds its gain reduction over the last four seconds), the
//! main control as a large knob and every other parameter as a knob (toggles and choices as
//! pills), in the plugin's order. A plugin whose parameters come in several groups (a
//! FireStation engine's lanes, bands, sections) shows the groups as tabs, one group at a time.
//! A drag is one undo step. Third-party modules have no drawer: their own window opens instead.

use crate::SoundApp;
use crate::chain_view::{cleanq_drag, knobs_menu, live_values, streams_for};
use crate::engine_ui::engine_ui;
use crate::key_knobs::default_key_knobs;
use crate::module_graphs::{GraphKind, LiveValues, draw_graph, eq_band_drag, graph_kind, module_key};
use crate::theme::{Tokens, bold, module_accent_for, regular};
use crate::widgets::{KnobResponse, KnobStyle, knob};
use egui::{Align2, Color32, CornerRadius, Painter, Pos2, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::{ParamInfo, PluginInfo, Unit};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Session, Track, TrackId};
use std::collections::{HashMap, VecDeque};

/// Width of the drawer column.
pub const DRAWER_W: f32 = 280.0;
/// Gain-reduction points kept per drawer: four seconds, one every [`GR_TICK`].
pub const GR_HISTORY_LEN: usize = 240;
/// Seconds between gain-reduction points, whatever the frame rate.
pub const GR_TICK: f64 = 4.0 / GR_HISTORY_LEN as f64;
/// From the bottom of a parameter's name to the centre of its knob.
pub const KNOB_BELOW_LABEL: f32 = 26.0;

/// Inner margin of the drawer card.
const PAD: f32 = 10.0;
/// Parameter grid: columns, cell height, knob diameter.
const COLS: usize = 4;
const CELL_H: f32 = 86.0;
const KNOB: f32 = 30.0;
/// The large knob of a module's main control.
const HERO: f32 = 78.0;
const HERO_BAND: f32 = 128.0;
/// The range of the gain-reduction history, dB.
const GR_RANGE: f32 = 24.0;
/// Meter scale floor, dBFS.
const METER_FLOOR: f32 = -60.0;

/// Whether a module opens in a drawer: built-ins only.
pub fn has_drawer(plugin: &str) -> bool {
    !soundcraft_mix::is_third_party(plugin) && soundcraft_dsp::plugin_info(plugin).is_some()
}

/// Drops every drawer whose module is gone (removed by any path, undo included), whose track was
/// deleted, or whose module no longer opens in a drawer. Others are kept.
pub fn prune_drawers(drawers: &mut HashMap<TrackId, ModuleId>, s: &Session) {
    drawers.retain(|t, m| s.track(*t).and_then(|t| t.mixer.module(*m)).is_some_and(|i| has_drawer(&i.plugin)));
}

/// A compressor drawer's gain reduction, one point every [`GR_TICK`] seconds (dB, newest last).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct GrHistory {
    pub values: VecDeque<f32>,
    /// When the next point is due (UI clock, seconds).
    pub next: f64,
}

/// Adds the gain-reduction reading at time `now` (seconds; dB, either sign) for every point due
/// since the last one, keeping the last [`GR_HISTORY_LEN`]: the history spans four seconds at any
/// frame rate. A nonsense reading counts as no reduction.
pub fn feed_gr_history(history: &mut GrHistory, now: f64, gr_db: f32) {
    let v = if gr_db.is_finite() { gr_db.abs() } else { 0.0 };
    let now = if now.is_finite() { now } else { history.next };
    if history.values.is_empty() || now - history.next > 4.0 {
        // The first point, or after a long pause (the window was hidden): start again from now.
        history.values.clear();
        history.next = now;
    }
    let mut pushed = 0;
    while history.next <= now && pushed < GR_HISTORY_LEN {
        while history.values.len() >= GR_HISTORY_LEN {
            history.values.pop_front();
        }
        history.values.push_back(v);
        history.next += GR_TICK;
        pushed += 1;
    }
}

/// Closes drawers that lost their module and forgets the history of closed ones.
pub(crate) fn prune(app: &mut SoundApp) {
    let s = app.engine.session_arc();
    prune_drawers(&mut app.ui.drawers, &s);
    let drawers = &app.ui.drawers;
    app.ui.gr_history.retain(|(t, m), _| drawers.get(t) == Some(m));
    crate::cleanq_ceiling::prune(&mut app.ui.cq_learn, drawers);
}

/// The parameter shown as the large knob: a compressor's threshold, Comp's amount, Sat's, Tape's
/// and Vint's drive, Twine's ceiling, else the first default key knob. An EQ's curve (CleanQ's
/// too) is its main control, so it has none.
fn hero_param(info: &'static PluginInfo) -> Option<&'static ParamInfo> {
    if matches!(graph_kind(info.id), GraphKind::EqCurve | GraphKind::CleanQ) {
        return None;
    }
    let id = match info.id {
        "compressor" => "threshold",
        "channel_strip" => "comp_threshold",
        // A FireStation engine's own hero (`EngineUi::hero`).
        _ => match engine_ui(info.id).and_then(|u| u.hero) {
            Some(id) => id,
            None => default_key_knobs(info).first().copied()?,
        },
    };
    info.params.iter().find(|p| p.id == id && !matches!(p.unit, Unit::Toggle | Unit::Choice))
}

/// The hero for this module: [`hero_param`], or where the key knobs follow the engine's model
/// (`EngineUi::model_key_knobs`), the model's first key knob (Amp: each amp's Volume, the AC30's
/// Brilliant, the Orange's Gain).
pub fn hero_param_for(info: &'static PluginInfo, ins: &Insert) -> Option<&'static ParamInfo> {
    if engine_ui(info.id).is_some_and(|u| u.model_key_knobs.is_some()) {
        let id = crate::key_knobs::key_knobs_for(ins, info).first().copied()?;
        return info.param(id);
    }
    hero_param(info)
}

/// The amp an Amp module is on (its `amp` choice, the default when unset).
pub fn amp_of(ins: &Insert) -> usize {
    usize::try_from(model_index(ins, "amp")).unwrap_or(0).min(soundcraft_dsp::plugins::fs::amp::NUM_AMPS - 1)
}

/// A parameter no control of SoundCraft offers, whatever the model: a FireStation engine's own
/// `bypass` (the module's bypass stands for it; setting it would silence the module with no
/// way back from the strip). A built-in's only: a hosted plugin's parameters are its own.
pub fn never_offered(plugin: &str, id: &str) -> bool {
    id == "bypass" && soundcraft_dsp::plugin_info(plugin).is_some_and(|i| i.category == soundcraft_dsp::Category::FireStation)
}

/// A link role parameter ([`soundcraft_dsp::LinkSpec`]'s `role_param` or `enable_param`, LevelUp's
/// Role): it sets the host's rendering order, which changes only when the session is synced again,
/// so it is shown in the drawer but never offered for automation (`soundcraft_dsp::link`).
pub fn link_role_param(info: &PluginInfo, id: &str) -> bool {
    info.link.is_some_and(|l| l.role_param == Some(id) || l.enable_param == Some(id))
}

/// Whether the drawer shows a parameter's control for the module as it is now: not the engine's
/// own bypass, and not a switch or dial the current model hides ([`model_switch`]). A control
/// in a switched-off group ([`group_disabled`]) is still shown there, greyed.
pub fn drawer_shows(ins: &Insert, p: &ParamInfo) -> bool {
    !never_offered(&ins.plugin, p.id) && model_switch(ins, p.id) != Some(Switch::Hide)
}

/// Whether a parameter is offered as a live control outside the drawer: the generic plugin
/// window ("Open in Window") and the automation lane picker for a new lane. The drawer's own rule
/// ([`drawer_shows`]), less a group the drawer has switched off, so the three cannot drift: what
/// the drawer hides (a model's unused switches, Manual BPM while the session tempo drives sync)
/// is hidden here, and what it deliberately shows (a Tempo Source left on Manual, so it can be
/// put back) is shown here.
///
/// A group [`group_disabled`] switches off is closed here too, so it closes (and reopens) in the
/// drawer, the generic window and the automation picker at once.
pub fn param_offered(ins: &Insert, p: &ParamInfo) -> bool {
    drawer_shows(ins, p) && group_disabled(&ins.plugin, p.group).is_none()
}

/// The parameters a module's generic plugin window lists, in the plugin's order: every one for a
/// hosted plugin, the [`param_offered`] ones for a built-in.
pub fn window_params(ins: &Insert, info: &'static PluginInfo) -> Vec<&'static ParamInfo> {
    info.params.iter().filter(|p| param_offered(ins, p)).collect()
}

/// The positions a choice control shows: the model's own names for a model-dependent switch
/// (Comp's variant per style), else the parameter's.
pub fn choice_names(ins: &Insert, p: &ParamInfo) -> &'static [&'static str] {
    match model_switch(ins, p.id) {
        Some(Switch::Show { opts, .. }) if !opts.is_empty() => opts,
        _ => p.choices,
    }
}

/// A parameter group shown greyed and switched off in the drawer, with the one-line note shown
/// in its place: `None` for every group in use. Every group of every engine is in use now (CleanQ's
/// Ceiling opened with its drawn and learned ceiling); the hook stays for a group that is not.
pub fn group_disabled(plugin: &str, group: &str) -> Option<&'static str> {
    let _ = (plugin, group);
    None
}

/// The groups of a plugin's shown parameters, in the order they first appear. More than one: the
/// drawer shows them as tabs.
pub fn drawer_groups(info: &PluginInfo) -> Vec<&'static str> {
    let mut out: Vec<&'static str> = Vec::new();
    for p in info.params.iter().filter(|p| !never_offered(info.id, p.id)) {
        if !out.contains(&p.group) {
            out.push(p.group);
        }
    }
    out
}

/// The groups of this module's parameters the drawer shows as tabs: [`drawer_groups`] without a
/// group whose every parameter the current model hides (Amp shows only its amp's own tab and the
/// global one, as the C++ editor shows one amp's face).
pub fn drawer_groups_for(info: &PluginInfo, ins: &Insert) -> Vec<&'static str> {
    drawer_groups(info).into_iter().filter(|g| info.params.iter().any(|p| p.group == *g && drawer_shows(ins, p))).collect()
}

/// What a model-dependent switch is called and what its positions are, as the engine's own
/// editor shows it; or that the current model does not use it (one option or none). A knob the
/// model renames (Pitch's Dry is OC-2's "Direct") is a `Show` with its label and no positions;
/// one the model does not use is hidden the same way.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Switch {
    Show { label: &'static str, opts: &'static [&'static str] },
    Hide,
}

/// The current value of a choice parameter of `ins` as a whole index (its default when unset).
pub(crate) fn model_index(ins: &Insert, pid: &str) -> i32 {
    let p = soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.param(pid));
    p.map_or(0, |p| p.clamp(ins.params.get(pid).copied().unwrap_or(p.default)).round() as i32)
}

/// Whether the hero knob does anything in the engine's current model (`EngineUi::hero_applies`:
/// Vint's drive without an output stage, Delay's Time under Sync); the editor greys the dial out,
/// and so does the drawer.
pub fn hero_applies(ins: &Insert) -> bool {
    engine_ui(&ins.plugin).and_then(|u| u.hero_applies).is_none_or(|f| f(ins))
}

/// The tab a drawer opens on: the engine's own choice (`EngineUi::default_group`: Vint's current
/// circuit, Amp's amp), else the first group.
pub fn default_group(ins: &Insert, groups: &[&'static str]) -> &'static str {
    let first = groups.first().copied().unwrap_or("");
    engine_ui(&ins.plugin).and_then(|u| u.default_group).and_then(|f| f(ins, groups)).unwrap_or(first)
}

/// A switch as the model defines it (label and positions): shown with more than one position,
/// else hidden.
pub(crate) fn show_switch(d: Option<(&'static str, &'static [&'static str])>) -> Switch {
    match d {
        Some((label, opts)) if opts.len() > 1 => Switch::Show { label, opts },
        _ => Switch::Hide,
    }
}

/// How the drawer shows a parameter whose meaning depends on the engine's model (Comp's variant
/// and detector per lane style, Sat's variant per circuit, Tape's formula per machine, Vint's
/// output stage per circuit: `EngineUi::model_switch`); `None` for any other parameter. As the
/// editors do, a switch the model does not use is hidden.
pub fn model_switch(ins: &Insert, id: &str) -> Option<Switch> {
    engine_ui(&ins.plugin).and_then(|u| u.model_switch).and_then(|f| f(ins, id))
}

/// AmpEngine's editor shows one amp's face: another amp's controls are hidden (and dimmed on the
/// tile), and each wears its name without the amp ("Volume (Twin)" is "Volume").
pub(crate) fn amp_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::amp::{AMP_PARAMS, param_amp, short_name};
    let owner = param_amp(id)?;
    if owner != amp_of(ins) {
        return Some(Switch::Hide);
    }
    let p = AMP_PARAMS.iter().find(|p| p.id == id)?;
    let opts: &'static [&'static str] = if p.unit == Unit::Choice { p.choices } else { &[] };
    Some(Switch::Show { label: short_name(p.name), opts })
}

/// PitchEngine's `refreshAll` (`PitchConfig.h`): a style shows only the controls it uses (the 5th
/// Mode row only once the 5th is up), and names some its own way; the engine's own bypass is the
/// drawer's to hide.
pub(crate) fn pitch_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::pitch::{PITCH_PARAMS, pitch_config as pc};
    let index = PITCH_PARAMS.iter().position(|p| p.id == id)?;
    let fifth = PITCH_PARAMS.iter().find(|p| p.id == "fifth").map_or(0.0, |p| p.clamp(ins.params.get("fifth").copied().unwrap_or(p.default)));
    let style = model_index(ins, "style");
    if !pc::shown_for(style, fifth, index) {
        return Some(Switch::Hide);
    }
    let name = PITCH_PARAMS.get(index)?.name;
    pc::knob_label(style, index).filter(|l| *l != name).map(|label| Switch::Show { label, opts: &[] })
}

/// ModEngine's `applyCircuitHook` (`ModConfig.h`): Variant takes the circuit's own names, and a
/// dial the circuit does not use is hidden (the editor dims it); Sync decides between Rate and
/// Division. Every switch row and dial wears the editor's label. Tempo is hidden: in the DAW the
/// session tempo drives Sync (plan A3; the knob is only the engine's fallback without a host
/// tempo). Link and its two set points (Level at Depth 0 and 10, which the editor's calibration
/// writes) are shown; Level is locked while Link drives it.
pub(crate) fn mod_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::mod_engine::mod_config as mc;
    let c = mc::circuit(model_index(ins, "circuit"));
    let synced = model_index(ins, "sync") == 1;
    let show = |label: &'static str, opts: &'static [&'static str]| Switch::Show { label, opts };
    let used = |on: bool, label: &'static str| if on { show(label, &[]) } else { Switch::Hide };
    Some(match id {
        "circuit" => show("Circuit", &mc::CIRCUIT_SHORT),
        "variant" if c.variants.len() > 1 => show("Variant", c.variants),
        "variant" => Switch::Hide,
        "trim" => show("Input", &[]),
        "rate" => used(!synced, "Rate"),
        "division" if synced => show(mc::DIVISION_SWITCH.0, mc::DIVISION_SWITCH.1),
        "division" | "tempo" => Switch::Hide,
        "regen" => used(c.regen, "Regen"),
        "noise" => used(c.noise, "Noise"),
        "bias" => used(c.bias, "Bias"),
        "tone" => used(c.tone, "Tone"),
        "stereo" => show(mc::STEREO_SWITCH.0, mc::STEREO_SWITCH.1),
        "shape" => show(mc::SHAPE_SWITCH.0, mc::SHAPE_SWITCH.1),
        "sync" => show(mc::SYNC_SWITCH.0, mc::SYNC_SWITCH.1),
        _ => return None,
    })
}

/// Whether an engine's Tempo Source is off Host (Manual): its Manual BPM is then in force, and
/// the drawer shows both so the session tempo can be taken back. The one gate for every engine
/// with a Tempo Source (Delay, Reverb, Filter; Swale's `manual_tempo` reads the same parameter).
fn off_host(ins: &Insert) -> bool {
    model_index(ins, "tempoSrc") != 0
}

/// DelayEngine's `refreshAll` (`DelayConfig.h`): Character takes the style's name, and what
/// does not apply to the style, routing and sync (`whatApplies`) is hidden: Heads on Tape and Drum
/// only, Time or the Note (Sync swaps them), Swing on Ping-Pong, R Offset and R Note on Dual.
/// Manual BPM and Tempo Src are hidden while it is Host: in the DAW the session tempo drives Sync
/// (plan A3/A6). Off Host (a tap, a preset, a session, automation set Manual) both show, so the
/// tempo in force can be seen and the session's taken back.
pub(crate) fn delay_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::delay::delay_config as dc;
    let style = model_index(ins, "style");
    let a = dc::what_applies(
        style,
        model_index(ins, "routing"),
        model_index(ins, "sync") == 1,
        model_index(ins, "pattern"),
        model_index(ins, "reverse") == 1,
        model_index(ins, "divisionR"),
    );
    let show = |label: &'static str, opts: &'static [&'static str]| Switch::Show { label, opts };
    let used = |on: bool, label: &'static str| if on { show(label, &[]) } else { Switch::Hide };
    let choices = |pid: &str| soundcraft_dsp::plugin_info("fs_delay").and_then(|i| i.param(pid)).map_or(&[][..], |p| p.choices);
    Some(match id {
        "character" => show(dc::character_label(style), &[]),
        "heads" if a.heads => show("Heads", choices("heads")),
        "time" => used(a.time, "Time"),
        "division" if a.division => show("Note", &dc::DIVISIONS),
        "swing" => used(a.swing, "Swing"),
        "offset" => used(a.offset, "R Offset"),
        "divisionR" if a.division_r => show("R Note", &dc::DIVISIONS_R),
        "tempoSrc" if off_host(ins) => show("Tempo", choices("tempoSrc")),
        "tempo" if off_host(ins) => return None,
        "heads" | "division" | "divisionR" | "tempoSrc" | "tempo" => Switch::Hide,
        _ => return None,
    })
}

/// ReverbEngine's `refreshAll` (`ReverbConfig.h` `whatApplies`, `characterLabel`): Character takes
/// the machine's name for it (Early / Zing / Dwell / Bloom / Shimmer); Interval shows on the Shimmer
/// only; the Spring has no Low Decay, Diffusion, Color or Attack; Sync swaps the Pre-Delay knob for
/// the note. Manual BPM and Tempo Src are hidden while it is Host: in the DAW the session tempo
/// drives a synced pre-delay. Off Host both show, so the tempo in force can be seen and put back.
pub(crate) fn reverb_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::reverb::reverb_config as rc;
    let machine = model_index(ins, "machine");
    let a = rc::what_applies(machine, model_index(ins, "sync") == 1);
    let opts = |pid: &str| soundcraft_dsp::plugin_info("fs_reverb").and_then(|i| i.param(pid)).map_or(&[][..], |p| p.choices);
    let used = |on: bool, label: &'static str, opts: &'static [&'static str]| if on { Switch::Show { label, opts } } else { Switch::Hide };
    Some(match id {
        "character" => Switch::Show { label: rc::character_label(machine), opts: &[] },
        "interval" => used(a.interval, "Interval", opts("interval")),
        "bass" => used(a.low_decay, "Low Decay", &[]),
        "diffusion" => used(a.diffusion, "Diffusion", &[]),
        "color" => used(a.color, "Color", opts("color")),
        "attack" => used(a.attack, "Attack", &[]),
        "preDelay" => used(a.pre_delay, "Pre-Delay", &[]),
        "division" => used(a.division, "Pre-Delay Note", opts("division")),
        // Shown only off Host, so a session or automation set to Manual can be put back.
        "tempoSrc" => used(off_host(ins), "Tempo Src", opts("tempoSrc")),
        "tempo" if off_host(ins) => return None,
        "tempo" => Switch::Hide,
        _ => return None,
    })
}

/// Delay's hero under Sync: Time no longer applies and the editor shows the Note in its place
/// (`formatValue`: "1/8.  ·  375 ms" at the tempo in force, the engine's readout `[2]`, else its
/// Manual BPM). `None` for any other hero or while free-running.
pub fn delay_hero_note(ins: &Insert, info: &ParamInfo, live: Option<LiveValues>) -> Option<String> {
    use soundcraft_dsp::plugins::fs::delay::{delay_config as dc, delay_engine_dsp as dd};
    if ins.plugin != "fs_delay" || info.id != "time" || model_index(ins, "sync") != 1 {
        return None;
    }
    let bpm = live.map(|l| l.readouts[2]).filter(|b| b.is_finite() && *b > 0.0).unwrap_or_else(|| {
        let t = soundcraft_dsp::plugin_info("fs_delay").and_then(|i| i.param("tempo"));
        dd::knob_to_bpm(t.map_or(0.4, |p| p.clamp(ins.params.get("tempo").copied().unwrap_or(p.default))))
    });
    Some(dc::division_text(model_index(ins, "division") as f32, bpm))
}

/// A switch that is pressed, not toggled: Delay's Tap (the C++ holds it for 60 ms), LevelUp's
/// Normalize and Learn.
pub fn is_momentary(ins: &Insert, id: &str) -> bool {
    engine_ui(&ins.plugin).is_some_and(|u| u.momentary.iter().any(|(p, _)| *p == id))
}

/// Delay's Tap tooltip, also the tip of a press no engine describes.
pub(crate) const TAP_TIP: &str = "Tap the tempo: two taps set it, more average (sets Sync and a Manual tempo)";

/// What a momentary press does, for its tooltip (the engines' editors' own tooltips,
/// `EngineUi::momentary`).
pub fn momentary_tip(ins: &Insert, id: &str) -> &'static str {
    engine_ui(&ins.plugin).and_then(|u| u.momentary.iter().find(|(p, _)| *p == id)).map_or(TAP_TIP, |(_, tip)| tip)
}

/// What a parameter does, when its engine says (`EngineUi::tips`): the first line of its tooltip.
pub fn param_tip(ins: &Insert, id: &str) -> Option<String> {
    engine_ui(&ins.plugin).and_then(|u| u.tips.iter().find(|(p, _)| *p == id)).map(|(_, tip)| (*tip).to_string())
}

/// How long a momentary switch is held at 1, seconds: the C++ editor's 60 ms, more than one block
/// (a parameter is only seen at a block boundary).
pub const MOMENTARY_HOLD_S: f64 = 0.06;

/// Drives a momentary switch: 1 when pressed, back to 0 once released and held for at least
/// [`MOMENTARY_HOLD_S`], as one undo step. Whether it is held right now.
fn momentary(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, id: &str, down: bool) -> bool {
    let key = ui.id().with(("momentary", track.0, ins.id.0, id));
    let now = ui.input(|i| i.time);
    let pressed: Option<f64> = ui.ctx().data(|d| d.get_temp(key));
    let merge = format!("momentary:{}:{}:{id}", track.0, ins.id.0);
    let send = |app: &mut SoundApp, v: f32| {
        let cmd = json!({"track": track.0, "module": ins.id.0, "param": id, "value": v});
        if let Err(e) = app.engine.execute_merged("mix.module_param", &cmd, &merge) {
            app.ui.status = e.to_string();
        }
    };
    match pressed {
        None if down => {
            send(app, 1.0);
            ui.ctx().data_mut(|d| d.insert_temp(key, now));
            ui.ctx().request_repaint();
            true
        }
        Some(t0) if !down && now - t0 >= MOMENTARY_HOLD_S => {
            send(app, 0.0);
            app.engine.end_merge();
            ui.ctx().data_mut(|d| d.remove::<f64>(key));
            false
        }
        Some(_) => {
            ui.ctx().request_repaint();
            true
        }
        None => false,
    }
}

/// A drawer knob's value text: through an engine's own hook where its C++ prints a value from
/// more than the value (ChamberMade's `formatParam` reads the built room; Delay's Mod Rate is a
/// note value while Mod Sync is on, `formatValue`), else the parameter's own text.
pub fn value_text(ctx: &egui::Context, ins: &Insert, info: &ParamInfo, v: f32) -> String {
    engine_ui(&ins.plugin).and_then(|u| u.value_text).and_then(|f| f(ctx, ins, info, v)).unwrap_or_else(|| info.format(v))
}

/// The values the C++ editor writes after the DSP accepts a tap (`timerCallback`): Tempo Src to
/// Manual ("a tap means you meant it"), the Manual BPM to the tapped tempo (40..240), and Sync
/// on. `readouts[4]` is the tap sequence, `[5]` the tapped BPM; `seen` is the last sequence acted
/// on. `None`: nothing new (or no reading).
pub fn delay_tap_params(ins: &Insert, seen: Option<f32>, readouts: &[f32; 8]) -> Option<serde_json::Map<String, serde_json::Value>> {
    let (seq, bpm) = (readouts[4], readouts[5]);
    if !seq.is_finite() || !bpm.is_finite() || seen.is_none_or(|s| seq <= s) {
        return None;
    }
    let bpm = bpm.clamp(40.0, 240.0);
    let mut m = serde_json::Map::new();
    m.insert("tempoSrc".into(), json!(1.0));
    m.insert("tempo".into(), json!((bpm - 40.0) / 200.0));
    if model_index(ins, "sync") != 1 {
        m.insert("sync".into(), json!(1.0));
    }
    Some(m)
}

/// The last tap sequence seen per Delay module, `(track, module)`.
pub type TapSeen = HashMap<(u64, u32), f32>;

/// Every Delay module whose engine accepted a new tap since `seen`, with the editor's follow-up
/// ([`delay_tap_params`]) for it; `seen` is brought up to date. Reads the strip meters' readouts,
/// so it works wherever the tap came from (the drawer, a lane, a controller) and whether or not a
/// drawer is open.
pub fn delay_tap_followups(
    tracks: &[Track],
    meters: &soundcraft_playback::MeterSnapshot,
    seen: &mut TapSeen,
) -> Vec<(TrackId, ModuleId, serde_json::Map<String, serde_json::Value>)> {
    let mut out = Vec::new();
    for t in tracks {
        let Some(strip) = meters.tracks.get(&t.id) else { continue };
        for ins in t.mixer.inserts.iter().filter(|i| i.plugin == "fs_delay") {
            let Some(m) = strip.modules.iter().find(|m| m.module == ins.id) else { continue };
            let key = (t.id.0, ins.id.0);
            let prev = seen.get(&key).copied();
            if m.readouts[4].is_finite() {
                seen.insert(key, m.readouts[4]);
            }
            if let Some(params) = delay_tap_params(ins, prev, &m.readouts) {
                out.push((t.id, ins.id, params));
            } else if prev.is_none() && m.readouts[6] >= 0.5 && model_index(ins, "tempoSrc") != 1 {
                // First sight of an engine already off the session (a tap made before this ran):
                // the same follow-up at the tempo it reports, so the Tempo switch shows.
                let mut r = m.readouts;
                r[5] = r[2];
                if let Some(params) = delay_tap_params(ins, Some(f32::NEG_INFINITY), &r) {
                    out.push((t.id, ins.id, params));
                }
            }
        }
    }
    out
}

/// Runs the C++ editor's tap follow-up for every Delay module (about ten times a second, from the
/// app's frame logic): a tap is never left as a hidden tempo override; it becomes Tempo Src Manual
/// at the tapped tempo with Sync on, and the drawer's Tempo switch (Host / Manual) shows.
pub(crate) fn delay_tap_watch(app: &mut SoundApp, ctx: &egui::Context) {
    let has_delay = app.engine.session().tracks.iter().any(|t| t.mixer.inserts.iter().any(|i| i.plugin == "fs_delay"));
    if !has_delay {
        return;
    }
    let now = ctx.input(|i| i.time);
    let due_key = egui::Id::new("delay-tap-watch-due");
    let due: f64 = ctx.data(|d| d.get_temp(due_key)).unwrap_or(0.0);
    if now < due {
        return;
    }
    ctx.data_mut(|d| d.insert_temp(due_key, now + 0.1));
    let Some(meters) = app.player.as_ref().map(|p| p.meters()) else { return };
    let seen_key = egui::Id::new("delay-tap-seen");
    let mut seen: TapSeen = ctx.data(|d| d.get_temp(seen_key)).unwrap_or_default();
    let todo = delay_tap_followups(&app.engine.session().tracks, &meters, &mut seen);
    ctx.data_mut(|d| d.insert_temp(seen_key, seen));
    for (track, module, params) in todo {
        let _ = app.run("mix.module_params", json!({"track": track.0, "module": module.0, "params": params}));
    }
}

/// DistEngine's panel for the selected circuit (`PedalConfig.h`), as its editor shows it: the
/// circuit's own dial names, its switch rows (the other circuits' are hidden) and its pair of
/// Link set points (the editor's calibration, here as two knobs).
pub(crate) fn dist_switch(circuit: i32, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::dist::wrapper::{CIRCUITS, circuit as def};
    let c = def(circuit);
    if let Some(label) = dist_dial(c, id) {
        return Some(Switch::Show { label, opts: &[] });
    }
    if let Some((_, label)) = c.switches.iter().find(|(s, _)| *s == id) {
        let opts = soundcraft_dsp::plugin_info("fs_dist").and_then(|i| i.param(id)).map_or(&[][..], |p| p.choices);
        return Some(if opts.len() > 1 { Switch::Show { label, opts } } else { Switch::Hide });
    }
    if id == c.link_lo {
        return Some(Switch::Show { label: "Link Lo", opts: &[] });
    }
    if id == c.link_hi {
        return Some(Switch::Show { label: "Link Hi", opts: &[] });
    }
    let other = CIRCUITS.iter().any(|o| o.link_lo == id || o.link_hi == id || o.switches.iter().any(|(s, _)| *s == id));
    other.then_some(Switch::Hide)
}

/// What a Dist dial is called on the circuit: the RAT has Distortion, Filter and Volume.
fn dist_dial(c: &soundcraft_dsp::plugins::fs::dist::wrapper::CircuitDef, id: &str) -> Option<&'static str> {
    match id {
        "drive" => Some(c.dial_drive),
        "tone" => Some(c.dial_tone),
        "level" => Some(c.dial_level),
        _ => None,
    }
}

/// A parameter its engine derives from others right now, whose control is locked (shown, not
/// movable): Dist's and Mod's Level while Link is on, as their editors lock the dial.
pub fn param_locked(ins: &Insert, id: &str) -> bool {
    engine_ui(&ins.plugin).and_then(|u| u.locked).is_some_and(|f| f(ins, id))
}

/// ChamberMade's inspector (`PluginEditor.cpp`): an object slot's position, size and rotate are
/// edited only once something stands in it, and Rotate only for a panel (`objectIsFlat`: "when it
/// has a face"); the drawer hides them otherwise.
pub(crate) fn chamber_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::chamber::chamber_geometry::object_is_flat;
    let rest = id.strip_prefix('o')?;
    let (slot, field) = rest.split_at(rest.find(|c: char| !c.is_ascii_digit())?);
    if !matches!(field, "x" | "y" | "s" | "r") || slot.is_empty() {
        return None;
    }
    let ty = model_index(ins, &format!("o{slot}t"));
    let used = if field == "r" { object_is_flat(ty) } else { ty > 0 };
    (!used).then_some(Switch::Hide)
}

/// FilterEngine's `refreshAll` (`FilterConfig.h`): the circuit's own switch rows (named and
/// labelled its way), Cutoff and Reso by the circuit's dial names, and only the modulation
/// controls the source uses (the step row up to the step count). The BPM knob and the Tempo
/// Source are hidden while it is Host: the session tempo drives sync in the DAW. Set to Manual by
/// any path (a preset, a session, automation), both show, so it can be seen and put back (as
/// Swale's do).
pub(crate) fn filter_switch(ins: &Insert, id: &str) -> Option<Switch> {
    use soundcraft_dsp::plugins::fs::filter::{FILTER_PARAMS, filter_config as fc, filter_engine_dsp as fd};
    if matches!(id, "tempoSrc" | "bpm") {
        return (!off_host(ins)).then_some(Switch::Hide);
    }
    let index = FILTER_PARAMS.iter().position(|p| p.id == id)?;
    let info = FILTER_PARAMS.get(index)?;
    let circuit = model_index(ins, "circuit");
    let sync = FILTER_PARAMS.get(fd::P_LFO_SYNC).is_some_and(|p| p.clamp(ins.params.get(p.id).copied().unwrap_or(p.default)) >= 0.5);
    let steps = fd::knob_to_step_count(model_index(ins, "seqSteps") as f32);
    if !fc::shown_for(circuit, model_index(ins, "modSrc"), sync, steps, index) {
        return Some(Switch::Hide);
    }
    let label = fc::knob_label(circuit, index)?;
    if info.unit == Unit::Choice {
        return Some(Switch::Show { label, opts: info.choices });
    }
    (label != info.name).then_some(Switch::Show { label, opts: &[] })
}

/// The parameters of the drawer's grid with their labels: every shown one but the hero and the
/// switches the model does not use, of the selected tab's group (`None`: no tabs).
pub fn grid_params(info: &'static PluginInfo, ins: &Insert, hero: Option<&str>, group: Option<&str>) -> (Vec<&'static ParamInfo>, Vec<String>) {
    let params: Vec<&'static ParamInfo> =
        info.params.iter().filter(|p| drawer_shows(ins, p) && hero != Some(p.id) && group.is_none_or(|g| p.group == g)).collect();
    let names = if group.is_some() { tab_names(&params) } else { params.iter().map(|p| p.name.to_string()).collect() };
    let names = params.iter().zip(names).map(|(p, n)| dial_name(ins, p.id).map_or(n, str::to_string)).collect();
    (params, names)
}

/// Where a drawer keeps its selected tab (the group's name).
pub(crate) fn tab_key(track: TrackId, module: ModuleId) -> egui::Id {
    egui::Id::new(("drawer-tab", track.0, module.0))
}

/// The tab a drawer shows: the one picked (by its pill, or by touching a band on the graph), else
/// its [`default_group`].
fn selected_group(ctx: &egui::Context, key: egui::Id, ins: &Insert, groups: &[&'static str]) -> &'static str {
    let picked: Option<String> = ctx.data(|d| d.get_temp(key));
    picked.and_then(|g| groups.iter().copied().find(|x| *x == g)).unwrap_or_else(|| default_group(ins, groups))
}

/// A tab's label: a numbered group ("Band 12") by its number, an unnamed one as "Main".
fn tab_label(group: &str) -> String {
    match group.rsplit_once(' ') {
        Some((head, n)) if head == "Band" && n.parse::<u32>().is_ok() => n.to_string(),
        _ if group.is_empty() => "Main".to_string(),
        _ => group.to_string(),
    }
}

/// The parameter names within a tab, without the word every one of them starts with ("A Amount"
/// in Lane A shows as "Amount", "B3 Freq" in Band 3 as "Freq").
pub fn tab_names(params: &[&ParamInfo]) -> Vec<String> {
    let first = |n: &str| n.split_once(' ').map(|(w, _)| w.to_string());
    let common =
        params.first().and_then(|p| first(p.name)).filter(|w| params.len() >= 2 && params.iter().all(|p| first(p.name).as_deref() == Some(w)));
    params
        .iter()
        .map(|p| match &common {
            Some(w) => p.name.strip_prefix(w.as_str()).map_or(p.name, str::trim_start).to_string(),
            None => p.name.to_string(),
        })
        .collect()
}

/// The engine's own name for a control under its current model, as its editor labels it (Pitch's
/// Dry is "Direct" on the OC-2, Mod's Variant takes the circuit's names, Dist names its dials per
/// circuit: the RAT's Drive is "Distortion"); `None` when the parameter's own name stands. The one
/// source for the drawer's grid labels, the hero's caption and a tile's knob labels.
pub fn dial_name(ins: &Insert, id: &str) -> Option<&'static str> {
    match model_switch(ins, id) {
        Some(Switch::Show { label, .. }) => Some(label),
        _ => None,
    }
}

/// The hero knob's caption: its [`dial_name`], else its own name without a lane letter; in
/// capitals.
pub fn hero_label(ins: &Insert, p: &ParamInfo) -> String {
    dial_name(ins, p.id).unwrap_or_else(|| hero_name(p)).to_uppercase()
}

/// A parameter's name without a lane letter its group ends with ("A Amount" in "Lane A":
/// "Amount"), for a hero knob or a tile's knob label.
pub(crate) fn hero_name(p: &ParamInfo) -> &str {
    match (p.group.rsplit_once(' '), p.name.split_once(' ')) {
        (Some((_, tag)), Some((w, rest))) if tag == w => rest,
        _ => p.name,
    }
}

/// A mark on a tab: CleanQ's bands in use, in their dot's colour.
fn tab_mark(ins: &Insert, group: &str) -> Option<Color32> {
    if ins.plugin != "fs_cleanq" {
        return None;
    }
    let slot = group.strip_prefix("Band ")?.parse::<usize>().ok()?.checked_sub(1)?;
    let on = ins.params.get(crate::module_graphs::cq_param_id(slot, soundcraft_dsp::plugins::fs::cleanq::F_ON)).copied().unwrap_or(0.0);
    (on >= 0.5).then(|| crate::module_graphs::cq_band_color(slot))
}

/// The tab pills, wrapping over as many rows as they need from `y`; returns the y under them.
#[allow(clippy::too_many_arguments)]
fn tabs(ui: &mut Ui, track: TrackId, ins: &Insert, groups: &[&'static str], selected: &str, inner: Rect, y: f32, accent: Color32) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let (h, gap) = (18.0, 4.0);
    let (mut x, mut y) = (inner.min.x, y);
    for g in groups {
        let label = tab_label(g);
        let galley = p.layout_no_wrap(label.clone(), bold(8.5), t.text);
        let w = (galley.size().x + 14.0).max(22.0);
        if x > inner.min.x && x + w > inner.max.x {
            x = inner.min.x;
            y += h + gap;
        }
        let r = Rect::from_min_size(pos2(x, y), vec2(w, h));
        let resp = ui.interact(r, ui.id().with(("tab", *g)), Sense::click());
        let on = *g == selected;
        // A switched-off group's tab stays selectable (it shows why) but reads greyed.
        let off = group_disabled(&ins.plugin, g);
        resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, on, if g.is_empty() { "Main" } else { g }));
        let round = CornerRadius::same(9);
        let lit = if off.is_some() { accent.gamma_multiply(0.35) } else { accent };
        p.rect_filled(r, round, if on { lit } else { t.graph_bg });
        let edge = if on || resp.hovered() { lit } else { t.module_tile_border };
        p.rect_stroke(r, round, Stroke::new(1.0, edge), StrokeKind::Inside);
        let text = match (off, on) {
            (Some(_), _) => t.text_dim,
            (None, true) => t.mode_on_text,
            (None, false) => t.text,
        };
        p.text(r.center(), Align2::CENTER_CENTER, &label, bold(8.5), text);
        if let Some(c) = tab_mark(ins, g) {
            p.circle_filled(pos2(r.max.x - 4.0, r.min.y + 4.0), 2.2, if on { t.mode_on_text } else { c });
        }
        let hover = match off {
            Some(note) => format!("{g}: {note}"),
            None => (if g.is_empty() { "Main" } else { g }).to_string(),
        };
        if resp.on_hover_text(hover).clicked() {
            ui.data_mut(|d| d.insert_temp(tab_key(track, ins.id), g.to_string()));
        }
        x += w + gap;
    }
    y + h
}

fn rect_key(track: TrackId, module: ModuleId) -> egui::Id {
    egui::Id::new(("narrow-drawer", track.0, module.0))
}

/// The narrow drawer's column as last drawn (`None` if it was not drawn last frame, e.g. the
/// wide editor is showing). For scripts (the browser's thumbnails) and tests.
pub fn drawer_rect(ctx: &egui::Context, track: TrackId, module: ModuleId) -> Option<Rect> {
    let (rect, pass) = ctx.data(|d| d.get_temp::<(Rect, u64)>(rect_key(track, module)))?;
    (pass + 1 >= ctx.cumulative_pass_nr()).then_some(rect)
}

/// The drawer of `module` on `track`, a column [`DRAWER_W`] wide and as tall as the strips.
pub(crate) fn drawer(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    // A frozen module (Track › Freeze) is read-only and greyed; its header menu still works.
    let frozen = track.mixer.module_index(module).is_some_and(|i| crate::chain_view::module_frozen(track, i));
    let wide_editor = track.module(module).and_then(|i| crate::engine_ui::engine_ui(&i.plugin)).and_then(|u| u.wide_editor).is_some();
    if app.ui.wide_editors && wide_editor {
        ui.scope(|ui| {
            if frozen {
                ui.disable();
            }
            crate::editors::drawer(app, ui, track, module, meters);
        });
        return;
    }
    let t = Tokens::current();
    let h = ui.available_height().max(560.0);
    let (r, _) = ui.allocate_exact_size(vec2(DRAWER_W, h), Sense::hover());
    let pass = ui.ctx().cumulative_pass_nr();
    ui.data_mut(|d| d.insert_temp(rect_key(track.id, module), (r, pass)));
    let Some(ins) = track.module(module) else { return };
    let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin) else { return };
    let live = meters.and_then(|m| m.modules.iter().find(|x| x.module == module)).map(live_values);
    let kind = graph_kind(&ins.plugin);
    let accent = module_accent_for(ins);
    let streams = streams_for(ui, track.id, ins, meters);
    if matches!(kind, GraphKind::Compressor | GraphKind::Comp)
        && let Some(l) = live
    {
        let now = ui.input(|i| i.time);
        feed_gr_history(app.ui.gr_history.entry((track.id, module)).or_default(), now, l.gr_db);
        ui.ctx().request_repaint();
    }

    let mut ui = ui.new_child(egui::UiBuilder::new().id_salt(("drawer", track.id.0, module.0)).max_rect(r));
    ui.set_clip_rect(r.intersect(ui.clip_rect()));
    let ui = &mut ui;
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);

    // The column: the strip background, then a card lit in the accent, as the open tile is.
    crate::widgets::vgradient(ui.painter(), r, crate::widgets::shade(t.strip_bg, 0.06), t.strip_bg);
    let card = r.shrink2(vec2(4.0, 4.0));
    let p = ui.painter().clone();
    p.rect_filled(card, CornerRadius::same(8), t.module_tile_bg);
    p.rect_stroke(card, CornerRadius::same(8), Stroke::new(1.5, accent.gamma_multiply(0.85)), StrokeKind::Inside);
    // A soft accent wash along the top edge.
    let wash = Rect::from_min_size(card.min + vec2(1.5, 1.5), vec2(card.width() - 3.0, 46.0));
    crate::widgets::vgradient(&p, wash, accent.gamma_multiply(0.12), accent.gamma_multiply(0.0));

    let inner = card.shrink(PAD);
    let mut y = inner.min.y;
    y = header(app, ui, track, ins, info, inner, y, frozen);
    y += 10.0;

    let mut body = ui.new_child(egui::UiBuilder::new().id_salt("body").max_rect(Rect::from_x_y_ranges(inner.x_range(), y..=inner.max.y)));
    if ins.bypass || !ins.active {
        body.multiply_opacity(0.5);
    }
    let sr = app.engine.session().sample_rate.as_f64() as f32;
    let w = inner.width();
    let st = streams.as_deref();
    let mkey = module_key(track.id.0, module.0);
    let groups = drawer_groups_for(info, ins);
    let tabbed = groups.len() > 1;
    if let Some((_, _, tab)) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == track.id && *m == module) {
        ui.data_mut(|d| d.insert_temp(tab_key(track.id, module), tab));
    }
    let group = selected_group(ui.ctx(), tab_key(track.id, module), ins, &groups);
    // CleanQ's Ceiling tab edits the ceiling on its graph.
    let ceiling_tab = kind == GraphKind::CleanQ && group == "Ceiling";
    match kind {
        GraphKind::Compressor | GraphKind::Comp => {
            let side = 124.0;
            let gw = if kind == GraphKind::Comp { 150.0 } else { side };
            let gr = Rect::from_min_size(pos2(inner.min.x, y), vec2(gw, side));
            draw_graph(&mut body, gr, kind, ins, mkey, live.as_ref(), st, sr, true);
            if kind == GraphKind::Compressor {
                caption(&p, gr, "in → out");
            }
            let hist = Rect::from_min_max(pos2(gr.max.x + 6.0, y), pos2(inner.max.x, y + side));
            gr_history(&p, hist, app.ui.gr_history.get(&(track.id, module)), live.map(|l| l.gr_db));
            y = gr.max.y;
        }
        GraphKind::EqCurve | GraphKind::CleanQ | GraphKind::Vint | GraphKind::Amp => {
            let gh = match kind {
                GraphKind::CleanQ => 176.0,
                GraphKind::Amp => 132.0,
                _ => 150.0,
            };
            let gr = Rect::from_min_size(pos2(inner.min.x, y), vec2(w, gh));
            draw_graph(&mut body, gr, kind, ins, mkey, live.as_ref(), st, sr, true);
            match kind {
                GraphKind::CleanQ => {
                    // A Learn keeps capturing whatever tab is showing, as the C++ editor's does.
                    let analysis = meters.and_then(|m| m.analysis.iter().find(|(m, _)| *m == module)).map(|(_, a)| a);
                    crate::cleanq_ceiling::feed(app, &body, track.id, ins, analysis, sr);
                    cleanq_drag(app, &body, track.id, ins, gr, sr);
                    // After the band dots, so a ceiling node on top of one takes the drag.
                    crate::cleanq_ceiling::graph(app, &body, track.id, ins, gr, ceiling_tab);
                    // Scout's guides for this track, when an answer's are shown (visual only).
                    crate::scout::guides::overlay(app, &body, track.id, ins, gr);
                }
                GraphKind::EqCurve => eq_drag(app, &body, track.id, ins, gr, sr),
                _ => {}
            }
            y = gr.max.y;
        }
        GraphKind::Level
        | GraphKind::Console
        | GraphKind::Twine
        | GraphKind::Pitch
        | GraphKind::Mod
        | GraphKind::Dist
        | GraphKind::Transient
        | GraphKind::Filter
        | GraphKind::Delay
        | GraphKind::Reverb
        | GraphKind::Swale
        | GraphKind::Shale
        | GraphKind::LevelUp
        | GraphKind::Manners
        | GraphKind::Chamber => {
            let gh = match kind {
                GraphKind::Chamber | GraphKind::Shale | GraphKind::LevelUp => 176.0,
                GraphKind::Twine => 140.0,
                GraphKind::Transient => 156.0,
                GraphKind::Pitch | GraphKind::Filter | GraphKind::Swale => 128.0,
                GraphKind::Mod | GraphKind::Delay | GraphKind::Reverb | GraphKind::Manners => 120.0,
                GraphKind::Console | GraphKind::Dist => 112.0,
                _ => 96.0,
            };
            let gr = Rect::from_min_size(pos2(inner.min.x, y), vec2(w, gh));
            draw_graph(&mut body, gr, kind, ins, mkey, live.as_ref(), st, sr, true);
            if kind == GraphKind::Shale {
                // The span button and the draggable threshold line, as the C++ display's.
                crate::shale_ui::drawer_controls(app, &body, track.id, ins, gr, mkey);
            }
            y = gr.max.y;
        }
    }
    y += 8.0;
    if ins.plugin == "fs_amp" {
        y = amp_ir_row(app, &mut body, track.id, ins, inner, y) + 8.0;
    }

    let hero = hero_param_for(info, ins);
    if let Some(hp) = hero {
        let band = Rect::from_min_size(pos2(inner.min.x, y), vec2(w, HERO_BAND));
        // A hero the current model ignores is greyed out and disabled, as the engine's editor does
        // (`setEnabled (false)`).
        let mut hero_ui = body.new_child(egui::UiBuilder::new().id_salt("hero").max_rect(band));
        if !hero_applies(ins) {
            hero_ui.disable();
            hero_ui.multiply_opacity(0.7);
        }
        hero_band(app, &mut hero_ui, track.id, ins, hp, band, live, kind, accent);
        y = band.max.y + 4.0;
    }

    // Every other parameter, in the plugin's order, four to a row; the rest scrolls. Several
    // groups: one at a time, picked by its tab.
    divider(&p, inner, y);
    y += 6.0;
    if tabbed {
        y = tabs(&mut body, track.id, ins, &groups, group, inner, y, accent) + 8.0;
    }
    let (params, names) = grid_params(info, ins, hero.map(|h| h.id), tabbed.then_some(group));
    let off = if tabbed { group_disabled(&ins.plugin, group) } else { None };
    if let Some(note) = off {
        p.text(pos2(inner.min.x, y), Align2::LEFT_TOP, note, regular(9.5), t.text_dim);
        y += 18.0;
    }
    if ceiling_tab {
        let row = Rect::from_x_y_ranges(inner.x_range(), y..=y + 20.0);
        y = crate::cleanq_ceiling::controls(app, &mut body, track.id, ins, row, accent) + 8.0;
    }
    if ins.plugin == "fs_levelup" && tabbed && group == soundcraft_dsp::plugins::fs::levelup::DELIVER {
        // The Deliver tab's Reset (`resetDelivery`), a one-off action on the playing instance.
        let row = Rect::from_x_y_ranges(inner.x_range(), y..=y + 20.0);
        y = crate::levelup_ui::deliver_controls(app, &mut body, track.id, ins, row, accent) + 8.0;
    }
    if ins.plugin == "fs_shale" && tabbed && group == soundcraft_dsp::plugins::fs::shale::LINK_TAB {
        // The link strip's status and the phase row (Line Up), from the module's analysis report.
        let report = meters.and_then(|m| m.analysis.iter().find(|(m, _)| *m == module)).map(|(_, a)| a.report());
        let rows = Rect::from_x_y_ranges(inner.x_range(), y..=y + 60.0);
        y = crate::shale_ui::link_controls(app, &mut body, track.id, ins, rows, accent, report, live.as_ref()) + 8.0;
    }
    let grid = Rect::from_x_y_ranges(inner.x_range().expand(PAD - 2.0), y..=inner.max.y);
    let mut gui = body.new_child(egui::UiBuilder::new().id_salt("grid").max_rect(grid));
    if off.is_some() {
        gui.disable();
        gui.multiply_opacity(0.45);
    }
    let cells = grid_cells(&params, COLS);
    let rows = cells.iter().map(|c| c.0 + 1).max().unwrap_or(0);
    egui::ScrollArea::vertical().id_salt("drawer-grid").auto_shrink([false, false]).max_height(grid.height()).show(&mut gui, |ui| {
        let (area, _) = ui.allocate_exact_size(vec2(grid.width(), rows as f32 * CELL_H), Sense::hover());
        let cell_w = (area.width() - 2.0 * (PAD - 2.0)) / COLS as f32;
        let cell_rect = |(row, col): (usize, usize)| {
            Rect::from_min_size(pos2(area.min.x + PAD - 2.0 + cell_w * col as f32, area.min.y + CELL_H * row as f32), vec2(cell_w, CELL_H))
        };
        // Parameters of one group (a band, a filter, a section) share a faint well; a tab is one
        // group already.
        let mut i = if tabbed { params.len() } else { 0 };
        while i < params.len() {
            let (key, row) = (group_key(params[i].id), cells[i].0);
            let mut j = i + 1;
            while j < params.len() && cells[j].0 == row && group_key(params[j].id) == key {
                j += 1;
            }
            if j - i >= 2 {
                let well = cell_rect(cells[i]).union(cell_rect(cells[j - 1])).shrink2(vec2(2.0, 3.0));
                ui.painter().rect_filled(well, CornerRadius::same(6), t.graph_bg.gamma_multiply(0.55));
                if let Some(c) = crate::module_graphs::band_color(&ins.plugin, params[i].id) {
                    // An EQ band's well wears its dot's colour.
                    let bar = Rect::from_min_size(pos2(well.center().x - 14.0, well.min.y), vec2(28.0, 2.5));
                    ui.painter().rect_filled(bar, CornerRadius::same(2), c);
                }
            }
            i = j;
        }
        for ((pinfo, cell), name) in params.iter().zip(&cells).zip(&names) {
            if param_locked(ins, pinfo.id) {
                // Derived by the engine (Dist's Level under Link): shown, greyed, not movable.
                ui.scope(|ui| {
                    ui.disable();
                    param_cell(app, ui, track.id, ins, pinfo, name, cell_rect(*cell), accent);
                });
            } else {
                param_cell(app, ui, track.id, ins, pinfo, name, cell_rect(*cell), accent);
            }
        }
    });
}

/// A cabinet impulse response's name as the drawer shows it: a bundled Fender IR by its mic and
/// position ("Twin 2x12 · R121 1 - Cap"), a Marshall one by its name, a file by its stem.
pub fn ir_label(reference: &str) -> String {
    const FENDER: &str = "1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - ";
    match reference.strip_prefix("bundled:") {
        Some(name) => {
            let (folder, file) = name.split_once('/').unwrap_or(("", name));
            let stem = file.strip_suffix(".wav").unwrap_or(file);
            match stem.strip_prefix(FENDER) {
                Some(rest) => format!("Twin 2x12 · {rest}"),
                None if folder.is_empty() => stem.to_string(),
                None => format!("{folder} · {stem}"),
            }
        }
        None => std::path::Path::new(reference).file_stem().and_then(|s| s.to_str()).unwrap_or(reference).to_string(),
    }
}

/// The bundled IRs as the picker groups them: (folder, mic, position, name). A Marshall IR has no
/// position (its mic is the whole name).
fn bundled_ir_menu() -> Vec<(&'static str, String, Option<String>, &'static str)> {
    soundcraft_engine::amp_irs::bundled_ir_names()
        .map(|name| {
            let folder = name.split_once('/').map_or("", |(f, _)| f);
            let label = ir_label(&format!("bundled:{name}"));
            let rest = label.split_once(" · ").map_or(label.as_str(), |(_, r)| r);
            match rest.split_once(" - ") {
                Some((mic, pos)) => (folder, mic.to_string(), Some(pos.to_string()), name),
                None => (folder, rest.to_string(), None, name),
            }
        })
        .collect()
}

/// Amp's cabinet IR for the amp in use: the bundled ones by mic and position, a file, or none
/// (`mix.module_ir`; loading one puts the amp's Cabinet on IR, as the engine's editor does).
fn amp_ir_row(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, inner: Rect, y: f32) -> f32 {
    use soundcraft_dsp::plugins::fs::amp::IR_KEYS;
    let t = Tokens::current();
    let amp = amp_of(ins);
    let reference = ins.assets.get(IR_KEYS[amp]).cloned();
    let s = app.engine.session();
    let ready = reference.as_ref().is_some_and(|r| s.assets.contains(r, s.sample_rate.hz()));
    let text = match &reference {
        None => "Built-in cabinet only · load an IR…".to_string(),
        Some(r) if ready => ir_label(r),
        Some(r) => format!("{} (missing)", ir_label(r)),
    };
    let p = ui.painter().clone();
    p.text(pos2(inner.min.x, y + 11.0), Align2::LEFT_CENTER, "IR", bold(9.5), t.text_dim);
    let field = Rect::from_min_max(pos2(inner.min.x + 22.0, y), pos2(inner.max.x, y + 22.0));
    // A bundled IR's folder names the cabinet it was measured from: its credit line goes in the hover.
    let credit = reference
        .as_deref()
        .and_then(|r| r.strip_prefix("bundled:"))
        .and_then(|n| n.split_once('/'))
        .map_or("", |(folder, _)| crate::hardware_credit::ir_folder_credit(folder));
    let hover = crate::hardware_credit::with_credit("The cabinet impulse response for this amp.", credit);
    let resp = menu_field(ui, field, &text, "amp-ir", true).on_hover_text(hover);
    let module = ins.id.0;
    let mut pick: Option<serde_json::Value> = None;
    egui::Popup::menu(&resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
        let menu = bundled_ir_menu();
        let mut folders: Vec<&str> = menu.iter().map(|m| m.0).collect();
        folders.dedup();
        for folder in folders {
            let credit = crate::hardware_credit::ir_folder_credit(folder);
            let folder_menu = ui.menu_button(folder, |ui| {
                let mut mics: Vec<&String> = menu.iter().filter(|m| m.0 == folder).map(|m| &m.1).collect();
                mics.dedup();
                for mic in mics {
                    let items: Vec<_> = menu.iter().filter(|m| m.0 == folder && &m.1 == mic).collect();
                    match items.as_slice() {
                        [one] if one.2.is_none() => {
                            if ui.button(mic.as_str()).clicked() {
                                pick = Some(json!(one.3));
                                ui.close();
                            }
                        }
                        _ => {
                            ui.menu_button(mic.as_str(), |ui| {
                                for m in &items {
                                    if ui.button(m.2.as_deref().unwrap_or(m.3)).clicked() {
                                        pick = Some(json!(m.3));
                                        ui.close();
                                    }
                                }
                            });
                        }
                    }
                }
            });
            if !credit.is_empty() {
                folder_menu.response.on_hover_text(credit);
            }
        }
        ui.separator();
        let can_pick = app.services.pick_open.is_some();
        if ui.add_enabled(can_pick, egui::Button::new("Load file…")).clicked() {
            if let Some(open) = &app.services.pick_open
                && let Some(path) = open("mix.module_ir", &["wav", "wave", "aif", "aiff", "flac"])
            {
                pick = Some(json!({"path": path}));
            }
            ui.close();
        }
        if reference.is_some() && ui.button("Clear IR").clicked() {
            pick = Some(serde_json::Value::Null);
            ui.close();
        }
    });
    if let Some(ir) = pick {
        let _ = app.run("mix.module_ir", json!({"track": track.0, "module": module, "amp": amp, "ir": ir}));
    }
    y + 22.0
}

/// The group a parameter belongs to: its id up to the last `_` ("low_shelf_q" → "low_shelf").
fn group_key(id: &str) -> &str {
    id.rsplit_once('_').map_or(id, |(head, _)| head)
}

/// Where each parameter sits in a grid `cols` wide, as (row, column), in the plugin's order: a
/// group that fits in what is left of a row joins it, else it starts a new row; groups wider than
/// a row wrap.
pub fn grid_cells(params: &[&ParamInfo], cols: usize) -> Vec<(usize, usize)> {
    let cols = cols.max(1);
    let mut out = Vec::with_capacity(params.len());
    let (mut row, mut col) = (0usize, 0usize);
    let mut i = 0;
    while i < params.len() {
        let key = group_key(params[i].id);
        let len = params[i..].iter().take_while(|p| group_key(p.id) == key).count().max(1);
        if col > 0 && col + len.min(cols) > cols {
            row += 1;
            col = 0;
        }
        for _ in 0..len {
            if col == cols {
                row += 1;
                col = 0;
            }
            out.push((row, col));
            col += 1;
        }
        i += len;
    }
    out
}

/// The header rows: names, bypass and fold; then the preset and key-knob menus; then, for a
/// plugin that takes a sidechain, its source picker, for one that links, its link group, and for
/// one with reference slots, a row per slot. Returns the y under them.
/// The drawer's header. `frozen`: the module is frozen (Track › Freeze), so everything after the
/// fold button and the title's menu is read-only (the rest of the drawer too).
#[allow(clippy::too_many_arguments)]
fn header(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert, info: &'static PluginInfo, inner: Rect, y: f32, frozen: bool) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let id = track.id;
    let off = ins.bypass || !ins.active;

    // Fold (✕) and the power button at the right.
    let fold = Rect::from_min_size(pos2(inner.max.x - 20.0, y), vec2(20.0, 20.0));
    let fresp = ui.interact(fold, ui.id().with("fold"), Sense::click());
    fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Fold the drawer"));
    if fresp.hovered() {
        p.rect_filled(fold, CornerRadius::same(5), t.text.gamma_multiply(0.08));
    }
    let c = fold.center();
    let xc = if fresp.hovered() { t.text } else { t.text_dim };
    p.line_segment([c - vec2(4.0, 4.0), c + vec2(4.0, 4.0)], Stroke::new(1.5, xc));
    p.line_segment([c + vec2(-4.0, 4.0), c + vec2(4.0, -4.0)], Stroke::new(1.5, xc));
    if fresp.on_hover_text("Fold the drawer").clicked() {
        app.ui.drawers.remove(&id);
    }
    // The title (track · module, and the plugin's name) opens the module's menu: Freeze.
    if crate::chain_view::offers_freeze(track) {
        let title = Rect::from_min_max(pos2(inner.min.x, y), pos2(fold.min.x - 30.0, y + 30.0));
        let tresp = ui.interact(title, ui.id().with("title-menu"), Sense::click());
        tresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} {} drawer", track.name, info.name)));
        tresp.on_hover_text("Right-click: Freeze up to here, Freeze whole chain, Unfreeze").context_menu(|ui| {
            ui.set_min_width(170.0);
            crate::chain_view::freeze_items(app, ui, track, ins);
        });
    }
    if frozen {
        ui.disable();
    }

    let power = Rect::from_min_size(pos2(fold.min.x - 26.0, y), vec2(20.0, 20.0));
    let presp = ui.interact(power, ui.id().with("power"), Sense::click());
    presp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} on or off", info.short_name)));
    let pc = power.center();
    let on_col = if off { t.text_dim } else { t.meter_green };
    if !off {
        p.circle_filled(pc, 10.0, t.meter_green.gamma_multiply(0.14));
    }
    if presp.hovered() {
        p.circle_stroke(pc, 10.0, Stroke::new(1.0, t.text_dim));
    }
    // The power symbol: an open ring and a bar through its gap.
    let ring: Vec<Pos2> = (0..=24).map(|i| (-150.0f32 + 300.0 * i as f32 / 24.0).to_radians()).map(|a| pc + vec2(a.sin(), -a.cos()) * 5.5).collect();
    p.add(Shape::line(ring, Stroke::new(1.6, on_col)));
    p.line_segment([pc - vec2(0.0, 7.0), pc - vec2(0.0, 1.5)], Stroke::new(1.6, on_col));
    let tip = if off { "Bypassed: click to turn on" } else { "On: click to bypass" };
    if presp.on_hover_text(tip).clicked() {
        let _ = app.run("mix.module_bypass", json!({"track": id.0, "module": ins.id.0}));
    }

    // Track · module, then the plugin's full name.
    let mut job = egui::text::LayoutJob::default();
    let fmt = |c: Color32| egui::TextFormat { font_id: bold(11.0), color: c, extra_letter_spacing: 0.7, ..Default::default() };
    job.append(&track.name.to_uppercase(), 0.0, fmt(t.text_dim));
    job.append(" · ", 0.0, fmt(t.text_dim));
    job.append(&info.short_name.to_uppercase(), 0.0, fmt(t.text));
    let g = p.layout_job(job);
    let title_clip = Rect::from_min_max(pos2(inner.min.x, y), pos2(power.min.x - 4.0, y + 20.0));
    p.with_clip_rect(title_clip).galley(pos2(inner.min.x, y + 1.0), g, t.text);
    let sub = if off { format!("{} · bypassed", info.name) } else { info.name.to_string() };
    p.with_clip_rect(title_clip.translate(vec2(0.0, 14.0))).text(pos2(inner.min.x, y + 16.0), Align2::LEFT_TOP, sub, regular(9.5), t.text_dim);
    let y = y + 34.0;

    // The preset menu, and which knobs the tile shows.
    let knobs_w = 74.0;
    let preset_r = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width() - knobs_w - 6.0, 22.0));
    let knobs_r = Rect::from_min_size(pos2(preset_r.max.x + 6.0, y), vec2(knobs_w, 22.0));
    let preset = if ins.preset.is_empty() { "<factory default>".to_string() } else { ins.preset.clone() };
    let presp = menu_field(ui, preset_r, &preset, "preset", true);
    let presp = presp.on_hover_text("Presets: recall, or save the current settings");
    egui::Popup::menu(&presp)
        .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
        .show(|ui| crate::panels::preset_menu(app, ui, id, ins.id, info, ins));
    let kresp = menu_field(ui, knobs_r, "Key knobs…", "knobs", false);
    let kresp = kresp.on_hover_text("Which knobs the tile shows on the strip (up to four)");
    egui::Popup::menu(&kresp).show(|ui| knobs_menu(app, ui, id, ins, info));
    let mut y = y + 22.0;
    if crate::sidechain_picker::shows_picker(info, ins) {
        y = crate::sidechain_picker::row(app, ui, track, ins, inner, y + 6.0);
    }
    if crate::link_picker::shows_link(info, ins) {
        y = crate::link_picker::link_row(app, ui, track, ins, info, inner, y + 6.0);
    }
    if info.references > 0 {
        y = crate::link_picker::reference_rows(app, ui, track, ins, info, inner, y + 6.0) - 4.0;
    }
    y
}

/// A flat menu field: a dark well with its text, and a small arrow when `arrow`.
pub(crate) fn menu_field(ui: &mut Ui, r: Rect, text: &str, salt: &str, arrow: bool) -> egui::Response {
    let t = Tokens::current();
    let resp = ui.interact(r, ui.id().with(("field", salt)), Sense::click());
    let label = text.to_string();
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, &label));
    let p = ui.painter();
    let fill = if resp.hovered() { t.graph_bg.lerp_to_gamma(t.module_tile_border, 0.5) } else { t.graph_bg };
    p.rect_filled(r, CornerRadius::same(5), fill);
    let edge = if resp.hovered() { t.accent.gamma_multiply(0.7) } else { t.module_tile_border };
    p.rect_stroke(r, CornerRadius::same(5), Stroke::new(1.0, edge), StrokeKind::Inside);
    let text_r = if arrow { r.with_max_x(r.max.x - 16.0) } else { r };
    let (pos, align) = if arrow { (pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER) } else { (r.center(), Align2::CENTER_CENTER) };
    p.with_clip_rect(text_r.shrink(2.0)).text(pos, align, text, regular(10.0), t.text);
    if arrow {
        let a = pos2(r.max.x - 10.0, r.center().y);
        p.add(Shape::convex_polygon(vec![a + vec2(-3.5, -1.5), a + vec2(3.5, -1.5), a + vec2(0.0, 2.5)], t.text_dim, Stroke::NONE));
    }
    resp
}

/// A hairline across the card.
fn divider(p: &Painter, inner: Rect, y: f32) {
    p.hline(inner.x_range(), y, Stroke::new(1.0, Tokens::current().module_tile_border));
}

/// A small caption in the top-left corner of a graph.
fn caption(p: &Painter, r: Rect, text: &str) {
    let t = Tokens::current();
    p.text(r.min + vec2(5.0, 4.0), Align2::LEFT_TOP, text, regular(8.5), t.text_dim);
}

/// The gain reduction over the last four seconds, newest at the right, reduction hanging down
/// from the top; the current amount above it.
fn gr_history(p: &Painter, r: Rect, history: Option<&GrHistory>, now: Option<f32>) {
    let t = Tokens::current();
    let p = p.with_clip_rect(r);
    p.rect_filled(r, CornerRadius::same(3), t.graph_bg);
    let y_of = |db: f32| r.top() + 2.0 + (r.height() - 16.0) * (db.clamp(0.0, GR_RANGE) / GR_RANGE);
    for db in [6.0, 12.0, 18.0] {
        let y = y_of(db);
        p.hline(r.x_range(), y, Stroke::new(1.0, t.graph_grid));
        p.text(pos2(r.max.x - 3.0, y - 1.0), Align2::RIGHT_BOTTOM, format!("-{db:.0}"), regular(8.0), t.text_dim);
    }
    if let Some(h) = history.map(|h| &h.values).filter(|h| h.len() >= 2) {
        let step = r.width() / (GR_HISTORY_LEN.saturating_sub(1).max(1)) as f32;
        let n = h.len();
        let pts: Vec<Pos2> = h.iter().enumerate().map(|(i, v)| pos2(r.max.x - (n - 1 - i) as f32 * step, y_of(*v))).collect();
        let top = y_of(0.0);
        for w in pts.windows(2) {
            if let [a, b] = w {
                p.add(Shape::convex_polygon(vec![pos2(a.x, top), pos2(b.x, top), *b, *a], t.gr_color.gamma_multiply(0.22), Stroke::NONE));
            }
        }
        p.add(Shape::line(pts, Stroke::new(1.5, t.gr_color)));
    }
    let gr = now.filter(|v| v.is_finite()).map_or(0.0, f32::abs);
    let text = match now {
        // Nothing to reduce reads 0.0, not −0.0.
        Some(_) if gr < 0.05 => "GR 0.0 dB".to_string(),
        Some(_) => format!("GR −{gr:.1} dB"),
        None => "GR".to_string(),
    };
    p.text(pos2(r.min.x + 5.0, r.max.y - 3.0), Align2::LEFT_BOTTOM, text, bold(9.0), if gr > 0.05 { t.gr_color } else { t.text_dim });
    p.text(pos2(r.max.x - 4.0, r.max.y - 3.0), Align2::RIGHT_BOTTOM, "last 4 s", regular(8.0), t.text_dim);
    p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, t.module_tile_border), StrokeKind::Inside);
}

/// The hero's value text: the parameter's own, except the Filter's Cutoff, which (as its editor's
/// dial does) prints the frequency the engine is actually running at (readout `[2]`), or where the
/// knob puts it when the module is not running.
///
/// Departure: while the knob is dragged it prints where the knob puts the cutoff (the engine's
/// map, unmodulated) rather than the live value as the C++ dial does, so the number answers the
/// drag at once instead of lagging the engine's 40 ms readout cadence.
fn hero_value_text(ctx: &egui::Context, ins: &Insert, info: &ParamInfo, shown: f32, dragging: bool, live: Option<&LiveValues>) -> String {
    use soundcraft_dsp::plugins::fs::filter::filter_config::format_cutoff_hz;
    if ins.plugin != "fs_filter" || info.id != "cutoff" {
        return value_text(ctx, ins, info, shown);
    }
    let live_hz = live.map_or(f32::NAN, |l| l.readouts[2]);
    if !dragging && live_hz.is_finite() && live_hz > 0.0 {
        return format_cutoff_hz(live_hz);
    }
    let mut at = ins.clone();
    at.params.insert("cutoff".into(), shown);
    format_cutoff_hz(crate::module_graphs::filter_display_cached(ctx, &at).knob_hz)
}

/// The main control as a large knob, its name above and value below; the module's in and out
/// levels (and a compressor's gain reduction) as meters beside it.
#[allow(clippy::too_many_arguments)]
fn hero_band(
    app: &mut SoundApp,
    ui: &mut Ui,
    track: TrackId,
    ins: &Insert,
    info: &'static ParamInfo,
    band: Rect,
    live: Option<LiveValues>,
    kind: GraphKind,
    accent: Color32,
) {
    let t = Tokens::current();
    let knob_area = Rect::from_min_max(band.min, pos2(band.min.x + band.width() * 0.62, band.max.y));
    let cx = knob_area.center().x;
    let p = ui.painter().clone();
    let (label, synced_note) = match delay_hero_note(ins, info, live) {
        Some(note) => ("NOTE".to_string(), Some(note)),
        None => (hero_label(ins, info), None),
    };
    p.text(pos2(cx, band.min.y), Align2::CENTER_TOP, label, bold(9.5), t.text_dim);
    let center = pos2(cx, band.min.y + 14.0 + 50.0);
    let value = ins.params.get(info.id).copied().unwrap_or(info.default);
    let out = knob_at(ui, info, value, center, HERO, KnobStyle::Hero, ("hero", info.id), accent);
    let shown = out.changed.unwrap_or(value);
    p.text(
        pos2(cx, band.max.y),
        Align2::CENTER_BOTTOM,
        synced_note.unwrap_or_else(|| hero_value_text(ui.ctx(), ins, info, shown, out.changed.is_some(), live.as_ref())),
        bold(12.0),
        t.text,
    );
    apply_knob(app, ui, track, ins, info, out);

    // Meters.
    let mut bars: Vec<(&str, f32, Color32)> = Vec::new();
    let lv = live.unwrap_or_default();
    bars.push(("IN", level_pos(lv.in_peak), t.meter_green));
    bars.push(("OUT", level_pos(lv.out_peak), t.meter_green));
    if matches!(kind, GraphKind::Compressor | GraphKind::Comp | GraphKind::Twine) {
        let gr = if lv.gr_db.is_finite() { (lv.gr_db.abs() / GR_RANGE).clamp(0.0, 1.0) } else { 0.0 };
        bars.push(("GR", gr, t.gr_color));
    }
    let meters = Rect::from_min_max(pos2(knob_area.max.x + 4.0, band.min.y + 6.0), pos2(band.max.x, band.max.y));
    let col_w = meters.width() / bars.len().max(1) as f32;
    for (i, (label, v, color)) in bars.into_iter().enumerate() {
        let cx = meters.min.x + col_w * (i as f32 + 0.5);
        let bar = Rect::from_min_max(pos2(cx - 4.0, meters.min.y), pos2(cx + 4.0, meters.max.y - 16.0));
        p.rect_filled(bar, CornerRadius::same(2), t.graph_bg);
        p.rect_stroke(bar, CornerRadius::same(2), Stroke::new(1.0, t.module_tile_border), StrokeKind::Inside);
        let inner = bar.shrink(1.5);
        let fill = if label == "GR" {
            // Reduction hangs from the top.
            Rect::from_min_size(inner.min, vec2(inner.width(), inner.height() * v))
        } else {
            Rect::from_min_max(pos2(inner.min.x, inner.max.y - inner.height() * v), inner.max)
        };
        let color = if label != "GR" && v > 0.97 {
            t.meter_red
        } else if label != "GR" && v > 0.85 {
            t.meter_yellow
        } else {
            color
        };
        if v > 0.0 {
            p.rect_filled(fill, CornerRadius::same(1), color);
        }
        p.text(pos2(cx, meters.max.y), Align2::CENTER_BOTTOM, label, bold(8.5), t.text_dim);
    }
}

/// A linear peak as a 0..1 meter position over [`METER_FLOOR`]..0 dBFS.
fn level_pos(peak: f32) -> f32 {
    if !peak.is_finite() || peak <= 0.0 {
        return 0.0;
    }
    ((20.0 * peak.log10() - METER_FLOOR) / -METER_FLOOR).clamp(0.0, 1.0)
}

/// A knob centred on `center`.
pub(crate) fn knob_at(
    ui: &mut Ui,
    info: &ParamInfo,
    value: f32,
    center: Pos2,
    size: f32,
    style: KnobStyle,
    salt: impl std::hash::Hash + std::fmt::Debug,
    accent: Color32,
) -> KnobResponse {
    let r = Rect::from_center_size(center, Vec2::splat(knob_side(size, style)));
    let mut c = ui.new_child(egui::UiBuilder::new().id_salt(egui::Id::new(salt)).max_rect(r).layout(egui::Layout::top_down(egui::Align::Center)));
    knob(&mut c, info, value, size, accent, style)
}

/// The side of the square a knob of `size` allocates (see `widgets::knob_geom`).
pub(crate) fn knob_side(size: f32, style: KnobStyle) -> f32 {
    let hero = style == KnobStyle::Hero;
    let s = (size / if hero { 140.0 } else { 64.0 }).clamp(0.5, 2.0);
    let (tick_r, tick_len) = if size < 24.0 { (size * 0.5 + 4.0, 3.0) } else { (size * 0.5 + 9.0 * s, 6.0 * s) };
    ((tick_r + tick_len * 0.5).max(size * 0.5) + 1.0) * 2.0
}

/// One parameter of the grid: its name, its knob (a pill for toggles and choices), its value.
#[allow(clippy::too_many_arguments)]
pub(crate) fn param_cell(
    app: &mut SoundApp,
    ui: &mut Ui,
    track: TrackId,
    ins: &Insert,
    info: &'static ParamInfo,
    name: &str,
    cell: Rect,
    accent: Color32,
) {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let value = ins.params.get(info.id).copied().unwrap_or(info.default);
    let g = p.layout_no_wrap(name.to_string(), regular(9.5), t.text_dim);
    let label_at = Align2::CENTER_TOP.anchor_size(pos2(cell.center().x, cell.min.y + 4.0), g.size()).min;
    let label_bottom = label_at.y + g.size().y;
    p.with_clip_rect(cell).galley(label_at, g, t.text_dim);
    let center = pos2(cell.center().x, label_bottom + KNOB_BELOW_LABEL);
    match info.unit {
        Unit::Toggle if is_momentary(ins, info.id) => {
            let r = Rect::from_center_size(center, vec2(46.0, 20.0));
            let resp = ui.interact(r, ui.id().with(("tap", info.id)), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, info.name));
            let held = momentary(app, ui, track, ins, info.id, resp.is_pointer_button_down_on());
            // Delay's is a Tap; LevelUp's Normalize and Learn are presses.
            let word = if ins.plugin == "fs_delay" { "TAP" } else { "PRESS" };
            pill(&p, r, word, held, resp.hovered(), accent);
            let _ = resp.on_hover_text(momentary_tip(ins, info.id));
        }
        Unit::Toggle => {
            let on = value >= 0.5;
            let r = Rect::from_center_size(center, vec2(46.0, 20.0));
            let resp = ui.interact(r, ui.id().with(("pill", info.id)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, on, info.name));
            pill(&p, r, if on { "ON" } else { "OFF" }, on, resp.hovered(), accent);
            let what = param_tip(ins, info.id).map_or(String::new(), |tip| format!("{tip}\n"));
            if resp.on_hover_text(format!("{what}{}: click to turn {}", info.name, if on { "off" } else { "on" })).clicked() {
                let _ =
                    app.run("mix.module_param", json!({"track": track.0, "module": ins.id.0, "param": info.id, "value": if on { 0.0 } else { 1.0 }}));
            }
        }
        Unit::Choice => {
            let r = Rect::from_center_size(center, vec2((cell.width() - 6.0).min(72.0), 20.0));
            let resp = ui.interact(r, ui.id().with(("choice", info.id)), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, name));
            // A model-dependent switch shows its positions by the model's own names.
            let opts = choice_names(ins, info);
            let text = opts.get(value.round().max(0.0) as usize).map_or_else(|| info.format(value), |o| (*o).to_string());
            pill(&p, r, &text, false, resp.hovered(), accent);
            let resp = resp.on_hover_text(name);
            egui::Popup::menu(&resp).show(|ui| {
                for (i, c) in opts.iter().enumerate() {
                    let cur = (value.round() as usize) == i;
                    if ui.add(egui::Button::selectable(cur, *c)).clicked() {
                        let _ = app.run("mix.module_param", json!({"track": track.0, "module": ins.id.0, "param": info.id, "value": i as f32}));
                        ui.close();
                    }
                }
            });
        }
        _ => {
            let out = knob_at(ui, info, value, center, KNOB, KnobStyle::Dark, ("knob", info.id), accent);
            let shown = out.changed.unwrap_or(value);
            let vy = center.y + knob_side(KNOB, KnobStyle::Dark) * 0.5 + 1.0;
            p.with_clip_rect(cell).text(pos2(cell.center().x, vy), Align2::CENTER_TOP, value_text(ui.ctx(), ins, info, shown), regular(9.5), t.text);
            apply_knob(app, ui, track, ins, info, out);
        }
    }
}

/// A rounded pill: lit in the accent when `on`.
pub(crate) fn pill(p: &Painter, r: Rect, text: &str, on: bool, hovered: bool, accent: Color32) {
    let t = Tokens::current();
    let round = CornerRadius::same((r.height() * 0.5) as u8);
    let fill = if on { accent } else { t.graph_bg };
    p.rect_filled(r, round, fill);
    let edge = if hovered || on { accent } else { t.module_tile_border };
    p.rect_stroke(r, round, Stroke::new(1.0, edge), StrokeKind::Inside);
    let color = if on { t.mode_on_text } else { t.text };
    // A long name (a Dist circuit) steps down a size before it is clipped.
    let fits = |size: f32| p.layout_no_wrap(text.to_string(), bold(size), color).size().x <= r.width() - 8.0;
    let size = [9.0, 8.0, 7.0].into_iter().find(|s| fits(*s)).unwrap_or(7.0);
    p.with_clip_rect(r.shrink(2.0)).text(r.center(), Align2::CENTER_CENTER, text, bold(size), color);
}

/// Sends a knob's change: merged while it is dragged, so a drag is one undo step.
pub(crate) fn apply_knob(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, info: &ParamInfo, out: KnobResponse) {
    let busy = ui.id().with(("drawer-knob-busy", info.id));
    if let Some(v) = out.changed {
        let key = format!("modparam:{}:{}:{}", track.0, ins.id.0, info.id);
        if let Err(e) =
            app.engine.execute_merged("mix.module_param", &json!({"track": track.0, "module": ins.id.0, "param": info.id, "value": v}), &key)
        {
            app.ui.status = e.to_string();
        }
        if !out.dragging {
            app.engine.end_merge();
        }
    }
    if out.dragging {
        // Held, it keeps writing automation standing still.
        app.hold_control(track, soundcraft_model::AutoParam::Plugin { module: ins.id, param: info.id.to_string() });
        ui.data_mut(|d| d.insert_temp(busy, true));
    } else if ui.data(|d| d.get_temp::<bool>(busy)).unwrap_or(false) {
        ui.data_mut(|d| d.remove::<bool>(busy));
        app.engine.end_merge();
    }
}

/// Dragging an EQ band dot sets its frequency and gain, one undo step per drag.
fn eq_drag(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, r: Rect, sr: f32) {
    let busy = ui.id().with("drawer-eq-busy");
    if let Some(params) = eq_band_drag(ui, r, ins, sr) {
        let map: serde_json::Map<String, serde_json::Value> = params.into_iter().map(|(k, v)| (k, json!(v))).collect();
        let key = format!("modparams:{}:{}", track.0, ins.id.0);
        if let Err(e) = app.engine.execute_merged("mix.module_params", &json!({"track": track.0, "module": ins.id.0, "params": map}), &key) {
            app.ui.status = e.to_string();
        }
        ui.data_mut(|d| d.insert_temp(busy, true));
    } else if ui.data(|d| d.get_temp::<bool>(busy)).unwrap_or(false) {
        ui.data_mut(|d| d.remove::<bool>(busy));
        app.engine.end_merge();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn heroes_are_main_controls() {
        let hero = |id: &str| soundcraft_dsp::plugin_info(id).and_then(hero_param).map(|p| p.id);
        assert_eq!(hero("compressor"), Some("threshold"));
        assert_eq!(hero("channel_strip"), Some("comp_threshold"));
        assert_eq!(hero("eq_7band"), None);
        for p in soundcraft_dsp::plugins() {
            if let Some(h) = hero_param(p) {
                assert!(!matches!(h.unit, Unit::Toggle | Unit::Choice), "{}", p.id);
            }
        }
    }

    #[test]
    fn firestation_heroes_and_tabs() {
        let hero = |id: &str| soundcraft_dsp::plugin_info(id).and_then(hero_param).map(|p| p.id);
        assert_eq!(hero("fs_comp"), Some("amount"));
        assert_eq!(hero("fs_sat"), Some("drive"));
        assert_eq!(hero("fs_tape"), Some("drive"));
        assert_eq!(hero("fs_twine"), Some("ceiling"));
        assert_eq!(hero("fs_cleanq"), None, "the curve is the hero");
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        assert_eq!(hero_name(comp.param("amount").unwrap()), "Amount");

        // Ordered distinct groups; the engine's own bypass (group "") is hidden.
        assert_eq!(drawer_groups(comp), ["Lane A", "Global", "Lane B", "Lane C", "Crossover"]);
        let sat = soundcraft_dsp::plugin_info("fs_sat").unwrap();
        assert_eq!(drawer_groups(sat), ["Circuit", "Drive", "Output", "Options"]);
        let cq = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        let g = drawer_groups(cq);
        assert_eq!(g.len(), 26);
        assert_eq!((g[0], g[1], g[24], g[25]), ("Global", "Band 1", "Band 24", "Ceiling"));
        for p in [comp, sat, cq] {
            assert!(!drawer_groups(p).contains(&""), "{}: bypass hidden", p.id);
        }
        // A built-in keeps one (unnamed) group: no tabs, and nothing hidden.
        let eq = soundcraft_dsp::plugin_info("eq_7band").unwrap();
        assert_eq!(drawer_groups(eq), [""]);
        assert!(eq.params.iter().all(|p| drawer_shows(&Insert::new("eq_7band"), p)));

        assert_eq!(tab_label("Band 12"), "12");
        assert_eq!(tab_label("Lane A"), "Lane A");
        assert_eq!(tab_label(""), "Main");
        let lane: Vec<&ParamInfo> = comp.params.iter().filter(|p| p.group == "Lane B").collect();
        assert_eq!(tab_names(&lane)[..3], ["On", "Level", "Style"]);
        let global: Vec<&ParamInfo> = comp.params.iter().filter(|p| p.group == "Global").collect();
        assert_eq!(tab_names(&global), ["Dry", "Channels", "Dry On"], "no shared word: names kept");
    }

    #[test]
    fn cleanq_ceiling_group_is_open() {
        // B3: the dynamic ceiling has its drawn and learned ceiling, so its tab is a live one.
        let cq = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        for g in drawer_groups(cq) {
            assert_eq!(group_disabled("fs_cleanq", g), None, "{g}");
        }
        assert_eq!(cq.param("ceilOn").map(|p| p.group), Some("Ceiling"));
        assert_eq!(group_disabled("fs_twine", "Limiter"), None);
        assert_eq!(group_disabled("fs_vint", "Ceiling"), None);
    }

    #[test]
    fn reverb_shows_what_its_machine_shows() {
        // ReverbConfig.h `whatApplies` / `characterLabel`: Character renamed per machine, Interval on
        // the Shimmer only, the Spring without Low Decay / Diffusion / Color / Attack, Sync swapping
        // Pre-Delay for the note; Manual BPM and Tempo Src hidden (the session tempo drives sync).
        use crate::key_knobs::{tile_knob_applies, tile_knob_label};
        let r = soundcraft_dsp::plugin_info("fs_reverb").unwrap();
        assert_eq!(drawer_groups(r), ["Main", "Time", "Setup", "Tone", "Motion", "Dynamics"]);
        assert_eq!(hero_param(r).map(|p| p.id), Some("decay"));
        let ids = |ins: &Insert, g: &str| grid_params(r, ins, Some("decay"), Some(g)).0.iter().map(|p| p.id).collect::<Vec<_>>();
        let names = |ins: &Insert, g: &str| grid_params(r, ins, Some("decay"), Some(g)).1;
        let mut ins = Insert::new("fs_reverb");
        // Hall: Early, no Interval.
        assert_eq!(ids(&ins, "Main"), ["mix", "machine", "size", "character", "freeze", "color"]);
        assert_eq!(names(&ins, "Main"), ["Mix", "Machine", "Size", "Early", "Freeze", "Color"]);
        assert_eq!(ids(&ins, "Motion"), ["modRate", "modDepth"]);
        assert_eq!(ids(&ins, "Setup"), ["level", "width", "trails"]);
        assert_eq!(ids(&ins, "Time"), ["preDelay", "sync", "attack"]);
        assert_eq!(ids(&ins, "Tone"), ["lowCut", "highCut", "damping", "bass", "diffusion"]);
        // Shimmer: its Interval, Character is Shimmer.
        ins.params.insert("machine".into(), 5.0);
        assert_eq!(ids(&ins, "Motion"), ["modRate", "modDepth", "interval"]);
        assert_eq!(model_switch(&ins, "interval"), Some(Switch::Show { label: "Interval", opts: &["+Oct", "+5th", "-Oct"] }));
        assert_eq!(dial_name(&ins, "character"), Some("Shimmer"));
        // Spring: Dwell; no band-split decay, diffusion, colour or attack.
        ins.params.insert("machine".into(), 3.0);
        assert_eq!(dial_name(&ins, "character"), Some("Dwell"));
        assert_eq!(ids(&ins, "Tone"), ["lowCut", "highCut", "damping"]);
        assert_eq!(ids(&ins, "Main"), ["mix", "machine", "size", "character", "freeze"]);
        assert_eq!(ids(&ins, "Time"), ["preDelay", "sync"]);
        // Plate: Zing.
        ins.params.insert("machine".into(), 2.0);
        assert_eq!(dial_name(&ins, "character"), Some("Zing"));
        // Sync: the note instead of Pre-Delay, and the tile's Pre-Delay knob dims.
        let pre = r.param("preDelay").unwrap();
        assert!(tile_knob_applies(&ins, "preDelay"));
        assert_eq!(tile_knob_label(&ins, pre), "Pre-Delay");
        ins.params.insert("sync".into(), 1.0);
        assert_eq!(ids(&ins, "Time"), ["sync", "division", "attack"]);
        assert_eq!(names(&ins, "Time")[1], "Pre-Delay Note");
        assert!(!tile_knob_applies(&ins, "preDelay"));
        // Tempo Src and Manual BPM: hidden on Host, shown (to see and put it back) on Manual.
        assert_eq!(model_switch(&ins, "tempoSrc"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "tempo"), Some(Switch::Hide));
        ins.params.insert("tempoSrc".into(), 1.0);
        assert_eq!(model_switch(&ins, "tempoSrc"), Some(Switch::Show { label: "Tempo Src", opts: &["Host", "Manual"] }));
        assert_eq!(model_switch(&ins, "tempo"), None);
        // Nonsense reads as the default machine.
        ins.params.insert("machine".into(), f32::NAN);
        assert_eq!(dial_name(&ins, "character"), Some("Early"));
        assert_eq!(model_switch(&ins, "decay"), None);
    }

    #[test]
    fn mod_shows_what_its_circuit_shows() {
        // ModConfig.h: the circuit's own variant names and dials, Sync between Rate and Division.
        let m = soundcraft_dsp::plugin_info("fs_mod").unwrap();
        assert_eq!(drawer_groups(m), ["Pedal", "Voicing", "LFO"]);
        assert_eq!(hero_param(m).map(|p| p.id), Some("depth"));
        let ids = |ins: &Insert, g: &str| grid_params(m, ins, Some("depth"), Some(g)).0.iter().map(|p| p.id).collect::<Vec<_>>();
        let names = |ins: &Insert, g: &str| grid_params(m, ins, Some("depth"), Some(g)).1;
        let mut ins = Insert::new("fs_mod");
        // Dim D: its six buttons; Noise only; Rate free-running; Link and its set points.
        assert_eq!(ids(&ins, "Pedal"), ["rate", "level", "circuit", "variant", "trim", "link", "linkLo", "linkHi", "mix", "width"]);
        assert_eq!(names(&ins, "Pedal"), ["Rate", "Level", "Circuit", "Variant", "Input", "Link", "Level@0", "Level@10", "Mix", "Width"]);
        // Link owns Level: its dial (and the tile's Level knob) is locked while Link is on.
        assert!(!param_locked(&ins, "level"));
        ins.params.insert("link".into(), 1.0);
        assert!(param_locked(&ins, "level"));
        assert!(!param_locked(&ins, "depth"));
        ins.params.insert("link".into(), 0.0);
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Show { label: "Variant", opts: &["1", "2", "3", "4", "1 + 4", "2 + 3"] }));
        assert_eq!(ids(&ins, "Voicing"), ["drive", "noise", "stereo"]);
        assert_eq!(names(&ins, "Voicing"), ["Drive", "Noise", "Input"]);
        assert_eq!(ids(&ins, "LFO"), ["shape", "sync"]);
        // CE-2: no variants.
        ins.params.insert("circuit".into(), 2.0);
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Hide));
        // A/DA: Regen is live.
        ins.params.insert("circuit".into(), 7.0);
        assert_eq!(ids(&ins, "Voicing"), ["regen", "drive", "noise", "stereo"]);
        // Small Stone: Colour off/on, Regen and Bias, no Noise.
        ins.params.insert("circuit".into(), 9.0);
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Show { label: "Variant", opts: &["Colour off", "Colour on"] }));
        assert_eq!(ids(&ins, "Voicing"), ["regen", "drive", "bias", "stereo"]);
        // Magnatone: Bias and Tone.
        ins.params.insert("circuit".into(), 13.0);
        assert_eq!(ids(&ins, "Voicing"), ["drive", "bias", "tone", "stereo"]);
        // Sync on: Division instead of Rate; Tempo stays hidden (the session tempo drives it).
        ins.params.insert("sync".into(), 1.0);
        assert_eq!(ids(&ins, "Pedal")[0], "level");
        assert_eq!(ids(&ins, "LFO"), ["shape", "sync", "division"]);
        assert_eq!(names(&ins, "LFO"), ["Shape", "Sync", "Div"]);
        // Nonsense reads as a real circuit (the default).
        ins.params.insert("circuit".into(), f32::NAN);
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Show { label: "Variant", opts: &["1", "2", "3", "4", "1 + 4", "2 + 3"] }));
        assert_eq!(model_switch(&ins, "depth"), None);
        assert!(matches!(model_switch(&ins, "circuit"), Some(Switch::Show { opts, .. }) if opts.len() == 14 && opts[3] == "Clone"));
    }

    #[test]
    fn filter_switches_follow_the_circuit_and_the_source() {
        let mut ins = Insert::new("fs_filter");
        // Q-Tron (the default): its Range and Mode rows, nobody else's.
        assert_eq!(model_switch(&ins, "qtMode"), Some(Switch::Show { label: "Mode", opts: &["LP", "BP", "HP", "Mix"] }));
        assert_eq!(model_switch(&ins, "qtRange"), Some(Switch::Show { label: "Range", opts: &["Lo", "Hi"] }));
        assert_eq!(model_switch(&ins, "muMode"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "cbVoice"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "cutoff"), Some(Switch::Show { label: "Sweep", opts: &[] }));
        assert_eq!(model_switch(&ins, "drive"), None);
        // The session tempo drives sync: no BPM knob, no tempo source.
        assert_eq!(model_switch(&ins, "bpm"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "tempoSrc"), Some(Switch::Hide));
        // Envelope (the default source): its knobs, not the LFO's or the steps'.
        assert_eq!(model_switch(&ins, "envSens"), None);
        assert_eq!(model_switch(&ins, "lfoShape"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "step1"), Some(Switch::Hide));
        // Cry Baby: Pedal and Q, its Inductor and Sweep rows.
        ins.params.insert("circuit".into(), 2.0);
        assert_eq!(model_switch(&ins, "cutoff"), Some(Switch::Show { label: "Pedal", opts: &[] }));
        assert_eq!(model_switch(&ins, "cbVoice"), Some(Switch::Show { label: "Sweep", opts: &["Stock", "Deep", "Vocal"] }));
        assert_eq!(model_switch(&ins, "qtMode"), Some(Switch::Hide));
        // Moog keeps the name Cutoff.
        ins.params.insert("circuit".into(), 4.0);
        assert_eq!(model_switch(&ins, "cutoff"), None);
        assert_eq!(hero_label(&ins, soundcraft_dsp::plugin_info("fs_filter").unwrap().param("reso").unwrap()), "EMPHASIS");
        // Manual: Depth does nothing. Synced LFO: the note, not the free rate.
        ins.params.insert("modSrc".into(), 0.0);
        assert_eq!(model_switch(&ins, "depth"), Some(Switch::Hide));
        ins.params.insert("modSrc".into(), 2.0);
        assert_eq!(model_switch(&ins, "note"), None);
        assert_eq!(model_switch(&ins, "lfoRate"), Some(Switch::Hide));
        ins.params.insert("lfoSync".into(), 0.0);
        assert_eq!(model_switch(&ins, "lfoRate"), None);
        assert_eq!(model_switch(&ins, "note"), Some(Switch::Hide));
        // Steps: the row up to the count (index 2: four steps).
        ins.params.insert("modSrc".into(), 3.0);
        ins.params.insert("seqSteps".into(), 2.0);
        assert_eq!(model_switch(&ins, "step4"), None);
        assert_eq!(model_switch(&ins, "step5"), Some(Switch::Hide));
        let filter = soundcraft_dsp::plugin_info("fs_filter").unwrap();
        assert_eq!(drawer_groups(filter), ["Filter", "Mod"]);
        // The Cutoff hero prints Hz: live when running, the knob's map otherwise.
        let ctx = egui::Context::default();
        let cutoff = filter.param("cutoff").unwrap();
        let mut lv = LiveValues { readouts: [f32::NAN; 8], ..LiveValues::default() };
        assert_eq!(hero_value_text(&ctx, &ins, cutoff, 0.5, false, Some(&lv)), "735 Hz");
        lv.readouts[2] = 1234.0;
        assert_eq!(hero_value_text(&ctx, &ins, cutoff, 0.5, false, Some(&lv)), "1.23 kHz");
        assert_eq!(hero_value_text(&ctx, &ins, cutoff, 0.5, true, Some(&lv)), "735 Hz");
        assert_eq!(hero_value_text(&ctx, &ins, filter.param("drive").unwrap(), 0.0, false, Some(&lv)), "Off");
        // Hostile stored values never panic.
        for bad in [f32::NAN, f32::INFINITY, -9.0, 1e9] {
            for k in ["circuit", "modSrc", "seqSteps", "lfoSync"] {
                ins.params.insert(k.into(), bad);
            }
            for p in filter.params {
                let _ = model_switch(&ins, p.id);
            }
        }
    }

    #[test]
    fn model_switches_follow_the_engine_editors() {
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        let mut ins = Insert::new("fs_comp");
        ins.params.insert("style".into(), 1.0);
        let (params, names) = grid_params(comp, &ins, Some("amount"), Some("Lane A"));
        let label = |id: &str| params.iter().position(|p| p.id == id).map(|i| names[i].clone());
        assert_eq!(label("variant").as_deref(), Some("Revision"));
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Show { label: "Revision", opts: &["Blue stripe", "Blackface"] }));
        assert_eq!(label("detMode"), None, "the 1176 has no detector switch");
        // Lanes follow their own style: B (default 1176) and C (default LA-2A).
        assert_eq!(model_switch(&ins, "bDetMode"), Some(Switch::Hide));
        assert!(matches!(model_switch(&ins, "cDetMode"), Some(Switch::Show { label: "Emphasis", .. })));
        ins.params.insert("style".into(), 0.0);
        assert_eq!(model_switch(&ins, "variant"), Some(Switch::Hide), "Clean has neither");
        ins.params.insert("style".into(), 7.0);
        assert!(matches!(model_switch(&ins, "detMode"), Some(Switch::Show { label: "Detector", opts }) if opts.len() == 4));
        assert_eq!(model_switch(&ins, "ratio"), None);

        let sat = soundcraft_dsp::plugin_info("fs_sat").unwrap();
        let mut s = Insert::new("fs_sat");
        s.params.insert("circuit".into(), 1.0);
        assert_eq!(model_switch(&s, "variant"), Some(Switch::Show { label: "Variant", opts: &["Triode", "Pentode", "Starved"] }));
        let (params, names) = grid_params(sat, &s, Some("drive"), Some("Circuit"));
        assert_eq!(names, ["Circuit", "Variant"]);
        assert_eq!(params[1].id, "variant");
        let mut t = Insert::new("fs_tape");
        t.params.insert("machine".into(), 3.0);
        assert!(matches!(model_switch(&t, "formula"), Some(Switch::Show { opts, .. }) if opts[0] == "Type I"));
    }

    #[test]
    fn vint_switches_tabs_and_hero_follow_the_circuit() {
        let vint = soundcraft_dsp::plugin_info("fs_vint").unwrap();
        // Its groups as tabs, the engine's own bypass hidden.
        assert_eq!(drawer_groups(vint), ["Global", "Passive", "Console", "Proportional", "British", "Enhancer", "Air", "Sampler"]);
        assert_eq!(soundcraft_dsp::plugin_info("fs_vint").and_then(hero_param).map(|p| p.id), Some("drive"));
        let mut ins = Insert::new("fs_vint");
        let global = |ins: &Insert| grid_params(vint, ins, Some("drive"), Some("Global"));
        // The Console (the 1073): its class A stage, named; its drive is the gain switch.
        ins.params.insert("circuit".into(), 1.0);
        assert_eq!(model_switch(&ins, "stage"), Some(Switch::Show { label: "Class A Stage", opts: &["Off", "On"] }));
        let (params, names) = global(&ins);
        assert_eq!(params.iter().position(|p| p.id == "stage").map(|i| names[i].as_str()), Some("Class A Stage"));
        assert!(!hero_applies(&ins), "the Console's drive is its gain switch");
        assert_eq!(default_group(&ins, &drawer_groups(vint)), "Console");
        // The Sampler and the Passives have no output stage: the switch is hidden.
        for c in [0.0, 6.0, 7.0] {
            ins.params.insert("circuit".into(), c);
            assert_eq!(model_switch(&ins, "stage"), Some(Switch::Hide), "circuit {c}");
            assert!(global(&ins).0.iter().all(|p| p.id != "stage"), "circuit {c}");
            assert!(!hero_applies(&ins), "circuit {c}: no stage, no drive");
        }
        assert_eq!(default_group(&ins, &drawer_groups(vint)), "Passive", "the MEQ-5 lives in the Passive tab");
        ins.params.insert("circuit".into(), 3.0);
        assert_eq!(model_switch(&ins, "stage"), Some(Switch::Show { label: "Op-Amp Stage", opts: &["Off", "On"] }));
        assert!(hero_applies(&ins));
        assert_eq!(model_switch(&ins, "conHpf"), None);
        // Other engines open on their first tab.
        let comp = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        assert_eq!(default_group(&Insert::new("fs_comp"), &drawer_groups(comp)), "Lane A");
        assert!(hero_applies(&Insert::new("fs_comp")));
    }

    #[test]
    fn transient_is_one_page_with_attack_as_the_hero() {
        // The editor's single page: no tabs, Attack the big knob, nothing renamed or hidden per
        // source (TransientConfig.h has only the sources' blurbs).
        let tr = soundcraft_dsp::plugin_info("fs_transient").unwrap();
        assert_eq!(drawer_groups(tr), ["Transient"]);
        assert_eq!(hero_param(tr).map(|p| p.id), Some("attack"));
        let mut ins = Insert::new("fs_transient");
        for source in 0..7 {
            ins.params.insert("source".into(), source as f32);
            for p in tr.params {
                assert_eq!(model_switch(&ins, p.id), None, "{}", p.id);
            }
        }
        let (ids, _) = grid_params(tr, &ins, Some("attack"), None);
        let ids: Vec<&str> = ids.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["source", "sustain", "attackTime", "sustainTime", "attackFocus", "sustainFocus", "mix", "output"]);
        assert_eq!(crate::chain_view::model_name(&ins, tr).as_deref(), Some("Full Mix"));
        ins.params.insert("source".into(), 3.0);
        assert_eq!(crate::chain_view::model_name(&ins, tr).as_deref(), Some("Piano"));
    }

    #[test]
    fn pitch_shows_what_its_style_shows() {
        // PitchConfig.h: a style shows only the controls it uses, named its own way.
        let pitch = soundcraft_dsp::plugin_info("fs_pitch").unwrap();
        assert_eq!(drawer_groups(pitch), ["Voices", "Shape"]);
        assert_eq!(hero_param(pitch).map(|p| p.id), Some("dry"));
        let ids = |ins: &Insert, g: &str| grid_params(pitch, ins, Some("dry"), Some(g)).0.iter().map(|p| p.id).collect::<Vec<_>>();
        let names = |ins: &Insert, g: &str| grid_params(pitch, ins, Some("dry"), Some(g)).1;
        let mut ins = Insert::new("fs_pitch");
        // POG: the octave voices and Detune; 5th Mode only once the 5th is up.
        assert_eq!(ids(&ins, "Voices"), ["style", "sub", "octDown", "octUp", "oct2Up", "fifth", "detune"]);
        assert_eq!(ids(&ins, "Shape"), ["attack", "lpf", "reso", "level", "trim", "width", "grain"]);
        ins.params.insert("fifth".into(), 0.5);
        assert!(ids(&ins, "Voices").contains(&"fifthMode"));
        // Micro POG: sub and octave up.
        ins.params.insert("style".into(), 1.0);
        assert_eq!(ids(&ins, "Voices"), ["style", "sub", "octUp"]);
        // OC-2: the dividers, Direct, Track; no Grain or Width.
        ins.params.insert("style".into(), 2.0);
        assert_eq!(ids(&ins, "Voices"), ["style", "sub", "octDown"]);
        assert_eq!(names(&ins, "Voices"), ["Style", "-2 Oct", "-1 Oct"]);
        assert_eq!(ids(&ins, "Shape"), ["attack", "lpf", "reso", "level", "trim", "track"]);
        assert_eq!(model_switch(&ins, "dry"), Some(Switch::Show { label: "Direct", opts: &[] }));
        assert_eq!(hero_label(&ins, pitch.param("dry").unwrap()), "DIRECT");
        // H910: Spread and Doubles.
        ins.params.insert("style".into(), 3.0);
        assert_eq!(ids(&ins, "Voices"), ["style", "detune", "voice"]);
        assert_eq!(names(&ins, "Voices"), ["Style", "Spread", "Doubles"]);
        // Whammy: the pedal, its interval and glide.
        ins.params.insert("style".into(), 4.0);
        assert_eq!(ids(&ins, "Voices"), ["style", "voice", "interval", "pedal", "glide"]);
        assert_eq!(names(&ins, "Voices")[1], "Whammy");
        assert_eq!(model_switch(&ins, "level"), None);
        assert_eq!(hero_label(&ins, pitch.param("dry").unwrap()), "DRY");
    }

    #[test]
    fn groups_keep_together_in_the_grid() {
        let eq = soundcraft_dsp::plugin_info("eq_7band").unwrap();
        let params: Vec<&ParamInfo> = eq.params.iter().collect();
        let cells = grid_cells(&params, 4);
        assert_eq!(cells.len(), params.len());
        // Input with the high-pass filter, then a band to a row, then the low-pass and output.
        let row = |id: &str| cells[params.iter().position(|p| p.id == id).unwrap()].0;
        assert_eq!((row("input_gain"), row("hpf_on"), row("hpf_slope")), (0, 0, 0));
        assert_eq!((row("low_shelf_freq"), row("low_shelf_q")), (1, 1));
        assert_eq!((row("high_mid_freq"), row("high_mid_q")), (4, 4));
        assert_eq!((row("lpf_on"), row("output_gain")), (6, 6));
        // Every cell is used once.
        let mut seen = cells.clone();
        seen.sort_unstable();
        seen.dedup();
        assert_eq!(seen.len(), cells.len());
        assert!(cells.iter().all(|c| c.1 < 4));
        // Groups wider than a row wrap; no params, no cells.
        let strip = soundcraft_dsp::plugin_info("channel_strip").unwrap();
        let sp: Vec<&ParamInfo> = strip.params.iter().collect();
        assert!(grid_cells(&sp, 4).iter().all(|c| c.1 < 4));
        assert!(grid_cells(&[], 4).is_empty());
        assert_eq!(group_key("input"), "input");
    }

    #[test]
    fn dist_shows_its_circuits_own_dials_switches_and_set_points() {
        let dist = soundcraft_dsp::plugin_info("fs_dist").unwrap();
        assert_eq!(drawer_groups(dist), ["Pedal"], "one panel, as the editor");
        assert_eq!(hero_param(dist).map(|p| p.id), Some("drive"));
        let mut ins = Insert::new("fs_dist");
        ins.params.insert("circuit".into(), 1.0); // RAT
        let (params, names) = grid_params(dist, &ins, Some("drive"), None);
        let ids: Vec<&str> = params.iter().map(|p| p.id).collect();
        assert_eq!(ids, ["tone", "level", "circuit", "trim", "link", "ratLo", "ratHi", "ratModel", "ratDiode", "ratChip", "ratFat"]);
        assert_eq!(names, ["Filter", "Volume", "Circuit", "Input", "Link", "Link Lo", "Link Hi", "Model", "Diode", "Chip", "Fat"]);
        assert_eq!(model_switch(&ins, "ratChip"), Some(Switch::Show { label: "Chip", opts: &["Stock", "LM308", "OP07", "LM741", "TL071"] }));
        assert_eq!(model_switch(&ins, "tsModel"), Some(Switch::Hide));
        assert_eq!(model_switch(&ins, "tsLo"), Some(Switch::Hide));
        assert_eq!(dial_name(&ins, "drive"), Some("Distortion"));
        assert_eq!(dial_name(&ins, "trim"), None);
        assert_eq!(dial_name(&Insert::new("fs_sat"), "drive"), None);
        // Link owns Level: its dial is locked while Link is on, as the editor locks it.
        assert!(!param_locked(&ins, "level"));
        ins.params.insert("link".into(), 1.0);
        assert!(param_locked(&ins, "level"));
        assert!(!param_locked(&ins, "drive"));
        assert!(!param_locked(&Insert::new("fs_sat"), "level"));
        // Every circuit shows its own switches, all with at least two positions.
        for c in 0..12 {
            ins.params.insert("circuit".into(), c as f32);
            let (params, _) = grid_params(dist, &ins, Some("drive"), None);
            let switches = params.iter().filter(|p| p.unit == Unit::Choice && p.id != "circuit").count();
            assert_eq!(switches, soundcraft_dsp::plugins::fs::dist::wrapper::CIRCUITS[c].switches.len(), "circuit {c}");
        }
        // Garbage circuit values fall back to a real circuit.
        ins.params.insert("circuit".into(), f32::NAN);
        assert_eq!(dial_name(&ins, "drive"), Some("Drive"));
    }

    #[test]
    fn knob_sides_match_the_widget() {
        // The widget's own geometry: a 30-point dark knob takes a 44-point square.
        assert!((knob_side(30.0, KnobStyle::Dark) - 44.0).abs() < 0.01);
        assert!(knob_side(HERO, KnobStyle::Hero) > HERO);
    }

    #[test]
    fn delay_shows_what_its_style_routing_and_sync_use() {
        // DelayConfig.h `whatApplies` and `characterLabel`.
        let m = soundcraft_dsp::plugin_info("fs_delay").unwrap();
        assert_eq!(drawer_groups(m), ["Delay", "Stereo", "Setup", "Tone", "Motion", "Dynamics", "Rhythm"]);
        assert_eq!(hero_param(m).map(|p| p.id), Some("time"));
        let ids = |ins: &Insert, g: &str| grid_params(m, ins, Some("time"), Some(g)).0.iter().map(|p| p.id).collect::<Vec<_>>();
        let names = |ins: &Insert, g: &str| grid_params(m, ins, Some("time"), Some(g)).1;
        let mut ins = Insert::new("fs_delay");
        // Digital, Stereo, free time: no Heads, no Note; Character is Crush; Manual BPM hidden.
        assert_eq!(ids(&ins, "Delay"), ["feedback", "mix", "style", "sync", "character", "freeze", "tap", "reverse"]);
        assert_eq!(names(&ins, "Delay")[4], "Crush");
        assert_eq!(ids(&ins, "Stereo"), ["routing", "width"]);
        assert_eq!(ids(&ins, "Setup"), ["trim", "level", "trails"]);
        assert!(hero_applies(&ins));
        // Tape: Heads, and Character is Age.
        ins.params.insert("style".into(), 3.0);
        assert!(ids(&ins, "Delay").contains(&"heads"));
        assert_eq!(dial_name(&ins, "character"), Some("Age"));
        // ...not while a pattern rewrites the taps.
        ins.params.insert("pattern".into(), 2.0);
        assert!(!ids(&ins, "Delay").contains(&"heads"));
        // Sync: the Note instead of Time (the hero dims), the session tempo drives it.
        ins.params.insert("sync".into(), 1.0);
        assert!(ids(&ins, "Delay").contains(&"division"));
        assert_eq!(dial_name(&ins, "division"), Some("Note"));
        assert!(!hero_applies(&ins));
        assert!(!crate::key_knobs::tile_knob_applies(&ins, "time"));
        let time = m.param("time").unwrap();
        assert_eq!(delay_hero_note(&ins, time, None).as_deref(), Some("1/8.  \u{b7}  375 ms"));
        let mut lv = LiveValues::default();
        lv.readouts[2] = 100.0;
        assert_eq!(delay_hero_note(&ins, time, Some(lv)).as_deref(), Some("1/8.  \u{b7}  450 ms"));
        // Ping-pong: Swing. Dual: R Offset, and R Note while synced (Offset goes once R Note is set).
        ins.params.insert("routing".into(), 2.0);
        assert_eq!(ids(&ins, "Stereo"), ["routing", "swing", "width"]);
        ins.params.insert("routing".into(), 3.0);
        assert_eq!(ids(&ins, "Stereo"), ["routing", "offset", "width", "divisionR"]);
        ins.params.insert("divisionR".into(), 4.0);
        assert_eq!(ids(&ins, "Stereo"), ["routing", "width", "divisionR"]);
        // A tap set Manual: Tempo Src shows (to hand the tempo back), with the BPM in force.
        assert_eq!(model_switch(&ins, "tempo"), Some(Switch::Hide));
        ins.params.insert("tempoSrc".into(), 1.0);
        assert_eq!(ids(&ins, "Setup"), ["trim", "level", "trails", "tempo", "tempoSrc"]);
        assert_eq!(model_switch(&ins, "tempo"), None);
        // Mod Rate prints a note value under Mod Sync.
        let rate = m.param("modRate").unwrap();
        assert_eq!(value_text(&egui::Context::default(), &ins, rate, 0.4537), "0.50 Hz");
        ins.params.insert("modSync".into(), 1.0);
        assert_eq!(value_text(&egui::Context::default(), &ins, rate, 0.4537), "1 bar");
        // Nonsense reads as the defaults.
        let mut junk = Insert::new("fs_delay");
        for k in ["style", "routing", "sync", "pattern", "divisionR", "tempoSrc"] {
            junk.params.insert(k.into(), f32::NAN);
        }
        assert_eq!(ids(&junk, "Delay"), ["feedback", "mix", "style", "sync", "character", "freeze", "tap", "reverse"]);
    }

    #[test]
    fn delay_tap_mirrors_the_editor() {
        let ins = Insert::new("fs_delay");
        let mut r = [f32::NAN; 8];
        assert_eq!(delay_tap_params(&ins, None, &r), None, "no reading");
        r[4] = 0.0;
        r[5] = 120.0;
        assert_eq!(delay_tap_params(&ins, None, &r), None, "first sight is not a tap");
        assert_eq!(delay_tap_params(&ins, Some(0.0), &r), None, "nothing new");
        r[4] = 1.0;
        r[5] = 300.0;
        let p = delay_tap_params(&ins, Some(0.0), &r).unwrap();
        assert_eq!(p["tempoSrc"], 1.0);
        assert_eq!(p["tempo"], 1.0, "clamped to 240 bpm");
        assert_eq!(p["sync"], 1.0);
        // A reset starts the count again: not a tap.
        r[4] = 0.0;
        assert_eq!(delay_tap_params(&ins, Some(3.0), &r), None);
        let mut synced = Insert::new("fs_delay");
        synced.params.insert("sync".into(), 1.0);
        r[4] = 4.0;
        r[5] = 90.0;
        let p = delay_tap_params(&synced, Some(3.0), &r).unwrap();
        assert!(!p.contains_key("sync"));
        assert!((p["tempo"].as_f64().unwrap() - 0.25).abs() < 1e-6);
        assert!(is_momentary(&ins, "tap") && !is_momentary(&ins, "freeze") && !is_momentary(&Insert::new("fs_mod"), "tap"));
    }

    #[test]
    fn a_tap_anywhere_shows_the_tempo_switch_and_host_takes_it_back() {
        // A tap the engine accepted with no drawer open (a lane, a controller): the app's watcher
        // still runs the editor's follow-up, so the Tempo switch shows; Host takes the session
        // tempo back (the engine clears the tap's force when Tempo Src is touched).
        use soundcraft_model::{ChannelFormat, TrackKind};
        let mut t = Track::new(TrackId(3), "Gtr", TrackKind::Audio, ChannelFormat::Stereo, [1, 2, 3]);
        let mut ins = Insert::new("fs_delay");
        ins.id = ModuleId(7);
        t.mixer.inserts.push(ins);
        let mut snap = soundcraft_playback::MeterSnapshot::default();
        let meter = |seq: f32, bpm: f32| soundcraft_mix::ModuleMeter {
            module: ModuleId(7),
            readouts: [500.0, 500.0, bpm, 0.0, seq, bpm, if seq > 0.0 { 1.0 } else { 0.0 }, 0.0],
            ..Default::default()
        };
        let mut strip = soundcraft_mix::StripMeter::default();
        strip.modules.push(meter(0.0, 120.0));
        snap.tracks.insert(TrackId(3), strip.clone());
        let mut seen = TapSeen::new();
        assert!(delay_tap_followups(std::slice::from_ref(&t), &snap, &mut seen).is_empty(), "first sight");
        // First sight of an engine already off the session: the follow-up runs at its tempo.
        let mut off = strip.clone();
        off.modules[0].readouts[6] = 1.0;
        off.modules[0].readouts[2] = 96.0;
        let mut fresh_seen = TapSeen::new();
        let mut snap_off = soundcraft_playback::MeterSnapshot::default();
        snap_off.tracks.insert(TrackId(3), off);
        let todo = delay_tap_followups(std::slice::from_ref(&t), &snap_off, &mut fresh_seen);
        assert_eq!(todo.len(), 1);
        assert_eq!(todo[0].2["tempoSrc"], 1.0);
        assert!((todo[0].2["tempo"].as_f64().unwrap() - 0.28).abs() < 1e-6);
        assert_eq!(model_switch(&t.mixer.inserts[0], "tempoSrc"), Some(Switch::Hide));
        strip.modules[0] = meter(1.0, 100.0);
        snap.tracks.insert(TrackId(3), strip.clone());
        let todo = delay_tap_followups(std::slice::from_ref(&t), &snap, &mut seen);
        assert_eq!(todo.len(), 1);
        let (track, module, params) = &todo[0];
        assert_eq!((*track, *module), (TrackId(3), ModuleId(7)));
        for (k, v) in params {
            t.mixer.inserts[0].params.insert(k.clone(), v.as_f64().unwrap() as f32);
        }
        let shown = model_switch(&t.mixer.inserts[0], "tempoSrc");
        assert_eq!(shown, Some(Switch::Show { label: "Tempo", opts: &["Host", "Manual"] }));
        assert!(delay_tap_followups(std::slice::from_ref(&t), &snap, &mut seen).is_empty(), "acted on once");
        // Host: the switch goes (and the engine is back on the session tempo; see the dsp tap test).
        t.mixer.inserts[0].params.insert("tempoSrc".into(), 0.0);
        assert_eq!(model_switch(&t.mixer.inserts[0], "tempoSrc"), Some(Switch::Hide));
    }

    fn fs_engines() -> Vec<&'static PluginInfo> {
        let fs: Vec<&'static PluginInfo> =
            soundcraft_dsp::plugins().iter().copied().filter(|p| p.category == soundcraft_dsp::Category::FireStation).collect();
        assert!(fs.len() >= 15 && fs.iter().any(|p| p.id == "fs_chamber"), "{}", fs.len());
        fs
    }

    #[test]
    fn chambers_empty_object_slots_are_offered_nowhere() {
        // ChamberMade hides a slot's position, size and rotate until something stands in it (and
        // Rotate until it is a panel): the generic window and the lane picker follow the drawer.
        let mut ins = Insert::new("fs_chamber");
        let info = soundcraft_dsp::plugin_info("fs_chamber").unwrap();
        use soundcraft_model::{ChannelFormat, Track, TrackId, TrackKind};
        let t = Track::new(TrackId(1), "T", TrackKind::Audio, ChannelFormat::Stereo, [1, 2, 3]);
        for id in ["o1x", "o1y", "o1s", "o1r"] {
            ins.params.insert("o1t".into(), 0.0);
            assert!(!drawer_shows(&ins, info.param(id).unwrap()), "{id}");
            assert!(!offered(&ins, id), "{id}");
            assert!(!crate::edit_window::lane_params(&t, &ins, info).iter().any(|p| p.id == id), "{id}");
        }
        ins.params.insert("o1t".into(), 1.0);
        assert!(offered(&ins, "o1x") && offered(&ins, "o1s"));
    }

    fn offered(ins: &Insert, id: &str) -> bool {
        let info = soundcraft_dsp::plugin_info(&ins.plugin).unwrap();
        window_params(ins, info).iter().any(|p| p.id == id)
    }

    #[test]
    fn no_engines_own_bypass_is_offered_anywhere() {
        for info in fs_engines() {
            let ins = Insert::new(info.id);
            assert!(info.param("bypass").is_some(), "{}", info.id);
            assert!(!offered(&ins, "bypass"), "{}", info.id);
            assert!(never_offered(info.id, "bypass"));
            assert!(!drawer_groups(info).is_empty());
            // Everything the window offers, the drawer shows (and the reverse, but for a group the
            // drawer has switched off).
            for p in info.params {
                assert_eq!(param_offered(&ins, p), drawer_shows(&ins, p) && group_disabled(info.id, p.group).is_none(), "{} {}", info.id, p.id);
            }
        }
        // A hosted plugin's own parameter called bypass is its own business.
        assert!(!never_offered("clap:com.example.thing", "bypass"));
        let any = soundcraft_dsp::plugins().iter().find_map(|i| i.params.first()).unwrap();
        let hosted = ParamInfo { id: "bypass", ..any.clone() };
        assert!(param_offered(&Insert::new("clap:com.example.thing"), &hosted));
    }

    #[test]
    fn the_generic_window_hides_what_the_drawer_hides() {
        // A model's unused switch: Sat's variant is shown, a Comp lane's variant per its style.
        let mut comp = Insert::new("fs_comp");
        comp.params.insert("style".into(), 0.0);
        let hidden = model_switch(&comp, "variant") == Some(Switch::Hide);
        assert_eq!(offered(&comp, "variant"), !hidden);
        comp.params.insert("style".into(), 1.0);
        assert!(offered(&comp, "variant"), "the 1176 style has a Revision switch");
        let p = soundcraft_dsp::plugin_info("fs_comp").unwrap().param("variant").unwrap();
        assert_eq!(choice_names(&comp, p), ["Blue stripe", "Blackface"]);
        // Mod's Tempo is inert (the session tempo always drives sync); Link and its set points
        // are offered (the derived-parameter layer runs it).
        let m = Insert::new("fs_mod");
        assert!(!offered(&m, "tempo"));
        for id in ["depth", "link", "linkLo", "linkHi"] {
            assert!(offered(&m, id), "{id}");
        }
    }

    #[test]
    fn cleanq_ceiling_is_offered_everywhere() {
        // With B3 the Ceiling group reopens in the drawer, the generic window and the lane picker.
        let cq = Insert::new("fs_cleanq");
        let info = soundcraft_dsp::plugin_info("fs_cleanq").unwrap();
        let ceiling: Vec<_> = info.params.iter().filter(|p| p.group == "Ceiling").map(|p| p.id).collect();
        assert_eq!(ceiling, ["ceilOn", "ceilThr", "ceilAmount", "ceilMaxCut", "ceilAttack", "ceilRelease", "ceilDelta"]);
        let window: Vec<&str> = window_params(&cq, info).iter().map(|p| p.id).collect();
        for id in &ceiling {
            assert!(offered(&cq, id), "{id}");
            assert!(drawer_shows(&cq, info.param(id).unwrap()), "{id}");
            assert!(window.contains(id), "{id}");
        }
        assert!(offered(&cq, "outGain"));
        assert!(!offered(&cq, "bypass"), "the engine's own bypass stays hidden");
    }

    #[test]
    fn a_tempo_source_off_host_is_offered_so_it_can_be_put_back() {
        // Every engine with a Tempo Source: hidden on Host, shown (with the drawer) on Manual.
        let mut with_src: Vec<_> = fs_engines().into_iter().filter(|i| i.param("tempoSrc").is_some()).map(|i| i.id).collect();
        with_src.sort_unstable();
        assert_eq!(with_src, ["fs_delay", "fs_filter", "fs_reverb", "fs_swale"]);
        for id in with_src {
            let info = soundcraft_dsp::plugin_info(id).unwrap();
            let src = info.param("tempoSrc").unwrap();
            let mut ins = Insert::new(id);
            assert!(!drawer_shows(&ins, src) && !offered(&ins, "tempoSrc"), "{id} on Host");
            ins.params.insert("tempoSrc".into(), 1.0);
            assert!(drawer_shows(&ins, src) && offered(&ins, "tempoSrc"), "{id} on Manual");
            assert_eq!(choice_names(&ins, src), ["Host", "Manual"], "{id}");
        }
        // Each shows its Manual BPM with it (the tempo in force).
        for (id, bpm) in [("fs_delay", "tempo"), ("fs_reverb", "tempo"), ("fs_filter", "bpm"), ("fs_swale", "bpm")] {
            let p = soundcraft_dsp::plugin_info(id).unwrap().param(bpm).unwrap();
            let mut ins = Insert::new(id);
            assert!(!drawer_shows(&ins, p) && !offered(&ins, bpm), "{id} {bpm} on Host");
            ins.params.insert("tempoSrc".into(), 1.0);
            assert!(drawer_shows(&ins, p) && offered(&ins, bpm), "{id} {bpm} on Manual");
        }
        // No other engine has a tempo of its own that could leave the session's: Pitch and Dist
        // have none, Mod's Tempo is only its fallback without a host tempo.
        for id in ["fs_pitch", "fs_dist", "fs_mod"] {
            let info = soundcraft_dsp::plugin_info(id).unwrap();
            assert!(info.param("tempoSrc").is_none(), "{id}");
        }
    }
}

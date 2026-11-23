//! The Remote Mix window's insert editors: a built-in effect on the studio opened from here.
//!
//! **Full editors** are the Mix window's own wide editors, drawn unchanged over a one-track
//! *mirror* of the remote module: a private engine holding the plug-in with the parameter values
//! the studio's feed carries. While the editor draws, the app's engine is that mirror (swapped in
//! and back around the call, nothing else runs meanwhile); whatever the editor changed in it is
//! then sent to the studio as `mix.module_params` (a drag as one gesture, one undo step there),
//! and values the feed brings are written into the mirror whenever no drag is under way here.
//! The mirror never reaches the player: its tracks are not this PC's session, and its ids are
//! far from any session's.
//!
//! **Generic editors** (a list of the parameters in plain units, with a slider each) stand in
//! where a full editor would mislead: editors whose display depends on module state or one-off
//! actions the feed does not carry (CleanQ, CopyQ, LevelUp, Chamber's impulse responses), the
//! analysers that need the studio's sound (Spectrum, Loudness Meter, Phase Meter, Tuner), plug-ins
//! with no wide editor, and editors not tried here yet. Third-party plug-ins get neither: name and
//! bypass only.

use crate::SoundApp;
use crate::net_ui::NetState;
use crate::theme::Tokens;
use serde_json::{Map, Value, json};
use soundcraft_engine::Engine;
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_net::studio::{Control, RemoteInsert, RemoteStrip};
use std::collections::BTreeMap;

/// The built-in editors drawn in full over a mirror (each checked to keep everything it shows
/// in the module's parameters).
pub const FULL_EDITORS: &[&str] = &[
    "fs_amp",
    "fs_comp",
    "fs_delay",
    "fs_dist",
    "fs_filter",
    "fs_manners",
    "fs_mod",
    "fs_pitch",
    "fs_reverb",
    "fs_sat",
    "fs_shale",
    "fs_sift",
    "fs_swale",
    "fs_tape",
    "fs_transient",
    "fs_twine",
    "heft",
    "keel",
    "lull",
    "rez",
    "sprawl",
];

/// Why a plug-in with a wide editor gets the generic list here instead.
pub fn generic_reason(plugin: &str) -> Option<&'static str> {
    match plugin {
        "fs_cleanq" | "fs_copyq" => Some("its editor keeps settings the studio does not send"),
        "fs_levelup" => Some("its editor runs actions on the studio's playing instance"),
        "fs_chamber" => Some("its impulse responses are files on the studio PC"),
        "spectrum" | "loudness" | "phase_meter" | "tuner" => Some("it shows the studio's sound, which is not sent here"),
        _ => None,
    }
}

/// How an insert opens from the Remote Mix window.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EditorKind {
    Full,
    Generic,
}

/// How `ins` opens here (`None`: a third-party plug-in or one this build does not know).
pub fn kind_of(ins: &RemoteInsert) -> Option<EditorKind> {
    if ins.hosted || soundcraft_dsp::plugin_info(&ins.plugin).is_none() {
        return None;
    }
    if FULL_EDITORS.contains(&ins.plugin.as_str()) && crate::editors::has_wide(&ins.plugin) {
        Some(EditorKind::Full)
    } else {
        Some(EditorKind::Generic)
    }
}

/// The mirror's ids start here: far from any session's, so nothing keyed by track or module id
/// (an open editor's own state, a meter) can be confused with this PC's.
const MIRROR_IDS: u64 = 1 << 40;

/// An editor open on a studio's insert.
pub struct RemoteEditor {
    pub strip: u64,
    pub module: u32,
    pub plugin: String,
    /// `<insert> on <strip>`.
    pub title: String,
    pub kind: EditorKind,
    /// The mirror (full editors) and its track and module.
    mirror: Option<(Engine, TrackId, ModuleId)>,
    /// What the editor shows now (the feed's values, and this PC's moves).
    shown: BTreeMap<String, f32>,
    bypass: bool,
    /// A drag in the editor is under way (its values are this PC's until the hand comes off).
    dragging: bool,
}

impl RemoteEditor {
    /// Opens `ins` of strip `s` (`None` when it does not open here).
    pub fn open(s: &RemoteStrip, ins: &RemoteInsert) -> Option<RemoteEditor> {
        let kind = kind_of(ins)?;
        let shown: BTreeMap<String, f32> = ins.params.iter().cloned().collect();
        let mirror = match kind {
            EditorKind::Full => Some(mirror_of(s, ins, &shown)?),
            EditorKind::Generic => None,
        };
        Some(RemoteEditor {
            strip: s.id,
            module: ins.module,
            plugin: ins.plugin.clone(),
            title: format!("{} on {}", ins.name, s.name),
            kind,
            mirror,
            shown,
            bypass: ins.bypass,
            dragging: false,
        })
    }

    /// The control a value of this editor is (its optimistic value and gesture).
    fn control(&self, param: &str) -> Control {
        Control::Param(self.strip, self.module, param.to_string())
    }

    /// The gesture key of a drag in this editor (one undo step on the studio).
    pub fn gesture(&self) -> String {
        format!("editor:{}:{}", self.strip, self.module)
    }

    /// Brings the feed's values in (not while this PC drags: its values stand until let go).
    fn take_feed(&mut self, ins: &RemoteInsert) {
        if self.dragging {
            return;
        }
        let mut changed = Map::new();
        for (k, v) in &ins.params {
            // The feed rounds to 0.01: a value this PC set more finely is not "changed" by it.
            if self.shown.get(k).is_none_or(|s| (s - v).abs() > 0.006) {
                self.shown.insert(k.clone(), *v);
                changed.insert(k.clone(), json!(v));
            }
        }
        self.bypass = ins.bypass;
        if let Some((e, t, m)) = &mut self.mirror {
            if !changed.is_empty() {
                let _ = e.execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": changed}));
            }
            if let Some(i) = e.session().track(*t).and_then(|tr| tr.mixer.module_index(*m))
                && e.session().track(*t).and_then(|tr| tr.mixer.inserts.get(i)).is_some_and(|x| x.bypass != ins.bypass)
            {
                let _ = e.execute("mix.insert_bypass", &json!({"track": t.0, "slot": i, "value": ins.bypass}));
            }
        }
    }
}

/// A one-track engine holding the plug-in of `ins` at the values `shown`.
fn mirror_of(s: &RemoteStrip, ins: &RemoteInsert, shown: &BTreeMap<String, f32>) -> Option<(Engine, TrackId, ModuleId)> {
    let mut e = Engine::default();
    e.session_mut().next_id = MIRROR_IDS;
    e.execute("track.new", &json!({"name": s.name, "kind": "audio"})).ok()?;
    let t = e.session().tracks.iter().find(|t| t.id.0 >= MIRROR_IDS)?.id;
    let r = e.execute("mix.insert", &json!({"track": t.0, "plugin": ins.plugin})).ok()?;
    let m = ModuleId(u32::try_from(r.get("module")?.as_u64()?).ok()?);
    let params: Map<String, Value> = shown.iter().map(|(k, v)| (k.clone(), json!(v))).collect();
    let _ = e.execute("mix.module_params", &json!({"track": t.0, "module": m.0, "params": params}));
    if ins.bypass {
        let _ = e.execute("mix.insert_bypass", &json!({"track": t.0, "slot": 0, "value": true}));
    }
    Some((e, t, m))
}

/// Opens the editor of insert `module` on strip `strip` (from its name in the strip).
pub(crate) fn open(net: &mut NetState, s: &RemoteStrip, ins: &RemoteInsert) {
    match RemoteEditor::open(s, ins) {
        Some(ed) => net.studio.editor = Some(ed),
        None => net.studio.editor = None,
    }
}

/// The open editor's window, over the Remote Mix window (once a frame).
pub(crate) fn window(app: &mut SoundApp, net: &mut NetState, ctx: &egui::Context, view: &soundcraft_net::studio::MixerView, connected: bool) {
    let Some(mut ed) = net.studio.editor.take() else { return };
    // Gone on the studio (removed, its track deleted): closed here.
    let Some(ins) = view.strip(ed.strip).and_then(|s| s.inserts.iter().find(|i| i.module == ed.module)).cloned() else { return };
    if ins.plugin != ed.plugin {
        return;
    }
    ed.take_feed(&ins);
    let before = ed.shown.clone();
    let before_bypass = ed.bypass;
    let t = Tokens::current();
    let mut open = true;
    let title = format!("{} (on {})", ed.title, net.studio.studio_name());
    egui::Window::new(title).id(egui::Id::new("remote-editor")).open(&mut open).resizable(true).default_size([980.0, 680.0]).show(ctx, |ui| {
        if !connected {
            ui.disable();
        }
        ui.label(egui::RichText::new("Changes go to the studio. Graphs and meters that need its sound stay still here.").small().color(t.text_dim));
        match ed.kind {
            EditorKind::Full => full(app, ui, &mut ed),
            EditorKind::Generic => generic(ui, &mut ed),
        }
    });
    if !open {
        return;
    }
    let down = ctx.input(|i| i.pointer.any_down());
    send_changes(net, &mut ed, &before, before_bypass, down);
    net.studio.editor = Some(ed);
}

/// The wide editor, drawn over the mirror (the app's engine for the length of the call).
fn full(app: &mut SoundApp, ui: &mut egui::Ui, ed: &mut RemoteEditor) {
    let Some((mut mirror, t, m)) = ed.mirror.take() else { return };
    std::mem::swap(&mut app.engine, &mut mirror);
    let track = app.engine.session_arc().track(t).cloned();
    if let Some(track) = track {
        egui::ScrollArea::both().auto_shrink([false, false]).show(ui, |ui| {
            let _ = crate::editors::drawer(app, ui, &track, m, None);
        });
    }
    std::mem::swap(&mut app.engine, &mut mirror);
    // What the editor set, as it is in the mirror now.
    if let Some(ins) = mirror.session().track(t).and_then(|tr| tr.module(m)) {
        for (k, v) in &ins.params {
            ed.shown.insert(k.clone(), *v);
        }
        ed.bypass = ins.bypass;
    }
    ed.mirror = Some((mirror, t, m));
}

/// The plain list: each parameter's name, a slider in its own units, its value as text.
fn generic(ui: &mut egui::Ui, ed: &mut RemoteEditor) {
    let Some(info) = soundcraft_dsp::plugin_info(&ed.plugin) else { return };
    let t = Tokens::current();
    if let Some(why) = generic_reason(&ed.plugin) {
        ui.label(egui::RichText::new(format!("Its parameters as a list: {why}.")).small().color(t.text_dim));
    }
    ui.checkbox(&mut ed.bypass, "Bypassed");
    egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        egui::Grid::new("remote-editor-params").num_columns(3).striped(true).spacing([10.0, 4.0]).show(ui, |ui| {
            for p in info.params {
                let Some(v) = ed.shown.get_mut(p.id) else { continue };
                ui.label(p.name);
                let mut x = *v;
                let r = match p.unit {
                    soundcraft_dsp::Unit::Toggle => {
                        let mut on = x > 0.5;
                        let r = ui.checkbox(&mut on, "");
                        x = if on { p.max } else { p.min };
                        r
                    }
                    soundcraft_dsp::Unit::Choice if !p.choices.is_empty() => {
                        let mut k = x.round().clamp(p.min, p.max) as usize;
                        let r = egui::ComboBox::from_id_salt(("remote-choice", p.id))
                            .selected_text(p.choices.get(k).copied().unwrap_or("?"))
                            .show_ui(ui, |ui| {
                                for (i, c) in p.choices.iter().enumerate() {
                                    ui.selectable_value(&mut k, i, *c);
                                }
                            })
                            .response;
                        x = k as f32;
                        r
                    }
                    _ => ui.add(
                        egui::Slider::new(&mut x, p.min..=p.max)
                            .show_value(false)
                            .logarithmic(matches!(p.unit, soundcraft_dsp::Unit::Hz) && p.min > 0.0),
                    ),
                };
                let r = r.on_hover_text(p.id);
                r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, p.name));
                *v = p.clamp(x);
                ui.label(egui::RichText::new(p.format(*v)).monospace());
                ui.end_row();
            }
        });
    });
}

/// What the editor changed this frame goes to the studio; the drag is one gesture until the
/// hand comes off.
fn send_changes(net: &mut NetState, ed: &mut RemoteEditor, before: &BTreeMap<String, f32>, before_bypass: bool, down: bool) {
    let changed: Map<String, Value> =
        ed.shown.iter().filter(|(k, v)| before.get(*k).is_none_or(|b| (*b - **v).abs() > f32::EPSILON)).map(|(k, v)| (k.clone(), json!(v))).collect();
    let gesture = ed.gesture();
    if !changed.is_empty() {
        ed.dragging = down;
        if let Some(link) = &net.studio.link {
            {
                let mut m = link.mirror();
                for (k, v) in &changed {
                    m.set(ed.control(k), v.as_f64().unwrap_or(0.0) as f32, down);
                }
            }
            link.execute("mix.module_params", json!({"track": ed.strip, "module": ed.module, "params": changed}), down.then_some(gesture.as_str()));
        } else if let Some(d) = &mut net.studio.demo
            && let Some(s) = d.view.strips.iter_mut().find(|s| s.id == ed.strip)
        {
            for (k, v) in &changed {
                soundcraft_net::studio::set_control(s, &ed.control(k), v.as_f64().unwrap_or(0.0) as f32);
            }
        }
    } else if ed.dragging && !down {
        // The hand came off: the studio closes the step; the feed shows what it has.
        ed.dragging = false;
        if let Some(link) = &net.studio.link {
            link.end_gesture(&gesture);
            let mut m = link.mirror();
            for k in ed.shown.keys() {
                m.release(&ed.control(k));
            }
        }
    }
    if ed.bypass != before_bypass {
        let on = ed.bypass;
        let slot = net.studio.link.as_ref().and_then(|l| {
            l.mirror().feed_view().and_then(|v| v.strip(ed.strip).and_then(|s| s.inserts.iter().find(|i| i.module == ed.module).map(|i| i.slot)))
        });
        if let (Some(link), Some(slot)) = (&net.studio.link, slot) {
            link.mirror().set(Control::Bypass(ed.strip, ed.module), if on { 1.0 } else { 0.0 }, false);
            link.execute("mix.insert_bypass", json!({"track": ed.strip, "slot": slot, "value": on}), None);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ins(plugin: &str, hosted: bool) -> RemoteInsert {
        RemoteInsert { slot: 0, module: 1, plugin: plugin.into(), name: plugin.into(), bypass: false, hosted, params: Vec::new() }
    }

    #[test]
    fn which_editor_each_insert_gets() {
        assert_eq!(kind_of(&ins("fs_comp", false)), Some(EditorKind::Full));
        assert_eq!(kind_of(&ins("fs_cleanq", false)), Some(EditorKind::Generic));
        assert_eq!(kind_of(&ins("spectrum", false)), Some(EditorKind::Generic));
        assert_eq!(kind_of(&ins("eq_7band", false)), Some(EditorKind::Generic));
        assert_eq!(kind_of(&ins("clap:com.acme.verb", true)), None, "third-party: name and bypass only");
        for p in FULL_EDITORS {
            assert!(crate::editors::has_wide(p), "{p} has a wide editor");
            assert!(generic_reason(p).is_none());
        }
    }

    #[test]
    fn a_mirror_holds_the_feeds_values_far_from_any_sessions_ids() {
        let info = soundcraft_dsp::plugin_info("fs_comp").unwrap();
        let p = info.params.iter().find(|p| p.max - p.min > 1.0).unwrap();
        let v = p.min + (p.max - p.min) * 0.25;
        let s = RemoteStrip { id: 7, name: "Vocal".into(), ..RemoteStrip::default() };
        let mut i = ins("fs_comp", false);
        i.params = vec![(p.id.to_string(), v)];
        let (e, t, m) = mirror_of(&s, &i, &i.params.iter().cloned().collect()).unwrap();
        assert!(t.0 >= MIRROR_IDS);
        let got = e.session().track(t).unwrap().module(m).unwrap().params.get(p.id).copied();
        assert!(got.is_some_and(|g| (g - v).abs() < 1e-4), "{got:?} vs {v}");
    }
}

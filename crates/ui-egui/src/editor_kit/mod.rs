//! The editor kit: what every VST-faithful plugin editor (`crate::editors`) is built from.
//!
//! - [`VstEditor`] and [`run`]: the wide-drawer frame. The editor is drawn at its VST editor's own
//!   size (`setSize`, or `requiredHeight()` where the VST has one) at the top of a column as tall
//!   as the strips; the space below is left blank, and when the Mix window is shorter than the
//!   editor the column scrolls vertically. Never stretched, never squashed.
//! - The command helpers: every gesture changes the session through a command (a drag or a wheel
//!   burst is one undo step).
//! - [`palette`], [`look`], [`controls`], [`cards`], [`icons`], [`presets`], [`family`]: the
//!   translation of FireStation's `Common/` (`Theme.h`, `EngineLook.h`, `Controls.h`,
//!   `PresetBrowser`, `PresetManager`) and the CleanQ pilot's generic pieces.

pub mod cards;
pub mod controls;
pub mod crossover;
pub mod family;
pub mod icons;
pub mod link_cal;
pub mod look;
pub mod palette;
pub mod presets;
pub mod slim;

pub use palette::{VstPalette, argb, rgb};

use crate::SoundApp;
use egui::{Id, Rect, Sense, Ui, Vec2, vec2};
use serde_json::{Map, Value, json};
use soundcraft_dsp::PluginInfo;
use soundcraft_dsp::analysis::LiveValues;
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, Track, TrackId};

/// Which keys an editor takes from the DAW while the pointer is over it (`keyPressed`).
pub type KeyOwner = fn(egui::Key, egui::Modifiers) -> bool;

/// A plugin's VST editor, translated.
///
/// State that is purely visual (selection, open cards, display range, hover) lives in the
/// implementing struct, which [`run`] keeps in egui memory per track and module; anything the VST
/// persists goes into the module's parameters or state through commands.
pub(crate) trait VstEditor: Clone + Default + Send + Sync + 'static {
    /// The plugin id (`fs_cleanq`).
    const PLUGIN: &'static str;
    /// `setSize`: the default width, and `requiredHeight()` for this state where the VST has one.
    fn size(&self, ins: &Insert) -> Vec2;
    /// Paints the editor into `host.rect` and handles its gestures.
    fn ui(&mut self, app: &mut SoundApp, ui: &mut Ui, host: &Host<'_>);
    /// `keyPressed`: the keys this editor takes from the DAW while hovered.
    fn owns_key(_key: egui::Key, _mods: egui::Modifiers) -> bool {
        false
    }
    /// The editor's constructor: what it reads from the module when it opens (its `uiState`).
    fn opened(&mut self, _ins: &Insert) {}
}

/// What an editor needs to know about where it is drawn.
pub(crate) struct Host<'a> {
    pub track: TrackId,
    pub ins: &'a Insert,
    pub info: &'static PluginInfo,
    pub meters: Option<&'a StripMeter>,
    /// The session's sample rate (48 kHz when it is missing or nonsense).
    pub sr: f64,
    /// The editor's own rectangle, exactly its `size`.
    pub rect: Rect,
    /// The session as this frame draws it (track names, Scout's shown guides).
    pub session: std::sync::Arc<soundcraft_model::Session>,
}

impl Host<'_> {
    /// A merge key unique to this module and `gesture`.
    pub fn key(&self, gesture: &str) -> String {
        format!("vst:{}:{}:{gesture}", self.track.0, self.ins.id.0)
    }
}

/// The smallest and largest editor the frame accepts (a hostile `size` is clamped).
const MIN_SIDE: f32 = 64.0;
const MAX_SIDE: f32 = 4096.0;

fn sane_size(v: Vec2) -> Vec2 {
    let s = |x: f32, d: f32| if x.is_finite() { x.clamp(MIN_SIDE, MAX_SIDE) } else { d };
    vec2(s(v.x, 800.0), s(v.y, 500.0))
}

fn frame_key(track: TrackId, module: ModuleId) -> Id {
    Id::new(("vst-editor-frame", track.0, module.0))
}

/// The wide-drawer frame around editor `E` for `module` on `track` (the unit tests'; every editor
/// draws through [`run_with`]).
#[cfg(test)]
pub(crate) fn run<E: VstEditor>(app: &mut SoundApp, ui: &mut Ui, track: &Track, module: ModuleId, meters: Option<&StripMeter>) {
    run_with::<E>(app, ui, track, module, meters, |_, _| {});
}

/// The wide-drawer frame around editor `E` for `module` on `track`, with `prep` given the editor
/// (and the app) before it is sized and drawn: a DAW-side setting the editor follows (the slim
/// layout).
pub(crate) fn run_with<E: VstEditor>(
    app: &mut SoundApp,
    ui: &mut Ui,
    track: &Track,
    module: ModuleId,
    meters: Option<&StripMeter>,
    prep: impl FnOnce(&mut E, &SoundApp),
) {
    let Some(ins) = track.mixer.module(module) else { return };
    let Some(info) = soundcraft_dsp::plugin_info(&ins.plugin) else { return };
    let key = Id::new(("vst-editor", E::PLUGIN, track.id.0, module.0));
    let mut ed: Box<E> = app.editor_states.remove(&key).and_then(|b| b.downcast::<E>().ok()).unwrap_or_else(|| {
        let mut e = Box::<E>::default();
        e.opened(ins);
        e
    });
    prep(&mut ed, app);
    let size = sane_size(ed.size(ins));

    // The column: the editor's width, as tall as the strips (never less than a sliver).
    // What is visible of it: the Mix window may be shorter than the strips' layout height.
    let visible = ui.clip_rect().bottom() - ui.cursor().top();
    let col_h = ui.available_height().min(visible).max(MIN_SIDE);
    let (col, _) = ui.allocate_exact_size(vec2(size.x, col_h), Sense::hover());
    // Top-down inside (the mixer lays its strips out left to right).
    let down = egui::Layout::top_down(egui::Align::Min);
    let mut cui = ui.new_child(egui::UiBuilder::new().id_salt(("vst-col", track.id.0, module.0)).max_rect(col).layout(down));
    cui.set_clip_rect(col.intersect(ui.clip_rect()));
    let sr = {
        let s = app.engine.session().sample_rate.as_f64();
        if s.is_finite() && s >= 8000.0 { s } else { 48_000.0 }
    };
    let mut drawn = None;
    egui::ScrollArea::vertical().id_salt(("vst-scroll", track.id.0, module.0)).auto_shrink([false, false]).max_height(col_h).show(&mut cui, |sui| {
        sui.spacing_mut().item_spacing = Vec2::ZERO;
        routing_strip(app, sui, track, ins, info, size.x);
        let (rect, _) = sui.allocate_exact_size(size, Sense::hover());
        let mut eui = sui.new_child(egui::UiBuilder::new().id_salt(("vst-ed", E::PLUGIN, track.id.0, module.0)).max_rect(rect).layout(down));
        eui.set_clip_rect(rect.intersect(sui.clip_rect()));
        eui.spacing_mut().item_spacing = Vec2::ZERO;
        if eui.rect_contains_pointer(rect) {
            app.ui.wide_key_owner.insert(eui.ctx().viewport_id(), E::owns_key);
        }
        let host = Host { track: track.id, ins, info, meters, sr, rect, session: app.engine.session_arc() };
        ed.ui(app, &mut eui, &host);
        drawn = Some(rect);
    });
    if let Some(rect) = drawn {
        let pass = ui.ctx().cumulative_pass_nr();
        ui.data_mut(|d| d.insert_temp(frame_key(track.id, module), (rect, pass)));
    }
    app.editor_states.insert(key, ed);
}

/// The narrow drawer's routing rows above the VST face, in the DAW's look (they are the DAW's,
/// not the plugin's): the sidechain picker and the LINK row on the left, the reference rows on
/// the right (stacked under them when the editor is narrow). Nothing when the module has none.
/// Allocates the strip's height in `ui` and returns it.
fn routing_strip(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert, info: &'static PluginInfo, width: f32) -> f32 {
    use crate::{link_picker, sidechain_picker};
    let side = sidechain_picker::shows_picker(info, ins);
    let link = link_picker::shows_link(info, ins);
    let refs = info.references > 0;
    if !(side || link || refs) {
        return 0.0;
    }
    const PAD: f32 = 8.0;
    const GAP: f32 = 6.0;
    const COL_MAX: f32 = 420.0;
    let t = crate::theme::Tokens::current();
    let top = ui.cursor().min;
    let two = (side || link) && refs && width >= 2.0 * 260.0 + 3.0 * PAD;
    let col_w = if two { ((width - 3.0 * PAD) / 2.0).min(COL_MAX) } else { (width - 2.0 * PAD).min(COL_MAX) }.max(120.0);
    let mut cui =
        ui.new_child(egui::UiBuilder::new().id_salt(("vst-routing", track.id.0, ins.id.0)).max_rect(Rect::from_min_size(top, vec2(width, MAX_SIDE))));
    // The background goes under the rows, sized once they are laid out.
    let bg = cui.painter().add(egui::Shape::Noop);
    let left = Rect::from_min_size(top + vec2(PAD, 0.0), vec2(col_w, MAX_SIDE));
    let mut y = top.y + PAD - GAP;
    if side {
        y = sidechain_picker::row(app, &mut cui, track, ins, left, y + GAP);
    }
    if link {
        y = link_picker::link_row(app, &mut cui, track, ins, info, left, y + GAP);
    }
    let mut end = y;
    if refs {
        let (col, start) = if two && (side || link) { (left.translate(vec2(col_w + PAD, 0.0)), top.y + PAD - GAP) } else { (left, y) };
        // `reference_rows` leaves 4 points under each row.
        end = end.max(link_picker::reference_rows(app, &mut cui, track, ins, info, col, start + GAP) - 4.0);
    }
    let h = (end + PAD - top.y).max(0.0);
    let strip = Rect::from_min_size(top, vec2(width, h));
    cui.painter().set(bg, egui::Shape::rect_filled(strip, 0.0, t.strip_bg));
    cui.painter().hline(strip.x_range(), strip.max.y - 0.5, egui::Stroke::new(1.0, t.border));
    ui.allocate_exact_size(vec2(width, h), Sense::hover());
    h
}

/// The editor's rectangle as last drawn (`None` if it was not drawn last frame, e.g. the narrow
/// drawer is showing). For tests and scripts.
pub fn frame_rect(ctx: &egui::Context, track: TrackId, module: ModuleId) -> Option<Rect> {
    let (rect, pass) = ctx.data(|d| d.get_temp::<(Rect, u64)>(frame_key(track, module)))?;
    (pass + 1 >= ctx.cumulative_pass_nr()).then_some(rect)
}

// ---------------------------------------------------------------------------- commands

fn report(app: &mut SoundApp, r: Result<(), String>) {
    if let Err(e) = r {
        app.ui.status = e;
    }
}

fn params_json(host: &Host, list: &[(&str, f32)], preset: Option<&str>) -> Value {
    let map: Map<String, Value> = list.iter().map(|(k, v)| ((*k).to_string(), json!(v))).collect();
    let mut p = json!({"track": host.track.0, "module": host.ins.id.0, "params": map});
    if let (Some(name), Some(o)) = (preset, p.as_object_mut()) {
        o.insert("preset".into(), json!(name));
    }
    p
}

/// `mix.module_params`, one undo step.
pub(crate) fn set_params(app: &mut SoundApp, host: &Host, list: &[(&str, f32)]) {
    if list.is_empty() {
        return;
    }
    let r = app.run("mix.module_params", params_json(host, list, None)).map(|_| ());
    report(app, r);
}

/// `mix.module_params` merged into the open `gesture` (a drag or a wheel burst): one undo step
/// until [`end_gesture`].
pub(crate) fn set_params_merged(app: &mut SoundApp, host: &Host, list: &[(&str, f32)], gesture: &str) {
    if list.is_empty() {
        return;
    }
    let r = app.engine.execute_merged("mix.module_params", &params_json(host, list, None), &host.key(gesture)).map(|_| ()).map_err(|e| e.to_string());
    report(app, r);
}

/// Ends the open gesture: the next change is a new undo step.
pub(crate) fn end_gesture(app: &mut SoundApp) {
    app.engine.end_merge();
}

/// The editor changed the module's stored state: `mix.module_state_edit` with `bytes`, one undo
/// step.
pub(crate) fn set_state(app: &mut SoundApp, host: &Host, bytes: &[u8]) {
    let b64 = soundcraft_model::b64::encode(bytes);
    let r = app.run("mix.module_state_edit", json!({"track": host.track.0, "module": host.ins.id.0, "state": b64})).map(|_| ());
    report(app, r);
}

/// `mix.module_state_edit` merged into `gesture`.
pub(crate) fn set_state_merged(app: &mut SoundApp, host: &Host, bytes: &[u8], gesture: &str) {
    let b64 = soundcraft_model::b64::encode(bytes);
    let p = json!({"track": host.track.0, "module": host.ins.id.0, "state": b64});
    let r = app.engine.execute_merged("mix.module_state_edit", &p, &host.key(gesture)).map(|_| ()).map_err(|e| e.to_string());
    report(app, r);
}

/// Several changes as ONE undo step: `body` runs with a fresh merge `gesture` open (use the
/// `*_merged` helpers with it inside), which is closed after.
pub(crate) fn one_step(app: &mut SoundApp, body: impl FnOnce(&mut SoundApp)) {
    app.engine.end_merge();
    body(app);
    app.engine.end_merge();
}

/// Recalls a preset: `params` and its name through `mix.module_params {preset}`, merged into
/// `gesture` (so a state change that goes with it joins the same undo step).
pub(crate) fn recall_preset_merged(app: &mut SoundApp, host: &Host, name: &str, params: &Map<String, Value>, gesture: &str) {
    let p = json!({"track": host.track.0, "module": host.ins.id.0, "params": params, "preset": name});
    let r = app.engine.execute_merged("mix.module_params", &p, &host.key(gesture)).map(|_| ()).map_err(|e| e.to_string());
    report(app, r);
    // `setCurrentPreset (name, false)`: the loaded preset starts clean.
    presets::set_dirty(app, host, false);
}

/// The module's `uiState` (what the VST keeps beside its parameters and saves with the session,
/// outside the undo history: a panel left open, a display span): a JSON object in the module's
/// stored state. Empty when there is none or it is not JSON.
pub(crate) fn ui_state(ins: &Insert) -> Map<String, Value> {
    ins.state_bytes().and_then(|b| serde_json::from_slice::<Value>(&b).ok()).and_then(|v| v.as_object().cloned()).unwrap_or_default()
}

/// `uiState.setProperty`: `entries` merged into the module's `uiState` through
/// `mix.module_state` (not undoable, as in the C++).
pub(crate) fn set_ui_state(app: &mut SoundApp, host: &Host, entries: &[(&str, Value)]) {
    let live = app.engine.session().track(host.track).and_then(|t| t.mixer.module(host.ins.id)).map(ui_state);
    let mut m = live.unwrap_or_else(|| ui_state(host.ins));
    for (k, v) in entries {
        m.insert((*k).to_string(), v.clone());
    }
    let bytes = Value::Object(m).to_string();
    let p = json!({"track": host.track.0, "module": host.ins.id.0, "state": soundcraft_model::b64::encode(bytes.as_bytes())});
    let r = app.run("mix.module_state", p).map(|_| ());
    report(app, r);
}

/// `mix.module_action {action: name}`.
pub(crate) fn action(app: &mut SoundApp, host: &Host, name: &str) {
    let r = app.run("mix.module_action", json!({"track": host.track.0, "module": host.ins.id.0, "action": name})).map(|_| ());
    report(app, r);
}

/// The module's live values ([`soundcraft_dsp::analysis::Analysis::live`]) while it plays.
pub(crate) fn live<'a>(host: &Host<'a>) -> Option<&'a LiveValues> {
    let m = host.meters?;
    m.analysis.iter().find(|(id, _)| *id == host.ins.id).map(|(_, a)| &a.live).filter(|l| !l.is_empty())
}

/// The module's eight engine readouts for the last block (`Plugin::readouts`; NaN where it wrote
/// nothing), while it plays.
pub(crate) fn readouts(host: &Host) -> Option<[f32; 8]> {
    host.meters?.modules.iter().find(|m| m.module == host.ins.id).map(|m| m.readouts)
}

/// A parameter's value as the plugin would clamp it (its default when missing).
pub(crate) fn param(host: &Host, id: &str) -> f32 {
    host.info.params.iter().find(|p| p.id == id).map_or(0.0, |p| p.clamp(host.ins.params.get(id).copied().unwrap_or(p.default)))
}

// ---------------------------------------------------------------------------- the wheel

/// juce_Windowing_windows.cpp: a notch is `WHEEL_DELTA` (120) and `deltaY = amount / 256.0f`.
pub const JUCE_NOTCH_DELTA_Y: f32 = 120.0 / 256.0;

/// JUCE's `MouseWheelDetails::deltaY` for this frame's wheel events (positive: wheel up, as egui).
/// Line units are notches; point units (smooth scrolling, trackpads) are
/// `widgets::KNOB_NOTCH_PX` points to a notch. 0 in the Mix window's strips and drawers, where the
/// wheel only scrolls and the editors' knobs, sliders, fields and graphs leave it alone
/// ([`crate::widgets::wheel_scrolls`], spec 2026-11-02 item 3).
pub(crate) fn juce_wheel_delta_y(ui: &Ui) -> f32 {
    if crate::widgets::wheel_scrolls(ui.ctx()) { 0.0 } else { juce_wheel_delta_y_any(ui) }
}

fn juce_wheel_delta_y_any(ui: &Ui) -> f32 {
    ui.input(|i| {
        i.events
            .iter()
            .map(|e| match e {
                egui::Event::MouseWheel { unit, delta, .. } => match unit {
                    egui::MouseWheelUnit::Line => delta.y * JUCE_NOTCH_DELTA_Y,
                    egui::MouseWheelUnit::Page => delta.y * 3.0 * JUCE_NOTCH_DELTA_Y,
                    egui::MouseWheelUnit::Point => delta.y / crate::widgets::KNOB_NOTCH_PX * JUCE_NOTCH_DELTA_Y,
                },
                _ => 0.0,
            })
            .sum::<f32>()
    })
}

/// Takes this frame's wheel away from scroll areas around the editor (the control used it).
pub(crate) fn consume_wheel(ui: &Ui) {
    ui.input_mut(|i| i.smooth_scroll_delta = Vec2::ZERO);
}

#[cfg(test)]
mod tests;

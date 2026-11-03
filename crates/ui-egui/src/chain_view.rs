//! The channel chain of a Mix window strip: one tile per module, top to bottom, then a "+" row.
//!
//! A built-in module's tile shows an on/off LED, its name (and preset, in wide strips), its live
//! graph and, in wide strips, its key knobs. A third-party plugin's tile shows its name, LED and
//! in/out levels; a click opens the plugin's own window. Tiles are dragged by their header:
//! within the strip they move, onto another strip they move (Alt: copy). The title opens the
//! module's drawer; right-click has Bypass, Remove, Duplicate, Presets, Knobs and Replace.

use crate::SoundApp;
use crate::key_knobs::key_knobs_for;
use crate::mix_window::{Pick, module_picker, plugin_info};
use crate::module_graphs::{
    GraphKind, LiveValues, Streams, cleanq_band_drag, draw_graph, eq_band_drag, graph_kind, module_key, module_streams, uses_streams,
};
use crate::theme::{Tokens, bold, regular};
use crate::widgets::{KnobStyle, knob};
use egui::{Align2, Color32, CornerRadius, CursorIcon, Id, Rect, Sense, Shape, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_dsp::PluginInfo;
use soundcraft_mix::{ModuleMeter, SlotKey, StripMeter};
use soundcraft_model::{Insert, ModuleId, Track, TrackId, TrackKind};

/// Inner margin of a tile.
const PAD: f32 = 4.0;
/// The header row: LED, name, preset.
const HEAD: f32 = 14.0;
/// Space between tiles (also where the thin "+" appears).
const GAP: f32 = 4.0;
/// Space between a tile's rows.
const ROW_GAP: f32 = 3.0;
const GRAPH_WIDE: f32 = 46.0;
const GRAPH_SLIM: f32 = 18.0;
/// A FireStation engine's graph is its face: a little taller.
const GRAPH_FS_WIDE: f32 = 56.0;
const GRAPH_FS_SLIM: f32 = 22.0;
/// Key knobs and their names, wide strips only.
const KNOB_ROW: f32 = 40.0;
/// A third-party tile's in/out level bars.
const LEVELS_WIDE: f32 = 9.0;
const LEVELS_SLIM: f32 = 5.0;
/// The "+" row under the chain.
const PLUS_H: f32 = 18.0;
const MAX_KNOBS: usize = 4;

/// The module meter as the graphs read it.
pub fn live_values(m: &ModuleMeter) -> LiveValues {
    let finite = |v: f32| if v.is_finite() { v } else { 0.0 };
    LiveValues { in_peak: finite(m.in_peak), out_peak: finite(m.out_peak), gr_db: finite(m.gr_db), readouts: m.readouts }
}

/// What a tile shows for a module.
#[derive(Clone, Copy)]
enum Face {
    BuiltIn(&'static PluginInfo),
    Hosted(&'static PluginInfo),
    /// A plugin this machine doesn't have (or can't load).
    Missing,
}

fn face(ins: &Insert) -> Face {
    if soundcraft_mix::is_third_party(&ins.plugin) {
        plugin_info(&ins.plugin).map_or(Face::Missing, Face::Hosted)
    } else {
        soundcraft_dsp::plugin_info(&ins.plugin).map_or(Face::Missing, Face::BuiltIn)
    }
}

/// A module's name on its tile: the plugin's short name, or for a missing one the end of its id.
pub(crate) fn tile_name(ins: &Insert) -> String {
    match face(ins) {
        Face::BuiltIn(i) | Face::Hosted(i) => i.short_name.to_string(),
        Face::Missing => id_tail(&ins.plugin).chars().take(16).collect(),
    }
}

/// The title a tile paints: a wide tile has room for the plugin's full name ("Transient"), a slim
/// one its short name ("Transnt"), the one the tile's controls are labelled by either way.
pub(crate) fn tile_title(ins: &Insert, wide: bool) -> String {
    match face(ins) {
        Face::BuiltIn(i) | Face::Hosted(i) if wide => i.name.to_string(),
        _ => tile_name(ins),
    }
}

/// How much of a wide tile's header a full name may take before the tile falls back to the short
/// one, leaving the rest to the preset or model name.
const TITLE_SHARE: f32 = 0.55;

/// The parameter ids a tile shows as knobs: the pinned ones, else the plugin's defaults.
fn key_knobs(ins: &Insert, info: &'static PluginInfo) -> Vec<&'static soundcraft_dsp::ParamInfo> {
    let ids: Vec<String> = if ins.knobs.is_empty() { key_knobs_for(ins, info).into_iter().map(str::to_string).collect() } else { ins.knobs.clone() };
    ids.iter().filter_map(|id| info.params.iter().find(|p| p.id == id.as_str())).take(MAX_KNOBS).collect()
}

/// The height of a built-in module's graph on its tile.
fn graph_height(info: &PluginInfo, wide: bool) -> f32 {
    match (info.category == soundcraft_dsp::Category::FireStation, wide) {
        (true, true) => GRAPH_FS_WIDE,
        (true, false) => GRAPH_FS_SLIM,
        (false, true) => GRAPH_WIDE,
        (false, false) => GRAPH_SLIM,
    }
}

/// What a FireStation tile names after the engine when no preset is loaded: its current style,
/// circuit or machine (the parameter Station titles the module with).
///
/// From the engine's `EngineUi`: its `model_name` (Amp's short amp name), else the text of its
/// `model_param`.
pub fn model_name(ins: &Insert, info: &PluginInfo) -> Option<String> {
    let ui = crate::engine_ui::engine_ui(info.id)?;
    if let Some(f) = ui.model_name {
        return f(ins);
    }
    let id = ui.model_param?;
    let p = info.param(id)?;
    Some(p.format(ins.params.get(id).copied().unwrap_or(p.default)))
}

/// The streams of a module whose graph reads them, from the strip's meters.
pub(crate) fn streams_for(ui: &Ui, track: TrackId, ins: &Insert, meters: Option<&StripMeter>) -> Option<std::sync::Arc<Streams>> {
    let kind = graph_kind(&ins.plugin);
    if !uses_streams(kind) {
        return None;
    }
    let a = meters?.analysis.iter().find(|(m, _)| *m == ins.id).map(|(_, a)| a)?;
    Some(module_streams(ui.ctx(), Id::new(("module-streams", track.0, ins.id.0)), a, kind))
}

fn tile_height(ins: &Insert, wide: bool) -> f32 {
    let body = match face(ins) {
        Face::BuiltIn(info) => {
            let graph = graph_height(info, wide);
            let knobs = if wide && !key_knobs(ins, info).is_empty() { ROW_GAP + KNOB_ROW } else { 0.0 };
            ROW_GAP + graph + knobs
        }
        Face::Hosted(_) => ROW_GAP + if wide { LEVELS_WIDE } else { LEVELS_SLIM },
        Face::Missing => 0.0,
    };
    PAD + HEAD + body + PAD
}

/// Height of the whole chain as drawn: every tile, the gaps and the "+" row.
pub(crate) fn chain_height(track: &Track, wide: bool) -> f32 {
    track.mixer.inserts.iter().map(|i| tile_height(i, wide) + GAP).sum::<f32>() + PLUS_H
}

/// A tile being dragged: which module of which track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ChainDrag {
    track: TrackId,
    module: ModuleId,
}

/// What dropping a dragged module does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropAction {
    /// Within its own strip: to the gap `gap` (0 = before the first module).
    Move { track: TrackId, module: ModuleId, gap: usize },
    /// Onto another strip with Alt held: a copy lands at `at`.
    Copy { from: TrackId, module: ModuleId, to: TrackId, at: usize },
    /// Onto another strip: moved to `at` with its automation lanes, as one undo step.
    MoveAcross { from: TrackId, module: ModuleId, to: TrackId, at: usize },
}

/// The action for dropping module `src` into gap `dst` of a chain (`alt`: Alt/Option held).
pub fn drop_action(src: (TrackId, ModuleId), dst: (TrackId, usize), alt: bool) -> DropAction {
    let ((from, module), (to, at)) = (src, dst);
    if from == to {
        DropAction::Move { track: from, module, gap: at }
    } else if alt {
        DropAction::Copy { from, module, to, at }
    } else {
        DropAction::MoveAcross { from, module, to, at }
    }
}

/// Runs a drop. A move to where the module already is does nothing.
pub fn apply_drop(app: &mut SoundApp, action: DropAction) {
    let r = match action {
        DropAction::Move { track, module, gap } => {
            let Some(from) = app.engine.session().track(track).and_then(|t| t.mixer.module_index(module)) else { return };
            let to = if gap > from { gap - 1 } else { gap };
            if to == from {
                return;
            }
            app.run("mix.module_move", json!({"track": track.0, "module": module.0, "to": to}))
        }
        DropAction::Copy { from, module, to, at } => {
            app.run("mix.module_copy", json!({"track": from.0, "module": module.0, "to_track": to.0, "at": at}))
        }
        DropAction::MoveAcross { from, module, to, at } => {
            app.run("mix.module_move", json!({"track": from.0, "module": module.0, "to_track": to.0, "to": at}))
        }
    };
    if let Err(e) = r {
        app.ui.status = e;
    }
}

/// Opens a module: a third-party plugin's own editor (else its plugin window), a built-in's
/// drawer in the Mix window.
pub(crate) fn open_module(app: &mut SoundApp, track: TrackId, module: ModuleId) {
    let Some(ins) = app.engine.session().track(track).and_then(|t| t.mixer.module(module)).cloned() else { return };
    if soundcraft_mix::is_third_party(&ins.plugin) {
        open_hosted(app, track, SlotKey::Module(module));
    } else {
        app.module_focus = Some((track, SlotKey::Module(module)));
        app.ui.drawers.insert(track, module);
    }
}

/// The plugin id in slot `key` of `track` (its instrument or a chain module), if there is one.
pub(crate) fn slot_plugin(app: &SoundApp, track: TrackId, key: SlotKey) -> Option<String> {
    let t = app.engine.session().track(track)?;
    let ins = match key {
        SlotKey::Instrument => t.instrument.as_ref(),
        SlotKey::Module(m) => t.mixer.module(m),
    };
    ins.map(|i| i.plugin.clone())
}

/// A plugin's name for menus and the status line: its own, else the end of its id (a hosted
/// plugin not described yet, or not installed).
pub(crate) fn plugin_name(plugin: &str) -> String {
    match plugin_info(plugin) {
        Some(i) => i.name.to_string(),
        None => id_tail(plugin).to_string(),
    }
}

/// The end of a plugin id ("missing-thing" of "clap:com.acme.missing-thing"): the name of a
/// plugin with no description.
fn id_tail(plugin: &str) -> &str {
    plugin.rsplit([':', '.', '/']).find(|s| !s.is_empty()).unwrap_or(plugin)
}

/// Opens a hosted plugin, instrument or insert alike: its own editor when it has one, else (no
/// editor, or opening it failed) its generic parameter window.
pub(crate) fn open_hosted(app: &mut SoundApp, track: TrackId, key: SlotKey) {
    let Some(plugin) = slot_plugin(app, track, key) else { return };
    app.module_focus = Some((track, key));
    if let Some(p) = &app.player
        && p.has_editor(track, key)
    {
        match p.open_editor(track, key) {
            Ok(()) => return,
            Err(e) => app.ui.status = format!("{}: {e}", plugin_name(&plugin)),
        }
    }
    show_parameters(app, track, key);
}

/// Shows a slot's generic plugin window (every parameter as a slider).
pub(crate) fn show_parameters(app: &mut SoundApp, track: TrackId, key: SlotKey) {
    app.module_focus = Some((track, key));
    if !app.ui.plugin_windows.contains(&(track, key)) {
        app.ui.plugin_windows.push((track, key));
    }
}

/// Shows a module's plugin window (every parameter as a slider).
pub(crate) fn open_window(app: &mut SoundApp, track: TrackId, module: ModuleId) {
    show_parameters(app, track, SlotKey::Module(module));
}

/// A hosted plugin's menu items for its slot: "Show Parameters…" (the generic window) and
/// "Capture Thumbnail Now" (enabled while its own editor is open).
pub(crate) fn hosted_menu_items(app: &mut SoundApp, ui: &mut Ui, track: TrackId, key: SlotKey) {
    if ui.button("Show Parameters…").clicked() {
        show_parameters(app, track, key);
        ui.close();
    }
    let can = crate::browser::thumbs::can_capture_now(app, (track, key));
    let resp = ui.add_enabled(can, egui::Button::new("Capture Thumbnail Now"));
    let resp = resp
        .on_hover_text("Take the plugin browser's picture of this plugin from its open editor, now")
        .on_disabled_hover_text("Open the plugin's editor first");
    if resp.clicked() {
        crate::browser::thumbs::capture_now(app, (track, key));
        ui.close();
    }
}

/// Opens or closes a module's drawer (one per strip).
fn toggle_drawer(app: &mut SoundApp, track: TrackId, module: ModuleId) {
    if app.ui.drawers.get(&track) == Some(&module) {
        app.ui.drawers.remove(&track);
    } else {
        app.ui.drawers.insert(track, module);
        app.module_focus = Some((track, SlotKey::Module(module)));
    }
}

/// The chain of `track`: its tiles, top to bottom, then the "+" row, in the width of `ui`.
/// A frozen track's chain (Track › Freeze): how many modules, from the top, its render holds, and
/// whether that is the whole chain (nothing can be added to it then).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct Frozen {
    pub modules: usize,
    pub whole: bool,
}

impl Frozen {
    /// The track's frozen chain, while it is frozen.
    pub(crate) fn of(track: &Track) -> Option<Frozen> {
        let f = track.frozen_audio()?;
        Some(Frozen { modules: track.frozen_modules(), whole: f.upto.is_none() })
    }

    /// Chain module `index` is frozen: read-only until the track is unfrozen.
    pub(crate) fn holds(self, index: usize) -> bool {
        self.whole || index < self.modules
    }

    /// A module may be inserted at chain position `at`: after the freeze point.
    pub(crate) fn may_insert(self, at: usize) -> bool {
        !self.whole && at >= self.modules
    }
}

/// Whether chain module `index` of `track` is frozen.
pub(crate) fn module_frozen(track: &Track, index: usize) -> bool {
    Frozen::of(track).is_some_and(|f| f.holds(index))
}

/// Whether a track's menus offer Freeze: audio, MIDI and instrument tracks (the items are greyed,
/// saying why, on one that can't be frozen, such as an instrument track with no instrument).
pub(crate) fn offers_freeze(track: &Track) -> bool {
    matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi)
}

/// The Freeze items of a module's menus (its tile, its drawer header): freeze the chain down to
/// and including this module, freeze it all, or unfreeze. Audio, MIDI and instrument tracks.
pub(crate) fn freeze_items(app: &mut SoundApp, ui: &mut Ui, track: &Track, ins: &Insert) {
    if !offers_freeze(track) {
        return;
    }
    let fz = track.frozen_audio().copied();
    let refusal = soundcraft_engine::cmd::cannot_freeze(track);
    let can = refusal.is_none();
    let id = track.id.0;
    let here = fz.is_some_and(|f| f.upto == Some(ins.id));
    let whole = fz.is_some_and(|f| f.upto.is_none());
    let run = |app: &mut SoundApp, ui: &Ui, cmd: &str, p: serde_json::Value| {
        if let Err(e) = app.run(cmd, p) {
            app.ui.status = e;
        }
        ui.close();
    };
    let up = ui.add_enabled(can && !here, egui::Button::new("Freeze up to here"));
    let up = match &refusal {
        Some(why) => up.on_disabled_hover_text(why.as_str()),
        None => up.on_hover_text("Render the chain down to and including this module; the modules below it stay live"),
    };
    if up.clicked() {
        run(app, ui, "track.freeze", json!({"tracks": [id], "upto": ins.id.0}));
    }
    let all = ui.add_enabled(can && !whole, egui::Button::new("Freeze whole chain"));
    let all = match &refusal {
        Some(why) => all.on_disabled_hover_text(why.as_str()),
        None => all,
    };
    if all.clicked() {
        run(app, ui, "track.freeze", json!({"tracks": [id], "freeze": true}));
    }
    if fz.is_some() && ui.button("Unfreeze").clicked() {
        run(app, ui, "track.unfreeze", json!({"tracks": [id]}));
    }
}

/// The chain: its tiles, the "+" row and the gaps that insert. On a frozen track, returns the
/// frozen part (its tiles, and the "+" row when the whole chain is frozen) for the strip to wash
/// and mark, and whether it is the whole chain.
pub(crate) fn chain(app: &mut SoundApp, ui: &mut Ui, track: &Track, meters: Option<&StripMeter>, wide: bool) -> Option<(Rect, bool)> {
    let frozen = Frozen::of(track);
    let w = ui.available_width();
    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
    let dragged = egui::DragAndDrop::payload::<ChainDrag>(ui.ctx());
    let mut tiles: Vec<Rect> = Vec::with_capacity(track.mixer.inserts.len());
    for (i, ins) in track.mixer.inserts.iter().enumerate() {
        let (r, _) = ui.allocate_exact_size(vec2(w, tile_height(ins, wide)), Sense::hover());
        let lifted = dragged.as_deref().is_some_and(|d| d.track == track.id && d.module == ins.id);
        tile(app, ui, track, i, ins, meters, r, wide, lifted);
        tiles.push(r);
        ui.add_space(GAP);
    }
    // A whole-chain freeze takes no new module; a partial one takes them after its point.
    let closed = frozen.is_some_and(|f| f.whole);
    let (plus, plus_resp) = ui.allocate_exact_size(vec2(w, PLUS_H), Sense::click());
    plus_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !closed, "Add module"));
    plus_row(ui, plus, plus_resp.hovered() && !closed);
    let full = track.mixer.inserts.len() >= soundcraft_model::MAX_MODULES;
    let plus_resp = if closed {
        plus_resp.on_hover_text("Frozen: unfreeze the track, or freeze it up to a module, to add modules")
    } else if full {
        plus_resp.on_hover_text("The chain is full")
    } else {
        plus_resp.on_hover_text("Add a module at the end")
    };
    if !full && !closed {
        picker_popup(app, &plus_resp, track.id, Pick::Add(None), "end");
    }
    let mut area = Rect::from_min_max(tiles.first().map_or(plus.min, |r| r.min), plus.max);
    area.min.y -= GAP;
    let area = area.intersect(ui.clip_rect());
    crate::browser::drop::chain_targets(app, ui, track, &tiles, plus, GAP);
    if dragged.is_some() {
        drop_target(app, ui, track, &tiles, plus, area, frozen);
    } else if !full {
        gap_adders(app, ui, track.id, &tiles, frozen);
    }
    let f = frozen?;
    let top = tiles.first().map_or(plus.min, |r| r.min);
    let bottom = if f.whole { plus.max } else { tiles.get(f.modules.checked_sub(1)?).map(|r| r.max)? };
    Some((Rect::from_min_max(top, pos2(plus.max.x, bottom.y)), f.whole))
}

/// The "+" row: a dashed outline and a plus sign, brighter on hover.
fn plus_row(ui: &Ui, r: Rect, hovered: bool) {
    let t = Tokens::current();
    let p = ui.painter();
    let color = if hovered { t.accent } else { t.text_dim.gamma_multiply(0.7) };
    if hovered {
        p.rect_filled(r, CornerRadius::same(5), t.accent.gamma_multiply(0.12));
    }
    let r = r.shrink(0.5);
    let corners = [r.left_top(), r.right_top(), r.right_bottom(), r.left_bottom(), r.left_top()];
    for w in corners.windows(2) {
        if let [a, b] = w {
            p.extend(Shape::dashed_line(&[*a, *b], Stroke::new(1.0, color), 3.0, 3.0));
        }
    }
    let c = r.center();
    p.line_segment([c - vec2(4.0, 0.0), c + vec2(4.0, 0.0)], Stroke::new(1.5, color));
    p.line_segment([c - vec2(0.0, 4.0), c + vec2(0.0, 4.0)], Stroke::new(1.5, color));
}

/// Hovering the gap between two tiles shows a thin "+" that inserts a module there (not inside
/// the frozen part of a frozen chain).
fn gap_adders(app: &mut SoundApp, ui: &mut Ui, track: TrackId, tiles: &[Rect], frozen: Option<Frozen>) {
    let t = Tokens::current();
    for (i, pair) in tiles.windows(2).enumerate() {
        let [above, below] = pair else { continue };
        if frozen.is_some_and(|f| !f.may_insert(i + 1)) {
            continue;
        }
        let zone = Rect::from_min_max(pos2(above.min.x, above.max.y - 1.0), pos2(above.max.x, below.min.y + 1.0));
        let resp = ui.interact(zone, ui.id().with(("chain-gap", track.0, i)), Sense::click());
        let open = egui::Popup::is_id_open(ui.ctx(), egui::Popup::default_response_id(&resp));
        if resp.hovered() || open {
            let y = zone.center().y;
            let p = ui.painter();
            p.hline(zone.x_range().shrink(6.0), y, Stroke::new(1.0, t.accent));
            let c = pos2(zone.center().x, y);
            p.circle_filled(c, 5.0, t.accent);
            p.line_segment([c - vec2(2.5, 0.0), c + vec2(2.5, 0.0)], Stroke::new(1.2, Color32::WHITE));
            p.line_segment([c - vec2(0.0, 2.5), c + vec2(0.0, 2.5)], Stroke::new(1.2, Color32::WHITE));
        }
        let resp = resp.on_hover_text("Insert a module here");
        picker_popup(app, &resp, track, Pick::Add(Some(i + 1)), i + 1);
    }
}

/// The searchable plugin picker under `resp` (opened by a click).
fn picker_popup(app: &mut SoundApp, resp: &egui::Response, track: TrackId, pick: Pick, salt: impl std::hash::Hash + std::fmt::Debug) {
    let salt = Id::new(salt);
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| module_picker(app, ui, track, pick, salt));
}

/// While a tile is dragged over this chain: the insertion line, and on release the drop. Not into
/// the frozen part of a frozen chain (a move or an Alt-copy alike): no line, a "not allowed"
/// cursor, and a drop there says why and changes nothing.
#[allow(clippy::too_many_arguments)]
fn drop_target(app: &mut SoundApp, ui: &Ui, target: &Track, tiles: &[Rect], plus: Rect, area: Rect, frozen: Option<Frozen>) {
    let track = target.id;
    let Some(pos) = ui.ctx().pointer_hover_pos().or_else(|| ui.ctx().pointer_latest_pos()) else { return };
    if !area.contains(pos) {
        return;
    }
    let centers: Vec<f32> = tiles.iter().map(|r| r.center().y).collect();
    let gap = gap_at(&centers, pos.y);
    if frozen.is_some_and(|f| !f.may_insert(gap)) {
        ui.ctx().set_cursor_icon(CursorIcon::NotAllowed);
        if ui.input(|i| i.pointer.any_released()) && egui::DragAndDrop::take_payload::<ChainDrag>(ui.ctx()).is_some() {
            app.ui.status = format!("`{}` is frozen. Unfreeze the track to edit it", target.name);
        }
        return;
    }
    let y = match (gap.checked_sub(1).and_then(|i| tiles.get(i)), tiles.get(gap)) {
        (Some(a), Some(b)) => (a.max.y + b.min.y) * 0.5,
        (None, Some(b)) => b.min.y - GAP * 0.5,
        (Some(a), None) => (a.max.y + plus.min.y) * 0.5,
        (None, None) => plus.min.y - GAP * 0.5,
    };
    let t = Tokens::current();
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, Id::new("chain-drop-line")));
    p.hline(area.x_range(), y, Stroke::new(2.0, t.accent));
    p.circle_filled(pos2(area.min.x + 2.0, y), 3.0, t.accent);
    p.circle_filled(pos2(area.max.x - 2.0, y), 3.0, t.accent);
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
    if ui.input(|i| i.pointer.any_released())
        && let Some(src) = egui::DragAndDrop::take_payload::<ChainDrag>(ui.ctx())
    {
        let alt = ui.input(|i| i.modifiers.alt);
        apply_drop(app, drop_action((src.track, src.module), (track, gap), alt));
    }
}

/// Whether low-latency monitoring skips chain module `index` (its bit in the strip's meter).
pub(crate) fn llm_bypassed(meters: Option<&StripMeter>, index: usize) -> bool {
    index < 64 && meters.is_some_and(|m| (m.llm_bypassed >> index) & 1 == 1)
}

/// The name, letter-spaced capitals, as a galley.
fn caps(ui: &Ui, text: &str, size: f32, color: Color32) -> std::sync::Arc<egui::Galley> {
    let mut job = egui::text::LayoutJob::default();
    job.append(&text.to_uppercase(), 0.0, egui::TextFormat { font_id: bold(size), color, extra_letter_spacing: 0.6, ..Default::default() });
    ui.painter().layout_job(job)
}

/// One module's tile in `r`.
#[allow(clippy::too_many_arguments)]
fn tile(app: &mut SoundApp, ui: &mut Ui, track: &Track, index: usize, ins: &Insert, meters: Option<&StripMeter>, r: Rect, wide: bool, lifted: bool) {
    let t = Tokens::current();
    let id = track.id;
    let meter = meters.and_then(|m| m.modules.iter().find(|x| x.module == ins.id));
    let face = face(ins);
    let name = tile_name(ins);
    let failed = meter.is_some_and(|m| m.failed);
    let off = ins.bypass || !ins.active;
    let open = app.ui.drawers.get(&id) == Some(&ins.id);
    // Frozen (Track › Freeze): the tile still opens its drawer and its menu (to move the freeze
    // point or unfreeze), but it neither bypasses, moves nor edits.
    let frozen = module_frozen(track, index);
    let mut ui = ui.new_child(egui::UiBuilder::new().id_salt(("tile", id.0, ins.id.0)).max_rect(r));
    if lifted {
        ui.multiply_opacity(0.35);
    }
    let ui = &mut ui;

    // Face and outline; an open drawer's tile is lit in the accent colour. A FireStation engine
    // wears its own: a wash along the top, a bar at the left, the open outline.
    let p = ui.painter().clone();
    let fs = matches!(face, Face::BuiltIn(i) if i.category == soundcraft_dsp::Category::FireStation);
    let accent = if fs { crate::theme::module_accent_for(ins) } else { t.accent };
    p.rect_filled(r, CornerRadius::same(6), t.module_tile_bg);
    if fs {
        let wash = Rect::from_min_size(r.min + vec2(1.0, 1.0), vec2(r.width() - 2.0, (PAD + HEAD + 6.0).min(r.height() - 2.0)));
        crate::widgets::vgradient(&p, wash, accent.gamma_multiply(if off { 0.05 } else { 0.13 }), accent.gamma_multiply(0.0));
        let bar = Rect::from_min_size(r.min + vec2(1.5, 5.0), vec2(2.0, (r.height() - 10.0).max(0.0)));
        p.rect_filled(bar, CornerRadius::same(1), if off { accent.gamma_multiply(0.35) } else { accent });
    }
    let edge = if open { Stroke::new(1.5, accent) } else { Stroke::new(1.0, t.module_tile_border) };
    p.rect_stroke(r, CornerRadius::same(6), edge, StrokeKind::Inside);

    // Header: drag handle, title (drawer / editor) and the LED (bypass).
    let head = Rect::from_min_size(r.min + vec2(PAD, PAD), vec2(r.width() - 2.0 * PAD, HEAD));
    let resp = ui.interact(head, ui.id().with("head"), Sense::click_and_drag());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{name} module")));
    let led_c = pos2(head.min.x + 4.0, head.center().y);
    let led_r = Rect::from_center_size(led_c, vec2(12.0, 14.0));
    let led = if frozen {
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(led_r));
        c.disable();
        c.interact(led_r, ui.id().with("led"), Sense::click())
    } else {
        ui.interact(led_r, ui.id().with("led"), Sense::click())
    };
    led.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, !frozen, format!("Bypass {name}")));
    let missing = matches!(face, Face::Missing);
    if missing || failed {
        // A red mark: the module passes the signal through untouched.
        p.circle_filled(led_c, 3.5, t.meter_red);
        p.line_segment([led_c - vec2(1.6, 1.6), led_c + vec2(1.6, 1.6)], Stroke::new(1.2, t.module_tile_bg));
        p.line_segment([led_c + vec2(-1.6, 1.6), led_c + vec2(1.6, -1.6)], Stroke::new(1.2, t.module_tile_bg));
    } else if off {
        p.circle_stroke(led_c, 3.0, Stroke::new(1.2, t.text_dim));
    } else {
        p.circle_filled(led_c, 6.0, t.meter_green.gamma_multiply(0.18));
        p.circle_filled(led_c, 3.2, t.meter_green);
    }
    if led.hovered() {
        p.circle_stroke(led_c, 5.5, Stroke::new(1.0, t.text_dim));
    }
    let name_color = if off || missing { t.text_dim } else { t.text };
    let size = if wide { 9.0 } else { 8.5 };
    let mut g = caps(ui, &tile_title(ins, wide), size, name_color);
    if wide && g.size().x > head.width() * TITLE_SHARE {
        // A long full name ("Compressor/Limiter") would crowd out the preset: the short one.
        g = caps(ui, &name, size, name_color);
    }
    // ⤢ at the right: the module's full editor in a window of its own.
    let mut head_right = head.max.x;
    let full_ok = !missing || soundcraft_mix::is_third_party(&ins.plugin);
    if full_ok {
        let fr = Rect::from_center_size(pos2(head.max.x - 6.0, head.center().y), vec2(13.0, 13.0));
        let full = ui.interact(fr, ui.id().with("full-editor"), Sense::click());
        full.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{name} full editor")));
        if full.hovered() {
            p.rect_filled(fr.expand(1.0), CornerRadius::same(3), t.text_dim.gamma_multiply(0.25));
        }
        full_editor_icon(&p, fr, if full.hovered() { t.text } else { t.text_dim });
        if full.on_hover_text("Open the full editor in its own window").clicked() {
            open_full_editor(app, ui, id, ins.id);
        }
        head_right = fr.min.x - 3.0;
    }
    // Low-latency monitoring skips this module on the live path: an "LLM" badge at the right.
    if llm_bypassed(meters, index) {
        let g = ui.painter().layout_no_wrap("LLM".into(), bold(7.0), t.text_dark);
        let pill = Rect::from_min_size(pos2(head_right - g.size().x - 6.0, head.center().y - 5.5), vec2(g.size().x + 6.0, 11.0));
        p.rect_filled(pill, CornerRadius::same(3), t.input);
        p.galley(pill.center() - g.size() * 0.5, g, t.text_dark);
        let badge = ui.interact(pill, ui.id().with("llm"), Sense::hover());
        badge.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, "LLM"));
        badge.on_hover_text("Bypassed by low-latency monitoring while this track is live");
        head_right = pill.min.x - 3.0;
    }
    let name_x = head.min.x + 11.0;
    let name_w = g.size().x;
    let name_clip = Rect::from_min_max(pos2(name_x, head.min.y), pos2(head_right, head.max.y));
    ui.painter().with_clip_rect(name_clip).galley(pos2(name_x, head.center().y - g.size().y * 0.5), g, name_color);
    if open {
        // An arrow after the name: the drawer is out.
        let a = pos2((name_x + name_w + 6.0).min(head.max.x - 4.0), head.center().y);
        p.add(Shape::convex_polygon(vec![a + vec2(-2.0, -3.5), a + vec2(2.5, 0.0), a + vec2(-2.0, 3.5)], accent, Stroke::NONE));
    }
    let right = if missing {
        Some(("missing".to_string(), t.text_dim))
    } else if failed {
        Some(("failed".to_string(), t.meter_red))
    } else if !ins.active {
        Some(("off".to_string(), t.text_dim))
    } else if !ins.preset.is_empty() && !ins.preset.starts_with('<') {
        Some((ins.preset.clone(), t.text_dim))
    } else if let Face::BuiltIn(info) = face {
        model_name(ins, info).map(|m| (m, accent.gamma_multiply(0.9)))
    } else {
        None
    };
    if let Some((text, color)) = right.filter(|_| wide) {
        let room = (head_right - (name_x + name_w + if open { 14.0 } else { 6.0 })).max(0.0);
        let g = ui.painter().layout_no_wrap(text, regular(8.5), color);
        if room >= 18.0 {
            let x = head_right - g.size().x.min(room);
            let clip = Rect::from_min_max(pos2(head_right - room, head.min.y), pos2(head_right, head.max.y));
            ui.painter().with_clip_rect(clip).galley(pos2(x, head.center().y - g.size().y * 0.5), g, color);
        }
    }

    // Clicks: the LED bypasses; the title opens the drawer (a hosted plugin's own window).
    if led.clicked() && !missing && !frozen {
        // Alt: the module at this position on every track; Alt+Shift: on the selected tracks.
        let m = ui.input(|i| i.modifiers);
        crate::scope::bypass(app, crate::MainWindow::Mix, id, ins.id, m);
    } else if resp.clicked() {
        if ui.input(|i| i.modifiers.command) && !missing {
            if frozen {
                app.ui.status = format!("`{}` is frozen. Unfreeze the track to edit it", track.name);
            } else {
                let m = ui.input(|i| i.modifiers);
                crate::scope::bypass(app, crate::MainWindow::Mix, id, ins.id, m);
            }
        } else {
            match face {
                Face::Hosted(_) => open_module(app, id, ins.id),
                Face::BuiltIn(_) => toggle_drawer(app, id, ins.id),
                // A hosted plugin not described (yet) still opens: its editor, else its window.
                Face::Missing if soundcraft_mix::is_third_party(&ins.plugin) => open_module(app, id, ins.id),
                Face::Missing => {}
            }
        }
    }
    if resp.drag_started() && !frozen {
        egui::DragAndDrop::set_payload(ui.ctx(), ChainDrag { track: id, module: ins.id });
    }
    if resp.dragged() {
        drag_ghost(ui, &name);
    } else if resp.hovered() {
        ui.ctx().set_cursor_icon(CursorIcon::Grab);
    }
    let full_name = match face {
        Face::BuiltIn(i) | Face::Hosted(i) => i.name.to_string(),
        Face::Missing => format!("{} (missing: not installed on this computer)", ins.plugin),
    };
    let cmd = if ui.ctx().os().is_mac() { "⌘" } else { "Ctrl+" };
    let open_tip = match face {
        Face::Hosted(_) => "Click: open the plugin",
        Face::BuiltIn(_) => "Click: open the drawer",
        Face::Missing if soundcraft_mix::is_third_party(&ins.plugin) => "Click: open the plugin",
        Face::Missing => "",
    };
    let resp =
        resp.on_hover_text(format!("{full_name}\n{open_tip} · drag: move (Alt: copy to another strip) · {cmd}click: bypass · right-click: menu"));
    let led = led.on_hover_text(if frozen {
        "Frozen: unfreeze the track to edit this module"
    } else if off {
        "Off: click to turn on"
    } else {
        "On: click to bypass"
    });
    let _ = led;
    resp.context_menu(|ui| module_menu(app, ui, track, index, ins));

    // Body.
    let mut y = head.max.y + ROW_GAP;
    let inner_x = r.x_range().shrink(PAD);
    let mut body = ui.new_child(egui::UiBuilder::new().id_salt("body").max_rect(Rect::from_x_y_ranges(inner_x, y..=r.max.y - PAD)));
    if off {
        body.multiply_opacity(0.45);
    }
    if frozen {
        body.disable();
    }
    match face {
        Face::BuiltIn(info) => {
            let gh = graph_height(info, wide);
            let gr = Rect::from_x_y_ranges(inner_x, y..=y + gh);
            let live = meter.map(live_values);
            let sr = app.engine.session().sample_rate.as_f64() as f32;
            let kind = graph_kind(&ins.plugin);
            let streams = streams_for(&body, id, ins, meters);
            draw_graph(&mut body, gr, kind, ins, module_key(id.0, ins.id.0), live.as_ref(), streams.as_deref(), sr, false);
            if wide && kind == GraphKind::EqCurve {
                eq_drag(app, &body, id, ins, gr, sr);
            }
            if wide && kind == GraphKind::CleanQ {
                cleanq_drag(app, &body, id, ins, gr, sr);
            }
            y = gr.max.y + ROW_GAP;
            if wide {
                let knobs = key_knobs(ins, info);
                if !knobs.is_empty() {
                    let kr = Rect::from_x_y_ranges(inner_x, y..=y + KNOB_ROW);
                    knob_row(app, &mut body, id, ins, &knobs, kr, accent);
                }
            }
        }
        Face::Hosted(_) => {
            let lh = if wide { LEVELS_WIDE } else { LEVELS_SLIM };
            let lr = Rect::from_x_y_ranges(inner_x, y..=y + lh);
            level_bars(&body, lr, meter.map(live_values).unwrap_or_default());
            let click = body.interact(lr.expand2(vec2(0.0, 2.0)), body.id().with("levels"), Sense::click());
            if click.on_hover_text("Click: open the plugin").clicked() {
                open_module(app, id, ins.id);
            }
        }
        // The red mark and "missing" (the tooltip, in slim strips) say it all.
        Face::Missing => {}
    }
}

/// The ⤢ glyph: a diagonal with an arrowhead at each end.
pub(crate) fn full_editor_icon(p: &egui::Painter, r: Rect, color: Color32) {
    let r = r.shrink(2.5);
    let (a, b) = (r.left_bottom(), r.right_top());
    let s = Stroke::new(1.2, color);
    p.line_segment([a, b], s);
    let k = r.width() * 0.45;
    p.line_segment([b, b - vec2(k, 0.0)], s);
    p.line_segment([b, b + vec2(0.0, k)], s);
    p.line_segment([a, a + vec2(k, 0.0)], s);
    p.line_segment([a, a - vec2(0.0, k)], s);
}

/// Opens `module`'s full editor (`ui.mix.full_editor`) next to the pointer.
fn open_full_editor(app: &mut SoundApp, ui: &Ui, track: TrackId, module: ModuleId) {
    if let Some(at) = ui.ctx().pointer_latest_pos() {
        let key = SlotKey::Module(module);
        ui.ctx().data_mut(|d| {
            d.insert_temp(Id::new(("full-editor", track.0, module.0)).with("anchor"), at);
            d.insert_temp(Id::new(("plugin-anchor", track.0, key)), at);
        });
    }
    if let Err(e) = app.run("ui.mix.full_editor", json!({"track": track.0, "module": module.0})) {
        app.ui.status = e;
    }
}

/// The dragged tile's name, following the pointer.
fn drag_ghost(ui: &Ui, name: &str) {
    let t = Tokens::current();
    let Some(pos) = ui.ctx().pointer_latest_pos() else { return };
    let p = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, Id::new("chain-drag-ghost")));
    let g = p.layout_no_wrap(name.to_uppercase(), bold(9.0), t.text);
    let r = Rect::from_min_size(pos + vec2(10.0, 6.0), g.size() + vec2(14.0, 8.0));
    p.rect(r, CornerRadius::same(5), t.module_tile_bg, Stroke::new(1.5, t.accent), StrokeKind::Inside);
    p.galley(r.min + vec2(7.0, 4.0), g, t.text);
    ui.ctx().set_cursor_icon(CursorIcon::Grabbing);
}

/// A third-party module's input and output levels, as two thin bars.
fn level_bars(ui: &Ui, r: Rect, live: LiveValues) {
    let t = Tokens::current();
    let p = ui.painter();
    let pos = |peak: f32| if peak.is_finite() && peak > 0.0 { ((20.0 * peak.log10() + 60.0) / 60.0).clamp(0.0, 1.0) } else { 0.0 };
    let bar_h = ((r.height() - 1.0) * 0.5).max(1.0);
    for (i, v) in [pos(live.in_peak), pos(live.out_peak)].into_iter().enumerate() {
        let b = Rect::from_min_size(pos2(r.min.x, r.min.y + i as f32 * (bar_h + 1.0)), vec2(r.width(), bar_h));
        p.rect_filled(b, 1.0, t.graph_bg);
        let color = if v > 0.97 {
            t.meter_red
        } else if v > 0.85 {
            t.meter_yellow
        } else {
            t.meter_green
        };
        p.rect_filled(Rect::from_min_size(b.min, vec2(b.width() * v, b.height())), 1.0, color);
    }
}

/// Dragging an EQ band dot sets its frequency and gain, one undo step per drag.
fn eq_drag(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, r: Rect, sr: f32) {
    let busy = ui.id().with("eq-drag-busy");
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

/// Dragging a CleanQ band dot sets its frequency (and gain), one undo step per drag; touching a
/// dot also shows that band in the drawer, when it is open.
pub(crate) fn cleanq_drag(app: &mut SoundApp, ui: &Ui, track: TrackId, ins: &Insert, r: Rect, sr: f32) {
    let busy = ui.id().with("cq-drag-busy");
    let touch = cleanq_band_drag(ui, r, ins, sr);
    if let Some(t) = &touch {
        let group = format!("Band {}", t.slot + 1);
        ui.data_mut(|d| d.insert_temp(crate::module_drawer::tab_key(track, ins.id), group));
    }
    match touch.filter(|t| !t.params.is_empty()) {
        Some(t) => {
            let map: serde_json::Map<String, serde_json::Value> = t.params.into_iter().map(|(k, v)| (k, json!(v))).collect();
            let key = format!("modparams:{}:{}", track.0, ins.id.0);
            if let Err(e) = app.engine.execute_merged("mix.module_params", &json!({"track": track.0, "module": ins.id.0, "params": map}), &key) {
                app.ui.status = e.to_string();
            }
            ui.data_mut(|d| d.insert_temp(busy, true));
        }
        None => {
            if ui.data(|d| d.get_temp::<bool>(busy)).unwrap_or(false) && !ui.ctx().input(|i| i.pointer.any_down()) {
                ui.data_mut(|d| d.remove::<bool>(busy));
                app.engine.end_merge();
            }
        }
    }
}

/// The key knobs of a wide tile, evenly spaced, each named underneath.
fn knob_row(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, knobs: &[&'static soundcraft_dsp::ParamInfo], r: Rect, accent: Color32) {
    let t = Tokens::current();
    let n = knobs.len().max(1) as f32;
    let cell_w = r.width() / n;
    let size = if knobs.len() >= 4 { 15.0 } else { 18.0 };
    for (i, info) in knobs.iter().enumerate() {
        let cell = Rect::from_min_size(pos2(r.min.x + cell_w * i as f32, r.min.y), vec2(cell_w, r.height()));
        let knob_r = Rect::from_min_max(cell.min, pos2(cell.max.x, cell.max.y - 10.0));
        let value = ins.params.get(info.id).copied().unwrap_or(info.default);
        let mut c = ui.new_child(
            egui::UiBuilder::new().id_salt(("knob", info.id)).max_rect(knob_r).layout(egui::Layout::centered_and_justified(egui::Direction::TopDown)),
        );
        // A knob the engine's current model does not use (hidden in its drawer) keeps its place,
        // dimmed and inert; one the engine derives from others (Dist's Level under Link) is shown,
        // greyed, not movable.
        let applies = crate::key_knobs::tile_knob_applies(ins, info.id);
        if !applies || crate::module_drawer::param_locked(ins, info.id) {
            c.disable();
        }
        if !applies {
            c.multiply_opacity(0.4);
        }
        let out = c.vertical_centered(|ui| knob(ui, info, value, size, accent, KnobStyle::Dark)).inner;
        let busy = ui.id().with(("knob-busy", info.id));
        if let Some(v) = out.changed {
            let key = format!("modparam:{}:{}:{}", track.0, ins.id.0, info.id);
            let _ = app.engine.execute_merged("mix.module_param", &json!({"track": track.0, "module": ins.id.0, "param": info.id, "value": v}), &key);
            if !out.dragging {
                app.engine.end_merge();
            }
        }
        if out.dragging {
            app.hold_control(track, soundcraft_model::AutoParam::Plugin { module: ins.id, param: info.id.to_string() });
            ui.data_mut(|d| d.insert_temp(busy, true));
        } else if ui.data(|d| d.get_temp::<bool>(busy)).unwrap_or(false) {
            ui.data_mut(|d| d.remove::<bool>(busy));
            app.engine.end_merge();
        }
        let label = crate::key_knobs::tile_knob_label(ins, info);
        let color = if applies { t.text_dim } else { t.text_dim.gamma_multiply(0.45) };
        let g = ui.painter().layout_no_wrap(label, regular(8.0), color);
        let at = Align2::CENTER_BOTTOM.anchor_size(pos2(cell.center().x, cell.max.y), g.size()).min;
        ui.painter().with_clip_rect(cell).galley(at, g, color);
    }
}

/// A parameter name short enough for a knob label.
pub(crate) fn short_param(name: &str) -> String {
    let n = name.trim();
    match n {
        "Threshold" => "Thresh".into(),
        "Makeup Gain" | "Makeup" => "Makeup".into(),
        "Output Gain" | "Output" => "Output".into(),
        _ if n.chars().count() > 9 => n.split_whitespace().next().unwrap_or(n).chars().take(9).collect(),
        _ => n.to_string(),
    }
}

/// A tile's right-click menu.
fn module_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track, index: usize, ins: &Insert) {
    let id = track.id;
    let face = face(ins);
    let m = ins.id.0;
    ui.set_min_width(170.0);
    // A frozen module only opens; it is not bypassed, duplicated into the frozen part, removed,
    // replaced or recalled.
    let frozen = Frozen::of(track);
    let held = frozen.is_some_and(|f| f.holds(index));
    let close = |app: &mut SoundApp, ui: &Ui, r: Result<serde_json::Value, String>| {
        if let Err(e) = r {
            app.ui.status = e;
        }
        ui.close();
    };
    if !matches!(face, Face::Missing) && ui.add_enabled(!held, egui::Button::selectable(ins.bypass, "Bypass")).clicked() {
        let r = app.run("mix.module_bypass", json!({"track": id.0, "module": m}));
        close(app, ui, r);
    }
    match face {
        Face::Hosted(_) | Face::Missing if soundcraft_mix::is_third_party(&ins.plugin) => {
            if ui.button("Open Plugin…").clicked() {
                open_module(app, id, ins.id);
                ui.close();
            }
            hosted_menu_items(app, ui, id, SlotKey::Module(ins.id));
        }
        Face::BuiltIn(_) => {
            // The generic window is reachable from the Edit window's inserts list.
            let open = app.ui.drawers.get(&id) == Some(&ins.id);
            if ui.button(if open { "Fold Drawer" } else { "Open Drawer" }).clicked() {
                toggle_drawer(app, id, ins.id);
                ui.close();
            }
        }
        Face::Hosted(_) | Face::Missing => {}
    }
    let full = track.mixer.inserts.len() >= soundcraft_model::MAX_MODULES;
    let may_copy = frozen.is_none_or(|f| f.may_insert(index + 1));
    if ui.add_enabled(!full && may_copy, egui::Button::new("Duplicate")).clicked() {
        let r = app.run("mix.module_copy", json!({"track": id.0, "module": m, "to_track": id.0, "at": index + 1}));
        close(app, ui, r);
    }
    if ui.add_enabled(!held, egui::Button::new("Remove")).clicked() {
        app.ui.drawers.retain(|t, x| !(*t == id && *x == ins.id));
        let r = app.run("mix.module_remove", json!({"track": id.0, "module": m}));
        close(app, ui, r);
    }
    ui.separator();
    if let Face::BuiltIn(info) = face {
        ui.add_enabled_ui(!held, |ui| ui.menu_button("Presets", |ui| crate::panels::preset_menu(app, ui, id, ins.id, info, ins)));
        ui.menu_button("Knobs", |ui| knobs_menu(app, ui, id, ins, info));
    }
    ui.add_enabled_ui(!held, |ui| ui.menu_button("Replace", |ui| module_picker(app, ui, id, Pick::Replace(ins.id), Id::new(("replace", m)))));
    if offers_freeze(track) {
        ui.separator();
        freeze_items(app, ui, track, ins);
    }
}

/// Which parameters a tile shows as knobs (at most four).
pub(crate) fn knobs_menu(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, info: &'static PluginInfo) {
    let current: Vec<String> = key_knobs(ins, info).iter().map(|p| p.id.to_string()).collect();
    ui.label(egui::RichText::new(format!("Knobs on the strip (up to {MAX_KNOBS})")).small().weak());
    let mut next = None;
    // Not toggles, and nothing from a group the drawer has switched off.
    for p in info.params.iter().filter(|p| p.unit != soundcraft_dsp::Unit::Toggle && crate::module_drawer::group_disabled(info.id, p.group).is_none())
    {
        let on = current.iter().any(|k| k == p.id);
        let enabled = on || current.len() < MAX_KNOBS;
        if ui.add_enabled(enabled, egui::Button::selectable(on, p.name)).clicked() {
            let mut list = current.clone();
            if on {
                list.retain(|k| k != p.id);
            } else {
                list.push(p.id.to_string());
            }
            next = Some(list);
        }
    }
    ui.separator();
    if ui.add_enabled(!ins.knobs.is_empty(), egui::Button::new("Defaults")).clicked() {
        next = Some(Vec::new());
    }
    if let Some(list) = next {
        // A failure shows in the status line (`SoundApp::run`).
        let _ = app.run("mix.module_knobs", json!({"track": track.0, "module": ins.id.0, "knobs": list}));
    }
}

/// The gap a pointer at `y` drops into, given the tiles' vertical centres (0 = before the first).
pub fn gap_at(tile_centers: &[f32], y: f32) -> usize {
    tile_centers.iter().filter(|c| **c < y).count()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_llm_badge_follows_the_meter_bits() {
        let m = StripMeter { llm_bypassed: 0b101 | (1 << 63), ..StripMeter::default() };
        let shown: Vec<usize> = (0..70).filter(|i| super::llm_bypassed(Some(&m), *i)).collect();
        assert_eq!(shown, [0, 2, 63]);
        assert!(!super::llm_bypassed(None, 0));
    }

    #[test]
    fn gaps_follow_tile_centres() {
        let centers = [10.0, 40.0, 70.0];
        assert_eq!(gap_at(&centers, 0.0), 0);
        assert_eq!(gap_at(&centers, 39.0), 1);
        assert_eq!(gap_at(&centers, 41.0), 2);
        assert_eq!(gap_at(&centers, 500.0), 3);
    }

    #[test]
    fn missing_plugins_are_named_by_their_id() {
        let ins = Insert::new("clap:com.acme.missing-thing");
        assert!(matches!(face(&ins), Face::Missing));
        assert_eq!(tile_name(&ins), "missing-thing");
        assert_eq!(tile_title(&ins, true), "missing-thing");
        assert_eq!(tile_height(&ins, true), PAD + HEAD + PAD);
    }

    #[test]
    fn wide_tiles_take_the_full_name() {
        let ins = Insert::new("fs_transient");
        assert_eq!(tile_title(&ins, true), "Transient");
        assert_eq!(tile_title(&ins, false), "Transnt");
        assert_eq!(tile_name(&ins), "Transnt");
    }

    #[test]
    fn param_labels_fit_under_a_knob() {
        assert_eq!(short_param("Threshold"), "Thresh");
        assert_eq!(short_param("Very Long Parameter"), "Very");
        assert_eq!(short_param("Ratio"), "Ratio");
    }

    /// The demo session with a live player in which Kick's compressor reports `b"live"`, a
    /// stand-in for a third-party plugin whose state changed after it loaded (the session still
    /// holds `b"saved"`).
    fn live_state_app() -> (SoundApp, TrackId, TrackId, ModuleId) {
        let mut engine = soundcraft_engine::demo::demo_engine();
        let kick = engine.session().track_by_name("Kick").cloned().unwrap();
        let bass = engine.session().track_by_name("Bass").unwrap().id;
        let comp = kick.mixer.inserts.iter().find(|i| i.plugin == "compressor").unwrap().id;
        let saved = soundcraft_model::b64::encode(b"saved");
        engine.execute("mix.module_state", &json!({"track": kick.id.0, "module": comp.0, "state": saved})).unwrap();
        let player = soundcraft_playback::Player::new(engine.session_arc());
        player.attach_state_reader(kick.id, SlotKey::Module(comp), Box::new(|| Some(b"live".to_vec())));
        (SoundApp::new(engine, Some(player), crate::Services::default()), kick.id, bass, comp)
    }

    fn state_of(app: &SoundApp, track: TrackId, module: ModuleId) -> Option<Vec<u8>> {
        app.engine.session().track(track).and_then(|t| t.mixer.module(module)).and_then(Insert::state_bytes)
    }

    fn new_module(r: &serde_json::Value) -> ModuleId {
        ModuleId(r["module"].as_u64().and_then(|m| u32::try_from(m).ok()).unwrap())
    }

    #[test]
    fn copies_and_moves_carry_the_live_plugin_state() {
        let (mut app, kick, bass, comp) = live_state_app();
        apply_drop(&mut app, DropAction::Copy { from: kick, module: comp, to: bass, at: 0 });
        let copy = app.engine.session().track(bass).unwrap().mixer.inserts[0].id;
        assert_eq!(state_of(&app, bass, copy).as_deref(), Some(&b"live"[..]), "Alt-drop copy");
        let dup = app.run("mix.module_copy", json!({"track": kick.0, "module": comp.0, "to_track": kick.0, "at": 0})).unwrap();
        assert_eq!(state_of(&app, kick, new_module(&dup)).as_deref(), Some(&b"live"[..]), "Duplicate");

        let (mut app, kick, bass, comp) = live_state_app();
        apply_drop(&mut app, DropAction::MoveAcross { from: kick, module: comp, to: bass, at: 0 });
        let moved = app.engine.session().track(bass).unwrap().mixer.inserts[0].id;
        assert_eq!(state_of(&app, bass, moved).as_deref(), Some(&b"live"[..]), "move to another strip");
    }

    #[test]
    fn undoing_a_remove_or_replace_restores_the_live_plugin_state() {
        let (mut app, kick, _, comp) = live_state_app();
        app.run("mix.module_remove", json!({"track": kick.0, "module": comp.0})).unwrap();
        assert!(app.engine.undo());
        assert_eq!(state_of(&app, kick, comp).as_deref(), Some(&b"live"[..]), "remove, undo");

        let (mut app, kick, _, comp) = live_state_app();
        crate::mix_window::replace_module(&mut app, kick, comp, "gain").unwrap();
        assert!(app.engine.undo());
        assert_eq!(state_of(&app, kick, comp).as_deref(), Some(&b"live"[..]), "replace, undo");
    }

    #[test]
    fn live_values_drop_non_finite_readings() {
        let m = ModuleMeter { module: ModuleId(1), in_peak: f32::NAN, out_peak: 0.5, gr_db: f32::INFINITY, readouts: [0.0; 8], failed: false };
        let l = live_values(&m);
        assert_eq!((l.in_peak, l.out_peak, l.gr_db), (0.0, 0.5, 0.0));
    }
}

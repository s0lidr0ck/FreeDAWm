//! Dragging a browser card onto the Edit and Mix windows, double-clicking it, and the card menu's
//! actions that place or open a plugin.
//!
//! The windows register the places a plugin can land as they draw ([`target`]); once everything
//! is drawn, [`finish`] outlines the one under the pointer and, on release, places the plugin
//! there ([`apply_drop`]). Every placement is the existing command, so undo works as it does for
//! the insert menus. A preset row of the Presets tab drags its plugin with that preset: placed
//! with it (one undo step), or loaded into a slot that already holds the plugin.

use super::catalog::{BrowserItem, Kind};
use super::presets::PresetRef;
use crate::SoundApp;
use crate::theme::{Tokens, bold};
use egui::{CornerRadius, DragAndDrop, Id, LayerId, Order, Rect, Stroke, StrokeKind, Ui, ViewportId, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_mix::SlotKey;
use soundcraft_model::{ModuleId, TrackId, TrackKind};

/// The egui drag-and-drop payload of a card being dragged.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginDrag {
    /// The plugin id inserts use.
    pub plugin: String,
    pub kind: Kind,
    /// A preset to place the plugin with (or to load into a slot already holding it).
    pub preset: Option<PresetRef>,
}

/// Where a dragged plugin can land.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DropTarget {
    /// An instrument track's instrument slot.
    InstrumentSlot(TrackId),
    /// A chain position (an empty one appends; a used one is replaced).
    InsertSlot(TrackId, usize),
    /// A track's header, row or strip: its instrument, or the end of its chain.
    Track(TrackId),
    /// Between two modules of a chain: inserted at that position.
    ChainGap(TrackId, usize),
    /// The space below the tracks (right of the strips): a new track with the plugin.
    NewTrack,
}

/// A place registered this frame.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Registered {
    viewport: ViewportId,
    layer: LayerId,
    rect: Rect,
    target: DropTarget,
}

/// Whether a card is being dragged.
pub(crate) fn dragging(ctx: &egui::Context) -> bool {
    DragAndDrop::has_payload_of_type::<PluginDrag>(ctx)
}

fn origin_id() -> Id {
    Id::new("browser-drag-viewport")
}

/// Starts dragging the card's plugin (from this viewport).
pub(crate) fn start(ctx: &egui::Context, item: &BrowserItem) {
    start_with(ctx, item, None);
}

/// Starts dragging the plugin of `item` with `preset` (a Presets tab row).
pub(crate) fn start_with(ctx: &egui::Context, item: &BrowserItem, preset: Option<PresetRef>) {
    DragAndDrop::set_payload(ctx, PluginDrag { plugin: item.id.clone(), kind: item.kind, preset });
    let viewport = ctx.viewport_id();
    ctx.data_mut(|d| d.insert_temp(origin_id(), viewport));
}

/// Registers `rect` (as far as it is visible in `ui`) as a place a dragged plugin can land. Costs
/// nothing while no card is dragged; a disabled `ui` (a frozen chain's rows) takes nothing.
pub(crate) fn target(app: &mut SoundApp, ui: &Ui, rect: Rect, target: DropTarget) {
    if !dragging(ui.ctx()) || !ui.is_enabled() {
        return;
    }
    let rect = rect.intersect(ui.clip_rect());
    if rect.is_positive() {
        app.plugin_drop_targets.push(Registered { viewport: ui.ctx().viewport_id(), layer: ui.layer_id(), rect, target });
    }
}

/// The most specific place under `pos` in this viewport (the smallest), unless a window covers it.
fn under(app: &SoundApp, ctx: &egui::Context, pos: egui::Pos2) -> Option<Registered> {
    let viewport = ctx.viewport_id();
    let top = ctx.layer_id_at(pos);
    app.plugin_drop_targets
        .iter()
        .filter(|r| r.viewport == viewport && r.rect.contains(pos))
        .filter(|r| top.is_none_or(|l| l == r.layer || l.order == Order::Background))
        .min_by(|a, b| a.rect.area().total_cmp(&b.rect.area()))
        .copied()
}

/// After this viewport is drawn: while a card is dragged, its name at the pointer and the place
/// under it outlined; on release, the plugin placed there.
pub(crate) fn finish(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(drag) = DragAndDrop::payload::<PluginDrag>(ctx) else {
        app.browser_debug.drop_hover = None;
        return;
    };
    // Only the window the drag started in has its pointer (egui's pointer is per viewport).
    let origin = ctx.data(|d| d.get_temp::<ViewportId>(origin_id()));
    if origin.is_some_and(|v| v != ctx.viewport_id()) {
        return;
    }
    let Some(pos) = ctx.input(|i| i.pointer.has_pointer().then(|| i.pointer.latest_pos()).flatten()) else {
        app.browser_debug.drop_hover = None;
        return;
    };
    let hit = under(app, ctx, pos);
    app.browser_debug.drop_hover = hit.map(|h| h.target);
    let t = Tokens::current();
    if let Some(h) = hit {
        let p = ctx.layer_painter(LayerId::new(Order::Foreground, Id::new("browser-drop-outline")));
        match h.target {
            DropTarget::ChainGap(..) => {
                let y = h.rect.center().y;
                p.hline(h.rect.x_range(), y, Stroke::new(2.0, t.accent));
                p.circle_filled(pos2(h.rect.min.x + 2.0, y), 3.0, t.accent);
                p.circle_filled(pos2(h.rect.max.x - 2.0, y), 3.0, t.accent);
            }
            _ => {
                p.rect_filled(h.rect, CornerRadius::same(2), t.accent.gamma_multiply(0.10));
                p.rect_stroke(h.rect, CornerRadius::same(2), Stroke::new(2.0, t.accent), StrokeKind::Inside);
            }
        }
    }
    // The plugin's name (and preset's) follows the pointer.
    let name = match &drag.preset {
        Some(p) => format!("{} \u{2014} {}", plugin_name(app, &drag.plugin), super::presets::label(app, &drag.plugin, p)),
        None => plugin_name(app, &drag.plugin),
    };
    let p = ctx.layer_painter(LayerId::new(Order::Tooltip, Id::new("browser-drag-ghost")));
    let galley = p.layout_no_wrap(name, bold(11.5), t.text);
    let r = Rect::from_min_size(pos + vec2(14.0, 10.0), galley.size() + vec2(12.0, 6.0));
    p.rect(
        r,
        CornerRadius::same(4),
        t.panel_bg2.gamma_multiply(0.95),
        Stroke::new(1.0, if hit.is_some() { t.accent } else { t.border }),
        StrokeKind::Inside,
    );
    p.galley(r.min + vec2(6.0, 3.0), galley, if hit.is_some() { t.text } else { t.text_dim });
    if ctx.input(|i| i.pointer.any_released())
        && let Some(drag) = DragAndDrop::take_payload::<PluginDrag>(ctx)
    {
        app.browser_debug.drop_hover = None;
        if let Some(h) = hit {
            let _ = apply_drop(app, &drag, h.target);
        }
    }
}

/// The plugin's name: the browser's, else the plugin registry's, else its id.
fn plugin_name(app: &SoundApp, plugin: &str) -> String {
    app.browser_catalog
        .as_ref()
        .and_then(|c| c.items.iter().find(|i| i.id == plugin))
        .map(|i| i.name.clone())
        .or_else(|| crate::mix_window::plugin_info(plugin).map(|i| i.name.to_string()))
        .unwrap_or_else(|| plugin.to_string())
}

/// Places the dragged plugin on `target`, as the insert and instrument menus would (a module
/// opens once the audio engine has it). With a preset, the preset is loaded in the same undo step;
/// onto a slot already holding the plugin, the preset is loaded into it instead. An error is also
/// shown in the status line.
pub fn apply_drop(app: &mut SoundApp, drag: &PluginDrag, target: DropTarget) -> Result<(), String> {
    let r = match &drag.preset {
        None => place(app, drag, target).map(|_| ()),
        Some(p) => place_with_preset(app, drag, p, target),
    };
    if let Err(e) = &r {
        app.ui.status = e.clone();
    }
    r
}

/// The slot `target` names when it already holds `drag`'s plugin: an insert of it, or an
/// instrument track's instrument.
fn holding(app: &SoundApp, drag: &PluginDrag, target: DropTarget) -> Option<(TrackId, SlotKey)> {
    let s = app.engine.session();
    match target {
        DropTarget::InsertSlot(t, slot) => {
            let ins = s.track(t)?.mixer.inserts.get(slot)?;
            (ins.plugin == drag.plugin).then_some((t, SlotKey::Module(ins.id)))
        }
        DropTarget::InstrumentSlot(t) | DropTarget::Track(t) if drag.kind == Kind::Instrument => {
            (s.track(t)?.instrument.as_ref()?.plugin == drag.plugin).then_some((t, SlotKey::Instrument))
        }
        _ => None,
    }
}

fn place_with_preset(app: &mut SoundApp, drag: &PluginDrag, preset: &PresetRef, target: DropTarget) -> Result<(), String> {
    if let Some((t, slot)) = holding(app, drag, target) {
        return super::presets::load_into(app, t, slot, &drag.plugin, preset);
    }
    // Worked out first, so a preset that cannot load places nothing.
    let load = super::presets::resolve(app, &drag.plugin, preset)?;
    let Some((t, slot)) = place(app, drag, target)? else {
        // Placed, but where is not known: take it away again rather than leave it without
        // the preset asked for.
        app.engine.undo_discard();
        app.pending_open = None;
        return Err(format!("{} was not placed with the preset", plugin_name(app, &drag.plugin)));
    };
    if let Err(e) = super::presets::apply(app, t, slot, &load, true) {
        // Take the placement away again, leaving nothing to redo.
        app.engine.undo_discard();
        app.pending_open = None;
        return Err(e);
    }
    Ok(())
}

/// Places the plugin; returns the slot it went into (`None` when that is not known).
fn place(app: &mut SoundApp, drag: &PluginDrag, target: DropTarget) -> Result<Option<(TrackId, SlotKey)>, String> {
    let plugin = drag.plugin.as_str();
    let instrument = drag.kind == Kind::Instrument;
    let refused = |app: &SoundApp| format!("{} is an instrument: drop it on an instrument track or below the tracks", plugin_name(app, plugin));
    let kind = |app: &SoundApp, t: TrackId| app.engine.session().track(t).map(|t| t.kind).ok_or_else(|| "The track is gone".to_string());
    match target {
        DropTarget::NewTrack => new_track(app, drag),
        DropTarget::InstrumentSlot(t) | DropTarget::Track(t) if instrument => {
            if kind(app, t)? != TrackKind::Instrument {
                return Err(refused(app));
            }
            app.run("mix.instrument", json!({"track": t.0, "plugin": plugin})).map(|_| Some((t, SlotKey::Instrument)))
        }
        DropTarget::InsertSlot(..) | DropTarget::ChainGap(..) if instrument => Err(refused(app)),
        DropTarget::InstrumentSlot(t) | DropTarget::Track(t) => {
            let r = app.run("mix.module_add", json!({"track": t.0, "plugin": plugin}));
            added(app, t, r)
        }
        DropTarget::ChainGap(t, at) => {
            let r = app.run("mix.module_add", json!({"track": t.0, "plugin": plugin, "at": at}));
            added(app, t, r)
        }
        DropTarget::InsertSlot(t, slot) => {
            kind(app, t)?;
            let old = app.engine.session().track(t).and_then(|tr| tr.mixer.inserts.get(slot)).map(|i| i.id);
            let r = match old {
                Some(old) => crate::mix_window::replace_module(app, t, old, plugin),
                None => app.run("mix.insert", json!({"track": t.0, "slot": slot, "plugin": plugin})),
            };
            added(app, t, r)
        }
    }
}

/// A module was placed (its id in `r`): it opens once the audio engine has it.
fn added(app: &mut SoundApp, track: TrackId, r: Result<Value, String>) -> Result<Option<(TrackId, SlotKey)>, String> {
    let v = r?;
    let m = v["module"].as_u64().and_then(|m| u32::try_from(m).ok()).map(ModuleId);
    if let Some(m) = m {
        app.pending_open = Some((track, m));
    }
    Ok(m.map(|m| (track, SlotKey::Module(m))))
}

/// A new track with the plugin: an instrument track playing it, or an audio track with it as its
/// first insert (one undo step).
fn new_track(app: &mut SoundApp, drag: &PluginDrag) -> Result<Option<(TrackId, SlotKey)>, String> {
    let plugin = drag.plugin.as_str();
    if drag.kind == Kind::Instrument {
        let v = app.run("track.new", json!({"kind": "instrument", "instrument": plugin}))?;
        return Ok(v["tracks"].get(0).and_then(Value::as_u64).map(|t| (TrackId(t), SlotKey::Instrument)));
    }
    if app.edit_waits("track.new") {
        return Err(app.ui.status.clone());
    }
    // Checked first, so an unknown plugin leaves no empty track behind.
    if crate::mix_window::plugin_info(plugin).is_none() {
        return Err(format!("unknown plugin `{plugin}`"));
    }
    let key = "browser:new-track";
    app.engine.end_merge();
    let made = app.engine.execute_merged("track.new", &json!({"kind": "audio"}), key).map_err(|e| e.to_string());
    let r = made.and_then(|v| {
        let t = v["tracks"].get(0).and_then(Value::as_u64).ok_or_else(|| "no track was made".to_string())?;
        let params = json!({"track": t, "plugin": plugin});
        match app.engine.execute_merged("mix.module_add", &params, key) {
            Ok(v) => {
                app.note_placed("mix.module_add", &params);
                Ok((TrackId(t), v))
            }
            Err(e) => {
                // Take the new track away again, leaving nothing to redo.
                app.engine.undo_discard();
                Err(e.to_string())
            }
        }
    });
    app.engine.end_merge();
    let (t, v) = r?;
    added(app, t, Ok(v))
}

/// The first selected track.
fn selected(app: &SoundApp) -> Option<TrackId> {
    app.engine.session().edit.selected_tracks.first().copied()
}

/// Double-click and "Insert on selected track": the plugin onto the selected track (an
/// instrument into an instrument track's slot, an effect at the end of the chain).
pub(crate) fn place_on_selected(app: &mut SoundApp, item: &BrowserItem) {
    place_on_selected_with(app, item, None);
}

/// [`place_on_selected`] with a preset (a double-clicked Presets tab row): an instrument track
/// already playing the plugin has the preset loaded into it.
pub(crate) fn place_on_selected_with(app: &mut SoundApp, item: &BrowserItem, preset: Option<PresetRef>) {
    let Some(t) = selected(app) else {
        app.ui.status = "Select a track first".into();
        return;
    };
    let instrument_track = app.engine.session().track(t).is_some_and(|t| t.kind == TrackKind::Instrument);
    if item.kind == Kind::Instrument && !instrument_track {
        app.ui.status = format!("Select an instrument track, or drag {} below the tracks", item.name);
        return;
    }
    let target = if item.kind == Kind::Instrument { DropTarget::InstrumentSlot(t) } else { DropTarget::Track(t) };
    let _ = apply_drop(app, &PluginDrag { plugin: item.id.clone(), kind: item.kind, preset }, target);
}

/// "Open editor": the plugin's editor on the selected track (its instrument, else the first
/// module playing it), opened as clicking its slot would.
pub(crate) fn open_on_selected(app: &mut SoundApp, plugin: &str) {
    let Some(t) = selected(app) else {
        app.ui.status = "Select a track first".into();
        return;
    };
    let Some(track) = app.engine.session().track(t).cloned() else { return };
    if track.instrument.as_ref().is_some_and(|i| i.plugin == plugin) {
        // Its own editor when it has one, else its generic window.
        crate::chain_view::open_hosted(app, t, SlotKey::Instrument);
        return;
    }
    let Some(module) = track.mixer.inserts.iter().find(|i| i.plugin == plugin).map(|i| i.id) else {
        app.ui.status = format!("{} is not on {}", plugin_name(app, plugin), track.name);
        return;
    };
    if soundcraft_mix::is_third_party(plugin) {
        crate::chain_view::open_module(app, t, module);
    } else {
        // A built-in opens in its drawer, in the Mix window.
        app.ui.drawers.insert(t, module);
        let _ = app.run("window.mix", json!({}));
    }
}

/// The file (bundle) a hosted plugin was loaded from, from the hosts' last scans.
fn plugin_path(plugin: &str) -> Option<std::path::PathBuf> {
    let vst3 = soundcraft_vst3_host::cached().unwrap_or_default().into_iter().find(|d| d.id == plugin).map(|d| d.path);
    vst3.or_else(|| soundcraft_clap_host::cached().unwrap_or_default().into_iter().find(|d| d.id == plugin).map(|d| d.path))
}

/// "Show in folder": the plugin's file selected in the system's file browser.
pub(crate) fn show_in_folder(app: &mut SoundApp, item: &BrowserItem) {
    let Some(path) = plugin_path(&item.id) else {
        app.ui.status = format!("{}: no plugin file to show", item.name);
        return;
    };
    if let Err(e) = reveal(&path) {
        app.ui.status = format!("Could not show {}: {e}", path.display());
    }
}

#[cfg(target_os = "windows")]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    use std::os::windows::process::CommandExt;
    // Explorer parses its own command line: the path is quoted after the comma, and passed raw so
    // the standard quoting does not wrap the whole `/select,` argument.
    std::process::Command::new("explorer").raw_arg(format!("/select,\"{}\"", path.display())).spawn().map(|_| ())
}

#[cfg(target_os = "macos")]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    std::process::Command::new("open").arg("-R").arg(path).spawn().map(|_| ())
}

#[cfg(all(not(target_os = "windows"), not(target_os = "macos"), not(target_arch = "wasm32")))]
fn reveal(path: &std::path::Path) -> std::io::Result<()> {
    let folder = path.parent().unwrap_or(path);
    std::process::Command::new("xdg-open").arg(folder).spawn().map(|_| ())
}

#[cfg(target_arch = "wasm32")]
fn reveal(_path: &std::path::Path) -> std::io::Result<()> {
    Ok(())
}

/// Registers a chain's tiles (replaced when dropped on), the gaps above each tile and its "+" row
/// (inserted there) as drop places, leaving out a frozen chain's frozen part.
pub(crate) fn chain_targets(app: &mut SoundApp, ui: &Ui, track: &soundcraft_model::Track, tiles: &[Rect], plus: Rect, gap: f32) {
    if !dragging(ui.ctx()) {
        return;
    }
    let frozen = crate::chain_view::Frozen::of(track);
    let full = track.mixer.inserts.len() >= soundcraft_model::MAX_MODULES;
    for (i, tile) in tiles.iter().enumerate() {
        if !frozen.is_some_and(|f| f.holds(i)) {
            target(app, ui, *tile, DropTarget::InsertSlot(track.id, i));
        }
        if full || frozen.is_some_and(|f| !f.may_insert(i)) {
            continue;
        }
        let top = i.checked_sub(1).and_then(|k| tiles.get(k)).map_or(tile.min.y - gap - 3.0, |above| above.max.y - 4.0);
        target(app, ui, Rect::from_min_max(pos2(tile.min.x, top), pos2(tile.max.x, tile.min.y + 4.0)), DropTarget::ChainGap(track.id, i));
    }
    let end = tiles.len();
    if !full && !frozen.is_some_and(|f| !f.may_insert(end)) {
        let top = tiles.last().map_or(plus.min.y, |r| r.max.y - 4.0);
        target(app, ui, Rect::from_min_max(pos2(plus.min.x, top), plus.max), DropTarget::ChainGap(track.id, end));
    }
}

//! What the Engine family's editors (PitchEngine, ModEngine, DistEngine, FilterEngine, ...) share
//! beyond `Controls.h`: the header's preset strip (`◀ name* ▶  Undo  A  B`) and Bypass pill, the
//! editor's own Undo stack (`pushUndo` / `doUndo`: up to 64 snapshots of the preset parameters,
//! pushed when a gesture starts), the A/B slots (`selectAbSlot`), the preset name's dirty star
//! (`PresetNameButton::setPresetName (name, isDirty)`), and the browser's Copy / Paste of
//! FireStation preset text (`PresetManager::toClipboardText` / `fromClipboardText`).
//!
//! Every change goes through `mix.module_params`. Undo and A/B are the editor's own, as in the
//! VST (they live as long as the editor; the DAW's undo still covers each change).

use super::controls::{DialOpts, PillLook, PillStyle, dial, pill_ex};
use super::look::{tracked_text, tracked_width};
use super::palette::VstPalette;
use super::presets::{self, BrowserOut, PresetState};
use super::{Host, end_gesture, param, set_params, set_params_merged};
use crate::SoundApp;
use egui::{Align2, CornerRadius, Rect, Response, Sense, Stroke, StrokeKind, Ui, vec2};
use serde_json::{Map, Value, json};

/// One engine's preset facts (`PresetCatalogue`) and how its A/B and undo snapshot.
#[derive(Clone, Copy, Debug)]
pub struct Family {
    /// `storageKey`: the "plugin" a copied preset names ("PitchEngine").
    pub storage_key: &'static str,
    /// Parameters `getPresetParams` leaves out besides the engine's own `bypass` (Pitch: trim,
    /// level).
    pub not_in_presets: &'static [&'static str],
    /// A/B and undo take the whole rig (`getAbParams`: every parameter but `bypass`) and the
    /// module's derived-parameter memory (DistEngine's knob bank), rather than the preset
    /// parameters.
    pub whole_rig: bool,
    /// The header's Bypass (and an editor's other `attachPill`s) push undo first.
    pub pills_push_undo: bool,
}

impl Family {
    /// An engine whose A/B and undo walk the preset parameters and whose pills push undo.
    pub const fn new(storage_key: &'static str, not_in_presets: &'static [&'static str]) -> Self {
        Family { storage_key, not_in_presets, whole_rig: false, pills_push_undo: true }
    }

    fn in_presets(&self, id: &str) -> bool {
        id != "bypass" && !self.not_in_presets.contains(&id)
    }

    fn in_snapshots(&self, id: &str) -> bool {
        if self.whole_rig { id != "bypass" } else { self.in_presets(id) }
    }
}

/// `snapshot()`: the values, and for a whole rig the knob bank as it stands.
#[derive(Clone, Debug, Default)]
pub struct Snapshot {
    pub values: Vec<(&'static str, f32)>,
    pub memory: Option<std::collections::BTreeMap<String, f32>>,
}

/// `undoStack`'s cap.
const UNDO_DEPTH: usize = 64;

/// The family editor's own state (kept in the editor struct).
#[derive(Clone, Default, Debug)]
pub struct FamilyState {
    pub presets: PresetState,
    undo: Vec<Snapshot>,
    /// `abActive == "B"`.
    on_b: bool,
    slot_a: Option<Snapshot>,
    slot_b: Option<Snapshot>,
    /// `pushUndo` does nothing while set (DistEngine: "not the wizard's own moves").
    pub hold_undo: bool,
    /// Undo snapshots every parameter (Shale's `snapshot()`), not the preset parameters.
    pub snapshot_all: bool,
    /// The Undo stack's depth when not the family's 64 (Shale keeps 32).
    pub undo_cap: Option<usize>,
    /// The single A/B button's other slot (Swale's and Shale's `toggleAb`).
    ab_other: Option<Snapshot>,
    /// A recall from the browser and a paste push Undo first (Shale's `loadPreset` and
    /// `pastePreset`, as its `<` / `>` do).
    pub undo_on_load: bool,
}

impl FamilyState {
    /// The Undo pill is dimmed while the stack is empty.
    pub fn can_undo(&self) -> bool {
        !self.undo.is_empty()
    }
    /// B is the active slot.
    pub fn on_b(&self) -> bool {
        self.on_b
    }
}

/// `snapshot()`. A whole rig's bank first learns the live circuit's knobs (`rememberKnobs
/// (cachedCircuit)`): the module memory's `<circuit>.drive/tone/level`.
pub(crate) fn snapshot(host: &Host, fam: &Family) -> Snapshot {
    let values = host.info.params.iter().filter(|p| fam.in_snapshots(p.id)).map(|p| (p.id, param(host, p.id))).collect();
    let memory = fam.whole_rig.then(|| {
        let mut m = host.ins.memory.clone();
        let circuit = param(host, "circuit").round() as i32;
        for knob in ["drive", "tone", "level"] {
            m.insert(format!("{circuit}.{knob}"), param(host, knob));
        }
        m
    });
    Snapshot { values, memory }
}

/// `pushUndo`.
pub(crate) fn push_undo(st: &mut FamilyState, host: &Host, fam: &Family) {
    if st.hold_undo {
        return;
    }
    let snap = if st.snapshot_all {
        Snapshot { values: host.info.params.iter().map(|p| (p.id, param(host, p.id))).collect(), memory: None }
    } else {
        snapshot(host, fam)
    };
    st.undo.push(snap);
    if st.undo.len() > st.undo_cap.unwrap_or(UNDO_DEPTH) {
        st.undo.remove(0);
    }
}

/// `restore`: the snapshot as one change (a bank goes back first, and the values land without
/// a knob swap: `mix.module_params {memory}`).
fn restore(app: &mut SoundApp, host: &Host, snap: &Snapshot) {
    end_gesture(app);
    match &snap.memory {
        None => set_params(app, host, &snap.values),
        Some(m) => {
            let params: Map<String, Value> = snap.values.iter().map(|(k, v)| ((*k).to_string(), json!(v))).collect();
            let r = app.run("mix.module_params", json!({"track": host.track.0, "module": host.ins.id.0, "params": params, "memory": m}));
            if let Err(e) = r {
                app.ui.status = e;
            }
        }
    }
}

/// `doUndo`.
pub(crate) fn do_undo(app: &mut SoundApp, host: &Host, st: &mut FamilyState) {
    if let Some(v) = st.undo.pop() {
        restore(app, host, &v);
    }
}

/// `selectAbSlot`: stash what is on screen into the slot being left, recall the other (an empty
/// slot keeps the current settings, so B starts as a copy of A).
pub(crate) fn select_ab(app: &mut SoundApp, host: &Host, st: &mut FamilyState, fam: &Family, b: bool) {
    if b == st.on_b {
        return;
    }
    let now = snapshot(host, fam);
    if st.on_b {
        st.slot_b = Some(now);
    } else {
        st.slot_a = Some(now);
    }
    st.on_b = b;
    let recall = if b { st.slot_b.clone() } else { st.slot_a.clone() };
    if let Some(v) = recall {
        restore(app, host, &v);
    }
}

/// `toggleAb` (one A/B button: Swale, Shale): pushes Undo, swaps what is on screen with the other
/// slot (the first press only stashes, so B starts as a copy of A).
pub(crate) fn toggle_ab(app: &mut SoundApp, host: &Host, st: &mut FamilyState, fam: &Family) {
    let current = snapshot(host, fam);
    push_undo(st, host, fam);
    if let Some(v) = st.ab_other.take() {
        restore(app, host, &v);
    }
    st.ab_other = Some(current);
    st.on_b = !st.on_b;
}

/// An `attachPill` click: pushes undo first where the engine's pills do.
pub(crate) fn pill_set(app: &mut SoundApp, host: &Host, st: &mut FamilyState, fam: &Family, id: &str, v: f32) {
    if fam.pills_push_undo {
        push_undo(st, host, fam);
    }
    end_gesture(app);
    set_params(app, host, &[(id, v)]);
}

/// A complete one-shot change (`setValueAsCompleteGesture` after `pushUndo`): a Segmented pick,
/// a Pill click.
pub(crate) fn set(app: &mut SoundApp, host: &Host, st: &mut FamilyState, fam: &Family, id: &str, v: f32) {
    push_undo(st, host, fam);
    end_gesture(app);
    set_params(app, host, &[(id, v)]);
}

/// `attachDial`: an `fs::Dial` bound to parameter `id` (normalised over its range). A press,
/// double-click or wheel step pushes undo (`onDragStart`); a drag is one change.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dial_param(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    fam: &Family,
    rect: Rect,
    pal: &VstPalette,
    id: &str,
    o: &DialOpts,
) -> Response {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else {
        return ui.interact(rect, ui.id().with(("vst-dial-missing", id)), Sense::hover());
    };
    let v01 = info.to_normalized(param(host, id));
    let out = dial(ui, rect, id, pal, v01, o);
    if out.started {
        end_gesture(app);
        push_undo(st, host, fam);
    }
    if let Some(v) = out.value {
        set_params_merged(app, host, &[(id, info.clamp(info.from_normalized(v)))], &format!("dial-{id}"));
    }
    if out.ended {
        end_gesture(app);
    }
    out.response
}

/// A parameter's normalised default (`dial.setDefault (convertFrom0to1 (getDefaultValue()))`).
pub(crate) fn default01(host: &Host, id: &str) -> f32 {
    host.info.params.iter().find(|p| p.id == id).map_or(0.5, |p| p.to_normalized(p.default))
}

/// A parameter's normalised value.
pub(crate) fn value01(host: &Host, id: &str) -> f32 {
    host.info.params.iter().find(|p| p.id == id).map_or(0.0, |p| p.to_normalized(param(host, id)))
}

// ---------------------------------------------------------------------------- the header

/// Where the header's controls sit (each editor's `resized()` decides).
#[derive(Clone, Copy, Debug)]
pub struct HeaderRects {
    pub prev: Rect,
    pub name: Rect,
    pub next: Rect,
    pub undo: Rect,
    pub a: Rect,
    pub b: Rect,
    pub bypass: Rect,
}

/// `Pill::preferredWidth` of a Small, untracked pill (`undoButton.preferredWidth()`).
pub(crate) fn small_pill_w(ui: &Ui, text: &str) -> f32 {
    super::controls::pill_width_ex(ui, text, PillStyle::Small, false)
}

fn plain() -> PillLook {
    PillLook { tracked: false, ..PillLook::default() }
}

/// The header's preset strip and Bypass: `prevPreset` / `nextPreset` step the current category,
/// the name opens the browser, Undo, A, B, and Bypass (the engine's own `bypass`, lit red).
pub(crate) fn header(app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, st: &mut FamilyState, fam: &Family, r: &HeaderRects) {
    st.presets.not_in_presets = fam.not_in_presets;
    for (rect, left) in [(r.prev, true), (r.next, false)] {
        let resp = pill_ex(ui, rect, ("fam-step", left), pal, "", false, PillStyle::Small, plain());
        let ink = if resp.hovered() { pal.text_bright } else { pal.text_dim };
        tri(ui.painter(), rect.center(), left, ink);
        if resp.clicked() {
            let out = presets::step(app, host, &mut st.presets, if left { -1 } else { 1 });
            if out.recalled.is_some() {
                end_gesture(app);
            }
        }
    }
    let shown = presets::shown_name(host).to_string();
    let dirty = presets::is_dirty(app, host, &mut st.presets);
    if preset_name(ui, r.name, pal, &shown, dirty).clicked() {
        presets::open(app, host, &mut st.presets);
    }
    let look = PillLook { dimmed: !st.can_undo(), ..plain() };
    if pill_ex(ui, r.undo, "fam-undo", pal, "Undo", false, PillStyle::Small, look).clicked() {
        do_undo(app, host, st);
    }
    if pill_ex(ui, r.a, "fam-a", pal, "A", !st.on_b, PillStyle::Small, plain()).clicked() {
        select_ab(app, host, st, fam, false);
    }
    if pill_ex(ui, r.b, "fam-b", pal, "B", st.on_b, PillStyle::Small, plain()).clicked() {
        select_ab(app, host, st, fam, true);
    }
    let on = param(host, "bypass") > 0.5;
    let look = PillLook { active_colour: Some(pal.bypass_red), ..plain() };
    if pill_ex(ui, r.bypass, "fam-bypass", pal, "Bypass", on, PillStyle::Small, look).clicked() && host.info.params.iter().any(|p| p.id == "bypass") {
        pill_set(app, host, st, fam, "bypass", if on { 0.0 } else { 1.0 });
    }
}

/// U+25C0 / U+25B6 at the Small pill's 11 px type.
fn tri(p: &egui::Painter, c: egui::Pos2, left: bool, col: egui::Color32) {
    let s = if left { -1.0 } else { 1.0 };
    p.add(egui::Shape::convex_polygon(vec![c + vec2(3.5 * s, 0.0), c + vec2(-3.0 * s, -4.0), c + vec2(-3.0 * s, 4.0)], col, Stroke::NONE));
}

/// `PresetNameButton`: the name letter-spaced, " *" in the accent when edited, the accent
/// border on hover; a click opens the browser.
pub(crate) fn preset_name(ui: &mut Ui, rect: Rect, pal: &VstPalette, name: &str, dirty: bool) -> Response {
    let resp = ui.interact(rect, ui.id().with("fam-preset-name"), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Preset {name}")));
    let p = ui.painter();
    let b = rect.shrink(0.5);
    p.rect_filled(b, CornerRadius::same(3), pal.panel);
    p.rect_stroke(b, CornerRadius::same(3), Stroke::new(1.0, if resp.hovered() { pal.accent } else { pal.border }), StrokeKind::Middle);
    let name_w = tracked_width(p, name, 11.0, 0.08, false);
    let star_w = if dirty { tracked_width(p, " *", 11.0, 0.08, false) } else { 0.0 };
    let w = (b.width() - 8.0).min(name_w + star_w).max(0.0);
    let row = Rect::from_center_size(b.center(), vec2(w, b.height()));
    let name_r = Rect::from_min_size(row.min, vec2((row.width() - star_w).max(0.0), row.height()));
    let clip = p.with_clip_rect(name_r.intersect(p.clip_rect()));
    tracked_text(&clip, name_r, Align2::LEFT_CENTER, name, 11.0, 0.08, false, false, pal.text_bright);
    if dirty {
        let star_r = Rect::from_min_max(egui::pos2(name_r.right(), row.top()), row.max);
        tracked_text(p, star_r, Align2::LEFT_CENTER, " *", 11.0, 0.08, false, false, pal.accent);
    }
    resp
}

// ---------------------------------------------------------------------------- the browser

/// `ui.mix.drawer {tab}` (scripts and tests): "Presets" opens the browser; any other tab for this
/// module is handed back to the editor.
pub(crate) fn take_drawer_tab(app: &mut SoundApp, host: &Host, st: &mut FamilyState) -> Option<String> {
    let (_, _, tab) = app.ui.drawer_tab.take_if(|(t, m, _)| *t == host.track && *m == host.ins.id)?;
    if tab == "Presets" {
        presets::open(app, host, &mut st.presets);
        return None;
    }
    Some(tab)
}

/// The full-panel browser while it is open (true: it covered the editor this frame). Recall is
/// one undo step (pushed on the editor's own Undo first where [`FamilyState::undo_on_load`]);
/// Copy and Paste are [`presets::clipboard`].
pub(crate) fn browser(app: &mut SoundApp, ui: &mut Ui, host: &Host, pal: &VstPalette, st: &mut FamilyState, fam: &Family, tags: &str) -> bool {
    if !st.presets.open {
        return false;
    }
    st.presets.tags_out = tags.to_string();
    st.presets.not_in_presets = fam.not_in_presets;
    let out: BrowserOut = presets::preset_browser(app, ui, host, host.rect, pal, &mut st.presets);
    let pasted = presets::clipboard(app, ui, host, &mut st.presets, &out);
    if out.recalled.is_some() {
        end_gesture(app);
    }
    // `host` still holds the values from before the recall: the snapshot `pushUndo` takes.
    if st.undo_on_load && (out.recalled.is_some() || pasted.is_some()) {
        push_undo(st, host, fam);
    }
    true
}

// ---------------------------------------------------------------------------- more attachments

/// `attachDial` and `attachSteppedDial`: [`dial_param`], but a choice parameter (the Note knobs)
/// rides the dial as its index over `steps - 1`, rounded on the way back.
#[allow(clippy::too_many_arguments)]
pub(crate) fn dial_attached(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    fam: &Family,
    rect: Rect,
    pal: &VstPalette,
    id: &str,
    o: &DialOpts,
) -> Response {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else {
        return ui.interact(rect, ui.id().with(("vst-dial-missing", id)), Sense::hover());
    };
    let Some(span) = (!info.choices.is_empty()).then(|| (info.choices.len().max(2) - 1) as f32) else {
        let o = DialOpts { default: info.to_normalized(info.default), ..*o };
        return dial_param(app, ui, host, st, fam, rect, pal, id, &o);
    };
    let value = param(host, id);
    let o = DialOpts { default: info.default / span, ..*o };
    let out = dial(ui, rect, id, pal, value / span, &o);
    if out.started {
        end_gesture(app);
        push_undo(st, host, fam);
    }
    if let Some(v) = out.value {
        let real = info.clamp((v * span).round());
        if (real - value).abs() > f32::EPSILON {
            set_params_merged(app, host, &[(id, real)], &format!("dial-{id}"));
        }
    }
    if out.ended {
        end_gesture(app);
    }
    out.response
}

/// `attachSegmented`: a Segmented over choice parameter `id` (its labels unless `options` names
/// them); a pick pushes Undo and is one undo step. Returns the response's id (for a tooltip).
#[allow(clippy::too_many_arguments)]
pub(crate) fn segmented_param(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    fam: &Family,
    rect: Rect,
    pal: &VstPalette,
    id: &str,
    options: Option<&[&str]>,
    rows: usize,
    style: super::controls::SelectionStyle,
    tracked: bool,
) {
    let Some(info) = host.info.params.iter().find(|p| p.id == id) else { return };
    let opts = options.unwrap_or(info.choices);
    let sel = param(host, id).round().max(0.0) as usize;
    if let Some(i) = super::controls::segmented_ex(ui, rect, id, pal, opts, rows, Some(sel), style, tracked) {
        set(app, host, st, fam, id, info.clamp(i as f32));
    }
}

/// `attachPill`: a Small pill lit while toggle `id` is on; a click pushes Undo and flips it.
#[allow(clippy::too_many_arguments)]
pub(crate) fn pill_param(
    app: &mut SoundApp,
    ui: &mut Ui,
    host: &Host,
    st: &mut FamilyState,
    fam: &Family,
    rect: Rect,
    pal: &VstPalette,
    id: &str,
    text: &str,
    look: PillLook,
) -> Response {
    let on = param(host, id) > 0.5;
    let resp = pill_ex(ui, rect, ("pill", id), pal, text, on, PillStyle::Small, look);
    if resp.clicked() {
        set(app, host, st, fam, id, if on { 0.0 } else { 1.0 });
    }
    resp
}

/// The `◀` / `▶` stepper pill (the UI font has neither glyph).
pub(crate) fn arrow_pill(ui: &mut Ui, r: Rect, salt: impl std::hash::Hash + std::fmt::Debug, pal: &VstPalette, left: bool) -> Response {
    let resp = pill_ex(ui, r, salt, pal, "", false, PillStyle::Small, plain());
    let ink = if resp.hovered() { pal.text_bright } else { pal.text_dim };
    tri(ui.painter(), r.center(), left, ink);
    resp
}

/// DelayEngine's and ReverbEngine's header row (`resized`): the preset strip centred,
/// `min (width - 360, 420)` wide (◀ 24, 5, name, 5, ▶ 24), and right to left Bypass 62, B 26,
/// A 26, Undo 48 with 6 / 3 / 6 gaps, all `Pill::Small` high.
pub(crate) fn engine_header_rects(header: Rect) -> HeaderRects {
    let pill_h = super::controls::pill_height(PillStyle::Small);
    let mid_w = (header.width() - 360.0).clamp(0.0, 420.0);
    let centred = Rect::from_center_size(header.center(), vec2(mid_w, pill_h));
    let at_right = |right: f32, w: f32| Rect::from_min_size(egui::pos2(right - w, header.center().y - pill_h * 0.5), vec2(w, pill_h));
    let mut x = header.right();
    let bypass = at_right(x, 62.0);
    x -= 62.0 + 6.0;
    let b = at_right(x, 26.0);
    x -= 26.0 + 3.0;
    let a = at_right(x, 26.0);
    x -= 26.0 + 6.0;
    let undo = at_right(x, 48.0);
    let prev = Rect::from_min_size(centred.min, vec2(24.0, pill_h));
    let next = Rect::from_min_size(egui::pos2(centred.right() - 24.0, centred.top()), vec2(24.0, pill_h));
    let name = Rect::from_min_max(egui::pos2(prev.right() + 5.0, centred.top()), egui::pos2(next.left() - 5.0, centred.bottom()));
    HeaderRects { prev, name, next, undo, a, b, bypass }
}

/// The header as SatEngine, TapeEngine, TransientEngine and VintEngine lay it out (`resized()`):
/// Bypass, B, A (each at least 28 wide), Undo placed from the right (6 apart, 2 more before
/// Undo), then the preset strip (24-wide arrows, the name 140..230) centred in what is left.
pub(crate) fn right_aligned_header(ui: &Ui, header: Rect) -> HeaderRects {
    let keep_centre = |r: Rect, w: f32, h: f32| {
        Rect::from_min_size(egui::pos2(r.left() + ((r.width() - w) / 2.0).trunc(), r.top() + ((r.height() - h) / 2.0).trunc()), vec2(w, h))
    };
    let btn_h = super::controls::pill_height(PillStyle::Small);
    let mut right = header;
    let place_right = |right: &mut Rect, text: &str, min_w: f32| {
        let w = min_w.max(small_pill_w(ui, text).trunc());
        let col = Rect::from_min_max(egui::pos2(right.right() - w, right.top()), right.max);
        right.max.x -= w + 6.0;
        keep_centre(col, w, btn_h)
    };
    let bypass = place_right(&mut right, "Bypass", 0.0);
    let b = place_right(&mut right, "B", 28.0);
    let a = place_right(&mut right, "A", 28.0);
    right.max.x -= 2.0;
    let undo = place_right(&mut right, "Undo", 0.0);
    let arrow_w = 24.0;
    let name_w = (right.width() - 2.0 * arrow_w - 12.0).clamp(140.0, 230.0);
    let mut strip = keep_centre(right, name_w + 2.0 * arrow_w + 12.0, header.height());
    let prev = keep_centre(Rect::from_min_size(strip.min, vec2(arrow_w, strip.height())), arrow_w, 22.0);
    strip.min.x += arrow_w + 6.0;
    let next = keep_centre(Rect::from_min_max(egui::pos2(strip.right() - arrow_w, strip.top()), strip.max), arrow_w, 22.0);
    strip.max.x -= arrow_w + 6.0;
    let name = keep_centre(strip, strip.width(), 22.0);
    HeaderRects { prev, name, next, undo, a, b, bypass }
}

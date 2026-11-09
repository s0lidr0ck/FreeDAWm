//! Juke — Drum Machine in its instrument window (look approved 2026-11-06, `docs/juke.md` §5).
//!
//! * **Kit view:** the pads as channel strips (a header band in the pad's family colour, Tune and
//!   Pan, the sends, a fader with a meter, audition, choke, solo and mute), Bus A, Bus B and the
//!   Output as strips at the side, and the trigger-note honeycomb underneath.
//! * **Pad edit view:** one pad at full size: its envelopes, sample layers, synth voice and
//!   playback options, with its own strip beside the buses.
//!
//! Every edit goes through a command (`mix.instrument_param`, `mix.kit_*`), so it undoes and
//! scripts like any other.

use crate::SoundApp;
use crate::theme::{bold, mono, regular};
use crate::widgets::{self, KnobStyle};
use egui::{Align2, Color32, Rect, Sense, Shape, Stroke, StrokeKind, Ui, Vec2, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::ParamInfo;
use soundcraft_dsp::plugins::juke::{JUKE_INFO, LAYERS, PADS, ROBINS, SOURCES, VOICE_NAMES, sample_key, unpack_levels};
use soundcraft_engine::cmd::juke::pad_layout;
use soundcraft_model::{ClipId, DrumFamily, Insert, KitBus, KitFx, TrackId};

pub use soundcraft_engine::cmd::juke::JUKE;

// The approved palette.
const GROUND: Color32 = Color32::from_rgb(0x2A, 0x24, 0x1F);
const TOP_BAR: Color32 = Color32::from_rgb(0x22, 0x1D, 0x19);
const CARD_TOP: Color32 = Color32::from_rgb(0x54, 0x48, 0x3C);
const CARD_BOTTOM: Color32 = Color32::from_rgb(0x46, 0x3C, 0x33);
const EDGE: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);
const CREAM: Color32 = Color32::from_rgb(0xF6, 0xEE, 0xDF);
const LABEL: Color32 = Color32::from_rgb(0xD9, 0xCC, 0xB6);
const AMBER: Color32 = Color32::from_rgb(0xF0, 0xB2, 0x4A);
const DARK_TEXT: Color32 = Color32::from_rgb(0x1E, 0x19, 0x15);
const SLATE_TOP: Color32 = Color32::from_rgb(0x4B, 0x59, 0x58);
const SLATE_BOTTOM: Color32 = Color32::from_rgb(0x3F, 0x4B, 0x4A);
const BRASS_TOP: Color32 = Color32::from_rgb(0x66, 0x55, 0x3A);
const BRASS_BOTTOM: Color32 = Color32::from_rgb(0x57, 0x48, 0x30);

/// Pad cards' size and the strips' width.
const CARD: Vec2 = vec2(206.0, 198.0);
const STRIP_W: f32 = 92.0;
const PAD_CAP: Vec2 = vec2(18.0, 30.0);
const STRIP_CAP: Vec2 = vec2(26.0, 48.0);
/// Lowest and highest note on the honeycomb: C1 to B5 (middle C is C3).
const HONEY_LOW: u8 = 36;
const HONEY_HIGH: u8 = 95;

/// A drum family's colour (pad headers and trigger cells share it).
pub fn family_color(f: DrumFamily) -> Color32 {
    match f {
        DrumFamily::Kick => Color32::from_rgb(0xE8, 0x70, 0x4F),
        DrumFamily::Snare => Color32::from_rgb(0xF2, 0xB5, 0x44),
        DrumFamily::Clap => Color32::from_rgb(0xEC, 0x8F, 0xB0),
        DrumFamily::Hat => Color32::from_rgb(0x62, 0xC6, 0xC8),
        DrumFamily::Tom => Color32::from_rgb(0xAE, 0x92, 0xE6),
        DrumFamily::Cymbal => Color32::from_rgb(0x7F, 0xA9, 0xEE),
        DrumFamily::Percussion => Color32::from_rgb(0x9E, 0xD2, 0x7C),
    }
}

/// A family's name as the menus show it.
pub fn family_name(f: DrumFamily) -> &'static str {
    match f {
        DrumFamily::Kick => "Kick",
        DrumFamily::Snare => "Snare",
        DrumFamily::Clap => "Clap",
        DrumFamily::Hat => "Hat",
        DrumFamily::Tom => "Tom",
        DrumFamily::Cymbal => "Cymbal",
        DrumFamily::Percussion => "Percussion",
    }
}

/// A pad parameter's id (`p3_tune`), pads from 0.
pub fn pad_param(pad: usize, name: &str) -> String {
    format!("p{}_{name}", pad + 1)
}

/// A pad parameter's name without its pad (`Pad 3 Send A` → `Send A`).
pub fn short_name(name: &'static str) -> &'static str {
    name.strip_prefix("Pad ").and_then(|rest| rest.split_once(' ')).map_or(name, |(_, n)| n)
}

fn info(id: &str) -> Option<&'static ParamInfo> {
    JUKE_INFO.param(id)
}

fn value(ins: &Insert, id: &str) -> f32 {
    let default = info(id).map_or(0.0, |p| p.default);
    ins.params.get(id).copied().unwrap_or(default)
}

/// Pad `pad`'s parameter `name` (its default when the kit has not set it).
pub fn pad_value(ins: &Insert, pad: usize, name: &str) -> f32 {
    value(ins, &pad_param(pad, name))
}

/// The pad a note triggers (the first, as the plugin plays every pad on a note).
pub fn pad_for_note(ins: &Insert, note: u8) -> Option<usize> {
    (0..PADS).find(|&p| pad_value(ins, p, "note").round() as i64 == i64::from(note))
}

/// The pad's trigger note.
pub fn pad_note(ins: &Insert, pad: usize) -> u8 {
    pad_value(ins, pad, "note").round().clamp(0.0, 127.0) as u8
}

/// The Juke window's views: the kit's pads, the selected pad's editor, the pattern's steps.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Tab {
    #[default]
    Kit,
    Pad,
    Steps,
}

/// Which pad is selected, per track.
#[derive(Debug, Clone, Copy, Default)]
struct View {
    pad: usize,
    /// The pad being auditioned while the pointer holds its play button.
    held: Option<(usize, u8)>,
}

fn view_id(t: TrackId) -> egui::Id {
    egui::Id::new(("juke-view", t.0))
}

/// The pad a dragged pad header carries to the honeycomb.
#[derive(Debug, Clone, Copy)]
struct PadDrag(usize);

/// What the window asked for this frame, done once drawing is over.
#[derive(Default)]
struct Out {
    params: Vec<(String, f32)>,
    cmds: Vec<(&'static str, Value)>,
    select: Option<usize>,
    /// The view to show.
    tab: Option<Tab>,
    /// (pad, layer, robin) to load a sample into from a file picker.
    load: Option<(usize, usize, usize)>,
    /// Press (`Some(note)`) or release (`None`) of the audition button.
    audition: Option<Option<(usize, u8)>>,
    /// Places a dropped audio file loads into: (where, (pad, layer, robin)).
    drops: Vec<(Rect, (usize, usize, usize))>,
    /// Places dropped drum takes are imported: a pad (`Some`) or the whole kit (`None`).
    imports: Vec<(Rect, Option<usize>)>,
    /// Import takes picked from a file dialog into this pad.
    import_pad: Option<usize>,
    /// Whether the session has an edit selection (for From Selection).
    has_selection: bool,
    /// Import the selected range of an audio track into this pad.
    from_selection: Option<usize>,
    /// Build the kit from a folder the user picks.
    #[cfg(not(target_arch = "wasm32"))]
    import_folder: bool,
    /// A kit effect whose editor to open.
    open_fx: Option<soundcraft_model::ModuleId>,
    /// Show the Steps view on the track's pattern clip (made if there is none).
    open_steps: bool,
    /// Show the Steps view on this pattern clip.
    steps_clip: Option<ClipId>,
    /// A pattern was just made (`pattern.new`): show it.
    new_pattern: bool,
    /// Copy this pattern clip right after itself (`edit.duplicate`) and show the copy.
    duplicate_pattern: Option<ClipId>,
    /// Recorded kits: fetch the release's list, download one, cancel the download.
    #[cfg(not(target_arch = "wasm32"))]
    fetch_kits: bool,
    #[cfg(not(target_arch = "wasm32"))]
    download_kit: Option<soundcraft_engine::ml::kits::KitEntry>,
    #[cfg(not(target_arch = "wasm32"))]
    cancel_kit: bool,
    /// Save the kit as a preset in the presets folder.
    #[cfg(not(target_arch = "wasm32"))]
    save_preset: bool,
    /// Write the kit to a folder the user picks.
    #[cfg(not(target_arch = "wasm32"))]
    export_kit: bool,
}

#[cfg(not(target_arch = "wasm32"))]
/// What the Kits menu shows of the recorded kits.
struct KitsView {
    /// (name, folder) of each installed kit.
    installed: Vec<(String, std::path::PathBuf)>,
    /// The installed kits' ids, and their folder.
    ids: Vec<String>,
    kits_dir: std::path::PathBuf,
    presets_dir: std::path::PathBuf,
    /// The release's kits not installed yet, once listed; or why the list is not there.
    available: Result<Vec<soundcraft_engine::ml::kits::KitEntry>, String>,
    /// The download running: (kit id, received, expected bytes).
    active: Option<(String, u64, u64)>,
    note: String,
}

#[cfg(not(target_arch = "wasm32"))]
fn kits_view(app: &mut SoundApp) -> KitsView {
    use crate::juke_kits_ui::ListState;
    app.juke_kits.step();
    let installed: Vec<_> = app.juke_kits.installed();
    let have: Vec<&str> = installed.iter().map(|(id, _, _)| id.as_str()).collect();
    let available = match &app.juke_kits.list {
        ListState::NotLoaded => Err(String::new()),
        ListState::Loading(_) => Err("Checking for kits…".into()),
        ListState::Failed(e) => Err(format!("Kits list: {e}")),
        ListState::Ready(m) => Ok(m.kits.iter().filter(|k| !have.contains(&k.id.as_str())).cloned().collect()),
    };
    let active = app.juke_kits.active.as_ref().map(|a| {
        let (done, total) = a.progress.get();
        (a.id.clone(), done, total)
    });
    let ids = installed.iter().map(|(id, _, _)| id.clone()).collect();
    KitsView {
        installed: installed.into_iter().map(|(_, name, path)| (name, path)).collect(),
        ids,
        kits_dir: app.juke_kits.dir(),
        presets_dir: app.juke_kits.presets_dir(),
        available,
        active,
        note: app.juke_kits.note.clone(),
    }
}

impl Out {
    fn set(&mut self, id: impl Into<String>, v: f32) {
        self.params.push((id.into(), v));
    }
}

/// The window's default size.
pub const WINDOW_SIZE: Vec2 = vec2(1000.0, 690.0);

/// Draws the Juke window's contents for track `tid`.
pub fn draw(app: &mut SoundApp, ui: &mut Ui, tid: TrackId, ins: &Insert, frozen: bool) {
    let ctx = ui.ctx().clone();
    let mut view: View = ctx.data(|d| d.get_temp(view_id(tid))).unwrap_or_default();
    if let Some((t, pad)) = app.ui.juke_pad_select
        && t == tid
    {
        view.pad = pad;
        app.ui.juke_pad_select = None;
    }
    view.pad = view.pad.min(PADS - 1);
    let (levels, bus_levels) = pad_levels(app, &ctx, tid);
    #[cfg(not(target_arch = "wasm32"))]
    let kits = kits_view(app);
    #[cfg(not(target_arch = "wasm32"))]
    if kits.active.is_some() || kits.available.as_ref().is_err_and(|e| e.starts_with("Checking")) {
        ctx.request_repaint_after(std::time::Duration::from_millis(200));
    }
    #[cfg(target_arch = "wasm32")]
    let kits = ();
    let mut out = Out { has_selection: !app.engine.session().edit.selection.is_empty(), ..Out::default() };
    let full = ui.available_rect_before_wrap();
    let full = Rect::from_min_size(full.min, vec2(full.width().max(760.0), full.height().max(520.0)));
    ui.allocate_rect(full, Sense::hover());
    // Drum takes dropped anywhere in the window (not on a pad or a cell) build a kit.
    import_target(ui, full, None, &mut out);
    ui.painter().rect_filled(full, 4.0, GROUND);
    let mut body = ui.new_child(egui::UiBuilder::new().max_rect(full.shrink(6.0)));
    if frozen {
        body.disable();
    }
    let inner = full.shrink(6.0);
    let top = Rect::from_min_size(inner.min, vec2(inner.width(), 28.0));
    let tab = app.ui.juke_tabs.get(&tid).copied().unwrap_or_default();
    let steps_clip = steps_clip(app, tid);
    top_bar(&mut body, top, tid, ins, &view, tab, steps_clip, &kits, &mut out);
    let honey_h = 64.0;
    let strips_w = STRIP_W * 3.0 + 12.0;
    let main = Rect::from_min_max(pos2(inner.min.x, top.max.y + 6.0), pos2(inner.max.x - strips_w - 8.0, inner.max.y - honey_h - 8.0));
    let strips = Rect::from_min_max(pos2(main.max.x + 8.0, main.min.y), pos2(inner.max.x, main.max.y));
    let honey = Rect::from_min_max(pos2(inner.min.x, inner.max.y - honey_h), inner.max);
    let picker = app.ui.hit_picker.clone().filter(|p| p.track == tid);
    let mut asks = Vec::new();
    match tab {
        // The hit picker takes the strips' room, over whichever view is shown.
        _ if picker.is_some() => {
            let r = Rect::from_min_max(main.min, pos2(inner.max.x, main.max.y));
            if let Some(pk) = &picker {
                crate::hit_picker::draw(&mut body, r, pk, app.ui.hit_playing, &mut asks);
            }
        }
        Tab::Pad => pad_edit(&mut body, main, tid, ins, view.pad, levels[view.pad], &mut out),
        Tab::Kit => pad_grid(&mut body, main, tid, ins, &view, &levels, &mut out),
        Tab::Steps => {
            // The steps take the strips' room too.
            let r = Rect::from_min_max(main.min, pos2(inner.max.x, main.max.y));
            steps_view(app, &mut body, r, tid, steps_clip, &mut out);
        }
    }
    if tab != Tab::Steps && picker.is_none() {
        let out_meter = app.meters.get(&tid).map(|m| (m.level, m.hold, m.clip)).unwrap_or_default();
        bus_strips(&mut body, strips, tid, ins, out_meter, bus_levels, &mut out);
    }
    honeycomb(&mut body, honey, tid, ins, view.pad, &mut out);
    apply(app, tid, &mut view, out);
    crate::hit_picker::apply(app, asks);
    ctx.data_mut(|d| d.insert_temp(view_id(tid), view));
}

/// The pads' and the two buses' meter levels, falling back smoothly between the audio thread's
/// block peaks.
fn pad_levels(app: &SoundApp, ctx: &egui::Context, tid: TrackId) -> ([f32; PADS], [f32; 2]) {
    let (pads, buses) = app.meter_snapshot.tracks.get(&tid).map(|m| unpack_levels(&m.instrument)).unwrap_or(([0.0; PADS], [0.0; 2]));
    let mut now = [0.0f32; PADS + 2];
    now[..PADS].copy_from_slice(&pads);
    now[PADS..].copy_from_slice(&buses);
    let id = egui::Id::new(("juke-levels", tid.0));
    let mut shown: [f32; PADS + 2] = ctx.data(|d| d.get_temp(id)).unwrap_or([0.0; PADS + 2]);
    let dt = ctx.input(|i| i.stable_dt).clamp(0.0, 0.25);
    let fall = (-dt * 6.0).exp();
    let mut moving = false;
    for (s, n) in shown.iter_mut().zip(now) {
        *s = (*s * fall).max(n);
        if *s < 1.0e-4 {
            *s = 0.0;
        }
        moving |= *s > 0.0;
    }
    ctx.data_mut(|d| d.insert_temp(id, shown));
    if moving {
        ctx.request_repaint();
    }
    let mut pads = [0.0; PADS];
    pads.copy_from_slice(&shown[..PADS]);
    (pads, [shown[PADS], shown[PADS + 1]])
}

/// Remembers `r` as a place a dropped audio file loads into `slot`, outlined while files are
/// dragged over it.
fn drop_target(ui: &Ui, r: Rect, slot: (usize, usize, usize), out: &mut Out) {
    let r = r.intersect(ui.clip_rect());
    if !r.is_positive() {
        return;
    }
    out.drops.push((r, slot));
    if ui.ctx().input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p))) {
        ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("juke-drop"))).rect_stroke(
            r,
            4.0,
            Stroke::new(2.0, AMBER),
            StrokeKind::Inside,
        );
    }
}

/// Remembers `r` as a place dropped drum takes are imported: into `pad`, or (`None`) as a kit.
/// Outlined, with what a drop does, while files are dragged over it.
fn import_target(ui: &Ui, r: Rect, pad: Option<usize>, out: &mut Out) {
    let r = r.intersect(ui.clip_rect());
    if !r.is_positive() {
        return;
    }
    out.imports.push((r, pad));
    let over = ui.ctx().input(|i| !i.raw.hovered_files.is_empty() && i.pointer.latest_pos().is_some_and(|p| r.contains(p)));
    // The window's own outline only where no pad claims the pointer.
    let under_pad =
        pad.is_none() && out.imports.iter().any(|(pr, p)| p.is_some() && ui.ctx().input(|i| i.pointer.latest_pos().is_some_and(|q| pr.contains(q))));
    if over && !under_pad {
        let painter = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("juke-import")));
        painter.rect_stroke(r, 4.0, Stroke::new(2.0, AMBER), StrokeKind::Inside);
        let hint = match pad {
            Some(_) => "Drop takes: pick the hits for each layer, soft to hard",
            None => "Drop takes or a folder: a kit, each file to its pad by name",
        };
        let at = pos2(r.center().x, r.max.y - 14.0);
        let galley = painter.layout_no_wrap(hint.to_string(), regular(11.0), DARK_TEXT);
        painter.rect_filled(Rect::from_center_size(at, galley.size() + vec2(12.0, 6.0)), 4.0, AMBER);
        painter.galley(at - galley.size() / 2.0, galley, DARK_TEXT);
    }
}

fn apply(app: &mut SoundApp, tid: TrackId, view: &mut View, out: Out) {
    app.kit_drop_targets.extend(out.drops.iter().map(|(r, slot)| (*r, tid, *slot)));
    // The window first, so a pad (registered after it) is found before it.
    app.kit_import_targets.extend(out.imports.iter().map(|(r, pad)| (*r, tid, *pad)));
    if let Some(pad) = out.from_selection {
        crate::hit_picker::open(app, tid, pad, json!({"selection": true}));
    }
    if let Some(pad) = out.import_pad
        && let Some(pick) = &app.services.pick_open
        && let Some(path) = pick("mix.kit_pad_import", crate::sampler_window::AUDIO_EXTS)
    {
        crate::hit_picker::open(app, tid, pad, json!({"paths": [path]}));
    }
    #[cfg(not(target_arch = "wasm32"))]
    if out.import_folder
        && let Some(pick) = &app.services.pick_folder
        && let Some(dir) = pick("mix.kit_import")
    {
        let r = app.run("mix.kit_import", json!({"track": tid.0, "folder": dir}));
        app.ui.status = kit_import_status(r);
    }
    for (id, v) in out.params {
        if let Err(e) = app.engine.execute_merged(
            "mix.instrument_param",
            &json!({"track": tid.0, "param": id, "value": v}),
            &format!("instrument_param:{}:{}", tid.0, id),
        ) {
            app.ui.status = e.to_string();
        }
    }
    for (id, p) in out.cmds {
        match app.run(id, p) {
            Ok(v) if id == "pattern.new" && out.new_pattern => {
                if let Some(c) = v["clip"].as_u64() {
                    app.ui.juke_clips.insert(tid, ClipId(c));
                }
            }
            Ok(_) => {}
            Err(e) => app.ui.status = e,
        }
    }
    if let Some(p) = out.select {
        view.pad = p.min(PADS - 1);
    }
    if let Some(m) = out.open_fx {
        crate::juke_fx::open(app, tid, m);
    }
    if out.open_steps {
        show_steps(app, tid, out.steps_clip);
    } else if let Some(c) = out.steps_clip {
        app.ui.juke_clips.insert(tid, c);
    }
    if let Some(c) = out.duplicate_pattern {
        duplicate_pattern(app, tid, c);
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        if out.fetch_kits {
            app.juke_kits.load_list();
        }
        if let Some(k) = &out.download_kit {
            app.juke_kits.download(k);
        }
        if out.cancel_kit {
            app.juke_kits.cancel();
        }
        if out.save_preset {
            save_preset(app, tid);
        }
        if out.export_kit
            && let Some(pick) = &app.services.pick_folder
            && let Some(dir) = pick("mix.kit_save")
        {
            let name = app
                .engine
                .session()
                .track(tid)
                .and_then(|t| t.instrument.as_ref())
                .and_then(|i| i.kit.as_ref())
                .map_or_else(|| "My Kit".to_string(), |k| k.name.clone());
            let path = std::path::Path::new(&dir).join(saved_kit_id(&name).trim_start_matches("my_"));
            match app.run("mix.kit_save", json!({"track": tid.0, "path": path.to_string_lossy()})) {
                Ok(_) => app.ui.status = format!("Exported {name} to {}", path.display()),
                Err(e) => app.ui.status = e,
            }
        }
    }
    if let Some(t) = out.tab {
        app.ui.juke_tabs.insert(tid, t);
    }
    if let Some((pad, layer, robin)) = out.load
        && let Some(pick) = &app.services.pick_open
        && let Some(path) = pick("mix.kit_sample_load", crate::sampler_window::AUDIO_EXTS)
    {
        let r = app.run("mix.kit_sample_load", json!({"track": tid.0, "pad": pad + 1, "layer": layer + 1, "robin": robin + 1, "path": path}));
        match r {
            Ok(v) => app.ui.status = format!("{} loaded into pad {}", v.get("name").and_then(Value::as_str).unwrap_or("Sample"), pad + 1),
            Err(e) => app.ui.status = e,
        }
    }
    if let Some(press) = out.audition {
        if let Some((_, note)) = view.held.take()
            && let Some(p) = &app.player
        {
            p.audition(tid, note, 0);
        }
        if let Some((pad, note)) = press
            && let Some(p) = &app.player
        {
            p.audition(tid, note, 110);
            view.held = Some((pad, note));
        }
    }
}

/// A kit's folder name in the kits folder: `my_` and its name in lower case, letters and digits.
pub fn saved_kit_id(name: &str) -> String {
    let mut slug = String::from("my_");
    for c in name.chars() {
        if c.is_ascii_alphanumeric() {
            slug.push(c.to_ascii_lowercase());
        } else if !slug.ends_with('_') {
            slug.push('_');
        }
    }
    let slug = slug.trim_end_matches('_');
    slug.chars().take(64).collect::<String>().trim_end_matches('_').to_string()
}

/// Saves track `tid`'s kit as a preset in the presets folder, where the Presets menu lists it.
#[cfg(not(target_arch = "wasm32"))]
fn save_preset(app: &mut SoundApp, tid: TrackId) {
    let name = app
        .engine
        .session()
        .track(tid)
        .and_then(|t| t.instrument.as_ref())
        .and_then(|i| i.kit.as_ref())
        .map_or_else(|| "My Kit".to_string(), |k| k.name.clone());
    let id = saved_kit_id(&name);
    let id = if id == "my" { "my_kit".to_string() } else { id };
    let file = app.juke_kits.presets_dir().join(format!("{id}.json"));
    let kits = app.juke_kits.dir();
    match app.run("mix.kit_preset_save", json!({"track": tid.0, "path": file.to_string_lossy(), "name": name, "kits_dir": kits.to_string_lossy()})) {
        Ok(_) => app.ui.status = format!("Saved the preset {name} (Presets › My Presets)"),
        Err(e) => app.ui.status = e,
    }
}

/// Copies pattern clip `cid` right after itself (`edit.duplicate` on its range) and shows the copy
/// in the Steps view.
fn duplicate_pattern(app: &mut SoundApp, tid: TrackId, cid: ClipId) {
    let Some(range) = app.engine.session().find_clip(cid).map(|(_, c)| c.range()) else { return };
    if app.run("edit.duplicate", json!({"tracks": [tid.0], "start": range.start, "end": range.end})).is_err() {
        return;
    }
    let copy = app
        .engine
        .session()
        .track(tid)
        .and_then(|t| t.clips().iter().find(|c| c.pattern.is_some() && c.start == range.end && c.id != cid).map(|c| c.id));
    if let Some(c) = copy {
        app.ui.juke_clips.insert(tid, c);
    }
}

/// The pattern clip track `tid`'s Steps view edits: the one last shown while it is still there,
/// else the one at the insertion point, else the first.
pub fn steps_clip(app: &SoundApp, tid: TrackId) -> Option<ClipId> {
    let t = app.engine.session().track(tid)?;
    let patterns = || t.clips().iter().filter(|c| c.pattern.is_some());
    if let Some(c) = app.ui.juke_clips.get(&tid)
        && patterns().any(|p| p.id == *c)
    {
        return Some(*c);
    }
    let at = app.engine.session().edit.selection.start;
    patterns().find(|c| c.range().contains(at)).or_else(|| patterns().next()).map(|c| c.id)
}

/// Opens track `tid`'s Juke window on its Steps view, editing `clip` (else the track's pattern clip,
/// one made at the insertion point if it has none).
pub fn show_steps(app: &mut SoundApp, tid: TrackId, clip: Option<ClipId>) {
    let clip = clip
        .or_else(|| steps_clip(app, tid))
        .or_else(|| app.run("pattern.new", json!({"track": tid.0, "bars": 2})).ok().and_then(|v| v["clip"].as_u64()).map(ClipId));
    if let Some(c) = clip {
        app.ui.juke_clips.insert(tid, c);
    }
    app.ui.juke_tabs.insert(tid, Tab::Steps);
    let key = soundcraft_mix::SlotKey::Instrument;
    if !app.ui.plugin_windows.contains(&(tid, key)) {
        app.ui.plugin_windows.push((tid, key));
    }
}

/// The Steps view: the track's pattern clips as tabs (and a new one), then the chosen clip's
/// step grid.
fn steps_view(app: &mut SoundApp, ui: &mut Ui, r: Rect, tid: TrackId, current: Option<ClipId>, out: &mut Out) {
    let s = app.engine.session();
    let sr = s.sample_rate;
    let mut clips: Vec<(ClipId, String, i64, i64, i64)> = s
        .track(tid)
        .map(|t| {
            t.clips()
                .iter()
                .filter(|c| c.pattern.is_some())
                .map(|c| {
                    let a = s.tempo.bar_beat_at(c.start, sr).bar;
                    let t0 = s.tempo.samples_to_ticks(c.start, sr);
                    let t1 = s.tempo.samples_to_ticks(c.range().end, sr).saturating_sub(1).max(t0);
                    let end = s.tempo.bar_beat_at_tick(t1).bar;
                    (c.id, c.name.clone(), a, end, c.range().end)
                })
                .collect()
        })
        .unwrap_or_default();
    clips.sort_by_key(|c| c.2);
    let tabs = Rect::from_min_size(r.min, vec2(r.width(), 26.0));
    let p = ui.painter().clone();
    let mut x = tabs.min.x;
    for (id, name, a, b, _) in &clips {
        let label = if a == b { format!("{name} · {a}") } else { format!("{name} · {a}–{b}") };
        let w = (p.layout_no_wrap(label.clone(), bold(11.0), LABEL).size().x + 20.0).clamp(60.0, 200.0);
        if x + w > tabs.max.x - 300.0 {
            break;
        }
        let tr = Rect::from_min_size(pos2(x, tabs.min.y), vec2(w, tabs.height()));
        let on = current == Some(*id);
        if chip(ui, tr, egui::Id::new(("juke-steps-clip", tid.0, id.0)), &label, on, AMBER)
            .on_hover_text(format!("Edit {name} (bars {a}–{b})"))
            .clicked()
            && !on
        {
            out.steps_clip = Some(*id);
        }
        x += w + 4.0;
    }
    let new_r = Rect::from_min_size(pos2(x, tabs.min.y), vec2(84.0, tabs.height()));
    if chip(ui, new_r, egui::Id::new(("juke-steps-new", tid.0)), "+ Pattern", false, AMBER)
        .on_hover_text("A new two-bar pattern after the track's last one")
        .clicked()
    {
        let mut p = json!({"track": tid.0, "bars": 2});
        if let Some(end) = clips.iter().map(|c| c.4).max() {
            p["start"] = json!(end);
        }
        out.cmds.push(("pattern.new", p));
        out.new_pattern = true;
    }
    let Some(cid) = current else {
        text(
            &p,
            r.center(),
            Align2::CENTER_CENTER,
            "No pattern on this track yet: + Pattern makes one, or pick a groove from Grooves.",
            regular(13.0),
            LABEL,
        );
        return;
    };
    // Duplicate: a copy of this pattern right after it, to go on from (the hats come in, a fill).
    let dup_r = Rect::from_min_size(pos2(new_r.max.x + 4.0, tabs.min.y), vec2(76.0, tabs.height()));
    if chip(ui, dup_r, egui::Id::new(("juke-steps-dup", tid.0)), "Duplicate", false, AMBER)
        .on_hover_text("A copy of this pattern right after it, with the same steps: change the copy for the next part of the song")
        .clicked()
    {
        out.duplicate_pattern = Some(cid);
    }
    let Some(clip) = app.engine.session().find_clip(cid).map(|(_, c)| c.clone()) else { return };
    let meter = s_meter(app, &clip);
    if let Some((_, _, a, b, _)) = clips.iter().find(|c| c.0 == cid) {
        let bars = if a == b { format!("bar {a}") } else { format!("bars {a}–{b}") };
        text(&p, pos2(tabs.max.x - 4.0, tabs.center().y), Align2::RIGHT_CENTER, format!("Pattern · {bars} · {meter}"), regular(11.0), LABEL);
    }
    let grid = Rect::from_min_max(pos2(r.min.x, tabs.max.y + 6.0), r.max);
    crate::step_grid::show(app, ui, grid, tid, cid, &clip);
}

/// The clip's time signature where it starts (`4/4`).
fn s_meter(app: &SoundApp, clip: &soundcraft_model::Clip) -> String {
    let s = app.engine.session();
    let t = s.tempo.samples_to_ticks(clip.start, s.sample_rate);
    let m = s.tempo.meter_at_tick(t);
    format!("{}/{}", m.numerator, m.denominator)
}

fn text(p: &egui::Painter, at: egui::Pos2, align: Align2, s: impl ToString, font: egui::FontId, c: Color32) -> Rect {
    p.text(at, align, s, font, c)
}

/// A small button painted on the surface: returns its response.
fn chip(ui: &mut Ui, r: Rect, id: egui::Id, label: &str, on: bool, color: Color32) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, ui.is_enabled(), on, label));
    let p = ui.painter();
    let fill = if on {
        color
    } else if resp.hovered() {
        Color32::from_black_alpha(70)
    } else {
        Color32::from_black_alpha(45)
    };
    p.rect_filled(r, 3.0, fill);
    p.rect_stroke(r, 3.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    text(p, r.center(), Align2::CENTER_CENTER, label, bold(10.0), if on { DARK_TEXT } else { LABEL });
    resp
}

/// An icon button on a header band.
fn icon_button(ui: &mut Ui, r: Rect, id: egui::Id, icon: &str, tip: &str) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click()).on_hover_text(tip);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, ui.is_enabled(), tip));
    if resp.hovered() {
        ui.painter().rect_filled(r, 3.0, Color32::from_black_alpha(40));
    }
    crate::icons::draw(ui.painter(), r.shrink(3.0), icon, DARK_TEXT);
    resp
}

/// A knob with its name and value, placed in `r`; its change goes to `out`.
fn knob_at(ui: &mut Ui, r: Rect, ins: &Insert, id: &str, size: f32, style: KnobStyle, out: &mut Out) {
    let Some(p) = info(id) else { return };
    let v = value(ins, id);
    let p = ParamInfo { name: short_name(p.name), ..p.clone() };
    let hero = matches!(style, KnobStyle::CreamHero);
    if hero {
        text(ui.painter(), pos2(r.center().x, r.min.y), Align2::CENTER_TOP, p.name.to_uppercase(), bold(9.5), LABEL);
    }
    let kr = if hero { Rect::from_min_max(pos2(r.min.x, r.min.y + 12.0), r.max) } else { r };
    let resp = ui.scope_builder(egui::UiBuilder::new().max_rect(kr).layout(egui::Layout::top_down(egui::Align::Center)), |ui| {
        ui.visuals_mut().override_text_color = Some(LABEL);
        widgets::knob_with_text(ui, &p, v, size, AMBER, style)
    });
    if let Some(x) = resp.inner.changed {
        out.set(id, x);
    }
}

#[cfg(not(target_arch = "wasm32"))]
type Recorded = KitsView;
#[cfg(target_arch = "wasm32")]
type Recorded = ();

#[allow(clippy::too_many_arguments)]
fn top_bar(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, view: &View, tab: Tab, steps_clip: Option<ClipId>, recorded: &Recorded, out: &mut Out) {
    let p = ui.painter();
    p.rect_filled(r, 4.0, TOP_BAR);
    text(p, pos2(r.min.x + 10.0, r.center().y), Align2::LEFT_CENTER, "JUKE", bold(14.0), AMBER);
    // The kit's name, edited in place.
    let kit = ins.kit.as_ref().map(|k| k.name.clone()).unwrap_or_default();
    let id = egui::Id::new(("juke-kit-name", tid.0));
    let mut name: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_else(|| kit.clone());
    let field = Rect::from_min_size(pos2(r.min.x + 58.0, r.min.y + 4.0), vec2(150.0, r.height() - 8.0));
    let resp = ui.put(field, egui::TextEdit::singleline(&mut name).hint_text("Kit name").font(bold(12.0)));
    if resp.has_focus() {
        ui.ctx().data_mut(|d| d.insert_temp(id, name.clone()));
    } else {
        ui.ctx().data_mut(|d| d.remove::<String>(id));
    }
    if resp.lost_focus() && name.trim() != kit && !name.trim().is_empty() {
        out.cmds.push(("mix.kit_name", json!({"track": tid.0, "name": name.trim()})));
    }
    // The starter kits: previous, next, and all of them.
    let kits = &soundcraft_engine::juke_kits::KITS;
    let at = kits.iter().position(|k| k.name == kit);
    let step = |d: isize| -> &'static str {
        let n = kits.len() as isize;
        let i = at.map_or(if d > 0 { 0 } else { n - 1 }, |i| (i as isize + d).rem_euclid(n));
        kits.get(i as usize).map_or("Porch Light", |k| k.name)
    };
    let prev = Rect::from_min_size(pos2(field.max.x + 4.0, field.min.y), vec2(20.0, field.height()));
    let next = prev.translate(vec2(22.0, 0.0));
    for (r, d, tip) in [(prev, -1, "Previous kit"), (next, 1, "Next kit")] {
        let resp = ui.interact(r, egui::Id::new(("juke-kit-step", tid.0, d)), Sense::click()).on_hover_text(tip);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, tip));
        let p = ui.painter();
        p.rect_filled(r, 3.0, if resp.hovered() { Color32::from_white_alpha(20) } else { Color32::from_white_alpha(8) });
        let c = r.center();
        let pts = if d < 0 {
            vec![pos2(c.x - 3.0, c.y), pos2(c.x + 3.0, c.y - 5.0), pos2(c.x + 3.0, c.y + 5.0)]
        } else {
            vec![pos2(c.x + 3.0, c.y), pos2(c.x - 3.0, c.y - 5.0), pos2(c.x - 3.0, c.y + 5.0)]
        };
        p.add(Shape::convex_polygon(pts, LABEL, Stroke::NONE));
        if resp.clicked() {
            out.cmds.push(("mix.kit_load", json!({"track": tid.0, "kit": step(d)})));
        }
    }
    let menu = Rect::from_min_size(pos2(next.max.x + 4.0, field.min.y), vec2(52.0, field.height()));
    let resp = ui.put(menu, egui::Button::new(egui::RichText::new("Kits").font(regular(11.0))));
    egui::Popup::menu(&resp).show(|ui| {
        let starters = soundcraft_engine::juke_kits::STARTER_KITS.min(kits.len());
        let mut item = |ui: &mut Ui, k: &soundcraft_engine::juke_kits::Kit| {
            let label = if k.rendered { format!("{} (rendered)", k.name) } else { k.name.to_string() };
            if ui.selectable_label(k.name == kit, label).clicked() {
                out.cmds.push(("mix.kit_load", json!({"track": tid.0, "kit": k.name})));
                ui.close();
            }
        };
        for k in &kits[..starters] {
            item(ui, k);
        }
        ui.menu_button("Styles", |ui| {
            for k in &kits[starters..] {
                item(ui, k);
            }
        });
        #[cfg(not(target_arch = "wasm32"))]
        recorded_menu(ui, tid, &kit, recorded, out);
        #[cfg(target_arch = "wasm32")]
        let _ = recorded;
    });
    // Presets: whole kit sounds (samples, tuning, envelopes, effects), by category.
    let presets = Rect::from_min_size(pos2(menu.max.x + 4.0, field.min.y), vec2(62.0, field.height()));
    let resp = ui.put(presets, egui::Button::new(egui::RichText::new("Presets").font(regular(11.0))));
    egui::Popup::menu(&resp).show(|ui| presets_menu(ui, tid, &kit, recorded, out));
    // Grooves: each style's kit and starter grooves, onto the track's pattern clip.
    let grooves = Rect::from_min_size(pos2(presets.max.x + 4.0, field.min.y), vec2(64.0, field.height()));
    let resp = ui.put(grooves, egui::Button::new(egui::RichText::new("Grooves").font(regular(11.0))));
    egui::Popup::menu(&resp).show(|ui| {
        let styles = soundcraft_engine::juke_styles::STYLES;
        let mut families: Vec<&str> = Vec::new();
        for s in styles {
            if !families.contains(&s.family) {
                families.push(s.family);
            }
        }
        for family in families {
            ui.menu_button(family, |ui| {
                for s in styles.iter().filter(|s| s.family == family) {
                    ui.menu_button(s.name, |ui| {
                        if ui.button(format!("Load the {} Kit", s.kit)).clicked() {
                            out.cmds.push(("mix.kit_load", json!({"track": tid.0, "kit": s.kit})));
                            ui.close();
                        }
                        ui.separator();
                        for g in s.grooves {
                            let resp = ui
                                .button(g.name)
                                .on_hover_text("Put this groove on the track's pattern clip at the insertion point (one is made if there is none)");
                            if resp.clicked() {
                                let mut p = json!({"style": s.name, "groove": g.name});
                                match steps_clip {
                                    Some(c) => p["clip"] = json!(c.0),
                                    None => p["track"] = json!(tid.0),
                                }
                                out.cmds.push(("pattern.groove", p));
                                out.open_steps = true;
                                ui.close();
                            }
                        }
                    });
                }
            });
        }
    });
    // The views: the kit's pads, the selected pad, the pattern's steps.
    let tabs = Rect::from_min_size(pos2(grooves.max.x + 16.0, r.min.y + 4.0), vec2(186.0, r.height() - 8.0));
    let w = tabs.width() / 3.0;
    let pad_name = pad_layout(ins, view.pad).name;
    let pad_tip = format!("Edit {pad_name}");
    let items: [(Tab, &str, &str); 3] = [
        (Tab::Kit, "Kit", "The kit's pads and buses"),
        (Tab::Pad, "Pad", pad_tip.as_str()),
        (Tab::Steps, "Steps", "The step sequencer: the track's patterns"),
    ];
    for (i, (t, label, tip)) in items.into_iter().enumerate() {
        let tr = Rect::from_min_size(pos2(tabs.min.x + i as f32 * w, tabs.min.y), vec2(w - 2.0, tabs.height()));
        if chip(ui, tr, egui::Id::new(("juke-tab", tid.0, i)), label, tab == t, AMBER).on_hover_text(tip).clicked() {
            if t == Tab::Steps {
                out.open_steps = true;
            } else {
                out.tab = Some(t);
            }
        }
    }
    // Randomize (the pad shown, else the kit) and Save kit.
    let rnd = Rect::from_min_size(pos2(tabs.max.x + 12.0, field.min.y), vec2(76.0, field.height()));
    let tip = if tab == Tab::Pad { format!("A new take on {pad_name}'s sound") } else { "A new take on every pad's sound".to_string() };
    if ui.put(rnd, egui::Button::new(egui::RichText::new("Randomize").font(regular(11.0)))).on_hover_text(tip).clicked() {
        let mut p = json!({"track": tid.0});
        if tab == Tab::Pad {
            p["pad"] = json!(view.pad + 1);
        }
        out.cmds.push(("mix.kit_randomize", p));
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let save = Rect::from_min_size(pos2(rnd.max.x + 4.0, field.min.y), vec2(80.0, field.height()));
        let resp = ui
            .put(save, egui::Button::new(egui::RichText::new("Save preset").font(regular(11.0))))
            .on_hover_text("Save the whole kit sound (samples, every setting, bus effects) as a preset under its name: Presets › My Presets");
        if resp.clicked() {
            out.save_preset = true;
        }
    }
    // Humanize, kit-wide.
    if let Some(info) = info("humanize") {
        let v = value(ins, "humanize");
        let x = r.max.x - 70.0;
        text(ui.painter(), pos2(x - 14.0, r.center().y), Align2::RIGHT_CENTER, "Humanize", regular(11.0), LABEL);
        let k = Rect::from_center_size(pos2(x, r.center().y), vec2(24.0, 24.0));
        let resp = ui.scope_builder(egui::UiBuilder::new().max_rect(k), |ui| widgets::knob(ui, info, v, 18.0, AMBER, KnobStyle::Cream));
        if let Some(nv) = resp.inner.changed {
            out.set("humanize", nv);
        }
        text(ui.painter(), pos2(x + 16.0, r.center().y), Align2::LEFT_CENTER, info.format(v), mono(11.0), CREAM);
    }
}

fn pad_grid(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, view: &View, levels: &[f32; PADS], out: &mut Out) {
    let cols = ((r.width() + 8.0) / (CARD.x + 8.0)).floor().clamp(1.0, 4.0) as usize;
    let rows = PADS.div_ceil(cols);
    let mut child = ui.new_child(egui::UiBuilder::new().max_rect(r));
    egui::ScrollArea::vertical().id_salt(("juke-pads", tid.0)).auto_shrink([false, false]).show(&mut child, |ui| {
        let (area, _) = ui.allocate_exact_size(vec2(cols as f32 * (CARD.x + 8.0) - 8.0, rows as f32 * (CARD.y + 8.0) - 8.0), Sense::hover());
        for (pad, &level) in levels.iter().enumerate() {
            let (row, col) = (pad / cols, pad % cols);
            let card = Rect::from_min_size(area.min + vec2(col as f32 * (CARD.x + 8.0), row as f32 * (CARD.y + 8.0)), CARD);
            if ui.is_rect_visible(card) {
                pad_card(ui, card, tid, ins, pad, view, level, out);
            }
        }
    });
}

/// The family menu and the pad's sample commands, for a pad's More button.
fn pad_menu(ui: &mut Ui, tid: TrackId, ins: &Insert, pad: usize, out: &mut Out) {
    let layout = pad_layout(ins, pad);
    ui.menu_button("Family", |ui| {
        for f in DrumFamily::ALL {
            if ui.selectable_label(layout.family == f, family_name(f)).clicked() {
                out.cmds.push(("mix.kit_pad", json!({"track": tid.0, "pad": pad + 1, "family": f.id()})));
                ui.close();
            }
        }
    });
    ui.menu_button("Rename", |ui| {
        let id = egui::Id::new(("juke-rename", tid.0, pad));
        let mut name: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_else(|| layout.name.clone());
        let resp = ui.text_edit_singleline(&mut name);
        ui.ctx().data_mut(|d| d.insert_temp(id, name.clone()));
        if resp.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && !name.trim().is_empty() {
            out.cmds.push(("mix.kit_pad", json!({"track": tid.0, "pad": pad + 1, "name": name.trim()})));
            ui.ctx().data_mut(|d| d.remove::<String>(id));
            ui.close();
        }
    });
    ui.separator();
    if ui
        .button("Import Takes…")
        .on_hover_text("A recording of the drum (one hit or a run of them, soft to hard): the hits trimmed, sorted into layers and round-robins, the pad levelled. Or drop files on the pad")
        .clicked()
    {
        out.import_pad = Some(pad);
        ui.close();
    }
    if ui
        .add_enabled(out.has_selection, egui::Button::new("From Selection"))
        .on_hover_text("The selected range of an audio track, as it plays, imported like takes")
        .on_disabled_hover_text("Select a range of drum hits on an audio track first")
        .clicked()
    {
        out.from_selection = Some(pad);
        ui.close();
    }
    if ui.button("Load Sample…").clicked() {
        out.load = Some((pad, 0, 0));
        ui.close();
    }
    let loaded = (0..LAYERS).any(|l| (0..ROBINS).any(|rr| ins.samples.contains_key(&sample_key(pad, l, rr))));
    if ui.add_enabled(loaded, egui::Button::new("Clear Samples")).clicked() {
        out.cmds.push(("mix.kit_sample_clear", json!({"track": tid.0, "pad": pad + 1})));
        ui.close();
    }
}

#[allow(clippy::too_many_arguments)]
fn pad_card(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, pad: usize, view: &View, level: f32, out: &mut Out) {
    let layout = pad_layout(ins, pad);
    let color = family_color(layout.family);
    let selected = view.pad == pad;
    // The card itself selects the pad (its controls, drawn after, take their own clicks).
    if ui.interact(r, egui::Id::new(("juke-card", tid.0, pad)), Sense::click()).clicked() {
        out.select = Some(pad);
    }
    import_target(ui, r, Some(pad), out);
    let p = ui.painter().clone();
    p.rect_filled(r.translate(vec2(0.0, 2.0)), 6.0, Color32::from_black_alpha(70));
    widgets::rounded_vgradient(&p, r, 6.0, CARD_TOP, CARD_BOTTOM);
    p.rect_stroke(r, 6.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    // Header band.
    let head = Rect::from_min_size(r.min, vec2(r.width(), 26.0));
    let resp = ui.interact(head, egui::Id::new(("juke-head", tid.0, pad)), Sense::click_and_drag());
    if resp.drag_started() {
        egui::DragAndDrop::set_payload(ui.ctx(), PadDrag(pad));
    }
    if resp.clicked() {
        out.select = Some(pad);
    }
    if resp.double_clicked() {
        out.select = Some(pad);
        out.tab = Some(Tab::Pad);
    }
    let resp = resp.on_hover_text("Drag onto the trigger notes to change the pad's note");
    let _ = resp;
    p.rect_filled(head, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, color);
    let note = pad_note(ins, pad);
    let source = SOURCES.get(pad_value(ins, pad, "source").round().max(0.0) as usize).copied().unwrap_or("?");
    text(&p, pos2(head.min.x + 8.0, head.min.y + 8.0), Align2::LEFT_CENTER, &layout.name, bold(11.5), DARK_TEXT);
    text(
        &p,
        pos2(head.min.x + 8.0, head.min.y + 19.0),
        Align2::LEFT_CENTER,
        format!("{} · {}", soundcraft_midi::note_name(note), source),
        regular(9.0),
        DARK_TEXT.gamma_multiply(0.8),
    );
    let b = 20.0;
    let more = Rect::from_min_size(pos2(head.max.x - b - 4.0, head.min.y + 3.0), vec2(b, b));
    let load = more.translate(vec2(-b - 2.0, 0.0));
    let edit = load.translate(vec2(-b - 2.0, 0.0));
    if icon_button(ui, edit, egui::Id::new(("juke-edit", tid.0, pad)), "pencil", "Edit the pad").clicked() {
        out.select = Some(pad);
        out.tab = Some(Tab::Pad);
    }
    if icon_button(ui, load, egui::Id::new(("juke-load", tid.0, pad)), "plus", "Load a sample onto the pad").clicked() {
        out.select = Some(pad);
        out.load = Some((pad, 0, 0));
    }
    let resp = icon_button(ui, more, egui::Id::new(("juke-more", tid.0, pad)), "triangle_down", "Family, name and samples");
    egui::Popup::menu(&resp).show(|ui| pad_menu(ui, tid, ins, pad, out));
    // Knobs: Tune and Pan (hero), the sends under them.
    let body = Rect::from_min_max(pos2(r.min.x + 4.0, head.max.y + 4.0), pos2(r.max.x - 46.0, r.max.y - 28.0));
    let kw = body.width() * 0.5;
    let top = Rect::from_min_size(body.min, vec2(kw, 76.0));
    knob_at(ui, top, ins, &pad_param(pad, "tune"), 34.0, KnobStyle::CreamHero, out);
    knob_at(ui, top.translate(vec2(kw, 0.0)), ins, &pad_param(pad, "pan"), 34.0, KnobStyle::CreamHero, out);
    let low = Rect::from_min_size(pos2(body.min.x, top.max.y), vec2(kw, 56.0));
    knob_at(ui, low, ins, &pad_param(pad, "send_a"), 22.0, KnobStyle::Cream, out);
    knob_at(ui, low.translate(vec2(kw, 0.0)), ins, &pad_param(pad, "send_b"), 22.0, KnobStyle::Cream, out);
    // Fader and meter.
    let fr = Rect::from_min_max(pos2(r.max.x - 44.0, head.max.y + 8.0), pos2(r.max.x - 16.0, r.max.y - 32.0));
    let mr = Rect::from_min_max(pos2(r.max.x - 13.0, fr.min.y), pos2(r.max.x - 7.0, fr.max.y));
    let lid = pad_param(pad, "level");
    let db = value(ins, &lid);
    if let Some(x) = widgets::fader_cream(ui, fr, db, egui::Id::new(("juke-fader", tid.0, pad)), PAD_CAP) {
        out.set(lid.as_str(), x.clamp(-48.0, 6.0));
    }
    widgets::meter(ui, mr, level, level, false);
    // Footer: audition, choke, solo, mute.
    let foot = Rect::from_min_max(pos2(r.min.x + 6.0, r.max.y - 24.0), pos2(r.max.x - 6.0, r.max.y - 6.0));
    let play = Rect::from_min_size(foot.min, vec2(26.0, foot.height()));
    let resp = ui.interact(play, egui::Id::new(("juke-play", tid.0, pad)), Sense::click_and_drag()).on_hover_text("Play the pad");
    let down = resp.is_pointer_button_down_on();
    let held = view.held.is_some_and(|(p, _)| p == pad);
    if down && !held {
        out.audition = Some(Some((pad, note)));
        out.select = Some(pad);
    } else if !down && held {
        out.audition = Some(None);
    }
    p.rect_filled(play, 3.0, if down { color } else { Color32::from_black_alpha(45) });
    crate::icons::draw(&p, play.shrink(3.0), "play", if down { DARK_TEXT } else { CREAM });
    let choke_id = pad_param(pad, "choke");
    let choke = value(ins, &choke_id).round().max(0.0) as usize;
    let cr = Rect::from_min_size(pos2(play.max.x + 4.0, foot.min.y), vec2(62.0, foot.height()));
    let label = if choke == 0 { "Choke –".to_string() } else { format!("Choke {choke}") };
    let resp = chip(ui, cr, egui::Id::new(("juke-choke", tid.0, pad)), &label, choke > 0, AMBER)
        .on_hover_text("Pads in the same choke group cut each other off. Click for the next group, right-click for none");
    if resp.clicked() {
        out.set(choke_id.as_str(), ((choke + 1) % 9) as f32);
    } else if resp.secondary_clicked() {
        out.set(choke_id.as_str(), 0.0);
    }
    for (i, (name, tip)) in [("mute", "Mute the pad"), ("solo", "Solo the pad")].into_iter().enumerate() {
        let id = pad_param(pad, name);
        let on = value(ins, &id) >= 0.5;
        let br = Rect::from_min_size(pos2(foot.max.x - 22.0 - i as f32 * 24.0, foot.min.y), vec2(22.0, foot.height()));
        let lit = if name == "solo" { AMBER } else { Color32::from_rgb(0xE8, 0x70, 0x4F) };
        if chip(ui, br, egui::Id::new(("juke-ms", tid.0, pad, name)), &name[..1].to_uppercase(), on, lit).on_hover_text(tip).clicked() {
            out.set(id, if on { 0.0 } else { 1.0 });
        }
    }
    if selected {
        p.rect_stroke(r.expand(1.5), 7.0, Stroke::new(2.0, CREAM), StrokeKind::Outside);
    }
}

/// The Kits menu's recorded kits: those installed (click to load), and the kits release's list
/// with a download for each.
/// The Presets menu: the built-in presets by category (one that needs a recorded kit not
/// installed says so), then the presets saved from Juke.
fn presets_menu(ui: &mut Ui, tid: TrackId, current: &str, recorded: &Recorded, out: &mut Out) {
    let lib = soundcraft_engine::juke_presets::builtin();
    let mut cats: Vec<&str> = Vec::new();
    for q in lib {
        if !cats.contains(&q.category.as_str()) {
            cats.push(&q.category);
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    let (ids, kits_dir) = (recorded.ids.clone(), recorded.kits_dir.to_string_lossy().to_string());
    #[cfg(target_arch = "wasm32")]
    let (ids, kits_dir): (Vec<String>, String) = {
        let _ = recorded;
        (Vec::new(), String::new())
    };
    for cat in cats {
        ui.menu_button(cat, |ui| {
            for q in lib.iter().filter(|q| q.category == cat) {
                let missing: Vec<String> = q.needs().into_iter().filter(|k| !ids.contains(k)).collect();
                let label = if missing.is_empty() { q.name.clone() } else { format!("{} (download)", q.name) };
                let mut tip = q.about.clone();
                if !missing.is_empty() {
                    tip = format!("{tip}. Needs the recorded kit {}: Kits › Recorded & saved", missing.join(", "));
                }
                let resp = ui.selectable_label(current == q.name, label).on_hover_text(tip);
                if resp.clicked() {
                    out.cmds.push(("mix.kit_preset", json!({"track": tid.0, "preset": q.name, "kits_dir": kits_dir})));
                    ui.close();
                }
            }
        });
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        ui.separator();
        ui.menu_button("My Presets", |ui| {
            let mine = crate::juke_kits_ui::read_presets(&recorded.presets_dir);
            if mine.is_empty() {
                ui.label(egui::RichText::new("None yet: Save preset adds the kit here").weak());
            }
            for (name, path) in mine {
                if ui.selectable_label(current == name, &name).clicked() {
                    out.cmds.push(("mix.kit_preset", json!({"track": tid.0, "path": path.to_string_lossy(), "kits_dir": kits_dir})));
                    ui.close();
                }
            }
        });
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn recorded_menu(ui: &mut Ui, tid: TrackId, current: &str, v: &KitsView, out: &mut Out) {
    ui.separator();
    ui.label(egui::RichText::new("Recorded & saved").strong());
    if ui
        .button("Import Kit Folder…")
        .on_hover_text("Build the kit from a folder of your drum takes: each file to its pad by name (kick, snare, hat…), hits trimmed, sorted into layers and levelled")
        .clicked()
    {
        out.import_folder = true;
        ui.close();
    }
    if ui.button("Export Kit Folder…").on_hover_text("Write the kit (samples as WAVs, every setting, bus effects) to a folder, to share it").clicked()
    {
        out.export_kit = true;
        ui.close();
    }
    // Each kit as recorded, or with a character laid over it.
    for (name, path) in &v.installed {
        let shown = if current == name || current.starts_with(&format!("{name} · ")) { format!("{name} ✓") } else { name.clone() };
        ui.menu_button(shown, |ui| {
            if ui.selectable_label(current == name, "As recorded").clicked() {
                out.cmds.push(("mix.kit_load", json!({"track": tid.0, "path": path.to_string_lossy()})));
                ui.close();
            }
            ui.separator();
            for c in soundcraft_engine::juke_kits::CHARACTERS {
                let on = current == format!("{name} · {}", c.name);
                if ui.selectable_label(on, c.name).on_hover_text(c.about).clicked() {
                    out.cmds.push(("mix.kit_load", json!({"track": tid.0, "path": path.to_string_lossy(), "character": c.name})));
                    ui.close();
                }
            }
        });
    }
    match &v.active {
        Some((id, done, total)) => {
            let pct = if *total > 0 { *done as f64 / *total as f64 * 100.0 } else { 0.0 };
            ui.label(format!("Downloading {id}: {pct:.0} %"));
            if ui.button("Cancel Download").clicked() {
                out.cancel_kit = true;
            }
        }
        None => match &v.available {
            Ok(list) if list.is_empty() => {
                ui.label(egui::RichText::new("No more kits to download").weak());
            }
            Ok(list) => {
                for k in list {
                    let mb = k.size as f64 / 1_048_576.0;
                    let tip = format!("{}. {}", k.license, k.credit);
                    if ui.button(format!("Download {} ({mb:.0} MB)", if k.name.is_empty() { &k.id } else { &k.name })).on_hover_text(tip).clicked() {
                        out.download_kit = Some(k.clone());
                    }
                }
            }
            Err(msg) => {
                if !msg.is_empty() {
                    ui.label(egui::RichText::new(msg).weak());
                }
                if !msg.starts_with("Checking")
                    && ui.button("Get Recorded Kits…").on_hover_text("List the kits FreeDAWm's kits release offers").clicked()
                {
                    out.fetch_kits = true;
                }
            }
        },
    }
    if !v.note.is_empty() {
        ui.label(egui::RichText::new(&v.note).weak());
    }
}

/// Bus A, Bus B and the output, each a strip with four effect slots, a fader and a meter.
fn bus_strips(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, out_meter: ([f32; 2], [f32; 2], bool), buses: [f32; 2], out: &mut Out) {
    let strips = [
        ("BUS A", "Room and grit", "return_a", SLATE_TOP, SLATE_BOTTOM),
        ("BUS B", "Space", "return_b", SLATE_TOP, SLATE_BOTTOM),
        ("OUTPUT", "Kit glue · stereo", "level", BRASS_TOP, BRASS_BOTTOM),
    ];
    let top_y = 34.0;
    for (i, (name, sub, param, top, bottom)) in strips.into_iter().enumerate() {
        let s = Rect::from_min_size(pos2(r.min.x + i as f32 * (STRIP_W + 6.0), r.min.y), vec2(STRIP_W, r.height()));
        let p = ui.painter().clone();
        p.rect_filled(s.translate(vec2(0.0, 2.0)), 6.0, Color32::from_black_alpha(70));
        widgets::rounded_vgradient(&p, s, 6.0, top, bottom);
        p.rect_stroke(s, 6.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
        text(&p, pos2(s.center().x, s.min.y + 11.0), Align2::CENTER_CENTER, name, bold(11.0), CREAM);
        let sub_c = if i == 2 { Color32::from_rgb(0xE9, 0xD7, 0xAE) } else { Color32::from_rgb(0xC9, 0xDC, 0xDB) };
        text(&p, pos2(s.center().x, s.min.y + 24.0), Align2::CENTER_CENTER, sub, regular(9.5), sub_c);
        // Effect slots (filled by the effects buses).
        let bus = KitBus::ALL[i];
        for slot in 0..KitFx::SLOTS {
            let sr = Rect::from_min_size(pos2(s.min.x + 6.0, s.min.y + top_y + slot as f32 * 22.0), vec2(s.width() - 12.0, 19.0));
            match crate::juke_fx::slot(ui, sr, tid, ins, bus, slot) {
                Some(crate::juke_fx::FxAction::Open(m)) => out.open_fx = Some(m),
                Some(crate::juke_fx::FxAction::Run(id, p)) => out.cmds.push((id, p)),
                None => {}
            }
        }
        let level_y = s.max.y - 20.0;
        let lane = Rect::from_min_max(pos2(s.min.x + 6.0, s.min.y + top_y + 4.0 * 22.0 + 6.0), pos2(s.max.x - 6.0, level_y - 6.0));
        let fr = Rect::from_min_max(pos2(lane.min.x, lane.min.y), pos2(lane.min.x + 54.0, lane.max.y));
        let db = value(ins, param);
        if let Some(x) = widgets::fader_cream(ui, fr, db, egui::Id::new(("juke-bus-fader", tid.0, i)), STRIP_CAP) {
            out.set(param, x.clamp(-48.0, 6.0));
        }
        // The output meters the track; a bus its level as it returns into the kit.
        let (level, hold, clip) = match buses.get(i) {
            Some(&b) => ([b; 2], [b; 2], false),
            None => out_meter,
        };
        for (c, x) in [lane.max.x - 15.0, lane.max.x - 7.0].into_iter().enumerate() {
            let mr = Rect::from_min_max(pos2(x, lane.min.y), pos2(x + 6.0, lane.max.y));
            widgets::meter(ui, mr, level[c], hold[c], clip);
        }
        let fmt = info(param).map_or_else(|| format!("{db:.1}"), |p| p.format(db));
        text(&p, pos2(s.center().x, level_y + 8.0), Align2::CENTER_CENTER, fmt, mono(11.0), AMBER);
    }
}

/// Whether `note` is a black key.
fn is_black(note: u8) -> bool {
    matches!(note % 12, 1 | 3 | 6 | 8 | 10)
}

/// The trigger notes as a two-row honeycomb: white keys along the bottom, each black key nested
/// above between its neighbours. Click a cell to select its pad; drop a pad header on one to move
/// the pad's note there.
fn honeycomb(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, selected: usize, out: &mut Out) {
    let p = ui.painter().clone();
    p.rect_filled(r, 4.0, TOP_BAR);
    let whites = (HONEY_LOW..=HONEY_HIGH).filter(|n| !is_black(*n)).count() as f32;
    let w = ((r.width() - 16.0) / whites).clamp(12.0, 38.0);
    let h = (r.height() - 8.0) / 1.75;
    let x0 = r.center().x - w * whites * 0.5;
    let hex = |c: egui::Pos2| -> Vec<egui::Pos2> {
        let (hw, hh) = (w * 0.5 - 1.0, h * 0.5);
        vec![
            pos2(c.x, c.y - hh),
            pos2(c.x + hw, c.y - hh * 0.5),
            pos2(c.x + hw, c.y + hh * 0.5),
            pos2(c.x, c.y + hh),
            pos2(c.x - hw, c.y + hh * 0.5),
            pos2(c.x - hw, c.y - hh * 0.5),
        ]
    };
    let mut white_i = 0.0;
    for note in HONEY_LOW..=HONEY_HIGH {
        let black = is_black(note);
        let c = if black {
            pos2(x0 + white_i * w, r.max.y - 4.0 - h * 1.25)
        } else {
            let c = pos2(x0 + (white_i + 0.5) * w, r.max.y - 4.0 - h * 0.5);
            white_i += 1.0;
            c
        };
        let cell = Rect::from_center_size(c, vec2(w - 2.0, h));
        let pad = pad_for_note(ins, note);
        let resp = ui.interact(cell, egui::Id::new(("juke-honey", tid.0, note)), Sense::click());
        let resp = match pad {
            Some(pd) => resp.on_hover_text(format!("{} · {}", soundcraft_midi::note_name(note), pad_layout(ins, pd).name)),
            None => resp.on_hover_text(soundcraft_midi::note_name(note)),
        };
        if resp.clicked()
            && let Some(pd) = pad
        {
            out.select = Some(pd);
        }
        if let Some(drag) = resp.dnd_release_payload::<PadDrag>() {
            out.set(pad_param(drag.0, "note"), f32::from(note));
            out.select = Some(drag.0);
        }
        let hovered_drop = resp.dnd_hover_payload::<PadDrag>().is_some();
        let fill = match pad {
            Some(pd) => family_color(pad_layout(ins, pd).family),
            None if black => Color32::from_rgb(0x3A, 0x32, 0x2B),
            None => Color32::from_rgb(0x4A, 0x40, 0x37),
        };
        let stroke = if pad == Some(selected) {
            Stroke::new(2.0, CREAM)
        } else if hovered_drop || resp.hovered() {
            Stroke::new(1.5, AMBER)
        } else {
            Stroke::new(1.0, EDGE)
        };
        p.add(Shape::convex_polygon(hex(c), fill, stroke));
        if note % 12 == 0 {
            let tc = if pad.is_some() { DARK_TEXT } else { LABEL };
            text(&p, c, Align2::CENTER_CENTER, soundcraft_midi::note_name(note), regular(8.5), tc);
        }
    }
}

/// The selected pad at full size: its envelopes, sample layers, synth voice and playback options.
fn pad_edit(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, pad: usize, level: f32, out: &mut Out) {
    let layout = pad_layout(ins, pad);
    let color = family_color(layout.family);
    let p = ui.painter().clone();
    widgets::rounded_vgradient(&p, r, 6.0, CARD_TOP, CARD_BOTTOM);
    p.rect_stroke(r, 6.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    let head = Rect::from_min_size(r.min, vec2(r.width(), 30.0));
    p.rect_filled(head, egui::CornerRadius { nw: 6, ne: 6, sw: 0, se: 0 }, color);
    let back = Rect::from_min_size(pos2(head.min.x + 6.0, head.min.y + 5.0), vec2(56.0, 20.0));
    let resp = ui.interact(back, egui::Id::new(("juke-back", tid.0)), Sense::click()).on_hover_text("Back to the kit");
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Back to the kit"));
    p.rect_filled(back, 3.0, if resp.hovered() { Color32::from_black_alpha(60) } else { Color32::from_black_alpha(35) });
    let c = pos2(back.min.x + 12.0, back.center().y);
    p.add(Shape::convex_polygon(vec![pos2(c.x - 4.0, c.y), pos2(c.x + 3.0, c.y - 5.0), pos2(c.x + 3.0, c.y + 5.0)], DARK_TEXT, Stroke::NONE));
    text(&p, pos2(back.center().x + 6.0, back.center().y), Align2::CENTER_CENTER, "Kit", bold(10.5), DARK_TEXT);
    if resp.clicked() {
        out.tab = Some(Tab::Kit);
    }
    let note = pad_note(ins, pad);
    text(
        &p,
        pos2(back.max.x + 10.0, head.center().y),
        Align2::LEFT_CENTER,
        format!("Pad {} · {} · {}", pad + 1, layout.name, soundcraft_midi::note_name(note)),
        bold(12.0),
        DARK_TEXT,
    );
    // From Selection, as Lift's: the selected range of an audio track becomes the pad.
    let from = Rect::from_min_size(pos2(head.max.x - 332.0, head.min.y + 5.0), vec2(120.0, 20.0));
    let resp = ui
        .add_enabled_ui(out.has_selection, |ui| chip(ui, from, egui::Id::new(("juke-from-sel", tid.0)), "From Selection", false, AMBER))
        .inner
        .on_hover_text("The selected range of an audio track, as it plays: its hits trimmed, sorted soft to hard into layers, the pad levelled")
        .on_disabled_hover_text("Select a range of drum hits on an audio track first");
    if resp.clicked() {
        out.from_selection = Some(pad);
    }
    // Sample / Synth / Both.
    let src_id = pad_param(pad, "source");
    let src = value(ins, &src_id).round() as usize;
    for (i, name) in SOURCES.iter().enumerate() {
        let br = Rect::from_min_size(pos2(head.max.x - 200.0 + i as f32 * 64.0, head.min.y + 5.0), vec2(60.0, 20.0));
        let resp = ui.interact(br, egui::Id::new(("juke-src", tid.0, i)), Sense::click());
        let on = src == i;
        p.rect_filled(br, 3.0, if on { DARK_TEXT } else { Color32::from_black_alpha(30) });
        text(&p, br.center(), Align2::CENTER_CENTER, *name, bold(10.0), if on { color } else { DARK_TEXT });
        if resp.clicked() {
            out.set(src_id.as_str(), i as f32);
        }
    }
    let inner = Rect::from_min_max(pos2(r.min.x + 8.0, head.max.y + 8.0), pos2(r.max.x - 8.0, r.max.y - 8.0));
    let strip_w = 80.0;
    let side_w = (inner.width() * 0.36).clamp(220.0, 300.0);
    let env = Rect::from_min_max(inner.min, pos2(inner.max.x - side_w - strip_w - 16.0, inner.max.y));
    let side = Rect::from_min_max(pos2(env.max.x + 8.0, inner.min.y), pos2(env.max.x + 8.0 + side_w, inner.max.y));
    let strip = Rect::from_min_max(pos2(side.max.x + 8.0, inner.min.y), inner.max);
    envelopes(ui, env, tid, ins, pad, color, out);
    side_column(ui, side, tid, ins, pad, color, out);
    pad_strip(ui, strip, tid, ins, pad, level, out);
}

/// Amp, pitch and filter envelope rows: a hero knob, the curve and the stage knobs.
fn envelopes(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, pad: usize, color: Color32, out: &mut Out) {
    let rows: [(&str, &str, &str, &[&str]); 3] = [
        // Trim: the kit's balance for this pad (a recorded kit sets it), added to the level.
        ("Amp envelope", "how the hit rises and dies away", "level", &["attack", "hold", "decay", "velocity", "trim"]),
        ("Pitch envelope", "a quick drop gives the snap", "tune", &["fine", "pitch_amount", "pitch_decay"]),
        ("Filter envelope", "low-pass, opens on hard hits", "cutoff", &["filter", "resonance", "filter_env", "filter_decay", "drive"]),
    ];
    let row_h = r.height() / 3.0;
    let p = ui.painter().clone();
    for (i, (title, hint, hero, stages)) in rows.into_iter().enumerate() {
        let row = Rect::from_min_size(pos2(r.min.x, r.min.y + i as f32 * row_h), vec2(r.width(), row_h - 6.0));
        let hk = Rect::from_min_size(pos2(row.min.x, row.min.y + 4.0), vec2(84.0, row.height() - 4.0));
        knob_at(ui, hk, ins, &pad_param(pad, hero), 40.0, KnobStyle::CreamHero, out);
        // The envelope's lamp, title and what it does.
        let head_y = row.min.y + 7.0;
        envelope_lamp(ui, pos2(hk.max.x + 6.0, head_y), tid, ins, pad, i, out);
        let t = text(&p, pos2(hk.max.x + 34.0, head_y), Align2::LEFT_CENTER, title, bold(11.0), CREAM);
        // What it does, where it fits whole.
        let hint_g = p.layout_no_wrap(hint.to_string(), regular(10.0), LABEL.gamma_multiply(0.75));
        if t.max.x + 8.0 + hint_g.size().x <= row.max.x {
            p.galley(pos2(t.max.x + 8.0, head_y - hint_g.size().y * 0.5), hint_g, LABEL);
        }
        let well = Rect::from_min_max(pos2(hk.max.x + 6.0, row.min.y + 16.0), pos2(row.max.x, row.min.y + 16.0 + (row.height() - 16.0) * 0.46));
        widgets::inset(&p, well, Color32::from_rgb(0x1A, 0x16, 0x12));
        curve(ui, well, tid, ins, pad, i, color, out);
        let knobs = Rect::from_min_max(pos2(well.min.x, well.max.y + 2.0), pos2(row.max.x, row.max.y));
        let kw = (knobs.width() / stages.len() as f32).min(70.0);
        for (j, st) in stages.iter().enumerate() {
            let kr = Rect::from_min_size(pos2(knobs.min.x + j as f32 * kw, knobs.min.y), vec2(kw, knobs.height()));
            knob_at(ui, kr, ins, &pad_param(pad, st), 22.0, KnobStyle::Cream, out);
        }
    }
}

/// Whether envelope row `row` (amp, pitch, filter) shapes the pad: the amp always, the pitch with
/// an amount, the filter when one is chosen.
pub fn envelope_on(ins: &Insert, pad: usize, row: usize) -> bool {
    match row {
        1 => pad_value(ins, pad, "pitch_amount").abs() > 1.0e-3,
        2 => pad_value(ins, pad, "filter").round() as i32 != 0,
        _ => true,
    }
}

/// An envelope's ON lamp: lit while it shapes the pad; the pitch and filter ones switch theirs.
fn envelope_lamp(ui: &mut Ui, at: egui::Pos2, tid: TrackId, ins: &Insert, pad: usize, row: usize, out: &mut Out) {
    let on = envelope_on(ins, pad, row);
    let r = Rect::from_min_size(pos2(at.x, at.y - 7.0), vec2(24.0, 14.0));
    let label = ["Amp envelope on", "Pitch envelope on", "Filter envelope on"].get(row).copied().unwrap_or_default();
    let resp = ui.interact(r, egui::Id::new(("juke-env-lamp", tid.0, pad, row)), if row == 0 { Sense::hover() } else { Sense::click() });
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, label));
    let p = ui.painter();
    p.rect_filled(r, 7.0, if on { AMBER } else { Color32::from_black_alpha(60) });
    text(p, r.center(), Align2::CENTER_CENTER, "ON", bold(8.5), if on { DARK_TEXT } else { LABEL.gamma_multiply(0.6) });
    let tip = match row {
        0 => "The amp envelope always shapes the hit",
        1 => "Switch the pitch sweep on (+12 st) or off",
        _ => "Switch the filter on (low-pass) or off",
    };
    if resp.on_hover_text(tip).clicked() {
        match row {
            1 => out.set(pad_param(pad, "pitch_amount"), if on { 0.0 } else { 12.0 }),
            2 => out.set(pad_param(pad, "filter"), if on { 0.0 } else { 1.0 }),
            _ => {}
        }
    }
}

/// An envelope's curve in its well: amber, with cream handles that drag the stages.
#[allow(clippy::too_many_arguments)]
fn curve(ui: &mut Ui, well: Rect, tid: TrackId, ins: &Insert, pad: usize, row: usize, color: Color32, out: &mut Out) {
    let p = ui.painter_at(well);
    let w = well.shrink2(vec2(8.0, 6.0));
    let x_ms = |ms: f32, total: f32| w.min.x + w.width() * (ms / total.max(1.0)).clamp(0.0, 1.0);
    let line = Stroke::new(2.0, AMBER);
    match row {
        0 => {
            let (a, h, d) = (pad_value(ins, pad, "attack"), pad_value(ins, pad, "hold"), pad_value(ins, pad, "decay"));
            let total = (a + h + d * 1.2).max(50.0);
            let pts = [
                pos2(w.min.x, w.max.y),
                pos2(x_ms(a, total), w.min.y),
                pos2(x_ms(a + h, total), w.min.y),
                pos2(x_ms(a + h + d * 0.35, total), w.min.y + w.height() * 0.7),
                pos2(x_ms(a + h + d, total), w.max.y),
            ];
            p.add(Shape::line(pts.to_vec(), line));
            // Handles: the attack's peak and the decay's end.
            let handles = [("attack", pts[1], a, total), ("decay", pts[4], d, total)];
            for (name, at, v, total) in handles {
                let id = pad_param(pad, name);
                let hr = Rect::from_center_size(at, vec2(10.0, 10.0));
                let resp = ui.interact(hr, egui::Id::new(("juke-env", tid.0, pad, name)), Sense::drag());
                p.circle(at, 4.0, CREAM, Stroke::new(1.0, EDGE));
                if resp.dragged()
                    && let Some(inf) = info(&id)
                {
                    let ms = resp.drag_delta().x / w.width().max(1.0) * total;
                    out.set(id, inf.clamp(v + ms));
                }
            }
        }
        1 => {
            let amt = pad_value(ins, pad, "pitch_amount");
            let dec = pad_value(ins, pad, "pitch_decay");
            let mid = w.center().y;
            let start = mid - amt / 24.0 * w.height() * 0.5;
            let n = 24;
            let pts: Vec<_> = (0..=n)
                .map(|i| {
                    let t = i as f32 / n as f32;
                    let ms = t * 400.0;
                    pos2(w.min.x + w.width() * t, mid + (start - mid) * (-ms / dec.max(1.0)).exp())
                })
                .collect();
            p.hline(w.x_range(), mid, Stroke::new(1.0, Color32::from_white_alpha(20)));
            p.add(Shape::line(pts, line));
        }
        _ => {
            let kind = pad_value(ins, pad, "filter").round() as usize;
            let cutoff = pad_value(ins, pad, "cutoff");
            let res = pad_value(ins, pad, "resonance") / 100.0;
            let x_c = w.min.x + w.width() * ((cutoff.max(20.0) / 20.0).ln() / 1000f32.ln()).clamp(0.0, 1.0);
            let n = 48;
            let pts: Vec<_> = (0..=n)
                .map(|i| {
                    let x = w.min.x + w.width() * i as f32 / n as f32;
                    let d = (x - x_c) / (w.width() * 0.12);
                    let bump = res * (-d * d).exp() * 0.35;
                    let g = match kind {
                        1 => 1.0 / (1.0 + d.max(0.0).powi(2)),
                        2 => 1.0 / (1.0 + (-d).max(0.0).powi(2)),
                        3 => (-d * d * 0.5).exp(),
                        _ => 1.0,
                    };
                    let g = if kind == 0 { 0.7 } else { (g * 0.7 + bump).min(1.0) };
                    pos2(x, w.max.y - w.height() * g)
                })
                .collect();
            p.add(Shape::line(pts, if kind == 0 { Stroke::new(1.5, AMBER.gamma_multiply(0.4)) } else { line }));
            if kind > 0 {
                let id = pad_param(pad, "cutoff");
                let at = pos2(x_c, w.min.y + w.height() * 0.3);
                let resp = ui.interact(Rect::from_center_size(at, vec2(10.0, 10.0)), egui::Id::new(("juke-cut", tid.0, pad)), Sense::drag());
                p.circle(at, 4.0, CREAM, Stroke::new(1.0, EDGE));
                if resp.dragged()
                    && let Some(inf) = info(&id)
                {
                    let t = inf.to_normalized(cutoff) + resp.drag_delta().x / w.width().max(1.0);
                    out.set(id, inf.from_normalized(t));
                }
            }
        }
    }
    let _ = color;
}

/// The side column: sample layers, the synth voice, playback options.
/// The velocity layers' names, hardest first.
pub const LAYER_NAMES: [&str; LAYERS] = ["Hard", "Medium", "Soft", "Ghost"];

/// A section's title in the pad editor's side column.
fn section(p: &egui::Painter, x: f32, y: f32, s: &str) {
    text(p, pos2(x, y + 6.0), Align2::LEFT_CENTER, s.to_uppercase(), bold(10.5), LABEL);
}

/// A horizontal slider for parameter `id` in `r`: an amber run from the left (or from `from`)
/// under a cream cap. Its change goes to `out`.
fn hslider(ui: &mut Ui, r: Rect, ins: &Insert, id: &str, out: &mut Out) {
    let Some(inf) = info(id) else { return };
    let v = value(ins, id);
    let p = ui.painter().clone();
    let track = Rect::from_center_size(r.center(), vec2(r.width() - 12.0, 6.0));
    let t = inf.to_normalized(v).clamp(0.0, 1.0);
    let at = |t: f32| track.min.x + track.width() * t;
    p.rect_filled(track, 3.0, Color32::from_rgb(0x15, 0x11, 0x0E));
    p.rect_filled(Rect::from_min_max(track.min, pos2(at(t), track.max.y)), 3.0, AMBER.gamma_multiply(0.7));
    let cap = Rect::from_center_size(pos2(at(t), track.center().y), vec2(11.0, 16.0));
    p.rect_filled(cap.translate(vec2(0.0, 2.0)), 3.0, Color32::from_black_alpha(90));
    p.rect_filled(cap, 3.0, Color32::from_rgb(0xE6, 0xD9, 0xC2));
    p.rect_stroke(cap, 3.0, Stroke::new(1.0, Color32::from_rgb(0x3A, 0x2E, 0x22)), StrokeKind::Inside);
    let resp = ui.interact(r, egui::Id::new(("juke-hslider", id)), Sense::click_and_drag());
    let name = short_name(inf.name);
    resp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(v), name));
    let resp = resp.on_hover_text(format!("{name}: {} (double-click: {})", inf.format(v), inf.format(inf.default)));
    if resp.double_clicked() {
        out.set(id, inf.default);
    } else if (resp.dragged() || resp.clicked())
        && let Some(pos) = resp.interact_pointer_pos()
    {
        let nt = ((pos.x - track.min.x) / track.width().max(1.0)).clamp(0.0, 1.0);
        let nv = inf.clamp(inf.from_normalized(nt));
        if (nv - v).abs() > f32::EPSILON {
            out.set(id, nv);
        }
    }
}

/// A pill toggle on the pad editor.
fn pad_pill(ui: &mut Ui, r: Rect, id: egui::Id, label: &str, on: bool) -> egui::Response {
    let resp = ui.interact(r, id, Sense::click());
    resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, ui.is_enabled(), on, label));
    let p = ui.painter();
    let fill = if on {
        AMBER
    } else if resp.hovered() {
        Color32::from_black_alpha(60)
    } else {
        Color32::from_black_alpha(38)
    };
    p.rect_filled(r, r.height() * 0.5, fill);
    p.rect_stroke(r, r.height() * 0.5, Stroke::new(1.0, Color32::from_black_alpha(89)), StrokeKind::Inside);
    text(p, r.center(), Align2::CENTER_CENTER, label, bold(11.0), if on { DARK_TEXT } else { CREAM });
    resp
}

/// A choice parameter as a combo box in `r`.
fn choice_combo(ui: &mut Ui, r: Rect, tid: TrackId, pad: usize, ins: &Insert, id: &str, out: &mut Out) {
    let Some(inf) = info(id) else { return };
    let v = value(ins, id);
    ui.scope_builder(egui::UiBuilder::new().max_rect(r), |ui| {
        let mut idx = v.round().max(0.0) as usize;
        let shown = inf.choices.get(idx).copied().map_or_else(|| inf.format(v), str::to_string);
        egui::ComboBox::from_id_salt(("juke-pb", tid.0, pad, inf.id)).width(r.width() - 8.0).selected_text(shown).show_ui(ui, |ui| {
            for (i, c) in inf.choices.iter().enumerate() {
                if ui.selectable_value(&mut idx, i, *c).clicked() {
                    out.set(id, i as f32);
                }
            }
        });
    });
}

fn side_column(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, pad: usize, color: Color32, out: &mut Out) {
    let p = ui.painter().clone();
    // Sample layers: a row per velocity layer, hardest on top; round-robins across.
    section(&p, r.min.x, r.min.y, "Sample layers");
    text(&p, pos2(r.max.x, r.min.y + 6.0), Align2::RIGHT_CENTER, "round-robin →", regular(9.5), LABEL);
    let name_w = 46.0;
    let grid = Rect::from_min_size(pos2(r.min.x + name_w, r.min.y + 15.0), vec2(r.width() - name_w, 4.0 * 24.0));
    let cw = grid.width() / ROBINS as f32;
    for row in 0..LAYERS {
        let layer = LAYERS - 1 - row;
        let y = grid.min.y + row as f32 * 24.0;
        text(&p, pos2(r.min.x, y + 11.0), Align2::LEFT_CENTER, LAYER_NAMES.get(row).copied().unwrap_or_default(), regular(10.5), LABEL);
        for robin in 0..ROBINS {
            let cell = Rect::from_min_size(pos2(grid.min.x + robin as f32 * cw, y), vec2(cw - 3.0, 21.0));
            let key = sample_key(pad, layer, robin);
            let filled = ins.samples.contains_key(&key);
            let resp = ui.interact(cell, egui::Id::new(("juke-layer", tid.0, pad, row, robin)), Sense::click());
            drop_target(ui, cell, (pad, layer, robin), out);
            let tip = format!("{} layer, round-robin {}", LAYER_NAMES.get(row).copied().unwrap_or_default(), robin + 1);
            resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &tip));
            if filled {
                p.rect_filled(cell, 5.0, color.gamma_multiply(1.0 - row as f32 * 0.15));
                let mid = cell.center().y;
                let mut pts = Vec::new();
                for k in 0..=24 {
                    let x = k as f32 / 24.0;
                    let e = (-x * 5.0).exp();
                    pts.push(pos2(cell.min.x + 3.0 + x * (cell.width() - 6.0), mid - 7.0 * e * (x * (14.0 + robin as f32 * 3.0)).sin()));
                }
                p.add(Shape::line(pts, Stroke::new(1.2, DARK_TEXT.gamma_multiply(0.75))));
            } else {
                let c = if resp.hovered() { LABEL } else { LABEL.gamma_multiply(0.35) };
                let rr = cell.shrink(0.5);
                for w in [rr.left_top(), rr.right_top(), rr.right_bottom(), rr.left_bottom(), rr.left_top()].windows(2) {
                    if let [a, b] = w {
                        p.add(Shape::dashed_line(&[*a, *b], Stroke::new(1.0, c), 3.0, 2.0));
                    }
                }
                text(&p, cell.center(), Align2::CENTER_CENTER, "+", regular(12.0), c);
            }
            let resp = resp.on_hover_text(if filled {
                format!("{tip}. Click to replace, right-click to clear")
            } else {
                format!("{tip}. Click to load a sample, or drop files (several fill the row)")
            });
            if resp.clicked() {
                out.load = Some((pad, layer, robin));
            }
            if resp.secondary_clicked() && filled {
                out.cmds.push(("mix.kit_sample_clear", json!({"track": tid.0, "pad": pad + 1, "layer": layer + 1, "robin": robin + 1})));
            }
        }
    }
    let mut y = grid.max.y + 2.0;
    text(&p, pos2(r.min.x, y + 6.0), Align2::LEFT_CENTER, "Drop files on a cell; several fill its row.", regular(9.0), LABEL.gamma_multiply(0.8));
    y += 18.0;
    // Synth voice: the voice, its tone and snap, and the blend with the samples.
    section(&p, r.min.x, y + 2.0, "Synth voice");
    let voice_id = pad_param(pad, "voice");
    let voice = value(ins, &voice_id).round().max(0.0) as usize;
    let combo = Rect::from_min_size(pos2(r.min.x + 96.0, y), vec2(r.width() - 96.0, 22.0));
    ui.scope_builder(egui::UiBuilder::new().max_rect(combo), |ui| {
        egui::ComboBox::from_id_salt(("juke-voice", tid.0, pad))
            .width(combo.width() - 8.0)
            .selected_text(VOICE_NAMES.get(voice).copied().unwrap_or("?"))
            .show_ui(ui, |ui| {
                for (i, n) in VOICE_NAMES.iter().enumerate() {
                    if ui.selectable_label(i == voice, *n).clicked() {
                        out.set(voice_id.as_str(), i as f32);
                    }
                }
            });
    });
    y += 26.0;
    let kw = r.width() / 2.0;
    for (j, st) in ["tone", "snappy"].iter().enumerate() {
        let kr = Rect::from_min_size(pos2(r.min.x + j as f32 * kw, y), vec2(kw, 52.0));
        knob_at(ui, kr, ins, &pad_param(pad, st), 20.0, KnobStyle::Cream, out);
    }
    y += 54.0;
    let blend = Rect::from_min_size(pos2(r.min.x, y), vec2(r.width(), 18.0));
    text(&p, pos2(blend.min.x, blend.center().y), Align2::LEFT_CENTER, "Sample", regular(10.5), LABEL);
    text(&p, pos2(blend.max.x, blend.center().y), Align2::RIGHT_CENTER, "Synth", regular(10.5), LABEL);
    hslider(ui, Rect::from_min_max(pos2(blend.min.x + 46.0, blend.min.y), pos2(blend.max.x - 40.0, blend.max.y)), ins, &pad_param(pad, "blend"), out);
    y += 24.0;
    // Humanize: this pad's own, per hit.
    section(&p, r.min.x, y, "Humanize");
    y += 15.0;
    for (label, name) in [("Level", "hum_level"), ("Pitch", "hum_pitch"), ("Filter", "hum_filter"), ("Timing", "hum_timing")] {
        let id = pad_param(pad, name);
        let row = Rect::from_min_size(pos2(r.min.x, y), vec2(r.width(), 18.0));
        text(&p, pos2(row.min.x, row.center().y), Align2::LEFT_CENTER, label, regular(10.5), LABEL);
        let v = value(ins, &id);
        let shown = match name {
            "hum_level" => format!("±{v:.1} dB"),
            "hum_pitch" => format!("±{v:.0} c"),
            "hum_filter" => format!("±{v:.0}%"),
            _ => format!("≤{v:.1} ms"),
        };
        text(&p, pos2(row.max.x, row.center().y), Align2::RIGHT_CENTER, shown, mono(10.5), CREAM);
        hslider(ui, Rect::from_min_max(pos2(row.min.x + 46.0, row.min.y), pos2(row.max.x - 62.0, row.max.y)), ins, &id, out);
        y += 19.0;
    }
    y += 4.0;
    // Playback: one-shot or gate, reverse, the choke group and the stacked pads.
    section(&p, r.min.x, y, "Playback");
    y += 16.0;
    let mode_id = pad_param(pad, "mode");
    let gate = value(ins, &mode_id) >= 0.5;
    let pills: [(&str, bool, &str); 2] = [("One-shot", !gate, "The hit plays out whole"), ("Gate", gate, "The hit stops when the note ends")];
    let mut x = r.min.x;
    for (i, (label, on, tip)) in pills.into_iter().enumerate() {
        let pr = Rect::from_min_size(pos2(x, y), vec2(if i == 0 { 70.0 } else { 46.0 }, 22.0));
        if pad_pill(ui, pr, egui::Id::new(("juke-mode", tid.0, pad, i)), label, on).on_hover_text(tip).clicked() && !on {
            out.set(mode_id.as_str(), i as f32);
        }
        x = pr.max.x + 4.0;
    }
    let rev_id = pad_param(pad, "reverse");
    let rev = value(ins, &rev_id) >= 0.5;
    let rr = Rect::from_min_size(pos2(x, y), vec2(64.0, 22.0));
    if pad_pill(ui, rr, egui::Id::new(("juke-rev", tid.0, pad)), "Reverse", rev).on_hover_text("Play the pad's samples backwards").clicked() {
        out.set(rev_id, if rev { 0.0 } else { 1.0 });
    }
    y += 26.0;
    let rows = [("Choke", "choke", None), ("Stack 1", "stack1", Some("stack1_ms")), ("Stack 2", "stack2", Some("stack2_ms"))];
    for (label, name, ms) in rows {
        if y + 22.0 > r.max.y {
            break;
        }
        text(&p, pos2(r.min.x, y + 11.0), Align2::LEFT_CENTER, label, regular(10.5), LABEL);
        let end = if ms.is_some() { r.max.x - 64.0 } else { r.max.x };
        choice_combo(ui, Rect::from_min_max(pos2(r.min.x + 56.0, y), pos2(end, y + 22.0)), tid, pad, ins, &pad_param(pad, name), out);
        if let Some(ms) = ms {
            let id = pad_param(pad, ms);
            let mut v = value(ins, &id);
            let dr = Rect::from_min_max(pos2(r.max.x - 60.0, y + 1.0), pos2(r.max.x, y + 21.0));
            let resp = ui
                .put(dr, egui::DragValue::new(&mut v).range(0.0..=50.0).speed(0.25).suffix(" ms").max_decimals(1))
                .on_hover_text("How late the stacked pad plays");
            if resp.changed() {
                out.set(id, v);
            }
        }
        y += 24.0;
    }
}

/// The pad's own strip in the edit view: Tune, Pan, the sends and its fader.
fn pad_strip(ui: &mut Ui, r: Rect, tid: TrackId, ins: &Insert, pad: usize, level: f32, out: &mut Out) {
    let p = ui.painter().clone();
    widgets::rounded_vgradient(&p, r, 6.0, CARD_TOP.gamma_multiply(1.1), CARD_BOTTOM);
    p.rect_stroke(r, 6.0, Stroke::new(1.0, EDGE), StrokeKind::Inside);
    let mut y = r.min.y + 4.0;
    for (name, size, style, h) in [
        ("tune", 28.0, KnobStyle::CreamHero, 74.0),
        ("pan", 28.0, KnobStyle::CreamHero, 74.0),
        ("send_a", 18.0, KnobStyle::Cream, 58.0),
        ("send_b", 18.0, KnobStyle::Cream, 58.0),
    ] {
        knob_at(ui, Rect::from_min_size(pos2(r.min.x, y), vec2(r.width(), h)), ins, &pad_param(pad, name), size, style, out);
        y += h;
    }
    let lane = Rect::from_min_max(pos2(r.min.x + 8.0, y + 6.0), pos2(r.max.x - 8.0, r.max.y - 22.0));
    let fr = Rect::from_min_max(lane.min, pos2(lane.min.x + 36.0, lane.max.y));
    let lid = pad_param(pad, "level");
    let db = value(ins, &lid);
    if let Some(x) = widgets::fader_cream(ui, fr, db, egui::Id::new(("juke-pad-strip", tid.0, pad)), STRIP_CAP) {
        out.set(lid.as_str(), x.clamp(-48.0, 6.0));
    }
    let mr = Rect::from_min_max(pos2(lane.max.x - 8.0, lane.min.y), pos2(lane.max.x - 2.0, lane.max.y));
    widgets::meter(ui, mr, level, level, false);
    let fmt = info(&lid).map_or_else(|| format!("{db:.1}"), |i| i.format(db));
    text(&p, pos2(r.center().x, r.max.y - 11.0), Align2::CENTER_CENTER, fmt, mono(11.0), AMBER);
}

/// What a pad import did, for the status line.
pub(crate) fn import_status(r: Result<Value, String>) -> String {
    match r {
        Ok(v) => {
            let name = v.get("name").and_then(Value::as_str).unwrap_or("Pad");
            let hits = v.get("hits").and_then(Value::as_u64).unwrap_or(0);
            let layers = v.get("layers").and_then(Value::as_u64).unwrap_or(0);
            let trim = v.get("trim").and_then(Value::as_f64).unwrap_or(0.0);
            let dropped = v.get("dropped").and_then(Value::as_u64).unwrap_or(0);
            let off = v.get("off_pitch").and_then(Value::as_u64).unwrap_or(0);
            let mut extra = String::new();
            if off > 0 {
                extra += &format!("; {off} at other notes left out");
            }
            if dropped > 0 {
                extra += &format!("; {dropped} more than the pad holds left out");
            }
            format!(
                "{name}: {hits} hit{} in {layers} layer{}, trim {trim:+.1} dB{extra}",
                if hits == 1 { "" } else { "s" },
                if layers == 1 { "" } else { "s" }
            )
        }
        Err(e) => e,
    }
}

/// What a kit import did, for the status line.
pub(crate) fn kit_import_status(r: Result<Value, String>) -> String {
    match r {
        Ok(v) => {
            let pads = v.get("pads").and_then(Value::as_array).map_or(0, Vec::len);
            let kit = v.get("kit").and_then(Value::as_str).map(|k| format!(" into {k}")).unwrap_or_default();
            let skipped = v.get("skipped").and_then(Value::as_array).map_or(0, Vec::len);
            let extra = if skipped > 0 { format!("; {skipped} left out (no free pad, or no hits)") } else { String::new() };
            format!("Imported {pads} pad{}{kit}{extra}", if pads == 1 { "" } else { "s" })
        }
        Err(e) => e,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_param_the_window_names_exists() {
        for name in [
            "note",
            "source",
            "voice",
            "tone",
            "snappy",
            "blend",
            "tune",
            "fine",
            "attack",
            "hold",
            "decay",
            "velocity",
            "pitch_amount",
            "pitch_decay",
            "filter",
            "cutoff",
            "resonance",
            "filter_env",
            "filter_decay",
            "drive",
            "level",
            "pan",
            "send_a",
            "send_b",
            "choke",
            "mode",
            "reverse",
            "mute",
            "solo",
            "stack1",
            "stack1_ms",
            "stack2",
            "stack2_ms",
            "trim",
        ] {
            for pad in 0..PADS {
                assert!(info(&pad_param(pad, name)).is_some(), "{}", pad_param(pad, name));
            }
        }
        for id in ["level", "humanize", "return_a", "return_b"] {
            assert!(info(id).is_some(), "{id}");
        }
    }

    #[test]
    fn the_default_kit_maps_its_notes_to_pads() {
        let ins = soundcraft_engine::cmd::juke::juke_insert();
        assert_eq!(pad_for_note(&ins, 36), Some(0));
        assert_eq!(pad_for_note(&ins, 38), Some(2));
        assert_eq!(pad_for_note(&ins, 100), None);
        assert!((HONEY_LOW..=HONEY_HIGH).contains(&pad_note(&ins, 15)));
        assert_eq!(soundcraft_midi::note_name(HONEY_LOW), "C1");
        assert_eq!(soundcraft_midi::note_name(HONEY_HIGH), "B5");
    }

    #[test]
    fn pad_names_drop_the_pad() {
        assert_eq!(short_name("Pad 12 Send A"), "Send A");
        assert_eq!(short_name("Pad 1 Tune"), "Tune");
        assert_eq!(short_name("Humanize"), "Humanize");
    }

    #[test]
    fn honeycomb_rows_alternate() {
        let blacks = (HONEY_LOW..=HONEY_HIGH).filter(|n| is_black(*n)).count();
        assert_eq!(blacks, 25);
    }
}

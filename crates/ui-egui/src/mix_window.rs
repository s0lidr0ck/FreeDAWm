//! The Mix window: one channel strip per track.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular, rgb};
use crate::widgets::{
    PannerEdit, PannerSpeaker, db_text, fader, fader_slot, groove, input_monitor_key, inset, meter, meter_scale, multi_meter, pan_knob, pan_text,
    raised_key, rec_toggle, selector_box, selector_box_with_tooltip, shade, surround_panner, text_toggle, vgradient,
};
use egui::{Align2, Color32, CornerRadius, Modifiers, Rect, Sense, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ChannelFormat, ModuleId, Route, StripSize, SurroundPan, Track, TrackId, TrackKind};
use soundcraft_playback::MeterSnapshot;

/// A strip's width: Wide 150, Slim 80; `Auto` follows the Mix window's global toggle
/// (`global_wide`, wide unless View › Narrow Mix / Slim Strips is on).
pub fn strip_width(size: StripSize, global_wide: bool) -> f32 {
    if strip_is_wide(size, global_wide) { 150.0 } else { 80.0 }
}

/// Whether a strip of `size` is drawn wide.
pub fn strip_is_wide(size: StripSize, global_wide: bool) -> bool {
    match size {
        StripSize::Auto => global_wide,
        StripSize::Wide => true,
        StripSize::Slim => false,
    }
}

/// The shortest a fader gets when a long chain claims the strip's height.
const FADER_MIN_H: f32 = 140.0;
/// The shortest a chain gets (it scrolls) when the strip is short.
const CHAIN_MIN_H: f32 = 60.0;

pub fn show(app: &mut SoundApp, ui: &mut Ui) {
    // The slim top bar: transport, main counter, selection (Studio One keeps one over its console).
    egui::Panel::top("mix_toolbar")
        .exact_size(crate::toolbar::MIX_HEIGHT)
        .frame(egui::Frame::NONE.fill(Tokens::current().toolbar_bg))
        .show(ui, |ui| crate::toolbar::mix_bar(app, ui));
    crate::docking::show(app, ui, crate::MainWindow::Mix);
}

/// The Mix window in its own native window (Window › Mix in Separate Window), so Edit and Mix can
/// be seen at once, e.g. one per monitor. Closing that window brings Mix back into the main one.
/// Backends without multiple windows show it as a floating egui window instead.
pub fn show_separate(app: &mut SoundApp, ctx: &egui::Context) {
    let title = format!("{}  —  Mix", app.engine.session().name);
    let builder = egui::ViewportBuilder::default().with_title(title).with_inner_size([1100.0, 720.0]).with_min_inner_size([480.0, 320.0]);
    ctx.show_viewport_immediate(crate::window_sync::mix_viewport(), builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        if std::mem::take(&mut app.focus_mix) {
            ui.ctx().send_viewport_cmd(egui::ViewportCommand::Focus);
        }
        // A real second window (backends without one embed Mix in the main window, which then
        // draws everything).
        let viewport = ui.ctx().viewport_id();
        let own = viewport != egui::ViewportId::ROOT;
        let before = crate::window_sync::opened(app);
        if own {
            // Its keys: transport, undo, save… work from here too.
            crate::shortcuts::handle(app, ui.ctx());
            app.ui.wide_key_owner.remove(&viewport);
        }
        show(app, ui);
        if own {
            // What the Mix window opened is drawn over it (claimed now, and again for anything
            // its windows opened in turn).
            crate::window_sync::claim(app, &before, viewport);
            let before = crate::window_sync::opened(app);
            crate::panels::plugin_windows(app, ui.ctx());
            crate::panels::full_editors(app, ui.ctx());
            send_window(app, ui.ctx());
            crate::dialogs::show_open(app, ui.ctx());
            crate::window_sync::claim(app, &before, viewport);
        }
        crate::browser::drop::finish(app, ui.ctx());
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            separate_closed(app);
        }
    });
}

/// The separate Mix window was closed: Mix goes back into the main window.
pub fn separate_closed(app: &mut SoundApp) {
    app.ui.mix_separate = false;
    app.ui.window = crate::MainWindow::Mix;
}

pub(crate) fn content(app: &mut SoundApp, ui: &mut Ui) {
    let sync = app.ui.sync;
    let ids: Vec<TrackId> = {
        let s = app.engine.session();
        s.tracks.iter().filter(|x| crate::window_sync::mix_shows(s, x, sync)).map(|x| x.id).collect()
    };
    // Per-channel peaks for multichannel meters (read once per frame).
    let snap = app.preview_meters.clone().or_else(|| app.player.as_ref().filter(|_| app.meters_live()).map(|p| p.meters()));
    // Every chain gets the room of the longest, so the sections under it line up across strips.
    let chain_h = {
        let s = app.engine.session();
        ids.iter()
            .filter_map(|id| s.track(*id))
            .map(|t| crate::chain_view::chain_height(t, strip_is_wide(t.mixer.strip_size, !app.ui.narrow_mix)))
            .fold(0.0f32, f32::max)
    };
    // The PITCH row's height (the tallest pitch box shown): strips without a box (or with a
    // shorter one) give that room to their chain, so the sections under it still line up.
    let pitch_row = crate::vocal_mixer::row_height(app, &ids);
    crate::band::prune(app);
    // The strips and drawers scroll: there the wheel scrolls, and knobs ignore it.
    crate::widgets::begin_wheel_scrolls(ui.ctx());
    crate::channel_width::bar(app, ui);
    egui::ScrollArea::horizontal().auto_shrink([false, false]).show(ui, |ui| {
        ui.horizontal_top(|ui| {
            ui.spacing_mut().item_spacing.x = 1.0;
            for &id in &ids {
                let r = strip(app, ui, id, snap.as_ref(), chain_h, pitch_row);
                // A track selected in the Edit window scrolls into view here.
                if app.window_sync.reveal_mix == Some(id) {
                    ui.scroll_to_rect(r, None);
                    app.window_sync.reveal_mix = None;
                }
                // A musician strip's band drawer sits right of it, before any module drawer.
                if app.ui.band_drawers.contains(&id) {
                    let session = app.engine.session_arc();
                    if let Some(track) = session.track(id)
                        && let Some(k) = crate::band::musician_of(track)
                    {
                        crate::band::drawer(app, ui, track, k);
                    }
                }
                // An open drawer sits right of its strip; the strips after it shift right.
                if let Some(module) = app.ui.drawers.get(&id).copied() {
                    let session = app.engine.session_arc();
                    if let Some(track) = session.track(id) {
                        let meters = snap.as_ref().and_then(|s| s.tracks.get(&id));
                        // A frozen module's drawer is read-only and greyed (the drawer does it, so
                        // its header menu can still move the freeze point or unfreeze).
                        crate::module_drawer::drawer(app, ui, track, module, meters);
                    }
                }
            }
            // A track with no strip here (hidden, in a closed folder) is not waited for.
            if app.window_sync.reveal_mix.is_some_and(|r| !ids.contains(&r)) {
                app.window_sync.reveal_mix = None;
            }
            let rest = Rect::from_min_max(pos2(ui.cursor().min.x, ui.clip_rect().min.y), ui.clip_rect().max);
            crate::browser::drop::target(app, ui, rest, crate::browser::drop::DropTarget::NewTrack);
            if rest.width() > 4.0 && ui.interact(rest, ui.id().with("past_strips"), Sense::click()).double_clicked() {
                let _ = app.dialogs.open_for_command(&app.engine, "track.new");
            }
        });
    });
    crate::widgets::end_wheel_scrolls(ui.ctx());
}

/// The strip's snowflake key: a click freezes the whole chain, or unfreezes a frozen track (whole
/// or partly frozen). Its tooltip names the freeze point; greyed, saying why, on a track that
/// can't be frozen.
pub(crate) fn strip_freeze_key(app: &mut SoundApp, ui: &mut Ui, track: &Track, size: egui::Vec2) {
    use crate::widgets::FreezeLook;
    let fz = track.frozen_audio().copied();
    let point = fz.and_then(|f| f.upto).and_then(|m| track.mixer.inserts.iter().find(|i| i.id == m));
    let (look, tip) = match (fz, point) {
        (None, _) => (FreezeLook::Live, "Freeze: render the whole chain (right-click the name to freeze up to a module)".to_string()),
        (Some(_), Some(ins)) => {
            (FreezeLook::Partly, format!("Frozen up to {}: the modules below it are live. Click to unfreeze", crate::chain_view::tile_name(ins)))
        }
        (Some(_), None) => (FreezeLook::Whole, "Frozen: the whole chain. Click to unfreeze".to_string()),
    };
    let why = soundcraft_engine::cmd::cannot_freeze(track);
    let label = format!("{} freeze", track.name);
    let resp = ui.add_enabled_ui(why.is_none() || fz.is_some(), |ui| crate::widgets::freeze_key(ui, size, look, &label)).inner;
    let resp = match &why {
        Some(why) if fz.is_none() => resp.on_disabled_hover_text(why.as_str()),
        _ => resp.on_hover_text(tip),
    };
    if resp.clicked() {
        let r = if fz.is_some() {
            app.run("track.unfreeze", json!({"tracks": [track.id.0]}))
        } else {
            app.run("track.freeze", json!({"tracks": [track.id.0], "freeze": true}))
        };
        if let Err(e) = r {
            app.ui.status = e;
        }
    }
}

/// The line under a partly frozen chain's freeze point: the modules above it are frozen, the ones
/// below it live. An ice-blue rule with a snowflake at its left end.
fn freeze_line(ui: &Ui, x: egui::Rangef, y: f32, track: &str) {
    let ice = crate::widgets::ice();
    let r = Rect::from_x_y_ranges(x, (y - 3.0)..=(y + 3.0));
    let resp = ui.interact(r, ui.id().with(("freeze-point", track)), Sense::hover());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{track} freeze point")));
    let p = ui.painter();
    p.hline((x.min + 12.0)..=(x.max - 1.0), y, Stroke::new(2.0, ice));
    p.circle_filled(pos2(x.min + 6.0, y), 5.5, Tokens::current().strip_bg);
    crate::widgets::snowflake(p, pos2(x.min + 6.0, y), 4.0, ice);
    resp.on_hover_text("Frozen up to here: the modules above the line play from the frozen audio, the ones below are live");
}

/// Format of the destination a track's output feeds (the main mix, a bus, or stereo).
fn output_format(app: &SoundApp, track: &Track) -> ChannelFormat {
    let s = app.engine.session();
    match &track.mixer.output {
        Route::Main => s.main_format(),
        Route::Bus(b) => s.bus(*b).map_or(ChannelFormat::Stereo, |b| b.format),
        _ => ChannelFormat::Stereo,
    }
}

/// Speaker dots of a format, placed on the panner square.
fn panner_speakers(fmt: ChannelFormat) -> Vec<PannerSpeaker> {
    let sp = fmt.speakers();
    let layout: Vec<soundcraft_dsp::pan::SpeakerPos> =
        sp.iter().map(|s| soundcraft_dsp::pan::SpeakerPos { az: s.azimuth(), el: s.elevation(), lfe: s.is_lfe() }).collect();
    sp.iter()
        .filter(|s| !s.is_lfe())
        .map(|s| {
            let (x, y) = soundcraft_dsp::pan::azimuth_to_puck(&layout, s.azimuth());
            PannerSpeaker { label: s.label(), x, y, height: s.is_height() }
        })
        .collect()
}

/// Ballistic per-channel meter levels for multichannel strips, kept in egui memory.
fn channel_levels(ui: &Ui, key: (&str, u64), peaks: &[f32], n: usize) -> Vec<f32> {
    let id = egui::Id::new(key);
    let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.25);
    let fall = 10f32.powf(-26.0 * dt / 20.0);
    let mut lv: Vec<f32> = ui.data(|d| d.get_temp(id)).unwrap_or_default();
    lv.resize(n, 0.0);
    for (i, l) in lv.iter_mut().enumerate() {
        let p = peaks.get(i).copied().filter(|x| x.is_finite()).unwrap_or(0.0);
        *l = if p >= *l { p } else { (*l * fall).max(p) };
        if *l < 1e-5 {
            *l = 0.0;
        }
    }
    ui.data_mut(|d| d.insert_temp(id, lv.clone()));
    lv
}

/// Pro Tools' names for sections in narrow strips, and in any strip too narrow for the full ones.
fn short_label(text: &str) -> &str {
    match text {
        "INSERTS A-E" => "I A-E",
        "INSERTS F-J" => "I F-J",
        "SENDS A-E" => "S A-E",
        "SENDS F-J" => "S F-J",
        "INSTRUMENT" => "INST",
        _ => text,
    }
}

/// A section's caption. Narrow strips always use the short names, as Pro Tools' Narrow Mix does, so
/// every narrow strip reads the same whatever the UI font: measuring alone left the full name in
/// place wherever a narrower font (egui's built-in face on a system with no fonts) happened to fit.
/// Other strips shorten a name only when it doesn't fit with 4 pt to spare on each side.
fn section_label(ui: &Ui, r: Rect, text: &str, narrow: bool) {
    let p = ui.painter();
    let color = Tokens::current().section_label;
    let text = if narrow { short_label(text) } else { text };
    let mut galley = p.layout_no_wrap(text.to_owned(), bold(10.0), color);
    if galley.size().x > r.width() - 8.0 {
        galley = p.layout_no_wrap(short_label(text).to_owned(), bold(10.0), color);
    }
    p.galley(Align2::CENTER_CENTER.anchor_size(pos2(r.center().x, r.min.y + 7.0), galley.size()).min, galley, color);
}

pub(crate) fn route_name(app: &SoundApp, r: &Route) -> String {
    match r {
        Route::None => "no output".into(),
        Route::Main => app.engine.session().outputs.first().map_or_else(|| "Out 1-2".into(), |o| o.name.clone()),
        Route::Bus(b) => app.engine.session().bus(*b).map_or_else(|| "bus?".into(), |b| b.name.clone()),
        Route::Hardware(h) => h.clone(),
        Route::Musician(n) => format!("Musician {}", u32::from(*n) + 1),
        Route::Modules(refs) => match refs.as_slice() {
            [one] => module_route_name(app.engine.session(), *one),
            many => format!("{} modules", many.len()),
        },
        Route::Midi(m) => m.label(),
    }
}

/// `Kick › Shale`: a module MIDI input's source track and module, or what is left of one whose
/// track or module is gone.
pub(crate) fn module_route_name(s: &soundcraft_model::Session, m: soundcraft_model::ModuleRef) -> String {
    let Some(t) = s.track(m.track) else { return "missing track".into() };
    match t.mixer.module(m.module) {
        Some(ins) => format!("{} › {}", t.name, crate::chain_view::plugin_name(&ins.plugin)),
        None => format!("{} › missing module", t.name),
    }
}

/// What a track's input box reads: "No Input" (drawn dimmed) when none is assigned.
pub fn input_name(app: &SoundApp, track: &Track) -> String {
    if no_input(track) { "No Input".to_string() } else { route_name(app, &track.mixer.input) }
}

/// A recordable track with no input assigned.
fn no_input(track: &Track) -> bool {
    track.kind.has_playlist() && track.mixer.input == Route::None
}

/// The input button's tooltip and whether it reads dimmed: a module MIDI input lists its sources
/// and dims when none of them sends (`Shale's MIDI Out is off`); any other input says its name.
pub fn input_tooltip(app: &SoundApp, track: &Track) -> (String, bool) {
    let s = app.engine.session();
    let refs = track.mixer.input.module_sources();
    if no_input(track) {
        // The engine still hears device input 1 on an audio track with none assigned.
        let tip = if track.kind == TrackKind::Audio { "No input assigned (it hears device input 1)" } else { "No input assigned" };
        return (tip.to_string(), true);
    }
    if refs.is_empty() {
        return (input_name(app, track), false);
    }
    let names: Vec<String> = refs.iter().map(|m| module_route_name(s, *m)).collect();
    // Which sources send: MIDI Out on (Shale's switch), or a plugin without one that emits.
    let sends = |m: &soundcraft_model::ModuleRef| {
        s.track(m.track).and_then(|t| t.mixer.module(m.module)).is_some_and(|ins| ins.params.get("midiOut").is_none_or(|v| *v >= 0.5))
    };
    let off: Vec<&str> = refs.iter().zip(&names).filter(|(m, _)| !sends(m)).map(|(_, n)| n.as_str()).collect();
    if off.len() == refs.len() {
        let what = if refs.len() == 1 { "Shale's MIDI Out is off".to_string() } else { "MIDI Out is off on every source".to_string() };
        return (format!("{what}\nMIDI from: {}", names.join(", ")), true);
    }
    let mut tip = format!("MIDI from: {}", names.join(", "));
    if !off.is_empty() {
        tip.push_str(&format!("\nMIDI Out off: {}", off.join(", ")));
    }
    (tip, false)
}

/// The input or output menu under `resp`. It closes only on a click outside, so its bus search
/// field can be typed in.
pub(crate) fn route_popup(app: &mut SoundApp, resp: &egui::Response, id: TrackId, input: bool) {
    egui::Popup::menu(resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| route_menu(app, ui, id, input));
}

/// Draws track `id`'s strip and returns its rect.
fn strip(app: &mut SoundApp, ui: &mut Ui, id: TrackId, snap: Option<&MeterSnapshot>, chain_h: f32, pitch_row: f32) -> Rect {
    let t = Tokens::current();
    let session = app.engine.session_arc();
    let Some(track) = session.track(id) else { return Rect::NOTHING };
    let wide = strip_is_wide(track.mixer.strip_size, !app.ui.narrow_mix);
    let narrow = !wide;
    let w = strip_width(track.mixer.strip_size, !app.ui.narrow_mix);
    let h = ui.available_height().max(560.0);
    let (r, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    crate::browser::drop::target(app, ui, r, crate::browser::drop::DropTarget::Track(id));
    let selected = crate::window_sync::shows_selected(app, crate::MainWindow::Mix, id);
    let bg = if selected { t.strip_selected } else { t.strip_bg };
    vgradient(ui.painter(), r, shade(bg, 0.06), bg);
    ui.painter().vline(r.min.x + 0.5, r.y_range(), Stroke::new(1.0, t.edge_light));
    let mut y = r.min.y + 4.0;
    let inner_w = w - 8.0;
    let x0 = r.min.x + 4.0;
    let views = app.ui.mix_views.clone();
    let has = |v: &str| views.iter().any(|x| x == v);
    let all = has("all");
    // Preamp, instrument and object rows keep their height on every strip so sections line up;
    // their controls only appear on tracks with an input.
    let has_input = matches!(track.kind, TrackKind::Audio | TrackKind::Instrument | TrackKind::Midi);
    // Frozen: what was frozen (trim, the modules down to the freeze point) is read-only and greyed
    // until the track is unfrozen; the modules after the point stay live.
    let frozen = track.frozen_audio().is_some();
    if all || has("mic_preamps") {
        // Input gain stage ahead of the inserts (the track's trim).
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 34.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "PREAMP", narrow);
        if has_input {
            let kr = Rect::from_min_size(pos2(x0 + 6.0, sec.min.y + 15.0), vec2(inner_w - 12.0, 16.0));
            let mut db = track.mixer.trim_db;
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            if frozen {
                c.disable();
            }
            let resp = c.add_sized(kr.size(), egui::DragValue::new(&mut db).range(-24.0..=24.0).speed(0.1).suffix(" dB").max_decimals(1));
            if resp.changed() {
                let _ = app.engine.execute_merged("mix.trim", &json!({"tracks": [id.0], "db": db}), &format!("trim:{}", id.0));
            }
            if resp.drag_stopped() || resp.lost_focus() {
                app.engine.end_merge();
            }
        }
        y = sec.max.y + 4.0;
    }
    if all || has("instruments") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 34.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "INSTRUMENT", narrow);
        let sr = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 16.0), vec2(inner_w - 4.0, 15.0));
        let slot = track.mixer.inserts.iter().position(|i| plugin_info(&i.plugin).is_some_and(|p| p.is_instrument));
        match slot {
            _ if track.kind == TrackKind::Instrument => instrument_slot(app, ui, track, sr),
            Some(k) => insert_slot(app, ui, track, k, sr, false),
            None if track.kind == TrackKind::Midi => {
                ui.painter().text(sr.center(), Align2::CENTER_CENTER, "MIDI in: all", regular(9.0), t.text_dim);
            }
            None => {}
        }
        y = sec.max.y + 4.0;
    }
    if all || has("object") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0));
        if has_input {
            let on = app.engine.session().edit.flag(&format!("object.{}", id.0));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(sec));
            let label = egui::RichText::new(if on { "OBJECT" } else { "BED" }).font(bold(9.0));
            if c.add_sized(sec.size(), egui::Button::new(label).selected(on)).on_hover_text("Route as an immersive object or to the bed").clicked() {
                let _ = app.run("track.object", json!({"tracks": [id.0], "object": !on}));
            }
        }
        y = sec.max.y + 4.0;
    }
    // This strip's pitch box (0: none) and the room of the PITCH row it leaves to its chain.
    let own_pitch = if pitch_row > 0.0 { crate::vocal_mixer::box_height(app.engine.session(), track) } else { 0.0 };
    let mut pitch_spare = if pitch_row > 0.0 { pitch_row + 4.0 - if own_pitch > 0.0 { own_pitch + 4.0 } else { 0.0 } } else { 0.0 };
    if all || has("inserts_ae") || has("inserts_fj") {
        // The channel chain (FireStation): tiles with live graphs, scrolling when the strip is
        // too short for all of them.
        let natural = crate::chain_view::chain_height(track, wide);
        let room = r.max.y - y - below_chain_height(app, track, &views, inner_w, pitch_row) - FADER_MIN_H;
        let h = natural.max(chain_h).min(room.max(CHAIN_MIN_H)) + std::mem::take(&mut pitch_spare);
        let cr = Rect::from_min_size(pos2(x0, y), vec2(inner_w, h));
        crate::vocal_editor::hook(ui.ctx(), &format!("mix-{}-chain", id.0), cr);
        let meters = snap.and_then(|s| s.tracks.get(&id));
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("chain", id.0)).max_rect(cr).layout(egui::Layout::top_down(egui::Align::Min)));
        c.set_clip_rect(cr.expand2(vec2(4.0, 2.0)).intersect(ui.clip_rect()));
        let frozen_part = if natural > h + 0.5 {
            // A solid bar beside the tiles, not floating over them (it would hide their ⤢).
            c.spacing_mut().scroll = egui::style::ScrollStyle::solid();
            egui::ScrollArea::vertical()
                .id_salt(("chain-scroll", id.0))
                .max_height(h)
                .auto_shrink([false, false])
                .show(&mut c, |ui| crate::chain_view::chain(app, ui, track, meters, wide))
                .inner
        } else {
            crate::chain_view::chain(app, &mut c, track, meters, wide)
        };
        if frozen {
            // A wash over the frozen modules (the whole chain, or down to the freeze point), so
            // they read as off in either theme, a line under the freeze point, then the badge top
            // right, clear of the first module's name.
            let (part, whole) = frozen_part.unwrap_or((cr, true));
            let wash = if whole { cr } else { Rect::from_min_max(cr.min, pos2(cr.max.x, part.max.y + 2.0)).intersect(cr) };
            if wash.height() > 0.0 {
                ui.painter().rect_filled(wash, 3.0, t.strip_bg.gamma_multiply(0.6));
            }
            if !whole && (cr.min.y..=cr.max.y).contains(&(part.max.y + 2.0)) {
                freeze_line(ui, cr.x_range(), part.max.y + 2.0, &track.name);
            }
            let bw = (inner_w - 2.0).min(70.0);
            let badge = Rect::from_min_size(pos2(x0 + inner_w - 1.0 - bw, cr.min.y + 2.0), vec2(bw, 16.0));
            crate::widgets::frozen_badge(ui, badge, &format!("{} frozen", track.name));
        }
        y = cr.max.y + 6.0;
    }
    if own_pitch > 0.0 {
        // A vocal (or tuned) strip's pitch box: compact, or the full Pitch panel.
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, own_pitch));
        crate::vocal_mixer::pitch_box(app, ui, sec, track);
        y = sec.max.y + 4.0;
    }
    // Without a chain shown, what is left of the row stays blank so Sends and I/O line up.
    y += pitch_spare;
    for (key, title, off) in [("sends_ae", "SENDS A-E", 0usize), ("sends_fj", "SENDS F-J", 5)] {
        if !(all || has(key)) {
            continue;
        }
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, sends_section_height(app.engine.session(), off)));
        crate::vocal_editor::hook(ui.ctx(), &format!("mix-{}-{key}", id.0), sec);
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, title, narrow);
        let mut sy = sec.min.y + 15.0;
        for k in 0..5 {
            let sr = Rect::from_min_size(pos2(x0 + 2.0, sy), vec2(inner_w - 4.0, 15.0));
            send_slot(app, ui, track, off + k, sr);
            if send_expanded(app.engine.session(), off + k) {
                let er = Rect::from_min_size(pos2(x0 + 2.0, sy + 17.0), vec2(inner_w - 4.0, EXPANDED_SEND_H - 2.0));
                expanded_send(app, ui, track, off + k, er, wide);
                sy += EXPANDED_SEND_H;
            }
            sy += 17.0;
        }
        y = sec.max.y + 4.0;
    }
    if all || has("comments") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 44.0));
        let mut c = track.comments.clone();
        let mut child = ui.new_child(egui::UiBuilder::new().max_rect(sec));
        let hint = if narrow { "" } else { "comments" };
        if child.add(egui::TextEdit::multiline(&mut c).desired_width(inner_w).desired_rows(2).hint_text(hint).font(regular(10.0))).changed() {
            let _ = app.engine.execute_merged("track.comments", &json!({"track": id.0, "comments": c}), &format!("comments:{}", id.0));
        }
        y = sec.max.y + 4.0;
    }
    if all || has("io") {
        let sec = Rect::from_min_size(pos2(x0, y), vec2(inner_w, 16.0 + 2.0 * 18.0 + 30.0));
        inset(ui.painter(), sec, t.strip_section);
        section_label(ui, sec, "I / O", narrow);
        // A musician strip keeps a Band key beside its input.
        let band_w = if crate::band::musician_of(track).is_some() { if narrow { 20.0 } else { 34.0 } } else { 0.0 };
        let in_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 15.0), vec2(inner_w - 4.0 - band_w, 16.0));
        if band_w > 0.0 {
            let kr = Rect::from_min_max(pos2(in_r.max.x + 2.0, in_r.min.y), pos2(x0 + inner_w - 2.0, in_r.max.y));
            crate::band::strip_key(app, ui, id, kr, narrow);
        }
        let out_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 33.0), vec2(inner_w - 4.0, 16.0));
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-input", id.0)).max_rect(in_r));
        let (tip, dim) = input_tooltip(app, track);
        let resp = crate::widgets::selector_box_with_tooltip(
            &mut c,
            in_r.width(),
            in_r.height(),
            &input_name(app, track),
            if dim { t.text_dim } else { t.text },
            &tip,
        );
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Input"));
        route_popup(app, &resp, id, true);
        let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-output", id.0)).max_rect(out_r));
        let resp = selector_box(&mut c, out_r.width(), out_r.height(), &route_name(app, &track.mixer.output), t.text);
        resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Output"));
        route_popup(app, &resp, id, false);
        // Automation mode.
        let am_r = Rect::from_min_size(pos2(x0 + 2.0, sec.min.y + 51.0), vec2(inner_w - 4.0, 16.0));
        ui.painter().text(pos2(am_r.center().x, am_r.min.y - 1.0), Align2::CENTER_BOTTOM, "", regular(9.0), t.text_dim);
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(am_r));
        let mode = track.mixer.automation_mode;
        let col = if mode == soundcraft_model::AutomationMode::Read {
            t.auto_read
        } else if mode == soundcraft_model::AutomationMode::Off {
            t.text_dim
        } else {
            t.auto_write
        };
        let full_label = format!("auto {}", mode.label());
        let label = if mode == soundcraft_model::AutomationMode::TouchLatch {
            if narrow { "t/l" } else { mode.label() }
        } else if narrow {
            mode.label()
        } else {
            &full_label
        };
        let resp = selector_box_with_tooltip(&mut c, am_r.width(), am_r.height(), label, col, &full_label);
        egui::Popup::menu(&resp).show(|ui| {
            for m in soundcraft_model::AutomationMode::ALL {
                if ui.selectable_label(m == mode, m.label()).clicked() {
                    let _ = app.run("mix.automation_mode", json!({"track": id.0, "mode": m.label()}));
                }
            }
        });
        y = sec.max.y + 4.0;
    }
    // Group selector.
    let no_group = if narrow { "nogrp" } else { "no group" };
    let gname = app.engine.session().groups.iter().find(|g| g.members.contains(&id)).map_or(no_group.to_string(), |g| g.name.clone());
    let gr = Rect::from_min_size(pos2(x0 + 2.0, y), vec2(inner_w - 4.0, 16.0));
    let mut c = ui.new_child(egui::UiBuilder::new().id_salt(("mix-group", id.0)).max_rect(gr));
    let resp = selector_box(&mut c, gr.width(), gr.height(), &gname, t.text_dim);
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::ComboBox, true, "Group"));
    egui::Popup::menu(&resp).show(|ui| group_menu(app, ui, id));
    y += 22.0;
    // Pan: a surround panner when the output is multichannel, else the stereo knobs.
    let out_fmt = output_format(app, track);
    let surround_out =
        out_fmt.channels() > 2 && !out_fmt.is_ambisonic() && !matches!(track.kind, TrackKind::Master | TrackKind::Vca) && track.channels() <= 2;
    if surround_out {
        let size = (inner_w - 4.0).min(88.0);
        let sp = track.mixer.surround.unwrap_or_else(|| {
            SurroundPan::from_stereo(if track.mixer.pan.len() == 1 { track.mixer.pan.first().copied().unwrap_or(0.0) } else { 0.0 })
        });
        let rows = if out_fmt.has_height() { 2.0 } else { 1.0 };
        let pr = Rect::from_min_size(pos2(x0 + (inner_w - size) * 0.5, y), vec2(size, size + rows * 11.0));
        let mut c = ui.new_child(egui::UiBuilder::new().max_rect(pr).layout(egui::Layout::top_down(egui::Align::Min)));
        c.spacing_mut().item_spacing.y = 1.0;
        let speakers = panner_speakers(out_fmt);
        let z = out_fmt.has_height().then_some(sp.z);
        if let Some(edit) = surround_panner(&mut c, size, &speakers, (sp.x, sp.y), sp.divergence, z) {
            let params = match edit {
                PannerEdit::Position(x, y) => json!({"track": id.0, "x": x, "y": y}),
                PannerEdit::Divergence(d) => json!({"track": id.0, "divergence": d}),
                PannerEdit::Height(h) => json!({"track": id.0, "z": h}),
            };
            let _ = app.engine.execute_merged("mix.surround_pan", &params, &format!("span:{}", id.0));
        }
        y += size + rows * 11.0 + 6.0;
    } else if !track.mixer.pan.is_empty() && track.kind != TrackKind::Vca {
        let n = track.mixer.pan.len().min(2);
        let ks = if n == 2 { (inner_w / 2.0 - 6.0).min(32.0) } else { 34.0 };
        for i in 0..n {
            let cx = if n == 2 { x0 + inner_w * (0.25 + 0.5 * i as f32) } else { x0 + inner_w * 0.5 };
            let kr = Rect::from_center_size(pos2(cx, y + 17.0), vec2(ks, ks));
            let mut c = ui.new_child(egui::UiBuilder::new().max_rect(kr));
            let mut v = track.mixer.pan.get(i).copied().unwrap_or(0.0);
            let before = v;
            let resp = pan_knob(&mut c, ks, &mut v, "Pan (drag; double-click centres)");
            if (v - before).abs() > f32::EPSILON {
                // Alt: every track's pan follows; Alt+Shift: the selected tracks'.
                let m = ui.input(|i| i.modifiers);
                crate::scope::pan(app, crate::MainWindow::Mix, id, i, v, m);
            }
            if resp.dragged() {
                app.hold_control(id, soundcraft_model::AutoParam::Pan(i as u8));
            }
            let _ = resp.on_hover_text("Alt-drag: all tracks · Alt+Shift-drag: selected tracks");
            let pr = Rect::from_center_size(pos2(cx, y + 42.0), vec2(ks + 8.0, 13.0));
            inset(ui.painter(), pr, t.counter_bg);
            let (_, entered) =
                editable_readout(ui, pr, ui.id().with(("pan-value", id.0, i)), &pan_text(v), &format!("{:.0}", v * 100.0), parse_pan, 0.0);
            if let Some(v) = entered {
                let _ = app.engine.execute_merged("mix.pan", &json!({"track": id.0, "pan": v, "index": i}), &format!("pan:{}:{i}", id.0));
            }
        }
        y += 54.0;
    }
    // Input / Rec / Freeze, then Solo / Mute. The Freeze key shares the record row (three keys
    // where a track can be frozen), so every strip's rows stay at the same heights.
    let bw = (inner_w - 6.0) / 2.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    let freezes = crate::chain_view::offers_freeze(track);
    let gap = if freezes { 4.0 } else { 6.0 };
    row.spacing_mut().item_spacing.x = gap;
    let kw = if freezes { (inner_w - 2.0 * gap) / 3.0 } else { bw };
    if track.kind.has_playlist() {
        if input_monitor_key(&mut row, vec2(kw, 18.0), track.mixer.input_monitor, input_monitor_tip(track.mixer.input_monitor)).clicked() {
            let m = row.input(|i| i.modifiers);
            crate::scope::input_monitor(app, crate::MainWindow::Mix, id, m);
        }
        // A frozen track is not recorded on: its R key is greyed until it is unfrozen.
        let rec = row.add_enabled_ui(!frozen, |ui| rec_toggle(ui, vec2(kw, 18.0), track.mixer.record_arm, "Record enable")).inner;
        if rec.on_disabled_hover_text("Unfreeze the track to record on it").clicked() {
            let m = row.input(|i| i.modifiers);
            crate::scope::toggle(app, crate::MainWindow::Mix, id, "mix.record_arm", m);
        }
    }
    if freezes {
        strip_freeze_key(app, &mut row, track, vec2(kw, 18.0));
    }
    y += 22.0;
    let mut row = ui.new_child(
        egui::UiBuilder::new().max_rect(Rect::from_min_size(pos2(x0, y), vec2(inner_w, 20.0))).layout(egui::Layout::left_to_right(egui::Align::Min)),
    );
    row.spacing_mut().item_spacing.x = 6.0;
    if track.kind != TrackKind::Master {
        // The engine's solo rule (solo follows the signal path), worked out once per revision.
        let look = crate::widgets::solo_state(&session, &app.solo_scope(), id);
        let (mut solo_label, mute_label) = look.labels(&track.name);
        let safe = track.mixer.solo_safe;
        if safe {
            solo_label.push_str(", solo safe");
        }
        let mac = row.ctx().os().is_mac();
        let sresp = crate::widgets::solo_key(&mut row, vec2(bw, 18.0), &look, &solo_label);
        if safe {
            crate::widgets::paint_solo_safe(row.painter(), sresp.rect);
        }
        let sresp = sresp.on_hover_text(crate::scope::solo_hint(mac));
        if sresp.clicked() {
            let m = row.input(|i| i.modifiers);
            crate::scope::solo(app, crate::MainWindow::Mix, id, m, mac);
        }
        sresp.context_menu(|ui| crate::scope::solo_menu(app, ui, id));
        if crate::widgets::mute_key(&mut row, vec2(bw, 18.0), track.mixer.mute, &look, &mute_label).clicked() {
            let m = row.input(|i| i.modifiers);
            crate::scope::toggle(app, crate::MainWindow::Mix, id, "mix.mute", m);
        }
    }
    y += 26.0;
    // Fader + meter.
    let bottom_h = 64.0;
    let fh = (r.max.y - bottom_h - y).max(120.0);
    let wide_meter = (if track.kind == TrackKind::Master { app.engine.session().main_format() } else { track.format }).channels() > 2;
    let fr = Rect::from_min_size(pos2(x0, y), vec2(inner_w * if wide_meter { 0.45 } else { 0.6 }, fh));
    // In Trim mode the fader sets the trim the pass writes to the trim lane (centred on 0 dB),
    // as in Pro Tools; the volume stays where it is.
    let trim_mode = track.mixer.automation_mode == soundcraft_model::AutomationMode::Trim;
    let shown_db = if trim_mode { app.engine.trim_shown(id) } else { track.mixer.volume_db };
    if let Some(db) = fader(ui, fr, shown_db, ui.id().with(("fader", id.0))) {
        app.gesture = Some(crate::Gesture::Fader { track: id, start_db: shown_db });
        if trim_mode {
            app.engine.automation_trim(id, Some(db));
        } else {
            // Alt-drag moves every fader, Alt+Shift the selected ones; an Alt+Shift-click resets those.
            let (m, reset) = ui.input(|i| (i.modifiers, !i.pointer.is_decidedly_dragging()));
            crate::scope::volume(
                app,
                crate::MainWindow::Mix,
                id,
                track.mixer.volume_db,
                db,
                reset,
                if reset && !m.shift { Modifiers::NONE } else { m },
            );
        }
    }
    let md = app.meters.get(&id).copied().unwrap_or_default();
    #[cfg(not(target_arch = "wasm32"))]
    let md = crate::net_ui::with_remote_level(app, track, md);
    let slot = fader_slot(fr);
    let mr = Rect::from_min_max(pos2(fr.max.x + 4.0, slot.min.y - 5.0), pos2(x0 + inner_w - 2.0, slot.max.y + if wide_meter { 17.0 } else { 1.0 }));
    if !narrow && !wide_meter {
        meter_scale(ui.painter(), mr, mr.min.x - 1.0);
    }
    let strip_fmt = if track.kind == TrackKind::Master { app.engine.session().main_format() } else { track.format };
    let all_chans = strip_fmt.channels().min(soundcraft_mix::MAX_CHANNELS);
    let chans = if track.kind == TrackKind::Master { 2 } else { track.channels().min(2) };
    if all_chans > 2 {
        // One bar per channel, labelled (L R C LFE …).
        let peaks = snap.and_then(|s| s.tracks.get(&id)).map(|m| m.peaks.as_slice()).unwrap_or(&[]);
        let lv = channel_levels(ui, ("mc_meter", id.0), peaks, all_chans);
        let labels: Vec<String> = (0..all_chans).map(|c| strip_fmt.channel_label(c)).collect();
        multi_meter(ui, mr, &lv, &labels, md.clip);
    } else if chans >= 2 {
        let mw = mr.width() / 2.0 - 1.0;
        meter(ui, Rect::from_min_size(mr.min, vec2(mw, mr.height())), md.level[0], md.hold[0], md.clip);
        meter(ui, Rect::from_min_size(pos2(mr.min.x + mw + 2.0, mr.min.y), vec2(mw, mr.height())), md.level[1], md.hold[1], md.clip);
    } else {
        meter(ui, mr, md.level[0], md.hold[0], md.clip);
    }
    if md.gr > 0.1 {
        ui.painter().text(pos2(mr.center().x, mr.min.y - 3.0), Align2::CENTER_BOTTOM, format!("-{:.0}", md.gr), regular(8.0), t.meter_yellow);
    }
    // A click on the meter clears its clip light and peak hold; Alt-click clears every track's.
    let mresp =
        ui.interact(mr, ui.id().with(("meter_clear", id.0)), Sense::click()).on_hover_text("Click: clear the clip light (Alt-click: all tracks)");
    if mresp.clicked() {
        if ui.input(|i| i.modifiers.alt) {
            let _ = app.run("track.clear_clip_indicators", json!({}));
        } else if let Some(m) = app.meters.get_mut(&id) {
            m.clip = false;
            m.peak = 0.0;
        }
    }
    // The peak-hold readout under the meter (multichannel meters keep that room for their labels).
    if !wide_meter {
        let pr = Rect::from_min_max(pos2(mr.min.x - 3.0, mr.max.y + 3.0), pos2(mr.max.x + 3.0, mr.max.y + 15.0));
        inset(ui.painter(), pr, t.counter_bg);
        let col = if md.peak >= 1.0 { t.meter_red } else { t.text_dim };
        ui.painter().with_clip_rect(pr).text(pr.center(), Align2::CENTER_CENTER, peak_text(md.peak), regular(8.5), col);
        if ui.interact(pr, ui.id().with(("peak_reset", id.0)), Sense::click()).on_hover_text("Peak hold: click to reset").clicked()
            && let Some(m) = app.meters.get_mut(&id)
        {
            m.peak = 0.0;
        }
    }
    // Volume readout and name.
    let vr = Rect::from_min_size(pos2(x0 + 4.0, r.max.y - bottom_h + 4.0), vec2(inner_w - 8.0, 15.0));
    inset(ui.painter(), vr, t.counter_bg);
    let (_, entered) = editable_readout(
        ui,
        vr,
        ui.id().with(("volume-value", id.0)),
        &if trim_mode { format!("trim {}", db_text(shown_db)) } else { db_text(shown_db) },
        &format!("{shown_db:.1}"),
        parse_db,
        0.0,
    );
    if let Some(db) = entered {
        if trim_mode {
            app.engine.automation_trim(id, Some(db));
        } else {
            let _ = app.engine.execute_merged("mix.volume", &json!({"track": id.0, "db": db}), &format!("fader:{}", id.0));
        }
    }
    let kind = match track.kind {
        TrackKind::Audio => "audio",
        TrackKind::Aux => "aux",
        TrackKind::Master => "master",
        TrackKind::Midi => "midi",
        TrackKind::Instrument => "inst",
        TrackKind::Vca => "vca",
        TrackKind::Folder => "folder",
        TrackKind::Video => "video",
    };
    ui.painter().text(pos2(vr.center().x, vr.max.y + 9.0), Align2::CENTER_CENTER, kind, regular(9.5), t.text_dim);
    let nr = Rect::from_min_size(pos2(x0, r.max.y - 26.0), vec2(inner_w, 18.0));
    ui.painter().rect(
        nr,
        CornerRadius::same(2),
        if selected { t.name_field_sel } else { t.name_field },
        Stroke::new(1.0, Color32::BLACK),
        StrokeKind::Inside,
    );
    ui.painter().with_clip_rect(nr).text(nr.center(), Align2::CENTER_CENTER, &track.name, bold(12.0), t.text_dark);
    if track.inactive {
        // An inactive track's strip is greyed down to its name plate, with a plate saying so.
        let wash = Rect::from_min_max(r.min, pos2(r.max.x, nr.min.y - 1.0));
        ui.painter().rect_filled(wash, 0.0, t.strip_bg.gamma_multiply(0.7));
        let plate = Rect::from_center_size(pos2(r.center().x, fr.min.y + 24.0), vec2((inner_w - 4.0).min(64.0), 16.0));
        ui.painter().rect(plate, 2.0, t.counter_bg, Stroke::new(1.0, t.text_dim), StrokeKind::Inside);
        ui.painter().text(plate.center(), Align2::CENTER_CENTER, "INACTIVE", bold(if narrow { 7.5 } else { 9.0 }), t.text_dim);
    }
    let nresp = ui.interact(nr, ui.id().with(("strip_name", id.0)), Sense::click());
    nresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} strip", track.name)));
    if nresp.clicked() {
        let sync = app.ui.sync;
        crate::window_sync::click_track(app, crate::MainWindow::Mix, id, ui.input(|i| i.modifiers), move |s, t| {
            crate::window_sync::mix_shows(s, t, sync)
        });
    }
    if nresp.double_clicked() {
        app.dialogs.open_rename_track(id, &track.name);
    }
    nresp.context_menu(|ui| {
        crate::edit_window::track_context_menu(app, ui, id, crate::MainWindow::Mix);
        ui.separator();
        crate::channel_width::menu(app, ui, id);
    });
    if app.ui.mix_views.iter().any(|v| v == "color") {
        ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x, r.max.y - 5.0), vec2(w, 5.0)), 0.0, rgb(track.color));
    }
    ui.painter().line_segment([pos2(r.max.x, r.min.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
    // Scout's PRE/POST lights either side of the track-type label, above the name plate.
    crate::scout::strip_lights(app, ui, id, Rect::from_center_size(pos2(r.center().x, vr.max.y + 9.0), vec2(inner_w - 4.0, 11.0)));
    r
}

/// The strip's group box menu: tick a group to join or leave it, or make a new one with this
/// track (and the Mix selection when it holds the track).
fn group_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let session = app.engine.session_arc();
    for g in &session.groups {
        let member = g.members.contains(&id);
        if ui.add(egui::Button::selectable(member, format!("{} ({})", g.name, g.letter))).clicked() {
            let members: Vec<u64> = if member {
                g.members.iter().filter(|m| **m != id).map(|m| m.0).collect()
            } else {
                g.members.iter().chain([&id]).map(|m| m.0).collect()
            };
            // The last member leaving takes the group with it.
            let _ = if members.is_empty() {
                app.run("track.ungroup", json!({"group": g.id.0}))
            } else {
                app.run("track.group_modify", json!({"group": g.id.0, "tracks": members}))
            };
            ui.close();
        }
    }
    if !session.groups.is_empty() {
        ui.separator();
    }
    if ui.button("New Group…").clicked() {
        let selected = crate::window_sync::selection_of(app, crate::MainWindow::Mix);
        let members = if selected.contains(&id) { selected } else { vec![id] };
        app.dialogs.open_new_group(&app.engine, &members);
        ui.close();
    }
}

/// Height of a strip's sections under the chain, down to the bottom, without the fader: what
/// the chain must leave room for (it mirrors the layout in `strip`).
fn below_chain_height(app: &SoundApp, track: &Track, views: &[String], inner_w: f32, pitch_row: f32) -> f32 {
    let has = |v: &str| views.iter().any(|x| x == v || x == "all");
    let mut h = if pitch_row > 0.0 { pitch_row + 4.0 } else { 0.0 };
    for (key, off) in [("sends_ae", 0), ("sends_fj", 5)] {
        if has(key) {
            h += sends_section_height(app.engine.session(), off) + 4.0;
        }
    }
    if has("comments") {
        h += 48.0;
    }
    if has("io") {
        h += 16.0 + 2.0 * 18.0 + 30.0 + 4.0;
    }
    h += 22.0; // group
    let out_fmt = output_format(app, track);
    let surround_out =
        out_fmt.channels() > 2 && !out_fmt.is_ambisonic() && !matches!(track.kind, TrackKind::Master | TrackKind::Vca) && track.channels() <= 2;
    if surround_out {
        let rows = if out_fmt.has_height() { 2.0 } else { 1.0 };
        h += (inner_w - 4.0).min(88.0) + rows * 11.0 + 6.0;
    } else if !track.mixer.pan.is_empty() && track.kind != TrackKind::Vca {
        h += 54.0;
    }
    h + 22.0 + 26.0 + 64.0
}

/// An instrument track's INSTRUMENT row: click opens the instrument (see [`instrument_click`]),
/// right-click (or click, when none is set) picks another instrument and offers
/// "Show Parameters…" and "Capture Thumbnail Now". Dropping an audio file on it loads the sampler.
pub(crate) fn instrument_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let ins = track.instrument.as_ref();
    let info = ins.and_then(|i| plugin_info(&i.plugin));
    let resp = ui.interact(r, ui.id().with(("instrument", id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "Instrument"));
    match info {
        Some(info) => used_slot(ui.painter(), r, t.slot_face, Mark::Dot, info.short_name, t.text, None),
        None => empty_slot(ui.painter(), r, resp.hovered(), Mark::Dot),
    }
    instrument_click(app, &resp, id);
    crate::sampler_window::drop_target(app, ui.ctx(), r, id);
    crate::browser::drop::target(app, ui, r, crate::browser::drop::DropTarget::InstrumentSlot(id));
}

/// What a plugin slot shows at its left: a dot for inserts, the send's letter, or an arrow for
/// a send on a short track.
#[derive(Clone, Copy)]
enum Mark {
    Dot,
    Letter(char),
    Arrow,
}

/// Slots narrower than this are a short track's squares: a mark, no name.
const COMPACT_SLOT: f32 = 30.0;
/// A send slot at least this wide (a wide strip's) shows its dB beside the destination.
pub(crate) const WIDE_SEND_SLOT: f32 = 110.0;
const NAME_SIZE: f32 = 9.5;

fn draw_mark(p: &egui::Painter, r: Rect, mark: Mark, color: Color32) {
    let at = r.min + vec2(4.75, 4.75);
    match mark {
        Mark::Dot => {
            p.circle_filled(at, 2.0, color);
        }
        Mark::Letter(l) => {
            p.text(pos2(r.min.x + 2.5, r.center().y), Align2::LEFT_CENTER, l, bold(NAME_SIZE), color);
        }
        Mark::Arrow => {
            let at = at + vec2(0.0, 0.5);
            p.rect_filled(Rect::from_min_max(at + vec2(-2.5, -1.5), at + vec2(0.0, 1.5)), 0.0, color);
            p.add(egui::Shape::convex_polygon(vec![at + vec2(-0.5, -3.5), at + vec2(2.75, 0.0), at + vec2(-0.5, 3.5)], color, Stroke::NONE));
        }
    }
}

fn empty_slot(p: &egui::Painter, r: Rect, hovered: bool, mark: Mark) {
    let t = Tokens::current();
    p.rect_filled(r, 2.0, if hovered { shade(t.slot_empty, 0.06) } else { t.slot_empty });
    let color = if matches!(mark, Mark::Letter(_)) { t.slot_mark.lerp_to_gamma(t.text, 0.55) } else { t.slot_mark };
    draw_mark(p, r, mark, color);
}

/// A used slot, a button a point bigger than the empty cell: the mark, a groove, the name
/// centred, and for sends a level box at the right. It doesn't brighten on hover, which would
/// cost its name contrast.
fn used_slot(p: &egui::Painter, r: Rect, faces: [Color32; 2], mark: Mark, name: &str, color: Color32, level: Option<f32>) {
    used_slot_with(p, r, faces, mark, name, color, level, None);
}

/// [`used_slot`], with a readout (a send's dB) right-aligned before the level box.
#[allow(clippy::too_many_arguments)]
fn used_slot_with(
    p: &egui::Painter,
    r: Rect,
    faces: [Color32; 2],
    mark: Mark,
    name: &str,
    color: Color32,
    level: Option<f32>,
    readout: Option<&str>,
) {
    let t = Tokens::current();
    let outer = r.expand(1.0);
    let top = raised_key(p, outer, faces, false);
    draw_mark(p, r, mark, if matches!(mark, Mark::Dot) { t.slot_mark.lerp_to_gamma(color, 0.35) } else { color });
    if r.width() < COMPACT_SLOT {
        return;
    }
    let groove_x = r.min.x + 10.5;
    groove(p, groove_x, r.y_range(), top);
    let mut text = r.with_min_x(groove_x + 1.5);
    if let Some(k) = level {
        let well = level_icon(r);
        p.rect_filled(well, 1.0, Color32::BLACK);
        let inner = well.shrink(1.0);
        p.rect_filled(inner, 0.0, Color32::from_rgb(36, 38, 8));
        p.rect_filled(inner.with_min_y(inner.max.y - inner.height() * k.clamp(0.0, 1.0)), 0.0, Color32::from_rgb(51, 49, 11));
        text.max.x = well.min.x - 1.0;
    }
    if let Some(v) = readout {
        let g = p.layout_no_wrap(v.to_string(), regular(8.5), Tokens::current().counter_text);
        let w = g.size().x;
        if text.width() - w > 30.0 {
            p.galley(pos2(text.max.x - 2.0 - w, text.center().y - g.size().y / 2.0), g, color);
            text.max.x -= w + 4.0;
        }
    }
    let g = p.layout_no_wrap(name.to_string(), bold(NAME_SIZE), color);
    let x = if g.size().x <= text.width() { text.center().x - g.size().x / 2.0 } else { text.min.x + 1.0 };
    p.with_clip_rect(text).galley(pos2(x, text.center().y - g.size().y / 2.0), g, color);
}

/// Room for a used slot's name (see [`used_slot`]).
fn slot_text_width(r: Rect) -> f32 {
    (r.width() - 12.0 - 2.0).max(0.0)
}

/// `name` cut to fit `width` points of slot text, ending in "…" when cut.
fn ellipsize(p: &egui::Painter, name: &str, width: f32) -> String {
    let fits = |s: &str| p.layout_no_wrap(s.to_string(), bold(NAME_SIZE), Color32::WHITE).size().x <= width;
    if fits(name) {
        return name.to_string();
    }
    let chars: Vec<char> = name.chars().collect();
    for n in (0..chars.len()).rev() {
        let s: String = chars.iter().take(n).collect::<String>().trim_end().to_string() + "…";
        if fits(&s) {
            return s;
        }
    }
    "…".to_string()
}

/// A used send's level icon: the small fader at the right of its slot.
fn level_icon(r: Rect) -> Rect {
    let outer = r.expand(1.0);
    Rect::from_min_max(pos2(outer.max.x - 9.0, outer.min.y + 2.0), pos2(outer.max.x - 4.0, outer.max.y - 2.0))
}

/// A chain module as a named row (the Edit window's Inserts column, a MIDI track's INSTRUMENT
/// row): click opens it (a built-in's drawer in the Mix window, a hosted plugin's own window),
/// Ctrl/Cmd-click bypasses, right-click has the module's menu. Rows past the chain's end are
/// empty; clicking one adds a module at the end.
///
/// `edit_list`: a row of the Edit window's inserts list, named "<module> insert" (long names
/// ellipsized), with "Open in Window" for built-ins; else the Mix window's INSTRUMENT row, named
/// "Insert A…", which never opens a built-in's generic window.
pub(crate) fn insert_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect, edit_list: bool) {
    let t = Tokens::current();
    let id = track.id;
    let ins = track.mixer.inserts.get(slot).cloned();
    let resp = ui.interact(r, ui.id().with(("ins", id.0, slot)), Sense::click());
    crate::browser::drop::target(app, ui, r, crate::browser::drop::DropTarget::InsertSlot(id, slot));
    let info = ins.as_ref().and_then(|i| plugin_info(&i.plugin));
    let name = if edit_list {
        ins.as_ref().map(crate::chain_view::tile_name).unwrap_or_default()
    } else {
        ins.as_ref().map(|i| info.map_or(i.plugin.clone(), |p| p.short_name.to_string())).unwrap_or_default()
    };
    let label = match (edit_list, ins.is_some()) {
        (true, true) => format!("{name} insert"),
        (true, false) => "Add insert".to_string(),
        (false, _) => format!("Insert {}", char::from(b'A'.saturating_add(u8::try_from(slot).unwrap_or(u8::MAX)))),
    };
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, &label));
    match &ins {
        None => empty_slot(ui.painter(), r, resp.hovered(), Mark::Dot),
        Some(i) => {
            let faces = if i.bypass { t.slot_bypassed } else { t.slot_face };
            let color = if info.is_some() { t.text } else { t.text_dim };
            let shown = if edit_list { ellipsize(ui.painter(), &name, slot_text_width(r)) } else { name.clone() };
            used_slot(ui.painter(), r, faces, Mark::Dot, &shown, color, None);
        }
    }
    let Some(ins) = ins else {
        if track.mixer.inserts.len() < soundcraft_model::MAX_MODULES {
            egui::Popup::menu(&resp)
                .close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside)
                .show(|ui| module_picker(app, ui, id, Pick::Add(None), egui::Id::new(("row", slot))));
        }
        return;
    };
    if resp.clicked() {
        if ui.input(|i| i.modifiers.command) {
            // Ctrl/Cmd+Alt: the module at this position on every track (with Shift: selected).
            let m = ui.input(|i| i.modifiers);
            let window = if edit_list { crate::MainWindow::Edit } else { crate::MainWindow::Mix };
            crate::scope::bypass(app, window, id, ins.id, m);
        } else if soundcraft_mix::is_third_party(&ins.plugin) {
            crate::chain_view::open_module(app, id, ins.id);
        } else {
            // A built-in opens in its drawer, in the Mix window.
            app.ui.drawers.insert(id, ins.id);
            let _ = app.run("window.mix", json!({}));
        }
    }
    let cmd = if ui.ctx().os().is_mac() { "⌘" } else { "Ctrl+" };
    let resp = resp.on_hover_text(format!("{name}: click to open · {cmd}click to bypass · right-click for more"));
    resp.context_menu(|ui| {
        ui.set_min_width(170.0);
        if ui.add(egui::Button::selectable(ins.bypass, "Bypass")).clicked() {
            run_pick(app, ui, "mix.module_bypass", json!({"track": id.0, "module": ins.id.0}));
        }
        if edit_list && crate::module_drawer::has_drawer(&ins.plugin) && ui.button("Open in Window").clicked() {
            // The generic window: every parameter as a slider, outside the Mix window.
            crate::chain_view::open_window(app, id, ins.id);
            ui.close();
        }
        if soundcraft_mix::is_third_party(&ins.plugin) {
            crate::chain_view::hosted_menu_items(app, ui, id, soundcraft_mix::SlotKey::Module(ins.id));
        }
        if ui.button("Remove").clicked() {
            app.ui.drawers.retain(|t, m| !(*t == id && *m == ins.id));
            run_pick(app, ui, "mix.module_remove", json!({"track": id.0, "module": ins.id.0}));
        }
        ui.separator();
        ui.menu_button("Replace", |ui| module_picker(app, ui, id, Pick::Replace(ins.id), egui::Id::new(("row-replace", slot))));
    });
}

/// What picking a plugin in [`module_picker`] does.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Pick {
    /// Adds it at this position of the chain (`None`: at the end).
    Add(Option<usize>),
    /// Puts it where this module is, in its place (one undo step).
    Replace(ModuleId),
}

/// The picker's groups of built-in plugins (effects, or `instruments`), in order: the FireStation
/// engines and the stock plugins listed with them ([`soundcraft_dsp::PluginInfo::with_engines`])
/// first (by name), then the rest by category in the registry's order ("Utilities" for
/// the effects, "Built-in" for the instruments). Only the listed plugins
/// ([`soundcraft_dsp::PluginInfo::listed`]): the stock effects are hidden.
pub fn picker_groups(instruments: bool) -> Vec<(String, Vec<&'static soundcraft_dsp::PluginInfo>)> {
    use soundcraft_dsp::Category;
    let listed = || soundcraft_dsp::listed_plugins().filter(|p| p.is_instrument == instruments);
    let mut out = Vec::new();
    let mut fs: Vec<_> = listed().filter(|p| p.with_engines()).collect();
    fs.sort_by_cached_key(|p| p.name.to_lowercase());
    if !fs.is_empty() {
        out.push(("FireStation".to_string(), fs));
    }
    let mut cats: Vec<Category> = Vec::new();
    for p in listed().filter(|p| !p.with_engines()) {
        if !cats.contains(&p.category) {
            cats.push(p.category);
        }
    }
    for c in cats {
        let name = match c {
            Category::Other => "Utilities".to_string(),
            Category::Instrument => "Built-in".to_string(),
            c => format!("{c:?}"),
        };
        out.push((name, listed().filter(|p| !p.with_engines() && p.category == c).collect()));
    }
    out
}

/// How well a plugin name answers a search (`query` lower-case), lower first: the exact name,
/// then a name that starts with it, then a word that does, then anywhere in it; on a tie the
/// FireStation engines before SoundCraft's own. `None`: no match.
pub fn search_rank(query: &str, p: &soundcraft_dsp::PluginInfo) -> Option<(u8, u8)> {
    let name = p.name.to_lowercase();
    let tier = if name == query {
        0
    } else if name.starts_with(query) {
        1
    } else if name.split(|c: char| !c.is_alphanumeric()).any(|w| w.starts_with(query)) {
        2
    } else if name.contains(query) {
        3
    } else {
        return None;
    };
    Some((tier, u8::from(!p.with_engines())))
}

/// The listed built-in plugins matching a search (`query` lower-case), best first (see
/// [`search_rank`]), then by name.
pub fn ranked_built_ins(query: &str, instruments: bool) -> Vec<&'static soundcraft_dsp::PluginInfo> {
    let mut hits: Vec<((u8, u8), String, &'static soundcraft_dsp::PluginInfo)> = soundcraft_dsp::listed_plugins()
        .filter(|p| p.is_instrument == instruments)
        .filter_map(|p| Some((search_rank(query, p)?, p.name.to_lowercase(), p)))
        .collect();
    hits.sort_by(|a, b| (a.0, &a.1).cmp(&(b.0, &b.1)));
    hits.into_iter().map(|(_, _, p)| p).collect()
}

/// The plugin picker: a search field that takes typing at once, then (until something is typed)
/// the FireStation engines, the utilities and the hosted plugins by vendor (all formats in one
/// list, see [`vendor_groups`]).
pub(crate) fn module_picker(app: &mut SoundApp, ui: &mut Ui, id: TrackId, pick: Pick, salt: egui::Id) {
    let search = MenuSearch::show(ui, egui::Id::new(("plugin-search", id.0, salt)), "Search plugins…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, false) {
            pick_module(app, ui, id, pick, &plugin);
        }
        return;
    }
    let groups = picker_groups(false);
    let fs_first = groups.first().is_some_and(|(g, _)| g == "FireStation");
    for (i, (group, list)) in groups.into_iter().enumerate() {
        ui.menu_button(group, |ui| {
            for p in list {
                if ui.button(p.name).clicked() {
                    pick_module(app, ui, id, pick, p.id);
                }
            }
        });
        if i == 0 && fs_first {
            // The owner's engines stand apart from SoundCraft's own.
            ui.separator();
        }
    }
    ui.separator();
    if let Some(plugin) = hosted_menus(ui, false) {
        pick_module(app, ui, id, pick, &plugin);
    }
}

/// Adds (or swaps in) the plugin picked and, as DAWs do, opens it once the audio engine has it
/// (`SoundApp::open_inserted`).
fn pick_module(app: &mut SoundApp, ui: &Ui, track: TrackId, pick: Pick, plugin: &str) {
    let r = match pick {
        Pick::Add(at) => app.run("mix.module_add", json!({"track": track.0, "plugin": plugin, "at": at})),
        Pick::Replace(old) => replace_module(app, track, old, plugin),
    };
    match r {
        Ok(v) => {
            if let Some(m) = v["module"].as_u64().and_then(|m| u32::try_from(m).ok()) {
                app.pending_open = Some((track, ModuleId(m)));
            }
        }
        Err(e) => app.ui.status = e,
    }
    ui.close();
}

/// Swaps module `old` for a new `plugin` module at the same place, as one undo step.
pub(crate) fn replace_module(app: &mut SoundApp, track: TrackId, old: ModuleId, plugin: &str) -> Result<serde_json::Value, String> {
    let at = app.engine.session().track(track).and_then(|t| t.mixer.module_index(old)).ok_or("the module is gone")?;
    if app.edit_waits("mix.module_add") {
        return Err(app.ui.status.clone());
    }
    // Checked first, so an unknown plugin leaves the chain and the undo history untouched.
    if plugin_info(plugin).is_none() {
        return Err(format!("unknown plugin `{plugin}`"));
    }
    // Undo puts back what is set in the plugin now, not what was last saved.
    app.capture_module_state(track, old);
    let key = format!("replace:{}:{}", track.0, old.0);
    app.engine.end_merge();
    // Removed first, so a full chain can still swap a module.
    let removed = app.engine.execute_merged("mix.module_remove", &json!({"track": track.0, "module": old.0}), &key);
    let r = removed.and_then(|_| {
        let params = json!({"track": track.0, "plugin": plugin, "at": at});
        let added = app.engine.execute_merged("mix.module_add", &params, &key);
        if added.is_ok() {
            app.note_placed("mix.module_add", &params);
        } else {
            // Put the old module back, leaving nothing to redo.
            app.engine.undo_discard();
        }
        added
    });
    app.engine.end_merge();
    if r.is_ok() && app.ui.drawers.get(&track) == Some(&old) {
        app.ui.drawers.remove(&track);
    }
    r.map_err(|e| e.to_string())
}

/// What clicking an instrument track's instrument (its Mix slot, its Edit window field) does:
/// opens the instrument itself, a hosted one's own editor (else its generic window), a built-in's
/// window (Juke, Lift, the parameters of the others). The picker with every instrument is on
/// right-click, or on click while the track has no instrument.
pub(crate) fn instrument_click(app: &mut SoundApp, resp: &egui::Response, id: TrackId) {
    let slot = soundcraft_mix::SlotKey::Instrument;
    let has = app
        .engine
        .session()
        .track(id)
        .and_then(|t| t.instrument.as_ref())
        .is_some_and(|i| soundcraft_mix::is_third_party(&i.plugin) || plugin_info(&i.plugin).is_some());
    if resp.clicked() && has {
        crate::chain_view::open_hosted(app, id, slot);
    }
    let menu_resp = if has {
        resp.clone().on_hover_text("Click: open the instrument · right-click: change it, parameters, thumbnail")
    } else {
        resp.clone().on_hover_text("Instrument")
    };
    let popup = if has { egui::Popup::context_menu(&menu_resp) } else { egui::Popup::menu(&menu_resp) };
    popup.close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| instrument_menu(app, ui, id));
}

/// The instrument picker of an instrument track: a search field that takes typing at once, then
/// (until something is typed) its instrument's editor, the built-in instruments and the hosted
/// ones by vendor (all formats in one list, see [`vendor_groups`]).
pub fn instrument_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId) {
    let (slot, key) = (soundcraft_mix::SlotKey::Instrument, soundcraft_mix::SlotKey::Instrument);
    let search = MenuSearch::show(ui, egui::Id::new(("instrument-search", id.0)), "Search instruments…");
    if search.typing() {
        if let Some(plugin) = plugin_search(ui, &search, true) {
            run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
        }
        return;
    }
    let current = app.engine.session().track(id).and_then(|t| t.instrument.clone());
    if let Some(current) = current.as_ref().filter(|i| soundcraft_mix::is_third_party(&i.plugin)) {
        // A hosted instrument: its own editor, its generic window and its browser picture.
        let name = crate::chain_view::plugin_name(&current.plugin);
        ui.label(egui::RichText::new(&name).strong());
        if let Some(p) = &app.player
            && p.has_editor(id, key)
        {
            if p.editor_open(id, key) {
                if ui.button("Close Plugin Editor").clicked() {
                    p.close_editor(id, key);
                    ui.close();
                }
            } else if ui.button("Open Plugin Editor").clicked() {
                if let Err(e) = p.open_editor(id, key) {
                    app.ui.status = format!("{name}: {e}");
                }
                ui.close();
            }
        }
        crate::chain_view::hosted_menu_items(app, ui, id, key);
        ui.separator();
    } else if let Some(info) = current.as_ref().and_then(|i| plugin_info(&i.plugin)) {
        ui.label(egui::RichText::new(info.name).strong());
        if ui.button("Edit Parameters…").clicked() {
            if !app.ui.plugin_windows.contains(&(id, slot)) {
                app.ui.plugin_windows.push((id, slot));
            }
            ui.close();
        }
        ui.separator();
    }
    let mut picked = None;
    for (group, list) in picker_groups(true) {
        ui.menu_button(group, |ui| {
            for p in list {
                if ui.button(p.name).clicked() {
                    picked = Some(p.id.to_string());
                }
            }
        });
    }
    ui.separator();
    if let Some(p) = hosted_menus(ui, true) {
        picked = Some(p);
    }
    if let Some(plugin) = picked {
        run_pick(app, ui, "mix.instrument", json!({"track": id.0, "plugin": plugin}));
    }
}

/// Runs a menu's pick, shows its error in the status line and closes the menu.
fn run_pick(app: &mut SoundApp, ui: &Ui, cmd: &str, params: serde_json::Value) {
    if let Err(e) = app.run(cmd, params) {
        app.ui.status = e;
    }
    ui.close();
}

/// The plugins whose name or vendor holds the search text: the built-in ones (best match first,
/// see [`search_rank`]), then Audio Units
/// (listing them creates no instances) and whichever of CLAP and VST3 have been scanned, with a
/// button to scan each of the others. Returns the id picked.
fn plugin_search(ui: &mut Ui, search: &MenuSearch, instruments: bool) -> Option<String> {
    let built_in = ranked_built_ins(&search.query, instruments);
    let (clap, vst3) = (soundcraft_clap_host::cached(), soundcraft_vst3_host::cached());
    let mut hosted: Vec<(MenuPlugin, &str)> = Vec::new();
    for d in clap.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "CLAP"));
    }
    for d in vst3.iter().flatten().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id.clone(), name: d.name.clone(), vendor: d.vendor.clone() }, "VST3"));
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments) {
        hosted.push((MenuPlugin { id: d.id, name: d.name, vendor: d.vendor }, "AU"));
    }
    hosted.retain(|(p, _)| search.matches(&p.name) || search.matches(&p.vendor));
    hosted.sort_by_cached_key(|(p, format)| (p.name.to_lowercase(), *format));
    let n = built_in.len() + hosted.len();
    let picked = search.rows(ui, n, 320.0, |ui, i, highlighted| match built_in.get(i) {
        Some(p) => menu_row(ui, highlighted, p.name),
        None => {
            let (p, format) = &hosted[i - built_in.len()];
            let label = if p.vendor.is_empty() { format!("{} ({format})", p.name) } else { format!("{} — {} ({format})", p.name, p.vendor) };
            menu_row(ui, highlighted, &label)
        }
    });
    if n == 0 {
        ui.weak("No matches");
    }
    if clap.is_none() || vst3.is_none() {
        ui.separator();
        scan_row(ui);
    }
    picked.map(|i| built_in.get(i).map_or_else(|| hosted[i - built_in.len()].0.id.clone(), |p| p.id.to_string()))
}

/// A picker menu's search field. It has the keyboard as soon as the menu opens and starts empty
/// each time; Up/Down move the highlight through the matches and Enter picks it.
struct MenuSearch {
    id: egui::Id,
    query: String,
    highlight: usize,
    keys: [bool; 3],
}

impl MenuSearch {
    fn show(ui: &mut Ui, id: egui::Id, hint: &str) -> Self {
        let pass = ui.ctx().cumulative_pass_nr();
        // Kept only while the menu stays open: one not shown last pass has just opened.
        let kept = ui.data(|d| d.get_temp::<(String, usize, u64)>(id)).filter(|s| s.2 + 1 >= pass);
        let opened = kept.is_none();
        let (mut text, mut highlight, _) = kept.unwrap_or_default();
        let field = ui.add(egui::TextEdit::singleline(&mut text).id(id.with("field")).hint_text(hint).return_key(None));
        if opened {
            field.request_focus();
        }
        if field.changed() {
            highlight = 0;
        }
        // egui's focus, as typing uses: `has_focus` also wants the window focused, which scripted input lacks.
        let focused = ui.memory(|m| m.has_focus(field.id));
        let keys = [egui::Key::ArrowUp, egui::Key::ArrowDown, egui::Key::Enter].map(|k| focused && ui.input(|i| i.key_pressed(k)));
        ui.data_mut(|d| d.insert_temp(id, (text.clone(), highlight, pass)));
        Self { id, query: text.trim().to_lowercase(), highlight, keys }
    }

    fn typing(&self) -> bool {
        !self.query.is_empty()
    }

    fn matches(&self, text: &str) -> bool {
        text.to_lowercase().contains(&self.query)
    }

    /// `n` matches in a scrolling list, each drawn by `row` (told whether it is highlighted);
    /// returns the one clicked, or the highlighted one on Enter.
    fn rows(&self, ui: &mut Ui, n: usize, max_height: f32, mut row: impl FnMut(&mut Ui, usize, bool) -> bool) -> Option<usize> {
        let [up, down, enter] = self.keys;
        let typing = self.typing();
        let highlight = match (up, down) {
            (_, true) => self.highlight + 1,
            (true, _) => self.highlight.saturating_sub(1),
            _ => self.highlight,
        }
        .min(n.saturating_sub(1));
        ui.data_mut(|d| d.get_temp_mut_or_default::<(String, usize, u64)>(self.id).1 = highlight);
        let mut picked = (typing && enter && n > 0).then_some(highlight);
        let row_h = ui.spacing().interact_size.y;
        let step = row_h + ui.spacing().item_spacing.y;
        egui::ScrollArea::vertical().id_salt(self.id.with("rows")).max_height(max_height).show_rows(ui, row_h, n, |ui, range| {
            if typing && (up || down) {
                let top = ui.max_rect().top() + (highlight as f32 - range.start as f32) * step;
                ui.scroll_to_rect(Rect::from_min_size(pos2(ui.max_rect().left(), top), vec2(1.0, row_h)), None);
            }
            for i in range {
                if row(ui, i, typing && i == highlight) {
                    picked = Some(i);
                }
            }
        });
        picked
    }
}

fn menu_row(ui: &mut Ui, selected: bool, text: &str) -> bool {
    let text = if selected { egui::RichText::new(text).color(Color32::WHITE) } else { egui::RichText::new(text) };
    ui.add(egui::Button::selectable(selected, text).min_size(vec2(0.0, ui.spacing().interact_size.y))).clicked()
}

/// A hosted plugin as the plugin menus list it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct MenuPlugin {
    pub id: String,
    pub name: String,
    pub vendor: String,
}

/// The plugin scan's line in the menus: its progress while it runs (in the background, out of
/// process), else a button that starts it.
fn scan_row(ui: &mut Ui) {
    match crate::browser::scan_job::running() {
        Some((done, total)) => {
            ui.weak(if total == 0 { "Scanning plugins…".to_string() } else { format!("Scanning {done} / {total}…") });
        }
        None => {
            if ui.small_button("Scan plugins").clicked() {
                crate::browser::scan_job::request(false);
            }
        }
    }
}

/// Hosted plugins grouped by vendor, as the plugin menus list them (one flat list, whatever the
/// format): the vendors by name, the plugins by name inside each, and a plugin that comes in
/// several formats once, in the first one `list` has it in (give CLAP's first, then VST3's, then
/// Audio Units'). The search still lists every format. An empty vendor is "Unknown vendor".
pub(crate) fn vendor_groups(list: Vec<MenuPlugin>) -> Vec<(String, Vec<MenuPlugin>)> {
    let key = |s: &str| s.trim().to_lowercase();
    let mut seen = std::collections::HashSet::new();
    let mut kept: Vec<MenuPlugin> = list.into_iter().filter(|p| seen.insert((key(&p.vendor), key(&p.name)))).collect();
    kept.sort_by_cached_key(|p| (key(&p.vendor), key(&p.name)));
    let mut out: Vec<(String, Vec<MenuPlugin>)> = Vec::new();
    for p in kept {
        match out.last_mut() {
            Some((_, l)) if l.first().is_some_and(|f| key(&f.vendor) == key(&p.vendor)) => l.push(p),
            _ => {
                let v = p.vendor.trim();
                out.push((if v.is_empty() { "Unknown vendor".to_string() } else { v.to_string() }, vec![p]));
            }
        }
    }
    out
}

/// The hosted plugins (effects or `instruments`) by vendor ([`vendor_groups`]): CLAP's and VST3's
/// last scans (the scan cache) and Audio Units; and whether both CLAP and VST3 have scanned.
fn hosted_groups(instruments: bool) -> (Vec<(String, Vec<MenuPlugin>)>, bool) {
    let (clap, vst3) = (soundcraft_clap_host::cached(), soundcraft_vst3_host::cached());
    let scanned = clap.is_some() && vst3.is_some();
    let mut all = Vec::new();
    for d in clap.into_iter().flatten().filter(|d| d.is_instrument == instruments) {
        all.push(MenuPlugin { id: d.id, name: d.name, vendor: d.vendor });
    }
    for d in vst3.into_iter().flatten().filter(|d| d.is_instrument == instruments) {
        all.push(MenuPlugin { id: d.id, name: d.name, vendor: d.vendor });
    }
    for d in soundcraft_au_host::scan().into_iter().filter(|d| d.is_instrument == instruments) {
        all.push(MenuPlugin { id: d.id, name: d.name, vendor: d.vendor });
    }
    (vendor_groups(all), scanned)
}

/// The hosted plugins' part of a picker: one submenu per vendor (as Pro Tools lists plug-ins by
/// manufacturer), the vendors and each vendor's list scrolling within the screen so that every
/// plugin stays reachable however many are installed. Until CLAP and VST3 have both scanned, or
/// when nothing was found, a row at the bottom says so with a button that starts the scan in the
/// background. Returns the id picked.
fn hosted_menus(ui: &mut Ui, instruments: bool) -> Option<String> {
    let (groups, scanned) = hosted_groups(instruments);
    let max_h = (ui.ctx().content_rect().height() - 80.0).max(160.0);
    let mut picked = None;
    if !groups.is_empty() {
        egui::ScrollArea::vertical().id_salt("vendors").max_height(max_h).show(ui, |ui| {
            for (vendor, list) in &groups {
                ui.menu_button(vendor, |ui| {
                    egui::ScrollArea::vertical().id_salt("plugins").max_height(max_h).show(ui, |ui| {
                        for p in list {
                            if ui.button(&p.name).clicked() {
                                picked = Some(p.id.clone());
                            }
                        }
                    });
                });
            }
        });
    }
    if !scanned {
        ui.label("Plugins are not scanned yet");
        scan_row(ui);
    } else if groups.is_empty() {
        ui.label(if instruments { "No instrument plugins found" } else { "No plugins found" });
        scan_row(ui);
    }
    picked
}

/// A built-in plugin's description, else a hosted CLAP (`clap:<id>`), VST3 (`vst3:<class id>`) or
/// Audio Unit (`au:<type>:<subtype>:<manufacturer>`) plugin's.
pub fn plugin_info(id: &str) -> Option<&'static soundcraft_dsp::PluginInfo> {
    soundcraft_dsp::plugin_info(id)
        .or_else(|| soundcraft_clap_host::plugin_info(id))
        .or_else(|| soundcraft_vst3_host::plugin_info(id))
        .or_else(|| soundcraft_au_host::plugin_info(id))
}

/// The rows an expanded send adds under its slot: its fader with the dB, then pan, PRE and M.
const EXPANDED_SEND_H: f32 = 36.0;

/// Whether View › Expanded Sends shows send `slot` (0 = A) expanded on the Mix strips (the
/// session's `expanded_send.a`..`j` flags, `view.expanded_send_a`..`j`, `_all`, `_none`).
pub fn send_expanded(s: &soundcraft_model::Session, slot: usize) -> bool {
    slot < 10 && s.edit.flag(&format!("expanded_send.{}", (b'a' + slot as u8) as char))
}

/// Height of a SENDS section whose first slot is `off`: five slots, and the expanded ones' rows.
fn sends_section_height(s: &soundcraft_model::Session, off: usize) -> f32 {
    let expanded = (off..off + 5).filter(|k| send_expanded(s, *k)).count() as f32;
    16.0 + 5.0 * 17.0 + expanded * EXPANDED_SEND_H
}

/// View > Expanded Sends: under a send's slot, a mini fader with its dB (drag; Shift for fine,
/// Alt-click for 0 dB, double-click for the Send window), then its pan (drag; double-click
/// centres), PRE and M. Blank when the slot has no send, so every strip's rows line up.
fn expanded_send(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect, wide: bool) {
    let t = Tokens::current();
    let id = track.id;
    let Some(send) = track.mixer.sends.get(slot).cloned().flatten() else { return };
    let which = (b'A' + slot as u8) as char;
    let label = |what: &str| format!("{} Send {which} {what}", track.name);
    // Row 1: the fader bar, its level as a fill along the fader taper, the dB over it.
    let fr = Rect::from_min_size(r.min, vec2(r.width(), 16.0));
    let fresp = ui.interact(fr, ui.id().with(("xsend-fader", id.0, slot)), Sense::click_and_drag());
    let shown = db_text(send.level_db);
    fresp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(send.level_db), label("level")));
    let p = ui.painter();
    p.rect_filled(fr, 2.0, t.counter_bg);
    let pos = soundcraft_model::fader_db_to_pos(send.level_db).clamp(0.0, 1.0);
    let fill = if send.mute { t.mute.gamma_multiply(0.45) } else { t.accent.gamma_multiply(0.45) };
    p.rect_filled(Rect::from_min_max(fr.min, pos2(fr.min.x + fr.width() * pos, fr.max.y)), 2.0, fill);
    let unity = fr.min.x + fr.width() * soundcraft_model::fader_db_to_pos(0.0);
    p.vline(unity, fr.y_range(), Stroke::new(1.0, t.text_dim));
    let text = if wide { format!("{shown} dB") } else { shown.clone() };
    p.text(fr.center(), Align2::CENTER_CENTER, text, regular(9.0), t.counter_text);
    if fresp.dragged() {
        let (dx, fine) = ui.input(|i| (i.pointer.delta().x, i.modifiers.shift));
        // The whole taper over the bar's width (a tenth of it with Shift).
        let per_point = if fine { 0.1 } else { 1.0 } / fr.width().max(1.0);
        let db = soundcraft_model::fader_pos_to_db((pos + dx * per_point).clamp(0.0, 1.0)).clamp(-144.0, 12.0);
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
    }
    if fresp.drag_stopped() {
        app.engine.end_merge();
    }
    if fresp.clicked() && ui.input(|i| i.modifiers.alt) {
        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
    } else if fresp.double_clicked() {
        app.ui.send_window = Some((id, slot));
    }
    let _ = fresp.on_hover_text(format!(
        "Send {which} to {}: {shown} dB\nDrag to adjust · Shift-drag for fine control · Alt-click for 0 dB · double-click for the Send window",
        route_name(app, &send.target)
    ));
    // Row 2: pan, PRE, M.
    let y = fr.max.y + 2.0;
    let (pre_w, m_w) = if wide { (30.0, 18.0) } else { (16.0, 16.0) };
    let mr = Rect::from_min_size(pos2(r.max.x - m_w, y), vec2(m_w, 16.0));
    let pr = Rect::from_min_size(pos2(mr.min.x - 2.0 - pre_w, y), vec2(pre_w, 16.0));
    let panr = Rect::from_min_max(pos2(r.min.x, y), pos2(pr.min.x - 2.0, y + 16.0));
    let presp = ui.interact(panr, ui.id().with(("xsend-pan", id.0, slot)), Sense::click_and_drag());
    presp.widget_info(|| egui::WidgetInfo::slider(true, f64::from(send.pan), label("pan")));
    let p = ui.painter();
    p.rect_filled(panr, 2.0, t.counter_bg);
    let cx = panr.center().x;
    let px = cx + send.pan.clamp(-1.0, 1.0) * panr.width() / 2.0;
    let bar = Rect::from_x_y_ranges(cx.min(px)..=cx.max(px).max(cx.min(px) + 1.0), panr.shrink2(vec2(0.0, 3.0)).y_range());
    p.rect_filled(bar, 1.0, t.accent.gamma_multiply(0.45));
    p.vline(cx, panr.y_range(), Stroke::new(1.0, t.text_dim));
    p.text(panr.center(), Align2::CENTER_CENTER, pan_text(send.pan), regular(9.0), t.counter_text);
    if presp.dragged() {
        let dx = ui.input(|i| i.pointer.delta().x);
        let pan = (send.pan + dx * 2.0 / panr.width().max(1.0)).clamp(-1.0, 1.0);
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "pan": pan}), &format!("send-pan:{}:{slot}", id.0));
    }
    if presp.drag_stopped() {
        app.engine.end_merge();
    }
    if presp.double_clicked() {
        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pan": 0.0}));
    }
    let _ = presp.on_hover_text("Send pan: drag; double-click centres");
    let mut keys = ui.new_child(
        egui::UiBuilder::new().id_salt(("xsend-keys", id.0, slot)).max_rect(pr.union(mr)).layout(egui::Layout::left_to_right(egui::Align::Center)),
    );
    keys.spacing_mut().item_spacing.x = 2.0;
    let pre_resp = text_toggle(&mut keys, pr.size(), if wide { "PRE" } else { "P" }, send.pre_fader, t.accent, "Pre-fader");
    pre_resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, send.pre_fader, label("pre-fader")));
    if pre_resp.clicked() {
        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": !send.pre_fader}));
    }
    let m_resp = text_toggle(&mut keys, mr.size(), "M", send.mute, t.mute, "Mute send");
    m_resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Button, true, send.mute, label("mute")));
    if m_resp.clicked() {
        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": !send.mute}));
    }
}

pub(crate) fn send_slot(app: &mut SoundApp, ui: &mut Ui, track: &Track, slot: usize, r: Rect) {
    let t = Tokens::current();
    let id = track.id;
    let snd = track.mixer.sends.get(slot).cloned().flatten();
    let compact = r.width() < COMPACT_SLOT;
    // As in Pro Tools, only the fader icon opens a send's level; the rest of the slot picks its
    // destination. A short track's square has no icon, so all of it is the level.
    let level_r = if compact { r } else { level_icon(r).expand2(vec2(3.0, 0.0)).with_min_y(r.min.y).with_max_y(r.max.y) };
    let route_r = Rect::from_min_max(r.min, pos2((level_r.min.x - 1.0).max(r.min.x), r.max.y));
    let route_resp = ui.interact(if snd.is_some() { route_r } else { r }, ui.id().with(("snd-route", id.0, slot)), Sense::click());
    route_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Send {}", (b'A' + slot as u8) as char)));
    let mark = if compact { Mark::Arrow } else { Mark::Letter((b'a' + slot as u8) as char) };
    match &snd {
        Some(s) => {
            let name = route_name(app, &s.target);
            let level = soundcraft_model::fader_db_to_pos(s.level_db);
            let color = if s.mute { t.mute } else { t.text };
            // Wide strips read the send's dB in the slot, no hover needed.
            let readout = (r.width() >= WIDE_SEND_SLOT).then(|| db_text(s.level_db));
            used_slot_with(ui.painter(), r, t.slot_face, mark, &name, color, Some(level), readout.as_deref());
            let which = (b'A' + slot as u8) as char;
            let shown = app.ui.send_window == Some((id, slot));
            let level_resp = ui.interact(level_r, ui.id().with(("send-value", id.0, slot)), Sense::click_and_drag());
            level_resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("Send {which} level")));
            if shown {
                ui.painter().rect_stroke(level_r.shrink(0.5), 1.0, Stroke::new(1.5, t.accent), StrokeKind::Inside);
            }
            if level_resp.clicked() {
                if ui.input(|i| i.modifiers.alt) {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
                } else {
                    app.ui.send_window = (!shown).then_some((id, slot));
                }
            }
            if level_resp.dragged() {
                app.hold_control(id, soundcraft_model::AutoParam::SendLevel(slot as u8));
                // Use this frame's pointer movement. `Response::drag_delta()` is cumulative, so
                // applying it to the already-updated value made trackpad adjustment accelerate.
                let (dy, fine) = ui.input(|i| (i.pointer.delta().y, i.modifiers.shift));
                let db = dragged_send_db(s.level_db, dy, fine);
                let _ =
                    app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
            }
            let _ = level_resp.clone().on_hover_text(format!(
                "Send {which} to {name}: {} dB\nClick for the send fader · drag vertically to adjust · Shift-drag for fine control · Alt-click for 0 dB · right-click for options",
                db_text(s.level_db)
            ));
            level_resp.context_menu(|ui| send_options_menu(app, ui, id, slot, s));
        }
        None => empty_slot(ui.painter(), r, ui.rect_contains_pointer(r), mark),
    }
    if let Some(send) = &snd {
        egui::Popup::menu(&route_resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(250.0);
            ui.horizontal(|ui| {
                ui.strong(format!("Send {}", (b'A' + slot as u8) as char));
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    ui.label(egui::RichText::new(route_name(app, &send.target)).color(Tokens::current().counter_text));
                });
            });
            ui.label(egui::RichText::new("DESTINATION").size(8.0).color(Tokens::current().text_dim));
            send_route_menu(app, ui, id, slot, Some(&send.target));
            ui.separator();
            if ui.button("Remove send").clicked() {
                let _ = app.run("mix.send_remove", json!({"track": id.0, "slot": slot}));
                ui.close();
            }
        });
    } else {
        egui::Popup::menu(&route_resp).close_behavior(egui::PopupCloseBehavior::CloseOnClickOutside).show(|ui| {
            ui.set_min_width(250.0);
            ui.strong(format!("Send {} · Choose destination", (b'A' + slot as u8) as char));
            send_route_menu(app, ui, id, slot, None);
        });
    }
}

/// The Send window, as in Pro Tools: one send's fader, level, pan, Mute and Pre-fader. Clicking a
/// send's level opens it, moves it to that send, or closes it, and so does Escape.
pub(crate) fn send_window(app: &mut SoundApp, ctx: &egui::Context) {
    if !crate::window_sync::draws_here(app.window_sync.send_home, ctx) {
        return;
    }
    // egui closes menus and drops text focus on Escape before this runs, so whether this Escape
    // was theirs comes from the last frame. A dialog or the search on top takes it too.
    let busy_id = egui::Id::new("send_window_escape");
    let busy = ctx.data(|d| d.get_temp(busy_id)).unwrap_or(false) || app.dialogs.open.is_some() || app.ui.show_search;
    if !busy && ctx.input(|i| i.key_pressed(egui::Key::Escape)) {
        app.ui.send_window = None;
    }
    send_window_contents(app, ctx);
    let busy = egui::Popup::is_any_open(ctx) || ctx.text_edit_focused();
    ctx.data_mut(|d| d.insert_temp(busy_id, busy));
}

fn send_window_contents(app: &mut SoundApp, ctx: &egui::Context) {
    let Some((id, slot)) = app.ui.send_window else { return };
    let Some((name, send)) = app.engine.session().track(id).and_then(|t| Some((t.name.clone(), t.mixer.sends.get(slot).cloned().flatten()?))) else {
        app.ui.send_window = None;
        return;
    };
    let t = Tokens::current();
    let mut open = true;
    egui::Window::new(format!("{name} · Send {}", (b'A' + slot as u8) as char))
        .id(egui::Id::new("send_window"))
        .open(&mut open)
        .auto_sized()
        .collapsible(false)
        .default_pos(ctx.pointer_latest_pos().map_or(pos2(200.0, 200.0), |p| p + vec2(24.0, -160.0)))
        .show(ctx, |ui| {
            ui.set_width(150.0);
            ui.vertical_centered(|ui| {
                ui.label(egui::RichText::new(route_name(app, &send.target)).color(t.counter_text));
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 4.0;
                    ui.add_space(((ui.available_width() - 62.0) / 2.0).max(0.0));
                    if text_toggle(ui, vec2(36.0, 18.0), "PRE", send.pre_fader, t.accent, "Pre-fader").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": !send.pre_fader}));
                    }
                    if text_toggle(ui, vec2(22.0, 18.0), "M", send.mute, t.mute, "Mute send").clicked() {
                        let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": !send.mute}));
                    }
                });
                let mut pan = send.pan;
                if pan_knob(ui, 32.0, &mut pan, "Send pan (drag; double-click centres)").dragged() {
                    app.hold_control(id, soundcraft_model::AutoParam::SendPan(slot as u8));
                }
                if (pan - send.pan).abs() > f32::EPSILON {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "pan": pan}),
                        &format!("send-pan:{}:{slot}", id.0),
                    );
                }
                ui.label(egui::RichText::new(pan_text(send.pan)).color(t.counter_text).small());
                let (fr, fresp) = ui.allocate_exact_size(vec2(56.0, 220.0), Sense::hover());
                fresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Slider, true, "Send level"));
                if ui.ctx().is_being_dragged(ui.id().with("send_fader")) {
                    app.hold_control(id, soundcraft_model::AutoParam::SendLevel(slot as u8));
                }
                if let Some(db) = fader(ui, fr, send.level_db, ui.id().with("send_fader")) {
                    let _ = app.engine.execute_merged(
                        "mix.send_level",
                        &json!({"track": id.0, "slot": slot, "db": db}),
                        &format!("send:{}:{slot}", id.0),
                    );
                }
                let (rr, _) = ui.allocate_exact_size(vec2(64.0, 15.0), Sense::hover());
                ui.painter().rect_filled(rr, 1.0, t.counter_bg);
                let shown = db_text(send.level_db);
                let (_, typed) = editable_readout(ui, rr, ui.id().with("send_level_value"), &shown, &format!("{:.1}", send.level_db), parse_db, 0.0);
                if let Some(db) = typed {
                    let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
                }
            });
        });
    if !open {
        app.ui.send_window = None;
    }
}

/// Secondary controls live behind the amount readout, separate from the bus selector like Logic.
fn send_options_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, send: &soundcraft_model::SendSlot) {
    ui.set_min_width(230.0);
    ui.strong(format!("Send {} · {}", (b'A' + slot as u8) as char, route_name(app, &send.target)));
    let mut db = send.level_db.max(-60.0);
    if ui.add(egui::Slider::new(&mut db, -60.0..=12.0).text("Level").suffix(" dB")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "db": db}), &format!("send:{}:{slot}", id.0));
    }
    send_level_scale(ui, send.level_db);
    ui.horizontal(|ui| {
        for (label, delta) in [("−3", -3.0), ("−1", -1.0), ("+1", 1.0), ("+3", 3.0)] {
            if ui.small_button(label).clicked() {
                let db = (send.level_db.max(-60.0) + delta).clamp(-60.0, 12.0);
                let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": db}));
            }
        }
        if ui.small_button("0 dB").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": 0.0}));
        }
        if ui.small_button("−∞").clicked() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "db": -144.0}));
        }
    });
    let mut pan = send.pan;
    if ui.add(egui::Slider::new(&mut pan, -1.0..=1.0).text("Pan")).changed() {
        let _ = app.engine.execute_merged("mix.send_level", &json!({"track": id.0, "slot": slot, "pan": pan}), &format!("send-pan:{}:{slot}", id.0));
    }
    ui.horizontal(|ui| {
        let mut mute = send.mute;
        if crate::widgets::checkbox(ui, &mut mute, "Mute").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "mute": mute}));
        }
        let mut pre = send.pre_fader;
        if crate::widgets::checkbox(ui, &mut pre, "Pre-fader").changed() {
            let _ = app.run("mix.send_level", json!({"track": id.0, "slot": slot, "pre_fader": pre}));
        }
    });
    if let Route::Bus(bus) = send.target
        && let Some(aux) =
            app.engine.session().tracks.iter().find(|t| t.kind == TrackKind::Aux && t.mixer.input == Route::Bus(bus)).map(|t| (t.id, t.name.clone()))
        && ui.button("Rename bus…").clicked()
    {
        app.dialogs.open_rename_track(aux.0, &aux.1);
        ui.close();
    }
}

/// A send level after a vertical drag of `dy` points: along the fader's taper, like the level bar,
/// so it runs smoothly down to −∞ and back. About 0.15 dB a point near 0 dB (0.02 when fine).
fn dragged_send_db(db: f32, dy: f32, fine: bool) -> f32 {
    let per_point = if fine { 0.0004 } else { 0.003 };
    soundcraft_model::fader_pos_to_db(soundcraft_model::fader_db_to_pos(db) - dy * per_point)
}

/// Paint a value until clicked, then replace it with a focused text field. Enter or clicking away
/// commits a valid value; Escape cancels, and so does the field losing focus while hidden.
/// Alt-click returns `reset`. The interaction response remains available for dragging.
fn editable_readout(
    ui: &mut Ui,
    r: Rect,
    id: egui::Id,
    display: &str,
    edit_initial: &str,
    parse: fn(&str) -> Option<f32>,
    reset: f32,
) -> (Option<egui::Response>, Option<f32>) {
    let t = Tokens::current();
    let active_id = id.with("active");
    let text_id = id.with("text");
    let mut active: bool = ui.data(|d| d.get_temp(active_id)).unwrap_or(false);
    let mut started = false;
    let mut response = None;
    let mut value = None;
    if !active {
        let resp = ui.interact(r, id, Sense::click_and_drag());
        let alt = ui.input(|i| i.modifiers.alt);
        if resp.clicked() && alt {
            value = Some(reset);
        }
        if resp.clicked() && !alt && !resp.double_clicked() {
            active = true;
            started = true;
            ui.data_mut(|d| d.insert_temp(text_id, edit_initial.to_string()));
        } else {
            ui.painter().with_clip_rect(r).text(r.center(), Align2::CENTER_CENTER, display, regular(9.0), t.counter_text);
        }
        response = Some(resp);
    }
    if active {
        let mut text: String = ui.data(|d| d.get_temp(text_id)).unwrap_or_else(|| edit_initial.to_string());
        let mut child = ui.new_child(egui::UiBuilder::new().id_salt(id).max_rect(r));
        child.visuals_mut().override_text_color = Some(t.counter_text);
        let edit = egui::TextEdit::singleline(&mut text)
            .id(id.with("editor"))
            .frame(egui::Frame::NONE)
            .horizontal_align(egui::Align::Center)
            .font(regular(9.0))
            .desired_width(r.width());
        let edit_resp = child.add_sized(r.size(), edit);
        if started {
            edit_resp.request_focus();
        }
        let (enter, escape) = ui.input(|i| (i.key_pressed(egui::Key::Enter), i.key_pressed(egui::Key::Escape)));
        if escape {
            active = false;
        } else if (enter && edit_resp.has_focus()) || edit_resp.lost_focus() {
            if let Some(parsed) = parse(&text) {
                value = Some(parsed);
                active = false;
            } else if edit_resp.lost_focus() {
                active = false;
            }
        } else if !started && !edit_resp.has_focus() {
            active = false;
        }
        ui.data_mut(|d| d.insert_temp(text_id, text));
    }
    ui.data_mut(|d| d.insert_temp(active_id, active));
    (response, value)
}

/// The peak-hold readout: dBFS of a linear peak, "-inf" for silence.
pub fn peak_text(peak: f32) -> String {
    if !peak.is_finite() || peak <= 1e-5 { "-inf".into() } else { format!("{:.1}", 20.0 * peak.log10()) }
}

fn parse_db(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace("db", "").replace(',', ".");
    if matches!(cleaned.trim(), "-inf" | "-infinity" | "off") {
        return Some(-144.0);
    }
    cleaned.trim().parse::<f32>().ok().filter(|v| v.is_finite()).map(|v| v.clamp(-144.0, 12.0))
}

fn parse_pan(text: &str) -> Option<f32> {
    let cleaned = text.trim().to_ascii_lowercase().replace(',', ".");
    let (sign, number) = if let Some(v) = cleaned.strip_prefix('<').or_else(|| cleaned.strip_prefix('l')) {
        (-1.0, v)
    } else if let Some(v) = cleaned.strip_suffix('>').or_else(|| cleaned.strip_prefix('r')) {
        (1.0, v)
    } else {
        (1.0, cleaned.as_str())
    };
    let raw = number.trim().parse::<f32>().ok()? * sign;
    let value = if number.contains('.') && raw.abs() <= 1.0 { raw } else { raw / 100.0 };
    value.is_finite().then_some(value.clamp(-1.0, 1.0))
}

/// A compact calibrated scale for a send. The marks use the same taper as the strip's level bar.
fn send_level_scale(ui: &mut Ui, db: f32) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 25.0), Sense::hover());
    let line_y = r.min.y + 4.0;
    ui.painter().line_segment([pos2(r.min.x, line_y), pos2(r.max.x, line_y)], Stroke::new(1.0, t.border));
    for (i, mark) in [-60.0f32, -24.0, -10.0, 0.0, 6.0, 12.0].into_iter().enumerate() {
        let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(mark);
        ui.painter().line_segment([pos2(x, line_y - 3.0), pos2(x, line_y + 3.0)], Stroke::new(1.0, t.text_dim));
        let align = if i == 0 {
            Align2::LEFT_TOP
        } else if i == 5 {
            Align2::RIGHT_TOP
        } else {
            Align2::CENTER_TOP
        };
        let label = if mark > 0.0 { format!("+{mark:.0}") } else { format!("{mark:.0}") };
        ui.painter().text(pos2(x, line_y + 5.0), align, label, regular(8.0), t.text_dim);
    }
    let x = r.min.x + r.width() * soundcraft_model::fader_db_to_pos(db);
    ui.painter().line_segment([pos2(x, r.min.y), pos2(x, line_y + 3.0)], Stroke::new(2.0, t.counter_text));
}

fn send_route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, slot: usize, current: Option<&Route>) {
    let busses = app.engine.session().busses.clone();
    let search = MenuSearch::show(ui, egui::Id::new(("send-bus-search", id.0, slot)), "Search buses…");
    let visible: Vec<_> = busses.iter().filter(|bus| search.matches(&bus.name)).collect();
    let picked = search.rows(ui, visible.len(), 260.0, |ui, i, highlighted| {
        let current = matches!(current, Some(Route::Bus(bus_id)) if *bus_id == visible[i].id);
        menu_row(ui, highlighted || (current && !search.typing()), &visible[i].name)
    });
    if let Some(i) = picked {
        let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": visible[i].name}));
        ui.close();
    }
    ui.separator();
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.iter().any(|bus| bus.name == format!("Bus {n}")) || app.engine.session().track_by_name(&format!("Bus {n}")).is_some() {
            n += 1;
        }
        let _ = app.run("mix.send", json!({"track": id.0, "slot": slot, "bus": format!("Bus {n}")}));
        ui.close();
    }
}

/// Whether a path of `width` channels from `first` (0-based) fits a device of `device` channels
/// (`None`: no device open, so nothing is marked missing).
pub fn path_fits(first: usize, width: usize, device: Option<usize>) -> bool {
    device.is_none_or(|n| first.saturating_add(width.max(1)) <= n)
}

/// The suffix of a path the open device lacks.
pub const NOT_AVAILABLE: &str = " (not available)";

/// The inputs a track's input menu offers, each with whether the open device has its channels:
/// the session's named inputs, or, with none, "In 1"…"In N" for the device's N inputs (8 when
/// no input device is open; at most 64).
pub fn input_choices(s: &soundcraft_model::Session, device_inputs: Option<usize>) -> Vec<(String, bool)> {
    if s.inputs.is_empty() {
        let n = device_inputs.filter(|n| *n > 0).unwrap_or(8).min(64);
        return (1..=n).map(|k| (format!("In {k}"), true)).collect();
    }
    s.inputs.iter().map(|i| (i.name.clone(), path_fits(usize::from(i.first_channel), i.format.channels().clamp(1, 2), device_inputs))).collect()
}

/// The hardware outputs an output menu offers: the main, then the session's other outputs, each
/// with whether the open device has its channels. The main on its first channel always plays (a
/// device with fewer channels gets the fold-down).
pub fn output_choices(s: &soundcraft_model::Session, device_outputs: Option<usize>) -> Vec<(String, bool)> {
    let Some(main) = s.outputs.first() else { return vec![("Out 1-2".to_string(), true)] };
    let main_ok = main.first_channel == 0 || path_fits(usize::from(main.first_channel), s.main_format().channels(), device_outputs);
    std::iter::once((main.name.clone(), main_ok))
        .chain(s.outputs.iter().skip(1).map(|o| (o.name.clone(), path_fits(usize::from(o.first_channel), o.format.channels(), device_outputs))))
        .collect()
}

/// The I key's label and tooltip for each input-monitoring state.
pub fn input_monitor_tip(m: soundcraft_model::InputMonitor) -> &'static str {
    match m {
        soundcraft_model::InputMonitor::Auto => "Input monitoring: Auto (armed tracks hear their input)",
        soundcraft_model::InputMonitor::On => "Input monitoring: On (always hears its input)",
        soundcraft_model::InputMonitor::Off => "Input monitoring: Off (direct monitoring)",
    }
}

fn route_menu(app: &mut SoundApp, ui: &mut Ui, id: TrackId, input: bool) {
    let key = if input { "input" } else { "output" };
    let cmd = if input { "track.input" } else { "track.output" };
    let session = app.engine.session_arc();
    let current = session.track(id).map(|t| if input { &t.mixer.input } else { &t.mixer.output }).cloned().unwrap_or_default();
    if ui.add(egui::Button::selectable(current == Route::None, if input { "no input" } else { "no output" })).clicked() {
        let _ = app.run(cmd, json!({"track": id.0, key: "none"}));
        ui.close();
    }
    let (dev_in, dev_out) = app.device_channels();
    let choices = if input { input_choices(&session, dev_in) } else { output_choices(&session, dev_out) };
    for (i, (name, available)) in choices.iter().enumerate() {
        let picked = match &current {
            Route::Main => !input && i == 0,
            Route::Hardware(h) => soundcraft_model::same_name(h, name),
            _ => false,
        };
        let label = if *available { name.clone() } else { format!("{name}{NOT_AVAILABLE}") };
        let text = if *available { egui::RichText::new(label) } else { egui::RichText::new(label).color(Tokens::current().text_dim) };
        if ui.add(egui::Button::selectable(picked, text)).clicked() {
            // The main by its role (its name could be anything); named paths by name.
            let value = if !input && i == 0 { "main".to_string() } else { name.clone() };
            let _ = app.run(cmd, json!({"track": id.0, key: value}));
            ui.close();
        }
    }
    if input {
        #[cfg(not(target_arch = "wasm32"))]
        crate::net_ui::input_menu(app, ui, id, &current);
        crate::midi_devices::input_menu(app, ui, id);
        crate::band::input_menu(app, ui, id);
        crate::band::module_input_menu(app, ui, id);
    }
    ui.separator();
    let busses: Vec<String> = app.engine.session().busses.iter().map(|b| b.name.clone()).collect();
    ui.strong("Bus");
    let search = MenuSearch::show(ui, egui::Id::new(("route-bus-search", id.0, input)), "Search buses…");
    let visible: Vec<&String> = busses.iter().filter(|b| search.matches(b)).collect();
    if let Some(i) = search.rows(ui, visible.len(), 420.0, |ui, i, highlighted| menu_row(ui, highlighted, visible[i])) {
        let _ = app.run(cmd, json!({"track": id.0, key: visible[i]}));
        ui.close();
    }
    if ui.button("new bus...").clicked() {
        let mut n = busses.len() + 1;
        while busses.contains(&format!("Bus {n}")) || app.engine.session().track_by_name(&format!("Bus {n}")).is_some() {
            n += 1;
        }
        let _ = app.run(cmd, json!({"track": id.0, key: format!("Bus {n}")}));
        ui.close();
    }
}

#[cfg(test)]
mod tests {
    use super::{dragged_send_db, editable_readout, parse_db, parse_pan};

    #[test]
    fn wide_strips_send_slots_read_their_db_and_narrow_ones_do_not() {
        use soundcraft_model::StripSize;
        // The slot is the strip's inner width less 4 (`strip`).
        let slot = |s| super::strip_width(s, true) - 12.0;
        assert!(slot(StripSize::Wide) >= super::WIDE_SEND_SLOT);
        assert!(slot(StripSize::Slim) < super::WIDE_SEND_SLOT);
    }
    use egui::{Event, Modifiers, PointerButton, Rect, pos2, vec2};

    #[test]
    fn a_failed_replace_leaves_the_chain_and_no_redo() {
        let mut engine = soundcraft_engine::Engine::default();
        engine.execute("track.new", &serde_json::json!({"name": "Vox"})).unwrap();
        let mut app = crate::SoundApp::new(engine, None, crate::Services::default());
        let t = app.engine.session().tracks[0].id;
        app.run("mix.module_add", serde_json::json!({"track": t.0, "plugin": "compressor"})).unwrap();
        let m = app.engine.session().tracks[0].mixer.inserts[0].id;
        app.ui.drawers.insert(t, m);
        let undo = app.engine.undo_history();
        assert!(super::replace_module(&mut app, t, m, "clap:com.nobody.nothing").is_err());
        let chain: Vec<_> = app.engine.session().tracks[0].mixer.inserts.iter().map(|i| (i.id, i.plugin.clone())).collect();
        assert_eq!(chain, [(m, "compressor".to_string())]);
        assert_eq!(app.engine.undo_history(), undo);
        assert!(!app.engine.can_redo(), "no redo that would delete the module");
        assert_eq!(app.ui.drawers.get(&t), Some(&m), "its drawer stays open");
        // A good replace still works, as one undo step.
        assert!(super::replace_module(&mut app, t, m, "eq_7band").is_ok());
        assert_eq!(app.engine.session().tracks[0].mixer.inserts[0].plugin, "eq_7band");
        assert_eq!(app.engine.undo_history().len(), undo.len() + 1);
        assert_eq!(app.ui.browser.recent[0], "eq_7band", "a replace is a Recent too");
    }

    #[test]
    fn the_picker_lists_firestation_then_utilities_only() {
        use soundcraft_dsp::Category;
        let groups = super::picker_groups(false);
        assert_eq!(groups[0].0, "FireStation");
        let fs: Vec<&str> = groups[0].1.iter().map(|p| p.name).collect();
        let mut sorted = fs.clone();
        sorted.sort_by_key(|n| n.to_lowercase());
        assert_eq!(fs, sorted, "by name");
        assert!(fs.contains(&"CleanQ") && fs.contains(&"Comp") && fs.contains(&"Twine"));
        assert!(fs.contains(&"Manners") && fs.contains(&"Rez") && fs.contains(&"Sprawl"), "Rez and Sprawl are offered with the engines: {fs:?}");
        // Then the utilities; no stock effects, no instruments, no format groups.
        let names: Vec<&str> = groups.iter().map(|(g, _)| g.as_str()).collect();
        assert_eq!(names, ["FireStation", "Utilities"]);
        let ids: Vec<&str> = groups[1].1.iter().map(|p| p.id).collect();
        for id in ["gain", "trim", "invert", "dc_offset_removal", "time_shift", "signal_generator"] {
            assert!(ids.contains(&id), "{id}");
        }
        for (_, l) in &groups {
            assert!(l.iter().all(|p| p.listed() && !p.is_instrument));
            assert!(l.iter().all(|p| !soundcraft_dsp::HIDDEN_STOCK.contains(&p.category) || soundcraft_dsp::LISTED_STOCK.contains(&p.id)));
        }
        let listed: usize = groups.iter().map(|(_, l)| l.len()).sum();
        assert_eq!(listed, soundcraft_dsp::listed_plugins().filter(|p| !p.is_instrument).count());
        assert!(groups[1].1.iter().all(|p| p.category == Category::Other));
        // The instruments: the built-in ones, unchanged.
        let inst = super::picker_groups(true);
        assert_eq!(inst.iter().map(|(g, _)| g.as_str()).collect::<Vec<_>>(), ["Built-in"]);
        assert_eq!(inst[0].1.len(), soundcraft_dsp::plugins().iter().filter(|p| p.is_instrument).count());
    }

    #[test]
    fn search_ranks_exact_then_prefix_then_firestation_and_skips_stock() {
        let ids = |q: &str| super::ranked_built_ins(q, false).iter().map(|p| p.id).collect::<Vec<_>>();
        // "comp": FireStation's Comp; the hidden stock Compressor is not offered.
        assert_eq!(ids("comp"), ["fs_comp"]);
        assert!(ids("compressor").is_empty() && ids("reverb").iter().all(|i| i.starts_with("fs_")));
        // A prefix tie: the FireStation engines first, then by name.
        let c = ids("c");
        assert_eq!(c[..3], ["fs_chamber", "fs_cleanq", "fs_comp"]);
        assert!(!c.contains(&"channel_strip") && !c.contains(&"chorus"));
        assert!(ids("zzzz").is_empty());
        assert_eq!(ids("trim"), ["trim"], "the utilities stay");
        // Word starts beat matches inside a word.
        let strip = soundcraft_dsp::plugin_info("channel_strip").unwrap();
        assert_eq!(super::search_rank("strip", strip), Some((2, 1)));
        assert_eq!(super::search_rank("trip", strip), Some((3, 1)));
    }

    #[test]
    fn vendors_merge_formats_and_list_a_plugin_once() {
        let p = |id: &str, name: &str, vendor: &str| super::MenuPlugin { id: id.into(), name: name.into(), vendor: vendor.into() };
        // In order of preference: CLAP, then VST3, then Audio Units.
        let list = vec![
            p("clap:com.fab.proq3", "Pro-Q 3", "FabFilter"),
            p("clap:org.surge", "Surge XT", "Surge Synth Team"),
            p("vst3:AAA", "Pro-Q 3", "FabFilter"),
            p("vst3:BBB", "Pro-C 2", "FabFilter"),
            p("vst3:CCC", "Valhalla Room", "Valhalla DSP"),
            p("vst3:DDD", "Mystery", ""),
            p("au:aufx:x:y", "pro-q 3 ", "fabfilter"),
        ];
        let groups = super::vendor_groups(list);
        let names: Vec<&str> = groups.iter().map(|(v, _)| v.as_str()).collect();
        assert_eq!(names, ["Unknown vendor", "FabFilter", "Surge Synth Team", "Valhalla DSP"], "vendors by name, each once");
        let fab: Vec<&str> = groups[1].1.iter().map(|p| p.id.as_str()).collect();
        assert_eq!(fab, ["vst3:BBB", "clap:com.fab.proq3"], "by name; Pro-Q 3 once, as CLAP");
        assert!(super::vendor_groups(Vec::new()).is_empty());
    }

    #[test]
    fn typed_db_accepts_console_notation_and_clamps() {
        assert_eq!(parse_db("-6 dB"), Some(-6.0));
        assert_eq!(parse_db("+3,5"), Some(3.5));
        assert_eq!(parse_db("off"), Some(-144.0));
        assert_eq!(parse_db("99"), Some(12.0));
        assert_eq!(parse_db("loud"), None);
    }

    #[test]
    fn typed_pan_accepts_percent_console_and_fractional_values() {
        assert_eq!(parse_pan("-50"), Some(-0.5));
        assert_eq!(parse_pan("<35"), Some(-0.35));
        assert_eq!(parse_pan("35>"), Some(0.35));
        assert_eq!(parse_pan("R25"), Some(0.25));
        assert_eq!(parse_pan("0.5"), Some(0.5));
        assert_eq!(parse_pan("center"), None);
    }

    #[test]
    fn send_drags_follow_the_fader_taper_down_to_minus_infinity() {
        assert!((dragged_send_db(0.0, -10.0, false) - 1.44).abs() < 0.01);
        assert!((dragged_send_db(0.0, -10.0, true) - 0.192).abs() < 0.01);
        assert_eq!(dragged_send_db(0.0, 400.0, false), -144.0);
        let up = dragged_send_db(-144.0, -1.0, false);
        assert!(up > -144.0 && up < -100.0, "one point up leaves −∞ gently, got {up}");
        assert_eq!(dragged_send_db(12.0, -50.0, false), 12.0);
    }

    #[test]
    fn readouts_reset_on_alt_click_and_drop_a_field_that_lost_focus() {
        let ctx = egui::Context::default();
        let r = Rect::from_min_size(pos2(10.0, 10.0), vec2(40.0, 14.0));
        let click = |mods: Modifiers| {
            let press = |pressed| Event::PointerButton { pos: r.center(), button: PointerButton::Primary, pressed, modifiers: mods };
            vec![Event::ModifiersChanged(mods), Event::PointerMoved(r.center()), press(true), press(false)]
        };
        let mut time = 0.0;
        let mut frame = |events: Vec<Event>, shown: bool| {
            time += 0.5;
            let mut out = (false, None);
            let mut full = ctx.run_ui(egui::RawInput { events, time: Some(time), ..Default::default() }, |ui| {
                if shown {
                    let (resp, value) = editable_readout(ui, r, egui::Id::new("readout"), "-3.0", "-3.0", parse_db, 0.0);
                    out = (resp.is_some(), value);
                }
            });
            full.textures_delta.clear();
            out
        };
        assert_eq!(frame(vec![], true), (true, None));
        assert_eq!(frame(click(Modifiers::ALT), true), (true, Some(0.0)), "Alt-click resets");
        frame(vec![Event::ModifiersChanged(Modifiers::NONE)], true);
        frame(click(Modifiers::NONE), true);
        assert_eq!(frame(vec![], true), (false, None), "a click opens the text field");
        for _ in 0..3 {
            frame(vec![], false);
        }
        frame(vec![], true);
        assert_eq!(frame(vec![], true), (true, None), "hidden, the field lost focus, so the readout is back");
    }
}

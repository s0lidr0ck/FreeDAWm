//! Comping with take layers (Studio One style), part of the Edit window.
//!
//! The track lane is the comp; its take layers (every other playlist) sit under it at
//! [`LAYER_H`], opened and closed by the ▾/▸ before the track name. Each layer shows its whole
//! take dimmed and the ranges the comp uses from it bright with a white outline; comp segments
//! are coloured by the layer they came from.

use super::*;

/// The height of one take layer (and of the "+ New Layer" row).
pub const LAYER_H: f32 = 38.0;

/// The take colours, by [`Track::take_color`] index (cycling).
pub const TAKE_COLORS: [[u8; 3]; 8] =
    [[232, 140, 92], [92, 172, 228], [140, 208, 120], [196, 140, 232], [230, 200, 90], [84, 198, 190], [232, 120, 170], [220, 96, 86]];

/// The dimming of a layer's unused material.
const DIM: f32 = 0.32;

/// Whether the track can comp with layers (audio tracks).
pub fn can_comp(t: &Track) -> bool {
    t.kind == TrackKind::Audio
}

/// Whether the track shows its take layers.
pub fn shows_layers(t: &Track) -> bool {
    t.layers_open && can_comp(t)
}

/// The height the open layers add under the track (each layer and the "+ New Layer" row).
pub fn layers_height(t: &Track) -> f32 {
    if shows_layers(t) { (t.layers().count() as f32 + 1.0) * LAYER_H } else { 0.0 }
}

/// The colour of take layer `layer`.
pub fn take_rgb(t: &Track, layer: usize) -> [u8; 3] {
    TAKE_COLORS.get(usize::from(t.take_color(layer)) % TAKE_COLORS.len()).copied().unwrap_or(t.color)
}

/// A comp segment's take: its layer's colour and name; `None` for a clip from no (existing) layer.
pub fn segment_take<'a>(t: &'a Track, c: &Clip) -> Option<([u8; 3], &'a str)> {
    let l = c.from_layer.filter(|l| t.is_layer(*l))?;
    Some((take_rgb(t, l), t.playlists.get(l).map_or("", |p| p.name.as_str())))
}

/// The ranges the comp uses from layer `layer`: each comp segment from it, cut to the layer's
/// material.
pub fn used_ranges(t: &Track, layer: usize) -> Vec<Range> {
    let Some(pl) = t.playlists.get(layer) else { return Vec::new() };
    t.clips()
        .iter()
        .filter(|c| c.from_layer == Some(layer))
        .flat_map(|c| pl.clips.iter().filter_map(move |lc| lc.range().intersect(&c.range())))
        .filter(|r| !r.is_empty())
        .collect()
}

/// Each layer row under a track whose top is `top`: the layer index (`None` for "+ New Layer"),
/// its header and its lane.
pub fn layer_rows(t: &Track, row: Rect, top: f32, tl: Rect) -> Vec<(Option<usize>, Rect, Rect)> {
    let mut y = top;
    let mut out = Vec::new();
    for l in t.layers().map(|(i, _)| Some(i)).chain([None]) {
        let head = Rect::from_min_max(pos2(row.min.x + STRIP_W, y), pos2(tl.min.x, y + LAYER_H));
        let lane = Rect::from_min_max(pos2(tl.min.x, y), pos2(tl.max.x, y + LAYER_H));
        out.push((l, head, lane));
        y += LAYER_H;
    }
    out
}

/// The ▾/▸ before the track name that opens and closes the layers. Returns how much of the name
/// plate it takes.
pub(super) fn disclosure(app: &mut SoundApp, ui: &mut Ui, track: &Track, r: Rect, ink: Color32) -> f32 {
    if !can_comp(track) {
        return 0.0;
    }
    let resp = ui.interact(r, ui.id().with(("layers", track.id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} layers", track.name)));
    let c = if resp.hovered() { Tokens::current().text } else { ink };
    arrow(ui.painter(), r.center(), track.layers_open, c);
    let n = track.layers().count();
    let tip = if track.layers_open {
        format!("Comp \u{b7} {n} take{}: hide the layers", if n == 1 { "" } else { "s" })
    } else {
        format!("Comp \u{b7} {n} take{}: show the layers (swipe a take into the comp)", if n == 1 { "" } else { "s" })
    };
    if resp.on_hover_text(tip).clicked() {
        let _ = app.run("comp.layers", json!({"track": track.id.0}));
    }
    r.width() + 1.0
}

/// The open layers under a track: `row` is the track's row, its comp lane `main_h` tall.
pub(super) fn layer_lanes(app: &mut SoundApp, ui: &mut Ui, track: &Track, row: Rect, main_h: f32, tl: Rect, lane_bg: Color32) {
    let t = Tokens::current();
    let s = app.engine.session_arc();
    for (layer, head, lane) in layer_rows(track, row, row.min.y + main_h, tl) {
        let painter = ui.painter().with_clip_rect(head.union(lane).intersect(ui.clip_rect()));
        painter.rect_filled(head, 0.0, t.panel_bg);
        painter.rect_filled(lane, 0.0, shade(lane_bg, -0.18));
        painter.hline(head.min.x..=lane.max.x, head.min.y + 0.5, Stroke::new(1.0, t.border));
        painter.vline(tl.min.x - 0.5, head.y_range(), Stroke::new(1.0, t.border));
        let Some(l) = layer else {
            new_layer_row(app, ui, track, head);
            continue;
        };
        let Some(pl) = track.playlists.get(l) else { continue };
        let color = rgb(take_rgb(track, l));
        // Header: the take's colour and name; a red dot on the record target.
        let sq = Rect::from_center_size(pos2(head.min.x + 12.0, head.center().y), vec2(9.0, 9.0));
        painter.rect_filled(sq, 1.0, color);
        painter.with_clip_rect(head.shrink(1.0)).text(pos2(head.min.x + 22.0, head.center().y), Align2::LEFT_CENTER, &pl.name, regular(12.0), t.text);
        if track.record_layer == Some(l) {
            painter.circle_filled(pos2(head.max.x - 10.0, head.center().y), 3.5, Color32::from_rgb(222, 64, 56));
        }
        let hresp = ui.interact(head, ui.id().with(("layer-head", track.id.0, l)), Sense::click());
        hresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, format!("{} layer {}", track.name, pl.name)));
        hresp.context_menu(|ui| layer_menu(app, ui, track, l));
        let lp = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
        draw_layer(&lp, &s, track, l, tl, lane, color);
        if let Some(resp) = swipe(app, ui, track, l, &pl.name, tl, lane) {
            resp.context_menu(|ui| layer_menu(app, ui, track, l));
        }
    }
}

/// Whether the tool swipes takes (Smart, Grabber, Selector).
fn swipes(s: &Session) -> bool {
    matches!(s.edit.tool, Tool::Smart | Tool::Grabber | Tool::Selector)
}

/// Swipe across layer `layer` (on release that range replaces the comp; Ctrl/Cmd: only where
/// the comp is empty); a click puts the whole take in the comp over its extent.
#[allow(clippy::too_many_arguments)]
fn swipe(app: &mut SoundApp, ui: &mut Ui, track: &Track, layer: usize, name: &str, tl: Rect, lane: Rect) -> Option<egui::Response> {
    let s = app.engine.session_arc();
    if !swipes(&s) {
        return None;
    }
    let resp = ui.interact(lane, ui.id().with(("layer-lane", track.id.0, layer)), Sense::click_and_drag());
    let key = resp.id.with("from");
    let mods = ui.input(|i| i.modifiers);
    let add = mods.command || mods.ctrl;
    if resp.drag_started()
        && let Some(o) = ui.input(|i| i.pointer.press_origin())
    {
        ui.data_mut(|d| d.insert_temp(key, snap(&s, sample_at(&s, tl, o.x))));
    }
    let from = ui.data(|d| d.get_temp::<Samples>(key));
    let to = resp.interact_pointer_pos().or_else(|| ui.ctx().pointer_latest_pos()).map(|p| snap(&s, sample_at(&s, tl, p.x)));
    if resp.dragged()
        && let (Some(a), Some(b)) = (from, to)
    {
        let r = Range::new(a.min(b), a.max(b));
        let (x0, x1) = (x_of(&s, tl, r.start), x_of(&s, tl, r.end));
        let band = Rect::from_min_max(pos2(x0, lane.min.y + 3.0), pos2(x1.max(x0 + 1.0), lane.max.y - 3.0));
        let painter = ui.painter().with_clip_rect(lane.intersect(ui.clip_rect()));
        painter.rect_filled(band, 2.0, Color32::from_white_alpha(28));
        let c = band.shrink(0.5);
        for seg in
            [[c.left_top(), c.right_top()], [c.right_top(), c.right_bottom()], [c.right_bottom(), c.left_bottom()], [c.left_bottom(), c.left_top()]]
        {
            painter.extend(Shape::dashed_line(&seg, Stroke::new(1.0, Color32::WHITE), 4.0, 3.0));
        }
        let what = if add { "adds" } else { "replaces" };
        let text = format!("{name} {what} {} in the comp", range_text(&s, r));
        if let Some(p) = ui.ctx().pointer_latest_pos() {
            let fg = ui.ctx().layer_painter(egui::LayerId::new(egui::Order::Tooltip, egui::Id::new("comp-swipe-tip")));
            let galley = fg.layout_no_wrap(text, regular(11.5), Color32::WHITE);
            let at = pos2(p.x + 12.0, lane.min.y - galley.size().y - 6.0);
            let bg = Rect::from_min_size(at, galley.size()).expand2(vec2(6.0, 3.0));
            fg.rect(bg, 3.0, Color32::from_rgb(20, 20, 22), Stroke::new(1.0, Color32::from_gray(90)), StrokeKind::Inside);
            fg.galley(at, galley, Color32::WHITE);
        }
    }
    if resp.drag_stopped() {
        ui.data_mut(|d| d.remove::<Samples>(key));
        if let (Some(a), Some(b)) = (from, to)
            && a != b
            && let Err(e) = app.run("comp.swipe", json!({"track": track.id.0, "layer": layer, "start": a.min(b), "end": a.max(b), "add": add}))
        {
            app.ui.status = e;
        }
    } else if resp.clicked() {
        let extent = track.playlists.get(layer).and_then(|p| Some(Range::new(p.clips.first()?.start, p.end())));
        if let Some(r) = extent.filter(|r| !r.is_empty())
            && let Err(e) = app.run("comp.swipe", json!({"track": track.id.0, "layer": layer, "start": r.start, "end": r.end, "add": add}))
        {
            app.ui.status = e;
        }
    }
    Some(resp.on_hover_text_at_pointer("Swipe to put this range of the take in the comp (Ctrl: only where the comp is empty); click: the whole take"))
}

/// A range as the main counter shows it, "a–b".
fn range_text(s: &Session, r: Range) -> String {
    let f = s.edit.main_counter;
    format!("{}\u{2013}{}", s.format_position(r.start, f).trim(), s.format_position(r.end, f).trim())
}

/// A layer's takes: dimmed, the parts the comp uses bright with a white outline.
fn draw_layer(painter: &egui::Painter, s: &Session, track: &Track, layer: usize, tl: Rect, lane: Rect, color: Color32) {
    let Some(pl) = track.playlists.get(layer) else { return };
    let body = |a: Samples, b: Samples| {
        Rect::from_min_max(pos2(x_of(s, tl, a), lane.min.y + 4.0), pos2(x_of(s, tl, b).max(x_of(s, tl, a) + 1.0), lane.max.y - 4.0))
    };
    let bg = shade(Tokens::current().panel_bg, -0.1);
    let dim = bg.lerp_to_gamma(color, DIM);
    let view = Range::new(sample_at(s, tl, tl.min.x), sample_at(s, tl, tl.max.x));
    for c in pl.clips.iter().filter(|c| c.range().overlaps(&view)) {
        let r = body(c.start, c.end());
        painter.rect_filled(r, 3.0, dim);
        wave(painter, s, track, c, tl, r, r, color.gamma_multiply(0.45));
    }
    for u in used_ranges(track, layer).into_iter().filter(|u| u.overlaps(&view)) {
        let r = body(u.start, u.end);
        painter.rect_filled(r, 3.0, color);
        for c in pl.clips.iter().filter(|c| c.range().overlaps(&u)) {
            wave(painter, s, track, c, tl, body(c.start, c.end()), r, shade(color, -0.55));
        }
        painter.rect_stroke(r, 3.0, Stroke::new(2.0, Color32::WHITE), StrokeKind::Inside);
    }
}

/// `clip`'s waveform over `body`, shown only inside `shown`.
fn wave(painter: &egui::Painter, s: &Session, track: &Track, clip: &Clip, tl: Rect, body: Rect, shown: Rect, color: Color32) {
    if let ClipContent::Audio { source, .. } = &clip.content
        && let Some(audio) = s.pool.get(*source)
    {
        let vis = shown.intersect(body).intersect(painter.clip_rect());
        if vis.width() > 0.0 && vis.height() > 2.0 {
            crate::waveform::draw(&painter.with_clip_rect(vis), s, tl, clip, audio, track.channels(), body.shrink2(vec2(0.0, 3.0)), color);
        }
    }
}

/// The "+ New Layer" row: adds an empty layer (the record target).
fn new_layer_row(app: &mut SoundApp, ui: &mut Ui, track: &Track, head: Rect) {
    let t = Tokens::current();
    let r = Rect::from_min_size(pos2(head.min.x + 6.0, head.center().y - 9.0), vec2((head.width() - 12.0).min(104.0), 18.0));
    let resp = ui.interact(r, ui.id().with(("layer-new", track.id.0)), Sense::click());
    resp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{} new layer", track.name)));
    raised_key(ui.painter(), r, t.slot_face, resp.hovered());
    ui.painter().text(r.center(), Align2::CENTER_CENTER, "+ New Layer", regular(11.0), t.text);
    if resp.on_hover_text("Add an empty take layer; the next recording goes into it").clicked() {
        let _ = app.run("comp.layer_new", json!({"track": track.id.0}));
    }
}

/// Up/Down with a comp segment selected (a clip on a track with its layers open, or the edit
/// selection there): that range switches to the take above/below with audio there, and stays
/// selected. Returns whether the key was used.
pub fn cycle_key(app: &mut SoundApp, down: bool) -> bool {
    let s = app.engine.session_arc();
    let target = s.edit.selected_clips.iter().find_map(|id| {
        let (t, c) = s.find_clip(*id)?;
        let tr = s.track(t)?;
        (shows_layers(tr) && tr.playlist()?.clip(*id).is_some()).then(|| (t, c.range()))
    });
    let target = target.or_else(|| {
        let r = s.edit.selection;
        let t = s.edit.selected_tracks.iter().copied().find(|t| s.track(*t).is_some_and(shows_layers))?;
        (!r.is_empty()).then_some((t, r))
    });
    let Some((t, r)) = target else { return false };
    let reselect = !s.edit.selected_clips.is_empty();
    match app.run("comp.cycle", json!({"track": t.0, "start": r.start, "end": r.end, "dir": if down { "down" } else { "up" }})) {
        Ok(v) => {
            // Keep the new segment selected, so the next press goes on from it.
            if reselect && let Some(id) = new_segment(app.engine.session(), t, r, &v) {
                let _ = app.run("edit.select", json!({"clips": [id]}));
            }
        }
        Err(e) => app.ui.status = e,
    }
    true
}

/// The comp segment `comp.cycle` put over most of `r`.
fn new_segment(s: &Session, t: TrackId, r: Range, result: &serde_json::Value) -> Option<u64> {
    let ids: Vec<u64> = result.get("clips")?.as_array()?.iter().filter_map(serde_json::Value::as_u64).collect();
    let tr = s.track(t)?;
    tr.clips().iter().filter(|c| ids.contains(&c.id.0)).max_by_key(|c| c.range().intersect(&r).map_or(0, |i| i.len())).map(|c| c.id.0)
}

/// A layer's right-click menu: Rename, Delete Take (refused while the comp uses it), Make Comp
/// from This Take, Record Into This Layer, New Layer.
pub fn layer_menu(app: &mut SoundApp, ui: &mut Ui, track: &Track, layer: usize) {
    let id = track.id.0;
    let Some(pl) = track.playlists.get(layer) else { return };
    ui.label(egui::RichText::new(&pl.name).strong());
    // Rename: type the new name, Enter applies it.
    let key = egui::Id::new(("layer-rename", id, layer));
    let mut name = ui.data(|d| d.get_temp::<String>(key)).unwrap_or_else(|| pl.name.clone());
    ui.horizontal(|ui| {
        ui.label("Rename");
        let r = ui.add(egui::TextEdit::singleline(&mut name).desired_width(110.0));
        if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) && name.trim() != pl.name {
            let _ = app.run("comp.layer_rename", json!({"track": id, "layer": layer, "name": name.trim()}));
            ui.close();
        }
    });
    ui.data_mut(|d| d.insert_temp(key, name));
    ui.separator();
    let used = track.clips().iter().any(|c| c.from_layer == Some(layer));
    if used {
        ui.label(egui::RichText::new(soundcraft_engine::cmd::comp::LAYER_IN_COMP).weak());
        if ui.button("Remove and Delete").clicked() {
            let _ = app.run("comp.layer_delete", json!({"track": id, "layer": layer, "remove": true}));
        }
    } else if ui.button("Delete Take").clicked() {
        let _ = app.run("comp.layer_delete", json!({"track": id, "layer": layer}));
    }
    if ui.button("Make Comp from This Take").clicked() {
        let _ = app.run("comp.layer_make_comp", json!({"track": id, "layer": layer}));
    }
    let target = track.record_layer == Some(layer);
    if ui.add(egui::Button::new("Record Into This Layer").selected(target)).clicked() {
        let v = if target { serde_json::Value::Null } else { json!(layer) };
        let _ = app.run("comp.layer_record_target", json!({"track": id, "layer": v}));
    }
    ui.separator();
    if ui.button("New Layer").clicked() {
        let _ = app.run("comp.layer_new", json!({"track": id}));
    }
}

/// The track header menu's comping items: Show/Hide Layers and New Layer.
pub fn track_items(app: &mut SoundApp, ui: &mut Ui, track: &Track) {
    let id = track.id.0;
    if ui.button(if track.layers_open { "Hide Layers" } else { "Show Layers" }).clicked() {
        let _ = app.run("comp.layers", json!({"track": id}));
    }
    if ui.button("New Layer").clicked() {
        let _ = app.run("comp.layer_new", json!({"track": id}));
    }
}

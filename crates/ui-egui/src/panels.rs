//! Side panels (Tracks, Groups, Clips) and floating windows.

use crate::SoundApp;
use crate::theme::{Tokens, bold, mono, regular, rgb};
use egui::{Align2, Color32, Rect, Sense, Stroke, Ui, pos2, vec2};
use serde_json::json;
use soundcraft_model::{ClipId, SourceId, TrackId};

/// Drag a Tracks list row to reorder the tracks: a line marks the gap it will land in. Rows are
/// counted as the list shows them (a closed folder's members left out), then mapped back to the
/// session's order (`window_sync::list_drop`).
fn drag_track(app: &mut SoundApp, ui: &Ui, row: Rect, id: u64, dropped: bool) {
    let s = app.engine.session();
    let rows: Vec<soundcraft_model::TrackId> = crate::window_sync::list_rows(s).into_iter().map(|(t, _)| t).collect();
    let id = soundcraft_model::TrackId(id);
    let (Some(from), Some(p)) = (rows.iter().position(|t| *t == id), ui.ctx().pointer_latest_pos()) else { return };
    let spacing = ui.spacing().item_spacing.y;
    let pitch = row.height() + spacing;
    let gap = (from as f32 + ((p.y - row.center().y) / pitch).floor() + 1.0).clamp(0.0, rows.len() as f32);
    if dropped {
        let session: Vec<soundcraft_model::TrackId> = s.tracks.iter().map(|t| t.id).collect();
        let block = crate::window_sync::drag_block(s, id);
        if let Some(to) = crate::window_sync::list_drop(&rows, &session, id, block, gap as usize) {
            let _ = app.run("track.move", json!({"track": id.0, "to": to, "count": block}));
        }
    } else {
        let y = row.min.y + (gap - from as f32) * pitch - spacing / 2.0;
        ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
        ui.ctx()
            .layer_painter(egui::LayerId::new(egui::Order::Foreground, egui::Id::new("track_list_reorder")))
            .line_segment([pos2(row.min.x, y), pos2(row.max.x, y)], Stroke::new(2.0, Tokens::current().accent));
    }
}

fn panel_header(ui: &mut Ui, title: &str) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 22.0), Sense::hover());
    ui.painter().rect_filled(r, 0.0, t.panel_bg2);
    ui.painter().text(pos2(r.min.x + 8.0, r.center().y), Align2::LEFT_CENTER, title, bold(11.5), t.header_text);
    ui.painter().line_segment([pos2(r.min.x, r.max.y), pos2(r.max.x, r.max.y)], Stroke::new(1.0, t.border));
}

/// The strip down the Tracks panel's right edge: the lists' scroll bars, and a clear line between
/// the panel and the window beside it.
const GUTTER_W: f32 = 10.0;

/// How far each folder level indents the Tracks list's names.
const LIST_INDENT: f32 = 10.0;

/// One row of the Tracks list.
struct ListRow {
    id: u64,
    name: String,
    shown: bool,
    color: [u8; 3],
    selected: bool,
    depth: usize,
    /// A folder track: whether it is open.
    folder: Option<bool>,
}

/// A small folder in the folder track's colour: a tab on the top left over the body.
fn folder_glyph(p: &egui::Painter, r: Rect, color: Color32) {
    let tab = Rect::from_min_size(r.min, vec2(r.width() * 0.45, 3.0));
    p.rect_filled(tab, 1.0, color);
    p.rect_filled(Rect::from_min_max(pos2(r.min.x, r.min.y + 2.0), r.max), 1.5, color);
}

/// The Tracks and Groups panel of `window` (Edit's and Mix's each have one).
pub fn tracks_and_groups(app: &mut SoundApp, ui: &mut Ui, window: crate::MainWindow) {
    let t = Tokens::current();
    let full = ui.max_rect();
    let gutter = full.with_min_x(full.max.x - GUTTER_W);
    ui.painter().rect_filled(gutter, 0.0, t.panel_bg.lerp_to_gamma(t.text_dim, 0.15));
    ui.painter().vline(gutter.min.x + 0.5, full.y_range(), Stroke::new(1.0, t.border));
    ui.painter().vline(gutter.max.x - 0.5, full.y_range(), Stroke::new(1.0, t.border));
    let ui = &mut ui.new_child(egui::UiBuilder::new().max_rect(full.with_max_x(gutter.min.x)));
    let total = ui.available_height();
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 22.0));
    panel_header(ui, "TRACKS");
    let plus = Rect::from_center_size(pos2(header.max.x - 13.0, header.center().y), vec2(18.0, 18.0));
    let keys = crate::menus::shortcut_text(app, "track.new", ui.ctx().os().is_mac());
    let tip = if keys.is_empty() { "New Tracks…".to_string() } else { format!("New Tracks… ({keys})") };
    if crate::widgets::icon_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(plus)), plus.size(), "plus", false, &tip).clicked() {
        let _ = app.dialogs.open_for_command(&app.engine, "track.new");
    }
    // The same folder tree as the Edit window: members indented under their folder, a closed
    // folder's members left out. The arrow opens or closes the folder in both windows (the state
    // is the session's).
    let tracks: Vec<ListRow> = {
        let s = app.engine.session();
        crate::window_sync::list_rows(s)
            .into_iter()
            .filter_map(|(id, depth)| s.track(id).map(|x| (x, depth)))
            .map(|(x, depth)| ListRow {
                id: x.id.0,
                name: x.name.clone(),
                shown: !x.hidden,
                color: x.color,
                selected: crate::window_sync::shows_selected(app, window, x.id),
                depth,
                folder: x.is_folder().then_some(x.folder_open),
            })
            .collect()
    };
    let hidden = egui::scroll_area::ScrollBarVisibility::AlwaysHidden;
    let lists =
        egui::ScrollArea::vertical().id_salt("tracks_scroll").max_height(total * 0.62).auto_shrink([false, false]).scroll_bar_visibility(hidden);
    let out = lists.show(ui, |ui| {
        for row in tracks {
            let ListRow { id, name, shown, color, selected: sel, depth, folder } = row;
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click_and_drag());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::SelectableLabel, true, sel, &name));
            if sel {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            }
            let dot = Rect::from_center_size(pos2(r.min.x + 10.0, r.center().y), vec2(8.0, 8.0));
            let dresp = ui.interact(dot.expand(3.0), ui.id().with(("vis", id)), Sense::click());
            ui.painter().circle(dot.center(), 3.5, if shown { t.visible_dot } else { Color32::TRANSPARENT }, Stroke::new(1.0, t.text_dim));
            if dresp.clicked() {
                let _ = app.run("track.hide", json!({"tracks": [id], "hidden": shown}));
            }
            let x = r.min.x + 20.0 + depth as f32 * LIST_INDENT;
            let ink = if shown { t.text } else { t.text_dim };
            let name_x = match folder {
                Some(open) => {
                    // The folder's arrow, then a folder glyph in the folder's colour.
                    let arrow = Rect::from_center_size(pos2(x + 4.0, r.center().y), vec2(12.0, 14.0));
                    let aresp = ui.interact(arrow, ui.id().with(("folder_arrow", id)), Sense::click());
                    let what = if open { "Close folder" } else { "Open folder" };
                    aresp.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, format!("{what} {name}")));
                    crate::edit_window::arrow(ui.painter(), arrow.center(), open, if aresp.hovered() { t.text } else { t.text_dim });
                    if aresp.on_hover_text(what).clicked() {
                        let _ = app.run("track.folder_toggle", json!({"track": id}));
                    }
                    folder_glyph(ui.painter(), Rect::from_min_size(pos2(x + 11.0, r.min.y + 4.0), vec2(12.0, 9.0)), rgb(color));
                    x + 27.0
                }
                None => {
                    ui.painter().rect_filled(Rect::from_min_size(pos2(x, r.min.y + 3.0), vec2(4.0, 11.0)), 0.0, rgb(color));
                    x + 10.0
                }
            };
            ui.painter().with_clip_rect(r).text(pos2(name_x, r.center().y), Align2::LEFT_CENTER, &name, regular(11.5), ink);
            if resp.clicked() {
                crate::window_sync::click_list_track(app, window, soundcraft_model::TrackId(id), ui.input(|i| i.modifiers));
            }
            if resp.dragged() || resp.drag_stopped() {
                drag_track(app, ui, r, id, resp.drag_stopped());
            }
            // The same menu as right-clicking the track itself (its header in Edit, its strip's
            // name in Mix): on the window's selection when it holds the track, else on the track.
            resp.context_menu(|ui| {
                let tid = soundcraft_model::TrackId(id);
                crate::edit_window::track_context_menu(app, ui, tid, window);
                if window == crate::MainWindow::Mix {
                    ui.separator();
                    crate::channel_width::menu(app, ui, tid);
                }
            });
        }
    });
    gutter_scroll(ui, gutter, &out);
    ui.add_space(4.0);
    let header = Rect::from_min_size(ui.cursor().min, vec2(ui.available_width(), 22.0));
    panel_header(ui, "GROUPS");
    let plus = Rect::from_center_size(pos2(header.max.x - 13.0, header.center().y), vec2(18.0, 18.0));
    if crate::widgets::icon_button(&mut ui.new_child(egui::UiBuilder::new().max_rect(plus)), plus.size(), "plus", false, "New Group…").clicked() {
        let selected = crate::window_sync::selection_of(app, window);
        app.dialogs.open_new_group(&app.engine, &selected);
    }
    let groups: Vec<(u64, char, String, bool, [u8; 3])> =
        app.engine.session().groups.iter().map(|g| (g.id.0, g.letter, g.name.clone(), g.active, g.color)).collect();
    let out = egui::ScrollArea::vertical().id_salt("groups_scroll").auto_shrink([false, false]).scroll_bar_visibility(hidden).show(ui, |ui| {
        let (r, _) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::hover());
        ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, "<ALL>", regular(11.5).clone(), t.text);
        let all = ui.interact(r, ui.id().with("group_all"), Sense::click());
        all.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Button, true, "<ALL>"));
        if all.double_clicked() {
            let _ = app.run("edit.select_all", json!({}));
        }
        all.context_menu(|ui| {
            if ui.button("New Group…").clicked() {
                let _ = app.dialogs.open_for_command(&app.engine, "track.group");
                ui.close();
            }
        });
        for (id, letter, name, active, color) in groups {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click());
            resp.widget_info(|| egui::WidgetInfo::selected(egui::WidgetType::Checkbox, true, active, &name));
            if active {
                ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 4.0, r.min.y + 2.0), vec2(14.0, 13.0)), 1.0, rgb(color));
            }
            ui.painter().text(
                pos2(r.min.x + 11.0, r.center().y),
                Align2::CENTER_CENTER,
                letter.to_string(),
                bold(10.0),
                if active { t.text_dark } else { t.text_dim },
            );
            ui.painter().text(pos2(r.min.x + 30.0, r.center().y), Align2::LEFT_CENTER, &name, regular(11.5), t.text);
            if resp.clicked() {
                let _ = app.run("track.group_toggle", json!({"group": id}));
            }
            let members: Vec<u64> =
                app.engine.session().groups.iter().find(|g| g.id.0 == id).map(|g| g.members.iter().map(|m| m.0).collect()).unwrap_or_default();
            if resp.double_clicked() {
                let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
            }
            resp.context_menu(|ui| {
                if ui.button("Select Tracks in Group").clicked() {
                    let _ = app.run("edit.select", json!({"tracks": members, "exact": true}));
                    ui.close();
                }
                if ui.button(if active { "Disable" } else { "Enable" }).clicked() {
                    let _ = app.run("track.group_toggle", json!({"group": id}));
                    ui.close();
                }
                if ui.button("Modify Group…").clicked() {
                    let _ = app.dialogs.open_modify_group(&app.engine, soundcraft_model::GroupId(id));
                    ui.close();
                }
                if ui.button("Delete Group").clicked() {
                    let _ = app.run("track.ungroup", json!({"group": id}));
                    ui.close();
                }
            });
        }
    });
    gutter_scroll(ui, gutter, &out);
}

/// A list's scroll thumb in the gutter beside it, when the list is longer than its space: drag it,
/// or click above or below it to page.
fn gutter_scroll<R>(ui: &Ui, gutter: Rect, out: &egui::scroll_area::ScrollAreaOutput<R>) {
    let t = Tokens::current();
    let (view, content) = (out.inner_rect.height(), out.content_size.y);
    if content <= view + 0.5 {
        return;
    }
    let bar = Rect::from_x_y_ranges(gutter.x_range(), out.inner_rect.y_range());
    let (max, h) = (content - view, (bar.height() * view / content).max(20.0));
    let mut state = out.state;
    let thumb = Rect::from_min_size(pos2(bar.min.x + 2.0, bar.min.y + (bar.height() - h) * state.offset.y / max), vec2(bar.width() - 4.0, h));
    let resp = ui.interact(bar, out.id.with("gutter"), Sense::click_and_drag());
    ui.painter().rect_filled(thumb, 3.0, if resp.hovered() || resp.dragged() { t.button_hi.lerp_to_gamma(Color32::WHITE, 0.1) } else { t.button_hi });
    let to = if resp.dragged() {
        Some(state.offset.y + resp.drag_delta().y * max / (bar.height() - h).max(1.0))
    } else if let Some(p) = resp.interact_pointer_pos().filter(|_| resp.clicked()) {
        let page = if p.y < thumb.min.y {
            -view
        } else if p.y > thumb.max.y {
            view
        } else {
            0.0
        };
        Some(state.offset.y + page)
    } else {
        None
    };
    if let Some(y) = to {
        state.offset.y = y.clamp(0.0, max);
        state.store(ui.ctx(), out.id);
    }
}

pub fn clip_list(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    panel_header(ui, "CLIPS");
    let items: Vec<(u64, String, bool, [u8; 3], bool, bool)> = {
        let s = app.engine.session();
        let mut v = Vec::new();
        // Whole files first (bold), then clips on tracks. A drum kit's pad samples belong to its
        // instrument (a rendered kit brings dozens): they are not listed.
        let pads: std::collections::HashSet<SourceId> =
            s.tracks.iter().filter_map(|t| t.instrument.as_ref()).flat_map(|i| i.samples.values().copied()).collect();
        // A field recording's scene and take (its iXML) beside its name, so a search finds them.
        let field =
            |id: SourceId| s.field_info.get(&id.0).map(|f| f.label()).filter(|l| !l.is_empty()).map(|l| format!(" · {l}")).unwrap_or_default();
        for src in s.sources.iter().filter(|src| !pads.contains(&src.id)) {
            // A file the session could not find says so (Relink… in its menu finds it).
            let missing = if s.pool.get(src.id).is_none() { " — missing" } else { "" };
            v.push((
                src.id.0,
                format!("{} ({}ch){}{missing}", src.name, src.channels, field(src.id)),
                true,
                [150, 150, 150],
                s.edit.selected_sources.contains(&src.id),
                false,
            ));
        }
        for tr in &s.tracks {
            for c in tr.clips() {
                let name = format!("{}{}", c.name, c.source().map(field).unwrap_or_default());
                v.push((c.id.0, name, false, c.color.unwrap_or(tr.color), s.edit.selected_clips.contains(&c.id), false));
            }
        }
        // Clips captured into the list (Clip › Capture), on no track.
        for c in &s.captured {
            v.push((c.id.0, format!("{} (captured)", c.name), false, c.color.unwrap_or([150, 150, 150]), false, true));
        }
        v
    };
    let fkey = egui::Id::new("clip_filter");
    let mut filter: String = ui.ctx().memory(|m| m.data.get_temp(fkey)).unwrap_or_default();
    ui.add(egui::TextEdit::singleline(&mut filter).hint_text("🔍 Find clips").desired_width(f32::INFINITY));
    ui.ctx().memory_mut(|m| m.data.insert_temp(fkey, filter.clone()));
    ui.horizontal(|ui| {
        if ui.small_button("Select Unused Files").on_hover_text("Files no clip on any playlist plays (Clear removes them)").clicked() {
            let unused = unused_sources(app.engine.session());
            if unused.is_empty() {
                app.ui.status = "Every file is used".into();
            } else {
                let ids: Vec<u64> = unused.iter().map(|s| s.0).collect();
                let _ = app.run("edit.select", json!({"sources": ids}));
            }
        }
        if ui
            .small_button("Compact")
            .on_hover_text("Make each file again with only the parts its clips use (and a second either side); written at the next save, the old files stay on disk. Undo puts them back")
            .clicked()
        {
            app.ui.status = match app.run("file.compact", json!({})) {
                Ok(v) => {
                    let mb = |f: &serde_json::Value| f.as_i64().unwrap_or(0) as f64 * 4.0 / 1e6;
                    format!("Compacted {} file(s): {:.1} MB of audio per channel down to {:.1} MB", v["compacted"], mb(&v["frames_before"]), mb(&v["frames_after"]))
                }
                Err(e) => e,
            };
        }
    });
    let needle = filter.to_lowercase();
    let items: Vec<_> = items.into_iter().filter(|(_, n, _, _, _, _)| needle.is_empty() || n.to_lowercase().contains(&needle)).collect();
    egui::ScrollArea::vertical().id_salt("clips_scroll").auto_shrink([false, false]).show(ui, |ui| {
        for (id, name, whole, color, selected, captured) in items {
            let (r, resp) = ui.allocate_exact_size(vec2(ui.available_width(), 17.0), Sense::click_and_drag());
            resp.context_menu(|ui| {
                if captured {
                    let sel_track = app.engine.session().edit.selected_tracks.first().map(|t| t.0);
                    if let Some(t) = sel_track
                        && ui.button("Place on Selected Track").clicked()
                    {
                        let at = app.engine.session().edit.selection.start;
                        if let Err(e) = app.run("clip.place_captured", json!({"clip": id, "track": t, "at": at})) {
                            app.ui.status = e;
                        }
                        ui.close();
                    }
                    if ui.button("Remove from List").clicked() {
                        let _ = app.run("clip.uncapture", json!({"clip": id}));
                        ui.close();
                    }
                    return;
                }
                if whole {
                    if ui.button("Place on New Track").clicked() {
                        let _ = app.run("clip.place_source", json!({"source": id}));
                        ui.close();
                    }
                    let sel_track = app.engine.session().edit.selected_tracks.first().map(|t| t.0);
                    if let Some(t) = sel_track
                        && ui.button("Place on Selected Track").clicked()
                    {
                        let _ = app.run("clip.place_source", json!({"source": id, "track": t}));
                        ui.close();
                    }
                    if app.services.pick_open.is_some() && ui.button("Relink…").on_hover_text("Play this file's clips from another file").clicked()
                    {
                        let picked = app.services.pick_open.as_ref().and_then(|pick| pick("file.relink_audio", &[]));
                        if let Some(path) = picked
                            && let Err(e) = app.run("file.relink_audio", json!({"source": id, "path": path}))
                        {
                            app.ui.status = e;
                        }
                        ui.close();
                    }
                    ui.separator();
                }
                if !whole {
                    if ui.button("Audition").clicked() {
                        crate::audition::clip(app, ClipId(id));
                        ui.close();
                    }
                    if ui.button("Rename…").clicked() {
                        let _ = app.run("edit.select", json!({"clips": [id], "exact": true}));
                        app.dialogs.open_for_command(&app.engine, "clip.rename");
                        ui.close();
                    }
                }
                if ui.button("Clear…").clicked() {
                    let (clips, sources) = if whole { (vec![], vec![SourceId(id)]) } else { (vec![ClipId(id)], vec![]) };
                    app.dialogs.open_clear(&app.engine, clips, sources);
                    ui.close();
                }
            });
            // Whole files and clips drag onto tracks (a clip lands as a copy).
            if whole {
                resp.dnd_set_drag_payload(crate::DragSource(id));
            } else if captured {
                resp.dnd_set_drag_payload(crate::DragCaptured(id));
            } else {
                resp.dnd_set_drag_payload(crate::DragClip(id));
            }
            if resp.dragged() {
                ui.ctx().set_cursor_icon(egui::CursorIcon::Grabbing);
            }
            if selected {
                ui.painter().rect_filled(r, 0.0, t.row_selected);
            }
            ui.painter().rect_filled(Rect::from_min_size(pos2(r.min.x + 6.0, r.min.y + 4.0), vec2(9.0, 9.0)), 1.0, rgb(color));
            ui.painter().with_clip_rect(r).text(
                pos2(r.min.x + 22.0, r.center().y),
                Align2::LEFT_CENTER,
                &name,
                if whole { bold(11.0) } else { regular(11.0) },
                t.text,
            );
            if captured {
                // Placed by dragging or from its menu; nothing to select on a track.
            } else if resp.clicked() && !whole && ui.input(|i| i.modifiers.alt) {
                // Alt-click auditions a clip, as in Pro Tools.
                crate::audition::clip(app, ClipId(id));
            } else if resp.clicked() {
                let _ = app.run("edit.select", if whole { json!({"sources": [id]}) } else { json!({"clips": [id]}) });
                if !whole {
                    reveal_clip(app, ClipId(id));
                }
            }
        }
    });
}

/// The files no clip plays on any track's playlists (a drum kit's pad samples, which its
/// instrument plays, count as used).
pub fn unused_sources(s: &soundcraft_model::Session) -> Vec<SourceId> {
    let mut used: std::collections::HashSet<SourceId> =
        s.tracks.iter().filter_map(|t| t.instrument.as_ref()).flat_map(|i| i.samples.values().copied()).collect();
    // A clip's process history keeps the clips it was made from: their files count as used.
    for c in s.all_clips().flat_map(|c| std::iter::once(c).chain(c.processes.iter().map(|p| &*p.before))) {
        if let soundcraft_model::ClipContent::Audio { source, .. } = &c.content {
            used.insert(*source);
        }
        if let Some(k) = c.clean_alt() {
            used.insert(k.source);
        }
    }
    s.sources.iter().map(|src| src.id).filter(|id| !used.contains(id)).collect()
}

/// Scrolls the Edit window to `clip` when it is off screen.
fn reveal_clip(app: &mut SoundApp, id: ClipId) {
    let s = app.engine.session();
    let Some((_, c)) = s.find_clip(id) else { return };
    let (start, end) = (c.start, c.end());
    let z = &s.edit.zoom;
    let width = f64::from(app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(1.0);
    let shown = (z.scroll, z.scroll.saturating_add((width * z.samples_per_px.max(0.01)) as i64));
    if end <= shown.0 || start >= shown.1 {
        let margin = ((shown.1 - shown.0) / 10).max(0);
        let _ = app.run("view.scroll", json!({"to": start.saturating_sub(margin).max(0)}));
    }
}

/// Floating windows.
pub fn floating(app: &mut SoundApp, ctx: &egui::Context) {
    transport_window(app, ctx);
    big_counter(app, ctx);
    plugin_windows(app, ctx);
    full_editors(app, ctx);
    crate::mix_window::send_window(app, ctx);
    about(app, ctx);
    session_info(app, ctx);
}

fn transport_window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_transport;
    if !open {
        return;
    }
    let t = Tokens::current();
    egui::Window::new("Transport").open(&mut open).resizable(false).default_pos(pos2(400.0, 500.0)).show(ctx, |ui| {
        let flags = app.engine.session().edit.flags.clone();
        let expanded = flags.contains("view.transport.expanded");
        ui.horizontal(|ui| {
            for (icon, cmd) in [
                ("rtz", "transport.rtz"),
                ("rewind", "transport.rewind"),
                ("stop", "transport.stop"),
                ("play", "transport.play"),
                ("ffwd", "transport.fast_forward"),
                ("end", "transport.go_to_end"),
                ("record", "transport.record"),
            ] {
                let resp = crate::widgets::icon_button(
                    ui,
                    vec2(34.0, 26.0),
                    icon,
                    (cmd == "transport.play" && app.is_playing()) || (cmd == "transport.record" && app.engine.transport.recording),
                    cmd,
                );
                let resp = match crate::record_unavailable(cfg!(target_arch = "wasm32")) {
                    Some(why) if cmd == "transport.record" => resp.on_hover_text(why),
                    _ => resp,
                };
                if cmd == "transport.record"
                    && let Some(why) = app.record_hint()
                {
                    egui::Tooltip::for_widget(&resp).show(|ui| ui.label(why));
                }
                crate::toolbar::transport_key(app, ui, &resp, cmd);
            }
            if flags.contains("view.transport.output_meters") {
                let (r, _) = ui.allocate_exact_size(vec2(22.0, 26.0), Sense::hover());
                let m = app.main_meter;
                crate::widgets::meter_of(ui, Rect::from_min_size(r.min, vec2(10.0, 26.0)), &m, 0);
                crate::widgets::meter_of(ui, Rect::from_min_size(pos2(r.min.x + 12.0, r.min.y), vec2(10.0, 26.0)), &m, 1);
            }
        });
        let s = app.engine.session();
        let fmt = |x: i64| s.format_position(x, s.edit.main_counter);
        if flags.contains("view.transport.counters") || expanded || flags.is_empty() {
            ui.label(egui::RichText::new(fmt(app.position())).font(mono(20.0)).color(t.counter_text));
        }
        if expanded {
            let e = s.edit.clone();
            egui::Grid::new("tx_exp").num_columns(4).show(ui, |ui| {
                ui.label("Start");
                ui.label(egui::RichText::new(fmt(e.selection.start)).font(mono(11.0)));
                ui.label("Pre-roll");
                ui.label(egui::RichText::new(format!("{:.2} s", s.sample_rate.seconds(e.pre_roll))).font(mono(11.0)));
                ui.end_row();
                ui.label("End");
                ui.label(egui::RichText::new(fmt(e.selection.end)).font(mono(11.0)));
                ui.label("Post-roll");
                ui.label(egui::RichText::new(format!("{:.2} s", s.sample_rate.seconds(e.post_roll))).font(mono(11.0)));
                ui.end_row();
            });
        }
        if flags.contains("view.transport.midi_controls") || expanded {
            let (click, countoff, merge, loop_rec, prepost) = {
                let e = &app.engine.session().edit;
                (e.click, e.countoff, e.midi_merge, e.loop_record, e.pre_post_roll)
            };
            ui.horizontal(|ui| {
                for (label, on, cmd) in [
                    ("Click", click, "options.click"),
                    ("Count Off", countoff, "options.countoff"),
                    ("MIDI Merge", merge, "options.midi_merge"),
                    ("Loop Rec", loop_rec, "options.loop_record"),
                    ("Pre/Post", prepost, "options.pre_post_roll"),
                ] {
                    let r = ui.selectable_label(on, label);
                    if r.clicked() {
                        let _ = app.run(cmd, json!({}));
                    }
                    if cmd == "options.midi_merge" {
                        // The MIDI record mode: Merge is this switch; right-click for the others.
                        let mode = soundcraft_engine::midi_record::record_mode(app.engine.session());
                        r.on_hover_text("On: a MIDI take merges into the clips under it. Right-click for the record mode").context_menu(|ui| {
                            for (m, text) in [
                                (soundcraft_engine::midi_record::RecordMode::Replace, "Replace"),
                                (soundcraft_engine::midi_record::RecordMode::Merge, "Merge"),
                                (soundcraft_engine::midi_record::RecordMode::NewPlaylist, "New Playlist"),
                                (soundcraft_engine::midi_record::RecordMode::TakeLanes, "Keep Takes as Lanes"),
                            ] {
                                if ui.radio(mode == m, text).clicked() {
                                    let _ = app.run("options.midi_record_mode", json!({"mode": m.name()}));
                                    ui.close();
                                }
                            }
                        });
                    }
                }
            });
            let tick = app.engine.session().tempo.samples_to_ticks(app.position(), app.engine.session().sample_rate);
            let bpm = app.engine.session().tempo.tempo_at_tick(tick);
            let m = app.engine.session().tempo.meter_at_tick(tick);
            ui.label(egui::RichText::new(format!("♩ = {bpm:.2}   {}/{}", m.numerator, m.denominator)).font(mono(12.0)).color(t.counter_text));
        }
    });
    app.ui.show_transport = open;
}

fn big_counter(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_big_counter;
    if !open {
        return;
    }
    egui::Window::new("Big Counter").open(&mut open).default_size(vec2(520.0, 110.0)).show(ctx, |ui| {
        let s = app.engine.session();
        let txt = s.format_position(app.position(), s.edit.main_counter);
        let r = ui.available_rect_before_wrap();
        let t = Tokens::current();
        ui.painter().rect_filled(r, 4.0, t.big_counter_bg);
        ui.painter().text(r.center(), Align2::CENTER_CENTER, txt, mono((r.height() * 0.6).clamp(20.0, 120.0)), t.counter_text);
    });
    app.ui.show_big_counter = open;
}

pub(crate) fn memory_locations(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        if ui.button("+ Marker").clicked() {
            let _ = app.run("markers.add", json!({}));
        }
        if ui.button("+ Selection").clicked() {
            let _ = app.run("markers.add", json!({"kind": "selection"}));
        }
        if ui.button("+ View").on_hover_text("A location that recalls this zoom, scroll, the track heights and which tracks are shown").clicked() {
            let _ = app.run("markers.add", json!({"kind": "none", "view": true}));
        }
    });
    let rows: Vec<(u32, String, String, String, String)> = {
        let s = app.engine.session();
        let f = |x: i64| s.format_position(x, s.edit.main_counter);
        s.markers
            .iter()
            .map(|m| {
                let kind = match m.kind {
                    soundcraft_model::MarkerKind::Marker => "marker",
                    soundcraft_model::MarkerKind::Selection => "selection",
                    soundcraft_model::MarkerKind::None => "view",
                };
                // A location that also recalls the view says so.
                let kind = if m.view.is_some() && m.kind != soundcraft_model::MarkerKind::None { format!("{kind} + view") } else { kind.to_string() };
                let len = if m.kind == soundcraft_model::MarkerKind::Selection { f(m.end - m.start) } else { String::new() };
                (m.number, m.name.clone(), f(m.start), len, kind)
            })
            .collect()
    };
    egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new("mem_grid").striped(true).num_columns(6).show(ui, |ui| {
            for h in ["#", "Name", "Location", "Length", "Type", ""] {
                ui.label(egui::RichText::new(h).strong());
            }
            ui.end_row();
            for (n, name, loc, len, kind) in rows {
                if ui.button(n.to_string()).on_hover_text("Recall").clicked() {
                    let _ = app.run("markers.recall", json!({"number": n}));
                }
                let key = egui::Id::new(("mem_name", n));
                let mut edit: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| name.clone());
                let r = ui.add(egui::TextEdit::singleline(&mut edit).desired_width(140.0));
                if r.changed() {
                    ui.ctx().memory_mut(|m| m.data.insert_temp(key, edit.clone()));
                }
                if r.lost_focus() && edit != name {
                    let _ = app.run("markers.edit", json!({"number": n, "rename": edit}));
                    ui.ctx().memory_mut(|m| m.data.remove::<String>(key));
                }
                ui.label(egui::RichText::new(loc).font(mono(11.0)));
                ui.label(egui::RichText::new(len).font(mono(11.0)));
                ui.label(kind);
                if ui.small_button("✕").on_hover_text("Delete").clicked() {
                    let _ = app.run("markers.delete", json!({"number": n}));
                }
                ui.end_row();
            }
        });
    });
}

pub(crate) fn undo_history(app: &mut SoundApp, ui: &mut Ui) {
    let hist = app.engine.undo_history();
    let redo = app.engine.redo_history();
    let n = hist.len();
    egui::ScrollArea::vertical().show(ui, |ui| {
        for (i, h) in hist.iter().enumerate() {
            if ui.selectable_label(i + 1 == n, format!("{}  {h}", i + 1)).clicked() {
                history_go(app, n - i - 1, 0);
            }
        }
        // The undone steps, dimmed: a click redoes up to and including that one.
        for (i, r) in redo.iter().enumerate() {
            let text = egui::RichText::new(format!("{}  {r}", n + i + 1)).italics().color(Tokens::current().text_dim);
            if ui.selectable_label(false, text).on_hover_text("Redo to here").clicked() {
                history_go(app, 0, i + 1);
            }
        }
    });
}

/// Undoes `undo` steps, then redoes `redo`: a click in the Undo History moves to that step.
pub(crate) fn history_go(app: &mut SoundApp, undo: usize, redo: usize) {
    for _ in 0..undo {
        let _ = app.run("edit.undo", json!({}));
    }
    for _ in 0..redo {
        let _ = app.run("edit.redo", json!({}));
    }
}

/// The plugin windows drawn in `ctx`'s viewport: each over the window that opened it.
pub(crate) fn plugin_windows(app: &mut SoundApp, ctx: &egui::Context) {
    follow_selection(app);
    let wins = app.ui.plugin_windows.clone();
    let mut keep = Vec::new();
    for (tid, key) in wins {
        if !crate::window_sync::draws_here(app.window_sync.plugin_homes.get(&(tid, key)).copied(), ctx) {
            keep.push((tid, key));
            continue;
        }
        // The track's instrument, or a chain module (found by id wherever it now sits).
        let instrument = key == soundcraft_mix::SlotKey::Instrument;
        let Some((tname, ins, pos, frozen)) = app.engine.session().track(tid).and_then(|t| {
            let (ins, pos) = match key {
                soundcraft_mix::SlotKey::Instrument => (t.instrument.clone(), 0),
                soundcraft_mix::SlotKey::Module(m) => (t.module(m).cloned(), t.mixer.module_index(m).unwrap_or_default()),
            };
            // A frozen track's instrument is read-only (its render plays), like its frozen modules.
            let frozen = match key {
                soundcraft_mix::SlotKey::Instrument => t.frozen_audio().is_some(),
                soundcraft_mix::SlotKey::Module(m) => match soundcraft_model::KitFx::place(m) {
                    // A kit effect is frozen with its instrument.
                    Some(_) => t.frozen_audio().is_some(),
                    None => t.mixer.module_index(m).is_some_and(|i| crate::chain_view::module_frozen(t, i)),
                },
            };
            ins.map(|i| (t.name.clone(), i, pos, frozen))
        }) else {
            continue;
        };
        let ins = edited_channel_view(ins);
        let hosted = soundcraft_mix::is_third_party(&ins.plugin);
        // A hosted plugin not described (yet) still gets its window, for its editor and picture.
        let info = crate::mix_window::plugin_info(&ins.plugin);
        if info.is_none() && !hosted {
            continue;
        }
        let name = crate::chain_view::plugin_name(&ins.plugin);
        let place = if instrument { "instrument".to_string() } else { format!("{}", pos + 1) };
        let juke = instrument && ins.plugin == crate::juke_window::JUKE && info.is_some();
        let sampler = instrument && ins.plugin == crate::sampler_window::SAMPLER;
        let view = PluginView { tid, key, ins: &ins, info, frozen, tname: &tname, name: &name, hosted, juke, sampler };
        let open = if app.ui.detached_plugins.contains(&(tid, key)) {
            detached_plugin_window(app, ctx, &view)
        } else {
            docked_plugin_window(app, ctx, &view, &place)
        };
        if open {
            keep.push((tid, key));
        }
    }
    app.ui.plugin_windows = keep;
}

/// What a plugin window shows of a module: for one running multi-mono unlinked with its window
/// on another channel ([`soundcraft_model::Insert::edit_channel`]), that channel's own values in
/// place of the first's, so every editor shows the channel it edits (its edits go there too:
/// `mix.module_param` writes the edited channel).
pub(crate) fn edited_channel_view(mut ins: soundcraft_model::Insert) -> soundcraft_model::Insert {
    let ch = usize::from(ins.edit_channel);
    if ins.multi_mono && ins.multi_mono_unlinked && ch > 0 {
        let own: Vec<(String, f32)> = ins
            .params
            .iter()
            .filter_map(|(k, v)| soundcraft_model::split_channel_param(k).filter(|(n, _)| *n == ch).map(|(_, p)| (p.to_string(), *v)))
            .collect();
        ins.params.extend(own);
    }
    ins
}

/// The channel picker of a multi-mono unlinked module's window: which channel it shows and
/// edits, and Link to give every channel the module's settings again.
fn channel_picker(app: &mut SoundApp, ui: &mut Ui, tid: soundcraft_model::TrackId, ins: &soundcraft_model::Insert) {
    let s = app.engine.session();
    let Some(t) = s.track(tid) else { return };
    let channels = soundcraft_mix::strip_channels(t, soundcraft_mix::main_channels(s));
    if !(ins.multi_mono && ins.multi_mono_unlinked) || channels < 2 {
        return;
    }
    let format = t.format;
    let name = |c: usize| format!("Channel {} ({})", c + 1, format.channel_label(c));
    let mut pick = usize::from(ins.edit_channel);
    egui::ComboBox::from_id_salt(("mm-channel", tid.0, ins.id.0)).selected_text(name(pick)).show_ui(ui, |ui| {
        for c in 0..channels {
            ui.selectable_value(&mut pick, c, name(c));
        }
    });
    if pick != usize::from(ins.edit_channel) {
        let _ = app.run("mix.module_channel", json!({"track": tid.0, "module": ins.id.0, "channel": pick}));
    }
    if ui.button("Link").on_hover_text("Multi-mono linked: every channel takes these settings").clicked() {
        let _ = app.run("mix.module_multi_mono", json!({"track": tid.0, "module": ins.id.0, "linked": true}));
    }
}

/// Moves every following plugin window ([`crate::UiState::following_plugins`]) to the track
/// selected last, when that track has the same plug-in: its instrument, or its first module of
/// that plug-in. A window whose plug-in the track lacks stays where it is; one already open there
/// is not opened twice. Pin, detach and follow go with the window.
pub(crate) fn follow_selection(app: &mut SoundApp) {
    let s = app.engine.session();
    let Some(&sel) = s.edit.selected_tracks.last() else { return };
    let Some(to) = s.track(sel) else { return };
    let mut moves = Vec::new();
    for &(tid, key) in &app.ui.following_plugins {
        if tid == sel {
            continue;
        }
        let plugin = s.track(tid).and_then(|t| match key {
            soundcraft_mix::SlotKey::Instrument => t.instrument.as_ref().map(|i| i.plugin.clone()),
            soundcraft_mix::SlotKey::Module(m) => t.module(m).map(|i| i.plugin.clone()),
        });
        let Some(plugin) = plugin else { continue };
        let target = match key {
            soundcraft_mix::SlotKey::Instrument => to.instrument.as_ref().filter(|i| i.plugin == plugin).map(|_| soundcraft_mix::SlotKey::Instrument),
            soundcraft_mix::SlotKey::Module(_) => to.mixer.inserts.iter().find(|i| i.plugin == plugin).map(|i| soundcraft_mix::SlotKey::Module(i.id)),
        };
        if let Some(target) = target {
            moves.push(((tid, key), (sel, target)));
        }
    }
    for (from, to) in moves {
        if app.ui.plugin_windows.contains(&to) {
            continue;
        }
        for list in [&mut app.ui.plugin_windows, &mut app.ui.pinned_plugins, &mut app.ui.detached_plugins, &mut app.ui.following_plugins] {
            for k in list.iter_mut().filter(|k| **k == from) {
                *k = to;
            }
        }
    }
}

/// What a plugin window shows: a slot's plugin, with what it is.
struct PluginView<'a> {
    tid: TrackId,
    key: soundcraft_mix::SlotKey,
    ins: &'a soundcraft_model::Insert,
    info: Option<&'static soundcraft_dsp::PluginInfo>,
    frozen: bool,
    tname: &'a str,
    name: &'a str,
    hosted: bool,
    /// The Juke drum machine's own window.
    juke: bool,
    /// The Lift sampler's own editor.
    sampler: bool,
}

/// The default size of a plugin's window: Juke's, the sampler editor's, or a parameter list's.
fn plugin_window_size(v: &PluginView) -> egui::Vec2 {
    if v.juke {
        crate::juke_window::WINDOW_SIZE
    } else if v.sampler {
        vec2(crate::sampler_window::EDITOR_WIDTH + 24.0, 640.0)
    } else {
        vec2(380.0, 520.0)
    }
}

/// A plugin window inside the main (or Mix) window. Returns whether it stays open.
fn docked_plugin_window(app: &mut SoundApp, ctx: &egui::Context, v: &PluginView, place: &str) -> bool {
    let (tid, key) = (v.tid, v.key);
    let mut open = true;
    let wid = egui::Id::new(("plugin", tid.0, key));
    let screen = ctx.content_rect();
    let anchor = ctx.data(|d| d.get_temp::<egui::Pos2>(egui::Id::new(("plugin-anchor", tid.0, key))));
    let start = match anchor {
        Some(_) => window_place(anchor, vec2(356.0, 320.0), screen).min,
        None => egui::pos2(screen.width() - 380.0, 120.0),
    };
    let mut window = egui::Window::new(format!("{} · {place} · {}", v.tname, v.name))
        .id(wid)
        .open(&mut open)
        .default_width(if v.sampler { crate::sampler_window::EDITOR_WIDTH } else { 340.0 })
        .max_height((screen.height() - 48.0).max(120.0))
        .constrain(true);
    if app.ui.pinned_plugins.contains(&(tid, key)) {
        // Pinned: drawn over every other window in the main window.
        window = window.order(egui::Order::Foreground);
    }
    if v.juke {
        let size = (crate::juke_window::WINDOW_SIZE + vec2(0.0, DETACH_BAR_H)).min(screen.size() - vec2(24.0, 48.0));
        window = window.default_size(size).resizable(true);
    }
    // Always wholly on screen, also a remembered position that no longer is (spec 2026-11-02
    // item 6).
    window = match back_on_screen(ctx, wid, screen) {
        Some(p) => window.current_pos(p),
        None => window.default_pos(start),
    };
    let shown = window.show(ctx, |ui| {
        detach_bar(app, ui, v, false);
        plugin_contents(app, ui, v);
    });
    if let Some(w) = shown
        && v.sampler
    {
        crate::sampler_window::drop_target(app, ctx, w.response.rect, tid);
    }
    open
}

/// The height of the row with the Detach (or Dock) button over a plugin window's contents.
const DETACH_BAR_H: f32 = 22.0;

/// The row over a plugin window's contents: Detach (in the main window) or Dock (in its own).
fn detach_bar(app: &mut SoundApp, ui: &mut Ui, v: &PluginView, detached: bool) {
    let key = (v.tid, v.key);
    ui.horizontal(|ui| {
        ui.set_height(DETACH_BAR_H - 6.0);
        if detached {
            let r = ui.small_button("⇲ Dock").on_hover_text("Put this window back inside the main window");
            if r.clicked() {
                app.ui.detached_plugins.retain(|k| *k != key);
            }
        } else {
            let r = ui
                .small_button("⇱ Detach")
                .on_hover_text("Move this window out of the main window, into a window of its own that you can put anywhere (another screen too)");
            if r.clicked() && !app.ui.detached_plugins.contains(&key) {
                app.ui.detached_plugins.push(key);
            }
        }
        let pinned = app.ui.pinned_plugins.contains(&key);
        let r = ui.selectable_label(pinned, "📌 Pin").on_hover_text("Keep this window on top of the others");
        if r.clicked() {
            if pinned {
                app.ui.pinned_plugins.retain(|k| *k != key);
            } else {
                app.ui.pinned_plugins.push(key);
            }
        }
        let following = app.ui.following_plugins.contains(&key);
        let r = ui.selectable_label(following, "Follow").on_hover_text("Show this plug-in on whichever track is selected");
        if r.clicked() {
            if following {
                app.ui.following_plugins.retain(|k| *k != key);
            } else {
                app.ui.following_plugins.push(key);
            }
        }
        ui.label(egui::RichText::new(format!("{} · {}", v.tname, v.name)).color(Tokens::current().text_dim).small());
    });
}

/// The viewport of a detached plugin window.
pub fn plugin_viewport(tid: TrackId, key: soundcraft_mix::SlotKey) -> egui::ViewportId {
    egui::ViewportId::from_hash_of(("soundcraft.plugin", tid.0, key))
}

/// A plugin window in a native window of its own (a backend without windows embeds it). Its
/// place and size are remembered per plugin. Closing it closes the plugin window; the plugin
/// stays detached, so the window comes back out when opened again. Returns whether it stays open.
fn detached_plugin_window(app: &mut SoundApp, ctx: &egui::Context, v: &PluginView) -> bool {
    let (tid, key) = (v.tid, v.key);
    let title = format!("{} · {}", v.tname, v.name);
    let mut builder = egui::ViewportBuilder::default().with_title(title).with_resizable(true).with_min_inner_size([320.0, 200.0]);
    if app.ui.pinned_plugins.contains(&(tid, key)) {
        // Pinned: the system keeps the window over every other one.
        builder = builder.with_window_level(egui::WindowLevel::AlwaysOnTop);
    }
    let remembered = app.ui.plugin_window_rects.get(&v.ins.plugin).copied();
    match remembered.filter(|r| r.iter().all(|x| x.is_finite()) && r[2] >= 200.0 && r[3] >= 120.0) {
        Some([x, y, w, h]) => builder = builder.with_position([x, y]).with_inner_size([w, h]),
        None => builder = builder.with_inner_size(plugin_window_size(v) + vec2(0.0, DETACH_BAR_H)),
    }
    let mut open = true;
    ctx.show_viewport_immediate(plugin_viewport(tid, key), builder, |ui, _class| {
        ui.set_style(ui.ctx().global_style());
        let own = ui.ctx().viewport_id() != egui::ViewportId::ROOT;
        if own {
            // Its keys: transport, undo, save… work from here too.
            crate::shortcuts::handle(app, ui.ctx());
            let info = ui.ctx().input(|i| (i.viewport().outer_rect, i.viewport().inner_rect));
            if let (Some(outer), Some(inner)) = info {
                app.ui.plugin_window_rects.insert(v.ins.plugin.clone(), [outer.min.x, outer.min.y, inner.width(), inner.height()]);
            }
        }
        let full = ui.max_rect();
        ui.painter().rect_filled(full, 0.0, ui.visuals().panel_fill);
        let mut inner = ui.new_child(egui::UiBuilder::new().max_rect(full.shrink(6.0)));
        detach_bar(app, &mut inner, v, true);
        egui::ScrollArea::both().auto_shrink([false, false]).show(&mut inner, |ui| {
            plugin_contents(app, ui, v);
        });
        if v.sampler {
            crate::sampler_window::drop_target(app, ui.ctx(), full, tid);
        }
        if own {
            // Files dropped on this window (the sampler's or Juke's cells).
            let _ = app.take_dropped_files(ui.ctx());
        }
        if ui.ctx().input(|i| i.viewport().close_requested()) {
            open = false;
        }
    });
    open
}

/// A plugin window's contents: Juke's window, the sampler editor, a hosted plugin's buttons or
/// the generic parameter list.
fn plugin_contents(app: &mut SoundApp, ui: &mut Ui, v: &PluginView) {
    let PluginView { tid, key, ins, info, frozen, tname, name, hosted, juke, .. } = *v;
    let instrument = key == soundcraft_mix::SlotKey::Instrument;
    let read_only = format!("`{tname}` is frozen. {}", soundcraft_engine::cmd::UNFREEZE_TO_EDIT);
    let module = ins.id;
    if juke {
        crate::juke_window::draw(app, ui, tid, ins, frozen);
        if frozen {
            ui.label(egui::RichText::new(read_only.as_str()).color(Tokens::current().text_dim));
        }
        return;
    }
    ui.horizontal(|ui| {
        // Bypass and presets act on insert slots.
        if !instrument && let Some(info) = info {
            ui.label(egui::RichText::new(&ins.preset).color(Tokens::current().text_dim));
            let label = if ins.bypass { "BYPASSED" } else { "Bypass" };
            if ui.button(label).clicked() {
                let _ = app.run("mix.module_bypass", json!({"track": tid.0, "module": module.0}));
            }
            ui.menu_button("Presets ▾", |ui| preset_menu(app, ui, tid, module, info, ins));
        }
        if !instrument {
            channel_picker(app, ui, tid, ins);
        }
        // A built-in instrument's factory presets (the Subtractive Synth's).
        if instrument
            && let Some(info) = info
            && !crate::fs_presets::factory_groups(info).is_empty()
        {
            ui.menu_button("Presets ▾", |ui| crate::fs_presets::instrument_factory_menu(app, ui, tid, info));
        }
        if hosted {
            let open = app.player.as_ref().is_some_and(|p| p.editor_open(tid, key));
            let available = app.player.as_ref().is_some_and(|p| p.has_editor(tid, key));
            let label = if open { "Close Plugin Editor" } else { "Open Plugin Editor" };
            let resp = ui.add_enabled(available, egui::Button::new(label));
            let resp = if available { resp } else { resp.on_disabled_hover_text("This plugin has no editor of its own, or it is not loaded") };
            if resp.clicked()
                && let Some(p) = &app.player
            {
                if open {
                    p.close_editor(tid, key);
                } else if let Err(e) = p.open_editor(tid, key) {
                    app.ui.status = format!("{name}: {e}");
                }
            }
            let can = crate::browser::thumbs::can_capture_now(app, (tid, key));
            let resp = ui.add_enabled(can, egui::Button::new("Capture Thumbnail Now"));
            let resp = resp
                .on_hover_text("Take the plugin browser's picture of this plugin from its open editor, now")
                .on_disabled_hover_text("Open the plugin's editor first");
            if resp.clicked() {
                crate::browser::thumbs::capture_now(app, (tid, key));
            }
            // A/B of the plug-in's whole state, beside its editor (an inserted one's preset menu
            // has it too).
            if !instrument {
                let b = crate::preset_house::on_b(ui.ctx(), tid, module);
                let resp = ui.button(if b { "B" } else { "A" }).on_hover_text(if b {
                    "Comparing: B. Click to go back to A"
                } else {
                    "Compare: click to switch to B (a copy of these settings the first time)"
                });
                if resp.clicked()
                    && let Err(e) = crate::preset_house::ab_toggle_hosted(app, ui.ctx(), tid, ins)
                {
                    app.ui.status = e;
                }
            }
        }
    });
    let Some(info) = info.filter(|_| !(instrument && hosted)) else {
        // A hosted instrument keeps its sounds and settings in its own editor.
        let loaded = app.player.as_ref().is_some_and(|p| p.has_editor(tid, key));
        let note = if info.is_none() && !loaded {
            "This plugin is not installed on this computer, or it is not loaded."
        } else if instrument {
            "Load and edit sounds in the plugin's own editor."
        } else {
            "Edit this plugin in its own editor."
        };
        ui.label(egui::RichText::new(note).color(Tokens::current().text_dim));
        return;
    };
    if instrument && ins.plugin == crate::sampler_window::SAMPLER {
        if frozen {
            ui.disable();
        }
        let r = ui.scope(|ui| crate::sampler_window::editor(app, ui, tid, ins)).response;
        if frozen {
            r.on_disabled_hover_text(read_only.as_str());
        }
        return;
    }
    if info.id == "eq_7band" || info.id == "eq_1band" {
        eq_curve(ui, info.id, ins);
    }
    if instrument && info.id == crate::synth_panel::SYNTH {
        // The synth's panel: its filter and envelopes drawn, over the controls.
        let r = ui.scope(|ui| {
            if frozen {
                ui.disable();
            }
            crate::synth_panel::draw(app, ui, tid, ins);
        });
        if frozen {
            r.response.on_disabled_hover_text(read_only.as_str());
        }
    }
    // What the drawer offers, by the drawer's names (a FireStation engine's own
    // bypass and a model's unused switches are not here).
    let params = crate::module_drawer::window_params(ins, info);
    let grid = egui::ScrollArea::vertical().show(ui, |ui| {
        egui::Grid::new(("pg", tid.0, key)).num_columns(3).show(ui, |ui| {
            if frozen {
                ui.disable();
            }
            for p in params {
                let v = ins.params.get(p.id).copied().unwrap_or(p.default);
                ui.label(crate::module_drawer::dial_name(ins, p.id).unwrap_or(p.name));
                let mut x = v;
                let choices = crate::module_drawer::choice_names(ins, p);
                let resp = if !choices.is_empty() {
                    let mut idx = x.round().max(0.0) as usize;
                    let r = egui::ComboBox::from_id_salt(("pc", tid.0, key, p.id)).selected_text(choices.get(idx).copied().unwrap_or("?")).show_ui(
                        ui,
                        |ui| {
                            for (i, c) in choices.iter().enumerate() {
                                ui.selectable_value(&mut idx, i, *c);
                            }
                        },
                    );
                    x = idx as f32;
                    r.response
                } else {
                    let mut sl = egui::Slider::new(&mut x, p.min..=p.max).show_value(false);
                    if instrument && crate::sampler_window::whole_number(&ins.plugin, p.id) {
                        sl = sl.integer();
                    }
                    if p.taper == soundcraft_dsp::Taper::Log && p.min > 0.0 {
                        sl = sl.logarithmic(true);
                    }
                    ui.add(sl)
                };
                if (x - v).abs() > f32::EPSILON && instrument {
                    // A refusal (the track is frozen) shows in the status line.
                    if let Err(e) = app.engine.execute_merged(
                        "mix.instrument_param",
                        &json!({"track": tid.0, "param": p.id, "value": x}),
                        &format!("instrument_param:{}:{}", tid.0, p.id),
                    ) {
                        app.ui.status = e.to_string();
                    }
                } else if (x - v).abs() > f32::EPSILON {
                    let _ = app.engine.execute_merged(
                        "mix.module_param",
                        &json!({"track": tid.0, "module": module.0, "param": p.id, "value": x}),
                        &format!("modparam:{}:{}:{}", tid.0, module.0, p.id),
                    );
                    if let Some(pl) = &app.player {
                        // Keep the plugin's own editor in step.
                        pl.editor_set_param(tid, key, p.id, x);
                    }
                }
                let _ = resp;
                let shown = if instrument {
                    crate::sampler_window::format_param(&ins.plugin, p, x)
                } else {
                    app.player.as_ref().and_then(|pl| pl.editor_value_text(tid, key, p.id, x)).unwrap_or_else(|| p.format(x))
                };
                ui.label(egui::RichText::new(shown).font(mono(11.0)));
                ui.end_row();
            }
        })
    });
    if frozen {
        grid.inner.response.on_disabled_hover_text(read_only.as_str());
    }
}

fn eq_curve(ui: &mut Ui, id: &str, ins: &soundcraft_model::Insert) {
    let t = Tokens::current();
    let (r, _) = ui.allocate_exact_size(vec2(ui.available_width().max(300.0), 110.0), Sense::hover());
    ui.painter().rect_filled(r, 4.0, Color32::from_rgb(14, 18, 22));
    let n = 160;
    let freqs: Vec<f32> = (0..n).map(|i| 20.0 * 1000f32.powf(i as f32 / (n - 1) as f32)).collect();
    let params: Vec<(&str, f32)> = ins.params.iter().map(|(k, v)| (k.as_str(), *v)).collect();
    let resp = if id == "eq_7band" {
        soundcraft_dsp::eq7_response(&params, &freqs, 48_000.0)
    } else {
        soundcraft_dsp::eq1_response(&params, &freqs, 48_000.0)
    };
    for db in [-12.0f32, 0.0, 12.0] {
        let y = r.center().y - db / 18.0 * r.height() * 0.5;
        ui.painter().line_segment([pos2(r.min.x, y), pos2(r.max.x, y)], Stroke::new(1.0, Color32::from_rgb(40, 50, 60)));
    }
    let pts: Vec<egui::Pos2> = resp
        .iter()
        .enumerate()
        .map(|(i, db)| pos2(r.min.x + r.width() * i as f32 / (n - 1) as f32, r.center().y - db.clamp(-18.0, 18.0) / 18.0 * r.height() * 0.5))
        .collect();
    ui.painter().add(egui::Shape::line(pts, Stroke::new(2.0, t.counter_text)));
}

fn about(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_about;
    if !open {
        return;
    }
    let tab_id = egui::Id::new("about_tab");
    egui::Window::new("About FreeDAWm").open(&mut open).default_size(vec2(640.0, 420.0)).collapsible(false).show(ctx, |ui| {
        let mut tab = ui.data_mut(|d| d.get_temp::<u8>(tab_id)).unwrap_or(0);
        ui.horizontal(|ui| {
            for (i, l) in (0u8..).zip(["About", "Contributors", "Models"]) {
                if ui.selectable_label(tab == i, l).clicked() {
                    tab = i;
                }
            }
        });
        ui.data_mut(|d| d.insert_temp(tab_id, tab));
        ui.separator();
        match tab {
            1 => crate::credits::contributors_ui(ui),
            2 => crate::credits::models_ui(ui),
            _ => {
                match crate::brand_art::BrandArt::get(ui.ctx()) {
                    Some(art) => {
                        ui.add_space(4.0);
                        ui.add(egui::Image::from_texture(egui::load::SizedTexture::new(art.bird.id(), vec2(96.0, 96.0))).alt_text("FreeDAWm logo"));
                        ui.add_space(6.0);
                        let w = 360.0_f32.min(ui.available_width());
                        ui.add(
                            egui::Image::from_texture(egui::load::SizedTexture::new(art.wordmark().id(), vec2(w, w / art.wordmark_aspect)))
                                .alt_text(crate::brand_art::APP_NAME),
                        );
                        ui.add_space(6.0);
                    }
                    None => {
                        ui.label(egui::RichText::new(crate::brand_art::APP_NAME).font(bold(22.0)));
                    }
                }
                ui.label(format!("Version {}", env!("CARGO_PKG_VERSION")));
                ui.label("An open-source digital audio workstation.");
                ui.label("Based on SoundCraft by the ArtCraft team. Dual-licensed MIT OR Apache-2.0. Made with Rust and egui.");
                ui.hyperlink_to("Source code and issues", crate::brand_art::REPOSITORY_URL);
                ui.add_space(6.0);
                ui.label(egui::RichText::new(TRADEMARK_NOTICE).small().weak());
            }
        }
    });
    app.ui.show_about = open;
}

/// The About box's general trademark notice (NOTICE has the full text).
const TRADEMARK_NOTICE: &str = "Pro Tools, AudioSuite and Avid are trademarks of Avid Technology, Inc. Hardware named in \
     modes, presets and impulse responses (1176, LA-2A, SSL, Neve, Pultec, Fender, Marshall and others) is named only to \
     identify the products studied; those names are trademarks of their respective owners. FreeDAWm is not affiliated \
     with or endorsed by any of them.";

fn session_info(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_session_info;
    if !open {
        return;
    }
    egui::Window::new("Session Info").open(&mut open).default_size(vec2(480.0, 360.0)).show(ctx, |ui| {
        let text = soundcraft_engine::inspect::session_text(&app.engine);
        egui::ScrollArea::vertical().show(ui, |ui| {
            ui.label(egui::RichText::new(text).font(mono(11.0)));
        });
    });
    app.ui.show_session_info = open;
}

/// Folder for a plugin's user presets.
fn preset_dir(app: &SoundApp, plugin: &str) -> Option<std::path::PathBuf> {
    app.preset_dir.as_ref().map(|d| d.join(crate::preset_folder_name(plugin)))
}

/// A module's presets: the factory default, a FireStation engine's factory presets, the user
/// presets saved for its plugin, and a field to save the current settings as one.
pub(crate) fn preset_menu(
    app: &mut SoundApp,
    ui: &mut Ui,
    tid: soundcraft_model::TrackId,
    module: soundcraft_model::ModuleId,
    info: &soundcraft_dsp::PluginInfo,
    ins: &soundcraft_model::Insert,
) {
    if ui.button("<factory default>").clicked() {
        let defaults: serde_json::Map<String, serde_json::Value> = info.params.iter().map(|p| (p.id.to_string(), json!(p.default))).collect();
        let _ = app.run("mix.module_params", json!({"track": tid.0, "module": module.0, "params": defaults, "preset": "<factory default>"}));
        ui.close();
    }
    // A FireStation engine's bundled presets, by group, above the user's.
    crate::fs_presets::factory_menu(app, ui, tid, module, info);
    let Some(dir) = preset_dir(app, info.id) else {
        ui.label("User presets need the desktop app.");
        return;
    };
    let mut names: Vec<String> = std::fs::read_dir(&dir)
        .map(|rd| rd.flatten().filter_map(|e| e.path().file_stem().and_then(|x| x.to_str()).map(str::to_string)).collect())
        .unwrap_or_default();
    names.sort();
    for n in &names {
        if ui.button(n).clicked() {
            if let Ok(text) = std::fs::read_to_string(dir.join(format!("{n}.json")))
                && let Ok(v) = serde_json::from_str::<serde_json::Value>(&text)
            {
                // Old presets may carry parameters the plugin no longer exposes; drop them so recall succeeds.
                let params: serde_json::Map<String, serde_json::Value> = v
                    .get("params")
                    .and_then(serde_json::Value::as_object)
                    .into_iter()
                    .flatten()
                    .filter(|(k, val)| {
                        info.params.iter().any(|pi| pi.id == k.as_str())
                            && k.as_str() != "bypass"
                            && !crate::editors::not_in_presets(info.id).contains(&k.as_str())
                            && val.as_f64().is_some_and(f64::is_finite)
                    })
                    .map(|(k, val)| (k.clone(), val.clone()))
                    .collect();
                let _ = app.run("mix.module_params", json!({"track": tid.0, "module": module.0, "params": params, "preset": n}));
            }
            ui.close();
        }
    }
    ui.separator();
    let key = egui::Id::new(("preset_name", tid.0, module.0));
    let mut name: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| format!("{} preset", info.name));
    ui.horizontal(|ui| {
        ui.add(egui::TextEdit::singleline(&mut name).desired_width(140.0));
        if ui.button("Save").clicked() {
            let clean = soundcraft_engine::io::sanitize_name(&name);
            if crate::editor_kit::presets::is_factory_name(info, &clean) {
                // A user preset of a factory preset's name would never load (the factory one wins).
                app.ui.status = format!("{clean} is a factory preset: pick another name");
            } else {
                // The file the wide editor's Save writes: name, tags, the same parameters, the
                // editor's extra block.
                let skip = crate::editors::not_in_presets(info.id);
                let value = |id: &str| ins.params.get(id).copied().or_else(|| info.param(id).map(|p| p.default)).unwrap_or(0.0);
                let extra = if info.id == "fs_cleanq" { crate::editors::cleanq::preset_extra(ui.ctx(), tid, ins) } else { None };
                let body = crate::editor_kit::presets::user_preset_body(info, skip, value, &clean, "", extra);
                // The one save path every view uses.
                let ok = crate::preset_house::write_user(app, info.id, &clean, &body).is_ok();
                app.ui.status = if ok { format!("Saved preset {clean}") } else { "Could not save the preset".into() };
                if ok {
                    let _ = app.run("mix.module_params", json!({"track": tid.0, "module": module.0, "params": {}, "preset": clean}));
                }
            }
        }
    });
    ui.ctx().memory_mut(|m| m.data.insert_temp(key, name));
    preset_housekeeping(app, ui, tid, info, ins, &names);
}

/// The preset menu's housekeeping rows: rename the user preset in use, choose the preset new
/// inserts start from, and A/B compare.
fn preset_housekeeping(
    app: &mut SoundApp,
    ui: &mut Ui,
    tid: soundcraft_model::TrackId,
    info: &soundcraft_dsp::PluginInfo,
    ins: &soundcraft_model::Insert,
    names: &[String],
) {
    use crate::preset_house as ph;
    ui.separator();
    // Rename the user preset this module shows.
    if names.contains(&ins.preset) {
        let key = egui::Id::new(("preset_rename", tid.0, ins.id.0));
        let mut to: String = ui.ctx().memory(|m| m.data.get_temp(key)).unwrap_or_else(|| ins.preset.clone());
        ui.horizontal(|ui| {
            ui.add(egui::TextEdit::singleline(&mut to).desired_width(140.0));
            if ui.button("Rename").on_hover_text(format!("Rename the user preset {}", ins.preset)).clicked() {
                app.ui.status = match ph::rename_user(app, info, &ins.preset, &to) {
                    Ok(n) => format!("Renamed preset to {n}"),
                    Err(e) => e,
                };
            }
        });
        ui.ctx().memory_mut(|m| m.data.insert_temp(key, to));
    }
    // The default for new inserts.
    let default = ph::default_for(app, info.id);
    let shown = ins.preset.trim();
    if !shown.is_empty()
        && shown != "<factory default>"
        && default.as_deref() != Some(shown)
        && ui.button(format!("Start New Inserts From \"{shown}\"")).clicked()
    {
        app.ui.status = match ph::set_default(app, info.id, Some(shown)) {
            Ok(()) => format!("New {} inserts start from {shown}", info.name),
            Err(e) => e,
        };
        ui.close();
    }
    if let Some(d) = default
        && ui.button(format!("Clear Default Preset ({d})")).clicked()
    {
        let _ = ph::set_default(app, info.id, None);
        ui.close();
    }
    // A/B compare.
    let b = ph::on_b(ui.ctx(), tid, ins.id);
    if ui.button(if b { "Compare: Back to A" } else { "Compare: Switch to B" }).on_hover_text("Two settings to flip between").clicked() {
        if soundcraft_mix::is_third_party(&ins.plugin) {
            if let Err(e) = ph::ab_toggle_hosted(app, ui.ctx(), tid, ins) {
                app.ui.status = e;
            }
        } else {
            ph::ab_toggle(app, ui.ctx(), tid, ins, info);
        }
        ui.close();
    }
    // Copy and paste settings between instances of the plug-in, on any track.
    if ui.button("Copy Settings").on_hover_text("For Paste Settings on another instance").clicked() {
        ph::copy_settings(ui.ctx(), ins, info);
        app.ui.status = format!("Copied the {} settings", info.name);
        ui.close();
    }
    if ui.add_enabled(ph::can_paste(ui.ctx(), info.id), egui::Button::new("Paste Settings")).clicked() {
        if let Err(e) = ph::paste_settings(app, ui.ctx(), tid, ins.id) {
            app.ui.status = e;
        }
        ui.close();
    }
}

/// Where a plugin window of `size` opens: around `anchor` (the pointer when it was opened), moved
/// so all of it is on `screen` ([`crate::widgets::fit_on_screen`]).
fn window_place(anchor: Option<egui::Pos2>, size: egui::Vec2, screen: Rect) -> Rect {
    let at = anchor.map_or(screen.center() - size / 2.0, |a| a + vec2(16.0, 16.0));
    crate::widgets::fit_on_screen(Rect::from_min_size(at, size), screen)
}

/// A window `id` remembered (or dragged) partly off `screen` comes back: the position that puts
/// it on screen, when it needs one.
fn back_on_screen(ctx: &egui::Context, id: egui::Id, screen: Rect) -> Option<egui::Pos2> {
    let prev = ctx.memory(|m| m.area_rect(id))?;
    let fit = crate::widgets::fit_on_screen(prev, screen);
    (fit.min.distance(prev.min) > 0.5).then_some(fit.min)
}

/// The full editor windows (`ui.mix.full_editor`, a tile's ⤢): a built-in module's VST-faithful
/// editor at its full size (never the slim layout) in a window of its own, opened next to where
/// it was asked for and always wholly on screen; one larger than the screen keeps its title bar
/// and top-left in view and scrolls.
pub(crate) fn full_editors(app: &mut SoundApp, ctx: &egui::Context) {
    let wins = app.ui.full_editors.clone();
    let mut keep = Vec::new();
    let screen = ctx.content_rect();
    for (tid, module) in wins {
        let key = soundcraft_mix::SlotKey::Module(module);
        if !crate::window_sync::draws_here(app.window_sync.plugin_homes.get(&(tid, key)).copied(), ctx) {
            keep.push((tid, module));
            continue;
        }
        let session = app.engine.session_arc();
        let Some(track) = session.track(tid) else { continue };
        let Some(ins) = track.module(module) else { continue };
        if !crate::editors::has_wide(&ins.plugin) {
            continue;
        }
        let id = egui::Id::new(("full-editor", tid.0, module.0));
        let size_id = id.with("size");
        // The editor's column as last drawn (title bar and margins on top), else a first guess.
        let natural: egui::Vec2 = ctx.data(|d| d.get_temp(size_id)).unwrap_or(vec2(900.0, 620.0));
        let room = (screen.size() - vec2(24.0, 48.0)).max(vec2(160.0, 120.0));
        let inner = natural.min(room);
        let outer = inner + vec2(16.0, 40.0);
        let anchor = ctx.data(|d| d.get_temp::<egui::Pos2>(id.with("anchor")));
        let title = format!("{} \u{b7} {}", track.name, crate::chain_view::plugin_name(&ins.plugin));
        let mut open = true;
        let mut window = egui::Window::new(title).id(id).open(&mut open).fixed_size(inner).collapsible(false).constrain(true);
        window = match back_on_screen(ctx, id, screen) {
            Some(p) => window.current_pos(p),
            None => window.default_pos(window_place(anchor, outer, screen).min),
        };
        let meters = app.player.as_ref().filter(|_| app.meters_live()).map(|p| p.meters()).or_else(|| app.preview_meters.clone());
        let shown = window.show(ctx, |ui| {
            crate::vocal_editor::hook(ui.ctx(), &format!("full-editor-{}-{}", tid.0, module.0), ui.max_rect());
            egui::ScrollArea::horizontal().id_salt(("full-editor-h", tid.0, module.0)).auto_shrink([false, false]).show(ui, |ui| {
                // The full layout here, whatever the drawers use.
                let slim = std::mem::replace(&mut app.ui.slim_editors, false);
                let top = ui.cursor().min.y;
                // Its controls follow their automation, as in the drawer.
                let shown = crate::mix_window::shown_params(app, track);
                crate::editors::drawer(app, ui, &shown, module, meters.as_ref().and_then(|m| m.tracks.get(&tid)));
                app.ui.slim_editors = slim;
                if let Some(face) = crate::editor_kit::frame_rect(ui.ctx(), tid, module) {
                    let want = vec2(face.width(), (face.max.y - top).max(64.0) + 2.0);
                    ui.ctx().data_mut(|d| d.insert_temp(size_id, want));
                }
            });
        });
        if shown.is_some() && open {
            keep.push((tid, module));
        }
    }
    app.ui.full_editors = keep;
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_engine::Engine;
    use soundcraft_mix::SlotKey;
    use soundcraft_model::ModuleId;

    /// Multi-mono unlinked: the plugin window shows the channel it edits, and a parameter set
    /// from it (as every editor sets one) lands on that channel; the lane picker lists the
    /// other channel's lanes.
    #[test]
    fn a_multi_mono_unlinked_window_shows_and_edits_its_channel() {
        let mut engine = Engine::new(soundcraft_model::Session::default());
        engine.execute("track.new", &json!({"name": "A", "format": "stereo"})).unwrap();
        let mut app = SoundApp::new(engine, None, crate::Services::default());
        let t = app.engine.session().tracks[0].id;
        let m = app.run("mix.module_add", json!({"track": t.0, "plugin": "gain", "params": {"gain": -3.0}})).unwrap()["module"].as_u64().unwrap();
        app.run("mix.module_multi_mono", json!({"track": t.0, "module": m, "linked": false})).unwrap();
        app.run("mix.module_channel", json!({"track": t.0, "module": m, "channel": 1})).unwrap();
        app.run("mix.module_param", json!({"track": t.0, "module": m, "param": "gain", "value": -10.0})).unwrap();
        let ins = app.engine.session().tracks[0].mixer.inserts[0].clone();
        assert_eq!(ins.params["gain"], -3.0, "the first channel keeps its value");
        assert_eq!(edited_channel_view(ins.clone()).params["gain"], -10.0, "the window shows channel 2's");
        let first = soundcraft_model::Insert { edit_channel: 0, ..ins };
        assert_eq!(edited_channel_view(first).params["gain"], -3.0);
        let s = app.engine.session();
        let lanes = crate::auto_lanes::choices(s, &s.tracks[0]);
        assert!(lanes.iter().any(|c| matches!(&c.param, soundcraft_model::AutoParam::Plugin { param, .. } if param == "c1:gain")));
    }

    /// The parity list's "follow the selected track": a plug-in window with Follow on moves to
    /// the same plug-in on the track selected next, keeping its pin; a track without it leaves
    /// the window where it is.
    #[test]
    fn a_following_plugin_window_moves_with_the_selection() {
        let mut engine = Engine::new(soundcraft_model::Session::default());
        for name in ["A", "B", "C"] {
            engine.execute("track.new", &json!({"name": name})).unwrap();
        }
        let mut app = SoundApp::new(engine, None, crate::Services::default());
        let ids: Vec<TrackId> = app.engine.session().tracks.iter().map(|t| t.id).collect();
        let add = |app: &mut SoundApp, t: TrackId, p: &str| -> ModuleId {
            let r = app.run("mix.module_add", json!({"track": t.0, "plugin": p})).unwrap();
            ModuleId(u32::try_from(r["module"].as_u64().unwrap()).unwrap())
        };
        let ma = add(&mut app, ids[0], "heft");
        add(&mut app, ids[1], "keel");
        let mb = add(&mut app, ids[1], "heft");
        add(&mut app, ids[2], "keel");
        let win = (ids[0], SlotKey::Module(ma));
        app.ui.plugin_windows.push(win);
        app.ui.pinned_plugins.push(win);
        app.ui.following_plugins.push(win);
        app.run("edit.select", json!({"tracks": [ids[1].0]})).unwrap();
        follow_selection(&mut app);
        let moved = (ids[1], SlotKey::Module(mb));
        assert_eq!(app.ui.plugin_windows, [moved]);
        assert_eq!(app.ui.pinned_plugins, [moved]);
        assert_eq!(app.ui.following_plugins, [moved]);
        // C has no Heft: the window stays on B
        app.run("edit.select", json!({"tracks": [ids[2].0]})).unwrap();
        follow_selection(&mut app);
        assert_eq!(app.ui.plugin_windows, [moved]);
    }
}

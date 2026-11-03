//! Keeping the Edit and Mix windows in step, above all while Mix has its own native window
//! (Window › Mix in Separate Window): the folder tree in the Tracks list of both windows, folder
//! collapse hiding strips, selection that brings a track into view in the other window, dialogs
//! over the window that started them, and the Window › Sync options.

use crate::{MainWindow, SoundApp};
use egui::ViewportId;
use serde::{Deserialize, Serialize};
use soundcraft_model::{Session, Track, TrackId, TrackKind};
use soundcraft_playback::SlotKey;
use std::collections::HashMap;

/// The separate Mix window's viewport (Window › Mix in Separate Window).
pub fn mix_viewport() -> ViewportId {
    ViewportId::from_hash_of("soundcraft.mix")
}

/// Window › Sync: what the Edit and Mix windows share. All on by default; kept in ui.json.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct SyncOptions {
    /// Selecting a track or strip in either window selects it in both. Off, the Mix window keeps
    /// its own selection.
    pub selection: bool,
    /// A track selected in one window scrolls into view in the other.
    pub scroll: bool,
    /// A closed folder hides its members' strips in the Mix window.
    pub folders: bool,
    /// A track hidden in the Edit window has no strip in the Mix window.
    pub visibility: bool,
}

impl Default for SyncOptions {
    fn default() -> Self {
        SyncOptions { selection: true, scroll: true, folders: true, visibility: true }
    }
}

/// Whether the Mix window shows a strip for `t`: not a video track, not hidden (with Sync Track
/// Visibility) and not inside a closed folder (with Follow Folder Collapse). A closed folder's own
/// strip stays.
pub fn mix_shows(s: &Session, t: &Track, o: SyncOptions) -> bool {
    t.kind != TrackKind::Video && !(o.visibility && t.hidden) && !(o.folders && soundcraft_engine::edit::folder_collapsed(s, t))
}

/// How many folders `track` sits in (at most 6, the indent the lists draw).
pub fn folder_depth(s: &Session, track: &Track) -> usize {
    let mut current = track.folder;
    let mut depth = 0;
    while let Some(folder) = current.and_then(|folder| s.track(folder))
        && depth < 6
    {
        depth += 1;
        current = folder.folder;
    }
    depth
}

/// The Tracks list's rows, top to bottom: each track in session order (a folder's members follow
/// it) with its folder depth, leaving out the members of a closed folder at any level. Hidden
/// tracks stay listed (dimmed), so they can be shown again.
pub fn list_rows(s: &Session) -> Vec<(TrackId, usize)> {
    s.tracks.iter().filter(|t| !soundcraft_engine::edit::folder_collapsed(s, t)).map(|t| (t.id, folder_depth(s, t))).collect()
}

/// What keeps the two windows in step from frame to frame (not saved).
#[derive(Default)]
pub struct WindowSync {
    /// The session's track selection as last seen, to tell when it changes.
    last_selection: Vec<TrackId>,
    /// A track to scroll into view in the Edit window, the next time it is drawn.
    pub reveal_edit: Option<TrackId>,
    /// A strip to scroll into view in the Mix window, the next time it is drawn.
    pub reveal_mix: Option<TrackId>,
    /// The Mix window's own selection while Sync Selection is off.
    pub mix_selection: Vec<TrackId>,
    /// Where the open dialog is drawn: the window that opened it (none: the main window).
    pub dialog_home: Option<ViewportId>,
    /// The open dialog as last homed, to tell a new or replacing one.
    dialog_seen: Option<&'static str>,
    /// Where the Send window is drawn.
    pub send_home: Option<ViewportId>,
    /// Where each plugin window is drawn (absent: the main window).
    pub plugin_homes: HashMap<(TrackId, SlotKey), ViewportId>,
}

/// What was open before a window drew, to tell what it opened.
pub struct Opened {
    dialog: Option<&'static str>,
    send: Option<(TrackId, usize)>,
    plugins: Vec<(TrackId, SlotKey)>,
    full: Vec<(TrackId, soundcraft_model::ModuleId)>,
}

/// What is open now.
pub fn opened(app: &SoundApp) -> Opened {
    Opened { dialog: app.dialogs.open_name(), send: app.ui.send_window, plugins: app.ui.plugin_windows.clone(), full: app.ui.full_editors.clone() }
}

/// What the window with `viewport` opened while it drew (since `before`) is drawn over it from
/// now on: a dialog (rename, New Tracks…), the Send window, plugin windows.
pub fn claim(app: &mut SoundApp, before: &Opened, viewport: ViewportId) {
    let now = app.dialogs.open_name();
    let w = &mut app.window_sync;
    // A new dialog, or one that replaced another, belongs to the window that opened it.
    if now.is_some() && now != before.dialog {
        w.dialog_home = Some(viewport);
    }
    w.dialog_seen = now;
    if app.ui.send_window.is_some() && app.ui.send_window != before.send {
        w.send_home = Some(viewport);
    }
    for key in app.ui.plugin_windows.iter().filter(|k| !before.plugins.contains(k)) {
        w.plugin_homes.insert(*key, viewport);
    }
    // A full editor window shares its module's home (keyed as its plugin window).
    for (t, m) in app.ui.full_editors.iter().filter(|k| !before.full.contains(k)) {
        w.plugin_homes.insert((*t, SlotKey::Module(*m)), viewport);
    }
}

/// A dialog the main window opened (or replaced the open one with) since the Mix window last
/// claimed one belongs to the main window. Run before the main window draws its dialog.
pub fn settle(app: &mut SoundApp) {
    let now = app.dialogs.open_name();
    let w = &mut app.window_sync;
    if now != w.dialog_seen {
        w.dialog_home = None;
        w.dialog_seen = now;
    }
}

/// Forgets where closed things were drawn, and everything once Mix is back in the main window.
pub fn tidy(app: &mut SoundApp) {
    settle(app);
    let separate = app.ui.mix_separate;
    let w = &mut app.window_sync;
    if !separate || app.dialogs.open.is_none() {
        w.dialog_home = None;
    }
    if !separate || app.ui.send_window.is_none() {
        w.send_home = None;
    }
    let open = &app.ui.plugin_windows;
    let full = &app.ui.full_editors;
    w.plugin_homes.retain(|k, _| separate && (open.contains(k) || full.iter().any(|(t, m)| (*t, SlotKey::Module(*m)) == *k)));
    // A deleted track leaves the Mix window's own selection (undo brings it back unselected).
    let s = app.engine.session();
    w.mix_selection.retain(|t| s.track(*t).is_some());
}

/// The window whose keys `ctx`'s viewport reads: the separate Mix window's own, or the main
/// window's, which shows Edit while Mix is separate (whatever Window › Mix/Edit last chose).
pub fn window_of(app: &SoundApp, ctx: &egui::Context) -> MainWindow {
    if ctx.viewport_id() == mix_viewport() {
        MainWindow::Mix
    } else if app.ui.mix_separate {
        MainWindow::Edit
    } else {
        app.ui.window
    }
}

/// Whether something kept in `home` (none: the main window) is drawn in `ctx`'s viewport.
pub fn draws_here(home: Option<ViewportId>, ctx: &egui::Context) -> bool {
    home.unwrap_or(ViewportId::ROOT) == ctx.viewport_id()
}

/// Whether `window` keeps its own selection: the Mix window with Sync Selection off.
pub fn own_selection(app: &SoundApp, window: MainWindow) -> bool {
    window == MainWindow::Mix && !app.ui.sync.selection
}

/// The tracks selected in `window`: the session's selection, or with Sync Selection off the Mix
/// window's own. What the window's menus and shortcuts act on.
pub fn selection_of(app: &SoundApp, window: MainWindow) -> Vec<TrackId> {
    if own_selection(app, window) { app.window_sync.mix_selection.clone() } else { app.engine.session().edit.selected_tracks.clone() }
}

/// Whether `window` draws track `id` as selected.
pub fn shows_selected(app: &SoundApp, window: MainWindow, id: TrackId) -> bool {
    if own_selection(app, window) { app.window_sync.mix_selection.contains(&id) } else { app.engine.session().edit.selected_tracks.contains(&id) }
}

/// Window › Sync › Sync Selection switched: turned off, the Mix window starts from the selection
/// both windows shared, so its highlight doesn't vanish.
pub fn selection_sync_changed(app: &mut SoundApp) {
    if !app.ui.sync.selection {
        app.window_sync.mix_selection = app.engine.session().edit.selected_tracks.clone();
    }
}

/// Pro Tools' track-name click on `id` over rows `order`: Shift adds the rows up to the nearest
/// selected one, Cmd/Ctrl adds or removes one, a plain click selects only it.
pub fn click_selection(order: &[TrackId], current: &[TrackId], id: TrackId, mods: egui::Modifiers) -> Vec<TrackId> {
    let row = |t: &TrackId| order.iter().position(|o| o == t);
    let mut tracks = current.to_vec();
    if mods.command {
        if tracks.contains(&id) {
            tracks.retain(|t| *t != id);
        } else {
            tracks.push(id);
        }
    } else if mods.shift
        && let Some(a) = row(&id)
        && let Some(b) = tracks.iter().filter_map(row).min_by_key(|b| b.abs_diff(a))
    {
        let span = order.get(a.min(b)..=a.max(b)).unwrap_or_default();
        tracks.extend(span.iter().filter(|t| !tracks.contains(t)).copied().collect::<Vec<_>>());
    } else {
        tracks = vec![id];
    }
    tracks
}

/// A click on track `id` in `window` (a strip's name or a Tracks list row) over the rows `shown`
/// keeps: the session's selection, or with Sync Selection off and `window` Mix, the Mix window's own.
pub fn click_track(app: &mut SoundApp, window: MainWindow, id: TrackId, mods: egui::Modifiers, shown: impl Fn(&Session, &Track) -> bool) {
    if own_selection(app, window) {
        let s = app.engine.session();
        let order: Vec<TrackId> = s.tracks.iter().filter(|t| shown(s, t)).map(|t| t.id).collect();
        app.window_sync.mix_selection = click_selection(&order, &app.window_sync.mix_selection, id, mods);
    } else {
        crate::edit_window::click_track(app, id, mods, shown);
    }
}

/// A click on a Tracks list row of `window`, over the list's rows.
pub fn click_list_track(app: &mut SoundApp, window: MainWindow, id: TrackId, mods: egui::Modifiers) {
    click_track(app, window, id, mods, |s, t| !soundcraft_engine::edit::folder_collapsed(s, t));
}

/// The one track that joined the selection between `before` and `now`; none when several did
/// (Select All, a Shift range), so a big selection doesn't scroll anything away.
pub fn newly_selected(before: &[TrackId], now: &[TrackId]) -> Option<TrackId> {
    let mut added = now.iter().filter(|t| !before.contains(t));
    match (added.next(), added.next()) {
        (Some(id), None) => Some(*id),
        _ => None,
    }
}

/// Notices a change of the session's selection made in `origin` (none: a menu, a command from
/// outside): with Sync Selection and Scroll to Selected Track, a track that alone joined it is
/// scrolled into view in the other window (in both when the origin is unknown). Scrolling never
/// focuses a window.
pub fn watch_selection(app: &mut SoundApp, origin: Option<MainWindow>) {
    let now = &app.engine.session().edit.selected_tracks;
    if *now == app.window_sync.last_selection {
        return;
    }
    let added = newly_selected(&app.window_sync.last_selection, now);
    app.window_sync.last_selection = now.clone();
    // A reveal still waiting for its window is dropped once its track is no longer selected.
    let w = &mut app.window_sync;
    for pending in [&mut w.reveal_edit, &mut w.reveal_mix] {
        if pending.is_some_and(|t| !now.contains(&t)) {
            *pending = None;
        }
    }
    let o = app.ui.sync;
    let Some(id) = added.filter(|_| o.selection && o.scroll) else { return };
    if origin != Some(MainWindow::Mix) {
        app.window_sync.reveal_mix = Some(id);
    }
    if origin != Some(MainWindow::Edit) {
        app.window_sync.reveal_edit = Some(id);
    }
}

/// How many tracks a Tracks list drag of `id` moves: a closed folder takes its hidden members
/// along (they follow it in session order), anything else moves alone.
pub fn drag_block(s: &Session, id: TrackId) -> usize {
    let Some(folder) = s.track(id).filter(|t| t.is_folder() && !t.folder_open) else { return 1 };
    let Some(i) = s.track_index(folder.id) else { return 1 };
    let inside = |t: &Track| {
        let mut current = t.folder;
        for _ in 0..64 {
            match current {
                Some(f) if f == id => return true,
                Some(f) => current = s.track(f).and_then(|x| x.folder),
                None => return false,
            }
        }
        false
    };
    1 + s.tracks.iter().skip(i + 1).take_while(|t| inside(t)).count()
}

/// Where a Tracks list drag lands, for `track.move {to, count: block}`: the `block` tracks starting
/// at `id` (a closed folder and its members) dropped in the gap before visible row `gap_row`
/// (`rows.len()`: after the last). The list leaves a closed folder's members out, so the gap is
/// mapped back to the session: before the track of that row, which puts a drop just under a closed
/// folder after its last member. `None` when nothing moves (a drop on or right under the block).
pub fn list_drop(rows: &[TrackId], session: &[TrackId], id: TrackId, block: usize, gap_row: usize) -> Option<usize> {
    let from = session.iter().position(|t| *t == id)?;
    let gap = match rows.get(gap_row) {
        Some(next) => session.iter().position(|t| t == next)?,
        None => session.len(),
    };
    let block = block.max(1);
    if (from..=from.saturating_add(block)).contains(&gap) {
        return None;
    }
    Some(if gap > from { gap - block } else { gap })
}

/// The scroll that brings content `top..bottom` into a view `view_h` tall now scrolled to
/// `scroll`, moving as little as it can (none when it is already in view; its top wins when it is
/// taller than the view).
pub fn scroll_to_show(top: f32, bottom: f32, scroll: f32, view_h: f32) -> f32 {
    if top < scroll || bottom - top > view_h {
        top
    } else if bottom > scroll + view_h {
        bottom - view_h
    } else {
        scroll
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    /// Kick and Snare in a folder "Drums", Bass outside it.
    pub(crate) fn folder_engine() -> (soundcraft_engine::Engine, TrackId) {
        let mut e = soundcraft_engine::Engine::default();
        for name in ["Kick", "Snare", "Bass"] {
            e.execute("track.new", &json!({"count": 1, "format": "mono", "name": name})).unwrap();
        }
        let ids: Vec<u64> = e.session().tracks.iter().take(2).map(|t| t.id.0).collect();
        e.execute("edit.select", &json!({"tracks": ids})).unwrap();
        let r = e.execute("track.move_to_new_folder", &json!({"name": "Drums"})).unwrap();
        let folder = TrackId(r["folder"].as_u64().unwrap());
        (e, folder)
    }

    #[test]
    fn a_list_drop_under_a_closed_folder_lands_after_its_last_member() {
        // Session: Drums(Kick, Snare) Bass Keys; Drums closed, so the list shows Drums Bass Keys.
        let [drums, kick, snare, bass, keys] = [1, 2, 3, 4, 5].map(TrackId);
        let session = [drums, kick, snare, bass, keys];
        let rows = [drums, bass, keys];
        // Keys dropped in the gap under Drums (before Bass): after Snare, not between Kick and Snare.
        assert_eq!(list_drop(&rows, &session, keys, 1, 1), Some(3));
        // Bass dropped below Keys: at the end.
        assert_eq!(list_drop(&rows, &session, bass, 1, 3), Some(4));
        // Keys dropped onto its own gap: nothing moves.
        assert_eq!(list_drop(&rows, &session, keys, 1, 2), None);
        assert_eq!(list_drop(&rows, &session, keys, 1, 3), None);
        // Bass to the top.
        assert_eq!(list_drop(&rows, &session, bass, 1, 0), Some(0));
    }

    #[test]
    fn a_closed_folder_drags_as_a_block_with_its_members() {
        let (mut e, folder) = folder_engine();
        e.execute("track.folder_toggle", &json!({"track": folder.0})).unwrap();
        let block = drag_block(e.session(), folder);
        assert_eq!(block, 3, "the folder and its two members");
        let session: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        let rows: Vec<TrackId> = list_rows(e.session()).into_iter().map(|(t, _)| t).collect();
        assert_eq!(rows.len(), 2, "Drums and Bass");
        // Dropped in the gap right under itself: nothing moves.
        assert_eq!(list_drop(&rows, &session, folder, block, 1), None);
        assert_eq!(list_drop(&rows, &session, folder, block, 0), None);
        // Dropped below Bass: the whole block goes after it.
        let to = list_drop(&rows, &session, folder, block, 2).unwrap();
        e.execute("track.move", &json!({"track": folder.0, "to": to, "count": block})).unwrap();
        let names: Vec<String> = e.session().tracks.iter().map(|t| t.name.clone()).collect();
        let folder_name = e.session().track(folder).unwrap().name.clone();
        assert_eq!(names, vec!["Bass".to_string(), folder_name, "Kick".into(), "Snare".into()]);
        assert_eq!(drag_block(e.session(), session[3]), 1, "a plain track moves alone");
    }

    #[test]
    fn only_a_single_newly_selected_track_is_brought_into_view() {
        let (a, b, c) = (TrackId(1), TrackId(2), TrackId(3));
        assert_eq!(newly_selected(&[], &[a]), Some(a));
        assert_eq!(newly_selected(&[a], &[b]), Some(b));
        assert_eq!(newly_selected(&[a], &[a, c]), Some(c), "Cmd+click adds one");
        assert_eq!(newly_selected(&[a], &[a, b, c]), None, "a range or Select All scrolls nothing");
        assert_eq!(newly_selected(&[a, b], &[a]), None, "deselecting scrolls nothing");
    }

    #[test]
    fn scrolling_to_show_a_row_moves_as_little_as_it_can() {
        assert_eq!(scroll_to_show(100.0, 150.0, 0.0, 400.0), 0.0, "already in view");
        assert_eq!(scroll_to_show(500.0, 550.0, 0.0, 400.0), 150.0, "below: its bottom at the view's bottom");
        assert_eq!(scroll_to_show(100.0, 150.0, 300.0, 400.0), 100.0, "above: its top at the view's top");
        assert_eq!(scroll_to_show(100.0, 900.0, 0.0, 400.0), 100.0, "taller than the view: its top");
    }

    #[test]
    fn click_selection_follows_the_track_name_rules() {
        let order = [TrackId(1), TrackId(2), TrackId(3), TrackId(4)];
        let shift = egui::Modifiers { shift: true, ..Default::default() };
        let cmd = egui::Modifiers { command: true, ..Default::default() };
        assert_eq!(click_selection(&order, &[TrackId(1)], TrackId(3), egui::Modifiers::NONE), vec![TrackId(3)]);
        assert_eq!(click_selection(&order, &[TrackId(1)], TrackId(3), shift), vec![TrackId(1), TrackId(2), TrackId(3)]);
        assert_eq!(click_selection(&order, &[TrackId(1)], TrackId(1), cmd), Vec::<TrackId>::new());
    }

    #[test]
    fn a_closed_folder_hides_its_members_strips_but_not_its_own() {
        let (mut e, folder) = folder_engine();
        let o = SyncOptions::default();
        let strips = |e: &soundcraft_engine::Engine, o: SyncOptions| -> Vec<String> {
            let s = e.session();
            s.tracks.iter().filter(|t| mix_shows(s, t, o)).map(|t| t.name.clone()).collect()
        };
        assert_eq!(strips(&e, o).len(), 4);
        e.execute("track.folder_toggle", &json!({"track": folder.0})).unwrap();
        let folder_name = e.session().track(folder).unwrap().name.clone();
        assert_eq!(strips(&e, o), vec![folder_name, "Bass".to_string()]);
        assert_eq!(strips(&e, SyncOptions { folders: false, ..o }).len(), 4, "Follow Folder Collapse off");
        let bass = e.session().tracks.iter().find(|t| t.name == "Bass").unwrap().id;
        e.execute("track.hide", &json!({"tracks": [bass.0], "hidden": true})).unwrap();
        assert!(!strips(&e, o).contains(&"Bass".to_string()), "a hidden track has no strip");
        assert!(strips(&e, SyncOptions { visibility: false, ..o }).contains(&"Bass".to_string()), "Sync Track Visibility off");
    }

    #[test]
    fn the_list_nests_members_under_their_folder_and_closing_it_leaves_them_out() {
        let (mut e, folder) = folder_engine();
        let names = |e: &soundcraft_engine::Engine| -> Vec<(String, usize)> {
            let s = e.session();
            list_rows(s).into_iter().map(|(id, d)| (s.track(id).unwrap().name.clone(), d)).collect()
        };
        let folder_name = e.session().track(folder).unwrap().name.clone();
        assert_eq!(names(&e), vec![(folder_name.clone(), 0), ("Kick".into(), 1), ("Snare".into(), 1), ("Bass".into(), 0)]);
        e.execute("track.folder_toggle", &json!({"track": folder.0})).unwrap();
        assert_eq!(names(&e), vec![(folder_name, 0), ("Bass".into(), 0)], "a closed folder's members leave the list");
    }
}

//! The Edit and Mix windows in step (docs/superpowers/specs/2026-10-26-window-sync.md), above all
//! while Mix has its own native window.
//!
//! Two harnesses: kittest (it embeds the separate Mix window in the main one, so its widgets can
//! be found by label), and [`TwoWindows`], which runs the Mix window as a real second viewport the
//! way eframe does (an immediate viewport run from inside the main window's pass, with its own
//! input), for what depends on which window has the keyboard or draws a dialog.
use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::Queryable;
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_ui_egui::{MainWindow, Services, SoundApp, window_sync};

/// An app with Kick and Snare in the folder "Drums", Bass outside it, and the Tracks list shown.
fn folder_app() -> (SoundApp, TrackId) {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for name in ["Kick", "Snare", "Bass"] {
        app.run("track.new", json!({"count": 1, "format": "mono", "name": name})).unwrap();
    }
    let ids: Vec<u64> = app.engine.session().tracks.iter().take(2).map(|t| t.id.0).collect();
    app.run("edit.select", json!({"tracks": ids})).unwrap();
    let r = app.run("track.move_to_new_folder", json!({})).unwrap();
    let folder = TrackId(r["folder"].as_u64().unwrap());
    app.run("track.rename", json!({"track": folder.0, "name": "Drums"})).unwrap();
    app.run("edit.select_none", json!({})).unwrap();
    app.ui.show_tracks_list = true;
    (app, folder)
}

fn kittest(app: SoundApp) -> Harness<'static, SoundApp> {
    let mut h = Harness::builder().with_size(vec2(1600.0, 1000.0)).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(4);
    h
}

// ---- 1. Folders in the Mix window's track list ------------------------------------------------

#[test]
fn the_mix_track_list_shows_the_folder_tree_and_its_arrow_closes_the_folder_in_both_windows() {
    let (mut app, folder) = folder_app();
    app.run("window.mix", json!({})).unwrap();
    let mut h = kittest(app);
    assert_eq!(h.state().ui.window, MainWindow::Mix);
    assert!(h.query_by_label("Kick").is_some(), "the folder's members are listed");
    h.get_by_label("Close folder Drums").click();
    h.run_steps(3);
    assert!(!h.state().engine.session().track(folder).unwrap().folder_open, "the folder closed");
    assert!(h.query_by_label("Kick").is_none(), "its members left the list");
    assert!(h.query_by_label("Bass").is_some());
    // The Edit window shares the state: its list shows the folder closed too.
    h.state_mut().run("window.edit", json!({})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Open folder Drums").is_some());
    assert!(h.query_by_label("Kick").is_none());
}

// ---- 3. Closing a folder hides its members' strips in the Mix window --------------------------

#[test]
fn closing_a_folder_hides_its_members_strips_in_the_mix_window_embedded() {
    let (mut app, folder) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut h = kittest(app);
    assert!(h.query_by_label("Kick strip").is_some());
    h.state_mut().run("track.folder_toggle", json!({"track": folder.0, "open": false})).unwrap();
    h.run_steps(3);
    assert!(h.query_by_label("Kick strip").is_none(), "a closed folder's members have no strip");
    assert!(h.query_by_label("Snare strip").is_none());
    assert!(h.query_by_label("Drums strip").is_some(), "the folder's own strip stays");
    assert!(h.query_by_label("Bass strip").is_some());
    // Follow Folder Collapse off: the strips come back.
    h.state_mut().ui.sync.folders = false;
    h.run_steps(3);
    assert!(h.query_by_label("Kick strip").is_some());
}

// ---- 4. Selecting in one window brings the track into view in the other -----------------------

/// An app with tracks T1…T20 and the Mix window separate (no Tracks list, so names are unique).
/// Under kittest the separate Mix window is embedded in the main one as a floating window: these
/// tests check the strip filtering and scrolling, not which native window draws it.
fn many_tracks() -> SoundApp {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for i in 1..=20 {
        app.run("track.new", json!({"count": 1, "format": "mono", "name": format!("T{i}")})).unwrap();
    }
    app.run("edit.select_none", json!({})).unwrap();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    app.ui.show_tracks_list = false;
    app
}

fn track(app: &SoundApp, name: &str) -> TrackId {
    app.engine.session().tracks.iter().find(|t| t.name == name).map(|t| t.id).unwrap()
}

/// Scrolls the Edit window as far down as it goes.
fn scroll_edit_to_bottom(h: &mut Harness<'static, SoundApp>) {
    let l = &mut h.state_mut().edit_layout;
    let view = l.timeline[3] - l.timeline[1];
    l.scroll_by(100_000.0, view);
    h.run_steps(3);
}

/// Whether the Edit window drew track `id`'s row inside its timeline.
fn edit_row_in_view(app: &SoundApp, id: TrackId) -> bool {
    let tl = app.edit_layout.timeline;
    app.edit_layout.rows.iter().find(|(t, _)| *t == id.0).is_some_and(|(_, r)| r[1] >= tl[1] - 0.5 && r[3] <= tl[3] + 0.5)
}

#[test]
fn selecting_a_track_in_the_edit_window_scrolls_its_strip_into_view_in_the_mix_window_embedded() {
    let mut h = kittest(many_tracks());
    let far = h.get_by_label("T20 strip").rect();
    assert!(far.min.x > 1600.0, "T20's strip starts off screen ({far:?})");
    scroll_edit_to_bottom(&mut h);
    let scrolled = h.state().edit_layout.scroll_y;
    h.get_by_label("T20 name").click();
    h.run_steps(4);
    let id = track(h.state(), "T20");
    assert_eq!(h.state().engine.session().edit.selected_tracks, vec![id]);
    let now = h.get_by_label("T20 strip").rect();
    assert!(now.max.x <= 1600.0 && now.min.x >= 0.0, "the Mix window scrolled to T20 ({now:?})");
    assert_eq!(h.state().edit_layout.scroll_y, scrolled, "the Edit window, where the click was, stays put");
}

#[test]
fn selecting_a_strip_in_the_mix_window_embedded_scrolls_the_edit_window_to_its_track() {
    let mut h = kittest(many_tracks());
    scroll_edit_to_bottom(&mut h);
    let id = track(h.state(), "T1");
    assert!(!edit_row_in_view(h.state(), id), "T1 starts above the Edit window's view");
    h.get_by_label("T1 strip").click();
    h.run_steps(4);
    assert_eq!(h.state().engine.session().edit.selected_tracks, vec![id]);
    assert!(edit_row_in_view(h.state(), id), "the Edit window scrolled to T1");
}

#[test]
fn with_sync_selection_off_a_strip_menu_acts_on_the_mix_selection_not_the_edit_one() {
    let (mut app, _) = folder_app();
    let (kick, bass) = (track(&app, "Kick"), track(&app, "Bass"));
    app.run("edit.select", json!({"tracks": [bass.0]})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    app.ui.show_tracks_list = false;
    app.run("window.sync_selection", json!({"value": false})).unwrap();
    assert_eq!(app.window_sync.mix_selection, vec![bass], "turned off, the Mix window starts from the shared selection");
    let mut h = kittest(app);
    h.get_by_label("Kick strip").click();
    h.run_steps(3);
    assert_eq!(h.state().window_sync.mix_selection, vec![kick]);
    h.get_by_label("Kick strip").click_secondary();
    h.run_steps(3);
    h.get_by_label("Delete").click();
    h.run_steps(3);
    let s = h.state().engine.session();
    assert!(s.track(kick).is_none(), "the strip's track went");
    assert!(s.track(bass).is_some(), "the Edit selection's track stayed");
    assert_eq!(s.edit.selected_tracks, vec![bass]);
    assert!(h.state().window_sync.mix_selection.is_empty(), "the deleted track left the Mix selection");
}

#[test]
fn with_sync_selection_off_the_mix_window_keeps_its_own_selection() {
    let mut app = many_tracks();
    app.ui.sync.selection = false;
    let mut h = kittest(app);
    scroll_edit_to_bottom(&mut h);
    let scrolled = h.state().edit_layout.scroll_y;
    h.get_by_label("T1 strip").click();
    h.run_steps(3);
    let id = track(h.state(), "T1");
    assert!(h.state().engine.session().edit.selected_tracks.is_empty(), "the Edit selection is untouched");
    assert_eq!(h.state().window_sync.mix_selection, vec![id]);
    assert_eq!(h.state().edit_layout.scroll_y, scrolled, "nothing scrolls");
}

// ---- The two-viewport harness ------------------------------------------------------------------

/// What the separate Mix window gets next frame, and what it drew last.
#[derive(Default)]
struct MixFeed {
    events: Vec<egui::Event>,
    time: f64,
    focused: bool,
    /// Whether it ran this frame.
    ran: bool,
    /// Its widgets' labels and rects (from AccessKit).
    widgets: Vec<(String, egui::Rect)>,
}

/// The labelled widgets of a frame's AccessKit update.
fn widgets(out: &egui::FullOutput) -> Vec<(String, egui::Rect)> {
    let Some(update) = &out.platform_output.accesskit_update else { return vec![] };
    update
        .nodes
        .iter()
        .filter_map(|(_, n)| {
            let b = n.bounds()?;
            let label = n.label()?.to_string();
            Some((label, egui::Rect::from_min_max(egui::pos2(b.x0 as f32, b.y0 as f32), egui::pos2(b.x1 as f32, b.y1 as f32))))
        })
        .collect()
}

thread_local! {
    static MIX: std::cell::RefCell<MixFeed> = std::cell::RefCell::new(MixFeed::default());
}

const MIX_SIZE: egui::Vec2 = vec2(1100.0, 720.0);

/// The app with its Mix window as a real second viewport: eframe's way of running an immediate
/// viewport (from inside the main window's pass, with its own input), without a GPU.
struct TwoWindows {
    ctx: egui::Context,
    app: SoundApp,
    time: f64,
    /// The main window's widgets last frame.
    main: Vec<(String, egui::Rect)>,
}

impl TwoWindows {
    fn new(app: SoundApp) -> Self {
        egui::Context::set_immediate_viewport_renderer(|ctx, viewport| {
            let (events, time, focused) = MIX.with_borrow_mut(|m| (std::mem::take(&mut m.events), m.time, m.focused));
            let mut raw = egui::RawInput {
                viewport_id: viewport.ids.this,
                screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, MIX_SIZE)),
                time: Some(time),
                events,
                focused,
                ..Default::default()
            };
            raw.viewports.insert(viewport.ids.this, egui::ViewportInfo { focused: Some(focused), ..Default::default() });
            let mut ui_cb = viewport.viewport_ui_cb;
            let mut out = ctx.run_ui(raw, |ui| {
                ui_cb(ui);
                MIX.with_borrow_mut(|m| m.ran = true);
            });
            out.textures_delta.clear();
            MIX.with_borrow_mut(|m| m.widgets = widgets(&out));
        });
        let ctx = egui::Context::default();
        ctx.enable_accesskit();
        ctx.set_embed_viewports(false);
        let mut w = TwoWindows { ctx, app, time: 0.0, main: vec![] };
        for _ in 0..4 {
            w.frame(vec![], vec![]);
        }
        w
    }

    /// One frame of both windows; the window that gets events has the keyboard focus.
    fn frame(&mut self, main: Vec<egui::Event>, mix: Vec<egui::Event>) {
        self.time += 1.0 / 60.0;
        let mix_focused = !mix.is_empty();
        MIX.with_borrow_mut(|m| {
            m.events = mix;
            m.time = self.time;
            m.focused = mix_focused;
            m.ran = false;
        });
        let mut raw = egui::RawInput {
            viewport_id: egui::ViewportId::ROOT,
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, vec2(1600.0, 1000.0))),
            time: Some(self.time),
            events: main,
            focused: !mix_focused,
            ..Default::default()
        };
        raw.viewports.insert(egui::ViewportId::ROOT, egui::ViewportInfo { focused: Some(!mix_focused), ..Default::default() });
        let app = &mut self.app;
        let mut out = self.ctx.run_ui(raw, |ui| {
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        });
        out.textures_delta.clear();
        self.main = widgets(&out);
    }

    fn frames(&mut self, n: usize) {
        for _ in 0..n {
            self.frame(vec![], vec![]);
        }
    }

    /// Whether a widget labelled `label` is on screen in the main window / the Mix window.
    fn in_main(&self, label: &str) -> bool {
        self.main.iter().any(|(l, _)| l == label)
    }

    fn in_mix(&self, label: &str) -> bool {
        MIX.with_borrow(|m| m.widgets.iter().any(|(l, _)| l == label))
    }

    /// The rect of the Mix window's widget labelled `label`.
    fn mix_widget(&self, label: &str) -> egui::Rect {
        let found = MIX.with_borrow(|m| m.widgets.iter().find(|(l, _)| l == label).map(|(_, r)| *r));
        found.unwrap_or_else(|| panic!("no widget {label:?} in the Mix window"))
    }
}

fn key(key: egui::Key, modifiers: egui::Modifiers) -> Vec<egui::Event> {
    vec![
        egui::Event::Key { key, physical_key: None, pressed: true, repeat: false, modifiers },
        egui::Event::Key { key, physical_key: None, pressed: false, repeat: false, modifiers },
    ]
}

/// A click at `p`.
fn click(p: egui::Pos2) -> Vec<Vec<egui::Event>> {
    let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    vec![vec![egui::Event::PointerMoved(p)], vec![button(true)], vec![button(false)]]
}

/// A double click at `p`.
fn double_click(p: egui::Pos2) -> Vec<Vec<egui::Event>> {
    let button = |pressed| egui::Event::PointerButton { pos: p, button: egui::PointerButton::Primary, pressed, modifiers: egui::Modifiers::NONE };
    vec![vec![egui::Event::PointerMoved(p)], vec![button(true)], vec![button(false)], vec![button(true)], vec![button(false)]]
}

#[test]
fn the_harness_runs_the_mix_window_as_its_own_viewport() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    w.frames(1);
    assert!(MIX.with_borrow(|m| m.ran), "the Mix window drew in its own viewport");
}

// ---- 2. A dialog started from the Mix window opens over the Mix window -------------------------

#[test]
fn renaming_from_the_separate_mix_window_opens_the_dialog_over_the_mix_window() {
    let (mut app, _) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    let rename = "Rename Track";
    // The Mix window's Bass strip: its name plate is at the bottom of the strip.
    let bass = track(&w.app, "Bass");
    w.app.dialogs.open_rename_track(bass, "Bass");
    w.frames(2);
    assert!(w.in_main(rename) && !w.in_mix(rename), "a dialog the main window opened stays there");
    w.app.dialogs.open = None;
    w.frames(2);
    // Double-click the Bass strip's name in the Mix window.
    let plate = w.mix_widget("Bass strip").center();
    for events in double_click(plate) {
        w.frame(vec![], events);
    }
    w.frames(2);
    assert_eq!(w.app.dialogs.open_name(), Some("rename_track"));
    assert!(w.in_mix(rename), "the rename dialog is in the Mix window");
    assert!(!w.in_main(rename), "not over the Edit window");
    let dialog = w.mix_widget("Rename Track").center();
    assert!((dialog.x - MIX_SIZE.x / 2.0).abs() < 200.0, "centred on the Mix window ({dialog:?})");
    // Its keys are the Mix window's: type a name and press Enter there.
    w.frame(vec![], key(egui::Key::A, egui::Modifiers::COMMAND));
    w.frame(vec![], vec![egui::Event::Text("Low End".into())]);
    w.frame(vec![], key(egui::Key::Enter, egui::Modifiers::NONE));
    w.frames(2);
    assert!(w.app.dialogs.open.is_none(), "Enter in the Mix window confirmed it");
    assert_eq!(w.app.engine.session().track(bass).unwrap().name, "Low End");
}

#[test]
fn a_send_window_opened_from_the_mix_window_is_drawn_over_it_until_mix_rejoins_the_main_window() {
    let (mut app, _) = folder_app();
    let bass = track(&app, "Bass");
    app.run("mix.send", json!({"tracks": [bass.0], "bus": "Verb"})).unwrap();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    let title = "Bass · Send A";
    // Click Bass's Send A level in the Mix window.
    let level = w.mix_widget("Send A level").center();
    for events in click(level) {
        w.frame(vec![], events);
    }
    w.frames(3);
    assert_eq!(w.app.ui.send_window, Some((bass, 0)));
    assert!(w.in_mix(title) && !w.in_main(title), "over the Mix window");
    // Mix back in the main window: so is the Send window.
    w.app.run("window.mix_separate", json!({"value": false})).unwrap();
    w.frames(3);
    assert!(w.in_main(title), "back in the main window");
}

#[test]
fn a_rename_dialog_keeps_its_focus_and_text_when_mix_rejoins_the_main_window() {
    let (mut app, _) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    let bass = track(&w.app, "Bass");
    for events in double_click(w.mix_widget("Bass strip").center()) {
        w.frame(vec![], events);
    }
    w.frames(2);
    assert!(w.in_mix("Rename Track"));
    w.frame(vec![], vec![egui::Event::Text("Lo".into())]);
    // The Mix window's close box: Mix goes back into the main window, and the dialog with it.
    soundcraft_ui_egui::mix_window::separate_closed(&mut w.app);
    w.frames(3);
    assert!(w.in_main("Rename Track"), "the dialog moved to the main window");
    w.frame(vec![egui::Event::Text("w".into())], vec![]);
    w.frame(key(egui::Key::Enter, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert_eq!(w.app.engine.session().track(bass).unwrap().name, "Low", "typing went on where it was");
}

// ---- 5. Shortcuts work from whichever window has the keyboard ----------------------------------

/// An app with the Mix window separate, after Window › Mix (so `ui.window` is Mix, as the owner
/// leaves it).
fn separate_after_window_mix() -> TwoWindows {
    let (mut app, _) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    TwoWindows::new(app)
}

#[test]
fn space_starts_and_stops_playback_from_the_edit_window_with_mix_separate() {
    let mut w = separate_after_window_mix();
    assert!(!w.app.is_playing());
    w.frame(key(egui::Key::Space, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert!(w.app.is_playing(), "Space in the Edit window plays");
    w.frame(key(egui::Key::Space, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert!(!w.app.is_playing(), "and stops");
}

#[test]
fn space_and_undo_work_from_the_separate_mix_window() {
    let mut w = separate_after_window_mix();
    w.frame(vec![], key(egui::Key::Space, egui::Modifiers::NONE));
    w.frames(2);
    assert!(w.app.is_playing(), "Space in the Mix window plays");
    w.frame(vec![], key(egui::Key::Space, egui::Modifiers::NONE));
    w.frames(2);
    assert!(!w.app.is_playing());
    let bass = track(&w.app, "Bass");
    w.app.run("track.rename", json!({"track": bass.0, "name": "Sub"})).unwrap();
    w.frame(vec![], key(egui::Key::Z, egui::Modifiers::COMMAND));
    w.frames(2);
    assert_eq!(w.app.engine.session().track(bass).unwrap().name, "Bass", "Cmd/Ctrl+Z in the Mix window undoes");
}

#[test]
fn a_dialog_takes_the_keys_of_its_own_window_and_the_other_keeps_its_shortcuts() {
    let mut w = separate_after_window_mix();
    let bass = track(&w.app, "Bass");
    // Opened by the main window: it lives there.
    w.app.dialogs.open_rename_track(bass, "Bass");
    w.frames(2);
    w.frame(key(egui::Key::Space, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert!(!w.app.is_playing(), "Space in the dialog's window types, it doesn't play");
    w.frame(vec![], key(egui::Key::Space, egui::Modifiers::NONE));
    w.frames(2);
    assert!(w.app.is_playing(), "Space in the other window still plays");
    assert!(w.app.dialogs.open.is_some(), "and leaves the dialog open");
}

#[test]
fn edit_window_shortcuts_stay_in_the_edit_window() {
    let mut w = separate_after_window_mix();
    let grid = |w: &TwoWindows| w.app.engine.session().edit.grid;
    let before = grid(&w);
    w.frame(vec![], key(egui::Key::Equals, egui::Modifiers::SHIFT));
    w.frames(2);
    assert_eq!(grid(&w), before, "Shift+= (Grid value) does nothing in the Mix window");
    w.frame(key(egui::Key::Equals, egui::Modifiers::SHIFT), vec![]);
    w.frames(2);
    assert_ne!(grid(&w), before, "it steps the Grid value in the Edit window, even after Window › Mix");
}

#[test]
fn shortcuts_work_with_mix_docked_in_the_main_window_too() {
    let (mut app, _) = folder_app();
    app.run("window.mix", json!({})).unwrap();
    let mut w = TwoWindows::new(app);
    w.frame(key(egui::Key::Space, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert!(w.app.is_playing(), "Space plays from the docked Mix window");
    w.frame(key(egui::Key::Space, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert!(!w.app.is_playing());
}

// ---- Window › Sync ------------------------------------------------------------------------------

#[test]
fn the_sync_options_toggle_are_checked_and_saved_in_ui_json() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    assert_eq!(app.ui.sync, window_sync::SyncOptions { selection: true, scroll: true, folders: true, visibility: true }, "all on");
    for id in ["window.sync_selection", "window.sync_scroll", "window.sync_folders", "window.sync_visibility"] {
        assert!(soundcraft_ui_egui::menus::is_toggle(&app, "", Some(id)), "{id} shows a check mark");
        assert_eq!(app.run(id, json!({})).unwrap(), json!({"value": false}));
    }
    assert_eq!(app.ui.sync, window_sync::SyncOptions { selection: false, scroll: false, folders: false, visibility: false });
    let saved = serde_json::to_value(&app.ui).unwrap();
    assert_eq!(saved["sync"], json!({"selection": false, "scroll": false, "folders": false, "visibility": false}));
    // An older ui.json without it loads with every option on.
    let mut old = saved.clone();
    old.as_object_mut().unwrap().remove("sync");
    let ui: soundcraft_ui_egui::UiState = serde_json::from_value(old).unwrap();
    assert_eq!(ui.sync, window_sync::SyncOptions::default());
}

#[test]
fn with_sync_selection_off_cut_typed_into_the_mix_window_does_not_cut_on_the_mix_selection() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    for name in ["Pad", "Lead"] {
        app.run("track.new", json!({"count": 1, "format": "mono", "kind": "midi", "name": name})).unwrap();
    }
    let (pad, lead) = (track(&app, "Pad"), track(&app, "Lead"));
    for t in [pad, lead] {
        app.run("midi.new_clip", json!({"track": t.0, "start": 0, "end": 48_000})).unwrap();
    }
    // The Edit window's selection: Lead over the first second. The Mix window's own: Pad.
    app.run("edit.select", json!({"tracks": [lead.0], "start": 0, "end": 48_000})).unwrap();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    app.run("window.sync_selection", json!({"value": false})).unwrap();
    app.window_sync.mix_selection = vec![pad];
    let mut w = TwoWindows::new(app);
    let clips = |w: &TwoWindows, t: TrackId| w.app.engine.session().track(t).unwrap().clips().len();
    assert_eq!(clips(&w, pad), 1);
    w.frame(vec![], key(egui::Key::X, egui::Modifiers::COMMAND));
    w.frames(2);
    assert_eq!(clips(&w, pad), 1, "Cut keeps the Edit window's range and tracks: Pad's clip stays");
}

#[test]
fn a_dialog_shortcut_in_the_main_window_leaves_the_mix_windows_dialog_and_its_text_alone() {
    let (mut app, _) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    let bass = track(&w.app, "Bass");
    w.app.run("edit.select", json!({"tracks": [bass.0]})).unwrap();
    for events in double_click(w.mix_widget("Bass strip").center()) {
        w.frame(vec![], events);
    }
    w.frames(2);
    w.frame(vec![], vec![egui::Event::Text("Lo".into())]);
    // Cmd/Ctrl+G (Group…) in the Edit window would open its own dialog over the rename.
    w.frame(key(egui::Key::G, egui::Modifiers::COMMAND), vec![]);
    w.frames(2);
    assert_eq!(w.app.dialogs.open_name(), Some("rename_track"), "the rename is still open");
    assert!(w.in_mix("Rename Track") && !w.in_main("Create Group"));
    w.frame(vec![], key(egui::Key::Enter, egui::Modifiers::NONE));
    w.frames(2);
    assert_eq!(w.app.engine.session().track(bass).unwrap().name, "Lo", "what was typed survived");
}

#[test]
fn a_dialog_replaced_from_the_main_window_moves_to_the_main_window() {
    let (mut app, _) = folder_app();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    let mut w = TwoWindows::new(app);
    for events in double_click(w.mix_widget("Bass strip").center()) {
        w.frame(vec![], events);
    }
    w.frames(2);
    assert!(w.in_mix("Rename Track"));
    // A menu of the main window replaces it with New Tracks.
    let _ = w.app.dialogs.open_for_command(&w.app.engine, "track.new");
    w.frames(3);
    assert!(w.in_main("New Tracks") && !w.in_mix("New Tracks"), "homed where it was opened");
}

#[test]
fn delete_clears_the_edit_selection_from_the_edit_window_only() {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("track.new", json!({"count": 1, "format": "mono", "kind": "midi", "name": "Pad"})).unwrap();
    let pad = track(&app, "Pad");
    app.run("midi.new_clip", json!({"track": pad.0, "start": 0, "end": 48_000})).unwrap();
    app.run("edit.select", json!({"tracks": [pad.0], "start": 0, "end": 48_000})).unwrap();
    app.run("window.mix_separate", json!({"value": true})).unwrap();
    app.run("window.mix", json!({})).unwrap();
    let mut w = TwoWindows::new(app);
    let clips = |w: &TwoWindows| w.app.engine.session().track(pad).unwrap().clips().len();
    w.frame(vec![], key(egui::Key::Delete, egui::Modifiers::NONE));
    w.frames(2);
    assert_eq!(clips(&w), 1, "Delete typed into the Mix window clears nothing");
    w.frame(key(egui::Key::Delete, egui::Modifiers::NONE), vec![]);
    w.frames(2);
    assert_eq!(clips(&w), 0, "in the Edit window it clears the selection, even after Window › Mix");
}

//! Hosted plugins open their own editors first (instrument slot and insert tile alike), the generic
//! parameter window is one right-click away ("Show Parameters…"), and "Capture Thumbnail Now"
//! takes an open editor's picture for the browser at once. Instruments wait longer than effects
//! before their picture is taken on its own.

use egui::vec2;
use egui_kittest::Harness;
use egui_kittest::kittest::{NodeT, Queryable};
use serde_json::json;
use soundcraft_model::TrackId;
use soundcraft_playback::SlotKey;
use soundcraft_ui_egui::browser::catalog::{BrowserCategory, BrowserItem, Catalog, Format, Kind};
use soundcraft_ui_egui::browser::thumbs::{
    CAPTURE_TRIES, RESTORE_AND_RETRY, capture_delay, capture_now, capture_settled, refresh_pending, refresh_thumbnail, thumb_path,
};
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

const KEYS: &str = "vst3:fake.keys";
const COMP: &str = "vst3:fake.comp";

fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("soundcraft-editor-thumbs-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    d
}

fn solid(w: u32, h: u32, rgb: [u8; 3]) -> Vec<u8> {
    (0..w * h).flat_map(|_| [rgb[0], rgb[1], rgb[2], 255]).collect()
}

fn gradient(w: u32, h: u32) -> Vec<u8> {
    (0..h).flat_map(|y| (0..w).flat_map(move |x| [(x * 255 / w.max(1)) as u8, (y * 255 / h.max(1)) as u8, 90, 255])).collect()
}

/// A hosted editor stand-in: opens and closes, and captures the picture it is given (none for
/// the first `misses` attempts: not drawn yet, or minimised). `fail_open`: opening it fails.
struct FakeEditor {
    open: bool,
    picture: Arc<std::sync::Mutex<(u32, u32, Vec<u8>)>>,
    captures: Arc<AtomicUsize>,
    misses: usize,
    fail_open: bool,
}

impl soundcraft_dsp::PluginEditor for FakeEditor {
    fn open(&mut self) -> Result<(), String> {
        if self.fail_open {
            return Err("the host refused the window".into());
        }
        self.open = true;
        Ok(())
    }
    fn close(&mut self) {
        self.open = false;
    }
    fn is_open(&self) -> bool {
        self.open
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
    fn capture(&mut self) -> Option<(u32, u32, Vec<u8>)> {
        let n = self.captures.fetch_add(1, Ordering::Relaxed);
        if n < self.misses {
            return None;
        }
        self.picture.lock().ok().map(|p| p.clone())
    }
}

type Picture = Arc<std::sync::Mutex<(u32, u32, Vec<u8>)>>;

/// One fake editor's handles: what it captures next, and how many captures it took.
struct Fake {
    picture: Picture,
    captures: Arc<AtomicUsize>,
    misses: usize,
    fail_open: bool,
}

impl Fake {
    fn new(w: u32, h: u32) -> Fake {
        Fake {
            picture: Arc::new(std::sync::Mutex::new((w, h, gradient(w, h)))),
            captures: Arc::new(AtomicUsize::new(0)),
            misses: 0,
            fail_open: false,
        }
    }
    fn editor(&self) -> Box<FakeEditor> {
        Box::new(FakeEditor {
            open: false,
            picture: self.picture.clone(),
            captures: self.captures.clone(),
            misses: self.misses,
            fail_open: self.fail_open,
        })
    }
    fn count(&self) -> usize {
        self.captures.load(Ordering::Relaxed)
    }
}

fn item(id: &str, name: &str, kind: Kind) -> BrowserItem {
    BrowserItem {
        id: id.into(),
        name: name.into(),
        vendor: "Someone".into(),
        version: "1".into(),
        format: Format::Vst3,
        kind,
        category: BrowserCategory::Other,
    }
}

struct Rig {
    h: Harness<'static, SoundApp>,
    keys: TrackId,
    comp: (TrackId, SlotKey),
    keys_item: BrowserItem,
    comp_item: BrowserItem,
}

/// An instrument track "Keys" playing a hosted instrument and an audio track "Vox" with a hosted
/// insert, the Mix window showing the instrument row; `editors` attaches fake editors to both.
/// Pictures are stored in `dir`.
fn rig(dir: &Path, editors: Option<(&Fake, &Fake)>) -> Rig {
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"kind": "instrument", "name": "Keys"})).unwrap();
    engine.execute("track.new", &json!({"name": "Vox"})).unwrap();
    let keys = engine.session().track_by_name("Keys").unwrap().id;
    let vox = engine.session().track_by_name("Vox").unwrap().id;
    // Ids no host can parse: nothing is scanned or loaded for them; they go into the session
    // directly, as a session saved with them would have them.
    engine.session_mut().track_mut(keys).unwrap().instrument = Some(soundcraft_model::Insert::new(KEYS));
    let m = engine.session_mut().track_mut(vox).unwrap().mixer.add_module(soundcraft_model::Insert::new(COMP), None).unwrap();
    let comp = (vox, SlotKey::Module(m));
    let player = soundcraft_playback::Player::new(engine.session_arc());
    if let Some((k, c)) = editors {
        player.attach_editor(keys, SlotKey::Instrument, k.editor());
        player.attach_editor(comp.0, comp.1, c.editor());
    }
    let mut app = SoundApp::new(engine, Some(player), Services { thumb_dir: Some(dir.to_path_buf()), ..Services::default() });
    let keys_item = item(KEYS, "Fake Keys", Kind::Instrument);
    let comp_item = item(COMP, "Fake Comp", Kind::Effect);
    app.browser_catalog = Some(Catalog { items: vec![keys_item.clone(), comp_item.clone()], failures: Vec::new() });
    app.run("window.mix", json!({})).unwrap();
    app.run("view.mix_section", json!({"section": "minimal"})).unwrap();
    app.run("view.mix_section", json!({"section": "instruments"})).unwrap();
    app.run("view.mix_section", json!({"section": "inserts_ae"})).unwrap();
    let mut builder = Harness::builder().with_size(vec2(1400.0, 900.0)).with_step_dt(0.25);
    if proof_dir().is_some() {
        builder = builder.wgpu();
    }
    let mut h = builder.build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(3);
    Rig { h, keys, comp, keys_item, comp_item }
}

/// Where to save proof renders (`SOUNDCRAFT_THUMBS_PROOF_DIR`), if anywhere.
fn proof_dir() -> Option<PathBuf> {
    std::env::var_os("SOUNDCRAFT_THUMBS_PROOF_DIR").map(PathBuf::from)
}

fn proof(h: &mut Harness<SoundApp>, name: &str) {
    if let Some(dir) = proof_dir() {
        std::fs::create_dir_all(&dir).unwrap();
        h.render().unwrap().save(dir.join(format!("{name}.png"))).unwrap();
    }
}

fn editor_open(h: &Harness<SoundApp>, t: TrackId, k: SlotKey) -> bool {
    h.state().player.as_ref().unwrap().editor_open(t, k)
}

#[test]
fn clicking_a_hosted_instrument_slot_opens_its_own_editor() {
    let dir = temp_dir("click");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    r.h.get_by_label("Instrument").click();
    r.h.run_steps(2);
    assert!(editor_open(&r.h, r.keys, SlotKey::Instrument), "the plugin's own editor opened");
    assert!(r.h.state().ui.plugin_windows.is_empty(), "not the generic window");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_instrument_without_an_editor_opens_the_generic_window() {
    let dir = temp_dir("noeditor");
    let mut r = rig(&dir, None);
    r.h.get_by_label("Instrument").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().ui.plugin_windows, vec![(r.keys, SlotKey::Instrument)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn show_parameters_opens_the_generic_window() {
    let dir = temp_dir("params");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    r.h.get_by_label("Instrument").click_secondary();
    r.h.run_steps(2);
    r.h.get_by_label("Show Parameters…").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().ui.plugin_windows, vec![(r.keys, SlotKey::Instrument)]);
    assert!(!editor_open(&r.h, r.keys, SlotKey::Instrument), "the generic window, not the editor");
    // The window offers the capture too, enabled now that the editor is open.
    r.h.state().player.as_ref().unwrap().open_editor(r.keys, SlotKey::Instrument).unwrap();
    r.h.run_steps(2);
    proof(&mut r.h, "generic-window-capture-button");
    r.h.get_by_label("Capture Thumbnail Now").click();
    r.h.run_steps(1);
    assert_eq!(k.count(), 1);
    assert_eq!(r.h.state().ui.status, "Captured Fake Keys thumbnail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_now_is_enabled_only_while_the_editor_is_open() {
    let dir = temp_dir("enabled");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    r.h.get_by_label("Instrument").click_secondary();
    r.h.run_steps(2);
    assert!(r.h.get_by_label("Capture Thumbnail Now").accesskit_node().is_disabled(), "the editor is closed");
    r.h.key_press(egui::Key::Escape);
    r.h.run_steps(2);
    r.h.state().player.as_ref().unwrap().open_editor(r.keys, SlotKey::Instrument).unwrap();
    r.h.run_steps(1);
    r.h.get_by_label("Instrument").click_secondary();
    r.h.run_steps(2);
    proof(&mut r.h, "instrument-menu-editor-open");
    let item = r.h.get_by_label("Capture Thumbnail Now");
    assert!(!item.accesskit_node().is_disabled(), "the editor is open");
    item.click();
    r.h.run_steps(1);
    assert_eq!(k.count(), 1, "taken at once, no delay");
    assert_eq!(c.count(), 0, "only that plugin");
    assert!(thumb_path(&dir, &r.keys_item).exists());
    assert!(!thumb_path(&dir, &r.comp_item).exists());
    assert_eq!(r.h.state().ui.status, "Captured Fake Keys thumbnail");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_now_replaces_the_stored_picture_and_a_blank_one_keeps_it() {
    let dir = temp_dir("replace");
    let (k, c) = (Fake::new(300, 200), Fake::new(320, 180));
    let mut r = rig(&dir, Some((&k, &c)));
    // A fresh stored picture, which an automatic capture would keep.
    let path = thumb_path(&dir, &r.comp_item);
    std::fs::write(&path, soundcraft_ui_egui::browser::thumbs::builtin_thumbnail("eq_7band").unwrap()).unwrap();
    let (t, slot) = r.comp;
    r.h.state().player.as_ref().unwrap().open_editor(t, slot).unwrap();
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 1);
    let img = image::open(&path).unwrap();
    assert_eq!((img.width(), img.height()), (320, 180), "replaced by the editor's picture");
    assert_eq!(r.h.state().ui.status, "Captured Fake Comp thumbnail");
    let ctx = r.h.ctx.clone();
    let tex = r.h.state_mut().browser_thumbs.texture(&ctx, &r.comp_item).expect("the card shows it");
    assert_eq!(tex.size(), [320, 180]);
    // The automatic capture of this opening is not taken on top of it, even once the stored
    // picture is stale (so only the opening's own state holds it back).
    assert_eq!(capture_settled(r.h.state(), (t, slot)), Some(true));
    let stale = std::time::SystemTime::now() - std::time::Duration::from_secs(60 * 24 * 60 * 60);
    std::fs::File::options().write(true).open(&path).unwrap().set_modified(stale).unwrap();
    r.h.run_steps(12);
    assert_eq!(c.count(), 1);
    // A blank capture says so and keeps the picture.
    *c.picture.lock().unwrap() = (320, 180, solid(320, 180, [10, 10, 10]));
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 2);
    assert!(r.h.state().ui.status.contains("blank"), "{}", r.h.state().ui.status);
    let img = image::open(&path).unwrap();
    assert_eq!((img.width(), img.height()), (320, 180), "the old picture is kept");
    assert!(!img.to_rgba8().pixels().all(|p| p.0 == [10, 10, 10, 255]));
    // A closed editor is not captured.
    r.h.state().player.as_ref().unwrap().close_editor(t, slot);
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 2);
    assert_eq!(k.count(), 0, "the other plugin was never touched");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn instruments_wait_longer_before_their_picture_is_taken() {
    // The spec's values: 10 s for instruments, 1.5 s for effects.
    assert_eq!(capture_delay(SlotKey::Instrument), 10.0);
    assert_eq!(capture_delay(SlotKey::Module(soundcraft_model::ModuleId(1))), 1.5);
    let dir = temp_dir("delay");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    let p = r.h.state().player.as_ref().unwrap();
    p.open_editor(r.keys, SlotKey::Instrument).unwrap();
    p.open_editor(r.comp.0, r.comp.1).unwrap();
    // 0.25 s a step: the effect after 1.5 s, the instrument only after 10 s.
    r.h.run_steps(8);
    assert_eq!((k.count(), c.count()), (0, 1));
    r.h.run_steps(30);
    assert_eq!(k.count(), 0, "still loading, maybe");
    r.h.run_steps(4);
    assert_eq!(k.count(), 1);
    assert!(thumb_path(&dir, &r.keys_item).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refresh_thumbnail_explains_it_waits_for_the_plugin_to_load() {
    let help = soundcraft_ui_egui::browser::card::REFRESH_HELP;
    assert!(help.contains("once it has loaded"), "{help}");
    assert!(help.contains("Capture Thumbnail Now"), "{help}");
    assert!(!help.contains("  ") && !help.contains('\n'), "one line of prose: {help:?}");
}

#[test]
fn capture_now_finishes_a_refresh_and_leaves_its_editor_open() {
    let dir = temp_dir("refresh");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    let (t, slot) = r.comp;
    r.h.state_mut().engine.session_mut().edit.selected_tracks = vec![t];
    refresh_thumbnail(r.h.state_mut(), &r.comp_item);
    assert!(editor_open(&r.h, t, slot), "the refresh opened the editor");
    assert!(refresh_pending(r.h.state()));
    r.h.run_steps(1);
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 1);
    assert!(!refresh_pending(r.h.state()), "the refresh has its picture");
    assert!(r.h.state().ui.status.starts_with("Captured Fake Comp thumbnail"), "{}", r.h.state().ui.status);
    assert!(r.h.state().ui.status.contains("stays open"), "{}", r.h.state().ui.status);
    r.h.run_steps(12);
    assert!(editor_open(&r.h, t, slot), "not closed under the user");
    assert_eq!(c.count(), 1, "and not captured again");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn capture_now_while_the_automatic_capture_is_retrying() {
    let dir = temp_dir("midretry");
    let k = Fake::new(300, 200);
    // No picture for the first two attempts (a minimised or undrawn window).
    let c = Fake { misses: 2, ..Fake::new(300, 200) };
    let mut r = rig(&dir, Some((&k, &c)));
    let (t, slot) = r.comp;
    r.h.state().player.as_ref().unwrap().open_editor(t, slot).unwrap();
    r.h.run_steps(8);
    assert_eq!(c.count(), 1, "the automatic attempt missed");
    assert_eq!(capture_settled(r.h.state(), (t, slot)), Some(false), "retrying");
    // Capture Now misses too: it says what to do, and the retries go on.
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 2);
    assert_eq!(r.h.state().ui.status, format!("Fake Comp: {RESTORE_AND_RETRY}"));
    assert_eq!(capture_settled(r.h.state(), (t, slot)), Some(false));
    // Again, now that the editor draws: taken, and the retries stop.
    capture_now(r.h.state_mut(), (t, slot));
    assert_eq!(c.count(), 3);
    assert_eq!(r.h.state().ui.status, "Captured Fake Comp thumbnail");
    assert_eq!(capture_settled(r.h.state(), (t, slot)), Some(true));
    r.h.run_steps(8 * CAPTURE_TRIES as usize);
    assert_eq!(c.count(), 3, "no retry after the picture was taken");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_undescribed_hosted_insert_tile_opens_and_offers_the_hosted_items() {
    let dir = temp_dir("tile");
    let (k, c) = (Fake::new(300, 200), Fake::new(300, 200));
    let mut r = rig(&dir, Some((&k, &c)));
    let (t, slot) = r.comp;
    // A fresh stored picture: the automatic capture keeps it, so every capture here is ours.
    std::fs::write(thumb_path(&dir, &r.comp_item), soundcraft_ui_egui::browser::thumbs::builtin_thumbnail("eq_7band").unwrap()).unwrap();
    r.h.get_by_label("comp module").click();
    r.h.run_steps(2);
    assert!(editor_open(&r.h, t, slot), "the tile opens the plugin's own editor");
    r.h.get_by_label("comp module").click_secondary();
    r.h.run_steps(2);
    assert!(r.h.query_by_label("Open Plugin…").is_some());
    let item = r.h.get_by_label("Capture Thumbnail Now");
    assert!(!item.accesskit_node().is_disabled());
    item.click();
    r.h.run_steps(1);
    assert_eq!(c.count(), 1);
    assert_eq!(r.h.state().ui.status, "Captured Fake Comp thumbnail");
    r.h.get_by_label("comp module").click_secondary();
    r.h.run_steps(2);
    r.h.get_by_label("Show Parameters…").click();
    r.h.run_steps(2);
    assert_eq!(r.h.state().ui.plugin_windows, vec![(t, slot)]);
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn an_editor_that_fails_to_open_falls_back_to_the_generic_window() {
    let dir = temp_dir("failopen");
    let k = Fake { fail_open: true, ..Fake::new(300, 200) };
    let c = Fake::new(300, 200);
    let mut r = rig(&dir, Some((&k, &c)));
    r.h.get_by_label("Instrument").click();
    r.h.run_steps(2);
    assert!(!editor_open(&r.h, r.keys, SlotKey::Instrument));
    assert_eq!(r.h.state().ui.plugin_windows, vec![(r.keys, SlotKey::Instrument)]);
    assert!(r.h.state().ui.status.contains("refused"), "{}", r.h.state().ui.status);
    let _ = std::fs::remove_dir_all(&dir);
}

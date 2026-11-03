//! The plugin browser's thumbnails: the built-in and FireStation pictures (pre-rendered by
//! `cargo xtask browser-thumbs`) and the cache that turns them into textures for visible cards.
mod editor_support;

use editor_support::app_with_tracks;
use egui::vec2;
use egui_kittest::Harness;
use serde_json::json;
use soundcraft_ui_egui::SoundApp;
use soundcraft_ui_egui::browser::BrowserTab;
use soundcraft_ui_egui::browser::catalog::{BrowserCategory, BrowserItem, Format, Kind, build_catalog};
use soundcraft_ui_egui::browser::thumbs::{MAX_WIDTH, ThumbCache, builtin_thumbnail};

fn item(id: &str, format: Format) -> BrowserItem {
    BrowserItem {
        id: id.into(),
        name: id.into(),
        vendor: "Someone".into(),
        version: "1".into(),
        format,
        kind: Kind::Effect,
        category: BrowserCategory::Other,
    }
}

#[test]
fn every_builtin_has_a_thumbnail() {
    let mut total = 0;
    for info in soundcraft_dsp::plugins() {
        let bytes = builtin_thumbnail(info.id).unwrap_or_else(|| panic!("{}: no thumbnail", info.id));
        total += bytes.len();
        let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Png).unwrap_or_else(|e| panic!("{}: {e}", info.id));
        assert!(img.width() <= MAX_WIDTH && img.width() >= 100, "{}: {} px wide", info.id, img.width());
        assert!(img.height() >= 60, "{}: {} px tall", info.id, img.height());
    }
    assert!(total <= 3 * 1024 * 1024, "thumbnails take {total} bytes");
    assert!(builtin_thumbnail("vst3:nothing").is_none());
}

#[test]
fn cache_loads_only_requested_items() {
    let ctx = egui::Context::default();
    let mut cache = ThumbCache::default();
    assert_eq!(cache.loaded(), 0);
    let comp = item("fs_comp", Format::FireStation);
    let tex = cache.texture(&ctx, &comp).expect("Comp has a picture");
    assert!(tex.size()[0] as u32 <= MAX_WIDTH);
    assert_eq!(cache.loaded(), 1);
    // Asked again: the same texture, not a second decode.
    assert_eq!(cache.texture(&ctx, &comp).unwrap().id(), tex.id());
    assert_eq!(cache.loaded(), 1);
    // A hosted plugin has no picture until one is handed to the cache (captured or from disk).
    let hosted = item("vst3:abc", Format::Vst3);
    assert!(cache.texture(&ctx, &hosted).is_none());
    assert_eq!(cache.loaded(), 1);
    cache.set_png("vst3:abc", builtin_thumbnail("eq_7band").unwrap().to_vec());
    assert!(cache.texture(&ctx, &hosted).is_some());
    assert_eq!(cache.loaded(), 2);
    // Bytes that are not a PNG keep the placeholder.
    cache.set_png("vst3:abc", b"not a png".to_vec());
    assert!(cache.texture(&ctx, &hosted).is_none());
    cache.forget("fs_comp");
    assert_eq!(cache.loaded(), 0);
}

#[test]
fn panel_loads_pictures_for_visible_cards_only() {
    let mut app = app_with_tracks(&["Voice"]);
    app.browser_catalog = Some(build_catalog(soundcraft_dsp::plugins(), &[], &[], &[], Vec::new()));
    app.ui.browser.tab = BrowserTab::Effects;
    app.run("ui.set", json!({"theme": "dark"})).unwrap();
    app.run("window.plugin_browser", json!({})).unwrap();
    // A short window: only a few rows of effects are in view.
    let mut h = Harness::builder().with_size(vec2(1200.0, 700.0)).with_step_dt(1.0 / 60.0).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(5);
    let built = h.state().browser_debug.cards_built;
    let loaded = h.state().browser_thumbs.loaded();
    let effects = soundcraft_dsp::plugins().iter().filter(|p| p.category != soundcraft_dsp::Category::Instrument).count();
    assert!(built > 0 && built < effects, "{built} of {effects} cards built");
    assert!(loaded > 0 && loaded <= built, "{loaded} pictures for {built} cards");
}

use soundcraft_ui_egui::Services;
use soundcraft_ui_egui::browser::thumbs::{CAPTURE_TRIES, NO_INSTANCE, is_blank, refresh_thumbnail, thumb_path};
use std::path::PathBuf;

/// A fresh, empty folder for one test.
fn temp_dir(name: &str) -> PathBuf {
    let d = std::env::temp_dir().join(format!("soundcraft-thumbs-{name}-{}", std::process::id()));
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

#[test]
fn blank_detection() {
    assert!(is_blank(&solid(300, 200, [0, 0, 0]), 300, 200), "all black");
    assert!(is_blank(&solid(300, 200, [128, 128, 128]), 300, 200), "all one grey");
    // 99 % one colour: one row in a hundred differs.
    let mut mostly = solid(100, 100, [40, 44, 52]);
    for px in mostly[..100 * 4].chunks_mut(4) {
        px.copy_from_slice(&[250, 10, 10, 255]);
    }
    assert!(is_blank(&mostly, 100, 100), "99 % one colour");
    // Within ±6 of one colour still counts as that colour (noise, dithering).
    let noisy: Vec<u8> = solid(64, 64, [100, 100, 100]).chunks(4).enumerate().flat_map(|(i, _)| [100 + (i % 5) as u8, 98, 103, 255]).collect();
    assert!(is_blank(&noisy, 64, 64), "near one colour");
    assert!(!is_blank(&gradient(400, 240), 400, 240), "a gradient is a picture");
    // 10 % another colour is a (sparse) picture.
    let mut tenth = solid(100, 100, [0, 0, 0]);
    for px in tenth[..1000 * 4].chunks_mut(4) {
        px.copy_from_slice(&[200, 200, 200, 255]);
    }
    assert!(!is_blank(&tenth, 100, 100));
    assert!(is_blank(&[], 0, 0), "nothing is blank");
    assert!(is_blank(&[1, 2, 3], 10, 10), "too few bytes for the size");
}

#[test]
fn thumb_path_sanitises() {
    let dir = PathBuf::from("thumbs");
    let mut it = item("vst3:0123ABCD", Format::Vst3);
    it.version = "1.2.3 (beta/2)".into();
    assert_eq!(thumb_path(&dir, &it), dir.join("vst3_0123ABCD-1.2.3__beta_2_.png"));
    let mut clap = item("clap:com.u-he.Diva", Format::Clap);
    clap.version = "1.4.5".into();
    assert_eq!(thumb_path(&dir, &clap), dir.join("clap_com.u-he.Diva-1.4.5.png"));
    // No path separators or drive letters survive.
    let mut odd = item(r"..\..\C:/x", Format::Vst3);
    odd.version = "../9".into();
    let p = thumb_path(&dir, &odd);
    assert_eq!(p.parent(), Some(dir.as_path()));
    let name = p.file_name().unwrap().to_str().unwrap();
    assert!(!name.contains(['/', '\\', ':']), "{name}");
}

#[test]
fn cached_png_is_used_for_card() {
    let dir = temp_dir("cached");
    let hosted = BrowserItem { version: "2.0".into(), ..item("vst3:00112233445566778899AABBCCDDEEFF", Format::Vst3) };
    let png = builtin_thumbnail("eq_7band").unwrap();
    std::fs::write(thumb_path(&dir, &hosted), png).unwrap();
    let ctx = egui::Context::default();
    // Without a folder the card keeps its placeholder.
    assert!(ThumbCache::default().texture(&ctx, &hosted).is_none());
    // The app hands its folder to the cache: the card loads the picture from disk.
    let services = Services { thumb_dir: Some(dir.clone()), ..Services::default() };
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, services);
    let tex = app.browser_thumbs.texture(&ctx, &hosted).expect("the disk picture");
    let decoded = image::load_from_memory(png).unwrap();
    assert_eq!(tex.size(), [decoded.width() as usize, decoded.height() as usize]);
    // Another version of the plugin has no picture yet.
    let newer = BrowserItem { version: "2.1".into(), ..hosted.clone() };
    app.browser_thumbs.forget(&newer.id);
    assert!(app.browser_thumbs.texture(&ctx, &newer).is_none());
    // A built-in keeps its built-in picture even with a file of its name on disk.
    let comp = item("fs_comp", Format::FireStation);
    std::fs::write(thumb_path(&dir, &comp), b"not a png").unwrap();
    assert!(app.browser_thumbs.texture(&ctx, &comp).is_some());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn refresh_without_an_instance_says_how() {
    let mut app = app_with_tracks(&["Voice"]);
    let hosted = item("vst3:00112233445566778899AABBCCDDEEFF", Format::Vst3);
    refresh_thumbnail(&mut app, &hosted);
    assert_eq!(app.ui.status, NO_INSTANCE);
}

/// A hosted plugin's editor stand-in: opens and closes, and captures a fixed picture.
struct FakeEditor {
    open: bool,
    picture: (u32, u32, Vec<u8>),
    captures: std::sync::Arc<std::sync::atomic::AtomicUsize>,
    /// Attempts that get no picture first (an editor not drawn yet).
    misses: usize,
}

impl soundcraft_dsp::PluginEditor for FakeEditor {
    fn open(&mut self) -> Result<(), String> {
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
        let n = self.captures.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        (n >= self.misses).then(|| self.picture.clone())
    }
}

#[test]
fn an_open_hosted_editor_is_captured_once_after_a_delay() {
    use soundcraft_playback::SlotKey;
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};
    let dir = temp_dir("capture");
    let mut engine = soundcraft_engine::Engine::default();
    engine.execute("track.new", &json!({"name": "Voice"})).unwrap();
    engine.execute("track.new", &json!({"name": "Pad"})).unwrap();
    // Ids no host can parse: nothing is scanned or loaded for them (the commands refuse them, so
    // they go into the session directly, as a session saved with a missing plugin would have them).
    let fake = "vst3:fake.voice";
    let flat = "vst3:fake.pad";
    for (name, plugin) in [("Voice", fake), ("Pad", flat)] {
        let t = engine.session().track_by_name(name).unwrap().id;
        engine.session_mut().track_mut(t).unwrap().mixer.add_module(soundcraft_model::Insert::new(plugin), None).unwrap();
    }
    let slot = |name: &str, plugin: &str| {
        let t = engine.session().track_by_name(name).unwrap();
        (t.id, SlotKey::Module(t.mixer.inserts.iter().find(|i| i.plugin == plugin).unwrap().id))
    };
    let (voice, voice_slot) = slot("Voice", fake);
    let (pad, pad_slot) = slot("Pad", flat);
    let player = soundcraft_playback::Player::new(engine.session_arc());
    let (shots, blank_shots) = (Arc::new(AtomicUsize::new(0)), Arc::new(AtomicUsize::new(0)));
    player.attach_editor(
        voice,
        voice_slot,
        Box::new(FakeEditor { open: false, picture: (800, 480, gradient(800, 480)), captures: shots.clone(), misses: 0 }),
    );
    player.attach_editor(
        pad,
        pad_slot,
        Box::new(FakeEditor { open: false, picture: (640, 400, solid(640, 400, [20, 20, 20])), captures: blank_shots.clone(), misses: 0 }),
    );
    let mut app = SoundApp::new(engine, Some(player), Services { thumb_dir: Some(dir.clone()), ..Services::default() });
    let voice_item = BrowserItem { version: "3.1".into(), ..item(fake, Format::Vst3) };
    let pad_item = BrowserItem { version: "1".into(), ..item(flat, Format::Vst3) };
    app.browser_catalog =
        Some(soundcraft_ui_egui::browser::catalog::Catalog { items: vec![voice_item.clone(), pad_item.clone()], failures: Vec::new() });
    let mut h = Harness::builder().with_size(vec2(1200.0, 700.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    h.run_steps(8);
    assert_eq!(shots.load(Ordering::Relaxed), 0, "closed editors are not captured");
    let player = h.state().player.as_ref().unwrap();
    player.open_editor(voice, voice_slot).unwrap();
    player.open_editor(pad, pad_slot).unwrap();
    h.run_steps(4);
    assert_eq!(shots.load(Ordering::Relaxed), 0, "not before the editor has drawn");
    h.run_steps(4);
    assert_eq!(shots.load(Ordering::Relaxed), 1);
    let path = thumb_path(&dir, &voice_item);
    let saved = image::open(&path).expect("the picture is saved");
    assert_eq!((saved.width(), saved.height()), (MAX_WIDTH, 240), "scaled to the stored width");
    let ctx = h.ctx.clone();
    assert!(h.state_mut().browser_thumbs.texture(&ctx, &voice_item).is_some(), "the card shows it now");
    assert_eq!(blank_shots.load(Ordering::Relaxed), 1);
    assert!(!thumb_path(&dir, &pad_item).exists(), "a blank capture keeps the placeholder");
    h.run_steps(20);
    assert_eq!(shots.load(Ordering::Relaxed), 1, "once per opening, and a fresh picture is kept");
    // Closed and opened again: the picture is fresh, so nothing more.
    h.state().player.as_ref().unwrap().close_editor(voice, voice_slot);
    h.run_steps(2);
    h.state().player.as_ref().unwrap().open_editor(voice, voice_slot).unwrap();
    h.run_steps(10);
    assert_eq!(shots.load(Ordering::Relaxed), 1);
    // "Refresh thumbnail" on the selected track: opens the closed editor, captures, closes it again.
    h.state().player.as_ref().unwrap().close_editor(voice, voice_slot);
    h.state_mut().engine.session_mut().edit.selected_tracks = vec![voice];
    h.run_steps(2);
    refresh_thumbnail(h.state_mut(), &voice_item);
    assert!(h.state().player.as_ref().unwrap().editor_open(voice, voice_slot), "opened to capture");
    h.run_steps(8);
    assert_eq!(shots.load(Ordering::Relaxed), 2, "a refresh captures again");
    assert!(!h.state().player.as_ref().unwrap().editor_open(voice, voice_slot), "and closes what it opened");
    assert!(!h.state().is_playing(), "all of it with the transport stopped");
    let _ = std::fs::remove_dir_all(&dir);
}

/// An app with one track per `(plugin, editor)` (each plugin id as its only insert), the editors
/// attached to a real (stopped) player, pictures stored in `dir`; with each slot.
fn rig(
    dir: &std::path::Path,
    editors: Vec<(&str, FakeEditor)>,
) -> (Harness<'static, SoundApp>, Vec<(soundcraft_model::TrackId, soundcraft_playback::SlotKey)>, Vec<BrowserItem>) {
    let mut engine = soundcraft_engine::Engine::default();
    let mut slots = Vec::new();
    for (i, (plugin, _)) in editors.iter().enumerate() {
        engine.execute("track.new", &json!({"name": format!("T{i}")})).unwrap();
        let t = engine.session().track_by_name(&format!("T{i}")).unwrap().id;
        let m = engine.session_mut().track_mut(t).unwrap().mixer.add_module(soundcraft_model::Insert::new(*plugin), None).unwrap();
        slots.push((t, soundcraft_playback::SlotKey::Module(m)));
    }
    let player = soundcraft_playback::Player::new(engine.session_arc());
    let items: Vec<BrowserItem> = editors.iter().map(|(p, _)| item(p, Format::Vst3)).collect();
    for ((t, k), (_, e)) in slots.iter().zip(editors) {
        player.attach_editor(*t, *k, Box::new(e));
    }
    let mut app = SoundApp::new(engine, Some(player), Services { thumb_dir: Some(dir.to_path_buf()), ..Services::default() });
    app.browser_catalog = Some(soundcraft_ui_egui::browser::catalog::Catalog { items: items.clone(), failures: Vec::new() });
    let h = Harness::builder().with_size(vec2(1200.0, 700.0)).with_step_dt(0.25).build_ui_state(
        |ui, app: &mut SoundApp| {
            app.logic(&ui.ctx().clone());
            app.ui(ui);
        },
        app,
    );
    (h, slots, items)
}

fn counter() -> std::sync::Arc<std::sync::atomic::AtomicUsize> {
    std::sync::Arc::new(std::sync::atomic::AtomicUsize::new(0))
}

fn count(c: &std::sync::Arc<std::sync::atomic::AtomicUsize>) -> usize {
    c.load(std::sync::atomic::Ordering::Relaxed)
}

#[test]
fn a_damaged_stored_picture_is_taken_again() {
    let dir = temp_dir("damaged");
    let shots = counter();
    let e = FakeEditor { open: false, picture: (300, 200, gradient(300, 200)), captures: shots.clone(), misses: 0 };
    let (mut h, slots, items) = rig(&dir, vec![("vst3:fake.damaged", e)]);
    // A recent but truncated file (a crash while writing, before writes were atomic).
    let path = thumb_path(&dir, &items[0]);
    std::fs::write(&path, &builtin_thumbnail("eq_7band").unwrap()[..40]).unwrap();
    h.state().player.as_ref().unwrap().open_editor(slots[0].0, slots[0].1).unwrap();
    h.run_steps(8);
    assert_eq!(count(&shots), 1, "a damaged picture is not fresh");
    let img = image::open(&path).expect("replaced by a whole picture");
    assert_eq!((img.width(), img.height()), (300, 200), "narrower than the stored width: kept as is");
    let leftovers: Vec<_> =
        std::fs::read_dir(&dir).unwrap().map(|e| e.unwrap().file_name()).filter(|n| n.to_string_lossy().ends_with(".tmp")).collect();
    assert!(leftovers.is_empty(), "{leftovers:?}");
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn a_capture_that_gets_nothing_is_retried_a_few_times() {
    let dir = temp_dir("retry");
    let (late, never) = (counter(), counter());
    let (mut h, slots, items) = rig(
        &dir,
        vec![
            ("vst3:fake.late", FakeEditor { open: false, picture: (300, 200, gradient(300, 200)), captures: late.clone(), misses: 2 }),
            ("vst3:fake.never", FakeEditor { open: false, picture: (300, 200, gradient(300, 200)), captures: never.clone(), misses: usize::MAX }),
        ],
    );
    for (t, k) in &slots {
        h.state().player.as_ref().unwrap().open_editor(*t, *k).unwrap();
    }
    h.run_steps(8);
    assert_eq!((count(&late), count(&never)), (1, 1), "the first attempt, after the delay");
    assert!(!thumb_path(&dir, &items[0]).exists());
    h.run_steps(10);
    assert_eq!(count(&late), 3, "about a second apart, until it gives a picture");
    assert!(thumb_path(&dir, &items[0]).exists());
    h.run_steps(40);
    assert_eq!(count(&late), 3, "then no more");
    assert_eq!(count(&never), CAPTURE_TRIES as usize, "a bounded number of attempts");
    assert!(!thumb_path(&dir, &items[1]).exists());
    let _ = std::fs::remove_dir_all(&dir);
}

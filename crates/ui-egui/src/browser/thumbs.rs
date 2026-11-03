//! The browser cards' pictures. Built-in and FireStation plugins carry one each, rendered from
//! their editors by `cargo xtask browser-thumbs` (dark theme, at most [`MAX_WIDTH`] px wide) and
//! built into the app ([`builtin_thumbnail`]). Hosted plugins get theirs at run time (captured
//! from their own editors, or read back from the disk cache), handed in with
//! [`ThumbCache::set_png`].
//!
//! [`ThumbCache`] decodes a picture into a texture the first time a card that shows it is
//! drawn, never for cards out of view.
//!
//! A hosted plugin's picture is taken from its own editor window
//! ([`ThumbCache::capture_when_open`]): once the editor has been open for [`CAPTURE_DELAY`]
//! seconds ([`INSTRUMENT_CAPTURE_DELAY`] for an instrument, which may show a loading screen
//! first), when there is no picture yet or the stored one is older than [`REFRESH_AFTER`].
//! "Capture Thumbnail Now" ([`capture_now`]) takes an open editor's picture at once. A blank
//! capture (a GPU-drawn editor the system could not read) keeps the picture there was. Pictures
//! are stored at most [`MAX_WIDTH`] px wide in the app's `PluginThumbs` folder, one file per
//! plugin and version ([`thumb_path`]). Taking one reads the window on the UI thread only:
//! playback is never involved.

use super::catalog::{BrowserCategory, BrowserItem, Format, Kind};
use crate::SoundApp;
use egui::{ColorImage, Context, TextureHandle, TextureOptions};
use soundcraft_model::TrackId;
use soundcraft_playback::SlotKey;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

include!("thumbs_table.rs");

/// The widest a stored picture is, in pixels (a large card at two pixels per point).
pub const MAX_WIDTH: u32 = 400;

/// Seconds a hosted effect's editor is open before its picture is taken (it has drawn by then).
pub const CAPTURE_DELAY: f64 = 1.5;

/// Seconds a hosted instrument's editor is open before its picture is taken: instruments often
/// show a loading screen while their sounds load.
pub const INSTRUMENT_CAPTURE_DELAY: f64 = 10.0;

/// How long the editor in `slot` is open before its picture is taken on its own.
pub fn capture_delay(slot: SlotKey) -> f64 {
    match slot {
        SlotKey::Instrument => INSTRUMENT_CAPTURE_DELAY,
        SlotKey::Module(_) => CAPTURE_DELAY,
    }
}

/// A stored picture older than this is taken again the next time the editor is open.
pub const REFRESH_AFTER: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// Attempts at one opening's picture when the editor gives none (not drawn yet, or hidden).
pub const CAPTURE_TRIES: u32 = 5;

/// Seconds between those attempts.
pub const RETRY_EVERY: f64 = 1.0;

/// What "Refresh thumbnail" says when the selected track has no instance of the plugin.
pub const NO_INSTANCE: &str = "Put the plugin on a track and open it once to capture its picture";

/// The pre-rendered picture of a built-in or FireStation plugin, as PNG bytes.
pub fn builtin_thumbnail(id: &str) -> Option<&'static [u8]> {
    THUMBS.binary_search_by(|(k, _)| (*k).cmp(id)).ok().map(|i| THUMBS[i].1)
}

/// Whether the plugin's picture comes from its own editor at run time (not built in).
fn is_hosted(format: Format) -> bool {
    matches!(format, Format::Vst3 | Format::Clap | Format::Au)
}

/// Where a hosted plugin's picture is stored: `<sanitised id>-<sanitised version>.png` in `dir`.
pub fn thumb_path(dir: &Path, item: &BrowserItem) -> PathBuf {
    dir.join(format!("{}-{}.png", crate::preset_folder_name(&item.id), crate::preset_folder_name(&item.version)))
}

/// Whether a capture shows nothing: at least 98 % of the sampled pixels (a grid of at most about
/// 64×64) are within ±6 of one colour (per channel, alpha ignored). No pixels, or too few bytes
/// for the size, is blank too.
pub fn is_blank(rgba: &[u8], w: u32, h: u32) -> bool {
    let (w, h) = (w as usize, h as usize);
    if w == 0 || h == 0 || rgba.len() < w.saturating_mul(h).saturating_mul(4) {
        return true;
    }
    let (sx, sy) = ((w / 64).max(1), (h / 64).max(1));
    let samples: Vec<[u8; 3]> = (0..h)
        .step_by(sy)
        .flat_map(|y| {
            (0..w).step_by(sx).map(move |x| {
                let i = (y * w + x) * 4;
                [rgba[i], rgba[i + 1], rgba[i + 2]]
            })
        })
        .collect();
    let mut counts: HashMap<[u8; 3], usize> = HashMap::new();
    for c in &samples {
        *counts.entry(*c).or_default() += 1;
    }
    // The most common colour (the lowest of equals, so the answer never depends on hash order).
    let Some(common) = counts.iter().max_by(|a, b| a.1.cmp(b.1).then(b.0.cmp(a.0))).map(|(c, _)| *c) else { return true };
    let near = samples.iter().filter(|c| c.iter().zip(common).all(|(a, b)| a.abs_diff(b) <= 6)).count();
    near * 100 >= samples.len() * 98
}

/// A capture as the stored picture: scaled down to [`MAX_WIDTH`] px wide (never up), as PNG.
pub fn encode_thumbnail(rgba: Vec<u8>, w: u32, h: u32) -> Option<Vec<u8>> {
    let img = image::RgbaImage::from_raw(w, h, rgba)?;
    let img = if w > MAX_WIDTH {
        let nh = ((u64::from(h) * u64::from(MAX_WIDTH) + u64::from(w) / 2) / u64::from(w)).max(1);
        image::imageops::resize(&img, MAX_WIDTH, u32::try_from(nh).ok()?, image::imageops::FilterType::Triangle)
    } else {
        img
    };
    let mut out = Vec::new();
    img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).ok()?;
    Some(out)
}

/// A PNG decoded for the GPU, or `None` when it is not a readable PNG.
fn decode(png: &[u8]) -> Option<ColorImage> {
    let img = image::load_from_memory_with_format(png, image::ImageFormat::Png).ok()?.to_rgba8();
    let size = [img.width() as usize, img.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, img.as_raw()))
}

/// Texture filtering for a picture drawn at a fraction of its size.
fn options() -> TextureOptions {
    TextureOptions::LINEAR.with_mipmap_mode(Some(egui::TextureFilter::Linear))
}

/// An open hosted editor being waited on.
struct Watch {
    /// When it was first seen open (egui time, seconds).
    since: f64,
    /// Its picture was taken (or found fresh, or given up on) during this opening.
    done: bool,
    /// Attempts that got no picture from the editor so far.
    failed: u32,
}

/// A "Refresh thumbnail" under way.
struct Refresh {
    key: (TrackId, SlotKey),
    /// The editor was opened for it, and is closed again after the capture.
    close: bool,
}

/// Card pictures as textures, made lazily for the cards being drawn.
#[derive(Default)]
pub struct ThumbCache {
    /// Plugin id to its texture, or `None` once known to have no (readable) picture.
    textures: HashMap<String, Option<TextureHandle>>,
    /// Pictures handed in at run time (hosted plugins), over the built-in ones.
    pngs: HashMap<String, Vec<u8>>,
    /// The folder hosted plugins' pictures are stored in (`None`: kept in memory only).
    dir: Option<PathBuf>,
    /// Hosted editors open now, by slot.
    watch: HashMap<(TrackId, SlotKey), Watch>,
    refresh: Option<Refresh>,
}

impl ThumbCache {
    /// A cache storing hosted plugins' pictures in `dir` (the app's `PluginThumbs` folder).
    pub fn with_dir(dir: Option<PathBuf>) -> ThumbCache {
        ThumbCache { dir, ..ThumbCache::default() }
    }

    /// Drop the textures (made for an egui context that is gone); they are made again on demand.
    pub fn forget_textures(&mut self) {
        self.textures.clear();
    }

    /// The picture for `item`'s card, decoded the first time it is asked for; `None` keeps the
    /// placeholder. A hosted plugin's comes from its stored file, a built-in's from the app.
    pub fn texture(&mut self, ctx: &Context, item: &BrowserItem) -> Option<TextureHandle> {
        if let Some(t) = self.textures.get(&item.id) {
            return t.clone();
        }
        let stored;
        let png = match self.pngs.get(&item.id) {
            Some(p) => Some(p.as_slice()),
            None if is_hosted(item.format) => {
                stored = self.dir.as_deref().and_then(|d| std::fs::read(thumb_path(d, item)).ok());
                stored.as_deref()
            }
            None => builtin_thumbnail(&item.id),
        };
        let tex = png.and_then(decode).map(|img| ctx.load_texture(format!("browser-thumb:{}", item.id), img, options()));
        self.textures.insert(item.id.clone(), tex.clone());
        tex
    }

    /// Gives plugin `id` this picture (PNG bytes), replacing any it had: the next draw of its
    /// card decodes it.
    pub fn set_png(&mut self, id: &str, png: Vec<u8>) {
        self.pngs.insert(id.to_string(), png);
        self.textures.remove(id);
    }

    /// Drops plugin `id`'s texture only (it is made again when next shown); a picture handed in
    /// with [`ThumbCache::set_png`] is kept.
    pub fn forget(&mut self, id: &str) {
        self.textures.remove(id);
    }

    /// Textures held now.
    pub fn loaded(&self) -> usize {
        self.textures.values().filter(|t| t.is_some()).count()
    }

    /// Once per frame: takes the picture of each hosted plugin editor that has been open for
    /// [`capture_delay`] seconds, once per opening, unless a stored one is fresh (younger than
    /// [`REFRESH_AFTER`]); a "Refresh thumbnail" takes it regardless, then closes the editor if
    /// it opened it. A blank capture keeps the placeholder. UI thread only: the audio thread is
    /// never asked for anything.
    pub fn capture_when_open(app: &mut SoundApp, ctx: &Context) {
        let Some(player) = &app.player else { return };
        let cache = &app.browser_thumbs;
        if cache.watch.is_empty() && cache.refresh.is_none() && !player.any_editor_open() {
            return;
        }
        let now = ctx.input(|i| i.time);
        let open: Vec<((TrackId, SlotKey), String)> = hosted_slots(app).into_iter().filter(|(k, _)| player.editor_open(k.0, k.1)).collect();
        let cache = &mut app.browser_thumbs;
        cache.watch.retain(|k, _| open.iter().any(|(o, _)| o == k));
        if cache.refresh.as_ref().is_some_and(|r| !open.iter().any(|(o, _)| *o == r.key)) {
            // Its editor was closed before the picture was taken.
            cache.refresh = None;
        }
        let mut due = Vec::new();
        let mut wait: Option<f64> = None;
        for (k, plugin) in open {
            let w = cache.watch.entry(k).or_insert(Watch { since: now, done: false, failed: 0 });
            if w.done {
                continue;
            }
            let left = w.since + capture_delay(k.1) - now;
            if left <= 1e-9 {
                w.done = true;
                due.push((k, plugin));
            } else {
                wait = Some(wait.map_or(left, |x| x.min(left)));
            }
        }
        if let Some(s) = wait {
            ctx.request_repaint_after(Duration::from_secs_f64(s));
        }
        for (k, plugin) in due {
            let forced = app.browser_thumbs.refresh.as_ref().is_some_and(|r| r.key == k);
            let last = app.browser_thumbs.watch.get(&k).is_none_or(|w| w.failed + 1 >= CAPTURE_TRIES);
            if capture(app, k, &plugin, forced, last) == Shot::Missing
                && !last
                && let Some(w) = app.browser_thumbs.watch.get_mut(&k)
            {
                // Try again a little later (a refresh stays under way).
                w.failed += 1;
                w.done = false;
                w.since = now + RETRY_EVERY - capture_delay(k.1);
                ctx.request_repaint_after(Duration::from_secs_f64(RETRY_EVERY));
                continue;
            }
            let refresh = app.browser_thumbs.refresh.take_if(|r| r.key == k);
            if let (Some(r), Some(p)) = (refresh, &app.player)
                && r.close
            {
                p.close_editor(k.0, k.1);
            }
        }
    }
}

/// Every slot of the session holding a hosted plugin, with the plugin's id.
fn hosted_slots(app: &SoundApp) -> Vec<((TrackId, SlotKey), String)> {
    let mut out = Vec::new();
    for t in &app.engine.session().tracks {
        if let Some(i) = &t.instrument
            && soundcraft_mix::is_third_party(&i.plugin)
        {
            out.push(((t.id, SlotKey::Instrument), i.plugin.clone()));
        }
        for m in t.mixer.inserts.iter().filter(|m| soundcraft_mix::is_third_party(&m.plugin)) {
            out.push(((t.id, SlotKey::Module(m.id)), m.plugin.clone()));
        }
    }
    out
}

/// The browser's entry for `plugin` (its version names the stored file), or a bare one.
fn item_for(app: &mut SoundApp, plugin: &str) -> BrowserItem {
    super::panel::ensure_catalog(app);
    app.browser_catalog.as_ref().and_then(|c| c.items.iter().find(|i| i.id == plugin)).cloned().unwrap_or_else(|| BrowserItem {
        id: plugin.to_string(),
        name: plugin.to_string(),
        vendor: String::new(),
        version: String::new(),
        format: if plugin.starts_with(soundcraft_clap_host::ID_PREFIX) { Format::Clap } else { Format::Vst3 },
        kind: Kind::Effect,
        category: BrowserCategory::Other,
    })
}

/// Whether the stored picture at `path` is younger than [`REFRESH_AFTER`] and a readable PNG (a
/// damaged file is taken again).
fn fresh(path: &Path) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|m| m.modified()) else { return false };
    if std::fs::read(path).ok().and_then(|png| decode(&png)).is_none() {
        return false;
    }
    // A time in the future (a clock change) counts as fresh.
    std::time::SystemTime::now().duration_since(modified).map_or(true, |age| age < REFRESH_AFTER)
}

/// What one attempt at a picture came to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shot {
    /// Taken, found fresh, blank, or given up on (all final for this opening).
    Settled,
    /// The editor gave no picture: worth another try.
    Missing,
}

/// Writes `bytes` to `path` through a temporary file renamed over it, so a crash or a full disk
/// never leaves a half-written picture behind.
fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut tmp = path.as_os_str().to_owned();
    tmp.push(".tmp");
    let tmp = PathBuf::from(tmp);
    let r = std::fs::write(&tmp, bytes).and_then(|()| std::fs::rename(&tmp, path));
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

/// Takes, stores and shows the picture of the editor in slot `key` (`forced`: even if a fresh
/// one exists, saying how it went in the status line; `last`: no retry follows a miss).
fn capture(app: &mut SoundApp, key: (TrackId, SlotKey), plugin: &str, forced: bool, last: bool) -> Shot {
    let item = item_for(app, plugin);
    let path = app.browser_thumbs.dir.as_deref().map(|d| thumb_path(d, &item));
    if !forced {
        let have = match &path {
            Some(p) => fresh(p),
            None => app.browser_thumbs.pngs.contains_key(plugin),
        };
        if have {
            return Shot::Settled;
        }
    }
    let shot = app.player.as_ref().and_then(|p| p.capture_editor(key.0, key.1));
    let note = match shot {
        None if !last => return Shot::Missing,
        None => format!("{}: its editor could not be captured", item.name),
        Some((w, h, px)) if is_blank(&px, w, h) => {
            let kept = if has_picture(app, plugin, path.as_deref()) { "the old picture" } else { "the placeholder" };
            format!("{}: its editor came back blank; keeping {kept}", item.name)
        }
        Some((w, h, px)) => match encode_thumbnail(px, w, h) {
            None => format!("{}: the picture could not be encoded", item.name),
            Some(png) => {
                if let Some(p) = &path
                    && let Err(e) = write_atomic(p, &png)
                {
                    log::warn!("{}: {e}", p.display());
                }
                app.browser_thumbs.set_png(plugin, png);
                format!("Captured {} thumbnail", item.name)
            }
        },
    };
    if forced {
        app.ui.status = note;
    } else {
        log::debug!("plugin thumbnail: {note}");
    }
    Shot::Settled
}

/// Whether plugin `plugin` has a picture now (handed in, or stored at `path`).
fn has_picture(app: &SoundApp, plugin: &str, path: Option<&Path>) -> bool {
    app.browser_thumbs.pngs.contains_key(plugin) || path.is_some_and(Path::exists)
}

/// Whether the hosted plugin in `key` has its own editor open now ("Capture Thumbnail Now" is
/// offered only then).
pub fn can_capture_now(app: &SoundApp, key: (TrackId, SlotKey)) -> bool {
    app.player.as_ref().is_some_and(|p| p.editor_open(key.0, key.1))
}

/// What "Capture Thumbnail Now" says when the editor gave no picture even after being restored.
pub const RESTORE_AND_RETRY: &str = "Restore the plugin window and try again";

/// "Capture Thumbnail Now": brings the editor open in slot `key` on top (restored first when it is
/// minimised, which leaves nothing to capture), takes its picture at once, replacing the stored
/// one (a blank capture keeps it), and says how it went in the status line. Only that plugin is
/// touched; once it has a picture (or a blank one), this opening's automatic capture is not taken
/// on top of it, and a "Refresh thumbnail" waiting on it is done, leaving the editor open. UI
/// thread only, as every capture.
pub fn capture_now(app: &mut SoundApp, key: (TrackId, SlotKey)) {
    let Some(plugin) = hosted_slots(app).into_iter().find(|(k, _)| *k == key).map(|(_, p)| p) else { return };
    let name = item_for(app, &plugin).name;
    if !can_capture_now(app, key) {
        app.ui.status = format!("Open {name}'s editor first to capture its thumbnail");
        return;
    }
    // Opening an open editor raises it (and restores it from the taskbar).
    if let Some(p) = &app.player
        && let Err(e) = p.open_editor(key.0, key.1)
    {
        app.ui.status = format!("{name}: {e}");
        return;
    }
    if capture(app, key, &plugin, true, false) == Shot::Missing {
        app.ui.status = format!("{name}: {RESTORE_AND_RETRY}");
        return;
    }
    // `since` no longer matters once the opening is done.
    let since = app.browser_thumbs.watch.get(&key).map_or(0.0, |w| w.since);
    app.browser_thumbs.watch.insert(key, Watch { since, done: true, failed: 0 });
    if app.browser_thumbs.refresh.take_if(|r| r.key == key).is_some_and(|r| r.close) {
        app.ui.status.push_str(" (its editor stays open)");
    }
}

/// Whether the automatic capture of the editor in `key` is finished for this opening (taken,
/// given up on, or done by "Capture Thumbnail Now"); `None` while that editor is not watched.
pub fn capture_settled(app: &SoundApp, key: (TrackId, SlotKey)) -> Option<bool> {
    app.browser_thumbs.watch.get(&key).map(|w| w.done)
}

/// Whether a "Refresh thumbnail" is waiting on an editor.
pub fn refresh_pending(app: &SoundApp) -> bool {
    app.browser_thumbs.refresh.is_some()
}

/// "Refresh thumbnail": opens the plugin's editor on the selected track (its instrument, else the
/// first insert playing it) so its picture is taken after [`capture_delay`], then closed again if
/// it was closed. Without an instance there it says how to get a picture.
pub fn refresh_thumbnail(app: &mut SoundApp, item: &BrowserItem) {
    let session = app.engine.session();
    let key = session.edit.selected_tracks.first().and_then(|&t| session.track(t)).and_then(|t| {
        if t.instrument.as_ref().is_some_and(|i| i.plugin == item.id) {
            return Some((t.id, SlotKey::Instrument));
        }
        t.mixer.inserts.iter().find(|m| m.plugin == item.id).map(|m| (t.id, SlotKey::Module(m.id)))
    });
    let Some((key, player)) = key.zip(app.player.as_ref()).filter(|(k, p)| p.has_editor(k.0, k.1)) else {
        app.ui.status = NO_INSTANCE.to_string();
        return;
    };
    let was_open = player.editor_open(key.0, key.1);
    if !was_open && let Err(e) = player.open_editor(key.0, key.1) {
        app.ui.status = format!("{}: {e}", item.name);
        return;
    }
    // Start the wait over, so the picture shows the editor as it is now.
    app.browser_thumbs.watch.remove(&key);
    app.browser_thumbs.refresh = Some(Refresh { key, close: !was_open });
}

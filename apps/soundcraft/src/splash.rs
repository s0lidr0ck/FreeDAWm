//! The launch splash: the owner's animation (the bird sweeps in and settles into the logo), played
//! silently for its full six seconds in a borderless window centred on screen, while the app
//! loads behind it; a click or Esc skips it.
//!
//! The app has no video decoder: `packaging/splash.py` converts the film
//! (`assets/splash/freedawm-splash-source.mp4`) once into committed JPEG frames
//! (`assets/splash/frames/`, 960×540 at 24 fps) that `build.rs` lists and this module embeds.
//! Playback is paced by the wall clock, not by frame count: a slow machine drops frames rather
//! than stretching the six seconds. Loading runs behind it: the engine on a thread, and the part
//! that must stay on the UI thread (the audio device) restarts the film if it stalled it. Each frame is decoded when it is due (about 3 ms) and kept
//! as one texture, so the decoded film never sits in memory.
//!
//! Over the film, bottom centre, one encouraging statement picked at random on each launch from
//! `assets/splash/statements.txt` (never the previous launch's: `ui.json` `splash.last_statement`),
//! in the bundled Inter (`fonts::SPLASH_FAMILY`), with the version bottom right; drawn every frame,
//! not burned into the frames (spec `docs/superpowers/specs/2026-10-28-splash-statements.md`).
//!
//! The window is the app's own root window dressed as the splash (winit allows one event loop per
//! process, and eframe keeps a window hidden until its first paint): when the film and the loading
//! are both done, `main` gives it back its decorations, size and position (`main_window_commands`,
//! whose order Wayland depends on).
//!
//! Never shown for automation (`wanted`): `--headless`, `--control` (and `FREEDAWM_CONTROL_PORT`),
//! runs without preferences (`FREEDAWM_NO_PREFS`: agents' test runs, `ui_shot`), `CI`, or
//! `FREEDAWM_NO_SPLASH`; and off by the Appearance preference "Show splash screen at launch".

use std::ffi::OsString;
use std::time::{Duration, Instant};

include!(concat!(env!("OUT_DIR"), "/splash_frames.rs"));

/// The frames' size in pixels (and the splash window's size in points).
pub const WIDTH: u32 = 960;
pub const HEIGHT: u32 = 540;
/// The film's frame rate and length.
pub const FPS: f64 = 24.0;
pub const DURATION_SECS: f64 = 6.0;

/// The encouraging statements (`assets/splash/statements.txt`, the owner's to edit): one per line;
/// blank lines and lines starting with `#` are ignored. One is shown at random on every launch.
const STATEMENTS_TXT: &str = include_str!("../../../assets/splash/statements.txt");

/// The statements in `text`, trimmed, without blank or `#` lines (and without a BOM).
pub fn parse_statements(text: &str) -> Vec<&str> {
    text.trim_start_matches('\u{feff}').lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')).collect()
}

/// The embedded statements.
pub fn statements() -> Vec<&'static str> {
    parse_statements(STATEMENTS_TXT)
}

/// A random index among `count` statements from `seed`, never `last` when there is another one
/// (`None` when there are none).
pub fn pick(count: usize, last: Option<usize>, seed: u64) -> Option<usize> {
    match count {
        0 => None,
        1 => Some(0),
        _ => match last.filter(|l| *l < count) {
            // Draw among the others, then step over the previous one.
            Some(last) => {
                let i = (seed % (count as u64 - 1)) as usize;
                Some(if i >= last { i + 1 } else { i })
            }
            None => Some((seed % count as u64) as usize),
        },
    }
}

/// A seed that differs between launches: std's per-process random hasher keys, plus the clock.
fn seed() -> u64 {
    use std::hash::{BuildHasher, Hasher};
    let mut h = std::collections::hash_map::RandomState::new().build_hasher();
    h.write_u128(std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or_default());
    h.finish()
}

/// This launch's statement, and records its index in `last` (the preference
/// `splash.last_statement`). An empty statements file logs a warning and shows none.
pub fn choose_statement(last: &mut Option<usize>) -> Option<&'static str> {
    let all = statements();
    if all.is_empty() {
        log::warn!("assets/splash/statements.txt has no statements: the splash shows none");
        return None;
    }
    let i = pick(all.len(), *last, seed())?;
    *last = Some(i);
    all.get(i).copied()
}

/// The overlay's look at the film's native 960×540 scale (scaled with the frame).
const STATEMENT_SIZE: f32 = 20.0;
/// The last line's baseline, up from the frame's bottom edge.
const STATEMENT_BASELINE_UP: f32 = 46.0;
const STATEMENT_WRAP: f32 = 720.0;
const STATEMENT_ROWS: usize = 2;
/// The loading line (the open's phase) under the statement: size, and its baseline's height
/// above the film's bottom edge.
const PROGRESS_SIZE: f32 = 12.0;
const PROGRESS_BASELINE_UP: f32 = 16.0;
const VERSION_SIZE: f32 = 11.0;
const VERSION_RIGHT: f32 = 18.0;
const VERSION_BOTTOM: f32 = 12.0;

/// Paints the statement (bottom centre, with a soft shadow) and the version label (bottom right)
/// over the film frame `fit`, with the loading line `progress` (the open's phase and item)
/// centred under the statement. The splash font family must be bound
/// (`fonts::has_splash_family`).
pub fn paint_overlay(painter: &egui::Painter, fit: egui::Rect, statement: Option<&str>, progress: Option<&str>) {
    use egui::text::{LayoutJob, TextFormat, TextWrapping};
    use egui::{Color32, FontFamily, FontId, pos2, vec2};
    let scale = fit.width() / WIDTH as f32;
    if !scale.is_finite() || scale <= 0.0 {
        return;
    }
    let family = FontFamily::Name(soundcraft_ui_egui::fonts::SPLASH_FAMILY.into());
    if let Some(text) = statement.map(str::trim).filter(|t| !t.is_empty()) {
        let format = TextFormat::simple(FontId::new(STATEMENT_SIZE * scale, family.clone()), Color32::from_rgba_unmultiplied(255, 255, 255, 235));
        let mut job = LayoutJob::single_section(text.to_owned(), format);
        job.wrap =
            TextWrapping { max_width: STATEMENT_WRAP * scale, max_rows: STATEMENT_ROWS, break_anywhere: false, overflow_character: Some('…') };
        job.halign = egui::Align::Center;
        let galley = painter.layout_job(job);
        // The last line's baseline, relative to the galley.
        let baseline = galley.rows.last().and_then(|r| r.row.glyphs.first().map(|g| r.pos.y + g.pos.y)).unwrap_or_else(|| galley.rect.bottom());
        let pos = pos2(fit.center().x - galley.rect.center().x, fit.bottom() - STATEMENT_BASELINE_UP * scale - baseline);
        // egui has no blur: the shadow is the text four times at 1 px offsets, faintly.
        let shadow = Color32::from_rgba_unmultiplied(0, 0, 0, 89);
        for offset in [vec2(0.0, 1.0), vec2(-1.0, 1.0), vec2(1.0, 1.0), vec2(0.0, 2.0)] {
            painter.galley_with_override_text_color(pos + offset * scale, galley.clone(), shadow);
        }
        painter.galley(pos, galley, Color32::WHITE);
    }
    if let Some(text) = progress.map(str::trim).filter(|t| !t.is_empty()) {
        painter.text(
            pos2(fit.center().x, fit.bottom() - PROGRESS_BASELINE_UP * scale),
            egui::Align2::CENTER_BOTTOM,
            text,
            FontId::new(PROGRESS_SIZE * scale, family.clone()),
            Color32::from_rgba_unmultiplied(255, 255, 255, 170),
        );
    }
    painter.text(
        pos2(fit.right() - VERSION_RIGHT * scale, fit.bottom() - VERSION_BOTTOM * scale),
        egui::Align2::RIGHT_BOTTOM,
        version_label(),
        FontId::new(VERSION_SIZE * scale, family),
        Color32::from_rgba_unmultiplied(255, 255, 255, 71),
    );
}

/// "FreeDAWm <version>", bottom right of the splash.
pub fn version_label() -> String {
    format!("FreeDAWm {}", env!("CARGO_PKG_VERSION"))
}

/// The frame due `elapsed` seconds after the start (the last frame once the film is over).
pub fn frame_at(elapsed: f64) -> usize {
    let last = FRAMES.len().saturating_sub(1);
    if elapsed.is_nan() || elapsed <= 0.0 {
        return 0;
    }
    // Finite and non-negative here, so the cast saturates at worst.
    ((elapsed * FPS).floor() as usize).min(last)
}

/// The film is over.
pub fn finished(elapsed: f64) -> bool {
    elapsed.is_nan() || elapsed >= DURATION_SECS
}

/// How long until the next frame is due (a repaint request), at most one frame.
pub fn next_frame_in(elapsed: f64) -> Duration {
    let period = 1.0 / FPS;
    let into = if elapsed > 0.0 { elapsed % period } else { 0.0 };
    Duration::from_secs_f64((period - into).clamp(0.001, period))
}

/// Whether this launch shows the splash: the preference, and none of the automation gates.
/// `env` is the environment by full variable name (the `FREEDAWM_*` names also read their
/// `SOUNDCRAFT_*` spelling).
pub fn wanted(args: &[String], pref: bool, env: &dyn Fn(&str) -> Option<OsString>) -> bool {
    use soundcraft_engine::brand::env_var_os_with;
    let flag = |f: &str| args.iter().any(|a| a == f);
    pref && !FRAMES.is_empty()
        && !flag("--headless")
        && !flag("--control")
        && !flag("--no-splash")
        && env_var_os_with("CONTROL_PORT", env).is_none()
        && env_var_os_with("NO_PREFS", env).is_none()
        && env_var_os_with("NO_SPLASH", env).is_none()
        && env("CI").is_none()
}

/// The splash's window: borderless, the film's size, centred by [`Splash::ui`] once the screen
/// is known.
pub fn viewport() -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("FreeDAWm")
        .with_inner_size([WIDTH as f32, HEIGHT as f32])
        .with_decorations(false)
        .with_resizable(false)
        .with_maximized(false)
        .with_app_id("io.github.s0lidr0ck.FreeDAWm")
}

/// The playing splash.
pub struct Splash {
    started: Option<Instant>,
    texture: Option<(usize, egui::TextureHandle)>,
    skipped: bool,
    centred: bool,
    /// The statement shown over the film (none when the statements file is empty).
    statement: Option<&'static str>,
    /// The loading line under it: the open's phase and item while the session loads.
    progress: Option<String>,
}

impl Default for Splash {
    fn default() -> Self {
        Self::new()
    }
}

impl Splash {
    pub fn new() -> Self {
        Splash { started: None, texture: None, skipped: false, centred: false, statement: None, progress: None }
    }

    /// The splash showing `statement` over the film.
    pub fn with_statement(statement: Option<&'static str>) -> Self {
        Splash { statement, ..Self::new() }
    }

    /// Sets the loading line under the statement (`None` clears it).
    pub fn set_progress(&mut self, text: Option<String>) {
        self.progress = text;
    }

    /// Seconds since the first frame was drawn (0 before).
    pub fn elapsed(&self) -> f64 {
        self.started.map(|s| s.elapsed().as_secs_f64()).unwrap_or(0.0)
    }

    /// The film has played out, or the user skipped it.
    pub fn done(&self) -> bool {
        self.skipped || (self.started.is_some() && finished(self.elapsed()))
    }

    /// Starts the film over (after a stall on the UI thread that would have cut its opening).
    pub fn restart(&mut self) {
        if self.started.is_some() {
            self.started = Some(Instant::now());
        }
    }

    /// The first frame has been drawn (the window is on screen).
    pub fn shown(&self) -> bool {
        self.started.is_some()
    }

    /// Draws the frame that is due, filling the window; a click or Esc skips.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        let ctx = ui.ctx().clone();
        if !self.centred
            && let Some(cmd) = egui::ViewportCommand::center_on_screen(&ctx)
        {
            ctx.send_viewport_cmd(cmd);
            self.centred = true;
        }
        let started = *self.started.get_or_insert_with(Instant::now);
        let elapsed = started.elapsed().as_secs_f64();
        if ui.input(|i| i.key_pressed(egui::Key::Escape) || i.pointer.any_click()) {
            self.skipped = true;
        }
        let index = frame_at(elapsed);
        if self.texture.as_ref().is_none_or(|(i, _)| *i != index) {
            match decode(index) {
                Some(image) => {
                    let handle = ctx.load_texture(format!("splash:{index}"), image, egui::TextureOptions::LINEAR);
                    self.texture = Some((index, handle));
                }
                None => log::warn!("splash frame {index} did not decode"),
            }
        }
        let rect = ctx.content_rect();
        ui.painter().rect_filled(rect, 0.0, egui::Color32::BLACK);
        // Aspect-fit (the window is the film's shape, so this is the whole window).
        let scale = (rect.width() / WIDTH as f32).min(rect.height() / HEIGHT as f32);
        let size = egui::vec2(WIDTH as f32 * scale, HEIGHT as f32 * scale);
        let fit = egui::Rect::from_center_size(rect.center(), size);
        if let Some((_, tex)) = &self.texture {
            ui.painter().image(tex.id(), fit, egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)), egui::Color32::WHITE);
        }
        // The statement and the version over the film (the host binds the splash font before
        // the first frame; without it, no text rather than egui's panic on an unbound family).
        if soundcraft_ui_egui::fonts::has_splash_family(&ctx) {
            paint_overlay(ui.painter(), fit, self.statement, self.progress.as_deref());
        }
        ctx.request_repaint_after(next_frame_in(elapsed));
    }
}

/// Decodes frame `index` (`None` for a bad index or a corrupt frame).
pub fn decode(index: usize) -> Option<egui::ColorImage> {
    let bytes = FRAMES.get(index)?;
    let img = image::load_from_memory_with_format(bytes, image::ImageFormat::Jpeg).ok()?.into_rgba8();
    let (w, h) = (img.width() as usize, img.height() as usize);
    Some(egui::ColorImage::from_rgba_unmultiplied([w, h], img.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_frame_set_is_the_whole_film_at_its_size() {
        assert_eq!(FRAMES.len(), (DURATION_SECS * FPS) as usize, "144 frames: 6 s at 24 fps");
        for i in 0..FRAMES.len() {
            let img = decode(i).unwrap_or_else(|| panic!("frame {i} decodes"));
            assert_eq!(img.size, [WIDTH as usize, HEIGHT as usize], "frame {i}");
        }
        let total: usize = FRAMES.iter().map(|f| f.len()).sum();
        assert!((2_000_000..=5_000_000).contains(&total), "{total} bytes in all");
        assert!(decode(FRAMES.len()).is_none());
    }

    #[test]
    fn pacing_picks_the_frame_for_the_wall_clock_and_ends_at_six_seconds() {
        assert_eq!(frame_at(0.0), 0);
        assert_eq!(frame_at(-1.0), 0);
        assert_eq!(frame_at(f64::NAN), 0);
        assert_eq!(frame_at(0.04), 0);
        assert_eq!(frame_at(1.0 / 24.0 + 1e-9), 1);
        assert_eq!(frame_at(1.0), 24);
        assert_eq!(frame_at(2.5), 60);
        assert_eq!(frame_at(5.99), 143);
        // Slow frames drop: a 100 ms stall lands on the frame that is due, not the next in line.
        assert_eq!(frame_at(3.0 + 0.1), 74);
        assert_eq!(frame_at(6.0), 143);
        assert_eq!(frame_at(60.0), 143);
        assert!(!finished(5.999));
        assert!(finished(6.0));
        assert!(finished(f64::NAN));
        let step = next_frame_in(1.0 + 0.01).as_secs_f64();
        assert!((step - (1.0 / 24.0 - 0.01)).abs() < 1e-6, "{step}");
        assert!(next_frame_in(0.0).as_secs_f64() <= 1.0 / 24.0 + 1e-6);
    }

    #[test]
    fn the_statements_file_holds_fifty_short_lines() {
        let all = statements();
        assert_eq!(all.len(), 50);
        for s in &all {
            assert!(!s.is_empty() && !s.starts_with('#') && *s == s.trim(), "{s:?}");
            assert!(s.chars().count() <= 140, "too long for two lines: {s:?}");
        }
        assert_eq!(parse_statements("\u{feff}# heading\n\n  One.  \r\n#two\nThree.\n"), ["One.", "Three."]);
        assert!(parse_statements("# only comments\n\n").is_empty());
    }

    #[test]
    fn the_pick_never_repeats_the_last_statement() {
        assert_eq!(pick(0, None, 5), None);
        assert_eq!(pick(1, Some(0), 5), Some(0), "one statement: it is the only choice");
        for count in [2usize, 3, 50] {
            let mut seen = std::collections::BTreeSet::new();
            for last in 0..count {
                for seed in 0..200u64 {
                    let i = pick(count, Some(last), seed).unwrap();
                    assert!(i < count && i != last, "count {count} last {last} got {i}");
                    seen.insert(i);
                }
            }
            assert_eq!(seen.len(), count, "every statement can come up");
            assert!(pick(count, None, 7).is_some_and(|i| i < count));
            assert!(pick(count, Some(count + 3), 7).is_some_and(|i| i < count), "a stale index from a longer file");
        }
        let mut last = None;
        for _ in 0..20 {
            let before = last;
            assert!(choose_statement(&mut last).is_some());
            assert!(last.is_some() && last != before);
        }
    }

    /// The text shapes a splash pass paints over the 960×540 frame, as (text, screen rect).
    fn overlay_texts(statement: Option<&str>) -> (egui::Rect, Vec<(String, egui::Rect)>) {
        let ctx = egui::Context::default();
        ctx.set_fonts(soundcraft_ui_egui::fonts::definitions());
        let input = || egui::RawInput {
            screen_rect: Some(egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH as f32, HEIGHT as f32))),
            ..Default::default()
        };
        ctx.run_ui(input(), |_| {}).textures_delta.clear();
        assert!(soundcraft_ui_egui::fonts::has_splash_family(&ctx));
        let fit = egui::Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(WIDTH as f32, HEIGHT as f32));
        let mut out = ctx.run_ui(input(), |ui| paint_overlay(ui.painter(), fit, statement, None));
        out.textures_delta.clear();
        let texts = out
            .shapes
            .iter()
            .filter_map(|c| match &c.shape {
                egui::Shape::Text(t) => Some((t.galley.text().to_string(), t.galley.rect.translate(t.pos.to_vec2()))),
                _ => None,
            })
            .collect();
        (fit, texts)
    }

    #[test]
    fn the_statement_is_drawn_at_the_bottom_of_the_frame() {
        let line = "Creativity is the ultimate form of freedom.";
        let (fit, texts) = overlay_texts(Some(line));
        let drawn: Vec<_> = texts.iter().filter(|(t, _)| t == line).collect();
        assert_eq!(drawn.len(), 5, "four shadow passes and the text");
        for (_, r) in &drawn {
            assert!(r.top() >= fit.bottom() - 80.0 && r.bottom() <= fit.bottom(), "{r:?} not in the bottom 80 px");
            assert!((r.center().x - fit.center().x).abs() <= 2.0, "centred: {r:?}");
        }
        assert!(texts.iter().any(|(t, r)| *t == version_label() && r.right() <= fit.right() - 17.0), "version bottom right");
        // The longest statement fits in two lines within the wrap width.
        let longest = statements().into_iter().max_by_key(|s| s.chars().count()).unwrap();
        let (_, texts) = overlay_texts(Some(longest));
        let (_, r) = texts.iter().find(|(t, _)| t.starts_with(&longest[..10])).unwrap();
        assert!(r.width() <= STATEMENT_WRAP + 1.0 && r.height() < 3.0 * STATEMENT_SIZE * 1.5, "{r:?}");
        assert!(r.bottom() <= fit.bottom() - 30.0);
    }

    #[test]
    fn without_a_statement_only_the_version_is_drawn() {
        for statement in [None, Some(""), Some("   ")] {
            let (_, texts) = overlay_texts(statement);
            assert_eq!(texts.iter().map(|(t, _)| t.clone()).collect::<Vec<_>>(), [version_label()]);
        }
    }

    #[test]
    fn the_gates_keep_it_hidden_for_automation() {
        let none = |_: &str| None;
        let a = |s: &[&str]| s.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        assert!(wanted(&[], true, &none));
        assert!(wanted(&a(&["--demo", "song.fdawm"]), true, &none));
        assert!(!wanted(&[], false, &none), "preference off");
        assert!(!wanted(&a(&["--headless"]), true, &none));
        assert!(!wanted(&a(&["--demo", "--control", "0"]), true, &none));
        assert!(!wanted(&a(&["--no-splash"]), true, &none));
        let set = |names: &'static [&'static str]| move |n: &str| names.contains(&n).then(|| OsString::from("1"));
        assert!(!wanted(&[], true, &set(&["FREEDAWM_CONTROL_PORT"])), "control-channel / MCP launch");
        assert!(!wanted(&[], true, &set(&["SOUNDCRAFT_CONTROL_PORT"])));
        assert!(!wanted(&[], true, &set(&["FREEDAWM_NO_PREFS"])), "agents' test runs, ui_shot");
        assert!(!wanted(&[], true, &set(&["FREEDAWM_NO_SPLASH"])));
        assert!(!wanted(&[], true, &set(&["CI"])));
        assert!(wanted(&[], true, &set(&["RUST_LOG"])));
    }
}

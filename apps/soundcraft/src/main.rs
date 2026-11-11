//! FreeDAWm desktop app.
//!
//! Usage: `freedawm [--demo] [--control <port>] [--no-audio] [session.fdawm | audio files…]`
//!
//! `--control <port>` (or `FREEDAWM_CONTROL_PORT`; `SOUNDCRAFT_CONTROL_PORT` still works) starts a localhost JSON-lines control server
//! (`0` picks a free port and prints it): `{"id":1,"method":"ui.inspect","params":{}}` →
//! `{"id":1,"ok":true,"result":…}`. See `soundcraft_ui_egui::control` for the methods.
//!
//! `log` records go to standard error and `<settings dir>/logs/freedawm.log` (`RUST_LOG` sets the
//! levels); see [`logging`].
#![cfg_attr(all(target_os = "windows", not(debug_assertions)), windows_subsystem = "windows")]
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

#[cfg(not(target_arch = "wasm32"))]
mod claude;
mod config;
mod control_server;
mod gpu;
#[cfg(any(target_os = "windows", test))]
mod graphics;
mod logging;
#[cfg(target_os = "macos")]
mod native_menu;

mod splash;

use soundcraft_engine::Engine;
use soundcraft_ui_egui::{Services, SoundApp, UiState, dialogs};

/// What the engine's thread delivers: the engine, the command-line session loaded (behind the
/// splash: its reading, audio and waveforms done, see `soundcraft_engine::open_job`; its plugin
/// instances and the install follow in the app, see `Finish`), and what failed to load (for the
/// status line, as `SoundApp::run` would have shown it).
struct Prepared {
    engine: Engine,
    errors: Vec<String>,
    loaded: Option<soundcraft_engine::open_job::LoadedSession>,
}

/// The slow, thread-safe part of loading: the engine, and the command-line session loaded.
/// Behind the splash it runs on a thread, so the film keeps painting; without one, right away.
type Prepare = Box<dyn FnOnce() -> Prepared + Send>;
/// The audio device, opened on the UI thread (a `cpal` stream stays on the thread that made it)
/// for the session it is given: the launched one when a session was named on the command line
/// (so the stream opens at its rate, with no resampler), else the empty one as soon as the
/// engine's thread starts, so its wait falls at the film's start where a restart is invisible.
type Audio = Box<dyn FnOnce(std::sync::Arc<soundcraft_engine::model::Session>) -> Option<Box<soundcraft_playback::Player>>>;
/// The rest of the UI-thread part (the app, the control channel, the other files), run once the
/// engine is ready.
type Finish = Box<dyn FnOnce(&egui::Context, Prepared, Option<Box<soundcraft_playback::Player>>) -> SoundApp>;

/// The engine's thread at work.
struct Running {
    rx: std::sync::mpsc::Receiver<Prepared>,
    thread: Option<std::thread::JoinHandle<()>>,
    /// Opened already (no session on the command line), or still to open for the loaded session.
    player: Result<Option<Box<soundcraft_playback::Player>>, Audio>,
    finish: Finish,
}

enum Loading {
    Pending(Prepare, Audio, Finish),
    Running(Box<Running>),
    Done,
}

/// Whether the audio device opens before the engine arrives: only when no session was named
/// on the command line (an empty session has nothing the device's rate should follow).
fn audio_before_engine(sessions: &[String]) -> bool {
    sessions.is_empty()
}

/// How long `on_exit` waits for the engine's thread before plugin hosting shuts down.
const LOAD_JOIN_WAIT: std::time::Duration = std::time::Duration::from_secs(2);

/// The UI thread may stall this long (the audio device opening) without the film starting over.
const SPLASH_STALL_RESTART: std::time::Duration = std::time::Duration::from_millis(150);
/// Passes after the splash in which the main window's new size is awaited.
const FIT_PASS_BUDGET: u32 = 10;
/// The main window's smallest size, in points.
const MAIN_MIN_SIZE: egui::Vec2 = egui::vec2(960.0, 600.0);

struct App {
    app: Option<SoundApp>,
    loading: Loading,
    /// Where the launch session's loading stands (written by the engine's thread, shown under
    /// the splash's statement).
    load_progress: std::sync::Arc<std::sync::Mutex<soundcraft_engine::open_job::OpenProgress>>,
    /// See `audio_before_engine`.
    audio_first: bool,
    /// Without a splash: the launch session file's rate, for the device (`Some(None)`: unreadable).
    launch_rate: Option<Option<soundcraft_engine::model::time::SampleRate>>,
    /// The launch splash while it plays (the root window dressed as it, see `splash`).
    splash: Option<splash::Splash>,
    /// Files dropped on the splash, and the audio and MIDI files named on the command line:
    /// opened in the main window once no session is opening (they import into the session).
    dropped: Vec<String>,
    /// The main window's saved size and maximized state, applied when the splash ends.
    window: ([f32; 2], bool),
    /// The splash just ended: the coming passes await the main window's size (see `after_splash`).
    fit_pending: bool,
    fit_passes: u32,
    logger: Option<&'static logging::AppLogger>,
    #[cfg(target_os = "macos")]
    menu: Option<native_menu::NativeMenu>,
}

impl App {
    /// Drives the loading: starts the engine's thread behind the splash (or runs it inline), and
    /// finishes on the UI thread once the engine is ready.
    fn load(&mut self, ctx: &egui::Context) {
        match std::mem::replace(&mut self.loading, Loading::Done) {
            Loading::Done => {}
            Loading::Pending(prepare, audio, finish) => {
                if self.splash.is_some() {
                    let (tx, rx) = std::sync::mpsc::channel();
                    let thread = std::thread::Builder::new()
                        .name("load".into())
                        .spawn(move || {
                            let _ = tx.send(prepare());
                        })
                        .map_err(|e| log::warn!("load thread: {e}"))
                        .ok();
                    let player = if self.audio_first { Ok(self.stalling(|| audio(Engine::default().session_arc()))) } else { Err(audio) };
                    self.loading = Loading::Running(Box::new(Running { rx, thread, player, finish }));
                } else {
                    let prepared = prepare();
                    // The session named on the command line opens in the window: the device still
                    // opens at its rate (read from the file), so the stream needs no resampler.
                    let session = match self.launch_rate.take().flatten() {
                        Some(rate) => std::sync::Arc::new(soundcraft_engine::model::Session::new(String::new(), rate)),
                        None => prepared.engine.session_arc(),
                    };
                    let player = audio(session);
                    self.finish(ctx, prepared, player, finish);
                }
            }
            Loading::Running(running) => match running.rx.try_recv() {
                Ok(prepared) => self.finish_running(ctx, prepared, *running),
                Err(std::sync::mpsc::TryRecvError::Empty) => self.loading = Loading::Running(running),
                Err(std::sync::mpsc::TryRecvError::Disconnected) => {
                    // The thread died (a panic the engine's hook reported): start empty, not crash.
                    let message = "Loading the session failed; starting with an empty session.".to_string();
                    log::error!("{message}");
                    let prepared = Prepared { engine: Engine::default(), errors: vec![message], loaded: None };
                    self.finish_running(ctx, prepared, *running);
                }
            },
        }
    }

    /// The engine arrived: open the device for its session when that was left for now, then finish.
    fn finish_running(&mut self, ctx: &egui::Context, prepared: Prepared, running: Running) {
        let Running { player, finish, thread, .. } = running;
        if let Some(t) = thread {
            let _ = t.join();
        }
        let player = match player {
            Ok(player) => player,
            Err(audio) => {
                // At the loaded session's rate (the stream opens with no resampler); the session
                // itself follows through the app's open.
                let session = prepared.loaded.as_ref().map_or_else(
                    || prepared.engine.session_arc(),
                    |l| std::sync::Arc::new(soundcraft_engine::model::Session::new(String::new(), l.session.sample_rate)),
                );
                self.stalling(|| audio(session))
            }
        };
        self.finish(ctx, prepared, player, finish);
    }

    /// The text under the splash's statement: the launch session's loading phase (the engine's
    /// thread's, then the app's plugin phase), none once it is open.
    fn load_line(&self) -> Option<String> {
        let line = |p: &soundcraft_engine::open_job::OpenProgress| {
            if p.item.is_empty() { p.phase_line() } else { format!("{} — {}", p.phase_line(), p.item) }
        };
        if let Some(job) = self.app.as_ref().and_then(|a| a.open.as_ref()) {
            return Some(line(job.progress()));
        }
        matches!(self.loading, Loading::Running(_)).then(|| line(&self.load_progress.lock().unwrap_or_else(std::sync::PoisonError::into_inner)))
    }

    /// Runs a UI-thread step of the loading; a stall longer than [`SPLASH_STALL_RESTART`]
    /// restarts the film so it still plays whole and smooth.
    fn stalling<T>(&mut self, step: impl FnOnce() -> T) -> T {
        let started = std::time::Instant::now();
        let out = step();
        let stall = started.elapsed();
        log::info!("load step took {:.0} ms", stall.as_secs_f64() * 1000.0);
        if stall > SPLASH_STALL_RESTART
            && let Some(splash) = &mut self.splash
        {
            splash.restart();
        }
        out
    }

    /// The last UI-thread part of loading and, on macOS, the system menu bar.
    fn finish(&mut self, ctx: &egui::Context, prepared: Prepared, player: Option<Box<soundcraft_playback::Player>>, finish: Finish) {
        #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
        let mut app = self.stalling(|| finish(ctx, prepared, player));
        // If the system menu bar cannot be built, keep the egui menu bar rather than fail to start.
        #[cfg(target_os = "macos")]
        {
            self.menu = match native_menu::NativeMenu::new(&app, ctx) {
                Ok(m) => {
                    app.native_menu_bar = true;
                    Some(m)
                }
                Err(e) => {
                    log::warn!("system menu bar unavailable, using the in-window menu bar: {e}");
                    None
                }
            };
        }
        self.app = Some(app);
    }

    /// The splash is over and the app is loaded: the root window becomes the main window.
    fn end_splash(&mut self, ctx: &egui::Context) {
        self.splash = None;
        let (size, maximized) = self.window;
        let monitor = ctx.input(|i| i.viewport().monitor_size);
        for cmd in main_window_commands(egui::Vec2::from(size), maximized, monitor) {
            ctx.send_viewport_cmd(cmd);
        }
        self.fit_pending = true;
        self.fit_passes = 0;
        ctx.request_repaint();
    }
}

/// The viewport commands that turn the splash's window (borderless, fixed at the film's size)
/// into the main window: `size` and `maximized` are the saved ones, `monitor` the screen's size
/// when known (the window is centred on it).
///
/// The order matters on Wayland. winit pins a non-resizable window by setting its minimum and
/// maximum size to its current size, and making it resizable again restores those same pinned
/// values, so the splash's maximum (960×540) would still stand when the main window's minimum
/// (960×600) is set: a minimum taller than the maximum is an `xdg_toplevel` protocol error that
/// kills the connection, and the app hung on the splash. The maximum is cleared first.
fn main_window_commands(size: egui::Vec2, maximized: bool, monitor: Option<egui::Vec2>) -> Vec<egui::ViewportCommand> {
    use egui::ViewportCommand;
    let mut cmds = vec![
        ViewportCommand::Decorations(true),
        ViewportCommand::Resizable(true),
        ViewportCommand::MaxInnerSize(egui::Vec2::INFINITY),
        ViewportCommand::MinInnerSize(MAIN_MIN_SIZE),
        ViewportCommand::InnerSize(size),
    ];
    if let Some(monitor) = monitor.filter(|m| m.x > 1.0 && m.y > 1.0) {
        cmds.push(ViewportCommand::OuterPosition(((monitor - size) * 0.5).max(egui::Vec2::ZERO).to_pos2()));
    }
    if maximized {
        cmds.push(ViewportCommand::Maximized(true));
    }
    cmds
}

/// After the splash, whether the wait for the main window's size is over (`Some`), and then
/// whether the window must be maximized because it is bigger than its screen (like
/// `fit_window_to_screen` at a plain start). `inner` is the inner size in zoom-independent
/// points (`inner_rect.size() * zoom_factor`), `passes` the passes since the splash ended. The
/// wait ends as soon as the window is no longer the splash's size, or after [`FIT_PASS_BUDGET`]
/// passes (a window manager that refused the size: the state must still be saved).
fn after_splash(inner: Option<egui::Vec2>, monitor: Option<egui::Vec2>, passes: u32, want_maximized: bool) -> Option<bool> {
    let splash_size = egui::vec2(splash::WIDTH as f32, splash::HEIGHT as f32);
    let still_splash = inner.is_none_or(|i| (i - splash_size).abs().max_elem() < 2.0);
    if still_splash && passes < FIT_PASS_BUDGET {
        return None;
    }
    let too_big = !want_maximized && !still_splash && inner.zip(monitor).is_some_and(|(i, m)| m.x > 1.0 && m.y > 1.0 && (i - m).max_elem() > 0.0);
    Some(too_big)
}

/// Sends the title only when it changes, remembered here as X11 and Wayland can't read it back.
fn set_title(ctx: &egui::Context, title: String) {
    let id = egui::Id::new("window_title");
    if ctx.data(|d| d.get_temp::<String>(id)).as_deref() != Some(title.as_str()) {
        ctx.data_mut(|d| d.insert_temp(id, title.clone()));
        ctx.send_viewport_cmd(egui::ViewportCommand::Title(title));
    }
}

impl App {
    fn logic(&mut self, ctx: &egui::Context) {
        // What the realtime audio thread logged is written here, off that thread.
        if let Some(logger) = self.logger {
            logger.report_audio_thread();
        }
        if self.splash.is_some() {
            // The first pass paints the splash; loading starts once it is on screen and goes on
            // (the thread, then the UI-thread part) while the film plays.
            if self.splash.as_ref().is_some_and(splash::Splash::shown) {
                self.load(ctx);
            }
            if ctx.input(|i| i.viewport().close_requested()) {
                return;
            }
            // The launch session's plugin instances are made a frame at a time while the film
            // plays (the app's own frames start after the splash).
            if let Some(app) = &mut self.app {
                app.open_step(ctx);
            }
            let line = self.load_line();
            if let Some(splash) = &mut self.splash {
                splash.set_progress(line);
            }
            let opening = self.app.as_ref().is_some_and(|a| a.open.is_some());
            if self.splash.as_ref().is_some_and(splash::Splash::done) && self.app.is_some() && !opening {
                self.end_splash(ctx);
            }
            return;
        }
        self.load(ctx);
        let Some(app) = &mut self.app else { return };
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.menu {
            menu.process(app);
        }
        app.guarded(|app| app.logic(ctx));
        #[cfg(target_os = "macos")]
        if let Some(menu) = &mut self.menu {
            menu.sync(app);
        }
        // Files dropped on the window: audio on an instrument track or a Sampler window loads
        // the sampler; otherwise sessions open and audio/MIDI import.
        let dropped: Vec<String> =
            app.take_dropped_files(ctx).iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()).collect();
        self.dropped.extend(dropped);
        // Not while a session is opening: an import would land in the session about to go, and
        // a second session is refused anyway. They wait for the open.
        if app.open.is_none() {
            for p in std::mem::take(&mut self.dropped) {
                open_path(app, &p);
            }
        }
        if app.quit_requested {
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else if ctx.input(|i| i.viewport().close_requested()) && dialogs::ask_to_save(app, "app.quit", &serde_json::json!({})) {
            ctx.send_viewport_cmd(egui::ViewportCommand::CancelClose);
        }
        soundcraft_ui_egui::fit_window_to_screen(ctx);
        let (inner, maximized) = ctx.input(|i| (i.viewport().inner_rect, i.viewport().maximized.unwrap_or(false)));
        // After the splash the window was just resized: the saved size and state are not
        // overwritten with the splash's, and a window bigger than its screen opens maximized.
        if self.fit_pending {
            self.fit_passes = self.fit_passes.saturating_add(1);
            let zoom = ctx.zoom_factor();
            if let Some(maximize) =
                after_splash(inner.map(|r| r.size() * zoom), ctx.input(|i| i.viewport().monitor_size), self.fit_passes, self.window.1)
            {
                self.fit_pending = false;
                if maximize {
                    ctx.send_viewport_cmd(egui::ViewportCommand::Maximized(true));
                }
            }
        }
        if !self.fit_pending {
            app.ui.window_maximized = maximized;
            if let Some(r) = inner.filter(|_| !maximized) {
                app.ui.window_size = (r.size() * ctx.zoom_factor()).round().into();
            }
        }
        app.ui.ui_scale = app.extra.ui_scale;
        // Saved now rather than at exit: the recent list matters most after a crash.
        if app.take_recent_changed() {
            save_prefs(&app.ui);
        }
        // With the system menu bar there is no egui bar showing the window and dirty state, so the
        // title carries them; elsewhere the egui menu bar already does.
        let title = if app.native_menu_bar {
            format!(
                "{}{} — FreeDAWm — {}",
                app.engine.session().name,
                if app.engine.is_dirty() { " *" } else { "" },
                if app.ui.window == soundcraft_ui_egui::MainWindow::Edit { "Edit" } else { "Mix" }
            )
        } else {
            format!("{} — FreeDAWm", app.engine.session().name)
        };
        set_title(ctx, title);
    }
    fn raw_input_hook(&mut self, raw: &mut egui::RawInput) {
        if let Some(app) = &mut self.app {
            app.raw_input_hook(raw);
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui) {
        if let Some(splash) = &mut self.splash {
            splash.ui(ui);
            // Files dropped on the splash open in the main window.
            self.dropped
                .extend(ui.input(|i| i.raw.dropped_files.clone()).iter().map(|f| f.path().to_string_lossy().into_owned()).filter(|s| !s.is_empty()));
            return;
        }
        if let Some(app) = &mut self.app {
            app.guarded(|app| app.ui(ui));
        }
    }
    /// The process is ending (the user quit, or a run can't be recovered): unsaved changes stay
    /// in the autosave folder unless the user quit with Quit, the audio device is closed and
    /// plugin hosting shuts down.
    fn shutdown(&mut self) {
        // A close during the splash: the engine's thread may be inside a session open (plugin
        // lookups); let it finish before plugin hosting shuts down under it.
        if let Loading::Running(running) = std::mem::replace(&mut self.loading, Loading::Done)
            && let Some(thread) = running.thread
        {
            let until = std::time::Instant::now() + LOAD_JOIN_WAIT;
            while !thread.is_finished() && std::time::Instant::now() < until {
                std::thread::sleep(std::time::Duration::from_millis(10));
            }
            if thread.is_finished() {
                let _ = thread.join();
            } else {
                log::warn!("the session was still loading at exit");
            }
        }
        if let Some(app) = &mut self.app {
            // First: a plugin scan's helper process must not outlive the app.
            app.stop_plugin_scan();
            app.close_autosave();
            save_prefs(&app.ui);
        }
        if let Some(logger) = self.logger {
            logger.report_audio_thread();
        }
        // The system Quit (⌘Q) ends the process from inside the event loop, so `run_native` never
        // returns: release the player's plugin instances and shut plugin hosting down here.
        if let Some(app) = &mut self.app {
            app.player = None;
        }
        soundcraft_engine::shutdown_plugin_hosts();
    }

    /// The graphics device is lost and this window is about to close: the session is saved
    /// and plugin editors closed (see `SoundApp::save_for_recovery`); the app, its audio
    /// still running, waits for the next window (`after_gpu_restart`).
    fn before_gpu_restart(&mut self) -> bool {
        self.splash = None;
        self.fit_pending = false;
        let saved = self.app.as_mut().is_none_or(SoundApp::save_for_recovery);
        if !saved {
            log::error!("the session could not be autosaved before the window closed");
        }
        saved
    }

    /// The new window after a graphics device loss: the app draws again in its new context.
    fn after_gpu_restart(&mut self, ctx: &egui::Context) {
        self.splash = None;
        self.fit_pending = false;
        match &mut self.app {
            Some(app) => {
                app.reset_graphics();
                app.ui.status = "The graphics driver reset: FreeDAWm opened a new window. The session and audio carried on.".into();
            }
            None => self.load(ctx),
        }
        log::info!("graphics recovered: a new window and device");
    }

    /// The main window's size and state now (for a new window).
    fn window_now(&self) -> ([f32; 2], bool) {
        self.app.as_ref().map_or(self.window, |a| (sane_window_size(a.ui.window_size), a.ui.window_maximized))
    }

    /// The open session's file, for a process started in this one's place.
    fn session_path(&self) -> Option<String> {
        self.app.as_ref().and_then(|a| a.engine.path.clone())
    }
}

/// What eframe runs: the [`App`], kept outside the run (`main` owns it) so it outlives a run
/// ended by a graphics device loss, and the watch on this run's device (see `gpu`).
struct Host {
    app: std::rc::Rc<std::cell::RefCell<Option<App>>>,
    watch: std::sync::Arc<gpu::GpuWatch>,
    /// This run's device, for `--simulate-device-loss`.
    device: Option<eframe::wgpu::Device>,
    simulate_at: Option<std::time::Instant>,
    /// The device was lost: nothing is drawn any more (no buffer may grow on a lost device) and
    /// the window is closing. Shared with `main`, which starts the next run.
    lost: std::rc::Rc<std::cell::Cell<bool>>,
}

impl Host {
    fn with_app(&self, f: impl FnOnce(&mut App)) {
        if let Some(app) = self.app.borrow_mut().as_mut() {
            f(app);
        }
    }

    fn begin_recovery(&mut self, ctx: &egui::Context) {
        self.lost.set(true);
        self.with_app(|app| {
            app.before_gpu_restart();
        });
        ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        ctx.request_repaint();
    }
}

impl eframe::App for Host {
    fn logic(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        if self.lost.get() {
            ctx.request_repaint();
            return;
        }
        if let Some(at) = self.simulate_at {
            if std::time::Instant::now() >= at {
                self.simulate_at = None;
                log::warn!("simulating a graphics device loss ({})", gpu::SIMULATE_FLAG);
                self.watch.mark_lost("Simulated", "Device::destroy() for --simulate-device-loss");
                if let Some(device) = &self.device {
                    device.destroy();
                }
            } else {
                ctx.request_repaint_after(at.saturating_duration_since(std::time::Instant::now()));
            }
        }
        if self.watch.is_lost() {
            self.begin_recovery(ctx);
            return;
        }
        self.with_app(|app| app.logic(ctx));
    }
    fn raw_input_hook(&mut self, _ctx: &egui::Context, raw: &mut egui::RawInput) {
        if !self.lost.get() {
            self.with_app(|app| app.raw_input_hook(raw));
        }
    }
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        // After a loss nothing is drawn: egui-wgpu panics when a buffer must grow on a lost device.
        if !self.lost.get() {
            self.with_app(|app| app.ui(ui));
        }
    }
    fn on_exit(&mut self) {
        // A run closed for a lost device keeps everything for the next run.
        if !self.lost.get() {
            self.with_app(App::shutdown);
        }
    }
}

fn open_path(app: &mut SoundApp, p: &str) {
    let lower = p.to_ascii_lowercase();
    let r = if soundcraft_engine::model::is_session_path(&lower) {
        dialogs::run_or_ask(app, "session.open", serde_json::json!({"path": p}))
    } else if lower.ends_with(".mid") || lower.ends_with(".midi") || lower.ends_with(".smf") {
        app.run("file.import_midi", serde_json::json!({"path": p}))
    } else {
        let at = app.engine.session().edit.selection.start;
        app.run("file.import_audio", serde_json::json!({"path": p, "at": at}))
    };
    if let Err(e) = r {
        log::warn!("{p}: {e}");
    }
}

/// The per-user settings directory: `ui.json`, `Autosave/`, `Presets/` and `logs/` live here.
fn config_dir() -> Option<std::path::PathBuf> {
    config::config_dir()
}

fn prefs_path() -> Option<std::path::PathBuf> {
    config_dir().map(|b| b.join("ui.json"))
}

/// Runs without preferences (`FREEDAWM_NO_PREFS`, or the older `SOUNDCRAFT_NO_PREFS`; agents' test
/// runs) neither read nor write them.
fn prefs_enabled() -> bool {
    soundcraft_engine::brand::env_var_os("NO_PREFS").is_none()
}

fn load_prefs() -> Option<UiState> {
    if !prefs_enabled() {
        return None;
    }
    let path = prefs_path()?;
    match config::load_ui(&path) {
        Ok(state) => state,
        Err(error) => {
            log::warn!("{error}");
            None
        }
    }
}

fn save_prefs(ui: &UiState) {
    if !prefs_enabled() {
        return;
    }
    if let Some(path) = prefs_path()
        && let Err(error) = config::save_ui(&path, ui)
    {
        log::warn!("{error}");
    }
}

fn services() -> Services {
    let mut services = Services {
        pick_open: Some(Box::new(|purpose: &str, _exts: &[&str]| {
            let d = rfd::FileDialog::new();
            let d = match purpose {
                "session.open" => d.add_filter("FreeDAWm Session", &soundcraft_engine::model::SESSION_EXTENSIONS),
                "file.import_midi" => d.add_filter("MIDI", &["mid", "midi", "smf"]),
                "file.import_dawproject" => d.add_filter("DAWproject", &["dawproject"]),
                "scout.profile_import" => d.add_filter("Sound Profile", &["json"]),
                "track.load_preset" | "track.load_chain" => d.add_filter("Preset", &["json"]),
                _ => d.add_filter(
                    "Audio",
                    &[
                        "wav", "wave", "bwf", "aif", "aiff", "aifc", "flac", "mp3", "ogg", "oga", "opus", "m4a", "m4b", "aac", "mp4", "caf", "mkv",
                        "webm",
                    ],
                ),
            };
            d.pick_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_save: Some(Box::new(|purpose: &str, default: &str| {
            let path = std::path::Path::new(default);
            let mut d = match purpose {
                "file.bounce_mix" => rfd::FileDialog::new().add_filter("Audio", &["wav", "aif", "aiff", "flac", "mp3", "opus"]),
                "file.export_dawproject" => rfd::FileDialog::new().add_filter("DAWproject", &["dawproject"]),
                "scout.starting_prompt" => rfd::FileDialog::new().add_filter("Markdown", &["md"]),
                "scout.profile_export" => rfd::FileDialog::new().add_filter("Sound Profile", &["json"]),
                _ => rfd::FileDialog::new().add_filter("FreeDAWm Session", &[soundcraft_engine::model::SESSION_EXTENSION]),
            };
            if let Some(dir) = path.parent().filter(|dir| dir.is_dir()) {
                d = d.set_directory(dir);
            }
            if let Some(n) = path.file_name().and_then(|n| n.to_str()) {
                d = d.set_file_name(n);
            }
            d.save_file().map(|p| p.to_string_lossy().into_owned())
        })),
        pick_folder: Some(Box::new(|start: &str| {
            rfd::FileDialog::new().set_directory(start).pick_folder().map(|p| p.to_string_lossy().into_owned())
        })),
        // The plugin browser's pictures of hosted plugins (a cache: safe to delete).
        thumb_dir: config_dir().map(|d| d.join("PluginThumbs")),
        // Scout's sound profiles, shared by every project.
        profiles_dir: config_dir().map(|d| d.join("profiles")),
        // Setup › Network (the LAN remote): its own settings file, never in a session.
        network_file: config_dir().map(|d| d.join("network.json")),
        ..Services::default()
    };
    // Scout asks Claude with the owner's key, kept in its own settings file (never the session).
    claude::install(&mut services, claude::key_path());
    services
}

/// The window, Dock, taskbar and Alt-Tab icon. macOS gets Apple's icon grid (transparent margin);
/// elsewhere the full tile. Regenerate with `packaging/icons.sh`.
fn app_icon() -> Option<egui::IconData> {
    #[cfg(target_os = "macos")]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/freedawm-macos-512.png");
    #[cfg(not(target_os = "macos"))]
    let png: &[u8] = include_bytes!("../../../assets/app-icon/hicolor/256x256/apps/io.github.s0lidr0ck.FreeDAWm.png");
    eframe::icon_data::from_png_bytes(png).map_err(|e| log::warn!("app icon: {e}")).ok()
}

fn main() -> eframe::Result {
    // First, so every start-up record (and the engine's panic hook, installed with the first
    // Engine) is captured; see `logging`.
    let logger = logging::install();
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.iter().any(|a| a == "--version" || a == "-V") {
        println!("FreeDAWm {}", env!("CARGO_PKG_VERSION"));
        return Ok(());
    }
    let demo = args.iter().any(|a| a == "--demo" || a == "--sample");
    let no_audio = args.iter().any(|a| a == "--no-audio");
    let control_port: Option<u16> = args
        .iter()
        .position(|a| a == "--control")
        .and_then(|i| args.get(i + 1))
        .and_then(|p| p.parse().ok())
        .or_else(|| soundcraft_engine::brand::env_var("CONTROL_PORT").and_then(|p| p.parse().ok()));
    let files: Vec<String> = args
        .iter()
        .enumerate()
        .filter(|(i, a)| !(a.starts_with("--") || (i > &0 && args.get(i - 1).is_some_and(|p| p == "--control"))))
        .map(|(_, a)| a.clone())
        .collect();
    log::info!("FreeDAWm {} ({} {})", env!("CARGO_PKG_VERSION"), std::env::consts::OS, std::env::consts::ARCH);
    // The log file lives in the settings directory; opened after the arguments, so `--version`
    // leaves no file behind. Records logged until now are written to it first. Runs without
    // preferences (agents' test runs) log to standard error only, so they don't rotate away the
    // user's own logs.
    if let Some(logger) = logger {
        match config_dir() {
            _ if !prefs_enabled() => logger.no_file(),
            Some(dir) => match logger.attach_dir(&dir.join(logging::LOG_DIR)) {
                Ok(path) => log::info!("log file {}", path.display()),
                Err(e) => log::warn!("no log file: {e}"),
            },
            None => {
                logger.no_file();
                log::warn!("no log file: no settings directory (profile override is empty or platform configuration is unavailable)");
            }
        }
    }

    // The plugin scan cache (the plugin lists and where each plugin lives; scans run out of
    // process, see `soundcraft-plugin-scan`). Runs without preferences keep it in memory.
    soundcraft_engine::set_plugin_scan_cache_dir(if prefs_enabled() { config_dir().map(|d| d.join("PluginScan")) } else { None });
    let mut prefs = load_prefs().unwrap_or_default();
    let window = (sane_window_size(prefs.window_size), prefs.window_maximized);
    // A process started after a graphics reset its predecessor couldn't recover from (see
    // `gpu`): straight to the window, saying so.
    let gpu_restarted = args.iter().any(|a| a == gpu::RESTARTED_FLAG);
    let show_splash = !gpu_restarted && splash::wanted(&args, prefs.show_splash, &|name| std::env::var_os(name));
    // One encouraging statement over the film, never the last launch's (remembered in `ui.json`).
    let statement = if show_splash { splash::choose_statement(&mut prefs.splash.last_statement) } else { None };
    // With the splash, the root window opens as the splash (borderless, the film's size) and
    // `App::end_splash` turns it into the main window; see `splash`.
    let viewport = if show_splash { splash::viewport() } else { main_viewport(window) };
    let (sessions, others): (Vec<String>, Vec<String>) =
        files.into_iter().partition(|f| soundcraft_engine::model::is_session_path(f.to_ascii_lowercase()));
    let audio_first = audio_before_engine(&sessions);
    let launch_rate = sessions.last().cloned();
    let load_progress = std::sync::Arc::new(std::sync::Mutex::new(soundcraft_engine::open_job::OpenProgress::default()));
    let waker = control_server::Waker::default();
    let (prepare, audio, finish) = loaders(Launch {
        demo,
        no_audio,
        sessions,
        prefs,
        control_port,
        behind_splash: show_splash,
        progress: std::sync::Arc::clone(&load_progress),
        waker: waker.clone(),
        gpu_restarted,
    });
    let app = App {
        app: None,
        loading: Loading::Pending(prepare, audio, finish),
        load_progress,
        audio_first,
        launch_rate: (!show_splash).then(|| launch_rate.as_deref().and_then(soundcraft_engine::open_job::sample_rate_of)),
        splash: show_splash.then(|| splash::Splash::with_statement(statement)),
        dropped: others,
        window,
        fit_pending: false,
        fit_passes: 0,
        logger,
        #[cfg(target_os = "macos")]
        menu: None,
    };
    run(app, native_options(viewport), &args, &waker)
}

/// Runs the app: one run of eframe's event loop per window and graphics device. A run ended by a
/// device loss (or by a panic it caused) is followed by another with the same app, its session
/// and audio untouched; when that can't be done the session is saved, the user told, and the
/// process restarted or ended (see `gpu::Recovery`).
fn run(app: App, first_options: eframe::NativeOptions, args: &[String], waker: &control_server::Waker) -> eframe::Result {
    use std::rc::Rc;
    let slot = Rc::new(std::cell::RefCell::new(Some(app)));
    let mut first_options = Some(first_options);
    let mut simulate = gpu::simulate(args);
    let mut recovery = gpu::Recovery::default();
    loop {
        let first = first_options.is_some();
        let options = first_options.take().unwrap_or_else(|| {
            let window = slot.borrow().as_ref().map_or(([1600.0, 1000.0], false), App::window_now);
            native_options(main_viewport(window))
        });
        let watch = gpu::GpuWatch::new();
        let lost = Rc::new(std::cell::Cell::new(false));
        let started = Rc::new(std::cell::Cell::new(false));
        let creator: eframe::AppCreator<'_> = {
            let (slot, watch, lost, started, waker) =
                (Rc::clone(&slot), std::sync::Arc::clone(&watch), Rc::clone(&lost), Rc::clone(&started), waker.clone());
            // Once, unless asked to repeat (to try the give-up path).
            let simulate = if simulate.is_some_and(|s| s.repeat) { simulate } else { simulate.take() };
            Box::new(move |cc| {
                started.set(true);
                if let Some(render) = &cc.wgpu_render_state {
                    watch.watch(render, &cc.egui_ctx);
                }
                waker.set(&cc.egui_ctx);
                if let Some(app) = slot.borrow_mut().as_mut() {
                    if !first {
                        app.after_gpu_restart(&cc.egui_ctx);
                    } else if app.splash.is_some() {
                        // The statement's font, bound before the first frame (the UI's own fonts come later).
                        cc.egui_ctx.add_font(soundcraft_ui_egui::fonts::splash_font());
                    } else {
                        app.load(&cc.egui_ctx);
                    }
                }
                let simulate_at = simulate.map(|s| std::time::Instant::now() + s.after);
                let device = cc.wgpu_render_state.as_ref().map(|r| r.device.clone());
                Ok(Box::new(Host { app: slot, watch, device, simulate_at, lost }))
            })
        };
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| eframe::run_native_ext("FreeDAWm", options, None, creator)));
        if started.get() {
            recovery.started();
        }
        let end = match &result {
            Ok(Ok(())) if lost.get() => gpu::RunEnd::DeviceLost,
            Ok(Ok(())) => gpu::RunEnd::Quit,
            Ok(Err(e)) if watch.is_lost() => {
                log::error!("the window closed after the graphics device was lost: {e}");
                gpu::RunEnd::DeviceLost
            }
            Ok(Err(e)) => {
                log::error!("no window or graphics device: {e}");
                gpu::RunEnd::StartFailed
            }
            Err(payload) => {
                let text = gpu::panic_text(payload.as_ref());
                gpu::RunEnd::Panicked { gpu: watch.is_lost() || gpu::is_gpu_panic(&text) }
            }
        };
        let next = recovery.next(&end, std::time::Instant::now());
        log::info!("the window's run ended ({end:?}, device loss: {:?}); next: {next:?}", watch.loss());
        match next {
            gpu::Next::Exit => return result.unwrap_or(Ok(())),
            gpu::Next::Run { delay } => {
                if let gpu::RunEnd::Panicked { .. } = end
                    && let Some(app) = slot.borrow_mut().as_mut()
                {
                    // The panic stopped the frame before `Host` could prepare the app.
                    app.before_gpu_restart();
                }
                std::thread::sleep(delay);
            }
            gpu::Next::ResumePanic => {
                // Not a graphics problem: the session (with the plugins' state) is autosaved and
                // the panic goes on, as before; the next launch offers the unsaved work.
                if let Some(mut app) = slot.borrow_mut().take() {
                    app.shutdown();
                }
                if let Err(payload) = result {
                    std::panic::resume_unwind(payload);
                }
                return Ok(());
            }
            gpu::Next::Relaunch | gpu::Next::GiveUp => {
                let mut app = slot.borrow_mut().take();
                let session = app.as_ref().and_then(App::session_path);
                let saved = app.as_mut().is_none_or(App::before_gpu_restart);
                // The audio device is closed before a new process wants it (ASIO allows one).
                if let Some(app) = &mut app {
                    app.shutdown();
                }
                drop(app);
                let message = gpu::farewell(&next, saved);
                log::error!("{}", message.replace('\n', " "));
                rfd::MessageDialog::new()
                    .set_level(rfd::MessageLevel::Warning)
                    .set_title("FreeDAWm")
                    .set_description(&message)
                    .set_buttons(rfd::MessageButtons::Ok)
                    .show();
                if next == gpu::Next::Relaunch {
                    relaunch(args, session.as_deref());
                }
                return Ok(());
            }
        }
    }
}

/// Starts FreeDAWm again in this process's place (see `gpu::relaunch_args`).
fn relaunch(args: &[String], session: Option<&str>) {
    let spawned = std::env::current_exe().and_then(|exe| std::process::Command::new(exe).args(gpu::relaunch_args(args, session)).spawn());
    match spawned {
        Ok(child) => log::info!("restarted FreeDAWm (process {})", child.id()),
        Err(e) => log::error!("couldn't restart FreeDAWm: {e}"),
    }
}

/// The main window (without the splash, and after a graphics device loss).
fn main_viewport((size, maximized): ([f32; 2], bool)) -> egui::ViewportBuilder {
    egui::ViewportBuilder::default()
        .with_title("FreeDAWm")
        .with_inner_size(size)
        .with_maximized(maximized)
        .with_min_inner_size(MAIN_MIN_SIZE)
        .with_app_id("io.github.s0lidr0ck.FreeDAWm")
}

/// eframe's options for a window: its icon, file drops and the graphics backend.
fn native_options(viewport: egui::ViewportBuilder) -> eframe::NativeOptions {
    let mut options = eframe::NativeOptions { viewport: viewport.with_drag_and_drop(true), ..Default::default() };
    if let Some(icon) = app_icon() {
        options.viewport = options.viewport.with_icon(icon);
    }
    // Before eframe creates the wgpu instance: default Windows to DirectX 12 with FXC (see graphics.rs).
    #[cfg(target_os = "windows")]
    graphics::configure(&mut options, eframe::wgpu::Backends::from_env(), eframe::wgpu::Dx12Compiler::from_env());
    options
}

/// What a launch asked for.
struct Launch {
    demo: bool,
    no_audio: bool,
    /// Session files on the command line: the last one opens (the others are noted); the
    /// audio and MIDI files (`App::dropped`) import once it is open.
    sessions: Vec<String>,
    prefs: UiState,
    control_port: Option<u16>,
    /// Behind the splash the session loads on the engine's thread (its phase shown under the
    /// statement) and its plugins are made while the film plays; without one it opens in the
    /// main window behind the progress window, like any File › Open.
    behind_splash: bool,
    /// Where the loading stands, for the splash.
    progress: std::sync::Arc<std::sync::Mutex<soundcraft_engine::open_job::OpenProgress>>,
    /// What the control channel wakes (the current window's context).
    waker: control_server::Waker,
    /// Started in place of a process a graphics reset ended (`--gpu-restarted`).
    gpu_restarted: bool,
}

/// Loading, in three parts: the engine, with the command-line session loaded behind the splash
/// (thread-safe; on a thread so the film keeps painting), the audio device (UI thread, for the
/// session it is given), then the app and the control channel (UI thread, once the engine is
/// ready), where the session's plugin instances are made and it is installed (`SoundApp::open`).
/// Without a splash all three run before the first frame, as they always did, and the session
/// opens in the main window behind the progress window.
fn loaders(launch: Launch) -> (Prepare, Audio, Finish) {
    use soundcraft_engine::open_job::{OpenRequest, load_session};
    let Launch { demo, no_audio, sessions, prefs, control_port, behind_splash, progress, waker, gpu_restarted } = launch;
    let (last, skipped) = match sessions.split_last() {
        Some((last, rest)) => (Some(last.clone()), rest.to_vec()),
        None => (None, Vec::new()),
    };
    let opened_before = last.is_some();
    let loads_behind_splash = behind_splash && last.is_some();
    let to_load = loads_behind_splash.then(|| last.clone()).flatten();
    let prepare: Prepare = Box::new(move || {
        let mut engine = if demo { soundcraft_engine::demo::demo_engine() } else { Engine::default() };
        // The Band's user folder (players, targets, band and feel presets); runs without preferences
        // leave the user's folder alone and play the embedded Library.
        engine.set_band_user_folder(if prefs_enabled() { config_dir().map(|d| d.join("Band")) } else { None });
        let mut errors: Vec<String> = skipped.iter().map(|p| format!("{p}: only the last session named opens")).collect();
        let mut loaded = None;
        if let Some(path) = to_load {
            match load_session(OpenRequest::Open { path: path.clone() }, &mut |p| {
                *progress.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = p;
            }) {
                Ok(l) => loaded = Some(l),
                Err(e) => {
                    log::warn!("{path}: {e}");
                    errors.push(format!("{path}: {e}"));
                }
            }
        }
        Prepared { engine, errors, loaded }
    });
    let audio_config = prefs.audio.clone();
    let audio: Audio = Box::new(move |session| (!no_audio).then(|| Box::new(soundcraft_playback::Player::with_config(session, &audio_config))));
    let finish: Finish = Box::new(move |ctx: &egui::Context, prepared: Prepared, player: Option<Box<soundcraft_playback::Player>>| {
        let Prepared { engine, errors, loaded } = prepared;
        if let Some(player) = &player {
            // The device opened on the empty session (or one at the loaded session's rate): hand
            // the engine's over, as a session open does; the loaded one follows through the open.
            player.update_session(engine.session_arc());
        }
        let mut app = SoundApp::new(engine, player.map(|p| *p), services());
        if opened_before {
            app.note_session_opened_before();
        }
        app.autosave_dir = prefs_path().and_then(|p| p.parent().map(|d| d.join("Autosave")));
        // Audio a never-saved session makes goes beside its takes in the autosave folder.
        app.engine.unsaved_audio_root = app.autosave_dir.clone();
        app.leftovers = app.autosave_dir.as_deref().map(soundcraft_engine::autosave::leftovers).unwrap_or_default();
        app.extra.ui_scale = prefs.ui_scale;
        app.ui = prefs;
        // The Library (plugin presets, chains, track presets, Juke presets, templates): the
        // settings folder, unless the preferences name another.
        app.library_default = prefs_path().and_then(|p| p.parent().map(std::path::Path::to_path_buf));
        app.apply_library_dir();
        // The MIDI input ports (Setup › MIDI Input Devices), with the audio.
        app.use_system_midi = !no_audio;
        if !errors.is_empty() {
            app.ui.status = errors.join("  ");
        }
        if gpu_restarted {
            app.ui.status = "The graphics driver reset and FreeDAWm restarted. Unsaved work is offered for recovery.".into();
        }
        waker.set(ctx);
        if let Some(port) = control_port {
            let rx = control_server::start(port, waker.clone());
            app = app.with_control(rx);
        }
        // The launch session: loaded behind the splash, its plugins and install follow in the
        // app; without a splash it opens behind the progress window (the usual path).
        if let Some(loaded) = loaded {
            let name = loaded.path.as_deref().map(|p| OpenRequest::Open { path: p.to_string() }.display_name()).unwrap_or_default();
            soundcraft_ui_egui::open_progress::start_loaded(&mut app, "session.open", name, loaded);
        } else if let Some(path) = last.filter(|_| !loads_behind_splash)
            && let Err(e) = soundcraft_ui_egui::dialogs::run_or_ask(&mut app, "session.open", serde_json::json!({"path": path}))
        {
            log::warn!("{path}: {e}");
        }
        app
    });
    (prepare, audio, finish)
}

/// A saved window size, or the default when the prefs file holds nonsense (NaN, negative, tiny).
fn sane_window_size([w, h]: [f32; 2]) -> [f32; 2] {
    if w.is_finite() && h.is_finite() && w >= 960.0 && h >= 600.0 { [w.min(16_384.0), h.min(16_384.0)] } else { [1600.0, 1000.0] }
}

#[cfg(test)]
mod tests {
    use super::{FIT_PASS_BUDGET, Launch, after_splash, audio_before_engine, loaders, main_window_commands, set_title};
    use egui::vec2;

    #[test]
    fn a_session_launched_by_argument_opens_the_device_at_its_rate() {
        // A 44.1 kHz session file.
        let dir = std::env::temp_dir().join(format!("soundcraft-launch-rate-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("fortyfour.fdawm").to_string_lossy().into_owned();
        let mut e = soundcraft_engine::Engine::default();
        e.execute("session.new", &serde_json::json!({"name": "fortyfour", "sample_rate": 44100})).unwrap();
        e.execute("session.save", &serde_json::json!({"path": path})).unwrap();
        // Launched by argument: the device waits for the engine and opens for the loaded session.
        let sessions = vec![path.clone()];
        assert!(!audio_before_engine(&sessions));
        assert!(audio_before_engine(&[]));
        let progress = std::sync::Arc::new(std::sync::Mutex::new(soundcraft_engine::open_job::OpenProgress::default()));
        let (prepare, audio, _finish) = loaders(Launch {
            demo: false,
            no_audio: false,
            sessions,
            prefs: super::UiState::default(),
            control_port: None,
            behind_splash: true,
            progress: std::sync::Arc::clone(&progress),
            waker: super::control_server::Waker::default(),
            gpu_restarted: false,
        });
        let prepared = prepare();
        assert!(prepared.errors.is_empty(), "{:?}", prepared.errors);
        let loaded = prepared.loaded.as_ref().expect("loaded behind the splash, not installed yet");
        assert_eq!(loaded.session.sample_rate.hz(), 44_100);
        assert!(prepared.engine.session().tracks.is_empty(), "the engine is empty until the app installs it");
        assert_ne!(progress.lock().unwrap().phase, soundcraft_engine::open_job::OpenPhase::Plugins, "the thread reports its phases");
        // Without a splash the file's rate is read ahead for the device.
        assert_eq!(soundcraft_engine::open_job::sample_rate_of(&path).map(|r| r.hz()), Some(44_100));
        let player = audio(std::sync::Arc::new(soundcraft_engine::model::Session::new(String::new(), loaded.session.sample_rate))).unwrap();
        assert_eq!(player.session_rate(), 44_100);
        // The stream opens at the session's rate (no resampler) whenever the device offers it; a
        // device fixed at another rate (WASAPI shared mode at its mix rate) cannot, and the silent
        // clock runs at the session's rate.
        let offers_44100 = soundcraft_playback::devices::audio_systems()
            .iter()
            .filter(|s| s.name == player.system)
            .flat_map(|s| &s.outputs)
            .any(|d| d.name == player.device_name && d.sample_rates.contains(&44_100));
        if offers_44100 || player.device_channels == 0 {
            assert_eq!(player.device_rate, 44_100, "the stream opened at the session's rate: no resampler");
        } else {
            println!("SKIPPED the rate check: {} ({}) does not offer 44.1 kHz", player.device_name, player.system);
        }
        // A session that fails to open is reported for the status line.
        let (prepare, _, _) = loaders(Launch {
            demo: false,
            no_audio: true,
            sessions: vec![dir.join("missing.fdawm").to_string_lossy().into_owned()],
            prefs: super::UiState::default(),
            control_port: None,
            behind_splash: true,
            progress,
            waker: super::control_server::Waker::default(),
            gpu_restarted: false,
        });
        let prepared = prepare();
        assert_eq!(prepared.errors.len(), 1, "{:?}", prepared.errors);
        assert!(prepared.errors[0].contains("missing.fdawm"));
        assert!(prepared.loaded.is_none());
        drop(player);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn after_the_splash_the_wait_for_the_window_size_always_ends() {
        let monitor = Some(vec2(1728.0, 1117.0));
        // Still the splash's size: waiting (the resize is asynchronous).
        assert_eq!(after_splash(Some(vec2(960.0, 540.0)), monitor, 1, false), None);
        assert_eq!(after_splash(None, monitor, 1, false), None);
        // The saved size arrived: over, nothing to maximize.
        assert_eq!(after_splash(Some(vec2(1600.0, 1000.0)), monitor, 2, false), Some(false));
        // A maximized restore: the work area's size, never a second Maximized.
        assert_eq!(after_splash(Some(vec2(1728.0, 1069.0)), monitor, 1, true), Some(false));
        assert_eq!(after_splash(Some(vec2(2400.0, 1400.0)), monitor, 1, true), Some(false));
        // Bigger than the screen (the WM allowed it, or clamped it short of the saved size).
        assert_eq!(after_splash(Some(vec2(2400.0, 1000.0)), monitor, 1, false), Some(true));
        // A zoomed window reports its size in zoom-independent points: the caller multiplies.
        assert_eq!(after_splash(Some(vec2(1280.0, 800.0) * 1.25), monitor, 3, false), Some(false));
        // A window manager that refused the resize: the wait still ends within the budget.
        assert_eq!(after_splash(Some(vec2(960.0, 540.0)), monitor, FIT_PASS_BUDGET - 1, false), None);
        assert_eq!(after_splash(Some(vec2(960.0, 540.0)), monitor, FIT_PASS_BUDGET, false), Some(false));
        assert_eq!(after_splash(None, None, FIT_PASS_BUDGET, false), Some(false));
    }

    #[test]
    fn the_main_window_clears_the_splash_maximum_before_raising_the_minimum() {
        use egui::ViewportCommand;
        // The splash's window is pinned at 960×540 (min = max); Wayland rejects a minimum taller
        // than the maximum, so the maximum goes before the main window's 960×600 minimum.
        let cmds = main_window_commands(vec2(1600.0, 1000.0), false, Some(vec2(2560.0, 1440.0)));
        let position = |want: &dyn Fn(&ViewportCommand) -> bool| cmds.iter().position(want).unwrap_or_else(|| panic!("missing in {cmds:?}"));
        let resizable = position(&|c| *c == ViewportCommand::Resizable(true));
        let max = position(&|c| *c == ViewportCommand::MaxInnerSize(egui::Vec2::INFINITY));
        let min = position(&|c| *c == ViewportCommand::MinInnerSize(super::MAIN_MIN_SIZE));
        let size = position(&|c| *c == ViewportCommand::InnerSize(vec2(1600.0, 1000.0)));
        assert!(resizable < max && max < min && min < size, "{cmds:?}");
        // Centred on the screen; no Maximized unless saved so.
        assert!(cmds.contains(&ViewportCommand::OuterPosition(egui::pos2(480.0, 220.0))), "{cmds:?}");
        assert!(!cmds.iter().any(|c| matches!(c, ViewportCommand::Maximized(_))), "{cmds:?}");
        let cmds = main_window_commands(vec2(1600.0, 1000.0), true, None);
        assert!(cmds.contains(&ViewportCommand::Maximized(true)), "{cmds:?}");
        assert!(!cmds.iter().any(|c| matches!(c, ViewportCommand::OuterPosition(_))), "{cmds:?}");
    }

    #[test]
    fn the_title_is_sent_only_when_it_changes() {
        let ctx = egui::Context::default();
        let titles = |title: &str| {
            let mut out = ctx.run_ui(egui::RawInput::default(), |ui| set_title(ui.ctx(), title.to_string()));
            out.textures_delta.clear();
            out.viewport_output.values().flat_map(|v| &v.commands).filter(|c| matches!(c, egui::ViewportCommand::Title(_))).count()
        };
        assert_eq!(titles("Song — FreeDAWm"), 1);
        assert_eq!(titles("Song — FreeDAWm"), 0);
        assert_eq!(titles("Song * — FreeDAWm"), 1);
    }
}

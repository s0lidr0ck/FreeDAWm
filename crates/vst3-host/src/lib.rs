//! SoundCraft's VST3 plugin host.
//!
//! Scans the standard VST3 folders (plus `VST3_PATH`) out of process (the `freedawm-scan`
//! helper, a few bundles per child process, see [`scan_with`]; the results are kept in a scan
//! cache), loads the `.vst3` bundle of each plugin it creates in-process and exposes each "Audio
//! Module Class" as a [`soundcraft_dsp::Plugin`], so the mix engine treats
//! third-party plugins exactly like the built-in ones. Plugin ids are `vst3:<class id hex>` (32
//! upper-case hex digits of the component's class id); parameter ids are VST3 `ParamID`s as
//! decimal strings, with titles, units and plain ranges from the edit controller in
//! [`PluginInfo::params`]. Values are plain (display) values; the host converts to and from the
//! VST3 normalized 0..1 values. Stepped parameters become toggles or choices.
//!
//! Like `soundcraft-clap-host`, this is an isolated `unsafe` crate (craftrules never-crash):
//! `unsafe` is denied crate-wide and allowed only in the private `ffi` module. The public API is
//! safe, returns `Result`/`Option` and never panics. Limits of in-process hosting: a plugin that
//! itself crashes (segfault) takes the process down with it; we cannot sandbox native code.
//!
//! `PluginInfo` must be `&'static`: the registry leaks exactly one `PluginInfo` (with its param
//! table) per plugin *type* the first time that type is instantiated, and caches it. The leak is
//! bounded by the number of distinct installed plugins actually used. Loaded binaries likewise
//! stay mapped for the life of the process (module exit functions are never called).
//!
//! Plugin state: [`Plugin::save_state`] stores the component state and the controller state in
//! one blob (`SCV3` + length-prefixed parts); [`Plugin::load_state`] restores both (a bare
//! component state is accepted too) and re-reads the parameter values.
//!
//! Plugin editors (`IPlugView`): on macOS the view is attached to the content `NSView` of a host
//! `NSWindow` (titled and closable, floating above the app, resized on `IPlugFrame::resizeView`);
//! on Windows it is attached to a host `HWND` owned by the app's window (no taskbar button,
//! closing it hides it until the link tears it down), and the handle's `idle` keeps it drawing
//! while the app repaints continuously during playback (its `WM_TIMER`s are dispatched and its
//! pending paints run; see `window::service`).
//! [`Plugin::open_editor`] opens it directly; [`Plugin::editor`] returns a handle usable on the
//! main thread while the instance processes on the audio thread (VST3's own threading model:
//! the controller and its views live on the UI thread). Parameter edits made in the editor
//! (`IComponentHandler::performEdit`) are reported by the handle's `idle` so the host can write
//! them into the session. Linux (X11) editor windows are not hosted yet: opening reports
//! "unsupported".
//!
//! Processor ↔ controller messaging: `IHostApplication::createInstance` makes `IMessage` and
//! `IAttributeList` objects, and each side is connected to a host proxy whose target is the other
//! side, so a message either side sends off the main thread (the processor from the audio
//! thread, the controller from a GUI or loader thread) is queued and delivered to the other
//! side on the UI thread by the editor handle's `idle`, which also consumes the
//! `restartComponent` flags: `kParamValuesChanged` makes `idle` report every parameter's
//! current value (so the session follows a preset loaded in the plugin's own GUI),
//! `kLatencyChanged` re-reads the latency, `kIoChanged` is logged. Both therefore depend on the
//! host calling `idle` each UI frame (the player does, for every instance with a handle, window
//! open or not); headless hosts (the CLI and MCP, a render's copies) never call it, so there
//! off-thread messages stay queued and restart flags unread, which a render playing the
//! session's values does not need. Offline renders set up the processor in `kOffline` mode
//! ([`Plugin::set_offline`]).
//!
//! A plugin's first auxiliary audio input is its sidechain: activated with the main buses and fed
//! the key ([`Plugin::process_with_sidechain`]); other aux buses stay inactive. Not hosted yet:
//! 64-bit processing.

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod scan;
pub mod vstpreset;

#[allow(unsafe_code)]
mod ffi;
mod plugin;
mod registry;

pub use plugin::Vst3Plugin;

use soundcraft_dsp::{Category, Plugin, PluginInfo};
use std::path::{Path, PathBuf};

pub use soundcraft_plugin_scan::{ScanOptions, ScanProgress};

impl soundcraft_plugin_scan::Descriptor for Vst3Descriptor {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
}

/// Prefix of every hosted-VST3 plugin id.
pub const ID_PREFIX: &str = "vst3:";

/// The factory class category of audio processors (the only classes SoundCraft lists).
pub const AUDIO_MODULE_CLASS: &str = "Audio Module Class";

/// The 16-byte class id inside a `vst3:<32 hex digits>` SoundCraft id (`None` for other ids).
pub fn parse_id(id: &str) -> Option<[u8; 16]> {
    let hex = id.strip_prefix(ID_PREFIX)?;
    if hex.len() != 32 || !hex.is_ascii() {
        return None;
    }
    let mut out = [0u8; 16];
    for (i, o) in out.iter_mut().enumerate() {
        let pair = hex.get(i * 2..i * 2 + 2)?;
        *o = u8::from_str_radix(pair, 16).ok()?;
    }
    Some(out)
}

/// One plugin found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct Vst3Descriptor {
    /// SoundCraft id: `vst3:<class_id>`.
    pub id: String,
    /// The component's class id as 32 upper-case hex digits.
    pub class_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    /// The VST3 SDK version the plugin was built with (when reported).
    pub sdk_version: String,
    /// The `|`-separated sub-categories, split (`["Fx", "Reverb"]`).
    pub sub_categories: Vec<String>,
    pub category: Category,
    pub is_instrument: bool,
    /// The `.vst3` bundle or file it lives in.
    pub path: PathBuf,
    /// What it offers for ARA (Audio Random Access), when its binary has an ARA main factory
    /// class matching it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ara: Option<Vst3Ara>,
    /// Scanned by a version that looks for ARA (older scan results are scanned again once).
    #[serde(default)]
    pub ara_checked: bool,
    /// Why the scan found no ARA here (empty when `ara` is set), for the ARA log.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub ara_note: String,
}

/// What a VST3 plug-in offers for ARA 2 (from its `ARA::IMainFactory` and its audio module's
/// ARA entry interfaces), found by the scan without initialising ARA or the plug-in.
#[derive(Debug, Clone, PartialEq, Default, serde::Serialize, serde::Deserialize)]
pub struct Vst3Ara {
    /// The "ARA Main Factory Class" id (32 upper-case hex digits).
    pub main_class_id: String,
    /// The ARA factory's own name, maker and version.
    pub name: String,
    pub manufacturer: String,
    pub version: String,
    /// Unique, versioned id of its ARA document controller.
    pub factory_id: String,
    /// The ARA API generations it supports.
    pub lowest_generation: i32,
    pub highest_generation: i32,
    /// FreeDAWm can host it (ARA 2, a usable factory, the ARA 2 entry point); else `problem`.
    pub usable: bool,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub problem: String,
    /// The roles its instances can be bound with ("playback renderer", "editor renderer",
    /// "editor view"): all three through `IPlugInEntryPoint2`; none without it.
    pub roles: Vec<String>,
    /// Its audio module also offers the ARA 1 binding (`IPlugInEntryPoint`).
    pub ara1_binding: bool,
    /// What it can analyse ("notes", "tempo", …).
    pub content: Vec<String>,
    /// The playback transformations it supports (`kARAPlaybackTransformation*` flags).
    pub transformations: i32,
}

/// The ARA roles of a plug-in instance bound through `IPlugInEntryPoint2`, as the scan lists them.
pub const ARA_ROLES: [&str; 3] = ["playback renderer", "editor renderer", "editor view"];

/// A plug-in's ARA factory, ready for [`soundcraft_ara_host::Document::new`]: holds the plug-in's
/// `ARA::IMainFactory` object (the factory is valid only while it lives) and its binary loaded.
/// Keep it for as long as any document made from it exists, on the thread the documents live on.
pub struct AraFactory {
    main: ffi::AraMain,
    factory: soundcraft_ara_host::FactoryPtr,
}

impl AraFactory {
    pub fn ptr(&self) -> soundcraft_ara_host::FactoryPtr {
        self.factory
    }
}

impl std::fmt::Debug for AraFactory {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "AraFactory({:#x}, kept {})", self.factory.addr(), self.main.factory().is_some())
    }
}

/// Loads the ARA factory of the `vst3:` plug-in `id` (in this process; its scan said it offers a
/// usable ARA factory). Fails with the reason.
pub fn ara_factory(id: &str) -> Result<AraFactory, Vst3Error> {
    let main = registry::ara_main(id)?;
    let factory = main.factory().ok_or_else(|| Vst3Error::Instantiate(id.to_string(), "no ARA factory".into()))?;
    Ok(AraFactory { main, factory })
}

/// The folders a scan looks in (the standard VST3 folders, `VST3_PATH`, added folders).
pub fn search_dirs() -> Vec<PathBuf> {
    registry::search_dirs()
}

/// One line per scanned VST3 module (and per file the scan could not use) saying whether it offers
/// ARA 2 and, if not, why: for the ARA log, so a plug-in missing from the ARA list can be told
/// apart from one never found. Never scans.
pub fn ara_scan_report() -> Vec<String> {
    let mut out: Vec<String> = cached()
        .unwrap_or_default()
        .iter()
        .map(|d| {
            let what = match &d.ara {
                Some(a) if a.usable => format!(
                    "ARA 2 found: \"{}\" {} {} (generations {} to {}; roles: {})",
                    a.name,
                    a.manufacturer,
                    a.version,
                    a.lowest_generation,
                    a.highest_generation,
                    if a.roles.is_empty() { "none".to_string() } else { a.roles.join(", ") }
                ),
                Some(a) => format!("ARA main factory found but not usable: {}", a.problem),
                None if !d.ara_checked => "scanned before ARA was looked for (scanned again by the next scan)".to_string(),
                None if d.ara_note.is_empty() => "no ARA".to_string(),
                None => format!("no ARA: {}", d.ara_note),
            };
            format!("VST3 {} ({} {}) in {}: {what}", d.name, d.vendor, d.version, d.path.display())
        })
        .collect();
    out.extend(scan_failures().into_iter().map(|(p, why)| format!("VST3 {}: not scanned: {why}", p.display())));
    out
}

/// The plug-ins the scan found to offer usable ARA 2 (without scanning).
pub fn ara_plugins() -> Vec<Vst3Descriptor> {
    cached().unwrap_or_default().into_iter().filter(|d| d.ara.as_ref().is_some_and(|a| a.usable)).collect()
}

/// A plugin's built-in programs (`IUnitInfo` program list), selected through its program-change
/// parameter.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Vst3Programs {
    /// The program-change parameter's id (as in the plugin's `PluginInfo::params`).
    pub param: String,
    /// The program names, in program order.
    pub names: Vec<String>,
    /// The parameter's value selecting each program (same order as `names`).
    pub values: Vec<f32>,
}

/// Why a VST3 operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum Vst3Error {
    #[error("cannot load VST3 plugin {0}: {1}")]
    Load(String, String),
    #[error("no VST3 plugin `{0}`")]
    NotFound(String),
    #[error("cannot create VST3 plugin `{0}`: {1}")]
    Instantiate(String, String),
    #[error("VST3 processing: {0}")]
    Process(String),
    #[error("VST3 plugins are not supported on this platform")]
    Unsupported,
}

mod api {
    use super::*;

    pub fn cached() -> Option<Vec<Vst3Descriptor>> {
        registry::cached()
    }
    pub fn scan_with(opts: &ScanOptions, progress: &ScanProgress) -> Result<Vec<Vst3Descriptor>, String> {
        registry::scan_with(opts, progress)
    }
    pub fn set_scan_cache_dir(dir: Option<PathBuf>) {
        registry::set_cache_dir(dir)
    }
    pub fn clear_failed() {
        registry::clear_failed()
    }
    pub fn take_lookup_request() -> bool {
        registry::take_lookup_request()
    }
    pub fn set_blocking_lookups(report: Option<fn(usize, usize)>) {
        registry::set_blocking_lookups(report)
    }
    pub fn scan() -> Vec<Vst3Descriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<Vst3Descriptor> {
        registry::rescan()
    }
    pub fn add_search_dir(dir: &Path) -> usize {
        registry::add_search_dir(dir)
    }
    pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
        registry::scan_paths(dirs)
    }
    pub fn scan_failures() -> Vec<(PathBuf, String)> {
        registry::scan_failures()
    }
    pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
        registry::load_bundle(path)
    }
    pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
        registry::instantiate(id)
    }
    pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
        registry::plugin_info(id)
    }
    pub fn programs(id: &str) -> Option<Vst3Programs> {
        registry::programs(id)
    }
    pub fn programs_known(id: &str) -> bool {
        registry::programs_known(id)
    }
    pub fn shutdown() {
        ffi::exit_modules();
    }
}

/// Scans the standard VST3 folders, `VST3_PATH` and folders added with [`add_search_dir`]
/// **out of process**: the `freedawm-scan` helper next to the program loads the bundles, a few
/// per process, so a plugin that crashes or hangs while loading never takes this process down
/// (see `soundcraft-plugin-scan`). Only new and changed bundles are scanned (by modification time
/// and size); bundles that crashed or hung the scanner stay skipped until [`rescan`]. The results
/// are kept in the scan cache (in memory, and in [`set_scan_cache_dir`]'s folder when set).
/// Blocks until done; `progress` counts the bundles and cancels. Fails when the helper is missing
/// (nothing is scanned in this process instead).
pub fn scan_with(opts: &ScanOptions, progress: &ScanProgress) -> Result<Vec<Vst3Descriptor>, String> {
    api::scan_with(opts, progress)
}

/// [`scan_with`] with the default options, blocking the calling thread for the whole scan (the
/// command line's `engine.*_plugins`; the app scans on a worker thread and lists [`cached`]): every plugin known after it (what is known
/// already if the scanner is missing, with the reason logged).
pub fn scan() -> Vec<Vst3Descriptor> {
    api::scan()
}

/// Like [`scan`], also retrying the bundles that failed before, and forgetting the plugins that
/// failed to instantiate.
pub fn rescan() -> Vec<Vst3Descriptor> {
    api::rescan()
}

/// Forgets the plugins that failed to instantiate (before a [`scan_with`] that retries failures).
pub fn clear_failed() {
    api::clear_failed()
}

/// Whether a plugin was looked up (to create it, or for its description) and is not in the scan
/// cache, or its bundle is gone, since the last call. Lookups never scan nor wait for a scan: the
/// app answers this with a background [`scan_with`] (new and changed bundles), then creates the
/// missing plugins again. Each missing id asks once per process.
pub fn take_lookup_request() -> bool {
    api::take_lookup_request()
}

/// For command-line tools, which have no UI to keep responsive: a plugin looked up and missing
/// from the scan cache runs one blocking incremental scan through the `freedawm-scan` helper
/// (never in this process) before it is given up, calling `report(done, total)` about every half
/// second. `None` (the default, the app) asks for a background scan instead
/// ([`take_lookup_request`]).
pub fn set_blocking_lookups(report: Option<fn(usize, usize)>) {
    api::set_blocking_lookups(report)
}

/// Runs `f` while holding the scan lock, as a running scan does (tests check that lookups never
/// wait for a scan).
#[doc(hidden)]
pub fn with_scan_running<R>(f: impl FnOnce() -> R) -> R {
    registry::with_scan_running(f)
}

/// Where the scan cache is saved (`<config>/PluginScan`): `vst3.json` there is read by
/// [`cached`] and kept up to date by scans. `None` (the default) keeps it in memory only.
pub fn set_scan_cache_dir(dir: Option<PathBuf>) {
    api::set_scan_cache_dir(dir)
}

/// Adds a folder to the search path and lists its plugins now, **loading them in this process**
/// (for tools and tests pointing at a known folder; later scans cover it out of process). Returns
/// how many plugins the folder holds.
pub fn add_search_dir(dir: &Path) -> usize {
    api::add_search_dir(dir)
}

/// Lists the plugins in the given folders by loading them **in this process**, without touching
/// the cache (for tools and tests; the app uses [`scan_with`]).
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
    api::scan_paths(dirs)
}

/// What the cached scans could not use, each with the reason: the files they skipped because
/// they would not load (not a VST3 module, missing export, …) or that crashed or hung the scanner, then the plugins that failed to instantiate (by id). Never scans;
/// [`rescan`] clears it.
pub fn scan_failures() -> Vec<(PathBuf, String)> {
    api::scan_failures()
}

/// The plugins known, without scanning (reads the scan cache file once): `None` until something
/// has scanned.
pub fn cached() -> Option<Vec<Vst3Descriptor>> {
    api::cached()
}

/// Shuts plugin hosting down: runs the module exit of every loaded binary that has no live
/// instance left, each on the thread that loaded it; later loads fail. Call it on the main
/// thread once the plugin instances are gone, before the process exits. Without it the exits
/// run from an exit handler, by which time some plugins have already torn down state they need
/// (HALion Sonic crashes in its module exit after it has been used).
pub fn shutdown() {
    api::shutdown()
}

/// The audio module classes in one `.vst3` bundle or file.
pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    api::load_bundle(path)
}

/// Creates a plugin by `vst3:<class id>`, prepared for 48 kHz stereo with 1024-frame blocks (like
/// `soundcraft_dsp::create`), with the reason when it fails.
pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
    api::instantiate(id)
}

/// Makes the `vst3:` plug-in `id` for ARA and binds it to the document controller `dc` with the
/// roles `assigned` of `known` (`soundcraft_ara_host::sys::kARA*Role`), as ARA asks: right after
/// it is made, before it is prepared (activated), its state is set or its editor opened. Returns
/// it **unprepared**, with the ARA extension the binding gave (valid while the plug-in lives):
/// give the extension's renderers their playback regions, then `prepare` it. Call on the
/// document controller's thread. Fails with the reason (no ARA 2 entry point, the binding gave
/// nothing, the plug-in cannot be made).
pub fn instantiate_ara(
    id: &str,
    dc: soundcraft_ara_host::sys::ARADocumentControllerRef,
    known: i32,
    assigned: i32,
) -> Result<(Vst3Plugin, soundcraft_ara_host::RawExtension), Vst3Error> {
    registry::instantiate_ara(id, dc, known, assigned)
}

/// Like [`instantiate`], but returns the concrete [`Vst3Plugin`] (for its extra methods, such
/// as [`Vst3Plugin::editor_size`]).
pub fn instantiate_plugin(id: &str) -> Result<Vst3Plugin, Vst3Error> {
    registry::instantiate_plugin(id)
}

/// The binaries this process has loaded, oldest first (tests check what a call loaded).
#[doc(hidden)]
pub fn loaded_paths() -> Vec<PathBuf> {
    registry::loaded_paths()
}

/// Turns off the system's crash and critical-error dialogs for this process (and the processes it
/// starts): for the scanner helper, which must end at once when a plugin crashes it.
pub fn quiet_crash_dialogs() {
    ffi::quiet_crash_dialogs();
}

/// Dispatches this thread's window messages for `time`, as an app's event loop would: for tests
/// and tools that open plugin editors (on the process's main thread) without one.
#[doc(hidden)]
pub fn pump_editor_messages(time: std::time::Duration) {
    ffi::pump_messages(time);
}

/// Creates a plugin by `vst3:<class id>`; `None` if unknown or it fails to load.
pub fn create(id: &str) -> Option<Box<dyn Plugin>> {
    parse_id(id)?;
    match api::instantiate(id) {
        Ok(p) => Some(p),
        Err(e) => {
            log::warn!("{e}");
            None
        }
    }
}

/// The description of a `vst3:<class id>` plugin. The first call for a type instantiates it once
/// to read its parameters; the result is cached (and leaked, see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    api::plugin_info(id)
}

/// The built-in programs of a `vst3:<class id>` plugin, known once that plugin type has been
/// instantiated in this process (inserted, or opened with a session); `None` before that, since
/// listing presets never loads a plugin. Empty `names`: it has none.
pub fn programs(id: &str) -> Option<Vst3Programs> {
    api::programs(id)
}

/// Whether [`programs`] knows the plugin yet (cheap: no copy of the list).
pub fn programs_known(id: &str) -> bool {
    api::programs_known(id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        let id = "vst3:0123456789ABCDEFfedcba9876543210";
        assert_eq!(parse_id(id), Some([0x01, 0x23, 0x45, 0x67, 0x89, 0xAB, 0xCD, 0xEF, 0xFE, 0xDC, 0xBA, 0x98, 0x76, 0x54, 0x32, 0x10]));
        assert_eq!(parse_id("vst3:"), None);
        assert_eq!(parse_id("vst3:0123"), None);
        assert_eq!(parse_id("vst3:0123456789ABCDEF0123456789ABCDEG"), None);
        assert_eq!(parse_id("vst3:ééééééééééééééééé"), None);
        assert_eq!(parse_id("VST3:0123456789ABCDEF0123456789ABCDEF"), None);
        assert_eq!(parse_id("clap:x"), None);
        assert_eq!(parse_id("eq_7band"), None);
    }

    #[test]
    fn non_vst3_ids_are_rejected_without_scanning() {
        assert!(create("eq_7band").is_none());
        assert!(create("clap:com.x").is_none());
        assert!(plugin_info("reverb").is_none());
    }

    #[test]
    fn garbage_files_are_errors_not_crashes() {
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        let arch = if cfg!(target_os = "macos") {
            "MacOS"
        } else if cfg!(windows) {
            "x86_64-win"
        } else {
            "x86_64-linux"
        };
        let exe = if cfg!(target_os = "macos") {
            "Bundle"
        } else if cfg!(windows) {
            "Bundle.vst3"
        } else {
            "Bundle.so"
        };
        std::fs::create_dir_all(d.join(format!("Bundle.vst3/Contents/{arch}"))).unwrap();
        std::fs::write(d.join("garbage.vst3"), b"\x7fELF this is not a plugin \x00\x01\x02").unwrap();
        std::fs::write(d.join("empty.vst3"), b"").unwrap();
        std::fs::write(d.join(format!("Bundle.vst3/Contents/{arch}/{exe}")), [0xcf, 0xfa, 0xed, 0xfe, 1, 2, 3]).unwrap();
        std::fs::create_dir_all(d.join("NoBinary.vst3/Contents")).unwrap();
        for f in ["garbage.vst3", "empty.vst3", "Bundle.vst3", "NoBinary.vst3", "missing.vst3"] {
            assert!(load_bundle(&d.join(f)).is_err(), "{f}");
        }
        assert!(scan_paths(std::slice::from_ref(&d)).is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    // An unknown id is looked up in the scan of the system VST3 folders.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn unknown_ids_are_errors_not_crashes() {
        let missing = "vst3:00000000000000000000000000000000";
        assert!(create(missing).is_none());
        assert!(plugin_info(missing).is_none());
        assert!(instantiate(missing).is_err());
    }

    // Unix only: the test points a ".vst3" at a system library through a symlink.
    #[cfg(unix)]
    #[test]
    fn a_real_library_without_the_vst3_export_is_rejected() {
        // A system library loads fine but has no `GetPluginFactory`.
        // (macOS system libraries live only in the dyld shared cache, not as files.)
        let lib = if cfg!(target_os = "macos") {
            None
        } else {
            ["/lib/x86_64-linux-gnu/libz.so.1", "/usr/lib/libz.so.1", "/lib64/libz.so.1"].iter().map(PathBuf::from).find(|p| p.exists())
        };
        let Some(lib) = lib else { return };
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-nolib-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        // Point a single-file ".vst3" at it via a symlink (no copying of binaries).
        std::os::unix::fs::symlink(&lib, d.join("z.vst3")).unwrap();
        let e = load_bundle(&d.join("z.vst3")).unwrap_err();
        assert!(e.to_string().contains("not a VST3 plugin"), "{e}");
        let _ = std::fs::remove_dir_all(&d);
    }
}

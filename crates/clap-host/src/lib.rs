//! SoundCraft's CLAP plugin host.
//!
//! Scans the standard CLAP folders (plus `CLAP_PATH`) out of process (the `freedawm-scan`
//! helper, a few bundles per child process, see [`scan_with`]; the results are kept in a scan
//! cache), loads the `.clap` binary of each plugin it creates in-process and exposes each plugin
//! as a [`soundcraft_dsp::Plugin`], so the mix engine treats third-party
//! plugins exactly like the built-in ones. Plugin ids are `clap:<CLAP plugin id>`; parameter ids
//! are the CLAP param ids as decimal strings, with their human names in [`PluginInfo::params`].
//!
//! This is SoundCraft's single isolated `unsafe` crate (craftrules never-crash): `unsafe` is
//! denied crate-wide and allowed only in the private `ffi` module. The public API is safe, returns
//! `Result`/`Option` and never panics. Limits of in-process hosting: a plugin that itself crashes
//! (segfault) takes the process down with it; we cannot sandbox native code.
//!
//! `PluginInfo` must be `&'static`: the registry leaks exactly one `PluginInfo` (with its param
//! table) per plugin *type* the first time that type is instantiated, and caches it. The leak is
//! bounded by the number of distinct installed plugins actually used. Loaded binaries likewise
//! stay mapped for the life of the process.
//!
//! On `wasm32` the crate compiles to stubs: scans are empty and nothing can be created.
//!
//! Plugin state (`clap.state`) round-trips through [`Plugin::save_state`] /
//! [`Plugin::load_state`] as an opaque blob. Plugin editors (`clap.gui`) open as the plugin's own
//! floating window ([`Plugin::open_editor`], or [`Plugin::editor`] for a handle usable on the main
//! thread while the instance processes on the audio thread). Plugins that offer only an embedded
//! editor get a host window of SoundCraft's on Windows (resized on `request_resize`, closed by
//! its title bar); elsewhere they are not hosted yet, and SoundCraft's generic parameter editor
//! still works for them.
//! Parameter changes made in the plugin's GUI arrive as output events while the plugin processes
//! and are reported by the editor's `idle`, which also fires the plugin's `clap.timer-support`
//! timers and, on Windows, keeps an open embedded editor drawing while the app repaints
//! continuously (its `WM_TIMER`s are dispatched and its pending paints run; see
//! `hostwin::service`). Known gap: a floating (plugin-owned) editor gets the timer service only;
//! the host has no window of its own to `RedrawWindow`, so the paints those timers invalidate
//! stay starved while the transport plays, and such an editor stops repainting until the
//! transport stops (closing it would take enumerating the thread's top-level windows).

#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

pub mod scan;

#[cfg(not(target_arch = "wasm32"))]
#[allow(unsafe_code)]
mod ffi;
#[cfg(not(target_arch = "wasm32"))]
mod plugin;
#[cfg(not(target_arch = "wasm32"))]
mod registry;

#[cfg(not(target_arch = "wasm32"))]
pub use plugin::ClapPlugin;

use soundcraft_dsp::{Category, Plugin, PluginInfo};
use std::path::{Path, PathBuf};

pub use soundcraft_plugin_scan::{ScanOptions, ScanProgress};

impl soundcraft_plugin_scan::Descriptor for ClapDescriptor {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
}

/// Prefix of every hosted-CLAP plugin id.
pub const ID_PREFIX: &str = "clap:";

/// The CLAP plugin id inside a `clap:<id>` SoundCraft id (`None` for other ids or an empty id).
pub fn parse_id(id: &str) -> Option<&str> {
    id.strip_prefix(ID_PREFIX).filter(|s| !s.is_empty() && !s.contains('\0'))
}

/// One plugin found by a scan.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct ClapDescriptor {
    /// SoundCraft id: `clap:<plugin_id>`.
    pub id: String,
    /// The CLAP plugin id (reverse-DNS, e.g. `com.vendor.reverb`).
    pub plugin_id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
    pub category: Category,
    pub is_instrument: bool,
    /// The `.clap` file or bundle it lives in.
    pub path: PathBuf,
}

/// A preset reported by a plugin's preset-discovery provider.
#[derive(Debug, Clone, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
pub struct ClapPreset {
    pub name: String,
    /// The preset file; `None` for a preset inside the plugin.
    pub location: Option<String>,
    /// Which preset within the location, when it holds several.
    pub load_key: Option<String>,
}

impl ClapPreset {
    /// The CLAP location kind: a file, or the plugin itself.
    pub fn kind(&self) -> u32 {
        if self.location.is_some() { 0 } else { 1 }
    }
}

/// Why a CLAP operation failed.
#[derive(Debug, Clone, PartialEq, thiserror::Error)]
pub enum ClapError {
    #[error("cannot load CLAP plugin {0}: {1}")]
    Load(String, String),
    #[error("no CLAP plugin `{0}`")]
    NotFound(String),
    #[error("cannot create CLAP plugin `{0}`: {1}")]
    Instantiate(String, String),
    #[error("CLAP processing: {0}")]
    Process(String),
    #[error("CLAP plugins are not supported on this platform")]
    Unsupported,
}

#[cfg(not(target_arch = "wasm32"))]
mod api {
    use super::*;

    pub fn cached() -> Option<Vec<ClapDescriptor>> {
        registry::cached()
    }
    pub fn scan_with(opts: &ScanOptions, progress: &ScanProgress) -> Result<Vec<ClapDescriptor>, String> {
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
    pub fn scan() -> Vec<ClapDescriptor> {
        registry::scan()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        registry::rescan()
    }
    pub fn add_search_dir(dir: &Path) -> usize {
        registry::add_search_dir(dir)
    }
    pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
        registry::scan_paths(dirs)
    }
    pub fn scan_failures() -> Vec<(PathBuf, String)> {
        registry::scan_failures()
    }
    pub fn load_bundle(path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
        registry::load_bundle(path)
    }
    pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, ClapError> {
        registry::instantiate(id)
    }
    pub fn instantiate_plugin(id: &str) -> Result<ClapPlugin, ClapError> {
        registry::instantiate_plugin(id)
    }
    pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
        registry::plugin_info(id)
    }
    pub fn bundle_loaded(desc: &ClapDescriptor) -> bool {
        registry::bundle_loaded(desc)
    }
    pub fn binary_loaded(path: &Path) -> bool {
        registry::binary_loaded(path)
    }
    pub fn presets(desc: &ClapDescriptor) -> Vec<ClapPreset> {
        registry::presets(desc)
    }
    pub fn preset_state(id: &str, preset: &ClapPreset) -> Result<Vec<u8>, String> {
        registry::preset_state(id, preset)
    }
}

#[cfg(target_arch = "wasm32")]
mod api {
    use super::*;

    pub fn cached() -> Option<Vec<ClapDescriptor>> {
        Some(Vec::new())
    }
    pub fn scan_with(_opts: &ScanOptions, _progress: &ScanProgress) -> Result<Vec<ClapDescriptor>, String> {
        Ok(Vec::new())
    }
    pub fn set_scan_cache_dir(_dir: Option<PathBuf>) {}
    pub fn clear_failed() {}
    pub fn take_lookup_request() -> bool {
        false
    }
    pub fn set_blocking_lookups(_report: Option<fn(usize, usize)>) {}
    pub fn scan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn rescan() -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn add_search_dir(_dir: &Path) -> usize {
        0
    }
    pub fn scan_paths(_dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
        Vec::new()
    }
    pub fn scan_failures() -> Vec<(PathBuf, String)> {
        Vec::new()
    }
    pub fn load_bundle(_path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
        Err(ClapError::Unsupported)
    }
    pub fn instantiate(_id: &str) -> Result<Box<dyn Plugin>, ClapError> {
        Err(ClapError::Unsupported)
    }
    pub fn plugin_info(_id: &str) -> Option<&'static PluginInfo> {
        None
    }
    pub fn bundle_loaded(_desc: &ClapDescriptor) -> bool {
        false
    }
    pub fn binary_loaded(_path: &Path) -> bool {
        false
    }
    pub fn presets(_desc: &ClapDescriptor) -> Vec<ClapPreset> {
        Vec::new()
    }
    pub fn preset_state(_id: &str, _preset: &ClapPreset) -> Result<Vec<u8>, String> {
        Err("CLAP plugins are not supported on this platform".into())
    }
}

/// Scans the standard CLAP folders, `CLAP_PATH` and folders added with [`add_search_dir`]
/// **out of process**: the `freedawm-scan` helper next to the program loads the bundles, a few
/// per process, so a plugin that crashes or hangs while loading never takes this process down
/// (see `soundcraft-plugin-scan`). Only new and changed bundles are scanned (by modification time
/// and size); bundles that crashed or hung the scanner stay skipped until [`rescan`]. The results
/// are kept in the scan cache (in memory, and in [`set_scan_cache_dir`]'s folder when set).
/// Blocks until done; `progress` counts the bundles and cancels. Fails when the helper is missing
/// (nothing is scanned in this process instead).
pub fn scan_with(opts: &ScanOptions, progress: &ScanProgress) -> Result<Vec<ClapDescriptor>, String> {
    api::scan_with(opts, progress)
}

/// [`scan_with`] with the default options, blocking the calling thread for the whole scan (the
/// command line's `engine.*_plugins`; the app scans on a worker thread and lists [`cached`]): every plugin known after it (what is known
/// already if the scanner is missing, with the reason logged).
pub fn scan() -> Vec<ClapDescriptor> {
    api::scan()
}

/// Like [`scan`], also retrying the bundles that failed before, and forgetting the plugins that
/// failed to instantiate.
pub fn rescan() -> Vec<ClapDescriptor> {
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
#[cfg(not(target_arch = "wasm32"))]
pub fn with_scan_running<R>(f: impl FnOnce() -> R) -> R {
    registry::with_scan_running(f)
}

/// Where the scan cache is saved (`<config>/PluginScan`): `clap.json` there is read by
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
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<ClapDescriptor> {
    api::scan_paths(dirs)
}

/// What the cached scans could not use, each with the reason: the files they skipped because
/// they would not load (not a CLAP binary, missing entry, …), then the plugins that failed to instantiate (by id). Never scans;
/// [`rescan`] clears it.
pub fn scan_failures() -> Vec<(PathBuf, String)> {
    api::scan_failures()
}

/// The plugins known, without scanning (reads the scan cache file once): `None` until something
/// has scanned.
pub fn cached() -> Option<Vec<ClapDescriptor>> {
    api::cached()
}

/// The plugins in one `.clap` file or bundle.
pub fn load_bundle(path: &Path) -> Result<Vec<ClapDescriptor>, ClapError> {
    api::load_bundle(path)
}

/// Creates a plugin by `clap:<id>`, prepared for 48 kHz stereo with 1024-frame blocks (like
/// `soundcraft_dsp::create`), with the reason when it fails.
pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, ClapError> {
    api::instantiate(id)
}

/// [`instantiate`] as the concrete type, for its diagnostics (such as [`ClapPlugin::live_value`]).
#[cfg(not(target_arch = "wasm32"))]
pub fn instantiate_plugin(id: &str) -> Result<ClapPlugin, ClapError> {
    api::instantiate_plugin(id)
}

/// Creates a plugin by `clap:<id>`; `None` if unknown or it fails to load.
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

/// The description of a `clap:<id>` plugin. The first call for a type instantiates it once to
/// read its parameters; the result is cached (and leaked, see the crate docs).
pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    api::plugin_info(id)
}

/// Whether the plugin's binary is loaded in this process already (it was inserted, or opened
/// with a session): [`presets`] lists nothing before that, so the browser never loads a plugin
/// just to list its presets.
pub fn bundle_loaded(desc: &ClapDescriptor) -> bool {
    api::bundle_loaded(desc)
}

/// [`bundle_loaded`] by the binary's path (`ClapDescriptor::path`).
pub fn binary_loaded(path: &Path) -> bool {
    api::binary_loaded(path)
}

/// The plugin's presets from its binary's preset-discovery factory
/// (`clap.preset-discovery-factory`), once the binary is loaded; empty without one. Any thread
/// (CLAP lets a host index presets off the main thread; the browser lists them on a worker).
pub fn presets(desc: &ClapDescriptor) -> Vec<ClapPreset> {
    api::presets(desc)
}

/// The state of a fresh instance of `id` with `preset` loaded (`clap.preset-load`): what a module
/// stores to start with that preset. Creates the plugin (its own binary only) on this thread,
/// which must be the main thread.
pub fn preset_state(id: &str, preset: &ClapPreset) -> Result<Vec<u8>, String> {
    api::preset_state(id, preset)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_parse() {
        assert_eq!(parse_id("clap:com.u-he.diva"), Some("com.u-he.diva"));
        assert_eq!(parse_id("clap:"), None);
        assert_eq!(parse_id("eq_7band"), None);
        assert_eq!(parse_id("CLAP:x"), None);
        assert_eq!(parse_id("clap:a\0b"), None);
    }

    #[test]
    fn non_clap_ids_are_rejected_without_scanning() {
        assert!(create("eq_7band").is_none());
        assert!(plugin_info("reverb").is_none());
    }

    #[test]
    fn garbage_files_are_errors_not_crashes() {
        let d = std::env::temp_dir().join(format!("soundcraft-clap-garbage-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("Bundle.clap/Contents/MacOS")).unwrap();
        std::fs::write(d.join("garbage.clap"), b"\x7fELF this is not a plugin \x00\x01\x02").unwrap();
        std::fs::write(d.join("empty.clap"), b"").unwrap();
        std::fs::write(d.join("Bundle.clap/Contents/MacOS/Bundle"), [0xcf, 0xfa, 0xed, 0xfe, 1, 2, 3]).unwrap();
        for f in ["garbage.clap", "empty.clap", "Bundle.clap", "missing.clap"] {
            assert!(load_bundle(&d.join(f)).is_err(), "{f}");
        }
        assert!(scan_paths(std::slice::from_ref(&d)).is_empty());
        assert!(create("clap:does.not.exist").is_none());
        assert!(plugin_info("clap:does.not.exist").is_none());
        assert!(instantiate("clap:does.not.exist").is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

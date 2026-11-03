//! The FFI boundary: the only module in SoundCraft allowed to use `unsafe`.
//!
//! It loads CLAP binaries (`libloading`), calls the plugin entry/factory/plugin vtables
//! (`clap-sys` raw bindings) and implements the host callbacks. Everything it exposes to the rest
//! of the crate is a safe API returning `Result`.
//!
//! Soundness rests on the CLAP contract (the plugin honours the vtable signatures and the
//! documented pointer lifetimes). A plugin that violates it (e.g. dereferences garbage) can still
//! crash the process: hosting in-process can't prevent that. What this module guarantees is that
//! *SoundCraft* never hands the plugin dangling or mis-sized pointers, never calls a missing
//! (null) function pointer, and never reads plugin strings beyond their bounds.

use crate::{ClapError, scan};
use clap_sys::audio_buffer::clap_audio_buffer;
use clap_sys::entry::clap_plugin_entry;
use clap_sys::events::{
    CLAP_CORE_EVENT_SPACE_ID, CLAP_EVENT_MIDI, CLAP_EVENT_NOTE_OFF, CLAP_EVENT_NOTE_ON, CLAP_EVENT_PARAM_VALUE, CLAP_EVENT_TRANSPORT,
    CLAP_TRANSPORT_HAS_BEATS_TIMELINE, CLAP_TRANSPORT_HAS_SECONDS_TIMELINE, CLAP_TRANSPORT_HAS_TEMPO, CLAP_TRANSPORT_HAS_TIME_SIGNATURE,
    CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_RECORDING, clap_event_header, clap_event_midi, clap_event_note, clap_event_param_value,
    clap_event_transport, clap_input_events, clap_output_events,
};
use clap_sys::ext::audio_ports::{CLAP_AUDIO_PORT_IS_MAIN, CLAP_EXT_AUDIO_PORTS, clap_audio_port_info, clap_plugin_audio_ports};
use clap_sys::ext::gui::{CLAP_EXT_GUI, clap_host_gui, clap_plugin_gui, clap_window, clap_window_handle};
use clap_sys::ext::latency::{CLAP_EXT_LATENCY, clap_host_latency, clap_plugin_latency};
use clap_sys::ext::log::{CLAP_EXT_LOG, CLAP_LOG_ERROR, CLAP_LOG_FATAL, CLAP_LOG_WARNING, clap_host_log, clap_log_severity};
use clap_sys::ext::note_ports::{CLAP_EXT_NOTE_PORTS, clap_note_port_info, clap_plugin_note_ports};
use clap_sys::ext::params::{
    CLAP_EXT_PARAMS, clap_host_params, clap_param_clear_flags, clap_param_info, clap_param_rescan_flags, clap_plugin_params,
};
use clap_sys::ext::preset_load::{CLAP_EXT_PRESET_LOAD, CLAP_EXT_PRESET_LOAD_COMPAT, clap_plugin_preset_load};
use clap_sys::ext::render::{CLAP_EXT_RENDER, CLAP_RENDER_OFFLINE, CLAP_RENDER_REALTIME, clap_plugin_render};
use clap_sys::ext::state::{CLAP_EXT_STATE, clap_host_state, clap_plugin_state};
use clap_sys::ext::timer_support::{CLAP_EXT_TIMER_SUPPORT, clap_host_timer_support, clap_plugin_timer_support};
use clap_sys::factory::plugin_factory::{CLAP_PLUGIN_FACTORY_ID, clap_plugin_factory};
use clap_sys::factory::preset_discovery::{
    CLAP_PRESET_DISCOVERY_FACTORY_ID, CLAP_PRESET_DISCOVERY_FACTORY_ID_COMPAT, CLAP_PRESET_DISCOVERY_LOCATION_FILE,
    CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN, clap_preset_discovery_factory, clap_preset_discovery_filetype, clap_preset_discovery_indexer,
    clap_preset_discovery_location, clap_preset_discovery_metadata_receiver, clap_preset_discovery_soundpack,
};
use clap_sys::fixedpoint::{CLAP_BEATTIME_FACTOR, CLAP_SECTIME_FACTOR};
use clap_sys::host::clap_host;
use clap_sys::id::clap_id;
use clap_sys::plugin::clap_plugin;
use clap_sys::process::{CLAP_PROCESS_ERROR, clap_process};
use clap_sys::stream::{clap_istream, clap_ostream};
use clap_sys::timestamp::clap_timestamp;
use clap_sys::universal_plugin_id::clap_universal_plugin_id;
use clap_sys::version::{CLAP_VERSION, clap_version_is_compatible};
use soundcraft_dsp::Transport;
use std::collections::HashMap;
use std::ffi::{CStr, CString, c_char, c_void};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::time::{Duration, Instant};

/// Most plugins one factory may report (hostile-input cap).
const MAX_PLUGINS_PER_FACTORY: u32 = 1024;
/// Most parameters read from one plugin.
pub(crate) const MAX_PARAMS: u32 = 4096;
/// Most audio ports per direction we provide buffers for.
pub(crate) const MAX_PORTS: u32 = 16;
/// Most channels per audio port.
pub(crate) const MAX_PORT_CHANNELS: u32 = 64;
/// Most feature strings read from a descriptor.
const MAX_FEATURES: usize = 64;
/// Capacity of the per-block event list (preallocated: `process` never allocates).
pub(crate) const EVENT_CAP: usize = 1024;
/// Largest plugin state we accept from `clap_plugin_state.save` (hostile-input cap).
pub(crate) const MAX_STATE_BYTES: usize = 64 << 20;
/// Most editor parameter edits buffered between two UI frames.
const EDIT_CAP: usize = 256;

// ---- plugin strings ------------------------------------------------------------------------

/// Reads a NUL-terminated plugin string (`None` for null).
///
/// SAFETY contract for callers: `p` is null or points to a NUL-terminated string that stays valid
/// for the duration of the call (CLAP guarantees this for descriptor fields).
unsafe fn read_cstr(p: *const c_char) -> Option<String> {
    if p.is_null() {
        return None;
    }
    // SAFETY: non-null and NUL-terminated per this function's contract.
    Some(unsafe { CStr::from_ptr(p) }.to_string_lossy().into_owned())
}

/// Reads a fixed-size `char[N]` name buffer, stopping at the first NUL or the end of the array.
/// `c_char` is `i8` on x86 and `u8` on ARM/RISC-V Linux; the byte reinterpretation is the same for both.
fn read_name(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| u8::from_ne_bytes(c.to_ne_bytes())).collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

// ---- loaded binaries -----------------------------------------------------------------------

/// A plugin descriptor as reported by the factory.
#[derive(Debug, Clone)]
pub(crate) struct RawDescriptor {
    pub id: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub description: String,
    pub features: Vec<String>,
}

/// A loaded, initialised CLAP binary. Loaded binaries are cached for the life of the process
/// (see [`Bundle::load`]): plugin code must never be unmapped while an instance may still run.
pub(crate) struct Bundle {
    entry: *const clap_plugin_entry,
    factory: *const clap_plugin_factory,
    // Dropped last: the library must outlive every pointer above.
    _lib: libloading::Library,
}

// SAFETY: CLAP requires the entry and the plugin factory to be thread-safe; the raw pointers are
// read-only vtables inside the loaded library, which `_lib` keeps mapped.
unsafe impl Send for Bundle {}
// SAFETY: as above; `Bundle` has no interior mutability on the Rust side.
unsafe impl Sync for Bundle {}

fn loaded() -> &'static Mutex<HashMap<PathBuf, Arc<Bundle>>> {
    static L: OnceLock<Mutex<HashMap<PathBuf, Arc<Bundle>>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(HashMap::new()))
}

impl Bundle {
    /// Loads (once per path, then cached for the life of the process) and initialises a `.clap`.
    pub fn load(path: &Path) -> Result<Arc<Bundle>, ClapError> {
        let mut map = loaded().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(b) = map.get(path) {
            return Ok(b.clone());
        }
        let b = Arc::new(Bundle::open(path)?);
        map.insert(path.to_path_buf(), b.clone());
        Ok(b)
    }

    fn open(path: &Path) -> Result<Bundle, ClapError> {
        let err = |m: String| ClapError::Load(path.display().to_string(), m);
        let exe = scan::bundle_executable(path).ok_or_else(|| err("no loadable binary".into()))?;
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in CLAP plugin folders or named explicitly.
        let lib = unsafe { libloading::Library::new(&exe) }.map_err(|e| err(e.to_string()))?;
        // SAFETY: `clap_entry` is the CLAP-mandated exported data symbol of type
        // `const clap_plugin_entry`; the symbol's address is therefore a `*const clap_plugin_entry`.
        let entry: *const clap_plugin_entry = match unsafe { lib.get::<*const clap_plugin_entry>(b"clap_entry\0") } {
            Ok(sym) => *sym,
            Err(e) => return Err(err(format!("not a CLAP plugin: {e}"))),
        };
        if entry.is_null() {
            return Err(err("null clap_entry".into()));
        }
        // SAFETY: non-null pointer to the entry struct inside the library, which `lib` keeps mapped.
        let e = unsafe { *entry };
        if !clap_version_is_compatible(e.clap_version) {
            return Err(err(format!("unsupported CLAP version {}.{}", e.clap_version.major, e.clap_version.minor)));
        }
        let (Some(init), Some(get_factory)) = (e.init, e.get_factory) else { return Err(err("incomplete clap_entry".into())) };
        let cpath = CString::new(path.to_string_lossy().into_owned()).map_err(|_| err("path contains NUL".into()))?;
        // SAFETY: `init` comes from a valid entry; the path is a valid NUL-terminated string that
        // outlives the call.
        if !unsafe { init(cpath.as_ptr()) } {
            return Err(err("clap_entry.init failed".into()));
        }
        // SAFETY: the entry is initialised; the id is a static NUL-terminated string.
        let factory = unsafe { get_factory(CLAP_PLUGIN_FACTORY_ID.as_ptr()) } as *const clap_plugin_factory;
        if factory.is_null() {
            if let Some(deinit) = e.deinit {
                // SAFETY: balanced with the successful `init` above.
                unsafe { deinit() };
            }
            return Err(err("no plugin factory".into()));
        }
        Ok(Bundle { entry, factory, _lib: lib })
    }

    fn factory(&self) -> clap_plugin_factory {
        // SAFETY: `factory` was non-null at load time and points into the still-mapped library.
        unsafe { *self.factory }
    }

    /// The plugins this binary provides.
    pub fn descriptors(&self) -> Vec<RawDescriptor> {
        let f = self.factory();
        let (Some(count), Some(get)) = (f.get_plugin_count, f.get_plugin_descriptor) else { return Vec::new() };
        // SAFETY: vtable call with the factory pointer it belongs to.
        let n = unsafe { count(self.factory) }.min(MAX_PLUGINS_PER_FACTORY);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: index is below the reported count.
            let d = unsafe { get(self.factory, i) };
            if d.is_null() {
                continue;
            }
            // SAFETY: non-null descriptor owned by the plugin library, valid while it is loaded.
            let d = unsafe { *d };
            // SAFETY: descriptor string fields are null or NUL-terminated (CLAP contract).
            let Some(id) = (unsafe { read_cstr(d.id) }).filter(|s| !s.is_empty()) else { continue };
            // SAFETY: as above.
            let name = unsafe { read_cstr(d.name) }.unwrap_or_else(|| id.clone());
            let mut features = Vec::new();
            if !d.features.is_null() {
                for k in 0..MAX_FEATURES {
                    // SAFETY: `features` is a NULL-terminated array; we stop at the terminator
                    // (or the cap) and never read past it.
                    let p = unsafe { *d.features.add(k) };
                    if p.is_null() {
                        break;
                    }
                    // SAFETY: a non-null entry of the feature array is a NUL-terminated string.
                    if let Some(s) = unsafe { read_cstr(p) } {
                        features.push(s);
                    }
                }
            }
            out.push(RawDescriptor {
                id,
                name,
                // SAFETY: descriptor string fields are null or NUL-terminated.
                vendor: unsafe { read_cstr(d.vendor) }.unwrap_or_default(),
                // SAFETY: as above.
                version: unsafe { read_cstr(d.version) }.unwrap_or_default(),
                // SAFETY: as above.
                description: unsafe { read_cstr(d.description) }.unwrap_or_default(),
                features,
            });
        }
        out
    }

    /// Whether the binary at `path` is loaded in this process already.
    pub fn is_loaded(path: &Path) -> bool {
        loaded().lock().unwrap_or_else(PoisonError::into_inner).contains_key(path)
    }

    /// Every preset the binary's preset-discovery providers (`clap.preset-discovery-factory`)
    /// report: each provider's declared locations are indexed (a folder's files of the declared
    /// types, a few levels deep). Empty when the binary has no such factory. Any one thread at a
    /// time (CLAP lets a host index presets off the main thread; the browser does, on a worker).
    pub fn presets(&self) -> Vec<RawPreset> {
        // SAFETY: `entry` points into the still-mapped library (checked non-null at load).
        let Some(get_factory) = (unsafe { *self.entry }).get_factory else { return Vec::new() };
        // SAFETY: the entry is initialised; the ids are static NUL-terminated strings.
        let mut f = unsafe { get_factory(CLAP_PRESET_DISCOVERY_FACTORY_ID.as_ptr()) } as *const clap_preset_discovery_factory;
        if f.is_null() {
            // SAFETY: as above.
            f = unsafe { get_factory(CLAP_PRESET_DISCOVERY_FACTORY_ID_COMPAT.as_ptr()) } as *const clap_preset_discovery_factory;
        }
        // SAFETY: null or the plugin's static factory vtable, valid while the library is loaded.
        let Some(factory) = (unsafe { f.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get_desc), Some(create)) = (factory.count, factory.get_descriptor, factory.create) else { return Vec::new() };
        let mut out = Vec::new();
        // SAFETY: vtable call with its own factory pointer.
        let n = unsafe { count(f) }.min(MAX_PROVIDERS);
        for i in 0..n {
            // SAFETY: index below the count.
            let d = unsafe { get_desc(f, i) };
            // SAFETY: null or a descriptor owned by the library.
            let Some(d) = (unsafe { d.as_ref() }) else { continue };
            if d.id.is_null() {
                continue;
            }
            let indexer = Indexer::new();
            // SAFETY: valid factory; the indexer is boxed and outlives the provider (destroyed
            // below); `d.id` is the descriptor's NUL-terminated id.
            let provider = unsafe { create(f, &indexer.raw, d.id) };
            // SAFETY: null or the provider the factory just made.
            let Some(pv) = (unsafe { provider.as_ref() }) else { continue };
            let Some(destroy) = pv.destroy else { continue };
            // SAFETY: a fresh provider, initialised once.
            if pv.init.is_some_and(|init| unsafe { init(provider) })
                && let Some(get_metadata) = pv.get_metadata
            {
                let locations = std::mem::take(&mut *indexer.data.locations.lock().unwrap_or_else(PoisonError::into_inner));
                let types = std::mem::take(&mut *indexer.data.extensions.lock().unwrap_or_else(PoisonError::into_inner));
                for (kind, location) in locations {
                    let mut files = Vec::new();
                    match (kind, &location) {
                        (CLAP_PRESET_DISCOVERY_LOCATION_PLUGIN, _) => files.push(None),
                        (CLAP_PRESET_DISCOVERY_LOCATION_FILE, Some(loc)) => {
                            let path = PathBuf::from(loc);
                            if path.is_dir() {
                                preset_files(&path, &types, 0, &mut files);
                            } else {
                                files.push(Some(loc.clone()));
                            }
                        }
                        _ => {}
                    }
                    for file in files {
                        if out.len() >= MAX_PRESETS {
                            break;
                        }
                        let Ok(c) = file.as_deref().map(CString::new).transpose() else { continue };
                        let receiver = Receiver::new(kind, file);
                        let loc_ptr = c.as_ref().map_or(std::ptr::null(), |c| c.as_ptr());
                        // SAFETY: initialised provider; the location string and the boxed
                        // receiver outlive the call.
                        unsafe { get_metadata(provider, kind, loc_ptr, &receiver.raw) };
                        out.extend(receiver.take());
                    }
                }
            }
            // SAFETY: balanced with `create`; nothing uses the provider afterwards.
            unsafe { destroy(provider) };
        }
        out.truncate(MAX_PRESETS);
        out
    }

    /// Creates and initialises one plugin instance.
    pub fn instantiate(self: &Arc<Self>, plugin_id: &str) -> Result<Instance, ClapError> {
        let err = |m: &str| ClapError::Instantiate(plugin_id.to_string(), m.to_string());
        let create = self.factory().create_plugin.ok_or_else(|| err("factory cannot create plugins"))?;
        let cid = CString::new(plugin_id).map_err(|_| err("id contains NUL"))?;
        let state = Arc::new(HostState::default());
        let host = Box::new(clap_host {
            clap_version: CLAP_VERSION,
            host_data: Arc::as_ptr(&state).cast_mut().cast::<c_void>(),
            name: c"FreeDAWm".as_ptr(),
            vendor: c"SoundCraft contributors".as_ptr(),
            url: c"https://github.com/s0lidr0ck/FreeDAWm".as_ptr(),
            version: c"0.1.0".as_ptr(),
            get_extension: Some(host_get_extension),
            request_restart: Some(host_request_restart),
            request_process: Some(host_request_process),
            request_callback: Some(host_request_callback),
        });
        // SAFETY: the factory is valid; `host` is boxed (stable address) and owned by the returned
        // `Instance`, which destroys the plugin before dropping the box; `cid` outlives the call.
        let plugin = unsafe { create(self.factory, &*host, cid.as_ptr()) };
        if plugin.is_null() {
            return Err(err("factory returned no instance"));
        }
        let mut inst = Instance {
            plugin,
            params: std::ptr::null(),
            ports: std::ptr::null(),
            latency: std::ptr::null(),
            notes: std::ptr::null(),
            state_ext: std::ptr::null(),
            gui: Arc::new(GuiLink::default()),
            raw: Vec::with_capacity(EVENT_CAP),
            out_params: Vec::with_capacity(EVENT_CAP),
            active: false,
            processing: false,
            max_frames: 0,
            created: true,
            _host: host,
            state,
            _bundle: self.clone(),
        };
        // SAFETY: `plugin` is a fresh non-null instance; we copy its vtable struct.
        let vt = unsafe { *plugin };
        let ok = match vt.init {
            // SAFETY: `init` is called exactly once, right after creation, as CLAP requires.
            Some(init) => unsafe { init(plugin) },
            None => false,
        };
        if !ok {
            // `Drop` destroys the instance (without deactivating: it never activated).
            return Err(err("plugin init failed"));
        }
        inst.params = inst.extension::<clap_plugin_params>(CLAP_EXT_PARAMS);
        inst.ports = inst.extension::<clap_plugin_audio_ports>(CLAP_EXT_AUDIO_PORTS);
        inst.latency = inst.extension::<clap_plugin_latency>(CLAP_EXT_LATENCY);
        inst.notes = inst.extension::<clap_plugin_note_ports>(CLAP_EXT_NOTE_PORTS);
        inst.state_ext = inst.extension::<clap_plugin_state>(CLAP_EXT_STATE);
        let gui = inst.extension::<clap_plugin_gui>(CLAP_EXT_GUI);
        let timer = inst.extension::<clap_plugin_timer_support>(CLAP_EXT_TIMER_SUPPORT);
        inst.gui = Arc::new(GuiLink {
            inner: Mutex::new(GuiInner { plugin, gui, timer, state: inst.state_ext, alive: true, created: false, visible: false, window: None }),
            host: Arc::clone(&inst.state),
        });
        Ok(inst)
    }
}

impl Drop for Bundle {
    fn drop(&mut self) {
        // SAFETY: `entry` was valid at load time and `_lib` (dropped after this) keeps it mapped.
        let e = unsafe { *self.entry };
        if let Some(deinit) = e.deinit {
            // SAFETY: balanced with the successful `init` in `open`; no instance holds an `Arc`
            // to this bundle any more (they keep it alive), so no plugin code still runs.
            unsafe { deinit() };
        }
    }
}

// ---- host side -----------------------------------------------------------------------------

/// Flags the plugin raises through the host callbacks (any thread).
pub(crate) struct HostState {
    pub restart: AtomicBool,
    pub process: AtomicBool,
    pub callback: AtomicBool,
    pub latency_changed: AtomicBool,
    pub params_rescan: AtomicBool,
    /// `clap_host_state.mark_dirty`.
    pub dirty: AtomicBool,
    /// `clap_host_gui` requests, serviced by the editor's `idle` on the main thread.
    pub gui_closed: AtomicBool,
    pub gui_destroyed: AtomicBool,
    pub gui_show: AtomicBool,
    pub gui_hide: AtomicBool,
    /// A size an embedded editor asked for (`width << 32 | height`; 0 = none pending).
    pub gui_resize: AtomicU64,
    /// Parameter changes the plugin reported (output events: typically edits in its GUI),
    /// waiting for the UI. Bounded by `EDIT_CAP`; the audio thread only `try_lock`s it.
    pub edits: Mutex<Vec<(u32, f64)>>,
    /// `clap_host_timer_support` timers (main thread: registered and fired there), at most
    /// `MAX_TIMERS`.
    pub timers: Mutex<Vec<HostTimer>>,
    /// The next timer id to hand out.
    timer_ids: AtomicU32,
}

/// One periodic timer a plugin registered through `clap.timer-support`.
pub(crate) struct HostTimer {
    id: clap_id,
    period: Duration,
    due: Instant,
}

/// Most timers one plugin may register.
const MAX_TIMERS: usize = 16;
/// The shortest timer period (the editor service runs at most at 60 Hz; CLAP lets the host
/// lengthen a period).
const MIN_TIMER_PERIOD: Duration = Duration::from_millis(15);

impl HostState {
    /// Registers a timer of `period_ms` (lengthened to `MIN_TIMER_PERIOD`); `None` when the
    /// plugin has `MAX_TIMERS` already.
    fn register_timer(&self, period_ms: u32) -> Option<clap_id> {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        if t.len() >= MAX_TIMERS {
            return None;
        }
        let id = self.timer_ids.fetch_add(1, Ordering::Relaxed);
        let period = Duration::from_millis(u64::from(period_ms)).max(MIN_TIMER_PERIOD);
        t.push(HostTimer { id, period, due: Instant::now() + period });
        Some(id)
    }

    /// Forgets timer `id`; false when there is none.
    fn unregister_timer(&self, id: clap_id) -> bool {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        let before = t.len();
        t.retain(|x| x.id != id);
        t.len() != before
    }

    /// The timers due now, each re-armed one period from now (a missed period is not made up).
    /// Empty without an allocation.
    fn due_timers(&self) -> Vec<clap_id> {
        let mut t = self.timers.lock().unwrap_or_else(PoisonError::into_inner);
        let now = Instant::now();
        let mut due = Vec::new();
        for x in t.iter_mut().filter(|x| x.due <= now) {
            due.push(x.id);
            x.due = now + x.period;
        }
        due
    }
}

impl Default for HostState {
    fn default() -> Self {
        HostState {
            restart: AtomicBool::new(false),
            process: AtomicBool::new(false),
            callback: AtomicBool::new(false),
            latency_changed: AtomicBool::new(false),
            params_rescan: AtomicBool::new(false),
            dirty: AtomicBool::new(false),
            gui_closed: AtomicBool::new(false),
            gui_destroyed: AtomicBool::new(false),
            gui_show: AtomicBool::new(false),
            gui_hide: AtomicBool::new(false),
            gui_resize: AtomicU64::new(0),
            edits: Mutex::new(Vec::with_capacity(EDIT_CAP)),
            timers: Mutex::new(Vec::with_capacity(MAX_TIMERS)),
            timer_ids: AtomicU32::new(1),
        }
    }
}

/// SAFETY contract: `host` is null or one of our `clap_host` structs, whose `host_data` points
/// to the `HostState` owned by the same `Instance`.
unsafe fn state<'a>(host: *const clap_host) -> Option<&'a HostState> {
    if host.is_null() {
        return None;
    }
    // SAFETY: non-null host created by `Bundle::instantiate` (contract above).
    let data = unsafe { (*host).host_data } as *const HostState;
    // SAFETY: `host_data` points to the boxed `HostState`, alive as long as the instance.
    unsafe { data.as_ref() }
}

unsafe extern "C" fn host_request_restart(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.restart.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_request_process(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_request_callback(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.callback.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_log(_host: *const clap_host, severity: clap_log_severity, msg: *const c_char) {
    // SAFETY: CLAP passes a NUL-terminated message (or null, handled).
    let m = unsafe { read_cstr(msg) }.unwrap_or_default();
    match severity {
        CLAP_LOG_ERROR | CLAP_LOG_FATAL => log::error!("clap plugin: {m}"),
        CLAP_LOG_WARNING => log::warn!("clap plugin: {m}"),
        _ => log::debug!("clap plugin: {m}"),
    }
}

unsafe extern "C" fn host_params_rescan(host: *const clap_host, _flags: clap_param_rescan_flags) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.params_rescan.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_params_clear(_host: *const clap_host, _id: clap_id, _flags: clap_param_clear_flags) {}

unsafe extern "C" fn host_params_request_flush(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.process.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_latency_changed(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.latency_changed.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_state_mark_dirty(host: *const clap_host) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        s.dirty.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_gui_resize_hints_changed(_host: *const clap_host) {}

unsafe extern "C" fn host_gui_request_resize(host: *const clap_host, width: u32, height: u32) -> bool {
    // Floating windows belong to the plugin, which resizes them itself; an embedded editor's host
    // window is resized by the editor's `idle` on the main thread.
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) }
        && width > 0
        && height > 0
    {
        s.gui_resize.store((u64::from(width) << 32) | u64::from(height), Ordering::Relaxed);
    }
    true
}

unsafe extern "C" fn host_gui_request_show(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_show.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_request_hide(host: *const clap_host) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    match unsafe { state(host) } {
        Some(s) => {
            s.gui_hide.store(true, Ordering::Relaxed);
            true
        }
        None => false,
    }
}

unsafe extern "C" fn host_gui_closed(host: *const clap_host, was_destroyed: bool) {
    // SAFETY: the plugin passes back the host pointer we gave it.
    if let Some(s) = unsafe { state(host) } {
        if was_destroyed {
            s.gui_destroyed.store(true, Ordering::Relaxed);
        }
        s.gui_closed.store(true, Ordering::Relaxed);
    }
}

unsafe extern "C" fn host_timer_register(host: *const clap_host, period_ms: u32, timer_id: *mut clap_id) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it, and an out-pointer (or null).
    match (unsafe { state(host) }.and_then(|s| s.register_timer(period_ms)), unsafe { timer_id.as_mut() }) {
        (Some(id), Some(out)) => {
            *out = id;
            true
        }
        _ => false,
    }
}

unsafe extern "C" fn host_timer_unregister(host: *const clap_host, timer_id: clap_id) -> bool {
    // SAFETY: the plugin passes back the host pointer we gave it.
    unsafe { state(host) }.is_some_and(|s| s.unregister_timer(timer_id))
}

static HOST_STATE: clap_host_state = clap_host_state { mark_dirty: Some(host_state_mark_dirty) };
static HOST_TIMER: clap_host_timer_support =
    clap_host_timer_support { register_timer: Some(host_timer_register), unregister_timer: Some(host_timer_unregister) };
static HOST_GUI: clap_host_gui = clap_host_gui {
    resize_hints_changed: Some(host_gui_resize_hints_changed),
    request_resize: Some(host_gui_request_resize),
    request_show: Some(host_gui_request_show),
    request_hide: Some(host_gui_request_hide),
    closed: Some(host_gui_closed),
};
static HOST_LOG: clap_host_log = clap_host_log { log: Some(host_log) };
static HOST_PARAMS: clap_host_params =
    clap_host_params { rescan: Some(host_params_rescan), clear: Some(host_params_clear), request_flush: Some(host_params_request_flush) };
static HOST_LATENCY: clap_host_latency = clap_host_latency { changed: Some(host_latency_changed) };

unsafe extern "C" fn host_get_extension(_host: *const clap_host, id: *const c_char) -> *const c_void {
    if id.is_null() {
        return std::ptr::null();
    }
    // SAFETY: extension ids are NUL-terminated strings (CLAP contract); non-null checked above.
    let id = unsafe { CStr::from_ptr(id) };
    if id == CLAP_EXT_LOG {
        (&HOST_LOG as *const clap_host_log).cast()
    } else if id == CLAP_EXT_PARAMS {
        (&HOST_PARAMS as *const clap_host_params).cast()
    } else if id == CLAP_EXT_LATENCY {
        (&HOST_LATENCY as *const clap_host_latency).cast()
    } else if id == CLAP_EXT_STATE {
        (&HOST_STATE as *const clap_host_state).cast()
    } else if id == CLAP_EXT_GUI {
        (&HOST_GUI as *const clap_host_gui).cast()
    } else if id == CLAP_EXT_TIMER_SUPPORT {
        (&HOST_TIMER as *const clap_host_timer_support).cast()
    } else {
        // Everything else (thread-check, fd support, …) is unsupported: CLAP allows null.
        std::ptr::null()
    }
}

// ---- preset discovery ----------------------------------------------------------------------

/// Most preset-discovery providers asked per binary, and presets kept.
const MAX_PROVIDERS: u32 = 64;
const MAX_PRESETS: usize = 5000;
/// How deep a declared preset folder is searched.
const MAX_PRESET_DEPTH: usize = 4;

/// One preset a preset-discovery provider reported.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct RawPreset {
    pub name: String,
    /// `CLAP_PRESET_DISCOVERY_LOCATION_FILE` (0) or `_PLUGIN` (1).
    pub kind: u32,
    /// The file (`None` for presets inside the plugin).
    pub location: Option<String>,
    pub load_key: Option<String>,
    /// The CLAP plugin ids it is for (empty: not said).
    pub plugin_ids: Vec<String>,
}

#[derive(Default)]
struct IndexerData {
    /// (kind, location) of each declared location.
    locations: Mutex<Vec<(u32, Option<String>)>>,
    /// File extensions of the declared file types (lower case, no dot); empty: any file.
    extensions: Mutex<Vec<String>>,
}

/// The host's `clap_preset_discovery_indexer` and its data, boxed so their addresses are stable.
struct Indexer {
    raw: clap_preset_discovery_indexer,
    data: Box<IndexerData>,
}

impl Indexer {
    fn new() -> Box<Indexer> {
        let data = Box::<IndexerData>::default();
        let raw = clap_preset_discovery_indexer {
            clap_version: CLAP_VERSION,
            name: c"FreeDAWm".as_ptr(),
            vendor: c"SoundCraft contributors".as_ptr(),
            url: c"https://github.com/s0lidr0ck/FreeDAWm".as_ptr(),
            version: c"0.1.0".as_ptr(),
            indexer_data: (&*data as *const IndexerData).cast_mut().cast::<c_void>(),
            declare_filetype: Some(indexer_declare_filetype),
            declare_location: Some(indexer_declare_location),
            declare_soundpack: Some(indexer_declare_soundpack),
            get_extension: Some(indexer_get_extension),
        };
        Box::new(Indexer { raw, data })
    }
}

/// The indexer's data (`None` for a null indexer).
///
/// SAFETY contract for callers: `indexer` is null or one of our [`Indexer`]s, alive for the call.
unsafe fn indexer_data<'a>(indexer: *const clap_preset_discovery_indexer) -> Option<&'a IndexerData> {
    // SAFETY: per the contract; `indexer_data` points at the boxed `IndexerData` it was made with.
    unsafe { indexer.as_ref().and_then(|i| i.indexer_data.cast::<IndexerData>().as_ref()) }
}

unsafe extern "C" fn indexer_declare_filetype(indexer: *const clap_preset_discovery_indexer, ft: *const clap_preset_discovery_filetype) -> bool {
    // SAFETY: the plugin passes back our indexer and a valid (or null) file type.
    let (Some(d), Some(ft)) = (unsafe { indexer_data(indexer) }, unsafe { ft.as_ref() }) else { return false };
    // SAFETY: a NUL-terminated string or null (CLAP contract).
    let ext = unsafe { read_cstr(ft.file_extension) };
    if let Some(ext) = ext.map(|e| e.trim_start_matches('.').to_lowercase()).filter(|e| !e.is_empty()) {
        let mut l = d.extensions.lock().unwrap_or_else(PoisonError::into_inner);
        if l.len() < 64 {
            l.push(ext);
        }
    }
    true
}

unsafe extern "C" fn indexer_declare_location(indexer: *const clap_preset_discovery_indexer, loc: *const clap_preset_discovery_location) -> bool {
    // SAFETY: the plugin passes back our indexer and a valid (or null) location.
    let (Some(d), Some(loc)) = (unsafe { indexer_data(indexer) }, unsafe { loc.as_ref() }) else { return false };
    // SAFETY: a NUL-terminated string or null (CLAP contract).
    let location = unsafe { read_cstr(loc.location) };
    let mut l = d.locations.lock().unwrap_or_else(PoisonError::into_inner);
    if l.len() < 256 {
        l.push((loc.kind, location));
    }
    true
}

unsafe extern "C" fn indexer_declare_soundpack(_indexer: *const clap_preset_discovery_indexer, _sp: *const clap_preset_discovery_soundpack) -> bool {
    true
}

unsafe extern "C" fn indexer_get_extension(_indexer: *const clap_preset_discovery_indexer, _id: *const c_char) -> *const c_void {
    std::ptr::null()
}

/// The files under `dir` with one of `types`' extensions (any file when `types` is empty).
fn preset_files(dir: &Path, types: &[String], depth: usize, out: &mut Vec<Option<String>>) {
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    for e in rd.flatten() {
        if out.len() >= MAX_PRESETS {
            return;
        }
        let p = e.path();
        if e.file_type().is_ok_and(|t| t.is_dir()) {
            if depth < MAX_PRESET_DEPTH {
                preset_files(&p, types, depth + 1, out);
            }
            continue;
        }
        let ext = p.extension().map(|x| x.to_string_lossy().to_lowercase()).unwrap_or_default();
        if types.is_empty() || types.contains(&ext) {
            out.push(Some(p.to_string_lossy().into_owned()));
        }
    }
}

struct ReceiverData {
    kind: u32,
    location: Option<String>,
    presets: Mutex<Vec<RawPreset>>,
}

/// The host's metadata receiver for one `get_metadata` call and its data, boxed so their
/// addresses are stable.
struct Receiver {
    raw: clap_preset_discovery_metadata_receiver,
    data: Box<ReceiverData>,
}

impl Receiver {
    fn new(kind: u32, location: Option<String>) -> Box<Receiver> {
        let data = Box::new(ReceiverData { kind, location, presets: Mutex::default() });
        let raw = clap_preset_discovery_metadata_receiver {
            receiver_data: (&*data as *const ReceiverData).cast_mut().cast::<c_void>(),
            on_error: Some(receiver_on_error),
            begin_preset: Some(receiver_begin_preset),
            add_plugin_id: Some(receiver_add_plugin_id),
            set_soundpack_id: Some(receiver_str),
            set_flags: Some(receiver_flags),
            add_creator: Some(receiver_str),
            set_description: Some(receiver_str),
            set_timestamps: Some(receiver_timestamps),
            add_feature: Some(receiver_str),
            add_extra_info: Some(receiver_extra),
        };
        Box::new(Receiver { raw, data })
    }

    fn take(&self) -> Vec<RawPreset> {
        std::mem::take(&mut *self.data.presets.lock().unwrap_or_else(PoisonError::into_inner))
    }
}

/// The receiver's data.
///
/// SAFETY contract for callers: `r` is null or one of our [`Receiver`]s, alive for the call.
unsafe fn receiver_data<'a>(r: *const clap_preset_discovery_metadata_receiver) -> Option<&'a ReceiverData> {
    // SAFETY: per the contract; `receiver_data` points at the boxed `ReceiverData`.
    unsafe { r.as_ref().and_then(|r| r.receiver_data.cast::<ReceiverData>().as_ref()) }
}

unsafe extern "C" fn receiver_on_error(_r: *const clap_preset_discovery_metadata_receiver, _os: i32, msg: *const c_char) {
    // SAFETY: a NUL-terminated string or null (CLAP contract).
    let msg = unsafe { read_cstr(msg) }.unwrap_or_default();
    log::warn!("clap preset discovery: {msg}");
}

unsafe extern "C" fn receiver_begin_preset(r: *const clap_preset_discovery_metadata_receiver, name: *const c_char, load_key: *const c_char) -> bool {
    // SAFETY: the plugin passes back our receiver.
    let Some(d) = (unsafe { receiver_data(r) }) else { return false };
    // SAFETY: NUL-terminated strings or null (CLAP contract).
    let (name, load_key) = unsafe { (read_cstr(name), read_cstr(load_key)) };
    let mut list = d.presets.lock().unwrap_or_else(PoisonError::into_inner);
    if list.len() >= MAX_PRESETS {
        return false;
    }
    let file_name = || d.location.as_deref().and_then(|l| Path::new(l).file_stem()).map(|s| s.to_string_lossy().into_owned());
    let name = name.filter(|n| !n.trim().is_empty()).or_else(file_name).unwrap_or_else(|| "Preset".into());
    list.push(RawPreset { name, kind: d.kind, location: d.location.clone(), load_key, plugin_ids: Vec::new() });
    true
}

unsafe extern "C" fn receiver_add_plugin_id(r: *const clap_preset_discovery_metadata_receiver, id: *const clap_universal_plugin_id) {
    // SAFETY: the plugin passes back our receiver and a valid (or null) id.
    let (Some(d), Some(id)) = (unsafe { receiver_data(r) }, unsafe { id.as_ref() }) else { return };
    // SAFETY: NUL-terminated strings or null (CLAP contract).
    let (abi, pid) = unsafe { (read_cstr(id.abi), read_cstr(id.id)) };
    if abi.as_deref() != Some("clap") {
        return;
    }
    let mut list = d.presets.lock().unwrap_or_else(PoisonError::into_inner);
    if let (Some(pid), Some(last)) = (pid, list.last_mut())
        && last.plugin_ids.len() < 64
    {
        last.plugin_ids.push(pid);
    }
}

unsafe extern "C" fn receiver_str(_r: *const clap_preset_discovery_metadata_receiver, _s: *const c_char) {}

unsafe extern "C" fn receiver_flags(_r: *const clap_preset_discovery_metadata_receiver, _flags: u32) {}

unsafe extern "C" fn receiver_timestamps(_r: *const clap_preset_discovery_metadata_receiver, _c: clap_timestamp, _m: clap_timestamp) {}

unsafe extern "C" fn receiver_extra(_r: *const clap_preset_discovery_metadata_receiver, _k: *const c_char, _v: *const c_char) {}

// ---- events --------------------------------------------------------------------------------

/// A host → plugin event, safe to build anywhere; converted to CLAP structs inside `process`.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum Event {
    Param {
        time: u32,
        id: u32,
        value: f64,
    },
    /// `channel` is the MIDI channel, 0–15.
    NoteOn {
        time: u32,
        channel: u8,
        key: u8,
        velocity: f64,
    },
    NoteOff {
        time: u32,
        channel: u8,
        key: u8,
    },
    /// A raw MIDI 1.0 message (for plugins whose note port speaks only MIDI).
    Midi {
        time: u32,
        data: [u8; 3],
    },
}

impl Event {
    pub fn time(&self) -> u32 {
        match *self {
            Event::Param { time, .. } | Event::NoteOn { time, .. } | Event::NoteOff { time, .. } | Event::Midi { time, .. } => time,
        }
    }
    pub fn set_time(&mut self, t: u32) {
        match self {
            Event::Param { time, .. } | Event::NoteOn { time, .. } | Event::NoteOff { time, .. } | Event::Midi { time, .. } => *time = t,
        }
    }
}

#[derive(Clone, Copy)]
enum RawEvent {
    Note(clap_event_note),
    Param(clap_event_param_value),
    Midi(clap_event_midi),
}

impl RawEvent {
    fn header(&self) -> *const clap_event_header {
        // The header is the first field of every `repr(C)` CLAP event struct, so a pointer to it is
        // a pointer to the whole event, as CLAP expects.
        match self {
            RawEvent::Note(e) => &e.header,
            RawEvent::Param(e) => &e.header,
            RawEvent::Midi(e) => &e.header,
        }
    }

    fn from_event(e: &Event, offset: u32) -> RawEvent {
        let header = |size: usize, type_: u16, time: u32| clap_event_header {
            size: u32::try_from(size).unwrap_or(u32::MAX),
            time: time.saturating_sub(offset),
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_,
            flags: 0,
        };
        match *e {
            Event::Param { time, id, value } => RawEvent::Param(clap_event_param_value {
                header: header(size_of::<clap_event_param_value>(), CLAP_EVENT_PARAM_VALUE, time),
                param_id: id,
                cookie: std::ptr::null_mut(),
                note_id: -1,
                port_index: -1,
                channel: -1,
                key: -1,
                value,
            }),
            Event::NoteOn { time, channel, key, velocity } => RawEvent::Note(clap_event_note {
                header: header(size_of::<clap_event_note>(), CLAP_EVENT_NOTE_ON, time),
                note_id: -1,
                port_index: 0,
                channel: i16::from(channel & 0x0F),
                key: i16::from(key),
                velocity,
            }),
            Event::NoteOff { time, channel, key } => RawEvent::Note(clap_event_note {
                header: header(size_of::<clap_event_note>(), CLAP_EVENT_NOTE_OFF, time),
                note_id: -1,
                port_index: 0,
                channel: i16::from(channel & 0x0F),
                key: i16::from(key),
                velocity: 0.0,
            }),
            Event::Midi { time, data } => {
                RawEvent::Midi(clap_event_midi { header: header(size_of::<clap_event_midi>(), CLAP_EVENT_MIDI, time), port_index: 0, data })
            }
        }
    }
}

fn transport_event(t: &Transport) -> clap_event_transport {
    let fixed = |x: f64, factor: i64| if x.is_finite() { (x * factor as f64).round() as i64 } else { 0 };
    let mut flags =
        CLAP_TRANSPORT_HAS_TEMPO | CLAP_TRANSPORT_HAS_BEATS_TIMELINE | CLAP_TRANSPORT_HAS_SECONDS_TIMELINE | CLAP_TRANSPORT_HAS_TIME_SIGNATURE;
    if t.playing {
        flags |= CLAP_TRANSPORT_IS_PLAYING;
    }
    if t.recording {
        flags |= CLAP_TRANSPORT_IS_RECORDING;
    }
    clap_event_transport {
        header: clap_event_header {
            size: u32::try_from(size_of::<clap_event_transport>()).unwrap_or(u32::MAX),
            time: 0,
            space_id: CLAP_CORE_EVENT_SPACE_ID,
            type_: CLAP_EVENT_TRANSPORT,
            flags: 0,
        },
        flags,
        song_pos_beats: fixed(t.quarters, CLAP_BEATTIME_FACTOR),
        song_pos_seconds: fixed(t.seconds, CLAP_SECTIME_FACTOR),
        tempo: t.tempo,
        tempo_inc: 0.0,
        loop_start_beats: 0,
        loop_end_beats: 0,
        loop_start_seconds: 0,
        loop_end_seconds: 0,
        bar_start: fixed(t.bar_start, CLAP_BEATTIME_FACTOR),
        bar_number: i32::try_from(t.bar).unwrap_or(0),
        tsig_num: u16::try_from(t.numerator).unwrap_or(4),
        tsig_denom: u16::try_from(t.denominator).unwrap_or(4),
    }
}

unsafe extern "C" fn in_events_size(list: *const clap_input_events) -> u32 {
    if list.is_null() {
        return 0;
    }
    // SAFETY: `list` is the `clap_input_events` we built in `process`; its ctx is the event Vec.
    let v = unsafe { ((*list).ctx as *const Vec<RawEvent>).as_ref() };
    v.map_or(0, |v| u32::try_from(v.len()).unwrap_or(u32::MAX))
}

unsafe extern "C" fn in_events_get(list: *const clap_input_events, index: u32) -> *const clap_event_header {
    if list.is_null() {
        return std::ptr::null();
    }
    // SAFETY: as in `in_events_size`; the Vec is not mutated while the plugin processes.
    let v = unsafe { ((*list).ctx as *const Vec<RawEvent>).as_ref() };
    v.and_then(|v| v.get(index as usize)).map_or(std::ptr::null(), RawEvent::header)
}

unsafe extern "C" fn out_events_try_push(list: *const clap_output_events, event: *const clap_event_header) -> bool {
    // Plugin → host events are accepted. Parameter values (typically edits in the plugin's GUI)
    // are kept, while there is room, so the UI can write them into the session; the rest is
    // ignored.
    if list.is_null() || event.is_null() {
        return true;
    }
    // SAFETY: CLAP passes a valid event header for the duration of the call.
    let h = unsafe { &*event };
    if h.space_id != CLAP_CORE_EVENT_SPACE_ID || h.type_ != CLAP_EVENT_PARAM_VALUE || (h.size as usize) < size_of::<clap_event_param_value>() {
        return true;
    }
    // SAFETY: the header says this is a complete `clap_event_param_value` (type and size checked).
    let e = unsafe { &*event.cast::<clap_event_param_value>() };
    // SAFETY: `ctx` is null or the instance's preallocated `out_params` Vec, which `process`
    // borrows exclusively for the duration of the plugin call.
    if let Some(v) = unsafe { ((*list).ctx as *mut Vec<(u32, f64)>).as_mut() }
        && v.len() < v.capacity()
        && e.value.is_finite()
    {
        v.push((e.param_id, e.value));
    }
    true
}

// ---- state streams -------------------------------------------------------------------------

/// Read side of an in-memory state blob.
struct ReadCursor<'a> {
    data: &'a [u8],
    pos: usize,
}

unsafe extern "C" fn istream_read(stream: *const clap_istream, buffer: *mut c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the `ReadCursor` on `load_state`'s stack frame, alive during the call.
    let Some(c) = (unsafe { ((*stream).ctx as *mut ReadCursor<'_>).as_mut() }) else { return -1 };
    let rest = c.data.get(c.pos..).unwrap_or(&[]);
    let n = rest.len().min(usize::try_from(size).unwrap_or(usize::MAX));
    if let Some(src) = rest.get(..n) {
        // SAFETY: the plugin provides `size` ≥ `n` writable bytes at `buffer`; `src` is `n`
        // readable bytes of our own slice; they cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
    }
    c.pos += n;
    i64::try_from(n).unwrap_or(-1)
}

unsafe extern "C" fn ostream_write(stream: *const clap_ostream, buffer: *const c_void, size: u64) -> i64 {
    if stream.is_null() || buffer.is_null() {
        return -1;
    }
    // SAFETY: `ctx` is the output Vec on `save_state`'s stack frame, alive during the call.
    let Some(v) = (unsafe { ((*stream).ctx as *mut Vec<u8>).as_mut() }) else { return -1 };
    let Ok(n) = usize::try_from(size) else { return -1 };
    if v.len().saturating_add(n) > MAX_STATE_BYTES {
        return -1;
    }
    // SAFETY: the plugin provides `size` readable bytes at `buffer` for the call.
    let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
    v.extend_from_slice(src);
    i64::try_from(n).unwrap_or(-1)
}

// ---- editor (clap.gui) ---------------------------------------------------------------------

/// True on the process's main thread (AppKit requires GUI calls there).
#[cfg(target_os = "macos")]
pub(crate) fn on_main_thread() -> bool {
    unsafe extern "C" {
        fn pthread_main_np() -> std::ffi::c_int;
    }
    // SAFETY: a libSystem function without arguments or preconditions.
    unsafe { pthread_main_np() == 1 }
}

#[cfg(not(target_os = "macos"))]
pub(crate) fn on_main_thread() -> bool {
    true
}

/// The windowing API of this platform.
fn window_api() -> Option<&'static CStr> {
    if cfg!(target_os = "macos") {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_COCOA)
    } else if cfg!(windows) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_WIN32)
    } else if cfg!(all(unix, not(target_os = "android"))) {
        Some(clap_sys::ext::gui::CLAP_WINDOW_API_X11)
    } else {
        None
    }
}

struct GuiInner {
    plugin: *const clap_plugin,
    gui: *const clap_plugin_gui,
    /// The plugin's `clap.timer-support` (null without it): its timers fire from `idle`.
    timer: *const clap_plugin_timer_support,
    state: *const clap_plugin_state,
    /// The plugin instance exists (cleared by `Instance::drop` before it destroys the plugin).
    alive: bool,
    created: bool,
    visible: bool,
    /// The host window of an embedded editor (`None` for floating editors).
    window: Option<hostwin::HostWindow>,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor is driven and the state saved from the main thread: CLAP
/// allows `clap.gui` and `clap.state` (main-thread) calls concurrently with audio-thread calls.
/// Every such call happens under this lock, and the instance clears `alive` (destroying any GUI)
/// under the same lock before it destroys the plugin, so a handle can never call into a destroyed
/// plugin.
pub(crate) struct GuiLink {
    inner: Mutex<GuiInner>,
    host: Arc<HostState>,
}

impl Default for GuiLink {
    fn default() -> Self {
        GuiLink {
            inner: Mutex::new(GuiInner {
                plugin: std::ptr::null(),
                gui: std::ptr::null(),
                timer: std::ptr::null(),
                state: std::ptr::null(),
                alive: false,
                created: false,
                visible: false,
                window: None,
            }),
            host: Arc::new(HostState::default()),
        }
    }
}

// SAFETY: the raw pointers are only dereferenced under the mutex, while `alive` guarantees the
// plugin exists; CLAP's threading rules (main-thread GUI calls) are checked at each call.
unsafe impl Send for GuiInner {}

impl GuiLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, GuiInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    pub fn available(&self) -> bool {
        let g = self.lock();
        g.alive && !g.gui.is_null()
    }

    pub fn host(&self) -> &HostState {
        &self.host
    }

    fn vt(g: &GuiInner) -> Option<clap_plugin_gui> {
        // SAFETY: `gui` is null or the plugin's static extension vtable, valid while `alive`.
        (g.alive).then(|| unsafe { g.gui.as_ref() }.copied()).flatten()
    }

    /// Opens (creates and shows) the plugin's floating editor window.
    pub fn open(&self, title: &str) -> Result<(), String> {
        if !on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let mut g = self.lock();
        let vt = GuiLink::vt(&g).ok_or_else(|| "this plugin has no editor".to_string())?;
        if g.created {
            if let Some(show) = vt.show {
                // SAFETY: live plugin with a created GUI; main thread (checked).
                unsafe { show(g.plugin) };
            }
            if let Some(w) = &g.window {
                w.raise();
            }
            g.visible = true;
            return Ok(());
        }
        let api = window_api().ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        let (Some(supported), Some(create)) = (vt.is_api_supported, vt.create) else { return Err("incomplete clap.gui extension".into()) };
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread.
        if !unsafe { supported(g.plugin, api.as_ptr(), true) } {
            // SAFETY: as above.
            if hostwin::SUPPORTED && unsafe { supported(g.plugin, api.as_ptr(), false) } {
                return self.open_embedded(&mut g, vt, api, title);
            }
            return Err("this plugin offers only an embedded editor, which FreeDAWm hosts on Windows only".into());
        }
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        // SAFETY: as above; no GUI exists yet.
        if !unsafe { create(g.plugin, api.as_ptr(), true) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        if !cfg!(target_os = "macos")
            && let Some(f) = vt.set_scale
        {
            // SAFETY: live plugin with a created GUI. (Cocoa uses logical pixels: not called.)
            unsafe { f(g.plugin, 1.0) };
        }
        if let (Some(f), Ok(t)) = (vt.suggest_title, CString::new(title)) {
            // SAFETY: live plugin with a created floating GUI; `t` outlives the call.
            unsafe { f(g.plugin, t.as_ptr()) };
        }
        let shown = match vt.show {
            // SAFETY: as above.
            Some(f) => unsafe { f(g.plugin) },
            None => false,
        };
        if !shown {
            GuiLink::destroy(&mut g);
            return Err("the plugin could not show its editor".into());
        }
        g.visible = true;
        Ok(())
    }

    /// Embeds the editor in a host window of ours (plugins that offer no floating editor).
    fn open_embedded(&self, g: &mut GuiInner, vt: clap_plugin_gui, api: &CStr, title: &str) -> Result<(), String> {
        let (Some(create), Some(set_parent)) = (vt.create, vt.set_parent) else { return Err("incomplete clap.gui extension".into()) };
        self.host.gui_closed.store(false, Ordering::Relaxed);
        self.host.gui_destroyed.store(false, Ordering::Relaxed);
        self.host.gui_resize.store(0, Ordering::Relaxed);
        // SAFETY: live plugin; `api` is a static NUL-terminated string; main thread; no GUI yet.
        if !unsafe { create(g.plugin, api.as_ptr(), false) } {
            return Err("the plugin could not create its editor".into());
        }
        g.created = true;
        if let Some(f) = vt.set_scale {
            // SAFETY: live plugin with a created GUI.
            unsafe { f(g.plugin, 1.0) };
        }
        let (mut w, mut h) = (640u32, 480u32);
        if let Some(f) = vt.get_size {
            let (mut a, mut b) = (0u32, 0u32);
            // SAFETY: live plugin with a created GUI; both out-pointers are valid locals.
            if unsafe { f(g.plugin, &mut a, &mut b) } && a > 0 && b > 0 {
                (w, h) = (a.min(16_384), b.min(16_384));
            }
        }
        let win = match hostwin::HostWindow::new(title, f64::from(w), f64::from(h)) {
            Ok(win) => win,
            Err(e) => {
                GuiLink::destroy(g);
                return Err(e);
            }
        };
        let parent = clap_window { api: api.as_ptr(), specific: clap_window_handle { win32: win.parent() } };
        // SAFETY: live plugin with a created embedded GUI; `parent` names our live window and
        // outlives the call.
        if !unsafe { set_parent(g.plugin, &parent) } {
            win.close();
            GuiLink::destroy(g);
            return Err("the editor could not attach to its window".into());
        }
        g.window = Some(win);
        let shown = match vt.show {
            // SAFETY: live plugin with a created, parented GUI.
            Some(f) => unsafe { f(g.plugin) },
            None => true,
        };
        if !shown {
            GuiLink::destroy(g);
            return Err("the plugin could not show its editor".into());
        }
        if let Some(win) = &g.window {
            win.raise();
        }
        g.visible = true;
        Ok(())
    }

    fn destroy(g: &mut GuiInner) {
        if g.created
            && let Some(f) = GuiLink::vt(g).and_then(|v| v.destroy)
        {
            // SAFETY: live plugin with a created GUI; destroyed exactly once.
            unsafe { f(g.plugin) };
        }
        g.created = false;
        g.visible = false;
        // The plugin's child window is gone; now its host window can go too.
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// `clap_plugin_preset_load.from_location` on the live plugin, under this lock. `false`
    /// without the extension, when refused, or once the plugin is gone. Main thread.
    pub fn load_preset(&self, kind: u32, location: Option<&str>, load_key: Option<&str>) -> bool {
        let c = |s: Option<&str>| s.map(CString::new).transpose().ok();
        let (Some(loc), Some(key)) = (c(location), c(load_key)) else { return false };
        let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        let g = self.lock();
        if !g.alive {
            return false;
        }
        // SAFETY: the plugin is alive (under the lock); `get_extension` is its own vtable entry.
        let Some(get) = (unsafe { (*g.plugin).get_extension }) else { return false };
        // SAFETY: valid plugin; the ids are static NUL-terminated strings.
        let mut ext = unsafe { get(g.plugin, CLAP_EXT_PRESET_LOAD.as_ptr()) }.cast::<clap_plugin_preset_load>();
        if ext.is_null() {
            // SAFETY: as above.
            ext = unsafe { get(g.plugin, CLAP_EXT_PRESET_LOAD_COMPAT.as_ptr()) }.cast::<clap_plugin_preset_load>();
        }
        // SAFETY: null or the plugin's static extension vtable.
        let Some(from) = (unsafe { ext.as_ref() }).and_then(|e| e.from_location) else { return false };
        // SAFETY: live plugin; the strings live on this frame for the call.
        unsafe { from(g.plugin, kind, ptr(&loc), ptr(&key)) }
    }

    /// `clap_plugin_state.load` from memory into the live plugin, under this lock. `false`
    /// without the extension, when refused, or once the plugin is gone. Main thread.
    pub fn load_state(&self, data: &[u8]) -> bool {
        if data.len() > MAX_STATE_BYTES {
            return false;
        }
        let g = self.lock();
        if !g.alive {
            return false;
        }
        // SAFETY: `state` is null or the plugin's static extension vtable, valid while `alive`.
        let Some(load) = (unsafe { g.state.as_ref() }).and_then(|s| s.load) else { return false };
        let mut cursor = ReadCursor { data, pos: 0 };
        let stream = clap_istream { ctx: (&mut cursor as *mut ReadCursor<'_>).cast::<c_void>(), read: Some(istream_read) };
        // SAFETY: live plugin (`alive`, under the lock); the stream and its cursor live on this
        // frame for the call.
        unsafe { load(g.plugin, &stream) }
    }

    /// `clap_plugin_state.save` into memory (`None` without the extension, on failure, or once the
    /// plugin is gone).
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        if !g.alive {
            return None;
        }
        // SAFETY: `state` is null or the plugin's static extension vtable, valid while `alive`.
        let save = unsafe { g.state.as_ref() }?.save?;
        let mut out: Vec<u8> = Vec::new();
        let stream = clap_ostream { ctx: (&mut out as *mut Vec<u8>).cast::<c_void>(), write: Some(ostream_write) };
        // SAFETY: live plugin (`alive`, under the lock); the stream and its context live on this
        // frame for the call.
        unsafe { save(g.plugin, &stream) }.then_some(out)
    }

    /// Closes (destroys) the editor.
    pub fn close(&self) {
        if !on_main_thread() {
            return;
        }
        GuiLink::destroy(&mut self.lock());
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        g.alive && g.created && g.visible && g.window.as_ref().is_none_or(hostwin::HostWindow::is_visible)
    }

    /// A picture of the open embedded editor (see `hostwin::capture`); `None` for a floating
    /// editor (the plugin's own window). The lock is released before the plugin is asked to draw,
    /// so a plugin calling back into the host cannot deadlock it; the window is only destroyed on
    /// this (the main) thread, and `hostwin::capture` checks it.
    pub fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
        if !on_main_thread() {
            return None;
        }
        let raw = {
            let g = self.lock();
            if !(g.alive && g.created && g.visible) {
                return None;
            }
            g.window.as_ref()?.raw()
        };
        hostwin::capture(raw)
    }

    /// Main-thread housekeeping, once per UI frame: runs a requested `on_main_thread`, fires
    /// the plugin's due host timers, follows its show/hide/closed requests, services an open
    /// embedded editor's window (its own timers and the paints they invalidated, see
    /// `hostwin::service`) and returns the parameter edits reported since the last call.
    pub fn idle(&self) -> Vec<(u32, f64)> {
        if !on_main_thread() {
            return Vec::new();
        }
        let mut g = self.lock();
        if !g.alive {
            return Vec::new();
        }
        if self.host.callback.swap(false, Ordering::Relaxed) {
            // SAFETY: `plugin` is valid while `alive`; we copy its vtable struct.
            if let Some(f) = unsafe { (*g.plugin).on_main_thread } {
                // SAFETY: live plugin, main thread, requested via `request_callback`.
                unsafe { f(g.plugin) };
            }
        }
        // SAFETY: `timer` is null or the plugin's static extension vtable, valid while `alive`.
        if let Some(on_timer) = unsafe { g.timer.as_ref() }.and_then(|t| t.on_timer) {
            // The timer list is unlocked while the plugin runs: it may (un)register timers there.
            for id in self.host.due_timers() {
                // SAFETY: live plugin, main thread, a timer it registered.
                unsafe { on_timer(g.plugin, id) };
            }
        }
        if g.created {
            let resize = self.host.gui_resize.swap(0, Ordering::Relaxed);
            if resize != 0
                && let Some(win) = &g.window
            {
                let (w, h) = ((resize >> 32) as u32, resize as u32);
                hostwin::resize(win.raw(), f64::from(w), f64::from(h));
                if let Some(f) = GuiLink::vt(&g).and_then(|v| v.set_size) {
                    // SAFETY: live plugin with a created embedded GUI, main thread.
                    unsafe { f(g.plugin, w, h) };
                }
            }
            if g.window.as_ref().is_some_and(|w| !w.is_visible()) {
                // The user closed the embedded editor's host window: free the editor.
                GuiLink::destroy(&mut g);
            } else if self.host.gui_closed.swap(false, Ordering::Relaxed) {
                // The user closed the floating window (or the plugin lost its GUI): free it.
                self.host.gui_destroyed.store(false, Ordering::Relaxed);
                GuiLink::destroy(&mut g);
            } else if let Some(vt) = GuiLink::vt(&g) {
                if self.host.gui_show.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.show
                {
                    // SAFETY: live plugin with a created GUI, main thread.
                    g.visible = unsafe { f(g.plugin) };
                }
                if self.host.gui_hide.swap(false, Ordering::Relaxed)
                    && let Some(f) = vt.hide
                {
                    // SAFETY: as above.
                    unsafe { f(g.plugin) };
                    g.visible = false;
                }
            }
        }
        // An embedded editor's host window, or `Some(None)` for a plugin-owned (floating) one.
        let editor = g.created.then(|| g.window.as_ref().map(hostwin::HostWindow::raw));
        drop(g);
        // The lock is released before the editor's timers and paints run (as in `capture`): a
        // plugin calling back into the host from them cannot deadlock.
        match editor {
            Some(Some(window)) => hostwin::service(window),
            // The plugin's own window: its timers still need the pump. Its paints are not run
            // (no host HWND to `RedrawWindow`), so a floating editor stops repainting while the
            // transport plays: a known gap (see the crate docs and `docs/plugin-parity.md`).
            Some(None) => hostwin::pump_timers(),
            None => {}
        }
        let mut e = self.host.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }

    /// Called by the instance before it destroys the plugin.
    fn kill(&self) {
        let mut g = self.lock();
        if g.created {
            if on_main_thread() {
                GuiLink::destroy(&mut g);
            } else {
                log::warn!("clap: editor still open while its plugin is destroyed off the main thread");
                g.created = false;
                // Window calls are main-thread only: leave the host window rather than crash.
                g.window = None;
            }
        }
        g.alive = false;
    }
}

// ---- audio buffers -------------------------------------------------------------------------

/// Planar f32 buffers for every audio port in one direction, with the pointer tables CLAP needs.
/// Allocated in `new`; `process` only rewrites pointers in place (no allocation).
pub(crate) struct PortBuffers {
    data: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    bufs: Vec<clap_audio_buffer>,
    frames: usize,
}

// SAFETY: the raw pointers only ever point into `data`, which this struct owns; moving the struct
// to another thread moves that ownership with it.
unsafe impl Send for PortBuffers {}

impl PortBuffers {
    /// One buffer set per port, `channels[p]` channels each (capped), `frames` long.
    pub fn new(channels: &[u32], frames: usize) -> PortBuffers {
        let data: Vec<Vec<Vec<f32>>> =
            channels.iter().take(MAX_PORTS as usize).map(|&c| (0..c.min(MAX_PORT_CHANNELS)).map(|_| vec![0.0; frames]).collect()).collect();
        let ptrs = data.iter().map(|p| vec![std::ptr::null_mut(); p.len()]).collect();
        let bufs = data
            .iter()
            .map(|_| clap_audio_buffer { data32: std::ptr::null_mut(), data64: std::ptr::null_mut(), channel_count: 0, latency: 0, constant_mask: 0 })
            .collect();
        PortBuffers { data, ptrs, bufs, frames }
    }

    pub fn channels(&self, port: usize) -> usize {
        self.data.get(port).map_or(0, Vec::len)
    }

    pub fn channel(&self, port: usize, ch: usize) -> Option<&[f32]> {
        self.data.get(port).and_then(|p| p.get(ch)).map(Vec::as_slice)
    }

    pub fn channel_mut(&mut self, port: usize, ch: usize) -> Option<&mut [f32]> {
        self.data.get_mut(port).and_then(|p| p.get_mut(ch)).map(Vec::as_mut_slice)
    }

    /// Points the CLAP tables at the current data (no allocation).
    fn refresh(&mut self) {
        for ((d, p), b) in self.data.iter_mut().zip(self.ptrs.iter_mut()).zip(self.bufs.iter_mut()) {
            for (dc, pc) in d.iter_mut().zip(p.iter_mut()) {
                *pc = dc.as_mut_ptr();
            }
            b.data32 = p.as_mut_ptr();
            b.data64 = std::ptr::null_mut();
            b.channel_count = u32::try_from(p.len()).unwrap_or(0);
            b.constant_mask = 0;
        }
    }
}

// ---- instances -----------------------------------------------------------------------------

/// What one audio port looks like.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct PortInfo {
    pub channels: u32,
    pub main: bool,
}

/// One parameter as reported by `clap_plugin_params.get_info`.
#[derive(Debug, Clone)]
pub(crate) struct RawParam {
    pub id: u32,
    pub name: String,
    pub module: String,
    pub flags: u32,
    pub min: f64,
    pub max: f64,
    pub default: f64,
}

/// Note dialects of the plugin's first input note port.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) struct NotePort {
    pub supported: u32,
    pub preferred: u32,
}

/// A live plugin instance plus the host struct it was created with.
pub(crate) struct Instance {
    plugin: *const clap_plugin,
    params: *const clap_plugin_params,
    ports: *const clap_plugin_audio_ports,
    latency: *const clap_plugin_latency,
    notes: *const clap_plugin_note_ports,
    state_ext: *const clap_plugin_state,
    gui: Arc<GuiLink>,
    raw: Vec<RawEvent>,
    /// Parameter values the plugin reported through output events in the current `process`.
    out_params: Vec<(u32, f64)>,
    active: bool,
    processing: bool,
    max_frames: usize,
    created: bool,
    // Field order matters for drop: the plugin is destroyed in `Drop::drop` first; then the host
    // struct and its state, then the bundle (which keeps the code mapped) go.
    _host: Box<clap_host>,
    state: Arc<HostState>,
    _bundle: Arc<Bundle>,
}

// SAFETY: an `Instance` is used through `&mut self` only, so calls into the plugin are always
// serialised. CLAP lets audio-thread functions run on any one thread at a time; SoundCraft creates
// instances on the engine/UI thread and then moves them into the mix engine (documented
// limitation: some main-thread calls such as `activate` may then happen on the audio thread).
unsafe impl Send for Instance {}

impl Instance {
    fn vt(&self) -> clap_plugin {
        // SAFETY: `plugin` is non-null (checked at creation) and alive until `Drop`.
        unsafe { *self.plugin }
    }

    fn extension<T>(&self, id: &CStr) -> *const T {
        match self.vt().get_extension {
            // SAFETY: valid instance; `id` is a static NUL-terminated string.
            Some(f) => unsafe { f(self.plugin, id.as_ptr()) }.cast::<T>(),
            None => std::ptr::null(),
        }
    }

    pub fn state(&self) -> &HostState {
        &self.state
    }

    /// The editor link shared with editor handles (`None` when the plugin has no `clap.gui`).
    pub fn gui(&self) -> Option<Arc<GuiLink>> {
        self.gui.available().then(|| Arc::clone(&self.gui))
    }

    /// The same link, for state handles (`None` when the plugin has no `clap.state`).
    pub fn state_link(&self) -> Option<Arc<GuiLink>> {
        (!self.state_ext.is_null()).then(|| Arc::clone(&self.gui))
    }

    /// `clap_plugin_preset_load.from_location`: loads a discovered preset. `Err` without the
    /// extension or when the plugin refuses it. Main thread.
    pub fn load_preset(&self, kind: u32, location: Option<&str>, load_key: Option<&str>) -> Result<(), String> {
        let mut ext = self.extension::<clap_plugin_preset_load>(CLAP_EXT_PRESET_LOAD);
        if ext.is_null() {
            ext = self.extension::<clap_plugin_preset_load>(CLAP_EXT_PRESET_LOAD_COMPAT);
        }
        // SAFETY: null or the plugin's static extension vtable.
        let from = unsafe { ext.as_ref() }.and_then(|e| e.from_location).ok_or_else(|| "the plugin cannot load presets".to_string())?;
        let c = |s: Option<&str>| s.map(CString::new).transpose().map_err(|_| "the preset's name contains NUL".to_string());
        let (loc, key) = (c(location)?, c(load_key)?);
        let ptr = |s: &Option<CString>| s.as_ref().map_or(std::ptr::null(), |s| s.as_ptr());
        // SAFETY: valid instance; the strings live on this frame for the call.
        if unsafe { from(self.plugin, kind, ptr(&loc), ptr(&key)) } { Ok(()) } else { Err("the plugin refused the preset".into()) }
    }

    /// `clap_plugin_state.load` from memory. `false` without the extension or when refused.
    pub fn load_state(&self, data: &[u8]) -> bool {
        // SAFETY: the extension pointer, when non-null, points to a static vtable in the plugin.
        let Some(load) = (unsafe { self.state_ext.as_ref() }).and_then(|s| s.load) else { return false };
        let mut cursor = ReadCursor { data, pos: 0 };
        let stream = clap_istream { ctx: (&mut cursor as *mut ReadCursor<'_>).cast::<c_void>(), read: Some(istream_read) };
        // SAFETY: valid instance; the stream and its cursor live on this frame for the call.
        unsafe { load(self.plugin, &stream) }
    }

    /// Runs `on_main_thread` if the plugin asked for a callback.
    pub fn service_callback(&mut self) {
        if self.state.callback.swap(false, Ordering::Relaxed)
            && let Some(f) = self.vt().on_main_thread
        {
            // SAFETY: valid instance; the plugin asked for this call via `request_callback`.
            unsafe { f(self.plugin) };
        }
    }

    pub fn params(&self) -> Vec<RawParam> {
        // SAFETY: the extension pointer, when non-null, points to a static vtable in the plugin.
        let Some(p) = (unsafe { self.params.as_ref() }) else { return Vec::new() };
        let (Some(count), Some(get_info)) = (p.count, p.get_info) else { return Vec::new() };
        // SAFETY: vtable call with the instance it belongs to.
        let n = unsafe { count(self.plugin) }.min(MAX_PARAMS);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: `clap_param_info` is plain data (integers, floats, char arrays, a nullable
            // pointer); all-zero is a valid value.
            let mut info: clap_param_info = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is a valid, writable struct.
            if !unsafe { get_info(self.plugin, i, &mut info) } {
                continue;
            }
            let (min, max) = (info.min_value, info.max_value);
            if !(min.is_finite() && max.is_finite()) || max < min {
                continue;
            }
            let default = if info.default_value.is_finite() { info.default_value.clamp(min, max) } else { min };
            out.push(RawParam { id: info.id, name: read_name(&info.name), module: read_name(&info.module), flags: info.flags, min, max, default });
        }
        out
    }

    pub fn param_value(&self, id: u32) -> Option<f64> {
        // SAFETY: see `params`.
        let p = unsafe { self.params.as_ref() }?;
        let get = p.get_value?;
        let mut v = 0.0f64;
        // SAFETY: valid instance; `v` is writable.
        (unsafe { get(self.plugin, id, &mut v) } && v.is_finite()).then_some(v)
    }

    pub fn value_text(&self, id: u32, value: f64) -> Option<String> {
        // SAFETY: see `params`.
        let p = unsafe { self.params.as_ref() }?;
        let f = p.value_to_text?;
        let mut buf = [0 as c_char; 128];
        // SAFETY: valid instance; we pass the buffer's true capacity.
        let ok = unsafe { f(self.plugin, id, value, buf.as_mut_ptr(), buf.len() as u32) };
        ok.then(|| read_name(&buf)).filter(|s| !s.is_empty())
    }

    /// The plugin's audio ports in one direction. A layout beyond `MAX_PORTS` ports or
    /// `MAX_PORT_CHANNELS` channels per port is an error: we never hand a plugin fewer ports or
    /// channels than it advertised.
    pub fn audio_ports(&self, is_input: bool) -> Result<Vec<PortInfo>, ClapError> {
        // SAFETY: see `params`.
        let Some(p) = (unsafe { self.ports.as_ref() }) else { return Ok(Vec::new()) };
        let (Some(count), Some(get)) = (p.count, p.get) else { return Ok(Vec::new()) };
        let dir = if is_input { "input" } else { "output" };
        // SAFETY: vtable call with its instance.
        let n = unsafe { count(self.plugin, is_input) };
        if n > MAX_PORTS {
            return Err(ClapError::Process(format!("{n} audio {dir} ports (at most {MAX_PORTS} supported)")));
        }
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: plain data; all-zero (null port_type pointer) is valid.
            let mut info: clap_audio_port_info = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if unsafe { get(self.plugin, i, is_input, &mut info) } {
                if info.channel_count > MAX_PORT_CHANNELS {
                    return Err(ClapError::Process(format!(
                        "audio {dir} port {i} has {} channels (at most {MAX_PORT_CHANNELS} supported)",
                        info.channel_count
                    )));
                }
                out.push(PortInfo { channels: info.channel_count, main: info.flags & CLAP_AUDIO_PORT_IS_MAIN != 0 });
            } else {
                // Keep port indices aligned: an unreadable port still needs a (silent) buffer slot.
                out.push(PortInfo { channels: 0, main: false });
            }
        }
        Ok(out)
    }

    pub fn note_port(&self) -> Option<NotePort> {
        // SAFETY: see `params`.
        let p = unsafe { self.notes.as_ref() }?;
        let (count, get) = (p.count?, p.get?);
        // SAFETY: vtable call with its instance.
        if unsafe { count(self.plugin, true) } == 0 {
            return None;
        }
        // SAFETY: plain data; all-zero is valid.
        let mut info: clap_note_port_info = unsafe { std::mem::zeroed() };
        // SAFETY: index 0 is below the non-zero count; `info` is writable.
        unsafe { get(self.plugin, 0, true, &mut info) }.then_some(NotePort { supported: info.supported_dialects, preferred: info.preferred_dialect })
    }

    pub fn latency(&self) -> u32 {
        // SAFETY: see `params`.
        match unsafe { self.latency.as_ref() }.and_then(|l| l.get) {
            // SAFETY: valid instance.
            Some(f) => unsafe { f(self.plugin) },
            None => 0,
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn activate(&mut self, sample_rate: f64, min_frames: u32, max_frames: u32) -> Result<(), ClapError> {
        self.deactivate();
        let f = self.vt().activate.ok_or_else(|| ClapError::Process("plugin has no activate".into()))?;
        // SAFETY: valid, inactive instance; arguments are sane (checked by the caller).
        if !unsafe { f(self.plugin, sample_rate, min_frames, max_frames) } {
            return Err(ClapError::Process("activate failed".into()));
        }
        self.active = true;
        self.max_frames = max_frames as usize;
        Ok(())
    }

    /// `clap.render`: tells the plugin whether it renders offline (a bounce, as fast as the
    /// thread goes) or in realtime. Main thread, while inactive (before `activate`); no-op for
    /// plugins without the extension.
    pub fn set_render_mode(&self, offline: bool) {
        let render: *const clap_plugin_render = self.extension(CLAP_EXT_RENDER);
        // SAFETY: null or the plugin's static extension vtable, valid for the instance's life.
        if let Some(set) = unsafe { render.as_ref() }.and_then(|r| r.set) {
            // SAFETY: valid, inactive instance on the main thread.
            unsafe { set(self.plugin, if offline { CLAP_RENDER_OFFLINE } else { CLAP_RENDER_REALTIME }) };
        }
    }

    pub fn deactivate(&mut self) {
        self.stop_processing();
        if self.active {
            if let Some(f) = self.vt().deactivate {
                // SAFETY: valid, active instance.
                unsafe { f(self.plugin) };
            }
            self.active = false;
        }
    }

    fn stop_processing(&mut self) {
        if self.processing {
            if let Some(f) = self.vt().stop_processing {
                // SAFETY: valid instance in the processing state.
                unsafe { f(self.plugin) };
            }
            self.processing = false;
        }
    }

    pub fn reset(&mut self) {
        if self.active
            && let Some(f) = self.vt().reset
        {
            // SAFETY: valid, active instance.
            unsafe { f(self.plugin) };
        }
    }

    /// Runs one `clap_process` call over `frames` frames. Events must be sorted by time; their
    /// times are made relative to `offset`. Returns the CLAP process status.
    pub fn process(
        &mut self,
        frames: usize,
        steady_time: i64,
        inputs: &mut PortBuffers,
        outputs: &mut PortBuffers,
        events: &[Event],
        offset: u32,
        transport: Option<&Transport>,
    ) -> Result<i32, ClapError> {
        if !self.active {
            return Err(ClapError::Process("not active".into()));
        }
        if frames == 0 || frames > self.max_frames || frames > inputs.frames || frames > outputs.frames {
            return Err(ClapError::Process("block larger than the activated size".into()));
        }
        if !self.processing {
            let ok = match self.vt().start_processing {
                // SAFETY: valid, active instance.
                Some(f) => unsafe { f(self.plugin) },
                None => true,
            };
            if !ok {
                return Err(ClapError::Process("start_processing failed".into()));
            }
            self.processing = true;
        }
        let process = self.vt().process.ok_or_else(|| ClapError::Process("plugin has no process".into()))?;
        self.raw.clear();
        for e in events.iter().take(self.raw.capacity()) {
            self.raw.push(RawEvent::from_event(e, offset));
        }
        inputs.refresh();
        outputs.refresh();
        let in_events = clap_input_events {
            ctx: (&self.raw as *const Vec<RawEvent>).cast_mut().cast::<c_void>(),
            size: Some(in_events_size),
            get: Some(in_events_get),
        };
        self.out_params.clear();
        let out_events =
            clap_output_events { ctx: (&mut self.out_params as *mut Vec<(u32, f64)>).cast::<c_void>(), try_push: Some(out_events_try_push) };
        let transport = transport.map(transport_event);
        let p = clap_process {
            steady_time,
            frames_count: u32::try_from(frames).unwrap_or(0),
            transport: transport.as_ref().map_or(std::ptr::null(), std::ptr::from_ref),
            audio_inputs: inputs.bufs.as_ptr(),
            audio_outputs: outputs.bufs.as_mut_ptr(),
            audio_inputs_count: u32::try_from(inputs.bufs.len()).unwrap_or(0),
            audio_outputs_count: u32::try_from(outputs.bufs.len()).unwrap_or(0),
            in_events: &in_events,
            out_events: &out_events,
        };
        // SAFETY: valid, processing instance. Every buffer pointer refers to `frames` (or more)
        // valid f32s owned by `inputs`/`outputs`, which are borrowed for the whole call; the event
        // lists and their contexts and the transport live on this stack frame / in `self.raw`,
        // untouched until the call returns.
        let status = unsafe { process(self.plugin, &p) };
        if !self.out_params.is_empty()
            && let Ok(mut edits) = self.state.edits.try_lock()
        {
            // Never blocks or grows: a busy UI or a full queue drops the edits.
            for &e in &self.out_params {
                if edits.len() < edits.capacity() {
                    edits.push(e);
                }
            }
        }
        self.out_params.clear();
        if status == CLAP_PROCESS_ERROR { Err(ClapError::Process("plugin reported a processing error".into())) } else { Ok(status) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Editors first (CLAP: destroy the GUI before the plugin), and no handle may call in after.
        self.gui.kill();
        self.deactivate();
        if self.created
            && let Some(f) = self.vt().destroy
        {
            // SAFETY: valid instance, deactivated above; destroyed exactly once.
            unsafe { f(self.plugin) };
        }
        self.created = false;
    }
}

// ---- host window for embedded editors ------------------------------------------------------

/// The Windows host window for embedded editors: a top-level window owned by the app's active
/// window (no taskbar button) whose client area is the plugin's parent `HWND`. Closing it only
/// hides it; the editor link notices in `idle`, destroys the plugin's GUI, then the window.
/// Messages are dispatched by the app's own event loop on the main thread.
#[cfg(windows)]
mod hostwin {
    use std::ffi::c_void;
    use std::sync::OnceLock;
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{Duration, Instant};
    use windows_sys::Win32::Foundation::{HWND, LPARAM, LRESULT, POINT, RECT, WPARAM};
    use windows_sys::Win32::Graphics::Gdi::{
        BI_RGB, BITMAPINFO, BITMAPINFOHEADER, CreateCompatibleDC, CreateDIBSection, DIB_RGB_COLORS, DeleteDC, DeleteObject, GdiFlush, GetDC,
        GetMonitorInfoW, MONITOR_DEFAULTTONEAREST, MONITORINFO, MonitorFromPoint, MonitorFromWindow, RDW_ALLCHILDREN, RDW_UPDATENOW, RGBQUAD,
        RedrawWindow, ReleaseDC, SelectObject,
    };
    use windows_sys::Win32::Storage::Xps::{PW_CLIENTONLY, PrintWindow};
    use windows_sys::Win32::System::LibraryLoader::GetModuleHandleW;
    use windows_sys::Win32::UI::Input::KeyboardAndMouse::GetActiveWindow;
    use windows_sys::Win32::UI::WindowsAndMessaging::{
        AdjustWindowRectEx, CS_HREDRAW, CS_VREDRAW, CW_USEDEFAULT, CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GWL_EXSTYLE,
        GWL_STYLE, GetClientRect, GetCursorPos, GetWindowLongPtrW, GetWindowRect, IDC_ARROW, IsIconic, IsWindow, IsWindowVisible, LoadCursorW, MSG,
        PM_REMOVE, PW_RENDERFULLCONTENT, PeekMessageW, RegisterClassExW, SW_HIDE, SW_RESTORE, SW_SHOW, SWP_NOACTIVATE, SWP_NOMOVE, SWP_NOZORDER,
        SetForegroundWindow, SetWindowPos, ShowWindow, WM_CLOSE, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN, WS_OVERLAPPED, WS_SYSMENU,
    };

    pub const SUPPORTED: bool = true;

    /// Editors are serviced at most this often (60 Hz), whatever the UI's frame rate.
    const TICK: Duration = Duration::from_millis(15);
    /// Most timer messages dispatched per tick: a plugin re-arming a timer from its own timer
    /// procedure cannot keep the UI frame.
    const MAX_TIMER_MESSAGES: usize = 64;

    /// True at most once per [`TICK`], for the whole thread (one timer pump serves every editor).
    fn tick_due() -> bool {
        static EPOCH: OnceLock<Instant> = OnceLock::new();
        static LAST: AtomicU64 = AtomicU64::new(0);
        let now = u64::try_from(EPOCH.get_or_init(Instant::now).elapsed().as_micros()).unwrap_or(u64::MAX).saturating_add(1);
        let last = LAST.load(Ordering::Relaxed);
        if now.saturating_sub(last) < u64::try_from(TICK.as_micros()).unwrap_or(u64::MAX) {
            return false;
        }
        LAST.store(now, Ordering::Relaxed);
        true
    }

    /// Dispatches this thread's pending `WM_TIMER` messages, at most once per [`TICK`].
    ///
    /// Win32 retrieves timer messages (and paints) only when nothing else is queued. While the
    /// transport plays, the app's own window always has a repaint pending (eframe re-requests
    /// it before the event loop returns to `PeekMessageW`), so a plugin editor's timer-driven
    /// repaints never ran: a knob dragged during playback moved in the audio but not on screen
    /// until Pause. A `PeekMessageW` filtered to `WM_TIMER` alone bypasses that priority rule.
    /// The pump is thread-wide, not filtered to the editor's window: the app's loop (winit
    /// 0.30.13, eframe 0.36; checked: no `SetTimer`/`WM_TIMER` in its Windows backend) uses no
    /// timers, so every timer message dispatched here is a plugin's. A winit upgrade that adds
    /// a timer would have it dispatched from here, out of order with its other messages
    /// (harmless, but re-check on upgrade; a thread timer with a `TIMERPROC` is run by
    /// `DispatchMessageW` either way). Main thread only.
    pub fn pump_timers() {
        if on_main_thread() && tick_due() {
            pump_timers_now();
        }
    }

    fn pump_timers_now() {
        // SAFETY: `MSG` is plain data; all-zero is a valid value to be overwritten.
        let mut msg: MSG = unsafe { std::mem::zeroed() };
        for _ in 0..MAX_TIMER_MESSAGES {
            // SAFETY: `msg` is writable; null asks for every window of this thread; the range
            // keeps everything but timer messages in the queue.
            if unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), WM_TIMER, WM_TIMER, PM_REMOVE) } == 0 {
                break;
            }
            // SAFETY: a message just retrieved for this thread.
            unsafe { DispatchMessageW(&msg) };
        }
    }

    /// Per-frame service of an open embedded editor's host window (`window` from
    /// [`HostWindow::raw`]): the thread's due timers (see `pump_timers`), then the paints they
    /// invalidated (`RDW_UPDATENOW` sends `WM_PAINT` to the window and its children now, instead
    /// of when the queue is otherwise empty; never done for the app's own window, whose paint is
    /// eframe's). Main thread only; a few microseconds when nothing is due.
    pub fn service(window: *mut c_void) {
        if on_main_thread() {
            service_window(window);
        }
    }

    /// `service` without the thread check (any thread with a message queue; tests use it).
    fn service_window(window: *mut c_void) {
        // SAFETY: `IsWindow`, `IsWindowVisible` and `IsIconic` accept any value.
        if window.is_null() || unsafe { IsWindow(window) } == 0 || unsafe { IsWindowVisible(window) } == 0 || unsafe { IsIconic(window) } != 0 {
            return;
        }
        if tick_due() {
            pump_timers_now();
        }
        // SAFETY: a live, shown window of this thread; null rect and region mean the whole window.
        unsafe { RedrawWindow(window, std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN) };
    }

    /// Rust names the process's first thread "main"; the app runs its UI (and message loop) there.
    fn on_main_thread() -> bool {
        std::thread::current().name() == Some("main")
    }

    fn wide(s: &str) -> Vec<u16> {
        s.encode_utf16().filter(|&c| c != 0).chain(Some(0)).collect()
    }

    const STYLE: u32 = WS_OVERLAPPED | WS_CAPTION | WS_SYSMENU | WS_CLIPCHILDREN;

    unsafe extern "system" fn wndproc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if msg == WM_CLOSE {
            // SAFETY: `hwnd` is the window this procedure was called for.
            unsafe { ShowWindow(hwnd, SW_HIDE) };
            return 0;
        }
        // SAFETY: default handling of a message for the window it was sent to.
        unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) }
    }

    /// Registers the window class once; the name lives for the whole process.
    fn class_name() -> Option<&'static [u16]> {
        static CLASS: OnceLock<Option<Vec<u16>>> = OnceLock::new();
        CLASS
            .get_or_init(|| {
                let name = wide("SoundCraftClapEditor");
                // SAFETY: null asks for this process's module handle.
                let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
                let wc = WNDCLASSEXW {
                    cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                    style: CS_HREDRAW | CS_VREDRAW,
                    lpfnWndProc: Some(wndproc),
                    cbClsExtra: 0,
                    cbWndExtra: 0,
                    hInstance: instance,
                    hIcon: std::ptr::null_mut(),
                    // SAFETY: a system cursor (null instance + predefined id).
                    hCursor: unsafe { LoadCursorW(std::ptr::null_mut(), IDC_ARROW) },
                    hbrBackground: std::ptr::null_mut(),
                    lpszMenuName: std::ptr::null(),
                    lpszClassName: name.as_ptr(),
                    hIconSm: std::ptr::null_mut(),
                };
                // SAFETY: a fully initialised class description whose strings outlive the call
                // (the name is kept for the process lifetime in this `OnceLock`).
                (unsafe { RegisterClassExW(&wc) } != 0).then_some(name)
            })
            .as_deref()
    }

    /// Outer window size for a client area of `w`×`h`.
    fn outer_size(style: u32, ex_style: u32, w: f64, h: f64) -> (i32, i32) {
        let mut r = RECT { left: 0, top: 0, right: w as i32, bottom: h as i32 };
        // SAFETY: `r` is a valid, writable rect.
        unsafe { AdjustWindowRectEx(&mut r, style, 0, ex_style) };
        (r.right.saturating_sub(r.left), r.bottom.saturating_sub(r.top))
    }

    /// A picture of `hwnd`'s client area (the plugin's view and its children) as `(width, height,
    /// RGBA)`, rows from the top. `PrintWindow` with `PW_RENDERFULLCONTENT` asks the compositor
    /// for what is on screen, so GPU-drawn editors come back too (or blank, which callers check).
    /// Main thread only (the window's thread); it never involves the audio thread.
    fn capture_client(hwnd: HWND) -> Option<(u32, u32, Vec<u8>)> {
        let mut r = RECT { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: a live window of this thread (callers check); `r` is writable.
        if unsafe { GetClientRect(hwnd, &mut r) } == 0 {
            return None;
        }
        let (w, h) = (r.right.saturating_sub(r.left).min(16_384), r.bottom.saturating_sub(r.top).min(16_384));
        if w <= 0 || h <= 0 {
            return None;
        }
        // SAFETY: as above; released below.
        let screen = unsafe { GetDC(hwnd) };
        if screen.is_null() {
            return None;
        }
        // SAFETY: `screen` is a valid DC; the memory DC is deleted below.
        let mem = unsafe { CreateCompatibleDC(screen) };
        let mut out = None;
        if !mem.is_null() {
            let header = BITMAPINFOHEADER {
                biSize: std::mem::size_of::<BITMAPINFOHEADER>() as u32,
                biWidth: w,
                // Negative: a top-down bitmap (first row first).
                biHeight: -h,
                biPlanes: 1,
                biBitCount: 32,
                biCompression: BI_RGB,
                biSizeImage: 0,
                biXPelsPerMeter: 0,
                biYPelsPerMeter: 0,
                biClrUsed: 0,
                biClrImportant: 0,
            };
            let info = BITMAPINFO { bmiHeader: header, bmiColors: [RGBQUAD { rgbBlue: 0, rgbGreen: 0, rgbRed: 0, rgbReserved: 0 }] };
            let mut bits: *mut c_void = std::ptr::null_mut();
            // SAFETY: a valid DC and a fully initialised 32-bit description; `bits` receives the
            // pixel memory the section owns until it is deleted below.
            let bmp = unsafe { CreateDIBSection(mem, &info, DIB_RGB_COLORS, &mut bits, std::ptr::null_mut(), 0) };
            if !bmp.is_null() {
                // SAFETY: our DC and bitmap. Null means it could not be selected.
                let old = if bits.is_null() { std::ptr::null_mut() } else { unsafe { SelectObject(mem, bmp) } };
                if !old.is_null() {
                    // SAFETY: a live window of this thread and our memory DC with the section in it.
                    let printed = unsafe { PrintWindow(hwnd, mem, PW_CLIENTONLY | PW_RENDERFULLCONTENT) } != 0;
                    // SAFETY: flushes this thread's GDI batch so the section holds the drawing.
                    unsafe { GdiFlush() };
                    if printed {
                        let len = (w as usize) * (h as usize) * 4;
                        // SAFETY: the section is `w`×`h` 32-bit pixels, top-down with no row
                        // padding (32-bit rows are DWORD-aligned), alive until `DeleteObject` below.
                        let px = unsafe { std::slice::from_raw_parts(bits.cast::<u8>(), len) };
                        let mut rgba = Vec::with_capacity(len);
                        for p in px.chunks_exact(4) {
                            rgba.extend_from_slice(&[p[2], p[1], p[0], 255]);
                        }
                        out = Some((w as u32, h as u32, rgba));
                    }
                    // SAFETY: puts the DC's own bitmap back, so ours is no longer selected.
                    unsafe { SelectObject(mem, old) };
                }
                // SAFETY: our bitmap, not selected into any DC; deleted in every case.
                unsafe { DeleteObject(bmp) };
            }
            // SAFETY: our memory DC.
            unsafe { DeleteDC(mem) };
        }
        // SAFETY: the DC obtained above for this window.
        unsafe { ReleaseDC(hwnd, screen) };
        out
    }

    /// Owns the window handle; destroyed in `close` (main thread only).
    pub struct HostWindow {
        hwnd: HWND,
    }

    // SAFETY: the handle is only used on the main thread (every method checks); the struct
    // merely carries it between threads' locks.
    unsafe impl Send for HostWindow {}

    impl HostWindow {
        fn live(&self) -> Option<HWND> {
            // SAFETY: `IsWindow` accepts any value and only reports whether it names a window.
            (on_main_thread() && unsafe { IsWindow(self.hwnd) } != 0).then_some(self.hwnd)
        }

        pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
            if !on_main_thread() {
                return Err("plugin editors must be opened on the main thread".into());
            }
            let class = class_name().ok_or_else(|| "cannot register the editor window class".to_string())?;
            let title = wide(title);
            let (ow, oh) = outer_size(STYLE, 0, w, h);
            // SAFETY: main thread; the active window (or null) of this thread's queue.
            let owner = unsafe { GetActiveWindow() };
            // SAFETY: registered class and NUL-terminated strings that outlive the call; the
            // owner is null or a window of this thread.
            let hwnd = unsafe {
                CreateWindowExW(
                    0,
                    class.as_ptr(),
                    title.as_ptr(),
                    STYLE,
                    CW_USEDEFAULT,
                    CW_USEDEFAULT,
                    ow,
                    oh,
                    owner,
                    std::ptr::null_mut(),
                    GetModuleHandleW(std::ptr::null()),
                    std::ptr::null(),
                )
            };
            if hwnd.is_null() {
                return Err("cannot create the editor window".into());
            }
            keep_on_screen(hwnd, true);
            Ok(HostWindow { hwnd })
        }

        /// The window handle (for resizes).
        pub fn raw(&self) -> *mut c_void {
            self.hwnd
        }

        /// The `HWND` the plugin creates its child window in.
        pub fn parent(&self) -> *mut c_void {
            self.live().unwrap_or(std::ptr::null_mut())
        }

        /// Shows the window on top, restored first when it is minimised (a minimised window has
        /// no client area to capture).
        pub fn raise(&self) {
            if let Some(h) = self.live() {
                // SAFETY: a live window of this thread.
                unsafe {
                    ShowWindow(h, if IsIconic(h) != 0 { SW_RESTORE } else { SW_SHOW });
                    SetForegroundWindow(h);
                }
            }
        }

        pub fn is_visible(&self) -> bool {
            // SAFETY: a live window of this thread.
            self.live().is_some_and(|h| unsafe { IsWindowVisible(h) } != 0)
        }

        pub fn close(self) {
            if let Some(h) = self.live() {
                // SAFETY: our own live window; the plugin's GUI was destroyed first.
                unsafe { DestroyWindow(h) };
            }
        }
    }

    /// Resizes a host window's client area (`window` from [`HostWindow::raw`]); main thread only.
    pub fn resize(window: *mut c_void, w: f64, h: f64) {
        // SAFETY: `IsWindow` accepts any value.
        if !on_main_thread() || window.is_null() || unsafe { IsWindow(window) } == 0 {
            return;
        }
        // SAFETY: a live window of this thread; style values are read, not changed.
        let (style, ex) = unsafe { (GetWindowLongPtrW(window, GWL_STYLE) as u32, GetWindowLongPtrW(window, GWL_EXSTYLE) as u32) };
        let (ow, oh) = outer_size(style, ex, w, h);
        // SAFETY: as above; only the size changes.
        unsafe { SetWindowPos(window, std::ptr::null_mut(), 0, 0, ow, oh, SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE) };
        keep_on_screen(window, false);
    }

    /// Moves `hwnd` so all of it is on one monitor's work area (spec 2026-11-02 item 6): when it
    /// opens (`opening`), next to the pointer (where its slot was clicked) on the monitor under
    /// it; after a resize, on the monitor it is on. Larger than the monitor: its top-left stays in
    /// view at the largest size that fits (`soundcraft_dsp::fit_window`). Main thread only.
    fn keep_on_screen(hwnd: HWND, opening: bool) {
        // SAFETY: `RECT` is plain data; all-zero is valid and is overwritten.
        let mut wr: RECT = unsafe { std::mem::zeroed() };
        // SAFETY: a window of this thread (just created or checked live by the caller); `wr` is writable.
        if unsafe { GetWindowRect(hwnd, &mut wr) } == 0 {
            return;
        }
        let mut pt = POINT { x: wr.left, y: wr.top };
        // SAFETY: `pt` is writable; on failure it keeps the window's own corner.
        let at_pointer = opening && unsafe { GetCursorPos(&mut pt) } != 0;
        // SAFETY: both accept any point or window and return a monitor (the nearest one).
        let monitor = if at_pointer {
            unsafe { MonitorFromPoint(pt, MONITOR_DEFAULTTONEAREST) }
        } else {
            unsafe { MonitorFromWindow(hwnd, MONITOR_DEFAULTTONEAREST) }
        };
        // SAFETY: plain data; `cbSize` is set before the call reads it.
        let mut mi: MONITORINFO = unsafe { std::mem::zeroed() };
        mi.cbSize = u32::try_from(std::mem::size_of::<MONITORINFO>()).unwrap_or(0);
        // SAFETY: a monitor handle from the call above; `mi` is writable with its size set.
        if unsafe { GetMonitorInfoW(monitor, &mut mi) } == 0 {
            return;
        }
        let (w, h) = (wr.right.saturating_sub(wr.left), wr.bottom.saturating_sub(wr.top));
        let start = if at_pointer {
            let (x, y) = (pt.x.saturating_add(16), pt.y.saturating_add(16));
            [x, y, x.saturating_add(w), y.saturating_add(h)]
        } else {
            [wr.left, wr.top, wr.right, wr.bottom]
        };
        let work = mi.rcWork;
        let [l, t, r, b] = soundcraft_dsp::fit_window(start, [work.left, work.top, work.right, work.bottom]);
        if [l, t, r, b] != [wr.left, wr.top, wr.right, wr.bottom] {
            // SAFETY: a window of this thread; only its place and size change.
            unsafe { SetWindowPos(hwnd, std::ptr::null_mut(), l, t, r - l, b - t, SWP_NOZORDER | SWP_NOACTIVATE) };
        }
    }

    /// A picture of a shown host window's client area (`window` from [`HostWindow::raw`]), see
    /// `capture_client`; `None` off the main thread or for a hidden or destroyed window.
    pub fn capture(window: *mut c_void) -> Option<(u32, u32, Vec<u8>)> {
        // SAFETY: `IsWindow` and `IsWindowVisible` accept any value.
        if !on_main_thread() || window.is_null() || unsafe { IsWindow(window) } == 0 || unsafe { IsWindowVisible(window) } == 0 {
            return None;
        }
        capture_client(window)
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;
        use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, KillTimer, LWA_ALPHA, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, SetTimer, WM_PAINT,
            WS_EX_LAYERED, WS_EX_NOACTIVATE, WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
        };

        /// A stand-in for a plugin editor: its timer invalidates it, its paint counts.
        static TIMERS: AtomicUsize = AtomicUsize::new(0);
        static PAINTS: AtomicUsize = AtomicUsize::new(0);

        unsafe extern "system" fn fake_editor_proc(hwnd: HWND, msg: u32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
            match msg {
                WM_TIMER => {
                    TIMERS.fetch_add(1, Ordering::Relaxed);
                    // SAFETY: the window this procedure was called for; null = the whole client area.
                    unsafe { InvalidateRect(hwnd, std::ptr::null(), 0) };
                    0
                }
                WM_PAINT => {
                    PAINTS.fetch_add(1, Ordering::Relaxed);
                    // SAFETY: as above.
                    unsafe { ValidateRect(hwnd, std::ptr::null()) };
                    0
                }
                // SAFETY: default handling of a message for the window it was sent to.
                _ => unsafe { DefWindowProcW(hwnd, msg, wparam, lparam) },
            }
        }

        /// A tiny, nearly transparent, never-activated tool window of this thread, shown so
        /// that it paints (hidden windows get no `WM_PAINT`).
        fn fake_editor() -> HWND {
            let name = wide("SoundCraftClapEditorServiceTest");
            // SAFETY: null asks for this process's module handle.
            let instance = unsafe { GetModuleHandleW(std::ptr::null()) };
            let wc = WNDCLASSEXW {
                cbSize: std::mem::size_of::<WNDCLASSEXW>() as u32,
                style: 0,
                lpfnWndProc: Some(fake_editor_proc),
                cbClsExtra: 0,
                cbWndExtra: 0,
                hInstance: instance,
                hIcon: std::ptr::null_mut(),
                hCursor: std::ptr::null_mut(),
                hbrBackground: std::ptr::null_mut(),
                lpszMenuName: std::ptr::null(),
                lpszClassName: name.as_ptr(),
                hIconSm: std::ptr::null_mut(),
            };
            // SAFETY: a fully initialised class description; `name` outlives the registration
            // and the window creation below (the class copies the name).
            let atom = unsafe { RegisterClassExW(&wc) };
            assert_ne!(atom, 0, "register the fake editor class");
            let ex = WS_EX_LAYERED | WS_EX_TOOLWINDOW | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE;
            // SAFETY: registered class and NUL-terminated strings that outlive the call.
            let hwnd = unsafe {
                CreateWindowExW(
                    ex,
                    name.as_ptr(),
                    name.as_ptr(),
                    WS_POPUP,
                    0,
                    0,
                    8,
                    8,
                    std::ptr::null_mut(),
                    std::ptr::null_mut(),
                    instance,
                    std::ptr::null(),
                )
            };
            assert!(!hwnd.is_null(), "create the fake editor window");
            // SAFETY: our live layered window.
            unsafe {
                SetLayeredWindowAttributes(hwnd, 0, 1, LWA_ALPHA);
                ShowWindow(hwnd, SW_SHOWNOACTIVATE);
            }
            hwnd
        }

        #[test]
        fn an_editors_timer_and_paint_run_while_the_thread_never_waits_for_messages() {
            let hwnd = fake_editor();
            // SAFETY: our live window; a 10 ms timer delivered as WM_TIMER to its procedure.
            assert_ne!(unsafe { SetTimer(hwnd, 1, 10, None) }, 0);
            // A thread that never retrieves messages (the symptom: the app's loop always finds
            // its own repaint first) runs neither the timer nor the paint.
            std::thread::sleep(Duration::from_millis(150));
            assert_eq!((TIMERS.load(Ordering::Relaxed), PAINTS.load(Ordering::Relaxed)), (0, 0), "nothing runs by itself");
            // One second of UI frames with the transport playing: each frame services the editor.
            for _ in 0..60 {
                service_window(hwnd);
                std::thread::sleep(Duration::from_millis(16));
            }
            let (timers, paints) = (TIMERS.load(Ordering::Relaxed), PAINTS.load(Ordering::Relaxed));
            assert!(timers >= 20, "the editor's timer ran {timers} times in a second of frames");
            assert!(paints >= 20, "the editor repainted {paints} times in a second of frames");
            // SAFETY: our own window and timer.
            unsafe {
                KillTimer(hwnd, 1);
                DestroyWindow(hwnd);
            }
            // A destroyed or hidden window is left alone.
            service_window(hwnd);
            service_window(std::ptr::null_mut());
        }
    }
}

/// Other platforms: embedded editors are not hosted (floating ones still are).
#[cfg(not(windows))]
mod hostwin {
    use std::ffi::c_void;

    pub const SUPPORTED: bool = false;

    pub struct HostWindow;

    impl HostWindow {
        pub fn new(_title: &str, _w: f64, _h: f64) -> Result<HostWindow, String> {
            Err("embedded plugin editors are not hosted on this platform".into())
        }
        pub fn raw(&self) -> *mut c_void {
            std::ptr::null_mut()
        }
        pub fn parent(&self) -> *mut c_void {
            std::ptr::null_mut()
        }
        pub fn raise(&self) {}
        pub fn is_visible(&self) -> bool {
            false
        }
        pub fn close(self) {}
    }

    pub fn resize(_window: *mut c_void, _w: f64, _h: f64) {}

    /// Editor pictures are taken on Windows only.
    pub fn capture(_window: *mut c_void) -> Option<(u32, u32, Vec<u8>)> {
        None
    }

    /// Nothing to service: the platform's run loop drives the editor's timers and drawing.
    pub fn service(_window: *mut c_void) {}

    pub fn pump_timers() {}
}

/// Puts a scanner helper in a Windows job object that is killed when this process ends (the job
/// handle is never closed, so the system closes it at exit, crash included): a helper never
/// outlives the app. Elsewhere it does nothing (helpers stop on their own once their batch is
/// done, and the runner stops a hung one).
pub(crate) fn tie_to_this_process(child: &std::process::Child) {
    #[cfg(windows)]
    {
        use std::os::windows::io::AsRawHandle;
        use windows_sys::Win32::System::JobObjects::{
            AssignProcessToJobObject, CreateJobObjectW, JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE, JOBOBJECT_EXTENDED_LIMIT_INFORMATION,
            JobObjectExtendedLimitInformation, SetInformationJobObject,
        };
        static JOB: OnceLock<usize> = OnceLock::new();
        let job = *JOB.get_or_init(|| {
            // SAFETY: no security attributes and no name: plain arguments, returns a handle or null.
            let h = unsafe { CreateJobObjectW(std::ptr::null(), std::ptr::null()) };
            if h.is_null() {
                log::warn!("plugin scan: no job object; a scanner may outlive a crash of the app");
                return 0;
            }
            let mut info = JOBOBJECT_EXTENDED_LIMIT_INFORMATION::default();
            info.BasicLimitInformation.LimitFlags = JOB_OBJECT_LIMIT_KILL_ON_JOB_CLOSE;
            let size = std::mem::size_of::<JOBOBJECT_EXTENDED_LIMIT_INFORMATION>() as u32;
            // SAFETY: `h` is the job just created; `info` lives across the call and `size` is its size.
            let ok = unsafe { SetInformationJobObject(h, JobObjectExtendedLimitInformation, (&raw const info).cast(), size) };
            if ok == 0 {
                log::warn!("plugin scan: cannot set the job object's kill-on-close limit");
            }
            // Kept open for the life of the process, on purpose.
            h as usize
        });
        if job != 0 {
            // SAFETY: `job` is the job handle above (open for the process's life); the child's
            // handle is valid while `child` is borrowed.
            let ok = unsafe {
                AssignProcessToJobObject(
                    job as windows_sys::Win32::Foundation::HANDLE,
                    child.as_raw_handle() as windows_sys::Win32::Foundation::HANDLE,
                )
            };
            if ok == 0 {
                log::warn!("plugin scan: cannot tie scanner process {} to this process", child.id());
            }
        }
    }
    #[cfg(not(windows))]
    let _ = child;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_transport_reaches_clap_in_fixed_point() {
        let t = Transport {
            playing: true,
            tempo: 90.0,
            numerator: 3,
            denominator: 4,
            samples: 96_000,
            seconds: 2.0,
            quarters: 3.5,
            bar_start: 3.0,
            bar: 1,
            ..Transport::default()
        };
        let e = transport_event(&t);
        assert_eq!((e.header.type_, e.header.size as usize), (CLAP_EVENT_TRANSPORT, size_of::<clap_event_transport>()));
        assert_eq!(e.flags & CLAP_TRANSPORT_IS_PLAYING, CLAP_TRANSPORT_IS_PLAYING);
        assert_eq!(e.flags & CLAP_TRANSPORT_IS_RECORDING, 0);
        assert_eq!((e.song_pos_beats, e.bar_start), (7 * CLAP_BEATTIME_FACTOR / 2, 3 * CLAP_BEATTIME_FACTOR));
        assert_eq!(e.song_pos_seconds, 2 * CLAP_SECTIME_FACTOR);
        assert_eq!((e.tempo, e.bar_number, e.tsig_num, e.tsig_denom), (90.0, 1, 3, 4));
        let nan = transport_event(&Transport { quarters: f64::NAN, ..t });
        assert_eq!(nan.song_pos_beats, 0);
    }
}

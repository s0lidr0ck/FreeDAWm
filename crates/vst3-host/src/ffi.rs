//! The FFI boundary: the only module of this crate allowed to use `unsafe`.
//!
//! It loads VST3 binaries (`libloading`), runs the platform module entry, calls the COM
//! interfaces of the factory, component, audio processor and edit controller (`vst3` raw
//! bindings), and implements the host-side COM objects (host context, parameter changes, event
//! lists, a memory stream). Everything it exposes to the rest of the crate is a safe API.
//!
//! Soundness rests on the VST3 contract (the plugin honours the COM vtable signatures and the
//! documented pointer lifetimes). A plugin that violates it can still crash the process: hosting
//! in-process can't prevent that. What this module guarantees is that *SoundCraft* never hands
//! the plugin dangling or mis-sized pointers and never reads plugin strings beyond their bounds.

// VST3 enum constants are `c_int` on Windows and `c_uint` elsewhere, so casts that are no-ops on
// one platform are needed on another.
#![allow(clippy::unnecessary_cast)]

use crate::{Vst3Error, scan};
use soundcraft_dsp::Transport;
use std::ffi::{c_char, c_int, c_void};
use std::mem::ManuallyDrop;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicI32, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError, TryLockError, mpsc};
use std::time::Duration;
use vst3::Steinberg::Vst::{self as sv};
use vst3::Steinberg::Vst::{
    IAttributeListTrait, IAudioProcessorTrait, IComponentHandlerTrait, IComponentTrait, IConnectionPointTrait, IEditControllerTrait, IEventListTrait,
    IHostApplicationTrait, IMessageTrait, IMidiMappingTrait, IParamValueQueueTrait, IParameterChangesTrait, IUnitInfoTrait,
};
use vst3::Steinberg::{self as sb};
use vst3::Steinberg::{FUnknown, IBStreamTrait, IPlugFrameTrait, IPlugViewTrait, IPluginBaseTrait, IPluginFactory2Trait, IPluginFactoryTrait};
use vst3::{Class, ComPtr, ComWrapper, Interface};

/// Most classes one factory may report (hostile-input cap).
const MAX_CLASSES: i32 = 1024;
/// Most parameters read from one controller.
pub(crate) const MAX_PARAMS: i32 = 4096;
/// Most audio buses per direction we provide buffers for.
const MAX_BUSES: i32 = 16;
/// Most channels per audio bus.
const MAX_BUS_CHANNELS: u32 = 64;
/// Capacity of the per-block event lists (preallocated: `process` never allocates).
pub(crate) const EVENT_CAP: usize = 1024;
/// Most parameter queues per block (preallocated).
pub(crate) const PARAM_QUEUE_CAP: usize = 1024;
/// Largest plugin state we buffer (component or controller state).
const MAX_STATE_BYTES: usize = 64 << 20;
/// Most editor parameter edits buffered between two UI frames.
const EDIT_CAP: usize = 256;
/// Magic of SoundCraft's state container: `SCV3` + u32 LE component length + component state +
/// u32 LE controller length + controller state.
const STATE_MAGIC: &[u8; 4] = b"SCV3";

// ---- strings -------------------------------------------------------------------------------

/// Reads a fixed-size `char8[N]` buffer, stopping at the first NUL or the end of the array.
fn read_c8(buf: &[c_char]) -> String {
    let bytes: Vec<u8> = buf.iter().take_while(|c| **c != 0).map(|c| *c as u8).collect();
    String::from_utf8_lossy(&bytes).trim().to_string()
}

/// Reads a fixed-size UTF-16 `String128`, stopping at the first NUL or the end of the array.
fn read_c16(buf: &[u16]) -> String {
    let units: Vec<u16> = buf.iter().take_while(|c| **c != 0).copied().collect();
    String::from_utf16_lossy(&units).trim().to_string()
}

/// Writes `s` into a UTF-16 buffer, truncated and always NUL-terminated.
fn write_c16(s: &str, dst: &mut [u16]) {
    let Some(room) = dst.len().checked_sub(1) else { return };
    let mut n = 0;
    for (d, u) in dst.iter_mut().zip(s.encode_utf16().take(room)) {
        *d = u;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

fn ok(r: sb::tresult) -> bool {
    r == sb::kResultOk
}

/// VST3's numbers for the MIDI controllers that aren't CCs.
pub(crate) const AFTERTOUCH: usize = sv::ControllerNumbers_::kAfterTouch as usize;
pub(crate) const PITCH_BEND: usize = sv::ControllerNumbers_::kPitchBend as usize;
pub(crate) const PROGRAM_CHANGE: usize = sv::ControllerNumbers_::kCtrlProgramChange as usize;
/// Entries per MIDI channel in [`Instance::midi_mapping`]: CCs 0–127, aftertouch, pitch bend,
/// program change.
pub(crate) const MIDI_CTRLS: usize = PROGRAM_CHANGE + 1;

// ---- loaded binaries -----------------------------------------------------------------------

/// One class as reported by the factory.
#[derive(Debug, Clone)]
pub(crate) struct RawClass {
    pub cid: [u8; 16],
    pub category: String,
    pub name: String,
    pub vendor: String,
    pub version: String,
    pub sdk_version: String,
    pub sub_categories: String,
}

/// A loaded VST3 binary and its plugin factory. Loaded binaries are cached for the life of the
/// process (see [`Bundle::load`]): plugin code must never be unmapped while an instance may run,
/// and VST3 modules are not required to support being re-entered after their exit function, which
/// therefore runs once, at process exit (see [`exit_modules`]).
pub(crate) struct Bundle {
    factory: ComPtr<sb::IPluginFactory>,
    /// The module exit matching the entry that ran at load (`bundleExit`, `ExitDll`, `ModuleExit`).
    exit: Option<ModuleExit>,
    /// The thread that ran the module entry, and so must run the exit.
    entered_on: EntryThread,
    /// Set when plugin objects were leaked on purpose (see [`EditorLink::kill`]): the module exit
    /// must not run while any of its objects are alive.
    leaked: AtomicBool,
    // Never closed: code stays mapped for the process lifetime, even after the module exit.
    _lib: ManuallyDrop<libloading::Library>,
}

/// Loaded binaries, oldest first (module exits run newest first, the reverse of loading).
fn loaded() -> &'static Mutex<Vec<(PathBuf, Arc<Bundle>)>> {
    static L: OnceLock<Mutex<Vec<(PathBuf, Arc<Bundle>)>>> = OnceLock::new();
    L.get_or_init(|| Mutex::new(Vec::new()))
}

/// Set once the module exits have run: loading another module after that is refused.
static MODULES_EXITED: AtomicBool = AtomicBool::new(false);

/// Where a module's entry ran. Its exit must run on the same thread: Qt-based modules tie their
/// application object to it (Native Instruments' Massive X crashes in `bundleExit` on another
/// thread). Modules wanted on the main thread are entered there; any other thread's requests go
/// to the module thread, because the asking thread may be gone by the time the process exits.
#[derive(Clone, Copy, PartialEq, Eq)]
enum EntryThread {
    Main,
    Modules,
}

/// Whether this is the main thread. Not for exit handlers, where `std::thread::current` panics
/// (its thread-local data is gone by then): compare [`os_thread`] with [`MAIN_OS_THREAD`] there.
fn on_main_thread() -> bool {
    std::thread::current().name() == Some("main")
}

/// The OS id of the main thread, recorded when it loads a module (0 until then).
static MAIN_OS_THREAD: AtomicUsize = AtomicUsize::new(0);

/// The OS id of the calling thread, which (unlike `std::thread::current`) exit handlers can use.
fn os_thread() -> usize {
    #[cfg(unix)]
    {
        unsafe extern "C" {
            fn pthread_self() -> usize;
        }
        // SAFETY: `pthread_self` takes no arguments and cannot fail; `pthread_t` is an integer or
        // a pointer of pointer size on the Unix systems SoundCraft builds for.
        unsafe { pthread_self() }
    }
    #[cfg(windows)]
    {
        #[link(name = "kernel32")]
        unsafe extern "system" {
            fn GetCurrentThreadId() -> u32;
        }
        // SAFETY: `GetCurrentThreadId` takes no arguments and cannot fail.
        unsafe { GetCurrentThreadId() as usize }
    }
}

type Job = Box<dyn FnOnce() + Send>;

/// The module thread (started on first use): it runs jobs until the process exits.
fn module_thread() -> Option<&'static mpsc::Sender<Job>> {
    static TX: OnceLock<Option<mpsc::Sender<Job>>> = OnceLock::new();
    TX.get_or_init(|| {
        let (tx, rx) = mpsc::channel::<Job>();
        let spawned = std::thread::Builder::new().name("vst3-modules".into()).spawn(move || {
            while let Ok(job) = rx.recv() {
                let _ = std::panic::catch_unwind(std::panic::AssertUnwindSafe(job));
            }
        });
        spawned.ok().map(|_| tx)
    })
    .as_ref()
}

/// Runs `f` on the module thread and waits for its result (at most `timeout`, when given).
fn on_module_thread<R: Send + 'static>(f: impl FnOnce() -> R + Send + 'static, timeout: Option<Duration>) -> Option<R> {
    let (rtx, rrx) = mpsc::sync_channel(1);
    module_thread()?
        .send(Box::new(move || {
            let _ = rtx.send(f());
        }))
        .ok()?;
    match timeout {
        Some(t) => rrx.recv_timeout(t).ok(),
        None => rrx.recv().ok(),
    }
}

type GetFactory = unsafe extern "system" fn() -> *mut sb::IPluginFactory;
/// `bundleExit` (macOS), `ExitDll` (Windows) or `ModuleExit` (Linux/BSD). `extern "system"` is the
/// C ABI everywhere except 32-bit Windows, where it is the `PLUGIN_API` (stdcall) `ExitDll` uses.
type ModuleExit = unsafe extern "system" fn() -> bool;

#[cfg(target_os = "macos")]
mod cf {
    use std::ffi::c_void;
    #[link(name = "CoreFoundation", kind = "framework")]
    unsafe extern "C" {
        pub fn CFURLCreateFromFileSystemRepresentation(alloc: *const c_void, buf: *const u8, len: isize, is_dir: u8) -> *const c_void;
        pub fn CFBundleCreate(alloc: *const c_void, url: *const c_void) -> *mut c_void;
        pub fn CFRelease(cf: *const c_void);
    }
}

/// Opens the shared library and runs the platform module entry (`bundleEntry` with a
/// `CFBundleRef` on macOS, `InitDll` on Windows, `ModuleEntry` with the `dlopen` handle on Linux).
/// Also returns the matching module exit, when the entry ran and the module exports one.
fn open_library(bundle: &Path, exe: &Path) -> Result<(libloading::Library, Option<ModuleExit>), String> {
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        use libloading::os::unix::{Library as UnixLib, RTLD_LOCAL, RTLD_NOW};
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in VST3 plugin folders or named explicitly.
        let lib = unsafe { UnixLib::open(Some(exe), RTLD_NOW | RTLD_LOCAL) }.map_err(|e| e.to_string())?;
        type ModuleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
        // SAFETY: `ModuleEntry` is the VST3-mandated Linux entry with this signature.
        let entry = unsafe { lib.get::<ModuleEntry>(b"ModuleEntry\0") }.ok().map(|s| *s);
        let raw = lib.into_raw();
        // SAFETY: `raw` is the handle we just took out of `lib`; it is rewrapped right away.
        let lib = unsafe { UnixLib::from_raw(raw) };
        let mut exit = None;
        if let Some(f) = entry {
            // SAFETY: called once, with the module's own `dlopen` handle, as the spec requires.
            if !unsafe { f(raw) } {
                return Err("ModuleEntry failed".into());
            }
            // SAFETY: `ModuleExit` is the VST3-mandated Linux exit with this signature; the
            // pointer stays valid because loaded modules are never unmapped.
            exit = unsafe { lib.get::<ModuleExit>(b"ModuleExit\0") }.ok().map(|s| *s);
        }
        let _ = bundle;
        Ok((lib.into(), exit))
    }
    #[cfg(not(all(unix, not(target_os = "macos"))))]
    {
        // SAFETY: loading a shared library runs its static initialisers. That is inherent to
        // hosting plugins; we only load files found in VST3 plugin folders or named explicitly.
        let lib = unsafe { libloading::Library::new(exe) }.map_err(|e| e.to_string())?;
        let mut exit: Option<ModuleExit> = None;
        #[cfg(windows)]
        {
            let _ = bundle;
            type InitDll = unsafe extern "system" fn() -> bool;
            // SAFETY: `InitDll` is the VST3-mandated Windows entry with this signature.
            if let Ok(f) = unsafe { lib.get::<InitDll>(b"InitDll\0") } {
                // SAFETY: called once after loading, as the spec requires.
                if !unsafe { f() } {
                    return Err("InitDll failed".into());
                }
                // SAFETY: `ExitDll` is the VST3-mandated Windows exit with this signature; the
                // pointer stays valid because loaded modules are never unmapped.
                exit = unsafe { lib.get::<ModuleExit>(b"ExitDll\0") }.ok().map(|s| *s);
            }
        }
        #[cfg(target_os = "macos")]
        {
            type BundleEntry = unsafe extern "C" fn(*mut c_void) -> bool;
            // SAFETY: `bundleEntry`/`BundleEntry` is the VST3-mandated macOS entry with this signature.
            let entry = unsafe { lib.get::<BundleEntry>(b"bundleEntry\0") }
                .or_else(|_| {
                    // SAFETY: as above (older SDKs export the capitalised name).
                    unsafe { lib.get::<BundleEntry>(b"BundleEntry\0") }
                })
                .ok()
                .map(|s| *s);
            if let Some(f) = entry
                && bundle.is_dir()
            {
                use std::os::unix::ffi::OsStrExt;
                let bytes = bundle.as_os_str().as_bytes();
                let len = isize::try_from(bytes.len()).map_err(|_| "path too long".to_string())?;
                // SAFETY: `bytes` is a valid buffer of `len` bytes for the duration of the call;
                // a null allocator means the default one.
                let url = unsafe { cf::CFURLCreateFromFileSystemRepresentation(std::ptr::null(), bytes.as_ptr(), len, 1) };
                if url.is_null() {
                    return Err("cannot make a CFURL for the bundle".into());
                }
                // SAFETY: `url` is a valid CFURL created above.
                let cfbundle = unsafe { cf::CFBundleCreate(std::ptr::null(), url) };
                // SAFETY: we own `url` (Create rule) and no longer need it.
                unsafe { cf::CFRelease(url) };
                if cfbundle.is_null() {
                    return Err("not a loadable bundle".into());
                }
                // SAFETY: called once with a valid CFBundleRef. The bundle ref is intentionally
                // kept (never released): the module may hold on to it for its whole life, and
                // loaded modules are never unloaded.
                if !unsafe { f(cfbundle) } {
                    return Err("bundleEntry failed".into());
                }
                // SAFETY: `bundleExit`/`BundleExit` is the VST3-mandated macOS exit with this
                // signature; the pointer stays valid because loaded modules are never unmapped.
                exit = unsafe { lib.get::<ModuleExit>(b"bundleExit\0") }
                    .or_else(|_| {
                        // SAFETY: as above (older SDKs export the capitalised name).
                        unsafe { lib.get::<ModuleExit>(b"BundleExit\0") }
                    })
                    .ok()
                    .map(|s| *s);
            }
        }
        Ok((lib, exit))
    }
}

/// Dispatches this thread's window messages for `time` (see `window::pump`).
pub(crate) fn pump_messages(time: std::time::Duration) {
    window::pump(time);
}

/// The binaries this process has loaded, oldest first.
pub(crate) fn loaded_paths() -> Vec<PathBuf> {
    loaded().lock().unwrap_or_else(PoisonError::into_inner).iter().map(|(p, _)| p.clone()).collect()
}

/// Turns off the system's crash and critical-error dialogs for this process (and the processes
/// it starts), so a plugin that crashes it ends it at once instead of waiting for a click. For the
/// scanner helper only.
pub(crate) fn quiet_crash_dialogs() {
    #[cfg(windows)]
    {
        use windows_sys::Win32::System::Diagnostics::Debug::{SEM_FAILCRITICALERRORS, SEM_NOGPFAULTERRORBOX, SetErrorMode};
        // SAFETY: plain flag setter with no pointers; affects only this process's error mode.
        unsafe { SetErrorMode(SEM_FAILCRITICALERRORS | SEM_NOGPFAULTERRORBOX) };
    }
}

/// Runs the module exits at process exit, newest module first, each on the thread that ran its
/// entry. VST3 modules expect their exit before their static destructors run, and some crash in
/// those destructors without it (seen with Steinberg's HALion Sonic, after a plain scan). A module
/// with a live instance or leaked objects keeps running and is not exited, and so is one entered
/// on the main thread when the process exits from another thread. Runs once; later loads are
/// refused.
pub(crate) fn exit_modules() {
    if MODULES_EXITED.swap(true, Ordering::AcqRel) {
        return;
    }
    let mut cache = match loaded().try_lock() {
        Ok(g) => g,
        Err(TryLockError::Poisoned(p)) => p.into_inner(),
        // Another thread is loading a module right now: leave every module as it is.
        Err(TryLockError::WouldBlock) => return,
    };
    let main = MAIN_OS_THREAD.load(Ordering::Acquire) == os_thread();
    let (mut kept, mut here, mut there) = (Vec::new(), Vec::new(), Vec::new());
    while let Some((path, b)) = cache.pop() {
        if b.leaked.load(Ordering::Acquire) || (b.entered_on == EntryThread::Main && !main) {
            kept.push((path, b));
            continue;
        }
        match Arc::try_unwrap(b) {
            Ok(bundle) if bundle.entered_on == EntryThread::Main => here.push(bundle),
            Ok(bundle) => there.push(bundle),
            Err(b) => kept.push((path, b)),
        }
    }
    kept.reverse();
    *cache = kept;
    drop(cache);
    here.into_iter().for_each(Bundle::exit);
    if !there.is_empty() {
        // Bounded: a module thread stuck inside a plugin must not hang the process exit.
        let _ = on_module_thread(move || there.into_iter().for_each(Bundle::exit), Some(Duration::from_secs(5)));
    }
}

/// Registers [`exit_modules`] to run at process exit. Exit handlers run in reverse order of
/// registration, C++ static destructors included, so registering again after each module load
/// puts the exits ahead of the destructors of every module loaded so far.
fn register_exit_hook() {
    unsafe extern "C" {
        fn atexit(f: extern "C" fn()) -> c_int;
    }
    extern "C" fn hook() {
        // Nothing may unwind out of an exit handler.
        let _ = std::panic::catch_unwind(exit_modules);
    }
    // SAFETY: `atexit` is the C library function with this signature; `hook` is a plain function
    // of this binary, so it lives as long as the process.
    if unsafe { atexit(hook) } != 0 {
        log::warn!("vst3: cannot register the module exit handler");
    }
}

impl Bundle {
    /// Loads (once per path, then cached for the life of the process) a `.vst3`.
    pub fn load(path: &Path) -> Result<Arc<Bundle>, Vst3Error> {
        let mut cache = loaded().lock().unwrap_or_else(PoisonError::into_inner);
        if let Some((_, b)) = cache.iter().find(|(p, _)| p == path) {
            return Ok(b.clone());
        }
        let err = |m: &str| Vst3Error::Load(path.display().to_string(), m.to_string());
        if MODULES_EXITED.load(Ordering::Acquire) {
            return Err(err("plugin hosting has shut down"));
        }
        let b = if on_main_thread() {
            MAIN_OS_THREAD.store(os_thread(), Ordering::Release);
            Bundle::open(path, EntryThread::Main)?
        } else {
            let p = path.to_path_buf();
            on_module_thread(move || Bundle::open(&p, EntryThread::Modules), None).ok_or_else(|| err("the module thread is not running"))??
        };
        let b = Arc::new(b);
        cache.push((path.to_path_buf(), b.clone()));
        // After every new module, so that the exits run before its static destructors.
        register_exit_hook();
        Ok(b)
    }

    fn open(path: &Path, entered_on: EntryThread) -> Result<Bundle, Vst3Error> {
        let err = |m: String| Vst3Error::Load(path.display().to_string(), m);
        let exe = scan::bundle_binary(path).ok_or_else(|| err("no loadable binary".into()))?;
        let (lib, exit) = open_library(path, &exe).map_err(err)?;
        // The module entry ran: balance it before giving up on the module.
        let fail = |m: String| {
            if let Some(f) = exit {
                // SAFETY: balanced with the entry that ran in `open_library`; no plugin object exists.
                unsafe { f() };
            }
            err(m)
        };
        // SAFETY: `GetPluginFactory` is the VST3-mandated export with this signature.
        let get = match unsafe { lib.get::<GetFactory>(b"GetPluginFactory\0") } {
            Ok(sym) => *sym,
            Err(e) => return Err(fail(format!("not a VST3 plugin: {e}"))),
        };
        // SAFETY: the module is loaded and its entry function (if any) has run.
        let raw = unsafe { get() };
        // SAFETY: a non-null result is a factory pointer with one reference owned by the caller
        // (VST3 contract), which the `ComPtr` takes over.
        let factory = unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| fail("GetPluginFactory returned null".into()))?;
        Ok(Bundle { factory, exit, entered_on, leaked: AtomicBool::new(false), _lib: ManuallyDrop::new(lib) })
    }

    /// Releases the factory, then runs the module exit, as the VST3 SDK's own host does.
    fn exit(self) {
        let Bundle { factory, exit, .. } = self;
        drop(factory);
        if let Some(f) = exit {
            // SAFETY: balanced with the entry that ran when the module was loaded; called once
            // (`self` is consumed), after every instance is gone (each holds an `Arc` to the
            // bundle) and the factory released. The code stays mapped (`_lib` is never closed).
            unsafe { f() };
        }
    }

    /// Plugin objects of this module were leaked on purpose: never run its module exit.
    pub fn mark_leaked(&self) {
        self.leaked.store(true, Ordering::Release);
    }

    /// The vendor from the factory info (fallback for classes without one).
    fn factory_vendor(&self) -> String {
        // SAFETY: `PFactoryInfo` is plain data (char arrays and an int); all-zero is valid.
        let mut info: sb::PFactoryInfo = unsafe { std::mem::zeroed() };
        // SAFETY: valid factory; `info` is a writable struct of the right type.
        if ok(unsafe { self.factory.getFactoryInfo(&mut info) }) { read_c8(&info.vendor) } else { String::new() }
    }

    /// Every class this binary exports.
    pub fn classes(&self) -> Vec<RawClass> {
        // SAFETY: valid factory.
        let n = unsafe { self.factory.countClasses() }.clamp(0, MAX_CLASSES);
        let f2 = self.factory.cast::<sb::IPluginFactory2>();
        let vendor = self.factory_vendor();
        let mut out = Vec::new();
        for i in 0..n {
            if let Some(f2) = &f2 {
                // SAFETY: plain data; all-zero is valid.
                let mut c: sb::PClassInfo2 = unsafe { std::mem::zeroed() };
                // SAFETY: index below the count; `c` is writable.
                if ok(unsafe { f2.getClassInfo2(i, &mut c) }) {
                    let v = read_c8(&c.vendor);
                    out.push(RawClass {
                        cid: tuid_bytes(&c.cid),
                        category: read_c8(&c.category),
                        name: read_c8(&c.name),
                        vendor: if v.is_empty() { vendor.clone() } else { v },
                        version: read_c8(&c.version),
                        sdk_version: read_c8(&c.sdkVersion),
                        sub_categories: read_c8(&c.subCategories),
                    });
                    continue;
                }
            }
            // SAFETY: plain data; all-zero is valid.
            let mut c: sb::PClassInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `c` is writable.
            if ok(unsafe { self.factory.getClassInfo(i, &mut c) }) {
                out.push(RawClass {
                    cid: tuid_bytes(&c.cid),
                    category: read_c8(&c.category),
                    name: read_c8(&c.name),
                    vendor: vendor.clone(),
                    version: String::new(),
                    sdk_version: String::new(),
                    sub_categories: String::new(),
                });
            }
        }
        out
    }

    /// Creates an object of class `cid` and queries interface `I` on it.
    fn create<I: Interface>(&self, cid: &[u8; 16]) -> Option<ComPtr<I>> {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let iid = I::IID;
        // SAFETY: valid factory; `cid` and `iid` point to 16 readable bytes each (VST3 passes TUIDs
        // as `FIDString`); `obj` is writable.
        let r = unsafe { self.factory.createInstance(cid.as_ptr().cast::<c_char>(), iid.as_ptr().cast::<c_char>(), &mut obj) };
        if !ok(r) {
            return None;
        }
        // SAFETY: on success `obj` holds an `I` pointer with one reference owned by us.
        unsafe { ComPtr::from_raw(obj.cast::<I>()) }
    }

    /// Creates, initialises and wires up one plugin instance.
    pub fn instantiate(self: &Arc<Self>, cid: &[u8; 16]) -> Result<Instance, Vst3Error> {
        let name = hex(cid);
        let err = |m: &str| Vst3Error::Instantiate(name.clone(), m.to_string());
        let host = ComWrapper::new(HostContext::default());
        let host_unknown = host.as_com_ref::<sv::IHostApplication>().ok_or_else(|| err("host context"))?.upcast::<FUnknown>().as_ptr();
        let component = self.create::<sv::IComponent>(cid).ok_or_else(|| err("factory cannot create the component"))?;
        // SAFETY: fresh component; the host context outlives it (owned by the returned Instance).
        if !ok(unsafe { component.initialize(host_unknown) }) {
            return Err(err("component initialize failed"));
        }
        let mut inst = Instance {
            processor: None,
            controller: None,
            single: false,
            connections: None,
            proxies: None,
            component_initialized: true,
            controller_initialized: false,
            active: false,
            processing: false,
            max_frames: 0,
            in_params: ComWrapper::new(ParamChanges::new(PARAM_QUEUE_CAP)),
            out_params: ComWrapper::new(ParamChanges::new(PARAM_QUEUE_CAP)),
            in_events: ComWrapper::new(EventList::new(EVENT_CAP)),
            out_events: ComWrapper::new(EventList::new(EVENT_CAP)),
            // SAFETY: `ProcessContext` is plain data (numbers and small structs); all-zero is valid.
            context: unsafe { std::mem::zeroed() },
            event_in: false,
            mode: sv::ProcessModes_::kRealtime as i32,
            editor: Arc::new(EditorLink::default()),
            component,
            host,
            bundle: self.clone(),
        };
        let processor = inst.component.cast::<sv::IAudioProcessor>().ok_or_else(|| err("no IAudioProcessor"))?;
        inst.processor = Some(processor);
        // Edit controller: the component itself (single-component plugin) or a separate class.
        if let Some(c) = inst.component.cast::<sv::IEditController>() {
            inst.controller = Some(c);
            inst.single = true;
        } else {
            let mut ccid: sb::TUID = [0; 16];
            // SAFETY: valid component; `ccid` is a writable 16-byte TUID.
            if ok(unsafe { inst.component.getControllerClassId(&mut ccid) })
                && let Some(c) = self.create::<sv::IEditController>(&tuid_bytes(&ccid))
            {
                // SAFETY: fresh controller; the host context outlives it.
                if ok(unsafe { c.initialize(host_unknown) }) {
                    inst.controller = Some(c);
                    inst.controller_initialized = true;
                } else {
                    log::warn!("vst3 {name}: controller initialize failed; parameters unavailable");
                }
            }
        }
        if let Some(c) = &inst.controller {
            if !inst.single
                && let (Some(a), Some(b)) = (inst.component.cast::<sv::IConnectionPoint>(), c.cast::<sv::IConnectionPoint>())
            {
                // Each side is connected to a host proxy whose target is the other side (as the
                // SDK asks), so a message the processor sends from the audio thread reaches the
                // controller on the UI thread (see `ConnectionProxy`).
                let to_controller = ComWrapper::new(ConnectionProxy::new("processor", b.clone()));
                let to_component = ComWrapper::new(ConnectionProxy::new("controller", a.clone()));
                if let (Some(pc), Some(pk)) = (to_controller.as_com_ref::<sv::IConnectionPoint>(), to_component.as_com_ref::<sv::IConnectionPoint>())
                {
                    // SAFETY: both connection points are valid for as long as `inst` lives, and
                    // so are the proxies (kept in `inst`); disconnected in `Drop` before either
                    // side terminates.
                    unsafe {
                        a.connect(pc.as_ptr());
                        b.connect(pk.as_ptr());
                    }
                }
                inst.connections = Some((a, b));
                inst.proxies = Some((to_controller, to_component));
            }
            inst.sync_controller_state();
            if let Some(h) = inst.host.as_com_ref::<sv::IComponentHandler>() {
                // SAFETY: valid controller; the handler (host context) outlives it.
                unsafe { c.setComponentHandler(h.as_ptr()) };
            }
        }
        inst.editor = Arc::new(EditorLink {
            inner: Mutex::new(EditorInner {
                component: Some(inst.component.clone()),
                controller: inst.controller.clone(),
                messages: inst.proxies.as_ref().map(|(to_controller, _)| to_controller.clone()),
                to_component: inst.proxies.as_ref().map(|(_, to_component)| to_component.clone()),
                ..EditorInner::default()
            }),
        });
        Ok(inst)
    }
}

/// A `TUID` (`[i8; 16]`) as raw bytes.
fn tuid_bytes(t: &sb::TUID) -> [u8; 16] {
    let mut out = [0u8; 16];
    for (o, b) in out.iter_mut().zip(t.iter()) {
        *o = *b as u8;
    }
    out
}

/// Upper-case hex of a class id (the form used in `vst3:<hex>` ids).
pub(crate) fn hex(cid: &[u8; 16]) -> String {
    cid.iter().map(|b| format!("{b:02X}")).collect()
}

// ---- host-side COM objects -----------------------------------------------------------------

/// The host context handed to `initialize` (`IHostApplication`) and to the controller
/// (`IComponentHandler`: parameter edits made in the plugin's editor are queued for the UI, which
/// writes them into the session, from where they reach the processor).
pub(crate) struct HostContext {
    pub latency_changed: AtomicBool,
    /// `restartComponent` flags raised since the UI last took them (`RestartFlags`, ORed).
    pub restart_flags: AtomicU32,
    /// `performEdit` values (normalized), bounded by `EDIT_CAP`.
    pub edits: Mutex<Vec<(u32, f64)>>,
}

impl Default for HostContext {
    fn default() -> Self {
        HostContext { latency_changed: AtomicBool::new(false), restart_flags: AtomicU32::new(0), edits: Mutex::new(Vec::with_capacity(EDIT_CAP)) }
    }
}

impl HostContext {
    /// Takes the queued editor edits.
    pub fn take_edits(&self) -> Vec<(u32, f64)> {
        let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
        let out = e.clone();
        e.clear();
        out
    }
}

impl Class for HostContext {
    type Interfaces = (sv::IHostApplication, sv::IComponentHandler);
}

impl IHostApplicationTrait for HostContext {
    unsafe fn getName(&self, name: *mut sv::String128) -> sb::tresult {
        // SAFETY: the plugin passes a pointer to a writable String128 (or null, handled).
        match unsafe { name.as_mut() } {
            Some(n) => {
                write_c16("FreeDAWm", n);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }

    /// `IMessage` and `IAttributeList` objects for the plugin (processor ↔ controller messaging;
    /// a plugin that assumes `allocateMessage` succeeds dereferences a null without them).
    unsafe fn createInstance(&self, cid: *mut sb::TUID, iid: *mut sb::TUID, obj: *mut *mut c_void) -> sb::tresult {
        // SAFETY: `obj` is the plugin's out-pointer (or null, handled).
        let Some(out) = (unsafe { obj.as_mut() }) else { return sb::kInvalidArgument };
        *out = std::ptr::null_mut();
        // SAFETY: two TUIDs from the plugin (or null, handled).
        let (Some(cid), Some(iid)) = (unsafe { cid.as_ref() }, unsafe { iid.as_ref() }) else { return sb::kInvalidArgument };
        let (cid, iid) = (tuid_bytes(cid), tuid_bytes(iid));
        let wants = |own: [u8; 16]| iid == own || iid == FUnknown::IID;
        let made: Option<*mut c_void> = if cid == sv::IMessage::IID && wants(sv::IMessage::IID) {
            ComWrapper::new(HostMessage::default()).to_com_ptr::<sv::IMessage>().map(|p| p.into_raw().cast::<c_void>())
        } else if cid == sv::IAttributeList::IID && wants(sv::IAttributeList::IID) {
            ComWrapper::new(AttributeList::default()).to_com_ptr::<sv::IAttributeList>().map(|p| p.into_raw().cast::<c_void>())
        } else {
            None
        };
        match made {
            Some(p) => {
                *out = p;
                sb::kResultOk
            }
            None => sb::kNoInterface,
        }
    }
}

impl IComponentHandlerTrait for HostContext {
    unsafe fn beginEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn performEdit(&self, id: sv::ParamID, value: sv::ParamValue) -> sb::tresult {
        if value.is_finite() {
            let mut e = self.edits.lock().unwrap_or_else(PoisonError::into_inner);
            let v = value.clamp(0.0, 1.0);
            if let Some(x) = e.iter_mut().find(|(i, _)| *i == id) {
                x.1 = v;
            } else if e.len() < EDIT_CAP {
                e.push((id, v));
            }
        }
        sb::kResultOk
    }
    unsafe fn endEdit(&self, _id: sv::ParamID) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn restartComponent(&self, flags: sb::int32) -> sb::tresult {
        if flags as u32 & sv::RestartFlags_::kLatencyChanged as u32 != 0 {
            self.latency_changed.store(true, Ordering::Relaxed);
        }
        self.restart_flags.fetch_or(flags as u32, Ordering::Relaxed);
        sb::kResultOk
    }
}

/// Most attributes one [`AttributeList`] holds, and the longest string or binary attribute.
const MAX_ATTRS: usize = 1024;
const MAX_ATTR_BYTES: usize = 16 << 20;
/// Most processor → controller messages queued for the UI thread per instance.
const MESSAGE_QUEUE_CAP: usize = 256;

/// One attribute of an [`AttributeList`].
enum Attr {
    Int(i64),
    Float(f64),
    /// UTF-16, NUL-terminated.
    Str(Box<[u16]>),
    Bin(Box<[u8]>),
}

/// The host's `IAttributeList`: typed values by id (the attributes of a [`HostMessage`], or a
/// list the plugin asked `createInstance` for). The pointer `getBinary` hands out stays valid
/// until that attribute is set again or the list is released, as the SDK's own list promises.
#[derive(Default)]
pub(crate) struct AttributeList {
    items: Mutex<Vec<(Vec<u8>, Attr)>>,
}

impl Class for AttributeList {
    type Interfaces = (sv::IAttributeList,);
}

/// An attribute id as bytes. SAFETY contract: `id` is null or a NUL-terminated string.
unsafe fn attr_key(id: *const c_char) -> Option<Vec<u8>> {
    if id.is_null() {
        return None;
    }
    // SAFETY: per the contract.
    Some(unsafe { std::ffi::CStr::from_ptr(id) }.to_bytes().to_vec())
}

impl AttributeList {
    fn set(&self, key: Vec<u8>, attr: Attr) -> sb::tresult {
        let mut items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(slot) = items.iter_mut().find(|(k, _)| *k == key) {
            slot.1 = attr;
        } else if items.len() < MAX_ATTRS {
            items.push((key, attr));
        } else {
            return sb::kOutOfMemory;
        }
        sb::kResultOk
    }

    /// `f` on the attribute `key`, under the lock.
    fn with<R>(&self, key: &[u8], f: impl FnOnce(&Attr) -> Option<R>) -> Option<R> {
        let items = self.items.lock().unwrap_or_else(PoisonError::into_inner);
        items.iter().find(|(k, _)| k == key).and_then(|(_, a)| f(a))
    }

    fn get_number<T>(&self, id: *const c_char, out: *mut T, pick: impl FnOnce(&Attr) -> Option<T>) -> sb::tresult {
        // SAFETY: `id` is the plugin's attribute id; `out` is its out-pointer (or null).
        let (Some(k), Some(out)) = (unsafe { attr_key(id) }, unsafe { out.as_mut() }) else { return sb::kInvalidArgument };
        match self.with(&k, pick) {
            Some(v) => {
                *out = v;
                sb::kResultOk
            }
            None => sb::kResultFalse,
        }
    }
}

impl IAttributeListTrait for AttributeList {
    unsafe fn setInt(&self, id: sv::IAttributeList_::AttrID, value: sb::int64) -> sb::tresult {
        // SAFETY: `id` is the plugin's attribute id (NUL-terminated, or null).
        unsafe { attr_key(id) }.map_or(sb::kInvalidArgument, |k| self.set(k, Attr::Int(value)))
    }
    unsafe fn getInt(&self, id: sv::IAttributeList_::AttrID, value: *mut sb::int64) -> sb::tresult {
        self.get_number(id, value, |a| if let Attr::Int(v) = a { Some(*v) } else { None })
    }
    unsafe fn setFloat(&self, id: sv::IAttributeList_::AttrID, value: f64) -> sb::tresult {
        // SAFETY: as above.
        unsafe { attr_key(id) }.map_or(sb::kInvalidArgument, |k| self.set(k, Attr::Float(value)))
    }
    unsafe fn getFloat(&self, id: sv::IAttributeList_::AttrID, value: *mut f64) -> sb::tresult {
        self.get_number(id, value, |a| if let Attr::Float(v) = a { Some(*v) } else { None })
    }
    unsafe fn setString(&self, id: sv::IAttributeList_::AttrID, string: *const u16) -> sb::tresult {
        // SAFETY: as above.
        let Some(k) = (unsafe { attr_key(id) }) else { return sb::kInvalidArgument };
        if string.is_null() {
            return sb::kInvalidArgument;
        }
        let mut units = Vec::new();
        for i in 0..MAX_ATTR_BYTES / 2 {
            // SAFETY: a NUL-terminated UTF-16 string (plugin contract), read one unit at a time
            // up to its terminator.
            let u = unsafe { *string.add(i) };
            if u == 0 {
                break;
            }
            units.push(u);
        }
        units.push(0);
        self.set(k, Attr::Str(units.into_boxed_slice()))
    }
    unsafe fn getString(&self, id: sv::IAttributeList_::AttrID, string: *mut u16, size_in_bytes: sb::uint32) -> sb::tresult {
        // SAFETY: as above.
        let Some(k) = (unsafe { attr_key(id) }) else { return sb::kInvalidArgument };
        let room = size_in_bytes as usize / 2;
        if string.is_null() || room == 0 {
            return sb::kInvalidArgument;
        }
        // SAFETY: the plugin's buffer holds `room` units for the call; the copy, NUL included,
        // stays within it.
        let dst = unsafe { std::slice::from_raw_parts_mut(string, room) };
        match self.with(&k, |a| if let Attr::Str(units) = a { Some(units.clone()) } else { None }) {
            Some(units) => {
                write_c16_units(&units, dst);
                sb::kResultOk
            }
            None => sb::kResultFalse,
        }
    }
    unsafe fn setBinary(&self, id: sv::IAttributeList_::AttrID, data: *const c_void, size_in_bytes: sb::uint32) -> sb::tresult {
        // SAFETY: as above.
        let Some(k) = (unsafe { attr_key(id) }) else { return sb::kInvalidArgument };
        let n = size_in_bytes as usize;
        if n > MAX_ATTR_BYTES || (data.is_null() && n > 0) {
            return sb::kInvalidArgument;
        }
        // SAFETY: `data` points to `n` readable bytes for the call (plugin contract).
        let bytes = if n == 0 { &[][..] } else { unsafe { std::slice::from_raw_parts(data.cast::<u8>(), n) } };
        self.set(k, Attr::Bin(bytes.to_vec().into_boxed_slice()))
    }
    unsafe fn getBinary(&self, id: sv::IAttributeList_::AttrID, data: *mut *const c_void, size_in_bytes: *mut sb::uint32) -> sb::tresult {
        // SAFETY: as above; two out-pointers (or null).
        let (Some(k), Some(out), Some(size)) = (unsafe { attr_key(id) }, unsafe { data.as_mut() }, unsafe { size_in_bytes.as_mut() }) else {
            return sb::kInvalidArgument;
        };
        // The boxed bytes keep their address until this attribute is set again or the list is
        // released: the documented lifetime of the pointer.
        match self.with(&k, |a| if let Attr::Bin(b) = a { Some((b.as_ptr(), b.len())) } else { None }) {
            Some((ptr, len)) => {
                *out = ptr.cast::<c_void>();
                *size = u32::try_from(len).unwrap_or(u32::MAX);
                sb::kResultOk
            }
            None => sb::kResultFalse,
        }
    }
}

/// Copies a NUL-terminated UTF-16 string into `dst`, truncated and always NUL-terminated.
fn write_c16_units(src: &[u16], dst: &mut [u16]) {
    let Some(room) = dst.len().checked_sub(1) else { return };
    let mut n = 0;
    for (d, u) in dst.iter_mut().zip(src.iter().take_while(|u| **u != 0).take(room)) {
        *d = *u;
        n += 1;
    }
    if let Some(z) = dst.get_mut(n) {
        *z = 0;
    }
}

/// The host's `IMessage` (from `IHostApplication::createInstance`): an id and an
/// [`AttributeList`]. The id pointer a plugin reads stays valid until the id is set again or the
/// message is released.
pub(crate) struct HostMessage {
    id: Mutex<Box<[u8]>>,
    attrs: ComWrapper<AttributeList>,
}

impl Default for HostMessage {
    fn default() -> Self {
        HostMessage { id: Mutex::new(Box::new([0])), attrs: ComWrapper::new(AttributeList::default()) }
    }
}

impl Class for HostMessage {
    type Interfaces = (sv::IMessage,);
}

impl IMessageTrait for HostMessage {
    unsafe fn getMessageID(&self) -> sb::FIDString {
        self.id.lock().unwrap_or_else(PoisonError::into_inner).as_ptr().cast::<c_char>()
    }
    unsafe fn setMessageID(&self, id: sb::FIDString) {
        // SAFETY: null or a NUL-terminated string (plugin contract).
        let mut bytes = unsafe { attr_key(id) }.unwrap_or_default();
        bytes.truncate(MAX_ATTR_BYTES);
        bytes.push(0);
        *self.id.lock().unwrap_or_else(PoisonError::into_inner) = bytes.into_boxed_slice();
    }
    unsafe fn getAttributes(&self) -> *mut sv::IAttributeList {
        // Not a counted reference (the SDK's host message does the same): owned by the message.
        self.attrs.as_com_ref::<sv::IAttributeList>().map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

/// The host's end of a processor ↔ controller connection (`IConnectionPoint`). Each side is
/// connected to a proxy whose target is the other side, as the SDK asks of hosts: a `notify`
/// on the UI thread goes straight to the target, one from any other thread (a processor's
/// `sendMessage` from `process`, a controller's from a preset-browser or loader thread) is
/// queued (bounded, `try_lock`: never blocks the audio thread) and delivered on the UI thread
/// by [`ConnectionProxy::deliver`], so a controller never runs its message handler, which
/// touches its GUI objects, on the audio thread. Both proxies are delivered from
/// [`EditorLink::idle`], together with the `restartComponent` flags: delivery depends on the
/// host calling the instance's editor handle's `idle` each UI frame (the player does so for
/// every instance that has one, open window or not), not on the editor being open. Headless
/// hosts (the CLI and MCP, a render's copies) never call `idle`, so there a side's off-thread
/// messages stay queued; harmless for a render, which plays the session's parameter values.
pub(crate) struct ConnectionProxy {
    /// The sender, for the log line about dropped messages.
    sender: &'static str,
    target: Mutex<Option<ComPtr<sv::IConnectionPoint>>>,
    queue: Mutex<Vec<ComPtr<sv::IMessage>>>,
    /// Messages dropped because the queue was full or locked (reported once per idle).
    dropped: AtomicUsize,
}

impl Class for ConnectionProxy {
    type Interfaces = (sv::IConnectionPoint,);
}

impl ConnectionProxy {
    /// A proxy delivering to `target`, from the `sender` side ("processor" or "controller").
    pub fn new(sender: &'static str, target: ComPtr<sv::IConnectionPoint>) -> ConnectionProxy {
        ConnectionProxy {
            sender,
            target: Mutex::new(Some(target)),
            queue: Mutex::new(Vec::with_capacity(MESSAGE_QUEUE_CAP)),
            dropped: AtomicUsize::new(0),
        }
    }

    /// Forgets the target and the queued messages (before either side terminates).
    pub fn detach(&self) {
        *self.target.lock().unwrap_or_else(PoisonError::into_inner) = None;
        self.queue.lock().unwrap_or_else(PoisonError::into_inner).clear();
    }

    /// Delivers the queued messages to the target. UI thread.
    pub fn deliver(&self) {
        let dropped = self.dropped.swap(0, Ordering::Relaxed);
        if dropped > 0 {
            log::warn!("vst3: {dropped} {} messages were dropped (queue full)", self.sender);
        }
        let pending = {
            let mut q = self.queue.lock().unwrap_or_else(PoisonError::into_inner);
            if q.is_empty() {
                return;
            }
            std::mem::replace(&mut *q, Vec::with_capacity(MESSAGE_QUEUE_CAP))
        };
        let target = self.target.lock().unwrap_or_else(PoisonError::into_inner).clone();
        if let Some(t) = target {
            for m in &pending {
                // SAFETY: a valid target (cleared by `detach` before it terminates) and a message
                // the sender gave us a counted reference to.
                unsafe { t.notify(m.as_ptr()) };
            }
        }
        // The messages are released here, on the UI thread.
        drop(pending);
    }
}

impl IConnectionPointTrait for ConnectionProxy {
    unsafe fn connect(&self, other: *mut sv::IConnectionPoint) -> sb::tresult {
        if other.is_null() { sb::kInvalidArgument } else { sb::kResultOk }
    }
    unsafe fn disconnect(&self, _other: *mut sv::IConnectionPoint) -> sb::tresult {
        sb::kResultOk
    }
    unsafe fn notify(&self, message: *mut sv::IMessage) -> sb::tresult {
        // SAFETY: the sender's message, valid for the call (plugin contract); null handled.
        let Some(m) = (unsafe { vst3::ComRef::from_raw(message) }) else { return sb::kInvalidArgument };
        if on_main_thread() {
            let target = self.target.lock().unwrap_or_else(PoisonError::into_inner).clone();
            return match target {
                // SAFETY: a valid target (cleared by `detach` before it terminates).
                Some(t) => unsafe { t.notify(message) },
                None => sb::kResultFalse,
            };
        }
        // The audio thread (or a worker): keep a reference and let the UI thread deliver it.
        // Never waits (`try_lock`) and never grows the queue past its preallocated capacity.
        match self.queue.try_lock() {
            Ok(mut q) if q.len() < q.capacity() => {
                q.push(m.to_com_ptr());
                sb::kResultOk
            }
            _ => {
                self.dropped.fetch_add(1, Ordering::Relaxed);
                sb::kResultFalse
            }
        }
    }
}

/// One parameter's points for one block. SoundCraft sends at most one point per block; a
/// plugin writing output changes may add more, of which only the last is kept.
#[derive(Default)]
pub(crate) struct ParamQueue {
    id: AtomicU32,
    offset: AtomicI32,
    value: AtomicU64,
    points: AtomicI32,
}

impl Class for ParamQueue {
    type Interfaces = (sv::IParamValueQueue,);
}

impl IParamValueQueueTrait for ParamQueue {
    unsafe fn getParameterId(&self) -> sv::ParamID {
        self.id.load(Ordering::Relaxed)
    }
    unsafe fn getPointCount(&self) -> sb::int32 {
        self.points.load(Ordering::Relaxed)
    }
    unsafe fn getPoint(&self, index: sb::int32, offset: *mut sb::int32, value: *mut sv::ParamValue) -> sb::tresult {
        if index != 0 || self.points.load(Ordering::Relaxed) < 1 {
            return sb::kInvalidArgument;
        }
        // SAFETY: out-pointers from the plugin (or null, handled).
        if let (Some(o), Some(v)) = (unsafe { offset.as_mut() }, unsafe { value.as_mut() }) {
            *o = self.offset.load(Ordering::Relaxed);
            *v = f64::from_bits(self.value.load(Ordering::Relaxed));
            sb::kResultOk
        } else {
            sb::kInvalidArgument
        }
    }
    unsafe fn addPoint(&self, offset: sb::int32, value: sv::ParamValue, index: *mut sb::int32) -> sb::tresult {
        self.offset.store(offset, Ordering::Relaxed);
        self.value.store(value.to_bits(), Ordering::Relaxed);
        self.points.store(1, Ordering::Relaxed);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(i) = unsafe { index.as_mut() } {
            *i = 0;
        }
        sb::kResultOk
    }
}

/// A fixed pool of parameter queues (`IParameterChanges`), reused every block.
pub(crate) struct ParamChanges {
    queues: Vec<ComWrapper<ParamQueue>>,
    count: AtomicI32,
}

impl Class for ParamChanges {
    type Interfaces = (sv::IParameterChanges,);
}

impl ParamChanges {
    fn new(cap: usize) -> ParamChanges {
        ParamChanges { queues: (0..cap).map(|_| ComWrapper::new(ParamQueue::default())).collect(), count: AtomicI32::new(0) }
    }

    fn clear(&self) {
        self.count.store(0, Ordering::Relaxed);
    }

    /// Finds or adds the queue for `id`; `None` when the pool is full.
    fn queue_for(&self, id: u32) -> Option<(i32, &ParamQueue)> {
        let n = self.count.load(Ordering::Relaxed).max(0);
        for (i, q) in self.queues.iter().enumerate().take(n as usize) {
            if q.id.load(Ordering::Relaxed) == id {
                return Some((i32::try_from(i).ok()?, q));
            }
        }
        let q = self.queues.get(n as usize)?;
        q.id.store(id, Ordering::Relaxed);
        q.points.store(0, Ordering::Relaxed);
        self.count.store(n + 1, Ordering::Relaxed);
        Some((n, q))
    }

    /// Host side: one point for `id` at `offset` (replacing an earlier one this block).
    fn set(&self, id: u32, offset: i32, value: f64) {
        if let Some((_, q)) = self.queue_for(id) {
            q.offset.store(offset, Ordering::Relaxed);
            q.value.store(value.to_bits(), Ordering::Relaxed);
            q.points.store(1, Ordering::Relaxed);
        }
    }

    fn queue_ptr(&self, index: usize) -> *mut sv::IParamValueQueue {
        self.queues.get(index).and_then(|q| q.as_com_ref::<sv::IParamValueQueue>()).map_or(std::ptr::null_mut(), |r| r.as_ptr())
    }
}

impl IParameterChangesTrait for ParamChanges {
    unsafe fn getParameterCount(&self) -> sb::int32 {
        self.count.load(Ordering::Relaxed)
    }
    unsafe fn getParameterData(&self, index: sb::int32) -> *mut sv::IParamValueQueue {
        if index < 0 || index >= self.count.load(Ordering::Relaxed) {
            return std::ptr::null_mut();
        }
        self.queue_ptr(index as usize)
    }
    unsafe fn addParameterData(&self, id: *const sv::ParamID, index: *mut sb::int32) -> *mut sv::IParamValueQueue {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&id) = (unsafe { id.as_ref() }) else { return std::ptr::null_mut() };
        let Some((i, _)) = self.queue_for(id) else { return std::ptr::null_mut() };
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(out) = unsafe { index.as_mut() } {
            *out = i;
        }
        self.queue_ptr(i as usize)
    }
}

/// A bounded event list (`IEventList`). Only ever touched by one thread at a time (the host
/// fills it, then the plugin reads it inside `process`), so the lock is never contended; the
/// plugin side uses `try_lock` and never blocks.
pub(crate) struct EventList {
    events: Mutex<Vec<sv::Event>>,
    cap: usize,
}

impl Class for EventList {
    type Interfaces = (sv::IEventList,);
}

impl EventList {
    fn new(cap: usize) -> EventList {
        EventList { events: Mutex::new(Vec::with_capacity(cap)), cap }
    }
    fn with<R>(&self, f: impl FnOnce(&mut Vec<sv::Event>) -> R) -> Option<R> {
        match self.events.try_lock() {
            Ok(mut g) => Some(f(&mut g)),
            Err(std::sync::TryLockError::Poisoned(p)) => Some(f(&mut p.into_inner())),
            Err(std::sync::TryLockError::WouldBlock) => None,
        }
    }
}

impl IEventListTrait for EventList {
    unsafe fn getEventCount(&self) -> sb::int32 {
        self.with(|v| i32::try_from(v.len()).unwrap_or(i32::MAX)).unwrap_or(0)
    }
    unsafe fn getEvent(&self, index: sb::int32, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: out-pointer from the plugin (or null, handled).
        let Some(out) = (unsafe { e.as_mut() }) else { return sb::kInvalidArgument };
        let Ok(i) = usize::try_from(index) else { return sb::kInvalidArgument };
        match self.with(|v| v.get(i).copied()).flatten() {
            Some(ev) => {
                *out = ev;
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
    unsafe fn addEvent(&self, e: *mut sv::Event) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(&ev) = (unsafe { e.as_ref() }) else { return sb::kInvalidArgument };
        let cap = self.cap;
        // Output events (e.g. MIDI out) are accepted while there is room; SoundCraft ignores them.
        match self.with(|v| {
            if v.len() < cap {
                v.push(ev);
                true
            } else {
                false
            }
        }) {
            Some(true) => sb::kResultOk,
            _ => sb::kResultFalse,
        }
    }
}

/// An in-memory `IBStream`, used to hand the component's state to its controller.
pub(crate) struct MemoryStream {
    inner: Mutex<(Vec<u8>, usize)>,
}

impl Class for MemoryStream {
    type Interfaces = (sb::IBStream,);
}

impl MemoryStream {
    fn new(data: Vec<u8>) -> ComWrapper<MemoryStream> {
        ComWrapper::new(MemoryStream { inner: Mutex::new((data, 0)) })
    }

    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut self.lock().0)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, (Vec<u8>, usize)> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

impl IBStreamTrait for MemoryStream {
    unsafe fn read(&self, buffer: *mut c_void, num_bytes: sb::int32, num_read: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let want = usize::try_from(num_bytes).unwrap_or(0);
        let avail = data.len().saturating_sub(*pos);
        let n = want.min(avail);
        if n > 0 && !buffer.is_null() {
            let Some(src) = data.get(*pos..*pos + n) else { return sb::kInternalError };
            // SAFETY: the plugin provides `buffer` with room for `num_bytes` ≥ `n` bytes; `src`
            // is `n` valid bytes; the regions cannot overlap (ours is a private Vec).
            unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buffer.cast::<u8>(), n) };
            *pos += n;
        }
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { num_read.as_mut() } {
            *r = i32::try_from(n).unwrap_or(0);
        }
        sb::kResultOk
    }
    unsafe fn write(&self, buffer: *mut c_void, num_bytes: sb::int32, num_written: *mut sb::int32) -> sb::tresult {
        let mut g = self.lock();
        let (data, pos) = &mut *g;
        let n = usize::try_from(num_bytes).unwrap_or(0);
        let end = pos.saturating_add(n);
        if buffer.is_null() || end > MAX_STATE_BYTES {
            return sb::kResultFalse;
        }
        if data.len() < end {
            data.resize(end, 0);
        }
        // SAFETY: the plugin provides `n` readable bytes at `buffer`; the destination range was
        // just made valid.
        let src = unsafe { std::slice::from_raw_parts(buffer.cast::<u8>(), n) };
        if let Some(dst) = data.get_mut(*pos..end) {
            dst.copy_from_slice(src);
        }
        *pos = end;
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(w) = unsafe { num_written.as_mut() } {
            *w = num_bytes;
        }
        sb::kResultOk
    }
    unsafe fn seek(&self, pos: sb::int64, mode: sb::int32, result: *mut sb::int64) -> sb::tresult {
        let mut g = self.lock();
        let len = i64::try_from(g.0.len()).unwrap_or(i64::MAX);
        let cur = i64::try_from(g.1).unwrap_or(0);
        let base = match mode as u32 {
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekSet as u32 => 0,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekCur as u32 => cur,
            x if x == sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as u32 => len,
            _ => return sb::kInvalidArgument,
        };
        let new = base.saturating_add(pos).clamp(0, len);
        g.1 = usize::try_from(new).unwrap_or(0);
        // SAFETY: out-pointer from the plugin (or null, handled).
        if let Some(r) = unsafe { result.as_mut() } {
            *r = new;
        }
        sb::kResultOk
    }
    unsafe fn tell(&self, pos: *mut sb::int64) -> sb::tresult {
        let g = self.lock();
        // SAFETY: out-pointer from the plugin (or null, handled).
        match unsafe { pos.as_mut() } {
            Some(p) => {
                *p = i64::try_from(g.1).unwrap_or(0);
                sb::kResultOk
            }
            None => sb::kInvalidArgument,
        }
    }
}

// ---- events and buffers --------------------------------------------------------------------

/// A host → plugin note event, safe to build anywhere; converted inside `process`. `channel` is
/// the MIDI channel, 0–15.
#[derive(Debug, Clone, Copy, PartialEq)]
pub(crate) enum NoteEvent {
    On { time: u32, channel: u8, key: u8, velocity: f32 },
    Off { time: u32, channel: u8, key: u8 },
    Pressure { time: u32, channel: u8, key: u8, pressure: f32 },
}

impl NoteEvent {
    pub fn time(&self) -> u32 {
        match *self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => time,
        }
    }
    pub fn set_time(&mut self, t: u32) {
        match self {
            NoteEvent::On { time, .. } | NoteEvent::Off { time, .. } | NoteEvent::Pressure { time, .. } => *time = t,
        }
    }

    fn to_raw(self, offset: u32) -> sv::Event {
        // SAFETY: `Event` is plain data (numbers and a union of plain structs); all-zero is valid.
        let mut e: sv::Event = unsafe { std::mem::zeroed() };
        e.busIndex = 0;
        e.sampleOffset = i32::try_from(self.time().saturating_sub(offset)).unwrap_or(0);
        e.flags = sv::Event_::EventFlags_::kIsLive as u16;
        match self {
            NoteEvent::On { channel, key, velocity, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOnEvent as u16;
                e.__field0.noteOn =
                    sv::NoteOnEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), tuning: 0.0, velocity, length: 0, noteId: -1 };
            }
            NoteEvent::Off { channel, key, .. } => {
                e.r#type = sv::Event_::EventTypes_::kNoteOffEvent as u16;
                e.__field0.noteOff =
                    sv::NoteOffEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), velocity: 0.0, noteId: -1, tuning: 0.0 };
            }
            NoteEvent::Pressure { channel, key, pressure, .. } => {
                e.r#type = sv::Event_::EventTypes_::kPolyPressureEvent as u16;
                e.__field0.polyPressure = sv::PolyPressureEvent { channel: i16::from(channel & 0x0F), pitch: i16::from(key), pressure, noteId: -1 };
            }
        }
        e
    }
}

/// Planar f32 buffers for every audio bus in one direction, with the pointer tables VST3 needs.
/// Allocated in `new`; `process` only rewrites pointers in place (no allocation).
pub(crate) struct BusBuffers {
    data: Vec<Vec<Vec<f32>>>,
    ptrs: Vec<Vec<*mut f32>>,
    bufs: Vec<sv::AudioBusBuffers>,
    frames: usize,
}

// SAFETY: the raw pointers only ever point into `data`, which this struct owns; moving the struct
// to another thread moves that ownership with it.
unsafe impl Send for BusBuffers {}

impl BusBuffers {
    /// One buffer set per bus, `channels[b]` channels each (capped), `frames` long.
    pub fn new(channels: &[u32], frames: usize) -> BusBuffers {
        let data: Vec<Vec<Vec<f32>>> =
            channels.iter().take(MAX_BUSES as usize).map(|&c| (0..c.min(MAX_BUS_CHANNELS)).map(|_| vec![0.0; frames]).collect()).collect();
        let ptrs = data.iter().map(|p| vec![std::ptr::null_mut(); p.len()]).collect();
        let bufs = data
            .iter()
            .map(|_| sv::AudioBusBuffers {
                numChannels: 0,
                silenceFlags: 0,
                __field0: sv::AudioBusBuffers__type0 { channelBuffers32: std::ptr::null_mut() },
            })
            .collect();
        BusBuffers { data, ptrs, bufs, frames }
    }

    pub fn channels(&self, bus: usize) -> usize {
        self.data.get(bus).map_or(0, Vec::len)
    }

    pub fn channel(&self, bus: usize, ch: usize) -> Option<&[f32]> {
        self.data.get(bus).and_then(|p| p.get(ch)).map(Vec::as_slice)
    }

    pub fn channel_mut(&mut self, bus: usize, ch: usize) -> Option<&mut [f32]> {
        self.data.get_mut(bus).and_then(|p| p.get_mut(ch)).map(Vec::as_mut_slice)
    }

    /// Points the VST3 tables at the current data (no allocation).
    fn refresh(&mut self) {
        for ((d, p), b) in self.data.iter_mut().zip(self.ptrs.iter_mut()).zip(self.bufs.iter_mut()) {
            for (dc, pc) in d.iter_mut().zip(p.iter_mut()) {
                *pc = dc.as_mut_ptr();
            }
            b.numChannels = i32::try_from(p.len()).unwrap_or(0);
            b.silenceFlags = 0;
            b.__field0.channelBuffers32 = p.as_mut_ptr();
        }
    }
}

// ---- instances -----------------------------------------------------------------------------

/// One parameter as reported by `IEditController::getParameterInfo`, plus its plain range.
#[derive(Debug, Clone)]
pub(crate) struct RawParam {
    pub id: u32,
    pub title: String,
    pub units: String,
    pub step_count: i32,
    pub default_normalized: f64,
    pub flags: i32,
    /// The unit (`IUnitInfo`) the parameter belongs to (0: the root unit).
    pub unit: i32,
}

pub(crate) const PARAM_HIDDEN: i32 = sv::ParameterInfo_::ParameterFlags_::kIsHidden;
pub(crate) const PARAM_READ_ONLY: i32 = sv::ParameterInfo_::ParameterFlags_::kIsReadOnly;
pub(crate) const PARAM_BYPASS: i32 = sv::ParameterInfo_::ParameterFlags_::kIsBypass;
pub(crate) const PARAM_PROGRAM_CHANGE: i32 = sv::ParameterInfo_::ParameterFlags_::kIsProgramChange;

/// Most units, program lists and programs read from `IUnitInfo`.
const MAX_UNITS: i32 = 1024;
const MAX_PROGRAM_LISTS: i32 = 256;
const MAX_PROGRAMS: i32 = 4096;

/// The audio bus layout chosen by [`Instance::configure`].
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Layout {
    pub inputs: Vec<u32>,
    pub outputs: Vec<u32>,
    pub event_input: bool,
}

fn apply_transport(c: &mut sv::ProcessContext, t: &Transport) {
    use sv::ProcessContext_::StatesAndFlags_::*;
    let mut state = kTempoValid | kTimeSigValid | kProjectTimeMusicValid | kBarPositionValid | kContTimeValid;
    if t.playing {
        state |= kPlaying;
    }
    if t.recording {
        state |= kRecording;
    }
    // The flag constants are the C default enum type: i32 on Windows, u32 elsewhere.
    #[allow(clippy::unnecessary_cast)]
    let state = state as u32;
    c.state = state;
    c.tempo = t.tempo;
    c.timeSigNumerator = i32::try_from(t.numerator).unwrap_or(4);
    c.timeSigDenominator = i32::try_from(t.denominator).unwrap_or(4);
    c.projectTimeSamples = t.samples;
    c.projectTimeMusic = t.quarters;
    c.barPositionMusic = t.bar_start;
}

/// A live plugin: component + audio processor (+ edit controller) and the host objects.
pub(crate) struct Instance {
    processor: Option<ComPtr<sv::IAudioProcessor>>,
    controller: Option<ComPtr<sv::IEditController>>,
    /// The controller is the component itself (single-component plugin).
    single: bool,
    connections: Option<(ComPtr<sv::IConnectionPoint>, ComPtr<sv::IConnectionPoint>)>,
    /// The host proxies the two connection points are connected to (towards the controller,
    /// towards the component), see [`ConnectionProxy`].
    proxies: Option<(ComWrapper<ConnectionProxy>, ComWrapper<ConnectionProxy>)>,
    component_initialized: bool,
    controller_initialized: bool,
    active: bool,
    processing: bool,
    max_frames: usize,
    in_params: ComWrapper<ParamChanges>,
    out_params: ComWrapper<ParamChanges>,
    in_events: ComWrapper<EventList>,
    out_events: ComWrapper<EventList>,
    context: sv::ProcessContext,
    event_in: bool,
    /// `kRealtime`, or `kOffline` for an offline render (set up in `configure`, repeated in
    /// every `ProcessData`).
    mode: i32,
    editor: Arc<EditorLink>,
    // Field order matters for drop: `Drop::drop` shuts the plugin down first; then the component
    // and host objects are released, and the bundle (which keeps the code mapped) goes last.
    component: ComPtr<sv::IComponent>,
    host: ComWrapper<HostContext>,
    bundle: Arc<Bundle>,
}

// SAFETY: an `Instance` is used through `&mut self` only, so calls into the plugin are always
// serialised. SoundCraft creates instances on the engine/UI thread and then moves them into the
// mix engine (documented limitation: VST3 expects `setActive`, controller calls etc. on the UI
// thread; they may then happen on the audio thread).
unsafe impl Send for Instance {}

impl Instance {
    fn processor(&self) -> Option<&ComPtr<sv::IAudioProcessor>> {
        self.processor.as_ref()
    }

    pub fn host(&self) -> &HostContext {
        &self.host
    }

    /// A counted reference to the host context (editor handles read its edit queue).
    pub fn host_ref(&self) -> ComWrapper<HostContext> {
        self.host.clone()
    }

    /// The editor link shared with editor handles (`None` without an edit controller).
    pub fn editor(&self) -> Option<Arc<EditorLink>> {
        self.editor.has_controller().then(|| Arc::clone(&self.editor))
    }

    /// The same link, for state handles.
    pub fn state_link(&self) -> Arc<EditorLink> {
        Arc::clone(&self.editor)
    }

    /// Writes one state into a fresh memory stream.
    fn read_state(f: impl FnOnce(*mut sb::IBStream) -> sb::tresult) -> Option<Vec<u8>> {
        let stream = MemoryStream::new(Vec::new());
        let s = stream.as_com_ref::<sb::IBStream>()?;
        ok(f(s.as_ptr())).then(|| stream.take())
    }

    /// Restores a [`EditorLink::save_state`] blob (or a bare component state): component
    /// `setState`, then the controller's `setComponentState` and `setState`. `false` when the
    /// blob is malformed or the component refuses it.
    pub fn load_state(&self, data: &[u8]) -> bool {
        let Some((comp, ctrl)) = split_state(data) else { return false };
        let stream = MemoryStream::new(comp.to_vec());
        let Some(s) = stream.as_com_ref::<sb::IBStream>() else { return false };
        // SAFETY: valid component; the stream lives on this frame for the call.
        if !ok(unsafe { self.component.setState(s.as_ptr()) }) {
            return false;
        }
        if let Some(c) = &self.controller {
            stream.lock().1 = 0;
            // SAFETY: valid controller; the same stream, rewound.
            unsafe { c.setComponentState(s.as_ptr()) };
            if !ctrl.is_empty() {
                let cs = MemoryStream::new(ctrl.to_vec());
                if let Some(r) = cs.as_com_ref::<sb::IBStream>() {
                    // SAFETY: valid controller; the stream lives on this frame for the call.
                    unsafe { c.setState(r.as_ptr()) };
                }
            }
        }
        true
    }

    /// Copies the component's state into the controller (`getState` → `setComponentState`).
    fn sync_controller_state(&self) {
        let Some(c) = &self.controller else { return };
        let stream = ComWrapper::new(MemoryStream { inner: Mutex::new((Vec::new(), 0)) });
        let Some(s) = stream.as_com_ref::<sb::IBStream>() else { return };
        // SAFETY: valid component; the stream lives on this frame for the whole call.
        if ok(unsafe { self.component.getState(s.as_ptr()) }) {
            stream.lock().1 = 0;
            // SAFETY: valid controller; same stream, rewound.
            unsafe { c.setComponentState(s.as_ptr()) };
        }
    }

    pub fn params(&self) -> Vec<RawParam> {
        let Some(c) = &self.controller else { return Vec::new() };
        // SAFETY: valid controller.
        let n = unsafe { c.getParameterCount() }.clamp(0, MAX_PARAMS);
        let mut out = Vec::new();
        for i in 0..n {
            // SAFETY: `ParameterInfo` is plain data; all-zero is valid.
            let mut info: sv::ParameterInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if !ok(unsafe { c.getParameterInfo(i, &mut info) }) {
                continue;
            }
            let d = info.defaultNormalizedValue;
            out.push(RawParam {
                id: info.id,
                title: read_c16(&info.title),
                units: read_c16(&info.units),
                step_count: info.stepCount.max(0),
                default_normalized: if d.is_finite() { d.clamp(0.0, 1.0) } else { 0.0 },
                flags: info.flags,
                unit: info.unitId,
            });
        }
        out
    }

    /// Normalized → plain (`None` without a controller or for a non-finite answer).
    pub fn to_plain(&self, id: u32, normalized: f64) -> Option<f64> {
        let c = self.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.normalizedParamToPlain(id, normalized.clamp(0.0, 1.0)) };
        v.is_finite().then_some(v)
    }

    /// Plain → normalized, clamped to 0..1.
    pub fn to_normalized(&self, id: u32, plain: f64) -> Option<f64> {
        let c = self.controller.as_ref()?;
        if !plain.is_finite() {
            return None;
        }
        // SAFETY: valid controller.
        let v = unsafe { c.plainParamToNormalized(id, plain) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    pub fn normalized_value(&self, id: u32) -> Option<f64> {
        let c = self.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.getParamNormalized(id) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// The parameter each MIDI controller drives (`IMidiMapping`) on each MIDI channel 0–15:
    /// [`MIDI_CTRLS`] entries per channel (index `channel * MIDI_CTRLS + controller`), by VST3's
    /// controller numbers: CCs 0–127, then [`AFTERTOUCH`], [`PITCH_BEND`] and
    /// [`PROGRAM_CHANGE`]. Empty when the controller has no MIDI mapping. Main thread only.
    pub fn midi_mapping(&self) -> Vec<Option<u32>> {
        let Some(m) = self.controller.as_ref().and_then(|c| c.cast::<sv::IMidiMapping>()) else { return Vec::new() };
        (0..16i16)
            .flat_map(|ch| (0..MIDI_CTRLS).map(move |n| (ch, n)))
            .map(|(ch, n)| {
                let mut id: sv::ParamID = 0;
                let n = i16::try_from(n).unwrap_or(0);
                // SAFETY: valid mapping interface on the main thread; `id` is a local.
                ok(unsafe { m.getMidiControllerAssignment(0, ch, n, &mut id) }).then_some(id)
            })
            .collect()
    }

    /// The program names (`IUnitInfo` program list) of unit `unit`'s list, else of the first list
    /// the controller has: the plugin's built-in programs, which its program-change parameter
    /// selects. Empty without `IUnitInfo` or program lists. Main thread only.
    pub fn program_names(&self, unit: i32) -> Vec<String> {
        let Some(u) = self.controller.as_ref().and_then(|c| c.cast::<sv::IUnitInfo>()) else { return Vec::new() };
        let mut wanted = None;
        // SAFETY: valid unit-info interface of the controller, main thread.
        let units = unsafe { u.getUnitCount() }.clamp(0, MAX_UNITS);
        for i in 0..units {
            // SAFETY: `UnitInfo` is plain data; all-zero is valid.
            let mut info: sv::UnitInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `info` is writable.
            if ok(unsafe { u.getUnitInfo(i, &mut info) }) && info.id == unit && info.programListId != sv::kNoProgramListId {
                wanted = Some(info.programListId);
                break;
            }
        }
        // SAFETY: as above.
        let lists = unsafe { u.getProgramListCount() }.clamp(0, MAX_PROGRAM_LISTS);
        // The unit's own list, else the first list.
        let (mut chosen, mut first) = (None, None);
        for i in 0..lists {
            // SAFETY: `ProgramListInfo` is plain data; all-zero is valid.
            let mut li: sv::ProgramListInfo = unsafe { std::mem::zeroed() };
            // SAFETY: index below the count; `li` is writable.
            if !ok(unsafe { u.getProgramListInfo(i, &mut li) }) {
                continue;
            }
            let list = (li.id, li.programCount.clamp(0, MAX_PROGRAMS));
            first = first.or(Some(list));
            if wanted == Some(li.id) {
                chosen = Some(list);
                break;
            }
        }
        log::debug!("vst3 programs: {units} units, {lists} lists, unit {unit} wants {wanted:?}, chose {:?}", chosen.or(first));
        let chosen = chosen.or(first);
        let Some((list, count)) = chosen else { return Vec::new() };
        (0..count)
            .map(|j| {
                let mut s: sv::String128 = [0; 128];
                // SAFETY: valid interface; `s` is a writable String128.
                let named = ok(unsafe { u.getProgramName(list, j, &mut s) }).then(|| read_c16(&s)).filter(|n| !n.is_empty());
                named.unwrap_or_else(|| format!("Program {}", j + 1))
            })
            .collect()
    }

    pub fn value_text(&self, id: u32, normalized: f64) -> Option<String> {
        let c = self.controller.as_ref()?;
        let mut s: sv::String128 = [0; 128];
        // SAFETY: valid controller; `s` is a writable String128.
        ok(unsafe { c.getParamStringByValue(id, normalized.clamp(0.0, 1.0), &mut s) }).then(|| read_c16(&s)).filter(|t| !t.is_empty())
    }

    pub fn latency(&self) -> u32 {
        // SAFETY: valid processor.
        self.processor().map_or(0, |p| unsafe { p.getLatencySamples() })
    }

    pub fn tail(&self) -> u32 {
        // SAFETY: valid processor.
        self.processor().map_or(0, |p| unsafe { p.getTailSamples() })
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    fn bus_count(&self, media: sv::MediaTypes, dir: sv::BusDirections) -> i32 {
        // SAFETY: valid component.
        unsafe { self.component.getBusCount(media as sv::MediaType, dir as sv::BusDirection) }.clamp(0, MAX_BUSES)
    }

    fn arrangement(&self, dir: sv::BusDirections, index: i32) -> Option<sv::SpeakerArrangement> {
        let p = self.processor()?;
        let mut arr: sv::SpeakerArrangement = 0;
        // SAFETY: valid processor; `arr` is writable.
        ok(unsafe { p.getBusArrangement(dir as sv::BusDirection, index, &mut arr) }).then_some(arr)
    }

    fn bus_channels(&self, dir: sv::BusDirections, index: i32) -> u32 {
        if let Some(arr) = self.arrangement(dir, index) {
            return arr.count_ones().min(MAX_BUS_CHANNELS);
        }
        // SAFETY: plain data; all-zero is valid.
        let mut info: sv::BusInfo = unsafe { std::mem::zeroed() };
        // SAFETY: valid component; index below the bus count; `info` is writable.
        if ok(unsafe { self.component.getBusInfo(sv::MediaTypes_::kAudio as sv::MediaType, dir as sv::BusDirection, index, &mut info) }) {
            u32::try_from(info.channelCount).unwrap_or(0).min(MAX_BUS_CHANNELS)
        } else {
            0
        }
    }

    /// Stops processing, deactivates, negotiates the bus layout (main buses mono or stereo as
    /// asked, falling back to stereo and then to the plugin's own choice), activates the main
    /// buses, sets up 32-bit processing (`kOffline` mode when `offline`: an offline render, as
    /// fast as the main thread goes, which disk-streaming instruments handle differently from
    /// realtime; a plugin that refuses that mode is set up in `kRealtime` instead, logged) and
    /// activates the component.
    pub fn configure(&mut self, sample_rate: f64, max_frames: usize, mono: bool, offline: bool) -> Result<Layout, Vst3Error> {
        self.shutdown();
        self.mode = if offline { sv::ProcessModes_::kOffline as i32 } else { sv::ProcessModes_::kRealtime as i32 };
        let audio = sv::MediaTypes_::kAudio;
        let event = sv::MediaTypes_::kEvent;
        let (din, dout) = (sv::BusDirections_::kInput, sv::BusDirections_::kOutput);
        let nin = self.bus_count(audio, din);
        let nout = self.bus_count(audio, dout);
        let current = |s: &Self, dir: sv::BusDirections, n: i32| -> Vec<sv::SpeakerArrangement> {
            (0..n).map(|i| s.arrangement(dir, i).unwrap_or(sv::SpeakerArr::kStereo)).collect()
        };
        let want = if mono { sv::SpeakerArr::kMono } else { sv::SpeakerArr::kStereo };
        let p = self.processor().ok_or_else(|| Vst3Error::Process("no audio processor".into()))?.clone();
        let mut tried_ok = false;
        for main in [want, sv::SpeakerArr::kStereo] {
            let mut ins = current(self, din, nin);
            let mut outs = current(self, dout, nout);
            if let Some(a) = ins.first_mut() {
                *a = main;
            }
            if let Some(a) = outs.first_mut() {
                *a = main;
            }
            // SAFETY: valid processor; the arrays hold exactly `nin`/`nout` arrangements and
            // outlive the call.
            if ok(unsafe { p.setBusArrangements(ins.as_mut_ptr(), nin, outs.as_mut_ptr(), nout) }) {
                tried_ok = true;
                break;
            }
            if main == sv::SpeakerArr::kStereo {
                break;
            }
        }
        if !tried_ok {
            log::debug!("vst3: plugin refused mono/stereo bus arrangements; using its own");
        }
        let layout = Layout {
            inputs: (0..nin).map(|i| self.bus_channels(din, i)).collect(),
            outputs: (0..nout).map(|i| self.bus_channels(dout, i)).collect(),
            event_input: self.bus_count(event, din) > 0,
        };
        // Main buses on, aux buses off (VST3 still gets buffers for every bus).
        for (dir, n) in [(din, nin), (dout, nout)] {
            for i in 0..n {
                // SAFETY: valid component; index below the bus count.
                unsafe { self.component.activateBus(audio as sv::MediaType, dir as sv::BusDirection, i, u8::from(i == 0)) };
            }
        }
        if layout.event_input {
            // SAFETY: valid component; event input bus 0 exists.
            unsafe { self.component.activateBus(event as sv::MediaType, din as sv::BusDirection, 0, 1) };
        }
        self.event_in = layout.event_input;
        let sample32 = sv::SymbolicSampleSizes_::kSample32 as i32;
        // SAFETY: valid processor.
        if !ok(unsafe { p.canProcessSampleSize(sample32) }) {
            return Err(Vst3Error::Process("plugin cannot process 32-bit float".into()));
        }
        let max = i32::try_from(max_frames).unwrap_or(i32::MAX);
        let mut setup = sv::ProcessSetup { processMode: self.mode, symbolicSampleSize: sample32, maxSamplesPerBlock: max, sampleRate: sample_rate };
        // SAFETY: valid processor, inactive; `setup` is a valid struct for the call.
        if !ok(unsafe { p.setupProcessing(&mut setup) }) {
            // A plugin that refuses offline mode (some only know `kRealtime`) still renders:
            // set it up as realtime rather than leave it unprepared (silent) for the render.
            if self.mode != sv::ProcessModes_::kRealtime as i32 {
                log::warn!("vst3: plugin refused setupProcessing in offline mode; rendering it in realtime mode");
                self.mode = sv::ProcessModes_::kRealtime as i32;
                setup.processMode = self.mode;
                // SAFETY: as above; the plugin refused the previous setup, so it is still inactive.
                if !ok(unsafe { p.setupProcessing(&mut setup) }) {
                    return Err(Vst3Error::Process("setupProcessing failed".into()));
                }
            } else {
                return Err(Vst3Error::Process("setupProcessing failed".into()));
            }
        }
        // SAFETY: valid component, configured above.
        if !ok(unsafe { self.component.setActive(1) }) {
            return Err(Vst3Error::Process("setActive failed".into()));
        }
        self.active = true;
        self.max_frames = max_frames;
        self.context.sampleRate = sample_rate;
        self.context.tempo = 120.0;
        self.context.timeSigNumerator = 4;
        self.context.timeSigDenominator = 4;
        // The flag constants are the C default enum type: i32 on Windows, u32 elsewhere.
        #[allow(clippy::unnecessary_cast)]
        let flags = (sv::ProcessContext_::StatesAndFlags_::kTempoValid | sv::ProcessContext_::StatesAndFlags_::kTimeSigValid) as u32;
        self.context.state = flags;
        self.host.latency_changed.store(false, Ordering::Relaxed);
        Ok(layout)
    }

    /// `setProcessing(false)` + `setActive(false)` when needed.
    pub fn shutdown(&mut self) {
        if self.processing {
            if let Some(p) = self.processor() {
                // SAFETY: valid processor in the processing state.
                unsafe { p.setProcessing(0) };
            }
            self.processing = false;
        }
        if self.active {
            // SAFETY: valid, active component.
            unsafe { self.component.setActive(0) };
            self.active = false;
        }
    }

    /// Runs one `process` call over `frames` frames: `params` become one point each at offset 0,
    /// `controls` (time, id, value) one point at their time when it falls in this call (the last
    /// one wins per parameter), and `notes` (sorted by time) have their times made relative to
    /// `offset`.
    pub fn process(
        &mut self,
        frames: usize,
        inputs: &mut BusBuffers,
        outputs: &mut BusBuffers,
        params: &[(u32, f64)],
        controls: &[(u32, u32, f64)],
        notes: &[NoteEvent],
        offset: u32,
        project_time: i64,
        transport: Option<&Transport>,
    ) -> Result<(), Vst3Error> {
        if !self.active {
            return Err(Vst3Error::Process("not active".into()));
        }
        if frames == 0 || frames > self.max_frames || frames > inputs.frames || frames > outputs.frames {
            return Err(Vst3Error::Process("block larger than the configured size".into()));
        }
        let p = self.processor.clone().ok_or_else(|| Vst3Error::Process("no audio processor".into()))?;
        if !self.processing {
            // Many plugins return kNotImplemented here; that is not an error.
            // SAFETY: valid, active processor.
            unsafe { p.setProcessing(1) };
            self.processing = true;
        }
        self.in_params.clear();
        for &(id, v) in params.iter().take(PARAM_QUEUE_CAP) {
            self.in_params.set(id, 0, v.clamp(0.0, 1.0));
        }
        for &(time, id, v) in controls {
            if let Some(at) = time.checked_sub(offset).filter(|&t| (t as usize) < frames) {
                self.in_params.set(id, i32::try_from(at).unwrap_or(0), v.clamp(0.0, 1.0));
            }
        }
        self.out_params.clear();
        let event_in = self.event_in;
        self.in_events.with(|v| {
            v.clear();
            if event_in {
                for n in notes.iter().take(EVENT_CAP) {
                    v.push(n.to_raw(offset));
                }
            }
        });
        self.out_events.with(Vec::clear);
        inputs.refresh();
        outputs.refresh();
        self.context.continousTimeSamples = project_time;
        match transport {
            Some(t) => apply_transport(&mut self.context, t),
            None => self.context.projectTimeSamples = project_time,
        }
        let mut data = sv::ProcessData {
            processMode: self.mode,
            symbolicSampleSize: sv::SymbolicSampleSizes_::kSample32 as i32,
            numSamples: i32::try_from(frames).unwrap_or(0),
            numInputs: i32::try_from(inputs.bufs.len()).unwrap_or(0),
            numOutputs: i32::try_from(outputs.bufs.len()).unwrap_or(0),
            inputs: if inputs.bufs.is_empty() { std::ptr::null_mut() } else { inputs.bufs.as_mut_ptr() },
            outputs: if outputs.bufs.is_empty() { std::ptr::null_mut() } else { outputs.bufs.as_mut_ptr() },
            inputParameterChanges: self.in_params.as_com_ref::<sv::IParameterChanges>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            outputParameterChanges: self.out_params.as_com_ref::<sv::IParameterChanges>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            inputEvents: self.in_events.as_com_ref::<sv::IEventList>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            outputEvents: self.out_events.as_com_ref::<sv::IEventList>().map_or(std::ptr::null_mut(), |r| r.as_ptr()),
            processContext: &mut self.context,
        };
        // SAFETY: valid, active processor. Every channel pointer refers to `frames` (or more) valid
        // f32s owned by `inputs`/`outputs`, borrowed for the whole call; the bus tables, the
        // parameter/event objects and the context are owned by `self` and untouched until the
        // call returns.
        let r = unsafe { p.process(&mut data) };
        if ok(r) { Ok(()) } else { Err(Vst3Error::Process(format!("process returned {r}"))) }
    }
}

impl Drop for Instance {
    fn drop(&mut self) {
        // Close any editor and cut its handles off before the controller goes away.
        if self.editor.kill() {
            self.bundle.mark_leaked();
        }
        self.shutdown();
        if let (Some((a, b)), Some((to_controller, to_component))) = (self.connections.take(), self.proxies.take()) {
            if let (Some(pc), Some(pk)) = (to_controller.as_com_ref::<sv::IConnectionPoint>(), to_component.as_com_ref::<sv::IConnectionPoint>()) {
                // SAFETY: the sides were connected to these proxies in `instantiate`; disconnect
                // before terminating.
                unsafe {
                    a.disconnect(pc.as_ptr());
                    b.disconnect(pk.as_ptr());
                }
            }
            // Queued messages and the targets go now, before either side terminates.
            to_controller.detach();
            to_component.detach();
        }
        if let Some(c) = self.controller.take() {
            // SAFETY: valid controller; detach our handler before it goes away.
            unsafe { c.setComponentHandler(std::ptr::null_mut()) };
            if self.controller_initialized {
                // SAFETY: separately created controller, initialised once; terminated once.
                unsafe { c.terminate() };
            }
        }
        self.processor = None;
        if self.component_initialized {
            // SAFETY: valid component, inactive; terminated exactly once.
            unsafe { self.component.terminate() };
            self.component_initialized = false;
        }
    }
}

/// Splits a state blob into (component, controller) parts. A blob without SoundCraft's container
/// header is taken as a bare component state.
fn split_state(data: &[u8]) -> Option<(&[u8], &[u8])> {
    if data.len() > MAX_STATE_BYTES.saturating_mul(2) {
        return None;
    }
    let Some(rest) = data.strip_prefix(STATE_MAGIC) else { return (!data.is_empty()).then_some((data, &[][..])) };
    let take = |b: &[u8]| -> Option<(usize, usize)> {
        let n = u32::from_le_bytes(b.get(..4)?.try_into().ok()?) as usize;
        (n <= MAX_STATE_BYTES && b.len() >= 4 + n).then_some((4, 4 + n))
    };
    let (a, b) = take(rest)?;
    let comp = rest.get(a..b)?;
    let rest = rest.get(b..)?;
    let (c, d) = take(rest)?;
    let ctrl = rest.get(c..d)?;
    (d == rest.len()).then_some((comp, ctrl))
}

// ---- editor (IPlugView) --------------------------------------------------------------------

/// The `IPlugFrame` handed to a plugin view: `resizeView` resizes our host window.
pub(crate) struct PlugFrame {
    /// The host window (an `NSWindow*` on macOS), not owned; null when there is none.
    window: std::sync::atomic::AtomicPtr<c_void>,
}

impl Class for PlugFrame {
    type Interfaces = (sb::IPlugFrame,);
}

impl IPlugFrameTrait for PlugFrame {
    unsafe fn resizeView(&self, view: *mut sb::IPlugView, new_size: *mut sb::ViewRect) -> sb::tresult {
        // SAFETY: in-pointer from the plugin (or null, handled).
        let Some(r) = (unsafe { new_size.as_mut() }) else { return sb::kInvalidArgument };
        let (w, h) = rect_size(r);
        window::resize(self.window.load(Ordering::Relaxed), w, h);
        // SAFETY: the plugin passes its own live view (or null, handled).
        if let Some(v) = unsafe { vst3::ComRef::from_raw(view) } {
            // SAFETY: valid view; `r` is the plugin's own writable rect.
            unsafe { v.onSize(r) };
        }
        sb::kResultOk
    }
}

/// Width and height of a view rect, clamped to a sane window size.
fn rect_size(r: &sb::ViewRect) -> (f64, f64) {
    let w = r.right.saturating_sub(r.left).clamp(1, 16_384);
    let h = r.bottom.saturating_sub(r.top).clamp(1, 16_384);
    (f64::from(w), f64::from(h))
}

#[derive(Default)]
struct EditorInner {
    /// `None` once the instance is gone.
    component: Option<ComPtr<sv::IComponent>>,
    /// `None` once the instance is gone, or without an edit controller.
    controller: Option<ComPtr<sv::IEditController>>,
    view: Option<ComPtr<sb::IPlugView>>,
    frame: Option<ComWrapper<PlugFrame>>,
    window: Option<window::HostWindow>,
    /// The proxy towards the controller: its queued processor messages are delivered from
    /// `idle` (`None` for single-component plugins).
    messages: Option<ComWrapper<ConnectionProxy>>,
    /// The proxy towards the component: the messages a controller sent off the main thread
    /// are delivered from `idle` too (`None` for single-component plugins).
    to_component: Option<ComWrapper<ConnectionProxy>>,
}

/// The link between an instance and its editor and state handles. The instance may process on
/// the audio thread while the editor (the edit controller's `IPlugView`) is driven and the state
/// read from the main thread, which is VST3's own threading model (controller, views and
/// `getState` on the UI thread). All such calls happen under this lock, and the instance closes
/// the view and drops the component and controller references under the same lock before it
/// terminates, so a handle never calls into a dead plugin.
#[derive(Default)]
pub(crate) struct EditorLink {
    inner: Mutex<EditorInner>,
}

impl EditorLink {
    fn lock(&self) -> std::sync::MutexGuard<'_, EditorInner> {
        self.inner.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn has_controller(&self) -> bool {
        self.lock().controller.is_some()
    }

    fn create_view(g: &EditorInner) -> Result<ComPtr<sb::IPlugView>, String> {
        let c = g.controller.as_ref().ok_or_else(|| "the plugin is gone".to_string())?;
        // SAFETY: valid controller; the name is a static NUL-terminated string.
        let raw = unsafe { c.createView(c"editor".as_ptr()) };
        // SAFETY: a non-null view comes with one reference owned by the caller.
        unsafe { ComPtr::from_raw(raw) }.ok_or_else(|| "this plugin has no editor".to_string())
    }

    /// Creates the view, checks it supports this platform's window type and reads its size,
    /// without attaching it anywhere. For diagnostics and tests.
    pub fn probe(&self) -> Result<(u32, u32), String> {
        let g = self.lock();
        let view = EditorLink::create_view(&g)?;
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "plugin editors are not supported on this platform".to_string())?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let mut r = sb::ViewRect { left: 0, top: 0, right: 0, bottom: 0 };
        // SAFETY: valid view; `r` is writable.
        if !ok(unsafe { view.getSize(&mut r) }) {
            return Err("the editor reports no size".into());
        }
        let (w, h) = rect_size(&r);
        Ok((w as u32, h as u32))
    }

    /// Opens (or raises) the editor in a host window. Main thread only.
    pub fn open(&self, title: &str) -> Result<(), String> {
        if !window::on_main_thread() {
            return Err("plugin editors must be opened on the main thread".into());
        }
        let platform = window::PLATFORM_TYPE.ok_or_else(|| "VST3 editors are not hosted on this platform yet".to_string())?;
        let mut g = self.lock();
        if let Some(w) = &g.window {
            if w.is_visible() {
                w.raise();
                return Ok(());
            }
            // The user closed the window: start over.
            EditorLink::teardown(&mut g);
        }
        let view = EditorLink::create_view(&g)?;
        // SAFETY: valid view; `platform` is a static NUL-terminated string.
        if unsafe { view.isPlatformTypeSupported(platform) } != sb::kResultTrue {
            return Err("the editor does not support this platform's windows".into());
        }
        let mut r = sb::ViewRect { left: 0, top: 0, right: 400, bottom: 300 };
        // SAFETY: valid view; `r` is writable. On failure the default size stays.
        unsafe { view.getSize(&mut r) };
        let (w, h) = rect_size(&r);
        let win = window::HostWindow::new(title, w, h)?;
        let frame = ComWrapper::new(PlugFrame { window: std::sync::atomic::AtomicPtr::new(win.raw()) });
        if let Some(f) = frame.as_com_ref::<sb::IPlugFrame>() {
            // SAFETY: valid view; the frame outlives the view's attachment (kept in `g.frame`
            // and detached in `teardown`).
            unsafe { view.setFrame(f.as_ptr()) };
        }
        // SAFETY: valid view; the parent is the live content view of `win`, of the type named.
        if !ok(unsafe { view.attached(win.parent(), platform) }) {
            // SAFETY: valid view; detach our frame again.
            unsafe { view.setFrame(std::ptr::null_mut()) };
            win.close();
            return Err("the editor could not attach to its window".into());
        }
        win.raise();
        g.view = Some(view);
        g.frame = Some(frame);
        g.window = Some(win);
        Ok(())
    }

    fn teardown(g: &mut EditorInner) {
        if let Some(view) = g.view.take() {
            // SAFETY: valid, attached view; main thread (callers check). `removed` before the
            // window goes, and the frame is detached before it is released.
            unsafe {
                view.removed();
                view.setFrame(std::ptr::null_mut());
            }
        }
        g.frame = None;
        if let Some(w) = g.window.take() {
            w.close();
        }
    }

    /// Hands a host-set value to the controller before a state save (`setParamNormalized`,
    /// under the lock like [`EditorLink::save_state`]). No-op once the instance is gone.
    pub fn sync_param(&self, id: u32, n: f64) {
        if !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller (the instance clears it under this lock before it terminates).
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// Restores a [`EditorLink::save_state`] blob (or a bare component state) into the live
    /// instance, as [`Instance::load_state`] does: component `setState`, then the controller's
    /// `setComponentState` and `setState`, under this lock. `false` when the blob is malformed,
    /// the instance is gone or the component refuses it.
    pub fn load_state(&self, data: &[u8]) -> bool {
        let Some((comp, ctrl)) = split_state(data) else { return false };
        let g = self.lock();
        let Some(component) = g.component.as_ref() else { return false };
        let stream = MemoryStream::new(comp.to_vec());
        let Some(s) = stream.as_com_ref::<sb::IBStream>() else { return false };
        // SAFETY: valid component (cleared under this lock before the instance terminates); the
        // stream lives on this frame for the call.
        if !ok(unsafe { component.setState(s.as_ptr()) }) {
            return false;
        }
        if let Some(c) = &g.controller {
            stream.lock().1 = 0;
            // SAFETY: valid controller (as above); the same stream, rewound.
            unsafe { c.setComponentState(s.as_ptr()) };
            if !ctrl.is_empty() {
                let cs = MemoryStream::new(ctrl.to_vec());
                if let Some(r) = cs.as_com_ref::<sb::IBStream>() {
                    // SAFETY: valid controller; the stream lives on this frame for the call.
                    unsafe { c.setState(r.as_ptr()) };
                }
            }
        }
        true
    }

    /// The component state plus the controller state, in SoundCraft's container (see
    /// `STATE_MAGIC`). `None` when the component refuses `getState` or the instance is gone.
    pub fn save_state(&self) -> Option<Vec<u8>> {
        let g = self.lock();
        let component = g.component.as_ref()?;
        // SAFETY: valid component (the instance clears it under this lock before it terminates);
        // the stream lives in `read_state` for the whole call.
        let comp = Instance::read_state(|s| unsafe { component.getState(s) })?;
        let ctrl = match &g.controller {
            // SAFETY: valid controller; as above.
            Some(c) => Instance::read_state(|s| unsafe { c.getState(s) }).unwrap_or_default(),
            None => Vec::new(),
        };
        crate::vstpreset::pack_state(&comp, &ctrl)
    }

    /// Closes the editor. Main thread only (no-op elsewhere).
    pub fn close(&self) {
        if window::on_main_thread() {
            EditorLink::teardown(&mut self.lock());
        }
    }

    pub fn is_open(&self) -> bool {
        let g = self.lock();
        match &g.window {
            Some(w) if window::on_main_thread() => w.is_visible(),
            Some(_) => true,
            None => false,
        }
    }

    /// A picture of the open editor (see `window::capture`). The lock is released before the
    /// plugin is asked to draw, so a plugin calling back into the host cannot deadlock it; the
    /// window is only destroyed on this (the main) thread, and `window::capture` checks it.
    pub fn capture(&self) -> Option<(u32, u32, Vec<u8>)> {
        if !window::on_main_thread() {
            return None;
        }
        let raw = {
            let g = self.lock();
            g.view.as_ref()?;
            g.window.as_ref()?.raw()
        };
        window::capture(raw)
    }

    /// Main-thread housekeeping, once per UI frame: tears the view down when the user closed its
    /// window, and services an open editor (its timers and the paints they invalidate, see
    /// `window::service`), so it keeps drawing while the app's own window repaints continuously
    /// during playback.
    pub fn idle(&self) {
        if !window::on_main_thread() {
            return;
        }
        let raw = {
            let mut g = self.lock();
            if g.window.as_ref().is_some_and(|w| !w.is_visible()) {
                EditorLink::teardown(&mut g);
            }
            // Messages the processor sent from the audio thread reach the controller here, on
            // the UI thread, under the lock like every other controller call; so do the ones
            // the controller sent from one of its own threads (a preset browser, a loader),
            // which the processor's `notify` takes on the UI thread like the direct ones.
            if let Some(m) = &g.messages {
                m.deliver();
            }
            if let Some(m) = &g.to_component {
                m.deliver();
            }
            g.view.as_ref().and(g.window.as_ref()).map(window::HostWindow::raw)
        };
        // The lock is released before the plugin's timers and paints run (as in `capture`): a
        // plugin calling back into the host from them (a resize, an edit) cannot deadlock.
        if let Some(raw) = raw {
            window::service(raw);
        }
    }

    /// Normalized → plain through the controller (main thread).
    pub fn to_plain(&self, id: u32, n: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        // SAFETY: valid controller.
        let v = unsafe { c.normalizedParamToPlain(id, n.clamp(0.0, 1.0)) };
        v.is_finite().then_some(v)
    }

    /// The controller's current normalized value of a parameter (main thread; `None` once the
    /// instance is gone).
    pub fn normalized_value(&self, id: u32) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        // SAFETY: valid controller (cleared under this lock before the instance terminates).
        let v = unsafe { c.getParamNormalized(id) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// Plain → normalized through the controller (main thread).
    pub fn to_normalized(&self, id: u32, plain: f64) -> Option<f64> {
        let g = self.lock();
        let c = g.controller.as_ref()?;
        if !plain.is_finite() {
            return None;
        }
        // SAFETY: valid controller.
        let v = unsafe { c.plainParamToNormalized(id, plain) };
        v.is_finite().then(|| v.clamp(0.0, 1.0))
    }

    /// The plugin's own text for a normalized value (`getParamStringByValue`). Main thread only
    /// (`None` elsewhere, and once the instance is gone).
    pub fn value_text(&self, id: u32, n: f64) -> Option<String> {
        if !window::on_main_thread() || !n.is_finite() {
            return None;
        }
        let g = self.lock();
        let c = g.controller.as_ref()?;
        let mut s: sv::String128 = [0; 128];
        // SAFETY: valid controller (the instance clears it under this lock before it terminates),
        // main thread; `s` is a writable String128.
        ok(unsafe { c.getParamStringByValue(id, n.clamp(0.0, 1.0), &mut s) }).then(|| read_c16(&s)).filter(|t| !t.is_empty())
    }

    /// Shows a value change in the editor (`setParamNormalized`, main thread).
    pub fn set_normalized(&self, id: u32, n: f64) {
        if !window::on_main_thread() || !n.is_finite() {
            return;
        }
        let g = self.lock();
        if let Some(c) = &g.controller {
            // SAFETY: valid controller, main thread.
            unsafe { c.setParamNormalized(id, n.clamp(0.0, 1.0)) };
        }
    }

    /// Called by the instance before it terminates: closes the editor and cuts the handles off.
    /// Returns whether plugin objects had to be leaked.
    fn kill(&self) -> bool {
        let mut g = self.lock();
        let mut leaked = false;
        if g.view.is_some() || g.window.is_some() {
            if window::on_main_thread() {
                EditorLink::teardown(&mut g);
            } else {
                // Window calls are main-thread only; leak the view and window rather than crash.
                log::warn!("vst3: editor still open while its plugin is destroyed off the main thread");
                if let Some(v) = g.view.take() {
                    std::mem::forget(v);
                    leaked = true;
                }
                // `HostWindow` has no destructor: dropping it leaks its retain on purpose.
                g.window = None;
                if let Some(f) = g.frame.take() {
                    std::mem::forget(f);
                }
            }
        }
        g.component = None;
        g.controller = None;
        g.messages = None;
        g.to_component = None;
        leaked
    }
}

/// The platform host window for plugin views.
#[cfg(target_os = "macos")]
mod window {
    use objc2::rc::Retained;
    use objc2::{MainThreadMarker, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSBackingStoreType, NSFloatingWindowLevel, NSWindow, NSWindowCollectionBehavior, NSWindowStyleMask};
    use objc2_foundation::{NSPoint, NSRect, NSSize, NSString};
    use std::ffi::{c_char, c_void};

    pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeNSView);

    pub fn on_main_thread() -> bool {
        MainThreadMarker::new().is_some()
    }

    /// An `NSWindow` (titled, closable, floating above the app's windows) whose content view is
    /// the plugin view's parent. Holds one retain of the window; main thread only.
    pub struct HostWindow {
        window: *mut NSWindow,
    }

    // SAFETY: the pointer is only dereferenced on the main thread (every method checks
    // `MainThreadMarker`); the struct merely carries the retain between threads' locks.
    unsafe impl Send for HostWindow {}

    impl HostWindow {
        fn get(&self) -> Option<&NSWindow> {
            MainThreadMarker::new()?;
            // SAFETY: we hold a retain of this window (from `new`) until `close`.
            unsafe { self.window.as_ref() }
        }

        pub fn new(title: &str, w: f64, h: f64) -> Result<HostWindow, String> {
            let mtm = MainThreadMarker::new().ok_or_else(|| "not on the main thread".to_string())?;
            let _app = NSApplication::sharedApplication(mtm);
            let rect = NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(w, h));
            // Plugin editors are transient windows. In particular they must not be
            // minimizable: minimized NSWindows become persistent thumbnails in the macOS Dock,
            // which made every editor visited in a session look like another running app.
            let style = NSWindowStyleMask::Titled | NSWindowStyleMask::Closable;
            // SAFETY: a freshly allocated window on the main thread; valid rect and style.
            let window = unsafe {
                NSWindow::initWithContentRect_styleMask_backing_defer(NSWindow::alloc(mtm), rect, style, NSBackingStoreType::Buffered, false)
            };
            // SAFETY: we keep our own retain and release it in `close`, so AppKit must not
            // release the window when the user closes it.
            unsafe { window.setReleasedWhenClosed(false) };
            window.setTitle(&NSString::from_str(title));
            window.setLevel(NSFloatingWindowLevel);
            window.setHidesOnDeactivate(true);
            window.setExcludedFromWindowsMenu(true);
            window.setCollectionBehavior(NSWindowCollectionBehavior::Transient | NSWindowCollectionBehavior::IgnoresCycle);
            window.center();
            Ok(HostWindow { window: Retained::into_raw(window) })
        }

        /// The raw window pointer (for the plug frame's resizes).
        pub fn raw(&self) -> *mut c_void {
            self.window.cast()
        }

        /// The content view (`NSView*`) the plugin view attaches to; null when unavailable.
        pub fn parent(&self) -> *mut c_void {
            self.get().and_then(|w| w.contentView()).map_or(std::ptr::null_mut(), |v| Retained::as_ptr(&v).cast_mut().cast())
        }

        pub fn raise(&self) {
            if let Some(w) = self.get() {
                w.makeKeyAndOrderFront(None);
            }
        }

        pub fn is_visible(&self) -> bool {
            self.get().is_some_and(|w| w.isVisible())
        }

        pub fn close(self) {
            if let Some(w) = self.get() {
                w.orderOut(None);
                w.close();
            }
            if MainThreadMarker::new().is_some() {
                // SAFETY: balances the retain taken in `new`; nothing uses the pointer after this.
                drop(unsafe { Retained::from_raw(self.window) });
            }
        }
    }

    /// Resizes a host window's content (`window` from [`HostWindow::raw`]); main thread only.
    pub fn resize(window: *mut c_void, w: f64, h: f64) {
        if MainThreadMarker::new().is_none() {
            return;
        }
        // SAFETY: null or a window the editor link keeps retained while the frame is attached.
        if let Some(win) = unsafe { window.cast::<NSWindow>().as_ref() } {
            win.setContentSize(NSSize::new(w, h));
        }
    }

    /// Editor pictures are taken on Windows only.
    pub fn capture(_window: *mut c_void) -> Option<(u32, u32, Vec<u8>)> {
        None
    }

    /// Nothing to service here: AppKit's run loop drives the window's timers and drawing.
    pub fn service(_window: *mut c_void) {}

    /// No messages to dispatch here (macOS: the app's run loop drives windows).
    pub fn pump(time: std::time::Duration) {
        std::thread::sleep(time);
    }
}

/// The Windows host window: a top-level window owned by the app's active window (so it stays
/// above it and has no taskbar button) whose client area is the plugin view's `HWND` parent.
/// Closing it only hides it; the editor link notices in `idle`, detaches the view, then destroys
/// the window. Messages are dispatched by the app's own event loop on the main thread.
#[cfg(windows)]
mod window {
    use std::ffi::{c_char, c_void};
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
        SetForegroundWindow, SetWindowPos, ShowWindow, TranslateMessage, WM_CLOSE, WM_TIMER, WNDCLASSEXW, WS_CAPTION, WS_CLIPCHILDREN, WS_OVERLAPPED,
        WS_SYSMENU,
    };

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

    /// Dispatches this thread's pending `WM_TIMER` messages.
    ///
    /// Win32 retrieves timer messages (and paints) only when nothing else is queued. While the
    /// transport plays, the app's own window always has a repaint pending (eframe re-requests
    /// it before the event loop returns to `PeekMessageW`), so a plugin editor's timer-driven
    /// repaints (VSTGUI, JUCE and Waves GUIs all repaint from `SetTimer`) never ran: a knob
    /// dragged during playback moved in the audio but not on screen until Pause. A
    /// `PeekMessageW` filtered to `WM_TIMER` alone bypasses that priority rule. The pump is
    /// thread-wide, not filtered to the editor's window: the app's loop (winit 0.30.13, eframe
    /// 0.36; checked: no `SetTimer`/`WM_TIMER` in its Windows backend) uses no timers, so every
    /// timer message dispatched here is a plugin's. A winit upgrade that adds a timer would
    /// have it dispatched from here, out of order with its other messages (harmless, but
    /// re-check on upgrade; a thread timer with a `TIMERPROC` is run by `DispatchMessageW`
    /// either way).
    fn pump_timers() {
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

    /// Per-frame service of an open editor's host window (`window` from [`HostWindow::raw`]):
    /// the thread's due timers (see `pump_timers`), then the paints they invalidated
    /// (`RDW_UPDATENOW` sends `WM_PAINT` to the window and its children now, instead of when the
    /// queue is otherwise empty; never done for the app's own window, whose paint is eframe's).
    /// Main thread only; a few microseconds when nothing is due.
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
            pump_timers();
        }
        // SAFETY: a live, shown window of this thread; null rect and region mean the whole window.
        unsafe { RedrawWindow(window, std::ptr::null(), std::ptr::null_mut(), RDW_UPDATENOW | RDW_ALLCHILDREN) };
    }

    pub const PLATFORM_TYPE: Option<*const c_char> = Some(vst3::Steinberg::kPlatformTypeHWND);

    /// Rust names the process's first thread "main"; the app runs its UI (and message loop) there.
    pub fn on_main_thread() -> bool {
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
                let name = wide("SoundCraftVst3Editor");
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
                return Err("not on the main thread".into());
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

        /// The window handle (for the plug frame's resizes).
        pub fn raw(&self) -> *mut c_void {
            self.hwnd
        }

        /// The `HWND` the plugin view creates its child window in.
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
                // SAFETY: our own live window; the view was detached before (see `teardown`).
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

    /// Dispatches this thread's window messages for `time` (an app's event loop does this; tests
    /// and tools that open editors without one call it).
    pub fn pump(time: std::time::Duration) {
        let end = std::time::Instant::now() + time;
        loop {
            // SAFETY: `MSG` is plain data; all-zero is a valid value to be overwritten.
            let mut msg: MSG = unsafe { std::mem::zeroed() };
            // SAFETY: `msg` is writable; null asks for every window of this thread.
            while unsafe { PeekMessageW(&mut msg, std::ptr::null_mut(), 0, 0, PM_REMOVE) } != 0 {
                // SAFETY: a message just retrieved for this thread.
                unsafe {
                    TranslateMessage(&msg);
                    DispatchMessageW(&msg);
                }
            }
            if std::time::Instant::now() >= end {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        use std::sync::atomic::AtomicUsize;
        use windows_sys::Win32::Graphics::Gdi::{InvalidateRect, ValidateRect};
        use windows_sys::Win32::UI::WindowsAndMessaging::{
            KillTimer, LWA_ALPHA, SW_SHOWNOACTIVATE, SetLayeredWindowAttributes, SetTimer, WM_PAINT, WS_EX_LAYERED, WS_EX_NOACTIVATE,
            WS_EX_TOOLWINDOW, WS_EX_TRANSPARENT, WS_POPUP,
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
            let name = wide("SoundCraftVst3EditorServiceTest");
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

/// Other platforms: no editor windows yet (opening reports "unsupported").
#[cfg(not(any(target_os = "macos", windows)))]
mod window {
    use std::ffi::{c_char, c_void};

    pub const PLATFORM_TYPE: Option<*const c_char> = None;

    pub fn on_main_thread() -> bool {
        true
    }

    pub struct HostWindow;

    impl HostWindow {
        pub fn new(_title: &str, _w: f64, _h: f64) -> Result<HostWindow, String> {
            Err("VST3 editors are not hosted on this platform yet".into())
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

    pub fn service(_window: *mut c_void) {}

    /// No messages to dispatch here (macOS: the app's run loop drives windows).
    pub fn pump(time: std::time::Duration) {
        std::thread::sleep(time);
    }
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
    fn the_transport_fills_the_process_context() {
        use sv::ProcessContext_::StatesAndFlags_::*;
        // SAFETY: `ProcessContext` is plain data (numbers and small structs); all-zero is valid.
        let mut c: sv::ProcessContext = unsafe { std::mem::zeroed() };
        let t = Transport {
            playing: true,
            tempo: 90.0,
            numerator: 3,
            denominator: 4,
            samples: 96_000,
            quarters: 3.5,
            bar_start: 3.0,
            ..Transport::default()
        };
        apply_transport(&mut c, &t);
        let playing = kPlaying as u32;
        assert_eq!(c.state & playing, playing);
        assert_eq!(c.state & kRecording as u32, 0);
        assert_eq!((c.tempo, c.timeSigNumerator, c.timeSigDenominator), (90.0, 3, 4));
        assert_eq!((c.projectTimeSamples, c.projectTimeMusic, c.barPositionMusic), (96_000, 3.5, 3.0));
        apply_transport(&mut c, &Transport { playing: false, ..t });
        assert_eq!(c.state & playing, 0);
    }

    #[test]
    fn state_container_parses_strictly() {
        let mut blob = b"SCV3".to_vec();
        blob.extend_from_slice(&3u32.to_le_bytes());
        blob.extend_from_slice(b"abc");
        blob.extend_from_slice(&2u32.to_le_bytes());
        blob.extend_from_slice(b"xy");
        assert_eq!(split_state(&blob), Some((&b"abc"[..], &b"xy"[..])));
        assert_eq!(split_state(b"raw component"), Some((&b"raw component"[..], &[][..])), "bare component state");
        assert_eq!(split_state(b""), None);
        let mut trailing = blob.clone();
        trailing.push(0);
        assert_eq!(split_state(&trailing), None);
        assert_eq!(split_state(&blob[..blob.len() - 1]), None);
        let mut huge = b"SCV3".to_vec();
        huge.extend_from_slice(&u32::MAX.to_le_bytes());
        assert_eq!(split_state(&huge), None);
        assert_eq!(split_state(b"SCV3"), None);
    }

    #[test]
    fn note_events_carry_their_channel() {
        let on = NoteEvent::On { time: 0, channel: 9, key: 36, velocity: 1.0 }.to_raw(0);
        let off = NoteEvent::Off { time: 0, channel: 15, key: 36 }.to_raw(0);
        let pressure = NoteEvent::Pressure { time: 0, channel: 3, key: 36, pressure: 0.5 }.to_raw(0);
        // SAFETY: each union field read is the one its event type wrote.
        unsafe {
            assert_eq!((on.__field0.noteOn.channel, on.__field0.noteOn.pitch), (9, 36));
            assert_eq!(off.__field0.noteOff.channel, 15);
            assert_eq!(pressure.__field0.polyPressure.channel, 3);
        }
    }

    #[test]
    fn strings_are_bounded() {
        let mut b = [0u16; 4];
        write_c16("SoundCraft", &mut b);
        assert_eq!(read_c16(&b), "Sou");
        let full: Vec<u16> = "abcd".encode_utf16().collect();
        assert_eq!(read_c16(&full), "abcd");
        let c: Vec<c_char> = b"Fx|EQ\0junk".iter().map(|&x| x as c_char).collect();
        assert_eq!(read_c8(&c), "Fx|EQ");
        write_c16("x", &mut []);
    }

    #[test]
    fn host_objects_behave() {
        let pc = ParamChanges::new(2);
        pc.set(5, 0, 0.25);
        pc.set(5, 0, 0.5);
        pc.set(6, 0, 1.0);
        pc.set(7, 0, 1.0);
        // SAFETY: test calls through our own implementation.
        unsafe {
            assert_eq!(pc.getParameterCount(), 2, "full pool drops extra params");
            assert!(pc.getParameterData(2).is_null());
            assert!(pc.getParameterData(-1).is_null());
            let q = pc.queues[0].as_com_ref::<sv::IParamValueQueue>().unwrap();
            let (mut o, mut v) = (9, 0.0);
            assert_eq!(q.getPoint(0, &mut o, &mut v), sb::kResultOk);
            assert_eq!((o, v), (0, 0.5));
            assert_ne!(q.getPoint(1, &mut o, &mut v), sb::kResultOk);
            assert_ne!(q.getPoint(0, std::ptr::null_mut(), &mut v), sb::kResultOk);
        }
        let el = EventList::new(1);
        let mut e = NoteEvent::On { time: 5, channel: 0, key: 60, velocity: 1.0 }.to_raw(2);
        // SAFETY: as above.
        unsafe {
            assert_eq!(el.addEvent(&mut e), sb::kResultOk);
            assert_ne!(el.addEvent(&mut e), sb::kResultOk, "bounded");
            assert_eq!(el.getEventCount(), 1);
            let mut out: sv::Event = std::mem::zeroed();
            assert_eq!(el.getEvent(0, &mut out), sb::kResultOk);
            assert_eq!(out.sampleOffset, 3);
            assert_eq!(out.__field0.noteOn.pitch, 60);
            assert_ne!(el.getEvent(1, &mut out), sb::kResultOk);
        }
        let s = MemoryStream { inner: Mutex::new((Vec::new(), 0)) };
        let mut buf = *b"hello";
        // SAFETY: as above; buffers are valid for the given sizes.
        unsafe {
            let mut n = 0;
            assert_eq!(s.write(buf.as_mut_ptr().cast(), 5, &mut n), sb::kResultOk);
            assert_eq!(n, 5);
            let mut pos = 0i64;
            assert_eq!(s.seek(1, sb::IBStream_::IStreamSeekMode_::kIBSeekSet as i32, &mut pos), sb::kResultOk);
            let mut out = [0u8; 8];
            assert_eq!(s.read(out.as_mut_ptr().cast(), 8, &mut n), sb::kResultOk);
            assert_eq!((n, &out[..4]), (4, &b"ello"[..]));
            assert_eq!(s.seek(-100, sb::IBStream_::IStreamSeekMode_::kIBSeekEnd as i32, &mut pos), sb::kResultOk);
            assert_eq!(pos, 0);
            assert_eq!(s.write(std::ptr::null_mut(), 5, &mut n), sb::kResultFalse);
        }
        let h = HostContext::default();
        let mut name: sv::String128 = [0; 128];
        // SAFETY: as above.
        unsafe {
            assert_eq!(h.getName(&mut name), sb::kResultOk);
            assert_eq!(h.restartComponent(sv::RestartFlags_::kLatencyChanged as i32), sb::kResultOk);
        }
        assert_eq!(read_c16(&name), "FreeDAWm");
        assert!(h.latency_changed.load(Ordering::Relaxed));
        assert_eq!(h.restart_flags.swap(0, Ordering::Relaxed), sv::RestartFlags_::kLatencyChanged as u32);
        assert_eq!(hex(&[0xAB; 16]), "AB".repeat(16));
    }

    /// `createInstance` for `cid`/`iid`, as a plugin asks for it.
    fn host_create(h: &HostContext, cid: [u8; 16], iid: [u8; 16]) -> Option<*mut c_void> {
        let mut obj: *mut c_void = std::ptr::null_mut();
        let (mut c, mut i): (sb::TUID, sb::TUID) = ([0; 16], [0; 16]);
        for (d, s) in c.iter_mut().zip(cid) {
            *d = s as c_char;
        }
        for (d, s) in i.iter_mut().zip(iid) {
            *d = s as c_char;
        }
        // SAFETY: test call through our own implementation with valid TUIDs and an out-pointer.
        let r = unsafe { h.createInstance(&mut c, &mut i, &mut obj) };
        (r == sb::kResultOk && !obj.is_null()).then_some(obj)
    }

    #[test]
    fn the_host_makes_messages_and_attribute_lists_with_typed_values() {
        let h = HostContext::default();
        assert!(host_create(&h, [1; 16], [1; 16]).is_none(), "unknown classes are refused");
        assert!(host_create(&h, sv::IMessage::IID, sv::IAttributeList::IID).is_none(), "the iid must match");
        let raw = host_create(&h, sv::IMessage::IID, sv::IMessage::IID).expect("an IMessage");
        // SAFETY: a counted reference to the message we just made.
        let msg = unsafe { ComPtr::from_raw(raw.cast::<sv::IMessage>()) }.expect("non-null");
        let list_raw = host_create(&h, sv::IAttributeList::IID, FUnknown::IID).expect("an IAttributeList through FUnknown");
        // SAFETY: as above.
        let _list = unsafe { ComPtr::from_raw(list_raw.cast::<sv::IAttributeList>()) };
        // SAFETY: test calls through our own implementation; every pointer is valid for the call.
        unsafe {
            assert_eq!(std::ffi::CStr::from_ptr(msg.getMessageID()).to_bytes(), b"", "no id yet");
            msg.setMessageID(c"gain".as_ptr());
            assert_eq!(std::ffi::CStr::from_ptr(msg.getMessageID()).to_bytes(), b"gain");
            let a = vst3::ComRef::from_raw(msg.getAttributes()).expect("attributes");
            assert_eq!(a.setInt(c"n".as_ptr(), 7), sb::kResultOk);
            assert_eq!(a.setFloat(c"x".as_ptr(), 0.25), sb::kResultOk);
            let text: Vec<u16> = "héllo".encode_utf16().chain(Some(0)).collect();
            assert_eq!(a.setString(c"s".as_ptr(), text.as_ptr()), sb::kResultOk);
            assert_eq!(a.setBinary(c"b".as_ptr(), b"abc".as_ptr().cast(), 3), sb::kResultOk);
            let (mut n, mut x) = (0i64, 0.0f64);
            assert_eq!(a.getInt(c"n".as_ptr(), &mut n), sb::kResultOk);
            assert_eq!(a.getFloat(c"x".as_ptr(), &mut x), sb::kResultOk);
            assert_eq!((n, x), (7, 0.25));
            assert_eq!(a.getFloat(c"n".as_ptr(), &mut x), sb::kResultFalse, "an int is not a float");
            assert_eq!(a.getInt(c"missing".as_ptr(), &mut n), sb::kResultFalse);
            assert_eq!(a.getInt(c"n".as_ptr(), std::ptr::null_mut()), sb::kInvalidArgument);
            let mut s = [0xFFu16; 16];
            assert_eq!(a.getString(c"s".as_ptr(), s.as_mut_ptr(), 32), sb::kResultOk);
            assert_eq!(read_c16(&s), "héllo");
            let mut short = [0xFFu16; 3];
            assert_eq!(a.getString(c"s".as_ptr(), short.as_mut_ptr(), 6), sb::kResultOk);
            assert_eq!(short, [u16::from(b'h'), 0xE9, 0], "truncated, NUL-terminated");
            let (mut data, mut size): (*const c_void, u32) = (std::ptr::null(), 0);
            assert_eq!(a.getBinary(c"b".as_ptr(), &mut data, &mut size), sb::kResultOk);
            assert_eq!(std::slice::from_raw_parts(data.cast::<u8>(), size as usize), b"abc");
            assert_eq!(a.setInt(c"n".as_ptr(), 8), sb::kResultOk, "replaced in place");
            assert_eq!(a.getInt(c"n".as_ptr(), &mut n), sb::kResultOk);
            assert_eq!(n, 8);
            assert_eq!(a.setInt(std::ptr::null(), 1), sb::kInvalidArgument);
        }
    }

    /// A connection point standing in for an edit controller: counts the messages it gets and
    /// the thread they came on.
    struct Sink {
        got: AtomicUsize,
        off_main: AtomicUsize,
        last: AtomicU64,
    }

    impl Class for Sink {
        type Interfaces = (sv::IConnectionPoint,);
    }

    impl IConnectionPointTrait for Sink {
        unsafe fn connect(&self, _other: *mut sv::IConnectionPoint) -> sb::tresult {
            sb::kResultOk
        }
        unsafe fn disconnect(&self, _other: *mut sv::IConnectionPoint) -> sb::tresult {
            sb::kResultOk
        }
        unsafe fn notify(&self, message: *mut sv::IMessage) -> sb::tresult {
            self.got.fetch_add(1, Ordering::Relaxed);
            if !on_main_thread() {
                self.off_main.fetch_add(1, Ordering::Relaxed);
            }
            // SAFETY: the message passed to us, valid for the call.
            if let Some(m) = unsafe { vst3::ComRef::from_raw(message) } {
                let mut v = 0.0f64;
                // SAFETY: our own message's attributes.
                if let Some(a) = unsafe { vst3::ComRef::from_raw(m.getAttributes()) }
                    && unsafe { a.getFloat(c"value".as_ptr(), &mut v) } == sb::kResultOk
                {
                    self.last.store(v.to_bits(), Ordering::Relaxed);
                }
            }
            sb::kResultOk
        }
    }

    fn message_with(value: f64) -> ComPtr<sv::IMessage> {
        let m = ComWrapper::new(HostMessage::default()).to_com_ptr::<sv::IMessage>().expect("message");
        // SAFETY: our own message.
        unsafe {
            m.setMessageID(c"gain".as_ptr());
            vst3::ComRef::from_raw(m.getAttributes()).expect("attributes").setFloat(c"value".as_ptr(), value);
        }
        m
    }

    #[test]
    fn a_processor_message_from_another_thread_waits_for_the_ui_thread() {
        let sink = ComWrapper::new(Sink { got: AtomicUsize::new(0), off_main: AtomicUsize::new(0), last: AtomicU64::new(0) });
        let target = sink.to_com_ptr::<sv::IConnectionPoint>().expect("sink");
        let proxy = ComWrapper::new(ConnectionProxy::new("processor", target));
        let point = proxy.to_com_ptr::<sv::IConnectionPoint>().expect("proxy");
        // The test thread is not the main thread: like the audio thread, its notify is queued.
        let m = message_with(0.5);
        // SAFETY: test call through our own implementation.
        assert_eq!(unsafe { point.notify(m.as_ptr()) }, sb::kResultOk);
        assert_eq!(sink.got.load(Ordering::Relaxed), 0, "not delivered on the sending thread");
        let p2 = point.clone();
        std::thread::Builder::new()
            .name("fake-audio".into())
            .spawn(move || {
                let m = message_with(0.75);
                // SAFETY: as above, from a worker thread.
                assert_eq!(unsafe { p2.notify(m.as_ptr()) }, sb::kResultOk);
            })
            .expect("spawn")
            .join()
            .expect("join");
        assert_eq!(sink.got.load(Ordering::Relaxed), 0);
        proxy.deliver();
        assert_eq!(sink.got.load(Ordering::Relaxed), 2, "both delivered by the UI thread's idle, in order");
        assert_eq!(f64::from_bits(sink.last.load(Ordering::Relaxed)), 0.75);
        // A full queue drops (and counts) rather than grows or waits.
        for _ in 0..MESSAGE_QUEUE_CAP + 5 {
            // SAFETY: as above.
            unsafe { point.notify(m.as_ptr()) };
        }
        assert_eq!(proxy.dropped.load(Ordering::Relaxed), 5);
        proxy.deliver();
        assert_eq!(sink.got.load(Ordering::Relaxed), 2 + MESSAGE_QUEUE_CAP);
        // Detached: nothing reaches a terminated target.
        proxy.detach();
        // SAFETY: as above.
        unsafe { point.notify(m.as_ptr()) };
        proxy.deliver();
        assert_eq!(sink.got.load(Ordering::Relaxed), 2 + MESSAGE_QUEUE_CAP);
        // SAFETY: as above; a null message is refused.
        assert_eq!(unsafe { point.notify(std::ptr::null_mut()) }, sb::kInvalidArgument);
    }
}

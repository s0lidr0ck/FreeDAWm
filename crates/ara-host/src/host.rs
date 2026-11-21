//! The host side of ARA: a plug-in's factory, and one ARA document (a document controller and
//! the host interfaces it calls back into). The only module of the host side with `unsafe`; every
//! block says why it is sound, and every callback the plug-in calls is guarded against panics and
//! against refs it never got.
//!
//! Host refs. Every host ref handed to the plug-in (controllers, audio sources, modifications,
//! regions, readers, content readers) is a small number cast to a pointer, never the address of
//! host memory: a callback looks it up in a table and refuses what it does not find, so a plug-in
//! that passes a wrong or stale ref gets `false`, null or nothing instead of making the host read
//! memory it does not own.

use crate::sys::*;
use crate::{AnalysisState, AraError, AudioSource, FactoryInfo, Graph, HostEvent, Note, PlaybackRequest, ReadStats, SyncReport};
use std::collections::{BTreeMap, HashMap, VecDeque};
use std::ffi::{CString, c_char, c_void};
use std::marker::PhantomData;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, PoisonError};

/// Longest string read from a plug-in (names, ids, versions).
const MAX_STRING: usize = 1024;
/// Most content types or archive ids read from a factory.
const MAX_LIST: usize = 64;
/// Most samples per channel one `readAudioSamples` call may ask for.
const MAX_READ: i64 = 1 << 26;
/// Samples per channel read from an [`AudioSource`] at a time.
const CHUNK: usize = 1 << 14;
/// Most events read from one plug-in content reader.
const MAX_EVENTS: i32 = 1 << 20;
/// Most host events kept until [`Document::take_events`].
const MAX_HOST_EVENTS: usize = 4096;
/// Most modifications kept deactivated for the undo history before the oldest is destroyed.
const MAX_GRAVEYARD: usize = 512;
/// Largest archive a plug-in may write (bytes).
const MAX_ARCHIVE: usize = 1 << 30;

/// The API generations this host implements, oldest first: 2.0 final (every ARA 2 plug-in, and
/// the only one on ARM) and 2.3 final (the document-data change notification).
pub const HOST_GENERATIONS: [ARAAPIGeneration; 2] = [kARAAPIGeneration_2_0_Final, kARAAPIGeneration_2_3_Final];

/// Runs a host callback, turning a panic into `default` (nothing may unwind into plug-in code).
fn guard<R>(default: R, f: impl FnOnce() -> R) -> R {
    match catch_unwind(AssertUnwindSafe(f)) {
        Ok(r) => r,
        Err(_) => {
            crate::ara_log!("host callback panicked; answered with the default");
            default
        }
    }
}

fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Reads a NUL-terminated string the plug-in gave, at most [`MAX_STRING`] bytes, lossily.
///
/// # Safety
/// `p` is null or points to readable bytes up to a NUL or for [`MAX_STRING`] bytes.
unsafe fn read_cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    let mut bytes = Vec::new();
    for i in 0..MAX_STRING {
        // SAFETY: the caller promises the bytes up to the NUL (or MAX_STRING) are readable; we
        // stop at the first NUL.
        let b = unsafe { p.add(i).read() } as u8;
        if b == 0 {
            break;
        }
        bytes.push(b);
    }
    String::from_utf8_lossy(&bytes).into_owned()
}

/// A C string for the plug-in (properties are only read during the call): interior NULs dropped.
fn c_string(s: &str) -> CString {
    CString::new(s.replace('\0', "")).unwrap_or_default()
}

/// Copies a versioned struct the other side gave into a zeroed `T`: only the `structSize` bytes
/// it declares (never more than `T`), so members it does not have read as zero/`None`. `None`
/// for a null pointer or a struct smaller than `min`.
///
/// # Safety
/// `p` is null or points to a struct whose first field is its `structSize` and whose first
/// `structSize` bytes are readable; `T` is one of the `sys` structs, for which all-zero is valid.
unsafe fn copy_versioned<T: Copy>(p: *const T, min: usize) -> Option<(T, usize)> {
    if p.is_null() {
        return None;
    }
    // SAFETY: every versioned struct starts with `ARASize structSize` (readable per the contract);
    // read unaligned since x86 structs are packed.
    let size = unsafe { p.cast::<ARASize>().read_unaligned() };
    if size < min {
        return None;
    }
    // SAFETY: all-zero is a valid `T` (pointers null, integers 0, function pointers `None`).
    let mut out: T = unsafe { std::mem::zeroed() };
    let n = size.min(std::mem::size_of::<T>());
    // SAFETY: `n` bytes are readable at `p` (at most the declared size) and writable in `out`;
    // the two do not overlap.
    unsafe { std::ptr::copy_nonoverlapping(p.cast::<u8>(), (&raw mut out).cast::<u8>(), n) };
    Some((out, size))
}

// ---- factory --------------------------------------------------------------------------------

/// A plug-in's static ARA factory. Copy it freely; it is only a pointer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FactoryPtr(NonNull<ARAFactory>);

// SAFETY: the factory is static data and functions of the plug-in binary, valid from any thread
// (the threads its functions may be called on are the caller's business, see the crate docs).
unsafe impl Send for FactoryPtr {}
// SAFETY: as above; nothing here is mutated through the pointer.
unsafe impl Sync for FactoryPtr {}

impl FactoryPtr {
    /// Wraps a factory pointer (`None` for null).
    ///
    /// # Safety
    /// `p` is null, or points to an `ARAFactory` that, with everything it points to, stays valid
    /// and unchanged as long as the plug-in binary is loaded, and that binary stays loaded while
    /// the returned value or any [`Document`] made from it exists (FreeDAWm never unloads plug-in
    /// binaries). Its first `structSize` bytes must be readable.
    pub unsafe fn new(p: *const ARAFactory) -> Option<FactoryPtr> {
        NonNull::new(p.cast_mut()).map(FactoryPtr)
    }

    /// The address (identifies the plug-in binary's factory).
    pub fn addr(&self) -> usize {
        self.0.as_ptr() as usize
    }

    /// The factory, copied as far as its `structSize` reaches (`None` when too small).
    fn copy(&self) -> Option<(ARAFactory, usize)> {
        // SAFETY: valid for reading per `new`'s contract.
        unsafe { copy_versioned(self.0.as_ptr().cast_const(), kARAFactoryMinSize) }
    }
}

/// The generation this host uses with a plug-in supporting `lowest..=highest`: the newest of
/// [`HOST_GENERATIONS`] in that range. `None` when none is (an ARA 1 plug-in, or a range that only
/// holds the 2.x development draft).
pub fn choose_generation(lowest: ARAAPIGeneration, highest: ARAAPIGeneration) -> Option<ARAAPIGeneration> {
    if lowest > highest {
        return None;
    }
    HOST_GENERATIONS.iter().rev().copied().find(|g| (lowest..=highest).contains(g))
}

/// Reads and checks a factory: its size, the generations, the functions every host needs. The
/// strings and lists are copied (capped).
pub fn read_factory(f: FactoryPtr) -> Result<FactoryInfo, AraError> {
    if cfg!(not(target_pointer_width = "64")) {
        return Err(AraError::Unsupported);
    }
    let Some((fac, size)) = f.copy() else {
        return Err(AraError::Factory("its ARAFactory is smaller than the ARA 1 minimum".into()));
    };
    let (lowest, highest) = (fac.lowestSupportedApiGeneration, fac.highestSupportedApiGeneration);
    for (name, missing) in [
        ("initializeARAWithConfiguration", { fac.initializeARAWithConfiguration }.is_none()),
        ("uninitializeARA", { fac.uninitializeARA }.is_none()),
        ("createDocumentControllerWithDocument", { fac.createDocumentControllerWithDocument }.is_none()),
    ] {
        if missing {
            return Err(AraError::Factory(format!("its factory has no {name}")));
        }
    }
    let n_types = { fac.analyzeableContentTypesCount }.min(MAX_LIST);
    let types_ptr = { fac.analyzeableContentTypes };
    let content_types = if types_ptr.is_null() {
        Vec::new()
    } else {
        // SAFETY: the factory promises `analyzeableContentTypesCount` entries there (static
        // data); we read at most MAX_LIST of them, unaligned.
        (0..n_types).map(|i| unsafe { types_ptr.add(i).read_unaligned() }).collect()
    };
    let n_ids = { fac.compatibleDocumentArchiveIDsCount }.min(MAX_LIST);
    let ids_ptr = { fac.compatibleDocumentArchiveIDs };
    let compatible = if ids_ptr.is_null() {
        Vec::new()
    } else {
        // SAFETY: as above, `compatibleDocumentArchiveIDsCount` string pointers, each null or a
        // static NUL-terminated string.
        (0..n_ids).map(|i| unsafe { read_cstr(ids_ptr.add(i).read_unaligned()) }).collect()
    };
    let chunks = implements(size, std::mem::offset_of!(ARAFactory, supportsStoringAudioFileChunks)) && { fac.supportsStoringAudioFileChunks } != 0;
    // SAFETY (all `read_cstr` below): the factory's strings are null or static NUL-terminated
    // strings of the loaded binary.
    let info = unsafe {
        FactoryInfo {
            name: read_cstr(fac.plugInName),
            manufacturer: read_cstr(fac.manufacturerName),
            url: read_cstr(fac.informationURL),
            version: read_cstr(fac.version),
            factory_id: read_cstr(fac.factoryID),
            document_archive_id: read_cstr(fac.documentArchiveID),
            compatible_archive_ids: compatible,
            lowest_generation: lowest,
            highest_generation: highest,
            generation: choose_generation(lowest, highest),
            content_types,
            transformations: fac.supportedPlaybackTransformationFlags,
            audio_file_chunks: chunks,
        }
    };
    Ok(info)
}

/// Factories initialised (`initializeARAWithConfiguration`) and how many documents use each, by
/// address: the header allows one initialisation per binary at a time; the last document gone
/// uninitialises it.
fn initialised() -> &'static Mutex<Vec<(usize, usize)>> {
    static I: Mutex<Vec<(usize, usize)>> = Mutex::new(Vec::new());
    &I
}

/// The assert function slot handed to plug-ins (`ARAInterfaceConfiguration::assertFunctionAddress`):
/// one for every generation, valid for the life of the process. A plug-in may write its own
/// function into it (the header allows that), so it lives in an `UnsafeCell`.
struct AssertSlot(std::cell::UnsafeCell<ARAAssertFunction>);
// SAFETY: the slot is only written by plug-ins (as the ARA contract allows) and only read by the
// plug-ins that call through it; the host itself never reads or writes it after start-up.
unsafe impl Sync for AssertSlot {}
static ASSERT_SLOT: AssertSlot = AssertSlot(std::cell::UnsafeCell::new(Some(host_assert)));

unsafe extern "C" fn host_assert(category: ARAAssertCategory, argument: *const c_void, diagnosis: *const c_char) {
    guard((), || {
        // SAFETY: the plug-in passes null or a NUL-terminated diagnosis.
        let text = unsafe { read_cstr(diagnosis) };
        let kind = match category {
            kARAAssertInvalidArgument => "invalid argument",
            kARAAssertInvalidState => "invalid state",
            kARAAssertInvalidThread => "invalid thread",
            _ => "unspecified",
        };
        crate::ara_log!("ARA assert ({kind}, argument {argument:p}): {text}");
    })
}

fn initialise(f: &FactoryPtr, fac: &ARAFactory, generation: ARAAPIGeneration) {
    let mut inits = lock(initialised());
    if let Some(e) = inits.iter_mut().find(|(a, _)| *a == f.addr()) {
        e.1 += 1;
        return;
    }
    let config = ARAInterfaceConfiguration {
        structSize: std::mem::size_of::<ARAInterfaceConfiguration>(),
        desiredApiGeneration: generation,
        assertFunctionAddress: ASSERT_SLOT.0.get(),
    };
    if let Some(init) = { fac.initializeARAWithConfiguration } {
        // SAFETY: a valid factory function; `config` lives for the call (the plug-in copies it);
        // the assert slot lives for the process.
        unsafe { init(&config) };
    }
    inits.push((f.addr(), 1));
}

fn uninitialise(f: &FactoryPtr, fac: &ARAFactory) {
    let mut inits = lock(initialised());
    let Some(i) = inits.iter().position(|(a, _)| *a == f.addr()) else { return };
    let left = inits.get(i).map_or(0, |e| e.1.saturating_sub(1));
    if left > 0 {
        if let Some(e) = inits.get_mut(i) {
            e.1 = left;
        }
        return;
    }
    inits.remove(i);
    if let Some(un) = { fac.uninitializeARA } {
        // SAFETY: balanced with the initialisation above; no document of this factory is left.
        unsafe { un() };
    }
}

// ---- shared host state (reached from the plug-in's threads) -------------------------------

/// One audio reader the plug-in made.
struct Reader {
    source: Arc<dyn AudioSource>,
    use64: bool,
    /// Held while a read runs (one thread per reader at a time, per the ARA contract); also the
    /// scratch buffer.
    scratch: Mutex<Vec<f32>>,
}

/// A host content reader's events, laid out as the ARA structs the plug-in reads.
enum ContentEvents {
    Tempo(Box<[ARAContentTempoEntry]>),
    Bars(Box<[ARAContentBarSignature]>),
}

/// What the host callbacks need, shared with the plug-in's threads. Never locked while the host
/// calls into the plug-in.
struct Shared {
    /// Audio sources by host ref, with whether reading is enabled.
    sources: Mutex<HashMap<usize, (Arc<dyn AudioSource>, bool)>>,
    readers: Mutex<HashMap<usize, Arc<Reader>>>,
    content_readers: Mutex<HashMap<usize, ContentEvents>>,
    tempo: Mutex<Vec<ARAContentTempoEntry>>,
    bars: Mutex<Vec<ARAContentBarSignature>>,
    events: Mutex<VecDeque<RawEvent>>,
    /// Archives being read (`restoreObjectsFromArchive`), by reader host ref: the bytes and the
    /// document archive id they were written with.
    archive_in: Mutex<HashMap<usize, (Arc<[u8]>, CString)>>,
    /// Archives being written (`storeObjectsToArchive`), by writer host ref.
    archive_out: Mutex<HashMap<usize, Vec<u8>>>,
    next: AtomicUsize,
    samples_read: AtomicU64,
    reads: AtomicU64,
    failed_reads: AtomicU64,
}

/// A host callback the plug-in made, with the host refs it named.
#[derive(Debug, Clone, Copy)]
enum RawEvent {
    Progress(usize, ARAAnalysisProgressState, f32),
    SourceContent(usize),
    ModificationContent(usize),
    RegionContent(usize),
    DocumentData,
    Playback(PlaybackRequest),
}

impl Shared {
    fn new() -> Shared {
        Shared {
            sources: Mutex::new(HashMap::new()),
            readers: Mutex::new(HashMap::new()),
            content_readers: Mutex::new(HashMap::new()),
            tempo: Mutex::new(Vec::new()),
            bars: Mutex::new(Vec::new()),
            events: Mutex::new(VecDeque::new()),
            archive_in: Mutex::new(HashMap::new()),
            archive_out: Mutex::new(HashMap::new()),
            next: AtomicUsize::new(1),
            samples_read: AtomicU64::new(0),
            reads: AtomicU64::new(0),
            failed_reads: AtomicU64::new(0),
        }
    }
    fn new_ref(&self) -> usize {
        self.next.fetch_add(1, Ordering::Relaxed)
    }
    fn push(&self, e: RawEvent) {
        let mut q = lock(&self.events);
        if q.len() >= MAX_HOST_EVENTS {
            q.pop_front();
        }
        q.push_back(e);
    }
}

/// Live documents by controller host ref (one number serves as every controller's host ref).
fn documents() -> &'static Mutex<HashMap<usize, Arc<Shared>>> {
    static D: std::sync::OnceLock<Mutex<HashMap<usize, Arc<Shared>>>> = std::sync::OnceLock::new();
    D.get_or_init(|| Mutex::new(HashMap::new()))
}

static NEXT_DOCUMENT: AtomicUsize = AtomicUsize::new(1);

fn shared_of(controller: *mut c_void) -> Option<Arc<Shared>> {
    let found = lock(documents()).get(&(controller as usize)).cloned();
    if found.is_none() {
        crate::ara_log!("plug-in called the host with an unknown controller ref {controller:p}");
    }
    found
}

fn as_ref(n: usize) -> *mut c_void {
    n as *mut c_void
}

/// Fills `out` with the audio of `source`'s channel `ch` from frame `pos` (may be negative or past
/// the end: those frames are silence). `false` when the source failed to read.
pub(crate) fn read_window(source: &dyn AudioSource, ch: usize, pos: i64, out: &mut [f32]) -> bool {
    out.iter_mut().for_each(|x| *x = 0.0);
    let frames = i64::try_from(source.frames()).unwrap_or(i64::MAX);
    let n = i64::try_from(out.len()).unwrap_or(i64::MAX);
    let (from, to) = (pos.max(0), pos.saturating_add(n).min(frames));
    if from >= to {
        return true;
    }
    let skip = usize::try_from(from - pos).unwrap_or(0);
    let len = usize::try_from(to - from).unwrap_or(0);
    match out.get_mut(skip..skip + len) {
        Some(dst) => source.read(ch, u64::try_from(from).unwrap_or(0), dst),
        None => false,
    }
}

// ---- host interface callbacks ---------------------------------------------------------------

unsafe extern "C" fn create_audio_reader(
    ctrl: ARAAudioAccessControllerHostRef,
    source: ARAAudioSourceHostRef,
    use64: ARABool,
) -> ARAAudioReaderHostRef {
    guard(std::ptr::null_mut(), || {
        let Some(sh) = shared_of(ctrl) else { return std::ptr::null_mut() };
        let found = lock(&sh.sources).get(&(source as usize)).cloned();
        let Some((audio, enabled)) = found else {
            crate::ara_log!("createAudioReaderForSource: unknown audio source ref {source:p}");
            return std::ptr::null_mut();
        };
        if !enabled {
            crate::ara_log!("createAudioReaderForSource: reading is not enabled for audio source ref {source:p}");
            return std::ptr::null_mut();
        }
        let id = sh.new_ref();
        lock(&sh.readers).insert(id, Arc::new(Reader { source: audio, use64: use64 != kARAFalse, scratch: Mutex::new(Vec::new()) }));
        as_ref(id)
    })
}

unsafe extern "C" fn read_audio_samples(
    ctrl: ARAAudioAccessControllerHostRef,
    reader: ARAAudioReaderHostRef,
    position: ARASamplePosition,
    count: ARASampleCount,
    buffers: *const *mut c_void,
) -> ARABool {
    guard(kARAFalse, || {
        let Some(sh) = shared_of(ctrl) else { return kARAFalse };
        let found = lock(&sh.readers).get(&(reader as usize)).cloned();
        let Some(r) = found else {
            crate::ara_log!("readAudioSamples: unknown audio reader ref {reader:p}");
            sh.failed_reads.fetch_add(1, Ordering::Relaxed);
            return kARAFalse;
        };
        let channels = r.source.channels();
        if buffers.is_null() || !(0..=MAX_READ).contains(&count) {
            crate::ara_log!("readAudioSamples: refused (buffers {buffers:p}, {count} samples)");
            sh.failed_reads.fetch_add(1, Ordering::Relaxed);
            return kARAFalse;
        }
        // SAFETY: ARA passes one buffer pointer per channel of the audio source (its channel count,
        // which the reader's source reports); the array is readable for the call.
        let ptrs: Vec<*mut c_void> = (0..channels).map(|c| unsafe { buffers.add(c).read() }).collect();
        if ptrs.iter().any(|p| p.is_null()) {
            crate::ara_log!("readAudioSamples: a channel buffer is null");
            sh.failed_reads.fetch_add(1, Ordering::Relaxed);
            return kARAFalse;
        }
        let n = usize::try_from(count).unwrap_or(0);
        let write = |p: *mut c_void, at: usize, values: &[f32]| {
            for (i, v) in values.iter().enumerate() {
                if r.use64 {
                    // SAFETY: the caller's channel buffer holds `count` doubles (ARA contract);
                    // `at + i < count`. Written unaligned so a misaligned buffer is no UB.
                    unsafe { p.cast::<f64>().add(at + i).write_unaligned(f64::from(*v)) };
                } else {
                    // SAFETY: as above, `count` floats.
                    unsafe { p.cast::<f32>().add(at + i).write_unaligned(*v) };
                }
            }
        };
        let Ok(mut scratch) = r.scratch.try_lock() else {
            crate::ara_log!("readAudioSamples: one reader used from two threads at once; answered with silence");
            for p in &ptrs {
                for at in (0..n).step_by(CHUNK) {
                    write(*p, at, &vec![0.0; CHUNK.min(n - at)]);
                }
            }
            sh.failed_reads.fetch_add(1, Ordering::Relaxed);
            return kARAFalse;
        };
        let mut ok = true;
        for (c, p) in ptrs.iter().enumerate() {
            let mut at = 0;
            while at < n {
                let len = CHUNK.min(n - at);
                scratch.resize(len, 0.0);
                let pos = position.saturating_add(i64::try_from(at).unwrap_or(i64::MAX));
                if !read_window(r.source.as_ref(), c, pos, &mut scratch) {
                    ok = false;
                    scratch.iter_mut().for_each(|x| *x = 0.0);
                }
                write(*p, at, &scratch);
                at += len;
            }
        }
        sh.reads.fetch_add(1, Ordering::Relaxed);
        sh.samples_read.fetch_add(u64::try_from(n).unwrap_or(0).saturating_mul(channels as u64), Ordering::Relaxed);
        if ok {
            kARATrue
        } else {
            sh.failed_reads.fetch_add(1, Ordering::Relaxed);
            crate::ara_log!("readAudioSamples: the audio could not be read; silence given");
            kARAFalse
        }
    })
}

unsafe extern "C" fn destroy_audio_reader(ctrl: ARAAudioAccessControllerHostRef, reader: ARAAudioReaderHostRef) {
    guard((), || {
        let Some(sh) = shared_of(ctrl) else { return };
        if lock(&sh.readers).remove(&(reader as usize)).is_none() {
            crate::ara_log!("destroyAudioReader: unknown audio reader ref {reader:p}");
        }
    })
}

static AUDIO_ACCESS: ARAAudioAccessControllerInterface = ARAAudioAccessControllerInterface {
    structSize: std::mem::size_of::<ARAAudioAccessControllerInterface>(),
    createAudioReaderForSource: Some(create_audio_reader),
    readAudioSamples: Some(read_audio_samples),
    destroyAudioReader: Some(destroy_audio_reader),
};

// Archiving: an archive is open only while the host stores or restores a document; every call
// names it by its reader or writer host ref, checked against the open ones.
unsafe extern "C" fn get_archive_size(c: ARAArchivingControllerHostRef, r: ARAArchiveReaderHostRef) -> ARASize {
    guard(0, || {
        let Some(sh) = shared_of(c) else { return 0 };
        match lock(&sh.archive_in).get(&(r as usize)) {
            Some((data, _)) => data.len(),
            None => {
                crate::ara_log!("getArchiveSize: unknown archive reader ref {r:p}");
                0
            }
        }
    })
}
unsafe extern "C" fn read_archive(
    c: ARAArchivingControllerHostRef,
    r: ARAArchiveReaderHostRef,
    pos: ARASize,
    len: ARASize,
    buf: *mut ARAByte,
) -> ARABool {
    guard(kARAFalse, || {
        let Some(sh) = shared_of(c) else { return kARAFalse };
        let data = lock(&sh.archive_in).get(&(r as usize)).map(|(d, _)| d.clone());
        let Some(data) = data else {
            crate::ara_log!("readBytesFromArchive: unknown archive reader ref {r:p}");
            return kARAFalse;
        };
        let Some(src) = pos.checked_add(len).and_then(|end| data.get(pos..end)) else {
            crate::ara_log!("readBytesFromArchive: {len} bytes at {pos} is past the archive's {} bytes", data.len());
            return kARAFalse;
        };
        if buf.is_null() {
            crate::ara_log!("readBytesFromArchive: null buffer");
            return kARAFalse;
        }
        // SAFETY: the plug-in's buffer holds `len` bytes (ARA contract); `src` is `len` bytes of
        // our own archive; they cannot overlap.
        unsafe { std::ptr::copy_nonoverlapping(src.as_ptr(), buf, src.len()) };
        kARATrue
    })
}
unsafe extern "C" fn write_archive(
    c: ARAArchivingControllerHostRef,
    w: ARAArchiveWriterHostRef,
    pos: ARASize,
    len: ARASize,
    buf: *const ARAByte,
) -> ARABool {
    guard(kARAFalse, || {
        let Some(sh) = shared_of(c) else { return kARAFalse };
        let mut out = lock(&sh.archive_out);
        let Some(data) = out.get_mut(&(w as usize)) else {
            crate::ara_log!("writeBytesToArchive: unknown archive writer ref {w:p}");
            return kARAFalse;
        };
        let Some(end) = pos.checked_add(len).filter(|e| *e <= MAX_ARCHIVE) else {
            crate::ara_log!("writeBytesToArchive: {len} bytes at {pos} is more than an archive may hold");
            return kARAFalse;
        };
        if len == 0 {
            return kARATrue;
        }
        if buf.is_null() {
            crate::ara_log!("writeBytesToArchive: null buffer");
            return kARAFalse;
        }
        if data.len() < end {
            data.resize(end, 0);
        }
        let Some(dst) = data.get_mut(pos..end) else { return kARAFalse };
        // SAFETY: the plug-in's buffer holds `len` readable bytes (ARA contract); `dst` is `len`
        // bytes of our own vector.
        unsafe { std::ptr::copy_nonoverlapping(buf, dst.as_mut_ptr(), len) };
        kARATrue
    })
}
unsafe extern "C" fn archiving_progress(_c: ARAArchivingControllerHostRef, _v: f32) {}
unsafe extern "C" fn archive_id(c: ARAArchivingControllerHostRef, r: ARAArchiveReaderHostRef) -> ARAPersistentID {
    guard(std::ptr::null(), || {
        let Some(sh) = shared_of(c) else { return std::ptr::null() };
        // The string lives in the table until the restore it belongs to is over, as ARA asks.
        lock(&sh.archive_in).get(&(r as usize)).map_or(std::ptr::null(), |(_, id)| id.as_ptr())
    })
}

static ARCHIVING: ARAArchivingControllerInterface = ARAArchivingControllerInterface {
    structSize: std::mem::size_of::<ARAArchivingControllerInterface>(),
    getArchiveSize: Some(get_archive_size),
    readBytesFromArchive: Some(read_archive),
    writeBytesToArchive: Some(write_archive),
    notifyDocumentArchivingProgress: Some(archiving_progress),
    notifyDocumentUnarchivingProgress: Some(archiving_progress),
    getDocumentArchiveID: Some(archive_id),
};

/// The one musical context's host ref.
const CONTEXT_REF: usize = 1;

fn musical_available(sh: &Shared, ctx: *mut c_void, kind: ARAContentType) -> bool {
    if ctx as usize != CONTEXT_REF {
        return false;
    }
    match kind {
        kARAContentTypeTempoEntries => lock(&sh.tempo).len() >= 2,
        kARAContentTypeBarSignatures => lock(&sh.tempo).len() >= 2 && !lock(&sh.bars).is_empty(),
        _ => false,
    }
}

unsafe extern "C" fn is_musical_available(c: ARAContentAccessControllerHostRef, ctx: ARAMusicalContextHostRef, kind: ARAContentType) -> ARABool {
    guard(kARAFalse, || match shared_of(c) {
        Some(sh) if musical_available(&sh, ctx, kind) => kARATrue,
        _ => kARAFalse,
    })
}
unsafe extern "C" fn musical_grade(c: ARAContentAccessControllerHostRef, ctx: ARAMusicalContextHostRef, kind: ARAContentType) -> ARAContentGrade {
    guard(kARAContentGradeInitial, || match shared_of(c) {
        Some(sh) if musical_available(&sh, ctx, kind) => kARAContentGradeApproved,
        _ => kARAContentGradeInitial,
    })
}
unsafe extern "C" fn musical_reader(
    c: ARAContentAccessControllerHostRef,
    ctx: ARAMusicalContextHostRef,
    kind: ARAContentType,
    _range: *const ARAContentTimeRange,
) -> ARAContentReaderHostRef {
    guard(std::ptr::null_mut(), || {
        let Some(sh) = shared_of(c) else { return std::ptr::null_mut() };
        if !musical_available(&sh, ctx, kind) {
            crate::ara_log!("createMusicalContextContentReader: content type {kind} is not available");
            return std::ptr::null_mut();
        }
        // Every event: the range only says which ones the plug-in needs at least.
        let events = if kind == kARAContentTypeTempoEntries {
            ContentEvents::Tempo(lock(&sh.tempo).clone().into_boxed_slice())
        } else {
            ContentEvents::Bars(lock(&sh.bars).clone().into_boxed_slice())
        };
        let id = sh.new_ref();
        lock(&sh.content_readers).insert(id, events);
        as_ref(id)
    })
}
unsafe extern "C" fn source_available(_c: ARAContentAccessControllerHostRef, _s: ARAAudioSourceHostRef, _k: ARAContentType) -> ARABool {
    kARAFalse
}
unsafe extern "C" fn source_grade(_c: ARAContentAccessControllerHostRef, _s: ARAAudioSourceHostRef, _k: ARAContentType) -> ARAContentGrade {
    kARAContentGradeInitial
}
unsafe extern "C" fn source_reader(
    _c: ARAContentAccessControllerHostRef,
    _s: ARAAudioSourceHostRef,
    _k: ARAContentType,
    _r: *const ARAContentTimeRange,
) -> ARAContentReaderHostRef {
    guard(std::ptr::null_mut(), || {
        crate::ara_log!("createAudioSourceContentReader: the host has no content for audio sources");
        std::ptr::null_mut()
    })
}
unsafe extern "C" fn reader_count(c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef) -> ARAInt32 {
    guard(0, || {
        let Some(sh) = shared_of(c) else { return 0 };
        let n = match lock(&sh.content_readers).get(&(r as usize)) {
            Some(ContentEvents::Tempo(t)) => t.len(),
            Some(ContentEvents::Bars(b)) => b.len(),
            None => {
                crate::ara_log!("getContentReaderEventCount: unknown content reader ref {r:p}");
                0
            }
        };
        i32::try_from(n).unwrap_or(i32::MAX)
    })
}
unsafe extern "C" fn reader_event(c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef, index: ARAInt32) -> *const c_void {
    guard(std::ptr::null(), || {
        let Some(sh) = shared_of(c) else { return std::ptr::null() };
        let Ok(i) = usize::try_from(index) else { return std::ptr::null() };
        // The events live in the table, boxed, until the reader is destroyed: the pointer stays
        // valid that long, as ARA asks.
        match lock(&sh.content_readers).get(&(r as usize)) {
            Some(ContentEvents::Tempo(t)) => t.get(i).map_or(std::ptr::null(), |e| (e as *const ARAContentTempoEntry).cast()),
            Some(ContentEvents::Bars(b)) => b.get(i).map_or(std::ptr::null(), |e| (e as *const ARAContentBarSignature).cast()),
            None => std::ptr::null(),
        }
    })
}
unsafe extern "C" fn destroy_reader(c: ARAContentAccessControllerHostRef, r: ARAContentReaderHostRef) {
    guard((), || {
        if let Some(sh) = shared_of(c)
            && lock(&sh.content_readers).remove(&(r as usize)).is_none()
        {
            crate::ara_log!("destroyContentReader: unknown content reader ref {r:p}");
        }
    })
}

static CONTENT_ACCESS: ARAContentAccessControllerInterface = ARAContentAccessControllerInterface {
    structSize: std::mem::size_of::<ARAContentAccessControllerInterface>(),
    isMusicalContextContentAvailable: Some(is_musical_available),
    getMusicalContextContentGrade: Some(musical_grade),
    createMusicalContextContentReader: Some(musical_reader),
    isAudioSourceContentAvailable: Some(source_available),
    getAudioSourceContentGrade: Some(source_grade),
    createAudioSourceContentReader: Some(source_reader),
    getContentReaderEventCount: Some(reader_count),
    getContentReaderDataForEvent: Some(reader_event),
    destroyContentReader: Some(destroy_reader),
};

fn push_event(c: *mut c_void, e: RawEvent) {
    if let Some(sh) = shared_of(c) {
        sh.push(e);
    }
}

unsafe extern "C" fn analysis_progress(c: ARAModelUpdateControllerHostRef, s: ARAAudioSourceHostRef, state: ARAAnalysisProgressState, value: f32) {
    guard((), || push_event(c, RawEvent::Progress(s as usize, state, value)))
}
unsafe extern "C" fn source_content_changed(
    c: ARAModelUpdateControllerHostRef,
    s: ARAAudioSourceHostRef,
    _r: *const ARAContentTimeRange,
    _f: ARAContentUpdateFlags,
) {
    guard((), || push_event(c, RawEvent::SourceContent(s as usize)))
}
unsafe extern "C" fn modification_content_changed(
    c: ARAModelUpdateControllerHostRef,
    m: ARAAudioModificationHostRef,
    _r: *const ARAContentTimeRange,
    _f: ARAContentUpdateFlags,
) {
    guard((), || push_event(c, RawEvent::ModificationContent(m as usize)))
}
unsafe extern "C" fn region_content_changed(
    c: ARAModelUpdateControllerHostRef,
    p: ARAPlaybackRegionHostRef,
    _r: *const ARAContentTimeRange,
    _f: ARAContentUpdateFlags,
) {
    guard((), || push_event(c, RawEvent::RegionContent(p as usize)))
}
unsafe extern "C" fn document_data_changed(c: ARAModelUpdateControllerHostRef) {
    guard((), || push_event(c, RawEvent::DocumentData))
}

static MODEL_UPDATE: ARAModelUpdateControllerInterface = ARAModelUpdateControllerInterface {
    structSize: std::mem::size_of::<ARAModelUpdateControllerInterface>(),
    notifyAudioSourceAnalysisProgress: Some(analysis_progress),
    notifyAudioSourceContentChanged: Some(source_content_changed),
    notifyAudioModificationContentChanged: Some(modification_content_changed),
    notifyPlaybackRegionContentChanged: Some(region_content_changed),
    notifyDocumentDataChanged: Some(document_data_changed),
};

// Playback control: requests are queued and handed to the host as [`HostEvent::PlaybackRequest`]
// (the app carries them out on its transport); ARA allows a host to ignore any of them.
unsafe extern "C" fn request_start(c: ARAPlaybackControllerHostRef) {
    guard((), || push_event(c, RawEvent::Playback(PlaybackRequest::Start)))
}
unsafe extern "C" fn request_stop(c: ARAPlaybackControllerHostRef) {
    guard((), || push_event(c, RawEvent::Playback(PlaybackRequest::Stop)))
}
unsafe extern "C" fn request_position(c: ARAPlaybackControllerHostRef, t: ARATimePosition) {
    guard((), || {
        if t.is_finite() {
            push_event(c, RawEvent::Playback(PlaybackRequest::Position(t)));
        }
    })
}
unsafe extern "C" fn request_cycle(c: ARAPlaybackControllerHostRef, start: ARATimePosition, duration: ARATimeDuration) {
    guard((), || {
        if start.is_finite() && duration.is_finite() && duration > 0.0 {
            push_event(c, RawEvent::Playback(PlaybackRequest::Cycle(start, duration)));
        }
    })
}
unsafe extern "C" fn request_enable_cycle(c: ARAPlaybackControllerHostRef, on: ARABool) {
    guard((), || push_event(c, RawEvent::Playback(PlaybackRequest::EnableCycle(on != kARAFalse))))
}

static PLAYBACK: ARAPlaybackControllerInterface = ARAPlaybackControllerInterface {
    structSize: std::mem::size_of::<ARAPlaybackControllerInterface>(),
    requestStartPlayback: Some(request_start),
    requestStopPlayback: Some(request_stop),
    requestSetPlaybackPosition: Some(request_position),
    requestSetCycleRange: Some(request_cycle),
    requestEnableCycle: Some(request_enable_cycle),
};

// ---- the document -----------------------------------------------------------------------------

struct Seq {
    aref: ARARegionSequenceRef,
    desc: crate::SequenceDesc,
}

struct Src {
    aref: ARAAudioSourceRef,
    host: usize,
    name: String,
    format: (u64, f64, usize),
    audio: Arc<dyn AudioSource>,
    active: bool,
}

struct Region {
    aref: ARAPlaybackRegionRef,
    host: usize,
    desc: crate::RegionDesc,
}

struct Modification {
    aref: ARAAudioModificationRef,
    host: usize,
    source: u64,
    name: String,
    pid: String,
    active: bool,
    region: Option<Region>,
}

/// One ARA document: a plug-in's document controller for one FreeDAWm session (or anything else
/// a [`Graph`] describes), and the host interfaces it calls.
///
/// Threads: every method calls the document controller, which ARA requires to be called from one
/// thread (the "model thread"; FreeDAWm uses the UI thread). `Document` is neither `Send` nor
/// `Sync`, so the compiler keeps it on the thread that made it. The plug-in may call the audio
/// access controller from any other non-realtime thread; those calls only touch locked tables,
/// never this struct, and are never made from the audio thread.
pub struct Document {
    factory: FactoryPtr,
    fac: ARAFactory,
    info: FactoryInfo,
    generation: ARAAPIGeneration,
    dc: ARADocumentControllerRef,
    iface: ARADocumentControllerInterface,
    key: usize,
    shared: Arc<Shared>,
    /// Must outlive the controller (the plug-in keeps the pointer).
    _host: Box<ARADocumentControllerHostInstance>,
    context: ARAMusicalContextRef,
    name: String,
    tempo_bars: (Vec<(f64, f64)>, Vec<(i32, i32, f64)>),
    seqs: BTreeMap<u64, Seq>,
    srcs: BTreeMap<u64, Src>,
    mods: BTreeMap<u64, Modification>,
    /// Deactivated modifications, oldest first.
    graveyard: VecDeque<u64>,
    progress: HashMap<u64, (AnalysisState, f32)>,
    content_changed: Vec<u64>,
    editing: bool,
    /// An archive to restore in the next editing cycle (the bytes and their document archive id).
    restore: Option<(Arc<[u8]>, String)>,
    /// The archive reader of a restore that uses the deprecated begin/endRestoringDocumentFromArchive
    /// (a plug-in without `restoreObjectsFromArchive`) in place of begin/endEditing.
    old_restore: Option<usize>,
    /// How the last restore went (taken by [`Document::take_restore_result`]).
    restore_result: Option<Result<(), String>>,
    _not_send: PhantomData<*mut ()>,
}

/// Calls a document controller function by name; `None` when the plug-in did not provide it.
macro_rules! dc {
    ($self:ident . $f:ident ( $($a:expr),* $(,)? )) => {
        match { $self.iface.$f } {
            // SAFETY: a function the plug-in's document controller interface provided (non-null,
            // within its declared size), called on the document's thread with the controller ref
            // it gave and arguments that live for the call, as the ARA API documents.
            Some(f) => Some(unsafe { f($self.dc, $($a),*) }),
            None => None,
        }
    };
}

impl Document {
    /// Makes a document controller for a new, empty document named `name`, and its musical
    /// context. Fails, and leaves nothing behind, when the factory or the controller it makes is
    /// unusable.
    pub fn new(factory: FactoryPtr, name: &str) -> Result<Document, AraError> {
        let info = read_factory(factory).inspect_err(|e| crate::ara_log!("no ARA document \"{name}\": {e}"))?;
        let Some(generation) = info.generation else {
            let e = AraError::Generation(info.lowest_generation, info.highest_generation);
            crate::ara_log!("{}: no ARA document: {e}", info.name);
            return Err(e);
        };
        let Some((fac, _)) = factory.copy() else { return Err(AraError::Factory("unreadable".into())) };
        initialise(&factory, &fac, generation);
        let key = NEXT_DOCUMENT.fetch_add(1, Ordering::Relaxed);
        let shared = Arc::new(Shared::new());
        lock(documents()).insert(key, shared.clone());
        let host = Box::new(ARADocumentControllerHostInstance {
            structSize: std::mem::size_of::<ARADocumentControllerHostInstance>(),
            audioAccessControllerHostRef: as_ref(key),
            audioAccessControllerInterface: &AUDIO_ACCESS,
            archivingControllerHostRef: as_ref(key),
            archivingControllerInterface: &ARCHIVING,
            contentAccessControllerHostRef: as_ref(key),
            contentAccessControllerInterface: &CONTENT_ACCESS,
            modelUpdateControllerHostRef: as_ref(key),
            modelUpdateControllerInterface: &MODEL_UPDATE,
            playbackControllerHostRef: as_ref(key),
            playbackControllerInterface: &PLAYBACK,
        });
        let fail = |e: AraError| {
            lock(documents()).remove(&key);
            uninitialise(&factory, &fac);
            crate::ara_log!("{}: no ARA document: {e}", info.name);
            e
        };
        let cname = c_string(name);
        let props = ARADocumentProperties { structSize: std::mem::size_of::<ARADocumentProperties>(), name: cname.as_ptr() };
        let create = { fac.createDocumentControllerWithDocument };
        let Some(create) = create else { return Err(fail(AraError::Factory("no createDocumentController".into()))) };
        // SAFETY: a factory function (checked non-null); the host instance lives as long as the
        // document (boxed, kept in the struct); `props` and its string live for the call.
        let inst = unsafe { create(&*host, &props) };
        // SAFETY: null or the plug-in's instance struct, valid while the controller lives.
        let Some((inst, _)) = (unsafe { copy_versioned(inst, kARADocumentControllerInstanceMinSize) }) else {
            return Err(fail(AraError::Controller("the plug-in made no document controller".into())));
        };
        let dc = inst.documentControllerRef;
        // SAFETY: null or the plug-in's (static) interface struct.
        let Some((iface, _)) = (unsafe { copy_versioned(inst.documentControllerInterface, kARADocumentControllerInterfaceMinSize) }) else {
            // A controller whose interface we cannot use: destroy it if it offers that much.
            // SAFETY: as `copy_versioned`, reading only the first two members of the interface.
            if let Some((small, size)) = unsafe { copy_versioned(inst.documentControllerInterface, 0) }
                && implements(size, std::mem::offset_of!(ARADocumentControllerInterface, destroyDocumentController))
                && let Some(destroy) = { small.destroyDocumentController }
            {
                // SAFETY: the controller's own destroy function, called once on its ref.
                unsafe { destroy(dc) };
            }
            return Err(fail(AraError::Controller("its document controller interface is missing or too small".into())));
        };
        let mut doc = Document {
            factory,
            fac,
            info,
            generation,
            dc,
            iface,
            key,
            shared,
            _host: host,
            context: std::ptr::null_mut(),
            name: name.to_string(),
            tempo_bars: (Vec::new(), Vec::new()),
            seqs: BTreeMap::new(),
            srcs: BTreeMap::new(),
            mods: BTreeMap::new(),
            graveyard: VecDeque::new(),
            progress: HashMap::new(),
            content_changed: Vec::new(),
            editing: false,
            restore: None,
            old_restore: None,
            restore_result: None,
            _not_send: PhantomData,
        };
        if let Some(missing) = doc.missing_function() {
            // `Drop` destroys the controller and uninitialises.
            let e = AraError::Controller(format!("its document controller has no {missing}"));
            crate::ara_log!("{}: no ARA document: {e}", doc.info.name);
            return Err(e);
        }
        doc.begin();
        let cname = c_string(name);
        let props = ARAMusicalContextProperties {
            structSize: std::mem::size_of::<ARAMusicalContextProperties>(),
            name: cname.as_ptr(),
            orderIndex: 0,
            color: std::ptr::null(),
        };
        doc.context = dc!(doc.createMusicalContext(as_ref(CONTEXT_REF), &props)).unwrap_or(std::ptr::null_mut());
        doc.end();
        if doc.context.is_null() {
            let e = AraError::Controller("it made no musical context".into());
            crate::ara_log!("{}: no ARA document: {e}", doc.info.name);
            return Err(e);
        }
        crate::ara_log!(
            "{}: ARA document \"{name}\" made (API generation {generation}, plug-in supports {}..{})",
            doc.info.name,
            doc.info.lowest_generation,
            doc.info.highest_generation
        );
        Ok(doc)
    }

    /// A function every document controller must have that this one lacks.
    fn missing_function(&self) -> Option<&'static str> {
        let i = &self.iface;
        let required: [(&'static str, bool); 22] = [
            ("destroyDocumentController", { i.destroyDocumentController }.is_some()),
            ("beginEditing", { i.beginEditing }.is_some()),
            ("endEditing", { i.endEditing }.is_some()),
            ("notifyModelUpdates", { i.notifyModelUpdates }.is_some()),
            ("updateDocumentProperties", { i.updateDocumentProperties }.is_some()),
            ("createMusicalContext", { i.createMusicalContext }.is_some()),
            ("updateMusicalContextContent", { i.updateMusicalContextContent }.is_some()),
            ("destroyMusicalContext", { i.destroyMusicalContext }.is_some()),
            ("createAudioSource", { i.createAudioSource }.is_some()),
            ("updateAudioSourceProperties", { i.updateAudioSourceProperties }.is_some()),
            ("enableAudioSourceSamplesAccess", { i.enableAudioSourceSamplesAccess }.is_some()),
            ("deactivateAudioSourceForUndoHistory", { i.deactivateAudioSourceForUndoHistory }.is_some()),
            ("destroyAudioSource", { i.destroyAudioSource }.is_some()),
            ("createAudioModification", { i.createAudioModification }.is_some()),
            ("updateAudioModificationProperties", { i.updateAudioModificationProperties }.is_some()),
            ("deactivateAudioModificationForUndoHistory", { i.deactivateAudioModificationForUndoHistory }.is_some()),
            ("destroyAudioModification", { i.destroyAudioModification }.is_some()),
            ("createPlaybackRegion", { i.createPlaybackRegion }.is_some()),
            ("updatePlaybackRegionProperties", { i.updatePlaybackRegionProperties }.is_some()),
            ("destroyPlaybackRegion", { i.destroyPlaybackRegion }.is_some()),
            // Region sequences are ARA 2 (addendum 2.0 draft): every plug-in this host takes has them.
            ("createRegionSequence", { i.createRegionSequence }.is_some()),
            ("destroyRegionSequence", { i.destroyRegionSequence }.is_some()),
        ];
        required
            .iter()
            .find(|(_, ok)| !ok)
            .map(|(n, _)| *n)
            .or_else(|| { i.updateRegionSequenceProperties }.is_none().then_some("updateRegionSequenceProperties"))
    }

    fn begin(&mut self) {
        if !self.editing {
            self.editing = true;
            match self.old_restore {
                Some(h) => {
                    dc!(self.beginRestoringDocumentFromArchive(as_ref(h)));
                }
                None => {
                    dc!(self.beginEditing());
                }
            }
        }
    }

    fn end(&mut self) {
        if self.editing {
            self.editing = false;
            match self.old_restore.take() {
                Some(h) => {
                    let ok = dc!(self.endRestoringDocumentFromArchive(as_ref(h)));
                    self.restore_result = Some(restored(ok, &self.info.name));
                }
                None => {
                    dc!(self.endEditing());
                }
            }
            // Follow-up updates belong to the same undo frame (see `endEditing` in the header).
            dc!(self.notifyModelUpdates());
        }
    }

    /// The plug-in's document controller (what a plug-in instance is bound to).
    pub fn controller_ref(&self) -> ARADocumentControllerRef {
        self.dc
    }

    /// The playback region of the graph region `key`, while it has one.
    pub fn region_ref(&self, key: u64) -> Option<ARAPlaybackRegionRef> {
        self.mods.get(&key).filter(|m| m.active).and_then(|m| m.region.as_ref()).map(|r| r.aref)
    }

    /// The region sequence of the graph sequence `key`.
    pub fn sequence_ref(&self, key: u64) -> Option<ARARegionSequenceRef> {
        self.seqs.get(&key).map(|s| s.aref)
    }

    /// The graph keys of the regions that have a playback region now.
    pub fn regions(&self) -> Vec<u64> {
        self.mods.iter().filter(|(_, m)| m.active && m.region.is_some()).map(|(k, _)| *k).collect()
    }

    /// The head and tail time (seconds) the plug-in says region `key` sounds beyond its bounds
    /// (`(0, 0)` when it does not say; ARA 2.0's optional `getPlaybackRegionHeadAndTailTime`).
    pub fn head_tail(&self, key: u64) -> (f64, f64) {
        let Some(r) = self.region_ref(key) else { return (0.0, 0.0) };
        let (mut head, mut tail) = (0.0f64, 0.0f64);
        if dc!(self.getPlaybackRegionHeadAndTailTime(r, &mut head, &mut tail)).is_none() {
            return (0.0, 0.0);
        }
        let ok = |t: f64| if t.is_finite() { t.clamp(0.0, 60.0) } else { 0.0 };
        (ok(head), ok(tail))
    }

    /// Restores `archive` (written by [`Document::store`] of a plug-in whose factory gave
    /// `archive_id`) in the next editing cycle that builds the graph ([`Document::sync`]), as ARA
    /// asks: the objects are made, then the plug-in reads their saved state by persistent id.
    pub fn restore_next_sync(&mut self, archive: Vec<u8>, archive_id: &str) {
        self.restore = Some((archive.into(), archive_id.to_string()));
    }

    /// How the last restore went, once (`None`: none happened since the last call).
    pub fn take_restore_result(&mut self) -> Option<Result<(), String>> {
        self.restore_result.take()
    }

    /// The plug-in's whole document as an archive (`storeObjectsToArchive`, or the deprecated
    /// `storeDocumentToArchive` of a plug-in without it). Not while editing.
    pub fn store(&mut self) -> Result<Vec<u8>, String> {
        if self.editing {
            return Err("the document is being edited".into());
        }
        let host = self.shared.new_ref();
        lock(&self.shared.archive_out).insert(host, Vec::new());
        let ok = if { self.iface.storeObjectsToArchive }.is_some() {
            dc!(self.storeObjectsToArchive(as_ref(host), std::ptr::null()))
        } else {
            dc!(self.storeDocumentToArchive(as_ref(host)))
        };
        let data = lock(&self.shared.archive_out).remove(&host).unwrap_or_default();
        match ok {
            Some(b) if b != kARAFalse => {
                crate::ara_log!("{}: stored its ARA document ({} bytes)", self.info.name, data.len());
                Ok(data)
            }
            Some(_) => {
                crate::ara_log!("{}: storeObjectsToArchive failed", self.info.name);
                Err("the plug-in could not store its document".into())
            }
            None => {
                crate::ara_log!("{}: offers no way to store its document", self.info.name);
                Err("the plug-in offers no way to store its document".into())
            }
        }
    }

    /// What the plug-in's factory says about itself.
    pub fn info(&self) -> &FactoryInfo {
        &self.info
    }

    /// The API generation in use.
    pub fn generation(&self) -> ARAAPIGeneration {
        self.generation
    }

    /// The document's name.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Lets the plug-in send its pending notifications (`notifyModelUpdates`), then turns them
    /// into [`HostEvent`]s. ARA asks hosts to do this periodically while not editing: the app
    /// calls it every frame or so.
    pub fn idle(&mut self) -> Vec<HostEvent> {
        if !self.editing {
            dc!(self.notifyModelUpdates());
        }
        self.take_events()
    }

    /// The notifications the plug-in sent since the last call, named by graph keys.
    pub fn take_events(&mut self) -> Vec<HostEvent> {
        let raw: Vec<RawEvent> = lock(&self.shared.events).drain(..).collect();
        let src_key = |h: usize| self.srcs.iter().find(|(_, s)| s.host == h).map(|(k, _)| *k);
        let mod_key = |h: usize| self.mods.iter().find(|(_, m)| m.host == h).map(|(k, _)| *k);
        let region_key = |h: usize| self.mods.iter().find(|(_, m)| m.region.as_ref().is_some_and(|r| r.host == h)).map(|(k, _)| *k);
        let mut out = Vec::new();
        for e in raw {
            match e {
                RawEvent::Progress(h, state, value) => {
                    if let Some(k) = src_key(h) {
                        let s = match state {
                            kARAAnalysisProgressStarted => AnalysisState::Started,
                            kARAAnalysisProgressCompleted => AnalysisState::Completed,
                            _ => AnalysisState::Running,
                        };
                        let v = if value.is_finite() { value.clamp(0.0, 1.0) } else { 0.0 };
                        out.push(HostEvent::Analysis { source: k, state: s, progress: v });
                    }
                }
                RawEvent::SourceContent(h) => {
                    if let Some(k) = src_key(h) {
                        out.push(HostEvent::SourceContentChanged(k));
                    }
                }
                RawEvent::ModificationContent(h) => {
                    if let Some(k) = mod_key(h) {
                        out.push(HostEvent::RegionContentChanged(k));
                    }
                }
                RawEvent::RegionContent(h) => {
                    if let Some(k) = region_key(h) {
                        out.push(HostEvent::RegionContentChanged(k));
                    }
                }
                RawEvent::DocumentData => out.push(HostEvent::DocumentDataChanged),
                RawEvent::Playback(what) => out.push(HostEvent::PlaybackRequest(what)),
            }
        }
        for e in &out {
            match e {
                HostEvent::Analysis { source, state, progress } => {
                    self.progress.insert(*source, (*state, *progress));
                }
                HostEvent::SourceContentChanged(k) => self.content_changed.push(*k),
                _ => {}
            }
        }
        out
    }

    /// The last analysis progress the plug-in reported for a source.
    pub fn analysis(&self, source: u64) -> Option<(AnalysisState, f32)> {
        self.progress.get(&source).copied()
    }

    /// How much the plug-in has read through the audio access controller.
    pub fn read_stats(&self) -> ReadStats {
        ReadStats {
            reads: self.shared.reads.load(Ordering::Relaxed),
            samples: self.shared.samples_read.load(Ordering::Relaxed),
            failed: self.shared.failed_reads.load(Ordering::Relaxed),
            open_readers: lock(&self.shared.readers).len(),
        }
    }

    /// The live objects: (region sequences, active audio sources, active modifications, playback
    /// regions, deactivated modifications).
    pub fn counts(&self) -> (usize, usize, usize, usize, usize) {
        (
            self.seqs.len(),
            self.srcs.values().filter(|s| s.active).count(),
            self.mods.values().filter(|m| m.active).count(),
            self.mods.values().filter(|m| m.region.is_some()).count(),
            self.mods.values().filter(|m| !m.active).count(),
        )
    }

    /// Whether the plug-in has content of `kind` for an audio source (by graph key).
    pub fn source_content_available(&self, source: u64, kind: ARAContentType) -> bool {
        let Some(s) = self.srcs.get(&source).filter(|s| s.active) else { return false };
        dc!(self.isAudioSourceContentAvailable(s.aref, kind)).is_some_and(|b| b != kARAFalse)
    }

    /// The notes the plug-in found in an audio source (`None` when it has none yet).
    pub fn source_notes(&self, source: u64) -> Option<Vec<Note>> {
        if self.editing || !self.source_content_available(source, kARAContentTypeNotes) {
            return None;
        }
        let s = self.srcs.get(&source)?;
        let reader = dc!(self.createAudioSourceContentReader(s.aref, kARAContentTypeNotes, std::ptr::null()))?;
        if reader.is_null() {
            return None;
        }
        let n = dc!(self.getContentReaderEventCount(reader)).unwrap_or(0).clamp(0, MAX_EVENTS);
        let mut notes = Vec::new();
        for i in 0..n {
            let Some(p) = dc!(self.getContentReaderDataForEvent(reader, i)) else { break };
            if p.is_null() {
                continue;
            }
            // SAFETY: for notes content the plug-in returns a pointer to an `ARAContentNote`,
            // valid until the next call on this reader; read unaligned (packed on x86).
            let e = unsafe { p.cast::<ARAContentNote>().read_unaligned() };
            notes.push(Note {
                frequency: e.frequency,
                pitch: e.pitchNumber,
                volume: e.volume,
                start: e.startPosition,
                attack: e.attackDuration,
                duration: e.noteDuration,
                signal: e.signalDuration,
            });
        }
        dc!(self.destroyContentReader(reader));
        Some(notes)
    }

    /// Brings the document in line with `graph` in one editing cycle (none when nothing
    /// changed): creates, updates, deactivates (for the undo history) and reactivates objects so
    /// that every region of the graph is one audio modification with one playback region on its
    /// sequence, reading its source. Invalid entries (a region naming a source or sequence that is
    /// not there, a source without channels, non-finite times) are left out and logged.
    pub fn sync(&mut self, graph: &Graph) -> SyncReport {
        let mut report = SyncReport::default();
        let g = graph.valid();
        // What changes, worked out before calling the plug-in at all.
        if !self.differs(&g) {
            return report;
        }
        // A saved document to restore: in this cycle, once the graph is built.
        let reader = self.restore.take().map(|(data, id)| {
            let host = self.shared.new_ref();
            lock(&self.shared.archive_in).insert(host, (data, c_string(&id)));
            if { self.iface.restoreObjectsFromArchive }.is_none() && { self.iface.beginRestoringDocumentFromArchive }.is_some() {
                self.old_restore = Some(host);
            }
            host
        });
        let new_style = reader.is_some() && self.old_restore.is_none();
        self.begin();
        if self.name != g.name {
            let cname = c_string(&g.name);
            let props = ARADocumentProperties { structSize: std::mem::size_of::<ARADocumentProperties>(), name: cname.as_ptr() };
            dc!(self.updateDocumentProperties(&props));
            self.name.clone_from(&g.name);
            report.updated += 1;
        }
        if self.tempo_bars != (g.tempo.clone(), g.bars.clone()) {
            *lock(&self.shared.tempo) = g.tempo.iter().map(|&(t, q)| ARAContentTempoEntry { timePosition: t, quarterPosition: q }).collect();
            *lock(&self.shared.bars) = g.bars.iter().map(|&(n, d, q)| ARAContentBarSignature { numerator: n, denominator: d, position: q }).collect();
            self.tempo_bars = (g.tempo.clone(), g.bars.clone());
            dc!(self.updateMusicalContextContent(self.context, std::ptr::null(), kARAContentUpdateEverythingChanged));
            report.updated += 1;
        }
        let wanted: HashMap<u64, &crate::RegionDesc> = g.regions.iter().map(|r| (r.key, r)).collect();
        // 1. Regions that go: destroy the playback region and keep the modification, deactivated,
        // for the undo history. A modification whose clip now reads another source cannot follow
        // it: it is destroyed and made again below.
        let keys: Vec<u64> = self.mods.keys().copied().collect();
        for k in keys {
            let gone = !wanted.contains_key(&k);
            let moved_source = wanted.get(&k).is_some_and(|r| self.mods.get(&k).is_some_and(|m| m.source != r.source));
            if gone || moved_source {
                self.drop_region(k);
                if moved_source {
                    self.destroy_modification(k);
                } else if self.mods.get(&k).is_some_and(|m| m.active) {
                    self.set_modification_active(k, false);
                    self.graveyard.push_back(k);
                }
                report.removed += 1;
            }
        }
        // 2. Region sequences: new and changed.
        for s in &g.sequences {
            match self.seqs.get(&s.key) {
                Some(have) if have.desc == *s => {}
                Some(_) => {
                    let props_name = c_string(&s.name);
                    let color = s.color.map(|[r, g, b]| ARAColor { r, g, b });
                    let props = self.sequence_props(s, &props_name, color.as_ref());
                    if let Some(have) = self.seqs.get(&s.key) {
                        dc!(self.updateRegionSequenceProperties(have.aref, &props));
                    }
                    if let Some(have) = self.seqs.get_mut(&s.key) {
                        have.desc = s.clone();
                    }
                    report.updated += 1;
                }
                None => {
                    let host = self.shared.new_ref();
                    let props_name = c_string(&s.name);
                    let color = s.color.map(|[r, g, b]| ARAColor { r, g, b });
                    let props = self.sequence_props(s, &props_name, color.as_ref());
                    let aref = dc!(self.createRegionSequence(as_ref(host), &props)).unwrap_or(std::ptr::null_mut());
                    if aref.is_null() {
                        crate::ara_log!("{}: createRegionSequence refused \"{}\"", self.info.name, s.name);
                    } else {
                        self.seqs.insert(s.key, Seq { aref, desc: s.clone() });
                        report.created += 1;
                    }
                }
            }
        }
        // 3. Audio sources that are used: new, reactivated, changed.
        let used: std::collections::BTreeSet<u64> = g.regions.iter().map(|r| r.source).collect();
        for s in g.sources.iter().filter(|s| used.contains(&s.key)) {
            let format = (s.audio.frames(), s.audio.sample_rate(), s.audio.channels());
            if !self.srcs.contains_key(&s.key) {
                if self.create_source(s, format) {
                    report.created += 1;
                }
                continue;
            }
            if self.srcs.get(&s.key).is_some_and(|have| !have.active) {
                self.set_source_active(s.key, true);
                report.reactivated += 1;
            }
            let Some(have) = self.srcs.get(&s.key) else { continue };
            let (format_changed, named, swapped) = (have.format != format, have.name != s.name, !Arc::ptr_eq(&have.audio, &s.audio));
            let (aref, host) = (have.aref, have.host);
            if format_changed || named {
                // Sample count, rate and channels may only change while reading is off.
                if format_changed {
                    dc!(self.enableAudioSourceSamplesAccess(aref, kARAFalse));
                }
                lock(&self.shared.sources).insert(host, (s.audio.clone(), !format_changed));
                let (name, pid) = (c_string(&s.name), c_string(&s.persistent_id));
                let props = source_props(&name, &pid, format, s.audio.merits_64_bit());
                dc!(self.updateAudioSourceProperties(aref, &props));
                if format_changed {
                    dc!(self.updateAudioSourceContent(aref, std::ptr::null(), kARAContentUpdateEverythingChanged));
                    lock(&self.shared.sources).insert(host, (s.audio.clone(), true));
                    dc!(self.enableAudioSourceSamplesAccess(aref, kARATrue));
                }
                report.updated += 1;
            } else if swapped {
                lock(&self.shared.sources).insert(host, (s.audio.clone(), true));
                dc!(self.updateAudioSourceContent(aref, std::ptr::null(), kARAContentUpdateEverythingChanged));
                report.updated += 1;
            }
            if let Some(have) = self.srcs.get_mut(&s.key) {
                have.format = format;
                have.name.clone_from(&s.name);
                have.audio = s.audio.clone();
            }
        }
        // 4. Modifications and their playback regions.
        for r in &g.regions {
            if !self.srcs.get(&r.source).is_some_and(|s| s.active) || !self.seqs.contains_key(&r.sequence) {
                continue;
            }
            match self.mods.get(&r.key) {
                None => {
                    if self.create_modification(r) {
                        report.created += 1;
                    }
                }
                Some(m) if !m.active => {
                    self.graveyard.retain(|k| *k != r.key);
                    self.set_modification_active(r.key, true);
                    report.reactivated += 1;
                }
                Some(_) => {}
            }
            let Some(m) = self.mods.get(&r.key) else { continue };
            if m.name != r.name {
                let (name, pid) = (c_string(&r.name), c_string(&m.pid));
                let props = ARAAudioModificationProperties {
                    structSize: std::mem::size_of::<ARAAudioModificationProperties>(),
                    name: name.as_ptr(),
                    persistentID: pid.as_ptr(),
                };
                dc!(self.updateAudioModificationProperties(m.aref, &props));
                if let Some(m) = self.mods.get_mut(&r.key) {
                    m.name.clone_from(&r.name);
                }
                report.updated += 1;
            }
            if self.place_region(r) {
                report.updated += 1;
            }
        }
        // 5. Sequences nothing is on any more.
        let keep_seqs: std::collections::BTreeSet<u64> = g.sequences.iter().map(|s| s.key).collect();
        let gone: Vec<u64> = self.seqs.keys().copied().filter(|k| !keep_seqs.contains(k)).collect();
        for k in gone {
            if self.mods.values().any(|m| m.region.as_ref().is_some_and(|r| r.desc.sequence == k)) {
                continue;
            }
            if let Some(s) = self.seqs.remove(&k) {
                dc!(self.destroyRegionSequence(s.aref));
                report.removed += 1;
            }
        }
        // 6. Sources no active modification reads: deactivated (their modifications are).
        let keys: Vec<u64> = self.srcs.keys().copied().collect();
        for k in keys {
            let in_use = self.mods.values().any(|m| m.active && m.source == k);
            if !in_use && self.srcs.get(&k).is_some_and(|s| s.active) {
                self.set_source_active(k, false);
                report.deactivated += 1;
            }
        }
        // 7. The undo history keeps a bounded number of deactivated modifications.
        while self.graveyard.len() > MAX_GRAVEYARD {
            if let Some(k) = self.graveyard.pop_front() {
                self.destroy_modification(k);
            }
        }
        self.destroy_unused_sources();
        if let Some(h) = reader.filter(|_| new_style) {
            let ok = dc!(self.restoreObjectsFromArchive(as_ref(h), std::ptr::null()));
            self.restore_result = Some(restored(ok, &self.info.name));
        }
        self.end();
        if let Some(h) = reader {
            lock(&self.shared.archive_in).remove(&h);
        }
        report
    }

    /// Whether `g` differs from what the document holds.
    fn differs(&self, g: &Graph) -> bool {
        if self.name != g.name || self.tempo_bars != (g.tempo.clone(), g.bars.clone()) {
            return true;
        }
        let seqs_same = g.sequences.len() == self.seqs.len() && g.sequences.iter().all(|s| self.seqs.get(&s.key).is_some_and(|h| h.desc == *s));
        if !seqs_same {
            return true;
        }
        let active = self.mods.values().filter(|m| m.active).count();
        if active != g.regions.len() {
            return true;
        }
        let regions_same = g.regions.iter().all(|r| {
            self.mods
                .get(&r.key)
                .is_some_and(|m| m.active && m.source == r.source && m.name == r.name && m.region.as_ref().is_some_and(|x| x.desc == *r))
        });
        if !regions_same {
            return true;
        }
        let used: std::collections::BTreeSet<u64> = g.regions.iter().map(|r| r.source).collect();
        g.sources.iter().filter(|s| used.contains(&s.key)).any(|s| {
            self.srcs.get(&s.key).is_none_or(|h| {
                !h.active
                    || h.name != s.name
                    || !Arc::ptr_eq(&h.audio, &s.audio)
                    || h.format != (s.audio.frames(), s.audio.sample_rate(), s.audio.channels())
            })
        })
    }

    fn sequence_props(&self, s: &crate::SequenceDesc, name: &CString, color: Option<&ARAColor>) -> ARARegionSequenceProperties {
        ARARegionSequenceProperties {
            structSize: std::mem::size_of::<ARARegionSequenceProperties>(),
            name: name.as_ptr(),
            orderIndex: s.order,
            musicalContextRef: self.context,
            color: color.map_or(std::ptr::null(), |c| c as *const ARAColor),
        }
    }

    fn create_source(&mut self, s: &crate::SourceDesc, format: (u64, f64, usize)) -> bool {
        let host = self.shared.new_ref();
        // Registered before the plug-in hears of it: it may start reading while it is created.
        lock(&self.shared.sources).insert(host, (s.audio.clone(), false));
        let (name, pid) = (c_string(&s.name), c_string(&s.persistent_id));
        let props = source_props(&name, &pid, format, s.audio.merits_64_bit());
        let aref = dc!(self.createAudioSource(as_ref(host), &props)).unwrap_or(std::ptr::null_mut());
        if aref.is_null() {
            lock(&self.shared.sources).remove(&host);
            crate::ara_log!("{}: createAudioSource refused \"{}\"", self.info.name, s.name);
            return false;
        }
        lock(&self.shared.sources).insert(host, (s.audio.clone(), true));
        dc!(self.enableAudioSourceSamplesAccess(aref, kARATrue));
        self.srcs.insert(s.key, Src { aref, host, name: s.name.clone(), format, audio: s.audio.clone(), active: true });
        true
    }

    fn set_source_active(&mut self, k: u64, active: bool) {
        let Some(s) = self.srcs.get(&k) else { return };
        let aref = s.aref;
        dc!(self.deactivateAudioSourceForUndoHistory(aref, if active { kARAFalse } else { kARATrue }));
        if let Some(s) = self.srcs.get_mut(&k) {
            s.active = active;
        }
    }

    /// Destroys deactivated sources no modification (active or kept) reads any more.
    fn destroy_unused_sources(&mut self) {
        let gone: Vec<u64> = self.srcs.iter().filter(|(k, s)| !s.active && !self.mods.values().any(|m| m.source == **k)).map(|(k, _)| *k).collect();
        for k in gone {
            self.destroy_source(k);
        }
    }

    fn destroy_source(&mut self, k: u64) {
        let Some(s) = self.srcs.remove(&k) else { return };
        // Disabling blocks until the plug-in's reads of it finish; then nothing reads it again.
        dc!(self.enableAudioSourceSamplesAccess(s.aref, kARAFalse));
        lock(&self.shared.sources).remove(&s.host);
        dc!(self.destroyAudioSource(s.aref));
    }

    fn create_modification(&mut self, r: &crate::RegionDesc) -> bool {
        let Some(src) = self.srcs.get(&r.source) else { return false };
        let src_ref = src.aref;
        let host = self.shared.new_ref();
        let (name, pid) = (c_string(&r.name), c_string(&r.persistent_id));
        let props = ARAAudioModificationProperties {
            structSize: std::mem::size_of::<ARAAudioModificationProperties>(),
            name: name.as_ptr(),
            persistentID: pid.as_ptr(),
        };
        // A copy of a live modification (a duplicated or split clip) starts as its clone.
        let origin = r.origin.filter(|o| *o != r.key).and_then(|o| self.mods.get(&o)).filter(|m| m.active && m.source == r.source).map(|m| m.aref);
        let cloned = match origin {
            Some(o) if { self.iface.cloneAudioModification }.is_some() => {
                dc!(self.cloneAudioModification(o, as_ref(host), &props)).filter(|p| !p.is_null())
            }
            _ => None,
        };
        let aref = match cloned {
            Some(a) => a,
            None => dc!(self.createAudioModification(src_ref, as_ref(host), &props)).unwrap_or(std::ptr::null_mut()),
        };
        if aref.is_null() {
            crate::ara_log!("{}: createAudioModification refused \"{}\"", self.info.name, r.name);
            return false;
        }
        self.mods.insert(
            r.key,
            Modification { aref, host, source: r.source, name: r.name.clone(), pid: r.persistent_id.clone(), active: true, region: None },
        );
        true
    }

    fn set_modification_active(&mut self, k: u64, active: bool) {
        let Some(m) = self.mods.get(&k) else { return };
        let aref = m.aref;
        dc!(self.deactivateAudioModificationForUndoHistory(aref, if active { kARAFalse } else { kARATrue }));
        if let Some(m) = self.mods.get_mut(&k) {
            m.active = active;
        }
    }

    fn destroy_modification(&mut self, k: u64) {
        self.drop_region(k);
        self.graveyard.retain(|g| *g != k);
        if let Some(m) = self.mods.remove(&k) {
            dc!(self.destroyAudioModification(m.aref));
        }
    }

    fn drop_region(&mut self, k: u64) {
        let Some(r) = self.mods.get_mut(&k).and_then(|m| m.region.take()) else { return };
        dc!(self.destroyPlaybackRegion(r.aref));
    }

    /// Creates or updates the playback region of modification `r.key`; whether anything changed.
    fn place_region(&mut self, r: &crate::RegionDesc) -> bool {
        let Some(m) = self.mods.get(&r.key) else { return false };
        if m.region.as_ref().is_some_and(|x| x.desc == *r) {
            return false;
        }
        let Some(seq) = self.seqs.get(&r.sequence).map(|s| s.aref) else { return false };
        let name = c_string(&r.name);
        let color = r.color.map(|[r, g, b]| ARAColor { r, g, b });
        let props = ARAPlaybackRegionProperties {
            structSize: std::mem::size_of::<ARAPlaybackRegionProperties>(),
            transformationFlags: r.transformation,
            startInModificationTime: r.start_in_source,
            durationInModificationTime: r.source_duration,
            startInPlaybackTime: r.start_in_song,
            durationInPlaybackTime: r.duration,
            musicalContextRef: self.context,
            regionSequenceRef: seq,
            name: name.as_ptr(),
            color: color.as_ref().map_or(std::ptr::null(), |c| c as *const ARAColor),
        };
        let (maref, existing) = (m.aref, m.region.as_ref().map(|x| x.aref));
        if let Some(aref) = existing {
            dc!(self.updatePlaybackRegionProperties(aref, &props));
            if let Some(reg) = self.mods.get_mut(&r.key).and_then(|m| m.region.as_mut()) {
                reg.desc = r.clone();
            }
            return true;
        }
        let host = self.shared.new_ref();
        let aref = dc!(self.createPlaybackRegion(maref, as_ref(host), &props)).unwrap_or(std::ptr::null_mut());
        if aref.is_null() {
            crate::ara_log!("{}: createPlaybackRegion refused \"{}\"", self.info.name, r.name);
            return false;
        }
        if let Some(m) = self.mods.get_mut(&r.key) {
            m.region = Some(Region { aref, host, desc: r.clone() });
        }
        true
    }
}

/// A restore's outcome from the plug-in's answer.
fn restored(ok: Option<ARABool>, name: &str) -> Result<(), String> {
    match ok {
        Some(b) if b != kARAFalse => {
            crate::ara_log!("{name}: restored its saved ARA document");
            Ok(())
        }
        Some(_) => {
            crate::ara_log!("{name}: could not read its saved ARA document (it restored what it could)");
            Err("the plug-in could not read all of its saved state".into())
        }
        None => {
            crate::ara_log!("{name}: offers no way to restore its document; its saved state is kept unread");
            Err("the plug-in offers no way to restore its saved state".into())
        }
    }
}

fn source_props(name: &CString, pid: &CString, (frames, rate, channels): (u64, f64, usize), merits64: bool) -> ARAAudioSourceProperties {
    ARAAudioSourceProperties {
        structSize: std::mem::size_of::<ARAAudioSourceProperties>(),
        name: name.as_ptr(),
        persistentID: pid.as_ptr(),
        sampleCount: i64::try_from(frames).unwrap_or(i64::MAX),
        sampleRate: rate,
        channelCount: i32::try_from(channels).unwrap_or(i32::MAX),
        merits64BitSamples: if merits64 { kARATrue } else { kARAFalse },
        channelArrangementDataType: kARAChannelArrangementUndefined,
        channelArrangement: std::ptr::null(),
    }
}

impl Drop for Document {
    /// Tears the graph down in one editing cycle (regions, modifications, sources, sequences, the
    /// musical context: ARA wants every object gone first), destroys the controller and
    /// uninitialises the factory when this was its last document.
    fn drop(&mut self) {
        if { self.iface.destroyDocumentController }.is_some() {
            self.begin();
            let keys: Vec<u64> = self.mods.keys().copied().collect();
            for k in keys {
                self.destroy_modification(k);
            }
            let keys: Vec<u64> = self.srcs.keys().copied().collect();
            for k in keys {
                self.destroy_source(k);
            }
            let seqs = std::mem::take(&mut self.seqs);
            for s in seqs.into_values() {
                dc!(self.destroyRegionSequence(s.aref));
            }
            if !self.context.is_null() {
                dc!(self.destroyMusicalContext(self.context));
            }
            self.end();
            dc!(self.destroyDocumentController());
        }
        lock(documents()).remove(&self.key);
        uninitialise(&self.factory, &self.fac);
        crate::ara_log!("{}: ARA document \"{}\" closed", self.info.name, self.name);
    }
}

// ---- plug-in instances bound to a document ------------------------------------------------------

/// What binding a plug-in instance to a document controller gave
/// (`IPlugInEntryPoint2::bindToDocumentControllerWithRoles`): the playback renderer, editor
/// renderer and editor view interfaces of the roles assigned, each checked for size and
/// functions. Used on the model thread only, like the [`Document`] (it is neither `Send` nor
/// `Sync`); valid while the plug-in instance that gave it lives (see [`Extension::new`]).
pub(crate) struct Extension {
    name: String,
    playback: Option<(ARAPlaybackRendererRef, ARAPlaybackRendererInterface)>,
    editor: Option<(ARAEditorRendererRef, ARAEditorRendererInterface)>,
    view: Option<(ARAEditorViewRef, ARAEditorViewInterface)>,
    _not_send: PhantomData<*mut ()>,
}

impl std::fmt::Debug for Extension {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Extension({}: playback {}, editor {}, view {})", self.name, self.playback.is_some(), self.editor.is_some(), self.view.is_some())
    }
}

impl Extension {
    /// Reads the extension a binding returned, for the roles `assigned`. Fails (with why) when it
    /// is null or too small, or lacks an interface or function a role assigned needs.
    ///
    /// # Safety
    /// `p` is null or what the plug-in's `bindToDocumentControllerWithRoles` returned; the plug-in
    /// instance that returned it outlives the returned value and every call made through it.
    pub(crate) unsafe fn new(p: *const ARAPlugInExtensionInstance, assigned: ARAPlugInInstanceRoleFlags, name: &str) -> Result<Extension, String> {
        // SAFETY: per the contract, null or the plug-in's instance struct, its first `structSize`
        // bytes readable.
        let Some((inst, _)) = (unsafe { copy_versioned(p, kARAPlugInExtensionInstanceMinSize) }) else {
            return Err("the plug-in gave no ARA extension when it was bound (bindToDocumentControllerWithRoles)".into());
        };
        let mut ext = Extension { name: name.to_string(), playback: None, editor: None, view: None, _not_send: PhantomData };
        if assigned & kARAPlaybackRendererRole != 0 {
            // SAFETY: null or the plug-in's static interface struct, valid while the instance lives.
            let i = unsafe { copy_versioned(inst.playbackRendererInterface, kARAPlaybackRendererInterfaceMinSize) };
            match i {
                Some((i, _))
                    if !{ inst.playbackRendererRef }.is_null() && { i.addPlaybackRegion }.is_some() && { i.removePlaybackRegion }.is_some() =>
                {
                    ext.playback = Some((inst.playbackRendererRef, i));
                }
                _ => return Err("the plug-in gave no usable ARA playback renderer".into()),
            }
        }
        if assigned & kARAEditorRendererRole != 0 {
            // SAFETY: as above.
            let i = unsafe { copy_versioned(inst.editorRendererInterface, kARAEditorRendererInterfaceMinSize) };
            if let Some((i, _)) = i.filter(|_| !{ inst.editorRendererRef }.is_null()) {
                ext.editor = Some((inst.editorRendererRef, i));
            }
        }
        if assigned & kARAEditorViewRole != 0 {
            // SAFETY: as above.
            let i = unsafe { copy_versioned(inst.editorViewInterface, kARAEditorViewInterfaceMinSize) };
            if let Some((i, _)) = i.filter(|_| !{ inst.editorViewRef }.is_null()) {
                ext.view = Some((inst.editorViewRef, i));
            }
        }
        Ok(ext)
    }

    /// Whether it renders playback regions.
    pub fn has_playback(&self) -> bool {
        self.playback.is_some()
    }

    /// Whether it has an editor view to tell the selection.
    pub fn has_view(&self) -> bool {
        self.view.is_some()
    }

    /// Gives a playback region to its playback renderer (and its editor renderer, when it has one).
    /// Only while the instance is not active (VST3 `setActive(false)`), as ARA asks.
    pub fn add_region(&self, region: ARAPlaybackRegionRef) {
        if let Some((r, i)) = &self.playback
            && let Some(f) = { i.addPlaybackRegion }
        {
            // SAFETY: the instance's own function and renderer ref (alive per `new`), on the model
            // thread, with a live playback region of the document it is bound to.
            unsafe { f(*r, region) };
        }
        if let Some((r, i)) = &self.editor
            && let Some(f) = { i.addPlaybackRegion }
        {
            // SAFETY: as above.
            unsafe { f(*r, region) };
        }
    }

    /// Takes a playback region back from its renderers (inactive instance only).
    pub fn remove_region(&self, region: ARAPlaybackRegionRef) {
        if let Some((r, i)) = &self.playback
            && let Some(f) = { i.removePlaybackRegion }
        {
            // SAFETY: as in `add_region`.
            unsafe { f(*r, region) };
        }
        if let Some((r, i)) = &self.editor
            && let Some(f) = { i.removePlaybackRegion }
        {
            // SAFETY: as in `add_region`.
            unsafe { f(*r, region) };
        }
    }

    /// Tells its editor view what the host has selected: playback regions, region sequences and a
    /// time range (seconds), as ARA's `notifySelection` describes.
    pub fn notify_selection(&self, regions: &[ARAPlaybackRegionRef], sequences: &[ARARegionSequenceRef], range: Option<(f64, f64)>) {
        let Some((v, i)) = &self.view else { return };
        let Some(f) = ({ i.notifySelection }) else { return };
        let range =
            range.filter(|(s, d)| s.is_finite() && d.is_finite() && *d >= 0.0).map(|(start, duration)| ARAContentTimeRange { start, duration });
        let sel = ARAViewSelection {
            structSize: std::mem::size_of::<ARAViewSelection>(),
            playbackRegionRefsCount: regions.len(),
            playbackRegionRefs: if regions.is_empty() { std::ptr::null() } else { regions.as_ptr() },
            regionSequenceRefsCount: sequences.len(),
            regionSequenceRefs: if sequences.is_empty() { std::ptr::null() } else { sequences.as_ptr() },
            timeRange: range.as_ref().map_or(std::ptr::null(), |r| r as *const ARAContentTimeRange),
        };
        // SAFETY: the view's own function and ref (alive per `new`), on the model thread; the
        // selection and the arrays it points to live for the call.
        unsafe { f(*v, &sel) };
    }

    /// Tells its editor view which region sequences the host hides (`notifyHideRegionSequences`).
    pub fn hide_sequences(&self, sequences: &[ARARegionSequenceRef]) {
        let Some((v, i)) = &self.view else { return };
        let Some(f) = ({ i.notifyHideRegionSequences }) else { return };
        // SAFETY: as in `notify_selection`; the array lives for the call.
        unsafe { f(*v, sequences.len(), if sequences.is_empty() { std::ptr::null() } else { sequences.as_ptr() }) };
    }
}

/// The ARA extension a plug-in instance's binding returned, not yet read. Only the FFI side that
/// bound the instance can make one ([`RawExtension::new`]); a [`BoundInstance`] reads it and keeps
/// it together with that instance.
pub struct RawExtension(*const ARAPlugInExtensionInstance);

impl RawExtension {
    /// # Safety
    /// `p` is null, or what `bindToDocumentControllerWithRoles` of the plug-in instance this value
    /// is handed along with (to [`BoundInstance::new`]) returned: valid while that instance lives.
    pub unsafe fn new(p: *const ARAPlugInExtensionInstance) -> RawExtension {
        RawExtension(p)
    }
}

/// A plug-in instance shared between the model thread (which made it and will destroy it) and the
/// audio thread (which renders with it, never waiting for it).
pub struct InstanceCell<P: ?Sized> {
    inner: Mutex<Option<Box<P>>>,
}

impl<P: ?Sized> InstanceCell<P> {
    /// Runs `f` on the instance unless the other thread holds it or it is gone (never waits:
    /// for the audio thread).
    pub fn try_with<R>(&self, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        let mut g = self.inner.try_lock().ok()?;
        g.as_deref_mut().map(f)
    }

    /// Runs `f` on the instance, waiting for a block the audio thread is rendering with it
    /// (model thread only). `None` once it is gone.
    pub fn with<R>(&self, f: impl FnOnce(&mut P) -> R) -> Option<R> {
        lock(&self.inner).as_deref_mut().map(f)
    }

    /// Whether the instance is still there.
    pub fn alive(&self) -> bool {
        lock(&self.inner).is_some()
    }
}

/// A plug-in instance bound to an ARA document controller and the extension its binding gave,
/// kept together so the extension is never used once the instance is gone: dropping this destroys
/// the instance (on the model thread, where it was made, as ARA asks), the audio thread's share of
/// it ([`BoundInstance::cell`]) then holding nothing. Neither `Send` nor `Sync`.
pub struct BoundInstance<P: ?Sized> {
    cell: Arc<InstanceCell<P>>,
    ext: Extension,
}

impl<P: ?Sized> BoundInstance<P> {
    /// Reads the extension the binding of `instance` gave for the roles `assigned` (see
    /// [`Extension`]): fails with why when it is unusable (the instance is then destroyed here).
    pub fn new(instance: Box<P>, raw: RawExtension, assigned: ARAPlugInInstanceRoleFlags, name: &str) -> Result<BoundInstance<P>, String> {
        // SAFETY: `raw` came from the binding of `instance` (`RawExtension::new`'s contract), which
        // this value keeps alive for as long as the extension is used (it is used only through
        // `self`, and `Drop` destroys the instance, the extension going with `self`).
        let ext = unsafe { Extension::new(raw.0, assigned, name) }?;
        Ok(BoundInstance { cell: Arc::new(InstanceCell { inner: Mutex::new(Some(instance)) }), ext })
    }

    /// The instance, for the audio thread ([`InstanceCell::try_with`]) and the model thread.
    pub fn cell(&self) -> Arc<InstanceCell<P>> {
        self.cell.clone()
    }

    /// Runs `f` with the extension while the instance is alive (held for the call).
    fn with_ext(&self, f: impl FnOnce(&Extension)) {
        let g = lock(&self.cell.inner);
        if g.is_some() {
            f(&self.ext);
        }
    }

    /// Whether it renders playback regions / has an editor view.
    pub fn has_playback(&self) -> bool {
        self.ext.has_playback()
    }
    pub fn has_view(&self) -> bool {
        self.ext.has_view()
    }

    /// See [`Extension::add_region`] (only while the instance is not active).
    pub fn add_region(&self, region: ARAPlaybackRegionRef) {
        self.with_ext(|e| e.add_region(region));
    }

    /// See [`Extension::remove_region`] (only while the instance is not active).
    pub fn remove_region(&self, region: ARAPlaybackRegionRef) {
        self.with_ext(|e| e.remove_region(region));
    }

    /// See [`Extension::notify_selection`].
    pub fn notify_selection(&self, regions: &[ARAPlaybackRegionRef], sequences: &[ARARegionSequenceRef], range: Option<(f64, f64)>) {
        self.with_ext(|e| e.notify_selection(regions, sequences, range));
    }

    /// See [`Extension::hide_sequences`].
    pub fn hide_sequences(&self, sequences: &[ARARegionSequenceRef]) {
        self.with_ext(|e| e.hide_sequences(sequences));
    }
}

impl<P: ?Sized> Drop for BoundInstance<P> {
    fn drop(&mut self) {
        // Waits for a block the audio thread renders with it, then destroys it here.
        let instance = lock(&self.cell.inner).take();
        drop(instance);
    }
}

#[cfg(test)]
pub(crate) mod testing {
    //! Direct access to the host interfaces, as a plug-in would call them, for the hostile tests.
    use super::*;

    /// The host instance's audio access controller host ref and interface.
    pub fn audio_access(doc: &Document) -> (ARAAudioAccessControllerHostRef, &'static ARAAudioAccessControllerInterface) {
        (as_ref(doc.key), &AUDIO_ACCESS)
    }

    /// The host ref of a source (by graph key).
    pub fn source_ref(doc: &Document, key: u64) -> Option<ARAAudioSourceHostRef> {
        doc.srcs.get(&key).map(|s| as_ref(s.host))
    }

    /// Calls `createAudioReaderForSource`.
    pub fn create_reader(doc: &Document, source: ARAAudioSourceHostRef, use64: bool) -> ARAAudioReaderHostRef {
        let (c, i) = audio_access(doc);
        // SAFETY: the host's own function, any arguments allowed.
        let f = { i.createAudioReaderForSource };
        f.map_or(std::ptr::null_mut(), |f| unsafe { f(c, source, if use64 { kARATrue } else { kARAFalse }) })
    }

    /// Calls `readAudioSamples` into `channels` buffers of `count` f32 (or f64) samples.
    pub fn read(doc: &Document, reader: ARAAudioReaderHostRef, pos: i64, count: usize, channels: usize, use64: bool) -> (bool, Vec<Vec<f64>>) {
        let (c, i) = audio_access(doc);
        let mut b32 = vec![vec![7.0f32; count]; channels];
        let mut b64 = vec![vec![7.0f64; count]; channels];
        let ptrs: Vec<*mut c_void> =
            if use64 { b64.iter_mut().map(|b| b.as_mut_ptr().cast()).collect() } else { b32.iter_mut().map(|b| b.as_mut_ptr().cast()).collect() };
        // SAFETY: the host's own function; `ptrs` has one buffer of `count` samples per channel.
        let ok = { i.readAudioSamples }.is_some_and(|f| unsafe { f(c, reader, pos, count as i64, ptrs.as_ptr()) } != kARAFalse);
        let out = if use64 { b64 } else { b32.into_iter().map(|b| b.into_iter().map(f64::from).collect()).collect() };
        (ok, out)
    }

    /// Calls `readAudioSamples` with raw arguments.
    pub fn read_raw(doc: &Document, reader: ARAAudioReaderHostRef, pos: i64, count: i64, buffers: *const *mut c_void) -> bool {
        let (c, i) = audio_access(doc);
        // SAFETY: the host's own function; the tests pass what they want to see refused.
        let f = { i.readAudioSamples };
        f.is_some_and(|f| unsafe { f(c, reader, pos, count, buffers) } != kARAFalse)
    }

    /// Calls `destroyAudioReader`.
    pub fn destroy_reader(doc: &Document, reader: ARAAudioReaderHostRef) {
        let (c, i) = audio_access(doc);
        if let Some(f) = { i.destroyAudioReader } {
            // SAFETY: the host's own function, any arguments allowed.
            unsafe { f(c, reader) };
        }
    }

    /// The host instance handed to the plug-in.
    pub fn host_instance(doc: &Document) -> &ARADocumentControllerHostInstance {
        &doc._host
    }
}

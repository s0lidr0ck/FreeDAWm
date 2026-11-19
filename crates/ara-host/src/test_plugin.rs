//! The smallest ARA plug-in side, in Rust, against the same bindings (feature `test-plugin`; never
//! part of the app). Used by this crate's tests, the engine's tests and the ARA VST3 test fixture
//! (`crates/vst3-host/tests/ara_fixture`), which hands [`factory`] out through `ARA::IMainFactory`
//! and binds instances with [`bind`].
//!
//! Its document controller accepts region sequences, audio sources, modifications and playback
//! regions, checks the host keeps ARA's rules (edits inside editing cycles, objects destroyed and
//! deactivated in the right order, nothing used after it is gone) and records every break as a
//! violation. In `endEditing` it reads every audio source whose reading is enabled through the
//! host's audio access controller, once with 32-bit and once with 64-bit samples (in odd-sized
//! chunks, and a little past the end, which must read as silence), and reports what it read as
//! two "notes" of the source's notes content: `startPosition` holds [`checksum`] of what it read
//! shifted right by 11 bits (so it fits a double exactly), `noteDuration` the frames read per
//! channel, `pitchNumber` the channel count, `volume` 0.5 for the 32-bit read and 1.0 for the
//! 64-bit one. In `notifyModelUpdates` it reports the analysis of each source read as started and
//! completed. [`report`] gives tests the controller's own view by document name.
//!
//! [`Behaviour`]s make it misbehave in the ways a real plug-in might.

#![allow(clippy::missing_safety_doc)]

use crate::sys::*;
use std::collections::HashMap;
use std::ffi::{c_char, c_void};
use std::sync::{Mutex, PoisonError};

/// How the test plug-in behaves.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Behaviour {
    /// Keeps the API.
    Good = 0,
    /// `createDocumentControllerWithDocument` returns null.
    NoController = 1,
    /// Its controller instance has a null interface pointer.
    NullInterface = 2,
    /// Its controller interface says it is 16 bytes long.
    TinyInterface = 3,
    /// Its controller interface has no `createPlaybackRegion`.
    MissingFunction = 4,
    /// `createAudioSource` returns null.
    RefusesSources = 5,
    /// Calls the host with refs it never got, null buffers, negative counts, double destroys.
    Hostile = 6,
    /// Reads every source from four threads at once, each with its own reader.
    Concurrent = 7,
    /// An ARA 1 plug-in (generations 1 to 2).
    Ara1 = 8,
    /// Its factory says it is 16 bytes long.
    TinyFactory = 9,
    /// Its factory has no `createDocumentControllerWithDocument`.
    NullCreate = 10,
    /// Binding an instance gives no playback renderer.
    NoRenderer = 11,
}

/// What the controller of one document saw (by document name).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    /// Calls received, in order (`"createAudioSource <name>"`, `"endEditing"`, …).
    pub calls: Vec<String>,
    /// Broken ARA rules the host committed.
    pub violations: Vec<String>,
    /// Per audio source persistent id: (checksum of the 32-bit read, of the 64-bit read, frames
    /// read per channel, the reads past the end were silent).
    pub read: HashMap<String, (u64, u64, i64, bool)>,
    /// Live objects: sequences, active sources, active modifications, regions.
    pub live: (usize, usize, usize, usize),
    /// Deactivated sources and modifications.
    pub deactivated: (usize, usize),
    /// Tempo entries the host offered in the last `endEditing` (`None`: none available).
    pub tempo_entries: Option<i32>,
    /// The hostile calls made and whether the host answered each as it should.
    pub hostile: Vec<(String, bool)>,
    /// Playback regions added to a bound instance's playback renderer, minus removed.
    pub rendered: usize,
    /// Calls to bound instances' renderers made while the plug-in instance was rendering
    /// (`render` running): ARA wants regions changed only on an inactive instance.
    pub renderer_while_rendering: usize,
    /// The last selection an editor view was told: (playback regions, region sequences, time
    /// range), and how many region sequences it was told to hide.
    pub selection: Option<(usize, usize, Option<(f64, f64)>)>,
    pub hidden: usize,
    /// Archives stored and restored, and modifications cloned.
    pub stored: usize,
    pub restored: usize,
    pub cloned: usize,
    /// Whether the controller was destroyed.
    pub destroyed: bool,
}

fn reports() -> &'static Mutex<HashMap<String, Report>> {
    static R: std::sync::OnceLock<Mutex<HashMap<String, Report>>> = std::sync::OnceLock::new();
    R.get_or_init(|| Mutex::new(HashMap::new()))
}

/// What the controller of the document named `name` saw (the latest one by that name).
pub fn report(name: &str) -> Option<Report> {
    reports().lock().unwrap_or_else(PoisonError::into_inner).get(name).cloned()
}

/// FNV-1a over each channel's samples (as the bits of `f64`s), then over the channel hashes.
pub fn checksum(channels: &[Vec<f64>]) -> u64 {
    fn fnv(h: u64, bytes: &[u8]) -> u64 {
        bytes.iter().fold(h, |h, b| (h ^ u64::from(*b)).wrapping_mul(0x100_0000_01b3))
    }
    const SEED: u64 = 0xcbf2_9ce4_8422_2325;
    let per: Vec<u64> = channels.iter().map(|c| c.iter().fold(SEED, |h, x| fnv(h, &x.to_bits().to_le_bytes()))).collect();
    per.iter().fold(SEED, |h, x| fnv(h, &x.to_le_bytes()))
}

// ---- factory ----------------------------------------------------------------------------------

struct SyncFactory(ARAFactory);
// SAFETY: immutable static data (string literals, function pointers, a promoted constant array).
unsafe impl Sync for SyncFactory {}

const CONTENT_TYPES: &[ARAContentType] = &[kARAContentTypeNotes];
const ARCHIVE_IDS: &[ARAPersistentID] = &[c"org.freedawm.test.ara.archive.1".as_ptr()];

/// The transformations the test plug-in supports: time-stretch (a region may play its
/// modification over a different duration).
pub const TRANSFORMATIONS: ARAPlaybackTransformationFlags = kARAPlaybackTransformationTimestretch;

/// The gain the test plug-in renders a modification with until [`set_gain`] changes it.
pub const DEFAULT_GAIN: f32 = -0.5;

const fn make<const B: u8>() -> ARAFactory {
    let (lowest, highest) = if B == Behaviour::Ara1 as u8 {
        (kARAAPIGeneration_1_0_Draft, kARAAPIGeneration_1_0_Final)
    } else {
        (kARAAPIGeneration_2_0_Final, kARAAPIGeneration_2_3_Final)
    };
    ARAFactory {
        structSize: if B == Behaviour::TinyFactory as u8 { 16 } else { std::mem::size_of::<ARAFactory>() },
        lowestSupportedApiGeneration: lowest,
        highestSupportedApiGeneration: highest,
        factoryID: c"org.freedawm.test.ara".as_ptr(),
        initializeARAWithConfiguration: Some(initialize),
        uninitializeARA: Some(uninitialize),
        plugInName: c"FreeDAWm Test ARA".as_ptr(),
        manufacturerName: c"FreeDAWm tests".as_ptr(),
        informationURL: c"https://example.invalid".as_ptr(),
        version: c"1.0.0".as_ptr(),
        createDocumentControllerWithDocument: if B == Behaviour::NullCreate as u8 { None } else { Some(create_controller::<B>) },
        documentArchiveID: c"org.freedawm.test.ara.archive.1".as_ptr(),
        compatibleDocumentArchiveIDsCount: ARCHIVE_IDS.len(),
        compatibleDocumentArchiveIDs: ARCHIVE_IDS.as_ptr(),
        analyzeableContentTypesCount: CONTENT_TYPES.len(),
        analyzeableContentTypes: CONTENT_TYPES.as_ptr(),
        supportedPlaybackTransformationFlags: TRANSFORMATIONS,
        supportsStoringAudioFileChunks: kARAFalse,
    }
}

static FACTORIES: [SyncFactory; 12] = [
    SyncFactory(make::<0>()),
    SyncFactory(make::<1>()),
    SyncFactory(make::<2>()),
    SyncFactory(make::<3>()),
    SyncFactory(make::<4>()),
    SyncFactory(make::<5>()),
    SyncFactory(make::<6>()),
    SyncFactory(make::<7>()),
    SyncFactory(make::<8>()),
    SyncFactory(make::<9>()),
    SyncFactory(make::<10>()),
    SyncFactory(make::<11>()),
];

/// The static ARA factory of the test plug-in behaving as `b`.
pub fn factory(b: Behaviour) -> *const ARAFactory {
    FACTORIES.get(b as usize).map_or(std::ptr::null(), |f| &f.0 as *const ARAFactory)
}

/// [`factory`] as the host's [`crate::FactoryPtr`].
pub fn factory_ptr(b: Behaviour) -> crate::FactoryPtr {
    // SAFETY: a static of this binary, valid for the process; never null.
    match unsafe { crate::FactoryPtr::new(factory(b)) } {
        Some(f) => f,
        None => unreachable_factory(),
    }
}

#[cold]
fn unreachable_factory() -> crate::FactoryPtr {
    // `factory` only returns null for an index past the table, which `Behaviour` cannot name.
    std::process::abort()
}

/// `initializeARAWithConfiguration` calls minus `uninitializeARA` calls (all factories).
pub static INITIALISED: std::sync::atomic::AtomicI64 = std::sync::atomic::AtomicI64::new(0);

unsafe extern "C" fn initialize(config: *const ARAInterfaceConfiguration) {
    if !config.is_null() {
        INITIALISED.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }
}

unsafe extern "C" fn uninitialize() {
    INITIALISED.fetch_sub(1, std::sync::atomic::Ordering::SeqCst);
}

// ---- the document controller -----------------------------------------------------------------

#[derive(Default)]
struct PSrc {
    host: ARAAudioSourceHostRef,
    pid: String,
    frames: i64,
    channels: i32,
    rate: f64,
    access: bool,
    active: bool,
    read: bool,
    reported: bool,
    /// What the 32-bit read gave, per channel: what the renderer plays.
    audio: std::sync::Arc<Vec<Vec<f32>>>,
}

struct PMod {
    source: usize,
    active: bool,
    host: ARAAudioModificationHostRef,
    pid: String,
    /// The test plug-in's one "edit": the gain it renders the modification with.
    gain: f32,
    /// Its content changed since the host was last told.
    dirty: bool,
}

#[derive(Clone, Copy)]
struct PRegion {
    modification: usize,
    sequence: usize,
    start_mod: f64,
    dur_mod: f64,
    start_pb: f64,
    dur_pb: f64,
}

#[derive(Default)]
struct State {
    editing: bool,
    next: usize,
    contexts: Vec<usize>,
    seqs: Vec<usize>,
    srcs: HashMap<usize, PSrc>,
    mods: HashMap<usize, PMod>,
    regions: HashMap<usize, PRegion>,
    readers: HashMap<usize, Vec<ARAContentNote>>,
    notes: HashMap<usize, Vec<ARAContentNote>>,
    /// The document data changed since the host was last told.
    data_dirty: bool,
    /// Renders fail (non-finite output) from now on ([`set_fault`]).
    fault: bool,
}

struct Controller {
    behaviour: u8,
    name: String,
    host: ARADocumentControllerHostInstance,
    audio: ARAAudioAccessControllerInterface,
    content: ARAContentAccessControllerInterface,
    model: ARAModelUpdateControllerInterface,
    archiving: ARAArchivingControllerInterface,
    playback: ARAPlaybackControllerInterface,
    instance: ARADocumentControllerInstance,
    state: Mutex<State>,
}

/// Live controllers (addresses), so a host passing a stale or wrong ref is a recorded violation.
fn live() -> &'static Mutex<Vec<usize>> {
    static L: Mutex<Vec<usize>> = Mutex::new(Vec::new());
    &L
}

fn with_report(name: &str, f: impl FnOnce(&mut Report)) {
    let mut r = reports().lock().unwrap_or_else(PoisonError::into_inner);
    f(r.entry(name.to_string()).or_default());
}

/// What a call answers when the controller is not live.
trait Fallback {
    fn fallback() -> Self;
}
impl Fallback for () {
    fn fallback() -> Self {}
}
impl Fallback for bool {
    fn fallback() -> Self {
        false
    }
}
impl Fallback for i32 {
    fn fallback() -> Self {
        0
    }
}
impl Fallback for *mut c_void {
    fn fallback() -> Self {
        std::ptr::null_mut()
    }
}
impl<T> Fallback for Option<T> {
    fn fallback() -> Self {
        None
    }
}

/// Runs `f` on the controller behind `dc`, or records a violation when it is not live.
fn with<R: Fallback>(dc: ARADocumentControllerRef, call: &str, f: impl FnOnce(&Controller, &mut State) -> R) -> R {
    let ok = live().lock().unwrap_or_else(PoisonError::into_inner).contains(&(dc as usize));
    if !ok {
        with_report("<unknown>", |r| r.violations.push(format!("{call} on a controller that is not live")));
        return R::fallback();
    }
    // SAFETY: `dc` is the address of a live boxed `Controller` (checked above); only `destroy`
    // frees it, on the model thread, after removing it from `live`.
    let c = unsafe { &*(dc as *const Controller) };
    let mut s = c.state.lock().unwrap_or_else(PoisonError::into_inner);
    let r = f(c, &mut s);
    let live_counts = (s.seqs.len(), s.srcs.values().filter(|x| x.active).count(), s.mods.values().filter(|x| x.active).count(), s.regions.len());
    let deact = (s.srcs.values().filter(|x| !x.active).count(), s.mods.values().filter(|x| !x.active).count());
    with_report(&c.name, |rep| {
        rep.calls.push(call.to_string());
        rep.live = live_counts;
        rep.deactivated = deact;
    });
    r
}

fn violation(c: &Controller, what: String) {
    with_report(&c.name, |r| r.violations.push(what));
}

/// Checks an edit happens inside an editing cycle.
fn editing(c: &Controller, s: &State, call: &str) {
    if !s.editing {
        violation(c, format!("{call} outside beginEditing/endEditing"));
    }
}

fn new_ref(s: &mut State) -> usize {
    s.next += 1;
    s.next
}

fn ptr(n: usize) -> *mut c_void {
    n as *mut c_void
}

unsafe fn cstr(p: *const c_char) -> String {
    if p.is_null() {
        return String::new();
    }
    // SAFETY: the host passes NUL-terminated strings valid for the call.
    unsafe { std::ffi::CStr::from_ptr(p) }.to_string_lossy().into_owned()
}

unsafe extern "C" fn create_controller<const B: u8>(
    host: *const ARADocumentControllerHostInstance,
    props: *const ARADocumentProperties,
) -> *const ARADocumentControllerInstance {
    if B == Behaviour::NoController as u8 || host.is_null() {
        return std::ptr::null();
    }
    // SAFETY: the host's instance and the interfaces it points to are valid for the controller's
    // life (ARA contract); copied by value (they are fully sized: the host is ours).
    let (hi, audio, content, model, archiving, playback) = unsafe {
        let hi = host.read_unaligned();
        (
            hi,
            { hi.audioAccessControllerInterface }.read_unaligned(),
            { hi.contentAccessControllerInterface }.read_unaligned(),
            { hi.modelUpdateControllerInterface }.read_unaligned(),
            { hi.archivingControllerInterface }.read_unaligned(),
            { hi.playbackControllerInterface }.read_unaligned(),
        )
    };
    // SAFETY: valid for the call.
    let name = if props.is_null() { String::new() } else { unsafe { cstr(props.read_unaligned().name) } };
    reports().lock().unwrap_or_else(PoisonError::into_inner).insert(name.clone(), Report::default());
    let iface: *const ARADocumentControllerInterface = if B == Behaviour::NullInterface as u8 {
        std::ptr::null()
    } else if B == Behaviour::TinyInterface as u8 {
        &IFACE_TINY
    } else if B == Behaviour::MissingFunction as u8 {
        &IFACE_MISSING
    } else {
        &IFACE
    };
    let c = Box::new(Controller {
        behaviour: B,
        name,
        host: hi,
        audio,
        content,
        model,
        archiving,
        playback,
        instance: ARADocumentControllerInstance {
            structSize: std::mem::size_of::<ARADocumentControllerInstance>(),
            documentControllerRef: std::ptr::null_mut(),
            documentControllerInterface: iface,
        },
        state: Mutex::new(State::default()),
    });
    let p = Box::into_raw(c);
    // SAFETY: just made; its instance points at itself.
    unsafe { (*p).instance.documentControllerRef = p.cast() };
    live().lock().unwrap_or_else(PoisonError::into_inner).push(p as usize);
    // SAFETY: lives until `destroy`.
    unsafe { &raw const (*p).instance }
}

unsafe extern "C" fn destroy(dc: ARADocumentControllerRef) {
    let ok = with(dc, "destroyDocumentController", |c, s| {
        if !s.seqs.is_empty() || !s.srcs.is_empty() || !s.mods.is_empty() || !s.regions.is_empty() || !s.contexts.is_empty() {
            violation(c, "destroyDocumentController with objects left".into());
        }
        with_report(&c.name, |r| r.destroyed = true);
        true
    });
    if ok {
        live().lock().unwrap_or_else(PoisonError::into_inner).retain(|a| *a != dc as usize);
        // SAFETY: made by `Box::into_raw` in `create_controller`, removed from `live` above, so no
        // other call reaches it again.
        drop(unsafe { Box::from_raw(dc.cast::<Controller>()) });
    }
}

unsafe extern "C" fn get_factory(dc: ARADocumentControllerRef) -> *const ARAFactory {
    with(dc, "getFactory", |c, _| {
        Some(factory(match c.behaviour {
            6 => Behaviour::Hostile,
            7 => Behaviour::Concurrent,
            5 => Behaviour::RefusesSources,
            _ => Behaviour::Good,
        }))
    })
    .unwrap_or(std::ptr::null())
}

unsafe extern "C" fn begin_editing(dc: ARADocumentControllerRef) {
    with(dc, "beginEditing", |c, s| {
        if s.editing {
            violation(c, "beginEditing inside an editing cycle".into());
        }
        s.editing = true;
    })
}

unsafe extern "C" fn end_editing(dc: ARADocumentControllerRef) {
    with(dc, "endEditing", |c, s| {
        if !s.editing {
            violation(c, "endEditing without beginEditing".into());
        }
        s.editing = false;
        read_sources(c, s);
        if c.behaviour == Behaviour::Hostile as u8 {
            hostile(c, s);
        }
        let tempo = tempo_entries(c);
        with_report(&c.name, |r| r.tempo_entries = tempo);
    })
}

unsafe extern "C" fn notify_model_updates(dc: ARADocumentControllerRef) {
    with(dc, "notifyModelUpdates", |c, s| {
        if s.editing {
            violation(c, "notifyModelUpdates inside an editing cycle".into());
        }
        for src in s.srcs.values_mut().filter(|x| x.read && !x.reported && x.active) {
            src.reported = true;
            let r = c.host.modelUpdateControllerHostRef;
            if let Some(f) = { c.model.notifyAudioSourceAnalysisProgress } {
                // SAFETY: the host's function and refs, during notifyModelUpdates as ARA requires.
                unsafe {
                    f(r, src.host, kARAAnalysisProgressStarted, 0.0);
                    f(r, src.host, kARAAnalysisProgressCompleted, 1.0);
                }
            }
            if let Some(f) = { c.model.notifyAudioSourceContentChanged } {
                // SAFETY: as above.
                unsafe { f(r, src.host, std::ptr::null(), kARAContentUpdateEverythingChanged) };
            }
        }
        let r = c.host.modelUpdateControllerHostRef;
        for m in s.mods.values_mut().filter(|m| m.dirty) {
            m.dirty = false;
            if let Some(f) = { c.model.notifyAudioModificationContentChanged } {
                // SAFETY: as above.
                unsafe { f(r, m.host, std::ptr::null(), kARAContentUpdateEverythingChanged) };
            }
        }
        if std::mem::take(&mut s.data_dirty)
            && c.model.structSize > std::mem::offset_of!(ARAModelUpdateControllerInterface, notifyDocumentDataChanged)
            && let Some(f) = { c.model.notifyDocumentDataChanged }
        {
            // SAFETY: as above (the host's interface is large enough to have it).
            unsafe { f(r) };
        }
    })
}

/// Writes the test plug-in's archive: a header line, then one `persistent id <tab> gain` line per
/// modification stored.
unsafe extern "C" fn store_objects(dc: ARADocumentControllerRef, w: ARAArchiveWriterHostRef, filter: *const ARAStoreObjectsFilter) -> ARABool {
    with(dc, "storeObjectsToArchive", |c, s| {
        if s.editing {
            violation(c, "storeObjectsToArchive while editing".into());
        }
        let only: Option<Vec<usize>> = if filter.is_null() {
            None
        } else {
            // SAFETY: valid for the call, with `audioModificationRefsCount` refs.
            let f = unsafe { filter.read_unaligned() };
            let p = f.audioModificationRefs;
            Some(if p.is_null() {
                Vec::new()
            } else {
                (0..f.audioModificationRefsCount).map(|i| unsafe { p.add(i).read_unaligned() } as usize).collect()
            })
        };
        let mut text = String::from("FreeDAWm test ARA archive 1\n");
        let mut ids: Vec<(&String, f32)> =
            s.mods.iter().filter(|(k, _)| only.as_ref().is_none_or(|o| o.contains(k))).map(|(_, m)| (&m.pid, m.gain)).collect();
        ids.sort_by(|a, b| a.0.cmp(b.0));
        for (pid, gain) in ids {
            text.push_str(&format!("{pid}\t{}\n", gain.to_bits()));
        }
        let Some(write) = ({ c.archiving.writeBytesToArchive }) else { return kARAFalse };
        let bytes = text.as_bytes();
        // Two writes, the second at an offset, as plug-ins do.
        let half = bytes.len() / 2;
        // SAFETY: the host's function and writer ref, during this call as ARA requires.
        let ok = unsafe {
            write(c.host.archivingControllerHostRef, w, 0, half, bytes.as_ptr()) != kARAFalse
                && write(c.host.archivingControllerHostRef, w, half, bytes.len() - half, bytes.as_ptr().add(half)) != kARAFalse
        };
        with_report(&c.name, |r| r.stored += 1);
        if ok { kARATrue } else { kARAFalse }
    })
}

/// Reads an archive [`store_objects`] wrote back into the modifications with the same persistent
/// ids (inside an editing cycle, after the host made them).
unsafe extern "C" fn restore_objects(dc: ARADocumentControllerRef, r: ARAArchiveReaderHostRef, _filter: *const ARARestoreObjectsFilter) -> ARABool {
    with(dc, "restoreObjectsFromArchive", |c, s| {
        if !s.editing {
            violation(c, "restoreObjectsFromArchive outside an editing cycle".into());
        }
        let (Some(size), Some(read)) = ({ c.archiving.getArchiveSize }, { c.archiving.readBytesFromArchive }) else { return kARAFalse };
        let h = c.host.archivingControllerHostRef;
        // SAFETY: the host's functions and reader ref, during this call as ARA requires.
        let n = unsafe { size(h, r) };
        if n > 1 << 20 {
            return kARAFalse;
        }
        let mut buf = vec![0u8; n];
        // SAFETY: as above; `buf` holds `n` bytes.
        if n > 0 && unsafe { read(h, r, 0, n, buf.as_mut_ptr()) } == kARAFalse {
            return kARAFalse;
        }
        // SAFETY: as above.
        let id = unsafe { cstr({ c.archiving.getDocumentArchiveID }.map_or(std::ptr::null(), |f| f(h, r))) };
        if id != "org.freedawm.test.ara.archive.1" {
            violation(c, format!("restoreObjectsFromArchive of an archive with id \"{id}\""));
        }
        let text = String::from_utf8_lossy(&buf);
        let mut lines = text.lines();
        if lines.next() != Some("FreeDAWm test ARA archive 1") {
            return kARAFalse;
        }
        for line in lines {
            let Some((pid, bits)) = line.split_once('\t') else { return kARAFalse };
            let Ok(bits) = bits.parse::<u32>() else { return kARAFalse };
            for m in s.mods.values_mut().filter(|m| m.pid == pid) {
                m.gain = f32::from_bits(bits);
            }
        }
        with_report(&c.name, |rep| rep.restored += 1);
        kARATrue
    })
}

/// What [`store_objects`] wrote for a document's modifications, by persistent id (tests).
pub fn parse_archive(bytes: &[u8]) -> Option<HashMap<String, f32>> {
    let text = std::str::from_utf8(bytes).ok()?;
    let mut lines = text.lines();
    (lines.next()? == "FreeDAWm test ARA archive 1").then_some(())?;
    lines.map(|l| l.split_once('\t').and_then(|(p, b)| Some((p.to_string(), f32::from_bits(b.parse().ok()?))))).collect()
}

unsafe extern "C" fn head_tail(dc: ARADocumentControllerRef, _r: ARAPlaybackRegionRef, head: *mut ARATimeDuration, tail: *mut ARATimeDuration) {
    with(dc, "getPlaybackRegionHeadAndTailTime", |_, _| {
        if !head.is_null() && !tail.is_null() {
            // SAFETY: the host's out-pointers, valid for the call.
            unsafe {
                head.write_unaligned(0.0);
                tail.write_unaligned(0.0);
            }
        }
    })
}

unsafe extern "C" fn update_document(dc: ARADocumentControllerRef, _p: *const ARADocumentProperties) {
    with(dc, "updateDocumentProperties", |c, s| editing(c, s, "updateDocumentProperties"))
}

unsafe extern "C" fn create_context(
    dc: ARADocumentControllerRef,
    _h: ARAMusicalContextHostRef,
    _p: *const ARAMusicalContextProperties,
) -> ARAMusicalContextRef {
    with(dc, "createMusicalContext", |c, s| {
        editing(c, s, "createMusicalContext");
        let r = new_ref(s);
        s.contexts.push(r);
        ptr(r)
    })
}

unsafe extern "C" fn update_context_content(
    dc: ARADocumentControllerRef,
    m: ARAMusicalContextRef,
    _r: *const ARAContentTimeRange,
    _f: ARAContentUpdateFlags,
) {
    with(dc, "updateMusicalContextContent", |c, s| {
        editing(c, s, "updateMusicalContextContent");
        if !s.contexts.contains(&(m as usize)) {
            violation(c, "updateMusicalContextContent of an unknown context".into());
        }
    })
}

unsafe extern "C" fn destroy_context(dc: ARADocumentControllerRef, m: ARAMusicalContextRef) {
    with(dc, "destroyMusicalContext", |c, s| {
        editing(c, s, "destroyMusicalContext");
        if !s.seqs.is_empty() {
            violation(c, "destroyMusicalContext while region sequences use it".into());
        }
        s.contexts.retain(|x| *x != m as usize);
    })
}

unsafe extern "C" fn create_sequence(
    dc: ARADocumentControllerRef,
    _h: ARARegionSequenceHostRef,
    p: *const ARARegionSequenceProperties,
) -> ARARegionSequenceRef {
    with(dc, "createRegionSequence", |c, s| {
        editing(c, s, "createRegionSequence");
        // SAFETY: valid for the call.
        let props = unsafe { p.read_unaligned() };
        if props.structSize < kARARegionSequencePropertiesMinSize || !s.contexts.contains(&(props.musicalContextRef as usize)) {
            violation(c, "createRegionSequence with bad properties".into());
        }
        let r = new_ref(s);
        s.seqs.push(r);
        ptr(r)
    })
}

unsafe extern "C" fn update_sequence(dc: ARADocumentControllerRef, q: ARARegionSequenceRef, _p: *const ARARegionSequenceProperties) {
    with(dc, "updateRegionSequenceProperties", |c, s| {
        editing(c, s, "updateRegionSequenceProperties");
        if !s.seqs.contains(&(q as usize)) {
            violation(c, "updateRegionSequenceProperties of an unknown sequence".into());
        }
    })
}

unsafe extern "C" fn destroy_sequence(dc: ARADocumentControllerRef, q: ARARegionSequenceRef) {
    with(dc, "destroyRegionSequence", |c, s| {
        editing(c, s, "destroyRegionSequence");
        if s.regions.values().any(|r| r.sequence == q as usize) {
            violation(c, "destroyRegionSequence with playback regions on it".into());
        }
        s.seqs.retain(|x| *x != q as usize);
    })
}

unsafe extern "C" fn create_source(dc: ARADocumentControllerRef, h: ARAAudioSourceHostRef, p: *const ARAAudioSourceProperties) -> ARAAudioSourceRef {
    with(dc, "createAudioSource", |c, s| {
        editing(c, s, "createAudioSource");
        if c.behaviour == Behaviour::RefusesSources as u8 {
            return std::ptr::null_mut();
        }
        // SAFETY: valid for the call; the strings too.
        let props = unsafe { p.read_unaligned() };
        if props.structSize < kARAAudioSourcePropertiesMinSize || props.channelCount < 1 || props.sampleRate <= 0.0 {
            violation(c, "createAudioSource with bad properties".into());
        }
        // SAFETY: as above.
        let pid = unsafe { cstr(props.persistentID) };
        let r = new_ref(s);
        s.srcs.insert(
            r,
            PSrc { host: h, pid, frames: props.sampleCount, channels: props.channelCount, rate: props.sampleRate, active: true, ..PSrc::default() },
        );
        ptr(r)
    })
}

unsafe extern "C" fn update_source(dc: ARADocumentControllerRef, a: ARAAudioSourceRef, p: *const ARAAudioSourceProperties) {
    with(dc, "updateAudioSourceProperties", |c, s| {
        editing(c, s, "updateAudioSourceProperties");
        // SAFETY: valid for the call.
        let props = unsafe { p.read_unaligned() };
        let Some(src) = s.srcs.get_mut(&(a as usize)) else {
            violation(c, "updateAudioSourceProperties of an unknown source".into());
            return;
        };
        if !src.active {
            violation(c, "updateAudioSourceProperties of a deactivated source".into());
        }
        if src.access && (props.sampleCount != src.frames || props.channelCount != src.channels) {
            violation(c, "the format of an audio source changed while it could be read".into());
        }
        src.frames = props.sampleCount;
        src.channels = props.channelCount;
        src.rate = props.sampleRate;
        src.read = false;
        src.reported = false;
    })
}

unsafe extern "C" fn update_source_content(
    dc: ARADocumentControllerRef,
    a: ARAAudioSourceRef,
    _r: *const ARAContentTimeRange,
    _f: ARAContentUpdateFlags,
) {
    with(dc, "updateAudioSourceContent", |c, s| {
        editing(c, s, "updateAudioSourceContent");
        if let Some(src) = s.srcs.get_mut(&(a as usize)) {
            src.read = false;
            src.reported = false;
        }
    })
}

unsafe extern "C" fn enable_access(dc: ARADocumentControllerRef, a: ARAAudioSourceRef, enable: ARABool) {
    with(dc, "enableAudioSourceSamplesAccess", |c, s| match s.srcs.get_mut(&(a as usize)) {
        Some(src) => src.access = enable != kARAFalse,
        None => violation(c, "enableAudioSourceSamplesAccess of an unknown source".into()),
    })
}

unsafe extern "C" fn deactivate_source(dc: ARADocumentControllerRef, a: ARAAudioSourceRef, deactivate: ARABool) {
    with(dc, "deactivateAudioSourceForUndoHistory", |c, s| {
        editing(c, s, "deactivateAudioSourceForUndoHistory");
        if deactivate != kARAFalse && s.mods.values().any(|m| m.source == a as usize && m.active) {
            violation(c, "an audio source deactivated before its modifications".into());
        }
        match s.srcs.get_mut(&(a as usize)) {
            Some(src) => src.active = deactivate == kARAFalse,
            None => violation(c, "deactivateAudioSourceForUndoHistory of an unknown source".into()),
        }
    })
}

unsafe extern "C" fn destroy_source(dc: ARADocumentControllerRef, a: ARAAudioSourceRef) {
    with(dc, "destroyAudioSource", |c, s| {
        editing(c, s, "destroyAudioSource");
        if s.mods.values().any(|m| m.source == a as usize) {
            violation(c, "destroyAudioSource with modifications left".into());
        }
        if s.srcs.remove(&(a as usize)).is_none() {
            violation(c, "destroyAudioSource of an unknown source".into());
        }
        s.notes.remove(&(a as usize));
    })
}

unsafe extern "C" fn create_modification(
    dc: ARADocumentControllerRef,
    a: ARAAudioSourceRef,
    h: ARAAudioModificationHostRef,
    p: *const ARAAudioModificationProperties,
) -> ARAAudioModificationRef {
    with(dc, "createAudioModification", |c, s| {
        editing(c, s, "createAudioModification");
        if !s.srcs.get(&(a as usize)).is_some_and(|x| x.active) {
            violation(c, "createAudioModification of an unknown or deactivated source".into());
        }
        // SAFETY: valid for the call, the strings too.
        let pid = if p.is_null() { String::new() } else { unsafe { cstr(p.read_unaligned().persistentID) } };
        let r = new_ref(s);
        s.mods.insert(r, PMod { source: a as usize, active: true, host: h, pid, gain: DEFAULT_GAIN, dirty: false });
        ptr(r)
    })
}

unsafe extern "C" fn clone_modification(
    dc: ARADocumentControllerRef,
    m: ARAAudioModificationRef,
    h: ARAAudioModificationHostRef,
    p: *const ARAAudioModificationProperties,
) -> ARAAudioModificationRef {
    with(dc, "cloneAudioModification", |c, s| {
        editing(c, s, "cloneAudioModification");
        let Some((source, gain)) = s.mods.get(&(m as usize)).filter(|x| x.active).map(|x| (x.source, x.gain)) else {
            violation(c, "cloneAudioModification of an unknown or deactivated modification".into());
            return std::ptr::null_mut();
        };
        // SAFETY: valid for the call, the strings too.
        let pid = if p.is_null() { String::new() } else { unsafe { cstr(p.read_unaligned().persistentID) } };
        let r = new_ref(s);
        s.mods.insert(r, PMod { source, active: true, host: h, pid, gain, dirty: false });
        with_report(&c.name, |rep| rep.cloned += 1);
        ptr(r)
    })
}

unsafe extern "C" fn update_modification(dc: ARADocumentControllerRef, m: ARAAudioModificationRef, _p: *const ARAAudioModificationProperties) {
    with(dc, "updateAudioModificationProperties", |c, s| {
        editing(c, s, "updateAudioModificationProperties");
        if !s.mods.get(&(m as usize)).is_some_and(|x| x.active) {
            violation(c, "updateAudioModificationProperties of an unknown or deactivated modification".into());
        }
    })
}

unsafe extern "C" fn deactivate_modification(dc: ARADocumentControllerRef, m: ARAAudioModificationRef, deactivate: ARABool) {
    with(dc, "deactivateAudioModificationForUndoHistory", |c, s| {
        editing(c, s, "deactivateAudioModificationForUndoHistory");
        if deactivate != kARAFalse && s.regions.values().any(|r| r.modification == m as usize) {
            violation(c, "a modification deactivated before its playback regions were destroyed".into());
        }
        let source = s.mods.get(&(m as usize)).map(|x| x.source);
        if deactivate == kARAFalse && source.is_some_and(|src| !s.srcs.get(&src).is_some_and(|x| x.active)) {
            violation(c, "a modification reactivated before its audio source".into());
        }
        match s.mods.get_mut(&(m as usize)) {
            Some(x) => x.active = deactivate == kARAFalse,
            None => violation(c, "deactivateAudioModificationForUndoHistory of an unknown modification".into()),
        }
    })
}

unsafe extern "C" fn destroy_modification(dc: ARADocumentControllerRef, m: ARAAudioModificationRef) {
    with(dc, "destroyAudioModification", |c, s| {
        editing(c, s, "destroyAudioModification");
        if s.regions.values().any(|r| r.modification == m as usize) {
            violation(c, "destroyAudioModification with playback regions left".into());
        }
        if s.mods.remove(&(m as usize)).is_none() {
            violation(c, "destroyAudioModification of an unknown modification".into());
        }
    })
}

fn check_region(c: &Controller, s: &State, m: usize, p: *const ARAPlaybackRegionProperties, call: &str) -> Option<PRegion> {
    // SAFETY: valid for the call.
    let props = unsafe { p.read_unaligned() };
    let seq = props.regionSequenceRef as usize;
    let (d, dp) = (props.durationInModificationTime, props.durationInPlaybackTime);
    let flags = props.transformationFlags;
    let stretch_ok = d == dp || flags & kARAPlaybackTransformationTimestretch != 0;
    if props.structSize < kARAPlaybackRegionPropertiesMinSize
        || !s.seqs.contains(&seq)
        || !(d > 0.0 && dp > 0.0)
        || !stretch_ok
        || flags & !TRANSFORMATIONS != 0
    {
        violation(c, format!("{call} with bad properties"));
        return None;
    }
    Some(PRegion {
        modification: m,
        sequence: seq,
        start_mod: props.startInModificationTime,
        dur_mod: d,
        start_pb: props.startInPlaybackTime,
        dur_pb: dp,
    })
}

unsafe extern "C" fn create_region(
    dc: ARADocumentControllerRef,
    m: ARAAudioModificationRef,
    _h: ARAPlaybackRegionHostRef,
    p: *const ARAPlaybackRegionProperties,
) -> ARAPlaybackRegionRef {
    with(dc, "createPlaybackRegion", |c, s| {
        editing(c, s, "createPlaybackRegion");
        if !s.mods.get(&(m as usize)).is_some_and(|x| x.active) {
            violation(c, "createPlaybackRegion of an unknown or deactivated modification".into());
        }
        let region = check_region(c, s, m as usize, p, "createPlaybackRegion").unwrap_or(PRegion {
            modification: m as usize,
            sequence: 0,
            start_mod: 0.0,
            dur_mod: 0.0,
            start_pb: 0.0,
            dur_pb: 0.0,
        });
        let r = new_ref(s);
        s.regions.insert(r, region);
        ptr(r)
    })
}

unsafe extern "C" fn update_region(dc: ARADocumentControllerRef, r: ARAPlaybackRegionRef, p: *const ARAPlaybackRegionProperties) {
    with(dc, "updatePlaybackRegionProperties", |c, s| {
        editing(c, s, "updatePlaybackRegionProperties");
        let m = s.regions.get(&(r as usize)).map_or(0, |x| x.modification);
        let new = check_region(c, s, m, p, "updatePlaybackRegionProperties");
        match (s.regions.get_mut(&(r as usize)), new) {
            (Some(x), Some(n)) => *x = n,
            (None, _) => violation(c, "updatePlaybackRegionProperties of an unknown region".into()),
            _ => {}
        }
    })
}

unsafe extern "C" fn destroy_region(dc: ARADocumentControllerRef, r: ARAPlaybackRegionRef) {
    with(dc, "destroyPlaybackRegion", |c, s| {
        editing(c, s, "destroyPlaybackRegion");
        if bound_lock().values().any(|b| b.ctrl == c as *const Controller as usize && b.regions.contains(&(r as usize))) {
            violation(c, "destroyPlaybackRegion of a region a bound plug-in instance still renders".into());
        }
        if s.regions.remove(&(r as usize)).is_none() {
            violation(c, "destroyPlaybackRegion of an unknown region".into());
        }
    })
}

unsafe extern "C" fn source_content_available(dc: ARADocumentControllerRef, a: ARAAudioSourceRef, kind: ARAContentType) -> ARABool {
    with(dc, "isAudioSourceContentAvailable", |_, s| {
        if kind == kARAContentTypeNotes && s.notes.contains_key(&(a as usize)) && s.srcs.get(&(a as usize)).is_some_and(|x| x.active) {
            kARATrue
        } else {
            kARAFalse
        }
    })
}

unsafe extern "C" fn source_analysis_incomplete(_dc: ARADocumentControllerRef, _a: ARAAudioSourceRef, _k: ARAContentType) -> ARABool {
    kARAFalse
}

unsafe extern "C" fn source_grade(_dc: ARADocumentControllerRef, _a: ARAAudioSourceRef, _k: ARAContentType) -> ARAContentGrade {
    kARAContentGradeDetected
}

unsafe extern "C" fn source_reader(
    dc: ARADocumentControllerRef,
    a: ARAAudioSourceRef,
    kind: ARAContentType,
    _r: *const ARAContentTimeRange,
) -> ARAContentReaderRef {
    with(dc, "createAudioSourceContentReader", |c, s| {
        let Some(notes) = s.notes.get(&(a as usize)).filter(|_| kind == kARAContentTypeNotes).cloned() else {
            violation(c, "createAudioSourceContentReader for content that is not available".into());
            return std::ptr::null_mut();
        };
        let r = new_ref(s);
        s.readers.insert(r, notes);
        ptr(r)
    })
}

unsafe extern "C" fn reader_count(dc: ARADocumentControllerRef, r: ARAContentReaderRef) -> ARAInt32 {
    with(dc, "getContentReaderEventCount", |_, s| s.readers.get(&(r as usize)).map_or(0, |n| n.len() as i32))
}

unsafe extern "C" fn reader_event(dc: ARADocumentControllerRef, r: ARAContentReaderRef, i: ARAInt32) -> *const c_void {
    with(dc, "getContentReaderDataForEvent", |_, s| {
        // The notes stay in the table (their heap buffer does not move) until the reader is gone.
        Some(
            s.readers
                .get(&(r as usize))
                .and_then(|n| n.get(usize::try_from(i).ok()?))
                .map_or(std::ptr::null(), |n| (n as *const ARAContentNote).cast()),
        )
    })
    .unwrap_or(std::ptr::null())
}

unsafe extern "C" fn destroy_content_reader(dc: ARADocumentControllerRef, r: ARAContentReaderRef) {
    with(dc, "destroyContentReader", |c, s| {
        if s.readers.remove(&(r as usize)).is_none() {
            violation(c, "destroyContentReader of an unknown reader".into());
        }
    })
}

const fn iface(size: usize, missing: bool) -> ARADocumentControllerInterface {
    ARADocumentControllerInterface {
        structSize: size,
        destroyDocumentController: Some(destroy),
        getFactory: Some(get_factory),
        beginEditing: Some(begin_editing),
        endEditing: Some(end_editing),
        notifyModelUpdates: Some(notify_model_updates),
        beginRestoringDocumentFromArchive: None,
        endRestoringDocumentFromArchive: None,
        storeDocumentToArchive: None,
        updateDocumentProperties: Some(update_document),
        createMusicalContext: Some(create_context),
        updateMusicalContextProperties: None,
        updateMusicalContextContent: Some(update_context_content),
        destroyMusicalContext: Some(destroy_context),
        createAudioSource: Some(create_source),
        updateAudioSourceProperties: Some(update_source),
        updateAudioSourceContent: Some(update_source_content),
        enableAudioSourceSamplesAccess: Some(enable_access),
        deactivateAudioSourceForUndoHistory: Some(deactivate_source),
        destroyAudioSource: Some(destroy_source),
        createAudioModification: Some(create_modification),
        cloneAudioModification: Some(clone_modification),
        updateAudioModificationProperties: Some(update_modification),
        deactivateAudioModificationForUndoHistory: Some(deactivate_modification),
        destroyAudioModification: Some(destroy_modification),
        createPlaybackRegion: if missing { None } else { Some(create_region) },
        updatePlaybackRegionProperties: Some(update_region),
        destroyPlaybackRegion: Some(destroy_region),
        isAudioSourceContentAvailable: Some(source_content_available),
        isAudioSourceContentAnalysisIncomplete: Some(source_analysis_incomplete),
        requestAudioSourceContentAnalysis: None,
        getAudioSourceContentGrade: Some(source_grade),
        createAudioSourceContentReader: Some(source_reader),
        isAudioModificationContentAvailable: None,
        getAudioModificationContentGrade: None,
        createAudioModificationContentReader: None,
        isPlaybackRegionContentAvailable: None,
        getPlaybackRegionContentGrade: None,
        createPlaybackRegionContentReader: None,
        getContentReaderEventCount: Some(reader_count),
        getContentReaderDataForEvent: Some(reader_event),
        destroyContentReader: Some(destroy_content_reader),
        createRegionSequence: Some(create_sequence),
        updateRegionSequenceProperties: Some(update_sequence),
        destroyRegionSequence: Some(destroy_sequence),
        getPlaybackRegionHeadAndTailTime: Some(head_tail),
        restoreObjectsFromArchive: Some(restore_objects),
        storeObjectsToArchive: Some(store_objects),
        getProcessingAlgorithmsCount: None,
        getProcessingAlgorithmProperties: None,
        getProcessingAlgorithmForAudioSource: None,
        requestProcessingAlgorithmForAudioSource: None,
        isLicensedForCapabilities: None,
        storeAudioSourceToAudioFileChunk: None,
        isAudioModificationPreservingAudioSourceSignal: None,
    }
}

static IFACE: ARADocumentControllerInterface = iface(std::mem::size_of::<ARADocumentControllerInterface>(), false);
static IFACE_TINY: ARADocumentControllerInterface = iface(16, false);
static IFACE_MISSING: ARADocumentControllerInterface = iface(std::mem::size_of::<ARADocumentControllerInterface>(), true);

// ---- reading through the host -------------------------------------------------------------------

type ReadFn =
    unsafe extern "C" fn(ARAAudioAccessControllerHostRef, ARAAudioReaderHostRef, ARASamplePosition, ARASampleCount, *const *mut c_void) -> ARABool;

/// The host's `readAudioSamples` and controller ref, sendable to reader threads.
#[derive(Clone, Copy)]
struct Access {
    read: ReadFn,
    ctrl: usize,
}

impl Access {
    fn of(c: &Controller) -> Option<Access> {
        Some(Access { read: { c.audio.readAudioSamples }?, ctrl: c.host.audioAccessControllerHostRef as usize })
    }
}

/// Reads a whole source with one reader (in chunks of 997 frames, and 10 frames past the end):
/// the samples per channel, and whether the frames past the end were silent. `None` when the
/// host refused the reader or a read.
fn read_all(a: Access, reader: usize, frames: i64, channels: usize, use64: bool) -> Option<(Vec<Vec<f64>>, bool)> {
    let (read, ctrl, reader) = (a.read, std::ptr::without_provenance_mut::<c_void>(a.ctrl), std::ptr::without_provenance_mut::<c_void>(reader));
    let mut out = vec![Vec::new(); channels];
    let mut silent_tail = true;
    let mut pos = 0i64;
    let end = frames + 10;
    while pos < end {
        let n = 997.min(end - pos);
        let mut b32 = vec![vec![9.0f32; n as usize]; channels];
        let mut b64 = vec![vec![9.0f64; n as usize]; channels];
        let ptrs: Vec<*mut c_void> =
            if use64 { b64.iter_mut().map(|b| b.as_mut_ptr().cast()).collect() } else { b32.iter_mut().map(|b| b.as_mut_ptr().cast()).collect() };
        // SAFETY: the host's function and refs; one buffer of `n` samples per channel.
        if unsafe { read(ctrl, reader, pos, n, ptrs.as_ptr()) } == kARAFalse {
            return None;
        }
        for (ch, o) in out.iter_mut().enumerate() {
            let vals: Vec<f64> = if use64 { b64[ch].clone() } else { b32[ch].iter().map(|x| f64::from(*x)).collect() };
            for (i, v) in vals.into_iter().enumerate() {
                if pos + (i as i64) < frames {
                    o.push(v);
                } else if v != 0.0 {
                    silent_tail = false;
                }
            }
        }
        pos += n;
    }
    Some((out, silent_tail))
}

fn reader(c: &Controller, src: &PSrc, use64: bool) -> ARAAudioReaderHostRef {
    let create = { c.audio.createAudioReaderForSource };
    match create {
        // SAFETY: the host's function, called from endEditing as ARA allows.
        Some(f) => unsafe { f(c.host.audioAccessControllerHostRef, src.host, if use64 { kARATrue } else { kARAFalse }) },
        None => std::ptr::null_mut(),
    }
}

fn destroy_reader(c: &Controller, r: ARAAudioReaderHostRef) {
    if let Some(f) = { c.audio.destroyAudioReader } {
        // SAFETY: the host's function, on a reader it gave and no thread uses any more.
        unsafe { f(c.host.audioAccessControllerHostRef, r) };
    }
}

fn read_sources(c: &Controller, s: &mut State) {
    let keys: Vec<usize> = s.srcs.iter().filter(|(_, x)| x.access && x.active && !x.read).map(|(k, _)| *k).collect();
    for k in keys {
        let Some(src) = s.srcs.get(&k) else { continue };
        let channels = usize::try_from(src.channels).unwrap_or(0);
        let mut sums = [0u64; 2];
        let mut frames_read = 0;
        let mut silent = true;
        let mut audio = Vec::new();
        for (i, use64) in [false, true].into_iter().enumerate() {
            let r = reader(c, src, use64);
            if r.is_null() {
                violation(c, format!("the host gave no audio reader for {}", src.pid));
                return;
            }
            let got = match Access::of(c) {
                Some(a) if c.behaviour == Behaviour::Concurrent as u8 => read_concurrently(c, a, src, r, channels, use64),
                Some(a) => read_all(a, r as usize, src.frames, channels, use64),
                None => None,
            };
            destroy_reader(c, r);
            let Some((samples, tail)) = got else {
                violation(c, format!("the host refused to read {}", src.pid));
                return;
            };
            sums[i] = checksum(&samples);
            frames_read = samples.first().map_or(0, |x| x.len() as i64);
            silent &= tail;
            if !use64 {
                audio = samples.iter().map(|ch| ch.iter().map(|x| *x as f32).collect()).collect();
            }
        }
        let pid = src.pid.clone();
        let note = |sum: u64, volume: f32| ARAContentNote {
            frequency: kARADefaultConcertPitchFrequency,
            pitchNumber: channels as i32,
            volume,
            startPosition: (sum >> 11) as f64,
            attackDuration: 0.0,
            noteDuration: frames_read as f64,
            signalDuration: frames_read as f64,
        };
        s.notes.insert(k, vec![note(sums[0], 0.5), note(sums[1], 1.0)]);
        if let Some(src) = s.srcs.get_mut(&k) {
            src.read = true;
            src.audio = std::sync::Arc::new(std::mem::take(&mut audio));
        }
        with_report(&c.name, |r| {
            r.read.insert(pid, (sums[0], sums[1], frames_read, silent));
        });
    }
}

/// Reads with the given reader and, at the same time, three more readers on other threads; all
/// four must read the same. Returns the given reader's result.
fn read_concurrently(c: &Controller, a: Access, src: &PSrc, r: ARAAudioReaderHostRef, channels: usize, use64: bool) -> Option<(Vec<Vec<f64>>, bool)> {
    let others: Vec<usize> = (0..3).map(|_| reader(c, src, use64) as usize).collect();
    let frames = src.frames;
    let results: Vec<Option<(Vec<Vec<f64>>, bool)>> = std::thread::scope(|scope| {
        let handles: Vec<_> = others.iter().map(|&o| scope.spawn(move || read_all(a, o, frames, channels, use64))).collect();
        let mine = read_all(a, r as usize, frames, channels, use64);
        let mut all = vec![mine];
        all.extend(handles.into_iter().map(|h| h.join().ok().flatten()));
        all
    });
    for o in others {
        destroy_reader(c, std::ptr::without_provenance_mut(o));
    }
    let first = results.first().cloned().flatten();
    if results.iter().any(|x| *x != first) {
        violation(c, "concurrent readers read different audio".into());
    }
    first
}

/// The tempo entries the host offers (`None`: not available).
fn tempo_entries(c: &Controller) -> Option<i32> {
    let ctx = std::ptr::without_provenance_mut::<c_void>(1);
    let r = c.host.contentAccessControllerHostRef;
    let available = { c.content.isMusicalContextContentAvailable }?;
    // SAFETY: the host's function and refs (the host's musical context ref is 1 in this host; a
    // wrong guess is answered with "not available").
    if unsafe { available(r, ctx, kARAContentTypeTempoEntries) } == kARAFalse {
        return None;
    }
    let create = { c.content.createMusicalContextContentReader }?;
    // SAFETY: as above; a null range reads everything.
    let reader = unsafe { create(r, ctx, kARAContentTypeTempoEntries, std::ptr::null()) };
    if reader.is_null() {
        return None;
    }
    // SAFETY: as above, on the reader it gave.
    let n = { c.content.getContentReaderEventCount }.map(|f| unsafe { f(r, reader) });
    if let Some(f) = { c.content.destroyContentReader } {
        // SAFETY: as above.
        unsafe { f(r, reader) };
    }
    n
}

/// The hostile calls: each must be answered with a refusal, never a crash.
fn hostile(c: &Controller, s: &State) {
    let ctrl = c.host.audioAccessControllerHostRef;
    let bogus = std::ptr::without_provenance_mut::<c_void>(0xDEAD);
    let mut seen = Vec::new();
    if let (Some(create), Some(read), Some(destroy)) =
        ({ c.audio.createAudioReaderForSource }, { c.audio.readAudioSamples }, { c.audio.destroyAudioReader })
    {
        // SAFETY (every call below): the host's functions; the arguments are wrong on purpose and
        // the host must refuse them; the buffers passed are valid where any are passed.
        unsafe {
            seen.push(("reader for an unknown source".to_string(), create(ctrl, bogus, kARAFalse).is_null()));
            seen.push(("reader on an unknown controller".to_string(), create(bogus, bogus, kARAFalse).is_null()));
            let mut buf = vec![1.0f32; 16];
            let ptrs = [buf.as_mut_ptr().cast::<c_void>(); 8];
            seen.push(("read from an unknown reader".to_string(), read(ctrl, bogus, 0, 16, ptrs.as_ptr()) == kARAFalse));
            if let Some(src) = s.srcs.values().find(|x| x.access && x.active && (1..=8).contains(&x.channels)) {
                let r = create(ctrl, src.host, kARAFalse);
                seen.push(("a reader for a real source".to_string(), !r.is_null()));
                seen.push(("read into null buffers".to_string(), read(ctrl, r, 0, 16, std::ptr::null()) == kARAFalse));
                seen.push(("read a negative count".to_string(), read(ctrl, r, 0, -5, ptrs.as_ptr()) == kARAFalse));
                seen.push(("read an absurd count".to_string(), read(ctrl, r, 0, i64::MAX, ptrs.as_ptr()) == kARAFalse));
                let nulls = [std::ptr::null_mut::<c_void>(); 8];
                seen.push(("read into a null channel".to_string(), read(ctrl, r, 0, 16, nulls.as_ptr()) == kARAFalse));
                seen.push((
                    "read far before the start".to_string(),
                    read(ctrl, r, i64::MIN, 16, ptrs.as_ptr()) != kARAFalse && buf.iter().all(|x| *x == 0.0),
                ));
                destroy(ctrl, r);
                destroy(ctrl, r);
                seen.push(("read after destroy".to_string(), read(ctrl, r, 0, 16, ptrs.as_ptr()) == kARAFalse));
            }
            destroy(bogus, bogus);
        }
    }
    if let Some(f) = { c.model.notifyAudioSourceAnalysisProgress } {
        // SAFETY: as above: unknown refs must be ignored.
        unsafe { f(bogus, bogus, kARAAnalysisProgressStarted, f32::NAN) };
    }
    if let Some(f) = { c.content.getContentReaderDataForEvent } {
        // SAFETY: as above.
        seen.push(("event of an unknown content reader".to_string(), unsafe { f(c.host.contentAccessControllerHostRef, bogus, 0) }.is_null()));
    }
    with_report(&c.name, |r| r.hostile.extend(seen));
}

// ---- companion plug-in instances (the VST3 fixture, the engine's tests) --------------------------

struct SyncExt(ARAPlugInExtensionInstance);
// SAFETY: immutable after creation (function pointers and numbers).
unsafe impl Sync for SyncExt {}
unsafe impl Send for SyncExt {}

/// One bound instance: its controller (address), document name and the regions its playback
/// renderer was given.
struct Bound {
    ctrl: usize,
    name: String,
    regions: Vec<usize>,
    /// It is rendering now ([`render`] running).
    rendering: bool,
}

/// Bound instances by renderer id (the playback renderer, editor renderer and editor view refs
/// of one instance are the same number).
fn bound() -> &'static Mutex<HashMap<usize, Bound>> {
    static B: std::sync::OnceLock<Mutex<HashMap<usize, Bound>>> = std::sync::OnceLock::new();
    B.get_or_init(|| Mutex::new(HashMap::new()))
}

fn bound_lock() -> std::sync::MutexGuard<'static, HashMap<usize, Bound>> {
    bound().lock().unwrap_or_else(PoisonError::into_inner)
}

fn renderer_call(renderer: ARAPlaybackRendererRef, f: impl FnOnce(&mut Bound)) {
    let mut b = bound_lock();
    let Some(x) = b.get_mut(&(renderer as usize)) else { return };
    if x.rendering {
        let name = x.name.clone();
        with_report(&name, |r| r.renderer_while_rendering += 1);
    }
    f(x);
}

unsafe extern "C" fn add_region(renderer: ARAPlaybackRendererRef, region: ARAPlaybackRegionRef) {
    renderer_call(renderer, |b| {
        if !b.regions.contains(&(region as usize)) {
            b.regions.push(region as usize);
            with_report(&b.name, |r| r.rendered += 1);
        }
    });
}

unsafe extern "C" fn remove_region(renderer: ARAPlaybackRendererRef, region: ARAPlaybackRegionRef) {
    renderer_call(renderer, |b| {
        if let Some(i) = b.regions.iter().position(|x| *x == region as usize) {
            b.regions.remove(i);
            with_report(&b.name, |r| r.rendered = r.rendered.saturating_sub(1));
        }
    });
}

static RENDERER: ARAPlaybackRendererInterface = ARAPlaybackRendererInterface {
    structSize: std::mem::size_of::<ARAPlaybackRendererInterface>(),
    addPlaybackRegion: Some(add_region),
    removePlaybackRegion: Some(remove_region),
};

unsafe extern "C" fn editor_add(_r: ARAEditorRendererRef, _p: ARAPlaybackRegionRef) {}
unsafe extern "C" fn editor_remove(_r: ARAEditorRendererRef, _p: ARAPlaybackRegionRef) {}
unsafe extern "C" fn editor_add_seq(_r: ARAEditorRendererRef, _q: ARARegionSequenceRef) {}
unsafe extern "C" fn editor_remove_seq(_r: ARAEditorRendererRef, _q: ARARegionSequenceRef) {}

static EDITOR_RENDERER: ARAEditorRendererInterface = ARAEditorRendererInterface {
    structSize: std::mem::size_of::<ARAEditorRendererInterface>(),
    addPlaybackRegion: Some(editor_add),
    removePlaybackRegion: Some(editor_remove),
    addRegionSequence: Some(editor_add_seq),
    removeRegionSequence: Some(editor_remove_seq),
};

unsafe extern "C" fn notify_selection(view: ARAEditorViewRef, sel: *const ARAViewSelection) {
    let Some(name) = bound_lock().get(&(view as usize)).map(|b| b.name.clone()) else { return };
    if sel.is_null() {
        return;
    }
    // SAFETY: the host's selection, valid for the call; the time range too when given.
    let s = unsafe { sel.read_unaligned() };
    let range = if s.timeRange.is_null() {
        None
    } else {
        // SAFETY: as above.
        let r = unsafe { s.timeRange.read_unaligned() };
        Some((r.start, r.duration))
    };
    with_report(&name, |r| r.selection = Some((s.playbackRegionRefsCount, s.regionSequenceRefsCount, range)));
}

unsafe extern "C" fn notify_hide(view: ARAEditorViewRef, count: ARASize, _refs: *const ARARegionSequenceRef) {
    let Some(name) = bound_lock().get(&(view as usize)).map(|b| b.name.clone()) else { return };
    with_report(&name, |r| r.hidden = count);
}

static EDITOR_VIEW: ARAEditorViewInterface = ARAEditorViewInterface {
    structSize: std::mem::size_of::<ARAEditorViewInterface>(),
    notifySelection: Some(notify_selection),
    notifyHideRegionSequences: Some(notify_hide),
};

/// Binds a companion plug-in instance to the document controller `dc` with the given roles
/// (`IPlugInEntryPoint2::bindToDocumentControllerWithRoles`): the instance's extension, with the
/// interfaces of the roles assigned (none for a playback renderer with [`Behaviour::NoRenderer`]).
/// Null for a controller that is not live. The returned value is the instance's renderer id as
/// [`render`] and [`unbind`] take it. Leaks one small struct per call (test code).
pub fn bind(
    dc: ARADocumentControllerRef,
    known: ARAPlugInInstanceRoleFlags,
    assigned: ARAPlugInInstanceRoleFlags,
) -> *const ARAPlugInExtensionInstance {
    let got = with(dc, "bindToDocumentControllerWithRoles", |c, _| {
        if assigned & !known != 0 {
            violation(c, "assigned roles that are not known".into());
        }
        Some((c.name.clone(), c.behaviour))
    });
    let Some((name, behaviour)) = got else { return std::ptr::null() };
    static NEXT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(1);
    let id = NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    bound_lock().insert(id, Bound { ctrl: dc as usize, name, regions: Vec::new(), rendering: false });
    let playback = assigned & kARAPlaybackRendererRole != 0 && behaviour != Behaviour::NoRenderer as u8;
    let editor = assigned & kARAEditorRendererRole != 0;
    let view = assigned & kARAEditorViewRole != 0;
    let ext = Box::leak(Box::new(SyncExt(ARAPlugInExtensionInstance {
        structSize: std::mem::size_of::<ARAPlugInExtensionInstance>(),
        plugInExtensionRef: std::ptr::null_mut(),
        plugInExtensionInterface: std::ptr::null(),
        playbackRendererRef: if playback { ptr(id) } else { std::ptr::null_mut() },
        playbackRendererInterface: if playback { &RENDERER } else { std::ptr::null() },
        editorRendererRef: if editor { ptr(id) } else { std::ptr::null_mut() },
        editorRendererInterface: if editor { &EDITOR_RENDERER } else { std::ptr::null() },
        editorViewRef: if view { ptr(id) } else { std::ptr::null_mut() },
        editorViewInterface: if view { &EDITOR_VIEW } else { std::ptr::null() },
    })));
    &ext.0
}

/// [`bind`], for a host that keeps the extension with a [`crate::BoundInstance`]: the renderer id
/// and the extension.
pub fn bind_raw(
    dc: ARADocumentControllerRef,
    known: ARAPlugInInstanceRoleFlags,
    assigned: ARAPlugInInstanceRoleFlags,
) -> (usize, crate::RawExtension) {
    let ext = bind(dc, known, assigned);
    // SAFETY: the test plug-in's extensions are leaked: valid for the life of the process.
    (renderer_id(ext), unsafe { crate::RawExtension::new(ext) })
}

/// [`bind`], with the renderer id (as [`render`] and [`unbind`] take it; 0 when it gave nothing).
pub fn bind_id(
    dc: ARADocumentControllerRef,
    known: ARAPlugInInstanceRoleFlags,
    assigned: ARAPlugInInstanceRoleFlags,
) -> (*const ARAPlugInExtensionInstance, usize) {
    let ext = bind(dc, known, assigned);
    (ext, renderer_id(ext))
}

/// The renderer id of an extension [`bind`] returned (0 for null).
fn renderer_id(ext: *const ARAPlugInExtensionInstance) -> usize {
    if ext.is_null() {
        return 0;
    }
    // SAFETY: an extension `bind` made (leaked, so alive); read unaligned.
    let e = unsafe { ext.read_unaligned() };
    [e.playbackRendererRef, e.editorRendererRef, e.editorViewRef].into_iter().find(|p| !p.is_null()).map_or(0, |p| p as usize)
}

/// The instance went (its VST3 component was destroyed): it renders nothing any more.
pub fn unbind(renderer: usize) {
    bound_lock().remove(&renderer);
}

/// Renders the playback regions given to instance `renderer` for the block starting at song
/// sample `position` (at `rate` Hz) into `out` (one slice per output channel, all as long): each
/// region plays its modification's source, read through the host, times the modification's
/// gain (from [`DEFAULT_GAIN`], changed by [`set_gain`] or a restored archive), mapped through a
/// time-stretch linearly, nearest sample; output channel `c` plays source channel `c` (the last one
/// past its channels). Everything else is silence. `false` (and silence) for an unknown instance;
/// after [`set_fault`], NaN.
pub fn render(renderer: usize, position: i64, rate: f64, out: &mut [&mut [f32]]) -> bool {
    for ch in out.iter_mut() {
        ch.iter_mut().for_each(|x| *x = 0.0);
    }
    let (ctrl, regions) = {
        let mut b = bound_lock();
        let Some(x) = b.get_mut(&renderer) else { return false };
        x.rendering = true;
        (x.ctrl, x.regions.clone())
    };
    let ok = live().lock().unwrap_or_else(PoisonError::into_inner).contains(&ctrl);
    if ok && rate > 0.0 {
        // SAFETY: a live controller (checked above); see `with`.
        let c = unsafe { &*(ctrl as *const Controller) };
        let s = c.state.lock().unwrap_or_else(PoisonError::into_inner);
        let frames = out.first().map_or(0, |c| c.len());
        if s.fault {
            for ch in out.iter_mut() {
                ch.iter_mut().for_each(|x| *x = f32::NAN);
            }
        } else {
            for reg in regions.iter().filter_map(|r| s.regions.get(r)) {
                let Some(m) = s.mods.get(&reg.modification).filter(|m| m.active) else { continue };
                let Some(src) = s.srcs.get(&m.source).filter(|x| x.active) else { continue };
                let s0 = (reg.start_pb * rate).round() as i64;
                let len = (reg.dur_pb * rate).round() as i64;
                let src_rate = if src.rate > 0.0 { src.rate } else { rate };
                let m0 = (reg.start_mod * src_rate).round() as i64;
                let mlen = reg.dur_mod * src_rate;
                for k in 0..frames {
                    let p = position + k as i64;
                    if p < s0 || p >= s0 + len || len <= 0 {
                        continue;
                    }
                    let rel = p - s0;
                    let at = m0 + if (mlen - len as f64).abs() < 1e-6 { rel } else { (rel as f64 * mlen / len as f64).round() as i64 };
                    for (c, ch) in out.iter_mut().enumerate() {
                        let n = src.audio.len();
                        let Some(sch) = src.audio.get(c.min(n.saturating_sub(1))) else { continue };
                        let v = usize::try_from(at).ok().and_then(|i| sch.get(i)).copied().unwrap_or(0.0);
                        if let Some(o) = ch.get_mut(k) {
                            *o += v * m.gain;
                        }
                    }
                }
            }
        }
    }
    if let Some(x) = bound_lock().get_mut(&renderer) {
        x.rendering = false;
    }
    ok
}

fn with_named(doc: &str, f: impl FnOnce(&Controller, &mut State)) -> bool {
    let addrs: Vec<usize> = live().lock().unwrap_or_else(PoisonError::into_inner).clone();
    for a in addrs {
        // SAFETY: a live controller; see `with`.
        let c = unsafe { &*(a as *const Controller) };
        if c.name == doc {
            let mut s = c.state.lock().unwrap_or_else(PoisonError::into_inner);
            f(c, &mut s);
            return true;
        }
    }
    false
}

/// An edit in the plug-in (as if the user changed something in its editor): the gain of the
/// modification with persistent id `pid` in the document named `doc`. The plug-in tells the host
/// the modification's content and its document data changed at its next `notifyModelUpdates`.
pub fn set_gain(doc: &str, pid: &str, gain: f32) -> bool {
    let mut found = false;
    with_named(doc, |_, s| {
        for m in s.mods.values_mut().filter(|m| m.pid == pid) {
            m.gain = gain;
            m.dirty = true;
            found = true;
        }
        s.data_dirty |= found;
    });
    found
}

/// The gain of a modification (tests), `None` if there is none with that persistent id.
pub fn gain(doc: &str, pid: &str) -> Option<f32> {
    let mut out = None;
    with_named(doc, |_, s| out = s.mods.values().find(|m| m.pid == pid).map(|m| m.gain));
    out
}

/// From now on every render of document `doc` gives NaN (a plug-in gone wrong).
pub fn set_fault(doc: &str, fault: bool) -> bool {
    with_named(doc, |_, s| s.fault = fault)
}

/// The plug-in asks the host to start playback, then to locate to `at` seconds (its playback
/// controller), as a plug-in's transport buttons would.
pub fn request_playback(doc: &str, at: f64) -> bool {
    with_named(doc, |c, _| {
        let h = c.host.playbackControllerHostRef;
        // SAFETY: the host's functions and ref (valid while the controller lives).
        unsafe {
            if let Some(f) = { c.playback.requestSetPlaybackPosition } {
                f(h, at);
            }
            if let Some(f) = { c.playback.requestStartPlayback } {
                f(h);
            }
        }
    })
}

//! Test-only ARA VST3 plug-in for soundcraft-vst3-host. Its classes, by name:
//!
//! - "Fixture ARA" (`Fx`): an audio module (stereo pass-through) offering `ARA::IPlugInEntryPoint`
//!   and `ARA::IPlugInEntryPoint2`, and an "ARA Main Factory Class" of the same name whose
//!   `ARA::IMainFactory::getFactory` gives soundcraft-ara-host's test plug-in factory: a document
//!   controller that reads every audio source through the host and reports checksums of what it
//!   read as notes content.
//! - "Fixture ARA Null": the same, but its main factory gives a null factory.
//! - "Fixture ARA Tiny": its main factory gives a factory whose `structSize` is 16 bytes.
//! - "Fixture ARA One": an ARA 1 plug-in (generations 1 to 2) whose audio module has only the ARA 1
//!   entry point.
//! - "Fixture Plain": an audio module without ARA.
//!
//! Test code: panics are fine here; the host must survive anything anyway.

#![allow(non_snake_case, clippy::missing_safety_doc)]

use soundcraft_ara_host::sys::{self as ara, ARADocumentControllerRef, ARAFactory, ARAPlugInExtensionInstance, ARAPlugInInstanceRoleFlags};
use soundcraft_ara_host::test_plugin::{self, Behaviour};
use std::ffi::{c_char, c_void};
use vst3::Steinberg::Vst::*;
use vst3::Steinberg::*;
use vst3::com_scrape_types::{Guid, Header, Inherits, Unknown, Wrapper};
use vst3::{Class, ComWrapper, Interface, uid};

// ---- ARA's VST3 interfaces, as vst3-rs declares interfaces ---------------------------------------

unsafe fn raw_query(this: *mut c_void, iid: &Guid) -> Option<*mut c_void> {
    let vtbl = unsafe { *(this as *const *const FUnknownVtbl) };
    let mut obj = std::ptr::null_mut();
    let r = unsafe { ((*vtbl).queryInterface)(this as *mut FUnknown, iid.as_ptr() as *const TUID, &mut obj) };
    if r == kResultOk { Some(obj) } else { None }
}

unsafe fn raw_add_ref(this: *mut c_void) -> usize {
    let vtbl = unsafe { *(this as *const *const FUnknownVtbl) };
    unsafe { ((*vtbl).addRef)(this as *mut FUnknown) as usize }
}

unsafe fn raw_release(this: *mut c_void) -> usize {
    let vtbl = unsafe { *(this as *const *const FUnknownVtbl) };
    unsafe { ((*vtbl).release)(this as *mut FUnknown) as usize }
}

macro_rules! interface {
    ($name:ident, $vtbl:ident, $words:expr) => {
        #[repr(C)]
        #[derive(Copy, Clone)]
        pub struct $name {
            pub vtbl: *const $vtbl,
        }
        unsafe impl Send for $name {}
        unsafe impl Sync for $name {}
        unsafe impl Inherits<FUnknown> for $name {}
        impl Unknown for $name {
            unsafe fn query_interface(this: *mut Self, iid: &Guid) -> Option<*mut c_void> {
                unsafe { raw_query(this.cast(), iid) }
            }
            unsafe fn add_ref(this: *mut Self) -> usize {
                unsafe { raw_add_ref(this.cast()) }
            }
            unsafe fn release(this: *mut Self) -> usize {
                unsafe { raw_release(this.cast()) }
            }
        }
        unsafe impl Interface for $name {
            type Vtbl = $vtbl;
            const IID: Guid = ara::vst3_tuid($words);
            fn inherits(iid: &Guid) -> bool {
                iid == &Self::IID || FUnknown::inherits(iid)
            }
        }
    };
}

interface!(IMainFactory, IMainFactoryVtbl, ara::IMainFactory_iid);
interface!(IPlugInEntryPoint, IPlugInEntryPointVtbl, ara::IPlugInEntryPoint_iid);
interface!(IPlugInEntryPoint2, IPlugInEntryPoint2Vtbl, ara::IPlugInEntryPoint2_iid);

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IMainFactoryVtbl {
    pub base: FUnknownVtbl,
    pub getFactory: unsafe extern "system" fn(this: *mut IMainFactory) -> *const ARAFactory,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IPlugInEntryPointVtbl {
    pub base: FUnknownVtbl,
    pub getFactory: unsafe extern "system" fn(this: *mut IPlugInEntryPoint) -> *const ARAFactory,
    pub bindToDocumentController:
        unsafe extern "system" fn(this: *mut IPlugInEntryPoint, dc: ARADocumentControllerRef) -> *const ARAPlugInExtensionInstance,
}

#[repr(C)]
#[derive(Copy, Clone)]
pub struct IPlugInEntryPoint2Vtbl {
    pub base: FUnknownVtbl,
    pub bindToDocumentControllerWithRoles: unsafe extern "system" fn(
        this: *mut IPlugInEntryPoint2,
        dc: ARADocumentControllerRef,
        known: ARAPlugInInstanceRoleFlags,
        assigned: ARAPlugInInstanceRoleFlags,
    ) -> *const ARAPlugInExtensionInstance,
}

pub trait IMainFactoryTrait {
    unsafe fn getFactory(&self) -> *const ARAFactory;
}
pub trait IPlugInEntryPointTrait {
    unsafe fn getFactory(&self) -> *const ARAFactory;
    unsafe fn bindToDocumentController(&self, dc: ARADocumentControllerRef) -> *const ARAPlugInExtensionInstance;
}
pub trait IPlugInEntryPoint2Trait {
    unsafe fn bindToDocumentControllerWithRoles(
        &self,
        dc: ARADocumentControllerRef,
        known: ARAPlugInInstanceRoleFlags,
        assigned: ARAPlugInInstanceRoleFlags,
    ) -> *const ARAPlugInExtensionInstance;
}

unsafe fn data<C: Class, W: Wrapper<C>, const OFFSET: isize, I>(this: *mut I) -> *mut C {
    let header_ptr = unsafe { (this as *mut u8).offset(-OFFSET) } as *mut Header<C>;
    unsafe { W::data_from_header(header_ptr) }
}

impl IMainFactory {
    const fn make_vtbl<C: IMainFactoryTrait + Class, W: Wrapper<C>, const OFFSET: isize>() -> IMainFactoryVtbl {
        unsafe extern "system" fn get<C: IMainFactoryTrait + Class, W: Wrapper<C>, const OFFSET: isize>(
            this: *mut IMainFactory,
        ) -> *const ARAFactory {
            unsafe { (*data::<C, W, OFFSET, _>(this)).getFactory() }
        }
        IMainFactoryVtbl { base: FUnknown::make_vtbl::<C, W, OFFSET>(), getFactory: get::<C, W, OFFSET> }
    }
}
unsafe impl<C: IMainFactoryTrait + Class, W: Wrapper<C>, const OFFSET: isize> vst3::com_scrape_types::Construct<C, W, OFFSET> for IMainFactory {
    const OBJ: Self = IMainFactory { vtbl: &Self::make_vtbl::<C, W, OFFSET>() };
}

impl IPlugInEntryPoint {
    const fn make_vtbl<C: IPlugInEntryPointTrait + Class, W: Wrapper<C>, const OFFSET: isize>() -> IPlugInEntryPointVtbl {
        unsafe extern "system" fn get<C: IPlugInEntryPointTrait + Class, W: Wrapper<C>, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint,
        ) -> *const ARAFactory {
            unsafe { (*data::<C, W, OFFSET, _>(this)).getFactory() }
        }
        unsafe extern "system" fn bind<C: IPlugInEntryPointTrait + Class, W: Wrapper<C>, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint,
            dc: ARADocumentControllerRef,
        ) -> *const ARAPlugInExtensionInstance {
            unsafe { (*data::<C, W, OFFSET, _>(this)).bindToDocumentController(dc) }
        }
        IPlugInEntryPointVtbl {
            base: FUnknown::make_vtbl::<C, W, OFFSET>(),
            getFactory: get::<C, W, OFFSET>,
            bindToDocumentController: bind::<C, W, OFFSET>,
        }
    }
}
unsafe impl<C: IPlugInEntryPointTrait + Class, W: Wrapper<C>, const OFFSET: isize> vst3::com_scrape_types::Construct<C, W, OFFSET>
    for IPlugInEntryPoint
{
    const OBJ: Self = IPlugInEntryPoint { vtbl: &Self::make_vtbl::<C, W, OFFSET>() };
}

impl IPlugInEntryPoint2 {
    const fn make_vtbl<C: IPlugInEntryPoint2Trait + Class, W: Wrapper<C>, const OFFSET: isize>() -> IPlugInEntryPoint2Vtbl {
        unsafe extern "system" fn bind<C: IPlugInEntryPoint2Trait + Class, W: Wrapper<C>, const OFFSET: isize>(
            this: *mut IPlugInEntryPoint2,
            dc: ARADocumentControllerRef,
            known: ARAPlugInInstanceRoleFlags,
            assigned: ARAPlugInInstanceRoleFlags,
        ) -> *const ARAPlugInExtensionInstance {
            unsafe { (*data::<C, W, OFFSET, _>(this)).bindToDocumentControllerWithRoles(dc, known, assigned) }
        }
        IPlugInEntryPoint2Vtbl { base: FUnknown::make_vtbl::<C, W, OFFSET>(), bindToDocumentControllerWithRoles: bind::<C, W, OFFSET> }
    }
}
unsafe impl<C: IPlugInEntryPoint2Trait + Class, W: Wrapper<C>, const OFFSET: isize> vst3::com_scrape_types::Construct<C, W, OFFSET>
    for IPlugInEntryPoint2
{
    const OBJ: Self = IPlugInEntryPoint2 { vtbl: &Self::make_vtbl::<C, W, OFFSET>() };
}

// ---- the classes -------------------------------------------------------------------------------

const ARA_CID: TUID = uid(0x5C0A1A00, 0x11112222, 0x33334444, 0x55556666);
const ARA_MAIN_CID: TUID = uid(0x5C0A1A01, 0x11112222, 0x33334444, 0x55556666);
const NULL_CID: TUID = uid(0x5C0A1A02, 0x11112222, 0x33334444, 0x55556666);
const NULL_MAIN_CID: TUID = uid(0x5C0A1A03, 0x11112222, 0x33334444, 0x55556666);
const TINY_CID: TUID = uid(0x5C0A1A04, 0x11112222, 0x33334444, 0x55556666);
const TINY_MAIN_CID: TUID = uid(0x5C0A1A05, 0x11112222, 0x33334444, 0x55556666);
const ONE_CID: TUID = uid(0x5C0A1A06, 0x11112222, 0x33334444, 0x55556666);
const ONE_MAIN_CID: TUID = uid(0x5C0A1A07, 0x11112222, 0x33334444, 0x55556666);
const PLAIN_CID: TUID = uid(0x5C0A1A08, 0x11112222, 0x33334444, 0x55556666);

const MODULE: &str = "Audio Module Class";

const CLASSES: [(TUID, &str, &str); 9] = [
    (ARA_CID, MODULE, "Fixture ARA"),
    (ARA_MAIN_CID, ara::kARAMainFactoryClass, "Fixture ARA"),
    (NULL_CID, MODULE, "Fixture ARA Null"),
    (NULL_MAIN_CID, ara::kARAMainFactoryClass, "Fixture ARA Null"),
    (TINY_CID, MODULE, "Fixture ARA Tiny"),
    (TINY_MAIN_CID, ara::kARAMainFactoryClass, "Fixture ARA Tiny"),
    (ONE_CID, MODULE, "Fixture ARA One"),
    (ONE_MAIN_CID, ara::kARAMainFactoryClass, "Fixture ARA One"),
    (PLAIN_CID, MODULE, "Fixture Plain"),
];

fn copy_c8(src: &str, dst: &mut [c_char]) {
    for (d, s) in dst.iter_mut().zip(src.bytes().chain(std::iter::once(0))) {
        *d = s as c_char;
    }
}

/// An "ARA Main Factory Class" object.
struct MainFactory {
    factory: *const ARAFactory,
}
unsafe impl Send for MainFactory {}
unsafe impl Sync for MainFactory {}

impl Class for MainFactory {
    type Interfaces = (IMainFactory,);
}

impl IMainFactoryTrait for MainFactory {
    unsafe fn getFactory(&self) -> *const ARAFactory {
        self.factory
    }
}

/// The audio module: bound to an ARA document, it renders its playback regions through the test
/// plug-in's renderer (`test_plugin::render`); otherwise a stereo pass-through.
struct Effect {
    factory: *const ARAFactory,
    /// The renderer id its binding gave (0: not bound).
    renderer: std::sync::atomic::AtomicUsize,
    /// The sample rate `setupProcessing` gave.
    rate: std::sync::Mutex<f64>,
}

impl Drop for Effect {
    fn drop(&mut self) {
        let r = self.renderer.load(std::sync::atomic::Ordering::SeqCst);
        if r != 0 {
            test_plugin::unbind(r);
        }
    }
}
unsafe impl Send for Effect {}
unsafe impl Sync for Effect {}

/// The same with only the ARA 1 entry point.
struct OldEffect {
    factory: *const ARAFactory,
}
unsafe impl Send for OldEffect {}
unsafe impl Sync for OldEffect {}

/// Without ARA.
struct Plain;

impl Effect {
    fn new(factory: *const ARAFactory) -> Effect {
        Effect { factory, renderer: std::sync::atomic::AtomicUsize::new(0), rate: std::sync::Mutex::new(48_000.0) }
    }
}

impl Class for Effect {
    type Interfaces = (IComponent, IAudioProcessor, IPlugInEntryPoint, IPlugInEntryPoint2);
}
impl Class for OldEffect {
    type Interfaces = (IComponent, IAudioProcessor, IPlugInEntryPoint);
}
impl Class for Plain {
    type Interfaces = (IComponent, IAudioProcessor);
}

impl IPlugInEntryPointTrait for Effect {
    unsafe fn getFactory(&self) -> *const ARAFactory {
        self.factory
    }
    unsafe fn bindToDocumentController(&self, dc: ARADocumentControllerRef) -> *const ARAPlugInExtensionInstance {
        let all = ara::kARAPlaybackRendererRole | ara::kARAEditorRendererRole | ara::kARAEditorViewRole;
        let (ext, id) = test_plugin::bind_id(dc, all, all);
        self.renderer.store(id, std::sync::atomic::Ordering::SeqCst);
        ext
    }
}
impl IPlugInEntryPoint2Trait for Effect {
    unsafe fn bindToDocumentControllerWithRoles(
        &self,
        dc: ARADocumentControllerRef,
        known: ARAPlugInInstanceRoleFlags,
        assigned: ARAPlugInInstanceRoleFlags,
    ) -> *const ARAPlugInExtensionInstance {
        let (ext, id) = test_plugin::bind_id(dc, known, assigned);
        self.renderer.store(id, std::sync::atomic::Ordering::SeqCst);
        ext
    }
}
impl IPlugInEntryPointTrait for OldEffect {
    unsafe fn getFactory(&self) -> *const ARAFactory {
        self.factory
    }
    unsafe fn bindToDocumentController(&self, _dc: ARADocumentControllerRef) -> *const ARAPlugInExtensionInstance {
        std::ptr::null()
    }
}

/// What the audio module renders: bound to ARA, its regions; else nothing (a pass-through).
trait Render {
    fn setup(&self, _rate: f64) {}
    /// Renders the block into the output channels; `false`: pass the input through.
    fn render(&self, _data: &mut ProcessData) -> bool {
        false
    }
}
impl Render for OldEffect {}
impl Render for Plain {}
impl Render for Effect {
    fn setup(&self, rate: f64) {
        *self.rate.lock().unwrap() = rate;
    }
    fn render(&self, data: &mut ProcessData) -> bool {
        let r = self.renderer.load(std::sync::atomic::Ordering::SeqCst);
        if r == 0 || data.numOutputs < 1 || data.outputs.is_null() {
            return false;
        }
        let rate = *self.rate.lock().unwrap();
        let pos = if data.processContext.is_null() { 0 } else { unsafe { (*data.processContext).projectTimeSamples } };
        let frames = data.numSamples.max(0) as usize;
        let bus = unsafe { &mut *data.outputs };
        let n = bus.numChannels.max(0) as usize;
        let mut chans: Vec<&mut [f32]> =
            (0..n).map(|c| unsafe { std::slice::from_raw_parts_mut(*bus.__field0.channelBuffers32.add(c), frames) }).collect();
        test_plugin::render(r, pos, rate, &mut chans);
        true
    }
}

macro_rules! pass_through {
    ($t:ty) => {
        impl IPluginBaseTrait for $t {
            unsafe fn initialize(&self, _context: *mut FUnknown) -> tresult {
                kResultOk
            }
            unsafe fn terminate(&self) -> tresult {
                kResultOk
            }
        }
        impl IComponentTrait for $t {
            unsafe fn getControllerClassId(&self, _class_id: *mut TUID) -> tresult {
                kNotImplemented
            }
            unsafe fn setIoMode(&self, _mode: IoMode) -> tresult {
                kResultOk
            }
            unsafe fn getBusCount(&self, media: MediaType, _dir: BusDirection) -> i32 {
                if media == MediaTypes_::kAudio as MediaType { 1 } else { 0 }
            }
            unsafe fn getBusInfo(&self, media: MediaType, dir: BusDirection, index: i32, bus: *mut BusInfo) -> tresult {
                if index != 0 || media != MediaTypes_::kAudio as MediaType {
                    return kInvalidArgument;
                }
                let b = unsafe { &mut *bus };
                b.mediaType = media;
                b.direction = dir;
                b.channelCount = 2;
                b.busType = BusTypes_::kMain as BusType;
                b.flags = BusInfo_::BusFlags_::kDefaultActive as u32;
                kResultOk
            }
            unsafe fn getRoutingInfo(&self, _i: *mut RoutingInfo, _o: *mut RoutingInfo) -> tresult {
                kNotImplemented
            }
            unsafe fn activateBus(&self, _m: MediaType, _d: BusDirection, _i: i32, _s: TBool) -> tresult {
                kResultOk
            }
            unsafe fn setActive(&self, _state: TBool) -> tresult {
                kResultOk
            }
            unsafe fn setState(&self, _state: *mut IBStream) -> tresult {
                kResultOk
            }
            unsafe fn getState(&self, _state: *mut IBStream) -> tresult {
                kResultOk
            }
        }
        impl IAudioProcessorTrait for $t {
            unsafe fn setBusArrangements(&self, _i: *mut SpeakerArrangement, _ni: i32, _o: *mut SpeakerArrangement, _no: i32) -> tresult {
                kResultTrue
            }
            unsafe fn getBusArrangement(&self, _dir: BusDirection, _index: i32, arr: *mut SpeakerArrangement) -> tresult {
                unsafe { *arr = SpeakerArr::kStereo };
                kResultOk
            }
            unsafe fn canProcessSampleSize(&self, size: i32) -> tresult {
                if size == SymbolicSampleSizes_::kSample32 as i32 { kResultOk } else { kNotImplemented }
            }
            unsafe fn getLatencySamples(&self) -> u32 {
                0
            }
            unsafe fn setupProcessing(&self, setup: *mut ProcessSetup) -> tresult {
                if !setup.is_null() {
                    Render::setup(self, unsafe { (*setup).sampleRate });
                }
                kResultOk
            }
            unsafe fn setProcessing(&self, _state: TBool) -> tresult {
                kResultOk
            }
            unsafe fn process(&self, data: *mut ProcessData) -> tresult {
                if !data.is_null() {
                    Render::render(self, unsafe { &mut *data });
                }
                kResultOk
            }
            unsafe fn getTailSamples(&self) -> u32 {
                0
            }
        }
    };
}

pass_through!(Effect);
pass_through!(OldEffect);
pass_through!(Plain);

// ---- factory -------------------------------------------------------------------------------------

struct Factory;

impl Class for Factory {
    type Interfaces = (IPluginFactory2,);
}

impl IPluginFactoryTrait for Factory {
    unsafe fn getFactoryInfo(&self, info: *mut PFactoryInfo) -> tresult {
        let i = unsafe { &mut *info };
        copy_c8("FreeDAWm ARA tests", &mut i.vendor);
        copy_c8("https://example.invalid", &mut i.url);
        copy_c8("tests@example.invalid", &mut i.email);
        i.flags = PFactoryInfo_::FactoryFlags_::kUnicode as i32;
        kResultOk
    }
    unsafe fn countClasses(&self) -> i32 {
        CLASSES.len() as i32
    }
    unsafe fn getClassInfo(&self, index: i32, info: *mut PClassInfo) -> tresult {
        let Some(&(cid, cat, name)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        kResultOk
    }
    unsafe fn createInstance(&self, cid: FIDString, iid: FIDString, obj: *mut *mut c_void) -> tresult {
        let cid = unsafe { *(cid as *const TUID) };
        let good = test_plugin::factory(Behaviour::Good);
        let unknown = match cid {
            c if c == ARA_CID => ComWrapper::new(Effect::new(good)).to_com_ptr::<FUnknown>(),
            c if c == NULL_CID => ComWrapper::new(Effect::new(std::ptr::null())).to_com_ptr::<FUnknown>(),
            c if c == TINY_CID => ComWrapper::new(Effect::new(test_plugin::factory(Behaviour::TinyFactory))).to_com_ptr::<FUnknown>(),
            c if c == ONE_CID => ComWrapper::new(OldEffect { factory: test_plugin::factory(Behaviour::Ara1) }).to_com_ptr::<FUnknown>(),
            c if c == PLAIN_CID => ComWrapper::new(Plain).to_com_ptr::<FUnknown>(),
            c if c == ARA_MAIN_CID => ComWrapper::new(MainFactory { factory: good }).to_com_ptr::<FUnknown>(),
            c if c == NULL_MAIN_CID => ComWrapper::new(MainFactory { factory: std::ptr::null() }).to_com_ptr::<FUnknown>(),
            c if c == TINY_MAIN_CID => {
                ComWrapper::new(MainFactory { factory: test_plugin::factory(Behaviour::TinyFactory) }).to_com_ptr::<FUnknown>()
            }
            c if c == ONE_MAIN_CID => ComWrapper::new(MainFactory { factory: test_plugin::factory(Behaviour::Ara1) }).to_com_ptr::<FUnknown>(),
            _ => None,
        };
        match unknown {
            Some(u) => {
                let p = u.as_ptr();
                unsafe { ((*(*p).vtbl).queryInterface)(p, iid as *const TUID, obj) }
            }
            None => kInvalidArgument,
        }
    }
}

impl IPluginFactory2Trait for Factory {
    unsafe fn getClassInfo2(&self, index: i32, info: *mut PClassInfo2) -> tresult {
        let Some(&(cid, cat, name)) = CLASSES.get(index as usize) else { return kInvalidArgument };
        let i = unsafe { &mut *info };
        i.cid = cid;
        i.cardinality = PClassInfo_::ClassCardinality_::kManyInstances as i32;
        copy_c8(cat, &mut i.category);
        copy_c8(name, &mut i.name);
        i.classFlags = 0;
        copy_c8(if cat == MODULE { "Fx" } else { "" }, &mut i.subCategories);
        copy_c8("", &mut i.vendor);
        copy_c8("2.0.0", &mut i.version);
        copy_c8("VST 3.8.0", &mut i.sdkVersion);
        kResultOk
    }
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn InitDll() -> bool {
    true
}

#[cfg(target_os = "windows")]
#[unsafe(no_mangle)]
extern "system" fn ExitDll() -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleEntry(_bundle: *mut c_void) -> bool {
    true
}

#[cfg(target_os = "macos")]
#[unsafe(no_mangle)]
extern "C" fn bundleExit() -> bool {
    true
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleEntry(_handle: *mut c_void) -> bool {
    true
}

#[cfg(all(unix, not(target_os = "macos")))]
#[unsafe(no_mangle)]
extern "C" fn ModuleExit() -> bool {
    true
}

#[unsafe(no_mangle)]
extern "system" fn GetPluginFactory() -> *mut IPluginFactory {
    match ComWrapper::new(Factory).to_com_ptr::<IPluginFactory>() {
        Some(p) => p.into_raw(),
        None => std::ptr::null_mut(),
    }
}

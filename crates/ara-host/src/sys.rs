//! Hand-written Rust bindings to the ARA C API.
//!
//! Source: `ARAInterface.h` and `ARAVST3.h` from Celemony's ARA_API repository
//! (<https://github.com/Celemony/ARA_API>), commit `65ec5c43b943a48cb5446f448a0492db6af8534b`,
//! which is tag `releases/2.3.0` ("ARA SDK 2.3 release (aka 2.3.001) (2025/11/07)"). Both headers
//! are vendored unmodified in `crates/ara-host/vendor/ARA_API/` for review.
//!
//! The headers' notice: "Copyright (c) 2012-2025, Celemony Software GmbH, All Rights Reserved.
//! Developed in cooperation with PreSonus Software Ltd. Licensed under the Apache License,
//! Version 2.0 (the "License"); you may not use this file except in compliance with the License.
//! You may obtain a copy of the License at http://www.apache.org/licenses/LICENSE-2.0. Unless
//! required by applicable law or agreed to in writing, software distributed under the License is
//! distributed on an "AS IS" BASIS, WITHOUT WARRANTIES OR CONDITIONS OF ANY KIND, either express
//! or implied. See the License for the specific language governing permissions and limitations
//! under the License."
//!
//! Changes (Apache-2.0 section 4(b)): this file is a translation of those declarations into Rust
//! by the FreeDAWm contributors, written by hand from the header (no generator, no C compiler):
//! the C names are kept (with their `ARA`/`kARA` prefixes) so each item can be found in the
//! header; the C `typedef`s of 32-bit enums become `i32` type aliases with `pub const` values;
//! function pointers become `Option<unsafe extern "C" fn ...>` (a null pointer is `None`); the
//! documentation comments are shortened. The VST3 companion interfaces of `ARAVST3.h` (C++
//! classes) are given as their interface ids and vtable layouts.
//!
//! Layout. The header packs every struct with `#pragma pack(push, 1)` on x86 and x86-64 and with
//! `#pragma pack(push, 8)` on ARM; other CPUs are an error there. So on x86 and x86-64 every
//! struct here is `#[repr(C, packed)]` and on every other CPU `#[repr(C)]`, which is the same as
//! packing to 8 bytes because no ARA field needs more than 8-byte alignment. Fields of a packed
//! struct must be read by value (`{ s.field }`), never borrowed. `layout_asserts` below checks
//! every struct's size and the offset of every field at compile time against values worked out
//! by hand from the header, for 64-bit pointers (x86-64 and AArch64, the CPUs FreeDAWm ships
//! for); 32-bit builds compile the bindings without those checks and the host refuses ARA there.
//!
//! Calling convention: `ARA_CALL` is `__cdecl` with MSVC and empty elsewhere, which on every
//! 64-bit target is the platform's C convention, `extern "C"`. The VST3 interfaces use VST3's
//! `PLUGIN_API` (`__stdcall` on 32-bit Windows, the C convention elsewhere): `extern "system"`.
//!
//! Versioned structs: a struct's first field `structSize` says how much of it the other side
//! filled in; members marked `ARA_ADDENDUM` in the header may be missing. Use [`implements`] to
//! test for a member before reading it, and the `*_MIN_SIZE` constants (the header's
//! `k...MinSize`) to refuse structs too small to be valid.

#![allow(non_camel_case_types, non_snake_case, non_upper_case_globals, missing_docs)]

use std::ffi::{c_char, c_void};

/// Declares an ARA struct with the header's packing (see the module docs).
macro_rules! ara_struct {
    ($(#[$m:meta])* pub struct $name:ident { $($(#[$fm:meta])* pub $field:ident : $ty:ty),* $(,)? }) => {
        $(#[$m])*
        #[cfg_attr(any(target_arch = "x86", target_arch = "x86_64"), repr(C, packed))]
        #[cfg_attr(not(any(target_arch = "x86", target_arch = "x86_64")), repr(C))]
        #[derive(Clone, Copy)]
        pub struct $name { $($(#[$fm])* pub $field: $ty),* }
    };
}

// ---- basic types ----------------------------------------------------------------------------

pub type ARAByte = u8;
pub type ARAInt32 = i32;
pub type ARAInt64 = i64;
pub type ARASize = usize;
pub type ARABool = ARAInt32;
pub const kARATrue: ARABool = 1;
pub const kARAFalse: ARABool = 0;

pub type ARAUtf8Char = c_char;
pub type ARAUtf8String = *const ARAUtf8Char;
pub type ARAPersistentID = *const c_char;

pub type ARATimePosition = f64;
pub type ARATimeDuration = f64;
pub type ARASamplePosition = ARAInt64;
pub type ARASampleCount = ARAInt64;
pub type ARAQuarterPosition = f64;
pub type ARAQuarterDuration = f64;
pub type ARASampleRate = f64;
pub type ARAChannelCount = ARAInt32;

pub type ARAChannelArrangementDataType = ARAInt32;
pub const kARAChannelArrangementUndefined: ARAChannelArrangementDataType = 0;
pub const kARAChannelArrangementVST3SpeakerArrangement: ARAChannelArrangementDataType = 1;
pub const kARAChannelArrangementCoreAudioChannelLayout: ARAChannelArrangementDataType = 2;
pub const kARAChannelArrangementAAXStemFormat: ARAChannelArrangementDataType = 3;
pub const kARAChannelArrangementCLAPChannelMap: ARAChannelArrangementDataType = 4;
pub const kARAChannelArrangementCLAPAmbisonicInfo: ARAChannelArrangementDataType = 5;

ara_struct! {
    pub struct ARAColor {
        pub r: f32,
        pub g: f32,
        pub b: f32,
    }
}

// Object references: `ARA_REF(X)` / `ARA_HOST_REF(X)` are pointers to distinct incomplete
// structs, i.e. opaque pointers. Plug-in refs are opaque to the host and host refs to the plug-in.
macro_rules! opaque_ref {
    ($($name:ident),* $(,)?) => {
        $(pub type $name = *mut c_void;)*
    };
}

opaque_ref!(
    ARAMusicalContextRef,
    ARAMusicalContextHostRef,
    ARARegionSequenceRef,
    ARARegionSequenceHostRef,
    ARAAudioSourceRef,
    ARAAudioSourceHostRef,
    ARAAudioModificationRef,
    ARAAudioModificationHostRef,
    ARAPlaybackRegionRef,
    ARAPlaybackRegionHostRef,
    ARAContentReaderRef,
    ARAContentReaderHostRef,
    ARAAudioAccessControllerHostRef,
    ARAAudioReaderHostRef,
    ARAArchivingControllerHostRef,
    ARAArchiveReaderHostRef,
    ARAArchiveWriterHostRef,
    ARAContentAccessControllerHostRef,
    ARAModelUpdateControllerHostRef,
    ARAPlaybackControllerHostRef,
    ARADocumentControllerRef,
    ARAPlaybackRendererRef,
    ARAEditorRendererRef,
    ARAEditorViewRef,
    ARAPlugInExtensionRef,
);

// ---- versioning -----------------------------------------------------------------------------

pub type ARAAPIGeneration = ARAInt32;
/// Not on ARM (the header leaves the 1.x and 2.0 draft generations out there).
pub const kARAAPIGeneration_1_0_Draft: ARAAPIGeneration = 1;
/// Not on ARM.
pub const kARAAPIGeneration_1_0_Final: ARAAPIGeneration = 2;
/// Not on ARM.
pub const kARAAPIGeneration_2_0_Draft: ARAAPIGeneration = 3;
pub const kARAAPIGeneration_2_0_Final: ARAAPIGeneration = 4;
pub const kARAAPIGeneration_2_X_Draft: ARAAPIGeneration = 5;
pub const kARAAPIGeneration_2_3_Final: ARAAPIGeneration = 6;

/// `ARA_IMPLEMENTS_FIELD`: whether a received versioned struct whose `structSize` is
/// `struct_size` contains the member at `offset` (from `std::mem::offset_of!`).
pub const fn implements(struct_size: ARASize, offset: usize) -> bool {
    struct_size > offset
}

/// `ARA_IMPLEMENTED_STRUCT_SIZE`: the `structSize` of a struct filled up to and including the
/// member at `offset` of size `size`.
pub const fn implemented_size(offset: usize, size: usize) -> ARASize {
    offset + size
}

// ---- debugging ------------------------------------------------------------------------------

pub type ARAAssertCategory = ARAInt32;
pub const kARAAssertUnspecified: ARAAssertCategory = 0;
pub const kARAAssertInvalidArgument: ARAAssertCategory = -1;
pub const kARAAssertInvalidState: ARAAssertCategory = -2;
pub const kARAAssertInvalidThread: ARAAssertCategory = -3;

pub type ARAAssertFunction = Option<unsafe extern "C" fn(category: ARAAssertCategory, problematicArgument: *const c_void, diagnosis: *const c_char)>;

// ---- model graph objects ----------------------------------------------------------------------

ara_struct! {
    pub struct ARADocumentProperties {
        pub structSize: ARASize,
        pub name: ARAUtf8String,
    }
}

ara_struct! {
    pub struct ARAMusicalContextProperties {
        pub structSize: ARASize,
        /// Addendum 2.0 draft.
        pub name: ARAUtf8String,
        /// Addendum 2.0 draft.
        pub orderIndex: ARAInt32,
        /// Addendum 2.0 draft.
        pub color: *const ARAColor,
    }
}

ara_struct! {
    /// Addendum 2.0 draft.
    pub struct ARARegionSequenceProperties {
        pub structSize: ARASize,
        pub name: ARAUtf8String,
        pub orderIndex: ARAInt32,
        pub musicalContextRef: ARAMusicalContextRef,
        /// Addendum 2.0 draft.
        pub color: *const ARAColor,
    }
}

ara_struct! {
    pub struct ARAAudioSourceProperties {
        pub structSize: ARASize,
        pub name: ARAUtf8String,
        pub persistentID: ARAPersistentID,
        pub sampleCount: ARASampleCount,
        pub sampleRate: ARASampleRate,
        pub channelCount: ARAChannelCount,
        pub merits64BitSamples: ARABool,
        /// Addendum 2.0 final.
        pub channelArrangementDataType: ARAChannelArrangementDataType,
        /// Addendum 2.0 final.
        pub channelArrangement: *const c_void,
    }
}

ara_struct! {
    pub struct ARAAudioModificationProperties {
        pub structSize: ARASize,
        pub name: ARAUtf8String,
        pub persistentID: ARAPersistentID,
    }
}

pub type ARAPlaybackTransformationFlags = ARAInt32;
pub const kARAPlaybackTransformationNoChanges: ARAPlaybackTransformationFlags = 0;
pub const kARAPlaybackTransformationTimestretch: ARAPlaybackTransformationFlags = 1 << 0;
pub const kARAPlaybackTransformationTimestretchReflectingTempo: ARAPlaybackTransformationFlags = 1 << 1;
pub const kARAPlaybackTransformationContentBasedFadeAtTail: ARAPlaybackTransformationFlags = 1 << 2;
pub const kARAPlaybackTransformationContentBasedFadeAtHead: ARAPlaybackTransformationFlags = 1 << 3;
pub const kARAPlaybackTransformationContentBasedFades: ARAPlaybackTransformationFlags =
    kARAPlaybackTransformationContentBasedFadeAtHead | kARAPlaybackTransformationContentBasedFadeAtTail;

ara_struct! {
    pub struct ARAPlaybackRegionProperties {
        pub structSize: ARASize,
        pub transformationFlags: ARAPlaybackTransformationFlags,
        pub startInModificationTime: ARATimePosition,
        pub durationInModificationTime: ARATimeDuration,
        pub startInPlaybackTime: ARATimePosition,
        pub durationInPlaybackTime: ARATimeDuration,
        /// Deprecated since 2.0 draft (still part of the minimum size).
        pub musicalContextRef: ARAMusicalContextRef,
        /// Addendum 2.0 draft.
        pub regionSequenceRef: ARARegionSequenceRef,
        /// Addendum 2.0 draft.
        pub name: ARAUtf8String,
        /// Addendum 2.0 draft.
        pub color: *const ARAColor,
    }
}

// ---- content reading ------------------------------------------------------------------------

pub type ARAContentUpdateFlags = ARAInt32;
pub const kARAContentUpdateEverythingChanged: ARAContentUpdateFlags = 0;
pub const kARAContentUpdateSignalScopeRemainsUnchanged: ARAContentUpdateFlags = 1 << 0;
pub const kARAContentUpdateNoteScopeRemainsUnchanged: ARAContentUpdateFlags = 1 << 1;
pub const kARAContentUpdateTimingScopeRemainsUnchanged: ARAContentUpdateFlags = 1 << 2;
pub const kARAContentUpdateTuningScopeRemainsUnchanged: ARAContentUpdateFlags = 1 << 3;
pub const kARAContentUpdateHarmonicScopeRemainsUnchanged: ARAContentUpdateFlags = 1 << 4;

pub type ARAContentType = ARAInt32;
pub const kARAContentTypeNotes: ARAContentType = 10;
pub const kARAContentTypeTempoEntries: ARAContentType = 20;
pub const kARAContentTypeBarSignatures: ARAContentType = 21;
pub const kARAContentTypeStaticTuning: ARAContentType = 31;
pub const kARAContentTypeKeySignatures: ARAContentType = 42;
pub const kARAContentTypeSheetChords: ARAContentType = 45;

ara_struct! {
    pub struct ARAContentTimeRange {
        pub start: ARATimePosition,
        pub duration: ARATimeDuration,
    }
}

pub type ARAContentGrade = ARAInt32;
pub const kARAContentGradeInitial: ARAContentGrade = 0;
pub const kARAContentGradeDetected: ARAContentGrade = 1;
pub const kARAContentGradeAdjusted: ARAContentGrade = 2;
pub const kARAContentGradeApproved: ARAContentGrade = 3;

ara_struct! {
    pub struct ARAContentTempoEntry {
        pub timePosition: ARATimePosition,
        pub quarterPosition: ARAQuarterPosition,
    }
}

ara_struct! {
    pub struct ARAContentBarSignature {
        pub numerator: ARAInt32,
        pub denominator: ARAInt32,
        pub position: ARAQuarterPosition,
    }
}

pub type ARAPitchNumber = ARAInt32;
pub const kARAInvalidPitchNumber: ARAPitchNumber = i32::MIN;
pub const kARAInvalidFrequency: f32 = 0.0;
pub const kARADefaultConcertPitchFrequency: f32 = 440.0;

ara_struct! {
    pub struct ARAContentNote {
        pub frequency: f32,
        pub pitchNumber: ARAPitchNumber,
        pub volume: f32,
        pub startPosition: ARATimePosition,
        pub attackDuration: ARATimeDuration,
        pub noteDuration: ARATimeDuration,
        pub signalDuration: ARATimeDuration,
    }
}

pub type ARACircleOfFifthsIndex = ARAInt32;

ara_struct! {
    pub struct ARAContentTuning {
        pub concertPitchFrequency: f32,
        pub root: ARACircleOfFifthsIndex,
        pub tunings: [f32; 12],
        pub name: ARAUtf8String,
    }
}

pub type ARAKeySignatureIntervalUsage = ARAByte;
pub const kARAKeySignatureIntervalUnused: ARAKeySignatureIntervalUsage = 0x00;
pub const kARAKeySignatureIntervalUsed: ARAKeySignatureIntervalUsage = 0xFF;

ara_struct! {
    pub struct ARAContentKeySignature {
        pub root: ARACircleOfFifthsIndex,
        pub intervals: [ARAKeySignatureIntervalUsage; 12],
        pub name: ARAUtf8String,
        pub position: ARAQuarterPosition,
    }
}

pub type ARAChordIntervalUsage = ARAByte;
pub const kARAChordDiatonicDegree1: ARAChordIntervalUsage = 0x01;
pub const kARAChordDiatonicDegree2: ARAChordIntervalUsage = 0x02;
pub const kARAChordDiatonicDegree3: ARAChordIntervalUsage = 0x03;
pub const kARAChordDiatonicDegree4: ARAChordIntervalUsage = 0x04;
pub const kARAChordDiatonicDegree5: ARAChordIntervalUsage = 0x05;
pub const kARAChordDiatonicDegree6: ARAChordIntervalUsage = 0x06;
pub const kARAChordDiatonicDegree7: ARAChordIntervalUsage = 0x07;
pub const kARAChordDiatonicDegree9: ARAChordIntervalUsage = 0x09;
pub const kARAChordDiatonicDegree11: ARAChordIntervalUsage = 0x0B;
pub const kARAChordDiatonicDegree13: ARAChordIntervalUsage = 0x0D;
pub const kARAChordIntervalUsed: ARAChordIntervalUsage = 0xFF;
pub const kARAChordIntervalUnused: ARAChordIntervalUsage = 0x00;

ara_struct! {
    pub struct ARAContentChord {
        pub root: ARACircleOfFifthsIndex,
        pub bass: ARACircleOfFifthsIndex,
        pub intervals: [ARAChordIntervalUsage; 12],
        pub name: ARAUtf8String,
        pub position: ARAQuarterPosition,
    }
}

// ---- host side controller interfaces --------------------------------------------------------

ara_struct! {
    pub struct ARAAudioAccessControllerInterface {
        pub structSize: ARASize,
        pub createAudioReaderForSource: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAAudioAccessControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                use64BitSamples: ARABool,
            ) -> ARAAudioReaderHostRef,
        >,
        pub readAudioSamples: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAAudioAccessControllerHostRef,
                audioReaderHostRef: ARAAudioReaderHostRef,
                samplePosition: ARASamplePosition,
                samplesPerChannel: ARASampleCount,
                buffers: *const *mut c_void,
            ) -> ARABool,
        >,
        pub destroyAudioReader:
            Option<unsafe extern "C" fn(controllerHostRef: ARAAudioAccessControllerHostRef, audioReaderHostRef: ARAAudioReaderHostRef)>,
    }
}

ara_struct! {
    pub struct ARAArchivingControllerInterface {
        pub structSize: ARASize,
        pub getArchiveSize:
            Option<unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, archiveReaderHostRef: ARAArchiveReaderHostRef) -> ARASize>,
        pub readBytesFromArchive: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAArchivingControllerHostRef,
                archiveReaderHostRef: ARAArchiveReaderHostRef,
                position: ARASize,
                length: ARASize,
                buffer: *mut ARAByte,
            ) -> ARABool,
        >,
        pub writeBytesToArchive: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAArchivingControllerHostRef,
                archiveWriterHostRef: ARAArchiveWriterHostRef,
                position: ARASize,
                length: ARASize,
                buffer: *const ARAByte,
            ) -> ARABool,
        >,
        pub notifyDocumentArchivingProgress: Option<unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, value: f32)>,
        pub notifyDocumentUnarchivingProgress: Option<unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, value: f32)>,
        /// Addendum 2.0 final.
        pub getDocumentArchiveID: Option<
            unsafe extern "C" fn(controllerHostRef: ARAArchivingControllerHostRef, archiveReaderHostRef: ARAArchiveReaderHostRef) -> ARAPersistentID,
        >,
    }
}

ara_struct! {
    pub struct ARAContentAccessControllerInterface {
        pub structSize: ARASize,
        pub isMusicalContextContentAvailable: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                musicalContextHostRef: ARAMusicalContextHostRef,
                contentType: ARAContentType,
            ) -> ARABool,
        >,
        pub getMusicalContextContentGrade: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                musicalContextHostRef: ARAMusicalContextHostRef,
                contentType: ARAContentType,
            ) -> ARAContentGrade,
        >,
        pub createMusicalContextContentReader: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                musicalContextHostRef: ARAMusicalContextHostRef,
                contentType: ARAContentType,
                range: *const ARAContentTimeRange,
            ) -> ARAContentReaderHostRef,
        >,
        pub isAudioSourceContentAvailable: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                contentType: ARAContentType,
            ) -> ARABool,
        >,
        pub getAudioSourceContentGrade: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                contentType: ARAContentType,
            ) -> ARAContentGrade,
        >,
        pub createAudioSourceContentReader: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                contentType: ARAContentType,
                range: *const ARAContentTimeRange,
            ) -> ARAContentReaderHostRef,
        >,
        pub getContentReaderEventCount: Option<
            unsafe extern "C" fn(controllerHostRef: ARAContentAccessControllerHostRef, contentReaderHostRef: ARAContentReaderHostRef) -> ARAInt32,
        >,
        pub getContentReaderDataForEvent: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAContentAccessControllerHostRef,
                contentReaderHostRef: ARAContentReaderHostRef,
                eventIndex: ARAInt32,
            ) -> *const c_void,
        >,
        pub destroyContentReader:
            Option<unsafe extern "C" fn(controllerHostRef: ARAContentAccessControllerHostRef, contentReaderHostRef: ARAContentReaderHostRef)>,
    }
}

pub type ARAAnalysisProgressState = ARAInt32;
pub const kARAAnalysisProgressStarted: ARAAnalysisProgressState = 0;
pub const kARAAnalysisProgressUpdated: ARAAnalysisProgressState = 1;
pub const kARAAnalysisProgressCompleted: ARAAnalysisProgressState = 2;

ara_struct! {
    pub struct ARAModelUpdateControllerInterface {
        pub structSize: ARASize,
        pub notifyAudioSourceAnalysisProgress: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAModelUpdateControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                state: ARAAnalysisProgressState,
                value: f32,
            ),
        >,
        pub notifyAudioSourceContentChanged: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAModelUpdateControllerHostRef,
                audioSourceHostRef: ARAAudioSourceHostRef,
                range: *const ARAContentTimeRange,
                flags: ARAContentUpdateFlags,
            ),
        >,
        pub notifyAudioModificationContentChanged: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAModelUpdateControllerHostRef,
                audioModificationHostRef: ARAAudioModificationHostRef,
                range: *const ARAContentTimeRange,
                flags: ARAContentUpdateFlags,
            ),
        >,
        /// Addendum 2.0 draft.
        pub notifyPlaybackRegionContentChanged: Option<
            unsafe extern "C" fn(
                controllerHostRef: ARAModelUpdateControllerHostRef,
                playbackRegionHostRef: ARAPlaybackRegionHostRef,
                range: *const ARAContentTimeRange,
                flags: ARAContentUpdateFlags,
            ),
        >,
        /// Addendum 2.3 final.
        pub notifyDocumentDataChanged: Option<unsafe extern "C" fn(controllerHostRef: ARAModelUpdateControllerHostRef)>,
    }
}

ara_struct! {
    pub struct ARAPlaybackControllerInterface {
        pub structSize: ARASize,
        pub requestStartPlayback: Option<unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef)>,
        pub requestStopPlayback: Option<unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef)>,
        pub requestSetPlaybackPosition: Option<unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef, timePosition: ARATimePosition)>,
        pub requestSetCycleRange: Option<
            unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef, startTime: ARATimePosition, duration: ARATimeDuration),
        >,
        pub requestEnableCycle: Option<unsafe extern "C" fn(controllerHostRef: ARAPlaybackControllerHostRef, enable: ARABool)>,
    }
}

ara_struct! {
    pub struct ARADocumentControllerHostInstance {
        pub structSize: ARASize,
        pub audioAccessControllerHostRef: ARAAudioAccessControllerHostRef,
        pub audioAccessControllerInterface: *const ARAAudioAccessControllerInterface,
        pub archivingControllerHostRef: ARAArchivingControllerHostRef,
        pub archivingControllerInterface: *const ARAArchivingControllerInterface,
        pub contentAccessControllerHostRef: ARAContentAccessControllerHostRef,
        pub contentAccessControllerInterface: *const ARAContentAccessControllerInterface,
        pub modelUpdateControllerHostRef: ARAModelUpdateControllerHostRef,
        pub modelUpdateControllerInterface: *const ARAModelUpdateControllerInterface,
        pub playbackControllerHostRef: ARAPlaybackControllerHostRef,
        pub playbackControllerInterface: *const ARAPlaybackControllerInterface,
    }
}

// ---- plug-in side controller interfaces and factory -----------------------------------------

ara_struct! {
    /// Addendum 2.0 final.
    pub struct ARARestoreObjectsFilter {
        pub structSize: ARASize,
        pub documentData: ARABool,
        pub audioSourceIDsCount: ARASize,
        pub audioSourceArchiveIDs: *const ARAPersistentID,
        pub audioSourceCurrentIDs: *const ARAPersistentID,
        pub audioModificationIDsCount: ARASize,
        pub audioModificationArchiveIDs: *const ARAPersistentID,
        pub audioModificationCurrentIDs: *const ARAPersistentID,
    }
}

ara_struct! {
    /// Addendum 2.0 final.
    pub struct ARAStoreObjectsFilter {
        pub structSize: ARASize,
        pub documentData: ARABool,
        pub audioSourceRefsCount: ARASize,
        pub audioSourceRefs: *const ARAAudioSourceRef,
        pub audioModificationRefsCount: ARASize,
        pub audioModificationRefs: *const ARAAudioModificationRef,
    }
}

ara_struct! {
    /// Addendum 2.0 final.
    pub struct ARAProcessingAlgorithmProperties {
        pub structSize: ARASize,
        pub persistentID: ARAPersistentID,
        pub name: ARAUtf8String,
    }
}

type Dc = ARADocumentControllerRef;

ara_struct! {
    pub struct ARADocumentControllerInterface {
        pub structSize: ARASize,
        // destruction and factory link
        pub destroyDocumentController: Option<unsafe extern "C" fn(controllerRef: Dc)>,
        pub getFactory: Option<unsafe extern "C" fn(controllerRef: Dc) -> *const ARAFactory>,
        // update management
        pub beginEditing: Option<unsafe extern "C" fn(controllerRef: Dc)>,
        pub endEditing: Option<unsafe extern "C" fn(controllerRef: Dc)>,
        pub notifyModelUpdates: Option<unsafe extern "C" fn(controllerRef: Dc)>,
        // document persistency (deprecated since 2.0 final)
        pub beginRestoringDocumentFromArchive: Option<unsafe extern "C" fn(controllerRef: Dc, archiveReaderHostRef: ARAArchiveReaderHostRef) -> ARABool>,
        pub endRestoringDocumentFromArchive: Option<unsafe extern "C" fn(controllerRef: Dc, archiveReaderHostRef: ARAArchiveReaderHostRef) -> ARABool>,
        pub storeDocumentToArchive: Option<unsafe extern "C" fn(controllerRef: Dc, archiveWriterHostRef: ARAArchiveWriterHostRef) -> ARABool>,
        // document management
        pub updateDocumentProperties: Option<unsafe extern "C" fn(controllerRef: Dc, properties: *const ARADocumentProperties)>,
        // musical context management
        pub createMusicalContext: Option<
            unsafe extern "C" fn(controllerRef: Dc, hostRef: ARAMusicalContextHostRef, properties: *const ARAMusicalContextProperties) -> ARAMusicalContextRef,
        >,
        pub updateMusicalContextProperties:
            Option<unsafe extern "C" fn(controllerRef: Dc, musicalContextRef: ARAMusicalContextRef, properties: *const ARAMusicalContextProperties)>,
        pub updateMusicalContextContent: Option<
            unsafe extern "C" fn(controllerRef: Dc, musicalContextRef: ARAMusicalContextRef, range: *const ARAContentTimeRange, flags: ARAContentUpdateFlags),
        >,
        pub destroyMusicalContext: Option<unsafe extern "C" fn(controllerRef: Dc, musicalContextRef: ARAMusicalContextRef)>,
        // audio source management
        pub createAudioSource: Option<
            unsafe extern "C" fn(controllerRef: Dc, hostRef: ARAAudioSourceHostRef, properties: *const ARAAudioSourceProperties) -> ARAAudioSourceRef,
        >,
        pub updateAudioSourceProperties:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, properties: *const ARAAudioSourceProperties)>,
        pub updateAudioSourceContent: Option<
            unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, range: *const ARAContentTimeRange, flags: ARAContentUpdateFlags),
        >,
        pub enableAudioSourceSamplesAccess: Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, enable: ARABool)>,
        pub deactivateAudioSourceForUndoHistory: Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, deactivate: ARABool)>,
        pub destroyAudioSource: Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef)>,
        // audio modification management
        pub createAudioModification: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                audioSourceRef: ARAAudioSourceRef,
                hostRef: ARAAudioModificationHostRef,
                properties: *const ARAAudioModificationProperties,
            ) -> ARAAudioModificationRef,
        >,
        pub cloneAudioModification: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                audioModificationRef: ARAAudioModificationRef,
                hostRef: ARAAudioModificationHostRef,
                properties: *const ARAAudioModificationProperties,
            ) -> ARAAudioModificationRef,
        >,
        pub updateAudioModificationProperties: Option<
            unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef, properties: *const ARAAudioModificationProperties),
        >,
        pub deactivateAudioModificationForUndoHistory:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef, deactivate: ARABool)>,
        pub destroyAudioModification: Option<unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef)>,
        // playback region management
        pub createPlaybackRegion: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                audioModificationRef: ARAAudioModificationRef,
                hostRef: ARAPlaybackRegionHostRef,
                properties: *const ARAPlaybackRegionProperties,
            ) -> ARAPlaybackRegionRef,
        >,
        pub updatePlaybackRegionProperties:
            Option<unsafe extern "C" fn(controllerRef: Dc, playbackRegionRef: ARAPlaybackRegionRef, properties: *const ARAPlaybackRegionProperties)>,
        pub destroyPlaybackRegion: Option<unsafe extern "C" fn(controllerRef: Dc, playbackRegionRef: ARAPlaybackRegionRef)>,
        // content reading
        pub isAudioSourceContentAvailable:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, contentType: ARAContentType) -> ARABool>,
        pub isAudioSourceContentAnalysisIncomplete:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, contentType: ARAContentType) -> ARABool>,
        pub requestAudioSourceContentAnalysis: Option<
            unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, contentTypesCount: ARASize, contentTypes: *const ARAContentType),
        >,
        pub getAudioSourceContentGrade:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, contentType: ARAContentType) -> ARAContentGrade>,
        pub createAudioSourceContentReader: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                audioSourceRef: ARAAudioSourceRef,
                contentType: ARAContentType,
                range: *const ARAContentTimeRange,
            ) -> ARAContentReaderRef,
        >,
        pub isAudioModificationContentAvailable:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef, contentType: ARAContentType) -> ARABool>,
        pub getAudioModificationContentGrade:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef, contentType: ARAContentType) -> ARAContentGrade>,
        pub createAudioModificationContentReader: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                audioModificationRef: ARAAudioModificationRef,
                contentType: ARAContentType,
                range: *const ARAContentTimeRange,
            ) -> ARAContentReaderRef,
        >,
        pub isPlaybackRegionContentAvailable:
            Option<unsafe extern "C" fn(controllerRef: Dc, playbackRegionRef: ARAPlaybackRegionRef, contentType: ARAContentType) -> ARABool>,
        pub getPlaybackRegionContentGrade:
            Option<unsafe extern "C" fn(controllerRef: Dc, playbackRegionRef: ARAPlaybackRegionRef, contentType: ARAContentType) -> ARAContentGrade>,
        pub createPlaybackRegionContentReader: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                playbackRegionRef: ARAPlaybackRegionRef,
                contentType: ARAContentType,
                range: *const ARAContentTimeRange,
            ) -> ARAContentReaderRef,
        >,
        pub getContentReaderEventCount: Option<unsafe extern "C" fn(controllerRef: Dc, contentReaderRef: ARAContentReaderRef) -> ARAInt32>,
        pub getContentReaderDataForEvent:
            Option<unsafe extern "C" fn(controllerRef: Dc, contentReaderRef: ARAContentReaderRef, eventIndex: ARAInt32) -> *const c_void>,
        pub destroyContentReader: Option<unsafe extern "C" fn(controllerRef: Dc, contentReaderRef: ARAContentReaderRef)>,
        // region sequence management (addendum 2.0 draft)
        pub createRegionSequence: Option<
            unsafe extern "C" fn(controllerRef: Dc, hostRef: ARARegionSequenceHostRef, properties: *const ARARegionSequenceProperties) -> ARARegionSequenceRef,
        >,
        pub updateRegionSequenceProperties:
            Option<unsafe extern "C" fn(controllerRef: Dc, regionSequenceRef: ARARegionSequenceRef, properties: *const ARARegionSequenceProperties)>,
        pub destroyRegionSequence: Option<unsafe extern "C" fn(controllerRef: Dc, regionSequenceRef: ARARegionSequenceRef)>,
        pub getPlaybackRegionHeadAndTailTime: Option<
            unsafe extern "C" fn(controllerRef: Dc, playbackRegionRef: ARAPlaybackRegionRef, headTime: *mut ARATimeDuration, tailTime: *mut ARATimeDuration),
        >,
        // document persistency (addendum 2.0 final)
        pub restoreObjectsFromArchive: Option<
            unsafe extern "C" fn(controllerRef: Dc, archiveReaderHostRef: ARAArchiveReaderHostRef, filter: *const ARARestoreObjectsFilter) -> ARABool,
        >,
        pub storeObjectsToArchive: Option<
            unsafe extern "C" fn(controllerRef: Dc, archiveWriterHostRef: ARAArchiveWriterHostRef, filter: *const ARAStoreObjectsFilter) -> ARABool,
        >,
        // processing algorithms (addendum 2.0 final)
        pub getProcessingAlgorithmsCount: Option<unsafe extern "C" fn(controllerRef: Dc) -> ARAInt32>,
        pub getProcessingAlgorithmProperties:
            Option<unsafe extern "C" fn(controllerRef: Dc, algorithmIndex: ARAInt32) -> *const ARAProcessingAlgorithmProperties>,
        pub getProcessingAlgorithmForAudioSource: Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef) -> ARAInt32>,
        pub requestProcessingAlgorithmForAudioSource:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioSourceRef: ARAAudioSourceRef, algorithmIndex: ARAInt32)>,
        // license management (addendum 2.0 final)
        pub isLicensedForCapabilities: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                runModalActivationDialogIfNeeded: ARABool,
                contentTypesCount: ARASize,
                contentTypes: *const ARAContentType,
                transformationFlags: ARAPlaybackTransformationFlags,
            ) -> ARABool,
        >,
        // audio file chunks (addendum 2.0 final)
        pub storeAudioSourceToAudioFileChunk: Option<
            unsafe extern "C" fn(
                controllerRef: Dc,
                archiveWriterHostRef: ARAArchiveWriterHostRef,
                audioSourceRef: ARAAudioSourceRef,
                documentArchiveID: *mut ARAPersistentID,
                openAutomatically: *mut ARABool,
            ) -> ARABool,
        >,
        // (addendum 2.0 final)
        pub isAudioModificationPreservingAudioSourceSignal:
            Option<unsafe extern "C" fn(controllerRef: Dc, audioModificationRef: ARAAudioModificationRef) -> ARABool>,
    }
}

ara_struct! {
    pub struct ARADocumentControllerInstance {
        pub structSize: ARASize,
        pub documentControllerRef: ARADocumentControllerRef,
        pub documentControllerInterface: *const ARADocumentControllerInterface,
    }
}

ara_struct! {
    pub struct ARAInterfaceConfiguration {
        pub structSize: ARASize,
        pub desiredApiGeneration: ARAAPIGeneration,
        pub assertFunctionAddress: *mut ARAAssertFunction,
    }
}

ara_struct! {
    pub struct ARAFactory {
        pub structSize: ARASize,
        pub lowestSupportedApiGeneration: ARAAPIGeneration,
        pub highestSupportedApiGeneration: ARAAPIGeneration,
        pub factoryID: ARAPersistentID,
        pub initializeARAWithConfiguration: Option<unsafe extern "C" fn(config: *const ARAInterfaceConfiguration)>,
        pub uninitializeARA: Option<unsafe extern "C" fn()>,
        pub plugInName: ARAUtf8String,
        pub manufacturerName: ARAUtf8String,
        pub informationURL: ARAUtf8String,
        pub version: ARAUtf8String,
        pub createDocumentControllerWithDocument: Option<
            unsafe extern "C" fn(hostInstance: *const ARADocumentControllerHostInstance, properties: *const ARADocumentProperties) -> *const ARADocumentControllerInstance,
        >,
        pub documentArchiveID: ARAPersistentID,
        pub compatibleDocumentArchiveIDsCount: ARASize,
        pub compatibleDocumentArchiveIDs: *const ARAPersistentID,
        pub analyzeableContentTypesCount: ARASize,
        pub analyzeableContentTypes: *const ARAContentType,
        pub supportedPlaybackTransformationFlags: ARAPlaybackTransformationFlags,
        /// Addendum 2.0 final.
        pub supportsStoringAudioFileChunks: ARABool,
    }
}

pub type ARAPlugInInstanceRoleFlags = ARAInt32;
pub const kARAPlaybackRendererRole: ARAPlugInInstanceRoleFlags = 1 << 0;
pub const kARAEditorRendererRole: ARAPlugInInstanceRoleFlags = 1 << 1;
pub const kARAEditorViewRole: ARAPlugInInstanceRoleFlags = 1 << 2;

ara_struct! {
    /// Addendum 2.0 draft.
    pub struct ARAPlaybackRendererInterface {
        pub structSize: ARASize,
        pub addPlaybackRegion: Option<unsafe extern "C" fn(playbackRendererRef: ARAPlaybackRendererRef, playbackRegionRef: ARAPlaybackRegionRef)>,
        pub removePlaybackRegion: Option<unsafe extern "C" fn(playbackRendererRef: ARAPlaybackRendererRef, playbackRegionRef: ARAPlaybackRegionRef)>,
    }
}

ara_struct! {
    /// Addendum 2.0 draft.
    pub struct ARAEditorRendererInterface {
        pub structSize: ARASize,
        pub addPlaybackRegion: Option<unsafe extern "C" fn(editorRendererRef: ARAEditorRendererRef, playbackRegionRef: ARAPlaybackRegionRef)>,
        pub removePlaybackRegion: Option<unsafe extern "C" fn(editorRendererRef: ARAEditorRendererRef, playbackRegionRef: ARAPlaybackRegionRef)>,
        pub addRegionSequence: Option<unsafe extern "C" fn(editorRendererRef: ARAEditorRendererRef, regionSequenceRef: ARARegionSequenceRef)>,
        pub removeRegionSequence: Option<unsafe extern "C" fn(editorRendererRef: ARAEditorRendererRef, regionSequenceRef: ARARegionSequenceRef)>,
    }
}

ara_struct! {
    pub struct ARAViewSelection {
        pub structSize: ARASize,
        pub playbackRegionRefsCount: ARASize,
        pub playbackRegionRefs: *const ARAPlaybackRegionRef,
        pub regionSequenceRefsCount: ARASize,
        pub regionSequenceRefs: *const ARARegionSequenceRef,
        pub timeRange: *const ARAContentTimeRange,
    }
}

ara_struct! {
    /// Addendum 2.0 draft.
    pub struct ARAEditorViewInterface {
        pub structSize: ARASize,
        pub notifySelection: Option<unsafe extern "C" fn(editorViewRef: ARAEditorViewRef, selection: *const ARAViewSelection)>,
        pub notifyHideRegionSequences: Option<
            unsafe extern "C" fn(editorViewRef: ARAEditorViewRef, regionSequenceRefsCount: ARASize, regionSequenceRefs: *const ARARegionSequenceRef),
        >,
    }
}

ara_struct! {
    /// Deprecated since 2.0 draft.
    pub struct ARAPlugInExtensionInterface {
        pub structSize: ARASize,
        pub setPlaybackRegion: Option<unsafe extern "C" fn(plugInExtensionRef: ARAPlugInExtensionRef, playbackRegionRef: ARAPlaybackRegionRef)>,
        pub removePlaybackRegion: Option<unsafe extern "C" fn(plugInExtensionRef: ARAPlugInExtensionRef, playbackRegionRef: ARAPlaybackRegionRef)>,
    }
}

ara_struct! {
    pub struct ARAPlugInExtensionInstance {
        pub structSize: ARASize,
        /// Deprecated since 2.0 draft.
        pub plugInExtensionRef: ARAPlugInExtensionRef,
        /// Deprecated since 2.0 draft.
        pub plugInExtensionInterface: *const ARAPlugInExtensionInterface,
        pub playbackRendererRef: ARAPlaybackRendererRef,
        pub playbackRendererInterface: *const ARAPlaybackRendererInterface,
        pub editorRendererRef: ARAEditorRendererRef,
        pub editorRendererInterface: *const ARAEditorRendererInterface,
        pub editorViewRef: ARAEditorViewRef,
        pub editorViewInterface: *const ARAEditorViewInterface,
    }
}

// ---- minimum sizes (the header's `k...MinSize` enums) ------------------------------------------

macro_rules! min_size {
    ($name:ident = $ty:ident . $field:ident : $fty:ty) => {
        pub const $name: ARASize = implemented_size(std::mem::offset_of!($ty, $field), std::mem::size_of::<$fty>());
    };
}

min_size!(kARADocumentPropertiesMinSize = ARADocumentProperties.name: ARAUtf8String);
min_size!(kARAMusicalContextPropertiesMinSize = ARAMusicalContextProperties.structSize: ARASize);
min_size!(kARARegionSequencePropertiesMinSize = ARARegionSequenceProperties.musicalContextRef: ARAMusicalContextRef);
min_size!(kARAAudioSourcePropertiesMinSize = ARAAudioSourceProperties.merits64BitSamples: ARABool);
min_size!(kARAAudioModificationPropertiesMinSize = ARAAudioModificationProperties.persistentID: ARAPersistentID);
min_size!(kARAPlaybackRegionPropertiesMinSize = ARAPlaybackRegionProperties.musicalContextRef: ARAMusicalContextRef);
min_size!(kARAAudioAccessControllerInterfaceMinSize = ARAAudioAccessControllerInterface.destroyAudioReader: usize);
min_size!(kARAArchivingControllerInterfaceMinSize = ARAArchivingControllerInterface.notifyDocumentUnarchivingProgress: usize);
min_size!(kARAContentAccessControllerInterfaceMinSize = ARAContentAccessControllerInterface.destroyContentReader: usize);
min_size!(kARAModelUpdateControllerInterfaceMinSize = ARAModelUpdateControllerInterface.notifyAudioModificationContentChanged: usize);
min_size!(kARAPlaybackControllerInterfaceMinSize = ARAPlaybackControllerInterface.requestEnableCycle: usize);
min_size!(kARADocumentControllerHostInstanceMinSize = ARADocumentControllerHostInstance.playbackControllerInterface: usize);
min_size!(kARARestoreObjectsFilterMinSize = ARARestoreObjectsFilter.audioModificationCurrentIDs: usize);
min_size!(kARAStoreObjectsFilterMinSize = ARAStoreObjectsFilter.audioModificationRefs: usize);
min_size!(kARAProcessingAlgorithmPropertiesMinSize = ARAProcessingAlgorithmProperties.name: ARAUtf8String);
min_size!(kARADocumentControllerInterfaceMinSize = ARADocumentControllerInterface.destroyContentReader: usize);
min_size!(kARADocumentControllerInstanceMinSize = ARADocumentControllerInstance.documentControllerInterface: usize);
min_size!(kARAInterfaceConfigurationMinSize = ARAInterfaceConfiguration.assertFunctionAddress: usize);
min_size!(kARAFactoryMinSize = ARAFactory.supportedPlaybackTransformationFlags: ARAPlaybackTransformationFlags);
min_size!(kARAPlaybackRendererInterfaceMinSize = ARAPlaybackRendererInterface.removePlaybackRegion: usize);
min_size!(kARAEditorRendererInterfaceMinSize = ARAEditorRendererInterface.removeRegionSequence: usize);
min_size!(kARAViewSelectionMinSize = ARAViewSelection.timeRange: usize);
min_size!(kARAEditorViewInterfaceMinSize = ARAEditorViewInterface.notifyHideRegionSequences: usize);
min_size!(kARAPlugInExtensionInterfaceMinSize = ARAPlugInExtensionInterface.removePlaybackRegion: usize);
min_size!(kARAPlugInExtensionInstanceMinSize = ARAPlugInExtensionInstance.plugInExtensionInterface: usize);

// ---- VST3 companion API (ARAVST3.h) ---------------------------------------------------------

/// The factory class category of the class that offers [`IMainFactory`] (`kARAMainFactoryClass`).
pub const kARAMainFactoryClass: &str = "ARA Main Factory Class";

/// `DECLARE_CLASS_IID (IMainFactory, 0xDB2A1669, 0xFAFD42A5, 0xA82F864F, 0x7B6872EA)`: the four
/// 32-bit words of the interface id, laid out as bytes by [`vst3_tuid`].
pub const IMainFactory_iid: [u32; 4] = [0xDB2A1669, 0xFAFD42A5, 0xA82F864F, 0x7B6872EA];
/// `DECLARE_CLASS_IID (IPlugInEntryPoint, 0x12814E54, 0xA1CE4076, 0x82B96813, 0x16950BD6)`.
pub const IPlugInEntryPoint_iid: [u32; 4] = [0x12814E54, 0xA1CE4076, 0x82B96813, 0x16950BD6];
/// `DECLARE_CLASS_IID (IPlugInEntryPoint2, 0xCD9A5913, 0xC9EB46D7, 0x96CA53AD, 0xD1DB89F5)`
/// (addendum 2.0 draft).
pub const IPlugInEntryPoint2_iid: [u32; 4] = [0xCD9A5913, 0xC9EB46D7, 0x96CA53AD, 0xD1DB89F5];

/// The 16 bytes of a VST3 interface or class id from its four words, as the VST3 SDK's
/// `INLINE_UID` lays them out: in COM GUID order on Windows (`COM_COMPATIBLE`), big-endian words
/// elsewhere.
pub const fn vst3_tuid(w: [u32; 4]) -> [u8; 16] {
    let [a, b, c, d] = w;
    let (a, b, c, d) = (a.to_be_bytes(), b.to_be_bytes(), c.to_be_bytes(), d.to_be_bytes());
    if cfg!(windows) {
        [a[3], a[2], a[1], a[0], b[1], b[0], b[3], b[2], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
    } else {
        [a[0], a[1], a[2], a[3], b[0], b[1], b[2], b[3], c[0], c[1], c[2], c[3], d[0], d[1], d[2], d[3]]
    }
}

/// `Steinberg::FUnknown`'s vtable: `queryInterface`, `addRef`, `release`.
#[repr(C)]
#[derive(Clone, Copy)]
pub struct FUnknownVtbl {
    pub queryInterface: unsafe extern "system" fn(this: *mut c_void, iid: *const [u8; 16], obj: *mut *mut c_void) -> i32,
    pub addRef: unsafe extern "system" fn(this: *mut c_void) -> u32,
    pub release: unsafe extern "system" fn(this: *mut c_void) -> u32,
}

/// `ARA::IMainFactory`'s vtable (FUnknown, then `getFactory`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IMainFactoryVtbl {
    pub base: FUnknownVtbl,
    pub getFactory: unsafe extern "system" fn(this: *mut c_void) -> *const ARAFactory,
}

/// `ARA::IPlugInEntryPoint`'s vtable (FUnknown, `getFactory`, the deprecated
/// `bindToDocumentController`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IPlugInEntryPointVtbl {
    pub base: FUnknownVtbl,
    pub getFactory: unsafe extern "system" fn(this: *mut c_void) -> *const ARAFactory,
    pub bindToDocumentController:
        unsafe extern "system" fn(this: *mut c_void, documentControllerRef: ARADocumentControllerRef) -> *const ARAPlugInExtensionInstance,
}

/// `ARA::IPlugInEntryPoint2`'s vtable (FUnknown, `bindToDocumentControllerWithRoles`).
#[repr(C)]
#[derive(Clone, Copy)]
pub struct IPlugInEntryPoint2Vtbl {
    pub base: FUnknownVtbl,
    pub bindToDocumentControllerWithRoles: unsafe extern "system" fn(
        this: *mut c_void,
        documentControllerRef: ARADocumentControllerRef,
        knownRoles: ARAPlugInInstanceRoleFlags,
        assignedRoles: ARAPlugInInstanceRoleFlags,
    ) -> *const ARAPlugInExtensionInstance,
}

// ---- compile-time layout checks ---------------------------------------------------------------

/// Sizes and field offsets worked out by hand from `ARAInterface.h` (pack 1 on x86-64, pack 8 on
/// AArch64; 64-bit pointers, `size_t` and function pointers). A wrong binding fails the build.
#[allow(dead_code)]
mod layout_asserts {
    use super::*;
    use std::mem::{offset_of, size_of};

    macro_rules! check {
        ($ty:ident { size: $x86:expr, $arm:expr; $($field:ident: $fx:expr, $fa:expr);* $(;)? }) => {
            #[cfg(target_arch = "x86_64")]
            const _: () = {
                assert!(size_of::<$ty>() == $x86);
                $(assert!(offset_of!($ty, $field) == $fx);)*
            };
            #[cfg(target_arch = "aarch64")]
            const _: () = {
                assert!(size_of::<$ty>() == $arm);
                $(assert!(offset_of!($ty, $field) == $fa);)*
            };
        };
    }

    // The basic types the offsets below rely on.
    const _: () = assert!(size_of::<ARABool>() == 4 && size_of::<ARAInt64>() == 8 && size_of::<ARAAPIGeneration>() == 4);
    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(size_of::<ARASize>() == 8 && size_of::<ARAUtf8String>() == 8 && size_of::<ARAAssertFunction>() == 8);
        assert!(size_of::<Option<unsafe extern "C" fn()>>() == 8);
    };

    check!(ARAColor { size: 12, 12; r: 0, 0; g: 4, 4; b: 8, 8 });
    check!(ARADocumentProperties { size: 16, 16; structSize: 0, 0; name: 8, 8 });
    check!(ARAMusicalContextProperties { size: 28, 32; structSize: 0, 0; name: 8, 8; orderIndex: 16, 16; color: 20, 24 });
    check!(ARARegionSequenceProperties {
        size: 36, 40;
        structSize: 0, 0; name: 8, 8; orderIndex: 16, 16; musicalContextRef: 20, 24; color: 28, 32
    });
    check!(ARAAudioSourceProperties {
        size: 60, 64;
        structSize: 0, 0; name: 8, 8; persistentID: 16, 16; sampleCount: 24, 24; sampleRate: 32, 32;
        channelCount: 40, 40; merits64BitSamples: 44, 44; channelArrangementDataType: 48, 48; channelArrangement: 52, 56
    });
    check!(ARAAudioModificationProperties { size: 24, 24; structSize: 0, 0; name: 8, 8; persistentID: 16, 16 });
    check!(ARAPlaybackRegionProperties {
        size: 76, 80;
        structSize: 0, 0; transformationFlags: 8, 8; startInModificationTime: 12, 16; durationInModificationTime: 20, 24;
        startInPlaybackTime: 28, 32; durationInPlaybackTime: 36, 40; musicalContextRef: 44, 48; regionSequenceRef: 52, 56;
        name: 60, 64; color: 68, 72
    });
    check!(ARAContentTimeRange { size: 16, 16; start: 0, 0; duration: 8, 8 });
    check!(ARAContentTempoEntry { size: 16, 16; timePosition: 0, 0; quarterPosition: 8, 8 });
    check!(ARAContentBarSignature { size: 16, 16; numerator: 0, 0; denominator: 4, 4; position: 8, 8 });
    check!(ARAContentNote {
        size: 44, 48;
        frequency: 0, 0; pitchNumber: 4, 4; volume: 8, 8; startPosition: 12, 16; attackDuration: 20, 24;
        noteDuration: 28, 32; signalDuration: 36, 40
    });
    check!(ARAContentTuning { size: 64, 64; concertPitchFrequency: 0, 0; root: 4, 4; tunings: 8, 8; name: 56, 56 });
    check!(ARAContentKeySignature { size: 32, 32; root: 0, 0; intervals: 4, 4; name: 16, 16; position: 24, 24 });
    check!(ARAContentChord { size: 36, 40; root: 0, 0; bass: 4, 4; intervals: 8, 8; name: 20, 24; position: 28, 32 });
    check!(ARAAudioAccessControllerInterface {
        size: 32, 32;
        structSize: 0, 0; createAudioReaderForSource: 8, 8; readAudioSamples: 16, 16; destroyAudioReader: 24, 24
    });
    check!(ARAArchivingControllerInterface {
        size: 56, 56;
        structSize: 0, 0; getArchiveSize: 8, 8; readBytesFromArchive: 16, 16; writeBytesToArchive: 24, 24;
        notifyDocumentArchivingProgress: 32, 32; notifyDocumentUnarchivingProgress: 40, 40; getDocumentArchiveID: 48, 48
    });
    check!(ARAContentAccessControllerInterface {
        size: 80, 80;
        structSize: 0, 0; isMusicalContextContentAvailable: 8, 8; getMusicalContextContentGrade: 16, 16;
        createMusicalContextContentReader: 24, 24; isAudioSourceContentAvailable: 32, 32; getAudioSourceContentGrade: 40, 40;
        createAudioSourceContentReader: 48, 48; getContentReaderEventCount: 56, 56; getContentReaderDataForEvent: 64, 64;
        destroyContentReader: 72, 72
    });
    check!(ARAModelUpdateControllerInterface {
        size: 48, 48;
        structSize: 0, 0; notifyAudioSourceAnalysisProgress: 8, 8; notifyAudioSourceContentChanged: 16, 16;
        notifyAudioModificationContentChanged: 24, 24; notifyPlaybackRegionContentChanged: 32, 32; notifyDocumentDataChanged: 40, 40
    });
    check!(ARAPlaybackControllerInterface {
        size: 48, 48;
        structSize: 0, 0; requestStartPlayback: 8, 8; requestStopPlayback: 16, 16; requestSetPlaybackPosition: 24, 24;
        requestSetCycleRange: 32, 32; requestEnableCycle: 40, 40
    });
    check!(ARADocumentControllerHostInstance {
        size: 88, 88;
        structSize: 0, 0; audioAccessControllerHostRef: 8, 8; audioAccessControllerInterface: 16, 16;
        archivingControllerHostRef: 24, 24; archivingControllerInterface: 32, 32; contentAccessControllerHostRef: 40, 40;
        contentAccessControllerInterface: 48, 48; modelUpdateControllerHostRef: 56, 56; modelUpdateControllerInterface: 64, 64;
        playbackControllerHostRef: 72, 72; playbackControllerInterface: 80, 80
    });
    check!(ARARestoreObjectsFilter {
        size: 60, 64;
        structSize: 0, 0; documentData: 8, 8; audioSourceIDsCount: 12, 16; audioSourceArchiveIDs: 20, 24;
        audioSourceCurrentIDs: 28, 32; audioModificationIDsCount: 36, 40; audioModificationArchiveIDs: 44, 48;
        audioModificationCurrentIDs: 52, 56
    });
    check!(ARAStoreObjectsFilter {
        size: 44, 48;
        structSize: 0, 0; documentData: 8, 8; audioSourceRefsCount: 12, 16; audioSourceRefs: 20, 24;
        audioModificationRefsCount: 28, 32; audioModificationRefs: 36, 40
    });
    check!(ARAProcessingAlgorithmProperties { size: 24, 24; structSize: 0, 0; persistentID: 8, 8; name: 16, 16 });
    // 54 function pointers after `structSize`: the n-th (from 0) is at 8 + 8 n.
    check!(ARADocumentControllerInterface {
        size: 440, 440;
        structSize: 0, 0; destroyDocumentController: 8, 8; getFactory: 16, 16; beginEditing: 24, 24; endEditing: 32, 32;
        notifyModelUpdates: 40, 40; beginRestoringDocumentFromArchive: 48, 48; endRestoringDocumentFromArchive: 56, 56;
        storeDocumentToArchive: 64, 64; updateDocumentProperties: 72, 72; createMusicalContext: 80, 80;
        updateMusicalContextProperties: 88, 88; updateMusicalContextContent: 96, 96; destroyMusicalContext: 104, 104;
        createAudioSource: 112, 112; updateAudioSourceProperties: 120, 120; updateAudioSourceContent: 128, 128;
        enableAudioSourceSamplesAccess: 136, 136; deactivateAudioSourceForUndoHistory: 144, 144; destroyAudioSource: 152, 152;
        createAudioModification: 160, 160; cloneAudioModification: 168, 168; updateAudioModificationProperties: 176, 176;
        deactivateAudioModificationForUndoHistory: 184, 184; destroyAudioModification: 192, 192; createPlaybackRegion: 200, 200;
        updatePlaybackRegionProperties: 208, 208; destroyPlaybackRegion: 216, 216; isAudioSourceContentAvailable: 224, 224;
        isAudioSourceContentAnalysisIncomplete: 232, 232; requestAudioSourceContentAnalysis: 240, 240;
        getAudioSourceContentGrade: 248, 248; createAudioSourceContentReader: 256, 256; isAudioModificationContentAvailable: 264, 264;
        getAudioModificationContentGrade: 272, 272; createAudioModificationContentReader: 280, 280;
        isPlaybackRegionContentAvailable: 288, 288; getPlaybackRegionContentGrade: 296, 296;
        createPlaybackRegionContentReader: 304, 304; getContentReaderEventCount: 312, 312; getContentReaderDataForEvent: 320, 320;
        destroyContentReader: 328, 328; createRegionSequence: 336, 336; updateRegionSequenceProperties: 344, 344;
        destroyRegionSequence: 352, 352; getPlaybackRegionHeadAndTailTime: 360, 360; restoreObjectsFromArchive: 368, 368;
        storeObjectsToArchive: 376, 376; getProcessingAlgorithmsCount: 384, 384; getProcessingAlgorithmProperties: 392, 392;
        getProcessingAlgorithmForAudioSource: 400, 400; requestProcessingAlgorithmForAudioSource: 408, 408;
        isLicensedForCapabilities: 416, 416; storeAudioSourceToAudioFileChunk: 424, 424;
        isAudioModificationPreservingAudioSourceSignal: 432, 432
    });
    check!(ARADocumentControllerInstance { size: 24, 24; structSize: 0, 0; documentControllerRef: 8, 8; documentControllerInterface: 16, 16 });
    check!(ARAInterfaceConfiguration { size: 20, 24; structSize: 0, 0; desiredApiGeneration: 8, 8; assertFunctionAddress: 12, 16 });
    check!(ARAFactory {
        size: 128, 128;
        structSize: 0, 0; lowestSupportedApiGeneration: 8, 8; highestSupportedApiGeneration: 12, 12; factoryID: 16, 16;
        initializeARAWithConfiguration: 24, 24; uninitializeARA: 32, 32; plugInName: 40, 40; manufacturerName: 48, 48;
        informationURL: 56, 56; version: 64, 64; createDocumentControllerWithDocument: 72, 72; documentArchiveID: 80, 80;
        compatibleDocumentArchiveIDsCount: 88, 88; compatibleDocumentArchiveIDs: 96, 96; analyzeableContentTypesCount: 104, 104;
        analyzeableContentTypes: 112, 112; supportedPlaybackTransformationFlags: 120, 120; supportsStoringAudioFileChunks: 124, 124
    });
    check!(ARAPlaybackRendererInterface { size: 24, 24; structSize: 0, 0; addPlaybackRegion: 8, 8; removePlaybackRegion: 16, 16 });
    check!(ARAEditorRendererInterface {
        size: 40, 40;
        structSize: 0, 0; addPlaybackRegion: 8, 8; removePlaybackRegion: 16, 16; addRegionSequence: 24, 24; removeRegionSequence: 32, 32
    });
    check!(ARAViewSelection {
        size: 48, 48;
        structSize: 0, 0; playbackRegionRefsCount: 8, 8; playbackRegionRefs: 16, 16; regionSequenceRefsCount: 24, 24;
        regionSequenceRefs: 32, 32; timeRange: 40, 40
    });
    check!(ARAEditorViewInterface { size: 24, 24; structSize: 0, 0; notifySelection: 8, 8; notifyHideRegionSequences: 16, 16 });
    check!(ARAPlugInExtensionInterface { size: 24, 24; structSize: 0, 0; setPlaybackRegion: 8, 8; removePlaybackRegion: 16, 16 });
    check!(ARAPlugInExtensionInstance {
        size: 72, 72;
        structSize: 0, 0; plugInExtensionRef: 8, 8; plugInExtensionInterface: 16, 16; playbackRendererRef: 24, 24;
        playbackRendererInterface: 32, 32; editorRendererRef: 40, 40; editorRendererInterface: 48, 48; editorViewRef: 56, 56;
        editorViewInterface: 64, 64
    });

    // The header's minimum sizes, by hand.
    #[cfg(target_arch = "x86_64")]
    const _: () = {
        assert!(kARARegionSequencePropertiesMinSize == 28);
        assert!(kARAPlaybackRegionPropertiesMinSize == 52);
        assert!(kARAInterfaceConfigurationMinSize == 20);
    };
    #[cfg(target_arch = "aarch64")]
    const _: () = {
        assert!(kARARegionSequencePropertiesMinSize == 32);
        assert!(kARAPlaybackRegionPropertiesMinSize == 56);
        assert!(kARAInterfaceConfigurationMinSize == 24);
    };
    #[cfg(target_pointer_width = "64")]
    const _: () = {
        assert!(kARADocumentPropertiesMinSize == 16);
        assert!(kARAMusicalContextPropertiesMinSize == 8);
        assert!(kARAAudioSourcePropertiesMinSize == 48);
        assert!(kARAAudioModificationPropertiesMinSize == 24);
        assert!(kARAAudioAccessControllerInterfaceMinSize == 32);
        assert!(kARAArchivingControllerInterfaceMinSize == 48);
        assert!(kARAContentAccessControllerInterfaceMinSize == 80);
        assert!(kARAModelUpdateControllerInterfaceMinSize == 32);
        assert!(kARAPlaybackControllerInterfaceMinSize == 48);
        assert!(kARADocumentControllerHostInstanceMinSize == 88);
        assert!(kARADocumentControllerInterfaceMinSize == 336);
        assert!(kARADocumentControllerInstanceMinSize == 24);
        assert!(kARAFactoryMinSize == 124);
        assert!(kARAPlugInExtensionInstanceMinSize == 24);
        assert!(kARAViewSelectionMinSize == 48);
    };
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vst3_interface_ids_are_laid_out_as_the_sdk_does() {
        let t = vst3_tuid(IMainFactory_iid);
        if cfg!(windows) {
            assert_eq!(t, [0x69, 0x16, 0x2A, 0xDB, 0xFD, 0xFA, 0xA5, 0x42, 0xA8, 0x2F, 0x86, 0x4F, 0x7B, 0x68, 0x72, 0xEA]);
        } else {
            assert_eq!(t, [0xDB, 0x2A, 0x16, 0x69, 0xFA, 0xFD, 0x42, 0xA5, 0xA8, 0x2F, 0x86, 0x4F, 0x7B, 0x68, 0x72, 0xEA]);
        }
        assert_ne!(vst3_tuid(IPlugInEntryPoint_iid), vst3_tuid(IPlugInEntryPoint2_iid));
    }

    #[test]
    fn implements_tests_the_struct_size() {
        let o = std::mem::offset_of!(ARAFactory, supportsStoringAudioFileChunks);
        assert!(!implements(kARAFactoryMinSize, o), "the minimum factory has no addenda");
        assert!(implements(std::mem::size_of::<ARAFactory>(), o));
        assert_eq!(implemented_size(o, 4), std::mem::size_of::<ARAFactory>());
    }
}

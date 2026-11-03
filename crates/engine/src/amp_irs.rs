//! The cabinet impulse responses bundled with SoundCraft (`assets/amp-irs/`, the FireStation
//! owner's AmpEngine IRs; see ATTRIBUTION.md), compiled in so every build has them. A module
//! names one as `bundled:<name>` (`mix.module_ir`).

/// `(name, WAV bytes)`, by folder then file name.
pub static AMP_IRS: &[(&str, &[u8])] = &[
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Distant 12In.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Distant 12In.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Distant 24In.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Distant 24In.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 0In.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 0In.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 12In.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 12In.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 24In.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 - Rear 24In.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 1 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - C414 2 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 1 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - MD421 2 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 1 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - R121 2 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 1 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - SM57 2 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 1 - Cone.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cap Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cap Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cap.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cap.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cone Edge.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cone Edge.wav"),
    ),
    (
        "Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cone.wav",
        include_bytes!("../../../assets/amp-irs/Fender/1971 Fender Twin Reverb 2x12 Oxford 12T6-10 - i5 2 - Cone.wav"),
    ),
    ("Marshall/gb cc8.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb cc8.wav")),
    ("Marshall/gb m160.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb m160.wav")),
    ("Marshall/gb mc834.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb mc834.wav")),
    ("Marshall/gb md421.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb md421.wav")),
    ("Marshall/gb oc818.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb oc818.wav")),
    ("Marshall/gb sm57.wav", include_bytes!("../../../assets/amp-irs/Marshall/gb sm57.wav")),
];

/// The bytes of the bundled IR `name` (`Folder/File.wav`).
pub fn bundled_ir(name: &str) -> Option<&'static [u8]> {
    AMP_IRS.iter().find(|(n, _)| *n == name).map(|(_, b)| *b)
}

/// Every bundled IR's name.
pub fn bundled_ir_names() -> impl Iterator<Item = &'static str> {
    AMP_IRS.iter().map(|(n, _)| *n)
}

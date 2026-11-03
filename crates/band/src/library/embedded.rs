//! The factory Library, embedded at build time (see `build.rs`).

/// Every embedded Library file as `("Players/Clay.json", bytes)`, sorted by path. Covers
/// `Players/`, `Targets/`, `Presets/Bands/`, `Songs/Factory/` and `factory.json`.
pub fn files() -> &'static [(&'static str, &'static [u8])] {
    static FILES: &[(&str, &[u8])] = include!(concat!(env!("OUT_DIR"), "/embedded_files.rs"));
    FILES
}

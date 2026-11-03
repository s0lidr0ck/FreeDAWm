//! A session opened with a plugin the scan cache lacks: creating it fails at once (shown missing)
//! and asks for a background scan; once the plugin list has it, creating it again works.

use std::path::PathBuf;

mod common;

fn gain_id() -> &'static str {
    if cfg!(windows) { "vst3:001D0A5C111122223333444455556666" } else { "vst3:5C0A1D00111122223333444455556666" }
}

#[test]
fn an_uncached_plugin_resolves_after_the_background_scan() {
    let dir = common::install_fixture();
    let cache_dir: PathBuf = dir.join("config/PluginScan");
    soundcraft_vst3_host::set_scan_cache_dir(Some(cache_dir));
    // Opening the session: the plugin is missing for now, and a scan is asked for.
    assert!(soundcraft_vst3_host::create(gain_id()).is_none());
    assert!(soundcraft_vst3_host::take_lookup_request());
    assert!(soundcraft_vst3_host::loaded_paths().is_empty(), "nothing loaded while missing");
    // The background scan finds it (here: the fixture's folder, listed in this process).
    assert_eq!(soundcraft_vst3_host::add_search_dir(&dir), 2);
    // The app then creates the missing plugins again.
    let p = soundcraft_vst3_host::create(gain_id()).expect("resolved");
    assert_eq!(p.info().name, "Fixture Gain");
}

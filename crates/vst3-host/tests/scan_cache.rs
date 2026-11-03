//! The host and its scan cache: instantiating a plugin loads only its own bundle, found through
//! the cache, never a scan of the plugin folders.

use soundcraft_dsp::Category;
use soundcraft_plugin_scan::{Outcome, ScanCache};
use soundcraft_vst3_host::Vst3Descriptor;
use std::path::{Path, PathBuf};

mod common;

/// A plugin the cache lists in a bundle that is no longer there.
const MOVED: &str = "vst3:AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA";

/// The fixture's "Fixture Gain" (`uid()` lays out the first 8 bytes differently on Windows).
fn gain_id() -> &'static str {
    if cfg!(windows) { "vst3:001D0A5C111122223333444455556666" } else { "vst3:5C0A1D00111122223333444455556666" }
}

fn descriptor(id: &str, name: &str, path: &Path) -> Vst3Descriptor {
    Vst3Descriptor {
        id: id.into(),
        class_id: id.trim_start_matches("vst3:").into(),
        name: name.into(),
        vendor: "SoundCraft tests".into(),
        version: "1.2.3".into(),
        sdk_version: "VST 3.8.0".into(),
        sub_categories: vec!["Fx".into()],
        category: Category::Other,
        is_instrument: false,
        path: path.to_path_buf(),
    }
}

#[test]
fn instantiate_uses_cache_path_without_full_scan() {
    let dir = common::install_fixture();
    let bundle = dir.join("vendor/Fixture.vst3");
    // Another bundle the cache lists, in the same folder: it must never be loaded.
    let other = dir.join("vendor/Other.vst3");
    std::fs::create_dir_all(&other).unwrap();
    let cache_dir: PathBuf = dir.join("config/PluginScan");
    let mut cache: ScanCache<Vst3Descriptor> = ScanCache::default();
    cache.record(bundle.clone(), None, Outcome::Plugins(vec![descriptor(gain_id(), "Fixture Gain", &bundle)]));
    cache.record(other.clone(), None, Outcome::Plugins(vec![descriptor("vst3:0123456789ABCDEF0123456789ABCDEF", "Other", &other)]));
    let gone = dir.join("vendor/Gone.vst3");
    cache.record(gone.clone(), None, Outcome::Plugins(vec![descriptor(MOVED, "Moved", &gone)]));
    cache.save(&cache_dir.join("vst3.json")).unwrap();

    soundcraft_vst3_host::set_scan_cache_dir(Some(cache_dir));
    let listed = soundcraft_vst3_host::cached().expect("the cache file is read");
    assert_eq!(listed.len(), 3);
    assert!(soundcraft_vst3_host::loaded_paths().is_empty(), "reading the cache loads nothing");

    let p = soundcraft_vst3_host::instantiate(gain_id()).expect("created from the cached path");
    assert_eq!(p.info().name, "Fixture Gain");
    assert_eq!(soundcraft_vst3_host::loaded_paths(), vec![bundle.clone()], "only the plugin's own bundle was loaded");

    // An id the cache lacks is not found at once, asks for one background scan and loads nothing.
    let _ = soundcraft_vst3_host::take_lookup_request();
    let started = std::time::Instant::now();
    assert!(soundcraft_vst3_host::instantiate("vst3:FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").is_err());
    assert!(started.elapsed() < std::time::Duration::from_secs(2), "no scan on the calling thread");
    assert!(soundcraft_vst3_host::take_lookup_request(), "a background scan is asked for");
    assert!(soundcraft_vst3_host::instantiate("vst3:FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").is_err());
    assert!(!soundcraft_vst3_host::take_lookup_request(), "once per id");
    // A cached plugin whose bundle is gone (moved, uninstalled) counts as uncached.
    assert!(soundcraft_vst3_host::instantiate(MOVED).is_err());
    assert!(soundcraft_vst3_host::take_lookup_request());
    assert_eq!(soundcraft_vst3_host::loaded_paths(), vec![bundle]);

    // A running scan never blocks a lookup: it is not waited for.
    let (tx, rx) = std::sync::mpsc::channel();
    soundcraft_vst3_host::with_scan_running(|| {
        std::thread::spawn(move || {
            let r = soundcraft_vst3_host::instantiate("vst3:EEEEEEEEEEEEEEEEEEEEEEEEEEEEEEEE").is_err();
            let _ = tx.send(r);
        });
        assert_eq!(rx.recv_timeout(std::time::Duration::from_secs(5)), Ok(true), "the lookup returned while the scan held its lock");
    });
    assert!(soundcraft_vst3_host::take_lookup_request());
}

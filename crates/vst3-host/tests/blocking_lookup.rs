//! Command-line tools (`set_blocking_lookups`): a plugin missing from the scan cache runs one
//! blocking incremental scan through the helper before it is given up. The helper here is this
//! test binary (started with `--out`), which lists a plugin for `Wanted.vst3` without loading
//! anything; the test process itself loads no plugin.

use soundcraft_dsp::Category;
use soundcraft_plugin_scan::{ResultWriter, parse_helper_args};
use soundcraft_vst3_host::Vst3Descriptor;
use std::sync::atomic::{AtomicUsize, Ordering};

const WANTED: &str = "vst3:0123456789ABCDEF0123456789ABCDEF";

fn fake_helper() {
    let args = parse_helper_args(std::env::args_os().skip(1)).unwrap();
    let mut out = ResultWriter::create(&args.out).unwrap();
    for b in &args.bundles {
        let found = if b.file_name().is_some_and(|n| n == "Wanted.vst3") {
            vec![Vst3Descriptor {
                id: WANTED.into(),
                class_id: WANTED.trim_start_matches("vst3:").into(),
                name: "Wanted".into(),
                vendor: "Tests".into(),
                version: "1".into(),
                sdk_version: String::new(),
                sub_categories: vec!["Fx".into()],
                category: Category::Other,
                is_instrument: false,
                path: b.clone(),
            }]
        } else {
            Vec::new()
        };
        out.bundle(b, Ok(found)).unwrap();
    }
    out.done().unwrap();
}

static REPORTS: AtomicUsize = AtomicUsize::new(0);

fn report(done: usize, total: usize) {
    REPORTS.fetch_add(1, Ordering::Relaxed);
    eprintln!("scanning plugins: {done} / {total}");
}

fn main() {
    if std::env::args().any(|a| a == "--out") {
        fake_helper();
        return;
    }
    let dir = std::env::temp_dir().join(format!("soundcraft-blocking-lookup-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(dir.join("plugins/Wanted.vst3/Contents")).unwrap();
    let exe = std::env::current_exe().unwrap();
    let mut path = std::ffi::OsString::from(dir.join("plugins"));
    if let Some(old) = std::env::var_os("VST3_PATH") {
        path.push(if cfg!(windows) { ";" } else { ":" });
        path.push(old);
    }
    // SAFETY: single-threaded here, before anything reads the environment.
    #[allow(unsafe_code)]
    unsafe {
        std::env::set_var("FREEDAWM_SCAN_HELPER", &exe);
        std::env::set_var("VST3_PATH", &path);
    }
    soundcraft_vst3_host::set_scan_cache_dir(Some(dir.join("config/PluginScan")));
    soundcraft_vst3_host::set_blocking_lookups(Some(report));

    // Missing from the (empty) cache: the lookup scans first, so the plugin is found. Its bundle
    // is no real plugin, so creating it then fails to load, not as "no VST3 plugin".
    let e = soundcraft_vst3_host::instantiate(WANTED).err().expect("the fake bundle does not load");
    assert!(!matches!(e, soundcraft_vst3_host::Vst3Error::NotFound(_)), "found by the blocking scan: {e}");
    assert!(soundcraft_vst3_host::cached().unwrap().iter().any(|d| d.id == WANTED));
    assert!(REPORTS.load(Ordering::Relaxed) >= 1, "progress was reported");
    assert!(!soundcraft_vst3_host::take_lookup_request(), "no background scan asked for");
    println!("blocking lookup: ok");

    // An id no bundle has: one scan (incremental, nothing changed), then not found; no second scan.
    let before = REPORTS.load(Ordering::Relaxed);
    assert!(matches!(soundcraft_vst3_host::instantiate("vst3:FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF"), Err(soundcraft_vst3_host::Vst3Error::NotFound(_))));
    let after_first = REPORTS.load(Ordering::Relaxed);
    assert!(after_first > before);
    assert!(soundcraft_vst3_host::instantiate("vst3:FFFFFFFFFFFFFFFFFFFFFFFFFFFFFFFF").is_err());
    assert_eq!(REPORTS.load(Ordering::Relaxed), after_first, "once per id");
    println!("unknown id: ok");
    let _ = std::fs::remove_dir_all(&dir);
}

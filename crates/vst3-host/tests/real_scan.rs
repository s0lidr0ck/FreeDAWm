//! The real scan of the installed VST3 plugins, through the `freedawm-scan` helper: this test
//! process never loads a plugin (it only starts helpers). Build the helper first, then run with
//! `FREEDAWM_SCAN_HELPER=<target>/debug/freedawm-scan.exe cargo test -p soundcraft-vst3-host
//! --test real_scan -- --ignored --nocapture`.

use soundcraft_vst3_host::{ScanOptions, ScanProgress};
use std::time::Instant;

#[test]
#[ignore = "scans every installed VST3 plugin through the helper (minutes); run with --ignored"]
fn real_scan_through_the_helper() {
    let dir = std::env::temp_dir().join(format!("soundcraft-real-scan-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    soundcraft_vst3_host::set_scan_cache_dir(Some(dir.clone()));
    for (pass, opts) in [("first", ScanOptions::default()), ("second (incremental)", ScanOptions::default())] {
        let progress = ScanProgress::default();
        let started = Instant::now();
        let list = soundcraft_vst3_host::scan_with(&opts, &progress).expect("the helper runs");
        let failures = soundcraft_vst3_host::scan_failures();
        let count = |what: &str| failures.iter().filter(|(_, r)| r.contains(what)).count();
        println!(
            "{pass} scan: {:.1} s, {} bundles scanned, {} plugins listed, {} failed ({} crashed, {} timed out, {} would not load); {} helper crashes and {} hangs isolated",
            started.elapsed().as_secs_f64(),
            progress.done(),
            list.len(),
            failures.len(),
            count("crashed during scan"),
            count("timed out during scan"),
            failures.len() - count("crashed during scan") - count("timed out during scan"),
            progress.crashes(),
            progress.timeouts(),
        );
        for (p, r) in &failures {
            println!("  {}: {r}", p.display());
        }
    }
    assert!(soundcraft_vst3_host::loaded_paths().is_empty(), "the test process loaded no plugin");
    let _ = std::fs::remove_dir_all(&dir);
}

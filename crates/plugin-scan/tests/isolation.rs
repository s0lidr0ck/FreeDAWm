//! The scan's isolation of crashing and hanging bundles, its incremental plan and the cache file,
//! driven by a fake runner (no process, no plugin).

use serde::{Deserialize, Serialize};
use soundcraft_plugin_scan::{
    BatchEnd, BatchResult, BatchRunner, Descriptor, Entry, FailKind, Outcome, ScanCache, ScanError, ScanOptions, ScanProgress, file_stamp, run_scan,
};
use std::path::{Path, PathBuf};
use std::sync::atomic::AtomicBool;
use std::time::Duration;

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
struct Fake {
    id: String,
    name: String,
}

impl Descriptor for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
}

/// Loads bundles in order, like the helper: crashes on `crash_on`, stops reporting on `hang_on`,
/// and (process poisoning) crashes on any bundle once `poison_after` were loaded in one process.
#[derive(Default)]
struct FakeRunner {
    crash_on: Vec<PathBuf>,
    hang_on: Vec<PathBuf>,
    poison_after: Option<usize>,
    calls: Vec<Vec<PathBuf>>,
    timeouts: Vec<Duration>,
}

fn fake_of(p: &Path) -> Fake {
    let name = p.file_stem().and_then(|s| s.to_str()).unwrap_or_default().to_string();
    Fake { id: format!("fake:{name}"), name }
}

impl BatchRunner<Fake> for FakeRunner {
    fn run(&mut self, paths: &[PathBuf], per_bundle: Duration, _cancel: &AtomicBool) -> BatchResult<Fake> {
        self.calls.push(paths.to_vec());
        self.timeouts.push(per_bundle);
        let mut results = Vec::new();
        for (i, p) in paths.iter().enumerate() {
            if self.crash_on.contains(p) || self.poison_after.is_some_and(|n| i >= n) {
                return BatchResult { results, end: BatchEnd::Crashed("exit code 0xC0000005".into()) };
            }
            if self.hang_on.contains(p) {
                return BatchResult { results, end: BatchEnd::TimedOut };
            }
            results.push((p.clone(), Ok(vec![fake_of(p)])));
        }
        BatchResult { results, end: BatchEnd::Finished }
    }
}

fn bundles(n: usize) -> Vec<PathBuf> {
    (0..n).map(|i| PathBuf::from(format!("C:/VST3/b{i:02}.vst3"))).collect()
}

fn no_stamp(_: &Path) -> Option<soundcraft_plugin_scan::Stamp> {
    None
}

fn scan(cache: &mut ScanCache<Fake>, found: &[PathBuf], runner: &mut FakeRunner, opts: &ScanOptions) -> Result<(), ScanError> {
    run_scan(cache, found, &no_stamp, runner, opts, &ScanProgress::default(), &mut |_| {})
}

#[test]
fn split_until_isolated() {
    let found = bundles(40);
    let bad = found[17].clone();
    let mut runner = FakeRunner { crash_on: vec![bad.clone()], ..FakeRunner::default() };
    let mut cache = ScanCache::default();
    let progress = ScanProgress::default();
    run_scan(&mut cache, &found, &no_stamp, &mut runner, &ScanOptions::default(), &progress, &mut |_| {}).unwrap();
    let failures = cache.failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].0, bad);
    assert!(failures[0].1.contains("crashed during scan"), "{}", failures[0].1);
    assert_eq!(cache.plugins().len(), 39);
    assert!(runner.calls.iter().all(|c| c.len() <= 16), "batches of 16: {:?}", runner.calls);
    assert!(runner.calls.contains(&vec![bad.clone()]), "failed only once alone in a fresh process");
    assert_eq!((progress.done(), progress.total()), (40, 40));
}

#[test]
fn poisoning_never_blames_an_innocent_bundle() {
    // Every process dies on its sixth bundle whatever it is (cumulative poisoning): nothing is
    // marked failed, because each suspect loads fine alone.
    let found = bundles(40);
    let mut runner = FakeRunner { poison_after: Some(5), ..FakeRunner::default() };
    let mut cache = ScanCache::default();
    scan(&mut cache, &found, &mut runner, &ScanOptions::default()).unwrap();
    assert!(cache.failures().is_empty(), "{:?}", cache.failures());
    assert_eq!(cache.plugins().len(), 40);
}

#[test]
fn timeout_isolated() {
    let found = bundles(20);
    let hung = found[3].clone();
    let mut runner = FakeRunner { hang_on: vec![hung.clone()], ..FakeRunner::default() };
    let mut cache = ScanCache::default();
    scan(&mut cache, &found, &mut runner, &ScanOptions::default()).unwrap();
    let failures = cache.failures();
    assert_eq!(failures.len(), 1, "{failures:?}");
    assert_eq!(failures[0].0, hung);
    assert!(failures[0].1.contains("timed out during scan"), "{}", failures[0].1);
    assert_eq!(cache.plugins().len(), 19);
}

#[test]
fn incremental_skips_unchanged() {
    let dir = std::env::temp_dir().join(format!("soundcraft-plugin-scan-incr-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let file = |n: &str, body: &str| {
        let p = dir.join(n);
        std::fs::write(&p, body).unwrap();
        p
    };
    let (a, b, c) = (file("a.vst3", "a"), file("b.vst3", "b"), file("c.vst3", "c"));
    let mut runner = FakeRunner { crash_on: vec![a.clone()], ..FakeRunner::default() };
    let mut cache = ScanCache::default();
    let opts = ScanOptions::default();
    let go = |cache: &mut ScanCache<Fake>, found: &[PathBuf], runner: &mut FakeRunner, opts: &ScanOptions| {
        runner.calls.clear();
        run_scan(cache, found, &file_stamp, runner, opts, &ScanProgress::default(), &mut |_| {}).unwrap();
        runner.calls.concat()
    };
    let touched = go(&mut cache, &[a.clone(), b.clone(), c.clone()], &mut runner, &opts);
    assert!(touched.contains(&a) && touched.contains(&b) && touched.contains(&c));
    // Nothing changed: no process at all; the crashed bundle stays blocked.
    assert!(go(&mut cache, &[a.clone(), b.clone(), c.clone()], &mut runner, &opts).is_empty());
    // b changed size, d is new, c is gone.
    std::fs::write(&b, "bigger").unwrap();
    let d = file("d.vst3", "d");
    let touched = go(&mut cache, &[a.clone(), b.clone(), d.clone()], &mut runner, &opts);
    assert_eq!(touched, vec![b.clone(), d.clone()]);
    let names: Vec<String> = cache.plugins().into_iter().map(|p| p.name).collect();
    assert_eq!(names, vec!["b", "d"]);
    assert!(cache.entries.iter().all(|e| e.path != c), "a removed bundle leaves the cache");
    // Retry failed: only the blocked bundle runs again (and now loads).
    runner.crash_on.clear();
    let touched = go(&mut cache, &[a.clone(), b.clone(), d.clone()], &mut runner, &ScanOptions { retry_failed: true, ..ScanOptions::default() });
    assert_eq!(touched, vec![a.clone()]);
    assert!(cache.failures().is_empty());
    let _ = std::fs::remove_dir_all(&dir);
}

#[test]
fn cache_round_trip() {
    let dir = std::env::temp_dir().join(format!("soundcraft-plugin-scan-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("PluginScan").join("vst3.json");
    let mut cache: ScanCache<Fake> = ScanCache::default();
    cache.record(
        PathBuf::from("C:/VST3/Good.vst3"),
        Some(soundcraft_plugin_scan::Stamp { modified_ms: 1_700_000_000_123, size: 4096 }),
        Outcome::Plugins(vec![Fake { id: "fake:good".into(), name: "Good".into() }]),
    );
    cache.record(PathBuf::from("C:/VST3/Hang.vst3"), None, Outcome::Failed { reason: "timed out during scan".into(), kind: FailKind::TimedOut });
    cache.save(&path).unwrap();
    let back: ScanCache<Fake> = ScanCache::load(&path);
    assert_eq!(back, cache);
    assert_eq!(back.find("fake:good").map(|d| d.name), Some("Good".to_string()));
    assert_eq!(back.failures(), vec![(PathBuf::from("C:/VST3/Hang.vst3"), "timed out during scan".to_string())]);
    // A broken or missing file is an empty cache, never an error.
    std::fs::write(&path, b"{ not json").unwrap();
    assert!(ScanCache::<Fake>::load(&path).entries.is_empty());
    assert!(ScanCache::<Fake>::load(&dir.join("missing.json")).entries.is_empty());
    let _ = std::fs::remove_dir_all(&dir);
    let _: Vec<Entry<Fake>> = Vec::new();
}

#[test]
fn cancel_stops_between_batches_and_keeps_what_was_scanned() {
    let found = bundles(40);
    let progress = ScanProgress::default();
    let mut cache = ScanCache::default();
    let mut runner = FakeRunner::default();
    let mut batches = 0;
    let r = run_scan(&mut cache, &found, &no_stamp, &mut runner, &ScanOptions::default(), &progress, &mut |_| {
        batches += 1;
        progress.cancel();
    });
    assert_eq!(r, Err(ScanError::Cancelled));
    assert_eq!(runner.calls.len(), 1);
    assert_eq!(cache.plugins().len(), 16, "the finished batch is kept");
    assert!(batches >= 1);
}

#[test]
fn a_suspect_retried_alone_gets_the_longer_timeout() {
    let found = bundles(8);
    let hung = found[2].clone();
    let mut runner = FakeRunner { hang_on: vec![hung.clone()], ..FakeRunner::default() };
    let mut cache = ScanCache::default();
    scan(&mut cache, &found, &mut runner, &ScanOptions::default()).unwrap();
    let solo = runner.calls.iter().position(|c| *c == vec![hung.clone()]).expect("retried alone");
    assert_eq!(runner.timeouts[solo], Duration::from_secs(60));
    assert_eq!(runner.timeouts[0], Duration::from_secs(20));
}

#[test]
fn every_format_counts_as_planned_even_with_nothing_to_do() {
    let progress = ScanProgress::default();
    let mut cache: ScanCache<Fake> = ScanCache::default();
    let mut runner = FakeRunner::default();
    run_scan(&mut cache, &[], &no_stamp, &mut runner, &ScanOptions::default(), &progress, &mut |_| {}).unwrap();
    run_scan(&mut cache, &bundles(3), &no_stamp, &mut runner, &ScanOptions::default(), &progress, &mut |_| {}).unwrap();
    assert_eq!((progress.planned(), progress.total(), progress.done()), (2, 3, 3));
}

#[test]
fn the_cache_lock_keeps_a_second_scanner_out() {
    use soundcraft_plugin_scan::CacheLock;
    let dir = std::env::temp_dir().join(format!("soundcraft-plugin-scan-lock-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    let path = dir.join("vst3.lock");
    let held = CacheLock::acquire(&path).unwrap();
    let e = CacheLock::acquire(&path).unwrap_err();
    assert!(e.contains("scanning plugins"), "{e}");
    drop(held);
    let again = CacheLock::acquire(&path).unwrap();
    drop(again);
    // Saving through a per-process temporary file leaves no temporary behind.
    let cache: ScanCache<Fake> = ScanCache::default();
    cache.save(&dir.join("vst3.json")).unwrap();
    let names: Vec<String> = std::fs::read_dir(&dir).unwrap().flatten().map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    assert_eq!(names, vec!["vst3.json".to_string(), "vst3.lock".to_string()]);
    let _ = std::fs::remove_dir_all(&dir);
}

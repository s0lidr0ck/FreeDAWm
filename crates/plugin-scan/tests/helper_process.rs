//! The real runner against real child processes: this test binary doubles as a fake scanner
//! helper (started with `--out`), whose "bundles" crash, hang or report by their names.

use soundcraft_plugin_scan::{BatchEnd, BatchRunner, HelperRunner, ResultWriter, ScanCache, ScanOptions, ScanProgress, parse_helper_args, run_scan};
use std::path::PathBuf;
use std::sync::atomic::AtomicBool;
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
struct Fake {
    id: String,
    name: String,
}

impl soundcraft_plugin_scan::Descriptor for Fake {
    fn id(&self) -> &str {
        &self.id
    }
    fn name(&self) -> &str {
        &self.name
    }
}

fn fake_helper() {
    let args = parse_helper_args(std::env::args_os().skip(1)).unwrap();
    let mut out = ResultWriter::create(&args.out).unwrap();
    for b in &args.bundles {
        let name = b.file_stem().unwrap().to_string_lossy().into_owned();
        if name.starts_with("crash") {
            std::process::abort();
        }
        if name.starts_with("hang") {
            std::thread::sleep(Duration::from_secs(600));
        }
        if name.starts_with("bad") {
            out.bundle::<Fake>(b, Err("not a plugin".into())).unwrap();
        } else {
            out.bundle(b, Ok(vec![Fake { id: format!("fake:{name}"), name }])).unwrap();
        }
    }
    out.done().unwrap();
}

fn paths(names: &[&str]) -> Vec<PathBuf> {
    names.iter().map(|n| PathBuf::from(format!("{n}.vst3"))).collect()
}

fn main() {
    if std::env::args().any(|a| a == "--out") {
        fake_helper();
        return;
    }
    let exe = std::env::current_exe().unwrap();
    let mut runner = HelperRunner::new(exe, "vst3");
    let never = AtomicBool::new(false);
    let per = Duration::from_secs(2);

    BatchRunner::<Fake>::check(&mut runner, &never).unwrap();

    let r = BatchRunner::<Fake>::run(&mut runner, &paths(&["a", "bad", "b"]), per, &never);
    assert_eq!(r.end, BatchEnd::Finished);
    assert_eq!(r.results.len(), 3);
    assert_eq!(r.results[1].1, Err("not a plugin".to_string()));
    println!("finished batch: ok");

    let r = BatchRunner::<Fake>::run(&mut runner, &paths(&["a", "crash", "b"]), per, &never);
    assert!(matches!(r.end, BatchEnd::Crashed(_)), "{:?}", r.end);
    assert_eq!(r.results.len(), 1, "what was reported before the crash is kept");
    println!("crashed batch: ok ({:?})", r.end);

    let started = Instant::now();
    let r = BatchRunner::<Fake>::run(&mut runner, &paths(&["a", "hang", "b"]), per, &never);
    assert_eq!(r.end, BatchEnd::TimedOut);
    assert_eq!(r.results.len(), 1);
    assert!(started.elapsed() < Duration::from_secs(10), "stopped after the per-bundle timeout");
    println!("hung batch: ok");

    // End to end: the crashing and hanging bundles are isolated, the others listed.
    let found = paths(&["a", "b", "crash1", "c", "hang1", "d", "bad1", "e"]);
    let mut cache: ScanCache<Fake> = ScanCache::default();
    let opts = ScanOptions { per_bundle: per, solo_timeout: per, ..ScanOptions::default() };
    run_scan(&mut cache, &found, &|_| None, &mut runner, &opts, &ScanProgress::default(), &mut |_| {}).unwrap();
    let names: Vec<String> = cache.plugins().into_iter().map(|p| p.name).collect();
    assert_eq!(names, vec!["a", "b", "c", "d", "e"]);
    let failures = cache.failures();
    assert_eq!(failures.len(), 3, "{failures:?}");
    assert!(failures.iter().any(|(p, r)| p.ends_with("crash1.vst3") && r.contains("crashed during scan")), "{failures:?}");
    assert!(failures.iter().any(|(p, r)| p.ends_with("hang1.vst3") && r.contains("timed out during scan")), "{failures:?}");
    assert!(failures.iter().any(|(p, r)| p.ends_with("bad1.vst3") && r == "not a plugin"), "{failures:?}");
    println!("isolating scan: ok");

    // Cancel stops the running helper at once.
    let cancel = AtomicBool::new(true);
    let r = BatchRunner::<Fake>::run(&mut runner, &paths(&["hang"]), Duration::from_secs(60), &cancel);
    assert_eq!(r.end, BatchEnd::Cancelled);
    println!("cancel: ok");

    // A missing helper is reported once, before any bundle.
    let mut missing = HelperRunner::new(PathBuf::from("no-such-scanner.exe"), "vst3");
    assert!(BatchRunner::<Fake>::check(&mut missing, &never).is_err());

    // The spawn hook sees every helper started.
    static STARTED: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);
    let mut hooked = HelperRunner::new(std::env::current_exe().unwrap(), "vst3").on_spawn(|_| {
        STARTED.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    });
    let _ = BatchRunner::<Fake>::run(&mut hooked, &paths(&["a"]), per, &never);
    assert_eq!(STARTED.load(std::sync::atomic::Ordering::Relaxed), 1);
    println!("spawn hook: ok");
    println!("missing helper: ok");
}

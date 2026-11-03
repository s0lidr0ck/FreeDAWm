//! The process-wide VST3 registry: scan cache, leaked `PluginInfo`s, instantiation.

use crate::ffi::{Bundle, Instance, PARAM_BYPASS, PARAM_HIDDEN, PARAM_PROGRAM_CHANGE, PARAM_READ_ONLY, RawParam};
use crate::plugin::Vst3Plugin;
use crate::scan as paths;
use crate::{AUDIO_MODULE_CLASS, Vst3Descriptor, Vst3Error, Vst3Programs, parse_id};
use soundcraft_dsp::{ParamInfo, Plugin, PluginInfo, Taper, Unit};
use soundcraft_plugin_scan::{CacheLock, HelperRunner, ScanCache, ScanOptions, ScanProgress, Stamp, file_stamp, helper_exe, run_scan};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock, PoisonError};

/// Most labels read for a stepped parameter (more steps are shown as a plain range).
const MAX_CHOICES: i32 = 128;

/// The scan cache's file name in the cache folder.
const CACHE_FILE: &str = "vst3.json";

#[derive(Default)]
struct Registry {
    /// The plugins known now: the scan cache's, then those of folders added with
    /// [`add_search_dir`]. `None` until something has scanned (or a cache file was read).
    scanned: Option<Vec<Vst3Descriptor>>,
    /// The out-of-process scans' results per bundle (see `soundcraft-plugin-scan`).
    cache: Option<ScanCache<Vst3Descriptor>>,
    /// Where the scan cache is saved (`<config>/PluginScan`); `None` keeps it in memory only.
    cache_dir: Option<PathBuf>,
    /// The cache file was read (once per folder).
    cache_read: bool,
    /// Plugins of the folders scanned in-process by [`add_search_dir`], and the files it skipped.
    added: Vec<Vst3Descriptor>,
    added_failures: Vec<(PathBuf, String)>,
    /// Ids looked up and not found: each asks once for a background scan (see
    /// [`take_lookup_request`]).
    requested: std::collections::HashSet<String>,
    /// A lookup is waiting for the app to run a background scan.
    lookup_wanted: bool,
    /// Command-line tools: a missing id runs one blocking scan right away (through the helper),
    /// reporting (done, total) about every half second.
    blocking_lookups: Option<fn(usize, usize)>,
    extra_dirs: Vec<PathBuf>,
    /// One leaked `PluginInfo` per plugin type that was actually instantiated (bounded by the
    /// number of distinct installed plugins).
    infos: HashMap<String, &'static PluginInfo>,
    /// The built-in programs of each plugin type that was instantiated (read with its info).
    programs: HashMap<String, Vst3Programs>,
    /// Ids that failed to instantiate (not retried every UI frame), with the file they live in
    /// and the reason.
    failed: HashMap<String, (PathBuf, String)>,
    /// Files the scans skipped (they would not load, crashed or hung the scanner), with the reason.
    scan_failures: Vec<(PathBuf, String)>,
}

fn registry() -> &'static Mutex<Registry> {
    static R: OnceLock<Mutex<Registry>> = OnceLock::new();
    R.get_or_init(|| Mutex::new(Registry::default()))
}

fn lock() -> std::sync::MutexGuard<'static, Registry> {
    registry().lock().unwrap_or_else(PoisonError::into_inner)
}

/// Held while a scan runs: one at a time.
fn scanning() -> std::sync::MutexGuard<'static, ()> {
    static S: Mutex<()> = Mutex::new(());
    S.lock().unwrap_or_else(PoisonError::into_inner)
}

/// Descriptors of every audio module class in one `.vst3` (loaded in this process).
pub fn load_bundle(path: &Path) -> Result<Vec<Vst3Descriptor>, Vst3Error> {
    let b = Bundle::load(path)?;
    Ok(b.classes()
        .into_iter()
        .filter(|c| c.category == AUDIO_MODULE_CLASS)
        .map(|c| {
            let subs = paths::split_sub_categories(&c.sub_categories);
            let class_id = crate::ffi::hex(&c.cid);
            Vst3Descriptor {
                id: format!("{}{class_id}", crate::ID_PREFIX),
                category: paths::category_from_sub_categories(&subs),
                is_instrument: paths::is_instrument(&subs),
                class_id,
                name: if c.name.is_empty() { "VST3 plugin".to_string() } else { c.name },
                vendor: c.vendor,
                version: c.version,
                sdk_version: c.sdk_version,
                sub_categories: subs,
                path: path.to_path_buf(),
            }
        })
        .collect())
}

/// Scans `dirs` in this process (uncached). Unloadable files are logged and skipped; the first
/// plugin with a given id wins.
pub fn scan_paths(dirs: &[PathBuf]) -> Vec<Vst3Descriptor> {
    scan_dirs(dirs).0
}

/// Like [`scan_paths`], also returning the skipped files with the reason.
fn scan_dirs(dirs: &[PathBuf]) -> (Vec<Vst3Descriptor>, Vec<(PathBuf, String)>) {
    let mut out: Vec<Vst3Descriptor> = Vec::new();
    let mut skipped = Vec::new();
    for path in paths::find_bundles(dirs) {
        match load_bundle(&path) {
            Ok(ds) => {
                for d in ds {
                    if !out.iter().any(|o| o.id == d.id) {
                        out.push(d);
                    }
                }
            }
            Err(e) => {
                log::warn!("{e}");
                skipped.push((path, e.to_string()));
            }
        }
    }
    out.sort_by_key(|d| d.name.to_lowercase());
    (out, skipped)
}

/// Rebuilds the plugin list and the failures from the scan cache and the added folders.
fn refresh(r: &mut Registry) {
    if r.cache.is_none() && r.added.is_empty() && r.added_failures.is_empty() {
        r.scanned = None;
        r.scan_failures.clear();
        return;
    }
    let mut list = r.cache.as_ref().map(ScanCache::plugins).unwrap_or_default();
    for d in &r.added {
        if !list.iter().any(|o| o.id == d.id) {
            list.push(d.clone());
        }
    }
    let mut failures = r.cache.as_ref().map(ScanCache::failures).unwrap_or_default();
    for f in &r.added_failures {
        if !failures.iter().any(|(p, _)| *p == f.0) {
            failures.push(f.clone());
        }
    }
    r.scanned = Some(list);
    r.scan_failures = failures;
}

fn cache_file(r: &Registry) -> Option<PathBuf> {
    r.cache_dir.as_ref().map(|d| d.join(CACHE_FILE))
}

/// Reads the cache file once (no plugin is loaded).
fn read_cache(r: &mut Registry) {
    if r.cache_read {
        return;
    }
    r.cache_read = true;
    if r.cache.is_none()
        && let Some(file) = cache_file(r).filter(|f| f.is_file())
    {
        r.cache = Some(ScanCache::load(&file));
        refresh(r);
    }
}

pub fn set_cache_dir(dir: Option<PathBuf>) {
    let mut r = lock();
    if r.cache_dir != dir {
        r.cache_dir = dir;
        r.cache_read = false;
    }
}

/// A bundle's stamp: its binary's (a bundle folder's own time does not change on updates).
fn stamp(path: &Path) -> Option<Stamp> {
    paths::bundle_binary(path).and_then(|b| file_stamp(&b)).or_else(|| file_stamp(path))
}

/// Scans new and changed bundles of the search folders out of process (see the crate docs).
pub fn scan_with(opts: &ScanOptions, progress: &ScanProgress) -> Result<Vec<Vst3Descriptor>, String> {
    let _one = scanning();
    let (dirs, file, memory) = {
        let mut r = lock();
        read_cache(&mut r);
        let mut dirs = paths::default_search_paths();
        dirs.extend(r.extra_dirs.iter().cloned());
        (dirs, cache_file(&r), r.cache.clone())
    };
    let exe = helper_exe()?;
    // One scanner per cache file (the app and the command line share it), starting from what is
    // on disk now.
    let _held = match &file {
        Some(f) => Some(CacheLock::acquire(&f.with_extension("lock"))?),
        None => None,
    };
    let mut cache = match &file {
        Some(f) if f.is_file() => ScanCache::load(f),
        _ => memory.unwrap_or_default(),
    };
    let found = paths::find_bundles(&dirs);
    let mut runner = HelperRunner::new(exe, "vst3").on_spawn(crate::ffi::tie_to_this_process);
    let ran = run_scan(&mut cache, &found, &stamp, &mut runner, opts, progress, &mut |c| {
        if let Some(f) = &file
            && let Err(e) = c.save(f)
        {
            log::warn!("plugin scan cache {}: {e}", f.display());
        }
        let mut r = lock();
        r.cache = Some(c.clone());
        refresh(&mut r);
    });
    ran.map_err(|e| e.to_string())?;
    Ok(lock().scanned.clone().unwrap_or_default())
}

/// [`scan_with`] without progress; on error, logs it and returns what is known.
fn scan_logged(opts: &ScanOptions) -> Vec<Vst3Descriptor> {
    match scan_with(opts, &ScanProgress::default()) {
        Ok(list) => list,
        Err(e) => {
            log::warn!("VST3 scan: {e}");
            cached().unwrap_or_default()
        }
    }
}

pub fn scan() -> Vec<Vst3Descriptor> {
    scan_logged(&ScanOptions::default())
}

pub fn cached() -> Option<Vec<Vst3Descriptor>> {
    let mut r = lock();
    read_cache(&mut r);
    r.scanned.clone()
}

pub fn rescan() -> Vec<Vst3Descriptor> {
    clear_failed();
    scan_logged(&ScanOptions { retry_failed: true, ..ScanOptions::default() })
}

/// Forgets the plugins that failed to instantiate (a rescan retries them).
pub fn clear_failed() {
    let mut r = lock();
    r.failed.clear();
}

pub fn add_search_dir(dir: &Path) -> usize {
    let mut r = lock();
    if !r.extra_dirs.iter().any(|d| d == dir) {
        r.extra_dirs.push(dir.to_path_buf());
    }
    let (found, skipped) = scan_dirs(&[dir.to_path_buf()]);
    for s in skipped {
        if !r.added_failures.iter().any(|(p, _)| *p == s.0) {
            r.added_failures.push(s);
        }
    }
    let n = found.len();
    for d in found {
        if !r.added.iter().any(|o| o.id == d.id) {
            r.added.push(d);
        }
    }
    refresh(&mut r);
    n
}

/// The descriptor of `id` from what is known, without scanning.
fn known(r: &mut Registry, id: &str) -> Option<Vst3Descriptor> {
    read_cache(r);
    r.scanned.as_ref().and_then(|l| l.iter().find(|d| d.id == id).cloned())
}

/// The descriptor of `id` from the scan cache, never scanning nor waiting for a scan: an id the
/// cache lacks, or whose bundle is gone, asks (once) for a background scan
/// ([`take_lookup_request`]) and is not found for now. With [`set_blocking_lookups`] (command-line
/// tools) it runs that scan here instead, once per id, through the helper.
fn find(id: &str) -> Option<Vst3Descriptor> {
    let report = {
        let mut r = lock();
        if let Some(d) = known(&mut r, id).filter(|d| d.path.exists()) {
            return Some(d);
        }
        if !r.requested.insert(id.to_string()) {
            return None;
        }
        match r.blocking_lookups {
            Some(report) => report,
            None => {
                r.lookup_wanted = true;
                return None;
            }
        }
    };
    blocking_scan(report);
    known(&mut lock(), id).filter(|d| d.path.exists())
}

/// Runs one incremental scan (through the helper) on a worker while this thread reports its
/// progress; for [`set_blocking_lookups`].
fn blocking_scan(report: fn(usize, usize)) {
    let progress = ScanProgress::default();
    let done = std::sync::atomic::AtomicBool::new(false);
    std::thread::scope(|s| {
        let worker = s.spawn(|| {
            let r = scan_with(&ScanOptions::default(), &progress);
            done.store(true, std::sync::atomic::Ordering::Release);
            r
        });
        while !done.load(std::sync::atomic::Ordering::Acquire) {
            std::thread::sleep(std::time::Duration::from_millis(500));
            if !done.load(std::sync::atomic::Ordering::Acquire) {
                report(progress.done(), progress.total());
            }
        }
        match worker.join() {
            Ok(Ok(_)) => report(progress.done(), progress.total()),
            Ok(Err(e)) => log::warn!("VST3 scan: {e}"),
            Err(_) => log::warn!("VST3 scan: the scan thread failed"),
        }
    });
}

/// Command-line tools (no UI to keep responsive): a plugin missing from the scan cache runs one
/// blocking incremental scan through the helper before it is given up, calling `report` with the
/// progress. `None` (the default, the app) asks for a background scan instead.
pub fn set_blocking_lookups(report: Option<fn(usize, usize)>) {
    lock().blocking_lookups = report;
}

/// Whether a plugin was looked up and not found since the last call: the app then scans new and
/// changed bundles in the background and creates the missing plugins again.
pub fn take_lookup_request() -> bool {
    std::mem::take(&mut lock().lookup_wanted)
}

/// Runs `f` while holding the scan lock, as a running scan does (tests check that lookups never
/// wait for a scan).
pub fn with_scan_running<R>(f: impl FnOnce() -> R) -> R {
    let _one = scanning();
    f()
}

pub fn loaded_paths() -> Vec<PathBuf> {
    crate::ffi::loaded_paths()
}

fn leak(s: String) -> &'static str {
    Box::leak(s.into_boxed_str())
}

/// Maps a VST3 unit label to a SoundCraft display unit.
pub(crate) fn unit_from_label(units: &str) -> Unit {
    match units.trim().to_ascii_lowercase().as_str() {
        "db" => Unit::Db,
        "hz" => Unit::Hz,
        "ms" => Unit::Ms,
        "%" => Unit::Percent,
        "s" | "sec" | "secs" | "seconds" => Unit::Seconds,
        "st" | "semi" | "semitones" => Unit::Semitones,
        "ct" | "cent" | "cents" => Unit::Cents,
        _ => Unit::None,
    }
}

fn param_info(inst: &Instance, p: &RawParam) -> ParamInfo {
    let name = if p.title.is_empty() { format!("Param {}", p.id) } else { p.title.clone() };
    let base = ParamInfo {
        id: leak(p.id.to_string()),
        name: leak(name),
        min: 0.0,
        max: 1.0,
        default: 0.0,
        unit: Unit::None,
        taper: Taper::Linear,
        choices: &[],
        display: None,
        group: "",
    };
    if p.step_count == 1 {
        return ParamInfo { unit: Unit::Toggle, default: p.default_normalized.round() as f32, ..base };
    }
    if (2..=MAX_CHOICES).contains(&p.step_count) {
        let steps = f64::from(p.step_count);
        let labels: Vec<&'static str> =
            (0..=p.step_count).map(|i| leak(inst.value_text(p.id, f64::from(i) / steps).unwrap_or_else(|| i.to_string()))).collect();
        return ParamInfo {
            max: p.step_count as f32,
            default: (p.default_normalized * steps).round() as f32,
            unit: Unit::Choice,
            choices: Box::leak(labels.into_boxed_slice()),
            ..base
        };
    }
    // Continuous (or too many steps for a menu): plain range from the controller.
    let unit = unit_from_label(&p.units);
    match (inst.to_plain(p.id, 0.0), inst.to_plain(p.id, 1.0)) {
        (Some(lo), Some(hi)) if hi > lo && (lo as f32).is_finite() && (hi as f32).is_finite() && (hi as f32) > (lo as f32) => {
            let default = inst.to_plain(p.id, p.default_normalized).map_or(lo, |d| d.clamp(lo, hi));
            // A logarithmic plugin mapping (frequency knobs) shows its geometric mean at mid travel.
            let taper = match inst.to_plain(p.id, 0.5) {
                Some(mid)
                    if lo > 0.0
                        && ((mid - (lo * hi).sqrt()).abs() <= 0.01 * (lo * hi).sqrt())
                        && (mid - (lo + hi) / 2.0).abs() > 0.01 * (hi - lo) =>
                {
                    Taper::Log
                }
                _ => Taper::Linear,
            };
            ParamInfo { min: lo as f32, max: hi as f32, default: default as f32, unit, taper, ..base }
        }
        _ => ParamInfo { default: p.default_normalized as f32, unit, ..base },
    }
}

/// Builds and leaks the `PluginInfo` for one plugin type. Called at most once per id (cached).
fn build_info(d: &Vst3Descriptor, inst: &Instance) -> &'static PluginInfo {
    let params: Vec<ParamInfo> =
        inst.params().iter().filter(|p| p.flags & (PARAM_HIDDEN | PARAM_READ_ONLY | PARAM_BYPASS) == 0).map(|p| param_info(inst, p)).collect();
    let short: String = d.name.chars().take(8).collect();
    Box::leak(Box::new(PluginInfo {
        id: leak(d.id.clone()),
        name: leak(d.name.clone()),
        short_name: leak(short),
        category: d.category,
        params: Box::leak(params.into_boxed_slice()),
        is_instrument: d.is_instrument,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    }))
}

/// The plugin's built-in programs: the names in the program list of the program-change
/// parameter's unit and that parameter's value for each (as `info`'s parameters take values).
fn read_programs(inst: &Instance, info: &PluginInfo) -> Vst3Programs {
    let raw = inst.params();
    let usable = |p: &&RawParam| p.flags & PARAM_PROGRAM_CHANGE != 0 && p.flags & (PARAM_HIDDEN | PARAM_READ_ONLY | PARAM_BYPASS) == 0;
    let Some(p) = raw.iter().filter(usable).min_by_key(|p| p.unit != 0) else { return Vst3Programs::default() };
    let Some(pi) = info.params.iter().find(|x| x.id == p.id.to_string()) else { return Vst3Programs::default() };
    let mut names = inst.program_names(p.unit);
    if p.step_count > 0 {
        names.truncate(usize::try_from(p.step_count).unwrap_or(0).saturating_add(1));
    }
    let last = names.len().saturating_sub(1).max(1) as f64;
    let steps = if p.step_count > 0 { f64::from(p.step_count) } else { last };
    let plain_range = matches!((inst.to_plain(p.id, 0.0), inst.to_plain(p.id, 1.0)), (Some(lo), Some(hi))
        if (lo as f32 - pi.min).abs() <= 1e-6 * pi.min.abs().max(1.0) && (hi as f32 - pi.max).abs() <= 1e-6 * pi.max.abs().max(1.0));
    let values = (0..names.len())
        .map(|i| {
            let n = (i as f64 / steps).clamp(0.0, 1.0);
            let v = match pi.unit {
                Unit::Toggle | Unit::Choice => i as f64,
                _ if plain_range => inst.to_plain(p.id, n).unwrap_or(n),
                _ => f64::from(pi.min) + n * f64::from(pi.max - pi.min),
            };
            pi.clamp(v as f32)
        })
        .collect();
    Vst3Programs { param: pi.id.to_string(), names, values }
}

/// Finds the descriptor, loads its binary and creates an instance plus its (cached) info.
fn instantiate_raw(id: &str) -> Result<(Instance, &'static PluginInfo), Vst3Error> {
    let cid = parse_id(id).ok_or_else(|| Vst3Error::NotFound(id.to_string()))?;
    if lock().failed.contains_key(id) {
        return Err(Vst3Error::Instantiate(id.to_string(), "failed before".into()));
    }
    // Only this plugin's own bundle is loaded, found through the scan cache.
    let desc = find(id).ok_or_else(|| Vst3Error::NotFound(id.to_string()))?;
    let made = Bundle::load(&desc.path).and_then(|b| b.instantiate(&cid));
    let mut r = lock();
    match made {
        Ok(inst) => {
            let info = match r.infos.get(id) {
                Some(i) => *i,
                None => {
                    let i = build_info(&desc, &inst);
                    r.infos.insert(id.to_string(), i);
                    r.programs.insert(id.to_string(), read_programs(&inst, i));
                    i
                }
            };
            Ok((inst, info))
        }
        Err(e) => {
            r.failed.insert(id.to_string(), (desc.path.clone(), e.to_string()));
            Err(e)
        }
    }
}

pub fn instantiate(id: &str) -> Result<Box<dyn Plugin>, Vst3Error> {
    Ok(Box::new(instantiate_plugin(id)?))
}

pub fn instantiate_plugin(id: &str) -> Result<Vst3Plugin, Vst3Error> {
    let (inst, info) = instantiate_raw(id)?;
    let mut p = Vst3Plugin::new(inst, info);
    p.prepare(48_000.0, 1024, 2);
    Ok(p)
}

/// The files the scans skipped and the plugins that failed to instantiate, each with the reason
/// (scan failures first, then instantiate failures by id). Never scans.
pub fn scan_failures() -> Vec<(PathBuf, String)> {
    failures_of(&lock())
}

fn failures_of(r: &Registry) -> Vec<(PathBuf, String)> {
    let mut failed: Vec<(&String, &(PathBuf, String))> = r.failed.iter().collect();
    failed.sort_by(|a, b| a.0.cmp(b.0));
    r.scan_failures.iter().cloned().chain(failed.into_iter().map(|(_, f)| f.clone())).collect()
}

/// Whether [`programs`] knows the plugin (it was instantiated in this process).
pub fn programs_known(id: &str) -> bool {
    lock().programs.contains_key(id)
}

/// The built-in programs of a plugin instantiated in this process (`None` before that: listing
/// them never loads a plugin).
pub fn programs(id: &str) -> Option<Vst3Programs> {
    lock().programs.get(id).cloned()
}

pub fn plugin_info(id: &str) -> Option<&'static PluginInfo> {
    parse_id(id)?;
    {
        let r = lock();
        if let Some(i) = r.infos.get(id) {
            return Some(*i);
        }
        if r.failed.contains_key(id) {
            return None;
        }
    }
    // Instantiate once to learn the parameters (VST3 exposes them only on a live controller).
    instantiate_raw(id).ok().map(|(_, info)| info)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn unit_labels_map() {
        assert_eq!(unit_from_label("dB"), Unit::Db);
        assert_eq!(unit_from_label(" Hz "), Unit::Hz);
        assert_eq!(unit_from_label("%"), Unit::Percent);
        assert_eq!(unit_from_label("ms"), Unit::Ms);
        assert_eq!(unit_from_label("sec"), Unit::Seconds);
        assert_eq!(unit_from_label("cents"), Unit::Cents);
        assert_eq!(unit_from_label("st"), Unit::Semitones);
        assert_eq!(unit_from_label("furlongs"), Unit::None);
    }

    #[test]
    fn skipped_files_are_reported_with_the_reason() {
        let d = std::env::temp_dir().join(format!("soundcraft-vst3-skips-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        std::fs::write(d.join("garbage.vst3"), b"ELF this is not a plugin  ").unwrap();
        let (found, skipped) = scan_dirs(std::slice::from_ref(&d));
        assert!(found.is_empty());
        assert_eq!(skipped.len(), 1, "{skipped:?}");
        assert_eq!(skipped[0].0, d.join("garbage.vst3"));
        assert!(!skipped[0].1.is_empty());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn failures_list_scan_skips_then_instantiate_failures_by_id() {
        let mut r = Registry::default();
        r.scan_failures.push((PathBuf::from("bad.vst3"), "not a plugin".into()));
        r.failed.insert("vst3:b".into(), (PathBuf::from("b.vst3"), "b failed".into()));
        r.failed.insert("vst3:a".into(), (PathBuf::from("a.vst3"), "a failed".into()));
        let want: Vec<(PathBuf, String)> = vec![
            (PathBuf::from("bad.vst3"), "not a plugin".into()),
            (PathBuf::from("a.vst3"), "a failed".into()),
            (PathBuf::from("b.vst3"), "b failed".into()),
        ];
        assert_eq!(failures_of(&r), want);
    }
}

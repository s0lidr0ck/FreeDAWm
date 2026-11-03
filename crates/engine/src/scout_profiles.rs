//! Scout sound profiles (spec 2026-10-29-scout-profiles §1): measuring audio offline with Scout's
//! meter, the profile library (one JSON file per profile in `<settings>/profiles`, outside every
//! session) and per-stem capture through the session's stem separation.
//!
//! The `scout.profile_*` commands (`cmd::scout_profile`) are the only callers in the app; the
//! library functions take the folder so tests and hosts choose it.

use soundcraft_model::{ProfileKind, ScoutMetrics, ScoutProfile, profile_file_stem};
use std::path::{Path, PathBuf};

/// Most profile files the library reads.
pub const MAX_PROFILES: usize = 10_000;
/// Frames fed to the meter per block.
const BLOCK: usize = 4096;
/// Most bytes of an audio file measured as a reference.
pub const MAX_REFERENCE_BYTES: u64 = 2 << 30;

/// Scout's measurements of `audio` (channels at `rate`), as a capture of a strip would take them:
/// mono reads as both sides, more than two channels read the first two.
pub fn measure(audio: &[Vec<f32>], rate: f64) -> ScoutMetrics {
    let mut meter = soundcraft_dsp::scout::ScoutMeter::new(rate);
    meter.start();
    let empty: Vec<f32> = Vec::new();
    let l = audio.first().unwrap_or(&empty);
    let r = audio.get(1).unwrap_or(l);
    let frames = l.len().min(r.len());
    let mut at = 0;
    while at < frames {
        let end = (at + BLOCK).min(frames);
        if let (Some(a), Some(b)) = (l.get(at..end), r.get(at..end)) {
            meter.feed(a, b);
        }
        at = end;
    }
    soundcraft_mix::scout::to_model(&meter.finish())
}

/// The library folder: `$FREEDAWM_CONFIG_DIR/profiles`, else `<settings folder>/profiles`.
pub fn default_dir() -> Option<PathBuf> {
    if let Some(d) = crate::brand::env_var_os("CONFIG_DIR").filter(|d| !d.is_empty()) {
        return Some(PathBuf::from(d).join("profiles"));
    }
    crate::brand::profile_settings_dirs(crate::brand::Platform::current(), |n| std::env::var_os(n)).map(|(d, _)| d.join("profiles"))
}

/// Every profile in `dir` with its file, sorted by name. Unreadable files are skipped; a missing
/// folder is an empty library.
pub fn list(dir: &Path) -> Vec<(PathBuf, ScoutProfile)> {
    let mut out: Vec<(PathBuf, ScoutProfile)> = crate::io::list_dir(dir)
        .into_iter()
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
        .take(MAX_PROFILES)
        .filter_map(|p| read(&p).ok().map(|prof| (p, prof)))
        .collect();
    out.sort_by(|a, b| a.1.name.to_lowercase().cmp(&b.1.name.to_lowercase()).then_with(|| a.0.cmp(&b.0)));
    out
}

/// Reads one profile file (size-capped, lenient).
pub fn read(path: &Path) -> Result<ScoutProfile, String> {
    let bytes = crate::io::read_file_max(path, soundcraft_model::MAX_PROFILE_BYTES as u64).map_err(|e| e.to_string())?;
    let text = String::from_utf8(bytes).map_err(|e| format!("{}: {e}", path.display()))?;
    ScoutProfile::from_json(&text).map_err(|e| format!("{}: {e}", path.display()))
}

/// The profile named `name` (case-insensitive) in `dir`.
pub fn find(dir: &Path, name: &str) -> Option<(PathBuf, ScoutProfile)> {
    let name = name.trim();
    list(dir).into_iter().find(|(_, p)| p.name.eq_ignore_ascii_case(name))
}

/// `name`, or `name 2`, `name 3`… the first no profile in `dir` has.
pub fn unique_name(dir: &Path, name: &str) -> String {
    let taken: Vec<String> = list(dir).into_iter().map(|(_, p)| p.name.to_lowercase()).collect();
    let base = name.trim();
    let base = if base.is_empty() { "Profile" } else { base };
    if !taken.contains(&base.to_lowercase()) {
        return base.to_string();
    }
    (2..=MAX_PROFILES + 1).map(|n| format!("{base} {n}")).find(|c| !taken.contains(&c.to_lowercase())).unwrap_or_else(|| base.to_string())
}

/// Saves `p` as a new file in `dir` (made if missing), under a name no other profile has
/// (`p.name` is changed to it). → the file.
pub fn save_new(dir: &Path, p: &mut ScoutProfile) -> Result<PathBuf, String> {
    p.sanitize();
    p.name = unique_name(dir, &p.name);
    crate::io::make_dir(dir).map_err(|e| e.to_string())?;
    let stem = profile_file_stem(&p.name);
    let path = (1..=MAX_PROFILES)
        .map(|n| if n == 1 { dir.join(format!("{stem}.json")) } else { dir.join(format!("{stem} ({n}).json")) })
        .find(|f| !crate::io::file_found(f))
        .ok_or_else(|| "too many profiles".to_string())?;
    write(&path, p)?;
    Ok(path)
}

/// Writes `p` to `path` (whole file, through a temporary file).
pub fn write(path: &Path, p: &ScoutProfile) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    crate::io::write_path(&tmp, p.to_json()).map_err(|e| e.to_string())?;
    crate::io::rename_path(&tmp, path).map_err(|e| {
        let _ = crate::io::remove_path(&tmp);
        e.to_string()
    })
}

/// A profile of `metrics` with its defaults filled in: kind, role (`mix` for a mix), creation time.
pub fn make(name: &str, kind: ProfileKind, role: &str, tags: Vec<String>, metrics: ScoutMetrics, created: String) -> ScoutProfile {
    let mut p = ScoutProfile::new(name, metrics);
    p.kind = kind;
    p.role = if role.trim().is_empty() && kind == ProfileKind::Mix { "mix".into() } else { role.trim().to_string() };
    p.tags = tags;
    p.created = created;
    p.bands_version = soundcraft_dsp::scout::BANDS_VERSION;
    p
}

/// Decodes an audio file for measuring: → (channels, rate). Size-capped.
pub fn read_audio(path: &Path) -> Result<(Vec<Vec<f32>>, u32), String> {
    let bytes = crate::io::read_file_max(path, MAX_REFERENCE_BYTES).map_err(|e| e.to_string())?;
    let ext = path.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (_, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|e| format!("{}: {e}", path.display()))?;
    if buf.channels.first().is_none_or(Vec::is_empty) {
        return Err(format!("{}: no audio", path.display()));
    }
    Ok((buf.channels, buf.sample_rate))
}

/// Separates a song into stems: `(stem name, channels)`. Blocking.
pub type Separator<'a> = &'a dyn Fn(Vec<Vec<f32>>, u32) -> Result<Vec<(String, Vec<Vec<f32>>)>, String>;

/// The best stem separation the installed models allow, as Separate Stems opens with
/// ([`soundcraft_ml::pipeline::SepModel::best_installed`]): HTDemucs when no RoFormer is installed.
#[cfg(not(target_arch = "wasm32"))]
pub fn best_separation_model() -> crate::ml::SepModel {
    use crate::ml::{ModelStatus, SepModel, list_models, models_dir};
    let models = list_models(&models_dir());
    SepModel::best_installed(|id| models.iter().any(|m| m.model.id == id && matches!(m.status, ModelStatus::Installed(_))))
}

/// The session's stem separation with `model`.
#[cfg(not(target_arch = "wasm32"))]
pub fn separate_with(model: crate::ml::SepModel, audio: Vec<Vec<f32>>, rate: u32) -> Result<Vec<(String, Vec<Vec<f32>>)>, String> {
    use crate::ml::{Cancel, Ctx, Device, JobKind, Quality};
    let job = soundcraft_ml::Job {
        kind: JobKind::Separate { model },
        audio,
        rate,
        device: Device::Auto,
        models_dir: crate::ml::models_dir(),
        quality: Quality::Fast,
    };
    let ctx = Ctx::new(Cancel::new(), |_, _| {});
    soundcraft_ml::pipeline::run(&job, &ctx).map(|o| o.stems.into_iter().map(|s| (s.name, s.audio)).collect()).map_err(|e| e.to_string())
}

/// The separation's model name as a profile records it when the separator doesn't say
/// (`ml.separate`'s default, HTDemucs).
pub const DEFAULT_SEPARATION_MODEL: &str = "demucs";

/// "Capture per stem": one mix profile of `audio` named `<song> · mix`, and one track profile per
/// stem named `<song> · <stem>` (role from the stem name), all measured at `rate`.
pub fn stem_profiles(
    song: &str,
    audio: Vec<Vec<f32>>,
    rate: u32,
    separate: Separator<'_>,
    model: &str,
    source: &soundcraft_model::ProfileSource,
    created: &str,
) -> Result<Vec<ScoutProfile>, String> {
    let mix = measure(&audio, f64::from(rate));
    let stems = separate(audio, rate)?;
    let mut out = Vec::with_capacity(stems.len() + 1);
    let mut p = make(&format!("{song} · mix"), ProfileKind::Mix, "mix", Vec::new(), mix, created.to_string());
    p.source = source.clone();
    out.push(p);
    for (stem, a) in stems {
        let mut p =
            make(&format!("{song} · {stem}"), ProfileKind::Track, stem_role(&stem), Vec::new(), measure(&a, f64::from(rate)), created.to_string());
        p.source = source.clone();
        p.source.stem = Some(stem);
        p.source.model = Some(model.to_string());
        out.push(p);
    }
    Ok(out)
}

/// The role a stem's name suggests.
pub fn stem_role(stem: &str) -> &'static str {
    let s = stem.to_ascii_lowercase();
    if s.contains("vocal") {
        "vocal"
    } else if s.contains("drum") {
        "drums"
    } else if s.contains("bass") {
        "bass"
    } else if s.contains("guitar") {
        "guitar"
    } else if s.contains("piano") || s.contains("keys") {
        "keys"
    } else {
        "other"
    }
}

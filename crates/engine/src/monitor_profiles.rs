//! The monitor profile library: hearing corrections and simulated outputs for the monitor stage
//! (`soundcraft_dsp::monitor`), one JSON file each in `Monitor Profiles/Hearing` and
//! `Monitor Profiles/Devices` of the user's Library folder (the one the presets and templates
//! live in, so a Library that is synced between computers carries them), outside every session.
//! The library functions take the `Monitor Profiles` folder so tests and hosts choose it.

use soundcraft_dsp::monitor::{Curve, Plan, Sim, ear_curves};
use soundcraft_model::monitor::{DeviceProfile, HearingProfile, MAX_MONITOR_PROFILE_BYTES};
use soundcraft_model::profile_file_stem;
use std::path::{Path, PathBuf};

/// Most profile files of a kind the library reads.
pub const MAX_PROFILES: usize = 1000;

/// The folder's name in the Library.
pub const FOLDER: &str = "Monitor Profiles";

/// The folder in a Library at `library_root`.
pub fn dir_in(library_root: &Path) -> PathBuf {
    library_root.join(FOLDER)
}

fn hearing_dir(dir: &Path) -> PathBuf {
    dir.join("Hearing")
}

fn devices_dir(dir: &Path) -> PathBuf {
    dir.join("Devices")
}

fn texts(dir: &Path) -> impl Iterator<Item = String> {
    let mut files: Vec<PathBuf> =
        crate::io::list_dir(dir).into_iter().filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json"))).take(MAX_PROFILES).collect();
    files.sort();
    files.into_iter().filter_map(|p| crate::io::read_file_max(&p, MAX_MONITOR_PROFILE_BYTES as u64).ok()).filter_map(|b| String::from_utf8(b).ok())
}

/// Every hearing profile in the library `dir`, sorted by name. Unreadable and unnamed files are
/// skipped; a missing folder is an empty library.
pub fn hearing(dir: &Path) -> Vec<HearingProfile> {
    let mut out: Vec<HearingProfile> =
        texts(&hearing_dir(dir)).filter_map(|t| HearingProfile::from_json(&t).ok()).filter(|p| !p.name.is_empty()).collect();
    out.sort_by_key(|p| p.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    out
}

/// The measured outputs that come with the app (`monitor_devices/*.json`), in name order: each the
/// frequency response and stereo width of one of the project owner's captures of the capture test
/// file, level-trimmed so nothing is boosted above the mix.
const MEASURED: &[&str] = &[
    include_str!("monitor_devices/AirPods.json"),
    include_str!("monitor_devices/AL5.json"),
    include_str!("monitor_devices/BTANC45.json"),
    include_str!("monitor_devices/GamingLaptop15.json"),
    include_str!("monitor_devices/Genilux4.json"),
    include_str!("monitor_devices/IEM.json"),
    include_str!("monitor_devices/iphone23.json"),
    include_str!("monitor_devices/PASpeakers.json"),
    include_str!("monitor_devices/SmallBluetooth.json"),
    include_str!("monitor_devices/SmallRadio.json"),
    include_str!("monitor_devices/StandardCarDriver.json"),
    include_str!("monitor_devices/Studio701.json"),
    include_str!("monitor_devices/Studio7506.json"),
    include_str!("monitor_devices/Studio77.json"),
    include_str!("monitor_devices/StudioD900.json"),
    include_str!("monitor_devices/TV22.json"),
    include_str!("monitor_devices/TV37.json"),
    include_str!("monitor_devices/WhiteVintage7.json"),
    include_str!("monitor_devices/YellowProV8.json"),
];

/// The simulated outputs that come with the app: Mono, then the measured ones.
pub fn builtin_devices() -> Vec<DeviceProfile> {
    let mono = DeviceProfile { name: "Mono".into(), width: 0.0, notes: "Both sides summed, nothing else.".into(), ..DeviceProfile::default() };
    std::iter::once(mono).chain(MEASURED.iter().filter_map(|json| DeviceProfile::from_json(json).ok())).filter(|d| !d.name.is_empty()).collect()
}

/// Every simulated output: the user's files in the library `dir` (sorted by name), then the
/// built-in ones a file of the same name does not replace.
pub fn devices(dir: Option<&Path>) -> Vec<DeviceProfile> {
    let mut out: Vec<DeviceProfile> = dir
        .map(|d| texts(&devices_dir(d)).filter_map(|t| DeviceProfile::from_json(&t).ok()).filter(|p| !p.name.is_empty()).collect())
        .unwrap_or_default();
    out.sort_by_key(|p| p.name.to_lowercase());
    out.dedup_by(|a, b| a.name.eq_ignore_ascii_case(&b.name));
    for b in builtin_devices() {
        if !out.iter().any(|p| p.name.eq_ignore_ascii_case(&b.name)) {
            out.push(b);
        }
    }
    out
}

/// The hearing profile named `name` (case-insensitive).
pub fn find_hearing(dir: &Path, name: &str) -> Option<HearingProfile> {
    hearing(dir).into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

/// The simulated output named `name` (case-insensitive).
pub fn find_device(dir: Option<&Path>, name: &str) -> Option<DeviceProfile> {
    devices(dir).into_iter().find(|p| p.name.eq_ignore_ascii_case(name.trim()))
}

fn write(path: &Path, json: String) -> Result<(), String> {
    let tmp = path.with_extension("json.tmp");
    crate::io::write_path(&tmp, json).map_err(|e| e.to_string())?;
    crate::io::rename_path(&tmp, path).map_err(|e| {
        let _ = crate::io::remove_path(&tmp);
        e.to_string()
    })
}

/// Saves a hearing profile in the library `dir` (made if missing), replacing the one of the same
/// name. → the file.
pub fn save_hearing(dir: &Path, p: &HearingProfile) -> Result<PathBuf, String> {
    let mut p = p.clone();
    p.sanitize();
    if p.name.is_empty() {
        return Err("a profile needs a name".into());
    }
    let folder = hearing_dir(dir);
    crate::io::make_dir(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{}.json", profile_file_stem(&p.name)));
    write(&path, p.to_json())?;
    Ok(path)
}

/// Saves a simulated output in the library `dir` (made if missing), replacing the one of the
/// same name. → the file.
pub fn save_device(dir: &Path, p: &DeviceProfile) -> Result<PathBuf, String> {
    let mut p = p.clone();
    p.sanitize();
    if p.name.is_empty() {
        return Err("a profile needs a name".into());
    }
    let folder = devices_dir(dir);
    crate::io::make_dir(&folder).map_err(|e| e.to_string())?;
    let path = folder.join(format!("{}.json", profile_file_stem(&p.name)));
    write(&path, p.to_json())?;
    Ok(path)
}

/// The monitor stage's plan at `rate`: each stage's filters from its profile (kept while the
/// stage is off, so turning it off fades out of them).
pub fn plan(rate: f32, ear: Option<&HearingProfile>, ear_on: bool, sim: Option<&DeviceProfile>, sim_on: bool) -> Plan {
    let sim = sim.map(|d| {
        let mut curve = Curve::fit(&d.curve, d.trim_db, rate);
        if let Some(hz) = d.highpass_hz {
            curve = curve.cut(false, hz, rate);
        }
        if let Some(hz) = d.lowpass_hz {
            curve = curve.cut(true, hz, rate);
        }
        Sim { curve, width: d.width }
    });
    Plan { rate, sim_on: sim_on && sim.is_some(), sim, ear: ear.map(|h| ear_curves(&h.left, &h.right, rate)), ear_on: ear_on && ear.is_some() }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_library_saves_lists_and_finds_profiles_and_user_devices_replace_built_in_ones() {
        let dir = std::env::temp_dir().join(format!("soundcraft-monitor-profiles-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let dir = dir.as_path();
        assert!(hearing(dir).is_empty());
        assert_eq!(devices(Some(dir)).len(), builtin_devices().len());
        let me = HearingProfile { name: "Me / HD600".into(), headphones: "HD 600".into(), left: vec![[4000.0, 6.0]], right: vec![] };
        let file = save_hearing(dir, &me).unwrap();
        assert!(file.starts_with(dir.join("Hearing")));
        save_hearing(dir, &HearingProfile { left: vec![[4000.0, 9.0]], ..me.clone() }).unwrap();
        assert_eq!(hearing(dir).len(), 1);
        assert_eq!(find_hearing(dir, "me / hd600").unwrap().left, vec![[4000.0, 9.0]]);
        assert!(save_hearing(dir, &HearingProfile::default()).is_err());
        let mine = DeviceProfile { name: "mono".into(), width: 0.5, ..DeviceProfile::default() };
        save_device(dir, &mine).unwrap();
        let all = devices(Some(dir));
        assert_eq!(all.len(), builtin_devices().len());
        assert_eq!(find_device(Some(dir), "Mono").unwrap().width, 0.5);
        assert_eq!(find_device(None, "Mono").unwrap().width, 0.0);
        std::fs::write(dir.join("Devices").join("broken.json"), "{").unwrap();
        assert_eq!(devices(Some(dir)).len(), all.len());
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn a_plan_holds_each_stage_only_with_its_profile() {
        let phone = find_device(None, "iphone23").unwrap();
        let p = plan(48_000.0, None, true, Some(&phone), true);
        assert!(p.sim_on && !p.ear_on && p.ear.is_none());
        let s = p.sim.unwrap();
        assert!(s.width < 0.1);
        assert!(s.curve.magnitude_db(80.0, 48_000.0) < -35.0 && s.curve.magnitude_db(4000.0, 48_000.0) > -6.0);
        assert!(plan(48_000.0, None, true, None, true).is_idle());
        // Every embedded file reads, and none boosts the mix. (The bells are designed to keep
        // their analogue shape near the top of the band, which lets the fit follow a measured
        // rise at 15 kHz a little more closely than a squeezed bell did: 0.61 dB at most.)
        assert_eq!(builtin_devices().len(), MEASURED.len() + 1);
        for d in builtin_devices() {
            let c = plan(48_000.0, None, false, Some(&d), true).sim.unwrap().curve;
            assert!((0..120).all(|i| c.magnitude_db(20.0 * 2f32.powf(i as f32 / 12.0), 48_000.0) < 0.75), "{}", d.name);
        }
    }
}

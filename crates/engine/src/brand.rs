//! The product's public name (FreeDAWm; the crates keep their `soundcraft-*` names), its
//! environment variables and its settings folders, shared by the desktop app and the CLI.
//!
//! - Environment variables are `FREEDAWM_*`; the old `SOUNDCRAFT_*` name of each is still read
//!   when the new one is not set, so existing scripts keep working ([`env_var_os`]).
//! - The settings folder moved from `SoundCraft` (`soundcraft` on Linux) to `FreeDAWm`
//!   (`freedawm`). On the first start after the rename, [`migrate_folder`] copies the old folder
//!   into the new one (entry by entry, never overwriting, until a run finishes clean and leaves
//!   [`MIGRATION_MARKER`] in the new folder), and leaves the old one as a backup.

use std::ffi::OsString;
use std::io;
use std::path::{Path, PathBuf};

/// The app's public name.
pub const APP_NAME: &str = "FreeDAWm";
/// The app's name before the rename (settings folders are migrated from it).
pub const LEGACY_APP_NAME: &str = "SoundCraft";
/// Prefix of the environment variables users and tests set.
pub const ENV_PREFIX: &str = "FREEDAWM_";
/// Prefix those variables had before the rename, still read as a fallback.
pub const LEGACY_ENV_PREFIX: &str = "SOUNDCRAFT_";
/// Portable mode's data folder beside the executable.
pub const PORTABLE_DATA_DIR: &str = "FreeDAWmData";
/// Portable mode's data folder before the rename.
pub const LEGACY_PORTABLE_DATA_DIR: &str = "SoundCraftData";

/// `FREEDAWM_<suffix>` when it is set (even to an empty value), else `SOUNDCRAFT_<suffix>`.
pub fn env_var_os(suffix: &str) -> Option<OsString> {
    env_var_os_with(suffix, |name| std::env::var_os(name))
}

/// [`env_var_os`] with the environment given (tests).
pub fn env_var_os_with(suffix: &str, env: impl Fn(&str) -> Option<OsString>) -> Option<OsString> {
    env(&format!("{ENV_PREFIX}{suffix}")).or_else(|| env(&format!("{LEGACY_ENV_PREFIX}{suffix}")))
}

/// [`env_var_os`] as UTF-8 (`None` when unset or not Unicode).
pub fn env_var(suffix: &str) -> Option<String> {
    env_var_os(suffix).and_then(|v| v.into_string().ok())
}

/// The platform whose settings-folder convention applies.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Platform {
    Mac,
    Windows,
    Other,
}

impl Platform {
    /// The platform this build runs on.
    pub fn current() -> Self {
        if cfg!(target_os = "macos") {
            Self::Mac
        } else if cfg!(windows) {
            Self::Windows
        } else {
            Self::Other
        }
    }
}

/// The settings folder in the user profile and the pre-rename one it is migrated from:
/// `%APPDATA%\FreeDAWm` (Windows), `~/Library/Application Support/FreeDAWm` (macOS),
/// `$XDG_CONFIG_HOME/freedawm` or `~/.config/freedawm` (elsewhere). `None` without the variables
/// that locate the profile.
pub fn profile_settings_dirs(platform: Platform, env: impl Fn(&str) -> Option<OsString>) -> Option<(PathBuf, PathBuf)> {
    let base = match platform {
        Platform::Mac => env("HOME").map(|h| PathBuf::from(h).join("Library").join("Application Support")),
        Platform::Windows => env("APPDATA").map(PathBuf::from),
        Platform::Other => env("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| env("HOME").map(|h| PathBuf::from(h).join(".config"))),
    }?;
    Some(match platform {
        Platform::Mac | Platform::Windows => (base.join(APP_NAME), base.join(LEGACY_APP_NAME)),
        Platform::Other => (base.join(APP_NAME.to_lowercase()), base.join(LEGACY_APP_NAME.to_lowercase())),
    })
}

/// Written at the top of the new folder once the copy from the old one finished clean; while it
/// is absent and the old folder exists, every start copies again (never overwriting).
pub const MIGRATION_MARKER: &str = ".migrated-from-soundcraft";

/// What one run of [`migrate_folder`] did.
#[derive(Debug, Default)]
pub struct Migration {
    /// Files copied into the new folder.
    pub copied: usize,
    /// Files already present in the new folder, left as they were.
    pub kept: usize,
    /// Entries that could not be copied (the old path and the error); the rest was still copied.
    pub failures: Vec<(PathBuf, io::Error)>,
}

impl Migration {
    /// Every entry arrived (the marker was written, so no later start copies again).
    pub fn finished_clean(&self) -> bool {
        self.failures.is_empty()
    }
}

/// Copies the pre-rename folder `old` into `new` when `old` is a folder and `new` does not carry
/// the [`MIGRATION_MARKER`] yet (a missing `new` is created). The copy goes entry by entry: a file
/// already in `new` is never overwritten (so a `new` the app made for its logs before a failed
/// copy still receives everything on the next start), an entry that fails is reported and the
/// copy continues, and the marker is written only when nothing failed, so a partial copy is
/// retried on the next start. `old` stays as it was (a backup). Skipped: `logs/`, lock files
/// (`lock`, `*.lock`), temporary files (`*.tmp`, write probes) and symbolic links; autosave takes
/// are copied (recovery needs them). `None` when there was nothing to do (no old folder, the
/// marker is present, or `new` is a file or a link).
pub fn migrate_folder(old: &Path, new: &Path) -> io::Result<Option<Migration>> {
    if !old.is_dir() || new.join(MIGRATION_MARKER).exists() {
        return Ok(None);
    }
    if new.symlink_metadata().is_ok_and(|m| !m.is_dir()) {
        return Ok(None);
    }
    std::fs::create_dir_all(new)?;
    let mut report = Migration::default();
    copy_tree(old, new, &mut report)?;
    if report.finished_clean()
        && let Err(e) = std::fs::write(new.join(MIGRATION_MARKER), b"settings copied from the SoundCraft folder\n")
    {
        report.failures.push((new.join(MIGRATION_MARKER), e));
    }
    Ok(Some(report))
}

/// Whether an entry of the old folder is left out of the copy.
fn skipped(name: &std::ffi::OsStr, is_dir: bool) -> bool {
    let name = name.to_string_lossy();
    if is_dir {
        return name == "logs";
    }
    name == "lock" || name == MIGRATION_MARKER || name.ends_with(".lock") || name.ends_with(".tmp") || name.starts_with(".write-test-")
}

/// Copies `from` into `to` entry by entry; only an unreadable `from` is an error, every other
/// failure is recorded in `report` and the copy goes on.
fn copy_tree(from: &Path, to: &Path, report: &mut Migration) -> io::Result<()> {
    remove_stale_partials(to);
    for entry in std::fs::read_dir(from)? {
        let entry = match entry {
            Ok(e) => e,
            Err(e) => {
                report.failures.push((from.to_path_buf(), e));
                continue;
            }
        };
        let path = entry.path();
        let kind = match entry.file_type() {
            Ok(k) => k,
            Err(e) => {
                report.failures.push((path, e));
                continue;
            }
        };
        if skipped(&entry.file_name(), kind.is_dir()) {
            continue;
        }
        let target = to.join(entry.file_name());
        if kind.is_dir() {
            if let Err(e) = std::fs::create_dir_all(&target).and_then(|()| copy_tree(&path, &target, report)) {
                report.failures.push((path, e));
            }
        } else if kind.is_file() {
            if target.symlink_metadata().is_ok() {
                report.kept += 1;
            } else {
                match copy_file(&path, &target) {
                    Ok(()) => report.copied += 1,
                    Err(e) => report.failures.push((path, e)),
                }
            }
        }
    }
    Ok(())
}

/// Removes the `<name>.migrating-<pid>.tmp` partials a crash mid-copy left in `dir`: their file
/// is copied again (it is not there under its own name), and nothing else ever cleans them.
fn remove_stale_partials(dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        if name.contains(".migrating-") && name.ends_with(".tmp") && entry.file_type().is_ok_and(|k| k.is_file()) {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

/// Copies through a temporary name next to `to`, so an interrupted copy never leaves a short
/// file that the next start would keep as "already there".
fn copy_file(from: &Path, to: &Path) -> io::Result<()> {
    let name = to.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let partial = to.with_file_name(format!("{name}.migrating-{}.tmp", std::process::id()));
    let copied = std::fs::copy(from, &partial).and_then(|_| std::fs::rename(&partial, to));
    if copied.is_err() {
        let _ = std::fs::remove_file(&partial);
    }
    copied
}

/// Migrates the profile settings folder (see [`migrate_folder`]) and logs one line when it did.
pub fn migrate_profile_settings(platform: Platform, env: impl Fn(&str) -> Option<OsString>) {
    if let Some((new, old)) = profile_settings_dirs(platform, env) {
        log_migration(&old, &new);
    }
}

/// [`migrate_folder`] with its outcome logged: one line when it copied, a warning per entry
/// that failed (and a summary saying the copy is retried on the next start).
pub fn log_migration(old: &Path, new: &Path) {
    let result = migrate_folder(old, new);
    if let Ok(Some(report)) = &result {
        for (path, e) in &report.failures {
            log::warn!("settings: could not copy {}: {e}", path.display());
        }
    }
    match migration_summary(old, new, &result) {
        Some((true, line)) => log::warn!("{line}"),
        Some((false, line)) => log::info!("{line}"),
        None => {}
    }
}

/// One line describing a [`migrate_folder`] outcome (and whether it is a warning); `None` when
/// nothing was copied and nothing failed.
pub fn migration_summary(old: &Path, new: &Path, result: &io::Result<Option<Migration>>) -> Option<(bool, String)> {
    match result {
        Ok(None) => None,
        Ok(Some(r)) if r.finished_clean() => (r.copied > 0).then(|| {
            (false, format!("settings: copied {} files from {} to {} (the old folder is kept as a backup)", r.copied, old.display(), new.display()))
        }),
        Ok(Some(r)) => {
            let (first_path, first_err) = r.failures.first().map(|(p, e)| (p.display().to_string(), e.to_string())).unwrap_or_default();
            Some((
                true,
                format!(
                    "settings: copied {} files from {} to {}; {} could not be copied (first: {first_path}: {first_err}); the rest is copied again on the next start",
                    r.copied,
                    old.display(),
                    new.display(),
                    r.failures.len(),
                ),
            ))
        }
        Err(e) => Some((true, format!("settings: could not copy {} to {}: {e}", old.display(), new.display()))),
    }
}

#[cfg(test)]
mod tests {
    #![allow(clippy::unwrap_used)]
    use super::*;

    fn vars<'a>(list: &'a [(&'a str, &'a str)]) -> impl Fn(&str) -> Option<OsString> + 'a {
        move |name| list.iter().find_map(|(k, v)| (*k == name).then(|| OsString::from(v)))
    }

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("soundcraft-brand-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn the_new_env_name_wins_and_the_old_one_is_the_fallback() {
        let both = [("FREEDAWM_CONFIG_DIR", "/new"), ("SOUNDCRAFT_CONFIG_DIR", "/old")];
        assert_eq!(env_var_os_with("CONFIG_DIR", vars(&both)), Some("/new".into()));
        assert_eq!(env_var_os_with("CONFIG_DIR", vars(&[("SOUNDCRAFT_CONFIG_DIR", "/old")])), Some("/old".into()));
        assert_eq!(env_var_os_with("CONFIG_DIR", vars(&[("FREEDAWM_CONFIG_DIR", "/new")])), Some("/new".into()));
        // An empty new value is still "set": it does not fall through to the old one.
        assert_eq!(env_var_os_with("CONFIG_DIR", vars(&[("FREEDAWM_CONFIG_DIR", ""), ("SOUNDCRAFT_CONFIG_DIR", "/old")])), Some("".into()));
        assert_eq!(env_var_os_with("CONFIG_DIR", vars(&[])), None);
    }

    #[test]
    fn profile_folders_are_renamed_per_platform() {
        let v = [("HOME", "/user"), ("APPDATA", "/roaming"), ("XDG_CONFIG_HOME", "/xdg")];
        let base = PathBuf::from("/user/Library/Application Support");
        assert_eq!(profile_settings_dirs(Platform::Mac, vars(&v)), Some((base.join("FreeDAWm"), base.join("SoundCraft"))));
        let base = PathBuf::from("/roaming");
        assert_eq!(profile_settings_dirs(Platform::Windows, vars(&v)), Some((base.join("FreeDAWm"), base.join("SoundCraft"))));
        let base = PathBuf::from("/xdg");
        assert_eq!(profile_settings_dirs(Platform::Other, vars(&v)), Some((base.join("freedawm"), base.join("soundcraft"))));
        let base = PathBuf::from("/user/.config");
        assert_eq!(profile_settings_dirs(Platform::Other, vars(&[("HOME", "/user")])), Some((base.join("freedawm"), base.join("soundcraft"))));
        assert_eq!(profile_settings_dirs(Platform::Windows, vars(&[])), None);
    }

    fn copied(old: &Path, new: &Path) -> Migration {
        migrate_folder(old, new).unwrap().expect("something to copy")
    }

    #[test]
    fn an_old_folder_is_copied_when_the_new_one_is_missing() {
        let root = temp("copy");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        std::fs::create_dir_all(old.join("Presets").join("EQ")).unwrap();
        std::fs::write(old.join("ui.json"), b"{\"ui\":1}").unwrap();
        std::fs::write(old.join("Presets").join("EQ").join("Bright.json"), b"preset").unwrap();
        let report = copied(&old, &new);
        assert!(report.finished_clean());
        assert_eq!((report.copied, report.kept), (2, 0));
        assert_eq!(std::fs::read(new.join("ui.json")).unwrap(), b"{\"ui\":1}");
        assert_eq!(std::fs::read(new.join("Presets").join("EQ").join("Bright.json")).unwrap(), b"preset");
        // Copied, not moved: the old folder stays as a backup.
        assert_eq!(std::fs::read(old.join("ui.json")).unwrap(), b"{\"ui\":1}");
        // The marker says the copy finished; no temporary file is left behind.
        assert!(new.join(MIGRATION_MARKER).is_file());
        let names: Vec<_> = std::fs::read_dir(&new).unwrap().map(|e| e.unwrap().file_name()).collect();
        assert_eq!(names.len(), 3, "{names:?}");
        // A second start does nothing.
        assert!(migrate_folder(&old, &new).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_marked_new_folder_is_left_untouched() {
        let root = temp("keep");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        std::fs::create_dir_all(&old).unwrap();
        std::fs::create_dir_all(&new).unwrap();
        std::fs::write(old.join("ui.json"), b"old").unwrap();
        std::fs::write(old.join("only-old.txt"), b"old").unwrap();
        std::fs::write(new.join("ui.json"), b"new").unwrap();
        std::fs::write(new.join(MIGRATION_MARKER), b"").unwrap();
        assert!(migrate_folder(&old, &new).unwrap().is_none());
        assert_eq!(std::fs::read(new.join("ui.json")).unwrap(), b"new");
        assert!(!new.join("only-old.txt").exists());
        // A file (or a link) where the new folder should be is never touched either.
        let file = root.join("FreeDAWm-file");
        std::fs::write(&file, b"").unwrap();
        assert!(migrate_folder(&old, &file).unwrap().is_none());
        assert_eq!(std::fs::metadata(&file).unwrap().len(), 0);
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_new_folder_without_the_marker_still_receives_what_it_lacks() {
        // The app made `FreeDAWm/logs` for its log file before a copy could finish: the next start
        // copies the rest, keeps every file the new folder already has, and then writes the marker.
        let root = temp("retry");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        std::fs::create_dir_all(old.join("Presets")).unwrap();
        std::fs::create_dir_all(new.join("logs")).unwrap();
        std::fs::write(old.join("ui.json"), b"old").unwrap();
        std::fs::write(old.join("recent.json"), b"recent").unwrap();
        std::fs::write(old.join("Presets").join("p.json"), b"p").unwrap();
        std::fs::write(new.join("ui.json"), b"new").unwrap();
        std::fs::write(new.join("logs").join("freedawm.log"), b"log").unwrap();
        let report = copied(&old, &new);
        assert!(report.finished_clean());
        assert_eq!((report.copied, report.kept), (2, 1));
        assert_eq!(std::fs::read(new.join("ui.json")).unwrap(), b"new", "an existing file is never overwritten");
        assert_eq!(std::fs::read(new.join("recent.json")).unwrap(), b"recent");
        assert_eq!(std::fs::read(new.join("Presets").join("p.json")).unwrap(), b"p");
        assert_eq!(std::fs::read(new.join("logs").join("freedawm.log")).unwrap(), b"log");
        assert!(new.join(MIGRATION_MARKER).is_file());
        assert!(migrate_folder(&old, &new).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn logs_lock_and_temporary_files_are_not_copied_but_autosave_takes_are() {
        let root = temp("skip");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        let take = old.join("Autosave").join("Song").join("Audio Files");
        std::fs::create_dir_all(&take).unwrap();
        std::fs::create_dir_all(old.join("logs")).unwrap();
        std::fs::write(old.join("logs").join("soundcraft.log"), b"log").unwrap();
        std::fs::write(old.join("Autosave").join("Song").join("lock"), b"").unwrap();
        std::fs::write(old.join("Autosave").join("Song").join("session.fdawm"), b"session").unwrap();
        std::fs::write(take.join("Take 1.wav"), b"wav").unwrap();
        std::fs::write(old.join("ui.json.123.tmp"), b"half").unwrap();
        std::fs::write(old.join("scan.lock"), b"").unwrap();
        std::fs::write(old.join(".write-test-9"), b"").unwrap();
        std::fs::write(old.join("ui.json"), b"ui").unwrap();
        let report = copied(&old, &new);
        assert!(report.finished_clean());
        assert_eq!(report.copied, 3, "ui.json, session.fdawm and the take");
        assert!(!new.join("logs").exists());
        assert!(!new.join("Autosave").join("Song").join("lock").exists());
        assert!(!new.join("ui.json.123.tmp").exists());
        assert!(!new.join("scan.lock").exists());
        assert!(!new.join(".write-test-9").exists());
        assert_eq!(std::fs::read(new.join("Autosave").join("Song").join("session.fdawm")).unwrap(), b"session");
        assert_eq!(std::fs::read(new.join("Autosave").join("Song").join("Audio Files").join("Take 1.wav")).unwrap(), b"wav");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_failing_entry_does_not_stop_the_copy_and_keeps_the_marker_away() {
        let root = temp("fail");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        std::fs::create_dir_all(old.join("Presets")).unwrap();
        std::fs::write(old.join("a.json"), b"a").unwrap();
        std::fs::write(old.join("blocked"), b"blocked").unwrap();
        std::fs::write(old.join("Presets").join("p.json"), b"p").unwrap();
        std::fs::write(old.join("z.json"), b"z").unwrap();
        // A folder in the new tree where the old tree has a file: that one entry cannot be copied.
        std::fs::create_dir_all(new.join("blocked").join("inner")).unwrap();
        std::fs::write(new.join("blocked").join("inner").join("x"), b"x").unwrap();
        // Make the failure real by pretending a file sits where a folder goes, too.
        std::fs::create_dir_all(old.join("Thumbs")).unwrap();
        std::fs::write(old.join("Thumbs").join("t.png"), b"t").unwrap();
        std::fs::write(new.join("Thumbs"), b"a file in the way").unwrap();
        let report = copied(&old, &new);
        assert!(!report.finished_clean());
        assert_eq!(report.copied, 3, "a.json, p.json and z.json: {report:?}");
        assert_eq!(report.failures.len(), 1, "{report:?}");
        assert!(report.failures[0].0.ends_with("Thumbs"), "{report:?}");
        assert_eq!(report.kept, 1, "`blocked` already exists in the new folder, as a folder");
        assert_eq!(std::fs::read(new.join("z.json")).unwrap(), b"z", "the copy went on past the failure");
        assert!(!new.join(MIGRATION_MARKER).exists(), "a partial copy is tried again next start");
        // Next start, with the obstacle gone: only the missing entry is copied, then the marker.
        std::fs::remove_file(new.join("Thumbs")).unwrap();
        let report = copied(&old, &new);
        assert!(report.finished_clean(), "{report:?}");
        assert_eq!((report.copied, report.kept), (1, 4), "{report:?}");
        assert_eq!(std::fs::read(new.join("Thumbs").join("t.png")).unwrap(), b"t");
        assert!(new.join(MIGRATION_MARKER).is_file());
        assert!(migrate_folder(&old, &new).unwrap().is_none());
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn a_partial_left_by_a_crash_is_removed_and_its_file_copied() {
        let root = temp("partial");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        std::fs::create_dir_all(old.join("Presets")).unwrap();
        std::fs::write(old.join("a.json"), b"a").unwrap();
        std::fs::write(old.join("Presets").join("p.json"), b"p").unwrap();
        std::fs::create_dir_all(new.join("Presets")).unwrap();
        std::fs::write(new.join("a.json.migrating-4242.tmp"), b"half of").unwrap();
        std::fs::write(new.join("Presets").join("p.json.migrating-4242.tmp"), b"half").unwrap();
        std::fs::write(new.join("keep.tmp"), b"someone else's").unwrap();
        let report = copied(&old, &new);
        assert!(report.finished_clean(), "{report:?}");
        assert_eq!(report.copied, 2);
        assert_eq!(std::fs::read(new.join("a.json")).unwrap(), b"a");
        assert!(!new.join("a.json.migrating-4242.tmp").exists(), "the stale partial is gone");
        assert!(!new.join("Presets").join("p.json.migrating-4242.tmp").exists());
        assert!(new.join("keep.tmp").exists(), "only our own partials are removed");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn the_summary_line_says_what_happened() {
        let (old, new) = (Path::new("old"), Path::new("new"));
        assert!(migration_summary(old, new, &Ok(None)).is_none());
        assert!(migration_summary(old, new, &Ok(Some(Migration::default()))).is_none(), "nothing new to copy: silent");
        let clean = Migration { copied: 4, kept: 1, failures: vec![] };
        let (warning, line) = migration_summary(old, new, &Ok(Some(clean))).unwrap();
        assert!(!warning);
        assert!(line.contains("copied 4 files") && line.contains("backup"), "{line}");
        let partial = Migration { copied: 2, kept: 0, failures: vec![(PathBuf::from("old/x"), io::Error::other("locked"))] };
        let (warning, line) = migration_summary(old, new, &Ok(Some(partial))).unwrap();
        assert!(warning);
        assert!(line.contains("1 could not be copied") && line.contains("locked") && line.contains("next start"), "{line}");
        let (warning, line) = migration_summary(old, new, &Err(io::Error::other("denied"))).unwrap();
        assert!(warning && line.contains("denied"), "{line}");
    }

    #[test]
    fn nothing_happens_without_an_old_folder() {
        let root = temp("none");
        let (old, new) = (root.join("SoundCraft"), root.join("FreeDAWm"));
        assert!(migrate_folder(&old, &new).unwrap().is_none());
        assert!(!new.exists());
        std::fs::remove_dir_all(root).unwrap();
    }
}

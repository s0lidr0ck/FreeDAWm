//! Per-user settings and automation profile directories.

use soundcraft_engine::brand::{self, Platform};
use std::ffi::OsString;
use std::path::{Path, PathBuf};

/// Portable mode (#268): this file beside the executable keeps settings, autosaves, presets and
/// logs in [`brand::PORTABLE_DATA_DIR`] next to it instead of the user profile.
const PORTABLE_MARKER: &str = "portable.txt";

/// FreeDAWm's settings folder. A nonempty `$FREEDAWM_CONFIG_DIR` (or the older
/// `$SOUNDCRAFT_CONFIG_DIR`) overrides the platform default. The override is the complete
/// directory; no application-name suffix is appended. An explicitly empty value disables the
/// settings directory instead of falling back to a personal profile. Otherwise `portable.txt`
/// beside the executable selects `<exe dir>/FreeDAWmData`, when that folder can be written (a
/// read-only medium falls back to the platform default).
///
/// The pre-rename folder (`SoundCraft`, `soundcraft`, `SoundCraftData`) is copied into the new
/// one, entry by entry and never overwriting, on every start until a copy finishes clean and
/// leaves [`brand::MIGRATION_MARKER`] there ([`brand::migrate_folder`]); runs without preferences
/// (`FREEDAWM_NO_PREFS`) never copy.
pub fn config_dir() -> Option<PathBuf> {
    static DIR: std::sync::OnceLock<Option<PathBuf>> = std::sync::OnceLock::new();
    DIR.get_or_init(|| {
        let exe = std::env::current_exe().ok();
        let migrate = brand::env_var_os("NO_PREFS").is_none();
        config_dir_with(Platform::current(), |name| std::env::var_os(name), exe.as_deref().and_then(Path::parent), migrate)
    })
    .clone()
}

fn config_dir_with(platform: Platform, env: impl Fn(&str) -> Option<OsString>, exe_dir: Option<&Path>, migrate: bool) -> Option<PathBuf> {
    if let Some(dir) = brand::env_var_os_with("CONFIG_DIR", &env) {
        return (!dir.is_empty()).then(|| PathBuf::from(dir));
    }
    if let Some(exe_dir) = exe_dir.filter(|d| d.join(PORTABLE_MARKER).is_file()) {
        let data = exe_dir.join(brand::PORTABLE_DATA_DIR);
        match writable(&data) {
            Ok(()) => {
                // After the write probe: a read-only medium then falls back to the profile without
                // a copy attempt, and the folder the probe made is filled by the copy anyway.
                if migrate {
                    brand::log_migration(&exe_dir.join(brand::LEGACY_PORTABLE_DATA_DIR), &data);
                }
                return Some(data);
            }
            Err(e) => log::warn!("portable mode: {} can't be written ({e}); using the user profile instead", data.display()),
        }
    }
    let (dir, legacy) = brand::profile_settings_dirs(platform, &env)?;
    if migrate {
        brand::log_migration(&legacy, &dir);
    }
    Some(dir)
}

/// Create `dir` if needed and prove a file can be written in it.
fn writable(dir: &Path) -> std::io::Result<()> {
    std::fs::create_dir_all(dir)?;
    let probe = dir.join(format!(".write-test-{}", std::process::id()));
    std::fs::write(&probe, b"")?;
    std::fs::remove_file(probe)
}

/// Read bounded settings, recovering unreadable docking metadata independently of other settings.
pub fn load_ui(path: &std::path::Path) -> Result<Option<soundcraft_ui_egui::UiState>, String> {
    read_ui(path, true)
}

fn read_ui(path: &std::path::Path, recover_docking: bool) -> Result<Option<soundcraft_ui_egui::UiState>, String> {
    use std::io::Read as _;
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(format!("Settings could not be read: {error}")),
    };
    let mut bytes = Vec::new();
    file.take(MAX_UI_BYTES + 1).read_to_end(&mut bytes).map_err(|error| format!("Settings could not be read: {error}"))?;
    if bytes.len() as u64 > MAX_UI_BYTES {
        return Err("Settings file exceeds the 4 MiB limit".into());
    }
    let mut saved: serde_json::Value = serde_json::from_slice(&bytes).map_err(|error| format!("Settings could not be restored: {error}"))?;
    if recover_docking && let Some(fields) = saved.as_object_mut() {
        for (field, mix) in [("docking", false), ("mix_docking", true)] {
            let unreadable =
                fields.get(field).is_some_and(|value| match serde_json::from_value::<soundcraft_ui_egui::docking::Workspace>(value.clone()) {
                    Ok(workspace) => if mix { workspace.validate_mix() } else { workspace.validate() }.is_err(),
                    Err(_) => true,
                });
            if unreadable {
                fields.remove(field);
            }
        }
    }
    let mut state: soundcraft_ui_egui::UiState = serde_json::from_value(saved).map_err(|error| format!("Settings could not be restored: {error}"))?;
    // Mix window sections added since these settings were saved are switched on once.
    state.offer_new_mix_views();
    state.docking.validate()?;
    state.mix_docking.validate_mix()?;
    Ok(Some(state))
}

const MAX_UI_BYTES: u64 = 4 * 1024 * 1024;

/// Write settings atomically. Preserve an unreadable or malformed existing file for recovery.
pub fn save_ui(path: &std::path::Path, ui: &soundcraft_ui_egui::UiState) -> Result<(), String> {
    use std::io::Write as _;
    // Loading can recover a future layout for this session, but saving must preserve its source.
    read_ui(path, false)?;
    ui.docking.validate()?;
    ui.mix_docking.validate_mix()?;
    let mut saved = ui.clone();
    saved.plugin_windows.clear();
    saved.drawers.clear();
    saved.status.clear();
    let bytes = serde_json::to_vec_pretty(&saved).map_err(|error| error.to_string())?;
    if bytes.len() as u64 > MAX_UI_BYTES {
        return Err("Settings exceed the 4 MiB limit".into());
    }
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent).map_err(|error| error.to_string())?;
    }
    let nonce = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_err(|error| error.to_string())?.as_nanos();
    let temporary = path.with_extension(format!("json.{}.{nonce}.tmp", std::process::id()));
    let mut file =
        std::fs::OpenOptions::new().write(true).create_new(true).open(&temporary).map_err(|error| format!("Settings could not be saved: {error}"))?;
    let written = (|| -> std::io::Result<()> {
        file.write_all(&bytes)?;
        file.sync_all()?;
        std::fs::rename(&temporary, path)
    })();
    if written.is_err() {
        let _ = std::fs::remove_file(&temporary);
    }
    written.map_err(|error| format!("Settings could not be saved: {error}"))
}

#[cfg(test)]
mod config_tests {
    use super::*;

    #[test]
    fn settings_saved_before_the_pitch_view_get_it_once() {
        let directory = std::env::temp_dir().join(format!("soundcraft-pitch-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("ui.json");
        let mut value = serde_json::to_value(soundcraft_ui_egui::UiState::default()).unwrap();
        value["mix_views"] = serde_json::json!(["inserts_ae", "io"]);
        value.as_object_mut().unwrap().remove("mix_views_offered");
        std::fs::create_dir_all(&directory).unwrap();
        std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
        let mut ui = load_ui(&path).unwrap().unwrap();
        assert_eq!(ui.mix_views, ["inserts_ae", "io", "pitch"]);
        ui.mix_views.retain(|v| v != "pitch");
        save_ui(&path, &ui).unwrap();
        assert_eq!(load_ui(&path).unwrap().unwrap().mix_views, ["inserts_ae", "io"], "turned off, it stays off");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn open_drawers_are_not_saved() {
        let directory = std::env::temp_dir().join(format!("soundcraft-drawer-prefs-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&directory);
        let path = directory.join("ui.json");
        let mut value = serde_json::to_value(soundcraft_ui_egui::UiState::default()).unwrap();
        value["drawers"] = serde_json::json!({"7": 3});
        let ui: soundcraft_ui_egui::UiState = serde_json::from_value(value).unwrap();
        assert_eq!(ui.drawers.len(), 1);
        save_ui(&path, &ui).unwrap();
        let back = load_ui(&path).unwrap().unwrap();
        assert!(back.drawers.is_empty(), "no stray drawer opens next session");
        assert_eq!(ui.drawers.len(), 1, "the running session keeps its drawers");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_file_reload_preserves_edit_mix_and_hidden_floating_panels() {
        use soundcraft_ui_egui::{MainWindow, Services, SoundApp, docking::Panel};
        let directory = std::env::temp_dir().join(format!("soundcraft-dock-prefs-{}", std::process::id()));
        let path = directory.join("ui.json");
        let _ = std::fs::remove_dir_all(&directory);
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[80,90,260,400]})).unwrap();
        app.run("ui.dock", serde_json::json!({"operation":"close","panel":"tracks"})).unwrap();
        app.ui.theme = soundcraft_ui_egui::theme::ThemeMode::Light;
        let edit = serde_json::to_value(&app.ui.docking).unwrap();
        app.ui.window = MainWindow::Mix;
        app.run("ui.dock", serde_json::json!({"operation":"open","panel":"tracks"})).unwrap();
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[140,150,280,420]})).unwrap();
        save_ui(&path, &app.ui).unwrap();
        app.ui = load_ui(&path).unwrap().unwrap();
        assert_eq!(app.ui.theme, soundcraft_ui_egui::theme::ThemeMode::Light);
        assert_eq!(serde_json::to_value(&app.ui.docking).unwrap(), edit);
        assert_eq!(app.ui.mix_docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Tracks)).unwrap().rect, [140.0, 150.0, 280.0, 420.0]);
        app.ui.window = MainWindow::Edit;
        app.run("ui.dock", serde_json::json!({"operation":"open","panel":"tracks"})).unwrap();
        assert_eq!(app.ui.docking.layout.floating.iter().find(|g| g.panels.contains(&Panel::Tracks)).unwrap().rect, [80.0, 90.0, 260.0, 400.0]);
        let mut invalid_mix = app.ui.clone();
        invalid_mix.mix_docking = soundcraft_ui_egui::docking::Workspace::default();
        let invalid_bytes = serde_json::to_vec(&invalid_mix).unwrap();
        std::fs::write(&path, &invalid_bytes).unwrap();
        let recovered = load_ui(&path).unwrap().unwrap();
        assert_eq!(recovered.theme, app.ui.theme);
        assert_eq!(serde_json::to_value(recovered.docking).unwrap(), serde_json::to_value(&app.ui.docking).unwrap());
        recovered.mix_docking.validate_mix().unwrap();
        assert!(save_ui(&path, &app.ui).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), invalid_bytes);
        std::fs::write(&path, b"damaged settings").unwrap();
        assert!(load_ui(&path).is_err());
        assert!(save_ui(&path, &app.ui).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), b"damaged settings");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn future_edit_and_mix_layouts_preserve_other_preferences_and_the_original_file() {
        use soundcraft_ui_egui::{Services, SoundApp};
        let directory = std::env::temp_dir().join(format!("soundcraft-future-dock-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ui.json");
        let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
        app.run("ui.dock", serde_json::json!({"operation":"float","panel":"tracks","rect":[80,90,260,400]})).unwrap();
        app.ui.theme = soundcraft_ui_egui::theme::ThemeMode::Light;
        app.ui.narrow_mix = true;
        for field in ["docking", "mix_docking"] {
            for invalid in [serde_json::json!({"layout":{"root":{"FutureLayout":{}}}}), serde_json::json!(false)] {
                let mut saved = serde_json::to_value(&app.ui).unwrap();
                saved[field] = invalid;
                assert!(serde_json::from_value::<soundcraft_ui_egui::UiState>(saved.clone()).is_err());
                let bytes = serde_json::to_vec(&saved).unwrap();
                std::fs::write(&path, &bytes).unwrap();
                let recovered = load_ui(&path).unwrap().unwrap();
                assert_eq!(recovered.theme, app.ui.theme);
                assert!(recovered.narrow_mix);
                recovered.docking.validate().unwrap();
                recovered.mix_docking.validate_mix().unwrap();
                let other = if field == "docking" { "mix_docking" } else { "docking" };
                assert_eq!(serde_json::to_value(&recovered).unwrap()[other], saved[other]);
                assert!(save_ui(&path, &recovered).is_err());
                assert_eq!(std::fs::read(&path).unwrap(), bytes);
            }
        }
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn oversized_settings_are_rejected_and_preserved() {
        let directory = std::env::temp_dir().join(format!("soundcraft-bounded-prefs-{}", std::process::id()));
        std::fs::create_dir_all(&directory).unwrap();
        let path = directory.join("ui.json");
        let bytes = vec![b' '; MAX_UI_BYTES as usize + 1];
        std::fs::write(&path, &bytes).unwrap();
        assert!(load_ui(&path).is_err());
        assert!(save_ui(&path, &soundcraft_ui_egui::UiState::default()).is_err());
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn resolve(platform: Platform, vars: &[(&str, &str)]) -> Option<PathBuf> {
        config_dir_with(platform, |name| vars.iter().find_map(|(key, value)| (*key == name).then(|| OsString::from(value))), None, false)
    }

    #[test]
    fn config_override_wins_without_reading_user_profile() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            let actual = config_dir_with(
                platform,
                |name| {
                    assert_eq!(name, "FREEDAWM_CONFIG_DIR");
                    Some(OsString::from("isolated/settings"))
                },
                None,
                false,
            );
            assert_eq!(actual, Some(PathBuf::from("isolated/settings")));
        }
    }

    #[test]
    fn config_empty_override_disables_profile_without_reading_defaults() {
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            assert_eq!(
                config_dir_with(
                    platform,
                    |name| {
                        assert_eq!(name, "FREEDAWM_CONFIG_DIR");
                        Some(OsString::new())
                    },
                    None,
                    false
                ),
                None
            );
        }
    }

    #[test]
    fn config_unset_override_keeps_platform_defaults() {
        let vars = [("HOME", "/user"), ("APPDATA", "/roaming"), ("XDG_CONFIG_HOME", "/xdg")];
        assert_eq!(resolve(Platform::Mac, &vars), Some(PathBuf::from("/user/Library/Application Support").join("FreeDAWm")));
        assert_eq!(resolve(Platform::Windows, &vars), Some(PathBuf::from("/roaming").join("FreeDAWm")));
        assert_eq!(resolve(Platform::Other, &vars), Some(PathBuf::from("/xdg").join("freedawm")));
        assert_eq!(resolve(Platform::Other, &[("HOME", "/user")]), Some(PathBuf::from("/user/.config").join("freedawm")));
        for platform in [Platform::Mac, Platform::Windows, Platform::Other] {
            assert_eq!(resolve(platform, &[]), None);
        }
    }

    #[test]
    fn portable_txt_beside_the_exe_keeps_settings_next_to_it() {
        let exe = std::env::temp_dir().join(format!("soundcraft-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&exe);
        std::fs::create_dir_all(&exe).unwrap();
        let vars = [("APPDATA", "/roaming")];
        assert_eq!(resolve_with_exe(&vars, &exe), Some(PathBuf::from("/roaming").join("FreeDAWm")), "no marker: the profile");
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        assert_eq!(resolve_with_exe(&vars, &exe), Some(exe.join("FreeDAWmData")));
        assert!(exe.join("FreeDAWmData").is_dir());
        // The explicit override still wins, under the new and the old variable name.
        assert_eq!(resolve_with_exe(&[("FREEDAWM_CONFIG_DIR", "/custom")], &exe), Some(PathBuf::from("/custom")));
        assert_eq!(resolve_with_exe(&[("SOUNDCRAFT_CONFIG_DIR", "/custom")], &exe), Some(PathBuf::from("/custom")));
        // A data folder that can't be made (a file is in the way) falls back to the profile.
        std::fs::remove_dir_all(exe.join("FreeDAWmData")).unwrap();
        std::fs::write(exe.join("FreeDAWmData"), b"").unwrap();
        assert_eq!(resolve_with_exe(&vars, &exe), Some(PathBuf::from("/roaming").join("FreeDAWm")));
        std::fs::remove_dir_all(exe).unwrap();
    }

    fn resolve_with_exe(vars: &[(&str, &str)], exe: &Path) -> Option<PathBuf> {
        config_dir_with(
            Platform::Windows,
            |name| vars.iter().find_map(|(key, value)| (*key == name).then(|| OsString::from(value))),
            Some(exe),
            false,
        )
    }

    #[test]
    fn the_old_config_variable_is_still_read() {
        let old = [("SOUNDCRAFT_CONFIG_DIR", "/old"), ("APPDATA", "/roaming")];
        assert_eq!(resolve(Platform::Windows, &old), Some(PathBuf::from("/old")));
        let both = [("FREEDAWM_CONFIG_DIR", "/new"), ("SOUNDCRAFT_CONFIG_DIR", "/old")];
        assert_eq!(resolve(Platform::Windows, &both), Some(PathBuf::from("/new")));
        // An empty old override still disables the settings folder.
        assert_eq!(resolve(Platform::Windows, &[("SOUNDCRAFT_CONFIG_DIR", ""), ("APPDATA", "/roaming")]), None);
    }

    #[test]
    fn the_first_start_copies_the_old_profile_folder() {
        let root = std::env::temp_dir().join(format!("soundcraft-migrate-profile-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("SoundCraft").join("Presets")).unwrap();
        std::fs::write(root.join("SoundCraft").join("ui.json"), b"{}").unwrap();
        std::fs::write(root.join("SoundCraft").join("Presets").join("p.json"), b"p").unwrap();
        let appdata = root.clone().into_os_string();
        let env = |name: &str| (name == "APPDATA").then(|| appdata.clone());
        // Runs without preferences never copy.
        assert_eq!(config_dir_with(Platform::Windows, env, None, false), Some(root.join("FreeDAWm")));
        assert!(!root.join("FreeDAWm").exists());
        assert_eq!(config_dir_with(Platform::Windows, env, None, true), Some(root.join("FreeDAWm")));
        assert_eq!(std::fs::read(root.join("FreeDAWm").join("ui.json")).unwrap(), b"{}");
        assert_eq!(std::fs::read(root.join("FreeDAWm").join("Presets").join("p.json")).unwrap(), b"p");
        assert!(root.join("SoundCraft").join("ui.json").is_file(), "the old folder stays as a backup");
        assert!(root.join("FreeDAWm").join(brand::MIGRATION_MARKER).is_file());
        // Once copied, the new folder is never overwritten.
        std::fs::write(root.join("FreeDAWm").join("ui.json"), b"new").unwrap();
        std::fs::write(root.join("SoundCraft").join("ui.json"), b"old").unwrap();
        config_dir_with(Platform::Windows, env, None, true);
        assert_eq!(std::fs::read(root.join("FreeDAWm").join("ui.json")).unwrap(), b"new");
        std::fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn portable_mode_copies_the_old_data_folder() {
        let exe = std::env::temp_dir().join(format!("soundcraft-migrate-portable-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&exe);
        std::fs::create_dir_all(exe.join("SoundCraftData")).unwrap();
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        std::fs::write(exe.join("SoundCraftData").join("ui.json"), b"{}").unwrap();
        assert_eq!(config_dir_with(Platform::Windows, |_| None, Some(&exe), true), Some(exe.join("FreeDAWmData")));
        assert_eq!(std::fs::read(exe.join("FreeDAWmData").join("ui.json")).unwrap(), b"{}");
        assert!(exe.join("SoundCraftData").join("ui.json").is_file());
        assert!(exe.join("FreeDAWmData").join(brand::MIGRATION_MARKER).is_file());
        std::fs::remove_dir_all(exe).unwrap();
    }

    #[test]
    fn a_data_folder_made_before_the_copy_finished_is_filled_on_the_next_start() {
        // Portable mode's write probe (or the app's log folder) created `FreeDAWmData` while the
        // first copy did not finish: without the marker, the next start copies what is missing.
        let exe = std::env::temp_dir().join(format!("soundcraft-migrate-portable-retry-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&exe);
        std::fs::create_dir_all(exe.join("SoundCraftData").join("Presets")).unwrap();
        std::fs::create_dir_all(exe.join("FreeDAWmData").join("logs")).unwrap();
        std::fs::write(exe.join("portable.txt"), b"").unwrap();
        std::fs::write(exe.join("SoundCraftData").join("ui.json"), b"old").unwrap();
        std::fs::write(exe.join("SoundCraftData").join("Presets").join("p.json"), b"p").unwrap();
        std::fs::write(exe.join("FreeDAWmData").join("ui.json"), b"new").unwrap();
        assert_eq!(config_dir_with(Platform::Windows, |_| None, Some(&exe), true), Some(exe.join("FreeDAWmData")));
        assert_eq!(std::fs::read(exe.join("FreeDAWmData").join("ui.json")).unwrap(), b"new", "never overwritten");
        assert_eq!(std::fs::read(exe.join("FreeDAWmData").join("Presets").join("p.json")).unwrap(), b"p");
        assert!(exe.join("FreeDAWmData").join(brand::MIGRATION_MARKER).is_file());
        std::fs::remove_dir_all(exe).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn config_override_preserves_non_unicode_paths() {
        use std::os::unix::ffi::OsStringExt as _;
        let value = OsString::from_vec(b"isolated/\xff".to_vec());
        assert_eq!(config_dir_with(Platform::Mac, |_| Some(value.clone()), None, false), Some(PathBuf::from(value)));
    }
}

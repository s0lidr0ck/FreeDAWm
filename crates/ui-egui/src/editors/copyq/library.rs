//! CopyQ's profile library: the made-up factory profiles, and the user's own as one JSON file
//! each in `<presets>/CopyQ Profiles/`. A profile is not tied to a session: it is saved here and
//! copied into the state of every module that matches to it.

use soundcraft_dsp::plugins::fs::copyq::Profile;
use soundcraft_dsp::plugins::fs::copyq::profile::factory_profiles;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// The folder under the user preset folder.
pub(crate) const FOLDER: &str = "CopyQ Profiles";
/// Most user profiles listed.
const MAX_LISTED: usize = 500;

/// One profile in the library.
#[derive(Clone, Debug)]
pub(crate) struct Entry {
    pub name: String,
    /// Made up in code (always there, never deleted).
    pub factory: bool,
    pub profile: Arc<Profile>,
}

/// The library folder (`None` where there is no preset folder: the web).
pub(crate) fn dir(preset_dir: Option<&Path>) -> Option<PathBuf> {
    preset_dir.map(|d| d.join(FOLDER))
}

/// A profile's file name without its extension: the name with anything a file system could
/// trip on turned into `_`, at most 60 characters, never empty or all dots.
pub(crate) fn file_stem(name: &str) -> String {
    let cleaned: String = name
        .chars()
        .map(|c| if c.is_alphanumeric() || matches!(c, ' ' | '-' | '_' | '(' | ')' | '+' | ',' | '&' | '.' | '\'') { c } else { '_' })
        .take(60)
        .collect();
    let cleaned = cleaned.trim().trim_matches('.').trim().to_string();
    if cleaned.is_empty() { "Profile".into() } else { cleaned }
}

/// Every profile: the factory ones, then the user's by name.
pub(crate) fn scan(dir: Option<&Path>) -> Vec<Entry> {
    let mut out: Vec<Entry> = factory_profiles().into_iter().map(|p| Entry { name: p.name.clone(), factory: true, profile: Arc::new(p) }).collect();
    let mut user: Vec<Entry> = dir
        .map(disk::json_files)
        .unwrap_or_default()
        .into_iter()
        .filter_map(|(stem, text)| {
            let mut p = Profile::from_json(&text)?;
            if !p.is_measured() {
                return None;
            }
            // The file's name is the profile's name: renaming the file renames the profile.
            p.name = stem.clone();
            Some(Entry { name: stem, factory: false, profile: Arc::new(p) })
        })
        .take(MAX_LISTED)
        .collect();
    user.sort_by_key(|e| e.name.to_lowercase());
    out.extend(user);
    out
}

/// Saves `profile` under its name (replacing a profile of that name). Returns the name it is
/// listed under.
pub(crate) fn save(dir: &Path, profile: &Profile) -> Result<String, String> {
    if !profile.is_measured() {
        return Err("nothing was measured yet: switch Capture on and play".into());
    }
    let stem = file_stem(&profile.name);
    let mut named = profile.clone();
    named.name = stem.clone();
    if disk::write(&dir.join(format!("{stem}.json")), &named.to_json()) {
        Ok(stem)
    } else {
        Err(format!("could not write the profile to {}", dir.display()))
    }
}

/// Removes the user profile `name`.
pub(crate) fn delete(dir: &Path, name: &str) {
    disk::remove(&dir.join(format!("{}.json", file_stem(name))));
}

#[cfg(not(target_arch = "wasm32"))]
mod disk {
    use std::path::Path;
    pub fn json_files(dir: &Path) -> Vec<(String, String)> {
        let Ok(rd) = std::fs::read_dir(dir) else { return Vec::new() };
        rd.flatten()
            .take(4000)
            .map(|e| e.path())
            .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("json")))
            .filter(|p| std::fs::metadata(p).is_ok_and(|m| m.is_file() && m.len() <= 1 << 20))
            .filter_map(|p| Some((p.file_stem()?.to_str()?.to_string(), std::fs::read_to_string(&p).ok()?)))
            .collect()
    }
    pub fn write(f: &Path, text: &str) -> bool {
        f.parent().is_none_or(|d| std::fs::create_dir_all(d).is_ok()) && std::fs::write(f, text).is_ok()
    }
    pub fn remove(f: &Path) {
        let _ = std::fs::remove_file(f);
    }
}

#[cfg(target_arch = "wasm32")]
mod disk {
    use std::path::Path;
    pub fn json_files(_dir: &Path) -> Vec<(String, String)> {
        Vec::new()
    }
    pub fn write(_f: &Path, _text: &str) -> bool {
        false
    }
    pub fn remove(_f: &Path) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("copyq-library-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        d
    }

    #[test]
    fn names_become_safe_file_names() {
        assert_eq!(file_stem("Snare (top) + room"), "Snare (top) + room");
        assert_eq!(file_stem("a/b\\c:d*e?f\"g<h>i|j"), "a_b_c_d_e_f_g_h_i_j");
        assert_eq!(file_stem("  ..  "), "Profile");
        assert_eq!(file_stem(""), "Profile");
        assert_eq!(file_stem("../../etc/passwd"), "_.._etc_passwd");
        assert_eq!(file_stem(&"x".repeat(200)).len(), 60);
    }

    #[test]
    fn the_library_lists_factory_then_user_and_saves_and_deletes() {
        let d = temp("list");
        let only_factory = scan(Some(&d));
        assert_eq!(only_factory.len(), 5);
        assert!(only_factory.iter().all(|e| e.factory));
        assert_eq!(scan(None).len(), 5);
        let mut p = Profile::tilt("my: snare", -1.0);
        assert_eq!(save(&d, &p).unwrap(), "my_ snare");
        p.name = "Bass DI".into();
        assert_eq!(save(&d, &p).unwrap(), "Bass DI");
        // Junk in the folder is passed over.
        std::fs::write(d.join("junk.json"), "{not json").unwrap();
        std::fs::write(d.join("notes.txt"), "hello").unwrap();
        std::fs::write(d.join("empty.json"), Profile::silent().to_json()).unwrap();
        let all = scan(Some(&d));
        let user: Vec<&str> = all.iter().filter(|e| !e.factory).map(|e| e.name.as_str()).collect();
        assert_eq!(user, ["Bass DI", "my_ snare"]);
        assert_eq!(all.iter().find(|e| e.name == "Bass DI").unwrap().profile.name, "Bass DI");
        // The same name again replaces it.
        save(&d, &p).unwrap();
        assert_eq!(scan(Some(&d)).len(), 7);
        delete(&d, "Bass DI");
        assert_eq!(scan(Some(&d)).len(), 6);
        // Nothing measured is not saved.
        assert!(save(&d, &Profile::silent()).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }
}

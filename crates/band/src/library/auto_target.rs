//! Target "Auto": the target whose name matches the track's loaded instrument plugin.
//!
//! The DAW's own rule (MIDI Studio has no plugin to read; the spec's "Target auto-pick"): both
//! names are compared as lowercase letters and digits only. An exact match wins; else the
//! longest target name the plugin's name starts with ("Pianoteq 8 STAGE" plays Pianoteq); else
//! slot 0, Generic.

use super::{K_TARGET_SLOTS, LibraryData};

/// `s` as lowercase letters and digits only ("MODO BASS 2" is "modobass2").
fn normalized(s: &str) -> String {
    s.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The target slot "Auto" picks for an instrument plugin named `plugin_name`.
pub fn auto_target(lib: &LibraryData, plugin_name: &str) -> usize {
    let plugin = normalized(plugin_name);
    if plugin.is_empty() {
        return 0;
    }
    let mut best: Option<(usize, usize)> = None; // (slot, matched length)
    for slot in 0..K_TARGET_SLOTS {
        let Some(t) = lib.target(slot as i32) else { continue };
        let name = normalized(&t.name);
        if name.is_empty() {
            continue;
        }
        if name == plugin {
            return slot;
        }
        if plugin.starts_with(&name) && best.is_none_or(|(_, len)| name.len() > len) {
            best = Some((slot, name.len()));
        }
    }
    best.map_or(0, |(slot, _)| slot)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::library::embedded_library;

    #[test]
    fn auto_target_picks_by_name() {
        let lib = embedded_library();
        let name = |plugin: &str| lib.target(auto_target(&lib, plugin) as i32).map(|t| t.name.clone()).unwrap_or_default();
        assert_eq!(name("MODO DRUM"), "MODO DRUM");
        assert_eq!(name("MODO BASS 2"), "MODO BASS 2");
        assert_eq!(name("Pianoteq 8 STAGE"), "Pianoteq");
        assert_eq!(name("Serum"), "Generic");
        assert_eq!(name(""), "Generic");
        assert_eq!(auto_target(&lib, "Serum"), 0);
        // Case, spaces and punctuation don't count.
        assert_eq!(name("modo-drum"), "MODO DRUM");
        assert_eq!(name("swam strings"), "SWAM Strings");
    }
}

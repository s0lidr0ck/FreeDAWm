//! Third-party VST3 plugins: discovery (`engine.vst3_plugins`, which blocks for the scan, run
//! out of process; `cached: true` lists the scan cache without scanning). Hosting itself lives in
//! `soundcraft-vst3-host`; id lookup for the mixer commands is shared with CLAP in
//! `cmd::clap::plugin_info`.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        query "engine.vst3_plugins",
        "List VST3 Plugins",
        [],
        None,
        "{rescan?: bool, cached?: bool} → [{id: \"vst3:<class id hex>\", class_id, name, vendor, version, sdk_version, sub_categories, category, is_instrument, path}]",
        always,
        |_, p| {
            // Scanning blocks the caller until done (out of process; minutes the first time):
            // `cached` lists the last scan instead, as the app's own lists do.
            let list = if p.get("cached").and_then(Value::as_bool).unwrap_or(false) {
                soundcraft_vst3_host::cached().unwrap_or_default()
            } else if p.get("rescan").and_then(Value::as_bool).unwrap_or(false) {
                soundcraft_vst3_host::rescan()
            } else {
                soundcraft_vst3_host::scan()
            };
            Ok(json!(list))
        }
    )]
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    // `engine.vst3_plugins` scans the system VST3 folders, loading every installed binary.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn vst3_plugins_query_returns_a_list() {
        let mut e = Engine::default();
        let v = e.execute("engine.vst3_plugins", &json!({})).unwrap();
        assert!(v.is_array());
    }

    #[test]
    fn cached_vst3_plugins_never_scan() {
        let mut e = Engine::default();
        let v = e.execute("engine.vst3_plugins", &json!({"cached": true})).unwrap();
        assert!(v.is_array());
    }

    #[test]
    fn malformed_vst3_insert_is_an_error() {
        // A malformed id is refused before any scan.
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        assert!(e.execute("mix.insert", &json!({"track": t, "plugin": "vst3:not-hex"})).is_err());
    }

    // A well-formed but unknown id is looked up in the scan of the system VST3 folders.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn unknown_vst3_insert_is_an_error() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({})).unwrap();
        let t = e.session().tracks[0].id.0;
        let id = "vst3:00000000000000000000000000000000";
        assert!(e.execute("mix.insert", &json!({"track": t, "plugin": id})).is_err());
        assert!(crate::cmd::clap::plugin_info(id).is_none());
    }
}

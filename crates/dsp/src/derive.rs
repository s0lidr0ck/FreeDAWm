//! Derived parameters: host-side parameter logic a plugin declares, as a C++ wrapper's own
//! parameter listeners (FireStation Dist's Link, which derives Level from Drive, and its
//! per-circuit knob memory; Mod's Link, which derives Level from Depth).
//!
//! The host runs a plugin's [`DeriveFn`] in its command layer right after an edit has been
//! applied to the module's stored values, never on the audio thread, so whatever it writes is
//! part of the same (undoable) edit. A parameter the host marks locked (one with an automation
//! lane) is never written: the lane wins. The function may also keep a small per-module memory
//! (`memory`), which the host stores with the module (it travels with the session and undo).

use std::collections::BTreeMap;

use crate::PluginInfo;

/// What kind of edit the derived parameters react to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamEdit {
    /// Parameters set by a control, a command or the generic editor.
    Edit,
    /// A preset recall (a whole set of values from a preset).
    Preset,
}

/// One run of a plugin's derived-parameter logic.
pub struct Derive<'a> {
    info: &'static PluginInfo,
    before: &'a BTreeMap<String, f32>,
    values: &'a mut BTreeMap<String, f32>,
    /// The module's own memory (per plugin keys), kept by the host with the module.
    pub memory: &'a mut BTreeMap<String, f32>,
    changed: &'a [String],
    edit: ParamEdit,
    locked: &'a dyn Fn(&str) -> bool,
}

impl<'a> Derive<'a> {
    /// `before`: the stored values before the edit; `values`: after it (written in place);
    /// `changed`: the ids the edit set; `locked`: parameters derived writes must leave alone.
    pub fn new(
        info: &'static PluginInfo,
        before: &'a BTreeMap<String, f32>,
        values: &'a mut BTreeMap<String, f32>,
        memory: &'a mut BTreeMap<String, f32>,
        changed: &'a [String],
        edit: ParamEdit,
        locked: &'a dyn Fn(&str) -> bool,
    ) -> Self {
        Derive { info, before, values, memory, changed, edit, locked }
    }

    pub fn edit(&self) -> ParamEdit {
        self.edit
    }

    /// The ids the edit set.
    pub fn changed(&self) -> &[String] {
        self.changed
    }

    /// Whether the edit set `id`.
    pub fn was_set(&self, id: &str) -> bool {
        self.changed.iter().any(|c| c == id)
    }

    /// A parameter's current value (stored, else its default; clamped). NaN for an unknown id.
    pub fn get(&self, id: &str) -> f32 {
        self.info.param(id).map_or(f32::NAN, |p| p.clamp(self.values.get(id).copied().unwrap_or(p.default)))
    }

    /// A parameter's value before the edit (as [`Derive::get`]).
    pub fn before(&self, id: &str) -> f32 {
        self.info.param(id).map_or(f32::NAN, |p| p.clamp(self.before.get(id).copied().unwrap_or(p.default)))
    }

    /// Sets a parameter (clamped into its range). `false`, and nothing written, for an unknown or
    /// locked parameter.
    pub fn set(&mut self, id: &str, value: f32) -> bool {
        let Some(p) = self.info.param(id) else { return false };
        if (self.locked)(id) {
            return false;
        }
        self.values.insert(id.to_string(), p.clamp(value));
        true
    }
}

/// A plugin's derived-parameter logic.
pub type DeriveFn = fn(&mut Derive<'_>);

/// The derived-parameter logic of a plugin, `None` for the (many) plugins without any.
pub fn param_deriver(plugin_id: &str) -> Option<DeriveFn> {
    match plugin_id {
        "fs_dist" => Some(crate::plugins::fs::dist::derive),
        "fs_mod" => Some(crate::plugins::fs::mod_engine::derive),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn set_clamps_and_respects_locks_and_unknown_ids() {
        let info = crate::plugin_info("fs_dist").unwrap();
        let before = BTreeMap::new();
        let mut values = BTreeMap::new();
        let mut memory = BTreeMap::new();
        let changed = vec!["drive".to_string()];
        let locked = |id: &str| id == "tone";
        let mut d = Derive::new(info, &before, &mut values, &mut memory, &changed, ParamEdit::Edit, &locked);
        assert!(d.set("level", 7.0));
        assert_eq!(d.get("level"), 1.0);
        assert!(!d.set("tone", 0.1), "locked");
        assert_eq!(d.get("tone"), 0.5, "default");
        assert!(!d.set("nope", 0.1));
        assert!(d.get("nope").is_nan());
        assert!(d.was_set("drive") && !d.was_set("tone"));
        assert_eq!(d.before("circuit"), 0.0);
        assert!(param_deriver("fs_dist").is_some());
        assert!(param_deriver("fs_mod").is_some());
        assert!(param_deriver("compressor").is_none());
    }
}

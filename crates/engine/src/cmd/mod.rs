//! The command registry. One module per menu area; each exposes `specs()`.

use crate::{Engine, EngineError, Result};
use serde_json::Value;
use soundcraft_model::{ClipId, TrackId};
use soundcraft_time::{Range, Samples};
use std::sync::OnceLock;

mod audio_midi;
pub(crate) mod auto_write;
pub use auto_write::TOUCH_HOLD_SECONDS;
mod audiosuite;
mod automation_edit;
mod band;
pub mod chord_lane;
mod clap;
mod clip;
mod clip_clean;
mod clip_list;
mod clip_more;
pub mod comp;
mod edit;
mod edit_more;
pub mod editor_link;
mod event;
pub(crate) mod event_more;
mod file;
pub(crate) mod freeze;
mod module_record;
pub use freeze::all_frozen;
pub use freeze::{UNFREEZE_TO_EDIT, cannot_freeze, cannot_freeze_now};
mod health;
mod instrument;
pub mod juke;
mod looping;
mod lyric_alt;
pub mod lyric_check;
pub mod lyric_place;
mod lyric_sheet;
mod lyric_words;
mod lyrics;
mod midi;
mod midi_cleanup;
pub mod midi_ctrl;
mod mix;
pub mod mix_view;
#[cfg(not(target_arch = "wasm32"))]
pub(crate) mod ml;
pub(crate) use mix::restore_plugin_param_ids;
mod chain;
pub(crate) mod copyq;
pub(crate) mod module;
pub(crate) mod module_asset;
pub(crate) mod module_ceiling;
pub(crate) mod module_link;
mod module_preset;
mod more_util;
pub mod notes;
mod options;
mod pattern;
mod plugin_state;
mod query;
pub(crate) mod sampler;
mod scout;
pub mod scout_profile;
mod setup_more;
mod surround;
mod tab;
pub mod tempo;
mod track;
mod track_more;
mod transport;
mod video;
mod view;
mod view_more;
mod vocal;
mod vocal_studio;
mod vst3;
mod zoom_toggle;

pub use clap::plugin_info;
pub use file::bounce_job;
/// What the UI needs of Scout: the store refusal for a capture no longer pending, and "same section".
pub use scout::{NOT_PENDING as SCOUT_NOT_PENDING, same_as as scout_same_as};

pub(crate) use edit::remember_selection;
pub(crate) use looping::sync_loop;
pub use looping::{NO_LOOP as LOOP_NONE, NO_SELECTION as LOOP_NO_SELECTION};

pub type Run = fn(&mut Engine, &Value) -> Result<Value>;
/// `Err(reason)` = disabled.
pub type Enabled = fn(&Engine) -> std::result::Result<(), String>;

/// A registered command.
pub struct CommandSpec {
    pub id: &'static str,
    pub label: &'static str,
    /// Menu path, e.g. `["Edit", "Trim"]`; empty = not in menus.
    pub menu: &'static [&'static str],
    pub shortcut: Option<&'static str>,
    /// Human/agent parameter documentation.
    pub params: &'static str,
    pub enabled: Enabled,
    pub run: Run,
    pub journal: bool,
    pub undoable: bool,
    /// Only touches `edit.zoom`, through `Engine::zoom_mut`.
    pub view: bool,
}

/// Serializable description of a command.
#[derive(Debug, Clone, serde::Serialize)]
pub struct CommandInfo {
    pub id: String,
    pub label: String,
    pub menu: Vec<String>,
    pub shortcut: Option<String>,
    pub params: String,
    pub enabled: bool,
    pub disabled_reason: Option<String>,
}

impl CommandSpec {
    pub fn info(&self, e: &Engine) -> CommandInfo {
        let en = (self.enabled)(e);
        CommandInfo {
            id: self.id.into(),
            label: self.label.into(),
            menu: self.menu.iter().map(|s| s.to_string()).collect(),
            shortcut: self.shortcut.map(str::to_string),
            params: self.params.into(),
            enabled: en.is_ok(),
            disabled_reason: en.err(),
        }
    }
}

/// `cmd!(id, label, [menu…], shortcut, params, enabled, run)`; `query`/`noundo`/`view` variants.
#[macro_export]
macro_rules! cmd {
    (query $($t:tt)*) => { $crate::cmd!(@spec false, false, false; $($t)*) };
    (noundo $($t:tt)*) => { $crate::cmd!(@spec true, false, false; $($t)*) };
    (view $($t:tt)*) => { $crate::cmd!(@spec false, false, true; $($t)*) };
    (@spec $journal:literal, $undoable:literal, $view:literal; $id:literal, $label:literal, [$($m:literal),*], $sc:expr, $params:literal, $en:expr, $run:expr) => {
        $crate::cmd::CommandSpec { id: $id, label: $label, menu: &[$($m),*], shortcut: $sc, params: $params, enabled: $en, run: $run, journal: $journal, undoable: $undoable, view: $view }
    };
    ($id:literal, $($t:tt)*) => { $crate::cmd!(@spec true, true, false; $id, $($t)*) };
}

pub fn command_specs() -> &'static [CommandSpec] {
    static SPECS: OnceLock<Vec<CommandSpec>> = OnceLock::new();
    SPECS.get_or_init(|| {
        let mut v = Vec::new();
        v.extend(file::specs());
        v.extend(edit::specs());
        v.extend(module_record::specs());
        v.extend(edit_more::specs());
        v.extend(view::specs());
        v.extend(track::specs());
        v.extend(freeze::specs());
        v.extend(clip::specs());
        v.extend(pattern::specs());
        v.extend(juke::specs());
        v.extend(clip_clean::specs());
        v.extend(event::specs());
        v.extend(mix::specs());
        v.extend(mix_view::specs());
        v.extend(automation_edit::specs());
        v.extend(auto_write::specs());
        v.extend(options::specs());
        v.extend(transport::specs());
        v.extend(tab::specs());
        v.extend(query::specs());
        v.extend(health::specs());
        v.extend(audiosuite::specs());
        v.extend(audio_midi::specs());
        v.extend(midi::specs());
        v.extend(midi_ctrl::specs());
        v.extend(midi_cleanup::specs());
        v.extend(view_more::specs());
        v.extend(track_more::specs());
        v.extend(chain::specs());
        v.extend(clip_more::specs());
        v.extend(clip_list::specs());
        v.extend(event_more::specs());
        v.extend(tempo::specs());
        v.extend(setup_more::specs());
        v.extend(clap::specs());
        v.extend(vst3::specs());
        v.extend(surround::specs());
        v.extend(plugin_state::specs());
        v.extend(module::specs());
        v.extend(module_ceiling::specs());
        v.extend(copyq::specs());
        v.extend(module_asset::specs());
        v.extend(module_link::specs());
        v.extend(module_preset::specs());
        v.extend(instrument::specs());
        v.extend(sampler::specs());
        v.extend(video::specs());
        v.extend(zoom_toggle::specs());
        v.extend(editor_link::specs());
        v.extend(scout::specs());
        v.extend(scout_profile::specs());
        v.extend(looping::specs());
        v.extend(band::specs());
        v.extend(lyrics::specs());
        v.extend(notes::specs());
        v.extend(lyric_sheet::specs());
        v.extend(lyric_check::specs());
        v.extend(lyric_alt::specs());
        v.extend(lyric_words::specs());
        v.extend(lyric_place::specs());
        v.extend(vocal::specs());
        v.extend(vocal_studio::specs());
        v.extend(comp::specs());
        #[cfg(not(target_arch = "wasm32"))]
        v.extend(ml::specs());
        v
    })
}

/// Old command ids that keep working, `(old, current)`: scripts and control clients written
/// against an old id still run the command. The registry, the menus and the command list show
/// only the current id.
pub const COMMAND_ALIASES: &[(&str, &str)] = &[
    // AudioSuite was renamed Process (2026-11-02).
    ("audiosuite.process", "process.run"),
];

/// The current id for `id`: itself, or what an old id in [`COMMAND_ALIASES`] is now called.
pub fn canonical_id(id: &str) -> &str {
    COMMAND_ALIASES.iter().find(|(old, _)| *old == id).map_or(id, |(_, new)| new)
}

pub fn find_command(id: &str) -> Option<&'static CommandSpec> {
    let id = canonical_id(id);
    command_specs().iter().find(|c| c.id == id)
}

// ---- enablement --------------------------------------------------------------------------

pub fn always(_: &Engine) -> std::result::Result<(), String> {
    Ok(())
}

pub fn has_tracks(e: &Engine) -> std::result::Result<(), String> {
    if e.session().tracks.is_empty() { Err("the session has no tracks".into()) } else { Ok(()) }
}

pub fn has_selection(e: &Engine) -> std::result::Result<(), String> {
    let s = e.session();
    if s.edit.selected_tracks.is_empty() && s.edit.selected_clips.is_empty() { Err("nothing is selected".into()) } else { Ok(()) }
}

pub fn has_range(e: &Engine) -> std::result::Result<(), String> {
    let s = e.session();
    if (s.edit.selected_tracks.is_empty() || s.edit.selection.is_empty()) && s.edit.selected_clips.is_empty() {
        Err("make an edit selection first".into())
    } else {
        Ok(())
    }
}

pub fn can_undo(e: &Engine) -> std::result::Result<(), String> {
    if e.can_undo() { Ok(()) } else { Err("nothing to undo".into()) }
}

pub fn can_redo(e: &Engine) -> std::result::Result<(), String> {
    if e.can_redo() { Ok(()) } else { Err("nothing to redo".into()) }
}

pub fn has_clipboard(e: &Engine) -> std::result::Result<(), String> {
    if e.clipboard.tracks.is_empty() { Err("the clipboard is empty".into()) } else { Ok(()) }
}

// ---- params ------------------------------------------------------------------------------

pub fn bad(cmd: &str, msg: impl Into<String>) -> EngineError {
    EngineError::BadParams(cmd.to_string(), msg.into())
}

pub fn f64_or(p: &Value, key: &str, d: f64) -> f64 {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or(d)
}

pub fn f32_or(p: &Value, key: &str, d: f32) -> f32 {
    f64_or(p, key, f64::from(d)) as f32
}

pub fn i64_or(p: &Value, key: &str, d: i64) -> i64 {
    p.get(key).and_then(|v| v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite()).map(|f| f.round() as i64))).unwrap_or(d)
}

pub fn bool_or(p: &Value, key: &str, d: bool) -> bool {
    p.get(key).and_then(Value::as_bool).unwrap_or(d)
}

pub fn str_param<'a>(p: &'a Value, key: &str) -> Option<&'a str> {
    p.get(key).and_then(Value::as_str)
}

/// Resolve a track from `track` (id number or name) params.
pub fn track_param(e: &Engine, cmd: &str, p: &Value, key: &str) -> Result<Option<TrackId>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let s = e.session();
    if let Some(n) = v.as_u64() {
        return s.track(TrackId(n)).map(|t| Some(t.id)).ok_or_else(|| bad(cmd, format!("no track with id {n}")));
    }
    if let Some(name) = v.as_str() {
        return s.track_by_name(name).map(|t| Some(t.id)).ok_or_else(|| bad(cmd, format!("no track named `{name}`")));
    }
    Err(bad(cmd, format!("`{key}` must be a track id or name")))
}

/// Tracks to act on: `tracks` (array of ids/names) or `track`, else the selected tracks.
pub fn tracks_param(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<TrackId>> {
    if let Some(arr) = p.get("tracks").and_then(Value::as_array) {
        let s = e.session();
        let mut out = Vec::new();
        for v in arr {
            let id = match (v.as_u64(), v.as_str()) {
                (Some(n), _) => s.track(TrackId(n)).map(|t| t.id),
                (_, Some(name)) => s.track_by_name(name).map(|t| t.id),
                _ => None,
            };
            out.push(id.ok_or_else(|| bad(cmd, format!("unknown track {v}")))?);
        }
        return Ok(out);
    }
    if let Some(t) = track_param(e, cmd, p, "track")? {
        return Ok(vec![t]);
    }
    Ok(e.target_tracks())
}

/// Like `tracks_param` but errors when empty.
pub fn tracks_required(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<TrackId>> {
    let t = tracks_param(e, cmd, p)?;
    if t.is_empty() { Err(bad(cmd, "no tracks: pass `track`/`tracks` or select tracks")) } else { Ok(t) }
}

pub use soundcraft_time::MAX_POSITION;

/// Position from `key`: a number of samples, or `{"seconds": x}`, or a string in the main counter format.
/// Values are clamped to ±[`MAX_POSITION`].
pub fn position_param(e: &Engine, cmd: &str, p: &Value, key: &str) -> Result<Option<Samples>> {
    Ok(position_param_raw(e, cmd, p, key)?.map(|v| v.clamp(-MAX_POSITION, MAX_POSITION)))
}

fn position_param_raw(e: &Engine, cmd: &str, p: &Value, key: &str) -> Result<Option<Samples>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let s = e.session();
    if let Some(n) = v.as_i64() {
        return Ok(Some(n));
    }
    if let Some(f) = v.as_f64() {
        return Ok(Some(soundcraft_time::to_samples(f)));
    }
    if let Some(secs) = v.get("seconds").and_then(Value::as_f64) {
        return Ok(Some(s.sample_rate.samples(secs)));
    }
    if let Some(text) = v.as_str() {
        // Try each format, starting with an explicit prefix like "bars:5|1|000".
        let (fmt, body) = match text.split_once(':') {
            Some((f, b)) if soundcraft_time::TimeFormat::from_id(f).is_some() => (soundcraft_time::TimeFormat::from_id(f), b),
            _ => (None, text),
        };
        let fmts: Vec<soundcraft_time::TimeFormat> = match fmt {
            Some(f) => vec![f],
            None => {
                let mut v = vec![s.edit.main_counter];
                v.extend(soundcraft_time::TimeFormat::ALL);
                v
            }
        };
        for f in fmts {
            if let Ok(x) = s.parse_position(body, f) {
                return Ok(Some(x));
            }
        }
        return Err(bad(cmd, format!("cannot parse position `{text}`")));
    }
    Err(bad(cmd, format!("`{key}` must be samples, {{seconds}}, or a time string")))
}

/// Range from `start`/`end` params, else the edit selection.
pub fn range_param(e: &Engine, cmd: &str, p: &Value) -> Result<Range> {
    let st = position_param(e, cmd, p, "start")?;
    let en = position_param(e, cmd, p, "end")?;
    let len = position_param(e, cmd, p, "length")?;
    match (st, en, len) {
        (Some(a), Some(b), _) => Ok(Range::new(a, b)),
        (Some(a), None, Some(l)) => Ok(Range::new(a, a.saturating_add(l))),
        (Some(a), None, None) => Ok(Range::point(a)),
        _ => Ok(e.session().edit.selection),
    }
}

pub fn clip_id(p: &Value, cmd: &str) -> Result<ClipId> {
    p.get("clip").and_then(Value::as_u64).map(ClipId).ok_or_else(|| bad(cmd, "`clip` id required"))
}

pub fn clip_ids_param(e: &Engine, p: &Value) -> Vec<ClipId> {
    if let Some(arr) = p.get("clips").and_then(Value::as_array) {
        return arr.iter().filter_map(Value::as_u64).map(ClipId).collect();
    }
    if let Some(n) = p.get("clip").and_then(Value::as_u64) {
        return vec![ClipId(n)];
    }
    let s = e.session();
    if !s.edit.selected_clips.is_empty() {
        return s.edit.selected_clips.clone();
    }
    // Clips touched by the edit selection on selected tracks.
    let r = s.edit.selection;
    s.tracks
        .iter()
        .filter(|t| s.edit.selected_tracks.contains(&t.id))
        .flat_map(|t| t.clips().iter().filter(move |c| if r.is_empty() { c.range().contains(r.start) } else { c.range().overlaps(&r) }).map(|c| c.id))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ids_are_unique_and_documented() {
        let mut seen = std::collections::HashSet::new();
        for c in command_specs() {
            assert!(seen.insert(c.id), "duplicate command id {}", c.id);
            assert!(!c.label.is_empty());
            assert!(c.id.contains('.'), "ids are namespaced: {}", c.id);
        }
        assert!(command_specs().len() > 150, "only {} commands", command_specs().len());
    }

    // Includes `engine.vst3_plugins`, which scans (loads) every installed VST3 plugin;
    // `commands_without_plugin_loading_survive_empty_params` below runs the rest everywhere.
    #[test]
    #[ignore = "loads every installed VST3 plugin (slow; some abort the test process); run with --ignored"]
    fn every_command_survives_empty_params() {
        // No command may panic with `{}` params on an empty or demo session.
        for demo in [false, true] {
            for c in command_specs() {
                if matches!(c.id, "app.quit") {
                    continue;
                }
                let mut e = if demo { crate::demo::demo_engine() } else { Engine::default() };
                let r = e.execute(c.id, &serde_json::json!({}));
                if let Err(EngineError::Internal(id, msg)) = r {
                    panic!("{id} panicked: {msg}");
                }
            }
        }
    }

    /// Commands that reach the VST3 loader with `{}` params (the scan loads every installed
    /// binary). Some installed VST3 plugins hang or crash the test process, so the sweep below
    /// leaves these out; the full sweep above still covers them where loading is safe.
    const LOADS_VST3_BINARIES: &[&str] = &["engine.vst3_plugins"];

    #[test]
    fn commands_without_plugin_loading_survive_empty_params() {
        // The same sweep as `every_command_survives_empty_params`, minus the VST3 scan, so it
        // runs everywhere. Every panic is listed, not just the first.
        let mut panics = Vec::new();
        let mut ran = 0;
        for demo in [false, true] {
            for c in command_specs() {
                if c.id == "app.quit" || LOADS_VST3_BINARIES.contains(&c.id) {
                    continue;
                }
                ran += 1;
                let mut e = if demo { crate::demo::demo_engine() } else { Engine::default() };
                if let Err(EngineError::Internal(id, msg)) = e.execute(c.id, &serde_json::json!({})) {
                    panics.push(format!("{id} (demo: {demo}): {msg}"));
                }
            }
        }
        assert!(ran > 300, "only {ran} runs");
        assert!(panics.is_empty(), "commands panicked:\n{}", panics.join("\n"));
    }
}

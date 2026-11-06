//! Track › Freeze: an audio track's clips, or an instrument's output for a MIDI or instrument
//! track's MIDI clips (with its release tail), are rendered through the track's module chain
//! (pre-fader) to an audio file of the session, which the track then plays in place of its clips,
//! instrument and modules, so they cost nothing. The fader, pan, sends, mute, solo, output and their automation stay
//! live. "Freeze up to here" (`upto`) renders the chain only down to and including one module
//! (the freeze point): the modules after it stay live and process the render. While frozen, edits
//! to what was frozen (clips, the modules down to the point and their automation, input, trim,
//! phase, width) are refused by [`refused_edit`], which [`Engine::execute`] runs after every
//! command. Freezing, moving the point and unfreezing are one undo step each.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{AutoParam, FrozenAudio, Insert, ModuleId, Session, SourceId, Track, TrackKind};
use soundcraft_time::Samples;

/// What a refused edit says.
pub const UNFREEZE_TO_EDIT: &str = "Unfreeze the track to edit it";

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!("track.unfreeze", "Unfreeze", [], None, "{tracks?}", has_selection, |e, p| {
        let tracks = tracks_required(e, "track.unfreeze", p)?;
        Ok(json!({"unfrozen": unfreeze(e, &tracks)}))
    })]
}

/// `track.freeze {tracks?, freeze?: bool, upto?: module id}`: freezes the tracks (the whole chain,
/// or down to and including the module `upto`), or thaws them when `freeze` is false or (with no
/// `freeze` and no `upto`) when every one is frozen already. A frozen track asked for another
/// freeze point (or the whole chain, with `freeze: true`) is rendered again there.
/// Whether `track.freeze` without `freeze`/`upto` thaws `tracks` instead: every one is frozen
/// (Track › Freeze reads Unfreeze then).
pub fn all_frozen(e: &Engine, tracks: &[TrackId]) -> bool {
    !tracks.is_empty() && tracks.iter().all(|t| e.session().track(*t).is_some_and(|tr| tr.frozen_audio().is_some()))
}

/// What a `track.freeze` call renders: the tracks (with the end of their content) and the freeze
/// point, decided before anything is rendered (a refused freeze freezes nothing).
pub struct FreezePlan {
    pub upto: Option<ModuleId>,
    pub todo: Vec<(TrackId, Samples)>,
    pub warnings: Vec<String>,
}

/// The plan of a `track.freeze` call, or `None` when the call thaws instead.
pub fn plan(e: &Engine, p: &Value) -> Result<Option<FreezePlan>> {
    const C: &str = "track.freeze";
    let tracks = tracks_required(e, C, p)?;
    let upto = match p.get("upto") {
        None | Some(Value::Null) => None,
        Some(v) => Some(ModuleId(v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(C, "upto: a module id"))?)),
    };
    if !p.get("freeze").and_then(Value::as_bool).unwrap_or(upto.is_some() || !all_frozen(e, &tracks)) {
        return Ok(None);
    }
    let mut todo = Vec::new();
    for t in tracks {
        let tr = e.session().track(t).ok_or_else(|| bad(C, "no such track"))?;
        if let Some(m) = upto.filter(|m| tr.mixer.module_index(*m).is_none()) {
            return Err(bad(C, format!("`{}` has no module {}", tr.name, m.0)));
        }
        if tr.frozen_audio().is_some_and(|f| f.upto == upto) {
            continue;
        }
        if let Some(why) = cannot_freeze_now(tr) {
            return Err(bad(C, why));
        }
        let end = content_end(tr);
        crate::io::check_freeze_length(e.session(), tr, end)?;
        todo.push((t, end));
    }
    let warnings: Vec<String> = todo.iter().flat_map(|(t, _)| silent_keys(e.session(), *t, upto)).collect();
    Ok(Some(FreezePlan { upto, todo, warnings }))
}

pub fn freeze(e: &mut Engine, p: &Value) -> Result<Value> {
    let Some(FreezePlan { upto, todo, warnings }) = plan(e, p)? else {
        let tracks = tracks_required(e, "track.freeze", p)?;
        return Ok(json!({"unfrozen": unfreeze(e, &tracks)}));
    };
    for w in &warnings {
        e.message(w.clone());
    }
    // The render plays each MIDI clip's cleaned notes as they are now (an edit made outside a
    // command may have left a preview stale); frozen, the preview is kept with the render.
    if crate::midi_cleanup::any_stale(e.session()) {
        crate::midi_cleanup::refresh(e.session_mut());
    }
    let mut frozen = Vec::new();
    let mut seconds = 0.0;
    for (t, end) in todo {
        let (source, frames) = crate::io::freeze_track(e, t, end, upto)?;
        let s = e.session_mut();
        if let Some(tr) = s.track_mut(t) {
            tr.frozen = true;
            // Moving the freeze point replaces the render.
            if let Some(old) = tr.freeze.replace(FrozenAudio { source, start: 0, upto }) {
                drop_unused_source(s, old.source);
            }
        }
        seconds += s.sample_rate.seconds(frames);
        frozen.push(t);
    }
    Ok(json!({"frozen": frozen, "seconds": seconds, "warnings": warnings}))
}

/// Why `t` can't be frozen (`None`: it can, given something audible to render): Freeze renders
/// audio tracks and MIDI and instrument tracks with an instrument. Shown as is (refusals, menu
/// tooltips).
pub fn cannot_freeze(t: &Track) -> Option<String> {
    const WHAT: &str = "Freeze works on audio tracks and tracks with an instrument";
    match t.kind {
        TrackKind::Audio => None,
        TrackKind::Instrument if t.instrument.is_none() => Some(format!("{WHAT}: `{}` has no instrument", t.name)),
        // Only an instrument track can be given one (`mix.instrument`).
        TrackKind::Midi if t.instrument.is_none() => {
            Some(format!("{WHAT}: `{}` is a MIDI track with no instrument (use an instrument track)", t.name))
        }
        TrackKind::Instrument | TrackKind::Midi if matches!(t.mixer.input, soundcraft_model::Route::Musician(_)) => {
            Some(format!("`{}` plays a band musician live: set its input back to its clips to freeze it", t.name))
        }
        TrackKind::Instrument | TrackKind::Midi if matches!(t.mixer.input, soundcraft_model::Route::Modules(_)) => {
            Some(format!("`{}` plays a module's MIDI live: set its input back to its clips to freeze it", t.name))
        }
        TrackKind::Instrument | TrackKind::Midi => None,
        k => {
            let a = if k.label().starts_with(['A', 'E', 'I', 'O', 'U']) { "an" } else { "a" };
            Some(format!("{WHAT}: `{}` is {a} {} with no instrument", t.name, k.label()))
        }
    }
}

/// Why `t` can't be frozen now: [`cannot_freeze`], or nothing audible to render (no clips, or
/// every clip muted). What `track.freeze` refuses with; the UI skips such tracks of a selection.
pub fn cannot_freeze_now(t: &Track) -> Option<String> {
    if let Some(why) = cannot_freeze(t) {
        return Some(why);
    }
    if content_end(t) <= 0 {
        let why = if t.clips().is_empty() { "it has no clips" } else { "every clip is muted" };
        return Some(format!("`{}` has nothing audible to freeze: {why}", t.name));
    }
    None
}

/// Where the render differs from live: the render plays the track alone, so a sidechain key from
/// another track is silent in it. Only the frozen modules (down to `upto`) count: the live ones
/// after the point still hear their keys.
fn silent_keys(s: &Session, t: TrackId, upto: Option<ModuleId>) -> Vec<String> {
    let Some(tr) = s.track(t) else { return Vec::new() };
    let n = upto.and_then(|m| tr.mixer.module_index(m)).map_or(tr.mixer.inserts.len(), |at| at.saturating_add(1));
    tr.mixer
        .inserts
        .iter()
        .take(n)
        .filter(|i| i.active && !i.bypass)
        .filter_map(|i| i.sidechain.as_ref().filter(|k| k.track != t))
        .map(|k| {
            let from = s.track(k.track).map_or_else(|| "a deleted track".to_string(), |x| format!("`{}`", x.name));
            format!("`{}` was rendered without the sidechain key from {from}", tr.name)
        })
        .collect()
}

/// The end of the track's last unmuted clip (0 with none).
fn content_end(t: &Track) -> Samples {
    t.clips().iter().filter(|c| !c.muted).map(soundcraft_model::Clip::end).max().unwrap_or(0)
}

/// Thaws the tracks; their renders leave the session (the files stay on disk, and undo brings
/// them back). Returns the tracks thawed.
fn unfreeze(e: &mut Engine, tracks: &[TrackId]) -> Vec<TrackId> {
    let s = e.session_mut();
    let mut out = Vec::new();
    for t in tracks {
        let Some(tr) = s.track_mut(*t) else { continue };
        if !tr.frozen && tr.freeze.is_none() {
            continue;
        }
        tr.frozen = false;
        if let Some(fz) = tr.freeze.take() {
            drop_unused_source(s, fz.source);
        }
        out.push(*t);
    }
    out
}

/// Removes a source nothing plays: no clip, no other frozen track, no module or instrument
/// sample.
pub(crate) fn drop_unused_source(s: &mut Session, src: SourceId) {
    let used = s.tracks.iter().any(|t| {
        t.freeze.is_some_and(|f| f.source == src)
            || t.mixer.inserts.iter().any(|i| i.sources().any(|x| x == src))
            || t.instrument.as_ref().is_some_and(|i| i.sources().any(|x| x == src))
            || t.playlists.iter().flat_map(|p| &p.clips).any(|c| c.uses_source(src))
    });
    if !used {
        s.sources.retain(|x| x.id != src);
        s.pool.remove(src);
    }
}

/// Commands that may change a frozen track's frozen parts: freezing and thawing, undo and redo
/// (which restore whole documents), and storing a hosted plugin's own state (not an edit).
const EXEMPT: [&str; 5] = ["track.freeze", "track.unfreeze", "edit.undo", "edit.redo", "mix.module_state"];

/// What a refused record enable says.
pub const UNFREEZE_TO_RECORD: &str = "Unfreeze the track to record on it";

/// Why the command `id`, which turned `before` into `after`, must be refused: it changed how a
/// track still frozen with the same render would sound live (so the render no longer matches),
/// or record-enabled it.
pub fn refused_edit(id: &str, before: &Session, after: &Session) -> Option<String> {
    if EXEMPT.contains(&id) {
        return None;
    }
    // A frozen MIDI or instrument track's render played its notes at the old tempo.
    if before.tempo != after.tempo
        && let Some(t) = before
            .tracks
            .iter()
            .find(|t| t.kind.is_midi() && t.frozen_audio().is_some() && after.track(t.id).is_some_and(|n| n.frozen_audio() == t.frozen_audio()))
    {
        return Some(format!("`{}` is frozen, and its MIDI plays to the tempo. Unfreeze it to change the tempo", t.name));
    }
    // Its render also played them with the MIDI Real-Time Properties and the MIDI Track Offset of
    // the time (session settings, not on the track).
    for t in before.tracks.iter().filter(|t| t.kind.is_midi() && t.frozen_audio().is_some()) {
        if !after.track(t.id).is_some_and(|n| n.frozen_audio() == t.frozen_audio()) {
            continue;
        }
        if track_values(before, "rtp", t.id) != track_values(after, "rtp", t.id) {
            return Some(format!("`{}` is frozen, and its MIDI Real-Time Properties shape its notes. Unfreeze it to change them", t.name));
        }
        let offset = format!("midi.offset.{}", t.id.0);
        if before.edit.values.get(&offset) != after.edit.values.get(&offset) {
            return Some(format!("`{}` is frozen, and its MIDI Track Offset moves its notes. Unfreeze it to change it", t.name));
        }
    }
    let refused = before.tracks.iter().filter(|t| t.frozen_audio().is_some()).find_map(|old| {
        // Only a track the command changed: one left alone never blocks a command.
        let new = after.track(old.id).filter(|new| new.frozen_audio() == old.frozen_audio() && *new != old)?;
        if new.mixer.record_arm && !old.mixer.record_arm {
            Some(format!("`{}` is frozen. {UNFREEZE_TO_RECORD}", old.name))
        } else if sounds_differ(before, old, after, new) {
            Some(format!("`{}` is frozen. {UNFREEZE_TO_EDIT}", old.name))
        } else {
            None
        }
    });
    if refused.is_some() {
        return refused;
    }
    // A frozen clip on Cleaned plays its MIDI Cleanup's notes, derived from the band's chords
    // and key: an edit of those under the clip (the preview is not saved with the session, so it
    // could not be kept past a reopen) is refused; one elsewhere leaves its inputs alone.
    before.tracks.iter().filter(|t| t.kind.is_midi() && t.frozen_audio().is_some()).find_map(|t| {
        let new = after.track(t.id).filter(|n| n.frozen_audio() == t.frozen_audio())?;
        let moved = t.clips().iter().zip(new.clips()).any(|(a, b)| {
            a.midi_cleanup.as_ref().is_some_and(|m| !m.original)
                && crate::midi_cleanup::fingerprint(before, a) != crate::midi_cleanup::fingerprint(after, b)
        });
        moved.then(|| format!("`{}` is frozen, and its MIDI Cleanup follows the chords. Unfreeze it to change them", t.name))
    })
}

/// What a freeze rendered, as far as it changes the sound: the active playlist's clips (where,
/// what, gain, fades, mute, stretch, clip effects; not names, colours, ratings, locks, groups or
/// sync points), the frozen modules (the whole chain, or down to and including the freeze point:
/// which ones, in which order, and their plugin, parameters, bypass, active, state, sample,
/// sidechain, files, link; not knobs or the preset label), their automation and the automation
/// mode, the input, trim, phase, width and Elastic Audio; and on a MIDI or instrument track the
/// instrument (plugin, parameters, state) and the clips' timebase. Modules after the point are live: any
/// edit to them (adding, removing and reordering them among themselves included) is allowed.
fn sounds_differ(sa: &Session, a: &Track, sb: &Session, b: &Track) -> bool {
    let (fa, fb) = (frozen_part(a), frozen_part(b));
    let module_lanes = |t: &Track| {
        let frozen = |m: &ModuleId| fa.is_some_and(|f| f.iter().any(|i| i.id == *m));
        t.automation.iter().filter(|l| matches!(&l.param, AutoParam::Plugin { module, .. } if frozen(module))).cloned().collect::<Vec<_>>()
    };
    !same_audible_clips(sa, a, sb, b)
        || fa.map(|f| f.iter().map(audible_module).collect::<Vec<_>>()) != fb.map(|f| f.iter().map(audible_module).collect::<Vec<_>>())
        || a.mixer.automation_mode != b.mixer.automation_mode
        || a.mixer.input != b.mixer.input
        || a.mixer.trim_db != b.mixer.trim_db
        || a.mixer.phase_invert != b.mixer.phase_invert
        || a.format != b.format
        || a.kind != b.kind
        || a.elastic != b.elastic
        || a.instrument.as_ref().map(audible_module) != b.instrument.as_ref().map(audible_module)
        || a.ticks_timebase != b.ticks_timebase
        || module_lanes(a) != module_lanes(b)
}

/// The modules a frozen track's render holds: the whole chain, or down to and including the freeze
/// point; [`Track::frozen_modules`], which the mix plays by (a point naming no module of the chain
/// counts as the whole chain, so removing the point module changes the frozen part). `None` while
/// not frozen.
fn frozen_part(t: &Track) -> Option<&[Insert]> {
    t.frozen_audio()?;
    t.mixer.inserts.get(..t.frozen_modules())
}

type AudibleModule<'a> = (
    soundcraft_model::ModuleId,
    &'a str,
    &'a std::collections::BTreeMap<String, f32>,
    bool,
    bool,
    &'a Option<String>,
    Option<SourceId>,
    &'a Option<soundcraft_model::SidechainSource>,
    &'a std::collections::BTreeMap<String, String>,
    Option<u32>,
    &'a std::collections::BTreeMap<String, SourceId>,
);

fn audible_module(i: &soundcraft_model::Insert) -> AudibleModule<'_> {
    (i.id, i.plugin.as_str(), &i.params, i.bypass, i.active, &i.state, i.sample, &i.sidechain, &i.assets, i.link_group, &i.samples)
}

/// The active playlists play the same: clip for clip, everything but identity and labels, clip
/// effects included (they live in the session's edit values, by clip id), and on a MIDI clip its
/// MIDI Cleanup (the settings, the Original/Cleaned switch, and the cleaned notes that play).
fn same_audible_clips(sa: &Session, a: &Track, sb: &Session, b: &Track) -> bool {
    let (ca, cb) = (a.clips(), b.clips());
    ca.len() == cb.len()
        && ca.iter().zip(cb).all(|(x, y)| {
            x.start == y.start
                && x.length == y.length
                && x.content == y.content
                && x.midi_cleanup == y.midi_cleanup
                && x.midi_playing() == y.midi_playing()
                && x.gain_db == y.gain_db
                && x.gain_env == y.gain_env
                && x.fade_in == y.fade_in
                && x.fade_out == y.fade_out
                && x.muted == y.muted
                && x.stretch == y.stretch
                && clip_fx(sa, x.id.0) == clip_fx(sb, y.id.0)
        })
}

/// A clip's effect settings (`clip_fx.<clip>.*` and its bypass), keyed without the clip id.
fn clip_fx(s: &Session, clip: u64) -> Vec<(String, f64)> {
    let mut out = values_under(s, &format!("clip_fx.{clip}."));
    if let Some(v) = s.edit.values.get(&format!("clip_fx.bypass.{clip}")) {
        out.push(("bypass".into(), *v));
    }
    out
}

/// A track's settings under `<group>.<track>.` (`rtp.<track>.*`: the MIDI Real-Time Properties),
/// keyed without that prefix.
fn track_values(s: &Session, group: &str, t: TrackId) -> Vec<(String, f64)> {
    values_under(s, &format!("{group}.{}.", t.0))
}

/// The edit values whose key starts with `pre`, keyed without it.
fn values_under(s: &Session, pre: &str) -> Vec<(String, f64)> {
    s.edit
        .values
        .range::<str, _>((std::ops::Bound::Included(pre), std::ops::Bound::Unbounded))
        .take_while(|(k, _)| k.starts_with(pre))
        .map(|(k, v)| (k.get(pre.len()..).unwrap_or_default().to_string(), *v))
        .collect()
}

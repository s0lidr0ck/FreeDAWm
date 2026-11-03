//! The Band: its six musicians, the band settings, band and feel presets, section definitions,
//! the section and chord lanes, factory songs, MIDI chord import and the Library.
//!
//! Positions are samples, as in the rest of the protocol; the lanes keep ticks, snapped to the
//! song grid's half bars (half a bar of the meter the band plays: 4/4, 3/4 and 6/8 as they are,
//! any other meter as 4/4). An item laid over others on its lane cuts them: what it covers goes,
//! and one it lands inside is split around it.

use super::{always, bad, position_param, str_param};
use crate::{Engine, Result, cmd};
use serde_json::{Map, Value, json};
use soundcraft_band::band_preset::{BandPreset, MusicianPreset, factory_band_presets, user_band_presets};
use soundcraft_band::chord_match::{match_chord, parse_chord_name};
use soundcraft_band::library::{INSTRUMENT_NAMES, LibraryData};
use soundcraft_band::song::Song;
use soundcraft_band::song_feel::{
    FEEL_DIAL_COUNT, UserFeelPreset, delete_feel_preset, dial_name, dial_positions, dial_title, factory_feel_presets, feel_presets_to_json,
    feel_presets_writable, load_feel_presets, save_feel_preset,
};
use soundcraft_band::song_file::factory_songs;
use soundcraft_mix::half_ticks;
use soundcraft_model::{BandState, ChordEvent, Clip, MAX_BLOCKS, MAX_CHORDS, MusicianSlot, Route, SectionBlock, SectionDef, Session, TrackId};
use soundcraft_time::{Range, Samples};
use std::path::{Path, PathBuf};

pub fn specs() -> Vec<super::CommandSpec> {
    vec![
        cmd!(
            "band.slot",
            "Band Musician",
            [],
            None,
            "{slot: 1-6, enabled?, mute?, instrument?: name|0-12, player?: name|\"Session\", target?: name|\"auto\", energy?: 1-4, swing?: 0-100, humanize?: 0-100, dynamics?: 0-100, fills?: 0-100, respect?, octave?: -2-2, length?: 10-100, velocity?: 1-127, speed?: half|normal|double, voice?: auto|top|2nd|3rd|bass, swaminst?: name|0-12, ksoctave?: -2-2|null, art?: [4 × 0-15], ample?: 0-2, groove?: 0-100, amplechords?: 0-1, pluckchords?: 0-1}",
            always,
            slot
        ),
        cmd!(
            "band.settings",
            "Band Settings",
            [],
            None,
            "{key?: 0-11|\"C\"..\"B\", scale?: major|minor, take?: 1-16, feel?: {natural?, tension?, bass_lock?, fills_yield?, leave_space?, dynamics?}, song_feel?: {on?, time?, density?, pocket?, groove?, touch?}, feel_preset?: name, feel_ignore?: [slot]}",
            always,
            settings
        ),
        cmd!("band.preset.load", "Load Band Preset", [], None, "{name}", always, preset_load),
        cmd!(noundo "band.preset.save", "Save Band Preset", [], None, "{name, replace?: false}", always, preset_save),
        cmd!(query "band.preset.list", "Band Presets", [], None, "{}", always, preset_list),
        cmd!(noundo "band.feel_preset.save", "Save Feel Preset", [], None, "{name}", always, feel_preset_save),
        cmd!(noundo "band.feel_preset.delete", "Delete Feel Preset", [], None, "{name}", always, feel_preset_delete),
        cmd!(
            "band.def",
            "Section Definition",
            [],
            None,
            "{id: A-H, name?, energy_shift?: -3-3, shape?: none|build|drop, role?: none|intro|ending, take?: 0-16, follow_idea?, musicians?: [{plays?, energy?: 0-4}], feel?: [5 × -1..]}",
            always,
            def
        ),
        cmd!("band.def.delete", "Delete Section Definition", [], None, "{id: A-H}", always, def_delete),
        cmd!("band.section.add", "Add Section", [], None, "{section: A-H, start, end}", always, section_add),
        cmd!("band.section.set", "Edit Section", [], None, "{block, start?, end?, section?: A-H, energy?: -3-3, take?: 0-16}", always, section_set),
        cmd!("band.section.copy", "Copy Section", [], None, "{block, to}", always, section_copy),
        cmd!("band.section.remove", "Remove Section", [], None, "{block}", always, section_remove),
        cmd!("band.chord.add", "Add Chord", [], None, "{at, end?, chord: \"Am7\"|slot 0-111, bass?: -1-11, snap?: true}", always, chord_add),
        cmd!("band.chord.set", "Edit Chord", [], None, "{chord, at?, end?, chord_text?|slot?: 0-111, bass?: -1-11, snap?: true}", always, chord_set),
        cmd!("band.chord.remove", "Remove Chord", [], None, "{chord}", always, chord_remove),
        cmd!("band.chords.delete", "Delete Chords", [], None, "{chords?: [id], range?: {start, end}, all?: bool} → {deleted}", always, chords_delete),
        cmd!("band.song.factory", "Factory Song", [], None, "{name, at?: 0, replace?: true}", always, song_factory),
        cmd!("band.chords.import", "Import Chords", [], None, "{path, at?: 0, key?: 0-11|\"C\"..\"B\", scale?: major|minor}", always, chords_import),
        cmd!(
            "band.print",
            "Print Band",
            [],
            None,
            "{range?: {start, end} | block?: id | song?: true, slots?: [1-6] (default: every slot a track plays)}",
            always,
            print
        ),
        cmd!(noundo "band.library.reload", "Reload Band Library", [], None, "{}", always, |e, _| Ok(json!({"errors": e.reload_band_library()}))),
        cmd!(query "band.list", "Band", [], None, "{}", always, list),
    ]
}

// ---- parameters ---------------------------------------------------------------------------------

const KEY_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const KEY_FLATS: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];
const SPEEDS: [&str; 3] = ["half", "normal", "double"];
const VOICES: [&str; 5] = ["auto", "top", "2nd", "3rd", "bass"];
const SWAM: [&str; 13] =
    ["Violin", "Viola", "Cello", "Double Bass", "Trumpet", "Trombone", "Horn", "Tuba", "Flute", "Clarinet", "Oboe", "Bassoon", "Alto Sax"];
const SHAPES: [&str; 3] = ["none", "build", "drop"];
const ROLES: [&str; 3] = ["none", "intro", "ending"];
const SCALES: [&str; 2] = ["major", "minor"];
/// MS `newSection`'s names, by how many sections there are.
const NEW_SECTION_NAMES: [&str; 8] = ["Verse", "Chorus", "Bridge", "Intro", "Outro", "Pre-chorus", "Solo", "Tag"];
/// Largest MIDI file read for chord import.
#[cfg(not(target_arch = "wasm32"))]
const MAX_MIDI_BYTES: u64 = 16 * 1024 * 1024;
/// Largest feel presets file read.
const MAX_TEXT_BYTES: u64 = 4 * 1024 * 1024;

/// A whole number, or `None` for anything else (fractions, NaN-free floats included).
fn whole(v: &Value) -> Option<i64> {
    v.as_i64().or_else(|| v.as_f64().filter(|f| f.is_finite() && f.fract() == 0.0 && f.abs() < 1e15).map(|f| f as i64))
}

fn range_text(lo: i64, hi: i64) -> String {
    format!("{lo}-{hi}")
}

/// `p[key]` as a whole number in `lo..=hi`; absent = `None`.
fn int_in(cmd: &str, p: &Value, key: &str, lo: i64, hi: i64) -> Result<Option<i64>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    match whole(v) {
        Some(n) if (lo..=hi).contains(&n) => Ok(Some(n)),
        _ => Err(bad(cmd, format!("`{key}` must be {} (got {v})", range_text(lo, hi)))),
    }
}

fn bool_in(cmd: &str, p: &Value, key: &str) -> Result<Option<bool>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    match (v.as_bool(), whole(v)) {
        (Some(b), _) => Ok(Some(b)),
        (_, Some(0)) => Ok(Some(false)),
        (_, Some(1)) => Ok(Some(true)),
        _ => Err(bad(cmd, format!("`{key}` must be true or false (got {v})"))),
    }
}

/// A choice by name (any case) or index.
fn choice_in(cmd: &str, p: &Value, key: &str, names: &[&str]) -> Result<Option<i64>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    let n = names.len() as i64;
    let by_name = v.as_str().and_then(|s| names.iter().position(|c| c.eq_ignore_ascii_case(s.trim())));
    match (by_name, whole(v)) {
        (Some(i), _) => Ok(Some(i as i64)),
        (_, Some(i)) if (0..n).contains(&i) => Ok(Some(i)),
        _ => Err(bad(cmd, format!("`{key}` must be {} or {} (got {v})", names.join("|"), range_text(0, n - 1)))),
    }
}

fn key_in(cmd: &str, p: &Value, key: &str) -> Result<Option<i64>> {
    let Some(v) = p.get(key) else { return Ok(None) };
    if let Some(s) = v.as_str() {
        let s = s.trim();
        let i = KEY_NAMES.iter().chain(KEY_FLATS.iter()).position(|k| k.eq_ignore_ascii_case(s));
        return i.map(|i| Some((i % 12) as i64)).ok_or_else(|| bad(cmd, format!("`{key}` must be 0-11 or C..B (got `{s}`)")));
    }
    int_in(cmd, p, key, 0, 11)
}

/// A section id A..H (either case).
fn section_id(cmd: &str, v: Option<&Value>, key: &str) -> Result<char> {
    let s = v.and_then(Value::as_str).map(str::trim).unwrap_or_default();
    let mut chars = s.chars();
    match (chars.next().map(|c| c.to_ascii_uppercase()), chars.next()) {
        (Some(c @ 'A'..='H'), None) => Ok(c),
        _ => Err(bad(cmd, format!("`{key}` must be a section A-H (got `{s}`)"))),
    }
}

fn id_in(cmd: &str, p: &Value, key: &str) -> Result<u32> {
    p.get(key).and_then(Value::as_u64).and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(cmd, format!("`{key}` id required")))
}

fn name_in<'a>(cmd: &str, p: &'a Value) -> Result<&'a str> {
    str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(cmd, "`name` required"))
}

fn as_u8(v: i64) -> u8 {
    v.clamp(0, 255) as u8
}

fn as_i8(v: i64) -> i8 {
    v.clamp(-128, 127) as i8
}

// ---- the song grid ------------------------------------------------------------------------------

/// The nearest half bar to a sample position, in ticks (never before 0).
fn snap(s: &Session, at: Samples) -> i64 {
    let half = half_ticks(s);
    let t = s.tempo.samples_to_ticks(at.max(0), s.sample_rate).max(0);
    t.saturating_add(half / 2).div_euclid(half).saturating_mul(half)
}

/// `[start, end)` in ticks from two positions: refused unless `end` comes after `start`; at least
/// one half bar long.
fn span(s: &Session, cmd: &str, start: Samples, end: Samples) -> Result<(i64, i64)> {
    if end <= start {
        return Err(bad(cmd, "`end` must be after `start`"));
    }
    let (a, b) = (snap(s, start), snap(s, end));
    Ok((a, if b > a { b } else { a.saturating_add(half_ticks(s)) }))
}

/// `snap: false` (a chord placed by hand off the grid): positions keep their exact tick.
fn exact(cmd: &str, p: &Value) -> Result<bool> {
    match p.get("snap") {
        None => Ok(false),
        Some(v) => v.as_bool().map(|on| !on).ok_or_else(|| bad(cmd, "`snap` must be true or false")),
    }
}

/// A position in ticks: the nearest half bar, or (`exact`) its own tick.
fn tick_at(s: &Session, at: Samples, exact: bool) -> i64 {
    // Rounded, so a tick sent as samples comes back as the same tick at any tempo.
    if exact { (s.tempo.samples_to_ticks_f(at.max(0), s.sample_rate).round() as i64).max(0) } else { snap(s, at) }
}

/// [`span`], or (`exact`) the ticks as they are, at least an eighth of a bar long.
fn span_at(s: &Session, cmd: &str, start: Samples, end: Samples, exact: bool) -> Result<(i64, i64)> {
    if !exact {
        return span(s, cmd, start, end);
    }
    if end <= start {
        return Err(bad(cmd, "`end` must be after `start`"));
    }
    let (a, b) = (tick_at(s, start, true), tick_at(s, end, true));
    Ok((a, b.max(a.saturating_add((half_ticks(s) / 4).max(1)))))
}

fn position(e: &Engine, cmd: &str, p: &Value, key: &str) -> Result<Samples> {
    position_param(e, cmd, p, key)?.ok_or_else(|| bad(cmd, format!("`{key}` required")))
}

/// A lane item's id and span, and how to set them.
pub(crate) trait LaneItem: Clone {
    fn span(&self) -> (u32, i64, i64);
    fn set(&mut self, id: u32, start: i64, len: i64);
}

impl LaneItem for SectionBlock {
    fn span(&self) -> (u32, i64, i64) {
        (self.id, self.start_tick, self.length_ticks)
    }
    fn set(&mut self, id: u32, start: i64, len: i64) {
        (self.id, self.start_tick, self.length_ticks) = (id, start, len);
    }
}

impl LaneItem for ChordEvent {
    fn span(&self) -> (u32, i64, i64) {
        (self.id, self.tick, self.length_ticks)
    }
    fn set(&mut self, id: u32, start: i64, len: i64) {
        (self.id, self.tick, self.length_ticks) = (id, start, len);
    }
}

pub(crate) fn take_id(next: &mut u32) -> u32 {
    let id = (*next).max(1);
    *next = id.saturating_add(1);
    id
}

/// Cuts `[start, end)` out of a lane, leaving `keep` alone: items inside go, items across an edge
/// are trimmed, and an item over the whole span is split (its tail takes a new id).
pub(crate) fn carve<T: LaneItem>(items: &mut Vec<T>, start: i64, end: i64, keep: Option<u32>, next_id: &mut u32) {
    let mut out = Vec::with_capacity(items.len().saturating_add(1));
    for it in items.drain(..) {
        let (id, a, len) = it.span();
        let b = a.saturating_add(len);
        if Some(id) == keep || b <= start || a >= end {
            out.push(it);
            continue;
        }
        if a < start {
            let mut head = it.clone();
            head.set(id, a, start - a);
            out.push(head);
        }
        if b > end {
            let tail_id = if a < start { take_id(next_id) } else { id };
            let mut tail = it;
            tail.set(tail_id, end, b - end);
            out.push(tail);
        }
    }
    *items = out;
}

/// Lane content shows the Sections and Chords rulers (a new session hides them).
pub(crate) fn show_lane_rulers(e: &mut Engine) {
    let rulers = &mut e.session_mut().edit.rulers;
    for id in ["sections", "band_chords"] {
        if !rulers.iter().any(|r| r == id) {
            super::view_more::set_ruler(rulers, id, true);
        }
    }
}

/// Edits a copy of the band, sanitized and put in place when it changed.
fn edit_band(e: &mut Engine, f: impl FnOnce(&Session, &mut BandState) -> Result<Value>) -> Result<Value> {
    let mut st = e.session().band.clone();
    let v = f(e.session(), &mut st)?;
    st.sanitize();
    if st != e.session().band {
        e.session_mut().band = st;
    }
    Ok(v)
}

// ---- musicians ----------------------------------------------------------------------------------

fn instrument_name(i: u8) -> &'static str {
    INSTRUMENT_NAMES.get(usize::from(i)).copied().unwrap_or("")
}

/// The instrument's first player in the Library's order (MS `firstPlayerSeed`).
fn first_player(lib: &LibraryData, inst: u8) -> Option<(String, u16)> {
    lib.players.get(usize::from(inst))?.iter().find_map(|&s| Some((lib.persona(s)?.name.clone(), u16::try_from(s).ok()?)))
}

fn become_first(lib: &LibraryData, m: &mut MusicianSlot) {
    let (name, seed) = first_player(lib, m.instrument).unwrap_or_default();
    (m.player, m.player_seed) = (name, seed);
}

/// MS `handleAsyncUpdate` after an instrument change: the player stays if they play the new
/// instrument, else the instrument's first player takes over.
fn repick_player(lib: &LibraryData, m: &mut MusicianSlot) {
    let seed = lib.seed_for_name(&m.player);
    match u16::try_from(seed) {
        Ok(s) if seed > 0 && lib.plays(i32::from(m.instrument), seed) => m.player_seed = s,
        _ => become_first(lib, m),
    }
}

/// A player picked by name: "Session" (or nothing) is the instrument's first player.
fn set_player(cmd: &str, lib: &LibraryData, m: &mut MusicianSlot, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("Session") {
        become_first(lib, m);
        return Ok(());
    }
    let seed = lib.seed_for_name(name);
    let Ok(s) = u16::try_from(seed).map_err(|_| ()).and_then(|s| if s > 0 { Ok(s) } else { Err(()) }) else {
        return Err(bad(cmd, format!("unknown player `{name}`")));
    };
    if !lib.plays(i32::from(m.instrument), seed) {
        return Err(bad(cmd, format!("`{name}` doesn't play {}", instrument_name(m.instrument))));
    }
    (m.player, m.player_seed) = (name.to_string(), s);
    Ok(())
}

/// A target picked by name ("auto" lets the band pick): its keyswitch octave comes with it (MS
/// `setTargetByName` → `applyTargetKsDefault`).
fn set_target(cmd: &str, lib: &LibraryData, m: &mut MusicianSlot, name: &str) -> Result<()> {
    let name = name.trim();
    if name.is_empty() || name.eq_ignore_ascii_case("auto") {
        m.target = "auto".into();
        m.ksoctave = None;
        return Ok(());
    }
    let Some(t) = lib.target(lib.target_slot_for_name(name)) else {
        return Err(bad(cmd, format!("unknown target `{name}`")));
    };
    m.target = name.to_string();
    m.ksoctave = Some(t.ks_octave_default.clamp(-2, 2) as i8);
    Ok(())
}

fn slot(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.slot";
    let k = p.get("slot").and_then(whole).filter(|k| (1..=6).contains(k)).ok_or_else(|| bad(C, "`slot` must be 1-6"))?;
    let k = usize::try_from(k - 1).unwrap_or_default();
    let lib = e.band_library();
    edit_band(e, |_, st| {
        let Some(m) = st.slots.get_mut(k) else { return Err(bad(C, "`slot` must be 1-6")) };
        if let Some(v) = bool_in(C, p, "enabled")? {
            m.enabled = v;
        }
        if let Some(v) = bool_in(C, p, "mute")? {
            m.mute = v;
        }
        if let Some(i) = choice_in(C, p, "instrument", &INSTRUMENT_NAMES)? {
            m.instrument = as_u8(i);
            if p.get("player").is_none() {
                repick_player(&lib, m);
            }
        }
        if let Some(v) = p.get("player") {
            let name = v.as_str().ok_or_else(|| bad(C, "`player` must be a player's name or \"Session\""))?;
            set_player(C, &lib, m, name)?;
        }
        if let Some(v) = p.get("target") {
            let name = v.as_str().ok_or_else(|| bad(C, "`target` must be a target's name or \"auto\""))?;
            set_target(C, &lib, m, name)?;
        }
        let ranges: [(&str, i64, i64); 10] = [
            ("energy", 1, 4),
            ("swing", 0, 100),
            ("humanize", 0, 100),
            ("dynamics", 0, 100),
            ("fills", 0, 100),
            ("length", 10, 100),
            ("velocity", 1, 127),
            ("ample", 0, 2),
            ("groove", 0, 100),
            ("amplechords", 0, 1),
        ];
        for (key, lo, hi) in ranges {
            if let Some(v) = int_in(C, p, key, lo, hi)? {
                let f = match key {
                    "energy" => &mut m.energy,
                    "swing" => &mut m.swing,
                    "humanize" => &mut m.humanize,
                    "dynamics" => &mut m.dynamics,
                    "fills" => &mut m.fills,
                    "length" => &mut m.length,
                    "velocity" => &mut m.velocity,
                    "ample" => &mut m.ample,
                    "groove" => &mut m.groove,
                    _ => &mut m.amplechords,
                };
                *f = as_u8(v);
            }
        }
        if let Some(v) = int_in(C, p, "pluckchords", 0, 1)? {
            m.pluckchords = as_u8(v);
        }
        if let Some(v) = bool_in(C, p, "respect")? {
            m.respect = v;
        }
        if let Some(v) = int_in(C, p, "octave", -2, 2)? {
            m.octave = as_i8(v);
        }
        if let Some(v) = choice_in(C, p, "speed", &SPEEDS)? {
            m.speed = as_u8(v);
        }
        if let Some(v) = choice_in(C, p, "voice", &VOICES)? {
            m.voice = as_u8(v);
        }
        if let Some(v) = choice_in(C, p, "swaminst", &SWAM)? {
            m.swaminst = as_u8(v);
        }
        match p.get("ksoctave") {
            Some(Value::Null) => m.ksoctave = None,
            Some(_) => m.ksoctave = int_in(C, p, "ksoctave", -2, 2)?.map(as_i8),
            None => {}
        }
        if let Some(v) = p.get("art") {
            let arr = v.as_array().filter(|a| a.len() == 4).ok_or_else(|| bad(C, "`art` must be four keys 0-15 (long, short, pluck, tremolo)"))?;
            for (a, x) in m.art.iter_mut().zip(arr) {
                *a = whole(x).filter(|n| (0..=15).contains(n)).map(as_u8).ok_or_else(|| bad(C, format!("`art` keys must be 0-15 (got {x})")))?;
            }
        }
        let m = m.clone();
        Ok(json!({"slot": k + 1, "musician": m}))
    })
}

// ---- settings and feel presets -----------------------------------------------------------------

fn feel_file(dir: &Path) -> PathBuf {
    dir.join("FeelPresets.json")
}

/// The user's feel presets text: `Ok(None)` when there is no file, `Err` when it can't be read.
fn read_feel_file(dir: &Path) -> std::result::Result<Option<String>, String> {
    let f = feel_file(dir);
    if !crate::io::file_found(&f) {
        return Ok(None);
    }
    let bytes = crate::io::read_file_max(&f, MAX_TEXT_BYTES).map_err(|err| format!("FeelPresets.json can't be read: {err}"))?;
    String::from_utf8(bytes).map(Some).map_err(|_| "FeelPresets.json can't be read: it isn't UTF-8 text".to_string())
}

fn user_feel_presets(dir: Option<&Path>) -> Vec<UserFeelPreset> {
    dir.and_then(|d| read_feel_file(d).ok().flatten()).map(|t| load_feel_presets(&t)).unwrap_or_default()
}

fn set_dials(st: &mut BandState, dials: [i8; 5]) {
    let f = &mut st.song_feel;
    (f.time, f.density, f.pocket, f.groove, f.touch) = (dials[0], dials[1], dials[2], dials[3], dials[4]);
}

fn settings(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.settings";
    // The user's feel presets are read only when a preset is named.
    let user = if p.get("feel_preset").is_some() { user_feel_presets(e.band_user_folder()) } else { Vec::new() };
    edit_band(e, |_, st| {
        if let Some(v) = key_in(C, p, "key")? {
            st.key = as_u8(v);
        }
        if let Some(v) = choice_in(C, p, "scale", &SCALES)? {
            st.scale = as_u8(v);
        }
        if let Some(v) = int_in(C, p, "take", 1, 16)? {
            st.take = as_u8(v);
        }
        if let Some(f) = p.get("feel") {
            let o = f.as_object().ok_or_else(|| bad(C, "`feel` must be an object"))?;
            for k in o.keys() {
                let slot = match k.as_str() {
                    "natural" => &mut st.feel.natural,
                    "tension" => &mut st.feel.tension,
                    "bass_lock" => &mut st.feel.bass_lock,
                    "fills_yield" => &mut st.feel.fills_yield,
                    "leave_space" => &mut st.feel.leave_space,
                    "dynamics" => &mut st.feel.dynamics,
                    _ => return Err(bad(C, format!("`feel` has no switch `{k}`"))),
                };
                *slot = bool_in(C, f, k)?.unwrap_or(*slot);
            }
        }
        if let Some(name) = p.get("feel_preset") {
            let name = name.as_str().map(str::trim).unwrap_or_default();
            let found = factory_feel_presets()
                .iter()
                .find(|f| f.name.eq_ignore_ascii_case(name))
                .map(|f| [f.time, f.density, f.pocket, f.groove, f.touch])
                .or_else(|| {
                    user.iter().find(|u| u.name.eq_ignore_ascii_case(name)).map(|u| {
                        let d = &u.dials;
                        [d.time, d.density, d.pocket, d.groove, d.touch]
                    })
                });
            let dials = found.ok_or_else(|| bad(C, format!("unknown feel preset `{name}`")))?;
            set_dials(st, dials);
            st.song_feel.on = true;
        }
        if let Some(f) = p.get("song_feel") {
            let o = f.as_object().ok_or_else(|| bad(C, "`song_feel` must be an object"))?;
            for k in o.keys() {
                if k == "on" {
                    st.song_feel.on = bool_in(C, f, "on")?.unwrap_or(st.song_feel.on);
                    continue;
                }
                let d = (0..FEEL_DIAL_COUNT)
                    .find(|d| dial_title(*d).eq_ignore_ascii_case(k))
                    .ok_or_else(|| bad(C, format!("`song_feel` has no dial `{k}`")))?;
                let names: Vec<&str> = (0..dial_positions(d)).map(|i| dial_name(d, i)).collect();
                let v = choice_in(C, f, k, &names)?.unwrap_or_default();
                let sf = &mut st.song_feel;
                let mut dials = [sf.time, sf.density, sf.pocket, sf.groove, sf.touch];
                if let Some(x) = dials.get_mut(d as usize) {
                    *x = as_i8(v);
                }
                set_dials(st, dials);
            }
        }
        if let Some(v) = p.get("feel_ignore") {
            let arr = v.as_array().ok_or_else(|| bad(C, "`feel_ignore` must be a list of musicians 1-6"))?;
            let mut mask = 0u8;
            for x in arr {
                let k = whole(x).filter(|k| (1..=6).contains(k)).ok_or_else(|| bad(C, format!("`feel_ignore`: musicians are 1-6 (got {x})")))?;
                mask |= 1 << (k - 1);
            }
            st.feel_ignore = mask;
        }
        Ok(Value::Null)
    })
    .map(|_| json!({"band": serde_json::to_value(&e.session().band).unwrap_or(Value::Null)}))
}

fn band_folder(e: &Engine, cmd: &str) -> Result<PathBuf> {
    e.band_user_folder().map(Path::to_path_buf).ok_or_else(|| bad(cmd, "there is no user Band folder (none is set, and the web build has none)"))
}

fn feel_preset_save(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.feel_preset.save";
    let name = name_in(C, p)?.to_string();
    let dir = band_folder(e, C)?;
    let text = read_feel_file(&dir).map_err(|m| bad(C, m))?;
    if !feel_presets_writable(text.as_deref()) {
        return Err(bad(C, "FeelPresets.json isn't a feel presets file: it is left as it is"));
    }
    let mut all = text.map(|t| load_feel_presets(&t)).unwrap_or_default();
    let f = &e.session().band.song_feel;
    let dials = soundcraft_band::song_feel::SongFeel {
        on: true,
        time: f.time,
        density: f.density,
        pocket: f.pocket,
        groove: f.groove,
        touch: f.touch,
        ..Default::default()
    };
    save_feel_preset(&mut all, &name, &dials);
    write_text(&feel_file(&dir), &feel_presets_to_json(&all)).map_err(|m| bad(C, m))?;
    Ok(json!({"name": name}))
}

fn feel_preset_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.feel_preset.delete";
    let name = name_in(C, p)?.to_string();
    let dir = band_folder(e, C)?;
    let text = read_feel_file(&dir).map_err(|m| bad(C, m))?;
    if !feel_presets_writable(text.as_deref()) {
        return Err(bad(C, "FeelPresets.json isn't a feel presets file: it is left as it is"));
    }
    let mut all = text.map(|t| load_feel_presets(&t)).unwrap_or_default();
    if !delete_feel_preset(&mut all, &name) {
        return Err(bad(C, format!("no feel preset `{name}`")));
    }
    write_text(&feel_file(&dir), &feel_presets_to_json(&all)).map_err(|m| bad(C, m))?;
    Ok(json!({"name": name}))
}

/// Writes beside, then moves into place.
fn write_text(dest: &Path, text: &str) -> std::result::Result<(), String> {
    if let Some(parent) = dest.parent() {
        crate::io::make_dir(parent).map_err(|err| err.to_string())?;
    }
    crate::io::replace_file(&dest.with_extension("json.tmp"), dest, text.as_bytes()).map_err(|err| err.to_string())
}

// ---- band presets -------------------------------------------------------------------------------

/// Every band preset (factory, then the user's), and the user files skipped with why.
fn all_presets(e: &Engine) -> (Vec<BandPreset>, Vec<String>) {
    let mut all = factory_band_presets();
    let (user, skipped) = e.band_user_folder().map(|d| user_band_presets(&d.join("Bands"))).unwrap_or_default();
    all.extend(user);
    (all, skipped)
}

/// A preset's per-musician value into a slot field: rounded and clamped (preset files are lenient).
fn apply_param(m: &mut MusicianSlot, key: &str, v: f32) {
    let v = if v.is_finite() { i64::from(v.round() as i32) } else { return };
    let u = |lo: i64, hi: i64| as_u8(v.clamp(lo, hi));
    match key {
        "energy" => m.energy = u(1, 4),
        "swing" => m.swing = u(0, 100),
        "humanize" => m.humanize = u(0, 100),
        "dynamics" => m.dynamics = u(0, 100),
        "fills" => m.fills = u(0, 100),
        "respect" => m.respect = v != 0,
        "octave" => m.octave = as_i8(v.clamp(-2, 2)),
        "length" => m.length = u(10, 100),
        "velocity" => m.velocity = u(1, 127),
        "speed" => m.speed = u(0, 2),
        "ksoctave" => m.ksoctave = Some(as_i8(v.clamp(-2, 2))),
        "swaminst" => m.swaminst = u(0, 12),
        "voice" => m.voice = u(0, 4),
        "art_long" => m.art[0] = u(0, 15),
        "art_short" => m.art[1] = u(0, 15),
        "art_pluck" => m.art[2] = u(0, 15),
        "art_trem" => m.art[3] = u(0, 15),
        "ample" => m.ample = u(0, 2),
        "groove" => m.groove = u(0, 100),
        "amplechords" => m.amplechords = u(0, 1),
        "pluckchords" => m.pluckchords = u(0, 1),
        _ => {}
    }
}

/// A musician of a band preset into a slot, as MS restores one: a player the Library doesn't have is
/// kept by name (MS's "wanted" player) while the instrument's first player plays; one who doesn't
/// play the instrument gives way to the first player.
fn apply_musician(lib: &LibraryData, m: &mut MusicianSlot, pm: &MusicianPreset) {
    m.enabled = pm.enabled;
    m.mute = pm.mute;
    if let Some(i) = INSTRUMENT_NAMES.iter().position(|n| n.eq_ignore_ascii_case(&pm.instrument)) {
        m.instrument = as_u8(i as i64);
    }
    let name = pm.player.trim();
    if set_player("", lib, m, name).is_err() {
        become_first(lib, m);
        if lib.seed_for_name(name) <= 0 {
            m.player = name.to_string();
        }
    }
    if set_target("", lib, m, &pm.target).is_err() {
        m.target = pm.target.trim().to_string();
        m.ksoctave = None;
    }
    for (k, v) in &pm.params {
        apply_param(m, k, *v);
    }
}

fn preset_load(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.preset.load";
    let name = name_in(C, p)?.to_string();
    let (all, _) = all_presets(e);
    let preset = all
        .iter()
        .find(|b| b.name == name)
        .or_else(|| all.iter().find(|b| b.name.eq_ignore_ascii_case(&name)))
        .cloned()
        .ok_or_else(|| bad(C, format!("no band preset `{name}`")))?;
    let lib = e.band_library();
    edit_band(e, |_, st| {
        for (m, pm) in st.slots.iter_mut().zip(&preset.musicians) {
            apply_musician(&lib, m, pm);
        }
        st.band_name = preset.name.clone();
        Ok(json!({"band": preset.name}))
    })
}

fn slot_preset(m: &MusicianSlot, k: usize) -> MusicianPreset {
    let mut params = std::collections::BTreeMap::new();
    let mut put = |key: &str, v: i64| {
        params.insert(key.to_string(), v as f32);
    };
    for (key, v) in [
        ("energy", m.energy),
        ("swing", m.swing),
        ("humanize", m.humanize),
        ("dynamics", m.dynamics),
        ("fills", m.fills),
        ("length", m.length),
        ("velocity", m.velocity),
        ("speed", m.speed),
        ("swaminst", m.swaminst),
        ("voice", m.voice),
        ("art_long", m.art[0]),
        ("art_short", m.art[1]),
        ("art_pluck", m.art[2]),
        ("art_trem", m.art[3]),
        ("ample", m.ample),
        ("groove", m.groove),
        ("amplechords", m.amplechords),
        ("pluckchords", m.pluckchords),
    ] {
        put(key, i64::from(v));
    }
    put("respect", i64::from(m.respect));
    put("octave", i64::from(m.octave));
    if let Some(ks) = m.ksoctave {
        put("ksoctave", i64::from(ks));
    }
    MusicianPreset {
        enabled: m.enabled,
        mute: m.mute,
        channel: as_u8(k as i64 + 1),
        instrument: instrument_name(m.instrument).to_string(),
        player: if m.player.is_empty() { "Session".into() } else { m.player.clone() },
        target: if m.target == "auto" { String::new() } else { m.target.clone() },
        params,
    }
}

#[cfg(not(target_arch = "wasm32"))]
fn save_preset(dir: &Path, p: &BandPreset, replace: bool) -> std::result::Result<(), String> {
    soundcraft_band::band_preset::save_band_preset(dir, p, replace)
}

#[cfg(target_arch = "wasm32")]
fn save_preset(_: &Path, _: &BandPreset, _: bool) -> std::result::Result<(), String> {
    Err("the web build has no user Band folder".into())
}

fn preset_save(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.preset.save";
    let name = name_in(C, p)?.to_string();
    let replace = bool_in(C, p, "replace")?.unwrap_or(false);
    let dir = band_folder(e, C)?.join("Bands");
    let b = &e.session().band;
    let preset = BandPreset {
        name: name.clone(),
        factory: false,
        tags: Vec::new(),
        musicians: std::array::from_fn(|k| b.slots.get(k).map(|m| slot_preset(m, k)).unwrap_or_default()),
    };
    if factory_band_presets().iter().any(|f| f.name.eq_ignore_ascii_case(&name)) {
        return Err(bad(C, format!("\"{name}\" is a factory band: save under another name.")));
    }
    save_preset(&dir, &preset, replace).map_err(|m| bad(C, m))?;
    Ok(json!({"name": name}))
}

fn preset_list(e: &mut Engine, _: &Value) -> Result<Value> {
    let (all, skipped) = all_presets(e);
    let presets: Vec<Value> = all.iter().map(|b| json!({"name": b.name, "factory": b.factory, "tags": b.tags})).collect();
    Ok(json!({"presets": presets, "errors": skipped}))
}

// ---- section definitions ------------------------------------------------------------------------

fn def(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.def";
    let id = section_id(C, p.get("id"), "id")?;
    edit_band(e, |_, st| {
        if st.def(id).is_none() {
            let name = NEW_SECTION_NAMES.get(st.defs.len().min(7)).copied().unwrap_or("Verse");
            st.defs.push(SectionDef { id, name: name.into(), ..SectionDef::default() });
        }
        let Some(d) = st.defs.iter_mut().find(|d| d.id == id) else { return Err(bad(C, "no such section")) };
        if let Some(v) = p.get("name") {
            let n = v.as_str().map(str::trim).filter(|n| !n.is_empty()).ok_or_else(|| bad(C, "`name` must be a non-empty text"))?;
            d.name = n.to_string();
        }
        if let Some(v) = int_in(C, p, "energy_shift", -3, 3)? {
            d.energy_shift = as_i8(v);
        }
        if let Some(v) = choice_in(C, p, "shape", &SHAPES)? {
            d.shape = as_u8(v);
        }
        if let Some(v) = choice_in(C, p, "role", &ROLES)? {
            d.role = as_u8(v);
        }
        if let Some(v) = int_in(C, p, "take", 0, 16)? {
            d.take = as_u8(v);
        }
        if let Some(v) = bool_in(C, p, "follow_idea")? {
            d.follow_idea = v;
        }
        if let Some(v) = p.get("musicians") {
            let arr = v.as_array().filter(|a| a.len() <= 6).ok_or_else(|| bad(C, "`musicians` must be a list of at most six {plays?, energy?}"))?;
            for (m, x) in d.musicians.iter_mut().zip(arr) {
                if !x.is_object() {
                    return Err(bad(C, "`musicians` entries must be {plays?, energy?: 0-4}"));
                }
                if let Some(b) = bool_in(C, x, "plays")? {
                    m.plays = b;
                }
                if let Some(n) = int_in(C, x, "energy", 0, 4)? {
                    m.energy = as_u8(n);
                }
            }
        }
        if let Some(v) = p.get("feel") {
            let arr = v
                .as_array()
                .filter(|a| a.len() == 5)
                .ok_or_else(|| bad(C, "`feel` must be five dials (time, density, pocket, groove, touch), -1 = the song's"))?;
            // Time is the song's alone: MS sections override density, pocket, groove and touch.
            if arr.first().and_then(whole) != Some(-1) {
                return Err(bad(C, "`feel`: time can't be set per section (the song's Time rules); give -1 first"));
            }
            for (i, (f, x)) in d.feel.iter_mut().zip(arr).enumerate() {
                let max = i64::from(dial_positions(i as i32) - 1);
                *f = whole(x)
                    .filter(|n| (-1..=max).contains(n))
                    .map(as_i8)
                    .ok_or_else(|| bad(C, format!("`feel` dial {} must be -1-{max} (got {x})", dial_title(i as i32))))?;
            }
        }
        let d = d.clone();
        Ok(json!({"def": d}))
    })
}

fn def_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.def.delete";
    let id = section_id(C, p.get("id"), "id")?;
    edit_band(e, |_, st| {
        if st.def(id).is_none() {
            return Err(bad(C, format!("no section {id}")));
        }
        let used = st.sections.iter().filter(|b| b.section == id).count();
        if used > 0 {
            return Err(bad(C, format!("section {id} is used by {used} block(s) on the song lane: remove them first")));
        }
        if st.defs.len() <= 1 {
            return Err(bad(C, "the band needs at least one section definition"));
        }
        st.defs.retain(|d| d.id != id);
        Ok(json!({"id": id.to_string()}))
    })
}

// ---- section blocks -----------------------------------------------------------------------------

fn need_def(cmd: &str, st: &BandState, id: char) -> Result<()> {
    if st.def(id).is_some() { Ok(()) } else { Err(bad(cmd, format!("no section definition {id}: make it with band.def"))) }
}

fn lay_block(cmd: &str, st: &mut BandState, mut b: SectionBlock) -> Result<u32> {
    // Cut first (a split adds a block), then count.
    carve(&mut st.sections, b.start_tick, b.start_tick.saturating_add(b.length_ticks), None, &mut st.next_id);
    if st.sections.len() >= MAX_BLOCKS {
        return Err(bad(cmd, format!("the song lane is full ({MAX_BLOCKS} blocks)")));
    }
    b.id = take_id(&mut st.next_id);
    let id = b.id;
    st.sections.push(b);
    Ok(id)
}

fn section_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.section.add";
    let section = section_id(C, p.get("section"), "section")?;
    let (start, end) = (position(e, C, p, "start")?, position(e, C, p, "end")?);
    let (a, b) = span(e.session(), C, start, end)?;
    edit_band(e, |_, st| {
        need_def(C, st, section)?;
        let id = lay_block(C, st, SectionBlock { id: 0, start_tick: a, length_ticks: b - a, section, energy: 0, take: 0 })?;
        Ok(json!({"block": id}))
    })
    .inspect(|_| show_lane_rulers(e))
}

fn find_block(cmd: &str, st: &BandState, id: u32) -> Result<SectionBlock> {
    st.sections.iter().find(|b| b.id == id).cloned().ok_or_else(|| bad(cmd, format!("no section block {id}")))
}

fn section_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.section.set";
    let id = id_in(C, p, "block")?;
    let old = find_block(C, &e.session().band, id)?;
    let start = position_param(e, C, p, "start")?;
    let end = position_param(e, C, p, "end")?;
    let s = e.session();
    let (a, b) = match (start, end) {
        (None, None) => (old.start_tick, old.start_tick.saturating_add(old.length_ticks)),
        (Some(st), None) => {
            let a = snap(s, st);
            (a, a.saturating_add(old.length_ticks))
        }
        (st, en) => {
            let st = st.unwrap_or_else(|| s.tempo.tick_to_samples(old.start_tick, s.sample_rate));
            span(s, C, st, en.unwrap_or_default())?
        }
    };
    let section = match p.get("section") {
        Some(v) => Some(section_id(C, Some(v), "section")?),
        None => None,
    };
    edit_band(e, |_, st| {
        if let Some(sec) = section {
            need_def(C, st, sec)?;
        }
        carve(&mut st.sections, a, b, Some(id), &mut st.next_id);
        let Some(blk) = st.sections.iter_mut().find(|x| x.id == id) else { return Err(bad(C, format!("no section block {id}"))) };
        (blk.start_tick, blk.length_ticks) = (a, b - a);
        if let Some(sec) = section {
            blk.section = sec;
        }
        if let Some(v) = int_in(C, p, "energy", -3, 3)? {
            blk.energy = as_i8(v);
        }
        if let Some(v) = int_in(C, p, "take", 0, 16)? {
            blk.take = as_u8(v);
        }
        Ok(json!({"block": id}))
    })
}

fn section_copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.section.copy";
    let id = id_in(C, p, "block")?;
    let old = find_block(C, &e.session().band, id)?;
    let to = snap(e.session(), position(e, C, p, "to")?);
    edit_band(e, |_, st| {
        let id = lay_block(C, st, SectionBlock { start_tick: to, ..old })?;
        Ok(json!({"block": id}))
    })
}

fn section_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.section.remove";
    let id = id_in(C, p, "block")?;
    edit_band(e, |_, st| {
        find_block(C, st, id)?;
        st.sections.retain(|b| b.id != id);
        Ok(json!({"block": id}))
    })
}

// ---- chords -------------------------------------------------------------------------------------

/// A chord from text (in the band's key) or a slot: `(slot, bass, exact)`.
fn chord_value(cmd: &str, st: &BandState, key: &str, v: &Value) -> Result<(u8, i8, bool)> {
    if let Some(text) = v.as_str() {
        let pcs = parse_chord_name(text).ok_or_else(|| bad(cmd, format!("can't read the chord `{text}`")))?;
        let m = match_chord(&pcs, i32::from(st.key), i32::from(st.scale));
        return Ok((as_u8(i64::from(m.slot.clamp(0, 111))), as_i8(i64::from(m.bass.clamp(-1, 11))), m.exact));
    }
    match whole(v) {
        Some(n) if (0..=111).contains(&n) => Ok((as_u8(n), -1, true)),
        _ => Err(bad(cmd, format!("`{key}` must be a chord name (\"Am7\") or a slot 0-111 (got {v})"))),
    }
}

fn lay_chord(cmd: &str, st: &mut BandState, mut c: ChordEvent) -> Result<u32> {
    // Cut first (a split adds a chord), then count.
    carve(&mut st.chords, c.tick, c.tick.saturating_add(c.length_ticks), None, &mut st.next_id);
    if st.chords.len() >= MAX_CHORDS {
        return Err(bad(cmd, format!("the chord lane is full ({MAX_CHORDS} chords)")));
    }
    c.id = take_id(&mut st.next_id);
    let id = c.id;
    st.chords.push(c);
    Ok(id)
}

fn chord_add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.chord.add";
    let at = position(e, C, p, "at")?;
    let end = position_param(e, C, p, "end")?;
    let exact = exact(C, p)?;
    let s = e.session();
    let half = half_ticks(s);
    let (a, b) = match end {
        Some(end) => span_at(s, C, at, end, exact)?,
        None => {
            let a = tick_at(s, at, exact);
            // The next chord or one bar, whichever comes first.
            let next = s.band.chords.iter().map(|c| c.tick).filter(|t| *t > a).min();
            let bar = a.saturating_add(2 * half);
            (a, next.map_or(bar, |n| n.min(bar)))
        }
    };
    let bass = int_in(C, p, "bass", -1, 11)?;
    let v = p.get("chord").ok_or_else(|| bad(C, "`chord` required (\"Am7\" or a slot 0-111)"))?;
    edit_band(e, |_, st| {
        let (slot, matched_bass, exact) = chord_value(C, st, "chord", v)?;
        let bass = bass.map_or(matched_bass, as_i8);
        let id = lay_chord(C, st, ChordEvent { id: 0, tick: a, length_ticks: b - a, slot, bass })?;
        Ok(json!({"chord": id, "slot": slot, "bass": bass, "exact": exact}))
    })
    .inspect(|_| show_lane_rulers(e))
}

fn chord_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.chord.set";
    let id = id_in(C, p, "chord")?;
    let old = e.session().band.chords.iter().find(|c| c.id == id).cloned().ok_or_else(|| bad(C, format!("no chord {id}")))?;
    let at = position_param(e, C, p, "at")?;
    let end = position_param(e, C, p, "end")?;
    let exact = exact(C, p)?;
    let s = e.session();
    let (a, b) = match (at, end) {
        (None, None) => (old.tick, old.tick.saturating_add(old.length_ticks)),
        (Some(at), None) => {
            let a = tick_at(s, at, exact);
            (a, a.saturating_add(old.length_ticks))
        }
        (at, en) => {
            let at = at.unwrap_or_else(|| s.tempo.tick_to_samples(old.tick, s.sample_rate));
            span_at(s, C, at, en.unwrap_or_default(), exact)?
        }
    };
    let bass = int_in(C, p, "bass", -1, 11)?;
    edit_band(e, |_, st| {
        let new = match (p.get("chord_text"), p.get("slot")) {
            (Some(t), _) if !t.is_string() => return Err(bad(C, "`chord_text` must be a chord name")),
            (Some(t), _) => Some(chord_value(C, st, "chord_text", t)?),
            (None, Some(v)) if v.is_string() => return Err(bad(C, "`slot` must be 0-111")),
            (None, Some(v)) => Some(chord_value(C, st, "slot", v)?),
            (None, None) => None,
        };
        carve(&mut st.chords, a, b, Some(id), &mut st.next_id);
        let Some(c) = st.chords.iter_mut().find(|c| c.id == id) else { return Err(bad(C, format!("no chord {id}"))) };
        (c.tick, c.length_ticks) = (a, b - a);
        let mut exact = true;
        if let Some((slot, matched_bass, ex)) = new {
            (c.slot, c.bass, exact) = (slot, matched_bass, ex);
        }
        if let Some(v) = bass {
            c.bass = as_i8(v);
        }
        Ok(json!({"chord": id, "slot": c.slot, "bass": c.bass, "exact": exact}))
    })
}

fn chord_remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.chord.remove";
    let id = id_in(C, p, "chord")?;
    edit_band(e, |_, st| {
        if !st.chords.iter().any(|c| c.id == id) {
            return Err(bad(C, format!("no chord {id}")));
        }
        st.chords.retain(|c| c.id != id);
        Ok(json!({"chord": id}))
    })
}

/// `band.chords.delete`: the chords listed, those starting inside `range`, or all of them.
fn chords_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.chords.delete";
    let ids: Option<Vec<u32>> = match p.get("chords") {
        None => None,
        Some(Value::Array(a)) => Some(
            a.iter()
                .map(|v| v.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or_else(|| bad(C, "`chords` must be chord ids")))
                .collect::<Result<_>>()?,
        ),
        Some(_) => return Err(bad(C, "`chords` must be an array of chord ids")),
    };
    let range = match p.get("range") {
        None => None,
        Some(r) => {
            let a = position_param(e, C, r, "start")?.ok_or_else(|| bad(C, "`range` needs `start` and `end`"))?;
            let b = position_param(e, C, r, "end")?.ok_or_else(|| bad(C, "`range` needs `start` and `end`"))?;
            let s = e.session();
            let tick = |x: Samples| s.tempo.samples_to_ticks(x.max(0), s.sample_rate);
            Some((tick(a.min(b)), tick(a.max(b))))
        }
    };
    let all = match p.get("all") {
        None => false,
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`all` must be true or false"))?,
    };
    if ids.is_none() && range.is_none() && !all {
        return Err(bad(C, "give `chords`, `range` or `all: true`"));
    }
    edit_band(e, |_, st| {
        let picked =
            |c: &ChordEvent| all || ids.as_ref().is_some_and(|ids| ids.contains(&c.id)) || range.is_some_and(|(a, b)| c.tick >= a && c.tick < b);
        let n = st.chords.iter().filter(|c| picked(c)).count();
        if n == 0 {
            return Err(bad(C, "no chords there"));
        }
        st.chords.retain(|c| !picked(c));
        Ok(json!({"deleted": n}))
    })
}

// ---- songs --------------------------------------------------------------------------------------

/// Where laid items span, in ticks (`None` when there are none).
fn extent<T: LaneItem>(items: &[T]) -> Option<(i64, i64)> {
    let a = items.iter().map(|i| i.span().1).min()?;
    let b = items.iter().map(|i| i.span().1.saturating_add(i.span().2)).max()?;
    Some((a, b))
}

/// Fresh ids for laid items, from `next_id`.
fn renumber<T: LaneItem>(items: Vec<T>, next_id: &mut u32) -> Vec<T> {
    items
        .into_iter()
        .map(|mut it| {
            let (_, a, len) = it.span();
            it.set(take_id(next_id), a, len);
            it
        })
        .collect()
}

fn bump_next_id(st: &mut BandState) {
    let top = st.sections.iter().map(|b| b.id).chain(st.chords.iter().map(|c| c.id)).max().unwrap_or(0);
    st.next_id = st.next_id.max(top.saturating_add(1)).max(1);
}

fn song_factory(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.song.factory";
    let name = name_in(C, p)?.to_string();
    let songs = factory_songs();
    let f = songs.iter().find(|f| f.name == name).or_else(|| songs.iter().find(|f| f.name.eq_ignore_ascii_case(&name))).ok_or_else(|| {
        bad(C, format!("no factory song `{name}` (one of: {})", songs.iter().map(|f| f.name.as_str()).collect::<Vec<_>>().join(", ")))
    })?;
    let at = position_param(e, C, p, "at")?.unwrap_or(0);
    let replace = bool_in(C, p, "replace")?.unwrap_or(true);
    let (defs, blocks, chords) = soundcraft_mix::lanes_from_song(e.session(), &f.song, at);
    let song = &f.song;
    edit_band(e, |_, st| {
        if replace {
            st.sections.clear();
            st.chords.clear();
            st.defs = defs;
        } else {
            for d in defs {
                match st.defs.iter_mut().find(|x| x.id == d.id) {
                    Some(x) => *x = d,
                    None => st.defs.push(d),
                }
            }
            for (a, b) in extent(&blocks).into_iter().chain(extent(&chords)) {
                carve(&mut st.sections, a, b, None, &mut st.next_id);
                carve(&mut st.chords, a, b, None, &mut st.next_id);
            }
        }
        if st.sections.len().saturating_add(blocks.len()) > MAX_BLOCKS || st.chords.len().saturating_add(chords.len()) > MAX_CHORDS {
            return Err(bad(C, format!("the lanes would hold more than {MAX_BLOCKS} blocks or {MAX_CHORDS} chords")));
        }
        let (nb, nc) = (blocks.len(), chords.len());
        // Ids after the cut: a split above took ids of its own.
        st.sections.extend(renumber(blocks, &mut st.next_id));
        st.chords.extend(renumber(chords, &mut st.next_id));
        bump_next_id(st);
        st.key = as_u8(i64::from(f.key.clamp(0, 11)));
        st.scale = as_u8(i64::from(f.scale.clamp(0, 1)));
        st.take = as_u8(i64::from(f.take.clamp(1, 16)));
        let sf = &song.feel;
        st.song_feel.on = sf.on;
        set_dials(st, [sf.time, sf.density, sf.pocket, sf.groove, sf.touch]);
        st.feel_ignore = song.feel_ignore;
        st.feel.tension = f.tension_on();
        Ok(json!({"song": f.name, "blocks": nb, "chords": nc}))
    })
    .inspect(|_| show_lane_rulers(e))
}

#[cfg(not(target_arch = "wasm32"))]
fn read_midi(path: &str) -> Result<Vec<u8>> {
    crate::io::read_file_max(path, MAX_MIDI_BYTES)
}

#[cfg(target_arch = "wasm32")]
fn read_midi(_: &str) -> Result<Vec<u8>> {
    Err(crate::EngineError::Unsupported("importing MIDI chords on the web build".into()))
}

fn chords_import(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.chords.import";
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(C, "`path` required"))?.to_string();
    let key = key_in(C, p, "key")?.unwrap_or(i64::from(e.session().band.key));
    let scale = choice_in(C, p, "scale", &SCALES)?.unwrap_or(i64::from(e.session().band.scale));
    let at = position_param(e, C, p, "at")?.unwrap_or(0);
    let bytes = read_midi(&path)?;
    // Matched in the key given (the band's by default): the slots are key-relative.
    let preview = soundcraft_band::midi_chord_import::preview_midi_import(&bytes, Some((key as i32, scale as i32))).map_err(|m| bad(C, m))?;
    let song: Song = soundcraft_band::midi_chord_import::song_from_preview(&preview);
    let (_, _, chords) = soundcraft_mix::lanes_from_song(e.session(), &song, at);
    let notes = preview.notes.clone();
    edit_band(e, |_, st| {
        if let Some((a, b)) = extent(&chords) {
            carve(&mut st.chords, a, b, None, &mut st.next_id);
        }
        if st.chords.len().saturating_add(chords.len()) > MAX_CHORDS {
            return Err(bad(C, format!("the chord lane would hold more than {MAX_CHORDS} chords")));
        }
        let n = chords.len();
        // Ids after the cut: a split above took ids of its own.
        st.chords.extend(renumber(chords, &mut st.next_id));
        bump_next_id(st);
        Ok(json!({"chords": n, "key": key, "scale": scale, "notes": notes}))
    })
    .inspect(|_| show_lane_rulers(e))
}

// ---- the Library --------------------------------------------------------------------------------

fn list(e: &mut Engine, _: &Value) -> Result<Value> {
    let lib = e.band_library();
    let mut players = Map::new();
    for (i, seeds) in lib.players.iter().enumerate() {
        let names: Vec<&str> = seeds.iter().filter_map(|s| lib.persona(*s)).map(|p| p.name.as_str()).collect();
        players.insert(instrument_name(as_u8(i as i64)).to_string(), json!(names));
    }
    let targets: Vec<Value> = (0..32)
        .filter_map(|slot| {
            let t = lib.target(slot)?;
            let inst: Vec<&str> = t.instruments.iter().enumerate().filter(|(_, on)| **on).map(|(i, _)| instrument_name(as_u8(i as i64))).collect();
            Some(json!({"slot": slot, "name": t.name, "instruments": inst}))
        })
        .collect();
    let (presets, skipped) = all_presets(e);
    let mut errors: Vec<String> = skipped.into_iter().map(|m| format!("Bands/{m}")).collect();
    let presets: Vec<Value> = presets.iter().map(|b| json!({"name": b.name, "factory": b.factory})).collect();
    let songs: Vec<String> = factory_songs().into_iter().map(|f| f.name).collect();
    let mut feel: Vec<Value> = factory_feel_presets().iter().map(|f| json!({"name": f.name, "factory": true})).collect();
    if let Some(dir) = e.band_user_folder() {
        match read_feel_file(dir) {
            Ok(text) => {
                feel.extend(text.map(|t| load_feel_presets(&t)).unwrap_or_default().iter().map(|u| json!({"name": u.name, "factory": false})))
            }
            Err(m) => errors.push(m),
        }
    }
    let mut lib_errors = lib.errors.clone();
    lib_errors.extend(errors);
    Ok(json!({
        "state": serde_json::to_value(&e.session().band).unwrap_or(Value::Null),
        "players_by_instrument": players,
        "targets": targets,
        "presets": presets,
        "factory_songs": songs,
        "feel_presets": feel,
        "library_errors": lib_errors,
        "user_folder": e.band_user_folder(),
    }))
}

// ---- print --------------------------------------------------------------------------------------

/// The musicians to print (0-based): `slots` (1-6), or every one some track plays.
fn print_slots(cmd: &str, s: &Session, p: &Value) -> Result<Vec<u8>> {
    let Some(v) = p.get("slots") else {
        let mut all: Vec<u8> = s.tracks.iter().filter_map(|t| if let Route::Musician(n) = t.mixer.input { Some(n) } else { None }).collect();
        all.sort_unstable();
        all.dedup();
        return Ok(all);
    };
    let list = v.as_array().ok_or_else(|| bad(cmd, format!("`slots` must be a list of slots 1-6 (got {v})")))?;
    list.iter()
        .map(|x| match whole(x) {
            Some(n) if (1..=6).contains(&n) => Ok(as_u8(n - 1)),
            _ => Err(bad(cmd, format!("`slots` must be a list of slots 1-6 (got {x})"))),
        })
        .collect()
}

/// The range to print: `range`, a section `block`, or the song (from the lanes' start to its end).
fn print_range(e: &Engine, cmd: &str, p: &Value, lib: &std::sync::Arc<LibraryData>) -> Result<Range> {
    let s = e.session();
    let r = if let Some(rv) = p.get("range") {
        let a = position_param(e, cmd, rv, "start")?.ok_or_else(|| bad(cmd, "`range.start` required"))?;
        let b = position_param(e, cmd, rv, "end")?.ok_or_else(|| bad(cmd, "`range.end` required"))?;
        // On a tick, so the clip's ticks fall where the band played.
        let a = s.tempo.tick_to_samples(s.tempo.samples_to_ticks(a.max(0), s.sample_rate), s.sample_rate);
        Range { start: a, end: b }
    } else if p.get("block").is_some() {
        let blk = find_block(cmd, &s.band, id_in(cmd, p, "block")?)?;
        let at = |tick: i64| s.tempo.tick_to_samples(tick, s.sample_rate);
        Range { start: at(blk.start_tick), end: at(blk.start_tick.saturating_add(blk.length_ticks)) }
    } else {
        if p.get("song").is_some_and(|v| v.as_bool() == Some(false)) {
            return Err(bad(cmd, "give a `range` or a `block` (or `song: true`)"));
        }
        let u = soundcraft_mix::prepare_band(s, lib, &|_| None);
        match u.as_deref().map(|u| (u.start, u.song_end)) {
            Some((Some(a), Some(b))) => Range { start: a, end: b },
            _ => return Err(bad(cmd, "the song lanes are empty: nothing to print")),
        }
    };
    if r.end <= r.start {
        return Err(bad(cmd, "the range to print is empty"));
    }
    Ok(r)
}

/// The name of the instrument on track `t`, as playback names it for the "auto" targets: the
/// plugin's description from the registry it comes from (built-in, CLAP, VST3, Audio Units). A
/// hosted instance reports that same cached description as its `info()`.
fn print_instrument_name(s: &Session, t: TrackId) -> Option<String> {
    let id = &s.track(t)?.instrument.as_ref()?.plugin;
    super::clap::plugin_info(id).map(|i| i.name.to_string())
}

fn print(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "band.print";
    let slots = print_slots(C, e.session(), p)?;
    let tracks: Vec<(TrackId, u8)> = e
        .session()
        .tracks
        .iter()
        .filter(|t| t.kind.is_midi())
        .filter_map(|t| match t.mixer.input {
            Route::Musician(n) if slots.contains(&n) => Some((t.id, n)),
            _ => None,
        })
        .collect();
    if tracks.is_empty() {
        return Err(bad(C, "No track plays a musician"));
    }
    let lib = e.band_library();
    let r = print_range(e, C, p, &lib)?;
    let seqs = {
        let s = e.session();
        crate::band_print::render_print(s, &lib, &|t| print_instrument_name(s, t), r, &slots)
    };
    let seq_of = |n: u8| seqs.get(usize::from(n)).cloned().unwrap_or_default();
    if tracks.iter().all(|&(_, n)| {
        let q = seq_of(n);
        q.notes.is_empty() && q.ctrls.is_empty()
    }) {
        return Err(bad(C, "Nothing to print in that range"));
    }
    let s = e.session_mut();
    let mut clips = Vec::new();
    let mut replaced = Vec::new();
    for (t, n) in tracks {
        let seq = seq_of(n);
        let notes = seq.notes.len();
        // What the printed clip lays over (cut or removed), reported.
        if let Some(tr) = s.track(t) {
            replaced.extend(tr.clips().iter().filter(|c| c.range().overlaps(&r)).map(|c| json!({"track": t.0, "clip": c.id})));
        }
        let id = s.new_clip_id();
        let name = s.track(t).map(|x| x.name.clone()).unwrap_or_default();
        let mut clip = Clip::midi(id, name, r.start, r.len(), seq);
        clip.midi_channels = true;
        crate::edit::place_clip(s, t, clip);
        if let Some(tr) = s.track_mut(t) {
            tr.mixer.input = Route::None;
        }
        clips.push(json!({"track": t.0, "clip": id, "notes": notes}));
    }
    Ok(json!({"clips": clips, "replaced": replaced, "start": r.start, "end": r.end}))
}

//! Juke, the drum machine (`docs/juke.md`): kits on instrument tracks.
//!
//! `track.new_beat` makes an instrument track playing Juke with its default kit and a first
//! pattern clip. A pad's samples are session sources (`Insert::samples`, keyed
//! `p{pad}.v{layer}.r{robin}`), saved in `Audio Files/` with the session like the Sampler's; the
//! mix engine hands each to the running instrument. The pads' names and families
//! (`Insert::kit`) colour the kit view and the step grid; everything that sounds is a parameter,
//! set with `mix.instrument_param` like any instrument's.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::plugins::juke::{JUKE_INFO, LAYERS, PADS, ROBINS, sample_key};
use soundcraft_model::{DrumFamily, Insert, KitBus, KitFx, KitLayout, KitPad, Route, TrackKind};
use std::path::Path;

/// Juke's plugin id.
pub const JUKE: &str = "juke";

/// Largest sample a pad takes (frames × channels): one minute of 192 kHz stereo.
const MAX_PAD_SAMPLE_VALUES: usize = 192_000 * 60 * 2;

/// The default kit's pads: names and families, in pad order (the notes and voices are the
/// plugin's parameter defaults: General MIDI drums).
pub const DEFAULT_PADS: [(&str, DrumFamily); PADS] = [
    ("Kick", DrumFamily::Kick),
    ("Rim", DrumFamily::Percussion),
    ("Snare", DrumFamily::Snare),
    ("Clap", DrumFamily::Clap),
    ("Closed hat", DrumFamily::Hat),
    ("Pedal hat", DrumFamily::Hat),
    ("Open hat", DrumFamily::Hat),
    ("Low tom", DrumFamily::Tom),
    ("Mid tom", DrumFamily::Tom),
    ("High tom", DrumFamily::Tom),
    ("Crash", DrumFamily::Cymbal),
    ("Ride", DrumFamily::Cymbal),
    ("Tambourine", DrumFamily::Percussion),
    ("Cowbell", DrumFamily::Percussion),
    ("Shaker", DrumFamily::Percussion),
    ("Clave", DrumFamily::Percussion),
];

/// The default kit's layout.
pub fn default_layout() -> KitLayout {
    KitLayout { name: "Porch Light".into(), pads: DEFAULT_PADS.iter().map(|(n, f)| KitPad { name: (*n).into(), family: *f }).collect() }
}

/// A Juke instrument with every parameter at its default and the default kit's layout.
pub fn juke_insert() -> Insert {
    let mut ins = Insert::new(JUKE);
    if let Some(info) = soundcraft_dsp::plugin_info(JUKE) {
        for p in info.params {
            ins.params.insert(p.id.to_string(), p.default);
        }
    }
    ins.kit = Some(default_layout());
    ins
}

/// A pad's layout entry as shown: the kit's own, else the default's.
pub fn pad_layout(ins: &Insert, pad: usize) -> KitPad {
    ins.kit.as_ref().and_then(|k| k.pads.get(pad)).cloned().unwrap_or_else(|| {
        let (name, family) = DEFAULT_PADS.get(pad).copied().unwrap_or(("Pad", DrumFamily::Percussion));
        KitPad { name: name.into(), family }
    })
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "track.new_beat",
            "New Beat",
            [],
            None,
            "{name?: 'Beat', start?, bars?: 1-64 (default 2), style?: a style from pattern.grooves, groove?} -> {track, clip}: an instrument track playing Juke (the default kit) with a first pattern clip; with `style`, that style's kit and groove (its first unless `groove` names one)",
            always,
            new_beat
        ),
        cmd!(
            "mix.kit_sample_load",
            "Load Pad Sample",
            [],
            None,
            "{track?, pad: 1-16, path, layer?: 1-4 (1 the softest; default 1), robin?: 1-4 (default 1)} -> {track, pad, key, source, name}: the pad plays the file (a pad on Synth switches to Sample)",
            has_selection,
            sample_load
        ),
        cmd!(
            "mix.kit_sample_clear",
            "Clear Pad Samples",
            [],
            None,
            "{track?, pad: 1-16, layer?: 1-4, robin?: 1-4} -> {cleared}: the slot given, else every sample of the layer, else of the pad",
            has_selection,
            sample_clear
        ),
        cmd!(
            "mix.kit_pad_import",
            "Import Pad Takes",
            [],
            None,
            "{track?, pad: 1-16, paths: [audio files], family?: kick|snare|clap|hat|tom|cymbal|percussion} -> {track, pad, name, family, hits, layers, robins, dropped, trim}: the pad's samples become the hits in the takes (a file holds one hit or a run of them): each trimmed to its attack and ring, sorted by strength into velocity layers (softest first) of up to four round-robins, and the pad's trim set to level it with the kit",
            has_selection,
            pad_import
        ),
        cmd!(
            "mix.kit_pad_from_selection",
            "Pad from Selection",
            [],
            None,
            "{track: the Juke track, pad: 1-16, from?, start?, end?, family?} -> as mix.kit_pad_import: the range (default the edit selection) of audio track `from` (default the first selected audio track), rendered as its clips play, imported into the pad: its hits trimmed, sorted into layers and round-robins, the pad levelled; named for the clip under the selection",
            has_selection,
            pad_from_selection
        ),
        cmd!(
            "mix.kit_import",
            "Import Kit",
            [],
            None,
            "{track?, folder? | paths?: [audio files], name?} -> {kit, pads: [{pad, name, hits, layers}], skipped: [name]}: drum takes to pads by their names (kick, snare, rim, clap, hat, pedal hat, open hat, toms, crash, ride, tambourine, cowbell, shaker, clave; a sub-folder's name counts too), each pad imported as by mix.kit_pad_import; files of no known drum take the free pads. A folder names the kit",
            has_selection,
            kit_import
        ),
        cmd!(
            "mix.kit_pad",
            "Edit Pad",
            [],
            None,
            "{track?, pad: 1-16, name?, family?: kick|snare|clap|hat|tom|cymbal|percussion} -> {pad}",
            has_selection,
            pad_edit
        ),
        cmd!("mix.kit_name", "Rename Kit", [], None, "{track?, name} -> {name}", has_selection, kit_name),
        cmd!(
            "mix.kit_randomize",
            "Randomize Kit",
            [],
            None,
            "{track?, pad?: 1-16 (default every pad), seed?} -> {pads}: new takes on the pads' sounds: their tuning, decay, tone, snap, drive and (kicks and toms) pitch sweep, around what they are now",
            has_selection,
            kit_randomize
        ),
        cmd!(
            query "mix.kit_presets",
            "List Kit Presets",
            [],
            None,
            "{} -> {presets: [{name, category, about, source, needs}]}: Juke's built-in presets; `needs` names the recorded kits one needs installed",
            always,
            kit_presets
        ),
        cmd!(
            "mix.kit_preset",
            "Load Kit Preset",
            [],
            None,
            "{track?, preset: a name from mix.kit_presets | path: a preset file, kits_dir?: the recorded kits' folder} -> {preset, kit}: a whole kit sound: its samples (a starter, style or recorded kit, pads borrowed from other recorded kits), every pad's tuning, envelopes, filter, drive, level, pan and sends, the kit's levels and its bus effects with their settings; one undo step",
            has_selection,
            kit_preset
        ),
        cmd!(
            noundo "mix.kit_preset_save",
            "Save Kit Preset",
            [],
            None,
            "{track?, path: a .json file, name?, category?, kits_dir?} -> {path, name, samples}: the Juke kit as a preset file that mix.kit_preset {path} loads back: every setting and bus effect; samples from installed recorded kits are named, any other sample is saved as a WAV beside the file",
            has_selection,
            kit_preset_save
        ),
        cmd!(
            noundo "mix.kit_save",
            "Save Kit",
            [],
            None,
            "{track?, path: a folder} -> {path, samples, name}: the Juke kit as a kit folder (kit.json and its samples as WAV files) that mix.kit_load {path} loads back: its pads' names, families, notes, samples, every setting and its bus effects",
            has_selection,
            kit_save
        ),
        cmd!(
            query "mix.kit_list",
            "List Kits",
            [],
            None,
            "{} -> {kits: [{name, rendered}], characters: [{name, about}]}: Juke's starter kits, and the characters mix.kit_load can lay over any kit",
            always,
            kit_list
        ),
        cmd!(
            "mix.kit_load",
            "Load Kit",
            [],
            None,
            "{track?, kit: name | path: an installed recorded kit's folder, character?: a name from mix.kit_list (Tight, Punch, Big Room…)} -> {kit, samples, character?}: a starter kit (or a recorded kit downloaded from the kits release) on the Juke track: its sounds, names and bus effects; the pads keep their notes (unless a recorded kit gives them), mutes and solos. A rendered kit's pads play samples made from their synth voices. A character lays its settings and bus effects over the kit (named `Kit · Character`)",
            has_selection,
            kit_load
        ),
        cmd!(
            "mix.kit_fx",
            "Kit Effect",
            [],
            None,
            "{track?, bus: a|b|out, slot: 1-4, plugin?: a built-in effect id, or '' to empty the slot, params?: {id: value}, bypass?} -> {bus, slot, module, plugin}: a FireStation effect on one of Juke's buses (bus A, bus B or the output); `module` is its module id for the module commands",
            has_selection,
            kit_fx
        ),
    ]
}

/// The Juke track a kit command acts on.
fn juke_track(e: &Engine, id: &str, p: &Value) -> Result<TrackId> {
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    match e.session().track(t) {
        Some(tr) if tr.instrument.as_ref().is_some_and(|i| i.plugin == JUKE) => Ok(t),
        Some(_) => Err(bad(id, "the track's instrument is not Juke")),
        None => Err(bad(id, "no track")),
    }
}

fn slot_param(p: &Value, key: &str, max: usize, id: &str) -> Result<Option<usize>> {
    match p.get(key) {
        None => Ok(None),
        Some(v) => match v.as_u64() {
            Some(n) if (1..=max as u64).contains(&n) => Ok(Some(n as usize - 1)),
            _ => Err(bad(id, format!("`{key}` must be a whole number from 1 to {max}"))),
        },
    }
}

fn pad_param(p: &Value, id: &str) -> Result<usize> {
    slot_param(p, "pad", PADS, id)?.ok_or_else(|| bad(id, format!("`pad` (1 to {PADS}) required")))
}

fn juke_mut(e: &mut Engine, t: TrackId) -> Option<&mut Insert> {
    e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).filter(|i| i.plugin == JUKE)
}

fn new_beat(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "track.new_beat";
    let name = str_param(p, "name").unwrap_or("Beat").to_string();
    let bars = match p.get("bars") {
        None => 2,
        Some(v) => v.as_i64().filter(|n| (1..=64).contains(n)).ok_or_else(|| bad(id, "`bars` must be a whole number from 1 to 64"))?,
    };
    let at = position_param(e, id, p, "start")?.unwrap_or(e.session().edit.selection.start);
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Instrument, soundcraft_model::ChannelFormat::Stereo, Some(&name));
    if let Some(tr) = s.track_mut(t) {
        tr.instrument = Some(juke_insert());
        tr.mixer.input = Route::Midi(soundcraft_model::MidiIn::all());
    }
    s.edit.selected_tracks = vec![t];
    let (clip, _, _) = super::pattern::place_pattern(e, t, at, bars, 16, &super::pattern::DEFAULT_LANES)?;
    if let Some(name) = str_param(p, "style") {
        let style = crate::juke_styles::find(name).ok_or_else(|| bad(id, format!("no style `{name}` (pattern.grooves lists them)")))?;
        kit_load(e, &json!({"track": t, "kit": style.kit}))?;
        let mut g = json!({"clip": clip.0, "style": style.name});
        if let Some(v) = p.get("groove") {
            g["groove"] = v.clone();
        }
        super::pattern::groove(e, &g)?;
    }
    Ok(json!({"track": t, "clip": clip.0}))
}

fn sample_load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_sample_load";
    let t = juke_track(e, id, p)?;
    let pad = pad_param(p, id)?;
    let layer = slot_param(p, "layer", LAYERS, id)?.unwrap_or(0);
    let robin = slot_param(p, "robin", ROBINS, id)?.unwrap_or(0);
    let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    let bytes = crate::io::read_file(&path)?;
    let name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
    attach_sample(e, t, (pad, layer, robin), &name, &bytes, Some(&path))
}

/// Decodes `bytes` (a file called `name`, read from `path` when there is one) into slot
/// `(pad, layer, robin)` of track `t`'s Juke, each from 0. Decodes before touching the session,
/// so a bad file changes nothing.
pub fn attach_sample(
    e: &mut Engine,
    t: TrackId,
    (pad, layer, robin): (usize, usize, usize),
    name: &str,
    bytes: &[u8],
    path: Option<&str>,
) -> Result<Value> {
    let id = "mix.kit_sample_load";
    if pad >= PADS || layer >= LAYERS || robin >= ROBINS {
        return Err(bad(id, "no such pad, layer or round-robin"));
    }
    if juke_mut(e, t).is_none() {
        return Err(bad(id, "the track's instrument is not Juke"));
    }
    let name = name.to_string();
    let ext = Path::new(&name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (format, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    if buf.frames() == 0 || buf.num_channels() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    let buf = crate::io::at_rate(buf, e.session().sample_rate.hz(), MAX_PAD_SAMPLE_VALUES)
        .ok_or_else(|| EngineError::Io(format!("{name}: too long for a pad (a minute of 192 kHz stereo at most)")))?;
    let key = sample_key(pad, layer, robin);
    let s = e.session_mut();
    let src = crate::io::add_source(s, &name, buf, path, format.format);
    let stem = Path::new(&name).file_stem().and_then(|x| x.to_str()).unwrap_or("Sample").to_string();
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    let first = !ins.samples.keys().any(|k| k.starts_with(&format!("p{}.", pad + 1)));
    ins.samples.insert(key.clone(), src);
    // A synth pad given a sample plays it; a pad given its first sample takes the file's name.
    let source_id = format!("p{}_source", pad + 1);
    if ins.params.get(&source_id).copied().unwrap_or(1.0).round() as i64 == 1 {
        ins.params.insert(source_id, 0.0);
    }
    if first {
        let layout = ins.kit.get_or_insert_with(default_layout);
        while layout.pads.len() < PADS {
            let i = layout.pads.len();
            let (n, f) = DEFAULT_PADS.get(i).copied().unwrap_or(("Pad", DrumFamily::Percussion));
            layout.pads.push(KitPad { name: n.into(), family: f });
        }
        if let Some(entry) = layout.pads.get_mut(pad) {
            entry.name = stem.chars().take(32).collect();
        }
    }
    Ok(json!({"track": t, "pad": pad + 1, "key": key, "source": src, "name": stem}))
}

fn sample_clear(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_sample_clear";
    let t = juke_track(e, id, p)?;
    let pad = pad_param(p, id)?;
    let layer = slot_param(p, "layer", LAYERS, id)?;
    let robin = slot_param(p, "robin", ROBINS, id)?;
    if robin.is_some() && layer.is_none() {
        return Err(bad(id, "`robin` needs its `layer`"));
    }
    let keys: Vec<String> = match (layer, robin) {
        (Some(l), Some(r)) => vec![sample_key(pad, l, r)],
        (Some(l), None) => (0..ROBINS).map(|r| sample_key(pad, l, r)).collect(),
        _ => (0..LAYERS).flat_map(|l| (0..ROBINS).map(move |r| sample_key(pad, l, r))).collect(),
    };
    let present =
        e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map_or(0, |i| keys.iter().filter(|k| i.samples.contains_key(*k)).count());
    if present == 0 {
        return Ok(json!({"cleared": 0}));
    }
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    for k in &keys {
        ins.samples.remove(k);
    }
    Ok(json!({"cleared": present}))
}

/// Most hits read from one import (they are sorted and the best sixteen a pad holds kept).
const MAX_IMPORT_HITS: usize = 256;

fn pad_import(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_pad_import";
    let t = juke_track(e, id, p)?;
    let pad = pad_param(p, id)?;
    let paths = string_list(p, "paths");
    if paths.is_empty() {
        return Err(bad(id, "`paths` required: the audio files to import"));
    }
    let family = match p.get("family").and_then(Value::as_str) {
        Some(f) => Some(family_named(f).ok_or_else(|| bad(id, format!("no drum family `{f}`")))?),
        None => None,
    };
    let takes = read_takes(&paths)?;
    import_pad(e, t, pad, &takes, family)
}

/// The strings in `p[key]` (an array of strings).
fn string_list(p: &Value, key: &str) -> Vec<String> {
    p.get(key).and_then(Value::as_array).map(|a| a.iter().filter_map(Value::as_str).map(str::to_string).collect()).unwrap_or_default()
}

fn pad_from_selection(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_pad_from_selection";
    let t = match track_param(e, id, p, "track")? {
        Some(t) => t,
        None => e
            .target_tracks()
            .into_iter()
            .find(|x| juke_track(e, id, &json!({"track": x.0})).is_ok())
            .ok_or_else(|| bad(id, "`track` required: the Juke track"))?,
    };
    juke_track(e, id, &json!({"track": t.0}))?;
    let pad = pad_param(p, id)?;
    let family = match p.get("family").and_then(Value::as_str) {
        Some(f) => Some(family_named(f).ok_or_else(|| bad(id, format!("no drum family `{f}`")))?),
        None => None,
    };
    let s = e.session();
    let audio_kind = |k: TrackKind| k.has_playlist() && !k.is_midi();
    let from = match track_param(e, id, p, "from")? {
        Some(f) => f,
        None => e
            .target_tracks()
            .into_iter()
            .find(|x| s.track(*x).is_some_and(|tr| audio_kind(tr.kind)))
            .ok_or_else(|| bad(id, "select a range on an audio track, or pass `from`"))?,
    };
    let tr = s.track(from).ok_or_else(|| bad(id, "no source track"))?;
    if !audio_kind(tr.kind) {
        return Err(bad(id, format!("`{}` has no audio clips to sample", tr.name)));
    }
    let r = range_param(e, id, p)?;
    let start = r.start.max(0);
    let len = r.end.saturating_sub(start);
    if len <= 0 {
        return Err(bad(id, "the selection is empty: select a range of drum hits"));
    }
    let max = MAX_PAD_SAMPLE_VALUES * 10 / tr.channels().max(1);
    crate::io::refuse_longer(s, id, &tr.name, "import", len, max.min(crate::io::render_limit(tr.channels())))?;
    let range = soundcraft_time::Range::new(start, start + len);
    let audio = soundcraft_mix::render_clips(s, from, range);
    if audio.iter().all(|ch| ch.iter().all(|x| *x == 0.0)) {
        return Err(bad(id, format!("the selection on `{}` is silent", tr.name)));
    }
    // Named for the clip under the selection (its name usually says which drum), else the track.
    let name =
        tr.clips().iter().find(|c| c.range().start < range.end && c.range().end > range.start).map_or_else(|| tr.name.clone(), |c| c.name.clone());
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: audio };
    import_pad_audio(e, id, t, pad, &name, &[buf], soundcraft_audio_io::FileFormat::Wav, family)
}

/// Reads each file: (its name, its bytes).
fn read_takes(paths: &[String]) -> Result<Vec<(String, Vec<u8>)>> {
    paths
        .iter()
        .map(|path| {
            let name = Path::new(path).file_name().and_then(|n| n.to_str()).unwrap_or("Take").to_string();
            Ok((name, crate::io::read_file(path)?))
        })
        .collect()
}

fn family_named(f: &str) -> Option<DrumFamily> {
    DrumFamily::from_id(&f.to_ascii_lowercase())
}

/// Imports drum takes (file name, bytes) into pad `pad` (from 0) of track `t`'s Juke: the hits
/// found in them, sorted into velocity layers and round-robins, replace the pad's samples; the
/// pad takes the takes' name and family (`family`, else guessed from the name, else kept) and the
/// trim that levels it. Decodes everything before touching the session, so a bad file changes
/// nothing.
pub fn import_pad(e: &mut Engine, t: TrackId, pad: usize, takes: &[(String, Vec<u8>)], family: Option<DrumFamily>) -> Result<Value> {
    let rate = e.session().sample_rate.hz();
    let mut bufs = Vec::new();
    let mut format = None;
    for (name, bytes) in takes {
        let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let (info, buf) = soundcraft_audio_io::decode(bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        let buf = crate::io::at_rate(buf, rate, MAX_PAD_SAMPLE_VALUES * 10)
            .ok_or_else(|| EngineError::Io(format!("{name}: too long to import (ten minutes of 192 kHz stereo at most)")))?;
        format.get_or_insert(info.format);
        bufs.push(buf);
    }
    let first = takes.first().map_or("Pad", |(n, _)| n.as_str()).to_string();
    import_pad_audio(e, "mix.kit_pad_import", t, pad, &first, &bufs, format.unwrap_or(soundcraft_audio_io::FileFormat::Wav), family)
}

/// [`import_pad`] from audio already at the session's rate: `name` names the pad (and guesses
/// its family), `format` is the format its samples are saved in.
#[allow(clippy::too_many_arguments)]
fn import_pad_audio(
    e: &mut Engine,
    id: &str,
    t: TrackId,
    pad: usize,
    first: &str,
    bufs: &[soundcraft_audio_io::AudioBuffer],
    format: soundcraft_audio_io::FileFormat,
    family: Option<DrumFamily>,
) -> Result<Value> {
    use soundcraft_dsp::drum_import;
    if pad >= PADS {
        return Err(bad(id, "no such pad"));
    }
    if juke_mut(e, t).is_none() {
        return Err(bad(id, "the track's instrument is not Juke"));
    }
    let rate = e.session().sample_rate.hz();
    let mut hits = Vec::new();
    for buf in bufs {
        hits.extend(drum_import::split(&buf.channels, rate as f32));
        if hits.len() >= MAX_IMPORT_HITS {
            hits.truncate(MAX_IMPORT_HITS);
            break;
        }
    }
    if hits.is_empty() {
        return Err(bad(id, "no drum hits found in the audio"));
    }
    // A pitched drum (a kick or an 808, toms) keeps the hits at its most common note: an 808
    // line's other notes would play as wrong notes among the round-robins.
    let pitches: Vec<Option<f32>> = hits
        .iter()
        .map(|h| {
            let n = h.channels.len().max(1) as f32;
            let len = h.channels.iter().map(Vec::len).min().unwrap_or(0).min(rate as usize * 2);
            let mono: Vec<f32> = (0..len).map(|i| h.channels.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / n).collect();
            drum_import::hit_pitch(&mono, rate as f32)
        })
        .collect();
    let off_pitch = match drum_import::common_note(&pitches) {
        Some(note) => {
            let before = hits.len();
            // Only hits clearly at another note go; one whose pitch could not be read stays.
            let keep: Vec<bool> = pitches.iter().map(|p| p.is_none_or(|m| (m - note).abs() <= 0.6)).collect();
            let mut k = keep.iter();
            hits.retain(|_| k.next().copied().unwrap_or(false));
            before - hits.len()
        }
        None => 0,
    };
    let punch: Vec<f32> = hits.iter().map(|h| h.punch_db).collect();
    let plan = drum_import::plan(&punch);
    let name = pad_name(first);
    let family = family
        .or_else(|| guess_drum(first).1)
        .unwrap_or_else(|| pad_layout(e.session().track(t).and_then(|tr| tr.instrument.as_ref()).unwrap_or(&juke_insert()), pad).family);
    let loud: Vec<&drum_import::Hit> = plan.slots.iter().filter(|s| s.1 + 1 == plan.layers).filter_map(|s| hits.get(s.0)).collect();
    let loud_db = loud.iter().map(|h| h.punch_db).sum::<f32>() / loud.len().max(1) as f32;
    let peak = plan.slots.iter().filter_map(|s| hits.get(s.0)).fold(0.0f32, |m, h| m.max(h.peak));
    let trim = drum_import::trim(matches!(family, DrumFamily::Hat | DrumFamily::Cymbal), loud_db, peak);
    let mut robins = [0usize; LAYERS];
    let mut sources = Vec::new();
    {
        let s = e.session_mut();
        for &(h, layer, robin) in &plan.slots {
            let Some(hit) = hits.get(h) else { continue };
            let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: hit.channels.clone() };
            let src = crate::io::add_source(s, &format!("{name} {} {}.wav", layer + 1, robin + 1), buf, None, format);
            sources.push((sample_key(pad, layer, robin), src));
            if let Some(n) = robins.get_mut(layer) {
                *n += 1;
            }
        }
    }
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    let prefix = format!("p{}.", pad + 1);
    ins.samples.retain(|k, _| !k.starts_with(&prefix));
    ins.samples.extend(sources);
    ins.params.insert(format!("p{}_source", pad + 1), 0.0);
    ins.params.insert(format!("p{}_trim", pad + 1), trim);
    let layout = ins.kit.get_or_insert_with(default_layout);
    while layout.pads.len() < PADS {
        let i = layout.pads.len();
        let (n, f) = DEFAULT_PADS.get(i).copied().unwrap_or(("Pad", DrumFamily::Percussion));
        layout.pads.push(KitPad { name: n.into(), family: f });
    }
    if let Some(entry) = layout.pads.get_mut(pad) {
        entry.name = name.clone();
        entry.family = family;
    }
    Ok(json!({
        "track": t, "pad": pad + 1, "name": name, "family": family.id(),
        "hits": hits.len(), "layers": plan.layers, "robins": robins.get(..plan.layers).unwrap_or(&[]),
        "dropped": plan.dropped, "off_pitch": off_pitch, "trim": trim,
    }))
}

/// A pad name from a take's file name: its stem without take numbers, velocity marks and
/// separators ("Snare_03_hard.wav" is "Snare").
fn pad_name(file: &str) -> String {
    let stem = Path::new(file).file_stem().and_then(|x| x.to_str()).unwrap_or(file);
    let marks = ["hard", "soft", "medium", "med", "ghost", "loud", "quiet", "vel", "rr", "take", "hit", "layer"];
    let words: Vec<&str> = stem
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .filter(|w| !w.chars().all(|c| c.is_ascii_digit()))
        .filter(|w| !marks.iter().any(|m| w.trim_end_matches(|c: char| c.is_ascii_digit()).eq_ignore_ascii_case(m)))
        .collect();
    let name = if words.is_empty() { stem.to_string() } else { words.join(" ") };
    let name: String = name.trim_end_matches(|c: char| c.is_ascii_digit()).trim().chars().take(32).collect();
    if name.is_empty() { "Pad".into() } else { name }
}

/// The default layout's pad a drum take's name points to, and its family: "Kick 03.wav" is
/// pad 1 (kick); a tom without high, mid or low is a tom with no pad of its own.
pub fn guess_drum(name: &str) -> (Option<usize>, Option<DrumFamily>) {
    use DrumFamily::*;
    let lower = name.to_lowercase();
    let words: Vec<String> = lower
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| w.trim_end_matches(|c: char| c.is_ascii_digit()).to_string())
        .filter(|w| !w.is_empty())
        .collect();
    let has = |list: &[&str]| words.iter().any(|w| list.contains(&w.as_str()));
    let joined: String = lower.chars().filter(|c| c.is_alphanumeric()).collect();
    let hatty = has(&["hat", "hh", "hihat", "hats"]) || joined.contains("hihat");
    if (hatty && has(&["open", "opn"])) || has(&["oh", "ohh", "ohat"]) {
        return (Some(6), Some(Hat));
    }
    if (hatty && has(&["pedal", "foot", "ped"])) || has(&["ph", "phh"]) {
        return (Some(5), Some(Hat));
    }
    if hatty || has(&["ch", "chh", "chat"]) {
        return (Some(4), Some(Hat));
    }
    if has(&["kick", "kik", "bd", "kck", "kicks"]) || joined.contains("bassdrum") {
        return (Some(0), Some(Kick));
    }
    if has(&["rim", "rimshot", "sidestick", "xstick", "stick", "rs"]) {
        return (Some(1), Some(Percussion));
    }
    if has(&["snare", "sn", "sd", "snr", "snares"]) {
        return (Some(2), Some(Snare));
    }
    if has(&["clap", "cp", "claps", "clp"]) {
        return (Some(3), Some(Clap));
    }
    let tom = has(&["tom", "toms", "floor", "ft", "lt", "mt", "ht"]) || joined.contains("tom");
    if tom {
        let num =
            lower.split("tom").nth(1).and_then(|r| r.trim_start_matches(|c: char| !c.is_alphanumeric()).chars().next()).and_then(|c| c.to_digit(10));
        if has(&["floor", "ft", "lt", "low", "lo"]) || num == Some(3) || num == Some(4) {
            return (Some(7), Some(Tom));
        }
        if has(&["mid", "mt", "middle"]) || num == Some(2) {
            return (Some(8), Some(Tom));
        }
        if has(&["high", "hi", "ht", "rack"]) || num == Some(1) {
            return (Some(9), Some(Tom));
        }
        return (None, Some(Tom));
    }
    if has(&["crash", "cr", "splash", "china", "crsh"]) {
        return (Some(10), Some(Cymbal));
    }
    if has(&["ride", "rd", "bell"]) {
        return (Some(11), Some(Cymbal));
    }
    if has(&["tamb", "tambourine", "tambo"]) {
        return (Some(12), Some(Percussion));
    }
    if has(&["cowbell", "cow", "cb"]) {
        return (Some(13), Some(Percussion));
    }
    if has(&["shaker", "shk", "shake", "maraca", "maracas"]) {
        return (Some(14), Some(Percussion));
    }
    if has(&["clave", "claves", "block", "woodblock"]) {
        return (Some(15), Some(Percussion));
    }
    (None, None)
}

/// Whether `name` is an audio file Juke imports.
fn audio_file(name: &str) -> bool {
    let ext = Path::new(name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase).unwrap_or_default();
    ["wav", "wave", "aif", "aiff", "flac", "mp3", "ogg", "opus", "m4a", "caf"].contains(&ext.as_str())
}

fn kit_import(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_import";
    let t = juke_track(e, id, p)?;
    // (file path, what names it: its sub-folder and its name).
    let mut files: Vec<(String, String)> = Vec::new();
    let folder = p.get("folder").and_then(Value::as_str);
    if let Some(dir) = folder {
        let mut entries = crate::io::list_dir(dir);
        entries.sort();
        for entry in entries {
            let fname = entry.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
            if audio_file(&fname) {
                files.push((entry.to_string_lossy().into_owned(), fname));
            } else if entry.extension().is_none() {
                // One level of sub-folders: "Kick/01.wav" is a kick.
                let mut inner = crate::io::list_dir(&entry);
                inner.sort();
                for f in inner {
                    let n = f.file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
                    if audio_file(&n) {
                        files.push((f.to_string_lossy().into_owned(), format!("{fname} {n}")));
                    }
                }
            }
        }
    }
    for path in string_list(p, "paths") {
        let n = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or_default().to_string();
        if audio_file(&n) {
            files.push((path, n));
        }
    }
    if files.is_empty() {
        return Err(bad(id, "no audio files to import"));
    }
    // Group by pad: known drums to their pads, the rest by name.
    let mut fixed: Vec<(usize, Vec<(String, String)>)> = Vec::new();
    let mut loose: Vec<(String, Option<DrumFamily>, Vec<(String, String)>)> = Vec::new();
    for (path, label) in files {
        match guess_drum(&label) {
            (Some(pad), _) => match fixed.iter_mut().find(|g| g.0 == pad) {
                Some(g) => g.1.push((path, label)),
                None => fixed.push((pad, vec![(path, label)])),
            },
            (None, fam) => {
                let key = pad_name(&label).to_lowercase();
                match loose.iter_mut().find(|g| g.0 == key) {
                    Some(g) => g.2.push((path, label)),
                    None => loose.push((key, fam, vec![(path, label)])),
                }
            }
        }
    }
    let mut taken: Vec<usize> = fixed.iter().map(|g| g.0).collect();
    let mut groups: Vec<(usize, Option<DrumFamily>, Vec<(String, String)>)> = fixed.into_iter().map(|(pad, f)| (pad, None, f)).collect();
    let mut skipped: Vec<String> = Vec::new();
    for (key, fam, f) in loose {
        // A tom takes a free tom pad first; anything else the first free pad.
        let prefer: Vec<usize> = if fam == Some(DrumFamily::Tom) { vec![9, 8, 7] } else { Vec::new() };
        match prefer.into_iter().chain(0..PADS).find(|i| !taken.contains(i)) {
            Some(pad) => {
                taken.push(pad);
                groups.push((pad, fam, f));
            }
            None => skipped.push(key),
        }
    }
    groups.sort_by_key(|g| g.0);
    let mut pads = Vec::new();
    for (pad, fam, f) in groups {
        let paths: Vec<String> = f.iter().map(|(p, _)| p.clone()).collect();
        let label = f.first().map(|(_, l)| l.clone()).unwrap_or_default();
        let mut takes = read_takes(&paths)?;
        // The pad is named for what named the files (a sub-folder's name, else the file's).
        if let Some(first) = takes.first_mut() {
            first.0 = label;
        }
        match import_pad(e, t, pad, &takes, fam) {
            Ok(v) => pads.push(json!({"pad": pad + 1, "name": v["name"], "hits": v["hits"], "layers": v["layers"]})),
            Err(err) => skipped.push(format!("{}: {err}", pad_name(&takes.first().map(|x| x.0.clone()).unwrap_or_default()))),
        }
    }
    if pads.is_empty() {
        return Err(bad(id, format!("nothing imported: {}", skipped.join("; "))));
    }
    let kit = p
        .get("name")
        .and_then(Value::as_str)
        .map(str::to_string)
        .or_else(|| folder.and_then(|d| Path::new(d).file_name()).and_then(|n| n.to_str()).map(str::to_string));
    if let Some(name) = &kit
        && let Some(ins) = juke_mut(e, t)
    {
        ins.kit.get_or_insert_with(default_layout).name = name.chars().take(48).collect();
    }
    Ok(json!({"kit": kit, "pads": pads, "skipped": skipped}))
}

fn pad_edit(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_pad";
    let t = juke_track(e, id, p)?;
    let pad = pad_param(p, id)?;
    let name = match p.get("name") {
        None => None,
        Some(v) => Some(
            v.as_str()
                .map(|s| s.trim().chars().take(32).collect::<String>())
                .filter(|s| !s.is_empty())
                .ok_or_else(|| bad(id, "`name` must be some text"))?,
        ),
    };
    let family = match p.get("family") {
        None => None,
        Some(v) => Some(
            v.as_str().and_then(DrumFamily::from_id).ok_or_else(|| bad(id, "`family` must be kick, snare, clap, hat, tom, cymbal or percussion"))?,
        ),
    };
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    let mut layout = ins.kit.clone().unwrap_or_else(default_layout);
    while layout.pads.len() < PADS {
        let i = layout.pads.len();
        layout.pads.push(pad_layout(&Insert::new(JUKE), i));
    }
    if let Some(entry) = layout.pads.get_mut(pad) {
        if let Some(n) = name {
            entry.name = n;
        }
        if let Some(f) = family {
            entry.family = f;
        }
    }
    let out = layout.pads.get(pad).cloned();
    ins.kit = Some(layout);
    Ok(json!({"pad": out}))
}

fn kit_fx(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_fx";
    let t = juke_track(e, id, p)?;
    let bus = p
        .get("bus")
        .and_then(Value::as_str)
        .and_then(|b| KitBus::from_id(&b.trim().to_ascii_lowercase()))
        .ok_or_else(|| bad(id, "`bus` must be a, b or out"))?;
    let slot = slot_param(p, "slot", KitFx::SLOTS, id)?.ok_or_else(|| bad(id, format!("`slot` (1 to {}) required", KitFx::SLOTS)))?;
    let plugin = match p.get("plugin") {
        None => None,
        Some(Value::Null) => Some(String::new()),
        Some(v) => Some(v.as_str().ok_or_else(|| bad(id, "`plugin` must be a plugin id"))?.trim().to_string()),
    };
    let bypass = match p.get("bypass") {
        None => None,
        Some(v) => Some(v.as_bool().ok_or_else(|| bad(id, "`bypass` must be true or false"))?),
    };
    let module = KitFx::module_id(KitFx::index(bus, slot));
    let fresh = match plugin.as_deref() {
        Some("") | None => None,
        Some(name) => {
            let info = soundcraft_dsp::plugin_info(name).ok_or_else(|| bad(id, format!("`{name}` is not a built-in effect")))?;
            if info.is_instrument {
                return Err(bad(id, format!("`{name}` is an instrument, not an effect")));
            }
            Some(super::module::build_insert(id, info.id, p.get("params"))?)
        }
    };
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    let at = ins.kit_fx.iter().position(|f| f.bus == bus && usize::from(f.slot) == slot);
    match (plugin.as_deref(), fresh) {
        (_, Some(insert)) => {
            let fx = KitFx { bus, slot: slot as u8, insert };
            match at {
                Some(i) => {
                    if let Some(x) = ins.kit_fx.get_mut(i) {
                        *x = fx;
                    }
                }
                None => ins.kit_fx.push(fx),
            }
        }
        (Some(_), None) => {
            if let Some(i) = at {
                ins.kit_fx.remove(i);
            }
        }
        (None, None) => {
            let fx = at.and_then(|i| ins.kit_fx.get_mut(i)).ok_or_else(|| bad(id, "the slot is empty: give `plugin`"))?;
            let info = soundcraft_dsp::plugin_info(&fx.insert.plugin);
            if let Some(obj) = p.get("params").and_then(Value::as_object) {
                for (k, v) in obj {
                    let pi = info.and_then(|i| i.params.iter().find(|x| x.id == k)).ok_or_else(|| bad(id, format!("no parameter `{k}`")))?;
                    let x = v.as_f64().filter(|f| f.is_finite()).ok_or_else(|| bad(id, format!("`{k}` must be a number")))?;
                    fx.insert.params.insert(k.clone(), (x as f32).clamp(pi.min, pi.max));
                }
            }
        }
    }
    if let Some(b) = bypass
        && let Some(fx) = ins.kit_fx.iter_mut().find(|f| f.bus == bus && usize::from(f.slot) == slot)
    {
        fx.insert.bypass = b;
    }
    ins.sanitize_kit_fx();
    let held = ins.kit_effect(bus, slot).map(|f| f.insert.plugin.clone());
    Ok(json!({"bus": bus.id(), "slot": slot + 1, "module": module.0, "plugin": held}))
}

fn kit_list(_e: &mut Engine, _p: &Value) -> Result<Value> {
    let kits: Vec<Value> = crate::juke_kits::KITS.iter().map(|k| json!({"name": k.name, "rendered": k.rendered})).collect();
    let characters: Vec<Value> = crate::juke_kits::CHARACTERS.iter().map(|c| json!({"name": c.name, "about": c.about})).collect();
    Ok(json!({"kits": kits, "characters": characters}))
}

fn kit_load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_load";
    let character = match p.get("character").and_then(Value::as_str).map(str::trim).filter(|s| !s.is_empty()) {
        None => None,
        Some(c) => Some(crate::juke_kits::character(c).ok_or_else(|| {
            let names = crate::juke_kits::CHARACTERS.iter().map(|c| c.name).collect::<Vec<_>>().join(", ");
            bad(id, format!("`character` must be one of: {names}"))
        })?),
    };
    let t = juke_track(e, id, p)?;
    let mut v = kit_load_plain(e, p)?;
    if let Some(c) = character {
        let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
        let layout = ins.kit.clone().unwrap_or_else(default_layout);
        let mut params = std::mem::take(&mut ins.params);
        crate::juke_kits::apply_character(c, &mut params, |pad| layout.pads.get(pad).map_or(DrumFamily::Percussion, |p| p.family));
        ins.params = params;
        let name: String = format!("{} · {}", layout.name, c.name).chars().take(64).collect();
        ins.kit.get_or_insert_with(default_layout).name = name.clone();
        for (bus, slot, plugin, ps) in c.fx {
            let params: serde_json::Map<String, Value> = ps.iter().map(|(k, v)| ((*k).to_string(), json!(v))).collect();
            kit_fx(e, &json!({"track": t.0, "bus": bus.id(), "slot": usize::from(*slot) + 1, "plugin": plugin, "params": params}))?;
        }
        v["kit"] = json!(name);
        v["character"] = json!(c.name);
    }
    Ok(v)
}

fn kit_load_plain(e: &mut Engine, p: &Value) -> Result<Value> {
    use crate::juke_kits::{KITS, LAYER_VELOCITIES, find, render_pad, takes};
    let id = "mix.kit_load";
    let t = juke_track(e, id, p)?;
    if let Some(path) = str_param(p, "path") {
        return recorded_load(e, t, Path::new(path));
    }
    let names = || KITS.iter().map(|k| k.name).collect::<Vec<_>>().join(", ");
    let kit = p.get("kit").and_then(Value::as_str).and_then(find).ok_or_else(|| bad(id, format!("`kit` must be one of: {}", names())))?;
    let current = e.session().track(t).and_then(|tr| tr.instrument.clone()).ok_or_else(|| bad(id, "no instrument"))?;
    // Juke's defaults, the pads' notes, mutes and solos kept, then the kit's changes.
    let mut params: std::collections::BTreeMap<String, f32> = JUKE_INFO.params.iter().map(|q| (q.id.to_string(), q.default)).collect();
    for pad in 1..=PADS {
        for k in ["note", "mute", "solo"] {
            let key = format!("p{pad}_{k}");
            if let Some(v) = current.params.get(&key) {
                params.insert(key, *v);
            }
        }
    }
    for (k, v) in kit.params {
        params.insert((*k).to_string(), *v);
    }
    let mut fx = Vec::new();
    for (bus, slot, plugin, ps) in kit.fx {
        let obj: serde_json::Map<String, Value> = ps.iter().map(|(k, v)| ((*k).to_string(), json!(v))).collect();
        let insert = super::module::build_insert(id, plugin, Some(&Value::Object(obj)))?;
        fx.push(KitFx { bus: *bus, slot: *slot, insert });
    }
    let layout = KitLayout {
        name: kit.name.into(),
        pads: (0..PADS)
            .map(|i| match kit.pads.get(i) {
                Some((n, f)) => KitPad { name: (*n).into(), family: *f },
                None => {
                    let (n, f) = DEFAULT_PADS.get(i).copied().unwrap_or(("Pad", DrumFamily::Percussion));
                    KitPad { name: n.into(), family: f }
                }
            })
            .collect(),
    };
    let mut samples = std::collections::BTreeMap::new();
    if kit.rendered {
        let rate = e.session().sample_rate.hz();
        for (pad, kp) in layout.pads.iter().enumerate() {
            let mut any = false;
            for (layer, vel) in LAYER_VELOCITIES.iter().enumerate() {
                for take in 0..takes(kp.family).min(ROBINS) {
                    let cents = if take == 0 { 0.0 } else { 7.0 };
                    let Some(buf) = render_pad(&params, pad, *vel, cents, rate) else { continue };
                    let name = format!("{} {} v{} r{}.wav", kit.name, kp.name, layer + 1, take + 1);
                    let src = crate::io::add_source(e.session_mut(), &name, buf, None, soundcraft_audio_io::FileFormat::Wav);
                    samples.insert(sample_key(pad, layer, take), src);
                    any = true;
                }
            }
            if any {
                // The pad plays its takes; their loudness already follows the velocity.
                params.insert(format!("p{}_source", pad + 1), 0.0);
                params.insert(format!("p{}_velocity", pad + 1), 30.0);
            }
        }
    }
    let n = samples.len();
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    ins.params = params;
    ins.samples = samples;
    ins.kit = Some(layout);
    ins.kit_fx = fx;
    ins.sanitize_kit_fx();
    Ok(json!({"kit": kit.name, "samples": n}))
}

/// Loads the recorded kit installed in `dir` onto Juke track `t`.
fn recorded_load(e: &mut Engine, t: TrackId, dir: &Path) -> Result<Value> {
    let id = "mix.kit_load";
    let kit = crate::juke_kits::read_recorded(dir).map_err(|m| bad(id, m))?;
    let current = e.session().track(t).and_then(|tr| tr.instrument.clone()).ok_or_else(|| bad(id, "no instrument"))?;
    let mut params: std::collections::BTreeMap<String, f32> = JUKE_INFO.params.iter().map(|q| (q.id.to_string(), q.default)).collect();
    for pad in 1..=PADS {
        for k in ["note", "mute", "solo"] {
            let key = format!("p{pad}_{k}");
            if let Some(v) = current.params.get(&key) {
                params.insert(key, *v);
            }
        }
    }
    // A kit saved from Juke carries every setting.
    for (k, v) in &kit.params {
        if let Some(q) = JUKE_INFO.param(k) {
            params.insert(k.clone(), q.clamp(*v));
        }
    }
    for (i, pad) in kit.pads.iter().enumerate() {
        if let Some(n) = pad.note {
            params.insert(format!("p{}_note", i + 1), f32::from(n.min(127)));
        }
        if let Some(t) = pad.trim {
            params.insert(format!("p{}_trim", i + 1), t.clamp(-24.0, 24.0));
        }
        // Choke groups follow the kit's pads, not the default kit's slots.
        let hat = pad.family.as_deref() == Some("hat");
        let choke = pad.choke.map_or(if hat { 1.0 } else { 0.0 }, |c| f32::from(c.min(8)));
        params.insert(format!("p{}_choke", i + 1), choke);
    }
    {
        let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
        ins.params = params;
        ins.samples.clear();
        ins.kit_fx.clear();
    }
    // A missing or broken file fails the command, which leaves the session as it was.
    let mut n = 0;
    for (i, pad) in kit.pads.iter().enumerate() {
        for (key, file) in &pad.samples {
            let Some((layer, robin)) = crate::juke_kits::layer_robin(key) else { continue };
            let path = dir.join(file);
            let bytes = crate::io::read_file(&path)?;
            let name = path.file_name().and_then(|x| x.to_str()).unwrap_or("Sample").to_string();
            attach_sample(e, t, (i, layer, robin), &name, &bytes, Some(&path.to_string_lossy()))?;
            n += 1;
        }
    }
    let layout = KitLayout {
        name: kit.name.chars().take(64).collect(),
        pads: (0..PADS)
            .map(|i| match kit.pads.get(i) {
                Some(pad) => KitPad {
                    name: pad.name.chars().take(32).collect(),
                    family: pad.family.as_deref().and_then(DrumFamily::from_id).unwrap_or(DrumFamily::Percussion),
                },
                None => {
                    let (n, f) = DEFAULT_PADS.get(i).copied().unwrap_or(("Pad", DrumFamily::Percussion));
                    KitPad { name: n.into(), family: f }
                }
            })
            .collect(),
    };
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    for (i, pad) in kit.pads.iter().enumerate() {
        let layers =
            pad.samples.keys().filter_map(|k| crate::juke_kits::layer_robin(k)).map(|(l, _)| l).collect::<std::collections::BTreeSet<_>>().len();
        // Layered recordings carry their own dynamics; one layer leans on the velocity. A
        // recording plays whole and at its own pitch: none of the synth voice's short decay,
        // tuning or pitch sweep the pad had by default. (A saved kit keeps its own settings.)
        if layers > 0 && kit.params.is_empty() {
            let n = i + 1;
            ins.params.insert(format!("p{n}_velocity"), if layers > 1 { 30.0 } else { 75.0 });
            for (k, v) in [("decay", 5000.0), ("hold", 0.0), ("attack", 0.0), ("tune", 0.0), ("fine", 0.0), ("pitch_amount", 0.0), ("filter", 0.0)] {
                ins.params.insert(format!("p{n}_{k}"), v);
            }
        }
    }
    ins.kit = Some(layout);
    // A saved kit's bus effects.
    for f in &kit.fx {
        let mut q = f.clone();
        if let Some(o) = q.as_object_mut() {
            o.insert("track".into(), json!(t.0));
            kit_fx(e, &q)?;
        }
    }
    Ok(json!({"kit": kit.name, "samples": n, "license": kit.license, "credit": kit.credit}))
}

/// A small, repeatable random source for [`kit_randomize`] (splitmix64).
struct Dice(u64);

impl Dice {
    fn next(&mut self) -> u64 {
        self.0 = self.0.wrapping_add(0x9E37_79B9_7F4A_7C15);
        let mut z = self.0;
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }
    /// Uniform in `lo..hi`.
    fn range(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (hi - lo) * ((self.next() >> 40) as f32 / (1u64 << 24) as f32)
    }
}

fn kit_randomize(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_randomize";
    let t = juke_track(e, id, p)?;
    let pads: Vec<usize> = match p.get("pad") {
        None => (0..PADS).collect(),
        Some(_) => vec![pad_param(p, id)?],
    };
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    // Without a seed, the kit as it is now seeds the dice: each click gives a new take.
    let seed = match p.get("seed") {
        Some(v) => v.as_u64().ok_or_else(|| bad(id, "`seed` must be a whole number"))?,
        None => ins.params.iter().fold(0xC0FF_EE00_u64, |h, (k, v)| {
            k.bytes().chain(v.to_bits().to_le_bytes()).fold(h, |h, b| (h ^ u64::from(b)).wrapping_mul(0x100_0000_01B3))
        }),
    };
    let mut dice = Dice(seed);
    let get = |ins: &Insert, key: &str| ins.params.get(key).copied().or_else(|| JUKE_INFO.param(key).map(|q| q.default)).unwrap_or(0.0);
    for &pad in &pads {
        let n = pad + 1;
        let family = pad_layout(ins, pad).family;
        let decay = get(ins, &format!("p{n}_decay"));
        let mut set = |k: &str, v: f32| {
            let key = format!("p{n}_{k}");
            if let Some(q) = JUKE_INFO.param(&key) {
                ins.params.insert(key, q.clamp(v));
            }
        };
        set("tune", dice.range(-3.0, 3.0).round());
        set("fine", dice.range(-20.0, 20.0).round());
        set("decay", decay * 2f32.powf(dice.range(-0.7, 0.7)));
        set("tone", dice.range(25.0, 85.0).round());
        set("snappy", dice.range(30.0, 90.0).round());
        set("drive", dice.range(0.0, 25.0).round());
        if matches!(family, DrumFamily::Kick | DrumFamily::Tom) {
            set("pitch_amount", dice.range(4.0, 16.0).round());
            set("pitch_decay", dice.range(25.0, 120.0).round());
        }
    }
    Ok(json!({"pads": pads.iter().map(|p| p + 1).collect::<Vec<_>>()}))
}

fn kit_save(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_save";
    let t = juke_track(e, id, p)?;
    let dir = str_param(p, "path").map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(id, "`path` required: the kit's folder"))?;
    let dir = Path::new(dir);
    let s = e.session();
    let ins = s.track(t).and_then(|tr| tr.instrument.as_ref()).ok_or_else(|| bad(id, "no instrument"))?;
    crate::io::make_dir(dir)?;
    let name = ins.kit.as_ref().map_or_else(|| "My Kit".to_string(), |k| k.name.clone());
    let mut pads = Vec::new();
    let mut written = 0;
    for pad in 0..PADS {
        let layout = pad_layout(ins, pad);
        let mut samples = serde_json::Map::new();
        for layer in 0..LAYERS {
            for robin in 0..ROBINS {
                let Some(src) = ins.samples.get(&sample_key(pad, layer, robin)) else { continue };
                let Some(audio) = s.pool.get(*src) else { continue };
                let buf = audio.buffer();
                let bytes = soundcraft_audio_io::encode(
                    &buf,
                    &soundcraft_audio_io::EncodeOptions {
                        format: soundcraft_audio_io::FileFormat::Wav,
                        bit_depth: soundcraft_audio_io::BitDepth::Int24,
                        ..Default::default()
                    },
                )
                .map_err(|err| EngineError::Io(format!("pad {}: {err}", pad + 1)))?;
                let file = format!("p{}_v{}_r{}.wav", pad + 1, layer + 1, robin + 1);
                crate::io::write_file(&dir.join(&file).to_string_lossy(), &bytes)?;
                samples.insert(format!("v{}.r{}", layer + 1, robin + 1), json!(file));
                written += 1;
            }
        }
        let note =
            ins.params.get(&format!("p{}_note", pad + 1)).copied().or_else(|| JUKE_INFO.param(&format!("p{}_note", pad + 1)).map(|q| q.default));
        pads.push(json!({
            "name": layout.name,
            "family": layout.family.id(),
            "note": note.map(|n| n.round().clamp(0.0, 127.0) as u8),
            "samples": samples,
        }));
    }
    let fx: Vec<Value> = ins
        .kit_fx
        .iter()
        .map(|f| json!({"bus": f.bus.id(), "slot": usize::from(f.slot) + 1, "plugin": f.insert.plugin, "params": f.insert.params, "bypass": f.insert.bypass}))
        .collect();
    let kit = json!({"name": name, "license": "", "credit": "", "pads": pads, "params": ins.params, "fx": fx});
    let text = serde_json::to_string_pretty(&kit).map_err(|err| EngineError::Io(err.to_string()))?;
    crate::io::write_file(&dir.join("kit.json").to_string_lossy(), text.as_bytes())?;
    Ok(json!({"path": dir.to_string_lossy(), "samples": written, "name": name}))
}

fn kit_presets(_e: &mut Engine, _p: &Value) -> Result<Value> {
    let v: Vec<Value> = crate::juke_presets::builtin()
        .iter()
        .map(|q| json!({"name": q.name, "category": q.category, "about": q.about, "source": q.source, "needs": q.needs()}))
        .collect();
    Ok(json!({"presets": v}))
}

/// The recorded kits' folder: `kits_dir` when given, else the app's.
fn kits_dir_param(p: &Value, id: &str) -> Result<std::path::PathBuf> {
    match str_param(p, "kits_dir").map(str::trim).filter(|s| !s.is_empty()) {
        Some(d) => Ok(std::path::PathBuf::from(d)),
        None => crate::juke_presets::default_kits_dir().ok_or_else(|| bad(id, "recorded kits are not available here")),
    }
}

/// A recorded kit's folder, or why it can't be used.
fn recorded_dir(kits: &Path, kit: &str, id: &str) -> Result<std::path::PathBuf> {
    if !kit.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-') || kit.is_empty() {
        return Err(bad(id, format!("`{kit}` is not a recorded kit's id")));
    }
    let dir = kits.join(kit);
    crate::juke_kits::read_recorded(&dir)
        .map_err(|_| bad(id, format!("this preset needs the recorded kit `{kit}`: download it from Kits › Recorded & saved")))?;
    Ok(dir)
}

fn kit_preset(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_preset";
    let t = juke_track(e, id, p)?;
    let (preset, base): (crate::juke_presets::Preset, Option<std::path::PathBuf>) = match (str_param(p, "path"), str_param(p, "preset")) {
        (Some(path), _) => {
            let bytes = crate::io::read_file(path)?;
            let q = serde_json::from_slice(&bytes).map_err(|err| bad(id, format!("{path}: {err}")))?;
            (q, Path::new(path).parent().map(Path::to_path_buf))
        }
        (None, Some(name)) => {
            let q = crate::juke_presets::find(name).ok_or_else(|| bad(id, format!("no preset `{name}`: mix.kit_presets lists them")))?;
            (q.clone(), None)
        }
        (None, None) => return Err(bad(id, "`preset` (a name) or `path` (a preset file) required")),
    };
    let needs = preset.needs();
    let kits = if needs.is_empty() { None } else { Some(kits_dir_param(p, id)?) };
    let kit_dir = |k: &str| -> Result<std::path::PathBuf> {
        match &kits {
            Some(d) => recorded_dir(d, k, id),
            None => Err(bad(id, "recorded kits are not available here")),
        }
    };
    // 1. The source: a starter or style kit, a recorded kit, or Juke's defaults.
    if preset.source.is_empty() {
        let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
        let mut params: std::collections::BTreeMap<String, f32> = JUKE_INFO.params.iter().map(|q| (q.id.to_string(), q.default)).collect();
        for pad in 1..=PADS {
            for k in ["note", "mute", "solo"] {
                let key = format!("p{pad}_{k}");
                if let Some(v) = ins.params.get(&key) {
                    params.insert(key, *v);
                }
            }
        }
        ins.params = params;
        ins.samples.clear();
        ins.kit_fx.clear();
        ins.kit = Some(default_layout());
    } else if crate::juke_kits::find(&preset.source).is_some() {
        kit_load_plain(e, &json!({"track": t.0, "kit": preset.source}))?;
    } else {
        let dir = kit_dir(&preset.source)?;
        recorded_load(e, t, &dir)?;
    }
    // 2. Pads taken from other recorded kits.
    for (pad, b) in &preset.borrow {
        let to = pad.parse::<usize>().ok().filter(|n| (1..=PADS).contains(n)).ok_or_else(|| bad(id, format!("`borrow`: no pad {pad}")))? - 1;
        let dir = kit_dir(&b.kit)?;
        let kit = crate::juke_kits::read_recorded(&dir).map_err(|m| bad(id, m))?;
        let from = kit.pads.get(b.pad.wrapping_sub(1)).ok_or_else(|| bad(id, format!("`{}` has no pad {}", b.kit, b.pad)))?;
        if let Some(ins) = juke_mut(e, t) {
            ins.samples.retain(|k, _| !k.starts_with(&format!("p{}.", to + 1)));
        }
        for (key, file) in &from.samples {
            let Some((layer, robin)) = crate::juke_kits::layer_robin(key) else { continue };
            let path = dir.join(file);
            let bytes = crate::io::read_file(&path)?;
            let name = path.file_name().and_then(|x| x.to_str()).unwrap_or("Sample").to_string();
            attach_sample(e, t, (to, layer, robin), &name, &bytes, Some(&path.to_string_lossy()))?;
        }
        let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
        let n = to + 1;
        let layers =
            from.samples.keys().filter_map(|k| crate::juke_kits::layer_robin(k)).map(|(l, _)| l).collect::<std::collections::BTreeSet<_>>().len();
        ins.params.insert(format!("p{n}_source"), 0.0);
        ins.params.insert(format!("p{n}_velocity"), if layers > 1 { 30.0 } else { 75.0 });
        for (k, v) in [("decay", 5000.0), ("hold", 0.0), ("attack", 0.0), ("tune", 0.0), ("fine", 0.0), ("pitch_amount", 0.0), ("filter", 0.0)] {
            ins.params.insert(format!("p{n}_{k}"), v);
        }
        ins.params.insert(format!("p{n}_trim"), from.trim.unwrap_or(0.0).clamp(-24.0, 24.0));
        let layout = ins.kit.get_or_insert_with(default_layout);
        if let Some(slot) = layout.pads.get_mut(to) {
            slot.name = from.name.chars().take(32).collect();
            slot.family = from.family.as_deref().and_then(DrumFamily::from_id).unwrap_or(slot.family);
        }
    }
    // 3. A saved preset's samples and settings.
    if !preset.samples.is_empty() {
        if let Some(ins) = juke_mut(e, t) {
            ins.samples.clear();
        }
        for (key, src) in &preset.samples {
            let (pad, layer, robin) =
                soundcraft_dsp::plugins::juke::parse_sample_key(key).ok_or_else(|| bad(id, format!("`samples`: no slot {key}")))?;
            let path = if let Some(rest) = src.strip_prefix("kit:") {
                let (kit, file) = rest.split_once('/').ok_or_else(|| bad(id, format!("`samples`: {src}")))?;
                kit_dir(kit)?.join(file)
            } else if let Some(rest) = src.strip_prefix("file:") {
                base.clone().ok_or_else(|| bad(id, "a built-in preset names no files"))?.join(rest)
            } else {
                return Err(bad(id, format!("`samples`: `{src}` must start with kit: or file:")));
            };
            let bytes = crate::io::read_file(&path)?;
            let name = path.file_name().and_then(|x| x.to_str()).unwrap_or("Sample").to_string();
            attach_sample(e, t, (pad, layer, robin), &name, &bytes, Some(&path.to_string_lossy()))?;
        }
    }
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    for (k, v) in &preset.params {
        if let Some(q) = JUKE_INFO.param(k) {
            ins.params.insert(k.clone(), q.clamp(*v));
        }
    }
    // 4. Pad settings, from `all` to the pad, and the kit's.
    let mut layout = ins.kit.clone().unwrap_or_else(default_layout);
    for pad in 0..PADS {
        let family = layout.pads.get(pad).map_or(DrumFamily::Percussion, |x| x.family);
        for (k, v) in preset.pad_settings(pad, family) {
            match (k, v) {
                ("name", Value::String(n)) => {
                    if let Some(slot) = layout.pads.get_mut(pad) {
                        slot.name = n.chars().take(32).collect();
                    }
                }
                ("family", Value::String(f)) => {
                    if let (Some(slot), Some(f)) = (layout.pads.get_mut(pad), DrumFamily::from_id(f)) {
                        slot.family = f;
                    }
                }
                (k, v) => {
                    let key = format!("p{}_{k}", pad + 1);
                    let q = JUKE_INFO.param(&key).ok_or_else(|| bad(id, format!("`{}`: no pad setting `{k}`", preset.name)))?;
                    let x = v.as_f64().ok_or_else(|| bad(id, format!("`{}`: `{k}` must be a number", preset.name)))? as f32;
                    ins.params.insert(key, q.clamp(x));
                }
            }
        }
    }
    for (k, v) in &preset.kit {
        let q = JUKE_INFO.param(k).ok_or_else(|| bad(id, format!("`{}`: no kit setting `{k}`", preset.name)))?;
        ins.params.insert(k.clone(), q.clamp(*v));
    }
    layout.name = preset.name.chars().take(64).collect();
    ins.kit = Some(layout);
    // 5. The bus effects, in place of the source's.
    if let Some(fx) = &preset.fx {
        ins.kit_fx.clear();
        for f in fx {
            let mut q = f.clone();
            if let Some(o) = q.as_object_mut() {
                o.insert("track".into(), json!(t.0));
                kit_fx(e, &q)?;
            }
        }
    }
    Ok(json!({"preset": preset.name, "kit": preset.name}))
}

fn kit_preset_save(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_preset_save";
    let t = juke_track(e, id, p)?;
    let path = str_param(p, "path").map(str::trim).filter(|s| !s.is_empty()).ok_or_else(|| bad(id, "`path` required: the preset file"))?;
    let path = Path::new(path);
    let kits = kits_dir_param(p, id).ok();
    let s = e.session();
    let ins = s.track(t).and_then(|tr| tr.instrument.as_ref()).ok_or_else(|| bad(id, "no instrument"))?;
    let layout = ins.kit.clone().unwrap_or_else(default_layout);
    let name: String =
        str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map_or_else(|| layout.name.clone(), str::to_string).chars().take(64).collect();
    let stem = path.file_stem().and_then(|x| x.to_str()).unwrap_or("preset").to_string();
    let dir = path.parent().map(Path::to_path_buf).unwrap_or_default();
    let mut samples = std::collections::BTreeMap::new();
    let mut written = 0;
    for (key, src) in &ins.samples {
        let from = s.sources.iter().find(|x| x.id == *src).map(|x| Path::new(&x.path).to_path_buf());
        // A sample from an installed recorded kit is named, not copied.
        let in_kit = from.as_ref().zip(kits.as_ref()).and_then(|(f, k)| f.strip_prefix(k).ok().map(Path::to_path_buf));
        if let Some(rel) = in_kit.filter(|r| r.components().count() >= 2) {
            samples.insert(key.clone(), format!("kit:{}", rel.to_string_lossy().replace('\\', "/")));
            continue;
        }
        let Some(audio) = s.pool.get(*src) else { continue };
        let bytes = soundcraft_audio_io::encode(
            &audio.buffer(),
            &soundcraft_audio_io::EncodeOptions {
                format: soundcraft_audio_io::FileFormat::Wav,
                bit_depth: soundcraft_audio_io::BitDepth::Int24,
                ..Default::default()
            },
        )
        .map_err(|err| EngineError::Io(format!("{key}: {err}")))?;
        let rel = format!("{stem}_samples/{}.wav", key.replace('.', "_"));
        crate::io::make_dir(dir.join(format!("{stem}_samples")))?;
        crate::io::write_file(&dir.join(&rel).to_string_lossy(), &bytes)?;
        samples.insert(key.clone(), format!("file:{rel}"));
        written += 1;
    }
    let mut pads = std::collections::BTreeMap::new();
    for (i, pad) in layout.pads.iter().enumerate() {
        let mut m = std::collections::BTreeMap::new();
        m.insert("name".to_string(), json!(pad.name));
        m.insert("family".to_string(), json!(pad.family.id()));
        pads.insert((i + 1).to_string(), m);
    }
    let fx: Vec<Value> = ins
        .kit_fx
        .iter()
        .map(|f| json!({"bus": f.bus.id(), "slot": usize::from(f.slot) + 1, "plugin": f.insert.plugin, "params": f.insert.params, "bypass": f.insert.bypass}))
        .collect();
    let preset = crate::juke_presets::Preset {
        name: name.clone(),
        category: str_param(p, "category").map(str::trim).filter(|c| !c.is_empty()).unwrap_or("My Presets").to_string(),
        pads,
        fx: Some(fx),
        params: ins.params.clone(),
        samples,
        ..Default::default()
    };
    let text = serde_json::to_string_pretty(&preset).map_err(|err| EngineError::Io(err.to_string()))?;
    if !dir.as_os_str().is_empty() {
        crate::io::make_dir(&dir)?;
    }
    crate::io::write_file(&path.to_string_lossy(), text.as_bytes())?;
    Ok(json!({"path": path.to_string_lossy(), "name": name, "samples": written}))
}

fn kit_name(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_name";
    let t = juke_track(e, id, p)?;
    let name: String = str_param(p, "name")
        .map(|s| s.trim().chars().take(64).collect())
        .filter(|s: &String| !s.is_empty())
        .ok_or_else(|| bad(id, "`name` required"))?;
    let ins = juke_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not Juke"))?;
    ins.kit.get_or_insert_with(default_layout).name = name.clone();
    Ok(json!({"name": name}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-juke-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A generated click (no recorded or third-party audio) as a 24-bit WAV.
    fn click_wav(path: &Path, rate: u32) {
        let mut x = vec![0.0f32; rate as usize / 10];
        for (i, v) in x.iter_mut().enumerate().take(200) {
            *v = (1.0 - i as f32 / 200.0) * if i % 2 == 0 { 0.8 } else { -0.8 };
        }
        let buf = AudioBuffer { sample_rate: rate, channels: vec![x] };
        let bytes =
            soundcraft_audio_io::encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..Default::default() }).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    /// A generated take (no recorded audio): 200 ms of silence, then hits of the given
    /// amplitudes (decaying 150 Hz with a click), each 400 ms with 200 ms of silence after.
    fn take_wav(path: &Path, rate: u32, amps: &[f32]) {
        let sr = rate as f32;
        let mut x = vec![0.0f32; rate as usize / 5];
        for &a in amps {
            x.extend((0..(sr * 0.4) as usize).map(|i| {
                let t = i as f32 / sr;
                a * (-t / 0.06).exp() * (t * 150.0 * std::f32::consts::TAU).sin()
            }));
            x.extend(vec![0.0; rate as usize / 5]);
        }
        let buf = AudioBuffer { sample_rate: rate, channels: vec![x.clone(), x] };
        let bytes =
            soundcraft_audio_io::encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..Default::default() }).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    #[test]
    fn a_run_of_hits_becomes_a_pads_layers_and_round_robins_and_undoes() {
        let d = dir("pad-import");
        let wav = d.join("Snare_take 03.wav");
        // Sixteen hits, soft to hard and back, in one take.
        let amps: Vec<f32> = (0..16).map(|i| 0.03 * 1.25f32.powi((i * 5) % 16)).collect();
        take_wav(&wav, 44_100, &amps);
        let (mut e, t) = beat();
        let v = e.execute("mix.kit_pad_import", &json!({"track": t.0, "pad": 5, "paths": [wav.to_string_lossy()]})).unwrap();
        assert_eq!(v["hits"], 16, "{v}");
        assert_eq!(v["layers"], 4);
        assert_eq!(v["robins"], json!([4, 4, 4, 4]));
        assert_eq!(v["name"], "Snare");
        assert_eq!(v["family"], "snare");
        let ins = juke(&e, t);
        assert_eq!(ins.samples.keys().filter(|k| k.starts_with("p5.")).count(), 16);
        assert_eq!(ins.params.get("p5_source"), Some(&0.0));
        let trim = v["trim"].as_f64().unwrap() as f32;
        assert_eq!(ins.params.get("p5_trim"), Some(&trim));
        assert_eq!(pad_layout(&ins, 4).name, "Snare");
        // Each sample starts at its attack (no leading silence) and is at the session's rate.
        let src = ins.samples.get("p5.v4.r1").copied().unwrap();
        let audio = e.session().pool.get(src).unwrap().clone();
        let first = audio.channel(0).unwrap().iter().position(|v| v.abs() > 0.01).unwrap();
        assert!(first < 100, "attack at frame {first}");
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(!juke(&e, t).samples.keys().any(|k| k.starts_with("p5.")), "undo takes the import back");
        // A file with no hits refuses and changes nothing.
        let quiet = d.join("quiet.wav");
        take_wav(&quiet, 44_100, &[]);
        assert!(e.execute("mix.kit_pad_import", &json!({"track": t.0, "pad": 5, "paths": [quiet.to_string_lossy()]})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_selection_on_an_audio_track_becomes_a_pad() {
        let d = dir("pad-selection");
        let wav = d.join("Floor Tom.wav");
        take_wav(&wav, 48_000, &[0.1, 0.3, 0.9, 0.2, 0.6, 0.15]);
        let (mut e, t) = beat();
        e.execute("file.import_audio", &json!({"path": wav.to_string_lossy()})).unwrap();
        let audio = e.session().tracks.iter().find(|tr| tr.kind == TrackKind::Audio).map(|tr| tr.id).unwrap();
        // All six hits selected.
        let v = e.execute("mix.kit_pad_from_selection", &json!({"track": t.0, "pad": 8, "from": audio.0, "start": 0, "end": 48_000 * 4})).unwrap();
        assert_eq!(v["hits"], 6, "{v}");
        assert!(v["layers"].as_u64().unwrap() >= 2, "{v}");
        assert_eq!(v["name"], "Floor Tom");
        assert_eq!(v["family"], "tom");
        assert_eq!(juke(&e, t).samples.keys().filter(|k| k.starts_with("p8.")).count(), 6);
        // Just the first two hits (200 ms of silence, then 600 ms a hit).
        let v =
            e.execute("mix.kit_pad_from_selection", &json!({"track": t.0, "pad": 8, "from": audio.0, "start": 0, "end": 48_000 * 13 / 10})).unwrap();
        assert_eq!(v["hits"], 2, "{v}");
        // An empty or silent selection refuses.
        assert!(e.execute("mix.kit_pad_from_selection", &json!({"track": t.0, "pad": 8, "from": audio.0, "start": 0, "end": 0})).is_err());
        assert!(
            e.execute("mix.kit_pad_from_selection", &json!({"track": t.0, "pad": 8, "from": audio.0, "start": 48_000 * 60, "end": 48_000 * 61}))
                .is_err()
        );
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn takes_played_alike_are_round_robins_of_one_layer() {
        let d = dir("pad-alike");
        let paths: Vec<String> = (0..3)
            .map(|i| {
                let p = d.join(format!("kick {i}.wav"));
                take_wav(&p, 48_000, &[0.5 + 0.02 * i as f32]);
                p.to_string_lossy().into_owned()
            })
            .collect();
        let (mut e, t) = beat();
        let v = e.execute("mix.kit_pad_import", &json!({"track": t.0, "pad": 1, "paths": paths})).unwrap();
        assert_eq!((v["hits"].as_u64(), v["layers"].as_u64()), (Some(3), Some(1)), "{v}");
        assert_eq!(v["robins"], json!([3]));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_folder_of_takes_maps_to_pads_by_name() {
        let d = dir("kit-import").join("Garage Kit");
        std::fs::create_dir_all(d.join("Toms")).unwrap();
        for name in ["BD_01.wav", "snare-hard.wav", "HH closed.wav", "Open Hat.wav", "Crash 1.wav", "weird thing.wav", "notes.txt"] {
            if name.ends_with(".wav") {
                take_wav(&d.join(name), 48_000, &[0.4, 0.8]);
            } else {
                std::fs::write(d.join(name), "x").unwrap();
            }
        }
        take_wav(&d.join("Toms").join("floor.wav"), 48_000, &[0.6]);
        take_wav(&d.join("Toms").join("rack.wav"), 48_000, &[0.6]);
        let (mut e, t) = beat();
        let v = e.execute("mix.kit_import", &json!({"track": t.0, "folder": d.to_string_lossy()})).unwrap();
        let pads: Vec<(u64, String)> =
            v["pads"].as_array().unwrap().iter().map(|p| (p["pad"].as_u64().unwrap(), p["name"].as_str().unwrap().to_string())).collect();
        assert_eq!(v["kit"], "Garage Kit");
        let pad_of = |n: &str| pads.iter().find(|p| p.1.eq_ignore_ascii_case(n)).map(|p| p.0);
        assert_eq!(pad_of("BD"), Some(1), "{pads:?}");
        assert_eq!(pad_of("snare"), Some(3));
        assert_eq!(pad_of("HH closed"), Some(5));
        assert_eq!(pad_of("Open Hat"), Some(7));
        assert_eq!(pad_of("Crash"), Some(11));
        assert_eq!(pad_of("Toms floor"), Some(8));
        assert_eq!(pad_of("Toms rack"), Some(10));
        // The unknown one takes the first free pad (pad 2).
        assert_eq!(pad_of("weird thing"), Some(2));
        let ins = juke(&e, t);
        assert_eq!(ins.kit.as_ref().unwrap().name, "Garage Kit");
        assert_eq!(pad_layout(&ins, 10).family, DrumFamily::Cymbal);
        let _ = std::fs::remove_dir_all(d.parent().unwrap());
    }

    #[test]
    fn drum_names_point_to_their_pads() {
        for (name, pad) in [
            ("Kick In 02.wav", Some(0)),
            ("BassDrum.wav", Some(0)),
            ("sidestick.wav", Some(1)),
            ("SD_ghost.wav", Some(2)),
            ("clap.aif", Some(3)),
            ("hihat_closed.wav", Some(4)),
            ("HH pedal.wav", Some(5)),
            ("OH 01.wav", Some(6)),
            ("Tom 3.wav", Some(7)),
            ("Tom2.wav", Some(8)),
            ("Hi Tom.wav", Some(9)),
            ("Ride Bell.wav", Some(11)),
            ("tambourine.wav", Some(12)),
            ("shaker.wav", Some(14)),
            ("tom.wav", None),
            ("guitar.wav", None),
        ] {
            assert_eq!(guess_drum(name).0, pad, "{name}");
        }
        assert_eq!(pad_name("Snare_03_hard.wav"), "Snare");
        assert_eq!(pad_name("Kick In 02.wav"), "Kick In");
        assert_eq!(pad_name("01.wav"), "Pad");
    }

    fn beat() -> (Engine, TrackId) {
        let mut e = Engine::default();
        let v = e.execute("track.new_beat", &json!({})).unwrap();
        (e, TrackId(v["track"].as_u64().unwrap()))
    }

    fn juke(e: &Engine, t: TrackId) -> Insert {
        e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap()
    }

    #[test]
    fn a_new_beat_is_a_juke_track_with_a_pattern() {
        let mut e = Engine::default();
        let v = e.execute("track.new_beat", &json!({"name": "Groove", "bars": 4})).unwrap();
        let t = TrackId(v["track"].as_u64().unwrap());
        let tr = e.session().track(t).unwrap();
        assert_eq!(tr.name, "Groove");
        assert_eq!(tr.kind, TrackKind::Instrument);
        let ins = tr.instrument.as_ref().unwrap();
        assert_eq!(ins.plugin, JUKE);
        assert_eq!(ins.params.len(), soundcraft_dsp::plugin_info(JUKE).unwrap().params.len(), "every parameter at its default");
        assert_eq!(ins.kit.as_ref().map(|k| k.pads.len()), Some(PADS));
        let clip = tr.clips().first().unwrap();
        assert!(clip.pattern.is_some());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(t).is_none(), "one undo step takes it all back");
    }

    #[test]
    fn loading_layers_switches_the_pad_to_sample_and_saves_the_sources() {
        let d = dir("load");
        let wav = d.join("Big Snare.wav");
        click_wav(&wav, 44_100);
        let (mut e, t) = beat();
        let v = e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 3, "path": wav.to_string_lossy()})).unwrap();
        assert_eq!(v["key"], "p3.v1.r1");
        e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 3, "layer": 4, "robin": 2, "path": wav.to_string_lossy()})).unwrap();
        let ins = juke(&e, t);
        assert_eq!(ins.samples.keys().cloned().collect::<Vec<_>>(), vec!["p3.v1.r1".to_string(), "p3.v4.r2".to_string()]);
        assert_eq!(ins.params.get("p3_source"), Some(&0.0), "the synth pad now plays its sample");
        assert_eq!(pad_layout(&ins, 2).name, "Big Snare", "named after its first file");
        // The sources are in the session at its rate, and count as used.
        let src = *ins.samples.get("p3.v1.r1").unwrap();
        assert_eq!(e.session().source(src).map(|s| s.sample_rate), Some(e.session().sample_rate.hz()));
        assert!(ins.sources().any(|x| x == src));
        let cleared = e.execute("mix.kit_sample_clear", &json!({"track": t.0, "pad": 3, "layer": 4})).unwrap();
        assert_eq!(cleared["cleared"], 1);
        assert_eq!(juke(&e, t).samples.len(), 1);
        assert_eq!(e.execute("mix.kit_sample_clear", &json!({"track": t.0, "pad": 9})).unwrap()["cleared"], 0);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn pads_and_the_kit_take_names_and_families() {
        let (mut e, t) = beat();
        e.execute("mix.kit_pad", &json!({"track": t.0, "pad": 16, "name": "Tick", "family": "hat"})).unwrap();
        e.execute("mix.kit_name", &json!({"track": t.0, "name": "Back Porch"})).unwrap();
        let ins = juke(&e, t);
        assert_eq!(pad_layout(&ins, 15), KitPad { name: "Tick".into(), family: DrumFamily::Hat });
        assert_eq!(ins.kit.as_ref().map(|k| k.name.as_str()), Some("Back Porch"));
    }

    #[test]
    fn kit_commands_refuse_what_they_cannot_do() {
        let (mut e, t) = beat();
        assert!(e.execute("track.new", &json!({"count": 1, "kind": "instrument", "name": "Synth"})).is_ok());
        for (cmd, p) in [
            ("mix.kit_pad", json!({"track": "Synth", "pad": 1, "name": "x"})),
            ("mix.kit_pad", json!({"track": t.0, "pad": 0, "name": "x"})),
            ("mix.kit_pad", json!({"track": t.0, "pad": 17, "name": "x"})),
            ("mix.kit_pad", json!({"track": t.0, "pad": 1, "family": "bongo"})),
            ("mix.kit_pad", json!({"track": t.0, "pad": 1, "name": "  "})),
            ("mix.kit_sample_load", json!({"track": t.0, "pad": 1, "path": "/no/such/file.wav"})),
            ("mix.kit_sample_load", json!({"track": t.0, "pad": 1, "layer": 5, "path": "/x.wav"})),
            ("mix.kit_sample_clear", json!({"track": t.0, "pad": 1, "robin": 2})),
            ("mix.kit_name", json!({"track": t.0})),
            ("track.new_beat", json!({"bars": 0})),
        ] {
            assert!(e.execute(cmd, &p).is_err(), "{cmd} {p}");
        }
    }

    /// The peak of a bounce of the first bar.
    fn bounce_peak(e: &mut Engine, d: &Path, name: &str) -> f32 {
        let out = d.join(name);
        let bar = e.session().tempo.tick_to_samples(4 * soundcraft_midi::PPQ as i64, e.session().sample_rate);
        e.execute("file.bounce_mix", &json!({"path": out.to_string_lossy(), "start": 0, "end": bar, "bit_depth": "32f", "dither": false})).unwrap();
        let (_, buf) = soundcraft_audio_io::decode(&std::fs::read(&out).unwrap(), Some("wav")).unwrap();
        buf.peak()
    }

    #[test]
    fn a_pad_sample_plays_through_the_mix() {
        let d = dir("play");
        let wav = d.join("click.wav");
        click_wav(&wav, 48_000);
        let (mut e, t) = beat();
        let clip = e.session().track(t).unwrap().clips()[0].id;
        e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 1, "path": wav.to_string_lossy()})).unwrap();
        e.execute("pattern.step", &json!({"clip": clip.0, "lane": 0, "step": 0, "on": true, "velocity": 127})).unwrap();
        let with_sample = bounce_peak(&mut e, &d, "a.wav");
        assert!(with_sample > 0.05, "the kick pad plays its sample: {with_sample}");
        // A pad on Sample with its samples cleared is silent: the hand-off took the sample away too.
        e.execute("mix.kit_sample_clear", &json!({"track": t.0, "pad": 1})).unwrap();
        assert!(bounce_peak(&mut e, &d, "b.wav") < 1e-6, "no sample, no sound");
        // Back on its synth voice it sounds again.
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p1_source", "value": 1})).unwrap();
        assert!(bounce_peak(&mut e, &d, "c.wav") > 0.05);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn bus_effects_are_modules_that_play_undo_and_save() {
        let (mut e, t) = beat();
        let clip = e.session().track(t).unwrap().clips()[0].id;
        e.execute("pattern.step", &json!({"clip": clip.0, "lane": 0, "step": 0, "on": true, "velocity": 127})).unwrap();
        let d = dir("fx");
        let plain = bounce_peak(&mut e, &d, "plain.wav");
        // A gain of -12 dB on the output takes the whole kit down by a quarter.
        let v = e.execute("mix.kit_fx", &json!({"track": t.0, "bus": "out", "slot": 4, "plugin": "gain", "params": {"gain": -12.0412}})).unwrap();
        assert_eq!(v["plugin"], "gain");
        let module = soundcraft_model::ModuleId(u32::try_from(v["module"].as_u64().unwrap()).unwrap());
        assert_eq!(KitFx::place(module), Some((KitBus::Out, 3)));
        let quiet = bounce_peak(&mut e, &d, "quiet.wav");
        assert!((quiet / plain - 0.25).abs() < 0.02, "the output effect plays: {}", quiet / plain);
        // It is a module: the module commands edit it (the plugin window and full editors use them).
        e.execute("mix.module_param", &json!({"track": t.0, "module": module.0, "param": "gain", "value": 0.0})).unwrap();
        assert!((bounce_peak(&mut e, &d, "unity.wav") / plain - 1.0).abs() < 0.02);
        e.execute("mix.module_bypass", &json!({"track": t.0, "module": module.0})).unwrap();
        assert!(juke(&e, t).kit_effect(KitBus::Out, 3).is_some_and(|f| f.insert.bypass));
        // mix.kit_fx edits it too, and undo takes each step back.
        e.execute("mix.kit_fx", &json!({"track": t.0, "bus": "out", "slot": 4, "params": {"gain": 3.0}, "bypass": false})).unwrap();
        assert_eq!(juke(&e, t).kit_effect(KitBus::Out, 3).and_then(|f| f.insert.params.get("gain").copied()), Some(3.0));
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(juke(&e, t).kit_effect(KitBus::Out, 3).is_some_and(|f| f.insert.bypass));
        // A send bus with an effect, then saved and opened again.
        e.execute("mix.kit_fx", &json!({"track": t.0, "bus": "a", "slot": 1, "plugin": "fs_reverb"})).unwrap();
        let path = d.join("Song").join("Song.fdawm");
        e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
        let mut back = Engine::default();
        back.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        let tr = back.session().tracks.iter().find(|tr| tr.instrument.as_ref().is_some_and(|i| i.plugin == JUKE)).unwrap();
        let ins = tr.instrument.as_ref().unwrap();
        assert_eq!(ins.kit_effect(KitBus::A, 0).map(|f| f.insert.plugin.as_str()), Some("fs_reverb"));
        assert!(tr.module(KitFx::module_id(KitFx::index(KitBus::A, 0))).is_some());
        // Emptying a slot.
        e.execute("mix.kit_fx", &json!({"track": t.0, "bus": "a", "slot": 1, "plugin": ""})).unwrap();
        assert!(juke(&e, t).kit_effect(KitBus::A, 0).is_none());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn kit_effects_refuse_what_they_cannot_host() {
        let (mut e, t) = beat();
        for p in [
            json!({"track": t.0, "bus": "c", "slot": 1, "plugin": "gain"}),
            json!({"track": t.0, "bus": "a", "slot": 5, "plugin": "gain"}),
            json!({"track": t.0, "bus": "a", "slot": 1, "plugin": "juke"}),
            json!({"track": t.0, "bus": "a", "slot": 1, "plugin": "no_such_thing"}),
            json!({"track": t.0, "bus": "a", "slot": 1, "params": {"gain": 1.0}}),
            json!({"track": t.0, "bus": "a", "slot": 1, "plugin": "gain", "params": {"nope": 1.0}}),
        ] {
            assert!(e.execute("mix.kit_fx", &p).is_err(), "{p}");
        }
        assert!(juke(&e, t).kit_fx.is_empty());
    }

    #[test]
    fn starter_kits_load_render_and_undo() {
        let (mut e, t) = beat();
        let clip = e.session().track(t).unwrap().clips()[0].id;
        for s in [0, 8] {
            e.execute("pattern.step", &json!({"clip": clip.0, "lane": 0, "step": s, "on": true, "velocity": 127})).unwrap();
        }
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p1_note", "value": 35})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p1_note", "value": 36})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p12_mute", "value": 1})).unwrap();
        let kits = e.execute("mix.kit_list", &json!({})).unwrap();
        assert_eq!(kits["kits"].as_array().unwrap().len(), crate::juke_kits::KITS.len());
        let v = e.execute("mix.kit_load", &json!({"track": t.0, "kit": "Tin Roof"})).unwrap();
        let n = v["samples"].as_u64().unwrap();
        assert!(n >= 64, "every pad rendered at four velocities: {n}");
        let ins = juke(&e, t);
        assert_eq!(ins.kit.as_ref().unwrap().name, "Tin Roof");
        assert_eq!(ins.params.get("p1_source"), Some(&0.0), "rendered pads play samples");
        assert_eq!(ins.params.get("p12_mute"), Some(&1.0), "mutes stay");
        assert!(ins.samples.contains_key("p5.v4.r2"), "the hats get two takes");
        let d = dir("kits");
        assert!(bounce_peak(&mut e, &d, "tin.wav") > 0.05, "the rendered kick plays");
        // A synth kit with a bus effect, then undo back to the rendered one.
        e.execute("mix.kit_load", &json!({"track": t.0, "kit": "muddy water"})).unwrap();
        let ins = juke(&e, t);
        assert!(ins.samples.is_empty() && ins.kit_effect(KitBus::A, 0).is_some());
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(juke(&e, t).kit.as_ref().unwrap().name, "Tin Roof");
        assert!(e.execute("mix.kit_load", &json!({"track": t.0, "kit": "Nope"})).is_err());
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn a_recorded_kit_loads_from_its_folder() {
        let d = dir("recorded");
        std::fs::create_dir_all(d.join("kick")).unwrap();
        click_wav(&d.join("kick").join("v1r1.wav"), 48_000);
        click_wav(&d.join("kick").join("v2r1.wav"), 48_000);
        std::fs::write(
            d.join("kit.json"),
            r#"{"name":"Test Kit","license":"CC0","pads":[{"name":"Thud","family":"kick","note":36,"trim":-3.5,"samples":{"v1.r1":"kick/v1r1.wav","v2.r1":"kick/v2r1.wav"}}]}"#,
        )
        .unwrap();
        let (mut e, t) = beat();
        let clip = e.session().track(t).unwrap().clips()[0].id;
        e.execute("pattern.step", &json!({"clip": clip.0, "lane": 0, "step": 0, "on": true, "velocity": 127})).unwrap();
        let v = e.execute("mix.kit_load", &json!({"track": t.0, "path": d.to_string_lossy()})).unwrap();
        assert_eq!(v["samples"], 2);
        let ins = juke(&e, t);
        assert_eq!(pad_layout(&ins, 0).name, "Thud");
        assert_eq!(ins.kit.as_ref().unwrap().name, "Test Kit");
        assert!(ins.samples.contains_key("p1.v2.r1") && ins.params.get("p1_source") == Some(&0.0));
        assert_eq!(ins.params.get("p1_decay"), Some(&5000.0), "a recording plays whole");
        assert_eq!(ins.params.get("p1_trim"), Some(&-3.5), "the kit's balance");
        assert!(bounce_peak(&mut e, &d, "rec.wav") > 0.05, "the recorded kick plays");
        // A kit naming a missing file changes nothing.
        std::fs::write(d.join("kit.json"), r#"{"name":"Broken","pads":[{"name":"K","samples":{"v1.r1":"nope.wav"}}]}"#).unwrap();
        assert!(e.execute("mix.kit_load", &json!({"track": t.0, "path": d.to_string_lossy()})).is_err());
        assert_eq!(juke(&e, t).kit.as_ref().unwrap().name, "Test Kit");
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Checks a packed recorded kit before it is published: `FREEDAWM_TEST_KIT_ZIP=<kit>.zip cargo
    /// test -p soundcraft-engine --lib -- --ignored a_packed_kit_installs_and_plays`.
    #[test]
    #[ignore = "needs a packed kit zip (FREEDAWM_TEST_KIT_ZIP)"]
    fn a_packed_kit_installs_and_plays() {
        let zip = PathBuf::from(std::env::var_os("FREEDAWM_TEST_KIT_ZIP").expect("FREEDAWM_TEST_KIT_ZIP"));
        let id = zip.file_stem().and_then(|s| s.to_str()).unwrap().to_string();
        let d = dir("packed");
        let kits = d.join("kits");
        crate::ml::kits::install_kit_zip(&kits, &id, &zip).unwrap();
        let (mut e, t) = beat();
        let clip = e.session().track(t).unwrap().clips()[0].id;
        let v = e.execute("mix.kit_load", &json!({"track": t.0, "path": kits.join(&id).to_string_lossy()})).unwrap();
        let ins = juke(&e, t);
        println!("{}: {} samples; {}", v["kit"], v["samples"], v["license"]);
        for pad in 0..PADS {
            let layout = pad_layout(&ins, pad);
            let n = ins.samples.keys().filter(|k| k.starts_with(&format!("p{}.", pad + 1))).count();
            println!(
                "pad {:2}: {:12} {:?} note {} {n} samples",
                pad + 1,
                layout.name,
                layout.family,
                ins.params.get(&format!("p{}_note", pad + 1)).unwrap()
            );
            assert!(n > 0, "pad {} has samples", pad + 1);
        }
        // Every pad's note on the first steps of a lane, one after the other, then the mix.
        for pad in 0..PADS {
            let note = *ins.params.get(&format!("p{}_note", pad + 1)).unwrap();
            e.execute("pattern.lane", &json!({"clip": clip.0, "lane": 0, "note": note as u8})).unwrap();
            e.execute("pattern.step", &json!({"clip": clip.0, "lane": 0, "step": 0, "on": true, "velocity": 110})).unwrap();
            let p = bounce_peak(&mut e, &d, &format!("pad{pad}.wav"));
            println!("pad {:2} bounce peak {p:.3}", pad + 1);
            assert!(p > 0.01, "pad {} sounds", pad + 1);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn randomize_gives_the_pads_new_takes_and_undoes() {
        let (mut e, t) = beat();
        let before = juke(&e, t).params;
        e.execute("mix.kit_randomize", &json!({"track": t.0, "seed": 7})).unwrap();
        let after = juke(&e, t).params;
        assert_ne!(before, after);
        for pad in 1..=PADS {
            let tune = after.get(&format!("p{pad}_tune")).copied().unwrap();
            assert!((-3.0..=3.0).contains(&tune), "{tune}");
            assert!(after.get(&format!("p{pad}_decay")).is_some_and(|d| (10.0..=5000.0).contains(d)));
        }
        // The same seed, the same take; one pad leaves the others.
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(juke(&e, t).params, before);
        e.execute("mix.kit_randomize", &json!({"track": t.0, "seed": 7})).unwrap();
        assert_eq!(juke(&e, t).params, after);
        e.execute("edit.undo", &json!({})).unwrap();
        e.execute("mix.kit_randomize", &json!({"track": t.0, "pad": 3})).unwrap();
        let one = juke(&e, t).params;
        let tone = |m: &std::collections::BTreeMap<String, f32>, k: &str| m.get(k).copied().unwrap_or(50.0);
        assert_ne!((tone(&one, "p3_tone"), tone(&one, "p3_snappy")), (tone(&before, "p3_tone"), tone(&before, "p3_snappy")), "pad 3 took a new take");
        assert_eq!(one.get("p1_tone"), before.get("p1_tone"));
        assert!(e.execute("mix.kit_randomize", &json!({"track": t.0, "pad": 17})).is_err());
    }

    #[test]
    fn a_saved_kit_loads_back_with_its_samples_settings_and_effects() {
        let (mut e, t) = beat();
        let d = dir("kit-save");
        let wav = d.join("click.wav");
        click_wav(&wav, 48_000);
        e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 1, "path": wav.to_string_lossy(), "layer": 2, "robin": 3})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p2_tune", "value": 5.0})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p4_hum_timing", "value": 6.0})).unwrap();
        e.execute("mix.kit_fx", &json!({"track": t.0, "bus": "a", "slot": 2, "plugin": "gain", "params": {"gain": -3.0}})).unwrap();
        e.execute("mix.kit_pad", &json!({"track": t.0, "pad": 2, "name": "Rim Shot", "family": "snare"})).unwrap();
        e.execute("mix.kit_name", &json!({"track": t.0, "name": "Back Room"})).unwrap();
        let kit = d.join("back_room");
        let v = e.execute("mix.kit_save", &json!({"track": t.0, "path": kit.to_string_lossy()})).unwrap();
        assert_eq!(v["samples"], 1);
        assert!(kit.join("kit.json").is_file() && kit.join("p1_v2_r3.wav").is_file());
        let saved = juke(&e, t);
        // Onto a fresh beat: everything comes back.
        let (mut e2, t2) = beat();
        e2.execute("mix.kit_load", &json!({"track": t2.0, "path": kit.to_string_lossy()})).unwrap();
        let back = juke(&e2, t2);
        assert_eq!(back.kit.as_ref().map(|k| k.name.as_str()), Some("Back Room"));
        assert_eq!(pad_layout(&back, 1).name, "Rim Shot");
        assert!(back.samples.contains_key("p1.v2.r3"));
        for k in ["p2_tune", "p4_hum_timing", "p1_decay", "p1_source"] {
            assert_eq!(back.params.get(k), saved.params.get(k), "{k}");
        }
        assert_eq!(back.kit_effect(KitBus::A, 1).and_then(|f| f.insert.params.get("gain").copied()), Some(-3.0));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn every_character_lays_its_settings_and_effects_over_a_kit_in_one_undo_step() {
        for c in crate::juke_kits::CHARACTERS {
            let (mut e, t) = beat();
            let before = juke(&e, t);
            let v = e.execute("mix.kit_load", &json!({"track": t.0, "kit": "Muddy Water", "character": c.name.to_lowercase()})).unwrap();
            assert_eq!(v["character"], c.name);
            let ins = juke(&e, t);
            assert_eq!(ins.kit.as_ref().map(|k| k.name.clone()), Some(format!("Muddy Water · {}", c.name)));
            for (bus, slot, plugin, _) in c.fx {
                assert_eq!(ins.kit_effect(*bus, usize::from(*slot)).map(|f| f.insert.plugin.as_str()), Some(*plugin), "{}", c.name);
            }
            for (k, x) in c.kit {
                assert_eq!(ins.params.get(*k), Some(x), "{}", c.name);
            }
            e.execute("edit.undo", &json!({})).unwrap();
            let back = juke(&e, t);
            assert_eq!((back.params, back.kit, back.kit_fx.len()), (before.params.clone(), before.kit.clone(), before.kit_fx.len()), "{}", c.name);
        }
        // Tight shortens the drums, not the cymbals as much; an unknown character is refused.
        let (mut e, t) = beat();
        e.execute("mix.kit_load", &json!({"track": t.0, "kit": "Porch Light", "character": "Tight"})).unwrap();
        assert_eq!(juke(&e, t).params.get("p1_decay"), Some(&320.0));
        assert!(e.execute("mix.kit_load", &json!({"track": t.0, "kit": "Porch Light", "character": "Wobbly"})).is_err());
        let list = e.execute("mix.kit_list", &json!({})).unwrap();
        assert_eq!(list["characters"].as_array().map(Vec::len), Some(crate::juke_kits::CHARACTERS.len()));
    }

    /// Stand-ins for the three recorded kits: 16 pads, one click each (layer 2, take 1).
    fn fake_kits(name: &str) -> PathBuf {
        let d = dir(name);
        for (id, kit) in [("sm_drums", "SM Drums"), ("sm_drums_17", "SM Drums 17"), ("sm_drums_hybrid", "SM Drums Hybrid")] {
            let k = d.join(id);
            std::fs::create_dir_all(&k).unwrap();
            click_wav(&k.join("c.wav"), 44_100);
            let pads: Vec<Value> =
                (1..=PADS).map(|n| json!({"name": format!("{kit} {n}"), "family": "snare", "note": 35 + n, "samples": {"v2.r1": "c.wav"}})).collect();
            std::fs::write(k.join("kit.json"), json!({"name": kit, "pads": pads}).to_string()).unwrap();
        }
        d
    }

    #[test]
    fn every_built_in_preset_loads_its_whole_sound_in_one_undo_step() {
        let kits = fake_kits("presets");
        for preset in crate::juke_presets::builtin() {
            let (mut e, t) = beat();
            let before = juke(&e, t);
            e.execute("mix.kit_preset", &json!({"track": t.0, "preset": preset.name, "kits_dir": kits.to_string_lossy()}))
                .unwrap_or_else(|err| panic!("{}: {err}", preset.name));
            let ins = juke(&e, t);
            assert_eq!(ins.kit.as_ref().map(|k| k.name.as_str()), Some(preset.name.as_str()));
            assert_eq!(ins.kit_fx.len(), preset.fx.as_ref().map_or(0, Vec::len), "{}", preset.name);
            for (k, v) in &preset.kit {
                assert_eq!(ins.params.get(k), Some(v), "{}: {k}", preset.name);
            }
            for (pad, b) in &preset.borrow {
                let n: usize = pad.parse().unwrap();
                assert!(ins.samples.contains_key(&format!("p{n}.v2.r1")), "{}: pad {n} from {}", preset.name, b.kit);
            }
            e.execute("edit.undo", &json!({})).unwrap();
            let back = juke(&e, t);
            assert_eq!(
                (back.params, back.kit, back.samples.len()),
                (before.params.clone(), before.kit.clone(), before.samples.len()),
                "{}",
                preset.name
            );
        }
        // A family's setting reaches its pads: Arena drives the kick.
        let (mut e, t) = beat();
        e.execute("mix.kit_preset", &json!({"track": t.0, "preset": "analog 808"})).unwrap();
        assert_eq!(juke(&e, t).params.get("p1_decay"), Some(&1200.0));
        // Without the recorded kit it needs, it says what to download.
        let empty = dir("presets-empty");
        let err =
            e.execute("mix.kit_preset", &json!({"track": t.0, "preset": "Arena", "kits_dir": empty.to_string_lossy()})).unwrap_err().to_string();
        assert!(err.contains("sm_drums") && err.contains("download"), "{err}");
        assert!(e.execute("mix.kit_preset", &json!({"track": t.0, "preset": "No Such"})).is_err());
        let list = e.execute("mix.kit_presets", &json!({})).unwrap();
        assert_eq!(list["presets"].as_array().map(Vec::len), Some(crate::juke_presets::builtin().len()));
        let _ = std::fs::remove_dir_all(&kits);
        let _ = std::fs::remove_dir_all(&empty);
    }

    #[test]
    fn a_saved_preset_names_kit_samples_copies_its_own_and_loads_back() {
        let kits = fake_kits("preset-save");
        let (mut e, t) = beat();
        let k = kits.to_string_lossy().to_string();
        e.execute("mix.kit_preset", &json!({"track": t.0, "preset": "Mixed Kit", "kits_dir": k})).unwrap();
        let own = kits.join("own.wav");
        click_wav(&own, 48_000);
        e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 1, "path": own.to_string_lossy()})).unwrap();
        e.execute("mix.instrument_param", &json!({"track": t.0, "param": "p9_tune", "value": -4.0})).unwrap();
        let file = kits.join("presets").join("My Wood.json");
        let v = e.execute("mix.kit_preset_save", &json!({"track": t.0, "path": file.to_string_lossy(), "name": "My Wood", "kits_dir": k})).unwrap();
        assert_eq!(v["samples"], 1, "only the pad's own sample is copied");
        let text = std::fs::read_to_string(&file).unwrap();
        assert!(text.contains("kit:sm_drums/c.wav") && text.contains("file:My Wood_samples/p1_v1_r1.wav"), "{text}");
        let saved = juke(&e, t);
        let (mut e2, t2) = beat();
        e2.execute("mix.kit_preset", &json!({"track": t2.0, "path": file.to_string_lossy(), "kits_dir": k})).unwrap();
        let back = juke(&e2, t2);
        assert_eq!(back.params, saved.params);
        assert_eq!(back.samples.keys().collect::<Vec<_>>(), saved.samples.keys().collect::<Vec<_>>());
        assert_eq!(back.kit.as_ref().map(|x| x.name.as_str()), Some("My Wood"));
        assert_eq!(back.kit_fx.len(), saved.kit_fx.len());
        assert_eq!(crate::cmd::juke::pad_layout(&back, 2).name, crate::cmd::juke::pad_layout(&saved, 2).name);
        let _ = std::fs::remove_dir_all(&kits);
    }

    /// The library's loudness: the test groove's gated loudness (dB) and its highest peak (dBFS).
    /// `tools/kits/juke_presets.py --levels` sets each preset's kit level to meet them.
    const PRESET_TARGET_DB: f32 = -26.0;
    const PRESET_PEAK_MAX_DB: f32 = -1.0;

    /// One preset's measurement.
    struct Measured {
        peak_db: f32,
        loud_db: f32,
        /// Left over right, dB.
        balance_db: f32,
        /// High-frequency share: the first difference's RMS over the signal's, dB.
        bright_db: f32,
        level: f32,
        wav: Vec<u8>,
    }

    /// Loads `preset` on a new beat and renders its demo groove (four bars, then a bar of tail);
    /// without one, a fixed two-bar groove: kick, snare with a ghost note, eighth hats, an open hat,
    /// crash, ride and a tom fill.
    fn measure_preset(preset: &crate::juke_presets::Preset, kits: Option<&Path>, d: &Path) -> Measured {
        let (mut e, t) = beat();
        let mut p = json!({"track": t.0, "preset": preset.name});
        if let Some(k) = kits {
            p["kits_dir"] = json!(k.to_string_lossy());
        }
        e.execute("mix.kit_preset", &p).unwrap_or_else(|err| panic!("{}: {err}", preset.name));
        let s = e.session();
        let bar = s.tempo.tick_to_samples(4 * soundcraft_midi::PPQ as i64, s.sample_rate);
        let bars = if preset.demo.is_some() { 4 } else { 2 };
        let v = e.execute("pattern.new", &json!({"track": t.0, "bars": bars, "start": 4 * bar, "lanes": [36, 38, 42, 46, 49, 51, 50, 41]})).unwrap();
        let clip = v["clip"].as_u64().unwrap();
        if let Some(demo) = &preset.demo {
            let mut g = json!({"clip": clip, "style": demo.style});
            if !demo.groove.is_empty() {
                g["groove"] = json!(demo.groove);
            }
            e.execute("pattern.groove", &g).unwrap_or_else(|err| panic!("{}: {err}", preset.name));
        } else {
            for lane in 0..8 {
                e.execute("pattern.lane", &json!({"clip": clip, "lane": lane, "length": 32})).unwrap();
            }
            let mut hit = |lane: usize, step: usize, vel: u8| {
                e.execute("pattern.step", &json!({"clip": clip, "lane": lane, "step": step, "on": true, "velocity": vel})).unwrap();
            };
            for s in [0, 6, 8, 16, 22, 24] {
                hit(0, s, 115);
            }
            for s in [4, 12, 20, 28] {
                hit(1, s, 112);
            }
            hit(1, 15, 38);
            for s in (0..32).step_by(2).filter(|s| *s != 14 && *s < 24) {
                hit(2, s, if s % 4 == 0 { 92 } else { 64 });
            }
            hit(3, 14, 90);
            hit(4, 0, 110);
            for s in [24, 26] {
                hit(5, s, 85);
            }
            for s in [28, 29] {
                hit(6, s, 100);
            }
            for s in [30, 31] {
                hit(7, s, 105);
            }
        }
        let level = juke(&e, t).params.get("level").copied().unwrap_or(-6.0);
        let out = d.join(format!("{}.wav", preset.name.replace(['/', ' ', '\'', '&'], "_")));
        e.execute(
            "file.bounce_mix",
            &json!({"path": out.to_string_lossy(), "start": 4 * bar, "end": (5 + bars) * bar, "bit_depth": "32f", "dither": false}),
        )
        .unwrap();
        let wav = std::fs::read(&out).unwrap();
        let _ = std::fs::remove_file(&out);
        let (_, buf) = soundcraft_audio_io::decode(&wav, Some("wav")).unwrap();
        let ch = |i: usize| buf.channels.get(i).or_else(|| buf.channels.first()).cloned().unwrap_or_default();
        let (l, r) = (ch(0), ch(1));
        let db = |x: f64| (10.0 * x.max(1e-12).log10()) as f32;
        let ms = |x: &[f32]| x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64;
        let mono: Vec<f32> = l.iter().zip(&r).map(|(a, b)| 0.5 * (a + b)).collect();
        let diff: Vec<f32> = mono.windows(2).map(|w| w[1] - w[0]).collect();
        // Gated loudness, as a meter hears it: 100 ms blocks, the near-silent ones (more than
        // 15 dB under the rest) left out, so short hits and the gaps between them don't read
        // quiet.
        let block = (buf.sample_rate as usize / 10).max(1);
        let blocks: Vec<f64> = mono.chunks(block).map(ms).filter(|m| *m > 1e-7).collect();
        let mean = |v: &[f64]| v.iter().sum::<f64>() / v.len().max(1) as f64;
        let gate = mean(&blocks) * 10f64.powf(-1.5);
        let loud: Vec<f64> = blocks.iter().copied().filter(|m| *m >= gate).collect();
        Measured {
            peak_db: 20.0 * buf.peak().max(1e-9).log10(),
            loud_db: db(mean(&loud)),
            balance_db: db(ms(&l)) - db(ms(&r)),
            bright_db: db(ms(&diff)) - db(ms(&mono)),
            level,
            wav,
        }
    }

    #[test]
    fn presets_without_recorded_kits_play_at_the_librarys_level() {
        let d = dir("preset-levels");
        for q in crate::juke_presets::builtin().iter().filter(|q| q.needs().is_empty()) {
            let m = measure_preset(q, None, &d);
            assert!(m.peak_db <= PRESET_PEAK_MAX_DB + 1.0, "{}: peaks at {:.1} dBFS", q.name, m.peak_db);
            assert!(
                (m.loud_db - PRESET_TARGET_DB).abs() <= 2.0 || m.peak_db >= PRESET_PEAK_MAX_DB - 0.5,
                "{}: plays at {:.1} dB, the library at {PRESET_TARGET_DB} (run tools/kits/juke_presets.py --levels)",
                q.name,
                m.loud_db
            );
            assert!(m.balance_db.abs() <= 4.0, "{}: leans {:.1} dB", q.name, m.balance_db);
        }
        let _ = std::fs::remove_dir_all(&d);
    }

    /// Renders and measures every preset (with the recorded kits in `FREEDAWM_TEST_KITS_DIR`) and
    /// writes `report.json` (and each render, with `FREEDAWM_PRESET_WAVS=1`) to
    /// `FREEDAWM_PRESET_REPORT`, for `tools/kits/juke_presets.py --levels`.
    #[test]
    #[ignore = "needs the recorded kits (FREEDAWM_TEST_KITS_DIR) and FREEDAWM_PRESET_REPORT"]
    fn measure_every_preset() {
        let kits = std::env::var_os("FREEDAWM_TEST_KITS_DIR").map(PathBuf::from);
        let out = PathBuf::from(std::env::var_os("FREEDAWM_PRESET_REPORT").expect("FREEDAWM_PRESET_REPORT: where to write"));
        std::fs::create_dir_all(&out).unwrap();
        let wavs = std::env::var_os("FREEDAWM_PRESET_WAVS").is_some();
        let d = dir("preset-measure");
        let mut report = Vec::new();
        let only = std::env::var("FREEDAWM_PRESET_ONLY").ok();
        for q in crate::juke_presets::builtin() {
            if kits.is_none() && !q.needs().is_empty() || only.as_ref().is_some_and(|o| !o.split(',').any(|n| n == q.name)) {
                continue;
            }
            let m = measure_preset(q, kits.as_deref(), &d);
            println!(
                "{:28} peak {:6.1}  loud {:6.1}  bal {:5.1}  bright {:6.1}  level {:5.1}",
                q.name, m.peak_db, m.loud_db, m.balance_db, m.bright_db, m.level
            );
            if wavs {
                std::fs::write(out.join(format!("{}.wav", q.name.replace(['/', ' ', '\'', '&'], "_"))), &m.wav).unwrap();
            }
            report.push(json!({"name": q.name, "category": q.category, "peak_db": m.peak_db, "loud_db": m.loud_db, "balance_db": m.balance_db, "bright_db": m.bright_db, "level": m.level}));
        }
        std::fs::write(out.join("report.json"), serde_json::to_string_pretty(&report).unwrap()).unwrap();
    }

    #[test]
    fn styles_load_their_kit_and_groove_in_one_undo_step() {
        let mut e = Engine::default();
        let v = e.execute("track.new_beat", &json!({"style": "Worship", "groove": "Ballad"})).unwrap();
        let t = TrackId(v["track"].as_u64().unwrap());
        let clip = soundcraft_model::ClipId(v["clip"].as_u64().unwrap());
        assert_eq!(juke(&e, t).kit.as_ref().unwrap().name, "Worship");
        assert!(juke(&e, t).kit_effect(KitBus::A, 0).is_some(), "the Worship kit's reverb");
        let lanes = |e: &Engine| e.session().find_clip(clip).and_then(|(_, c)| c.pattern.clone()).unwrap().pattern.lanes;
        assert_eq!(lanes(&e).len(), 3, "Ballad: kick, snare, hat");
        // Another style's groove on the same clip, then undo.
        let g = e.execute("pattern.groove", &json!({"track": t.0, "style": "Metal", "groove": 3})).unwrap();
        assert_eq!(g["groove"], "Blast Beat");
        assert_eq!(g["clip"], clip.0, "the track's pattern clip at the insertion point");
        assert_eq!(lanes(&e).len(), 3);
        assert!(lanes(&e)[1].steps.iter().filter(|s| s.on).count() == 8, "blast beat snare");
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(lanes(&e)[0].steps.iter().filter(|s| s.on).count(), 2, "the Ballad kick is back");
        // The bars play: a bounce of the groove is not silent.
        let d = dir("styles");
        assert!(bounce_peak(&mut e, &d, "worship.wav") > 0.01);
        let _ = std::fs::remove_dir_all(&d);
        // Refusals.
        assert!(e.execute("pattern.groove", &json!({"track": t.0, "style": "Polka"})).is_err());
        assert!(e.execute("pattern.groove", &json!({"track": t.0, "style": "Metal", "groove": 9})).is_err());
        assert!(e.execute("track.new_beat", &json!({"style": "Polka"})).is_err());
        let list = e.execute("pattern.grooves", &json!({})).unwrap();
        assert_eq!(list["styles"].as_array().unwrap().len(), crate::juke_styles::STYLES.len());
    }

    #[test]
    fn every_style_loads_and_renders() {
        for s in crate::juke_styles::STYLES {
            let mut e = Engine::default();
            let v = e.execute("track.new_beat", &json!({"style": s.name})).unwrap_or_else(|err| panic!("{}: {err}", s.name));
            let t = TrackId(v["track"].as_u64().unwrap());
            assert_eq!(juke(&e, t).kit.as_ref().unwrap().name, s.kit, "{}", s.name);
        }
    }

    #[test]
    fn a_kit_survives_save_and_reopen() {
        let d = dir("save");
        let wav = d.join("kick.wav");
        click_wav(&wav, 48_000);
        let (mut e, t) = beat();
        e.execute("mix.kit_sample_load", &json!({"track": t.0, "pad": 1, "path": wav.to_string_lossy()})).unwrap();
        e.execute("mix.kit_pad", &json!({"track": t.0, "pad": 1, "family": "kick", "name": "Thump"})).unwrap();
        let path = d.join("Song").join("Song.fdawm");
        e.execute("session.save_as", &json!({"path": path.to_string_lossy()})).unwrap();
        let mut back = Engine::default();
        back.execute("session.open", &json!({"path": path.to_string_lossy()})).unwrap();
        let tr = back.session().tracks.iter().find(|tr| tr.instrument.as_ref().is_some_and(|i| i.plugin == JUKE)).unwrap();
        let ins = tr.instrument.as_ref().unwrap();
        let src = *ins.samples.get("p1.v1.r1").unwrap();
        assert!(
            back.session().pool.get(src).is_some_and(|a| a.in_memory().is_some()),
            "a pad's sample is loaded whole, so the instrument can read it"
        );
        assert_eq!(pad_layout(ins, 0).name, "Thump");
        let _ = std::fs::remove_dir_all(&d);
    }
}

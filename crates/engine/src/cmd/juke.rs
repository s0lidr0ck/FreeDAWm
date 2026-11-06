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
use soundcraft_dsp::plugins::juke::{LAYERS, PADS, ROBINS, sample_key};
use soundcraft_model::{DrumFamily, Insert, KitLayout, KitPad, Route, TrackKind};
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
            "{name?: 'Beat', start?, bars?: 1-64 (default 2)} -> {track, clip}: an instrument track playing Juke (the default kit) with a first pattern clip",
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
            "mix.kit_pad",
            "Edit Pad",
            [],
            None,
            "{track?, pad: 1-16, name?, family?: kick|snare|clap|hat|tom|cymbal|percussion} -> {pad}",
            has_selection,
            pad_edit
        ),
        cmd!("mix.kit_name", "Rename Kit", [], None, "{track?, name} -> {name}", has_selection, kit_name),
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
    Ok(json!({"track": t, "clip": clip.0}))
}

fn sample_load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.kit_sample_load";
    let t = juke_track(e, id, p)?;
    let pad = pad_param(p, id)?;
    let layer = slot_param(p, "layer", LAYERS, id)?.unwrap_or(0);
    let robin = slot_param(p, "robin", ROBINS, id)?.unwrap_or(0);
    let path = str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?.to_string();
    // Decode before touching the session: a bad file changes nothing.
    let bytes = crate::io::read_file(&path)?;
    let name = Path::new(&path).file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
    let ext = Path::new(&name).extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
    let (format, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
    if buf.frames() == 0 || buf.num_channels() == 0 {
        return Err(EngineError::Io(format!("{name}: no audio")));
    }
    let buf = crate::io::at_rate(buf, e.session().sample_rate.hz(), MAX_PAD_SAMPLE_VALUES)
        .ok_or_else(|| EngineError::Io(format!("{name}: too long for a pad (a minute of 192 kHz stereo at most)")))?;
    let key = sample_key(pad, layer, robin);
    let s = e.session_mut();
    let src = crate::io::add_source(s, &name, buf, Some(&path), format.format);
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

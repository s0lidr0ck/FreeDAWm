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
        cmd!(query "mix.kit_list", "List Kits", [], None, "{} -> {kits: [{name, rendered}]}: Juke's starter kits", always, kit_list),
        cmd!(
            "mix.kit_load",
            "Load Kit",
            [],
            None,
            "{track?, kit: name | path: an installed recorded kit's folder} -> {kit, samples}: a starter kit (or a recorded kit downloaded from the kits release) on the Juke track: its sounds, names and bus effects; the pads keep their notes (unless a recorded kit gives them), mutes and solos. A rendered kit's pads play samples made from their synth voices",
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
    Ok(json!({"kits": kits}))
}

fn kit_load(e: &mut Engine, p: &Value) -> Result<Value> {
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
    for (i, pad) in kit.pads.iter().enumerate() {
        if let Some(n) = pad.note {
            params.insert(format!("p{}_note", i + 1), f32::from(n.min(127)));
        }
        if let Some(t) = pad.trim {
            params.insert(format!("p{}_trim", i + 1), t.clamp(-24.0, 24.0));
        }
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
        // tuning or pitch sweep the pad had by default.
        if layers > 0 {
            let n = i + 1;
            ins.params.insert(format!("p{n}_velocity"), if layers > 1 { 30.0 } else { 75.0 });
            for (k, v) in [("decay", 5000.0), ("hold", 0.0), ("attack", 0.0), ("tune", 0.0), ("fine", 0.0), ("pitch_amount", 0.0), ("filter", 0.0)] {
                ins.params.insert(format!("p{n}_{k}"), v);
            }
        }
    }
    ins.kit = Some(layout);
    Ok(json!({"kit": kit.name, "samples": n, "license": kit.license, "credit": kit.credit}))
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

//! The Multisampler: instruments recorded note by note on instrument tracks.
//!
//! `mix.multisample_load` reads an SFZ file's key-down regions into the track's Multisampler.
//! Each sample is a session source (`Insert::samples`, keyed `n{note}.v{velocity}.r{robin}`: the
//! note it was recorded at, the top of its velocity range and its round-robin take), saved in
//! `Audio Files/` with the session; the mix engine hands each to the running instrument. Of an
//! SFZ file only the mapping is read (`sample`, `pitch_keycenter` or `key`, `hivel`,
//! `seq_position`, `default_path`, with `<global>`, `<master>` and `<group>` inherited): the
//! instrument's own parameters shape the sound.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_dsp::plugins::multisampler::{LAYERS, NOTES, ROBINS, sample_key};
use soundcraft_model::Insert;
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// The Multisampler's plugin id.
pub const MULTISAMPLER: &str = "multisampler";

/// Largest SFZ file read (bytes): mappings are text, a few hundred kilobytes at most.
const MAX_SFZ_BYTES: usize = 8 << 20;
/// Largest single sample (frames × channels): two minutes of 192 kHz stereo.
const MAX_SAMPLE_VALUES: usize = 192_000 * 120 * 2;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        "mix.multisample_load",
        "Load Multisample",
        [],
        None,
        "{track?, path: an .sfz file} -> {track, name, samples, notes, layers, skipped}: the track's Multisampler plays the file's samples in place of its own (the mapping only: recorded note, velocity layer and round-robin of each key-down region; at most 16 layers a note and 4 round-robins a layer, the rest skipped)",
        has_selection,
        load
    )]
}

/// A key-down region of an SFZ file.
#[derive(Debug, Clone, PartialEq)]
struct Region {
    /// The sample's path as the file gives it (after `default_path`), with `/` separators.
    sample: String,
    note: u8,
    velocity: u8,
    /// Round-robin take, from 1.
    robin: usize,
}

/// A note as SFZ writes it: a number, or a name (`c4` is 60).
fn sfz_note(text: &str) -> Option<u8> {
    let t = text.trim().to_ascii_lowercase();
    if let Ok(n) = t.parse::<i64>() {
        return u8::try_from(n).ok().filter(|n| *n <= 127);
    }
    let mut chars = t.chars();
    let mut n: i64 = match chars.next()? {
        'c' => 0,
        'd' => 2,
        'e' => 4,
        'f' => 5,
        'g' => 7,
        'a' => 9,
        'b' => 11,
        _ => return None,
    };
    let rest = chars.as_str();
    let rest = match rest.chars().next()? {
        '#' => {
            n += 1;
            rest.get(1..)?
        }
        'b' => {
            n -= 1;
            rest.get(1..)?
        }
        _ => rest,
    };
    let octave: i64 = rest.parse().ok()?;
    u8::try_from(n.checked_add(octave.checked_add(1)?.checked_mul(12)?)?).ok().filter(|n| *n <= 127)
}

/// The key-down regions of an SFZ file that name a sample file, in file order.
fn parse_sfz(text: &str) -> Vec<Region> {
    type Opcodes = BTreeMap<String, String>;
    let (mut control, mut global, mut master, mut group, mut region) =
        (Opcodes::new(), Opcodes::new(), Opcodes::new(), Opcodes::new(), Opcodes::new());
    let mut out = Vec::new();
    let mut scope = String::new();
    // The opcode a value's later words (a file name with spaces) belong to.
    let mut open: Option<String> = None;
    let finish = |region: &mut Opcodes, control: &Opcodes, global: &Opcodes, master: &Opcodes, group: &Opcodes, out: &mut Vec<Region>| {
        let get = |k: &str| region.get(k).or_else(|| group.get(k)).or_else(|| master.get(k)).or_else(|| global.get(k)).map(String::as_str);
        let sample = get("sample").map(|s| s.trim().replace('\\', "/")).filter(|s| !s.is_empty() && !s.starts_with('*'));
        let key_down = get("trigger").is_none_or(|t| t.trim().eq_ignore_ascii_case("attack"));
        if let Some(sample) = sample.filter(|_| key_down) {
            let dir = control.get("default_path").map(|d| d.trim().replace('\\', "/")).unwrap_or_default();
            let note = get("pitch_keycenter").or_else(|| get("key")).and_then(sfz_note).unwrap_or(60);
            let velocity = get("hivel").and_then(|v| v.trim().parse::<i64>().ok()).map_or(127, |v| v.clamp(1, 127) as u8);
            let robin = get("seq_position").and_then(|v| v.trim().parse::<usize>().ok()).unwrap_or(1).max(1);
            out.push(Region { sample: format!("{dir}{sample}"), note, velocity, robin });
        }
        region.clear();
    };
    for line in text.lines() {
        let line = line.split("//").next().unwrap_or("");
        for word in line.split_whitespace() {
            if let Some(header) = word.strip_prefix('<').and_then(|w| w.strip_suffix('>')) {
                if scope == "region" {
                    finish(&mut region, &control, &global, &master, &group, &mut out);
                }
                scope = header.to_ascii_lowercase();
                open = None;
                match scope.as_str() {
                    "global" => {
                        global.clear();
                        master.clear();
                        group.clear();
                    }
                    "master" => {
                        master.clear();
                        group.clear();
                    }
                    "group" => group.clear(),
                    _ => {}
                }
                continue;
            }
            let target = match scope.as_str() {
                "control" => &mut control,
                "global" => &mut global,
                "master" => &mut master,
                "group" => &mut group,
                "region" => &mut region,
                _ => continue,
            };
            match word.split_once('=') {
                Some((k, v)) if !k.is_empty() => {
                    let k = k.to_ascii_lowercase();
                    target.insert(k.clone(), v.to_string());
                    open = Some(k);
                }
                _ => {
                    if let Some(v) = open.as_ref().and_then(|k| target.get_mut(k)) {
                        v.push(' ');
                        v.push_str(word);
                    }
                }
            }
        }
    }
    if scope == "region" {
        finish(&mut region, &control, &global, &master, &group, &mut out);
    }
    out
}

fn multisampler_mut(e: &mut Engine, t: TrackId) -> Option<&mut Insert> {
    e.session_mut().track_mut(t).and_then(|tr| tr.instrument.as_mut()).filter(|i| i.plugin == MULTISAMPLER)
}

fn load(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "mix.multisample_load";
    let t = tracks_required(e, id, p)?.first().copied().ok_or_else(|| bad(id, "no track"))?;
    if multisampler_mut(e, t).is_none() {
        return Err(bad(id, "the track's instrument is not the Multisampler"));
    }
    let path = Path::new(str_param(p, "path").ok_or_else(|| bad(id, "`path` required"))?).to_path_buf();
    let bytes = crate::io::read_file(&path)?;
    if bytes.len() > MAX_SFZ_BYTES {
        return Err(EngineError::Io(format!("{}: too large for an SFZ file", path.display())));
    }
    let regions = parse_sfz(&String::from_utf8_lossy(&bytes));
    if regions.is_empty() {
        return Err(EngineError::Io(format!("{}: no samples to play (an SFZ file's key-down regions)", path.display())));
    }
    let dir = path.parent().unwrap_or(Path::new("."));
    // Where each region goes: a note keeps 16 velocities and a layer 4 takes; a second region for
    // a taken place is the layer's next take.
    let mut placed: BTreeMap<String, &Region> = BTreeMap::new();
    let mut layers: BTreeMap<u8, BTreeSet<u8>> = BTreeMap::new();
    let mut skipped = 0usize;
    for r in &regions {
        let of_note = layers.entry(r.note).or_default();
        if !of_note.contains(&r.velocity) && of_note.len() >= LAYERS {
            skipped += 1;
            continue;
        }
        let free = (r.robin..=ROBINS).map(|robin| sample_key(r.note, r.velocity, robin)).find(|k| !placed.contains_key(k));
        match free {
            Some(key) if placed.len() < NOTES * LAYERS * ROBINS => {
                of_note.insert(r.velocity);
                placed.insert(key, r);
            }
            _ => skipped += 1,
        }
    }
    if let Some(ins) = multisampler_mut(e, t) {
        ins.samples.clear();
    }
    // A missing or broken file fails the command, which leaves the session as it was.
    let rate = e.session().sample_rate.hz();
    let mut total = 0usize;
    for (key, r) in &placed {
        let file = dir.join(&r.sample);
        let name = file.file_name().and_then(|n| n.to_str()).unwrap_or("Sample").to_string();
        let ext = file.extension().and_then(|x| x.to_str()).map(str::to_ascii_lowercase);
        let bytes = crate::io::read_file(&file)?;
        let (format, buf) = soundcraft_audio_io::decode(&bytes, ext.as_deref()).map_err(|err| EngineError::Io(format!("{name}: {err}")))?;
        if buf.frames() == 0 || buf.num_channels() == 0 {
            return Err(EngineError::Io(format!("{name}: no audio")));
        }
        let buf = crate::io::at_rate(buf, rate, MAX_SAMPLE_VALUES)
            .ok_or_else(|| EngineError::Io(format!("{name}: too long for one note (two minutes at most)")))?;
        total = total.saturating_add(buf.frames().saturating_mul(buf.num_channels()));
        if total > crate::io::MAX_IMPORT_SAMPLES {
            return Err(EngineError::Io(format!("{}: too much audio to hold in memory", path.display())));
        }
        let src = crate::io::add_source(e.session_mut(), &name, buf, Some(&file.to_string_lossy()), format.format);
        let ins = multisampler_mut(e, t).ok_or_else(|| bad(id, "the track's instrument is not the Multisampler"))?;
        ins.samples.insert(key.clone(), src);
    }
    let name = path.file_stem().and_then(|n| n.to_str()).unwrap_or("Multisample");
    Ok(json!({
        "track": t.0,
        "name": name,
        "samples": placed.len(),
        "notes": layers.values().filter(|v| !v.is_empty()).count(),
        "layers": layers.values().map(BTreeSet::len).max().unwrap_or(0),
        "skipped": skipped,
    }))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
    use std::path::PathBuf;

    fn dir(name: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("sc-multi-{name}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    /// A generated tone (no recorded or third-party audio) as a 24-bit WAV.
    fn tone_wav(path: &Path, rate: u32) {
        let x: Vec<f32> = (0..rate as usize / 10).map(|i| 0.5 * (i as f32 * 0.05).sin()).collect();
        let buf = AudioBuffer { sample_rate: rate, channels: vec![x] };
        let bytes =
            soundcraft_audio_io::encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int24, ..Default::default() }).unwrap();
        std::fs::write(path, bytes).unwrap();
    }

    fn track(e: &mut Engine, instrument: &str) -> TrackId {
        let v = e.execute("track.new", &json!({"kind": "instrument", "instrument": instrument})).unwrap();
        TrackId(v["tracks"][0].as_u64().unwrap())
    }

    fn region(sample: &str, note: u8, velocity: u8, robin: usize) -> Region {
        Region { sample: sample.into(), note, velocity, robin }
    }

    #[test]
    fn notes_read_as_numbers_or_names() {
        assert_eq!(
            [sfz_note("60"), sfz_note("c4"), sfz_note("C#4"), sfz_note("db4"), sfz_note("a0"), sfz_note("c-1")],
            [Some(60), Some(60), Some(61), Some(61), Some(21), Some(0)]
        );
        assert_eq!([sfz_note("128"), sfz_note("-1"), sfz_note("h2"), sfz_note("c"), sfz_note("c99"), sfz_note("")], [None; 6]);
    }

    #[test]
    fn an_sfz_file_gives_its_key_down_regions() {
        let text = "// a comment\n<control> default_path=Samples\\\n<global> amp_veltrack=0 ampeg_release=0.4\n\
            <region> sample=060_C4_v040_rr1.wav pitch_keycenter=60 lokey=58 hikey=61 lovel=1 hivel=40 seq_length=2 seq_position=1\n\
            <region> sample=060_C4_v040_rr2.wav pitch_keycenter=60 lovel=1 hivel=40 seq_length=2 seq_position=2 // second take\n\
            <group> key=c5 hivel=90\n<region> sample=Grand Piano C5 soft.flac lovel=1\n<region> sample=loud.wav hivel=127\n\
            <region> sample=release.wav trigger=release\n<region> sample=*sine\n<region> lokey=1\n\
            <group>\n<region>\nsample=late.wav\nkey=72";
        assert_eq!(
            parse_sfz(text),
            [
                region("Samples/060_C4_v040_rr1.wav", 60, 40, 1),
                region("Samples/060_C4_v040_rr2.wav", 60, 40, 2),
                region("Samples/Grand Piano C5 soft.flac", 72, 90, 1),
                region("Samples/loud.wav", 72, 127, 1),
                region("Samples/late.wav", 72, 127, 1),
            ]
        );
        assert!(parse_sfz("").is_empty() && parse_sfz("sample=a.wav <nonsense> = == <region>").is_empty());
    }

    #[test]
    fn loading_an_sfz_file_gives_the_multisampler_its_samples() {
        let d = dir("load");
        for f in ["a.wav", "b.wav", "c.wav", "d.wav"] {
            tone_wav(&d.join(f), 44_100);
        }
        let sfz = d.join("Tones.sfz");
        std::fs::write(
            &sfz,
            "<region> sample=a.wav pitch_keycenter=48 hivel=60\n<region> sample=b.wav pitch_keycenter=48 hivel=127\n\
             <region> sample=c.wav key=60 seq_position=1\n<region> sample=d.wav key=60 seq_position=1\n",
        )
        .unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!(
            (v["name"].as_str(), v["samples"].as_u64(), v["notes"].as_u64(), v["layers"].as_u64(), v["skipped"].as_u64()),
            (Some("Tones"), Some(4), Some(2), Some(2), Some(0))
        );
        let ins = e.session().track(t).and_then(|tr| tr.instrument.clone()).unwrap();
        // The second sample for one place is that layer's next take.
        assert_eq!(ins.samples.keys().map(String::as_str).collect::<Vec<_>>(), ["n48.v127.r1", "n48.v60.r1", "n60.v127.r1", "n60.v127.r2"]);
        assert!(ins.samples.values().all(|s| e.session().source(*s).is_some()));
        // Loading again replaces what was there; one undo step takes a load back.
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        let keys = |e: &Engine| e.session().track(t).and_then(|tr| tr.instrument.as_ref()).map(|i| i.samples.len());
        assert_eq!(keys(&e), Some(1));
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(keys(&e), Some(4));
    }

    #[test]
    fn a_bad_load_changes_nothing() {
        let d = dir("bad");
        tone_wav(&d.join("a.wav"), 48_000);
        let sfz = d.join("x.sfz");
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        let before = e.session().track(t).and_then(|tr| tr.instrument.clone());
        // A missing sample, a file with no regions, and a track playing something else.
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n<region> sample=missing.wav key=52\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
        std::fs::write(&sfz, "<global> volume=3\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).is_err());
        assert_eq!(e.session().track(t).and_then(|tr| tr.instrument.clone()), before);
        let other = track(&mut e, "sampler");
        std::fs::write(&sfz, "<region> sample=a.wav key=50\n").unwrap();
        assert!(e.execute("mix.multisample_load", &json!({"track": other.0, "path": sfz.to_string_lossy()})).is_err());
    }

    #[test]
    fn layers_and_takes_past_the_limits_are_skipped() {
        let d = dir("limits");
        tone_wav(&d.join("a.wav"), 48_000);
        let mut text = String::new();
        for v in 1..=18 {
            text.push_str(&format!("<region> sample=a.wav key=60 hivel={v}\n"));
        }
        for _ in 0..6 {
            text.push_str("<region> sample=a.wav key=62\n");
        }
        let sfz = d.join("many.sfz");
        std::fs::write(&sfz, text).unwrap();
        let mut e = Engine::default();
        let t = track(&mut e, MULTISAMPLER);
        let v = e.execute("mix.multisample_load", &json!({"track": t.0, "path": sfz.to_string_lossy()})).unwrap();
        assert_eq!((v["samples"].as_u64(), v["layers"].as_u64(), v["skipped"].as_u64()), (Some(20), Some(16), Some(4)));
    }
}

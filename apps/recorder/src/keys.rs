//! The keyboard's sound: one of FreeDAWm's built-in synths (Burley, Cicada, Steeple) with one of
//! their factory presets, so playing a MIDI keyboard into the Recorder makes sound, and an idea
//! played on keys can be heard back.
//!
//! - [`KeySound`] is the choice: the instrument, its preset, its parameter values (the preset's,
//!   then whatever was turned), its patch (the preset's modulation and named macros), a volume
//!   and an octave. It is remembered in the settings and, for each layer played on keys, in the
//!   idea's `.fdidea`, so the idea plays back the same and FreeDAWm opens it with that instrument.
//! - The sound plays live through the rig's instrument track (armed, its input every keyboard),
//!   through the output like the DAW's own instruments ([`crate::rig::Rig::set_keys`]).
//! - [`render`] plays a MIDI file through the sound offline (the DAW's own render), for the
//!   optional `<take> (keys).wav` and to hear an idea's MIDI layers back.
//!
//! The octave is a transpose on the way in (the track's MIDI input transform), so the MIDI file
//! holds the notes as they were heard.

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use soundcraft_engine::Engine;
use std::collections::BTreeMap;

/// The built-in instruments offered, with their names in the DAW.
pub const INSTRUMENTS: [(&str, &str); 3] = [("fm_synth", "Steeple"), ("subtractive_synth", "Burley"), ("wavetable_synth", "Cicada")];

/// The sound a new Recorder starts with: the piano FreeDAWm's New Band uses.
pub const DEFAULT: (&str, &str) = ("fm_synth", "Tine Piano");

/// The keyboard's sound.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct KeySound {
    /// The built-in instrument's id (`fm_synth`).
    pub plugin: String,
    /// The factory preset it started from.
    pub preset: String,
    /// Every parameter's value.
    #[serde(default)]
    pub params: BTreeMap<String, f32>,
    /// The preset's patch (`{slots, macros}`), as FreeDAWm stores it.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub modulation: Option<Value>,
    /// The track's volume (dB).
    #[serde(default)]
    pub volume_db: f32,
    /// Octaves up (+) or down (−), −3..=3.
    #[serde(default)]
    pub octave: i8,
}

/// One preset offered: what kind of sound it is (in plain words), its name, its instrument.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Choice {
    pub kind: String,
    pub preset: String,
    pub plugin: &'static str,
}

impl Choice {
    /// `Piano & Keys · Tine Piano`.
    pub fn label(&self) -> String {
        format!("{} · {}", self.kind, self.preset)
    }
}

/// A preset's group as a person names the kind of sound.
fn kind_of(group: &str, name: &str) -> String {
    let lower = name.to_lowercase();
    match group {
        "Keys" if lower.contains("organ") => "Organ".into(),
        "Keys" => "Piano & Keys".into(),
        "Brass & Strings" | "Brass" => "Strings & Brass".into(),
        "Bells" | "Mallets" => "Bells & Mallets".into(),
        "Percussion" => "Drums & Percussion".into(),
        "Basics" => "Simple".into(),
        "Patched" => "Special".into(),
        "Texture" | "Effects" => "Textures & Effects".into(),
        g => g.to_string(),
    }
}

/// The order the kinds are listed in.
const KIND_ORDER: [&str; 13] = [
    "Piano & Keys",
    "Organ",
    "Bells & Mallets",
    "Bass",
    "Pad",
    "Lead",
    "Pluck",
    "Strings & Brass",
    "Drums & Percussion",
    "Textures & Effects",
    "Special",
    "Simple",
    "",
];

fn preset_json(plugin: &str, preset: &str) -> Option<&'static str> {
    soundcraft_dsp::plugins::fs::factory_presets(plugin).iter().find(|(n, _)| n.eq_ignore_ascii_case(preset)).map(|(_, j)| *j)
}

/// Every preset of the built-in instruments, grouped by kind (the piano first).
pub fn choices() -> Vec<Choice> {
    let mut out = Vec::new();
    for (plugin, _) in INSTRUMENTS {
        for (name, json) in soundcraft_dsp::plugins::fs::factory_presets(plugin) {
            let group =
                serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("group").and_then(Value::as_str).map(str::to_string)).unwrap_or_default();
            out.push(Choice { kind: kind_of(&group, name), preset: (*name).to_string(), plugin });
        }
    }
    let rank = |k: &str| KIND_ORDER.iter().position(|x| *x == k).unwrap_or(KIND_ORDER.len());
    out.sort_by_key(|c| rank(&c.kind));
    out
}

impl KeySound {
    /// `plugin` with its factory preset `preset` (its values over the defaults, its patch).
    pub fn from_preset(plugin: &str, preset: &str) -> Option<KeySound> {
        let info = soundcraft_dsp::plugin_info(plugin).filter(|i| i.is_instrument)?;
        let json = preset_json(plugin, preset)?;
        let p = soundcraft_dsp::plugins::fs::FsPreset::parse(json, info)?;
        let mut params: BTreeMap<String, f32> = info.params.iter().map(|pi| (pi.id.to_string(), pi.default)).collect();
        params.extend(p.values);
        let modulation = serde_json::from_str::<Value>(json).ok().and_then(|v| v.get("modulation").cloned()).filter(|m| !m.is_null());
        Some(KeySound { plugin: plugin.to_string(), preset: preset.to_string(), params, modulation, volume_db: 0.0, octave: 0 })
    }

    /// The piano a new Recorder starts with.
    pub fn default_piano() -> Option<KeySound> {
        KeySound::from_preset(DEFAULT.0, DEFAULT.1)
    }

    /// The instrument's name in FreeDAWm (`Steeple`).
    pub fn instrument_name(&self) -> &'static str {
        INSTRUMENTS.iter().find(|(id, _)| *id == self.plugin).map_or("Synth", |(_, n)| *n)
    }

    /// The choice it came from (its kind).
    pub fn choice(&self) -> Option<Choice> {
        choices().into_iter().find(|c| c.plugin == self.plugin && c.preset == self.preset)
    }

    /// The preset's named macros: `(parameter, name)` (`macro1`, `Open`).
    pub fn macros(&self) -> Vec<(String, String)> {
        let Some(list) = self.modulation.as_ref().and_then(|m| m.get("macros")).and_then(Value::as_array) else { return Vec::new() };
        list.iter()
            .enumerate()
            .filter_map(|(i, m)| {
                let name = m.get("name").and_then(Value::as_str).map(str::trim).filter(|n| !n.is_empty())?;
                let id = format!("macro{}", i + 1);
                self.params.contains_key(&id).then(|| (id, name.to_string()))
            })
            .take(4)
            .collect()
    }

    /// One parameter, clamped into its range.
    pub fn set_param(&mut self, id: &str, value: f32) {
        if let Some(pi) = soundcraft_dsp::plugin_info(&self.plugin).and_then(|i| i.params.iter().find(|p| p.id == id)) {
            self.params.insert(id.to_string(), pi.clamp(value));
        }
    }

    /// Puts the sound on instrument track `track` of `e`: the instrument with its values and
    /// patch, the volume, the octave on the way in.
    pub fn apply(&self, e: &mut Engine, track: u64) -> Result<(), String> {
        let mut p = json!({"tracks": [track], "plugin": self.plugin, "params": self.params, "preset": self.preset});
        if let Some(m) = &self.modulation {
            p["modulation"] = m.clone();
        }
        e.execute("mix.instrument", &p).map_err(|e| e.to_string())?;
        e.execute("mix.volume", &json!({"tracks": [track], "db": f64::from(self.volume_db.clamp(-60.0, 12.0))})).map_err(|e| e.to_string())?;
        e.execute("track.midi_input_transform", &json!({"tracks": [track], "transpose": i64::from(self.octave.clamp(-3, 3)) * 12}))
            .map_err(|e| e.to_string())?;
        Ok(())
    }
}

/// A MIDI file (`bytes`, its notes already at the octave heard) played through `sound` at `rate`
/// for `frames` frames: stereo, as FreeDAWm renders its instruments.
pub fn render(sound: &KeySound, bytes: &[u8], rate: u32, frames: usize) -> Result<Vec<Vec<f32>>, String> {
    let mut e = Engine::default();
    e.session_mut().sample_rate = soundcraft_time::SampleRate::new(rate).map_err(|e| e.to_string())?;
    let tracks = soundcraft_engine::io::import_midi_bytes(&mut e, bytes, 0, true).map_err(|e| e.to_string())?;
    let flat = KeySound { octave: 0, ..sound.clone() };
    for t in tracks {
        flat.apply(&mut e, t.0)?;
    }
    let end = i64::try_from(frames).unwrap_or(i64::MAX);
    let mut out = soundcraft_mix::render_range(e.session(), soundcraft_time::Range { start: 0, end }, 512);
    out.truncate(2);
    for c in &mut out {
        c.resize(frames, 0.0);
    }
    while out.len() < 2 {
        let first = out.first().cloned().unwrap_or_else(|| vec![0.0; frames]);
        out.push(first);
    }
    Ok(out)
}

/// [`render`] written as a 24-bit WAV at `path` (through a temporary file).
pub fn render_wav(sound: &KeySound, mid: &std::path::Path, rate: u32, frames: usize, path: &std::path::Path) -> Result<(), String> {
    let bytes = std::fs::read(mid).map_err(|e| e.to_string())?;
    let channels = render(sound, &bytes, rate, frames)?;
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels };
    let out = soundcraft_audio_io::encode(&buf, &soundcraft_audio_io::EncodeOptions::default()).map_err(|e| e.to_string())?;
    let part = path.with_extension("wav.part");
    std::fs::write(&part, out).map_err(|e| e.to_string())?;
    std::fs::rename(&part, path).map_err(|e| e.to_string())
}

/// The keys render's stem for a layer's stem (`Chorus (layer 2)` → `Chorus (layer 2) (keys)`).
pub fn keys_stem(stem: &str) -> String {
    format!("{stem} (keys)")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_presets_are_offered_by_kind_and_the_piano_is_there() {
        let all = choices();
        assert!(all.len() > 50, "{}", all.len());
        assert_eq!(all[0].kind, "Piano & Keys");
        assert!(all.iter().any(|c| c.plugin == DEFAULT.0 && c.preset == DEFAULT.1));
        for k in ["Bass", "Pad", "Lead", "Organ"] {
            assert!(all.iter().any(|c| c.kind == k), "{k}");
        }
        let piano = KeySound::default_piano().unwrap();
        assert_eq!(piano.instrument_name(), "Steeple");
        assert!(!piano.params.is_empty());
    }

    #[test]
    fn a_patched_preset_names_its_macros() {
        let pad = KeySound::from_preset("subtractive_synth", "Breathing Pad").unwrap();
        let names: Vec<String> = pad.macros().into_iter().map(|(_, n)| n).collect();
        assert_eq!(names, vec!["Open", "Wobble", "Grit", "Space"]);
        let mut p = pad.clone();
        p.set_param("macro1", 1e9);
        assert!(p.params["macro1"] <= 100.0, "clamped");
        let back: KeySound = serde_json::from_str(&serde_json::to_string(&pad).unwrap()).unwrap();
        assert_eq!(back, pad);
    }
}

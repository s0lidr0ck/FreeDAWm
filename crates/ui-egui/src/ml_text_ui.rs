//! Detect Chords and Transcribe Lyrics: their parts of the ML job dialog (`ml_ui`), the parameters
//! they run with, and what the status line says when they finish.

use crate::theme::Tokens;
use serde_json::{Value, json};
use soundcraft_engine::ml::{self, Grid};

/// Detect Chords' settings in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct ChordDialog {
    pub grid: Grid,
    /// Replace the lane's chords over the source (else fill only empty places).
    pub replace: bool,
    /// Guess inversions (a full mix only).
    pub bass: bool,
    /// `auto`, `keep`, or a key name.
    pub key: String,
    pub minor: bool,
}

impl ChordDialog {
    /// Defaults for a source: the bass guess on for a full mix; "As played" when the session has
    /// no tempo map from the recording ([`default_grid`]).
    pub fn new(full_mix: bool, grid: Grid) -> Self {
        ChordDialog { grid, replace: true, bass: full_mix, key: "auto".into(), minor: false }
    }
}

/// The grid Detect Chords starts on: "As played" when the session's tempo map has no tempo
/// changes (nothing mapped it to a recording, so a live take drifts against its bars), else half
/// bars.
pub fn default_grid(s: &soundcraft_model::Session) -> Grid {
    if s.tempo.tempos().len() > 1 { Grid::HalfBar } else { Grid::Free }
}

/// Transcribe Lyrics' settings in the dialog.
#[derive(Debug, Clone, PartialEq, Default)]
pub struct LyricsDialog {
    /// Detect the language (else English).
    pub auto_language: bool,
    pub prompt: String,
    /// The full f16 model ("Best") instead of the quantized one ("Fast", the default).
    pub best: bool,
    /// Leave long silences out before decoding.
    pub skip_silence: bool,
}

/// Generate MIDI from Text's settings in the dialog.
#[derive(Debug, Clone, PartialEq)]
pub struct TextMidiDialog {
    /// What to compose.
    pub prompt: String,
    /// Length, seconds (1–100).
    pub seconds: f32,
    /// Randomness (the sampling temperature, 0.1–2).
    pub temperature: f32,
    /// A seed to repeat a piece; empty = a new one each run.
    pub seed: String,
    /// The MIDI clip selected when the dialog opened (id, name), to continue.
    pub clip: Option<(u64, String)>,
    /// Start the piece with that clip's notes.
    pub continue_clip: bool,
}

impl Default for TextMidiDialog {
    fn default() -> Self {
        TextMidiDialog { prompt: String::new(), seconds: 30.0, temperature: 1.0, seed: String::new(), clip: None, continue_clip: false }
    }
}

impl TextMidiDialog {
    /// Whether Run can start: a prompt, and a seed that is a whole number (or none).
    pub fn ready(&self) -> bool {
        !self.prompt.trim().is_empty() && (self.seed.trim().is_empty() || self.seed.trim().parse::<u64>().is_ok())
    }
}

pub fn text_midi_params(o: &mut serde_json::Map<String, Value>, t: &TextMidiDialog) {
    o.insert("prompt".into(), json!(t.prompt.trim()));
    o.insert("seconds".into(), json!(t.seconds));
    o.insert("temperature".into(), json!(t.temperature));
    if let Ok(seed) = t.seed.trim().parse::<u64>() {
        o.insert("seed".into(), json!(seed));
    }
    if t.continue_clip
        && let Some((id, _)) = &t.clip
    {
        o.insert("continue_from".into(), json!(id));
    }
}

pub fn text_midi_options(ui: &mut egui::Ui, t: &mut TextMidiDialog, cursor: &str) {
    let tk = Tokens::current();
    ui.label("Describe the music: style, instruments, key, meter, tempo, mood.");
    ui.add(
        egui::TextEdit::multiline(&mut t.prompt)
            .hint_text("e.g. Upbeat jazz with saxophones, piano, bass and drums, in B-flat major, 4/4")
            .desired_rows(3)
            .desired_width(320.0),
    );
    ui.horizontal(|ui| {
        ui.label("Length");
        ui.add(egui::Slider::new(&mut t.seconds, 5.0..=100.0).step_by(1.0).suffix(" s"));
    });
    ui.horizontal(|ui| {
        ui.label("Randomness");
        ui.add(egui::Slider::new(&mut t.temperature, 0.5..=1.5).step_by(0.05));
    })
    .response
    .on_hover_text("1 is the model's own; lower stays safer, higher wanders more");
    ui.horizontal(|ui| {
        ui.label("Seed");
        ui.add(egui::TextEdit::singleline(&mut t.seed).hint_text("new each run").desired_width(120.0));
    })
    .response
    .on_hover_text("The same seed, prompt and settings give the same notes. Each run reports its seed in the status line.");
    if !t.seed.trim().is_empty() && t.seed.trim().parse::<u64>().is_err() {
        ui.colored_label(tk.rec, "The seed is a whole number.");
    }
    if let Some((_, name)) = &t.clip {
        ui.checkbox(&mut t.continue_clip, format!("Continue from the clip “{name}”")).on_hover_text(
            "The piece starts with the clip's notes (up to 170) and the model carries on from them; the new tracks start where the clip does",
        );
    }
    ui.label(
        egui::RichText::new(format!(
            "One new instrument track per General MIDI instrument, at {}. MIDI-LLM writes up to 682 notes; on the CPU that takes one to two minutes.",
            if t.continue_clip && t.clip.is_some() { "the clip's start" } else { cursor }
        ))
        .small()
        .color(tk.text_dim),
    );
}

const KEYS: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];

/// Whether the dialog's source (`{"track": id}` / `{"clip": id}`) looks like a full mix.
pub fn full_mix(s: &soundcraft_model::Session, target: &Value) -> bool {
    let track = if let Some(c) = target.get("clip").and_then(Value::as_u64) {
        s.find_clip(soundcraft_model::ClipId(c)).map(|(t, _)| t)
    } else {
        target.get("track").and_then(Value::as_u64).map(soundcraft_model::TrackId)
    };
    track.and_then(|t| s.track(t)).is_some_and(ml::looks_like_full_mix)
}

pub fn chord_params(o: &mut serde_json::Map<String, Value>, c: &ChordDialog) {
    o.insert("grid".into(), json!(c.grid.id()));
    o.insert("mode".into(), json!(if c.replace { "replace" } else { "fill" }));
    o.insert("bass".into(), json!(c.bass));
    o.insert("key".into(), json!(c.key));
    if KEYS.contains(&c.key.as_str()) {
        o.insert("scale".into(), json!(if c.minor { "minor" } else { "major" }));
    }
}

pub fn lyrics_params(o: &mut serde_json::Map<String, Value>, l: &LyricsDialog) {
    o.remove("device");
    o.insert("language".into(), json!(if l.auto_language { "auto" } else { "en" }));
    o.insert("quality".into(), json!(if l.best { "best" } else { "fast" }));
    o.insert("skip_silence".into(), json!(l.skip_silence));
    if !l.prompt.trim().is_empty() {
        o.insert("prompt".into(), json!(l.prompt.trim()));
    }
}

pub fn chord_options(ui: &mut egui::Ui, c: &mut ChordDialog, band_key: &str) {
    let t = Tokens::current();
    ui.label("Chords onto the Band's chord lane at the source's position (BTC, large vocabulary).");
    ui.horizontal(|ui| {
        ui.label("Snap to");
        ui.radio_value(&mut c.grid, Grid::HalfBar, "Half bars");
        ui.radio_value(&mut c.grid, Grid::Bar, "Bars");
        ui.radio_value(&mut c.grid, Grid::Free, "As played")
            .on_hover_text("Keep each change where it was heard (changes closer than an eighth of a bar merge)");
    });
    if c.grid == Grid::Free {
        // The band's lane scheduler works in half bars (docs/control-protocol.md, ml.chords).
        ui.label(
            egui::RichText::new("The band still changes chord on the nearest half bar; the rulers and editors use the exact time. A chord shorter than about half a bar may not sound in the band at all.")
                .small()
                .color(t.text_dim),
        );
    }
    ui.horizontal(|ui| {
        ui.radio_value(&mut c.replace, true, "Replace chords in range");
        ui.radio_value(&mut c.replace, false, "Only fill empty places");
    });
    ui.horizontal(|ui| {
        ui.label("Key");
        let label = match c.key.as_str() {
            "auto" => "Auto (from the chords)".to_string(),
            "keep" => format!("The band's ({band_key})"),
            k => k.to_string(),
        };
        egui::ComboBox::from_id_salt("ml_chords_key").selected_text(label).show_ui(ui, |ui| {
            ui.selectable_value(&mut c.key, "auto".to_string(), "Auto (from the chords)");
            ui.selectable_value(&mut c.key, "keep".to_string(), format!("The band's ({band_key})"));
            for k in KEYS {
                ui.selectable_value(&mut c.key, k.to_string(), k);
            }
        });
        if KEYS.contains(&c.key.as_str()) {
            ui.checkbox(&mut c.minor, "Minor");
        }
    })
    .response
    .on_hover_text(
        "Chord slots are relative to the band's key. Auto estimates the key from the chords and sets it when the lane holds no other chords.",
    );
    ui.checkbox(&mut c.bass, "The source is a full mix: guess the bass (inversions like C/E)")
        .on_hover_text("On a single instrument this reports its lowest voice, not the band's bass: leave it off for stems.");
    ui.label(egui::RichText::new("The Band plays the lane in half bars.").small().color(t.text_dim));
}

pub fn lyrics_options(ui: &mut egui::Ui, l: &mut LyricsDialog, seconds: f64) {
    let t = Tokens::current();
    ui.label("Timed lyrics on the source track (Whisper large-v3). Use a vocal track, not a full mix.");
    ui.horizontal(|ui| {
        ui.label("Language");
        ui.radio_value(&mut l.auto_language, false, "English");
        ui.radio_value(&mut l.auto_language, true, "Detect");
    });
    ui.horizontal(|ui| {
        ui.label("Prompt");
        ui.add(egui::TextEdit::singleline(&mut l.prompt).hint_text("optional: names and words to expect").desired_width(220.0));
    });
    ui.horizontal(|ui| {
        ui.label("Model");
        ui.radio_value(&mut l.best, false, "Fast (q8_0)");
        ui.radio_value(&mut l.best, true, "Best (f16)");
    })
    .response
    .on_hover_text("Fast is whisper large-v3 quantized to 8 bits: the same words on the test verse, about 1.3x faster");
    ui.checkbox(&mut l.skip_silence, "Skip long silences")
        .on_hover_text("Leaves out silences over 2 s before transcribing. Off: it did not speed up the test vocal.");
    // whisper.cpp large-v3 on the CPU (16 threads, Ryzen 9 8940HX, clang build): 4 min in 86 s
    // (q8_0) or 107 s (f16), model loading included.
    let est = (seconds * if l.best { 0.45 } else { 0.36 } + 5.0) / 60.0;
    ui.label(
        egui::RichText::new(format!(
            "Runs on the CPU: about {} for this {seconds:.0} s source.",
            if est < 1.0 { "a minute".to_string() } else { format!("{est:.0} min") }
        ))
        .color(t.text_dim),
    );
}

/// What the status line says about a finished Detect Chords or Transcribe Lyrics job.
pub fn summary(title: &str, v: &Value, secs: f64) -> Option<String> {
    // Generate MIDI from Text: the instruments and the seed that repeats the piece.
    if let Some(seed) = v.get("seed").and_then(Value::as_str) {
        let made = v.get("made")?.as_array()?;
        let names: Vec<&str> = made.iter().filter_map(|m| m.get("name").and_then(Value::as_str)).collect();
        let notes: u64 = made.iter().filter_map(|m| m.get("notes").and_then(Value::as_u64)).sum();
        return Some(format!("{title}: {notes} notes on {} tracks ({}) in {secs:.1} s, seed {seed}", names.len(), names.join(", ")));
    }
    let made = v.get("made")?.as_array()?.first()?;
    if let Some(ch) = made.get("chords").and_then(Value::as_array) {
        let names: Vec<&str> = ch.iter().filter_map(|c| c.get("name").and_then(Value::as_str)).collect();
        let shown = names.iter().take(8).copied().collect::<Vec<_>>().join(" ");
        let more = if names.len() > 8 { " …" } else { "" };
        let key = made.get("key").and_then(Value::as_str).unwrap_or("");
        return Some(format!("{title}: {} chords in {key} ({shown}{more}) in {secs:.1} s", names.len()));
    }
    let lines = made.get("lines").and_then(Value::as_u64)?;
    let words = made.get("words").and_then(Value::as_u64).unwrap_or(0);
    Some(format!("{title}: {lines} lines, {words} words in {secs:.1} s"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Services, SoundApp};

    #[test]
    fn generate_midi_from_text_reports_its_tracks_and_seed() {
        let v = json!({"seed": "77", "made": [{"name": "Acoustic Grand Piano", "notes": 40}, {"name": "Drums", "notes": 12}]});
        assert_eq!(
            summary("Generate MIDI from Text", &v, 61.04).unwrap(),
            "Generate MIDI from Text: 52 notes on 2 tracks (Acoustic Grand Piano, Drums) in 61.0 s, seed 77"
        );
    }

    #[test]
    fn menus_and_palette_open_the_dialogs_with_valid_params() {
        let mut app = SoundApp::new(soundcraft_engine::demo::demo_engine(), None, Services::default());
        app.engine.execute("edit.select", &json!({"tracks": ["Bass"]})).unwrap();
        for (id, cmd) in [("ml.chords", "ml.chords"), ("ui.ml.lyrics", "ml.lyrics")] {
            app.ml.dialog = None;
            crate::menus::invoke_menu(&mut app, id, "");
            let Some(d) = app.ml.dialog.clone() else { panic!("{id}: no dialog") };
            assert_eq!(d.command, cmd);
            assert!(app.ml.running.is_none(), "nothing runs until Run is pressed");
            let p = crate::ml_ui::params(&d, ml::Device::Cpu);
            if cmd == "ml.chords" {
                // "Bass" is named after an instrument: no bass guess.
                // A product decision (the spec's call), not a property of the demo: a session whose
                // tempo never changes starts on As played.
                assert_eq!((p["grid"].as_str(), p["mode"].as_str(), p["bass"].as_bool()), (Some("free"), Some("replace"), Some(false)));
                assert!(ml::lane_options(cmd, &p).is_ok());
            } else {
                assert_eq!(p["language"], "en");
                assert!(p.get("device").is_none());
                assert!(ml::lyrics_options(cmd, &p).is_ok());
            }
        }
        // A tempo map with changes (Identify Beat, a tempo-mapped recording): half bars.
        let mut s = soundcraft_model::Session::default();
        assert_eq!(default_grid(&s), Grid::Free);
        s.tempo.set_tempo(3840, 100.0).unwrap();
        assert_eq!(default_grid(&s), Grid::HalfBar);
        let mut c = ChordDialog::new(true, Grid::HalfBar);
        c.key = "E".into();
        c.minor = true;
        c.replace = false;
        let mut o = serde_json::Map::new();
        chord_params(&mut o, &c);
        let p = Value::Object(o);
        assert_eq!(
            ml::lane_options("ml.chords", &p).unwrap(),
            ml::LaneOptions { grid: Grid::HalfBar, replace: false, key: ml::KeyChoice::Set(4, 1) }
        );
        let mut l = LyricsDialog { auto_language: true, prompt: "  Yahweh, Elohim ".into(), ..LyricsDialog::default() };
        let mut o = serde_json::Map::new();
        lyrics_params(&mut o, &l);
        assert_eq!(Value::Object(o), json!({"language": "auto", "quality": "fast", "skip_silence": false, "prompt": "Yahweh, Elohim"}));
        l.prompt.clear();
        let summary_text = summary("Detect Chords — Mix", &json!({"made": [{"chords": [{"name": "Em"}, {"name": "C"}], "key": "E minor"}]}), 1.0);
        assert_eq!(summary_text.as_deref(), Some("Detect Chords — Mix: 2 chords in E minor (Em C) in 1.0 s"));
    }
}

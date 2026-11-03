//! The Chords ruler's chord picker: a chord name (anything `band.chord.add` reads: "Am7",
//! "F#m7b5", "G/B") with a live preview of the lane chord it becomes (exact, or the nearest one
//! the band's key holds), and quick buttons for the diatonic triads and sevenths of the key in
//! force at the chord's time.

use crate::SoundApp;
use crate::dialogs::Dialog;
use serde_json::json;
use soundcraft_band::chord_match::{match_chord, match_profile, parse_chord_name};
use soundcraft_band::chord_theory::{name_for, scale_offset};
use soundcraft_model::Session;
use soundcraft_time::Samples;

/// Asks the Chords ruler to flash "≈ nearest in key" on a chord (its id, in egui's temporary
/// memory under this id), after the picker laid a chord the key doesn't hold.
pub const FLASH_REQUEST: &str = "band.lanes.flash.request";

/// The picker's state (in [`Dialog::ChordPicker`]).
#[derive(Debug, Clone, PartialEq)]
pub struct ChordPicker {
    /// Where an added chord starts, and ends (the next chord, else the end of its bar).
    pub at: Samples,
    pub end: Samples,
    /// The chord it edits; `None` adds one.
    pub chord: Option<u32>,
    pub text: String,
    /// The quick buttons: the key in force's triads, then its sevenths.
    pub triads: Vec<String>,
    pub sevenths: Vec<String>,
    /// The name box took the focus (on the first frame).
    pub focused: bool,
    /// The chord in the box was named from keys held on a MIDI input: adding it moves the
    /// picker on to the next chord slot.
    pub from_midi: bool,
    /// The held notes last named (a new set renames; letting go leaves the name).
    pub held: Vec<u8>,
}

/// The chord `held` (MIDI pitches) makes in the band's key, as the lane would name it: the
/// Band's matcher for keys held live, the lowest note the bass ("C/E"). `None` for no notes.
pub fn held_chord_name(s: &Session, held: &[u8]) -> Option<String> {
    let lowest = *held.iter().min()?;
    let mut w = [0.0f32; 12];
    for p in held {
        if let Some(x) = w.get_mut(usize::from(p % 12)) {
            *x = 1.0;
        }
    }
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let m = match_profile(&w, i32::from(lowest % 12), key, scale);
    Some(name_for(m.slot.clamp(0, 111), key, scale, m.bass.clamp(-1, 11)))
}

const SHARP_NAMES: [&str; 12] = ["C", "C#", "D", "D#", "E", "F", "F#", "G", "G#", "A", "A#", "B"];
const FLAT_NAMES: [&str; 12] = ["C", "Db", "D", "Eb", "E", "F", "Gb", "G", "Ab", "A", "Bb", "B"];

/// Whether a key is written with flats (F, Bb, Eb, Ab, Db major; D, G, C, F, Bb, Eb minor).
fn uses_flats(pc: u8, minor: bool) -> bool {
    let flats: &[u8] = if minor { &[2, 7, 0, 5, 10, 3] } else { &[5, 10, 3, 8, 1] };
    flats.contains(&(pc % 12))
}

/// The diatonic triads and sevenths of a key (scale 0 major, 1 minor), spelled as the key spells
/// its notes: ["C", "Dm", …, "Bdim"] and ["Cmaj7", "Dm7", …, "Bm7b5"].
pub fn diatonic(key: u8, scale: u8) -> (Vec<String>, Vec<String>) {
    let mode = i32::from(scale.min(1));
    let names = if uses_flats(key, scale == 1) { &FLAT_NAMES } else { &SHARP_NAMES };
    let iv = |d: i32, k: i32| (scale_offset(mode, d + k) - scale_offset(mode, d)).rem_euclid(12);
    let mut triads = Vec::new();
    let mut sevenths = Vec::new();
    for d in 0..7 {
        let root = (i32::from(key) + scale_offset(mode, d)).rem_euclid(12);
        let name = names.get(root as usize).copied().unwrap_or("C");
        let (third, fifth, seventh) = (iv(d, 2), iv(d, 4), iv(d, 6));
        let triad = match (third, fifth) {
            (4, 7) => "",
            (3, 7) => "m",
            (3, 6) => "dim",
            (4, 8) => "aug",
            _ => "",
        };
        let sev = match (third, fifth, seventh) {
            (4, 7, 11) => "maj7",
            (4, 7, 10) => "7",
            (3, 7, 10) => "m7",
            (3, 6, 10) => "m7b5",
            (3, 6, 9) => "dim7",
            (3, 7, 11) => "mMaj7",
            _ => "7",
        };
        triads.push(format!("{name}{triad}"));
        sevenths.push(format!("{name}{sev}"));
    }
    (triads, sevenths)
}

/// What a typed name becomes on the lane in the band's key: its name there and whether it is
/// exact. `None` when the name can't be read.
pub fn preview(s: &Session, text: &str) -> Option<(String, bool)> {
    let pcs = parse_chord_name(text.trim())?;
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let m = match_chord(&pcs, key, scale);
    Some((name_for(m.slot.clamp(0, 111), key, scale, m.bass.clamp(-1, 11)), m.exact))
}

/// Opens the picker at tick `at` on the lane: on chord `edit` (its name in the box), or to add
/// one lasting to the next chord, else to the end of its bar.
pub fn open(app: &mut SoundApp, at: i64, edit: Option<u32>) {
    let s = app.engine.session();
    let half = soundcraft_mix::half_ticks(s).max(1);
    let bar = half.saturating_mul(2);
    let (key, scale) = (i32::from(s.band.key), i32::from(s.band.scale));
    let found = edit.and_then(|id| s.band.chords.iter().find(|c| c.id == id));
    let text = found.map_or_else(String::new, |c| name_for(i32::from(c.slot), key, scale, i32::from(c.bass)));
    let next = s.band.chords.iter().map(|c| c.tick).filter(|t| *t > at).min();
    let bar_end = at.div_euclid(bar).saturating_add(1).saturating_mul(bar);
    let end_tick = next.unwrap_or(bar_end);
    let sr = s.sample_rate;
    let at_samples = s.tempo.tick_to_samples(at, sr);
    let (kp, ks) = crate::session_key::key_pc_at(s, at_samples);
    let (triads, sevenths) = diatonic(kp, ks);
    app.dialogs.open = Some(Dialog::ChordPicker(ChordPicker {
        at: at_samples,
        end: s.tempo.tick_to_samples(end_tick, sr),
        chord: found.map(|c| c.id),
        text,
        triads,
        sevenths,
        focused: false,
        from_midi: false,
        held: Vec::new(),
    }));
}

/// The picker's body; the command and its parameters once OK is pressed (or Enter). `held`:
/// the notes held now on the MIDI inputs, named live into the box.
pub fn body(
    ui: &mut egui::Ui,
    s: &Session,
    p: &mut ChordPicker,
    held: &[u8],
    ok: impl FnOnce(&mut egui::Ui, bool) -> bool,
) -> Option<(String, serde_json::Value)> {
    let t = crate::theme::Tokens::current();
    // Keys held on a MIDI keyboard (or Musical Typing) name the chord as it is played; letting
    // go leaves the name in the box.
    if !held.is_empty() && held != p.held.as_slice() {
        p.held = held.to_vec();
        if let Some(name) = held_chord_name(s, held) {
            p.text = name;
            p.from_midi = true;
        }
    } else if held.is_empty() {
        p.held.clear();
    }
    ui.set_max_width(420.0);
    ui.horizontal(|ui| {
        ui.label("Chord");
        let mut out = egui::TextEdit::singleline(&mut p.text).hint_text("Am7, F#m7b5, G/B").desired_width(160.0).show(ui);
        if !p.focused {
            p.focused = true;
            out.response.request_focus();
            let all = egui::text::CCursorRange::two(egui::text::CCursor::new(0), egui::text::CCursor::new(p.text.chars().count()));
            out.state.cursor.set_char_range(Some(all));
            out.state.store(ui.ctx(), out.response.id);
        }
    });
    let shown = preview(s, &p.text);
    let band_key = crate::session_key::spelled_key(s.band.key, s.band.scale == 1);
    let line = match (&shown, p.text.trim().is_empty()) {
        (_, true) => "Type a chord or pick one below".to_string(),
        // The band plays slots in its own key: say which, as the quick buttons follow the key in force.
        (Some((name, true)), _) => format!("→ {name} · exact (in {band_key})"),
        (Some((name, false)), _) => format!("→ nearest: {name} (in {band_key})"),
        (None, false) => "Can't read that chord".to_string(),
    };
    ui.label(egui::RichText::new(line).color(t.text_dim));
    for row in [&p.triads, &p.sevenths] {
        let mut pick = None;
        ui.horizontal(|ui| {
            for name in row {
                if ui.add(egui::Button::selectable(p.text.trim() == name, name.as_str()).min_size(egui::vec2(46.0, 22.0))).clicked() {
                    pick = Some(name.clone());
                }
            }
        });
        if let Some(name) = pick {
            p.text = name;
        }
    }
    if !ok(ui, shown.is_some()) {
        return None;
    }
    let text = p.text.trim().to_string();
    Some(match p.chord {
        Some(id) => ("band.chord.set".to_string(), json!({"chord": id, "chord_text": text})),
        // `at` and `end` were placed here (on the half bar, or against an off-grid neighbour):
        // kept as they are, so a neighbour is neither cut nor left a gap.
        None => ("band.chord.add".to_string(), json!({"at": p.at, "end": p.end, "chord": text, "snap": false})),
    })
}

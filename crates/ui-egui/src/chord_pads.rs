//! Chord Pads (Window › Editors › Chord Pads…): eight pads, each a chord of the key on the Key
//! ruler at the playhead (else the band's): the seven triads of its scale (I ii iii IV V vi vii°
//! in major) and the dominant seventh, voiced from the octave the window sets. Holding a pad
//! plays its chord, letting go stops it. The notes go out on the Musical Typing port, so every
//! armed or input-monitored instrument track listening to it hears them and a take records them,
//! as Musical Typing's keys are.

use crate::SoundApp;
use soundcraft_band::chord_theory::scale_offset;

/// One pad: its name ("IV", "vii°") and the chord's notes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pad {
    pub name: String,
    pub notes: Vec<u8>,
}

const ROMAN: [&str; 7] = ["I", "II", "III", "IV", "V", "VI", "VII"];

/// The eight pads for the key `(tonic pitch class, scale)` with the tonic in the octave starting
/// at `base` (a C): the scale's seven triads, each named by its degree and quality, then V7.
pub fn pads(tonic: u8, scale: u8, base: u8) -> Vec<Pad> {
    let root = i32::from(base) + i32::from(tonic % 12);
    let deg = |d: i32| root + scale_offset(i32::from(scale), d);
    let note = |p: i32| u8::try_from(p.clamp(0, 127)).unwrap_or(60);
    let mut out: Vec<Pad> = (0..7)
        .map(|d| {
            let tones = [deg(d), deg(d + 2), deg(d + 4)];
            let (third, fifth) = (tones[1] - tones[0], tones[2] - tones[0]);
            let numeral = ROMAN.get(d as usize).copied().unwrap_or("?");
            let name = match (third, fifth) {
                (4, 7) => numeral.to_string(),
                (3, 7) => numeral.to_lowercase(),
                (3, 6) => format!("{}°", numeral.to_lowercase()),
                (4, 8) => format!("{numeral}+"),
                _ => numeral.to_string(),
            };
            Pad { name, notes: tones.iter().map(|p| note(*p)).collect() }
        })
        .collect();
    // V7: the fifth degree's major triad and a minor seventh, whatever the scale.
    let v = deg(4);
    out.push(Pad { name: "V7".into(), notes: [v, v + 4, v + 7, v + 10].iter().map(|p| note(*p)).collect() });
    out
}

/// Chord Pads' state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PadState {
    pub open: bool,
    /// The C the tonic's octave starts at.
    pub base: u8,
    /// The pad held down and the notes it started.
    pub held: Option<(usize, Vec<u8>)>,
}

impl Default for PadState {
    fn default() -> Self {
        PadState { open: false, base: 48, held: None }
    }
}

/// The pads for the key at the playhead.
pub fn current(app: &SoundApp) -> Vec<Pad> {
    let s = app.engine.session();
    let (pc, scale) = crate::session_key::key_pc_at(s, app.position().max(0));
    pads(pc, scale, app.pads.base)
}

/// Starts pad `i`'s chord (letting go of one already held).
pub fn press(app: &mut SoundApp, i: usize) {
    release(app);
    let Some(pad) = current(app).into_iter().nth(i) else { return };
    let vel = app.typing.velocity.clamp(1, 127);
    for n in &pad.notes {
        crate::musical_typing::send_live(app, [0x90, n & 0x7F, vel]);
    }
    app.pads.held = Some((i, pad.notes));
}

/// Lets the held pad go.
pub fn release(app: &mut SoundApp) {
    if let Some((_, notes)) = app.pads.held.take() {
        for n in notes {
            crate::musical_typing::send_live(app, [0x80, n & 0x7F, 0]);
        }
    }
}

/// The window.
pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.pads.open;
    if !open {
        release(app);
        return;
    }
    let key = crate::session_key::key_at(app.engine.session(), app.position().max(0));
    egui::Window::new("Chord Pads").open(&mut open).default_size(egui::vec2(520.0, 120.0)).show(ctx, |ui| {
        ui.horizontal(|ui| {
            ui.label(format!("Key: {key}"));
            ui.separator();
            ui.label(format!("Octave {}", i32::from(app.pads.base / 12) - 1));
            if ui.small_button("−").on_hover_text("An octave down").clicked() {
                app.pads.base = app.pads.base.saturating_sub(12).max(12);
            }
            if ui.small_button("+").on_hover_text("An octave up").clicked() {
                app.pads.base = (app.pads.base + 12).min(96);
            }
        });
        let mut down = None;
        ui.horizontal_wrapped(|ui| {
            for (i, pad) in current(app).into_iter().enumerate() {
                let names: Vec<String> = pad.notes.iter().map(|n| soundcraft_midi::note_name(*n)).collect();
                let lit = app.pads.held.as_ref().is_some_and(|h| h.0 == i);
                let b = egui::Button::new(egui::RichText::new(pad.name.clone()).size(16.0)).min_size(egui::vec2(56.0, 48.0)).selected(lit);
                let r = ui.add(b).on_hover_text(format!("Hold to play {}", names.join(" ")));
                if r.is_pointer_button_down_on() {
                    down = Some(i);
                }
            }
        });
        match (down, app.pads.held.as_ref().map(|h| h.0)) {
            (Some(i), Some(h)) if i == h => {}
            (Some(i), _) => press(app, i),
            (None, Some(_)) => release(app),
            (None, None) => {}
        }
        ui.label(egui::RichText::new("Plays on the Musical Typing input: arm or monitor an instrument track to hear and record it.").small());
    });
    if !open {
        release(app);
    }
    app.pads.open = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn c_major_gives_its_seven_triads_and_the_dominant_seventh() {
        let p = pads(0, 0, 60);
        let names: Vec<&str> = p.iter().map(|x| x.name.as_str()).collect();
        assert_eq!(names, ["I", "ii", "iii", "IV", "V", "vi", "vii°", "V7"]);
        assert_eq!(p[0].notes, vec![60, 64, 67]);
        assert_eq!(p[1].notes, vec![62, 65, 69]);
        assert_eq!(p[6].notes, vec![71, 74, 77]);
        assert_eq!(p[7].notes, vec![67, 71, 74, 77]);
    }

    #[test]
    fn a_minor_starts_on_a() {
        let p = pads(9, 1, 48);
        assert_eq!(p[0].name, "i");
        assert_eq!(p[0].notes, vec![57, 60, 64]);
        assert_eq!(p[1].name, "ii°");
    }
}

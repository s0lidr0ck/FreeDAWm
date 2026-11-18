//! A short first-run tour: a few cards, one at a time, on where things are. The desktop app opens
//! it once, on the first start with no settings saved yet; FreeDAWm › Tour of FreeDAWm opens
//! it again. It only shows text and changes nothing; Skip or the last card's Done closes it.

use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, vec2};

/// The cards: a title and what it says.
pub const STEPS: &[(&str, &str)] = &[
    (
        "Welcome to FreeDAWm",
        "A few cards on where things are. Nothing here changes your session; Skip closes the tour, and FreeDAWm › Tour of FreeDAWm opens it again.",
    ),
    (
        "Tracks and the Edit window",
        "Track › New… adds tracks. Drag audio or MIDI files from your computer onto the window to import them. The tools above the tracks select, trim and move; Edit › Undo takes back any step.",
    ),
    (
        "Recording",
        "Choose the audio interface in Setup › Playback Engine…. Click a track's R button to arm it, then the Record button in the toolbar. A take that was running when the app stopped is offered back the next time it starts.",
    ),
    (
        "Mixing",
        "Window › Mix shows a strip per track: inserts, sends, pan and fader. The built-in effects are in Window › Plugin Browser; drag one onto a track. File › Bounce Mix… writes the mix to a file, optionally at a loudness target.",
    ),
    (
        "The AI menu",
        "Stem separation, vocal tuning, transcription and the rest run on this computer. Each model is downloaded the first time you use it, with its licence shown first.",
    ),
    (
        "Scout",
        "Scout measures a mix or a channel and asks an AI model for advice, using your own API key. It only draws suggestions until you apply one, one at a time, each its own undo step.",
    ),
    (
        "Finding things",
        "Setup › Keyboard Shortcuts… lists the commands with their keys, and changes them. Scripts and AI agents can run every command too, from the command line or a running app.",
    ),
];

/// Opens the tour at its first card.
pub fn open(app: &mut SoundApp) {
    app.ui.tour_step = Some(0);
}

/// The tour card, while one is open.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(i) = app.ui.tour_step else { return };
    let Some((title, body)) = STEPS.get(i) else {
        app.ui.tour_step = None;
        return;
    };
    let t = Tokens::current();
    let last = i + 1 == STEPS.len();
    let mut next = Some(i);
    egui::Window::new("Tour of FreeDAWm")
        .id(egui::Id::new("freedawm-tour"))
        .anchor(Align2::CENTER_CENTER, vec2(0.0, 0.0))
        .collapsible(false)
        .resizable(false)
        .title_bar(false)
        .show(ctx, |ui| {
            ui.set_width(420.0);
            ui.label(egui::RichText::new(format!("{} of {}", i + 1, STEPS.len())).font(regular(11.0)).color(t.text_dim));
            ui.label(egui::RichText::new(*title).font(bold(16.0)).color(t.text));
            ui.add_space(4.0);
            ui.label(egui::RichText::new(*body).font(regular(13.0)).color(t.text));
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                if ui.button("Skip tour").clicked() {
                    next = None;
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.button(if last { "Done" } else { "Next" }).clicked() {
                        next = (!last).then_some(i + 1);
                    }
                    if i > 0 && ui.button("Back").clicked() {
                        next = Some(i - 1);
                    }
                });
            });
        });
    app.ui.tour_step = next;
}

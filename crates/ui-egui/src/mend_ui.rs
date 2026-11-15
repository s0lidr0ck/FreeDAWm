//! Mend's part of the module drawer: what its model is doing (loading, cleaning, not installed,
//! not keeping up), a button to download the model when it is missing, and its latency.

use crate::SoundApp;
use crate::theme::{Tokens, regular};
use egui::{Rect, Ui, pos2, vec2};
use soundcraft_dsp::plugins::mend::link::State;
use soundcraft_mix::StripMeter;
use soundcraft_model::Insert;

/// What Mend's readouts say, in the drawer's words: the line and whether it is a warning.
pub fn status(readouts: Option<&[f32; 8]>, desktop: bool, live: bool) -> (String, bool) {
    let Some(r) = readouts else { return ("Play to start it.".into(), false) };
    let state = State::from_u8(if r[0].is_finite() { r[0] as u8 } else { 0 });
    let load = if r[1].is_finite() { r[1] } else { 0.0 };
    let cleaning = r[2] >= 0.5;
    let latency = if r[3].is_finite() { r[3] } else { 0.0 };
    if live && latency <= 0.0 {
        return ("Live runs at 44.1 and 48 kHz only. The sound passes through; Studio works at this rate.".into(), true);
    }
    match state {
        State::Waiting if desktop => ("Starting…".into(), false),
        State::Waiting => ("Mend needs the desktop app: its model does not run in the browser. The sound passes through.".into(), true),
        State::Loading => ("Loading the model…".into(), false),
        State::NoModel => ("The Renaissance model (41 MB) is not installed. The sound passes through until it is.".into(), true),
        State::Failed => ("The model could not be loaded (the log says why). Trying again…".into(), true),
        State::Ready if cleaning => (format!("Cleaning · the model takes {load:.0} % of real time"), false),
        State::Ready if load > 95.0 => (format!("This computer cannot keep up ({load:.0} % of real time): the input is playing."), true),
        State::Ready if live => ("Ready".into(), false),
        State::Ready => ("Ready · the cleaned sound starts a few seconds into playback".into(), false),
    }
}

/// Mend's rows under the drawer's graph. Returns the y under them.
pub(crate) fn drawer_rows(app: &mut SoundApp, ui: &mut Ui, ins: &Insert, inner: Rect, y: f32, meters: Option<&StripMeter>) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let readouts = meters.and_then(|m| m.modules.iter().find(|m| m.module == ins.id)).map(|m| &m.readouts);
    let live = ins.params.get("mode").is_some_and(|v| *v >= 0.5);
    let (text, warn) = status(readouts, cfg!(not(target_arch = "wasm32")), live);
    let colour = if warn { t.text } else { t.text_dim };
    let galley = p.layout(text, regular(10.0), colour, inner.width());
    let h = galley.size().y;
    p.galley(pos2(inner.min.x, y), galley, colour);
    let mut y = y + h + 6.0;
    let missing = readouts.is_some_and(|r| State::from_u8(r[0] as u8) == State::NoModel);
    if missing && cfg!(not(target_arch = "wasm32")) {
        let at = Rect::from_min_size(pos2(inner.min.x, y), vec2(150.0, 22.0));
        let hover = "Downloads Smule Renaissance Small (MIT licence), the model Clean Vocal also uses. Mend starts by itself once it is in.";
        if ui.put(at, egui::Button::new("Download the model…")).on_hover_text(hover).clicked() {
            download(app);
        }
        y = at.max.y + 6.0;
    }
    let note = match (live, readouts.map(|r| r[3]).filter(|s| s.is_finite() && *s > 0.0)) {
        (true, Some(secs)) => {
            format!(
                "Live: {:.0} ms of latency. It takes noise off, not the room, and can pump on held sung notes. Studio cleans more.",
                secs * 1000.0
            )
        }
        (true, None) => "Live: about 60 ms of latency. It takes noise off, not the room. Studio cleans more.".to_string(),
        (false, secs) => format!(
            "Studio: {:.1} s of latency, made up for on playback. It cannot be monitored through: use Live while recording.",
            secs.unwrap_or(2.9)
        ),
    };
    let galley = p.layout(note, regular(9.5), t.text_dim, inner.width());
    let h = galley.size().y;
    p.galley(pos2(inner.min.x, y), galley, t.text_dim);
    y + h + 8.0
}

#[cfg(not(target_arch = "wasm32"))]
fn download(app: &mut SoundApp) {
    // The model Mend runs (the one Clean Vocal uses).
    crate::ml_downloads::request(app, &["srs_small".to_string()], false);
}

#[cfg(target_arch = "wasm32")]
fn download(_: &mut SoundApp) {}

#[cfg(test)]
mod tests {
    use super::*;

    fn r(state: f32, load: f32, cleaning: f32) -> [f32; 8] {
        [state, load, cleaning, 2.86, 0.0, f32::NAN, f32::NAN, f32::NAN]
    }

    #[test]
    fn the_status_line_says_what_the_model_is_doing() {
        assert_eq!(status(None, true, false), ("Play to start it.".into(), false));
        assert_eq!(status(Some(&r(0.0, 0.0, 0.0)), true, false).0, "Starting…");
        assert!(status(Some(&r(0.0, 0.0, 0.0)), false, false).0.contains("desktop app"));
        assert_eq!(status(Some(&r(1.0, 0.0, 0.0)), true, false).0, "Loading the model…");
        assert_eq!(status(Some(&r(2.0, 64.0, 1.0)), true, false), ("Cleaning · the model takes 64 % of real time".into(), false));
        assert!(status(Some(&r(2.0, 130.0, 0.0)), true, false).1, "not keeping up is a warning");
        assert!(status(Some(&r(2.0, 50.0, 0.0)), true, false).0.starts_with("Ready"));
        assert!(status(Some(&r(3.0, 0.0, 0.0)), true, false).0.contains("not installed"));
        assert!(status(Some(&r(4.0, 0.0, 0.0)), true, false).0.contains("could not be loaded"));
        assert_eq!(status(Some(&r(f32::NAN, f32::NAN, f32::NAN)), true, false).0, "Starting…");
        assert_eq!(status(Some(&r(2.0, 20.0, 0.0)), true, true).0, "Ready");
        let mut no_live = r(2.0, 0.0, 0.0);
        no_live[3] = 0.0;
        assert!(status(Some(&no_live), true, true).0.contains("44.1 and 48 kHz"));
    }
}

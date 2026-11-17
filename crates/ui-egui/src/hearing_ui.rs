//! The Hearing Test window: makes a hearing profile for the master fader's EAR key from what the
//! listener hears (`soundcraft_dsp::hearing_test`), with no calibrated equipment.
//!
//! One answer a step, twenty steps: set a listening level, centre each of ten bands of noise
//! between the ears, then make each band as loud as the 1 kHz one. The sounds are previews: they
//! play over the mix straight to the device and never pass through the monitor stage, so an ear
//! correction that is already on does not colour the test. The result opens in the Ear Profile
//! window to be looked over and saved.

use crate::SoundApp;
use crate::monitor_ui::Draft;
use crate::theme::{Tokens, bold, regular};
use egui::Align2;
use serde_json::{Value, json};
use soundcraft_dsp::hearing_test::{
    Answers, BALANCE_DB, LOUDNESS_DB, MAX_DBFS, REFERENCE, REFERENCE_DBFS, TEST_HZ, band_dbfs, band_noise, correction,
};

/// The rate the test's sounds are made at.
const RATE: u32 = 48_000;
/// Steps: the level, a balance per band, a loudness per band but the reference.
pub const STEPS: usize = 1 + TEST_HZ.len() + TEST_HZ.len() - 1;

/// What a step asks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    /// Set the headphone volume on the reference band.
    Level,
    /// Centre band `0` between the ears.
    Balance(usize),
    /// Make band `0` as loud as the reference band.
    Loudness(usize),
}

/// The step at `step` (past the end: the last).
pub fn kind(step: usize) -> Kind {
    let n = TEST_HZ.len();
    match step {
        0 => Kind::Level,
        s if s <= n => Kind::Balance(s - 1),
        s => {
            let i = (s - n - 1).min(n - 2);
            Kind::Loudness(if i >= REFERENCE { i + 1 } else { i })
        }
    }
}

/// A test under way.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct HearingTest {
    pub step: usize,
    pub answers: Answers,
    pub name: String,
    pub headphones: String,
    /// When the step's sound is played next (egui's clock); 0 = now.
    next_play: f64,
}

fn hz_text(hz: f32) -> String {
    if hz >= 1000.0 { format!("{} kHz", hz / 1000.0) } else { format!("{hz} Hz") }
}

impl HearingTest {
    /// The answer the current step sets, with its range.
    fn answer(&mut self) -> Option<(&mut f32, std::ops::RangeInclusive<f32>)> {
        match kind(self.step) {
            Kind::Level => None,
            Kind::Balance(i) => self.answers.balance.get_mut(i).map(|v| (v, BALANCE_DB)),
            Kind::Loudness(i) => self.answers.loudness.get_mut(i).map(|v| (v, LOUDNESS_DB)),
        }
    }

    /// One band's burst on both sides: its level as answered so far, the balance answer split
    /// between the ears, and never over the ceiling on either.
    fn burst(&self, band: usize, frames: usize, seed: u32) -> [Vec<f32>; 2] {
        let hz = TEST_HZ.get(band).copied().unwrap_or(1000.0);
        let balance = self.answers.balance.get(band).copied().unwrap_or(0.0).clamp(*BALANCE_DB.start(), *BALANCE_DB.end());
        let added = if band == REFERENCE { 0.0 } else { self.answers.loudness.get(band).copied().unwrap_or(0.0) };
        let level = band_dbfs(band, added).min(MAX_DBFS - balance.abs() / 2.0);
        let gain = |db: f32| 10f32.powf(db / 20.0);
        let noise = band_noise(hz, RATE as f32, frames, seed);
        [noise.iter().map(|v| v * gain(level - balance / 2.0)).collect(), noise.iter().map(|v| v * gain(level + balance / 2.0)).collect()]
    }

    /// The current step's sound (left, right at [`RATE`]): played again and again while the step
    /// is shown.
    pub fn clip(&self) -> [Vec<f32>; 2] {
        let secs = |s: f32| (s * RATE as f32) as usize;
        let seed = self.step as u32 + 1;
        let mut out = [Vec::new(), Vec::new()];
        let mut add = |part: [Vec<f32>; 2], gap: usize| {
            for (o, p) in out.iter_mut().zip(part) {
                o.extend(p);
                o.extend(std::iter::repeat_n(0.0, gap));
            }
        };
        match kind(self.step) {
            Kind::Level => {
                let n = band_noise(1000.0, RATE as f32, secs(0.7), seed);
                let g = 10f32.powf(REFERENCE_DBFS / 20.0);
                let side: Vec<f32> = n.iter().map(|v| v * g).collect();
                add([side.clone(), side], secs(0.4));
            }
            Kind::Balance(i) => add(self.burst(i, secs(0.6), seed), secs(0.4)),
            Kind::Loudness(i) => {
                add(self.burst(REFERENCE, secs(0.5), seed), secs(0.15));
                add(self.burst(i, secs(0.5), seed.wrapping_add(100)), secs(0.5));
            }
        }
        out
    }

    /// The hearing profile the answers so far ask for, as the Ear Profile window edits it.
    pub fn draft(&self) -> Draft {
        let (left, right) = correction(&self.answers);
        Draft { name: self.name.clone(), headphones: self.headphones.clone(), left, right }
    }

    fn state(&self) -> Value {
        let (what, band) = match kind(self.step) {
            Kind::Level => ("level", REFERENCE),
            Kind::Balance(i) => ("balance", i),
            Kind::Loudness(i) => ("loudness", i),
        };
        let value = match kind(self.step) {
            Kind::Level => 0.0,
            Kind::Balance(i) => self.answers.balance.get(i).copied().unwrap_or(0.0),
            Kind::Loudness(i) => self.answers.loudness.get(i).copied().unwrap_or(0.0),
        };
        json!({"open": true, "step": self.step + 1, "steps": STEPS, "kind": what, "hz": TEST_HZ.get(band), "value": value})
    }
}

fn open(app: &mut SoundApp) {
    if app.hearing.is_none() {
        let current = crate::monitor_ui::dir(app).and_then(|d| soundcraft_engine::monitor_profiles::find_hearing(&d, &app.ui.monitor_ear_profile));
        let (name, headphones) = current.map(|h| (h.name, h.headphones)).unwrap_or_default();
        app.hearing = Some(HearingTest { name, headphones, ..HearingTest::default() });
    }
    app.ui.show_hearing_test = true;
}

fn close(app: &mut SoundApp) {
    app.ui.show_hearing_test = false;
    if app.hearing.take().is_some()
        && let Some(p) = &app.player
    {
        p.preview(Vec::new(), RATE);
    }
}

/// The last step done: the result goes to the Ear Profile window to be looked over and saved.
fn finish(app: &mut SoundApp) {
    let Some(test) = app.hearing.clone() else { return };
    close(app);
    crate::monitor_ui::load_draft(app, test.draft());
    app.ui.status = "Hearing test done: look over the result and Save it".into();
}

/// UI commands handled here: `window.hearing_test {value?}`, `hearing_test.set {value}`,
/// `hearing_test.next`, `hearing_test.back` and `hearing_test.state`. None edits the session.
pub fn run(app: &mut SoundApp, id: &str, p: &Value) -> Option<Result<Value, String>> {
    let closed = || json!({"open": false, "steps": STEPS});
    Some(match id {
        "window.hearing_test" => {
            if p.get("value").and_then(Value::as_bool).unwrap_or(!app.ui.show_hearing_test) {
                open(app);
                if let (Some(t), Some(name)) = (app.hearing.as_mut(), p.get("name").and_then(Value::as_str)) {
                    t.name = name.trim().to_string();
                }
                if let (Some(t), Some(h)) = (app.hearing.as_mut(), p.get("headphones").and_then(Value::as_str)) {
                    t.headphones = h.trim().to_string();
                }
            } else {
                close(app);
            }
            Ok(app.hearing.as_ref().map_or_else(closed, HearingTest::state))
        }
        "hearing_test.state" => Ok(app.hearing.as_ref().map_or_else(closed, HearingTest::state)),
        "hearing_test.set" | "hearing_test.next" | "hearing_test.back" => {
            let Some(test) = app.hearing.as_mut() else { return Some(Err("the hearing test is not open (window.hearing_test)".into())) };
            match id {
                "hearing_test.set" => {
                    let Some(v) = p.get("value").and_then(Value::as_f64).filter(|v| v.is_finite()) else {
                        return Some(Err("hearing_test.set needs a `value` in dB".into()));
                    };
                    let Some((slot, range)) = test.answer() else {
                        return Some(Err("this step has nothing to set: go on with hearing_test.next".into()));
                    };
                    *slot = (v as f32).clamp(*range.start(), *range.end());
                    test.next_play = 0.0;
                }
                "hearing_test.back" => {
                    test.step = test.step.saturating_sub(1);
                    test.next_play = 0.0;
                }
                _ => {
                    if test.step + 1 >= STEPS {
                        finish(app);
                        return Some(Ok(json!({"open": false, "steps": STEPS, "finished": true})));
                    }
                    test.step += 1;
                    test.next_play = 0.0;
                }
            }
            Ok(app.hearing.as_ref().map_or_else(closed, HearingTest::state))
        }
        _ => return None,
    })
}

/// Plays the step's sound, again whenever it has ended (or an answer changed).
fn play(app: &mut SoundApp, ctx: &egui::Context) {
    let now = ctx.input(|i| i.time);
    let Some(test) = app.hearing.as_mut() else { return };
    if now < test.next_play {
        ctx.request_repaint_after(std::time::Duration::from_secs_f64((test.next_play - now).clamp(0.01, 0.25)));
        return;
    }
    let [l, r] = test.clip();
    test.next_play = now + l.len() as f64 / f64::from(RATE);
    if let Some(p) = &app.player {
        p.preview(vec![l, r], RATE);
    }
    ctx.request_repaint_after(std::time::Duration::from_millis(100));
}

/// The Hearing Test window.
pub fn window(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.ui.show_hearing_test {
        return;
    }
    if app.hearing.is_none() {
        open(app);
    }
    play(app, ctx);
    let Some(mut test) = app.hearing.clone() else { return };
    let before = test.clone();
    let t = Tokens::current();
    let playing = app.engine.transport.playing;
    let silent = app.player.is_none();
    let (mut still_open, mut next, mut back) = (true, false, false);
    egui::Window::new(egui::RichText::new("Hearing Test").font(bold(13.0)))
        .id(egui::Id::new("hearing_test_window"))
        .open(&mut still_open)
        .pivot(Align2::CENTER_CENTER)
        .default_pos(ctx.content_rect().center())
        .default_width(440.0)
        .resizable(false)
        .collapsible(false)
        .show(ctx, |ui| {
            ui.set_width(440.0);
            let dim = |text: &str| egui::RichText::new(text).font(regular(11.0)).color(t.text_dim);
            ui.label(egui::RichText::new(format!("Step {} of {STEPS}", test.step + 1)).font(bold(12.0)));
            ui.add(egui::ProgressBar::new((test.step + 1) as f32 / STEPS as f32).desired_height(4.0));
            ui.add_space(6.0);
            if silent {
                ui.label(egui::RichText::new("There is no audio device: the test cannot play its sounds.").color(t.accent));
            } else if playing {
                ui.label(egui::RichText::new("Stop playback: the test's sounds play over the mix.").color(t.accent));
            }
            match kind(test.step) {
                Kind::Level => {
                    ui.label(egui::RichText::new("Set your listening level").font(bold(13.0)));
                    ui.label(dim(
                        "Wear the headphones you mix on. A band of noise is repeating: set your headphone volume so it is clearly heard at a comfortable, normal listening level, then leave the volume alone until the test is over. A quiet room matters more than anything else here.",
                    ));
                    ui.add_space(6.0);
                    egui::Grid::new("hearing_test_names").num_columns(2).show(ui, |ui| {
                        ui.label("Profile name");
                        ui.add(egui::TextEdit::singleline(&mut test.name).desired_width(260.0).hint_text("Me on my headphones"));
                        ui.end_row();
                        ui.label("Headphones");
                        ui.add(egui::TextEdit::singleline(&mut test.headphones).desired_width(260.0));
                        ui.end_row();
                    });
                }
                Kind::Balance(i) => {
                    let hz = hz_text(TEST_HZ.get(i).copied().unwrap_or(1000.0));
                    ui.label(egui::RichText::new(format!("Centre the sound: {hz}")).font(bold(13.0)));
                    ui.label(dim(
                        "The noise should seem to come from the middle of your head. If it sits to one side, drag the slider toward the other side until it is centred. Closing your eyes helps.",
                    ));
                    ui.add_space(6.0);
                    if let Some((v, range)) = test.answer() {
                        ui.horizontal(|ui| {
                            ui.label("Left");
                            ui.spacing_mut().slider_width = 300.0;
                            ui.add(egui::Slider::new(v, range).step_by(0.5).show_value(false)).on_hover_text("Raises one ear and lowers the other");
                            ui.label("Right");
                        });
                        ui.label(dim(&format!("Right ear {v:+.1} dB against the left")));
                    }
                }
                Kind::Loudness(i) => {
                    let hz = hz_text(TEST_HZ.get(i).copied().unwrap_or(1000.0));
                    ui.label(egui::RichText::new(format!("Match the loudness: {hz}")).font(bold(13.0)));
                    ui.label(dim(
                        "Two sounds alternate: first the reference, then the one you set. Drag the slider until the second is as loud as the first. Judge loudness only; they will never sound alike.",
                    ));
                    ui.add_space(6.0);
                    if let Some((v, range)) = test.answer() {
                        ui.horizontal(|ui| {
                            ui.label("Quieter");
                            ui.spacing_mut().slider_width = 290.0;
                            ui.add(egui::Slider::new(v, range).step_by(0.5).show_value(false));
                            ui.label("Louder");
                        });
                        let capped = band_dbfs(i, *v) >= MAX_DBFS;
                        let note = if capped { format!("{v:+.1} dB (as loud as the test goes)") } else { format!("{v:+.1} dB") };
                        ui.label(dim(&note));
                    }
                }
            }
            ui.add_space(8.0);
            ui.horizontal(|ui| {
                back = ui.add_enabled(test.step > 0, egui::Button::new("Back")).clicked();
                next = ui.button(if test.step + 1 >= STEPS { "Finish" } else { "Next" }).clicked();
            });
        });
    // An answer that moved is heard at once.
    if test.answers != before.answers {
        test.next_play = 0.0;
        ctx.request_repaint();
    }
    app.hearing = Some(test);
    if back {
        let _ = app.run("hearing_test.back", json!({}));
    }
    if next {
        let _ = app.run("hearing_test.next", json!({}));
    }
    if !still_open {
        close(app);
    }
}

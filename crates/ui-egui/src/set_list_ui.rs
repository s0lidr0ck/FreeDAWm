//! Window › Set List (parity section 8, the show page, step 1): the songs of the show, each a
//! memory location holding a range, in the order they are played. A song plays from its start
//! to its end (`setlist.play`); then the transport waits on the next song, or with Go On plays
//! it at once. Stage View fills the app with the song playing and the next one, big enough to
//! read from a stage, with the counter and Play Next / Stop.
//!
//! Following the set is done here, frame by frame ([`follow`]): the audio thread stops the song
//! at its end, and the next one starts on the following frame (a short gap, not a seamless
//! join).

use crate::SoundApp;
use egui::{Align2, Color32, FontId, vec2};
use serde_json::json;
use soundcraft_engine::cmd::set_list::{next_song, song};
use soundcraft_model::MarkerKind;
use soundcraft_time::Samples;

/// The song playing from the set list: its index, its end, and where the transport was last
/// seen playing it.
#[derive(Clone, Copy, Debug, PartialEq)]
struct Running {
    index: usize,
    end: Samples,
    last: Samples,
}

/// What the set list stands on: the song playing, or the one the transport waits on.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct Cued(usize);

/// Stage View is on.
#[derive(Clone, Copy, Debug, PartialEq, Default)]
struct Stage(bool);

fn running_id() -> egui::Id {
    egui::Id::new("set_list_running")
}

fn cued_id() -> egui::Id {
    egui::Id::new("set_list_cued")
}

fn stage_id() -> egui::Id {
    egui::Id::new("set_list_stage")
}

/// Plays song `i` of the set list and follows it.
pub fn play(app: &mut SoundApp, ctx: &egui::Context, i: usize) {
    match app.run("setlist.play", json!({"index": i})) {
        Ok(v) => {
            let r = Running { index: i, end: v["end"].as_i64().unwrap_or(0), last: v["start"].as_i64().unwrap_or(0) };
            ctx.data_mut(|d| {
                d.insert_temp(running_id(), r);
                d.insert_temp(cued_id(), Cued(i));
            });
        }
        Err(e) => app.ui.status = e,
    }
}

/// Each frame, after the transport is read: while a song of the set list plays, keeps up with
/// it; once it has stopped at its end, plays the next song (Go On) or waits on it at its start.
/// Stopped by hand before the end, the set waits on that song.
pub fn follow(app: &mut SoundApp, ctx: &egui::Context) {
    let Some(mut r) = ctx.data(|d| d.get_temp::<Running>(running_id())) else { return };
    let t = app.engine.transport;
    if t.playing {
        r.last = t.position;
        ctx.data_mut(|d| d.insert_temp(running_id(), r));
        return;
    }
    ctx.data_mut(|d| d.remove::<Running>(running_id()));
    let s = app.engine.session();
    let near = s.sample_rate.samples(0.5);
    if r.last < r.end.saturating_sub(near) && t.position < r.end {
        return;
    }
    let Some(next) = next_song(s, r.index) else { return };
    if s.set_list.get(r.index).is_some_and(|x| x.go_on) {
        play(app, ctx, next);
    } else {
        let start = song(s, next).map_or(0, |m| m.start.max(0));
        ctx.data_mut(|d| d.insert_temp(cued_id(), Cued(next)));
        let _ = app.run("transport.locate", json!({"at": start}));
    }
}

/// The song the set list stands on, when there is one.
fn cued(app: &SoundApp, ctx: &egui::Context) -> Option<usize> {
    let s = app.engine.session();
    let c = ctx.data(|d| d.get_temp::<Cued>(cued_id())).map_or(0, |c| c.0);
    if song(s, c).is_some() { Some(c) } else { (0..s.set_list.len()).find(|k| song(s, *k).is_some()) }
}

fn length_text(s: &soundcraft_model::Session, len: Samples) -> String {
    let secs = (len.max(0) as f64 / s.sample_rate.as_f64()).round() as i64;
    format!("{}:{:02}", secs / 60, secs % 60)
}

/// The window, and Stage View over the whole app while it is on.
pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    stage(app, ctx);
    let mut open = app.ui.show_set_list;
    if !open {
        return;
    }
    let mut run: Option<(&'static str, serde_json::Value)> = None;
    let mut play_index = None;
    let at = cued(app, ctx);
    egui::Window::new("Set List").open(&mut open).default_size(vec2(380.0, 420.0)).show(ctx, |ui| {
        let s = app.engine.session();
        if s.set_list.is_empty() {
            ui.label(egui::RichText::new("No songs yet. A song is a memory location that keeps a selection: select a song's range, make a memory location of it, and add it below.").weak());
        }
        let n = s.set_list.len();
        egui::Grid::new("set_list_rows").num_columns(6).striped(true).show(ui, |ui| {
            for (i, x) in s.set_list.iter().enumerate() {
                let here = at == Some(i);
                let (name, len) = match song(s, i) {
                    Some(m) => (m.name.clone(), length_text(s, m.end - m.start)),
                    None => ("(memory location deleted)".to_string(), String::new()),
                };
                ui.label(egui::RichText::new(format!("{}. {name}", i + 1)).strong().color(if here { ui.visuals().selection.stroke.color } else { ui.visuals().text_color() }));
                ui.label(len);
                let mut go_on = x.go_on;
                if ui.checkbox(&mut go_on, "Go On").on_hover_text("At its end the next song plays at once (else the transport waits on it)").changed() {
                    run = Some(("setlist.go_on", json!({"index": i, "value": go_on})));
                }
                if ui.small_button("▶").on_hover_text("Play this song").clicked() {
                    play_index = Some(i);
                }
                ui.horizontal(|ui| {
                    if ui.add_enabled(i > 0, egui::Button::new("↑").small()).on_hover_text("Earlier in the set").clicked() {
                        run = Some(("setlist.move", json!({"index": i, "to": i - 1})));
                    }
                    if ui.add_enabled(i + 1 < n, egui::Button::new("↓").small()).on_hover_text("Later in the set").clicked() {
                        run = Some(("setlist.move", json!({"index": i, "to": i + 1})));
                    }
                });
                if ui.small_button("✕").on_hover_text("Take this song out of the set").clicked() {
                    run = Some(("setlist.remove", json!({"index": i})));
                }
                ui.end_row();
            }
        });
        ui.separator();
        let ranges: Vec<(u64, String)> =
            s.markers.iter().filter(|m| m.kind == MarkerKind::Selection && m.end > m.start).map(|m| (m.id.0, format!("{}. {}", m.number, m.name))).collect();
        ui.add_enabled_ui(!ranges.is_empty(), |ui| {
            ui.menu_button("Add Song", |ui| {
                for (id, label) in &ranges {
                    if ui.button(label).clicked() {
                        run = Some(("setlist.add", json!({"location": id})));
                        ui.close();
                    }
                }
            })
            .response
            .on_disabled_hover_text("Make a memory location that keeps a selection first (Window › Memory Locations)");
        });
        ui.horizontal(|ui| {
            if ui.add_enabled(at.is_some(), egui::Button::new("Play Set")).on_hover_text("Play the song the set stands on").clicked() {
                play_index = at;
            }
            if ui.button("Stage View").on_hover_text("The song and the next one, big, over the whole window (Esc leaves it)").clicked() {
                ctx.data_mut(|d| d.insert_temp(stage_id(), Stage(true)));
            }
        });
    });
    app.ui.show_set_list = open;
    if let Some((cmd, p)) = run
        && let Err(e) = app.run(cmd, p)
    {
        app.ui.status = e;
    }
    if let Some(i) = play_index {
        play(app, ctx, i);
    }
}

/// Stage View: the song playing (or waited on) and the next one over the whole app, with the
/// counter; Play / Stop and leave.
fn stage(app: &mut SoundApp, ctx: &egui::Context) {
    if !ctx.data(|d| d.get_temp::<Stage>(stage_id())).unwrap_or_default().0 {
        return;
    }
    let at = cued(app, ctx);
    let playing = app.engine.transport.playing;
    let (now, next, counter) = {
        let s = app.engine.session();
        let name = |i: Option<usize>| i.and_then(|i| song(s, i)).map(|m| m.name.clone());
        let pos = app.engine.transport.position;
        (name(at), name(at.and_then(|i| next_song(s, i))), s.format_position(pos, s.edit.main_counter))
    };
    let mut leave = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    let mut play_now = false;
    let mut stop = false;
    let screen = ctx.content_rect();
    egui::Area::new(egui::Id::new("set_list_stage_view")).order(egui::Order::Foreground).fixed_pos(screen.min).show(ctx, |ui| {
        ui.set_min_size(screen.size());
        let p = ui.painter();
        p.rect_filled(screen, 0.0, Color32::from_rgb(12, 12, 14));
        let c = screen.center();
        let big = (screen.height() * 0.12).clamp(28.0, 140.0);
        p.text(
            c - vec2(0.0, big * 1.2),
            Align2::CENTER_CENTER,
            now.as_deref().unwrap_or("Set list is empty"),
            FontId::proportional(big),
            Color32::WHITE,
        );
        let small = big * 0.4;
        let next_text = next.map_or_else(|| "Last song".to_string(), |n| format!("Next: {n}"));
        p.text(c + vec2(0.0, small * 0.8), Align2::CENTER_CENTER, next_text, FontId::proportional(small), Color32::from_gray(170));
        p.text(c + vec2(0.0, small * 2.4), Align2::CENTER_CENTER, counter, FontId::monospace(small), Color32::from_gray(200));
        let row = egui::Rect::from_center_size(egui::pos2(c.x, screen.max.y - 60.0), vec2(360.0, 44.0));
        ui.scope_builder(egui::UiBuilder::new().max_rect(row), |ui| {
            ui.horizontal_centered(|ui| {
                let label = if playing { "Stop" } else { "Play" };
                if ui.add_sized(vec2(110.0, 40.0), egui::Button::new(egui::RichText::new(label).size(20.0))).clicked() {
                    if playing { stop = true } else { play_now = true }
                }
                if ui.add_sized(vec2(110.0, 40.0), egui::Button::new(egui::RichText::new("Leave").size(20.0))).on_hover_text("Esc").clicked() {
                    leave = true;
                }
            });
        });
    });
    if leave {
        ctx.data_mut(|d| d.remove::<Stage>(stage_id()));
    }
    if stop {
        let _ = app.run("transport.stop", json!({}));
    }
    if play_now && let Some(i) = at {
        play(app, ctx, i);
    }
}

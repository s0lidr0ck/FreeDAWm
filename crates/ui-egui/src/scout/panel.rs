//! Window ▸ Scout: the captures (newest first, ticked to go with the question) and the session
//! goal on the left; the conversation and the question box with Send and Copy on the right.
//! Draws from the session each frame (cheap: names and a few numbers); the package is built only
//! on Copy.

use super::{Tab, ask, clock, guides, profiles};
use crate::SoundApp;
use crate::theme::{Tokens, bold, regular};
use egui::{Align, Color32, CornerRadius, Layout, Margin, RichText, Sense, Stroke, Ui, vec2};
use serde_json::json;
use soundcraft_model::{Guide, ScoutCapture, ScoutTurn, Session};

/// The empty capture list.
pub const NO_CAPTURES: &str = "Select tracks, buses or the master and press Scout";
/// The empty conversation without an API key.
pub const NO_KEY: &str = "Add your Anthropic API key (Scout ▸ Settings) or use Copy";

const LEFT_W: f32 = 300.0;

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    if !app.scout.show {
        return;
    }
    let mut open = true;
    egui::Window::new("Scout").id(egui::Id::new("scout_panel")).open(&mut open).default_size(vec2(960.0, 600.0)).min_size(vec2(640.0, 420.0)).show(
        ctx,
        |ui| {
            tabs(app, ui);
            if app.scout.tab == Tab::Profiles {
                profiles::show(app, ui);
                return;
            }
            let rect = ui.available_rect_before_wrap();
            let left = egui::Rect::from_min_size(rect.min, vec2(LEFT_W, rect.height()));
            let right = egui::Rect::from_min_max(egui::pos2(left.max.x + 13.0, rect.min.y), rect.max);
            let t = Tokens::current();
            ui.painter().vline(left.max.x + 6.0, rect.y_range(), Stroke::new(1.0, t.border));
            let mut l = ui.new_child(egui::UiBuilder::new().max_rect(left).layout(Layout::top_down(Align::Min)));
            captures(app, &mut l);
            let mut r = ui.new_child(egui::UiBuilder::new().max_rect(right).layout(Layout::top_down(Align::Min)));
            conversation(app, &mut r);
            ui.allocate_rect(rect, Sense::hover());
        },
    );
    if !open {
        app.scout.show = false;
    }
}

/// Captures | Profiles.
fn tabs(app: &mut SoundApp, ui: &mut Ui) {
    ui.horizontal(|ui| {
        for (tab, label) in [(Tab::Captures, "Captures"), (Tab::Profiles, "Profiles")] {
            if ui.selectable_label(app.scout.tab == tab, RichText::new(label).font(bold(12.0))).clicked() {
                app.scout.tab = tab;
            }
        }
    });
    ui.add_space(4.0);
}

fn chip(ui: &mut Ui, text: &str, fg: Color32, stroke: Color32) -> egui::Response {
    egui::Frame::NONE
        .stroke(Stroke::new(1.0, stroke))
        .corner_radius(CornerRadius::same(9))
        .inner_margin(Margin::symmetric(7, 1))
        .show(ui, |ui| ui.label(RichText::new(text).font(regular(10.5)).color(fg)))
        .response
}

/// Which spectrum bands a capture was measured with, as "v{N} bands": dim for the current
/// design, yellow with a warning for v1 (stored before the 6th-order Butterworth bands of
/// 2026-10-21), whose spectrum does not compare directly with a later one.
fn bands_chip(ui: &mut Ui, version: u32) {
    let t = Tokens::current();
    let text = format!("v{version} bands");
    if version >= 2 {
        chip(ui, &text, t.text_dim, t.border_light);
    } else {
        let r = egui::Frame::NONE
            .stroke(Stroke::new(1.0, t.meter_yellow))
            .corner_radius(CornerRadius::same(9))
            .inner_margin(Margin::symmetric(7, 1))
            .show(ui, |ui| ui.label(RichText::new(text).font(regular(10.5)).color(t.meter_yellow)));
        r.response.on_hover_text(V1_BANDS_NOTE);
    }
}

/// The hover on a v1 capture's chip.
const V1_BANDS_NOTE: &str = "Measured with Scout's older 2nd-order bands: its spectrum does not compare directly with a v2 capture. Cuts read shallower; broadband material reads about 1.5 dB higher below 5 kHz, about the same near 10 kHz, and lower in the top three bands (up to 6-7 dB at 20 kHz); its tilt reads about 0.1 dB/oct darker";

/// `m:ss` of a sample position.
fn at(c: &ScoutCapture, s: i64) -> String {
    clock(u64::try_from(s).unwrap_or(0), c.sample_rate)
}

/// The captured length, the played span, the rate and the tempo.
fn facts(c: &ScoutCapture) -> String {
    let len: i64 = c.played.iter().map(|r| r.end.saturating_sub(r.start).max(0)).fold(0, i64::saturating_add);
    let mut parts = vec![clock(u64::try_from(len).unwrap_or(0), c.sample_rate)];
    if let (Some(a), Some(b)) = (c.played.first(), c.played.last()) {
        parts.push(format!("{}–{}", at(c, a.start), at(c, b.end)));
    }
    parts.push(format!("{} kHz", (c.sample_rate / 100.0).round() / 10.0));
    if c.tempo > 0.0 {
        parts.push(format!("{} BPM", (c.tempo * 10.0).round() / 10.0));
    }
    parts.join(" · ")
}

fn captures(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let session = app.engine.session_arc();
    let same = soundcraft_engine::cmd::scout_same_as(&session.scout);
    let goal_h = 118.0;
    let list_h = (ui.available_height() - goal_h).max(80.0);
    ui.label(RichText::new("CAPTURES").font(bold(10.5)).color(t.text_dim));
    ui.add_space(4.0);
    egui::ScrollArea::vertical().id_salt("scout_captures").max_height(list_h).min_scrolled_height(list_h).auto_shrink([false, false]).show(
        ui,
        |ui| {
            if session.scout.captures.is_empty() {
                ui.add_space(24.0);
                ui.vertical_centered(|ui| ui.label(RichText::new(NO_CAPTURES).font(regular(12.0)).color(t.text_dim)));
                return;
            }
            for (i, c) in session.scout.captures.iter().enumerate().rev() {
                let ticked = app.scout.is_ticked(&session, c.id);
                let same_as = same.get(i).copied().flatten().and_then(|id| session.scout.captures.iter().find(|x| x.id == id));
                egui::Frame::NONE
                    .fill(t.panel_bg2)
                    .stroke(Stroke::new(1.0, if ticked { t.accent } else { t.border }))
                    .corner_radius(CornerRadius::same(6))
                    .inner_margin(Margin::same(8))
                    .show(ui, |ui| {
                        ui.set_width(ui.available_width());
                        capture_card(app, ui, c, ticked, same_as);
                    });
                ui.add_space(6.0);
            }
        },
    );
    ui.add_space(6.0);
    ui.label(RichText::new("Goal for this session").font(regular(11.0)).color(t.text_dim));
    let mut goal = session.scout.goal.clone();
    let resp = ui.add(egui::TextEdit::multiline(&mut goal).desired_rows(3).desired_width(f32::INFINITY).hint_text("What should this mix do?"));
    if resp.changed() {
        // One undo step per typing burst.
        if let Err(e) = app.engine.execute_merged("scout.goal", &json!({"text": goal}), "scout_goal") {
            app.ui.status = e.to_string();
        }
    }
    if resp.lost_focus() {
        app.engine.end_merge();
    }
}

fn capture_card(app: &mut SoundApp, ui: &mut Ui, c: &ScoutCapture, ticked: bool, same_as: Option<&ScoutCapture>) {
    let t = Tokens::current();
    ui.horizontal(|ui| {
        let mut on = ticked;
        if ui.checkbox(&mut on, "").on_hover_text("Send this capture with the question").changed() {
            let s = app.engine.session_arc();
            app.scout.tick(&s, c.id, on);
        }
        let renaming = app.scout.renaming.as_ref().is_some_and(|(id, _)| *id == c.id);
        if renaming {
            let mut done = false;
            if let Some((_, name)) = &mut app.scout.renaming {
                let r = ui.add(egui::TextEdit::singleline(name).desired_width(150.0).font(bold(12.5)));
                r.request_focus();
                done = r.lost_focus();
            }
            if done && let Some((id, name)) = app.scout.renaming.take() {
                let _ = app.run("scout.rename", json!({"capture": id, "name": name}));
            }
        } else {
            let r = ui.add(egui::Label::new(RichText::new(&c.name).font(bold(12.5)).color(t.text)).sense(Sense::click()));
            if r.on_hover_text("Double-click to rename").double_clicked() {
                app.scout.renaming = Some((c.id, c.name.clone()));
            }
        }
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            if ui.small_button("×").on_hover_text("Delete this capture").clicked() {
                let _ = app.run("scout.delete", json!({"capture": c.id}));
            }
            if c.loop_range.is_some() {
                chip(ui, "loop", t.text_dim, t.border_light);
            }
            bands_chip(ui, c.bands_version);
        });
    });
    ui.label(RichText::new(facts(c)).font(regular(11.0)).color(t.text_dim));
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing = vec2(4.0, 4.0);
        for st in &c.strips {
            let r = chip(ui, &st.name, t.text, t.border_light).interact(Sense::click());
            r.on_hover_text("Right-click to save this strip as a sound profile or give it a target").context_menu(|ui| {
                target_menu(app, ui, c.id, st);
                ui.separator();
                for (label, pos) in [("Save POST as Profile", "post"), ("Save PRE as Profile", "base")] {
                    if ui.button(label).clicked() {
                        let p = json!({"from": "strip", "capture": c.id, "track": st.track.0, "position": pos});
                        if profiles::profile_cmd(app, "scout.profile_capture", p).is_ok() {
                            app.ui.status = format!("Saved {} as a profile", st.name);
                        }
                        ui.close();
                    }
                }
            });
        }
    });
    for st in c.strips.iter() {
        if let Some(tg) = &st.target {
            let what = if tg.kind == soundcraft_model::ProfileKind::Mix { "mastering target" } else { "target" };
            ui.label(RichText::new(format!("{} → {what} {}", st.name, tg.name)).font(regular(11.0)).color(t.accent));
        }
    }
    if let Some(o) = same_as {
        ui.label(RichText::new(format!("Same section as {}", o.name)).font(regular(11.0)).color(t.accent));
    }
    for st in c.strips.iter().filter(|s| s.note.is_some()) {
        ui.label(RichText::new(format!("{}: {}", st.name, st.note.as_deref().unwrap_or(""))).font(regular(10.5)).color(t.meter_yellow));
    }
    if c.played_truncated {
        ui.label(RichText::new("Many jumps: the played ranges are approximate").font(regular(10.5)).color(t.text_dim));
    }
}

/// The empty-conversation hint: the web build can only Copy; without a key, set one or Copy.
fn send_hint(app: &mut SoundApp) -> Option<&'static str> {
    if !ask::can_send(app) {
        return Some(if cfg!(target_arch = "wasm32") { ask::WEB_HINT } else { NO_KEY });
    }
    (!ask::has_key(app)).then_some(NO_KEY)
}

/// Claude's answer as text: a line that is all `**bold**` is a section heading; other `**`
/// markers are dropped (the answer is plain prose otherwise).
fn answer_text(ui: &mut Ui, t: &Tokens, text: &str) {
    let mut para = String::new();
    let flush = |ui: &mut Ui, para: &mut String| {
        if !para.is_empty() {
            ui.label(RichText::new(para.trim_end()).font(regular(12.5)).color(t.text));
            para.clear();
        }
    };
    for line in text.lines() {
        let l = line.trim();
        let heading = l.len() > 4 && l.starts_with("**") && l.ends_with("**") && !l[2..l.len() - 2].contains("**");
        if heading {
            flush(ui, &mut para);
            ui.add_space(2.0);
            ui.label(RichText::new(l.trim_matches('*').trim_end_matches(':')).font(bold(12.5)).color(t.text));
        } else if l.starts_with('#') {
            flush(ui, &mut para);
            ui.add_space(2.0);
            ui.label(RichText::new(l.trim_start_matches('#').trim()).font(bold(12.5)).color(t.text));
        } else {
            para.push_str(&line.replace("**", ""));
            para.push('\n');
        }
    }
    flush(ui, &mut para);
}

fn question_bubble(ui: &mut Ui, t: &Tokens, q: &str) {
    ui.with_layout(Layout::right_to_left(Align::Min), |ui| {
        egui::Frame::NONE.fill(t.accent_dark).corner_radius(CornerRadius::same(6)).inner_margin(Margin::symmetric(10, 6)).show(ui, |ui| {
            ui.set_max_width(ui.available_width() * 0.75);
            ui.label(RichText::new(q).font(regular(12.5)).color(Color32::WHITE));
        });
    });
    ui.add_space(6.0);
}

/// Whether the track named `name` has a Clean Q to draw guides on.
fn has_clean_q(s: &Session, name: &str) -> bool {
    s.tracks.iter().filter(|t| t.name == name).any(|t| t.mixer.inserts.iter().any(|i| i.plugin == "fs_cleanq"))
}

/// A small numbered swatch: an EQ guide's colour, or an outline for a module suggestion.
fn swatch(ui: &mut Ui, t: &Tokens, colour: Option<Color32>, n: Option<usize>) {
    let (r, _) = ui.allocate_exact_size(vec2(18.0, 15.0), Sense::hover());
    let p = ui.painter();
    match colour {
        Some(c) => {
            p.rect_filled(r, CornerRadius::same(3), c.gamma_multiply(0.25));
            p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, c), egui::StrokeKind::Inside);
        }
        None => {
            p.rect_stroke(r, CornerRadius::same(3), Stroke::new(1.0, t.border_light), egui::StrokeKind::Inside);
        }
    }
    if let Some(n) = n {
        p.text(r.center(), egui::Align2::CENTER_CENTER, (n + 1).to_string(), egui::FontId::proportional(10.0), t.text);
    }
}

/// An EQ guide's description: "2.5k–4k Hz, −4.0 to −2.0 dB, dynamic, Q ≈ 2.0 — harshness".
fn eq_text(lo: f32, hi: f32, g_lo: f32, g_hi: f32, dynamic: bool, q: Option<f32>, label: &str) -> String {
    let gain = if (g_hi - g_lo).abs() < 0.05 { format!("{g_lo:+.1} dB") } else { format!("{g_lo:+.1} to {g_hi:+.1} dB") };
    let mut what = format!("{}, {gain}", guides::hz_text(lo, hi));
    if dynamic {
        what.push_str(", dynamic");
    }
    if let Some(q) = q {
        what.push_str(&format!(", Q ≈ {q:.1}"));
    }
    if !label.is_empty() {
        what.push_str(&format!(" — {label}"));
    }
    what
}

/// The answer's suggestions as rows tagged "Track · Module"; the EQ ones numbered and coloured as
/// their guides are on the Clean Q graphs (one sequence per answer).
fn suggestion_rows(ui: &mut Ui, t: &Tokens, s: &Session, list: &[Guide]) {
    for (i, g) in list.iter().enumerate() {
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            match g {
                Guide::Eq { track, lo_hz, hi_hz, gain_lo_db, gain_hi_db, dynamic, q, label } => {
                    // One sequence per answer, as the graph's chips number them.
                    let n = guides::eq_number(list, i).unwrap_or(0);
                    swatch(ui, t, Some(guides::colour_for(n)), Some(n));
                    ui.label(RichText::new(format!("{track} · Clean Q")).font(bold(11.5)).color(t.text));
                    ui.label(RichText::new(eq_text(*lo_hz, *hi_hz, *gain_lo_db, *gain_hi_db, *dynamic, *q, label)).font(regular(11.5)).color(t.text));
                    if !has_clean_q(s, track) {
                        ui.label(
                            RichText::new(format!("{track} has no Clean Q; the guide stays listed here")).font(regular(10.5)).color(t.meter_yellow),
                        );
                    }
                }
                Guide::Module { track, module, param, current, range, reason } => {
                    swatch(ui, t, None, None);
                    ui.label(RichText::new(format!("{track} · {module}")).font(bold(11.5)).color(t.text));
                    let mut what = param.clone();
                    if !current.is_empty() {
                        what.push_str(&format!(" now {current}"));
                    }
                    if !range.is_empty() {
                        what.push_str(&format!(", try {range}"));
                    }
                    if !reason.is_empty() {
                        what.push_str(&format!(" — {reason}"));
                    }
                    ui.label(RichText::new(what).font(regular(11.5)).color(t.text));
                }
            }
        });
    }
}

fn answer_card(app: &mut SoundApp, ui: &mut Ui, turn: &ScoutTurn, shown: bool) {
    let t = Tokens::current();
    let session = app.engine.session_arc();
    egui::Frame::NONE.fill(t.panel_bg2).stroke(Stroke::new(1.0, t.border)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(10)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            if let Some(e) = &turn.error {
                ui.label(RichText::new(e).font(regular(12.5)).color(t.rec));
            }
            answer_text(ui, &t, guides::answer_body(&turn.answer));
            if !turn.guides.is_empty() {
                ui.add_space(6.0);
                ui.label(RichText::new("SUGGESTIONS").font(bold(10.5)).color(t.text_dim));
                suggestion_rows(ui, &t, &session, &turn.guides);
            }
            for n in &turn.notes {
                ui.label(RichText::new(n).font(regular(10.5)).color(t.text_dim));
            }
            ui.add_space(4.0);
            ui.horizontal(|ui| {
                if turn.guides.iter().any(|g| matches!(g, Guide::Eq { .. })) {
                    let label = if shown { "Hide guides" } else { "Show guides on Clean Q" };
                    if ui.button(label).clicked() {
                        let turn = if shown { serde_json::Value::Null } else { json!(turn.id) };
                        let _ = app.run("scout.guides", json!({"turn": turn}));
                    }
                }
                if ui.button("Copy prompt + package").on_hover_text("Copy this question and the package of its captures").clicked()
                    && let Err(e) = app.run("ui.scout.copy", json!({"captures": turn.captures, "question": turn.question}))
                {
                    app.ui.status = e.to_string();
                }
                if !turn.model.is_empty() {
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(RichText::new(&turn.model).font(regular(10.5)).color(t.text_dim));
                    });
                }
            });
        },
    );
    ui.add_space(10.0);
}

/// Scout ▸ Settings: the API key (masked; typed here, stored by the host, never in the session or
/// `UiState`) and the model.
fn settings(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let frame = egui::Frame::NONE
        .fill(t.panel_bg2)
        .stroke(Stroke::new(1.0, t.border_light))
        .corner_radius(CornerRadius::same(6))
        .inner_margin(Margin::same(10));
    frame.show(ui, |ui| {
        ui.set_width(ui.available_width());
        ui.label(RichText::new("SCOUT SETTINGS").font(bold(10.5)).color(t.text_dim));
        let status = ask::key_status(app);
        let line = match &status {
            Some(m) => format!("Anthropic API key: {m}"),
            None => "No Anthropic API key".to_string(),
        };
        ui.label(RichText::new(line).font(regular(12.0)).color(t.text));
        let can_store = app.services.set_api_key.is_some();
        ui.horizontal(|ui| {
            ui.add_enabled(can_store, egui::TextEdit::singleline(&mut app.scout.key_draft).password(true).desired_width(260.0).hint_text("sk-ant-…"));
            if ui.add_enabled(can_store && !app.scout.key_draft.trim().is_empty(), egui::Button::new("Save key")).clicked()
                && let Some(set) = app.services.set_api_key.clone()
            {
                let key = std::mem::take(&mut app.scout.key_draft);
                app.scout.message = set(Some(key)).err();
                app.scout.key_status = None;
            }
            if ui.add_enabled(can_store && status.is_some(), egui::Button::new("Remove")).clicked()
                && let Some(set) = app.services.set_api_key.clone()
            {
                app.scout.message = set(None).err();
                app.scout.key_status = None;
            }
        });
        if !can_store {
            ui.label(RichText::new(ask::WEB_HINT).font(regular(11.0)).color(t.text_dim));
        }
        ui.horizontal(|ui| {
            ui.label(RichText::new("Model").font(regular(12.0)).color(t.text));
            ui.add(egui::TextEdit::singleline(&mut app.ui.scout_model).desired_width(200.0).hint_text(ask::DEFAULT_MODEL));
            if ui.small_button("Done").clicked() {
                app.scout.settings_open = false;
            }
        });
    });
    ui.add_space(6.0);
}

/// The question out with Claude: the answer so far, a spinner, or the error it ended with.
fn asking_card(app: &SoundApp, ui: &mut Ui, t: &Tokens) {
    let Some(a) = &app.scout.asking else { return };
    question_bubble(ui, t, &a.question);
    let stroke = if a.error.is_some() { t.rec } else { t.border };
    egui::Frame::NONE.fill(t.panel_bg2).stroke(Stroke::new(1.0, stroke)).corner_radius(CornerRadius::same(6)).inner_margin(Margin::same(10)).show(
        ui,
        |ui| {
            ui.set_width(ui.available_width());
            let body = guides::answer_body(&a.text);
            if !body.is_empty() {
                answer_text(ui, t, body);
            }
            match &a.error {
                Some(e) => {
                    ui.label(RichText::new(e).font(regular(12.5)).color(t.rec));
                }
                None => {
                    ui.horizontal(|ui| {
                        ui.spinner();
                        ui.label(RichText::new(format!("{} is answering…", a.model)).font(regular(11.0)).color(t.text_dim));
                    });
                }
            }
        },
    );
}

fn conversation(app: &mut SoundApp, ui: &mut Ui) {
    let t = Tokens::current();
    let session = app.engine.session_arc();
    if app.scout.settings_open {
        settings(app, ui);
    }
    let input_h = 64.0;
    let list_h = (ui.available_height() - input_h).max(80.0);
    let hint = send_hint(app);
    egui::ScrollArea::vertical()
        .id_salt("scout_turns")
        .max_height(list_h)
        .min_scrolled_height(list_h)
        .auto_shrink([false, false])
        .stick_to_bottom(true)
        .show(ui, |ui| {
            if session.scout.turns.is_empty() && app.scout.asking.is_none() {
                ui.add_space(40.0);
                ui.vertical_centered(|ui| {
                    ui.label(RichText::new("Ask about the ticked captures.").font(regular(13.0)).color(t.text));
                    if let Some(h) = hint {
                        ui.add_space(4.0);
                        ui.label(RichText::new(h).font(regular(12.0)).color(t.text_dim));
                    }
                });
                return;
            }
            for turn in &session.scout.turns {
                question_bubble(ui, &t, &turn.question);
                answer_card(app, ui, turn, session.scout.guides_shown == Some(turn.id));
            }
            asking_card(app, ui, &t);
        });
    if let Some(m) = &app.scout.message {
        ui.label(RichText::new(m).font(regular(11.5)).color(t.rec));
    }
    ui.add_space(4.0);
    let chosen = app.scout.chosen(&session);
    let qhint = match chosen.as_slice() {
        [] => "Tick a capture to ask about it…".to_string(),
        [one] => format!("Ask about {}…", session.scout.captures.iter().find(|c| c.id == *one).map_or("the capture", |c| c.name.as_str())),
        many => format!("Ask about {} captures…", many.len()),
    };
    let busy = app.scout.asking.as_ref().is_some_and(ask::Asking::busy);
    ui.horizontal(|ui| {
        let w = ui.available_width() - 204.0;
        let r =
            ui.add(egui::TextEdit::singleline(&mut app.scout.question).desired_width(w.max(80.0)).hint_text(qhint).margin(Margin::symmetric(8, 6)));
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let can = ask::can_send(app) && !busy && !chosen.is_empty() && !app.scout.question.trim().is_empty();
        if busy {
            if ui.add(egui::Button::new("Cancel").min_size(vec2(56.0, 28.0))).on_hover_text("Stop waiting for this answer").clicked() {
                ask::cancel(app);
            }
        } else {
            send_button(app, ui, can, enter);
        }
        if ui
            .add_enabled(!chosen.is_empty(), egui::Button::new("Copy").min_size(vec2(56.0, 28.0)))
            .on_hover_text("Copy the question and the Scout package, to paste into any chat")
            .clicked()
        {
            let _ = app.run("ui.scout.copy", json!({}));
        }
        if ui.add(egui::Button::new("⚙").min_size(vec2(28.0, 28.0))).on_hover_text("Scout settings: API key and model").clicked() {
            app.scout.settings_open = !app.scout.settings_open;
        }
        scout_menu(app, ui);
    });
}

fn send_button(app: &mut SoundApp, ui: &mut Ui, can: bool, enter: bool) {
    let send = ui.add_enabled(can, egui::Button::new("Send").min_size(vec2(56.0, 28.0)));
    let send = if ask::can_send(app) {
        send.on_hover_text("Ask Claude about the ticked captures")
    } else {
        send.on_disabled_hover_text(if cfg!(target_arch = "wasm32") { ask::WEB_HINT } else { NO_KEY })
    };
    if (send.clicked() || (enter && can))
        && let Err(e) = app.run("ui.scout.send", json!({}))
    {
        app.ui.status = e.to_string();
    }
}

/// The Scout menu (☰): the starting prompt, to copy or save as a Markdown file.
fn scout_menu(app: &mut SoundApp, ui: &mut Ui) {
    let r = ui.menu_button(RichText::new("☰").font(regular(14.0)), |ui| {
        if ui.button("Copy Starting Prompt").on_hover_text(STARTING_PROMPT_HINT).clicked() {
            let _ = app.run("ui.scout.starting_prompt_copy", json!({}));
            ui.close();
        }
        if ui.add_enabled(app.services.pick_save.is_some(), egui::Button::new("Save Starting Prompt…")).clicked() {
            if let Err(e) = app.run("ui.scout.starting_prompt_save", json!({})) {
                app.ui.status = e;
            }
            ui.close();
        }
    });
    r.response.on_hover_text("Scout menu: the starting prompt");
}

/// What the starting prompt is.
pub const STARTING_PROMPT_HINT: &str =
    "Copy a prompt that tells an AI what FreeDAWm and Scout are, how to reply with guides, and every FireStation module with all its parameters";

/// The strip menu's Target submenu: the library's profiles (mix profiles for the master, filtered
/// by the strip's track kind otherwise), and Clear Target.
fn target_menu(app: &mut SoundApp, ui: &mut Ui, capture: u32, st: &soundcraft_model::ScoutStrip) {
    let master = st.channel_type == "master";
    ui.menu_button(if master { "Set Mastering Target" } else { "Set Target" }, |ui| {
        let list = profiles::entries(app);
        let wanted = if master { soundcraft_model::ProfileKind::Mix } else { soundcraft_model::ProfileKind::Track };
        let (fit, other): (Vec<_>, Vec<_>) = list.iter().partition(|e| e.profile.kind == wanted);
        if list.is_empty() {
            ui.label(RichText::new("No profiles yet (Scout ▸ Profiles)").font(regular(11.0)));
        }
        for e in fit.iter().chain(other.iter()) {
            let current = st.target.as_ref().is_some_and(|t| t.name == e.profile.name);
            let label = if e.profile.role.is_empty() { e.profile.name.clone() } else { format!("{} ({})", e.profile.name, e.profile.role) };
            if ui.selectable_label(current, label).clicked() {
                let p = profiles::with_dir(app, json!({"capture": capture, "track": st.track.0, "profile": e.profile.name}));
                if let Err(err) = app.run("scout.target_set", p) {
                    app.ui.status = err;
                }
                ui.close();
            }
        }
    });
    if st.target.is_some() && ui.button("Clear Target").clicked() {
        let _ = app.run("scout.target_set", json!({"capture": capture, "track": st.track.0, "profile": null}));
        ui.close();
    }
}

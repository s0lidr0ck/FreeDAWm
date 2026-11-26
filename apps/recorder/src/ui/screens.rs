//! The four screens, drawn to the approved design (480×640 points; a wider window keeps the
//! column centred, a shorter one scrolls), and the settings.

use super::look::{self, CARD, DIM, TEXT};
use super::paint::{self, Style, Weight, button, button_fit, button_in, button_named, caps, focus_ring, hit, label, level_bar, paragraph, text_at};
use super::{RecorderApp, Screen, daw_args};
use crate::rig::{Cause, InputChoice, Rig};
use crate::words;
use egui::{Align2, CornerRadius, Margin, Rect, Sense, Stroke, StrokeKind, Ui, UiBuilder, pos2, vec2};
use soundcraft_net::glue::FollowBadge;

/// A section's gaps (the design's 20 points, a little less where the main screen needs the room
/// for two ideas).
const GAP: f32 = 20.0;

impl RecorderApp {
    /// Paints the window: the background (red-tinted while recording), then the screen showing
    /// in a centred column with the design's padding.
    pub(super) fn draw(&mut self, ui: &mut Ui) {
        let area = ui.ctx().content_rect();
        let recording = self.recording();
        let bg = if recording { look::BG_REC } else { look::BG };
        ui.painter().with_clip_rect(area).rect_filled(area, CornerRadius::ZERO, bg);
        let width = area.width().min(look::WIDTH);
        let col = Rect::from_min_max(pos2(area.center().x - width / 2.0, area.top()), pos2(area.center().x + width / 2.0, area.bottom()));
        let inner_h = (area.height() - 2.0 * look::PAD_Y).max(0.0);
        let screen = if recording { None } else { Some(self.screen) };
        ui.scope_builder(UiBuilder::new().max_rect(col), |ui| {
            egui::ScrollArea::vertical().id_salt(("screen", screen)).auto_shrink([false, false]).show(ui, |ui| {
                egui::Frame::NONE.inner_margin(Margin::symmetric(look::PAD_X as i8, look::PAD_Y as i8)).show(ui, |ui| {
                    ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                    ui.set_width(width - 2.0 * look::PAD_X);
                    match screen {
                        None => self.recording_screen(ui, inner_h),
                        Some(Screen::Main) => self.main_screen(ui, inner_h),
                        Some(Screen::Idea) => self.idea_screen(ui, inner_h),
                        Some(Screen::Settings) => self.settings_screen(ui),
                        Some(Screen::Studio) => self.studio_screen(ui, inner_h),
                    }
                });
            });
        });
    }

    /// What it records from, in plain words (`None` while the devices open).
    fn source_words(&self) -> Option<String> {
        let rig = self.rig.as_ref()?;
        let input = (self.prefs.audio_on && rig.input_channels() > 0).then(|| words::describe_input(&rig.input_name().unwrap_or_default()));
        let keys: Vec<String> = rig.midi_ports().into_iter().filter(|(_, on)| *on).map(|(n, _)| n).collect();
        Some(words::recording_from(input, words::describe_keys(&keys)).unwrap_or_default())
    }

    /// Screen 1: ready to record.
    fn main_screen(&mut self, ui: &mut Ui, h: f32) {
        let top = ui.cursor().top();
        let w = ui.available_width();
        // The header: the name, the settings.
        let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        let tr = text_at(ui, row.left_center(), Align2::LEFT_CENTER, "FreeDAWm Recorder", 14.0, Weight::Semi, DIM);
        paint::read_as(ui, tr, "FreeDAWm Recorder");
        let gear = Rect::from_min_size(pos2(row.right() - look::TOUCH, row.top()), vec2(look::TOUCH, look::TOUCH));
        let r = hit(ui, gear, "Settings", true);
        let fill = if r.hovered() { look::ROUND } else { CARD };
        ui.painter().rect_filled(gear, CornerRadius::same(10), fill);
        ui.painter().rect_stroke(gear, CornerRadius::same(10), Stroke::new(1.0, look::LINE), StrokeKind::Inside);
        paint::sliders_icon(ui.painter(), gear.center(), look::ICON, fill);
        focus_ring(ui, &r, 10.0);
        if r.clicked() {
            self.go(Screen::Settings);
        }
        ui.add_space(16.0);

        // Record.
        let can = self.rig.as_ref().is_none_or(Rig::can_record);
        let (area, _) = ui.allocate_exact_size(vec2(w, 184.0), Sense::hover());
        let c = area.center();
        let disc = Rect::from_center_size(c, vec2(184.0, 184.0));
        let r = hit(ui, disc, "Record", can);
        let p = ui.painter();
        if can {
            // The glow under it (the design's shadow: 0 10px 30px, a quarter of the red).
            for i in 0..10 {
                let k = i as f32 / 10.0;
                p.circle_filled(c + vec2(0.0, 10.0), 92.0 + 18.0 * k, look::RED.gamma_multiply(0.05 * (1.0 - k)));
            }
        }
        p.circle_filled(c, 92.0, if can { look::RED_RING } else { look::LINE });
        let fill = if !can {
            look::OFF
        } else if r.is_pointer_button_down_on() {
            paint::shade(look::RED, -16)
        } else if r.hovered() {
            paint::shade(look::RED, 14)
        } else {
            look::RED
        };
        p.circle_filled(c, 86.0, fill);
        text_at(ui, c, Align2::CENTER_CENTER, "Record", 30.0, Weight::Bold, if can { egui::Color32::WHITE } else { DIM });
        if r.has_focus() {
            ui.painter().circle_stroke(c, 96.0, Stroke::new(2.0, look::LINK));
        }
        if r.clicked() {
            self.toggle_record();
        }
        ui.add_space(10.0);
        let caption = if self.record_when_ready {
            "Starting as soon as your microphone is ready…"
        } else if !can {
            "Nothing to record from yet"
        } else {
            "Click, or press Space"
        };
        centred(ui, caption, 14.0, Weight::Regular, DIM);
        if let Some((m, bad)) = self.message.clone() {
            ui.add_space(6.0);
            centred_wrap(ui, &m, 13.0, if bad { look::AMBER } else { DIM });
        }
        ui.add_space(16.0);

        // Recording from.
        self.device_card(ui);
        ui.add_space(16.0);

        // The ideas, then the studio at the bottom.
        let bottom = 16.0 + 48.0;
        caps(ui, "YOUR IDEAS");
        ui.add_space(4.0);
        let room = top + h - ui.cursor().top() - bottom;
        let ideas: Vec<crate::files::TakeFiles> = self.ideas().into_iter().cloned().collect();
        if ideas.is_empty() {
            ui.add_space(6.0);
            label(ui, "No ideas yet. Press Record and play something.", 14.0, Weight::Regular, DIM);
        } else if room >= 104.0 {
            egui::ScrollArea::vertical().id_salt("ideas").max_height(room).auto_shrink([false, true]).show(ui, |ui| {
                ui.spacing_mut().item_spacing = vec2(0.0, 0.0);
                for t in &ideas {
                    ui.push_id(&t.stem, |ui| self.idea_row(ui, t));
                }
            });
        } else {
            for t in &ideas {
                ui.push_id(&t.stem, |ui| self.idea_row(ui, t));
            }
        }
        let left = top + h - ui.cursor().top() - 48.0;
        ui.add_space(left.max(16.0));
        let joined = self.link.paired();
        let text = if joined { format!("Studio: {}", self.link.booth_name()) } else { "Record with a studio".to_string() };
        let r = button(ui, vec2(w, 48.0), &text, Style::outlined(15.0, Weight::Medium, 12.0), true);
        if joined {
            let away = self.studio_view().is_none_or(|v| !v.connected);
            let tw = paint::text_width(ui, &text, 15.0, Weight::Medium);
            ui.painter().circle_filled(
                pos2(r.rect.center().x - tw / 2.0 - 14.0, r.rect.center().y),
                5.0,
                if away { look::AMBER } else { look::GREEN },
            );
        }
        if r.clicked() {
            self.go(Screen::Studio);
        }
    }

    /// What the studio followed is doing (`None`: not following, or the devices not open yet).
    fn studio_view(&self) -> Option<crate::booth::BoothView> {
        if let Some(d) = &self.link.demo {
            return Some(d.clone());
        }
        self.link.view(self.rig.as_ref()?)
    }

    /// The card that says what it records from, its level, and Change.
    fn device_card(&mut self, ui: &mut Ui) {
        let w = ui.available_width();
        let from = self.source_words();
        let no_input = from.as_deref() == Some("");
        let mic_missing = self.rig.as_ref().is_some_and(|r| self.prefs.audio_on && r.input_channels() == 0) && !no_input;
        let mut change = false;
        let mut retry = false;
        egui::Frame::NONE.fill(CARD).corner_radius(12).inner_margin(Margin::symmetric(16, 12)).show(ui, |ui| {
            ui.set_width(w - 32.0);
            let iw = ui.available_width();
            let (row, _) = ui.allocate_exact_size(vec2(iw, look::TOUCH), Sense::hover());
            let button_text = if no_input { "Try again" } else { "Change" };
            let bw = paint::text_width(ui, button_text, 14.0, Weight::Medium) + 32.0;
            let b = Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, look::TOUCH));
            let text_w = iw - bw - 12.0;
            text_at(ui, pos2(row.left(), row.center().y - 1.0), Align2::LEFT_BOTTOM, "Recording from", 12.0, Weight::Regular, DIM);
            let (what, colour) = match &from {
                None => ("Looking for your microphone…".to_string(), DIM),
                Some(s) if s.is_empty() => ("No microphone found".to_string(), look::AMBER),
                Some(s) => (s.clone(), TEXT),
            };
            let shown = paint::fit(ui, &what, 15.0, Weight::Semi, text_w);
            text_at(ui, pos2(row.left(), row.center().y + 1.0), Align2::LEFT_TOP, &shown, 15.0, Weight::Semi, colour);
            paint::read_as(ui, Rect::from_min_size(row.min, vec2(text_w, row.height())), &format!("Recording from: {what}"));
            let r = button_named(ui, b, button_text, button_text, Style::outlined(14.0, Weight::Medium, 10.0), true);
            if r.clicked() {
                if no_input {
                    retry = true;
                } else {
                    change = true;
                }
            }
            ui.add_space(10.0);
            let level = match &self.rig {
                Some(r) if self.prefs.audio_on && r.input_channels() > 0 => r.level(),
                _ => 0.0,
            };
            let (bar, _) = ui.allocate_exact_size(vec2(iw, 10.0), Sense::hover());
            level_bar(ui, bar, level);
            if no_input {
                ui.add_space(10.0);
                paragraph(ui, "Plug in a microphone, a headset or a keyboard, then press Try again.", 13.0, Weight::Regular, DIM, 18.0);
            } else if mic_missing {
                ui.add_space(10.0);
                paragraph(ui, "No microphone found: only the piano notes will be recorded.", 13.0, Weight::Regular, look::AMBER, 18.0);
            }
        });
        if change {
            self.go(Screen::Settings);
        }
        if retry {
            self.reopen();
        }
    }

    /// One idea in the list: Play, its name and when; the row opens it.
    fn idea_row(&mut self, ui: &mut Ui, t: &crate::files::TakeFiles) {
        let w = ui.available_width();
        let (row, _) = ui.allocate_exact_size(vec2(w, 52.0), Sense::hover());
        let name = words::display_name(&t.stem);
        let playing = self.playing().is_some_and(|(s, _)| s == t.stem);
        let play = Rect::from_center_size(pos2(row.left() + 22.0, row.center().y), vec2(look::TOUCH, look::TOUCH));
        let open = Rect::from_min_max(pos2(play.right() + 6.0, row.top() + 4.0), pos2(row.right(), row.bottom() - 4.0));
        let ro = hit(ui, open, &format!("Open {name}"), true);
        if ro.hovered() {
            ui.painter().rect_filled(open, CornerRadius::same(8), egui::Color32::from_white_alpha(6));
        }
        focus_ring(ui, &ro, 8.0);
        let label_play = if playing { format!("Stop {name}") } else { format!("Play {name}") };
        let rp = hit(ui, play, &label_play, t.wav.is_some());
        let fill = if rp.hovered() { paint::shade(look::ROUND, 16) } else { look::ROUND };
        ui.painter().circle_filled(play.center(), 22.0, fill);
        let ink = if t.wav.is_some() { TEXT } else { DIM };
        if playing {
            paint::stop_icon(ui.painter(), play.center(), 13.0, 2.0, ink);
        } else {
            paint::play_icon(ui.painter(), play.center(), 16.0, ink);
        }
        if rp.has_focus() {
            ui.painter().circle_stroke(play.center(), 25.0, Stroke::new(2.0, look::LINK));
        }
        let x = play.right() + 12.0;
        let tw = row.right() - x;
        let now = crate::settings::now_stamp();
        let title = paint::fit(ui, &name, 15.0, Weight::Semi, tw);
        text_at(ui, pos2(x, row.center().y - 1.0), Align2::LEFT_BOTTOM, &title, 15.0, Weight::Semi, TEXT);
        let when = paint::fit(ui, &words::when_line(t, now), 13.0, Weight::Regular, tw);
        let tr = text_at(ui, pos2(x, row.center().y + 2.0), Align2::LEFT_TOP, &when, 13.0, Weight::Regular, DIM);
        paint::read_as(ui, tr, &when);
        if rp.clicked() {
            self.play(t);
        } else if ro.clicked() {
            self.open_idea(&t.stem);
        }
    }

    /// Screen 2: recording.
    fn recording_screen(&mut self, ui: &mut Ui, h: f32) {
        let top = ui.cursor().top();
        let w = ui.available_width();
        let for_studio = self.rig.as_ref().is_some_and(Rig::recording_for_booth);
        let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        ui.painter().circle_filled(pos2(row.left() + 5.0, row.center().y), 5.0, look::RED_LIVE);
        let heading = if for_studio { format!("Recording with {}", self.link.booth_name()) } else { "Recording".to_string() };
        let r = text_at(ui, pos2(row.left() + 18.0, row.center().y), Align2::LEFT_CENTER, &heading, 14.0, Weight::Semi, look::RED_SOFT);
        paint::read_as(ui, r, &heading);
        ui.add_space(22.0);

        // The clock: each digit in a cell of its own, so it does not jiggle.
        let secs = self.rig.as_ref().map_or(0.0, Rig::elapsed);
        let t = words::clock(secs);
        let (area, _) = ui.allocate_exact_size(vec2(w, 88.0), Sense::hover());
        let digit = paint::text_width(ui, "0", 88.0, Weight::Bold) - 1.5;
        let colon = paint::text_width(ui, ":", 88.0, Weight::Bold) - 1.5;
        let total: f32 = t.chars().map(|c| if c == ':' { colon } else { digit }).sum();
        let mut x = area.center().x - total / 2.0;
        for ch in t.chars() {
            let cw = if ch == ':' { colon } else { digit };
            text_at(ui, pos2(x + cw / 2.0, area.center().y), Align2::CENTER_CENTER, &ch.to_string(), 88.0, Weight::Bold, TEXT);
            x += cw;
        }
        paint::read_as(ui, area, &format!("Recorded {t}"));
        ui.add_space(22.0);

        // The moving bars, from the input.
        let (area, _) = ui.allocate_exact_size(vec2(w, 96.0), Sense::hover());
        let peaks = self.rig.as_ref().map(|r| r.recent_peaks(20, 18)).unwrap_or_default();
        let x0 = area.center().x - (20.0 * 6.0 + 19.0 * 5.0) / 2.0;
        for i in 0..20 {
            let peak = peaks.len().checked_sub(20 - i).and_then(|k| peaks.get(k)).copied().unwrap_or(0.0);
            let bh = 8.0 + 88.0 * paint::level_pos(peak);
            let b = Rect::from_center_size(pos2(x0 + 3.0 + i as f32 * 11.0, area.center().y), vec2(6.0, bh));
            ui.painter().rect_filled(b, CornerRadius::same(3), look::RED_LIVE);
        }
        paint::read_as(ui, area, "Sound coming in");
        ui.add_space(22.0);

        // Stop.
        let (area, _) = ui.allocate_exact_size(vec2(w, 168.0), Sense::hover());
        let c = area.center();
        let r = hit(ui, Rect::from_center_size(c, vec2(168.0, 168.0)), "Stop", true);
        let p = ui.painter();
        p.circle_filled(c, 84.0, look::STOP_RING);
        let fill = if r.is_pointer_button_down_on() {
            paint::shade(TEXT, -24)
        } else if r.hovered() {
            egui::Color32::WHITE
        } else {
            TEXT
        };
        p.circle_filled(c, 78.0, fill);
        paint::stop_icon(p, c - vec2(0.0, 18.0), 44.0, 8.0, look::RED_STOP);
        text_at(ui, c + vec2(0.0, 28.0), Align2::CENTER_CENTER, "Stop", 20.0, Weight::Bold, look::BG_REC);
        if r.has_focus() {
            ui.painter().circle_stroke(c, 88.0, Stroke::new(2.0, look::LINK));
        }
        if r.clicked() {
            self.toggle_record();
        }

        // At the bottom: what it records from, the notes so far.
        let bottom = 18.0 + 6.0 + 16.0;
        let left = top + h - ui.cursor().top() - bottom;
        ui.add_space(left.max(22.0));
        centred(ui, &self.source_words().unwrap_or_default(), 15.0, Weight::Semi, TEXT);
        ui.add_space(6.0);
        let keys = self.rig.as_ref().is_some_and(|r| r.midi_ports().iter().any(|(_, on)| *on));
        let line = match (keys, self.notes.0) {
            (false, _) => "Saved as you go".to_string(),
            (true, 1) => "1 piano note so far · saved as you go".to_string(),
            (true, n) => format!("{n} piano notes so far · saved as you go"),
        };
        centred(ui, &line, 13.0, Weight::Regular, look::DIM_REC);
    }

    /// Screen 3: the idea just saved (or opened from the list).
    fn idea_screen(&mut self, ui: &mut Ui, h: f32) {
        let Some(idea) = self.idea.clone() else {
            self.screen = Screen::Main;
            return;
        };
        let top = ui.cursor().top();
        let w = ui.available_width();
        let length = idea.take.seconds.map(words::clock);
        // The header.
        let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        let heading = if idea.just_saved {
            match &length {
                Some(l) => format!("Saved · {l}"),
                None => "Saved".to_string(),
            }
        } else {
            words::when_line(&idea.take, crate::settings::now_stamp())
        };
        let x = if idea.just_saved {
            let c = pos2(row.left() + 11.0, row.center().y);
            if idea.problem.is_some() {
                ui.painter().circle_filled(c, 10.0, look::AMBER);
                text_at(ui, c, Align2::CENTER_CENTER, "!", 15.0, Weight::Bold, look::BG);
            } else {
                paint::check_icon(ui.painter(), c, 22.0, look::GREEN, 2.5);
            }
            row.left() + 32.0
        } else {
            row.left()
        };
        let r = text_at(ui, pos2(x, row.center().y), Align2::LEFT_CENTER, &heading, 16.0, Weight::Semi, TEXT);
        paint::read_as(ui, r, &heading);
        if let Some((cause, raw)) = &idea.problem {
            let text = match (cause, raw.as_str()) {
                (Cause::InputGone, _) => "Your microphone stopped (unplugged?). Everything up to then is saved.",
                (Cause::Disk, "full") => "Your disk is full, so recording stopped. Everything up to then is saved.",
                (Cause::Disk, _) => "Your disk couldn't keep up, so recording stopped. Everything up to then is saved.",
                (Cause::OutputGone, _) => "Your speakers or headphones stopped, so recording stopped. Everything up to then is saved.",
                (Cause::Other, _) => raw.as_str(),
            };
            ui.add_space(4.0);
            paragraph(ui, text, 13.0, Weight::Regular, look::AMBER, 18.0);
        }
        ui.add_space(GAP);

        // The card: name, play, shape.
        let mut typed = false;
        let mut enter = false;
        let mut lost = false;
        let mut play = false;
        egui::Frame::NONE.fill(CARD).corner_radius(14).inner_margin(Margin::same(18)).show(ui, |ui| {
            ui.set_width(w - 36.0);
            let iw = ui.available_width();
            label(ui, "Name it (optional)", 12.0, Weight::Regular, DIM);
            ui.add_space(6.0);
            let (rect, _) = ui.allocate_exact_size(vec2(iw, 48.0), Sense::hover());
            let mut name = idea.name.clone();
            let r = paint::text_box(ui, rect, &mut name, "Type a name", "idea-name", 16.0, 10.0);
            if r.changed() {
                typed = true;
                if let Some(i) = &mut self.idea {
                    i.name = name;
                }
            }
            if r.lost_focus() {
                lost = true;
                enter = ui.input(|i| i.key_pressed(egui::Key::Enter));
            }
            if let Some(e) = &idea.name_error {
                ui.add_space(6.0);
                paragraph(ui, e, 13.0, Weight::Regular, look::AMBER, 18.0);
            }
            ui.add_space(16.0);
            let (row, _) = ui.allocate_exact_size(vec2(iw, 64.0), Sense::hover());
            let pb = Rect::from_min_size(row.min, vec2(64.0, 64.0));
            let progress = self.playing().filter(|(s, _)| *s == idea.take.stem).map(|(_, f)| f);
            let has_wav = idea.take.wav.is_some();
            let rp = hit(ui, pb, if progress.is_some() { "Stop playing" } else { "Play this idea" }, has_wav);
            let fill = if !has_wav {
                look::LINE
            } else if rp.hovered() {
                egui::Color32::WHITE
            } else {
                TEXT
            };
            ui.painter().circle_filled(pb.center(), 32.0, fill);
            if progress.is_some() {
                paint::stop_icon(ui.painter(), pb.center(), 18.0, 3.0, look::BG);
            } else {
                paint::play_icon(ui.painter(), pb.center() + vec2(1.0, 0.0), 24.0, look::BG);
            }
            if rp.has_focus() {
                ui.painter().circle_stroke(pb.center(), 35.0, Stroke::new(2.0, look::LINK));
            }
            play = rp.clicked();
            // The take's shape (its real peaks), the part played lit.
            let x0 = pb.right() + 14.0;
            let lit = progress.unwrap_or(0.0);
            for (i, peak) in idea.shape.iter().enumerate() {
                let bh = 8.0 + 48.0 * (paint::level_pos(*peak) * 1.25 - 0.25).clamp(0.0, 1.0);
                let b = Rect::from_center_size(pos2(x0 + 2.0 + i as f32 * 7.0, row.center().y), vec2(4.0, bh));
                let colour = if (i as f32 + 0.5) / idea.shape.len().max(1) as f32 <= lit { TEXT } else { look::SHAPE };
                ui.painter().rect_filled(b, CornerRadius::same(2), colour);
            }
            if idea.shape.is_empty() {
                for i in 0..super::SHAPE_BARS {
                    let b = Rect::from_center_size(pos2(x0 + 2.0 + i as f32 * 7.0, row.center().y), vec2(4.0, 4.0));
                    ui.painter().rect_filled(b, CornerRadius::same(2), look::LINE);
                }
            }
            paint::read_as(ui, Rect::from_min_max(pos2(x0, row.top()), row.max), "The recording's shape");
            ui.add_space(16.0);
            let caption = words::saved_caption(idea.take.wav.is_some(), idea.take.mid.is_some(), idea.notes);
            label(ui, &caption, 13.0, Weight::Regular, DIM).on_hover_text(super::MIDI_TIP);
        });
        if typed {
            self.name_typed();
        }
        if enter || lost {
            self.apply_name();
        }
        if play {
            let take = self.idea.as_ref().map(|i| i.take.clone());
            if let Some(t) = take {
                self.play(&t);
            }
        }
        ui.add_space(GAP);

        // Record another, then FreeDAWm and the file.
        let r = button(
            ui,
            vec2(w, 64.0),
            "Record another",
            Style::filled(look::RED, egui::Color32::WHITE, 20.0, Weight::Bold, 14.0),
            self.rig.as_ref().is_none_or(Rig::can_record),
        );
        if r.clicked() {
            self.toggle_record();
        }
        ui.add_space(GAP);
        let (row, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        let half = (w - 12.0) / 2.0;
        let left = Rect::from_min_size(row.min, vec2(half, 48.0));
        let right = Rect::from_min_size(pos2(row.right() - half, row.top()), vec2(half, 48.0));
        let daw_text = if self.daw.is_some() { "Open in FreeDAWm" } else { "FreeDAWm not found" };
        if button_in(ui, left, daw_text, Style::outlined(15.0, Weight::Medium, 12.0), self.daw.is_some() && !daw_args(&idea.take).is_empty())
            .clicked()
        {
            self.open_in_daw();
        }
        if button_in(ui, right, "Show the file", Style::outlined(15.0, Weight::Medium, 12.0), true).clicked() {
            self.show_file();
        }
        let note = self.idea.as_ref().and_then(|i| i.note.clone());
        if let Some((n, bad)) = note {
            ui.add_space(8.0);
            paragraph(ui, &n, 13.0, Weight::Regular, if bad { look::AMBER } else { DIM }, 18.0);
        }

        // At the bottom: where it is kept, back to the list, throw away.
        let confirm = idea.confirm_throw;
        let bottom = 16.0 + 12.0 + look::TOUCH + if confirm { 26.0 } else { 0.0 };
        let left_room = top + h - ui.cursor().top() - bottom;
        ui.add_space(left_room.max(GAP));
        label(ui, &format!("Kept in {}", words::folder_words(&self.prefs.ideas())), 13.0, Weight::Regular, DIM);
        ui.add_space(12.0);
        if confirm {
            label(ui, "Throw this idea away? It can't be undone.", 13.0, Weight::Regular, look::AMBER);
            ui.add_space(8.0);
        }
        let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        if confirm {
            let keep_w = paint::text_width(ui, "Keep it", 15.0, Weight::Medium) + 32.0;
            let keep = Rect::from_min_size(row.min, vec2(keep_w, look::TOUCH));
            if button_in(ui, keep, "Keep it", Style::outlined(15.0, Weight::Medium, 10.0), true).clicked()
                && let Some(i) = &mut self.idea
            {
                i.confirm_throw = false;
            }
            let tw = paint::text_width(ui, "Throw away", 15.0, Weight::Semi) + 32.0;
            let throw = Rect::from_min_size(pos2(row.right() - tw, row.top()), vec2(tw, look::TOUCH));
            if button_named(
                ui,
                throw,
                "Throw away",
                "Yes, throw it away",
                Style::filled(look::RED, egui::Color32::WHITE, 15.0, Weight::Semi, 10.0),
                true,
            )
            .clicked()
            {
                self.throw_away();
            }
        } else {
            let aw = paint::text_width(ui, "All my ideas", 15.0, Weight::Medium) + 4.0;
            let all = Rect::from_min_size(row.min, vec2(aw, look::TOUCH));
            let r = hit(ui, all, "All my ideas", true);
            let ink = if r.hovered() { paint::shade(look::LINK, 40) } else { look::LINK };
            let tr = text_at(ui, all.left_center(), Align2::LEFT_CENTER, "All my ideas", 15.0, Weight::Medium, ink);
            if r.hovered() {
                ui.painter().hline(tr.x_range(), tr.bottom(), Stroke::new(1.0, ink));
            }
            focus_ring(ui, &r, 6.0);
            if r.clicked() {
                self.go(Screen::Main);
            }
            let tw = paint::text_width(ui, "Throw away", 15.0, Weight::Medium) + 28.0;
            let throw = Rect::from_min_size(pos2(row.right() - tw, row.top()), vec2(tw, look::TOUCH));
            if button_in(ui, throw, "Throw away", Style::plain(look::RED_TEXT, 15.0, Weight::Medium), true).clicked()
                && let Some(i) = &mut self.idea
            {
                i.confirm_throw = true;
            }
        }
    }

    /// A back link at the top of a screen.
    fn back(&mut self, ui: &mut Ui, text: &str) {
        let w = paint::text_width(ui, text, 15.0, Weight::Medium) + 26.0;
        let (rect, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        let r = hit(ui, rect, text, true);
        let ink = if r.hovered() { paint::shade(look::LINK, 40) } else { look::LINK };
        paint::back_icon(ui.painter(), pos2(rect.left() + 9.0, rect.center().y), 18.0, ink);
        text_at(ui, pos2(rect.left() + 26.0, rect.center().y), Align2::LEFT_CENTER, text, 15.0, Weight::Medium, ink);
        focus_ring(ui, &r, 6.0);
        if r.clicked() {
            self.go(Screen::Main);
        }
    }

    /// The settings: the input and its channel, the keyboards, the folder, the output, start-up,
    /// the studio.
    fn settings_screen(&mut self, ui: &mut Ui) {
        let w = ui.available_width();
        self.back(ui, "Back");
        ui.add_space(14.0);
        label(ui, "Settings", 24.0, Weight::Bold, TEXT);
        ui.add_space(GAP);
        let recording = self.recording();
        let lists = self.devices.clone();

        caps(ui, "MICROPHONE OR INPUT");
        ui.add_space(8.0);
        let current =
            self.rig.as_ref().and_then(Rig::input_name).or_else(|| self.prefs.audio.input.clone()).unwrap_or_else(|| "The computer's own".into());
        let mut options: Vec<(String, Option<String>)> = vec![("The computer's own (default)".into(), None)];
        match &lists {
            Some(l) => options.extend(l.inputs.iter().map(|d| (d.clone(), Some(d.clone())))),
            None => options.push(("Looking for others…".into(), self.prefs.audio.input.clone())),
        }
        if let Some(p) = combo(ui, "input", &current, w, &options, !recording) {
            self.prefs.audio.input = p;
            self.reopen();
        }
        ui.add_space(8.0);
        let n = self.rig.as_ref().map_or(1, Rig::input_channels).max(1);
        let choice = super::choice_of(&self.prefs);
        let mut chans: Vec<(String, InputChoice)> =
            (0..n).map(|c| InputChoice { channel: c, stereo: false, ..choice }).map(|c| (format!("Input {}", c.channel + 1), c)).collect();
        chans.extend(
            (0..n.saturating_sub(1))
                .step_by(2)
                .map(|c| InputChoice { channel: c, stereo: true, ..choice })
                .map(|c| (format!("Inputs {} and {} (stereo)", c.channel + 1, c.channel + 2), c)),
        );
        let shown = chans.iter().find(|(_, c)| *c == choice).map_or_else(|| choice.name(), |(t, _)| t.clone());
        if let Some(c) = combo(ui, "channel", &shown, w, &chans, !recording) {
            (self.prefs.channel, self.prefs.stereo) = (c.channel, c.stereo);
            if let Some(r) = &mut self.rig {
                r.set_choice(super::choice_of(&self.prefs));
            }
        }
        let mut on = self.prefs.audio_on;
        if paint::tick(ui, &mut on, "Record sound from it", 15.0, w).changed() && !recording {
            self.prefs.audio_on = on;
            if let Some(r) = &mut self.rig {
                r.set_choice(super::choice_of(&self.prefs));
            }
        }
        ui.add_space(GAP);

        caps(ui, "KEYBOARDS");
        ui.add_space(4.0);
        let ports = self.rig.as_ref().map(Rig::midi_ports).unwrap_or_default();
        if ports.is_empty() {
            ui.add_space(4.0);
            label(ui, "No keyboard plugged in. One plugged in later is recorded too.", 14.0, Weight::Regular, DIM);
        }
        for (name, on) in ports {
            let mut v = on;
            if paint::tick(ui, &mut v, &name, 15.0, w).changed()
                && let Some(r) = &mut self.rig
            {
                r.set_midi_port(&name, v);
            }
        }
        ui.add_space(GAP);

        caps(ui, "WHERE IDEAS ARE KEPT");
        ui.add_space(8.0);
        let folder = self.prefs.ideas().display().to_string();
        let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
        let bw = paint::text_width(ui, "Choose…", 14.0, Weight::Medium) + 32.0;
        let shown = paint::fit(ui, &folder, 14.0, Weight::Regular, w - bw - 12.0);
        text_at(ui, row.left_center(), Align2::LEFT_CENTER, &shown, 14.0, Weight::Regular, TEXT);
        let b = Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, look::TOUCH));
        if button_named(ui, b, "Choose…", "Choose the ideas folder", Style::outlined(14.0, Weight::Medium, 10.0), self.live && !recording).clicked()
            && let Some(dir) = rfd::FileDialog::new().set_directory(self.prefs.ideas()).pick_folder()
        {
            self.prefs.ideas_folder = dir.to_string_lossy().into_owned();
            self.rescan();
        }
        ui.add_space(GAP);

        caps(ui, "PLAY BACK THROUGH");
        ui.add_space(8.0);
        let current = self.prefs.audio.output.clone().unwrap_or_else(|| "The computer's own".into());
        let mut options: Vec<(String, Option<String>)> = vec![("The computer's own (default)".into(), None)];
        match &lists {
            Some(l) => options.extend(l.outputs.iter().map(|d| (d.clone(), Some(d.clone())))),
            None => options.push(("Looking for others…".into(), self.prefs.audio.output.clone())),
        }
        if let Some(p) = combo(ui, "output", &current, w, &options, !recording) {
            self.prefs.audio.output = p;
            self.reopen();
        }
        if let Some(p) = self.rig.as_ref().and_then(|r| r.output_problem.clone()) {
            ui.add_space(6.0);
            paragraph(ui, &p, 13.0, Weight::Regular, look::AMBER, 18.0);
        }
        ui.add_space(GAP);

        caps(ui, "WHEN IT OPENS");
        let mut v = self.prefs.record_on_open;
        if paint::tick(ui, &mut v, "Start recording as soon as the Recorder opens", 15.0, w).changed() {
            self.prefs.record_on_open = v;
        }
        ui.add_space(GAP);

        caps(ui, "STUDIO");
        ui.add_space(8.0);
        if self.link.paired() {
            let (row, _) = ui.allocate_exact_size(vec2(w, look::TOUCH), Sense::hover());
            let text = format!("Joined {}", self.link.booth_name());
            let bw = paint::text_width(ui, "Leave this studio", 14.0, Weight::Medium) + 32.0;
            let shown = paint::fit(ui, &text, 15.0, Weight::Regular, w - bw - 12.0);
            text_at(ui, row.left_center(), Align2::LEFT_CENTER, &shown, 15.0, Weight::Regular, TEXT);
            let b = Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, look::TOUCH));
            let busy = self.rig.as_ref().is_some_and(Rig::recording_for_booth);
            if button_in(ui, b, "Leave this studio", Style::outlined(14.0, Weight::Medium, 10.0), !busy).clicked() {
                self.leave_studio();
            }
            ui.add_space(4.0);
            label(ui, &format!("The studio sees this computer as {}", self.link.my_name()), 13.0, Weight::Regular, DIM);
        } else {
            label(ui, "Not joined to a studio. Nothing is sent on the network until you open “Record with a studio”.", 13.0, Weight::Regular, DIM);
        }
        ui.add_space(GAP);
        if button(ui, vec2(w, 48.0), "Done", Style::filled(TEXT, look::BG, 16.0, Weight::Bold, 12.0), true).clicked() {
            self.go(Screen::Main);
        }
    }

    /// Screen 4: record with a studio.
    fn studio_screen(&mut self, ui: &mut Ui, h: f32) {
        let top = ui.cursor().top();
        let w = ui.available_width();
        self.back(ui, "Back to my ideas");
        ui.add_space(18.0);
        label(ui, "Record with a studio", 24.0, Weight::Bold, TEXT);
        ui.add_space(6.0);
        paragraph(ui, "When the studio presses Record, you record too. Your take is sent to them when you stop.", 14.0, Weight::Regular, DIM, 20.0);
        ui.add_space(18.0);

        if self.link.paired() {
            self.joined_card(ui, false);
            ui.add_space(12.0);
            let view = self.studio_view();
            if let Some(v) = &view {
                let waiting = match (v.progress, v.queued) {
                    (Some(_), _) | (None, 0) => None,
                    (None, 1) => Some("1 take is kept here until the studio has it.".to_string()),
                    (None, n) => Some(format!("{n} takes are kept here until the studio has them.")),
                };
                if let Some(t) = waiting {
                    paragraph(ui, &t, 13.0, Weight::Regular, DIM, 18.0);
                    ui.add_space(6.0);
                }
            }
            if let Some(e) = self.link.error.clone() {
                paragraph(ui, &words::studio_error(&e), 13.0, Weight::Regular, look::AMBER, 18.0);
                ui.add_space(6.0);
            }
            let busy = self.rig.as_ref().is_some_and(Rig::recording_for_booth);
            if button_fit(ui, look::TOUCH, 14.0, "Leave this studio", Style::plain(look::RED_TEXT, 15.0, Weight::Medium), !busy).clicked() {
                self.leave_studio();
            }
            return;
        }

        // Studios found on this network, each with Join.
        let found = self.link.found();
        let asking = self.link.busy() || self.link.wants_code();
        for f in &found {
            let mut join = false;
            egui::Frame::NONE.fill(CARD).corner_radius(14).inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(w - 32.0);
                let iw = ui.available_width();
                let (row, _) = ui.allocate_exact_size(vec2(iw, 48.0), Sense::hover());
                ui.painter().circle_filled(pos2(row.left() + 5.0, row.center().y), 5.0, look::GREEN);
                let bw = paint::text_width(ui, "Join", 16.0, Weight::Bold) + 44.0;
                let tw = iw - bw - 34.0;
                let title = paint::fit(ui, &f.name, 16.0, Weight::Semi, tw);
                text_at(ui, pos2(row.left() + 22.0, row.center().y - 1.0), Align2::LEFT_BOTTOM, &title, 16.0, Weight::Semi, TEXT);
                let this = self.link.cfg.follow.booth_name == f.name && asking;
                let sub = match (this, self.link.wants_code(), self.link.pair_pin) {
                    (true, true, true) => "Waiting for you to type its PIN".to_string(),
                    (true, true, false) => "Showing a code on its screen".to_string(),
                    (true, false, _) => words::studio_words(&self.link.pair_status),
                    _ => "Found on this network".to_string(),
                };
                let sub = paint::fit(ui, &sub, 13.0, Weight::Regular, tw);
                let tr = text_at(ui, pos2(row.left() + 22.0, row.center().y + 2.0), Align2::LEFT_TOP, &sub, 13.0, Weight::Regular, DIM);
                paint::read_as(ui, tr, &sub);
                let b = Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 48.0));
                join = button_named(ui, b, "Join", &format!("Join {}", f.name), Style::filled(TEXT, look::BG, 16.0, Weight::Bold, 12.0), !asking)
                    .clicked();
            });
            ui.add_space(10.0);
            if join {
                self.studio_join_found(&f.name, &f.addr);
            }
        }
        if found.is_empty() {
            let long = self.finding_since.is_some_and(|t| t.elapsed().as_secs() >= 6);
            egui::Frame::NONE.fill(CARD).corner_radius(14).inner_margin(Margin::same(16)).show(ui, |ui| {
                ui.set_width(w - 32.0);
                let (title, sub) = if long {
                    ("No studio found here yet", "On the studio's computer, FreeDAWm needs Setup › Network › Allow control from this network.")
                } else {
                    ("Looking for studios on this network…", "Searching started when you opened this screen.")
                };
                label(ui, title, 16.0, Weight::Semi, if long { TEXT } else { DIM });
                ui.add_space(4.0);
                paragraph(ui, sub, 13.0, Weight::Regular, DIM, 18.0);
            });
            ui.add_space(10.0);
        }
        ui.add_space(8.0);

        // The box: a PIN, the code on the studio's screen, or an invite.
        let studio = if self.link.cfg.follow.booth_name.is_empty() { "the studio".to_string() } else { self.link.cfg.follow.booth_name.clone() };
        let (prompt, hint) = if self.link.wants_code() && self.link.pair_pin {
            (format!("Type {studio}'s PIN"), "PIN")
        } else if self.link.wants_code() {
            ("Type the code showing on the studio's screen".to_string(), "4-digit code")
        } else {
            ("Somewhere else? Paste the PIN or invite they sent you".to_string(), "PIN or invite")
        };
        paragraph(ui, &prompt, 14.0, Weight::Semi, TEXT, 18.0);
        ui.add_space(8.0);
        let (row, _) = ui.allocate_exact_size(vec2(w, 48.0), Sense::hover());
        let bw = paint::text_width(ui, "Join", 15.0, Weight::Semi) + 36.0;
        let field = Rect::from_min_max(row.min, pos2(row.right() - bw - 10.0, row.bottom()));
        let mut text = std::mem::take(&mut self.studio_box);
        let r = paint::text_box(ui, field, &mut text, hint, "studio-box", 16.0, 12.0);
        self.studio_box = text;
        if self.link.wants_code() && !r.has_focus() && self.studio_box.is_empty() && ui.memory(|m| m.focused().is_none()) {
            r.request_focus();
        }
        let enter = r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter));
        let b = Rect::from_min_size(pos2(row.right() - bw, row.top()), vec2(bw, 48.0));
        let busy = self.link.busy();
        if button_in(ui, b, "Join", Style::outlined(15.0, Weight::Semi, 12.0), !busy).clicked() || (enter && !busy) {
            self.studio_join();
        }
        let status = self.link.pair_status.clone();
        if !status.is_empty() && status != "Paired." {
            ui.add_space(8.0);
            let (text, colour) = if status.ends_with('…') {
                (words::studio_words(&status), DIM)
            } else if self.link.wants_code() {
                let extra = if self.link.pair_ask { " Someone at the studio also needs to press Allow." } else { "" };
                let who = if studio == "the studio" { "The studio".to_string() } else { studio.clone() };
                let t =
                    if self.link.pair_pin { format!("{who} asks for its PIN.{extra}") } else { format!("{who} is showing a 4-digit code.{extra}") };
                (t, DIM)
            } else {
                (words::studio_error(&status), look::AMBER)
            };
            paragraph(ui, &text, 13.0, Weight::Regular, colour, 18.0);
        }

        // After you join: what it will look like (the hear-the-mix setting works already).
        let bottom = 15.0 + 12.0 + 128.0;
        let left = top + h - ui.cursor().top() - bottom;
        ui.add_space(left.max(18.0));
        caps(ui, "AFTER YOU JOIN");
        ui.add_space(12.0);
        self.joined_card(ui, true);
    }

    /// The joined studio's card: its name and song, what it is doing, this PC's input level,
    /// hearing its mix. `preview`: before joining (dashed, as the design shows it).
    fn joined_card(&mut self, ui: &mut Ui, preview: bool) {
        let w = ui.available_width();
        let view = if preview { None } else { self.studio_view() };
        let name = if preview { "The studio".to_string() } else { self.link.booth_name() };
        let title = match view.as_ref().map(|v| v.session.clone()).filter(|s| !s.is_empty()) {
            Some(song) => format!("{name} · song “{song}”"),
            None if preview => "The studio · its song".to_string(),
            None => name.clone(),
        };
        let (status, away) = match view.as_ref() {
            None => ("Waiting for the studio to press Record".to_string(), false),
            Some(v) if !v.connected => ("The studio is away. Waiting for it…".to_string(), true),
            Some(v) => (
                match &v.badge {
                    FollowBadge::Recording(_) => "Recording with the studio".to_string(),
                    FollowBadge::Sending(_, Some(p)) => format!("Sending your take… {p}%"),
                    FollowBadge::Sending(_, None) | FollowBadge::Preparing(_) => "Sending your take…".to_string(),
                    FollowBadge::Waiting(1) => "Your take is waiting to be sent".to_string(),
                    FollowBadge::Waiting(n) => format!("{n} takes are waiting to be sent"),
                    FollowBadge::Sent(_) => "Sent".to_string(),
                    FollowBadge::Following(_) | FollowBadge::Away(_) => "Waiting for the studio to press Record".to_string(),
                },
                false,
            ),
        };
        let level = match &self.rig {
            Some(r) if self.prefs.audio_on && r.input_channels() > 0 => r.level(),
            _ => 0.0,
        };
        let frame = if preview { egui::Frame::NONE } else { egui::Frame::NONE.fill(CARD) };
        let res = frame.corner_radius(14).inner_margin(Margin::symmetric(16, 14)).show(ui, |ui| {
            ui.set_width(w - 32.0);
            let iw = ui.available_width();
            let (row, _) = ui.allocate_exact_size(vec2(iw, 40.0), Sense::hover());
            let tw = iw - 84.0 - 12.0;
            let dot = if preview { 0.0 } else { 16.0 };
            if !preview {
                ui.painter().circle_filled(pos2(row.left() + 5.0, row.center().y - 9.0), 5.0, if away { look::AMBER } else { look::GREEN });
            }
            let t = paint::fit(ui, &title, 15.0, Weight::Semi, tw - dot);
            let tr = text_at(ui, pos2(row.left() + dot, row.center().y - 1.0), Align2::LEFT_BOTTOM, &t, 15.0, Weight::Semi, TEXT);
            paint::read_as(ui, tr, &title);
            let s = paint::fit(ui, &status, 13.0, Weight::Regular, tw);
            let sr = text_at(
                ui,
                pos2(row.left(), row.center().y + 2.0),
                Align2::LEFT_TOP,
                &s,
                13.0,
                Weight::Regular,
                if away { look::AMBER } else { DIM },
            );
            paint::read_as(ui, sr, &status);
            let bar = Rect::from_min_size(pos2(row.right() - 84.0, row.center().y - 5.0), vec2(84.0, 10.0));
            level_bar(ui, bar, level);
            paint::read_as(ui, bar, "Your input level");
            ui.add_space(12.0);
            let (row, _) = ui.allocate_exact_size(vec2(iw, look::TOUCH), Sense::hover());
            let mut listen = self.prefs.listen;
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(row.min, pos2(row.right() - 140.0, row.bottom()))));
            if paint::tick(&mut child, &mut listen, "Hear the studio's mix", 15.0, iw - 140.0).changed() {
                self.prefs.listen = listen;
            }
            let mut v = (self.prefs.listen_gain.max(0.0).sqrt() / 2f32.sqrt()).clamp(0.0, 1.0);
            let mut child = ui.new_child(UiBuilder::new().max_rect(Rect::from_min_max(pos2(row.right() - 130.0, row.top()), row.max)));
            if paint::slider(&mut child, &mut v, 130.0, "Mix volume").changed() {
                // Half way is as loud as the studio sends it; all the way, twice as loud.
                self.prefs.listen_gain = 2.0 * v * v;
            }
        });
        if preview {
            paint::dashed(ui.painter(), res.response.rect, 14.0, look::LINE_FIELD);
        }
        if !preview && let Some(h) = view.filter(|v| v.connected).and_then(|v| v.stream).filter(|_| self.prefs.listen) {
            ui.add_space(6.0);
            let lost = h.lost + h.late;
            let t = if lost == 0 { "The mix is coming through.".to_string() } else { format!("The mix is coming through ({lost} small dropouts).") };
            label(ui, &t, 13.0, Weight::Regular, DIM);
        }
    }
}

/// A line of text centred in the column.
fn centred(ui: &mut Ui, text: &str, size: f32, weight: Weight, colour: egui::Color32) {
    let w = ui.available_width();
    let shown = paint::fit(ui, text, size, weight, w);
    let h = paint::galley(ui, &shown, size, colour).size().y;
    let (rect, _) = ui.allocate_exact_size(vec2(w, h), Sense::hover());
    let r = text_at(ui, rect.center(), Align2::CENTER_CENTER, &shown, size, weight, colour);
    paint::read_as(ui, r, text);
}

/// Text centred and wrapped in the column.
fn centred_wrap(ui: &mut Ui, text: &str, size: f32, colour: egui::Color32) {
    let w = ui.available_width();
    let mut job = egui::text::LayoutJob::single_section(
        text.to_string(),
        egui::TextFormat { font_id: egui::FontId::proportional(size), color: colour, ..Default::default() },
    );
    job.wrap.max_width = w;
    job.halign = egui::Align::Center;
    let g = ui.painter().layout_job(job);
    let (rect, r) = ui.allocate_exact_size(vec2(w, g.size().y), Sense::hover());
    ui.painter().galley(pos2(rect.center().x, rect.top()), g, colour);
    r.widget_info(|| egui::WidgetInfo::labeled(egui::WidgetType::Label, true, text));
}

/// A choice from a list (the design's plain chooser): the current one shown, the list below.
fn combo<T: Clone>(ui: &mut Ui, id: &str, current: &str, width: f32, options: &[(String, T)], enabled: bool) -> Option<T> {
    let mut picked = None;
    ui.add_enabled_ui(enabled, |ui| {
        ui.spacing_mut().item_spacing = vec2(8.0, 4.0);
        ui.spacing_mut().button_padding = vec2(14.0, 12.0);
        egui::ComboBox::from_id_salt(("settings", id)).selected_text(egui::RichText::new(current).size(15.0)).width(width).height(320.0).show_ui(
            ui,
            |ui| {
                for (text, v) in options {
                    if ui
                        .add(egui::Button::selectable(text == current, egui::RichText::new(text).size(15.0)).min_size(vec2(width - 16.0, 36.0)))
                        .clicked()
                    {
                        picked = Some(v.clone());
                    }
                }
            },
        );
    });
    picked
}

//! The recorder's one window: quick capture, the recent takes, and (when built) the booth.
//!
//! The look is the DAW's dark theme (its colours, `soundcraft-ui-egui`'s `theme::Tokens::DARK`),
//! set here without linking the DAW's whole interface.

use crate::booth::Link;
use crate::files::{self, TakeFiles};
use crate::rig::{InputChoice, Rig, Saved};
use crate::settings::{Paths, Prefs, unique_stem};
use egui::{Color32, RichText, vec2};
use std::path::PathBuf;
use std::time::{Duration, Instant};

/// The DAW's dark palette (the few colours this window uses).
pub mod look {
    use egui::Color32;
    pub const PANEL: Color32 = Color32::from_rgb(37, 37, 38);
    pub const PANEL2: Color32 = Color32::from_rgb(44, 44, 45);
    pub const SLOT: Color32 = Color32::from_rgb(28, 28, 29);
    pub const TEXT: Color32 = Color32::from_rgb(214, 214, 214);
    pub const DIM: Color32 = Color32::from_rgb(150, 150, 152);
    pub const ACCENT: Color32 = Color32::from_rgb(64, 132, 196);
    pub const BUTTON: Color32 = Color32::from_rgb(58, 58, 60);
    pub const BUTTON_HI: Color32 = Color32::from_rgb(78, 78, 80);
    pub const REC: Color32 = Color32::from_rgb(214, 52, 46);
    pub const GOOD: Color32 = Color32::from_rgb(63, 185, 80);
    pub const WARN: Color32 = Color32::from_rgb(232, 148, 40);
    pub const METER_BG: Color32 = Color32::from_rgb(8, 8, 8);
    pub const METER_GREEN: Color32 = Color32::from_rgb(60, 200, 80);
    pub const METER_YELLOW: Color32 = Color32::from_rgb(230, 210, 60);
    pub const METER_RED: Color32 = Color32::from_rgb(230, 50, 40);

    /// The DAW's dark visuals on `ctx`.
    pub fn apply(ctx: &egui::Context) {
        let mut v = egui::Visuals::dark();
        v.panel_fill = PANEL;
        v.window_fill = PANEL2;
        v.extreme_bg_color = SLOT;
        v.faint_bg_color = PANEL2;
        v.selection.bg_fill = ACCENT;
        v.widgets.inactive.weak_bg_fill = BUTTON;
        v.widgets.inactive.bg_fill = BUTTON;
        v.widgets.hovered.weak_bg_fill = BUTTON_HI;
        v.widgets.noninteractive.fg_stroke.color = TEXT;
        v.widgets.inactive.fg_stroke.color = TEXT;
        v.window_corner_radius = egui::CornerRadius::same(6);
        ctx.set_theme(egui::Theme::Dark);
        ctx.all_styles_mut(|s| {
            s.visuals = v.clone();
            s.spacing.item_spacing = egui::vec2(6.0, 5.0);
            s.spacing.button_padding = egui::vec2(8.0, 3.0);
            s.interaction.tooltip_delay = 0.4;
        });
    }
}

/// The tooltip on the take's MIDI file.
pub const MIDI_TIP: &str = "The MIDI file is at 120 bpm, each note at the real time it was played from the take's start \
(not moved to a grid), so it lines up with the WAV in any program.";

/// The audio devices the pickers list.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DeviceLists {
    pub inputs: Vec<String>,
    pub outputs: Vec<String>,
}

pub struct RecorderApp {
    pub rig: Rig,
    pub link: Link,
    pub prefs: Prefs,
    paths: Option<Paths>,
    saved_prefs: Prefs,
    pub takes: Vec<TakeFiles>,
    scanned: Option<Instant>,
    /// The last thing to say (a take saved, a problem).
    pub message: Option<(String, bool)>,
    rename: Option<(String, String)>,
    confirm_delete: Option<String>,
    playing: Option<String>,
    pub devices: DeviceLists,
    /// Real devices (not a test's): the pickers may reopen them.
    live: bool,
}

/// `m:ss` or `h:mm:ss`.
pub fn clock(secs: f64) -> String {
    let t = secs.max(0.0) as u64;
    let (h, m, s) = (t / 3600, (t / 60) % 60, t % 60);
    if h > 0 { format!("{h}:{m:02}:{s:02}") } else { format!("{m}:{s:02}") }
}

impl RecorderApp {
    /// The app on the real devices, with the settings in `paths`.
    pub fn open(paths: Option<Paths>) -> RecorderApp {
        let prefs = paths.as_ref().map(|p| Prefs::load(&p.prefs)).unwrap_or_default();
        let rig = Rig::open(&prefs.audio, choice_of(&prefs), prefs.midi.clone());
        let mut app = RecorderApp::with_rig(rig, prefs, paths);
        app.live = true;
        app.refresh_devices();
        app
    }

    /// The app on `rig` (tests), with settings `prefs` and no settings files.
    pub fn with_rig(rig: Rig, prefs: Prefs, paths: Option<Paths>) -> RecorderApp {
        let link = Link::new(paths.as_ref().map(|p| p.network.clone()), paths.as_ref().map(|p| p.outbox.clone()));
        RecorderApp {
            rig,
            link,
            saved_prefs: prefs.clone(),
            prefs,
            paths,
            takes: Vec::new(),
            scanned: None,
            message: None,
            rename: None,
            confirm_delete: None,
            playing: None,
            devices: DeviceLists::default(),
            live: false,
        }
    }

    fn refresh_devices(&mut self) {
        let live = self.rig.player.live_device();
        let systems = soundcraft_playback::devices::audio_systems_with(Some(&live));
        let system = self.prefs.audio.system_name();
        let sys = systems.iter().find(|s| s.name == system).or_else(|| systems.iter().find(|s| s.is_default));
        self.devices = DeviceLists {
            inputs: sys.map(|s| s.inputs.iter().map(|d| d.name.clone()).collect()).unwrap_or_default(),
            outputs: sys.map(|s| s.outputs.iter().map(|d| d.name.clone()).collect()).unwrap_or_default(),
        };
    }

    /// Reopens the devices as the settings say (a device picked, Retry).
    fn reopen(&mut self) {
        if !self.live || self.rig.recording() {
            return;
        }
        self.rig.play_file(None).ok();
        self.rig = Rig::open(&self.prefs.audio, choice_of(&self.prefs), self.rig.midi_choices());
        self.refresh_devices();
    }

    /// Once a frame, before drawing.
    pub fn step(&mut self) {
        if let Some(saved) = self.rig.step() {
            self.said_saved(&saved);
        }
        let folder = self.prefs.ideas();
        let was = self.rig.recording();
        self.link.step(&mut self.rig, &folder, self.prefs.listen, self.prefs.listen_gain);
        if was != self.rig.recording() {
            self.rescan();
        }
        self.prefs.midi = self.rig.midi_choices();
        if self.prefs != self.saved_prefs {
            if let Some(p) = &self.paths
                && let Err(e) = self.prefs.save(&p.prefs)
            {
                self.message = Some((format!("Settings not saved: {e}"), true));
            }
            self.saved_prefs = self.prefs.clone();
        }
        if self.scanned.is_none_or(|t| t.elapsed() >= Duration::from_secs(2)) {
            self.rescan();
        }
    }

    pub fn rescan(&mut self) {
        self.takes = files::recent(&self.prefs.ideas(), 50);
        self.scanned = Some(Instant::now());
    }

    fn said_saved(&mut self, s: &Saved) {
        let mut what: Vec<String> = Vec::new();
        if let Some(w) = &s.wav {
            what.push(w.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default());
        }
        if let Some(m) = &s.mid {
            what.push(format!("{} ({} notes)", m.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(), s.notes));
        }
        let text = match (&s.problem, what.is_empty()) {
            (Some(p), true) => p.clone(),
            (Some(p), false) => format!("{p} Saved: {}", what.join(", ")),
            (None, true) => "Nothing was recorded (no audio input and no MIDI came).".into(),
            (None, false) => format!("Saved {}", what.join(" and ")),
        };
        self.message = Some((text, s.problem.is_some()));
        self.rescan();
    }

    /// The window closes: a take recording is stopped and saved.
    pub fn shutdown(&mut self) {
        if self.rig.recording() {
            let _ = self.rig.stop();
        }
        self.link.shutdown();
    }

    /// Record (or Stop) as the big button does.
    pub fn toggle_record(&mut self) {
        if self.rig.recording() {
            let saved = self.rig.stop();
            self.said_saved(&saved);
            return;
        }
        let dir = self.prefs.ideas();
        let stem = unique_stem(&dir, "Idea", crate::settings::now_stamp());
        match self.rig.start(&dir, &stem, false, 0) {
            Ok(()) => self.message = None,
            Err(e) => self.message = Some((e, true)),
        }
    }

    /// The whole window.
    pub fn ui(&mut self, ui: &mut egui::Ui) {
        self.step();
        egui::ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
            ui.add_space(4.0);
            ui.label(RichText::new("FreeDAWm Recorder").size(18.0).strong().color(look::TEXT));
            ui.add_space(4.0);
            self.capture_ui(ui);
            ui.separator();
            self.takes_ui(ui);
            ui.separator();
            self.booth_ui(ui);
            ui.separator();
            self.listen_ui(ui);
        });
        let busy = self.rig.recording() || self.playing.is_some();
        ui.ctx().request_repaint_after(Duration::from_millis(if busy { 50 } else { 250 }));
    }

    fn capture_ui(&mut self, ui: &mut egui::Ui) {
        let recording = self.rig.recording();
        // Inputs.
        ui.add_enabled_ui(!recording, |ui| {
            ui.horizontal(|ui| {
                ui.checkbox(&mut self.prefs.audio_on, "Audio").on_hover_text("Record the audio input");
                let current = self.rig.input_name().or_else(|| self.prefs.audio.input.clone()).unwrap_or_else(|| "Default input".into());
                let mut pick = None;
                egui::ComboBox::from_id_salt("rec-input").selected_text(&current).width(190.0).show_ui(ui, |ui| {
                    if ui.selectable_label(self.prefs.audio.input.is_none(), "Default input").clicked() {
                        pick = Some(None);
                    }
                    for d in &self.devices.inputs {
                        if ui.selectable_label(self.prefs.audio.input.as_deref() == Some(d), d).clicked() {
                            pick = Some(Some(d.clone()));
                        }
                    }
                });
                if let Some(p) = pick {
                    self.prefs.audio.input = p;
                    self.reopen();
                }
                let n = self.rig.input_channels().max(1);
                let mut choice = choice_of(&self.prefs);
                egui::ComboBox::from_id_salt("rec-channel").selected_text(choice.name()).width(80.0).show_ui(ui, |ui| {
                    for c in 0..n {
                        let mono = InputChoice { channel: c, stereo: false, ..choice };
                        ui.selectable_value(&mut choice, mono, mono.name());
                    }
                    for c in (0..n.saturating_sub(1)).step_by(2) {
                        let pair = InputChoice { channel: c, stereo: true, ..choice };
                        ui.selectable_value(&mut choice, pair, pair.name());
                    }
                });
                (self.prefs.channel, self.prefs.stereo) = (choice.channel, choice.stereo);
            });
            if let Some(e) = self.rig.input_error.clone() {
                ui.horizontal(|ui| {
                    ui.label(RichText::new(e).color(look::WARN));
                    if self.live && ui.small_button("Retry").clicked() {
                        self.reopen();
                    }
                });
            }
            self.rig.set_choice(choice_of(&self.prefs));
            let ports = self.rig.midi_ports();
            ui.horizontal_wrapped(|ui| {
                ui.label("MIDI:");
                if ports.is_empty() {
                    ui.label(RichText::new("No MIDI input").color(look::DIM));
                }
                for (name, on) in ports {
                    let mut on2 = on;
                    if ui.checkbox(&mut on2, &name).changed() {
                        self.rig.set_midi_port(&name, on2);
                    }
                }
            });
        });
        // Level.
        let level = if self.prefs.audio_on && self.rig.input_channels() > 0 { self.rig.level() } else { 0.0 };
        meter(ui, level);
        // Record / Stop.
        let can = self.rig.can_record() || recording;
        ui.add_space(4.0);
        ui.horizontal(|ui| {
            let (text, fill) = if recording { ("Stop", look::REC) } else { ("Record", look::BUTTON) };
            let ink = if recording { Color32::WHITE } else { look::REC };
            let b = egui::Button::new(RichText::new(text).size(20.0).strong().color(ink)).fill(fill).min_size(vec2(170.0, 48.0));
            let r = ui.add_enabled(can, b).on_disabled_hover_text("No audio input and no MIDI input");
            // The transport's symbols, drawn (the fonts may lack them): a dot, a square.
            let at = egui::pos2(r.rect.left() + 26.0, r.rect.center().y);
            let ink = if can { ink } else { look::DIM };
            if recording {
                ui.painter().rect_filled(egui::Rect::from_center_size(at, vec2(13.0, 13.0)), 1.0, ink);
            } else {
                ui.painter().circle_filled(at, 7.5, ink);
            }
            if r.clicked() {
                self.toggle_record();
            }
            let t = clock(self.rig.elapsed());
            ui.label(RichText::new(t).size(24.0).monospace().color(if recording { look::REC } else { look::DIM }));
        });
        if !can {
            ui.label(RichText::new("No input devices found: plug in an audio interface or a MIDI keyboard.").color(look::WARN));
        }
        if let Some((m, bad)) = &self.message {
            ui.label(RichText::new(m).color(if *bad { look::WARN } else { look::GOOD }));
        }
        // Folder.
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Saved in").color(look::DIM));
            ui.label(RichText::new(self.prefs.ideas().display().to_string()).monospace().small());
            if self.live && ui.add_enabled(!recording, egui::Button::new("Change…").small()).clicked() {
                let start = self.prefs.ideas();
                if let Some(dir) = rfd::FileDialog::new().set_directory(&start).pick_folder() {
                    self.prefs.ideas_folder = dir.to_string_lossy().into_owned();
                    self.rescan();
                }
            }
        });
        ui.label(RichText::new("Each take: a 24-bit WAV, and a MIDI file with the same name when MIDI was played.").small().color(look::DIM))
            .on_hover_text(MIDI_TIP);
    }

    fn takes_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Recent takes").strong());
        if self.takes.is_empty() {
            ui.label(RichText::new("No takes yet.").color(look::DIM));
            return;
        }
        let mut action: Option<(String, &'static str)> = None;
        // The take recording now is listed once it is finished.
        let current = self.rig.current_stem().map(str::to_string);
        for t in self.takes.iter().filter(|t| Some(&t.stem) != current.as_ref()) {
            ui.horizontal(|ui| {
                if let Some((stem, edit)) = &mut self.rename
                    && *stem == t.stem
                {
                    let r = ui.add(egui::TextEdit::singleline(edit).desired_width(200.0));
                    if r.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) || ui.small_button("OK").clicked() {
                        action = Some((t.stem.clone(), "rename-ok"));
                    }
                    if ui.small_button("Cancel").clicked() {
                        action = Some((t.stem.clone(), "rename-cancel"));
                    }
                    return;
                }
                let playing = self.playing.as_deref() == Some(t.stem.as_str());
                let play = ui.add_enabled(t.wav.is_some(), egui::Button::new(if playing { "■" } else { "▶" }).small()).on_hover_text("Play");
                if play.clicked() {
                    action = Some((t.stem.clone(), if playing { "stop" } else { "play" }));
                }
                ui.label(&t.stem);
                let mut kinds = Vec::new();
                if t.wav.is_some() {
                    kinds.push(t.seconds.map_or_else(|| "WAV".to_string(), |s| format!("WAV {}", clock(s))));
                }
                if t.mid.is_some() {
                    kinds.push("MIDI".to_string());
                }
                ui.label(RichText::new(kinds.join(" + ")).small().color(look::DIM)).on_hover_text(MIDI_TIP);
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if self.confirm_delete.as_deref() == Some(t.stem.as_str()) {
                        if ui.small_button("Cancel").clicked() {
                            action = Some((t.stem.clone(), "delete-cancel"));
                        }
                        if ui.add(egui::Button::new(RichText::new("Delete for good").color(look::REC)).small()).clicked() {
                            action = Some((t.stem.clone(), "delete-ok"));
                        }
                        ui.label(RichText::new("Delete? It cannot be undone.").color(look::WARN));
                        return;
                    }
                    if ui.small_button("Delete").clicked() {
                        action = Some((t.stem.clone(), "delete"));
                    }
                    if ui.small_button("Rename").clicked() {
                        action = Some((t.stem.clone(), "rename"));
                    }
                    if ui.small_button("Show").on_hover_text("Show in its folder").clicked() {
                        action = Some((t.stem.clone(), "reveal"));
                    }
                });
            });
        }
        if let Some((stem, what)) = action {
            self.take_action(&stem, what);
        }
    }

    /// Record into a booth: find it, pair, follow; what it does now.
    fn booth_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Record into a booth").strong())
            .on_hover_text("Another PC running FreeDAWm with Setup › Network › Allow control from this network on: this recorder records when it records, and sends it the take.");
        let link = &mut self.link;
        if !link.paired() {
            ui.horizontal(|ui| {
                if ui.button("Find booths").on_hover_text("Look for FreeDAWm booths on this network").clicked() {
                    link.find();
                }
                let found = link.found();
                let shown = if link.cfg.follow.booth_name.is_empty() { "Pick a booth…".to_string() } else { link.cfg.follow.booth_name.clone() };
                let mut pick = None;
                egui::ComboBox::from_id_salt("rec-booth").selected_text(shown).width(170.0).show_ui(ui, |ui| {
                    for f in &found {
                        if ui.selectable_label(f.name == link.cfg.follow.booth_name, format!("{} ({})", f.name, f.addr)).clicked() {
                            pick = Some((f.name.clone(), f.addr.clone()));
                        }
                    }
                    if found.is_empty() {
                        ui.label("None found yet");
                    }
                });
                if let Some((n, a)) = pick {
                    link.choose(&n, &a);
                }
            });
            ui.horizontal(|ui| {
                ui.label("or address:");
                let r = ui.add(egui::TextEdit::singleline(&mut link.addr_edit).hint_text("192.168.1.20:48100").desired_width(150.0));
                if r.lost_focus() && link.addr_edit.trim() != link.cfg.follow.booth_addr {
                    let a = link.addr_edit.clone();
                    link.choose("", &a);
                }
                if ui
                    .add_enabled(!link.cfg.follow.booth_addr.is_empty(), egui::Button::new("Pair…"))
                    .on_hover_text("The booth shows a 4-digit code")
                    .clicked()
                {
                    link.pair();
                }
            });
            if link.wants_code() {
                ui.horizontal(|ui| {
                    ui.add(egui::TextEdit::singleline(&mut link.code_edit).desired_width(60.0).hint_text("0000"));
                    if ui.button("Pair").clicked() {
                        link.confirm();
                    }
                });
            }
            if !link.pair_status.is_empty() {
                ui.label(RichText::new(&link.pair_status).color(look::DIM));
            }
            ui.label(RichText::new("Nothing is sent anywhere until you pair. The booth sees this PC as").small().color(look::DIM));
            ui.label(RichText::new(link.my_name()).small().monospace());
        } else {
            let name = if link.cfg.follow.booth_name.is_empty() { link.cfg.follow.booth_addr.clone() } else { link.cfg.follow.booth_name.clone() };
            ui.horizontal(|ui| {
                ui.label(RichText::new(format!("✔ Paired with {name}")).color(look::GOOD));
                let mut on = link.cfg.follow.enabled;
                if ui.add_enabled(!self.rig.recording() || !on, egui::Checkbox::new(&mut on, "Follow")).changed() {
                    link.set_follow(on);
                }
                if ui.add_enabled(!self.rig.recording(), egui::Button::new("Forget").small()).clicked() {
                    link.forget();
                }
            });
        }
        if let Some(e) = &link.error {
            ui.label(RichText::new(e).color(look::WARN));
        }
        let Some(v) = link.view(&self.rig) else { return };
        let colour = if v.badge.recording() {
            look::REC
        } else if v.connected {
            look::TEXT
        } else {
            look::WARN
        };
        let detail = if v.connected { String::new() } else { format!(" ({})", v.state) };
        ui.label(RichText::new(format!("{}{detail}", v.badge.text())).strong().color(colour));
        if v.connected {
            let session = if v.session.is_empty() { "…".to_string() } else { v.session.clone() };
            ui.label(format!("Session: {session}   {} {}", v.transport, clock(v.position)));
        }
        let outbox = match (v.progress, v.queued) {
            (Some(p), _) => format!("Sending take: {}%", (p * 100.0).clamp(0.0, 100.0) as u8),
            (None, 0) => "Outbox: empty".to_string(),
            (None, 1) => "Outbox: 1 take waiting".to_string(),
            (None, n) => format!("Outbox: {n} takes waiting"),
        };
        ui.label(RichText::new(format!("{outbox}   Link: {}", v.link)).small().color(look::DIM))
            .on_hover_text("Takes are kept on this PC until the booth says it has them, also when it is away or this window closes.");
        if let Some(said) = &link.said {
            ui.label(RichText::new(said).small().color(look::DIM));
        }
    }

    /// Listen to the booth's mix.
    fn listen_ui(&mut self, ui: &mut egui::Ui) {
        ui.label(RichText::new("Listen to the booth's mix").strong());
        ui.horizontal(|ui| {
            ui.checkbox(&mut self.prefs.listen, "Listen").on_hover_text("The booth streams what it hears; it plays here");
            let current = self.prefs.audio.output.clone().unwrap_or_else(|| "Default output".into());
            let mut pick = None;
            egui::ComboBox::from_id_salt("rec-output").selected_text(current).width(170.0).show_ui(ui, |ui| {
                if ui.selectable_label(self.prefs.audio.output.is_none(), "Default output").clicked() {
                    pick = Some(None);
                }
                for d in &self.devices.outputs {
                    if ui.selectable_label(self.prefs.audio.output.as_deref() == Some(d), d).clicked() {
                        pick = Some(Some(d.clone()));
                    }
                }
            });
            if let Some(p) = pick {
                self.prefs.audio.output = p;
                self.reopen();
            }
        });
        ui.horizontal(|ui| {
            ui.label("Volume");
            let mut db = if self.prefs.listen_gain <= 1e-4 { -60.0 } else { 20.0 * self.prefs.listen_gain.log10() };
            if ui.add(egui::Slider::new(&mut db, -60.0..=6.0).suffix(" dB").fixed_decimals(0)).changed() {
                self.prefs.listen_gain = if db <= -60.0 { 0.0 } else { 10f32.powf(db / 20.0) };
            }
        });
        if let Some(p) = &self.rig.output_problem {
            ui.label(RichText::new(p).color(look::WARN));
        }
        let view = self.link.view(&self.rig);
        let text = match (self.prefs.listen, view.as_ref()) {
            (false, _) => "Off".to_string(),
            (true, None) => "Pair with a booth and follow it to hear its mix.".to_string(),
            (true, Some(v)) if !v.connected => "The booth is away.".to_string(),
            (true, Some(v)) => match &v.stream {
                Some(h) => format!("Buffer {:.0} ms · dropouts {} · late {}", h.buffer_ms, h.lost, h.late),
                None => "Waiting for the booth's mix (it streams while it plays or records)…".to_string(),
            },
        };
        ui.label(RichText::new(text).small().color(look::DIM));
    }

    /// A button on a take's row.
    pub fn take_action(&mut self, stem: &str, what: &str) {
        let Some(t) = self.takes.iter().find(|t| t.stem == stem).cloned() else { return };
        let result = match what {
            "play" => {
                let r = self.rig.play_file(t.wav.as_deref());
                self.playing = r.is_ok().then(|| stem.to_string());
                r
            }
            "stop" => {
                self.playing = None;
                self.rig.play_file(None)
            }
            "reveal" => files::reveal(t.wav.as_deref().or(t.mid.as_deref()).unwrap_or(&PathBuf::new())),
            "rename" => {
                self.rename = Some((stem.to_string(), stem.to_string()));
                Ok(())
            }
            "rename-ok" => {
                let new = self.rename.take().map(|(_, n)| n).unwrap_or_default();
                files::rename(&t, &new).map(|_| self.rescan())
            }
            "rename-cancel" => {
                self.rename = None;
                Ok(())
            }
            "delete" => {
                self.confirm_delete = Some(stem.to_string());
                Ok(())
            }
            "delete-ok" => {
                self.confirm_delete = None;
                if self.playing.as_deref() == Some(stem) {
                    self.playing = None;
                    let _ = self.rig.play_file(None);
                }
                files::delete(&t).map(|()| self.rescan())
            }
            "delete-cancel" => {
                self.confirm_delete = None;
                Ok(())
            }
            _ => Ok(()),
        };
        if let Err(e) = result {
            self.message = Some((e, true));
        }
    }
}

fn choice_of(p: &Prefs) -> InputChoice {
    InputChoice { on: p.audio_on, channel: p.channel, stereo: p.stereo }
}

/// A horizontal peak meter (0..1).
fn meter(ui: &mut egui::Ui, level: f32) {
    let (rect, _) = ui.allocate_exact_size(vec2(ui.available_width().min(360.0), 10.0), egui::Sense::hover());
    let p = ui.painter();
    p.rect_filled(rect, 2.0, look::METER_BG);
    // dB scale, -60..0.
    let db = if level <= 1e-6 { -120.0 } else { 20.0 * level.log10() };
    let pos = ((db + 60.0) / 60.0).clamp(0.0, 1.0);
    let colour = if db >= -0.5 {
        look::METER_RED
    } else if db >= -12.0 {
        look::METER_YELLOW
    } else {
        look::METER_GREEN
    };
    let mut lit = rect;
    lit.set_width(rect.width() * pos);
    p.rect_filled(lit, 2.0, colour);
}

//! Event-menu operation windows: Beat Detective, Tempo Operations, Time Operations, MIDI
//! Operations and MIDI Real-Time Properties. Each is a small form over engine commands.

use crate::SoundApp;
use crate::theme::Tokens;
use egui::vec2;
use serde_json::{Value, json};

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct OpsState {
    pub beat_sensitivity: f32,
    pub beat_separate: bool,
    pub beat_set_tempo: bool,
    pub beat_result: String,
    pub tempo_curve: String,
    /// Tap Tempo's taps (seconds of egui time); not saved.
    #[serde(skip)]
    pub tap_times: Vec<f64>,
    pub tempo_start: f64,
    pub tempo_end: f64,
    pub time_op: String,
    pub time_seconds: f64,
    pub meter_num: u32,
    pub meter_den: u32,
    pub midi_op: String,
    pub quant_grid: String,
    pub quant_strength: f64,
    pub quant_swing: f64,
    /// Quantize: only notes within this % of half a grid step, a random offset (% of half a
    /// step), note ends too, lengths kept, and the groove followed instead of the grid ("": none).
    pub quant_window: f64,
    pub quant_random: f64,
    pub quant_ends: bool,
    pub quant_keep_length: bool,
    pub quant_groove: String,
    /// The MIDI Real-Time Properties' playback quantize: grid ("off" or a note value) and strength.
    pub rtp_quantize: String,
    pub rtp_q_strength: f64,
    pub transpose: i64,
    /// Transpose by steps of the key (on the Key ruler) instead of semitones.
    pub transpose_in_key: bool,
    pub velocity_add: i64,
    /// Change Duration: "legato", "set", "add" or "scale", and its value (ticks, or percent to scale).
    pub dur_mode: String,
    pub dur_value: f64,
    /// Strum: ticks between strings and upward or not; Humanize: ± ticks and ± velocity; Half or
    /// Double Time: half time (twice as long) or not.
    pub strum_ticks: i64,
    pub strum_up: bool,
    pub human_timing: i64,
    pub human_velocity: i64,
    pub half_time: bool,
    /// Flam: ticks before each note, and the grace note's velocity in percent of the note's.
    pub flam_ticks: i64,
    pub flam_velocity: f64,
    /// Select Notes: pitch, velocity and length (ticks) ranges, the beats of the bar a note
    /// starts in (1-based, fractional), and whether matching notes move to a new track instead.
    pub rule_pitch: (i64, i64),
    pub rule_velocity: (i64, i64),
    pub rule_length: (i64, i64),
    pub rule_beats: Option<(f64, f64)>,
    pub rule_split: bool,
    /// Select Notes: the channels (from 1).
    pub rule_channel: (i64, i64),
    pub rtp_velocity: i64,
    pub rtp_transpose: i64,
    pub rtp_duration: f64,
    pub rtp_delay: i64,
    /// The tracks the MIDI Real-Time Properties window's values were read from: read again when
    /// the selection changes.
    #[serde(skip)]
    pub rtp_tracks: Vec<u64>,
    pub identify_bars: u32,
}

impl Default for OpsState {
    fn default() -> Self {
        OpsState {
            beat_sensitivity: 0.5,
            beat_separate: true,
            beat_set_tempo: false,
            beat_result: String::new(),
            tempo_curve: "linear".into(),
            tap_times: Vec::new(),
            tempo_start: 100.0,
            tempo_end: 120.0,
            time_op: "insert".into(),
            time_seconds: 4.0,
            meter_num: 4,
            meter_den: 4,
            midi_op: "quantize".into(),
            quant_grid: "1/16".into(),
            quant_strength: 100.0,
            quant_swing: 0.0,
            quant_window: 100.0,
            quant_random: 0.0,
            quant_ends: false,
            quant_keep_length: true,
            quant_groove: String::new(),
            rtp_quantize: "off".into(),
            rtp_q_strength: 100.0,
            transpose: 12,
            transpose_in_key: false,
            velocity_add: 10,
            dur_mode: "legato".into(),
            dur_value: 0.0,
            strum_ticks: 30,
            strum_up: true,
            human_timing: 10,
            human_velocity: 8,
            half_time: true,
            flam_ticks: 30,
            flam_velocity: 70.0,
            rule_pitch: (0, 127),
            rule_velocity: (1, 127),
            rule_length: (1, 15_360),
            rule_beats: None,
            rule_split: false,
            rule_channel: (1, 16),
            rtp_velocity: 0,
            rtp_transpose: 0,
            rtp_duration: 100.0,
            rtp_delay: 0,
            rtp_tracks: Vec::new(),
            identify_bars: 1,
        }
    }
}

fn result_line(ui: &mut egui::Ui, r: &Result<Value, String>) {
    match r {
        Ok(v) => {
            let s = v.to_string();
            ui.label(egui::RichText::new(if s.len() > 160 { format!("{}…", &s[..160]) } else { s }).small().color(Tokens::current().counter_text));
        }
        Err(e) => {
            ui.label(egui::RichText::new(e).small().color(Tokens::current().rec));
        }
    }
}

pub fn show(app: &mut SoundApp, ctx: &egui::Context) {
    beat_detective(app, ctx);
    tempo_ops(app, ctx);
    time_ops(app, ctx);
    midi_ops(app, ctx);
    rtp(app, ctx);
    input_quantize(app, ctx);
    midi_offsets(app, ctx);
    click_countoff(app, ctx);
    disk_allocation(app, ctx);
}

/// Setup › Disk Allocation: the folder the session's takes record into, and any track's own
/// (`setup.disk_allocation`). A relative folder is inside the session's folder; empty is the
/// default (`Audio Files`, or for a track the session's folder).
fn disk_allocation(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_disk_allocation;
    if !open {
        return;
    }
    // As `setup.disk_allocation` reports them (read here, not run every frame).
    let s = app.engine.session();
    let text = |key: &str| s.edit.flags.iter().find_map(|f| f.strip_prefix(key).and_then(|r| r.strip_prefix('=')).map(str::to_string));
    let root = text("disk.root").unwrap_or_else(|| "Audio Files".into());
    let rows: Vec<(u64, String, String)> = s
        .tracks
        .iter()
        .filter(|t| t.kind == soundcraft_model::TrackKind::Audio)
        .map(|t| (t.id.0, t.name.clone(), text(&format!("disk.track.{}", t.id.0)).unwrap_or_else(|| root.clone())))
        .collect();
    let mut change: Option<Value> = None;
    egui::Window::new("Disk Allocation").open(&mut open).default_size(vec2(460.0, 260.0)).show(ctx, |ui| {
        ui.label(
            egui::RichText::new(
                "Where takes are recorded. A folder that is not inside the session's folder keeps its takes there when the session is saved.",
            )
            .small(),
        );
        let mut folder_row = |ui: &mut egui::Ui, label: &str, key: &str, value: &str, track: Option<u64>| {
            ui.label(label);
            let id = egui::Id::new(("disk_alloc", key));
            let mut text: String = ui.ctx().data(|d| d.get_temp(id)).unwrap_or_else(|| value.to_string());
            let r = ui.add(egui::TextEdit::singleline(&mut text).desired_width(240.0));
            ui.ctx().data_mut(|d| d.insert_temp(id, text.clone()));
            let mut pick = None;
            if let Some(choose) = &app.services.pick_folder
                && ui.small_button("Choose…").clicked()
            {
                pick = choose("disk.allocation");
            }
            let commit = pick.or_else(|| (r.lost_focus() && text != value).then(|| text.clone()));
            if let Some(path) = commit {
                ui.ctx().data_mut(|d| d.remove::<String>(id));
                let mut p = json!({"path": path});
                if let (Some(t), Some(o)) = (track, p.as_object_mut()) {
                    o.insert("track".into(), json!(t));
                }
                change = Some(p);
            }
            ui.end_row();
        };
        egui::Grid::new("disk_allocation").num_columns(3).striped(true).show(ui, |ui| {
            folder_row(ui, "Session", "root", &root, None);
            for (id, name, path) in &rows {
                folder_row(ui, name, &id.to_string(), path, Some(*id));
            }
        });
    });
    if let Some(p) = change
        && let Err(e) = app.run("setup.disk_allocation", p)
    {
        app.ui.status = e;
    }
    app.ui.show_disk_allocation = open;
}

/// Setup › Click/Countoff: the metronome's level and sounds, whether it clicks only while
/// recording, and the count-off before a take. Shows the session's settings; each change runs
/// `setup.click_countoff`.
fn click_countoff(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_click_countoff;
    if !open {
        return;
    }
    let mut pickup = app.engine.session().edit.flag("midi.keep_pickup");
    // Where the click plays: the main mix, or one hardware output alone (a performer's phones).
    let outputs: Vec<String> = app.engine.session().outputs.iter().skip(1).map(|o| o.name.clone()).collect();
    let click_out: Option<String> = app.engine.session().edit.flags.iter().find_map(|f| f.strip_prefix("click.output=").map(str::to_string));
    let (mut db, mut accent, mut normal, mut countoff, mut bars, mut only_rec) = {
        let e = &app.engine.session().edit;
        (
            e.value("click.volume_db", -6.0),
            e.value("click.accent_note", 84.0) as u8,
            e.value("click.normal_note", 72.0) as u8,
            e.countoff,
            e.countoff_bars,
            e.flag("click.only_during_record"),
        )
    };
    egui::Window::new("Click/Countoff").open(&mut open).default_size(vec2(300.0, 180.0)).show(ctx, |ui| {
        let mut p = serde_json::Map::new();
        if ui.add(egui::Slider::new(&mut db, -60.0..=12.0).text("Level dB")).changed() {
            p.insert("volume_db".into(), json!(db));
        }
        ui.horizontal(|ui| {
            ui.label("Accent note");
            if ui.add(egui::DragValue::new(&mut accent).range(0..=127)).changed() {
                p.insert("accent_note".into(), json!(accent));
            }
            ui.label("Other beats");
            if ui.add(egui::DragValue::new(&mut normal).range(0..=127)).changed() {
                p.insert("normal_note".into(), json!(normal));
            }
        });
        if crate::widgets::checkbox(ui, &mut only_rec, "Only while recording").changed() {
            p.insert("only_during_record".into(), json!(only_rec));
        }
        ui.horizontal(|ui| {
            ui.label("Output");
            let shown = click_out.clone().unwrap_or_else(|| "Main mix".into());
            egui::ComboBox::from_id_salt("click_output")
                .selected_text(shown)
                .show_ui(ui, |ui| {
                    if ui.selectable_label(click_out.is_none(), "Main mix").clicked() {
                        p.insert("output".into(), json!("main"));
                    }
                    for o in &outputs {
                        if ui.selectable_label(click_out.as_deref().is_some_and(|c| soundcraft_model::same_name(c, o)), o).clicked() {
                            p.insert("output".into(), json!(o));
                        }
                    }
                })
                .response
                .on_hover_text("A hardware output plays the click alone, kept out of the main mix (Setup › I/O names the outputs)");
        });
        ui.separator();
        ui.horizontal(|ui| {
            if crate::widgets::checkbox(ui, &mut countoff, "Count off").changed() {
                p.insert("countoff".into(), json!(countoff));
            }
            if ui.add_enabled(countoff, egui::DragValue::new(&mut bars).range(1..=16).suffix(" bars")).changed() {
                p.insert("countoff_bars".into(), json!(bars));
            }
        });
        ui.label(egui::RichText::new("The count-off plays before a take started with Record, not before Play.").small());
        if crate::widgets::checkbox(ui, &mut pickup, "Keep MIDI played during the count-off")
            .on_hover_text("Notes played in the count-off are a pickup: the MIDI take begins at the bar they were played in")
            .changed()
        {
            p.insert("keep_pickup".into(), json!(pickup));
        }
        if !p.is_empty()
            && let Err(e) = app.run("setup.click_countoff", Value::Object(p))
        {
            app.ui.status = e;
        }
    });
    app.ui.show_click_countoff = open;
}

/// The input quantize grids offered, as `event.input_quantize` takes them, and their ticks.
const INPUT_GRIDS: [(&str, i64); 6] = [("1/4", 960), ("1/8", 480), ("1/8t", 320), ("1/16", 240), ("1/16t", 160), ("1/32", 120)];

/// Event › MIDI Operations › Input Quantize: whether MIDI takes are quantized as they are placed
/// (the played performance is kept to restore), on which grid, how strongly and with what swing.
/// Shows the session's settings; each change runs `event.input_quantize`.
fn input_quantize(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_input_quantize;
    if !open {
        return;
    }
    let (mut on, grid, mut strength, mut swing) = {
        let e = &app.engine.session().edit;
        (
            e.flag("midi.input_quantize"),
            e.value("midi.input_quantize.grid_ticks", 240.0) as i64,
            e.value("midi.input_quantize.strength", 100.0),
            e.value("midi.input_quantize.swing", 0.0),
        )
    };
    let mut pick = INPUT_GRIDS.iter().find(|g| g.1 == grid).map_or("1/16", |g| g.0);
    egui::Window::new("Input Quantize").open(&mut open).default_size(vec2(300.0, 150.0)).show(ctx, |ui| {
        let mut p = serde_json::Map::new();
        if crate::widgets::checkbox(ui, &mut on, "Quantize MIDI takes as they are recorded").changed() {
            p.insert("enabled".into(), json!(on));
        }
        ui.add_enabled_ui(on, |ui| {
            egui::ComboBox::from_label("Grid").selected_text(pick).show_ui(ui, |ui| {
                for (g, _) in INPUT_GRIDS {
                    if ui.selectable_value(&mut pick, g, g).clicked() {
                        p.insert("grid".into(), json!(g));
                    }
                }
            });
            if ui.add(egui::Slider::new(&mut strength, 0.0..=100.0).text("Strength %")).changed() {
                p.insert("strength".into(), json!(strength));
            }
            if ui.add(egui::Slider::new(&mut swing, 0.0..=100.0).text("Swing %")).changed() {
                p.insert("swing".into(), json!(swing));
            }
        });
        ui.label(egui::RichText::new("Notes only; the played timing is kept (Restore Performance).").small());
        if !p.is_empty()
            && let Err(e) = app.run("event.input_quantize", Value::Object(p))
        {
            app.ui.status = e;
        }
    });
    app.ui.show_input_quantize = open;
}

/// Event › MIDI Track Offsets: each MIDI and instrument track's playback offset in milliseconds
/// (its notes sound that much later, earlier if negative). Each change runs
/// `event.midi_track_offsets`.
fn midi_offsets(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_offsets;
    if !open {
        return;
    }
    let rows: Vec<(u64, String, f64)> = {
        let s = app.engine.session();
        let ms = |samples: f64| samples * 1000.0 / s.sample_rate.as_f64();
        s.tracks
            .iter()
            .filter(|t| t.kind.is_midi())
            .map(|t| (t.id.0, t.name.clone(), ms(s.edit.value(&format!("midi.offset.{}", t.id.0), 0.0))))
            .collect()
    };
    egui::Window::new("MIDI Track Offsets").open(&mut open).default_size(vec2(300.0, 200.0)).show(ctx, |ui| {
        if rows.is_empty() {
            ui.label("No MIDI or instrument tracks.");
        }
        egui::Grid::new("midi_offsets").num_columns(2).show(ui, |ui| {
            for (id, name, ms) in rows {
                let mut v = ms;
                ui.label(name);
                if ui.add(egui::DragValue::new(&mut v).range(-10_000.0..=10_000.0).speed(0.5).suffix(" ms")).changed()
                    && let Err(e) = app.run("event.midi_track_offsets", json!({"track": id, "offset": {"seconds": v / 1000.0}}))
                {
                    app.ui.status = e;
                }
                ui.end_row();
            }
        });
    });
    app.ui.show_midi_offsets = open;
}

fn beat_detective(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_beat_detective;
    if !open {
        return;
    }
    egui::Window::new("Beat Detective").open(&mut open).default_size(vec2(360.0, 220.0)).show(ctx, |ui| {
        ui.label("Analyses the edit selection on the selected tracks.");
        ui.add(egui::Slider::new(&mut app.ops.beat_sensitivity, 0.0..=1.0).text("Sensitivity"));
        crate::widgets::checkbox(ui, &mut app.ops.beat_separate, "Separate clips at detected beats");
        crate::widgets::checkbox(ui, &mut app.ops.beat_set_tempo, "Set the session tempo from the beats");
        ui.horizontal(|ui| {
            if ui.button("Analyze").clicked() {
                let r = app.run("event.beat_detective", json!({"sensitivity": app.ops.beat_sensitivity, "separate": false, "set_tempo": false}));
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
            if ui.button(egui::RichText::new("Apply").strong()).clicked() {
                let r = app.run(
                    "event.beat_detective",
                    json!({"sensitivity": app.ops.beat_sensitivity, "separate": app.ops.beat_separate, "set_tempo": app.ops.beat_set_tempo}),
                );
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
        });
        ui.separator();
        ui.horizontal(|ui| {
            ui.label("Identify Beat: selection is");
            ui.add(egui::DragValue::new(&mut app.ops.identify_bars).range(1..=64));
            ui.label("bar(s)");
            if ui.button("Set Tempo").clicked() {
                let r = app.run("event.identify_beat", json!({"bars": app.ops.identify_bars}));
                app.ops.beat_result = r.map_or_else(|e| e, |v| v.to_string());
            }
        });
        if !app.ops.beat_result.is_empty() {
            let s = app.ops.beat_result.clone();
            ui.label(egui::RichText::new(if s.len() > 300 { format!("{}…", &s[..300]) } else { s }).small());
        }
    });
    app.ui.show_beat_detective = open;
}

/// Command the Tempo Operations Apply button sends. Stretch uses `factor = start / end`
/// so a lower end BPM lengthens the selection (`event.tempo_stretch` has no `end_bpm`).
pub(crate) fn tempo_ops_apply(curve: &str, start_bpm: f64, end_bpm: f64) -> (&'static str, Value) {
    let start = if start_bpm.is_finite() { start_bpm.max(1.0) } else { 1.0 };
    let end = if end_bpm.is_finite() { end_bpm.max(1.0) } else { 1.0 };
    match curve {
        "constant" => ("event.tempo_constant", json!({"bpm": start_bpm})),
        "parabolic" => ("event.tempo_parabolic", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
        "s-curve" => ("event.tempo_s_curve", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
        "scale" => ("event.tempo_scale", json!({"factor": end / start})),
        "stretch" => ("event.tempo_stretch", json!({"factor": start / end})),
        _ => ("event.tempo_linear", json!({"start_bpm": start_bpm, "end_bpm": end_bpm})),
    }
}

fn tempo_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_tempo_ops;
    if !open {
        return;
    }
    egui::Window::new("Tempo Operations").open(&mut open).default_size(vec2(360.0, 200.0)).show(ctx, |ui| {
        egui::ComboBox::from_label("Curve").selected_text(app.ops.tempo_curve.clone()).show_ui(ui, |ui| {
            for c in ["constant", "linear", "parabolic", "s-curve", "scale", "stretch"] {
                ui.selectable_value(&mut app.ops.tempo_curve, c.to_string(), c);
            }
        });
        ui.add(egui::DragValue::new(&mut app.ops.tempo_start).range(5.0..=999.0).prefix("start bpm "));
        ui.add(egui::DragValue::new(&mut app.ops.tempo_end).range(5.0..=999.0).prefix("end bpm "));
        ui.label(
            egui::RichText::new(
                "Applies across the edit selection (constant: at the insertion point; scale: factor = end / start; stretch: factor = start / end).",
            )
            .small(),
        );
        ui.horizontal(|ui| {
            // Each tap after the first sets the tempo in effect at the insertion point; one undo step per run of taps.
            if ui.button("Tap").on_hover_text("Tap Tempo: click in time; a pause of 2 s starts over").clicked() {
                let now = ui.input(|i| i.time);
                tap_tempo(app, now);
            }
            if let Some(bpm) = soundcraft_engine::cmd::tap_tempo::tapped_bpm(&app.ops.tap_times) {
                ui.label(format!("{bpm:.1} bpm tapped"));
            }
        });
        if ui.button(egui::RichText::new("Apply").strong()).clicked() {
            let (id, p) = tempo_ops_apply(&app.ops.tempo_curve, app.ops.tempo_start, app.ops.tempo_end);
            let r = app.run(id, p);
            result_line(ui, &r);
        }
    });
    app.ui.show_tempo_ops = open;
}

/// One Tap Tempo tap at `now` seconds: kept with the taps since the last pause, and from the
/// second tap on the tempo they beat is set (`event.tap_tempo`), the run's taps merged into one undo step.
pub fn tap_tempo(app: &mut SoundApp, now: f64) {
    use soundcraft_engine::cmd::tap_tempo::{TAP_KEEP, TAP_RESET_SECONDS};
    let taps = &mut app.ops.tap_times;
    if taps.last().is_none_or(|t| now - t >= TAP_RESET_SECONDS || now <= *t) {
        taps.clear();
    }
    taps.push(now);
    if taps.len() > TAP_KEEP {
        taps.remove(0);
    }
    if taps.len() >= 2 {
        let key = format!("tap_tempo:{}", taps.first().copied().unwrap_or(0.0));
        let r = app.engine.execute_merged("event.tap_tempo", &json!({"taps": taps.clone()}), &key);
        if let Ok(v) = r
            && let Some(bpm) = v["bpm"].as_f64()
        {
            app.ops.tempo_start = bpm;
        }
    }
}

fn time_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_time_ops;
    if !open {
        return;
    }
    egui::Window::new("Time Operations").open(&mut open).default_size(vec2(360.0, 200.0)).show(ctx, |ui| {
        egui::ComboBox::from_label("Operation").selected_text(app.ops.time_op.clone()).show_ui(ui, |ui| {
            for c in ["insert", "cut", "change meter", "move song start"] {
                ui.selectable_value(&mut app.ops.time_op, c.to_string(), c);
            }
        });
        match app.ops.time_op.as_str() {
            "change meter" => {
                ui.horizontal(|ui| {
                    ui.add(egui::DragValue::new(&mut app.ops.meter_num).range(1..=64));
                    ui.label("/");
                    egui::ComboBox::from_id_salt("den").selected_text(app.ops.meter_den.to_string()).show_ui(ui, |ui| {
                        for d in [2u32, 4, 8, 16] {
                            ui.selectable_value(&mut app.ops.meter_den, d, d.to_string());
                        }
                    });
                });
            }
            "insert" => {
                ui.add(egui::DragValue::new(&mut app.ops.time_seconds).range(0.0..=3600.0).suffix(" s"));
            }
            _ => {
                ui.label(egui::RichText::new("Uses the edit selection.").small());
            }
        }
        if ui.button(egui::RichText::new("Apply").strong()).clicked() {
            let at = app.engine.session().edit.selection.start;
            let r = match app.ops.time_op.as_str() {
                "insert" => app.run("event.insert_time", json!({"start": at, "length": {"seconds": app.ops.time_seconds}})),
                "cut" => app.run("event.cut_time", json!({})),
                "change meter" => app.run("event.meter", json!({"numerator": app.ops.meter_num, "denominator": app.ops.meter_den, "at": at})),
                _ => app.run("event.move_song_start", json!({"to": at})),
            };
            result_line(ui, &r);
        }
    });
    app.ui.show_time_ops = open;
}

/// The MIDI Operations window's operations.
pub const MIDI_OPS: [&str; 14] = [
    "select notes",
    "quantize",
    "transpose",
    "change velocity",
    "change duration",
    "fix overlaps",
    "humanize",
    "strum",
    "flam",
    "reverse",
    "invert pitches",
    "half or double time",
    "flatten performance",
    "restore performance",
];

/// The grids Quantize offers (`event.quantize` reads each).
pub const QUANTIZE_GRIDS: [&str; 14] =
    ["1/1", "1/2", "1/4", "1/4t", "1/4d", "1/8", "1/8t", "1/8d", "1/16", "1/16t", "1/16d", "1/32", "1/32t", "1/64"];

/// The notes selected in the MIDI Editor while it shows them: the operation acts on those.
pub fn editor_selection(app: &SoundApp) -> Option<(soundcraft_model::ClipId, Vec<usize>)> {
    let clip = app.ui.show_midi_editor.then(|| crate::midi_editor::target_clip(app)).flatten()?;
    (!app.midi.selected.is_empty() && app.midi.clip == Some(clip)).then(|| (clip, app.midi.selected.clone()))
}

/// The command and values the window's operation runs (`None`: Flatten or Restore Performance,
/// which act on whole clips).
pub fn midi_op_params(o: &OpsState) -> Option<(&'static str, serde_json::Value)> {
    Some(match o.midi_op.as_str() {
        "quantize" => {
            let mut p = json!({
                "grid": o.quant_grid, "strength": o.quant_strength, "swing": o.quant_swing, "window": o.quant_window,
                "random": o.quant_random, "ends": o.quant_ends, "keep_length": o.quant_keep_length,
            });
            if !o.quant_groove.is_empty() {
                p["groove"] = json!(o.quant_groove);
            }
            ("event.quantize", p)
        }
        "transpose" if o.transpose_in_key => ("event.transpose", json!({"steps": o.transpose})),
        "transpose" => ("event.transpose", json!({"semitones": o.transpose})),
        "change velocity" => ("event.change_velocity", json!({"add": o.velocity_add})),
        "change duration" => {
            let v = o.dur_value;
            let p = match o.dur_mode.as_str() {
                "set" => json!({"set": v.round().max(1.0) as i64}),
                "add" => json!({"add": v.round() as i64}),
                "scale" => json!({"scale": (v / 100.0).clamp(0.01, 100.0)}),
                _ => json!({"legato": v.round().max(0.0) as i64}),
            };
            ("event.change_duration", p)
        }
        "fix overlaps" => ("event.fix_overlaps", json!({})),
        "humanize" => ("event.humanize", json!({"timing": o.human_timing, "velocity": o.human_velocity})),
        "strum" => ("event.strum", json!({"ticks": o.strum_ticks, "direction": if o.strum_up { "up" } else { "down" }})),
        "reverse" => ("event.reverse_notes", json!({})),
        "invert pitches" => ("event.invert_pitch", json!({})),
        "flam" => ("event.flam", json!({"ticks": o.flam_ticks, "velocity": o.flam_velocity})),
        "half or double time" => ("event.time_scale", json!({"factor": if o.half_time { 2.0 } else { 0.5 }})),
        "select notes" => {
            let mut p = json!({
                "pitch_from": o.rule_pitch.0, "pitch_to": o.rule_pitch.1,
                "velocity_from": o.rule_velocity.0, "velocity_to": o.rule_velocity.1,
                "length_from": o.rule_length.0, "length_to": o.rule_length.1,
                "action": if o.rule_split { "split" } else { "select" },
            });
            if o.rule_channel != (1, 16) {
                p["channel_from"] = json!(o.rule_channel.0);
                p["channel_to"] = json!(o.rule_channel.1);
            }
            // Beats of the bar (1-based) as ticks from the bar line; the end beat's whole length.
            if let Some((a, b)) = o.rule_beats {
                let q = soundcraft_midi::PPQ as f64;
                p["in_bar_from"] = json!((((a.min(b) - 1.0) * q).round() as i64).max(0));
                p["in_bar_to"] = json!((((a.max(b)) * q).round() as i64 - 1).max(0));
            }
            ("event.select_split_notes", p)
        }
        _ => return None,
    })
}

/// Event › Quantize… and Change Duration… from a menu: the MIDI Operations window on that
/// operation, where its grid, strength and swing (or its kind of change) are chosen.
pub fn open_midi_op(app: &mut SoundApp, id: &str) -> bool {
    let op = match id {
        "event.quantize" => "quantize",
        "event.change_duration" => "change duration",
        "event.select_split_notes" => "select notes",
        _ => return false,
    };
    app.ops.midi_op = op.into();
    app.ui.show_midi_ops = true;
    true
}

fn midi_ops(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_midi_ops;
    if !open {
        return;
    }
    egui::Window::new("MIDI Operations").open(&mut open).default_size(vec2(360.0, 220.0)).show(ctx, |ui| {
        egui::ComboBox::from_label("Operation").selected_text(app.ops.midi_op.clone()).show_ui(ui, |ui| {
            for c in MIDI_OPS {
                ui.selectable_value(&mut app.ops.midi_op, c.to_string(), c);
            }
        });
        match app.ops.midi_op.as_str() {
            "select notes" => rule_form(ui, &mut app.ops),
            "quantize" => {
                egui::ComboBox::from_label("Grid").selected_text(app.ops.quant_grid.clone()).show_ui(ui, |ui| {
                    for g in QUANTIZE_GRIDS {
                        ui.selectable_value(&mut app.ops.quant_grid, g.to_string(), g);
                    }
                });
                groove_combo(app, ui, "Groove", |o| &mut o.quant_groove);
                ui.add(egui::Slider::new(&mut app.ops.quant_strength, 0.0..=100.0).text("Strength %"));
                ui.add_enabled(app.ops.quant_groove.is_empty(), egui::Slider::new(&mut app.ops.quant_swing, 0.0..=100.0).text("Swing %"));
                ui.add(egui::Slider::new(&mut app.ops.quant_window, 0.0..=100.0).text("Window %"))
                    .on_hover_text("Only notes within this much of half a grid step from their line move");
                ui.add(egui::Slider::new(&mut app.ops.quant_random, 0.0..=100.0).text("Random %"))
                    .on_hover_text("A random offset after quantizing, up to this much of half a grid step");
                ui.horizontal(|ui| {
                    ui.checkbox(&mut app.ops.quant_ends, "Note ends too");
                    ui.add_enabled(!app.ops.quant_ends, egui::Checkbox::new(&mut app.ops.quant_keep_length, "Keep lengths"));
                });
                if ui
                    .button("Groove from Selected Clip")
                    .on_hover_text("Take the timing of the selected MIDI clip as a groove, on this grid")
                    .clicked()
                {
                    let r = app.run("event.groove_from_clip", json!({"grid": app.ops.quant_grid}));
                    if let Ok(v) = &r
                        && let Some(name) = v.get("name").and_then(|n| n.as_str())
                    {
                        app.ops.quant_groove = name.to_string();
                    }
                    result_line(ui, &r);
                }
            }
            "transpose" => {
                let unit = if app.ops.transpose_in_key { "Scale steps" } else { "Semitones" };
                ui.add(egui::Slider::new(&mut app.ops.transpose, -48..=48).text(unit));
                ui.checkbox(&mut app.ops.transpose_in_key, "In the key")
                    .on_hover_text("Move by steps of the key on the Key ruler (else the band's key): C up two in C major is E");
            }
            "change velocity" => {
                ui.add(egui::Slider::new(&mut app.ops.velocity_add, -127..=127).text("Add"));
            }
            "change duration" => {
                egui::ComboBox::from_label("Change").selected_text(app.ops.dur_mode.clone()).show_ui(ui, |ui| {
                    for m in ["legato", "set", "add", "scale"] {
                        ui.selectable_value(&mut app.ops.dur_mode, m.to_string(), m);
                    }
                });
                let (label, range) = match app.ops.dur_mode.as_str() {
                    "legato" => ("Gap (ticks)", 0.0..=960.0),
                    "set" => ("Length (ticks)", 1.0..=15_360.0),
                    "add" => ("Ticks", -3_840.0..=3_840.0),
                    _ => ("Percent", 1.0..=1_000.0),
                };
                ui.add(egui::Slider::new(&mut app.ops.dur_value, range).text(label));
            }
            "humanize" => {
                ui.add(egui::Slider::new(&mut app.ops.human_timing, 0..=240).text("Timing ± ticks"));
                ui.add(egui::Slider::new(&mut app.ops.human_velocity, 0..=64).text("Velocity ±"));
            }
            "strum" => {
                ui.add(egui::Slider::new(&mut app.ops.strum_ticks, 0..=240).text("Ticks between notes"));
                ui.horizontal(|ui| {
                    ui.radio_value(&mut app.ops.strum_up, true, "Up (lowest first)");
                    ui.radio_value(&mut app.ops.strum_up, false, "Down");
                });
            }
            "flam" => {
                ui.add(egui::Slider::new(&mut app.ops.flam_ticks, 1..=240).text("Ticks before"));
                ui.add(egui::Slider::new(&mut app.ops.flam_velocity, 0.0..=200.0).text("Grace velocity %"));
            }
            "invert pitches" => {
                ui.label("Mirrors the pitches around the middle of their range.");
            }
            "half or double time" => {
                ui.horizontal(|ui| {
                    ui.radio_value(&mut app.ops.half_time, true, "Half time");
                    ui.radio_value(&mut app.ops.half_time, false, "Double time");
                });
            }
            _ => {}
        }
        // Select Notes picks from every note of the clip open in the MIDI Editor.
        let rule_clip = (app.ops.midi_op == "select notes" && !app.ops.rule_split && app.ui.show_midi_editor)
            .then(|| crate::midi_editor::target_clip(app))
            .flatten();
        let picked = if app.ops.midi_op == "select notes" { None } else { editor_selection(app) };
        let whom = if rule_clip.is_some() {
            "Selects the matching notes in the MIDI Editor."
        } else if picked.is_some() {
            "Applies to the notes selected in the MIDI Editor."
        } else {
            "Applies to the selected MIDI clips (or those under the selection)."
        };
        ui.label(egui::RichText::new(whom).small());
        if ui.button(egui::RichText::new("Apply").strong()).clicked() {
            let r = match midi_op_params(&app.ops) {
                Some((id, mut p)) if rule_clip.is_some() => {
                    p["clips"] = json!([rule_clip.map(|c| c.0)]);
                    let r = app.run(id, p);
                    if let Ok(v) = &r
                        && let Some(a) = v.get("indices").and_then(|x| x.as_array())
                    {
                        app.midi.selected = a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect();
                        app.midi.clip = rule_clip;
                    }
                    r
                }
                Some((id, mut p)) => {
                    if let Some((clip, sel)) = &picked {
                        p["clips"] = json!([clip.0]);
                        p["indices"] = json!(sel);
                    }
                    let r = app.run(id, p);
                    // The selection stays on its notes through the re-sort.
                    if picked.is_some()
                        && let Ok(v) = &r
                        && let Some(a) = v.get("indices").and_then(|x| x.as_array())
                    {
                        app.midi.selected = a.iter().filter_map(|x| x.as_u64()).map(|x| x as usize).collect();
                    }
                    r
                }
                None if app.ops.midi_op == "flatten performance" => app.run("event.flatten_performance", json!({})),
                None => app.run("event.restore_performance", json!({})),
            };
            result_line(ui, &r);
        }
    });
    app.ui.show_midi_ops = open;
}

/// The grooves to pick from (built in and taken from clips), `""` being none.
pub fn groove_names(app: &mut SoundApp) -> Vec<String> {
    app.run("event.grooves", json!({}))
        .ok()
        .and_then(|v| {
            v.get("grooves").and_then(|g| g.as_array()).map(|a| a.iter().filter_map(|g| g.get("name")?.as_str().map(str::to_string)).collect())
        })
        .unwrap_or_default()
}

/// A Groove picker over `field` of the window's state: None (the grid) or a groove's name.
fn groove_combo(app: &mut SoundApp, ui: &mut egui::Ui, label: &str, field: fn(&mut OpsState) -> &mut String) {
    let names = groove_names(app);
    let cur = field(&mut app.ops).clone();
    egui::ComboBox::from_label(label).selected_text(if cur.is_empty() { "None (the grid)".to_string() } else { cur.clone() }).show_ui(ui, |ui| {
        if ui.selectable_label(cur.is_empty(), "None (the grid)").clicked() {
            field(&mut app.ops).clear();
        }
        for n in names {
            if ui.selectable_label(cur == n, n.as_str()).clicked() {
                *field(&mut app.ops) = n;
            }
        }
    });
}

/// Select Notes' form: the ranges a note must fall in, and select or split.
fn rule_form(ui: &mut egui::Ui, o: &mut OpsState) {
    let range = |ui: &mut egui::Ui, label: &str, v: &mut (i64, i64), lo: i64, hi: i64| {
        ui.horizontal(|ui| {
            ui.label(label);
            ui.add(egui::DragValue::new(&mut v.0).range(lo..=hi));
            ui.label("to");
            ui.add(egui::DragValue::new(&mut v.1).range(lo..=hi));
        });
    };
    ui.horizontal(|ui| {
        ui.label("Pitch");
        ui.add(egui::DragValue::new(&mut o.rule_pitch.0).range(0..=127).custom_formatter(|v, _| soundcraft_midi::note_name(v as u8)));
        ui.label("to");
        ui.add(egui::DragValue::new(&mut o.rule_pitch.1).range(0..=127).custom_formatter(|v, _| soundcraft_midi::note_name(v as u8)));
    });
    range(ui, "Velocity", &mut o.rule_velocity, 1, 127);
    range(ui, "Channel", &mut o.rule_channel, 1, 16);
    range(ui, "Length (ticks)", &mut o.rule_length, 1, 1_000_000);
    let mut beats = o.rule_beats.is_some();
    ui.horizontal(|ui| {
        if ui.checkbox(&mut beats, "On beats").on_hover_text("Only notes starting in these beats of their bar (1 is the downbeat)").changed() {
            o.rule_beats = beats.then_some((1.0, 1.0));
        }
        if let Some((a, b)) = &mut o.rule_beats {
            ui.add(egui::DragValue::new(a).range(1.0..=64.0).speed(0.25));
            ui.label("to");
            ui.add(egui::DragValue::new(b).range(1.0..=64.0).speed(0.25));
        }
    });
    ui.horizontal(|ui| {
        ui.radio_value(&mut o.rule_split, false, "Select them");
        ui.radio_value(&mut o.rule_split, true, "Move them to a new track");
    });
}

fn rtp(app: &mut SoundApp, ctx: &egui::Context) {
    let mut open = app.ui.show_rtp;
    if !open {
        return;
    }
    // The selected MIDI tracks' values (the first one's), read when the selection changes.
    let (sel, first) = {
        let s = app.engine.session();
        let sel: Vec<u64> = s.tracks.iter().filter(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi()).map(|t| t.id.0).collect();
        let first = sel.first().map(|id| {
            let v = |name: &str, d: f64| s.edit.value(&format!("rtp.{id}.{name}"), d);
            (v("velocity", 0.0) as i64, v("transpose", 0.0) as i64, v("duration", 100.0), v("delay", 0.0) as i64)
        });
        (sel, first)
    };
    if sel != app.ops.rtp_tracks {
        (app.ops.rtp_velocity, app.ops.rtp_transpose, app.ops.rtp_duration, app.ops.rtp_delay) = first.unwrap_or((0, 0, 100.0, 0));
        app.ops.rtp_tracks = sel;
    }
    egui::Window::new("MIDI Real-Time Properties").open(&mut open).default_size(vec2(340.0, 200.0)).show(ctx, |ui| {
        ui.label("Non-destructive, applied at playback to the selected MIDI tracks.");
        // A frozen track's render played its notes with the properties of the time: greyed, saying so.
        let frozen = {
            let s = app.engine.session();
            s.tracks.iter().find(|t| s.edit.selected_tracks.contains(&t.id) && t.kind.is_midi() && t.frozen_audio().is_some()).map(|t| t.name.clone())
        };
        let why = frozen.map(|n| format!("`{n}` is frozen, and its MIDI Real-Time Properties shape its notes. Unfreeze it to change them"));
        // Only the property moved is written (the others may differ between the selected tracks);
        // a drag is one undo step.
        let mut changed: Option<(&str, Value)> = None;
        let sliders = ui.add_enabled_ui(why.is_none(), |ui| {
            if ui.add(egui::Slider::new(&mut app.ops.rtp_velocity, -127..=127).text("Velocity +")).changed() {
                changed = Some(("velocity", json!(app.ops.rtp_velocity)));
            }
            if ui.add(egui::Slider::new(&mut app.ops.rtp_transpose, -48..=48).text("Transpose")).changed() {
                changed = Some(("transpose", json!(app.ops.rtp_transpose)));
            }
            if ui.add(egui::Slider::new(&mut app.ops.rtp_duration, 1.0..=400.0).text("Duration %")).changed() {
                changed = Some(("duration", json!(app.ops.rtp_duration)));
            }
            if ui.add(egui::Slider::new(&mut app.ops.rtp_delay, -960..=960).text("Delay (ticks)")).changed() {
                changed = Some(("delay", json!(app.ops.rtp_delay)));
            }
            // Playback quantize: the notes play on the grid and stay where they are.
            let before = app.ops.rtp_quantize.clone();
            egui::ComboBox::from_label("Quantize").selected_text(before.clone()).show_ui(ui, |ui| {
                for g in std::iter::once("off").chain(QUANTIZE_GRIDS) {
                    ui.selectable_value(&mut app.ops.rtp_quantize, g.to_string(), g);
                }
            });
            if app.ops.rtp_quantize != before {
                let ticks = soundcraft_engine::cmd::event::note_value_ticks(&app.ops.rtp_quantize).unwrap_or(0);
                changed = Some(("quantize", json!(ticks)));
            }
            if ui
                .add_enabled(app.ops.rtp_quantize != "off", egui::Slider::new(&mut app.ops.rtp_q_strength, 0.0..=100.0).text("Quantize strength %"))
                .changed()
            {
                changed = Some(("q_strength", json!(app.ops.rtp_q_strength)));
            }
        });
        if let Some(why) = &why {
            sliders.response.on_disabled_hover_text(why.as_str());
        }
        if let Some((name, v)) = changed {
            let mut p = serde_json::Map::new();
            p.insert(name.into(), v);
            if let Err(e) = app.engine.execute_merged("event.midi_rtp", &Value::Object(p), &format!("rtp.{name}")) {
                app.ui.status = e.to_string();
            }
        }
        ui.horizontal(|ui| {
            let greyed = why.clone().unwrap_or_default();
            if ui.add_enabled(why.is_none(), egui::Button::new("Write to Notes")).on_disabled_hover_text(greyed.as_str()).clicked() {
                let _ = app.run("track.write_midi_rtp", json!({}));
            }
            if ui.add_enabled(why.is_none(), egui::Button::new("Clear")).on_disabled_hover_text(greyed.as_str()).clicked() {
                let _ = app.run("event.midi_rtp", json!({"clear": true}));
                app.ops.rtp_velocity = 0;
                app.ops.rtp_transpose = 0;
                app.ops.rtp_duration = 100.0;
                app.ops.rtp_delay = 0;
                app.ops.rtp_quantize = "off".into();
                app.ops.rtp_q_strength = 100.0;
            }
        });
    });
    app.ui.show_rtp = open;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stretch_uses_start_over_end_and_scale_uses_end_over_start() {
        let (id, p) = tempo_ops_apply("stretch", 120.0, 60.0);
        assert_eq!(id, "event.tempo_stretch");
        assert!((p["factor"].as_f64().unwrap() - 2.0).abs() < 1e-9, "{p}");
        let (_, p) = tempo_ops_apply("stretch", 120.0, 120.0);
        assert!((p["factor"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{p}");
        let (id, p) = tempo_ops_apply("scale", 120.0, 60.0);
        assert_eq!(id, "event.tempo_scale");
        assert!((p["factor"].as_f64().unwrap() - 0.5).abs() < 1e-9, "{p}");
        let (_, p) = tempo_ops_apply("stretch", f64::NAN, f64::INFINITY);
        assert!((p["factor"].as_f64().unwrap() - 1.0).abs() < 1e-9, "{p}");
        let (id, p) = tempo_ops_apply("linear", 100.0, 140.0);
        assert_eq!(id, "event.tempo_linear");
        assert_eq!(p["end_bpm"], 140.0);
    }
}

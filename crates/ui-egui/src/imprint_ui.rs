//! Imprint's part of the module drawer: what the capture is doing (waiting for Play, running,
//! a take ready), the captured impulse response drawn as its decay, and Save IR…
//! (`ui.imprint.save`), which writes the response as a 32-bit float WAV.
//!
//! The response is worked out here, on the UI thread, from the take the module shares with its
//! editor (`soundcraft_dsp::plugins::imprint`): once per take, kept until the next one.

use crate::SoundApp;
use crate::module_drawer::menu_field;
use crate::theme::{Tokens, bold, regular};
use egui::{Align2, Color32, CornerRadius, Id, Rect, Stroke, StrokeKind, Ui, pos2, vec2};
use serde_json::{Value, json};
use soundcraft_dsp::plugins::imprint::{CapturedIr, POST_S, PRE_ROLL_S, ROLES, TAKE_READY, TAKE_RECORDING, Take, take_handle};
use soundcraft_mix::StripMeter;
use soundcraft_model::{Insert, ModuleId, TrackId};
use std::sync::Arc;

/// The file name Save IR… offers.
pub const IR_FILE: &str = "Imprint IR.wav";
/// The quietest level the decay picture shows, in dB under the response's peak.
const FLOOR_DB: f32 = -96.0;

/// A take's response, made once.
struct Made {
    /// The take's address: only compared, to tell one take from another.
    take: usize,
    serial: u32,
    ir: Result<CapturedIr, String>,
}

/// The takes of the Imprint modules seen playing, newest last: `((track, module), take)`. The
/// strip meters (and the way to a module's take with them) are only there while the audio runs,
/// so the take is kept here to draw and save after Stop. A handful at most.
static TAKES: std::sync::Mutex<Vec<((u64, u32), Arc<Take>)>> = std::sync::Mutex::new(Vec::new());
const KEPT_TAKES: usize = 6;

/// Notes the takes of a strip's Imprint modules (the Mix window calls it for every strip it
/// draws while the audio runs).
pub(crate) fn remember(track: TrackId, meters: &StripMeter) {
    for (module, a) in &meters.analysis {
        let Some(handle) = take_handle(a) else { continue };
        let (key, take) = ((track.0, module.0), handle.take());
        let mut kept = TAKES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if kept.last().is_some_and(|(k, t)| *k == key && Arc::ptr_eq(t, &take)) {
            continue;
        }
        kept.retain(|(k, _)| *k != key);
        kept.push((key, take));
        let extra = kept.len().saturating_sub(KEPT_TAKES);
        kept.drain(..extra);
    }
}

/// The take of the Imprint module `module` of `track`: the playing module's, else the one last
/// seen playing.
fn take_of(track: TrackId, meters: Option<&StripMeter>, module: ModuleId) -> Option<Arc<Take>> {
    if let Some(m) = meters {
        remember(track, m);
    }
    let kept = TAKES.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    kept.iter().find(|(k, _)| *k == (track.0, module.0)).map(|(_, t)| Arc::clone(t))
}

/// The response of `take`, made the first time it is asked for and kept until the take changes.
fn response(ui: &Ui, key: Id, take: &Arc<Take>) -> Arc<Made> {
    let (ptr, serial) = (Arc::as_ptr(take) as usize, take.serial());
    if let Some(m) = ui.data(|d| d.get_temp::<Arc<Made>>(key))
        && m.take == ptr
        && m.serial == serial
    {
        return m;
    }
    let made = Arc::new(Made { take: ptr, serial, ir: take.impulse_response() });
    ui.data_mut(|d| d.insert_temp(key, Arc::clone(&made)));
    made
}

fn param(ins: &Insert, id: &str) -> f32 {
    let info = soundcraft_dsp::plugin_info(&ins.plugin).and_then(|i| i.param(id));
    info.map_or(0.0, |p| p.clamp(ins.params.get(id).copied().unwrap_or(p.default)))
}

/// What a module with this role does, in a line.
fn role_line(role: usize, seconds: f32) -> String {
    match role {
        0 => "Plays the sweep when you press Play.".to_string(),
        1 => format!("Press Play to capture ({seconds:.1} s). A Send module plays the sweep."),
        _ => format!("Press Play to capture ({seconds:.1} s): sweep out, response in."),
    }
}

/// Imprint's rows under the drawer's graph: the status line, the decay picture and Save IR….
/// Returns the y under them.
pub(crate) fn drawer_rows(app: &mut SoundApp, ui: &mut Ui, track: TrackId, ins: &Insert, inner: Rect, y: f32, meters: Option<&StripMeter>) -> f32 {
    let t = Tokens::current();
    let p = ui.painter().clone();
    let role = (param(ins, "role").round() as usize).min(ROLES.len() - 1);
    let seconds = PRE_ROLL_S + param(ins, "length") + param(ins, "tail") + POST_S;
    let take = take_of(track, meters, ins.id);
    let state = take.as_ref().map_or(0, |k| k.state());
    let live = meters.and_then(|m| m.analysis.iter().find(|(m, _)| *m == ins.id)).map(|(_, a)| &a.live);
    let running = live.is_some_and(|l| l.get(0) == 1.0);
    let progress = live.map_or(0.0, |l| l.get(1)).clamp(0.0, 1.0);
    let peak_db = live.map_or(f32::NEG_INFINITY, |l| 20.0 * l.get(2).max(1.0e-9).log10());

    // The status line.
    let made =
        (state == TAKE_READY && !running).then(|| take.as_ref().map(|k| response(ui, Id::new(("imprint-ir", track.0, ins.id.0)), k))).flatten();
    let (status, colour) = match (&made, running) {
        (_, true) if role == 0 => (format!("Playing the sweep… {:.0} %", progress * 100.0), t.accent),
        (_, true) => (format!("Capturing… {:.0} %", progress * 100.0), t.accent),
        (Some(m), _) => match &m.ir {
            Ok(ir) => {
                let noise = ir.noise_db.map_or(String::new(), |n| format!(" · noise {n:.0} dB"));
                let width = if ir.channels.len() > 1 { "stereo" } else { "mono" };
                (format!("Take ready · {:.2} s · {width}{noise}", ir.frames() as f32 / ir.rate.max(1.0)), t.text)
            }
            Err(why) => (why.clone(), t.text_dim),
        },
        _ => (role_line(role, seconds), t.text_dim),
    };
    let line = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 30.0));
    let galley = p.layout(status, regular(10.0), colour, inner.width());
    p.galley(line.min, galley, colour);
    let mut y = line.max.y + 4.0;

    // The decay picture: the take's response in dB, or the capture's progress and input level.
    let pic = Rect::from_min_size(pos2(inner.min.x, y), vec2(inner.width(), 84.0));
    p.rect_filled(pic, CornerRadius::same(5), t.graph_bg);
    p.rect_stroke(pic, CornerRadius::same(5), Stroke::new(1.0, t.module_tile_border), StrokeKind::Inside);
    let area = pic.shrink(6.0);
    if running {
        let bar = Rect::from_min_size(pos2(area.min.x, area.center().y - 3.0), vec2(area.width(), 6.0));
        p.rect_filled(bar, CornerRadius::same(3), t.module_tile_border);
        p.rect_filled(bar.with_max_x(bar.min.x + bar.width() * progress), CornerRadius::same(3), t.accent);
        if role != 0 {
            let hot = peak_db >= -0.1;
            let text = if hot { "Input clipped: turn the level down and capture again".to_string() } else { format!("Input peak {peak_db:.1} dB") };
            p.text(
                pos2(area.center().x, bar.max.y + 14.0),
                Align2::CENTER_CENTER,
                text,
                regular(9.5),
                if hot { Color32::from_rgb(230, 80, 70) } else { t.text_dim },
            );
        }
        ui.ctx().request_repaint();
    } else if let Some(Ok(ir)) = made.as_ref().map(|m| &m.ir) {
        draw_decay(&p, area, ir, t.accent, t.module_tile_border);
        p.text(area.right_top(), Align2::RIGHT_TOP, "0 dB", regular(8.0), t.text_dim);
        p.text(area.left_bottom(), Align2::LEFT_BOTTOM, format!("{FLOOR_DB:.0}"), regular(8.0), t.text_dim);
        p.text(area.right_bottom(), Align2::RIGHT_BOTTOM, format!("{:.2} s", ir.frames() as f32 / ir.rate.max(1.0)), regular(8.0), t.text_dim);
    } else {
        p.text(area.center(), Align2::CENTER_CENTER, "No take yet", regular(10.0), t.text_dim);
    }
    y = pic.max.y + 8.0;

    // Save IR…
    if role != 0 {
        p.text(pos2(inner.min.x, y + 11.0), Align2::LEFT_CENTER, "IR", bold(9.5), t.text_dim);
        let field = Rect::from_min_max(pos2(inner.min.x + 22.0, y), pos2(inner.max.x, y + 22.0));
        let ready = matches!(made.as_ref().map(|m| &m.ir), Some(Ok(_)));
        let can_save = ready && app.services.pick_save.is_some();
        let resp = ui.add_enabled_ui(can_save, |ui| menu_field(ui, field, "Save IR…", "imprint-save", false)).inner;
        let hover = if ready {
            "Saves the captured impulse response as a 32-bit float WAV, for an IR loader or a convolution reverb."
        } else {
            "Capture a take first: press Play and let it run to the end."
        };
        if resp.on_hover_text(hover).on_disabled_hover_text(hover).clicked() {
            let r = app.run("ui.imprint.save", json!({"track": track.0, "module": ins.id.0}));
            if let Err(e) = r {
                app.ui.status = e.to_string();
            }
        }
        y += 22.0 + 8.0;
    }
    if state == TAKE_RECORDING || running {
        ui.ctx().request_repaint();
    }
    y
}

/// The response's level over time: per pixel column the loudest sample of any channel, in dB
/// under the peak.
fn draw_decay(p: &egui::Painter, area: Rect, ir: &CapturedIr, colour: Color32, grid: Color32) {
    let frames = ir.frames();
    let peak = ir.channels.iter().flatten().fold(0.0f32, |m, x| m.max(x.abs()));
    if frames == 0 || peak <= 0.0 {
        return;
    }
    for db in [-24.0f32, -48.0, -72.0] {
        let gy = area.min.y + area.height() * (db / FLOOR_DB);
        p.hline(area.x_range(), gy, Stroke::new(1.0, grid.gamma_multiply(0.6)));
    }
    let cols = area.width().max(1.0) as usize;
    for col in 0..cols {
        let (a, b) = (col * frames / cols, ((col + 1) * frames / cols).max(col * frames / cols + 1).min(frames));
        let loudest = ir.channels.iter().filter_map(|c| c.get(a..b)).flatten().fold(0.0f32, |m, x| m.max(x.abs()));
        let db = (20.0 * (loudest / peak).max(1.0e-9).log10()).max(FLOOR_DB);
        let x = area.min.x + col as f32 + 0.5;
        let top = area.min.y + area.height() * (db / FLOOR_DB);
        p.vline(x, top..=area.max.y, Stroke::new(1.0, colour.gamma_multiply(0.85)));
    }
}

/// `ui.imprint.save {track, module, path?}`: the impulse response of the module's take, written
/// as a 32-bit float WAV to `path`, else to the file the save dialog picks → {path, frames,
/// channels, sample_rate, noise_db}. Cancelled: {path: null}. Needs a finished take: one captured
/// while the Mix window showed the module's strip.
pub(crate) fn save_command(app: &mut SoundApp, p: &Value) -> Result<Value, String> {
    let (track, module) = crate::menus::named_module(app, p)?;
    let plugin = app.engine.session().track(track).and_then(|t| t.mixer.module(module)).map(|i| i.plugin.clone()).unwrap_or_default();
    if plugin != "imprint" {
        return Err(format!("`{plugin}` captures no impulse response: this is Imprint's command"));
    }
    let meters = app.player.as_ref().map(|pl| pl.meters());
    let take = take_of(track, meters.as_ref().and_then(|m| m.tracks.get(&track)), module)
        .ok_or("nothing captured yet: press Play with the Mix window open and let the capture run to its end")?;
    let ir = take.impulse_response()?;
    let path = match p.get("path").and_then(Value::as_str) {
        Some(path) => Some(path.to_string()),
        None => app.services.pick_save.as_ref().and_then(|pick| pick("ui.imprint.save", IR_FILE)),
    };
    let Some(mut path) = path else { return Ok(json!({"path": null})) };
    if std::path::Path::new(&path).extension().is_none() {
        path.push_str(".wav");
    }
    soundcraft_engine::file_access::check_write(&path)?;
    let (frames, channels) = (ir.frames(), ir.channels.len());
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: ir.rate.round() as u32, channels: ir.channels };
    let opts = soundcraft_audio_io::EncodeOptions {
        format: soundcraft_audio_io::FileFormat::Wav,
        bit_depth: soundcraft_audio_io::BitDepth::Float32,
        dither: false,
        bwf: None,
        bitrate_kbps: 0,
    };
    let bytes = soundcraft_audio_io::encode(&buf, &opts).map_err(|e| format!("Could not make the IR file: {e}"))?;
    std::fs::write(&path, bytes).map_err(|e| format!("Could not save the IR to {path}: {e}"))?;
    app.ui.status = format!("Impulse response saved to {path}");
    Ok(json!({"path": path, "frames": frames, "channels": channels, "sample_rate": buf.sample_rate, "noise_db": ir.noise_db}))
}

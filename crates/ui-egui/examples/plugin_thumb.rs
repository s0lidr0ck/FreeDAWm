//! Renders the plugin browser's card pictures of the built-in and FireStation plugins offscreen
//! and saves them as PNGs (run by `cargo xtask browser-thumbs`, which also writes the table that
//! builds them into the app).
//!
//! `cargo run -p soundcraft-ui-egui --example plugin_thumb -- <out dir> [plugin id…]`
//! `cargo run -p soundcraft-ui-egui --example plugin_thumb -- --list-presets`
//!
//! Each plugin is opened the way the owner sees it, dark theme: a FireStation engine's wide
//! (VST-faithful) editor, a built-in effect's drawer, an instrument's parameter window. It is
//! shown in a characteristic setting ([`look`]: a factory preset and/or parameters, set with the
//! commands the UI uses) and, for effects, with its meters and displays fed by a short drum and
//! chord loop run through the plugin offline ([`feed`]). The picture is cropped to the editor,
//! cut to its top part when taller than a card shows, scaled to at most `thumbs::MAX_WIDTH` px
//! wide and stored as an RGB PNG.

use egui::{Rect, vec2};
use serde_json::{Map, Value, json};
use soundcraft_mix::{ModuleMeter, SlotKey, StripMeter};
use soundcraft_model::{ModuleId, TrackId};
use soundcraft_playback::MeterSnapshot;
use soundcraft_ui_egui::browser::thumbs::MAX_WIDTH;
use soundcraft_ui_egui::module_graphs::{GraphKind, graph_kind};
use soundcraft_ui_egui::{Services, SoundApp};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Pixels per point of the render.
const PPP: f32 = 2.0;
/// The tallest a picture is, as a share of its width (cards are about 0.7).
const MAX_ASPECT: f32 = 0.75;
/// The narrow drawer's name and preset rows, in points, left out of its picture.
const DRAWER_TOP: f32 = 72.0;
/// The drawer's name rows (the module's short and full names), in points from its top.
const DRAWER_NAME: (f32, f32) = (6.0, 50.0);
/// The plain in/out meter display of a "Level" drawer, left out when a hero knob follows.
const LEVEL_GRAPH: f32 = 104.0;
/// The test signal's rate.
const SR: f32 = 48_000.0;

/// A plugin's characteristic setting: a factory preset (FireStation engines) and parameters.
struct Look {
    preset: Option<&'static str>,
    params: &'static [(&'static str, f32)],
}

const fn preset(name: &'static str) -> Look {
    Look { preset: Some(name), params: &[] }
}

const fn params(params: &'static [(&'static str, f32)]) -> Look {
    Look { preset: None, params }
}

/// The setting each plugin is pictured in.
fn look(id: &str) -> Look {
    match id {
        "eq_1band" => params(&[("freq", 2500.0), ("gain", 9.0), ("q", 1.4)]),
        "eq_7band" => params(&[
            ("hpf_on", 1.0),
            ("hpf_freq", 70.0),
            ("low_shelf_gain", 4.0),
            ("low_mid_gain", -3.0),
            ("mid_freq", 800.0),
            ("mid_gain", -5.0),
            ("high_mid_gain", 3.0),
            ("high_shelf_gain", 5.0),
        ]),
        "compressor" => params(&[("threshold", -30.0), ("ratio", 6.0), ("makeup", 6.0)]),
        "expander_gate" => params(&[("threshold", -30.0), ("range", 40.0)]),
        "de_esser" => params(&[("freq", 7500.0), ("threshold", -42.0), ("range", 14.0)]),
        "maximizer" => params(&[("threshold", -9.0), ("ceiling", -1.0)]),
        "rez" => params(&[
            ("depth", 85.0),
            ("selectivity", 30.0),
            ("low", 90.0),
            ("b1_gain", -14.0),
            ("b2_gain", 5.0),
            ("b3_freq", 3200.0),
            ("b3_gain", 9.0),
            ("b3_q", 1.6),
        ]),
        "channel_strip" => params(&[
            ("comp_on", 1.0),
            ("comp_threshold", -30.0),
            ("comp_ratio", 4.0),
            ("gate_on", 1.0),
            ("hpf_on", 1.0),
            ("low_gain", 3.0),
            ("mid_gain", -4.0),
            ("high_gain", 4.0),
        ]),
        "room_reverb" => params(&[("mix", 35.0), ("decay", 0.8), ("size", 35.0)]),
        "plate_reverb" => params(&[("mix", 40.0), ("decay", 3.6), ("size", 75.0)]),
        "mod_delay" => params(&[("time", 375.0), ("feedback", 45.0), ("mod_depth", 20.0)]),
        "chorus" => params(&[("rate", 1.3), ("depth", 65.0)]),
        "flanger" => params(&[("rate", 0.4), ("feedback", 70.0)]),
        "phaser" => params(&[("rate", 0.9), ("stages", 3.0), ("feedback", 60.0)]),
        "saturator" => params(&[("drive", 16.0), ("bias", 30.0)]),
        "lofi" => params(&[("bits", 6.0), ("sample_rate", 8000.0), ("noise", 20.0)]),
        "rectifier" => params(&[("mode", 0.0), ("mix", 70.0)]),
        "pitch_shifter" => params(&[("semitones", 7.0), ("mix", 60.0)]),
        "time_shift" => params(&[("delay_ms", 12.5)]),
        "gain" => params(&[("gain", 4.5)]),
        "trim" => params(&[("gain", -6.0), ("invert", 1.0)]),
        "dc_offset_removal" => params(&[("cutoff", 12.0)]),
        "signal_generator" => params(&[("waveform", 2.0), ("freq", 220.0), ("level", -12.0)]),
        "dither" => params(&[("bits", 0.0), ("noise_shaping", 1.0)]),
        "subtractive_synth" => {
            params(&[("osc1_wave", 1.0), ("osc2_semitones", 12.0), ("cutoff", 900.0), ("resonance", 55.0), ("filter_env_amount", 60.0)])
        }
        "drum_synth" => params(&[("kick_tune", -3.0), ("kick_decay", 650.0), ("snare_snappy", 80.0), ("open_hat_decay", 900.0)]),
        "sampler" => params(&[("transpose", 12.0), ("attack", 30.0), ("release", 600.0), ("loop_mode", 1.0)]),
        "fs_sat" => preset("Tube Warmth"),
        "fs_comp" => preset("Punchy VCA"),
        "fs_tape" => preset("Cassette Deck"),
        "fs_twine" => preset("Loud Master"),
        // Low shelf +4 dB at 100 Hz, a -5 dB bell at 800 Hz, a +3 dB high shelf at 8 kHz and a
        // low cut (frequencies 20 Hz-20 kHz and gains -30..30 dB, normalised).
        "fs_cleanq" => params(&[
            ("b01_on", 2.0),
            ("b01_type", 3.0),
            ("b01_freq", 0.12),
            ("b01_slope", 3.0),
            ("b02_on", 2.0),
            ("b02_type", 1.0),
            ("b02_freq", 0.233),
            ("b02_gain", 0.567),
            ("b03_on", 2.0),
            ("b03_type", 0.0),
            ("b03_freq", 0.534),
            ("b03_gain", 0.417),
            ("b03_q", 0.55),
            ("b04_on", 2.0),
            ("b04_type", 0.0),
            ("b04_freq", 0.72),
            ("b04_gain", 0.56),
            ("b05_on", 2.0),
            ("b05_type", 2.0),
            ("b05_freq", 0.867),
            ("b05_gain", 0.55),
        ]),
        "fs_vint" => preset("Pultec Low End"),
        "fs_pitch" => preset("Octave Up"),
        "fs_mod" => preset("Uni-Vibe"),
        "fs_dist" => preset("Klon Noon"),
        "fs_transient" => preset("Punch"),
        "fs_filter" => preset("Cry Baby Pedal"),
        "fs_delay" => preset("Dotted Eighth"),
        "fs_reverb" => preset("Big Hall"),
        "fs_swale" => preset("Slow Gear"),
        "fs_chamber" => preset("Hollywood Seven"),
        "fs_amp" => preset("AC30 Top Boost Chime"),
        "fs_shale" => preset("Snare Top - Start Here"),
        "fs_levelup" => preset("Gentle Rider"),
        _ => params(&[]),
    }
}

/// The parameters `look` sets on `info`'s plugin: its factory preset's, then its own.
fn look_params(info: &soundcraft_dsp::PluginInfo) -> Result<Map<String, Value>, String> {
    let l = look(info.id);
    let mut out = Map::new();
    if let Some(name) = l.preset {
        let json = soundcraft_ui_egui::fs_presets::factory_groups(info)
            .iter()
            .flat_map(|(_, list)| list.iter())
            .find(|(n, _)| *n == name)
            .map(|(_, j)| *j)
            .ok_or_else(|| format!("no factory preset `{name}`"))?;
        let (_, p) = soundcraft_ui_egui::fs_presets::factory_recall(info, name, json).ok_or_else(|| format!("preset `{name}` unreadable"))?;
        out.extend(p);
    }
    for (k, v) in l.params {
        if !info.params.iter().any(|p| p.id == *k) {
            return Err(format!("no parameter `{k}`"));
        }
        out.insert((*k).to_string(), json!(v));
    }
    Ok(out)
}

/// A drum and chord loop, stereo: a kick on every beat at 120 bpm, a noise snare on two and four,
/// and a saw chord under it, peaking about -6 dBFS. Ends just after a kick.
fn test_signal() -> [Vec<f32>; 2] {
    let n = (SR * 1.53) as usize;
    let mut seed = 0x2545_f491u32;
    let mut noise = move || {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        (seed >> 8) as f32 / (1u32 << 24) as f32 * 2.0 - 1.0
    };
    let chord = [110.0f32, 164.8, 220.0, 277.2];
    let mut out = [vec![0.0; n], vec![0.0; n]];
    for i in 0..n {
        let t = i as f32 / SR;
        let beat = t % 0.5;
        let kick = (std::f32::consts::TAU * (50.0 * beat + 60.0 * (1.0 - (-beat * 30.0).exp()) / 30.0)).sin() * (-beat * 9.0).exp() * 0.5;
        let snare = if (t % 1.0) >= 0.5 { noise() * (-beat * 25.0).exp() * 0.22 } else { 0.0 };
        for (c, ch) in out.iter_mut().enumerate() {
            let detune = if c == 0 { 1.0 } else { 1.003 };
            let pad: f32 = chord.iter().map(|f| ((t * f * detune).fract() * 2.0 - 1.0) * 0.045).sum();
            ch[i] = kick + snare + pad;
        }
    }
    out
}

/// The meters of `module` with `params` set, after the test signal ran through the plugin.
fn feed(plugin: &str, params: &Map<String, Value>, module: ModuleId) -> Option<StripMeter> {
    let mut p = soundcraft_dsp::create(plugin)?;
    const BLOCK: usize = 480;
    p.prepare(SR, BLOCK, 2);
    for (k, v) in params {
        if let Some(v) = v.as_f64() {
            p.set_param(k, v as f32);
        }
    }
    let input = test_signal();
    let mut m = ModuleMeter { module, ..ModuleMeter::default() };
    let mut start = 0;
    while start < input[0].len() {
        let frames = BLOCK.min(input[0].len() - start);
        let mut io: Vec<Vec<f32>> = input.iter().map(|ch| ch[start..start + frames].to_vec()).collect();
        m.in_peak = io.iter().flat_map(|c| c.iter()).fold(0.0f32, |a, x| a.max(x.abs()));
        p.process(&mut io, frames);
        m.out_peak = io.iter().flat_map(|c| c.iter()).fold(0.0f32, |a, x| a.max(x.abs())).min(4.0);
        start += frames;
    }
    m.gr_db = p.gain_reduction_db().max(0.0);
    p.readouts(&mut m.readouts);
    for r in m.readouts.iter_mut().filter(|r| r.is_infinite()) {
        *r = f32::NAN;
    }
    let analysis = p.analysis().map(|a| vec![(module, a)]).unwrap_or_default();
    Some(StripMeter {
        peak: [m.out_peak; 2],
        peaks: vec![m.out_peak; 2],
        gain_reduction_db: m.gr_db,
        modules: vec![m],
        analysis,
        ..StripMeter::default()
    })
}

/// What to crop: the wide editor, the narrow drawer or the parameter window.
enum Target {
    Module(TrackId, ModuleId),
    Window(egui::Id),
}

fn exec(app: &mut SoundApp, cmd: &str, p: Value) -> Result<(), String> {
    app.engine.execute(cmd, &p).map(|_| ()).map_err(|e| format!("{cmd}: {e}"))
}

/// The app with `info`'s plugin open in its look, and where it is.
fn open(info: &soundcraft_dsp::PluginInfo) -> Result<(SoundApp, Target), String> {
    let mut app = SoundApp::new(soundcraft_engine::Engine::default(), None, Services::default());
    app.run("ui.set", json!({"theme": "dark"}))?;
    app.ui.wide_editors = true;
    app.ui.slim_editors = false;
    let params = look_params(info)?;
    if info.category == soundcraft_dsp::Category::Instrument {
        exec(&mut app, "track.new", json!({"kind": "instrument", "name": "Inst 1"}))?;
        exec(&mut app, "mix.instrument", json!({"track": "Inst 1", "plugin": info.id}))?;
        let t = app.engine.session().track_by_name("Inst 1").ok_or("no track")?.id;
        for (k, v) in params {
            exec(&mut app, "mix.instrument_param", json!({"track": t.0, "param": k, "value": v}))?;
        }
        app.ui.plugin_windows.push((t, SlotKey::Instrument));
        return Ok((app, Target::Window(egui::Id::new(("plugin", t.0, SlotKey::Instrument)))));
    }
    exec(&mut app, "track.new", json!({"name": "Audio 1"}))?;
    exec(&mut app, "mix.module_add", json!({"track": "Audio 1", "plugin": info.id}))?;
    let t = app.engine.session().track_by_name("Audio 1").ok_or("no track")?;
    let (t, m) = (t.id, t.mixer.inserts.iter().rev().find(|i| i.plugin == info.id).ok_or("module not added")?.id);
    if !params.is_empty() {
        let preset = look(info.id).preset.unwrap_or("<factory default>");
        app.run("mix.module_params", json!({"track": t.0, "module": m.0, "params": params, "preset": preset}))?;
    }
    if let Some(strip) = feed(info.id, &params, m) {
        app.preview_meters = Some(MeterSnapshot { tracks: [(t, strip)].into_iter().collect(), main: StripMeter::default() });
    }
    app.ui.drawers.insert(t, m);
    app.run("window.mix", json!({}))?;
    Ok((app, Target::Module(t, m)))
}

/// Renders `info`'s picture into `dir/<id>.png`.
fn thumb(info: &'static soundcraft_dsp::PluginInfo, dir: &Path) -> Result<PathBuf, String> {
    let (app, target) = open(info)?;
    let ready = Arc::new(AtomicBool::new(false));
    let go = ready.clone();
    let mut h = egui_kittest::Harness::builder().with_size(vec2(2000.0, 1300.0)).with_pixels_per_point(PPP).wgpu().build_ui_state(
        move |ui, app: &mut SoundApp| {
            if !go.load(Ordering::Relaxed) {
                return;
            }
            let ctx = ui.ctx().clone();
            app.logic(&ctx);
            app.ui(ui);
        },
        app,
    );
    h.input_mut().max_texture_side = Some(8192);
    ready.store(true, Ordering::Relaxed);
    for _ in 0..12 {
        h.step();
    }
    // The parts of the screen the picture is made of, top to bottom, in points.
    let parts: Vec<Rect> = match target {
        Target::Module(t, m) => match soundcraft_ui_egui::editor_kit::frame_rect(&h.ctx, t, m) {
            Some(r) => vec![r],
            None => {
                let r = soundcraft_ui_egui::module_drawer::drawer_rect(&h.ctx, t, m).ok_or("the editor was not drawn")?;
                let below = |top: f32| Rect::from_min_max(r.min + vec2(0.0, top), r.max);
                if graph_kind(info.id) == GraphKind::Level {
                    // A plain in/out meter display tells these apart only by their knobs: the
                    // module's names, then (past the preset row, and the meters when a hero
                    // knob follows) its controls.
                    let s = h.state().engine.session();
                    let hero =
                        s.track(t).and_then(|t| t.mixer.module(m)).and_then(|ins| soundcraft_ui_egui::module_drawer::hero_param_for(info, ins));
                    let name = Rect::from_x_y_ranges(r.x_range(), r.min.y + DRAWER_NAME.0..=r.min.y + DRAWER_NAME.1);
                    vec![name, below(DRAWER_TOP + if hero.is_some() { LEVEL_GRAPH } else { 0.0 })]
                } else {
                    // The drawer's display and controls, below its name and preset row (the
                    // card shows the name; the row is the same for every plugin).
                    vec![below(DRAWER_TOP)]
                }
            }
        },
        Target::Window(id) => vec![h.ctx.memory(|m| m.area_rect(id)).ok_or("the parameter window was not drawn")?],
    };
    let img = h.render()?;
    let screen = Rect::from_min_size(egui::Pos2::ZERO, vec2(img.width() as f32, img.height() as f32));
    let crops: Vec<image::RgbaImage> = parts
        .iter()
        .filter_map(|rect| {
            let px = Rect::from_min_max((rect.min.to_vec2() * PPP).to_pos2(), (rect.max.to_vec2() * PPP).to_pos2()).intersect(screen);
            let px = Rect::from_min_max(px.min.round(), px.max.round());
            (px.width() >= 20.0 && px.height() >= 20.0)
                .then(|| image::imageops::crop_imm(&img, px.min.x as u32, px.min.y as u32, px.width() as u32, px.height() as u32).to_image())
        })
        .collect();
    let w = crops.iter().map(|c| c.width()).min().ok_or("nothing to crop")?;
    let hgt = (crops.iter().map(|c| c.height()).sum::<u32>()).min((w as f32 * MAX_ASPECT) as u32);
    let mut crop = image::RgbaImage::new(w, hgt);
    let mut y = 0;
    for c in &crops {
        image::imageops::replace(&mut crop, &image::imageops::crop_imm(c, 0, 0, w, c.height().min(hgt.saturating_sub(y))).to_image(), 0, y as i64);
        y += c.height();
        if y >= hgt {
            break;
        }
    }
    let (tw, th) = if w > MAX_WIDTH { (MAX_WIDTH, (hgt as f32 * MAX_WIDTH as f32 / w as f32).round() as u32) } else { (w, hgt) };
    let small = image::imageops::resize(&crop, tw, th, image::imageops::FilterType::Lanczos3);
    let rgb = image::DynamicImage::ImageRgba8(small).to_rgb8();
    let path = dir.join(format!("{}.png", info.id));
    let file = std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
    let enc = image::codecs::png::PngEncoder::new_with_quality(
        std::io::BufWriter::new(file),
        image::codecs::png::CompressionType::Best,
        image::codecs::png::FilterType::Adaptive,
    );
    rgb.write_with_encoder(enc).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(path)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.first().is_some_and(|a| a == "--list-presets") {
        for info in soundcraft_dsp::plugins() {
            for (group, list) in soundcraft_ui_egui::fs_presets::factory_groups(info) {
                println!("{} / {group}: {}", info.id, list.iter().map(|(n, _)| *n).collect::<Vec<_>>().join(" | "));
            }
        }
        return;
    }
    let Some(dir) = args.first().map(PathBuf::from) else {
        eprintln!("usage: plugin_thumb <out dir> [plugin id…] | plugin_thumb --list-presets");
        std::process::exit(2);
    };
    let only = &args[1..];
    if let Err(e) = std::fs::create_dir_all(&dir) {
        eprintln!("{}: {e}", dir.display());
        std::process::exit(1);
    }
    let mut failed = 0;
    for info in soundcraft_dsp::plugins().iter().filter(|p| only.is_empty() || only.iter().any(|o| o == p.id)) {
        match thumb(info, &dir) {
            Ok(path) => println!("wrote {}", path.display()),
            Err(e) => {
                eprintln!("{}: {e}", info.id);
                failed += 1;
            }
        }
    }
    if failed > 0 {
        std::process::exit(1);
    }
}

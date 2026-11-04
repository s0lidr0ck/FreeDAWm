//! Render the real UI offscreen (no window, no focus stealing) and save PNGs.
//!
//! `cargo run -p soundcraft-ui-egui --example ui_shot -- out.png [script.jsonl] [--blank] [--audio] [--size WxH] [--system-theme light|dark|none] [--fake-claude] [--user-dir DIR] [--fake-midi "Port A,Port B"]`
//!
//! `--fake-claude` answers Scout's Send with a canned answer streamed over about two seconds (a
//! question containing `[hold]` stays streaming), so the panel can be rendered without a key.
//!
//! Script lines are control requests (`{"method": "...", "params": {...}}`), `{"shot": "path.png"}`,
//! `{"click": [x, y]}` (points; `[x, y, 2]` double-clicks), `{"right_click": [x, y]}` (points; the menu stays open for the next shot), `{"hover": [x, y]}` (moves the pointer, no click), `{"steps": n}` or `{"wait_ms": n}` (frames in real time, about 30 a second, for that long).
//! `{"import_dawproject": "song.dawproject"}` imports one as File › Import › DAWproject… does (the
//! same progress window, then the import report window).
//! `{"open": "path.fdawm"}` opens a session as File › Open does (behind the progress window, see
//! `open_progress`; a control `session.open` is synchronous instead). With `--fake-plugins`,
//! plugin ids under `clap:test.shot.` stand in for hosted plugins that take 300 ms each to load,
//! and `{"save_with_fakes": "path.fdawm"}` saves the current session there with twelve of them over
//! its first three audio tracks: together they render the window mid-load, in its Plugins phase.
//! `{"system_theme": "light"}` (also `"dark"` or `"none"`) changes the simulated OS appearance for
//! subsequent frames. The demo session is loaded unless `--blank`. `--audio` runs the real player,
//! so playing feeds the meters and the modules' graphs. It opens the default OUTPUT DEVICE (a
//! silent clock only when there is none): to keep a render quiet, start the script with
//! `{"method":"engine.execute","params":{"command":"mix.volume","params":{"tracks":["Master 1"],"db":-144}}}`
//! (the demo's master track; strip meters read before it).
//!
//! `{"studio_demo": {"tracks": [{"name", "notes": [[midi (0: a rest), secs, cents?], …]}, …], "key", "chords",
//! "view", "edit"}}` adds synthetic sung lines (never a real recording) on vocal tracks, analyses
//! them and opens the Vocal Studio editing `edit` (see `studio_demo`); `{"studio_roll": [from_s,
//! to_s]}` zooms its roll to that span.
//! `{"vocal_demo": {"track": "Lead Vox", "view": [from_s, to_s], "select": [from_s, to_s],
//! "centre": 40, "drift": 30, "playhead": s, "key": "E", "chords": [[from_s, to_s, "E"], …],
//! "note_mode": "musical"}}`
//! adds a mono vocal track holding `$FREEDAWM_VOCAL_TEST_DIR/00_original.wav` (or `$SOUNDCRAFT_VOCAL_TEST_DIR`) (else
//! `cleanq-ceiling-renders/00_original.wav` found walking up from `CARGO_MANIFEST_DIR`, else a
//! synthetic sung line), analyses it (cutting notes in `note_mode` when given), tunes the notes
//! inside `select` and opens the Vocal Editor
//! on them with the Edit view showing `view`. `{"vocal_view": [from_s, to_s]}` sets the Edit view
//! (which the Vocal Editor follows) once the layout has settled; `{"vocal_dock": h}` makes the
//! lower dock holding it `h` points tall. `{"vocal_drag": {"index": i, "rows": r}}` presses on the
//! editor's i-th note bar drawn and drags it up `r` rows, the button still held (a mid-drag shot);
//! `{"release": [x, y]}` lets go. `{"drag": [x0, y0, x1, y1]}` presses at the first point, moves to
//! the second in steps and releases; `{"drag": [x0, y0, x1, y1, "hold"]}` keeps the button down
//! (a mid-drag shot; `release` lets go). `{"midi_audition_demo": "Bass"}` opens Convert to MIDI
//! on that track's first clip with a synthetic analysis (no model needed): the mini roll and the
//! Play / Source / Notes / Both row as after Analyse. `{"net_demo": true}` sets up Setup ›
//! Network as at a service (`net_ui::demo`): listening, paired devices, a piano PC connected
//! in-process and a booth track on its port with a live sketch (`{"net_demo": "audio"}`: a guitar
//! PC recording audio, its live level and clips on the booth's track; `"internet"`: Setup › Network
//! with an invite made and a friend joined over a real local internet connection). `{"follower_demo": "recording"}`
//! (or `waiting`, `sent`) shows this PC as a piano PC following a booth (`net_ui::follower_demo`).
//! `{"models_demo": "progress"}` (or `"confirm"`) shows Setup › ML Models over a fake model folder
//! and manifest with a download in progress (and the personal-use confirm; `ml_downloads::demo`),
//! without the network.
//! `{"performer_demo": "loud"}` (or `check`, `good`) shows a guitar PC's level window
//! (`net_ui::performer_demo`).

use soundcraft_ui_egui::{ControlRequest, Services, SoundApp};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

static READY: AtomicBool = AtomicBool::new(false);

fn system_theme(value: &str) -> Option<Option<egui::Theme>> {
    match value {
        "light" => Some(Some(egui::Theme::Light)),
        "dark" => Some(Some(egui::Theme::Dark)),
        "none" => Some(None),
        _ => None,
    }
}

const FAKE_ANSWER: &str = "**Summary**
Closer: POST shows 60–150 Hz down 1.8 dB on the bass against Capture 1, but kick and bass still share 90–120 Hz.

**Observations**
- Bass POST crest factor 9.1 dB (was 12.4): the compressor is doing its job.
- Kick low zone −14.2 dB vs bass −13.6 dB: they sit on top of each other.

**Suggestions**
1. Bass: cut 90–120 Hz by 2–3 dB, dynamic.
2. Kick: lift 3–5 kHz by 1–2 dB for the click.

```json
{\"format\": \"scout-guides\", \"version\": 1, \"guides\": [{\"kind\": \"eq\", \"track\": \"Bass\", \"lo_hz\": 90, \"hi_hz\": 120, \"gain_lo_db\": -3, \"gain_hi_db\": -2, \"dynamic\": true, \"q\": 2.5, \"label\": \"kick space\"}, {\"kind\": \"eq\", \"track\": \"Bass\", \"lo_hz\": 700, \"hi_hz\": 1100, \"gain_lo_db\": 1, \"gain_hi_db\": 2, \"dynamic\": false, \"label\": \"growl\"}, {\"kind\": \"eq\", \"track\": \"Bass\", \"lo_hz\": 2500, \"hi_hz\": 5000, \"gain_lo_db\": -2, \"gain_hi_db\": -1, \"dynamic\": false, \"label\": \"string noise\"}, {\"kind\": \"eq\", \"track\": \"Kick\", \"lo_hz\": 3000, \"hi_hz\": 5000, \"gain_lo_db\": 1, \"gain_hi_db\": 2, \"dynamic\": false, \"label\": \"click\"}, {\"kind\": \"module\", \"track\": \"Bass\", \"module\": \"Comp\", \"param\": \"Threshold\", \"current\": \"-18 dB\", \"range\": \"-22 to -24 dB\", \"reason\": \"peaks still 9 dB over the body\"}]}
```";

/// `--fake-plugins`: a "hosted" plugin (a gain) that takes a while to load, for ids under
/// `clap:test.shot.`.
fn fake_plugin(id: &str) -> Option<Box<dyn soundcraft_dsp::Plugin>> {
    if !id.starts_with("clap:test.shot.") {
        return None;
    }
    std::thread::sleep(std::time::Duration::from_millis(300));
    soundcraft_dsp::create("gain")
}

/// `{"save_with_fakes": path}`: twelve fake hosted plugins over the first three audio tracks, then Save As.
fn save_with_fakes(app: &mut SoundApp, path: &str) {
    let tracks: Vec<soundcraft_model::TrackId> =
        app.engine.session().tracks.iter().filter(|t| t.kind == soundcraft_model::TrackKind::Audio).map(|t| t.id).take(3).collect();
    for i in 0..12 {
        let Some(t) = tracks.get(i % tracks.len().max(1)) else { break };
        let ins = soundcraft_model::Insert::new(format!("clap:test.shot.{i}"));
        if let Some(track) = app.engine.session_mut().track_mut(*t)
            && let Err(e) = track.mixer.add_module(ins, None)
        {
            eprintln!("save_with_fakes: {e:?}");
        }
    }
    if let Err(e) = app.run("session.save_as", serde_json::json!({"path": path, "replace": true})) {
        eprintln!("save_with_fakes: {e}");
    }
}

/// A stand-in for the desktop app's Claude call: streams [`FAKE_ANSWER`] in pieces.
fn fake_claude() -> Services {
    use soundcraft_ui_egui::scout::ask::ClaudeEvent;
    Services {
        ask_claude: Some(std::sync::Arc::new(|req, mut on_event| {
            let hold = req.messages.last().is_some_and(|(_, m)| m.contains("[hold]"));
            std::thread::spawn(move || {
                let chars: Vec<char> = FAKE_ANSWER.chars().collect();
                let cut = if hold { chars.len() * 2 / 5 } else { chars.len() };
                for piece in chars[..cut].chunks(60) {
                    on_event(ClaudeEvent::Text(piece.iter().collect()));
                    std::thread::sleep(std::time::Duration::from_millis(40));
                }
                if hold {
                    // Still "answering" for the rest of the render.
                    std::thread::sleep(std::time::Duration::from_secs(600));
                }
                on_event(ClaudeEvent::Done);
            });
        })),
        api_key_status: Some(std::sync::Arc::new(|| Some("sk-ant-…a1b2".into()))),
        ..Services::default()
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let out = args.first().cloned().unwrap_or_else(|| "soundcraft.png".into());
    let script = args.get(1).filter(|a| !a.starts_with("--")).cloned();
    let blank = args.iter().any(|a| a == "--blank");
    let audio = args.iter().any(|a| a == "--audio");
    let appearance = if let Some(i) = args.iter().position(|a| a == "--system-theme") {
        let Some(appearance) = args.get(i + 1).and_then(|value| system_theme(value)) else {
            eprintln!("--system-theme requires light, dark or none");
            return;
        };
        appearance
    } else {
        None
    };
    let (w, h) = args
        .iter()
        .position(|a| a == "--size")
        .and_then(|i| args.get(i + 1))
        .and_then(|s| s.split_once('x'))
        .and_then(|(a, b)| Some((a.parse::<f32>().ok()?, b.parse::<f32>().ok()?)))
        .unwrap_or((1600.0, 1000.0));
    if args.iter().any(|a| a == "--fake-plugins") {
        soundcraft_mix::set_test_plugin_factory(Some(fake_plugin));
    }
    let engine = if blank { soundcraft_engine::Engine::default() } else { soundcraft_engine::demo::demo_engine() };
    let (tx, rx) = mpsc::channel::<ControlRequest>();
    let player = audio.then(|| soundcraft_playback::Player::new(engine.session_arc()));
    let services = if args.iter().any(|a| a == "--fake-claude") { fake_claude() } else { Services::default() };
    let mut app = SoundApp::new(engine, player, services).with_control(rx);
    // `--user-dir DIR`: the track presets and templates folders under DIR (as the app's settings).
    if let Some(dir) = args.iter().position(|a| a == "--user-dir").and_then(|i| args.get(i + 1)) {
        app.dialogs.dirs = soundcraft_ui_egui::user_files::UserDirs::under(std::path::Path::new(dir));
    }
    // `--fake-midi "Port A,Port B"`: MIDI input ports that exist only for the render (the fake
    // backend), so Setup › MIDI Input Devices and the track input menus list them.
    if let (Some(names), Some(hub)) = (args.iter().position(|a| a == "--fake-midi").and_then(|i| args.get(i + 1)), app.midi_hub.clone()) {
        let ports: Vec<&str> = names.split(',').map(str::trim).filter(|n| !n.is_empty()).collect();
        let fake = soundcraft_playback::midi_in::FakeBackend::with_ports(&ports);
        let mut inputs = soundcraft_playback::midi_in::MidiInputs::new(fake.boxed(), hub, app.ui.midi_inputs.clone());
        inputs.rescan();
        app.midi_inputs = Some(inputs);
    }
    let mut harness =
        egui_kittest::Harness::builder().with_size(egui::vec2(w, h)).with_pixels_per_point(2.0).with_max_steps(1_000_000).wgpu().build_ui_state(
            |ui, app: &mut SoundApp| {
                if !READY.load(Ordering::Relaxed) {
                    return;
                }
                let ctx = ui.ctx().clone();
                app.logic(&ctx);
                app.ui(ui);
            },
            app,
        );
    harness.input_mut().max_texture_side = Some(8192);
    harness.input_mut().system_theme = appearance;
    READY.store(true, Ordering::Relaxed);
    let step = |h: &mut egui_kittest::Harness<SoundApp>| {
        let mut raw = std::mem::take(h.input_mut());
        h.state_mut().raw_input_hook(&mut raw);
        *h.input_mut() = raw;
        h.step();
    };
    for _ in 0..6 {
        step(&mut harness);
    }
    let shoot = |h: &mut egui_kittest::Harness<SoundApp>, path: &str| match h.render() {
        Ok(img) => match img.save(path) {
            Ok(()) => println!("wrote {path}"),
            Err(e) => eprintln!("{path}: {e}"),
        },
        Err(e) => eprintln!("render failed: {e}"),
    };
    if let Some(script) = script {
        let text = std::fs::read_to_string(&script).unwrap_or_default();
        for line in text.lines().map(str::trim).filter(|l| !l.is_empty() && !l.starts_with('#')) {
            let Ok(v) = serde_json::from_str::<serde_json::Value>(line) else {
                eprintln!("bad line: {line}");
                continue;
            };
            if let Some(value) = v.get("system_theme") {
                let Some(appearance) = value.as_str().and_then(system_theme) else {
                    eprintln!("system_theme requires light, dark or none: {line}");
                    continue;
                };
                harness.input_mut().system_theme = appearance;
                step(&mut harness);
                continue;
            }
            if let Some(p) = v.get("shot").and_then(|x| x.as_str()) {
                for _ in 0..3 {
                    step(&mut harness);
                }
                shoot(&mut harness, p);
                continue;
            }
            if let Some(ms) = v.get("wait_ms").and_then(|x| x.as_u64()) {
                let until = std::time::Instant::now() + std::time::Duration::from_millis(ms);
                while std::time::Instant::now() < until {
                    step(&mut harness);
                    std::thread::sleep(std::time::Duration::from_millis(30));
                }
                continue;
            }
            if let Some(at) = v.get("click").and_then(|x| x.as_array()) {
                // A primary click at [x, y] (points), [x, y, 2] a double-click. Straight into the
                // frame's input (not the one-event-a-frame synthetic queue), a press and its
                // release in one frame, so a double-click's clicks fall within egui's delay.
                let num = |i: usize| at.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let pos = egui::pos2(num(0), num(1));
                let count = at.get(2).and_then(|x| x.as_u64()).unwrap_or(1).clamp(1, 3);
                let (button, m) = (egui::PointerButton::Primary, egui::Modifiers::NONE);
                harness.input_mut().events.push(egui::Event::PointerMoved(pos));
                step(&mut harness);
                for _ in 0..count {
                    for pressed in [true, false] {
                        harness.input_mut().events.push(egui::Event::PointerButton { pos, button, pressed, modifiers: m });
                    }
                    step(&mut harness);
                }
                for _ in 0..10 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(at) = v.get("right_click").and_then(|x| x.as_array()) {
                let pos =
                    egui::pos2(at.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32, at.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32);
                let m = egui::Modifiers::NONE;
                let button = egui::PointerButton::Secondary;
                harness.state_mut().synthetic.extend([
                    egui::Event::PointerMoved(pos),
                    egui::Event::PointerButton { pos, button, pressed: true, modifiers: m },
                    egui::Event::PointerButton { pos, button, pressed: false, modifiers: m },
                ]);
                for _ in 0..10 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(at) = v.get("hover").and_then(|x| x.as_array()) {
                // Move the pointer there (points), no click: opens a submenu under it.
                let pos =
                    egui::pos2(at.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32, at.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32);
                harness.state_mut().synthetic.push(egui::Event::PointerMoved(pos));
                for _ in 0..10 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(span) = v.get("studio_roll").and_then(|x| x.as_array()) {
                // The Vocal Studio's roll shows `[from_s, to_s]` (its own zoom, by hand).
                let ctx = harness.ctx.clone();
                let roll: Option<egui::Rect> = ctx.data(|m| m.get_temp(egui::Id::new("soundcraft-vocal-roll")));
                let (a, b) = (span.first().and_then(|x| x.as_f64()).unwrap_or(0.0), span.get(1).and_then(|x| x.as_f64()).unwrap_or(6.0));
                let app = harness.state_mut();
                let r = f64::from(app.engine.session().sample_rate.hz());
                if let Some(roll) = roll {
                    app.vocal.hand_zoom = true;
                    app.vocal.t0 = Some((a * r) as i64);
                    app.vocal.spp = Some(((b - a).max(0.1) * r / f64::from(roll.width().max(10.0))).max(0.01));
                }
                for _ in 0..4 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(d) = v.get("vocal_drag") {
                // Presses on the Vocal Editor's note `index` (of the bars drawn) and drags it up
                // `rows` rows (down when negative), holding the button for a mid-drag shot.
                let index = d.get("index").and_then(|x| x.as_u64()).unwrap_or(0) as usize;
                let rows = d.get("rows").and_then(|x| x.as_f64()).unwrap_or(1.0) as f32;
                let ctx = harness.ctx.clone();
                let shapes: Vec<soundcraft_ui_egui::vocal_editor::layout::NoteShape> =
                    ctx.data(|m| m.get_temp(egui::Id::new("soundcraft-vocal-shapes"))).unwrap_or_default();
                let row_h = harness.state().vocal.row_h;
                let Some(sh) = shapes.get(index) else {
                    eprintln!("vocal_drag: no note bar {index} ({} drawn)", shapes.len());
                    continue;
                };
                let from = sh.rect.center();
                let to = from - egui::vec2(0.0, rows * row_h);
                let m = egui::Modifiers::NONE;
                let button = egui::PointerButton::Primary;
                harness
                    .state_mut()
                    .synthetic
                    .extend([egui::Event::PointerMoved(from), egui::Event::PointerButton { pos: from, button, pressed: true, modifiers: m }]);
                step(&mut harness);
                for k in 1..=12 {
                    harness.state_mut().synthetic.push(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
                    step(&mut harness);
                }
                continue;
            }
            if let Some(at) = v.get("drag").and_then(|x| x.as_array()) {
                let num = |i: usize| at.get(i).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32;
                let (from, to) = (egui::pos2(num(0), num(1)), egui::pos2(num(2), num(3)));
                let hold = at.get(4).and_then(|x| x.as_str()) == Some("hold");
                let (button, m) = (egui::PointerButton::Primary, egui::Modifiers::NONE);
                harness
                    .state_mut()
                    .synthetic
                    .extend([egui::Event::PointerMoved(from), egui::Event::PointerButton { pos: from, button, pressed: true, modifiers: m }]);
                step(&mut harness);
                for k in 1..=12 {
                    harness.state_mut().synthetic.push(egui::Event::PointerMoved(from.lerp(to, k as f32 / 12.0)));
                    step(&mut harness);
                }
                if !hold {
                    harness.state_mut().synthetic.push(egui::Event::PointerButton { pos: to, button, pressed: false, modifiers: m });
                }
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(at) = v.get("release").and_then(|x| x.as_array()) {
                // Releases the primary button at [x, y] (points).
                let pos =
                    egui::pos2(at.first().and_then(|x| x.as_f64()).unwrap_or(0.0) as f32, at.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0) as f32);
                let (button, modifiers) = (egui::PointerButton::Primary, egui::Modifiers::NONE);
                harness.state_mut().synthetic.push(egui::Event::PointerButton { pos, button, pressed: false, modifiers });
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(h) = v.get("vocal_dock").and_then(|x| x.as_f64()) {
                // The lower dock holding the Vocal Editor made `h` points tall.
                let app = harness.state_mut();
                let mut path = Vec::new();
                if let Some(root) = app.ui.docking.layout.root.as_ref()
                    && vocal_split(root, &mut path)
                {
                    let _ = app.run("ui.dock", serde_json::json!({"operation": "resizeSplit", "path": path, "size": {"FixedSecond": h}}));
                }
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(p) = v.get("vocal_view").and_then(|x| x.as_array()) {
                // The Edit view (which the Vocal Editor follows) set to [from_s, to_s].
                let app = harness.state_mut();
                let r = f64::from(app.engine.session().sample_rate.hz());
                let (a, b) = (p.first().and_then(|x| x.as_f64()).unwrap_or(0.0), p.get(1).and_then(|x| x.as_f64()).unwrap_or(10.0));
                let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
                let z = &mut app.engine.session_mut().edit.zoom;
                z.samples_per_px = ((b - a).max(0.05) * r / f64::from(w)).max(0.01);
                z.scroll = (a * r) as i64;
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(p) = v.get("comp_demo") {
                comp_demo(harness.state_mut(), p);
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(p) = v.get("studio_demo") {
                studio_demo(harness.state_mut(), p);
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(p) = v.get("vocal_demo") {
                vocal_demo(harness.state_mut(), p);
                for _ in 0..6 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(path) = v.get("save_with_fakes").and_then(|x| x.as_str()) {
                save_with_fakes(harness.state_mut(), path);
                for _ in 0..4 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(which) = v.get("models_demo").and_then(serde_json::Value::as_str) {
                if let Err(e) = soundcraft_ui_egui::ml_downloads::demo(harness.state_mut(), which == "confirm") {
                    eprintln!("models_demo: {e}");
                }
                for _ in 0..3 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(track) = v.get("midi_audition_demo").and_then(serde_json::Value::as_str) {
                let app = harness.state_mut();
                let clip = app.engine.session().tracks.iter().find(|t| t.name == track).and_then(|t| t.clips().first()).map(|c| c.id);
                if let Some(clip) = clip {
                    let _ = soundcraft_ui_egui::ml_ui::open(app, "ml.midi", Some((serde_json::json!({"clip": clip.0}), track.to_string())));
                    if let Some(d) = app.ml.dialog.as_mut() {
                        d.midi.isolate = soundcraft_engine::ml::Isolate::None;
                    }
                    if let Err(e) = soundcraft_ui_egui::ml_audition::demo(app, clip) {
                        eprintln!("midi_audition_demo: {e}");
                    }
                }
                for _ in 0..3 {
                    step(&mut harness);
                }
                continue;
            }
            if v.get("net_demo").is_some() {
                // Setup › Network as at a service: see `net_ui::demo` (`"audio"`: `net_ui::demo_audio`).
                let made = if v["net_demo"] == "audio" {
                    soundcraft_ui_egui::net_ui::demo_audio(harness.state_mut())
                } else if v["net_demo"] == "internet" {
                    soundcraft_ui_egui::net_ui::demo_internet(harness.state_mut())
                } else {
                    soundcraft_ui_egui::net_ui::demo(harness.state_mut())
                };
                if let Err(e) = made {
                    eprintln!("net_demo: {e}");
                }
                for _ in 0..3 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(state) = v.get("performer_demo").and_then(|x| x.as_str()) {
                // A guitar PC's level window (`net_ui::performer_demo`): loud, check, good.
                if let Err(e) = soundcraft_ui_egui::net_ui::performer_demo(harness.state_mut(), state) {
                    eprintln!("performer_demo: {e}");
                }
                for _ in 0..3 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(state) = v.get("follower_demo").and_then(|x| x.as_str()) {
                // A piano PC following a booth (`net_ui::follower_demo`): recording, waiting, sent.
                if let Err(e) = soundcraft_ui_egui::net_ui::follower_demo(harness.state_mut(), state) {
                    eprintln!("follower_demo: {e}");
                }
                for _ in 0..3 {
                    step(&mut harness);
                }
                continue;
            }
            if let Some(path) = v.get("import_dawproject").and_then(|x| x.as_str()) {
                // As File › Import › DAWproject…: behind the progress window, then the report.
                let app = harness.state_mut();
                match app.run("file.import_dawproject", serde_json::json!({"path": path})) {
                    Ok(r) => println!("{r}"),
                    Err(e) => eprintln!("import_dawproject: {e}"),
                }
                continue;
            }
            if let Some(path) = v.get("open").and_then(|x| x.as_str()) {
                // As File › Open: the progress window shows while it loads; the next lines'
                // frames advance it (`wait_ms` for the reading, then one `steps` per plugin).
                let app = harness.state_mut();
                match app.run("session.open", serde_json::json!({"path": path})) {
                    Ok(r) => println!("{r}"),
                    Err(e) => eprintln!("open: {e}"),
                }
                continue;
            }
            if let Some(n) = v.get("steps").and_then(|x| x.as_u64()) {
                for _ in 0..n {
                    step(&mut harness);
                }
                continue;
            }
            let method = v.get("method").and_then(|m| m.as_str()).unwrap_or("").to_string();
            let params = v.get("params").cloned().unwrap_or(serde_json::json!({}));
            let (req, reply) = ControlRequest::new(method, params);
            let _ = tx.send(req);
            for _ in 0..40 {
                step(&mut harness);
                if let Ok(r) = reply.try_recv() {
                    let s = r.to_string();
                    println!("{}", if s.len() > 300 { &s[..300] } else { &s });
                    break;
                }
            }
            for _ in 0..12 {
                step(&mut harness);
            }
        }
    }
    for _ in 0..4 {
        step(&mut harness);
    }
    shoot(&mut harness, &out);
}

/// The local-only sung take, else `None`.
fn vocal_take() -> Option<(Vec<f32>, u32)> {
    let dir = soundcraft_engine::brand::env_var_os("VOCAL_TEST_DIR").map(std::path::PathBuf::from);
    let found = dir.map(|d| d.join("00_original.wav")).filter(|p| p.is_file()).or_else(|| {
        let m = std::env::var_os("CARGO_MANIFEST_DIR")?;
        std::path::Path::new(&m).ancestors().map(|d| d.join("cleanq-ceiling-renders").join("00_original.wav")).find(|p| p.is_file())
    })?;
    let bytes = std::fs::read(&found).ok()?;
    let (info, buf) = soundcraft_audio_io::decode(&bytes, Some("wav")).ok()?;
    Some((buf.channels.into_iter().next()?, info.sample_rate))
}

/// A synthetic sung line (vowel-like harmonics) when the real take is missing.
fn synthetic_take(rate: u32) -> Vec<f32> {
    // (midi + cents, seconds, dB)
    let notes = [(6359.0, 0.6, -24.0), (6441.0, 0.5, -21.0), (6578.0, 0.7, -18.0), (6722.0, 0.4, -22.0), (6880.0, 0.9, -19.0), (6359.0, 0.8, -27.0)];
    let mut out = vec![0.0f32; rate as usize / 4];
    for (c, secs, db) in notes {
        let f0 = 440.0 * 2f64.powf((c - 6900.0) / 1200.0);
        let n = (secs * f64::from(rate)) as usize;
        let g = 10f64.powf(db / 20.0);
        for i in 0..n {
            let t = i as f64 / f64::from(rate);
            let vib = 1.0 + 0.004 * (std::f64::consts::TAU * 5.5 * t).sin();
            let v: f64 = (1..=12).map(|k| (std::f64::consts::TAU * f0 * vib * k as f64 * t).sin() / k as f64).sum();
            out.push((v * g * 0.6) as f32);
        }
        out.extend(std::iter::repeat_n(0.0, rate as usize / 8));
    }
    out
}

/// `{"comp_demo": {"track": "Lead Vox", "takes": 4, "comp": [[bar, take], …], "view": [from_bar, to_bar]}}`:
/// a mono track holding `takes` sung takes (synthetic) over bars 1-6 on take layers, the layers
/// open, and a comp swiped together from them: each `[bar, take]` starts that take (1-based) at
/// that bar (fractional bars allowed) until the next.
fn comp_demo(app: &mut SoundApp, p: &serde_json::Value) {
    use serde_json::json;
    let name = p.get("track").and_then(|x| x.as_str()).unwrap_or("Lead Vox").to_string();
    let takes = p.get("takes").and_then(|x| x.as_u64()).unwrap_or(4).clamp(1, 8) as usize;
    let rate = app.engine.session().sample_rate.hz();
    let (tempo, sr) = (app.engine.session().tempo.clone(), app.engine.session().sample_rate);
    let bar = move |b: f64| {
        let whole = b.floor().max(1.0);
        let at = tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: whole as i64, beat: 1, tick: 0 }, sr);
        let next = tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: whole as i64 + 1, beat: 1, tick: 0 }, sr);
        at + ((next - at) as f64 * (b - whole)) as i64
    };
    let s = app.engine.session_mut();
    let (start, end) = (bar(1.0), bar(6.0));
    let len = (end - start).max(1) as usize;
    let base = synthetic_take(rate);
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some(&name));
    for k in 0..takes {
        // Each take: the line again, phrased a little differently.
        let shift = base.len() / 11 * k;
        let take: Vec<f32> = (0..len)
            .map(|i| {
                let x = base.get((i + shift) % base.len().max(1)).copied().unwrap_or(0.0);
                let swell = 0.75 + 0.25 * ((i as f32 / rate as f32) * (1.3 + k as f32 * 0.4)).sin();
                (x * swell * 5.0).clamp(-1.0, 1.0)
            })
            .collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: vec![take] };
        let src = soundcraft_engine::io::add_source(s, &format!("{name} take {}", k + 1), buf, None, soundcraft_audio_io::FileFormat::Wav);
        let layer = soundcraft_engine::cmd::comp::add_layer(s, t, None).unwrap_or(0);
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|tr| tr.playlists.get_mut(layer)) {
            pl.clips.push(soundcraft_model::Clip::audio(id, format!("Take {}", k + 1), src, 0, start, len as i64));
        }
    }
    if let Some(tr) = s.track_mut(t) {
        tr.record_layer = None;
        tr.height = soundcraft_model::TrackHeight::Medium;
    }
    let comp: Vec<(f64, usize)> = p
        .get("comp")
        .and_then(|x| x.as_array())
        .map(|a| a.iter().filter_map(|v| Some((v.get(0)?.as_f64()?, v.get(1)?.as_u64()? as usize))).collect())
        .unwrap_or_else(|| vec![(1.0, 1)]);
    for (i, (from, take)) in comp.iter().enumerate() {
        let to = comp.get(i + 1).map_or(6.0, |n| n.0);
        let (a, b) = (bar(*from), bar(to));
        if let Err(e) = app.run("comp.swipe", json!({"track": t.0, "layer": take, "start": a, "end": b})) {
            eprintln!("comp_demo: {e}");
        }
    }
    let view = p.get("view");
    let (v0, v1) =
        (view.and_then(|v| v.get(0)).and_then(|x| x.as_f64()).unwrap_or(0.8), view.and_then(|v| v.get(1)).and_then(|x| x.as_f64()).unwrap_or(6.2));
    let (a, b) = (bar(v0), bar(v1));
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    let z = &mut app.engine.session_mut().edit.zoom;
    z.samples_per_px = ((b - a).max(1) as f64 / f64::from(w)).max(0.01);
    z.scroll = a;
}

fn vocal_demo(app: &mut SoundApp, p: &serde_json::Value) {
    use serde_json::json;
    let secs = |v: Option<&serde_json::Value>, i: usize, d: f64| v.and_then(|a| a.get(i)).and_then(|x| x.as_f64()).unwrap_or(d);
    let name = p.get("track").and_then(|x| x.as_str()).unwrap_or("Lead Vox").to_string();
    let rate = app.engine.session().sample_rate.hz();
    let (take, take_rate) = vocal_take().unwrap_or_else(|| (synthetic_take(rate), rate));
    let len = take.len();
    let s = app.engine.session_mut();
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some(&name));
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: take_rate, channels: vec![take] };
    let src = soundcraft_engine::io::add_source(s, &name, buf, None, soundcraft_audio_io::FileFormat::Wav);
    let id = s.new_clip_id();
    soundcraft_engine::edit::place_clip(s, t, soundcraft_model::Clip::audio(id, &name, src, 0, 0, len as i64));
    if let Some(k) = p.get("key").and_then(|x| x.as_str()).and_then(soundcraft_engine::vocal::parse_key) {
        s.band.key = k.0;
        s.band.scale = k.1;
    }
    let r = f64::from(rate);
    for c in p.get("chords").and_then(|x| x.as_array()).into_iter().flatten() {
        let (a, b) = (secs(Some(c), 0, 0.0), secs(Some(c), 1, 0.0));
        let chord = c.get(2).and_then(|x| x.as_str()).unwrap_or("C");
        let _ = app.run("band.chord.add", json!({"at": (a * r) as i64, "end": (b * r) as i64, "chord": chord}));
    }
    if let Err(e) = soundcraft_engine::vocal::run_until_idle(&mut app.engine, &[id]) {
        eprintln!("vocal_demo: {e}");
    }
    if let Some(mode) = p.get("note_mode")
        && let Err(e) = app.run("vocal.note_mode", json!({"clip": id.0, "mode": mode}))
    {
        eprintln!("vocal_demo: {e}");
    }
    let sel = p.get("select");
    let (s0, s1) = ((secs(sel, 0, 0.0) * r) as i64, (secs(sel, 1, 0.0) * r) as i64);
    let notes = soundcraft_engine::vocal::track_notes(app.engine.session(), t, Some(soundcraft_time::Range::new(s0, s1)));
    if !notes.is_empty() {
        let mut params = json!({"notes": notes});
        for k in ["centre", "drift", "vibrato", "raise", "lower"] {
            if let Some(v) = p.get(k) {
                params[k] = v.clone();
            }
        }
        if let Err(e) = app.run("vocal.edit", params) {
            eprintln!("vocal_demo: {e}");
        }
    }
    let view = p.get("view");
    let (v0, v1) = (secs(view, 0, 0.0), secs(view, 1, 10.0));
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    let z = &mut app.engine.session_mut().edit.zoom;
    z.samples_per_px = ((v1 - v0).max(0.1) * r / f64::from(w)).max(0.01);
    z.scroll = (v0 * r) as i64;
    let ph = (p.get("playhead").and_then(|x| x.as_f64()).unwrap_or(v0) * r) as i64;
    let _ = app.run("edit.select", json!({"tracks": [t.0], "start": ph, "end": ph}));
    let _ = app.run("ui.vocal_editor", json!({"track": t.0, "at": ph}));
    let z = &mut app.engine.session_mut().edit.zoom;
    z.scroll = (v0 * r) as i64;
    app.vocal.selected = notes.into_iter().collect();
    app.ui.show_midi_editor = false;
}

/// A synthetic sung line: `notes` are `[midi, seconds, cents?]`, each a vowel-like harmonic
/// series with a little vibrato, 60 ms of silence after each; a `midi` of 0 is a rest as long.
fn sung_line(rate: u32, notes: &[(f64, f64, f64)]) -> Vec<f32> {
    let mut out = vec![0.0f32; rate as usize / 10];
    for (k, (midi, secs, cents)) in notes.iter().enumerate() {
        let f0 = 440.0 * 2f64.powf((midi * 100.0 + cents - 6900.0) / 1200.0);
        let n = (secs * f64::from(rate)) as usize;
        if *midi <= 0.0 {
            out.extend(std::iter::repeat_n(0.0, n + rate as usize * 6 / 100));
            continue;
        }
        let g = 10f64.powf((-21.0 + (k % 3) as f64) / 20.0);
        let formants = [(700.0, 110.0, 1.0), (1150.0, 120.0, 0.55), (2600.0, 170.0, 0.3)];
        let ks: Vec<(f64, f64)> = (1..=30)
            .map(f64::from)
            .filter(|h| h * f0 < f64::from(rate) / 2.0)
            .map(|h| (h, formants.iter().map(|(c, b, a)| a / (1.0 + ((h * f0 - c) / b).powi(2))).sum::<f64>() + 0.02 / h))
            .collect();
        let norm = ks.iter().map(|(_, a)| a).sum::<f64>().max(1e-9);
        let mut ph = 0.0f64;
        for i in 0..n {
            let t = i as f64 / f64::from(rate);
            let env = (t / 0.03).min(1.0).min(((n - i) as f64 / f64::from(rate)) / 0.03);
            let f = f0 * (1.0 + 0.003 * (std::f64::consts::TAU * 5.3 * t).sin());
            ph += std::f64::consts::TAU * f / f64::from(rate);
            let v: f64 = ks.iter().map(|(h, a)| a * (ph * h).sin()).sum();
            out.push((v / norm * g * 2.2 * env) as f32);
        }
        out.extend(std::iter::repeat_n(0.0, rate as usize * 6 / 100));
    }
    out
}

/// `{"studio_demo": {"tracks": [{"name": "Lead Vox", "notes": [[midi, secs, cents?], …]}, …],
/// "key": "E", "chords": [[from_s, to_s, "E"], …], "view": [from_s, to_s], "edit": "BV 1"}}`:
/// mono tracks holding synthetic sung lines (never a real recording), analysed, the band key
/// and chords set, and the Vocal Studio open editing `edit`.
fn studio_demo(app: &mut SoundApp, p: &serde_json::Value) {
    use serde_json::json;
    let rate = app.engine.session().sample_rate.hz();
    let r = f64::from(rate);
    let mut clips = Vec::new();
    let mut ids = Vec::new();
    for tr in p.get("tracks").and_then(|x| x.as_array()).into_iter().flatten() {
        let name = tr.get("name").and_then(|x| x.as_str()).unwrap_or("Vox").to_string();
        let notes: Vec<(f64, f64, f64)> = tr
            .get("notes")
            .and_then(|x| x.as_array())
            .into_iter()
            .flatten()
            .filter_map(|n| Some((n.get(0)?.as_f64()?, n.get(1)?.as_f64()?, n.get(2).and_then(|c| c.as_f64()).unwrap_or(0.0))))
            .collect();
        let take = sung_line(rate, &notes);
        let len = take.len();
        let s = app.engine.session_mut();
        let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some(&name));
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: rate, channels: vec![take] };
        let src = soundcraft_engine::io::add_source(s, &name, buf, None, soundcraft_audio_io::FileFormat::Wav);
        let id = s.new_clip_id();
        soundcraft_engine::edit::place_clip(s, t, soundcraft_model::Clip::audio(id, &name, src, 0, 0, len as i64));
        if let Some(x) = s.track_mut(t) {
            x.vocal = Some(true);
        }
        clips.push(id);
        ids.push((name, t));
    }
    if let Some(k) = p.get("key").and_then(|x| x.as_str()).and_then(soundcraft_engine::vocal::parse_key) {
        let s = app.engine.session_mut();
        s.band.key = k.0;
        s.band.scale = k.1;
    }
    for c in p.get("chords").and_then(|x| x.as_array()).into_iter().flatten() {
        let a = c.get(0).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let b = c.get(1).and_then(|x| x.as_f64()).unwrap_or(0.0);
        let chord = c.get(2).and_then(|x| x.as_str()).unwrap_or("C");
        if let Err(e) = app.run("band.chord.add", json!({"at": (a * r) as i64, "end": (b * r) as i64, "chord": chord})) {
            eprintln!("studio_demo: {e}");
        }
    }
    if let Err(e) = soundcraft_engine::vocal::run_until_idle(&mut app.engine, &clips) {
        eprintln!("studio_demo: {e}");
    }
    let edit = p.get("edit").and_then(|x| x.as_str()).and_then(|n| ids.iter().find(|(x, _)| x == n)).or(ids.first()).map(|(_, t)| *t);
    let view = p.get("view");
    let v0 = view.and_then(|v| v.get(0)).and_then(|x| x.as_f64()).unwrap_or(0.0);
    let v1 = view.and_then(|v| v.get(1)).and_then(|x| x.as_f64()).unwrap_or(6.0);
    if let Some(t) = edit {
        let _ = app.run("edit.select", json!({"tracks": [t.0], "start": (v0 * r) as i64, "end": (v0 * r) as i64}));
        let _ = app.run("ui.vocal_editor", json!({"track": t.0, "at": (v0 * r) as i64}));
        let _ = app.run("ui.vocal_studio", json!({"value": true, "edit": t.0}));
    }
    let w = (app.edit_layout.timeline[2] - app.edit_layout.timeline[0]).max(100.0);
    let z = &mut app.engine.session_mut().edit.zoom;
    z.samples_per_px = ((v1 - v0).max(0.1) * r / f64::from(w)).max(0.01);
    z.scroll = (v0 * r) as i64;
    app.ui.show_midi_editor = false;
}

/// The path of the dock split whose second side holds the Vocal Editor.
fn vocal_split(node: &craft_ui::docking::Node<soundcraft_ui_egui::docking::Panel>, path: &mut Vec<bool>) -> bool {
    use craft_ui::docking::Node;
    let Node::Split { first, second, .. } = node else { return false };
    if matches!(&**second, Node::Tabs { panels, .. } if panels.contains(&soundcraft_ui_egui::docking::Panel::Vocal)) {
        return true;
    }
    for (side, child) in [(false, first), (true, second)] {
        path.push(side);
        if vocal_split(child, path) {
            return true;
        }
        path.pop();
    }
    false
}

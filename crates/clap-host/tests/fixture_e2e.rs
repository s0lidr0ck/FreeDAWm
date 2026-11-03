//! End to end: build the test CLAP plugin in `tests/fixture` (a cdylib outside the workspace),
//! install it as a `.clap` in a temp folder, then scan → instantiate → process → params → notes.

use soundcraft_dsp::{Category, Unit};

mod common;
use common::fixture_dir;

const GAIN: &str = "clap:org.soundcraft.test.gain";
const SYNTH: &str = "clap:org.soundcraft.test.synth";

#[test]
fn scan_lists_both_fixture_plugins() {
    let dir = fixture_dir();
    let found = soundcraft_clap_host::scan_paths(&[dir.to_path_buf()]);
    assert_eq!(found.len(), 5);
    let g = found.iter().find(|d| d.id == GAIN).unwrap();
    assert_eq!(g.name, "Fixture Gain");
    assert_eq!(g.vendor, "SoundCraft tests");
    assert_eq!(g.features, vec!["audio-effect", "stereo"]);
    assert_eq!(g.category, Category::Other);
    assert!(!g.is_instrument);
    let s = found.iter().find(|d| d.id == SYNTH).unwrap();
    assert_eq!(s.category, Category::Instrument);
    assert!(s.is_instrument);
    let global = soundcraft_clap_host::cached().unwrap();
    assert!(global.iter().any(|d| d.id == GAIN) && global.iter().any(|d| d.id == SYNTH));
}

#[test]
fn info_exposes_visible_params_with_names() {
    fixture_dir();
    let info = soundcraft_clap_host::plugin_info(GAIN).unwrap();
    assert_eq!(info.id, GAIN);
    assert_eq!(info.short_name, "Fixture ");
    let ids: Vec<&str> = info.params.iter().map(|p| p.id).collect();
    assert_eq!(ids, vec!["7", "3"], "hidden param 9 is skipped");
    let gain = info.param("7").unwrap();
    assert_eq!((gain.name, gain.min, gain.max, gain.default), ("Gain", 0.0, 2.0, 1.0));
    let mode = info.param("3").unwrap();
    assert_eq!(mode.unit, Unit::Choice);
    assert_eq!(mode.choices, &["A", "B", "C"]);
    // Cached: the same leaked info every time.
    assert!(std::ptr::eq(info, soundcraft_clap_host::plugin_info(GAIN).unwrap()));
}

#[test]
fn gain_processes_audio_and_follows_params() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).unwrap();
    p.prepare(44_100.0, 256, 2);
    assert_eq!(p.latency(), 3);
    assert_eq!(p.param("7"), Some(1.0));
    let mut io = vec![vec![0.5f32; 256], vec![-0.25f32; 256]];
    p.process(&mut io, 256);
    assert!(io[0].iter().all(|&x| (x - 0.5).abs() < 1e-6));
    assert!(p.set_param("7", 2.0));
    assert!(!p.set_param("9", 1.0), "hidden params are not exposed");
    assert!(!p.set_param("Gain", 1.0), "ids are CLAP numbers");
    assert!(p.set_param("7", 5.0), "clamped");
    assert_eq!(p.param("7"), Some(2.0));
    // A block larger than max_block is chunked.
    let mut io = vec![vec![0.5f32; 700], vec![-0.25f32; 700]];
    p.process(&mut io, 700);
    assert!(io[0].iter().all(|&x| (x - 1.0).abs() < 1e-6));
    assert!(io[1].iter().all(|&x| (x + 0.5).abs() < 1e-6));
    // Mono strip: input duplicated into the stereo port, stereo output averaged back.
    p.prepare(48_000.0, 128, 1);
    let mut mono = vec![vec![0.25f32; 128]];
    p.process(&mut mono, 128);
    assert!(mono[0].iter().all(|&x| (x - 0.5).abs() < 1e-6));
    // Hostile shapes don't crash.
    p.process(&mut [], 64);
    let mut short = vec![vec![1.0f32; 4]];
    p.process(&mut short, 1_000_000);
    assert!(p.set_param("7", f32::NAN), "NaN becomes the default");
    assert_eq!(p.param("7"), Some(1.0));
    p.reset();
    p.prepare(f32::NAN, 0, 0);
    let mut io = vec![vec![0.1f32; 8]];
    p.process(&mut io, 8);
}

#[test]
fn synth_plays_notes_at_their_offsets() {
    fixture_dir();
    let info = soundcraft_clap_host::plugin_info(SYNTH).unwrap();
    assert!(info.is_instrument);
    let mut p = soundcraft_clap_host::create(SYNTH).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![9.0f32; 64], vec![9.0f32; 64]];
    p.note_on(10, 60, 127);
    p.note_off(40, 60);
    p.process(&mut io, 64);
    for (i, &x) in io[0].iter().enumerate() {
        let want = if (10..40).contains(&i) { 1.0 } else { 0.0 };
        assert!((x - want).abs() < 1e-6, "frame {i}: {x}");
    }
    assert_eq!(io[0], io[1]);
    // Offsets past the block are clamped into it; all_notes_off releases held notes.
    p.note_on(1000, 61, 64);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.process(&mut io, 64);
    assert!(io[0][62].abs() < 1e-6 && (io[0][63] - 64.0 / 127.0).abs() < 1e-6);
    p.all_notes_off();
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6));
    p.midi(20, [0xB0, 7, 64]);
    p.process(&mut io, 64);
    assert!(io[0][19].abs() < 1e-6 && (io[0][20] - 64.0 / 127.0).abs() < 1e-6, "controllers arrive as MIDI at their offset");
}

#[test]
fn layouts_beyond_the_host_limits_are_rejected_not_truncated() {
    fixture_dir();
    // Control: exactly at the limits (16 ports of 64 channels) activates and gets its full layout.
    let mut p = soundcraft_clap_host::create("clap:org.soundcraft.test.wide16").unwrap();
    p.prepare(44_100.0, 64, 2);
    assert_eq!(p.latency(), 3);
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    assert!(io.iter().flatten().all(|&x| x == 1.0), "16x64 layout delivered as advertised: {:?}", &io[0][..4]);
    // 17 ports, or a 65-channel port: never activated, never processed with a cut-down layout.
    for id in ["clap:org.soundcraft.test.wide17ports", "clap:org.soundcraft.test.wide65ch"] {
        let mut p = soundcraft_clap_host::create(id).unwrap();
        p.prepare(44_100.0, 64, 2);
        assert_eq!(p.latency(), 0, "{id}: not activated");
        let mut io = vec![vec![0.5f32; 64]; 2];
        p.process(&mut io, 64);
        assert!(io.iter().flatten().all(|&x| x == 0.5), "{id}: the plugin never ran: {:?}", &io[0][..4]);
    }
}

#[test]
fn many_instances_create_and_drop_cleanly() {
    fixture_dir();
    for _ in 0..50 {
        let mut a = soundcraft_clap_host::create(GAIN).unwrap();
        let b = soundcraft_clap_host::create(SYNTH).unwrap();
        let mut io = vec![vec![0.0f32; 32]; 2];
        a.process(&mut io, 32);
        drop(b);
    }
}

#[test]
fn state_saves_and_restores_into_a_new_instance() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    assert!(p.set_param("3", 2.0));
    let mut io = vec![vec![0.5f32; 64]; 2];
    p.process(&mut io, 64);
    let blob = p.save_state().expect("the gain fixture has clap.state");
    assert_eq!(blob.len(), 16);
    let mut q = soundcraft_clap_host::create(GAIN).unwrap();
    q.prepare(48_000.0, 64, 2);
    assert_eq!(q.param("7"), Some(1.0));
    assert!(q.load_state(&blob));
    assert_eq!((q.param("7"), q.param("3")), (Some(1.5), Some(2.0)), "values re-read after the load");
    let mut io = vec![vec![0.5f32; 64]; 2];
    q.process(&mut io, 64);
    assert!(io[0].iter().all(|&x| (x - 0.75).abs() < 1e-6), "{:?}", &io[0][..4]);
    // Hostile blobs are refused and leave the plugin as it was.
    assert!(!q.load_state(b"short"));
    assert!(!q.load_state(&[0xff; 16]), "NaN gain rejected by the plugin");
    assert!(!q.load_state(&[0u8; 4096]));
    assert_eq!(q.param("7"), Some(1.5));
    // Plugins without clap.state.
    let mut s = soundcraft_clap_host::create(SYNTH).unwrap();
    assert!(s.save_state().is_none());
    assert!(s.state_reader().is_none());
    assert!(!s.load_state(&blob));
    assert!(s.editor().is_none(), "the synth has no GUI");
    assert!(s.open_editor().is_err());
}

#[test]
fn state_reader_reads_while_another_thread_processes() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    p.process(&mut vec![vec![0.5f32; 64]; 2], 64);
    let mut reader = p.state_reader().expect("the gain fixture has clap.state");
    let blob = p.save_state().unwrap();
    let audio = std::thread::spawn(move || {
        for _ in 0..2_000 {
            p.process(&mut vec![vec![0.5f32; 64]; 2], 64);
        }
        p
    });
    while !audio.is_finished() {
        assert_eq!(reader.save_state().as_ref(), Some(&blob));
    }
    let p = audio.join().unwrap();
    assert_eq!(reader.save_state(), Some(blob));
    drop(p);
    assert_eq!(reader.save_state(), None);
}

#[test]
fn editors_refuse_to_open_off_the_main_thread() {
    fixture_dir();
    // Test threads are never the process main thread.
    let mut p = soundcraft_clap_host::create(GAIN).unwrap();
    let mut ed = p.editor().expect("the gain fixture has clap.gui");
    if cfg!(target_os = "macos") {
        assert!(ed.open().unwrap_err().contains("main thread"));
        assert!(p.open_editor().is_err());
        assert!(!ed.is_open());
    }
    ed.close();
    p.close_editor();
}

#[test]
fn clap_notes_keep_their_channel() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(SYNTH).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    // The fixture outputs velocity + channel; a note-off on another channel leaves the note held.
    p.midi_raw(10, [0x92, 60, 127]);
    p.midi_raw(20, [0x81, 60, 0]);
    p.midi_raw(30, [0x82, 60, 0]);
    p.process(&mut io, 64);
    for (i, &x) in io[0].iter().enumerate() {
        let want = if (10..30).contains(&i) { 3.0 } else { 0.0 };
        assert!((x - want).abs() < 1e-6, "frame {i}: {x}");
    }
    // Velocity 0 is a note-off; channel 1 notes are as before.
    p.midi_raw(5, [0x90, 61, 127]);
    p.midi_raw(15, [0x90, 61, 0]);
    p.process(&mut io, 64);
    assert!((io[0][5] - 1.0).abs() < 1e-6 && io[0][15].abs() < 1e-6);
    // Other channel messages, program change included, go as raw MIDI with their channel.
    p.midi_raw(20, [0xB5, 7, 64]);
    p.process(&mut io, 64);
    assert!(io[0][19].abs() < 1e-6 && (io[0][20] - 64.0 / 127.0).abs() < 1e-6);
    p.midi_raw(8, [0xC4, 3, 0]);
    p.process(&mut io, 64);
    assert!((io[0][7] - 64.0 / 127.0).abs() < 1e-6 && (io[0][8] - 3.25).abs() < 1e-6, "program change keeps its channel");
}

#[test]
fn all_notes_off_closes_every_channel() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(SYNTH).unwrap();
    p.prepare(48_000.0, 64, 2);
    let mut io = vec![vec![0.0f32; 64], vec![0.0f32; 64]];
    p.midi_raw(0, [0x90, 60, 127]);
    p.midi_raw(1, [0x9F, 60, 127]);
    p.midi_raw(2, [0x93, 62, 127]);
    p.process(&mut io, 64);
    assert!((io[0][63] - 4.0).abs() < 1e-6, "{}", io[0][63]);
    p.all_notes_off();
    p.process(&mut io, 64);
    assert!(io[0].iter().all(|x| x.abs() < 1e-6), "every (channel, key) got its note-off");
    // Nothing is left held: a second all_notes_off sends nothing new (and a new note still plays).
    p.all_notes_off();
    p.midi_raw(0, [0x90, 60, 127]);
    p.process(&mut io, 64);
    assert!((io[0][0] - 1.0).abs() < 1e-6);
}

/// Preset discovery: the binary's provider lists the presets inside the plugin (those for the
/// gain, and one for every plugin of the binary); loading one into a fresh instance gives the
/// state a module starts from.
#[test]
fn presets_are_discovered_once_loaded_and_load_into_state() {
    let dir = fixture_dir();
    let gain = soundcraft_clap_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.id == GAIN).unwrap();
    // Scanning in this process loaded the binary (the app scans out of process, where it is not).
    assert!(soundcraft_clap_host::bundle_loaded(&gain));
    let presets = soundcraft_clap_host::presets(&gain);
    let names: Vec<&str> = presets.iter().map(|p| p.name.as_str()).collect();
    assert_eq!(names, ["Loud", "Quiet", "Shared"]);
    let quiet = presets.iter().find(|p| p.name == "Quiet").unwrap();
    assert_eq!((quiet.location.as_deref(), quiet.load_key.as_deref(), quiet.kind()), (None, Some("quiet"), 1));
    let state = soundcraft_clap_host::preset_state(GAIN, quiet).unwrap();
    assert_eq!(state[..8], 0.5f64.to_le_bytes(), "gain 0.5 in the saved state");
    let bad = soundcraft_clap_host::ClapPreset { name: "x".into(), location: None, load_key: Some("nope".into()) };
    assert!(soundcraft_clap_host::preset_state(GAIN, &bad).unwrap_err().contains("refused"));
    // Not loaded: nothing is listed (and nothing is loaded to list it).
    let elsewhere = soundcraft_clap_host::ClapDescriptor { path: dir.join("missing.clap"), ..gain };
    assert!(!soundcraft_clap_host::bundle_loaded(&elsewhere));
    assert!(soundcraft_clap_host::presets(&elsewhere).is_empty());
}

/// A preset loads into the running instance through its state handle, with no new instance.
#[test]
fn state_handle_loads_into_the_running_instance() {
    fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).unwrap();
    p.prepare(48_000.0, 64, 2);
    assert!(p.set_param("7", 1.5));
    p.process(&mut vec![vec![0.5f32; 64]; 2], 64);
    let blob = p.save_state().unwrap();
    let mut q = soundcraft_clap_host::create(GAIN).unwrap();
    q.prepare(48_000.0, 64, 2);
    let mut handle = q.state_reader().unwrap();
    assert!(handle.load_state(&blob));
    assert_eq!(handle.save_state().as_ref(), Some(&blob));
    assert!(!handle.load_state(&[0xff; 16]), "refused by the plugin");
    // A discovered preset, into the running plugin (`clap.preset-load`).
    assert!(handle.load_preset(1, None, Some("quiet")));
    assert_eq!(handle.save_state().unwrap()[..8], 0.5f64.to_le_bytes());
    assert!(!handle.load_preset(1, None, Some("nope")));
    drop(q);
    assert!(!handle.load_state(&blob), "the instance is gone");
    assert!(!handle.load_preset(1, None, Some("quiet")));
}

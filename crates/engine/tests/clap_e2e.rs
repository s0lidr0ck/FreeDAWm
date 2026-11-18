//! A hosted CLAP plugin through the whole stack: `mix.insert` / `mix.insert_param` accept a
//! `clap:` id and the mix engine renders through it. Uses the test plugin built from
//! `crates/clap-host/tests/fixture` (a gain whose param "7" is linear gain).

use serde_json::json;
use soundcraft_audio_io::AudioBuffer;
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, Clip, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::path::Path;
use std::process::Command;
use std::sync::Arc;

fn install_fixture() {
    static ONCE: std::sync::Once = std::sync::Once::new();
    ONCE.call_once(install_fixture_now);
}

fn install_fixture_now() {
    let manifest = Path::new(&std::env::var_os("CARGO_MANIFEST_DIR").unwrap_or_else(|| env!("CARGO_MANIFEST_DIR").into()))
        .join("../clap-host/tests/fixture/Cargo.toml");
    let target = Path::new(env!("CARGO_TARGET_TMPDIR")).join("clap-fixture-target");
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".into());
    let st = Command::new(cargo)
        .args(["build", "--quiet", "--manifest-path"])
        .arg(&manifest)
        .arg("--target-dir")
        .arg(&target)
        .env_remove("RUSTFLAGS")
        .status()
        .unwrap();
    assert!(st.success());
    let lib = if cfg!(target_os = "macos") {
        "libclap_fixture.dylib"
    } else if cfg!(windows) {
        "clap_fixture.dll"
    } else {
        "libclap_fixture.so"
    };
    let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("engine-clap-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    std::fs::copy(target.join("debug").join(lib), dir.join("Fixture.clap")).unwrap();
    assert_eq!(soundcraft_clap_host::add_search_dir(&dir), 6);
}

#[test]
fn clap_insert_renders_through_the_mix() {
    install_fixture();
    let mut e = Engine::default();
    let list = e.execute("engine.clap_plugins", &json!({})).unwrap();
    assert!(list.as_array().unwrap().iter().any(|d| d["id"] == "clap:org.soundcraft.test.gain"));

    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.pool.insert(SourceId(77), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(77), 0, 0, 4800));
    let dry = soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000];
    assert!(dry.abs() > 0.01);

    let r = e.execute("mix.insert", &json!({"track": t.0, "plugin": "clap:org.soundcraft.test.gain", "params": {"7": 2.0}})).unwrap();
    assert_eq!(r["plugin"], "clap:org.soundcraft.test.gain");
    let wet = soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000];
    assert!((wet - 2.0 * dry).abs() < 1e-4, "dry {dry} wet {wet}");

    e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "7", "value": 0.5})).unwrap();
    let half = soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000];
    assert!((half - 0.5 * dry).abs() < 1e-4, "dry {dry} half {half}");
    assert!(e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "Gain", "value": 1.0})).is_err());
}

#[test]
fn a_refused_state_is_reported() {
    install_fixture();
    let gain = "clap:org.soundcraft.test.gain";
    let make = |state: Option<&[u8]>| soundcraft_mix::create_instance(gain, state, 48_000.0, 512, 2).unwrap();
    let saved = make(None).plugin.save_state().unwrap();
    assert!(!make(None).refused_state);
    assert!(!make(Some(&saved)).refused_state);
    assert!(make(Some(&[0xff; 16])).refused_state, "NaN gain");
}

#[test]
fn clap_state_is_captured_stored_and_restored_before_the_first_block() {
    install_fixture();
    let gain = "clap:org.soundcraft.test.gain";
    let mut e = Engine::default();
    let s = e.session_mut();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.pool.insert(SourceId(78), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] })));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(78), 0, 0, 4800));
    e.execute("mix.insert", &json!({"track": t.0, "plugin": gain, "params": {"7": 2.0}})).unwrap();

    // A realtime-style engine: the instance is created "on the UI thread" from the specs and
    // adopted; its live state (gain 1.5, changed behind the session's back as a plugin GUI
    // would) is captured and stored on the insert.
    let mut eng = soundcraft_mix::MixEngine::new(48_000.0, 512);
    eng.set_external_instances(true);
    let specs = soundcraft_mix::instance_specs(e.session());
    assert_eq!(specs.len(), 1);
    let mut p = soundcraft_mix::create_instance(&specs[0].id, None, 48_000.0, 512, specs[0].ch).unwrap().plugin;
    p.set_param("7", 1.5);
    let mut io = vec![vec![0.0f32; 64]];
    p.process(&mut io, 64);
    let mut reader = p.state_reader().unwrap();
    eng.adopt(vec![soundcraft_mix::PreparedInstance { track: t, slot: specs[0].slot, id: gain.into(), ch: 1, plugin: p }]);
    eng.ensure_synced(e.session());
    let blob = reader.save_state().unwrap();
    e.execute("mix.insert_state", &json!({"track": t.0, "slot": 0, "state": soundcraft_model::b64::encode(&blob)})).unwrap();
    assert_eq!(eng.take_retired().len(), 0);

    // Offline render (synchronous creation): the state is restored before the first block.
    // With the gain absent from the session's params, the state's 1.5 is what plays.
    let gain_of = |s: &soundcraft_model::Session| soundcraft_mix::render_range(s, Range::new(0, 4800), 512)[0][2000];
    let mut s2 = e.session().clone();
    s2.track_mut(t).unwrap().mixer.inserts[0].params.remove("7");
    let restored = gain_of(&s2);
    let mut s3 = e.session().clone();
    let ins = &mut s3.track_mut(t).unwrap().mixer.inserts[0];
    ins.state = None;
    ins.params.insert("7".into(), 1.5);
    let reference = gain_of(&s3);
    assert!((restored - reference).abs() < 1e-5, "state {restored} vs param 1.5 {reference}");
    // Session params apply on top of the state.
    e.execute("mix.insert_param", &json!({"track": t.0, "slot": 0, "param": "7", "value": 0.5})).unwrap();
    let half = gain_of(e.session());
    assert!((half - reference / 3.0).abs() < 1e-5, "{half} vs {}", reference / 3.0);
}

/// A CLAP plugin with a sidechain port is keyed from another track through the mixer, like the
/// built-in keyed modules: chosen with `mix.module_sidechain`, rendered into its key port.
#[test]
fn a_clap_sidechain_is_keyed_from_another_track() {
    install_fixture();
    let mut e = Engine::default();
    let s = e.session_mut();
    let main = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    let kick = s.add_track(TrackKind::Audio, ChannelFormat::Mono, None);
    s.pool.insert(SourceId(80), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 4800]] })));
    s.pool.insert(SourceId(81), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.125; 4800]] })));
    for (t, src) in [(main, 80), (kick, 81)] {
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "dc", SourceId(src), 0, 0, 4800));
    }
    let r = e.execute("mix.insert", &json!({"track": main.0, "plugin": "clap:org.soundcraft.test.keyed"})).unwrap();
    let module = r["module"].as_u64().unwrap();
    let mix = |e: &Engine| soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000];
    let unkeyed = mix(&e);
    e.execute("mix.module_sidechain", &json!({"track": main.0, "module": module, "source": kick.0})).unwrap();
    let keyed = mix(&e);
    // Unkeyed the mix is 0.25 + 0.125 (one pan law on both centred tracks); keyed, the plugin adds
    // the kick's 0.125 into the main track as well.
    assert!(unkeyed.abs() > 0.01);
    assert!(((keyed - unkeyed) - unkeyed / 3.0).abs() < 1e-4, "unkeyed {unkeyed} keyed {keyed}");
    // Cleared, the key port is silent again.
    e.execute("mix.module_sidechain", &json!({"track": main.0, "module": module, "source": null})).unwrap();
    assert!((mix(&e) - unkeyed).abs() < 1e-5);
}

/// A hosted instrument's second output port is listed once the mix has made it, and taken on a
/// mixer channel of its own (`mix.new_output_track`) it is heard there: the fixture synth's
/// "Aux" plays at half its level.
#[test]
fn a_clap_instruments_aux_output_plays_on_its_own_channel() {
    install_fixture();
    let mut e = Engine::default();
    let t = soundcraft_model::TrackId(e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap());
    e.execute("mix.instrument", &json!({"track": t.0, "plugin": "clap:org.soundcraft.test.synth"})).unwrap();
    let mut seq = soundcraft_midi::Sequence::default();
    seq.notes.push(soundcraft_midi::Note {
        probability: 100,
        pitch: 60,
        velocity: 127,
        muted: false,
        release_velocity: 64,
        channel: 0,
        start: 0,
        length: 9600,
    });
    let id = e.session_mut().new_clip_id();
    soundcraft_engine::edit::place_clip(e.session_mut(), t, Clip::midi(id, "n", 0, 48_000, seq));
    let mix = |e: &Engine| soundcraft_mix::render_range(e.session(), Range::new(0, 4800), 512)[0][2000];
    let alone = mix(&e);
    assert!(alone.abs() > 0.01);
    let names = soundcraft_engine::cmd::instrument::output_names(e.session().track(t).unwrap().instrument.as_ref().unwrap());
    assert_eq!(names, vec!["Aux".to_string()], "the port the plug-in declared");
    let r = e.execute("mix.new_output_track", &json!({"track": t.0, "output": 0})).unwrap();
    assert!(r["aux"].as_u64().is_some(), "{r}");
    let both = mix(&e);
    assert!(((both - alone) - 0.5 * alone).abs() < 1e-3, "alone {alone}, with the aux channel {both}");
}

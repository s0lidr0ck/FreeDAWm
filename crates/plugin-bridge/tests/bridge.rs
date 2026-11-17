//! The bridge end to end, with the real helper process (`freedawm-plugin-host`, built with these
//! tests): a built-in plug-in and the CLAP test plug-in served in it, and a helper killed while
//! audio runs through it.

use soundcraft_dsp::Plugin;
use soundcraft_plugin_bridge::BridgedPlugin;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::OnceLock;
use std::time::{Duration, Instant};

fn exe() -> &'static Path {
    Path::new(env!("CARGO_BIN_EXE_freedawm-plugin-host"))
}

/// Block `b`: 128 stereo frames, every sample distinct and small.
fn block(b: usize) -> Vec<Vec<f32>> {
    (0..2).map(|c| (0..128).map(|i| ((b * 256 + i * 2 + c) % 1000) as f32 / 2000.0 + 0.01).collect()).collect()
}

/// Processes `n` blocks from `from` as a realtime callback would, a period apart (the helper's
/// time to answer), returning what was sent and heard.
fn play(p: &mut BridgedPlugin, from: usize, n: usize) -> (Vec<Vec<f32>>, Vec<Vec<f32>>) {
    let (mut sent, mut heard) = (Vec::new(), Vec::new());
    for b in from..from + n {
        let mut io = block(b);
        sent.push(io[0].clone());
        p.process(&mut io, 128);
        heard.push(io[0].clone());
        std::thread::sleep(Duration::from_millis(3));
    }
    (sent, heard)
}

#[test]
fn a_built_in_plug_in_plays_through_its_own_process_one_block_late() {
    let mut p = BridgedPlugin::spawn_with(exe(), "gain", None).unwrap();
    assert_eq!(p.info().name, "Gain");
    assert_eq!(p.info().params.len(), 1);
    p.prepare(48_000.0, 128, 2);
    assert_eq!(p.latency(), 128, "the bridge's block");
    let (sent, heard) = play(&mut p, 0, 40);
    assert!(heard[0].iter().all(|x| *x == 0.0));
    // Unity gain: each block comes back unchanged, one block later (allowing for a block the
    // helper might have missed on a busy machine, which plays its input as it is: the same).
    for b in 1..40 {
        assert_eq!(heard[b], sent[b - 1], "block {b}");
    }
    // A parameter goes over with the next block: +6 dB, smoothed in over 20 ms.
    assert!(p.set_param("gain", 6.0206));
    assert_eq!(p.param("gain"), Some(6.0206));
    let (sent, heard) = play(&mut p, 40, 40);
    eprintln!("blocks not back in time: {}", p.overruns());
    if p.overruns() == 0 {
        let ratio = heard[39][64] / sent[38][64];
        assert!((ratio - 2.0).abs() < 1e-2, "{ratio}");
    }
    // Its state round-trips over the pipe (a built-in keeps none: None, not a hang).
    assert_eq!(p.save_state(), None);
    assert!(!p.crashed());
}

#[test]
fn a_helper_killed_mid_render_silences_its_plug_in_and_the_host_goes_on() {
    let mut p = BridgedPlugin::spawn_with(exe(), "gain", None).unwrap();
    p.prepare(48_000.0, 128, 2);
    play(&mut p, 0, 20);
    assert!(!p.crashed());
    let killed_at = Instant::now();
    p.kill_helper();
    // The audio thread never waits for it: every block returns at once, an effect passing its
    // input through as if bypassed, still one block late (the latency it reported).
    let mut slowest = Duration::ZERO;
    let mut checked = 0;
    for b in 20..200 {
        let gone = p.crashed();
        let mut io = block(b);
        let started = Instant::now();
        p.process(&mut io, 128);
        slowest = slowest.max(started.elapsed());
        if gone {
            assert_eq!(io, block(b - 1), "block {b}: the block before, passed through");
            checked += 1;
        }
        std::thread::sleep(Duration::from_millis(1));
    }
    assert!(slowest < Duration::from_millis(5), "a block took {slowest:?}");
    assert!(checked > 100, "its end was seen within a few blocks ({checked} blocks after it)");
    while !p.crashed() && killed_at.elapsed() < Duration::from_secs(5) {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(p.crashed());
    // Said once, by name.
    let deadline = Instant::now() + Duration::from_secs(5);
    let mut reports = Vec::new();
    while reports.is_empty() && Instant::now() < deadline {
        reports = soundcraft_plugin_bridge::take_crash_reports();
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(reports.iter().any(|r| r.starts_with("Gain crashed in its own process")), "{reports:?}");
    // What the host asks of it now fails at once instead of hanging, and dropping it is fine.
    let asked = Instant::now();
    assert_eq!(p.save_state(), None);
    p.prepare(48_000.0, 256, 2);
    let mut io = block(0);
    p.process(&mut io, 128);
    assert_eq!(io, block(0));
    assert!(asked.elapsed() < Duration::from_secs(1));
    drop(p);
}

#[test]
fn a_helper_that_cannot_open_the_plug_in_says_why() {
    let e = BridgedPlugin::spawn_with(exe(), "no.such.plugin", None).err().unwrap();
    assert!(e.contains("no plug-in"), "{e}");
    let e = BridgedPlugin::spawn_with(Path::new("/no/such/helper"), "gain", None).err().unwrap();
    assert!(e.contains("cannot start"), "{e}");
}

/// The CLAP test plug-in of `soundcraft-clap-host` (`tests/fixture`), built once and installed
/// in a folder of its own.
fn clap_fixture() -> &'static Path {
    static DIR: OnceLock<PathBuf> = OnceLock::new();
    DIR.get_or_init(|| {
        let manifest = Path::new(env!("CARGO_MANIFEST_DIR")).join("../clap-host/tests/fixture/Cargo.toml");
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
        assert!(st.success(), "building the CLAP fixture failed");
        let lib = if cfg!(target_os = "macos") {
            "libclap_fixture.dylib"
        } else if cfg!(windows) {
            "clap_fixture.dll"
        } else {
            "libclap_fixture.so"
        };
        let dir = Path::new(env!("CARGO_TARGET_TMPDIR")).join(format!("bridge-fixture-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(target.join("debug").join(lib), dir.join("Fixture.clap")).unwrap();
        dir
    })
}

#[test]
fn a_clap_plug_in_is_described_in_a_helper_and_a_crash_there_is_only_the_helpers() {
    use soundcraft_plugin_bridge::{DescribeError, describe_with};
    const GAIN: &str = "clap:org.soundcraft.test.gain";
    let info = describe_with(exe(), GAIN, Some(clap_fixture()), &[]).unwrap();
    assert_eq!(info.name, "Fixture Gain");
    assert_eq!(info.params.iter().map(|p| p.id.as_str()).collect::<Vec<_>>(), ["7", "3"]);
    assert!(!info.instrument);
    // The plug-in aborts its process as it is made: this test process goes on and is told so.
    let started = Instant::now();
    let crashed = describe_with(exe(), GAIN, Some(clap_fixture()), &[("SOUNDCRAFT_CLAP_FIXTURE_ABORT", "org.soundcraft.test.gain")]);
    assert_eq!(crashed.err(), Some(DescribeError::Crashed));
    assert!(started.elapsed() < Duration::from_secs(5), "{:?}", started.elapsed());
    // One the helper cannot make is refused with the reason, not reported as a crash.
    let refused = describe_with(exe(), "clap:org.soundcraft.test.none", Some(clap_fixture()), &[]);
    assert!(matches!(refused, Err(DescribeError::Refused(_))), "{refused:?}");
    let missing = describe_with(Path::new("/no/such/helper"), GAIN, None, &[]);
    assert!(matches!(missing, Err(DescribeError::Start(_))), "{missing:?}");
}

#[test]
fn a_crashed_plug_in_keeps_its_last_state_and_a_restart_brings_it_back() {
    const GAIN: &str = "clap:org.soundcraft.test.gain";
    let mut p = BridgedPlugin::spawn_with(exe(), GAIN, Some(clap_fixture())).unwrap();
    p.prepare(48_000.0, 128, 2);
    // Gain 2 lives in the plug-in's own state (the fixture saves gain and mode).
    let mut gain2 = BridgedPlugin::spawn_with(exe(), GAIN, Some(clap_fixture())).unwrap();
    assert!(gain2.set_param("7", 2.0));
    gain2.prepare(48_000.0, 128, 2);
    play(&mut gain2, 0, 4);
    let state = gain2.save_state().expect("the fixture keeps a state");
    drop(gain2);
    assert!(p.load_state(&state));
    let mut reader = p.state_reader().unwrap();
    assert!(!reader.crashed());
    p.kill_helper();
    let deadline = Instant::now() + Duration::from_secs(5);
    while !reader.crashed() && Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(reader.crashed(), "the app is told, so it offers Restart");
    // Gone, it still answers the state it last had: what Restart makes it again from.
    assert_eq!(reader.save_state(), Some(state.clone()));
    assert_eq!(p.save_state(), Some(state.clone()));
    let mut again = BridgedPlugin::spawn_with(exe(), GAIN, Some(clap_fixture())).unwrap();
    assert!(again.load_state(&state));
    assert_eq!(again.param("7"), Some(2.0), "the host's copy follows the state, and is not sent back over it");
    again.prepare(48_000.0, 128, 2);
    let (sent, heard) = play(&mut again, 0, 30);
    if again.overruns() == 0 {
        for b in 2..30 {
            assert!(heard[b].iter().zip(&sent[b - 1]).all(|(h, s)| (h - s * 2.0).abs() < 1e-6), "block {b}: gain 2 again");
        }
    }
    assert!(!again.crashed());
}

#[test]
fn a_clap_plug_in_runs_in_its_own_process() {
    // Loaded only in the helper: this process never loads the plug-in's binary.
    let mut p = BridgedPlugin::spawn_with(exe(), "clap:org.soundcraft.test.gain", Some(clap_fixture())).unwrap();
    assert_eq!(p.info().name, "Fixture Gain");
    let ids: Vec<&str> = p.info().params.iter().map(|q| q.id).collect();
    assert_eq!(ids, ["7", "3"]);
    p.prepare(48_000.0, 128, 2);
    assert_eq!(p.latency(), 3 + 128, "its own latency and the bridge's block");
    assert!(p.set_param("7", 2.0));
    let (sent, heard) = play(&mut p, 0, 30);
    eprintln!("blocks not back in time: {}", p.overruns());
    if p.overruns() == 0 {
        for b in 2..30 {
            let want: Vec<f32> = sent[b - 1].iter().map(|x| x * 2.0).collect();
            assert!(heard[b].iter().zip(&want).all(|(a, w)| (a - w).abs() < 1e-6), "block {b}");
        }
    }
    let state = p.save_state();
    if let Some(s) = &state {
        assert!(p.load_state(s));
    }
    p.kill_helper();
    std::thread::sleep(Duration::from_millis(200));
    assert!(p.crashed());
    for _ in 0..3 {
        p.process(&mut block(0), 128);
    }
    // Passed through, one block late as before.
    let mut io = block(5);
    p.process(&mut io, 128);
    assert_eq!(io, block(0));
}

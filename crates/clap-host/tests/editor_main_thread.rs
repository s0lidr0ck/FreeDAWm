//! Plugin editors must be driven from the process main thread, which the standard test harness
//! never uses, so this test binary has its own `main` (`harness = false`). The fixture's GUI is
//! window-less: nothing appears on screen.

mod common;

use soundcraft_dsp::Plugin;

const GAIN: &str = "clap:org.soundcraft.test.gain";

fn block(p: &mut Box<dyn Plugin>) -> Vec<Vec<f32>> {
    let mut io = vec![vec![1.0f32; 64]; 2];
    p.process(&mut io, 64);
    io
}

fn main() {
    common::fixture_dir();
    let mut p = soundcraft_clap_host::create(GAIN).expect("fixture");
    p.prepare(48_000.0, 64, 2);
    let mut ed = p.editor().expect("editor handle");
    assert!(!ed.is_open());
    ed.open().expect("open on the main thread");
    assert!(ed.is_open());
    ed.open().expect("opening again just raises it");
    // The fixture "edits" the gain in its GUI; the edit arrives as an output event of the next
    // block and is reported (in SoundCraft units) by idle.
    let io = block(&mut p);
    assert!((io[0][10] - 0.25).abs() < 1e-6, "{}", io[0][10]);
    assert_eq!(ed.idle(), vec![("7".to_string(), 0.25)]);
    assert!(ed.idle().is_empty(), "drained");
    ed.close();
    assert!(!ed.is_open());
    // Through the Plugin trait as well.
    p.open_editor().expect("open via the trait");
    assert!(ed.is_open(), "the handle sees the same editor");
    p.close_editor();
    assert!(!ed.is_open());
    // Dropping the instance with its editor open destroys the GUI first (the fixture asserts
    // that); the handle then refuses politely.
    ed.open().expect("reopen");
    drop(p);
    assert!(!ed.is_open());
    assert!(ed.open().is_err());
    assert!(ed.idle().is_empty());
    ed.close();
    host_timers_fire_from_idle();
    println!("editor_main_thread: ok");
}

/// `clap.timer-support`: the fixture registers a 20 ms timer at `init`; the host fires it from
/// the editor handle's `idle` on the main thread (never by itself), counted in its param 15.
fn host_timers_fire_from_idle() {
    let mut p = soundcraft_clap_host::instantiate_plugin(GAIN).expect("fixture");
    let mut ed = p.editor().expect("editor handle");
    std::thread::sleep(std::time::Duration::from_millis(100));
    assert_eq!(p.live_value("15"), Some(0.0), "timers only fire from idle");
    let start = std::time::Instant::now();
    for _ in 0..40 {
        assert!(ed.idle().is_empty());
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
    // Sleeps overshoot on busy machines (CI runners): bound the count by the time that passed.
    let elapsed_ms = start.elapsed().as_secs_f64() * 1000.0;
    let ticks = p.live_value("15").expect("the Ticks param");
    let most = elapsed_ms / 20.0 + 2.0;
    assert!((5.0..=most).contains(&f64::from(ticks)), "a 20 ms timer over {elapsed_ms:.0} ms of frames ticked {ticks} times");
}

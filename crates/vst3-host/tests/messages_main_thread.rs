//! Processor ↔ controller messaging through the host (`IMessage` from `createInstance`, the
//! host's connection proxies) and `restartComponent(kParamValuesChanged)`, on the process main
//! thread (`harness = false`: the proxies deliver to the controller on the main thread only).
//! The fixture gain allocates a message at `initialize`, sends one at `connect`, from `setState`
//! and from `process` when its Gain changes; its controller takes the value and asks for a
//! restart (see the fixture's docs).

mod common;

use soundcraft_dsp::Plugin;

fn block(p: &mut soundcraft_vst3_host::Vst3Plugin) -> Vec<Vec<f32>> {
    let mut io = vec![vec![1.0f32; 64]; 2];
    p.process(&mut io, 64);
    io
}

fn value(edits: &[(String, f32)], id: &str) -> Option<f32> {
    edits.iter().find(|(k, _)| k == id).map(|(_, v)| *v)
}

fn main() {
    let dir = common::fixture_dir();
    let gain = soundcraft_vst3_host::scan_paths(&[dir.to_path_buf()]).into_iter().find(|d| d.name == "Fixture Gain").expect("fixture").id;
    // Instantiation already allocated messages (initialize) and sent some (connect) both ways.
    let mut p = soundcraft_vst3_host::instantiate_plugin(&gain).expect("instantiate");
    let params = p.info().params.len();
    let mut ed = p.editor().expect("editor handle");
    assert_eq!(ed.idle().len(), 1, "only the fixture's setup edit so far");

    // Main thread: the processor's message from `process` reaches the controller at once, and
    // the controller's restart makes idle report every parameter's current value.
    assert!(p.set_param("7", 1.2));
    let io = block(&mut p);
    assert!((io[0][5] - 1.2).abs() < 1e-6, "{}", io[0][5]);
    let edits = ed.idle();
    assert_eq!(edits.len(), params, "every parameter after kParamValuesChanged: {edits:?}");
    assert_eq!(value(&edits, "7"), Some(1.2), "the controller took the message's value: {edits:?}");
    assert!(ed.idle().is_empty(), "reported once");

    // Audio thread: the same message is queued and arrives with the next idle on this thread.
    std::thread::scope(|s| {
        s.spawn(|| {
            assert!(p.set_param("7", 0.4));
            let io = block(&mut p);
            assert!((io[0][5] - 0.4).abs() < 1e-6, "{}", io[0][5]);
        });
    });
    let edits = ed.idle();
    assert_eq!(value(&edits, "7"), Some(0.4), "delivered on the main thread: {edits:?}");

    // The other way, off the main thread: a Mode set on a worker thread reaches the controller
    // at that thread's state save (`sync_param`), from where the controller tells the processor
    // by message and announces a latency change. The host's proxy queues the message (worker
    // thread) and delivers it with the next idle on this thread; the next block re-reads the
    // latency the processor now reports.
    assert_eq!(p.latency(), 3);
    std::thread::scope(|s| {
        s.spawn(|| {
            assert!(p.set_param("3", 2.0));
            assert!(p.state_reader().expect("state").save_state().is_some(), "syncs the host-set Mode into the controller");
        });
    });
    let _ = ed.idle();
    let _ = block(&mut p);
    assert_eq!(p.latency(), 5, "the controller's message reached the processor on the main thread");

    // A second instance while the first is live (a freeze or bounce makes one): its own
    // messages at initialize/connect, then the first one's state restored into it sends "gain"
    // from setState, and its controller follows.
    let state = p.state_reader().expect("state").save_state().expect("the first instance's state");
    let mut second = soundcraft_vst3_host::instantiate_plugin(&gain).expect("second instance");
    let mut ed2 = second.editor().expect("editor handle");
    let _ = ed2.idle();
    assert!(second.load_state(&state));
    let edits = ed2.idle();
    assert_eq!(value(&edits, "7"), Some(0.4), "the restored state reached the second controller: {edits:?}");
    let io = block(&mut second);
    assert!((io[0][5] - 0.4).abs() < 1e-6, "{}", io[0][5]);
    drop(second);
    // The first keeps working after the second is gone.
    assert!(p.set_param("7", 1.0));
    let io = block(&mut p);
    assert!((io[0][5] - 1.0).abs() < 1e-6, "{}", io[0][5]);
    assert_eq!(value(&ed.idle(), "7"), Some(1.0));
    drop(p);
    assert!(ed.idle().is_empty(), "the plugin is gone");
    println!("messages_main_thread: ok");
}

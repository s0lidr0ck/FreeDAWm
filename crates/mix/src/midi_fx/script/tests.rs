use super::super::{MidiFxHost, print};
use super::*;
use soundcraft_dsp::{Plugin, PluginInfo};
use soundcraft_model::{MidiFx, MidiFxKind, Session};
use soundcraft_time::Range;

/// A script compiled and kept as a track keeps it, with `values` set.
fn script(src: &str, values: &[(&str, f64)]) -> MidiScript {
    let p = compile(src).unwrap_or_else(|e| panic!("compiles: {e}"));
    MidiScript {
        id: 1,
        name: "t".into(),
        source: src.into(),
        values: values.iter().map(|(n, v)| (n.to_string(), *v)).collect(),
        runtime: ScriptRuntime { program: Some(p), made_from: None, error: None },
    }
}

fn example(name: &str) -> &'static str {
    EXAMPLES.iter().find(|e| e.0 == name).map(|e| e.1).unwrap()
}

fn fx() -> MidiFx {
    MidiFx { script: 1, ..MidiFx::new(MidiFxKind::Script) }
}

/// What the effect makes of `events` over a second at 120 bpm (a quarter is 24,000 samples).
fn run(sc: &MidiScript, events: &[(i64, [u8; 3])]) -> Vec<(i64, [u8; 3])> {
    print(&Session::default(), &[fx()], std::slice::from_ref(sc), Range::new(0, 96_000), events)
}

fn program(sc: &MidiScript) -> Arc<Program> {
    program_of(&sc.runtime).unwrap()
}

#[test]
fn transpose_moves_notes_and_ends_each_where_it_began() {
    let sc = script(example("Transpose"), &[("Semitones", 7.0)]);
    assert_eq!(program(&sc).params(), &[ParamDecl { name: "Semitones".into(), min: -24.0, max: 24.0, default: 0.0 }]);
    let out =
        run(&sc, &[(1_000, [0x90, 60, 100]), (2_000, [0xB0, 1, 64]), (10_000, [0x80, 60, 0]), (20_000, [0x90, 125, 90]), (21_000, [0x80, 125, 0])]);
    assert_eq!(out, vec![(1_000, [0x90, 67, 100]), (2_000, [0xB0, 1, 64]), (10_000, [0x80, 67, 0])], "the controller passes; 125 + 7 is left out");
    // Unset, the default (0) holds.
    let sc = script(example("Transpose"), &[]);
    assert_eq!(run(&sc, &[(0, [0x90, 60, 100]), (5, [0x80, 60, 0])]), vec![(0, [0x90, 60, 100]), (5, [0x80, 60, 0])]);
    // Out of its range, a value is held to it.
    let sc = script(example("Transpose"), &[("Semitones", 99.0)]);
    assert_eq!(run(&sc, &[(0, [0x90, 60, 100])]).first(), Some(&(0, [0x90, 84, 100])));
}

#[test]
fn velocity_curve_bends_and_bounds_velocities() {
    let notes = [(0, [0x90, 60, 64]), (100, [0x80, 60, 0])];
    let vel = |vals: &[(&str, f64)]| run(&script(example("Velocity Curve"), vals), &notes).first().map(|e| e.1[2]);
    assert_eq!(vel(&[]), Some(64), "a straight line by default");
    let louder = vel(&[("Curve", 0.8)]).unwrap();
    let softer = vel(&[("Curve", -0.8)]).unwrap();
    assert!(louder > 64 && softer < 64, "{louder} {softer}");
    assert_eq!(vel(&[("Lowest", 100.0)]), Some(100 + ((64.0 / 127.0) * 27.0f64).round() as u8));
    assert_eq!(vel(&[("Highest", 20.0)]), Some(1 + ((64.0 / 127.0) * 19.0f64).round() as u8));
    // The note-off passes as it was.
    assert_eq!(run(&script(example("Velocity Curve"), &[("Curve", 0.8)]), &notes).get(1), Some(&(100, [0x80, 60, 0])));
}

#[test]
fn the_arpeggio_steps_through_held_keys_on_the_grid() {
    let sc = script(example("Arpeggio"), &[]);
    // A C major chord held for a beat: four steps of a sixteenth (6,000 samples).
    let keys =
        [(0, [0x90, 67, 100]), (0, [0x90, 60, 100]), (0, [0x90, 64, 100]), (24_000, [0x80, 60, 0]), (24_000, [0x80, 64, 0]), (24_000, [0x80, 67, 0])];
    let out = run(&sc, &keys);
    let ons: Vec<(i64, u8)> = out.iter().filter(|e| e.1[0] == 0x90).map(|e| (e.0, e.1[1])).collect();
    assert_eq!(ons, vec![(0, 60), (6_000, 64), (12_000, 67), (18_000, 60)], "{out:?}");
    let offs: Vec<(i64, u8)> = out.iter().filter(|e| e.1[0] == 0x80).map(|e| (e.0, e.1[1])).collect();
    assert_eq!(offs, vec![(3_000, 60), (9_000, 64), (15_000, 67), (21_000, 60)], "half a step each");
}

#[test]
fn the_echo_repeats_notes_later_and_softer() {
    let sc = script(example("Note Echo"), &[("Echoes", 2.0), ("Time (beats)", 0.5), ("Decay", 0.5)]);
    let out = run(&sc, &[(0, [0x90, 60, 100]), (6_000, [0x80, 60, 0])]);
    assert_eq!(
        out,
        vec![
            (0, [0x90, 60, 100]),
            (6_000, [0x80, 60, 0]),
            (12_000, [0x90, 60, 50]),
            (18_000, [0x80, 60, 0]),
            (24_000, [0x90, 60, 25]),
            (30_000, [0x80, 60, 0])
        ]
    );
    // An echo landing on the note's own end: the end goes first, the echo sounds.
    let out = run(&sc, &[(0, [0x90, 60, 100]), (12_000, [0x80, 60, 0])]);
    let at: Vec<[u8; 3]> = out.iter().filter(|e| e.0 == 12_000).map(|e| e.1).collect();
    assert_eq!(at, vec![[0x80, 60, 0], [0x90, 60, 50]]);
}

#[test]
fn an_endless_loop_is_cut_off_and_the_script_bypassed() {
    for src in [
        "fn handle(e) { loop { } }",
        "fn handle(e) { send(e); }\nfn process(info) { let x = 0; while true { x += 1; } }",
        "fn f(n) { f(n + 1) }\nfn handle(e) { f(0); }",
    ] {
        let sc = script(src, &[]);
        let start = std::time::Instant::now();
        let keys = [(0, [0x90, 60, 100]), (30_000, [0x80, 60, 0]), (40_000, [0x90, 62, 90]), (50_000, [0x80, 62, 0])];
        let out = run(&sc, &keys);
        assert!(start.elapsed() < std::time::Duration::from_secs(5), "{src}: cut off quickly");
        // Everything passes unchanged; the note sounding when it failed (it came out of the script)
        // is let go at the next block, as the effect stops being the script.
        let mut want = keys.to_vec();
        want.insert(1, (256, [0x80, 60, 0]));
        assert_eq!(out, want, "{src}: everything passes unchanged");
        let p = program(&sc);
        assert!(p.failed(), "{src}");
        let err = p.status().error.unwrap();
        assert!(!err.is_empty(), "{src}: says why");
    }
    // A loop at the top is cut off while compiling.
    let e = compile("let x = 0; loop { x += 1; }").unwrap_err();
    assert!(e.contains("operations") || e.contains("Too many"), "{e}");
}

#[test]
fn a_bad_script_never_crashes_and_says_where() {
    let e = compile("fn handle(e) {\n  send(e)\n  let = ;\n}").unwrap_err();
    assert!(e.contains("line"), "{e}");
    // An error while running: bypassed from then on, the notes it left on let go.
    let sc = script("fn handle(e) { if e.pitch == 62 { let a = [1]; a[5].foo(); } send(e); }", &[]);
    let out = run(&sc, &[(0, [0x90, 60, 100]), (24_000, [0x90, 62, 100]), (30_000, [0x80, 62, 0]), (48_000, [0x90, 64, 100])]);
    assert_eq!(out.first(), Some(&(0, [0x90, 60, 100])));
    assert!(out.contains(&(24_000, [0x90, 62, 100])), "the failing event passes: {out:?}");
    assert!(out.contains(&(48_000, [0x90, 64, 100])), "so does everything after: {out:?}");
    assert!(out.iter().any(|e| e.1 == [0x80, 60, 0]), "the note it played is let go: {out:?}");
    let err = program(&sc).status().error.unwrap();
    assert!(err.contains("handle") && err.contains("line 1"), "{err}");
    // Unknown functions, bad parameters, too many parameters, an unknown parameter.
    assert!(compile("nope();").is_err());
    assert!(compile(r#"param("A", 5, 1, 0);"#).is_err());
    assert!(compile(&(0..9).map(|i| format!("param(\"p{i}\", 0, 1, 0);")).collect::<String>()).is_err());
    let sc = script(r#"fn handle(e) { if param("missing") > 0 { send(e); } }"#, &[]);
    run(&sc, &[(0, [0x90, 60, 100])]);
    assert!(program(&sc).failed());
    // Nothing reaches outside: no eval, no modules (no files), no unknown host functions.
    assert!(compile(r#"eval("1 + 1")"#).is_err());
    assert!(compile(r#"import "file" as f;"#).is_err());
    assert!(compile(r#"fn handle(e) { open_file("x"); }"#).is_ok_and(|p| !p.failed()), "it compiles; it would fail when called");
    // Huge strings and arrays stop at their limits.
    let sc = script(r#"fn handle(e) { let s = "x"; while true { s += s; } }"#, &[]);
    run(&sc, &[(0, [0x90, 60, 100])]);
    assert!(program(&sc).failed());
    // Events with nonsense in them are left out, not sent as something else.
    let sc = script(
        r#"fn handle(e) { e.kind = "sneeze"; send(e); send(#{kind: "note_on", pitch: 300}); send(#{kind: "cc", number: 7, value: 999, channel: 40}); }"#,
        &[],
    );
    let out = run(&sc, &[(0, [0x90, 60, 100])]);
    assert_eq!(out, vec![(0, [0xBF, 7, 127])]);
}

#[test]
fn scripts_see_the_transport_and_print_to_the_log() {
    let sc = script(r#"fn handle(e) { print(`${e.kind} ${e.beat} ${tempo()} ${playing()}`); send(e); }"#, &[]);
    run(&sc, &[(12_000, [0x90, 60, 100])]);
    let log = program(&sc).status().log;
    assert!(log.len() == 1 && log[0].starts_with("note_on 0.5") && log[0].ends_with(" 120.0 true"), "{log:?}");
}

/// An instrument that writes down what it is given: (block, offset, message).
struct Ear(Vec<(usize, usize, [u8; 3])>, usize);

impl Plugin for Ear {
    fn info(&self) -> &'static PluginInfo {
        &crate::track_midi::INFO
    }
    fn prepare(&mut self, _sample_rate: f32, _max_block: usize, _channels: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {
        self.1 += 1;
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.0.push((self.1, offset, msg));
    }
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.0.push((self.1, offset, [0x90, note, velocity]));
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.0.push((self.1, offset, [0x80, note, 0]));
    }
    fn host_wrapper(&mut self) -> Option<&mut dyn std::any::Any> {
        Some(self)
    }
}

/// Live, what the script makes of a block comes out in the next block at the same offset.
#[test]
fn live_the_script_is_heard_one_block_later() {
    let sc = script(example("Transpose"), &[("Semitones", 12.0)]);
    let mut host = MidiFxHost::new(Box::new(Ear(Vec::new(), 0)), 48_000.0);
    host.set_max_block(256);
    host.configure(&[fx()], std::slice::from_ref(&sc));
    host.set_transport(&Transport { tempo: 120.0, ..Transport::default() });
    let mut io: Vec<Vec<f32>> = Vec::new();
    host.note_on(10, 60, 100);
    host.process(&mut io, 256);
    // The worker answers within a block's time here; wait for it as the device would.
    for _ in 0..200 {
        std::thread::sleep(std::time::Duration::from_millis(2));
        if program(&sc).slots.iter().any(|s| s.from_worker.head.load(Ordering::Acquire) != s.from_worker.tail.load(Ordering::Acquire)) {
            break;
        }
    }
    host.note_off(20, 60);
    host.process(&mut io, 256);
    for _ in 0..4 {
        std::thread::sleep(std::time::Duration::from_millis(20));
        host.process(&mut io, 256);
    }
    let ear = host.inner.host_wrapper().and_then(|a| a.downcast_mut::<Ear>()).map(|e| e.0.clone()).unwrap();
    assert_eq!(ear.first(), Some(&(1, 10, [0x90, 72, 100])), "block 1, offset 10: {ear:?}");
    assert!(ear.contains(&(2, 20, [0x80, 72, 0])), "{ear:?}");
    // Its parameters change without a reset; a bypass passes at once.
    let mut moved = sc.clone();
    moved.values = vec![("Semitones".into(), -12.0)];
    host.configure(&[fx()], std::slice::from_ref(&moved));
    assert!(host.script(0).is_some_and(|p| Arc::ptr_eq(p, &program(&sc))), "the same program runs");
    host.configure(&[MidiFx { bypass: true, ..fx() }], std::slice::from_ref(&moved));
    host.note_on(5, 61, 90);
    host.process(&mut io, 256);
    let ear = host.inner.host_wrapper().and_then(|a| a.downcast_mut::<Ear>()).map(|e| e.0.clone()).unwrap();
    assert_eq!(ear.last(), Some(&(6, 5, [0x90, 61, 90])), "{ear:?}");
}

/// A Script effect with no program (none written yet) passes everything.
#[test]
fn a_script_effect_without_a_program_passes() {
    let empty = MidiScript { id: 1, name: String::new(), source: String::new(), values: Vec::new(), runtime: ScriptRuntime::default() };
    let keys = [(0, [0x90, 60, 100]), (100, [0x80, 60, 0])];
    assert_eq!(run(&empty, &keys), keys.to_vec());
    assert_eq!(print(&Session::default(), &[MidiFx::new(MidiFxKind::Script)], &[], Range::new(0, 1_000), &keys), keys.to_vec());
}

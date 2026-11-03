//! The audio path never allocates. There is no counting allocator in this crate: everything the runtime touches
//! per block is a fixed-size field or a buffer reserved in `Band::new` / `BandRuntime::new`, so (as in the Vint /
//! Comp tests) every buffer's address and capacity must be the same after 2000 blocks of the smoke scenario as
//! before: `pending`, `carry`, the musicians and each musician's queues.

mod common;

use soundcraft_band::library::load::embedded_library;
use soundcraft_band::midi_types::{MidiEvent, MidiSink};
use soundcraft_band::runtime::{BandRuntime, BandSettings, HostTransport, Retired};
use soundcraft_band::song::build_timeline;

/// A sink that keeps nothing (counts only).
#[derive(Default)]
struct Count(usize);

impl MidiSink for Count {
    fn event(&mut self, _e: &MidiEvent) {
        self.0 += 1;
    }
}

fn smoke() -> (common::Scenario, BandSettings) {
    let bytes = std::fs::read(common::crate_dir().join("tests/scenarios/smoke-factory-44.json")).unwrap_or_default();
    match common::read(&bytes).and_then(|s| common::settings(&s, embedded_library()).map(|b| (s, b))) {
        Ok(x) => x,
        Err(e) => panic!("smoke-factory-44: {e}"),
    }
}

/// Block `i`'s transport: the smoke scenario's moves (a seek back, a tempo and a meter change, a stop and a restart).
fn transport(i: usize, ppq: &mut f64) -> HostTransport {
    let phase = i % 1000;
    let (playing, bpm, num, den) = match phase {
        0..=599 => (true, 120.0, 4, 4),
        600..=799 => (true, 96.0, 3, 4),
        800..=819 => (false, 96.0, 3, 4),
        _ => (true, 96.0, 4, 4),
    };
    if phase == 300 {
        *ppq = 8.0; // a seek back
    }
    let t = HostTransport { playing, bpm, ppq: *ppq, num, den };
    if playing {
        *ppq += 512.0 / 48000.0 * bpm / 60.0;
    }
    t
}

#[test]
fn process_block_does_not_allocate() {
    let (s, settings) = smoke();
    let mut rt = BandRuntime::new(s.sr, settings);
    let before = rt.buffer_addresses();
    let mut out = Count::default();
    let mut ppq = 0.0;
    for i in 0..2000 {
        let t = transport(i, &mut ppq);
        rt.process_block(&t, 512, &mut out);
        if i == 1500 {
            rt.prepare(48000.0); // a re-prepare: the carry-over note-offs go out on the next block
        }
    }
    assert_eq!(before, rt.buffer_addresses(), "a buffer moved or grew: the audio path allocated");
}

#[test]
fn apply_does_not_allocate() {
    let (s, settings) = smoke();
    let base_song = settings.song.clone();
    let mut rt = BandRuntime::new(s.sr, settings.clone());
    let before = rt.buffer_addresses();
    // Two songs to swap between and the parameter moves the smoke scenario makes, all built before the loop.
    let other = build_timeline(&soundcraft_band::song::LegacySong::default().to_song());
    let mut variants = Vec::new();
    for i in 0..8 {
        let mut v = settings.clone();
        v.set_param("m1_energy", (i % 4 + 1) as f32);
        v.set_param("take", (i % 3 + 1) as f32);
        v.feel.bass_lock = i % 2 == 0;
        if i % 4 == 3 {
            v.song = Some(other.clone());
        }
        variants.push(v);
    }
    drop(settings);
    let mut out = Count::default();
    let mut ppq = 0.0;
    let mut retired = Retired::default();
    let mut retired_count = 0;
    for i in 0..2000 {
        if i % 7 == 0 {
            let v = variants[i % variants.len()].clone(); // the clone is the caller's (off the audio thread)
            rt.apply(v, &mut retired);
            if i % 21 == 0 {
                // the caller empties Retired (off the audio thread) only now and then: a full one defers the settings
                retired_count += retired.songs.iter().flatten().count();
                retired = Retired::default();
            }
        }
        let t = transport(i, &mut ppq);
        rt.process_block(&t, 512, &mut out);
    }
    assert_eq!(before, rt.buffer_addresses(), "a buffer moved or grew: apply or the audio path allocated");
    assert!(retired_count > 0, "swapped-out songs come back through Retired");
    // Arc conservation: every handle that went in comes back out (through Retired, or with the runtime); none leaked.
    assert_eq!(rt.leaked_arcs(), 0);
    drop((variants, retired, rt));
    assert_eq!(std::sync::Arc::strong_count(&other), 1, "a swapped-out song was leaked");
    assert!(base_song.as_ref().is_none_or(|a| std::sync::Arc::strong_count(a) == 1), "the scenario's song was leaked");
}

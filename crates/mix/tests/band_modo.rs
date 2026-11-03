//! MODO DRUM and MODO BASS 2, end to end: a session built with the engine's commands plays the
//! factory song through `MixEngine::render` into two recording instruments named "MODO DRUM" and
//! "MODO BASS 2", and each hears exactly what MIDI Studio's own processor played for its musician
//! (`crates/band/tests/refs/t21-modo-e2e.events`, made by `cargo xtask band-refs t21-modo-e2e`).
//!
//! A missing or stale ref prints `SKIPPED` and passes, unless `FIDELITY_STRICT=1`.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use soundcraft_band::library::{LibraryData, TargetData};
use soundcraft_dsp::{Category, Plugin, PluginInfo, Transport};
use soundcraft_engine::Engine;
use soundcraft_mix::{BandSongCache, MixEngine, PreparedInstance, SlotKey, auto_target, prepare_band_cached};
use soundcraft_model::{Route, Session, TrackId};
use soundcraft_time::Samples;

const SCENARIO: &str = "t21-modo-e2e";
const SR: f64 = 48_000.0;
const BLOCK: usize = 512;
const BPM: f64 = 120.0;
const BARS: f64 = 16.0;
/// The scenario's one event: the drummer goes from Energy 2 (cross-stick) to 3 (open-hat barks) at bar 9.
const ENERGY_AT_PPQ: f64 = 32.0;

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

static MODO_DRUM: PluginInfo = info("clap:test.modo_drum", "MODO DRUM");
static MODO_BASS: PluginInfo = info("clap:test.modo_bass_2", "MODO BASS 2");

const fn info(id: &'static str, name: &'static str) -> PluginInfo {
    PluginInfo {
        id,
        name,
        short_name: "MODO",
        category: Category::Instrument,
        params: &[],
        is_instrument: true,
        audiosuite: false,
        sidechain: false,
        link: None,
        references: 0,
        midi_out: false,
    }
}

/// A recording instrument: logs every band event (`midi_raw`) at its absolute sample.
struct Fake {
    info: &'static PluginInfo,
    pos: Samples,
    log: Log,
}

impl Plugin for Fake {
    fn info(&self) -> &'static PluginInfo {
        self.info
    }
    fn prepare(&mut self, _sr: f32, _mb: usize, _ch: usize) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _v: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn process(&mut self, _io: &mut [Vec<f32>], _frames: usize) {}
    fn set_transport(&mut self, t: &Transport) {
        self.pos = t.samples;
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, msg));
    }
}

fn ok(e: &mut Engine, id: &str, p: Value) -> Value {
    e.execute(id, &p).unwrap_or_else(|err| panic!("{id} {p}: {err}"))
}

/// An instrument track playing musician `n` (1-6) through a fake instrument, made with commands.
fn musician_track(e: &mut Engine, info: &'static PluginInfo, n: u8) -> (TrackId, PreparedInstance, Log) {
    let r = ok(e, "track.new", json!({"kind": "instrument", "name": info.name, "instrument": info.id}));
    let t = TrackId(r["tracks"][0].as_u64().expect("a track id"));
    ok(e, "track.input", json!({"tracks": [t.0], "input": format!("musician:{n}")}));
    let log: Log = Arc::default();
    let plugin = Fake { info, pos: 0, log: Arc::clone(&log) };
    (t, PreparedInstance { track: t, slot: SlotKey::Instrument, id: info.id.into(), ch: 2, plugin: Box::new(plugin) }, log)
}

fn band_dir() -> PathBuf {
    std::env::var_os("CARGO_MANIFEST_DIR").map(PathBuf::from).unwrap_or_default().join("../band/tests")
}

fn strict() -> bool {
    std::env::var_os("FIDELITY_STRICT").is_some_and(|v| v == "1")
}

/// FNV-1a 64 of the scenario's bytes: the ref's `# case` line (as `xtask band-refs` writes it).
fn case_line(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for &b in bytes {
        h ^= u64::from(b);
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("# case {h:016x}")
}

/// One ref line: `<block> <offset> <status> <d1> <d2>`.
struct RefEvent {
    block: i64,
    offset: i64,
    bytes: [u8; 3],
}

/// The ref's events, or why there are none (no ref, or a stale one).
fn read_ref(scenario: &[u8]) -> Result<Vec<RefEvent>, String> {
    let path = band_dir().join("refs").join(format!("{SCENARIO}.events"));
    let text = std::fs::read_to_string(&path).map_err(|_| format!("no ref {}", path.display()))?;
    let mut lines = text.lines();
    if lines.next() != Some(case_line(scenario).as_str()) {
        return Err("stale ref (made from other scenario bytes)".into());
    }
    let mut out = Vec::new();
    for l in lines.filter(|l| !l.is_empty()) {
        let n: Vec<i64> = l.split(' ').map(|x| x.parse().unwrap_or_else(|_| panic!("ref line `{l}`"))).collect();
        let byte = |i: usize| u8::try_from(n[i]).unwrap_or_else(|_| panic!("ref line `{l}`"));
        assert_eq!(n.len(), 5, "ref line `{l}`");
        out.push(RefEvent { block: n[0], offset: n[1], bytes: [byte(2), byte(3), byte(4)] });
    }
    Ok(out)
}

/// What musician `k` (0-based) plays in the ref, as its instrument hears it: its own channel
/// (`k+1`) rewritten to channel 1.
fn expected(reference: &[RefEvent], k: u8) -> Vec<(i64, [u8; 3])> {
    reference
        .iter()
        .filter(|e| e.bytes[0] < 0xF0 && e.bytes[0] & 0x0F == k)
        .map(|e| (e.block * BLOCK as i64 + e.offset, [e.bytes[0] & 0xF0, e.bytes[1], e.bytes[2]]))
        .collect()
}

fn note_ons(log: &[(i64, [u8; 3])], note: i32) -> Vec<i64> {
    log.iter().filter(|(_, b)| b[0] == 0x90 && b[2] > 0 && i32::from(b[1]) == note).map(|(at, _)| *at).collect()
}

fn cc_values(log: &[(i64, [u8; 3])], cc: i32) -> Vec<u8> {
    log.iter().filter(|(_, b)| b[0] == 0xB0 && i32::from(b[1]) == cc).map(|(_, b)| b[2]).collect()
}

fn update(e: &Engine, eng: &MixEngine, lib: &Arc<LibraryData>, cache: &mut BandSongCache) -> Box<soundcraft_mix::BandUpdate> {
    prepare_band_cached(e.session(), lib, &|t| eng.instrument_name(t), cache).expect("musician inputs")
}

#[test]
fn modo_drum_and_bass_play_end_to_end() {
    let bytes = std::fs::read(band_dir().join("scenarios").join(format!("{SCENARIO}.json"))).expect("the scenario");
    // The session below is this scenario; check the parts it copies.
    let sc: Value = serde_json::from_slice(&bytes).expect("scenario JSON");
    assert_eq!(sc["song"], "Pop Verse Chorus Bridge");
    assert_eq!((sc["bars"].as_f64(), sc["sample_rate"].as_f64(), sc["block"].as_u64()), (Some(BARS), Some(SR), Some(BLOCK as u64)));
    assert_eq!(sc["feel"], json!({"natural": true}));
    assert_eq!(sc["players"], json!([null, "Jay", "Walt", null, null, null]));
    assert_eq!(sc["targets"], json!([null, "MODO DRUM", "MODO BASS 2", null, null, null]));
    assert_eq!(sc["params"], json!({"m2_energy": 2, "m3_energy": 2, "m3_fills": 0}));
    assert_eq!(sc["events"], json!([{"at_ppq": 32, "param": "m2_energy", "value": 3}]));
    let reference = match read_ref(&bytes) {
        Ok(r) => r,
        Err(why) => {
            println!("SKIPPED {SCENARIO}: {why} (cargo xtask band-refs {SCENARIO})");
            assert!(!strict(), "{SCENARIO}: {why} (FIDELITY_STRICT=1)");
            return;
        }
    };

    // The session, through commands: the factory song on the lanes, Natural, Jay and Walt with
    // "auto" targets, and two tracks whose instruments name the MODO targets.
    let mut e = Engine::default();
    assert_eq!((e.session().sample_rate.as_f64(), e.session().tempo.tempo_at_tick(0)), (SR, BPM));
    ok(&mut e, "band.song.factory", json!({"name": "Pop Verse Chorus Bridge", "at": 0}));
    ok(&mut e, "band.settings", json!({"feel": {"natural": true}}));
    ok(&mut e, "band.slot", json!({"slot": 2, "player": "Jay", "target": "auto", "energy": 2}));
    ok(&mut e, "band.slot", json!({"slot": 3, "player": "Walt", "target": "auto", "energy": 2, "fills": 0}));
    let (drum_t, drum, drum_log) = musician_track(&mut e, &MODO_DRUM, 2);
    let (bass_t, bass, bass_log) = musician_track(&mut e, &MODO_BASS, 3);
    let s: &Session = e.session();
    assert_eq!(s.track(drum_t).map(|t| t.mixer.input.clone()), Some(Route::Musician(1)));
    assert_eq!(s.track(bass_t).map(|t| t.mixer.input.clone()), Some(Route::Musician(2)));

    let lib = e.band_library();
    let mut eng = MixEngine::new(SR as f32, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(vec![drum, bass]);
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    // The instances are placed by the first render's sync; the names come from them.
    let mut cache = BandSongCache::default();
    let names_before = (eng.instrument_name(drum_t), eng.instrument_name(bass_t));
    assert_eq!(names_before, (None, None), "no instance before the first render");

    // band_ref's stepping: the play head advances by a block's quarters (accumulated, as the
    // ref did); the Energy event applies on the first block starting at or after its ppq; once
    // the 16 bars are played, one block with the transport stopped.
    let end_ppq = BARS * 4.0;
    let mut ppq = 0.0f64;
    let mut block: i64 = 0;
    let mut energy_done = false;
    let mut u = None;
    let mut stop_at = 0;
    loop {
        if !energy_done && ppq >= ENERGY_AT_PPQ {
            ok(&mut e, "band.slot", json!({"slot": 2, "energy": 3}));
            eng.adopt_band(update(&e, &eng, &lib, &mut cache));
            energy_done = true;
        }
        let playing = ppq < end_ppq;
        eng.monitor_only = !playing;
        if !playing {
            stop_at = block * BLOCK as i64;
        }
        if block == 0 {
            // Place the instances (no band yet), then adopt the band: its targets come from them.
            eng.render(e.session(), -(BLOCK as i64), BLOCK, &mut out);
            let first = update(&e, &eng, &lib, &mut cache);
            u = Some(first.settings.clone());
            eng.adopt_band(first);
        }
        eng.render(e.session(), block * BLOCK as i64, BLOCK, &mut out);
        block += 1;
        if !playing {
            break;
        }
        ppq += BLOCK as f64 / SR * BPM / 60.0;
    }
    assert!(energy_done);

    // auto_target picked both MODO targets from the instruments' names.
    let settings = u.expect("the band");
    let target = |k: usize| -> &TargetData {
        let slot = settings.param(&format!("m{}_target", k + 1)).expect("a target") as i32;
        lib.target(slot).expect("a Library target")
    };
    assert_eq!(e.session().band.slots[1].target, "auto");
    assert_eq!(e.session().band.slots[2].target, "auto");
    assert_eq!((target(1).name.as_str(), target(2).name.as_str()), ("MODO DRUM", "MODO BASS 2"));
    assert_eq!(auto_target(&lib, "MODO DRUM") as i32, lib.target_slot_for_name("MODO DRUM"));
    assert_eq!(auto_target(&lib, "MODO BASS 2") as i32, lib.target_slot_for_name("MODO BASS 2"));

    // Each instrument heard exactly its musician's ref events, on channel 1.
    let drums = drum_log.lock().unwrap().clone();
    let bass = bass_log.lock().unwrap().clone();
    for (who, got, k) in [("drums", &drums, 1u8), ("bass", &bass, 2)] {
        let want = expected(&reference, k);
        if let Some(i) = got.iter().zip(&want).position(|(g, w)| g != w) {
            let from = i.saturating_sub(3);
            panic!(
                "{who}: first mismatch at event {i}\n  got  {:?}\n  want {:?}",
                &got[from..(i + 3).min(got.len())],
                &want[from..(i + 3).min(want.len())]
            );
        }
        assert!(got.len() >= want.len(), "{who}: {} events heard, {} in the ref", got.len(), want.len());
        // After the ref's last event, only the DAW's own all-notes-off on stop (spec: "the DAW's
        // all-notes-off on locate and stop"): CC64 0 / CC123 0 at the stop.
        let daw_stop = &got[want.len()..];
        let reset = |b: &[u8; 3]| b[0] == 0xB0 && matches!(b[1], 64 | 123) && b[2] == 0;
        assert!(daw_stop.iter().all(|(at, b)| *at == stop_at && reset(b)), "{who}: more than the ref: {daw_stop:?}");
        assert!(got.iter().all(|(_, b)| b[0] & 0x0F == 0), "{who}: everything on channel 1");
    }

    // Natural feel's MODO DRUM extras: cross-stick (Energy 2), and from bar 9 (Energy 3) open-hat
    // barks closed by the pedal hat.
    let dt = target(1);
    let change = (ENERGY_AT_PPQ * 60.0 / BPM * SR) as i64;
    assert!(!note_ons(&drums, dt.dr_cross_stick).is_empty(), "a cross-stick ({})", dt.dr_cross_stick);
    assert!(!note_ons(&drums, dt.dr_pedal_hat).is_empty(), "a pedal hat ({})", dt.dr_pedal_hat);
    let barks = note_ons(&drums, dt.dr_open_shank);
    assert!(!barks.is_empty(), "an open-hat bark ({})", dt.dr_open_shank);
    assert!(barks.iter().all(|&at| at >= change), "barks only at Energy 3+");

    // MODO BASS 2: a keyswitch note (below the playable range), vibrato and let ring.
    let bt = target(2);
    let keyswitches = [bt.kb_finger, bt.kb_pick, bt.kb_slap, bt.kb_ghost, bt.kb_hammer, bt.kb_stop, bt.kb_down];
    assert!(keyswitches.iter().any(|&n| !note_ons(&bass, n).is_empty()), "a keyswitch note ({keyswitches:?})");
    assert!(cc_values(&bass, bt.kb_vibrato_cc).iter().any(|&v| v > 0), "vibrato (CC{})", bt.kb_vibrato_cc);
    assert!(cc_values(&bass, bt.kb_let_ring_cc).iter().any(|&v| v > 0), "let ring (CC{})", bt.kb_let_ring_cc);
}

//! The Band in the mix engine: musician inputs, transport, hand-off, no allocation.
//!
//! The instrument is a recording fake whose `midi_raw` logs `(absolute sample, bytes)`. Only the
//! keys musician (slot 0, Piano) plays: the other slots are switched off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use soundcraft_band::library::{LibraryData, embedded_library};
use soundcraft_band::midi_types::VecSink;
use soundcraft_band::runtime::BandRuntime;
use soundcraft_band::song_file::factory_songs;
use soundcraft_dsp::{Plugin, PluginInfo, Transport};
use soundcraft_mix::{
    BandUpdate, MixEngine, PreparedInstance, RangeRender, SlotBuf, SlotKey, auto_target, band_song_end, band_transport, lanes_from_song, prepare_band,
};
use soundcraft_model::{ChannelFormat, Clip, Insert, Route, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;
const TAPE: &str = "clap:test.band_tape";

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

/// Logs the band's events (`midi_raw`) at their absolute sample; clip notes (`note_on`) are
/// logged with the status 0x00 so a test can tell them apart.
struct Tape {
    inner: Box<dyn Plugin>,
    pos: Samples,
    log: Log,
    /// Where `all_notes_off` was called.
    resets: Arc<Mutex<Vec<i64>>>,
}

impl Plugin for Tape {
    fn info(&self) -> &'static PluginInfo {
        self.inner.info()
    }
    fn prepare(&mut self, sr: f32, mb: usize, ch: usize) {
        self.inner.prepare(sr, mb, ch);
    }
    fn reset(&mut self) {
        self.inner.reset();
    }
    fn set_param(&mut self, id: &str, v: f32) -> bool {
        self.inner.set_param(id, v)
    }
    fn param(&self, id: &str) -> Option<f32> {
        self.inner.param(id)
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        self.inner.process(io, frames);
    }
    fn set_transport(&mut self, t: &Transport) {
        self.pos = t.samples;
    }
    fn note_on(&mut self, offset: usize, note: u8, _velocity: u8) {
        self.log.lock().unwrap().push((self.pos + offset as i64, [0x00, note, 0]));
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, msg));
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, msg));
    }
    fn all_notes_off(&mut self) {
        self.resets.lock().unwrap().push(self.pos);
    }
}

fn tape(t: TrackId) -> (PreparedInstance, Log) {
    let (inst, log, _) = tape_with_resets(t);
    (inst, log)
}

fn tape_with_resets(t: TrackId) -> (PreparedInstance, Log, Arc<Mutex<Vec<i64>>>) {
    let log: Log = Arc::default();
    let resets: Arc<Mutex<Vec<i64>>> = Arc::default();
    let plugin = Tape { inner: soundcraft_dsp::create("gain").unwrap(), pos: 0, log: Arc::clone(&log), resets: Arc::clone(&resets) };
    (PreparedInstance { track: t, slot: SlotKey::Instrument, id: TAPE.into(), ch: 2, plugin: Box::new(plugin) }, log, resets)
}

fn lib() -> Arc<LibraryData> {
    embedded_library()
}

/// A factory song laid onto the lanes at `at`, and the keys musician alone.
fn lay_song(s: &mut Session, at: Samples) {
    let songs = factory_songs();
    let blues = songs.iter().find(|f| f.name == "12-bar Blues").expect("the blues");
    let (defs, blocks, chords) = lanes_from_song(s, &blues.song, at);
    s.band.defs = defs;
    s.band.sections = blocks;
    s.band.chords = chords;
    for (k, m) in s.band.slots.iter_mut().enumerate() {
        m.enabled = k == 0;
    }
}

fn musician_track(s: &mut Session, k: u8) -> TrackId {
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new(TAPE));
    tr.mixer.input = Route::Musician(k);
    t
}

fn engine(s: &Session, instances: Vec<PreparedInstance>) -> MixEngine {
    let mut eng = MixEngine::new(SR, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(instances);
    eng.adopt_band(prepare_band(s, &lib(), &|_| None).expect("a musician input"));
    eng
}

fn render(eng: &mut MixEngine, s: &Session, pos: Samples) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    eng.render(s, pos, BLOCK, &mut out);
}

fn is_on(b: &[u8; 3]) -> bool {
    b[0] & 0xF0 == 0x90 && b[2] > 0
}
fn is_off(b: &[u8; 3]) -> bool {
    b[0] & 0xF0 == 0x80 || (b[0] & 0xF0 == 0x90 && b[2] == 0)
}

/// Notes still held after the events up to `until` (exclusive).
fn held(log: &[(i64, [u8; 3])], until: i64) -> usize {
    let mut on: HashMap<(u8, u8), i32> = HashMap::new();
    for (at, b) in log.iter().filter(|(at, _)| *at < until) {
        let _ = at;
        let key = (b[0] & 0x0F, b[1]);
        if is_on(b) {
            *on.entry(key).or_default() += 1;
        } else if is_off(b) {
            let n = on.entry(key).or_default();
            *n = (*n - 1).max(0);
        }
    }
    on.values().map(|&n| n as usize).sum()
}

#[test]
fn musician_input_plays_the_band_not_the_clips() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    let mut seq = soundcraft_midi::Sequence::default();
    seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 480 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 96_000, seq));
    let (inst, log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    let u = prepare_band(&s, &lib(), &|_| None).unwrap();
    let mut reference = BandRuntime::new(f64::from(SR), u.settings.clone());
    let mut want = Vec::new();
    let blocks = 10 * 48_000 / BLOCK; // inside the 24 s song
    for b in 0..blocks {
        let pos = (b * BLOCK) as i64;
        render(&mut eng, &s, pos);
        let mut sink = VecSink::default();
        reference.process_block(&band_transport(&s, pos, true), BLOCK, &mut sink);
        // SlotSink's routing: by musician; an event with none by its channel (k+1), unless lower.
        let to_slot_0 = |e: &soundcraft_band::midi_types::MidiEvent| e.musician == 0 || (e.musician < 0 && !e.lower && e.status & 0x0F == 1);
        for e in sink.events.iter().filter(|e| to_slot_0(e) && !e.rendered) {
            let status = if e.lower { e.status } else { e.status & 0xF0 };
            want.push((pos + i64::from(e.offset), [status, e.d1, e.d2]));
        }
    }
    let got = log.lock().unwrap().clone();
    assert!(got.iter().all(|(_, b)| b[0] != 0x00), "no clip note reaches a musician input");
    assert_eq!(got, want);
    assert!(!got.is_empty(), "the instrument heard the band");
}

#[test]
fn two_tracks_on_one_musician_both_play() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let a = musician_track(&mut s, 0);
    let b = musician_track(&mut s, 0);
    let (ia, la) = tape(a);
    let (ib, lb) = tape(b);
    let mut eng = engine(&s, vec![ia, ib]);
    for blk in 0..400 {
        render(&mut eng, &s, (blk * BLOCK) as i64);
    }
    let (la, lb) = (la.lock().unwrap().clone(), lb.lock().unwrap().clone());
    assert_eq!(la, lb);
    assert!(!la.is_empty(), "the instruments heard the band");
}

#[test]
fn stop_locate_and_loop_close_band_notes() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    let (inst, log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    let mut pos = 0i64;
    let play_to = |eng: &mut MixEngine, pos: &mut i64, end: i64| {
        while *pos < end {
            render(eng, &s, *pos);
            *pos += BLOCK as i64;
        }
    };
    let n = |log: &Log| log.lock().unwrap().len();
    let held_now = |log: &Log| held(&log.lock().unwrap(), i64::MAX);

    // Stop mid-song: the block of the stop closes everything.
    play_to(&mut eng, &mut pos, 4 * 48_000);
    let before_stop = n(&log);
    eng.monitor_only = true;
    render(&mut eng, &s, pos);
    assert_eq!(held_now(&log), 0, "a stop closes the band's notes in its block");
    render(&mut eng, &s, pos);
    eng.monitor_only = false;

    // Locate back: the block of the jump closes what the band held before it.
    play_to(&mut eng, &mut pos, 7 * 48_000);
    pos = 2 * 48_000;
    let mark = n(&log);
    render(&mut eng, &s, pos);
    let jump = log.lock().unwrap().clone();
    let mut all = jump[..mark].to_vec();
    all.extend(jump[mark..].iter().filter(|(at, b)| *at == pos && is_off(b)).cloned());
    assert_eq!(held(&all, i64::MAX), 0, "a locate closes the band's notes at the jump");
    pos += BLOCK as i64;

    // A loop wrap (6 s back to 4 s) is a locate too.
    play_to(&mut eng, &mut pos, 6 * 48_000);
    let mark = n(&log);
    pos = 4 * 48_000;
    render(&mut eng, &s, pos);
    let wrap = log.lock().unwrap().clone();
    let mut all = wrap[..mark].to_vec();
    all.extend(wrap[mark..].iter().filter(|(at, b)| *at == pos && is_off(b)).cloned());
    assert_eq!(held(&all, i64::MAX), 0, "a loop wrap closes the band's notes");
    pos += BLOCK as i64;

    // And nothing is left held at the end.
    play_to(&mut eng, &mut pos, 8 * 48_000);
    eng.monitor_only = true;
    render(&mut eng, &s, pos);
    assert_eq!(held_now(&log), 0);
    assert!(before_stop > 0, "the instrument heard the band");
}

#[test]
fn tempo_and_meter_changes_follow_the_session() {
    let mut s = Session::default();
    let bar = |s: &Session, n: i64| s.tempo.tick_to_samples(s.tempo.bar_start_tick(n), s.sample_rate);
    s.tempo.set_tempo(s.tempo.bar_start_tick(5), 90.0).unwrap();
    let start = bar(&s, 9);
    lay_song(&mut s, start);
    // A 7/8 bar inside the song: the band keeps the tick-0 meter (the lanes' grid).
    let b11 = s.tempo.bar_start_tick(11);
    s.tempo.set_meter(b11, 7, 8).unwrap();
    let b12 = s.tempo.bar_start_tick(12);
    s.tempo.set_meter(b12, 4, 4).unwrap();
    let t = musician_track(&mut s, 0);
    let (inst, log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    let end = band_song_end(&s).expect("the lanes end");
    assert!(end > start);
    let u = prepare_band(&s, &lib(), &|_| None).unwrap();
    assert_eq!((u.start, u.song_end), (Some(start), Some(end)));

    // The band's play head: silent outside the lanes, the session's tempo and tick-0 meter inside.
    assert!(!band_transport(&s, start - 1, true).playing);
    let at = band_transport(&s, start, true);
    assert!(at.playing);
    assert_eq!((at.bpm, at.num, at.den), (90.0, 4, 4));
    assert!(!band_transport(&s, end, true).playing);
    assert!(!band_transport(&s, start + 48_000, false).playing);
    let seven = band_transport(&s, bar(&s, 11), true);
    assert_eq!((seven.num, seven.den, seven.playing), (4, 4, true));
    assert!((at.ppq - s.tempo.samples_to_ticks_f(start, s.sample_rate) / 960.0).abs() < 1e-9);

    let mut pos = 0;
    while pos < end + 2 * 48_000 {
        render(&mut eng, &s, pos);
        pos += BLOCK as i64;
    }
    let got = log.lock().unwrap().clone();
    // Before bar 9 the band only resets its controllers (sustain off, all notes off) as the
    // processor does on its first block; no note, nothing else.
    let reset = |b: &[u8; 3]| b[0] & 0xF0 == 0xB0 && (b[1] == 64 || b[1] == 123) && b[2] == 0;
    assert!(got.iter().filter(|(at, _)| *at < start).all(|(_, b)| reset(b)), "nothing before bar 9 ({start})");
    assert!(got.iter().filter(|(_, b)| is_on(b)).all(|(at, _)| *at < end), "no note starts after the lanes end");
    assert!(got.iter().all(|(at, _)| *at <= end), "the notes close at the lanes' end");
    assert_eq!(held(&got, i64::MAX), 0);
    assert!(!got.is_empty(), "the instrument heard the band");
}

#[test]
fn band_render_does_not_allocate() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    let (inst, _log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    render(&mut eng, &s, 0);
    let first = eng.band_buffers();
    assert!(first.len() > BAND_COUNT);
    for (i, b) in eng.band_out().iter().enumerate() {
        assert_eq!(b.events.capacity(), soundcraft_mix::SLOT_CAPACITY, "slot {i}");
    }
    let mut changed = s.clone();
    changed.band.key = 5;
    changed.band.slots[0].energy = 4;
    let update = prepare_band(&changed, &lib(), &|_| None).unwrap();
    let mut update = Some(update);
    for blk in 1..2000 {
        let pos = (blk * BLOCK) as i64;
        if blk == 1000 {
            eng.adopt_band(update.take().unwrap());
            let _ = eng.take_retired_band();
            let _ = eng.take_spent_band();
        }
        let sess = if blk >= 1000 { &changed } else { &s };
        render(&mut eng, sess, pos);
        assert_eq!(eng.band_buffers(), first, "block {blk}");
    }
}

const BAND_COUNT: usize = soundcraft_mix::BAND_SLOTS;

#[test]
fn auto_target_picks_by_name() {
    let lib = lib();
    let name = |p: &str| lib.target(auto_target(&lib, p) as i32).map(|t| t.name.clone()).unwrap();
    assert_eq!(name("MODO DRUM"), "MODO DRUM");
    assert_eq!(name("MODO BASS 2"), "MODO BASS 2");
    assert_eq!(name("Pianoteq 8 STAGE"), "Pianoteq");
    assert_eq!(name("Serum"), "Generic");
    assert_eq!(name(""), "Generic");

    // prepare_band picks an "auto" target from the instrument on the musician's first track.
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    musician_track(&mut s, 0);
    let names = move |x: TrackId| (x == t).then(|| "Pianoteq 8 STAGE".to_string());
    let u = prepare_band(&s, &lib, &names).unwrap();
    let slot = u.settings.param("m1_target").unwrap();
    assert_eq!(lib.target(slot as i32).unwrap().name, "Pianoteq");
    let ks = lib.target(slot as i32).unwrap().ks_octave_default;
    assert_eq!(u.settings.param("m1_ksoctave"), Some(s.band.slots[0].ksoctave.map_or(ks, i32::from) as f32));
    assert_eq!(u.settings.param("m1_channel"), Some(1.0));
    assert_eq!(u.settings.param("m6_channel"), Some(6.0));

    // No musician input: no update.
    let mut plain = Session::default();
    lay_song(&mut plain, 0);
    assert!(prepare_band(&plain, &lib, &|_| None).is_none());
}

#[test]
fn bounce_includes_the_band() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("subtractive_synth"));
    tr.mixer.input = Route::Musician(0);
    let range = Range::new(0, 4 * 48_000);
    let mut rr = RangeRender::new(&s, None, range, 1024);
    assert!(rr.adopt_band_library(&s, &lib()));
    assert!(rr.has_band());
    let mut peak = 0.0f32;
    while let Some((chunk, n)) = rr.next_block(&s) {
        for c in chunk {
            peak = c.iter().take(n).fold(peak, |p, x| p.max(x.abs()));
        }
    }
    // Without the band the track has no clips: silence.
    let silent = soundcraft_mix::render_range(&s, range, 1024);
    assert!(silent.iter().flatten().all(|x| *x == 0.0));
    assert!(peak > 0.0, "the bounce plays the band");
}

#[test]
fn slot_buffer_overflow_counts_and_never_panics() {
    let mut b = SlotBuf::with_capacity(4);
    let cap = b.events.capacity();
    for i in 0..(cap as u32 + 6) {
        b.push(i, [0x91, 60, 100], false);
    }
    assert_eq!(b.events.len(), cap);
    assert_eq!(b.dropped, 6);
    assert_eq!(b.events.capacity(), cap, "never grown");

    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    let (inst, _log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    eng.set_band_slot_capacity(4);
    let caps: Vec<usize> = eng.band_out().iter().map(|b| b.events.capacity()).collect();
    let mut dropped = 0u64;
    for blk in 0..800 {
        render(&mut eng, &s, (blk * BLOCK) as i64);
        for (b, c) in eng.band_out().iter().zip(&caps) {
            assert!(b.events.len() <= *c);
            assert_eq!(b.events.capacity(), *c);
            dropped += u64::from(b.dropped);
        }
    }
    let _: Option<Box<BandUpdate>> = eng.take_spent_band();
    assert!(dropped > 0, "a capacity of 4 drops some of the band's events");
}

#[test]
fn moving_the_input_off_a_musician_closes_its_notes() {
    for next in [Route::None, Route::Musician(1)] {
        let mut s = Session::default();
        lay_song(&mut s, 0);
        let t = musician_track(&mut s, 0);
        let (inst, log, resets) = tape_with_resets(t);
        let mut eng = engine(&s, vec![inst]);
        let mut pos = 0;
        while pos < 3 * 48_000 {
            render(&mut eng, &s, pos);
            pos += BLOCK as i64;
        }
        let before = resets.lock().unwrap().len();
        let held_before = held(&log.lock().unwrap(), i64::MAX);
        let mut moved = s.clone();
        moved.track_mut(t).unwrap().mixer.input = next.clone();
        render(&mut eng, &moved, pos);
        let after = resets.lock().unwrap().clone();
        assert_eq!(after.len(), before + 1, "{next:?}: all notes off when the input moves");
        // Staying put changes nothing.
        render(&mut eng, &moved, pos + BLOCK as i64);
        assert_eq!(resets.lock().unwrap().len(), before + 1);
        assert!(held_before > 0, "{next:?}: the input moved mid-note");
    }
}

/// A 4/4 song turning 3/4 mid-song: the band reads the timeline with the tick-0 meter's halves, as
/// the lanes do, so its events match a runtime driven with 4/4 throughout, through a locate into
/// the 3/4 region too.
#[test]
fn a_mid_song_meter_change_keeps_the_tick_0_meter() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let b4 = s.tempo.bar_start_tick(4);
    s.tempo.set_meter(b4, 3, 4).unwrap();
    let three = s.tempo.tick_to_samples(b4, s.sample_rate);
    let t = musician_track(&mut s, 0);
    let (inst, log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    let u = prepare_band(&s, &lib(), &|_| None).unwrap();
    let mut reference = BandRuntime::new(f64::from(SR), u.settings.clone());
    let tick0 = |pos: Samples| {
        let t = band_transport(&s, pos, true);
        soundcraft_band::runtime::HostTransport { num: 4, den: 4, ..t }
    };
    assert_eq!((band_transport(&s, three + 48_000, true).num, band_transport(&s, three + 48_000, true).den), (4, 4), "the tick-0 meter");
    let to_slot_0 = |e: &soundcraft_band::midi_types::MidiEvent| e.musician == 0 || (e.musician < 0 && !e.lower && e.status & 0x0F == 1);
    let mut want = Vec::new();
    let take = |sink: VecSink, pos: i64, want: &mut Vec<(i64, [u8; 3])>| {
        for e in sink.events.iter().filter(|e| to_slot_0(e) && !e.rendered) {
            let status = if e.lower { e.status } else { e.status & 0xF0 };
            want.push((pos + i64::from(e.offset), [status, e.d1, e.d2]));
        }
    };
    // Play into the 3/4 region, then locate back before it and play through it again.
    let first: Vec<i64> = (0..(three as usize + 4 * 48_000) / BLOCK).map(|b| (b * BLOCK) as i64).collect();
    let jump = 48_000i64;
    let second: Vec<i64> = (0..(three as usize + 4 * 48_000 - jump as usize) / BLOCK).map(|b| jump + (b * BLOCK) as i64).collect();
    let third_at = three + 2 * 48_000; // a locate into the 3/4 region
    let third: Vec<i64> = (0..2 * 48_000 / BLOCK).map(|b| third_at + (b * BLOCK) as i64).collect();
    for (i, run) in [first, second, third].into_iter().enumerate() {
        for (j, pos) in run.into_iter().enumerate() {
            render(&mut eng, &s, pos);
            let mut sink = VecSink::default();
            if i > 0 && j == 0 {
                reference.flush(&mut sink);
            }
            reference.process_block(&tick0(pos), BLOCK, &mut sink);
            take(sink, pos, &mut want);
        }
    }
    let got = log.lock().unwrap().clone();
    assert!(got.iter().any(|(at, b)| *at > three && is_on(b)), "the band plays in the 3/4 region");
    assert_eq!(got, want);
}

/// A saved player who, after a Library change, is gone or no longer plays the musician's
/// instrument: the musician gets the instrument's first player (MS `setPlayer`), never the stale
/// seed.
#[test]
fn a_stale_player_falls_back_to_the_instruments_first() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    musician_track(&mut s, 0);
    let inst = usize::from(s.band.slots[0].instrument);
    let base = lib();
    let list = base.players[inst].clone();
    assert!(list.len() >= 2, "the instrument has players to choose from");
    let x = list[1];
    let name = base.personas.iter().find(|p| p.seed == x).unwrap().name.clone();
    s.band.slots[0].player = name.clone();
    s.band.slots[0].player_seed = u16::try_from(x).unwrap();
    // As saved, the player plays.
    assert_eq!(prepare_band(&s, &base, &|_| None).unwrap().settings.seeds[0], x);

    // The Library changes: the player no longer plays this instrument.
    let mut changed = (*base).clone();
    changed.players[inst].retain(|&p| p != x);
    let changed = Arc::new(changed);
    let first = {
        let mut st = soundcraft_band::runtime::BandSettings::processor_default(Arc::clone(&changed));
        st.set_param("m1_instrument", f32::from(s.band.slots[0].instrument));
        st.set_player(0, "");
        st.seeds[0]
    };
    assert_ne!(first, x);
    assert_eq!(prepare_band(&s, &changed, &|_| None).unwrap().settings.seeds[0], first, "not playing the instrument");

    // Or the name is gone and the seed is stale.
    let mut gone = (*changed).clone();
    for p in gone.personas.iter_mut().filter(|p| p.seed == x) {
        p.name = "Someone Else".into();
    }
    assert_eq!(prepare_band(&s, &Arc::new(gone), &|_| None).unwrap().settings.seeds[0], first, "missing");
}

/// The input back on the clips mid-play: the clips' controllers are chased there, as after a
/// locate.
#[test]
fn back_on_the_clips_chases_their_controllers() {
    let mut s = Session::default();
    lay_song(&mut s, 0);
    let t = musician_track(&mut s, 0);
    let mut seq = soundcraft_midi::Sequence::default();
    seq.ctrls.push(soundcraft_midi::CtrlEvent { tick: 0, channel: 0, kind: soundcraft_midi::CtrlKind::Cc(7), value: 99 });
    let cid = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(cid, "m", 0, 10 * 48_000, seq));
    let (inst, log) = tape(t);
    let mut eng = engine(&s, vec![inst]);
    let mut pos = 0;
    while pos < 2 * 48_000 {
        render(&mut eng, &s, pos);
        pos += BLOCK as i64;
    }
    assert!(!log.lock().unwrap().iter().any(|(_, b)| *b == [0xB0, 7, 99]), "the band plays, not the clip");
    let mut clips = s.clone();
    clips.track_mut(t).unwrap().mixer.input = Route::None;
    render(&mut eng, &clips, pos);
    let got = log.lock().unwrap().clone();
    assert!(got.contains(&(pos, [0xB0, 7, 99])), "the clip's volume is chased where the input returns");
}

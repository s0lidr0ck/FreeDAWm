//! `band.print`: the band printed into MIDI clips on the tracks that play a musician, which then
//! play what the band played.
//!
//! The instrument is a recording fake that logs what reaches it through `note_on`, `note_off` and
//! `midi` at its absolute sample: the band's events arrive through the default `midi_raw` routing,
//! the clips' through clip playback, so both are logged the same way. Only the keys musician
//! (slot 1, Piano) plays: the other slots are switched off.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use serde_json::json;
use soundcraft_band::library::{LibraryData, embedded_library};
use soundcraft_dsp::{Plugin, PluginInfo, Transport};
use soundcraft_engine::Engine;
use soundcraft_mix::{MixEngine, PreparedInstance, SlotKey, band_song_end, half_ticks, lanes_from_song, prepare_band};
use soundcraft_model::{ChannelFormat, ClipContent, Insert, Route, Session, TrackId, TrackKind};
use soundcraft_time::{Range, Samples, TICKS_PER_QUARTER};

const SR: f32 = 48_000.0;
const BLOCK: usize = 512;
const TAPE: &str = "clap:test.print_tape";

type Log = Arc<Mutex<Vec<(i64, [u8; 3])>>>;

struct Tape {
    inner: Box<dyn Plugin>,
    pos: Samples,
    log: Log,
    /// Log `midi_raw` as it comes (channel kept, as a plugin host does), not through the default
    /// routing into `note_on`/`note_off`/`midi`.
    raw: bool,
}

impl Tape {
    fn push(&self, offset: usize, b: [u8; 3]) {
        self.log.lock().unwrap().push((self.pos + offset as i64, b));
    }
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
    fn note_on(&mut self, offset: usize, note: u8, velocity: u8) {
        self.push(offset, [0x90, note, velocity]);
    }
    fn note_off(&mut self, offset: usize, note: u8) {
        self.push(offset, [0x80, note, 0]);
    }
    fn midi(&mut self, offset: usize, msg: [u8; 3]) {
        self.push(offset, msg);
    }
    fn midi_raw(&mut self, offset: usize, msg: [u8; 3]) {
        if self.raw {
            self.push(offset, msg);
            return;
        }
        let [status, d1, d2] = msg;
        match status & 0xF0 {
            0x90 if d2 > 0 => self.note_on(offset, d1, d2),
            0x80 | 0x90 => self.note_off(offset, d1),
            _ => self.midi(offset, msg),
        }
    }
}

fn tape(t: TrackId, raw: bool) -> (PreparedInstance, Log) {
    let log: Log = Arc::default();
    let plugin = Tape { inner: soundcraft_dsp::create("gain").unwrap(), pos: 0, log: Arc::clone(&log), raw };
    (PreparedInstance { track: t, slot: SlotKey::Instrument, id: TAPE.into(), ch: 2, plugin: Box::new(plugin) }, log)
}

/// A session with the 12-bar Blues on the lanes at 0, the keys musician alone, and one track
/// playing it.
fn session() -> (Session, TrackId) {
    let mut s = Session::default();
    let songs = soundcraft_band::song_file::factory_songs();
    let blues = songs.iter().find(|f| f.name == "12-bar Blues").expect("the blues");
    let (defs, blocks, chords) = lanes_from_song(&s, &blues.song, 0);
    s.band.defs = defs;
    s.band.sections = blocks;
    s.band.chords = chords;
    for (k, m) in s.band.slots.iter_mut().enumerate() {
        m.enabled = k == 0;
    }
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new(TAPE));
    tr.mixer.input = Route::Musician(0);
    (s, t)
}

/// Everything the instrument on `t` hears from sample 0 to `until`.
fn hear(s: &Session, t: TrackId, until: Samples) -> Vec<(i64, [u8; 3])> {
    hear_with(s, t, until, &embedded_library(), &|_| None, false)
}

/// [`hear`] with the Library, the instruments' names and the raw tape given.
fn hear_with(
    s: &Session,
    t: TrackId,
    until: Samples,
    lib: &Arc<LibraryData>,
    names: &dyn Fn(TrackId) -> Option<String>,
    raw: bool,
) -> Vec<(i64, [u8; 3])> {
    let (inst, log) = tape(t, raw);
    let mut eng = MixEngine::new(SR, BLOCK);
    eng.set_external_instances(true);
    eng.adopt(vec![inst]);
    if let Some(u) = prepare_band(s, lib, names) {
        eng.adopt_band(u);
    }
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut pos = 0i64;
    while pos <= until {
        eng.render(s, pos, BLOCK, &mut out);
        pos += BLOCK as i64;
    }
    log.lock().unwrap().clone()
}

/// Channel mode messages (all notes off, reset controllers, …) are the transport's, not the part's.
fn mode_message(b: &[u8; 3]) -> bool {
    b[0] & 0xF0 == 0xB0 && b[1] >= 120
}

/// The events by their bytes, each with its samples in order.
fn by_bytes(log: &[(i64, [u8; 3])]) -> BTreeMap<[u8; 3], Vec<i64>> {
    let mut m: BTreeMap<[u8; 3], Vec<i64>> = BTreeMap::new();
    for (at, b) in log {
        m.entry(*b).or_default().push(*at);
    }
    for v in m.values_mut() {
        v.sort_unstable();
    }
    m
}

/// One tick, in samples (rounded up), at the session's tempo.
fn one_tick(s: &Session) -> i64 {
    s.tempo.tick_to_samples(TICKS_PER_QUARTER, s.sample_rate) / TICKS_PER_QUARTER + 1
}

fn assert_same(s: &Session, want: &[(i64, [u8; 3])], got: &[(i64, [u8; 3])]) {
    let (want, got) = (by_bytes(want), by_bytes(got));
    assert_eq!(want.keys().collect::<Vec<_>>(), got.keys().collect::<Vec<_>>(), "the same messages");
    let tol = one_tick(s);
    for (b, w) in &want {
        let g = &got[b];
        assert_eq!(w.len(), g.len(), "how many {b:02X?}");
        for (x, y) in w.iter().zip(g) {
            assert!((x - y).abs() <= tol, "{b:02X?} at {x} printed at {y}");
        }
    }
}

/// Notes still sounding after the events before `until`.
fn held(log: &[(i64, [u8; 3])], until: i64) -> usize {
    let mut on: BTreeMap<u8, usize> = BTreeMap::new();
    for (_, b) in log.iter().filter(|(at, _)| *at < until) {
        match b[0] {
            0x90 => *on.entry(b[1]).or_default() += 1,
            0x80 => {
                on.remove(&b[1]);
            }
            _ => {}
        }
    }
    on.values().sum()
}

fn track_clips(s: &Session, t: TrackId) -> Vec<&soundcraft_model::Clip> {
    s.track(t).unwrap().clips().iter().collect()
}

#[test]
fn printed_clips_play_what_the_band_played() {
    let (s, t) = session();
    let end = band_song_end(&s).expect("a song");
    let live = hear(&s, t, end + 48_000);
    let mut e = Engine::new(s);
    let r = e.execute("band.print", &json!({})).unwrap();
    let clips = r["clips"].as_array().unwrap();
    assert_eq!(clips.len(), 1, "{r}");
    assert_eq!(clips[0]["track"], json!(t.0));
    assert!(clips[0]["notes"].as_u64().unwrap() > 0);
    let printed = e.session_arc();
    assert_eq!(printed.track(t).unwrap().mixer.input, Route::None);
    let cs = track_clips(&printed, t);
    assert_eq!(cs.len(), 1);
    assert_eq!((cs[0].start, cs[0].end()), (0, end));
    let got = hear(&printed, t, end + 48_000);

    // Nothing is left sounding after the clip.
    assert_eq!(held(&got, i64::MAX), 0, "no stuck note");
    // Up to the song's end, and the note-offs on it: the notes the band closes there, the clip
    // closes on its end (the band's other resets there are outside the range).
    let keep = |log: &[(i64, [u8; 3])]| {
        log.iter().filter(|(at, b)| (*at < end || (*at == end && b[0] & 0xF0 == 0x80)) && !mode_message(b)).copied().collect::<Vec<_>>()
    };
    let (want, got) = (keep(&live), keep(&got));
    assert!(want.iter().any(|(_, b)| b[0] == 0x90), "the band played");
    assert_same(&printed, &want, &got);
}

#[test]
fn print_a_section_block_only() {
    let (s, t) = session();
    // Bars 5-8 as a block of their own.
    let bar = s.tempo.tick_to_samples(4 * TICKS_PER_QUARTER, s.sample_rate);
    let section = s.band.sections.first().unwrap().section.to_string();
    let mut e = Engine::new(s);
    let r = e.execute("band.section.add", &json!({"section": section, "start": 4 * bar, "end": 8 * bar})).unwrap();
    let id = r["block"].as_u64().unwrap() as u32;
    let s = e.session().clone();
    let blk = s.band.sections.iter().find(|b| b.id == id).cloned().unwrap();
    let a = s.tempo.tick_to_samples(blk.start_tick, s.sample_rate);
    let b = s.tempo.tick_to_samples(blk.start_tick + blk.length_ticks, s.sample_rate);
    assert!(a > 0 && blk.start_tick % half_ticks(&s) == 0);
    let live = hear(&s, t, b + 48_000);
    e.execute("band.print", &json!({"block": blk.id, "slots": [1]})).unwrap();
    let printed = e.session_arc();
    let cs = track_clips(&printed, t);
    assert_eq!(cs.len(), 1);
    assert_eq!((cs[0].start, cs[0].end()), (a, b));
    let ClipContent::Midi { sequence } = &cs[0].content else { panic!("a MIDI clip") };
    assert!(!sequence.notes.is_empty());
    let len = printed.tempo.samples_to_ticks(b, printed.sample_rate) - printed.tempo.samples_to_ticks(a, printed.sample_rate);
    assert!(sequence.notes.iter().all(|n| n.start >= 0 && n.end() <= len), "inside the clip");
    // The block's note-ons, and nothing else's.
    let ons = |log: &[(i64, [u8; 3])]| log.iter().filter(|(at, x)| *at >= a && *at < b && x[0] == 0x90).copied().collect::<Vec<_>>();
    let got = hear(&printed, t, b + 48_000);
    assert!(got.iter().filter(|(_, x)| x[0] == 0x90).all(|(at, _)| *at >= a && *at < b), "nothing plays outside the block");
    assert_same(&printed, &ons(&live), &ons(&got));
}

#[test]
fn print_is_one_undo_step() {
    let (s, t) = session();
    let mut e = Engine::new(s);
    let before = e.session_arc();
    let undo = e.undo_history().len();
    e.execute("band.print", &json!({})).unwrap();
    assert_eq!(e.undo_history().len(), undo + 1);
    assert_eq!(e.undo_label(), Some("Print Band"));
    let after = e.session_arc();
    assert!(e.undo());
    assert_eq!(e.session().track(t).unwrap().mixer.input, Route::Musician(0));
    assert!(track_clips(e.session(), t).is_empty());
    assert_eq!(*e.session(), *before);
    assert!(e.redo());
    assert_eq!(e.session().track(t).unwrap().mixer.input, Route::None);
    assert_eq!(track_clips(e.session(), t).len(), 1);
    assert_eq!(*e.session(), *after);
}

#[test]
fn print_with_no_musician_inputs_is_refused() {
    let (mut s, t) = session();
    s.track_mut(t).unwrap().mixer.input = Route::None;
    let mut e = Engine::new(s);
    let undo = e.undo_history().len();
    let err = e.execute("band.print", &json!({})).unwrap_err().to_string();
    assert!(err.contains("No track plays a musician"), "{err}");
    assert_eq!(e.undo_history().len(), undo);

    // Musicians that no track plays are refused the same way.
    let (s, _) = session();
    let mut e = Engine::new(s);
    let err = e.execute("band.print", &json!({"slots": [2, 3]})).unwrap_err().to_string();
    assert!(err.contains("No track plays a musician"), "{err}");
}

#[test]
fn a_range_past_the_song_prints_what_the_song_plays() {
    let (s, t) = session();
    let end = band_song_end(&s).unwrap();
    let mut e = Engine::new(s);
    let far = 1_000_000_000_000i64;
    let r = e.execute("band.print", &json!({"range": {"start": 0, "end": far}})).unwrap();
    assert!(r["clips"][0]["notes"].as_u64().unwrap() > 0);
    let cs = track_clips(e.session(), t);
    let ClipContent::Midi { sequence } = &cs[0].content else { panic!("a MIDI clip") };
    assert!(sequence.notes.iter().all(|n| e.session().tempo.tick_to_samples(n.end(), e.session().sample_rate) <= end));
}

/// Note-offs as one form (`0x8n p 0`): the band sends `0x9n p 0` or `0x8n p v`, a printed note
/// keeps only a release velocity.
fn offs_as_one(log: &[(i64, [u8; 3])]) -> Vec<(i64, [u8; 3])> {
    log.iter()
        .map(|&(at, b)| match b[0] & 0xF0 {
            0x80 => (at, [b[0], b[1], 0]),
            0x90 if b[2] == 0 => (at, [0x80 | (b[0] & 0x0F), b[1], 0]),
            _ => (at, b),
        })
        .collect()
}

/// The organ's lower manual is not played by the factory Library yet (Task 20 ports it), so its
/// events are given here as the band delivers them: a printed clip keeps their channel and plays
/// them there; every other event goes on channel 1.
#[test]
fn a_printed_organ_plays_its_lower_manual_on_its_channel() {
    let (mut s, t) = session();
    let (tempo, sr) = (s.tempo.clone(), s.sample_rate);
    let at = |ticks: i64| tempo.tick_to_samples(ticks, sr);
    // (sample, bytes, lower): upper notes on the musician's channel 2, the lower manual on 3.
    let events = [
        (at(0), [0xB1, 11, 90], false),
        (at(0), [0x91, 60, 100], false),
        (at(0), [0x92, 48, 100], true),
        (at(480), [0x81, 60, 0], false),
        (at(960), [0x82, 48, 0], true),
    ];
    let end = at(1920);
    let seq = soundcraft_engine::band_print::events_to_sequence(&s, Range::new(0, end), &events);
    assert_eq!(seq.notes.iter().map(|n| (n.pitch, n.channel)).collect::<Vec<_>>(), vec![(48, 2), (60, 0)]);
    assert_eq!(seq.ctrls.iter().map(|c| c.channel).collect::<Vec<_>>(), vec![0]);
    let id = s.new_clip_id();
    let mut clip = soundcraft_model::Clip::midi(id, "printed", 0, end, seq);
    clip.midi_channels = true;
    let tr = s.track_mut(t).unwrap();
    tr.mixer.input = Route::None;
    tr.playlist_mut().unwrap().clips.push(clip);
    let got = offs_as_one(&hear_with(&s, t, end + 48_000, &embedded_library(), &|_| None, true));
    let got: Vec<(i64, [u8; 3])> = got.into_iter().filter(|(_, b)| !mode_message(b)).collect();
    // The controller first on its tick, then the notes; the lower manual on channel 3.
    assert_eq!(got, vec![(0, [0xB0, 11, 90]), (0, [0x92, 48, 100]), (0, [0x90, 60, 100]), (at(480), [0x80, 60, 0]), (at(960), [0x82, 48, 0])]);
}

#[test]
fn print_names_instruments_as_playback_does() {
    let (s, t) = session();
    let lib = embedded_library();
    // A third-party instrument's name picks the "auto" target, live and printed alike.
    let pianoteq = |_: TrackId| Some("Pianoteq 8 STAGE".to_string());
    let target = |names: &dyn Fn(TrackId) -> Option<String>| prepare_band(&s, &lib, names).unwrap().settings.param("m1_target");
    assert_ne!(target(&pianoteq), target(&|_| None), "the name picks a target of its own");
    let end = band_song_end(&s).unwrap();
    let cut = end;
    let live = hear_with(&s, t, end, &lib, &pianoteq, false);
    let live_ons: Vec<(i64, [u8; 3])> = live.iter().filter(|(at, b)| b[0] == 0x90 && *at < cut).copied().collect();
    let seqs = soundcraft_engine::band_print::render_print(&s, &lib, &pianoteq, Range::new(0, end), &[0]);
    let ons: Vec<(i64, [u8; 3])> = seqs[0]
        .notes
        .iter()
        .map(|n| (s.tempo.tick_to_samples(n.start, s.sample_rate), [0x90, n.pitch, n.velocity]))
        .filter(|(at, _)| *at < cut)
        .collect();
    assert!(!ons.is_empty());
    assert_same(&s, &live_ons, &ons);

    // The command names a built-in instrument from the registry, as playback does.
    let (mut s, t) = session();
    s.track_mut(t).unwrap().instrument = Some(Insert::new("subtractive_synth"));
    let name = soundcraft_dsp::plugin_info("subtractive_synth").unwrap().name.to_string();
    let want = soundcraft_engine::band_print::render_print(&s, &lib, &|_| Some(name.clone()), Range::new(0, end), &[0]);
    let mut e = Engine::new(s);
    e.execute("band.print", &json!({})).unwrap();
    let ClipContent::Midi { sequence } = &track_clips(e.session(), t)[0].content else { panic!("a MIDI clip") };
    assert_eq!(*sequence, want[0]);
}

#[test]
fn print_reports_what_it_replaced_and_refuses_an_empty_range() {
    let (mut s, t) = session();
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(soundcraft_model::Clip::midi(id, "old", 0, 48_000, Default::default()));
    let end = band_song_end(&s).unwrap();
    let mut e = Engine::new(s);
    // After the song the band is silent: nothing to print, nothing changed.
    let undo = e.undo_history().len();
    let err = e.execute("band.print", &json!({"range": {"start": end + 48_000, "end": end + 96_000}})).unwrap_err().to_string();
    assert!(err.contains("Nothing to print in that range"), "{err}");
    assert_eq!(e.undo_history().len(), undo);
    assert_eq!(e.session().track(t).unwrap().mixer.input, Route::Musician(0));
    // A range start between ticks is snapped to one; the clip the print lays over is reported.
    let r = e.execute("band.print", &json!({"range": {"start": 13, "end": end}})).unwrap();
    assert_eq!(r["replaced"], json!([{"track": t.0, "clip": id.0}]));
    let s = e.session();
    assert_eq!(r["start"].as_i64().unwrap(), s.tempo.tick_to_samples(s.tempo.samples_to_ticks(13, s.sample_rate), s.sample_rate));
}

/// What the instrument hears from `a` on when playback runs from the song's start: its controller
/// state at `a` (the last value of each controller, the program, a bend off centre), then the
/// events after it, without the note-offs of notes that started before `a`.
fn heard_from(log: &[(i64, [u8; 3])], a: i64, end: i64) -> Vec<(i64, [u8; 3])> {
    let mut state: BTreeMap<(u8, u8), [u8; 3]> = BTreeMap::new();
    for (_, b) in log.iter().filter(|(at, b)| *at < a && !mode_message(b)) {
        match b[0] & 0xF0 {
            0xB0 => {
                state.insert((b[0], b[1]), *b);
            }
            0xC0 | 0xE0 => {
                state.insert((b[0], 0), *b);
            }
            _ => {}
        }
    }
    let mut out: Vec<(i64, [u8; 3])> = state.into_values().filter(|b| *b != [0xE0, 0, 64]).map(|b| (a, b)).collect();
    let mut on: BTreeMap<(u8, u8), usize> = BTreeMap::new();
    for &(at, b) in log.iter().filter(|(at, b)| *at >= a && *at < end && !mode_message(b)) {
        let key = (b[0] & 0x0F, b[1]);
        match b[0] & 0xF0 {
            0x90 if b[2] > 0 => *on.entry(key).or_default() += 1,
            0x80 | 0x90 => match on.get_mut(&key) {
                Some(n) if *n > 0 => *n -= 1,
                _ => continue, // its note-on was before `a`
            },
            _ => {}
        }
        out.push((at, b));
    }
    out
}

/// A print that starts mid-song starts with the band's state there: played from its start, the
/// instrument hears what it heard from that point when the band played the song.
#[test]
fn a_print_from_mid_song_starts_with_the_bands_state() {
    let (mut s, t) = session();
    // A target with controllers set before the print's start.
    let lib = embedded_library();
    s.band.slots[0].target = "Crow Hill Pocket Strings".into();
    let end = band_song_end(&s).unwrap();
    let bar = s.tempo.tick_to_samples(4 * TICKS_PER_QUARTER, s.sample_rate);
    let a = 5 * bar;
    let live = offs_as_one(&hear_with(&s, t, end, &lib, &|_| None, true));
    let mut e = Engine::new(s);
    e.execute("band.print", &json!({"range": {"start": a, "end": end}})).unwrap();
    let printed = e.session_arc();
    let cs = track_clips(&printed, t);
    assert_eq!((cs[0].start, cs[0].end()), (a, end));
    let ClipContent::Midi { sequence } = &cs[0].content else { panic!("a MIDI clip") };
    assert!(sequence.ctrls.iter().any(|c| c.tick == 0), "the state on the first tick");
    let got = offs_as_one(&hear_with(&printed, t, end, &lib, &|_| None, true));
    let want = heard_from(&live, a, end);
    let got: Vec<(i64, [u8; 3])> = got.into_iter().filter(|(at, b)| *at >= a && *at < end && !mode_message(b)).collect();
    assert!(want.iter().any(|(_, b)| b[0] & 0xF0 == 0x90), "the band played");
    assert!(want.iter().any(|(at, b)| *at == a && b[0] & 0xF0 == 0xB0), "a controller set before the start");
    assert_same(&printed, &want, &got);
}

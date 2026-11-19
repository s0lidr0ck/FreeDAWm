//! ARA stage two: clips heard through the plug-in. The test ARA plug-in renders each playback
//! region as its source times its modification's gain ([`DEFAULT_GAIN`] until changed), so what
//! playback through its voices must give is known: the same session with the file scaled by that
//! gain and no plug-in. Checked at several block sizes, for bounces (offline renders), after
//! moves, trims, splits, duplicates, undo and redo, across a save and reopen (the plug-in's own
//! edit restored from its archive), without the plug-in (its archive kept), and with plug-ins
//! that misbehave (bypassed, the clip playing its own audio).

use serde_json::json;
use soundcraft_ara_host::test_plugin::{self, Behaviour, DEFAULT_GAIN, factory_ptr, report};
use soundcraft_audio_io::AudioBuffer;
use soundcraft_dsp::{Category, Plugin, PluginEditor, PluginInfo, Transport};
use soundcraft_engine::Engine;
use soundcraft_engine::ara::{AraHost, Factory, InstanceSource};
use soundcraft_mix::MixEngine;
use soundcraft_mix::ara::AraBoard;
use soundcraft_model::{ChannelFormat, Clip, ClipAra, ClipId, Session, Source, SourceAudio, SourceId, TrackKind};
use soundcraft_time::Range;
use std::sync::Arc;

const PLUGIN: &str = "test:good";
const RATE: u32 = 48_000;

static INFO: PluginInfo = PluginInfo {
    id: "test.ara_voice",
    name: "Test ARA voice",
    short_name: "ARA",
    category: Category::Other,
    params: &[],
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The test plug-in's instance as a FreeDAWm plug-in: renders its regions at the transport
/// position.
struct TestVoice {
    renderer: usize,
    rate: f64,
    pos: i64,
}

impl Plugin for TestVoice {
    fn info(&self) -> &'static PluginInfo {
        &INFO
    }
    fn prepare(&mut self, sample_rate: f32, _max_block: usize, _channels: usize) {
        self.rate = f64::from(sample_rate);
    }
    fn reset(&mut self) {}
    fn set_param(&mut self, _id: &str, _value: f32) -> bool {
        false
    }
    fn param(&self, _id: &str) -> Option<f32> {
        None
    }
    fn set_transport(&mut self, t: &Transport) {
        self.pos = t.samples;
    }
    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let mut outs: Vec<&mut [f32]> = io.iter_mut().map(|c| &mut c[..frames]).collect();
        test_plugin::render(self.renderer, self.pos, self.rate, &mut outs);
    }
    fn editor(&mut self) -> Option<Box<dyn PluginEditor>> {
        Some(Box::new(TestEditor(false)))
    }
    fn save_state(&mut self) -> Option<Vec<u8>> {
        Some(b"window layout".to_vec())
    }
    fn load_state(&mut self, data: &[u8]) -> bool {
        data == b"window layout"
    }
}

/// The editor window of a test instance: opens and closes, shows nothing.
struct TestEditor(bool);

impl PluginEditor for TestEditor {
    fn open(&mut self) -> Result<(), String> {
        self.0 = true;
        Ok(())
    }
    fn close(&mut self) {
        self.0 = false;
    }
    fn is_open(&self) -> bool {
        self.0
    }
    fn idle(&mut self) -> Vec<(String, f32)> {
        Vec::new()
    }
}

impl Drop for TestVoice {
    fn drop(&mut self) {
        test_plugin::unbind(self.renderer);
    }
}

fn factories(behaviour: Behaviour) -> soundcraft_engine::ara::FactorySource {
    Box::new(
        move |id| {
            if id == PLUGIN { Ok(Factory { ptr: factory_ptr(behaviour), keep: Box::new(()) }) } else { Err(format!("{id} is not installed")) }
        },
    )
}

fn instances() -> InstanceSource {
    Box::new(|id, dc, known, assigned| {
        if id != PLUGIN {
            return Err(format!("{id} is not installed"));
        }
        let (renderer, raw) = test_plugin::bind_raw(dc, known, assigned);
        Ok((Box::new(TestVoice { renderer, rate: f64::from(RATE), pos: 0 }) as Box<dyn Plugin>, raw))
    })
}

fn host(behaviour: Behaviour) -> AraHost {
    AraHost::with_instances(factories(behaviour), instances())
}

fn signal(frames: usize, seed: f32) -> Vec<f32> {
    (0..frames).map(|i| (i as f32 * 0.0123 + seed).sin() * 0.6 + ((i * 7) % 13) as f32 * 0.01).collect()
}

/// A session with one mono track holding a clip at 500 of a mono file from its sample 2 000, given
/// to the plug-in.
fn session(name: &str) -> (Engine, ClipId) {
    let mut s = Session { name: name.into(), ..Session::default() };
    s.pool.insert(SourceId(900), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: RATE, channels: vec![signal(60_000, 0.3)] })));
    s.sources.push(Source {
        id: SourceId(900),
        name: "vox".into(),
        path: "vox.wav".into(),
        channels: 1,
        frames: 60_000,
        sample_rate: RATE,
        format: soundcraft_audio_io::FileFormat::Wav,
        time_reference: 0,
        unsaved: false,
    });
    // No automatic edge fades: the file's own render (the reference) then has no ramps to step
    // through, so it equals the plug-in's sample for sample. (Bounces equal playback with them.)
    s.edit.values.insert("playback.auto_fade_ms".into(), 0.0);
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let id = s.new_clip_id();
    let mut c = Clip::audio(id, "take", SourceId(900), 2_000, 500, 30_000);
    c.ara = Some(ClipAra { plugin: PLUGIN.into(), name: "Test".into(), origin: Some(id.0), ..ClipAra::default() });
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
    (Engine::new(s), id)
}

/// What the plug-in should make the session sound like: each clip given to it plays its file
/// times its modification's gain (by clip), with no plug-in.
fn expected(s: &Session, gain: impl Fn(ClipId) -> f32, range: Range) -> Vec<Vec<f32>> {
    let mut x = s.clone();
    x.ara_renders = Default::default();
    let mut next = 50_000u64;
    let ids: Vec<ClipId> = x.tracks.iter().flat_map(|t| t.clips()).filter(|c| c.ara.as_ref().is_some_and(|a| !a.bypass)).map(|c| c.id).collect();
    for id in ids {
        let g = gain(id);
        let c = x.find_clip_mut(id).unwrap();
        let src = c.source().unwrap();
        c.ara = None;
        let audio = s.pool.get(src).unwrap();
        let ch: Vec<Vec<f32>> = (0..audio.num_channels())
            .map(|k| {
                let mut v = vec![0.0; audio.frames()];
                audio.read(k, 0, &mut v);
                v.iter().map(|x| x * g).collect()
            })
            .collect();
        next += 1;
        let scaled = SourceId(next);
        x.pool.insert(scaled, Arc::new(SourceAudio::new(AudioBuffer { sample_rate: RATE, channels: ch })));
        if let soundcraft_model::ClipContent::Audio { source, .. } = &mut x.find_clip_mut(id).unwrap().content {
            *source = scaled;
        }
    }
    soundcraft_mix::render_range(&x, range, 512)
}

/// Playback through the realtime engine with the host's voices, in blocks of `block`.
fn play(e: &Engine, h: &mut AraHost, block: usize, range: Range) -> Vec<Vec<f32>> {
    let board = Arc::new(AraBoard::default());
    h.set_realtime(Some(board.clone()), RATE as f32, block);
    h.sync(e);
    let mut m = MixEngine::new(RATE as f32, block);
    m.set_external_instances(true);
    m.set_ara_board(Some(board));
    let s = e.session();
    let ch = soundcraft_mix::main_channels(s);
    let mut out = vec![Vec::new(); ch];
    let mut buf = vec![vec![0.0f32; block]; ch];
    let mut pos = range.start;
    while pos < range.end {
        let n = block.min((range.end - pos) as usize);
        m.render(s, pos, n, &mut buf);
        for (o, b) in out.iter_mut().zip(&buf) {
            o.extend_from_slice(&b[..n]);
        }
        pos += n as i64;
    }
    out
}

fn close(a: &[Vec<f32>], b: &[Vec<f32>], what: &str) {
    assert_eq!(a.len(), b.len(), "{what}: channels");
    for (x, y) in a.iter().zip(b) {
        assert_eq!(x.len(), y.len(), "{what}: length");
        let worst = x.iter().zip(y).map(|(p, q)| (p - q).abs()).fold(0.0f32, f32::max);
        assert!(worst < 1e-5, "{what}: differs by {worst}");
    }
    assert!(a.iter().flatten().any(|v| v.abs() > 0.01), "{what}: silent");
}

fn no_violations(doc: &str) {
    let r = report(doc).unwrap();
    assert!(r.violations.is_empty(), "{doc}: {:?}", r.violations);
    assert_eq!(r.renderer_while_rendering, 0, "{doc}: regions given to an instance while it rendered");
}

const SPAN: Range = Range { start: 0, end: 40_000 };

#[test]
fn playback_is_the_plug_ins_render_at_any_block_size() {
    let (e, id) = session("ara play");
    let want = expected(e.session(), |_| DEFAULT_GAIN, SPAN);
    for block in [64, 333, 1024, 4096] {
        let mut h = host(Behaviour::Good);
        let got = play(&e, &mut h, block, SPAN);
        close(&got, &want, &format!("blocks of {block}"));
        let st = h.status(e.session().find_clip(id).unwrap().1).unwrap();
        assert!(st.contains("plays through the plug-in"), "{st}");
    }
    no_violations("ara play");
}

#[test]
fn a_bounce_equals_playback() {
    let (mut e, _) = session("ara bounce");
    // With the automatic edge fades, and clip gain: both apply to the plug-in's audio alike.
    e.session_mut().edit.values.insert("playback.auto_fade_ms".into(), 5.0);
    e.session_mut().tracks[0].playlist_mut().unwrap().clips[0].gain_db = -3.0;
    let mut h = host(Behaviour::Good);
    let heard = play(&e, &mut h, 512, SPAN);
    assert_eq!(h.prepare_offline(&mut e), 1);
    let bounced = soundcraft_mix::render_range(e.session(), SPAN, 1024);
    assert_eq!(heard, bounced, "the offline render is what playback played");
    // Nothing changed: nothing is rendered again.
    assert_eq!(h.prepare_offline(&mut e), 0);
    // An edit in the plug-in makes the render stale.
    let id = e.session().tracks[0].clips()[0].id;
    assert!(test_plugin::set_gain("ara bounce", &format!("clip-{}", id.0), 0.25));
    h.idle();
    assert_eq!(h.prepare_offline(&mut e), 1);
    let bounced = soundcraft_mix::render_range(e.session(), SPAN, 1024);
    assert_eq!(bounced, play(&e, &mut h, 700, SPAN), "after an edit too");
    let quarter: f32 = bounced[0].iter().map(|x| x.abs()).sum::<f32>() / heard[0].iter().map(|x| x.abs()).sum::<f32>();
    assert!((quarter - 0.5).abs() < 1e-3, "the edit (gain 0.25 for -0.5) is heard: {quarter}");
    // Consolidate makes a plain clip of what the plug-in plays.
    let t = e.session().tracks[0].id;
    e.execute("edit.consolidate", &json!({"tracks": [t.0], "start": 0, "end": 40_000})).unwrap();
    let c = &e.session().tracks[0].clips()[0];
    assert!(c.ara.is_none());
    close(&soundcraft_mix::render_range(e.session(), SPAN, 1024), &bounced, "consolidated");
    no_violations("ara bounce");
}

#[test]
fn edits_are_heard_and_a_duplicate_clones_the_plug_ins_edit() {
    let (mut e, a) = session("ara edits play");
    let mut h = host(Behaviour::Good);
    let t = e.session().tracks[0].id;
    let check = |e: &Engine, h: &mut AraHost, what: &str| {
        let gains: std::collections::HashMap<u64, f32> = e
            .session()
            .tracks
            .iter()
            .flat_map(|t| t.clips())
            .map(|c| (c.id.0, test_plugin::gain("ara edits play", &format!("clip-{}", c.id.0)).unwrap_or(DEFAULT_GAIN)))
            .collect();
        let want = expected(e.session(), |id| gains[&id.0], Range { start: 0, end: 80_000 });
        close(&play(e, h, 256, Range { start: 0, end: 80_000 }), &want, what);
    };
    check(&e, &mut h, "as made");
    e.execute("edit.move_clips", &json!({"clips": [a.0], "to": 6_000})).unwrap();
    check(&e, &mut h, "moved");
    e.execute("edit.trim_clip", &json!({"clip": a.0, "start": 9_000, "alone": true})).unwrap();
    check(&e, &mut h, "trimmed");
    // The plug-in's edit, then a split: both halves keep it.
    assert!(test_plugin::set_gain("ara edits play", &format!("clip-{}", a.0), 0.75));
    h.idle();
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 20_000})).unwrap();
    h.sync(&e);
    let ids: Vec<ClipId> = e.session().tracks[0].clips().iter().map(|c| c.id).collect();
    assert_eq!(ids.len(), 2);
    for id in &ids {
        assert_eq!(test_plugin::gain("ara edits play", &format!("clip-{}", id.0)), Some(0.75), "the split clip {id:?} keeps the edit");
    }
    check(&e, &mut h, "split");
    // Duplicate the second half: the copy's modification is a clone of it.
    let end = e.session().content_end();
    e.execute("edit.duplicate", &json!({"tracks": [t.0], "start": 20_000, "end": end})).unwrap();
    h.sync(&e);
    let all: Vec<ClipId> = e.session().tracks[0].clips().iter().map(|c| c.id).collect();
    assert_eq!(all.len(), 3);
    assert!(report("ara edits play").unwrap().cloned >= 2, "split and duplicate clone");
    check(&e, &mut h, "duplicated");
    for _ in 0..4 {
        e.execute("edit.undo", &json!({})).unwrap();
        check(&e, &mut h, "undone");
    }
    for _ in 0..4 {
        e.execute("edit.redo", &json!({})).unwrap();
        check(&e, &mut h, "redone");
    }
    // Bypassed: the clip plays its own audio.
    e.execute("clip.ara_bypass", &json!({"clips": [all[0].0], "on": true})).unwrap();
    check(&e, &mut h, "one clip bypassed");
    no_violations("ara edits play");
}

fn reopened(e: &Engine) -> Engine {
    let text = serde_json::to_string(e.session()).unwrap();
    let mut s: Session = serde_json::from_str(&text).unwrap();
    s.pool = e.session().pool.clone();
    Engine::new(s)
}

#[test]
fn saving_and_reopening_restores_the_plug_ins_edits_and_a_missing_plug_in_keeps_them() {
    let (mut e, id) = session("ara save");
    let mut h = host(Behaviour::Good);
    h.sync(&e);
    let pid = format!("clip-{}", id.0);
    assert!(test_plugin::set_gain("ara save", &pid, 0.25));
    h.idle();
    assert!(h.take_unsaved(), "the plug-in's edit makes the session want saving");
    h.store_archives(&mut e);
    let saved = e.session().ara_archives.clone();
    assert_eq!(saved.len(), 1);
    assert_eq!(saved[0].archive_id, "org.freedawm.test.ara.archive.1");
    assert_eq!(test_plugin::parse_archive(&saved[0].bytes().unwrap()).unwrap()[&pid], 0.25);
    drop(h);

    // Reopened: the plug-in restores its edit and plays it.
    let e2 = reopened(&e);
    let mut h2 = host(Behaviour::Good);
    let got = play(&e2, &mut h2, 512, SPAN);
    assert_eq!(test_plugin::gain("ara save", &pid), Some(0.25), "restored from the archive");
    assert!(report("ara save").unwrap().restored >= 1);
    close(&got, &expected(e2.session(), |_| 0.25, SPAN), "reopened");
    assert!(h2.take_messages().iter().any(|m| m.contains("restored")));
    no_violations("ara save");
    drop(h2);

    // Without the plug-in: the clip plays its own audio, the archive is kept as it was, and so
    // through another save.
    let mut e3 = reopened(&e);
    let mut h3 = AraHost::with_instances(Box::new(|id| Err(format!("{id} is not installed"))), instances());
    let got = play(&e3, &mut h3, 512, SPAN);
    let own = {
        let mut s = e3.session().clone();
        for t in &mut s.tracks {
            for c in t.playlist_mut().unwrap().clips.iter_mut() {
                c.ara = None;
            }
        }
        soundcraft_mix::render_range(&s, SPAN, 512)
    };
    close(&got, &own, "without the plug-in");
    let st = h3.status(e3.session().find_clip(id).unwrap().1).unwrap();
    assert!(st.contains("cannot be used") && st.contains("kept"), "{st}");
    h3.store_archives(&mut e3);
    assert_eq!(reopened(&e3).session().ara_archives, saved, "the archive survives a save without the plug-in");
}

#[test]
fn a_plug_in_that_will_not_render_or_goes_wrong_is_bypassed() {
    // No playback renderer when bound: the clip plays its own audio and says why.
    let (e, id) = session("ara no renderer");
    let mut h = host(Behaviour::NoRenderer);
    let got = play(&e, &mut h, 512, SPAN);
    let st = h.status(e.session().find_clip(id).unwrap().1).unwrap();
    assert!(st.contains("bypassed") && st.contains("playback renderer"), "{st}");
    let mut own = e.session().clone();
    own.tracks[0].playlist_mut().unwrap().clips[0].ara = None;
    close(&got, &soundcraft_mix::render_range(&own, SPAN, 512), "no renderer");

    // Renders that are not numbers: bypassed from then on.
    let (e, id) = session("ara fault");
    let mut h = host(Behaviour::Good);
    h.sync(&e);
    assert!(test_plugin::set_fault("ara fault", true));
    let got = play(&e, &mut h, 512, SPAN);
    assert!(got.iter().flatten().all(|x| x.is_finite()), "nothing that is not a number reaches the mix");
    let st = h.status(e.session().find_clip(id).unwrap().1).unwrap();
    assert!(st.contains("not a number"), "{st}");
}

#[test]
fn the_plug_ins_transport_requests_reach_the_host() {
    let (e, _) = session("ara transport");
    let mut h = host(Behaviour::Good);
    h.sync(&e);
    assert!(test_plugin::request_playback("ara transport", 1.5));
    h.idle();
    let r = h.take_requests();
    assert_eq!(r, vec![soundcraft_ara_host::PlaybackRequest::Position(1.5), soundcraft_ara_host::PlaybackRequest::Start]);
}

#[test]
fn the_editor_view_follows_the_edit_selection_and_keeps_its_window_state() {
    let (mut e, id) = session("ara view");
    let t = e.session().tracks[0].id;
    let mut h = host(Behaviour::Good);
    let board = Arc::new(AraBoard::default());
    h.set_realtime(Some(board.clone()), RATE as f32, 512);
    h.sync(&e);
    let clip = e.session().find_clip(id).unwrap().1.clone();
    assert!(h.can_open_editor(&clip).is_ok());
    e.execute("edit.select", &json!({"clips": [id.0]})).unwrap();
    h.sync(&e);
    let s = e.session_arc();
    h.open_editor(&s, id).unwrap();
    assert!(h.editor_open(PLUGIN));
    let (regions, _, _) = report("ara view").unwrap().selection.expect("told the selection");
    assert_eq!(regions, 1, "the selected clip's playback region");
    // A range on the track: its region sequence and the time range.
    e.execute("edit.select", &json!({"tracks": [t.0], "start": 4_800, "end": 9_600})).unwrap();
    h.sync(&e);
    let (_, seqs, range) = report("ara view").unwrap().selection.unwrap();
    assert_eq!(seqs, 1);
    assert_eq!(range, Some((0.1, 0.1)));
    // A hidden track is a hidden region sequence.
    e.session_mut().tracks[0].hidden = true;
    h.sync(&e);
    assert_eq!(report("ara view").unwrap().hidden, 1);
    // With its editor open the voices render while stopped too (the plug-in's previews).
    assert!(board.preview());
    let mut m = MixEngine::new(RATE as f32, 512);
    m.set_external_instances(true);
    m.set_ara_board(Some(board.clone()));
    m.monitor_only = true;
    let mut buf = vec![vec![0.0f32; 512]; soundcraft_mix::main_channels(e.session())];
    m.render(e.session(), 3_000, 512, &mut buf);
    assert!(buf.iter().flatten().any(|x| x.abs() > 0.001), "the plug-in is heard while stopped");
    // Its window state is saved with the document.
    h.store_archives(&mut e);
    let a = &e.session().ara_archives[0];
    assert_eq!(soundcraft_model::b64::decode(&a.view_state).unwrap(), b"window layout");
    no_violations("ara view");
}

#[test]
fn a_stretched_clip_reaches_a_plug_in_that_stretches_as_a_time_stretched_region() {
    let (mut e, id) = session("ara stretch");
    e.session_mut().find_clip_mut(id).unwrap().stretch = 2.0;
    let mut h = host(Behaviour::Good);
    h.sync(&e);
    let st = h.status(e.session().find_clip(id).unwrap().1).unwrap();
    assert!(st.contains("analysed the clip's audio"), "{st}");
    assert_eq!(h.prepare_offline(&mut e), 1);
    let r = e.session().ara_renders.0.get(&id).unwrap().clone();
    let src = signal(60_000, 0.3);
    let c = e.session().find_clip(id).unwrap().1.clone();
    let span = c.source_span(c.length) as f64;
    for rel in [0usize, 1, 999, 20_001, c.length as usize - 1] {
        let at = c.source_offset() as usize + (rel as f64 * span / c.length as f64).round() as usize;
        assert_eq!(r.audio[0][rel], DEFAULT_GAIN * src[at], "timeline sample {rel} plays file sample {at}");
    }
    no_violations("ara stretch");
}

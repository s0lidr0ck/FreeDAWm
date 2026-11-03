//! Scout PRE/POST taps in the mix engine: where they sit, when a capture ends, and what happens
//! when the session changes under a running capture.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::AudioBuffer;
use soundcraft_mix::scout::{ScoutRig, automated_params, channel_type};
use soundcraft_mix::{MixEngine, itu_stereo_fold};
use soundcraft_model::{
    AutoParam, AutomationLane, ChannelFormat, Clip, Insert, ModuleId, Route, ScoutStrip, SendSlot, Session, SourceAudio, SourceId, Speaker, TrackId,
    TrackKind,
};
use soundcraft_time::{Range, SampleRate};
use std::sync::Arc;

const BLOCK: usize = 512;

struct Fx {
    s: Session,
    track: TrackId,
    aux: TrackId,
    master: TrackId,
    gain: ModuleId,
}

fn sine(len: usize, amp: f32, sr: f32) -> Vec<f32> {
    (0..len).map(|n| amp * (std::f32::consts::TAU * 1000.0 * n as f32 / sr).sin()).collect()
}

/// An Audio track playing a -20 dBFS 1 kHz sine through a +6 dB gain module into a mix bus, an
/// Aux fed by that bus's output route, and the master.
fn fixture_at(rate: u32, secs: f64) -> Fx {
    let mut s = Session::new("Scout", SampleRate::new(rate).unwrap());
    let len = (secs * f64::from(rate)) as usize;
    let x = sine(len, 0.1, rate as f32);
    s.pool.insert(SourceId(5), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: rate, channels: vec![x.clone(), x] })));
    let bus = s.add_bus("Mix", ChannelFormat::Stereo);
    let track = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Vox"));
    let clip = s.new_clip_id();
    let tr = s.track_mut(track).unwrap();
    tr.playlist_mut().unwrap().clips.push(Clip::audio(clip, "sine", SourceId(5), 0, 0, len as i64));
    tr.mixer.output = Route::Bus(bus);
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), 6.0);
    let gain = tr.mixer.add_module(g, None).unwrap();
    let aux = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Mix Bus"));
    let a = s.track_mut(aux).unwrap();
    a.mixer.input = Route::Bus(bus);
    a.mixer.output = Route::Main;
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    Fx { s, track, aux, master, gain }
}

fn fixture() -> Fx {
    fixture_at(48_000, 4.0)
}

fn render(eng: &mut MixEngine, s: &Session, blocks: std::ops::Range<usize>) {
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in blocks {
        eng.render(s, (b * BLOCK) as i64, BLOCK, &mut out);
    }
}

/// Arms `strips`, renders `blocks`, stops and returns the finished strips.
fn capture(s: &Session, strips: &[TrackId], blocks: usize) -> Vec<ScoutStrip> {
    let mut eng = MixEngine::new(s.sample_rate.as_f64() as f32, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(s, strips, None, None))).is_none());
    render(&mut eng, s, 0..blocks);
    eng.stop_scout();
    eng.scout_done().expect("a stopped capture comes back").finish(s)
}

fn strip(out: &[ScoutStrip], t: TrackId) -> &ScoutStrip {
    out.iter().find(|x| x.track == t).expect("strip reported")
}

#[test]
fn pre_is_before_the_first_insert_and_post_after_the_last() {
    let mut f = fixture();
    let out = capture(&f.s, &[f.track], 200);
    let v = strip(&out, f.track);
    assert!((v.base.peak_dbfs + 20.0).abs() < 0.2, "pre {}", v.base.peak_dbfs);
    assert!((v.post.peak_dbfs + 14.0).abs() < 0.2, "post {}", v.post.peak_dbfs);
    assert!(v.base.valid && v.post.valid);
    assert_eq!((v.name.as_str(), v.channel_type.as_str(), v.role.as_str(), v.channels), ("Vox", "channel", "Channel", 2));
    // The fader comes after POST.
    f.s.track_mut(f.track).unwrap().mixer.volume_db = -10.0;
    let quiet = capture(&f.s, &[f.track], 200);
    let q = strip(&quiet, f.track);
    assert_eq!((q.base.peak_dbfs, q.post.peak_dbfs), (v.base.peak_dbfs, v.post.peak_dbfs));
}

// Every Scout metric is blind to a polarity flip of both channels (peaks, energies and the L/R
// correlation are unchanged), so this checks trim only; phase invert is applied in the same step.
#[test]
fn trim_is_before_pre() {
    let mut f = fixture();
    let m = &mut f.s.track_mut(f.track).unwrap().mixer;
    m.trim_db = -6.0;
    m.phase_invert = true;
    let out = capture(&f.s, &[f.track], 200);
    let v = strip(&out, f.track);
    assert!((v.base.peak_dbfs + 26.0).abs() < 0.2, "pre {}", v.base.peak_dbfs);
    assert!((v.post.peak_dbfs + 20.0).abs() < 0.2, "post {}", v.post.peak_dbfs);
}

#[test]
fn capture_ends_at_30_seconds() {
    // 8 kHz keeps 40 s of rendering quick; 30 s is 240 000 frames, which splits a 512 block.
    let f = fixture_at(8_000, 40.0);
    let budget = 30 * 8_000u64;
    let mut eng = MixEngine::new(8_000.0, BLOCK);
    let rig = ScoutRig::new(&f.s, &[f.track, f.master], None, None);
    let progress = rig.progress();
    assert!(eng.arm_scout(Box::new(rig)).is_none());
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    let mut back = None;
    let mut rendered = 0u64;
    for b in 0..(40 * 8_000 / BLOCK) {
        eng.render(&f.s, (b * BLOCK) as i64, BLOCK, &mut out);
        rendered += BLOCK as u64;
        if let Some(r) = eng.scout_done() {
            back = Some(r);
            break;
        }
    }
    let rig = back.expect("the capture ends by itself");
    assert_eq!(rendered, budget.div_ceil(BLOCK as u64) * BLOCK as u64, "comes back the block it fills");
    assert_eq!(rig.frames(), budget);
    assert_eq!(progress.load(std::sync::atomic::Ordering::Relaxed), budget);
    assert_eq!(rig.played().iter().map(|r| r.end - r.start).sum::<i64>(), budget as i64);
    assert_eq!(rig.played(), &[Range { start: 0, end: budget as i64 }]);
    let out = rig.finish(&f.s);
    for t in [f.track, f.master] {
        let v = strip(&out, t);
        assert_eq!((v.base.duration_sec, v.post.duration_sec), (30.0, 30.0));
    }
}

#[test]
fn stop_ends_the_capture_early() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track], None, None))).is_none());
    render(&mut eng, &f.s, 0..10);
    assert!(eng.scout_done().is_none(), "still running");
    eng.stop_scout();
    let rig = eng.scout_done().expect("stopped");
    assert_eq!(rig.frames(), 10 * BLOCK as u64);
    assert!(eng.scout_done().is_none(), "handed back once");
    // Rendering on after the hand-back is fine and feeds nothing.
    render(&mut eng, &f.s, 10..20);
    let v = rig.finish(&f.s);
    assert_eq!(strip(&v, f.track).base.duration_sec, 10.0 * BLOCK as f32 / 48_000.0);
}

#[test]
fn a_second_rig_is_refused_while_one_runs() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track], None, None))).is_none());
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.aux], None, None))).is_some());
}

#[test]
fn a_start_and_end_bound_the_capture() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track], Some(1000), Some(5000)))).is_none());
    let mut back = None;
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    for b in 0..20 {
        eng.render(&f.s, (b * BLOCK) as i64, BLOCK, &mut out);
        if let Some(r) = eng.scout_done() {
            back = Some((b, r));
            break;
        }
    }
    let (b, rig) = back.expect("ends at `end`");
    assert_eq!(b, 9, "5000 falls in block 9");
    assert_eq!(rig.frames(), 4000);
    assert_eq!(rig.played(), &[Range { start: 1000, end: 5000 }]);
}

#[test]
fn surround_strips_are_folded_to_stereo() {
    let mut s = Session::default();
    let len = 48_000 * 2;
    let fmt = ChannelFormat::Surround51;
    let c = fmt.speakers().iter().position(|sp| *sp == Speaker::C).unwrap();
    let chans: Vec<Vec<f32>> = (0..fmt.channels()).map(|i| if i == c { sine(len, 0.5, 48_000.0) } else { vec![0.0; len] }).collect();
    s.pool.insert(SourceId(9), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: chans })));
    let t = s.add_track(TrackKind::Audio, fmt, Some("Film"));
    let id = s.new_clip_id();
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "c", SourceId(9), 0, 0, len as i64));
    let out = capture(&s, &[t], 150);
    let v = strip(&out, t);
    let (gl, gr) = itu_stereo_fold(Speaker::C);
    assert_eq!(gl, gr);
    let want = 20.0 * (0.5 * gl).log10();
    assert!((v.base.peak_dbfs - want).abs() < 0.05, "peak {} want {want}", v.base.peak_dbfs);
    assert!(v.base.lr_balance_db.abs() < 1e-3, "balance {}", v.base.lr_balance_db);
    assert!((v.base.stereo_correlation - 1.0).abs() < 1e-3);
    assert_eq!(v.channels, 6);
}

#[test]
fn taps_survive_a_resync_mid_capture() {
    let f = fixture();
    let strips = [f.track, f.aux, f.master];
    let plain = capture(&f.s, &strips, 200);
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &strips, None, None))).is_none());
    render(&mut eng, &f.s, 0..100);
    // An unrelated edit: a new (silent) track changes the structure and re-syncs every strip.
    let mut edited = f.s.clone();
    edited.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("New"));
    render(&mut eng, &edited, 100..200);
    eng.stop_scout();
    let out = eng.scout_done().unwrap().finish(&edited);
    for t in strips {
        let (a, b) = (strip(&plain, t), strip(&out, t));
        assert_eq!((&a.base, &a.post), (&b.base, &b.post), "{}", a.name);
        assert_eq!(b.note, None);
    }
}

#[test]
fn a_strip_deleted_mid_capture_is_reported_not_dropped_on_the_audio_thread() {
    let f = fixture();
    let strips = [f.track, f.aux, f.master];
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &strips, None, None))).is_none());
    render(&mut eng, &f.s, 0..100);
    let mut edited = f.s.clone();
    edited.tracks.retain(|t| t.id != f.track);
    render(&mut eng, &edited, 100..300);
    eng.stop_scout();
    let out = eng.scout_done().unwrap().finish(&edited);
    assert_eq!(out.len(), 3, "the removed strip is still reported");
    let gone = strip(&out, f.track);
    assert_eq!(gone.note.as_deref(), Some("track removed during capture"));
    assert!(!gone.base.valid && !gone.post.valid);
    assert_eq!(gone.name, "Vox", "named from the snapshot taken at capture start");
    for t in [f.aux, f.master] {
        let v = strip(&out, t);
        assert!(v.base.valid && v.post.valid, "{}", v.name);
        assert_eq!(v.note, None);
    }
    // A whole new session (different track ids) leaves every strip orphaned, and still no crash.
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &strips, None, None))).is_none());
    render(&mut eng, &f.s, 0..10);
    let other = fixture_at(48_000, 1.0);
    let mut other_s = other.s;
    other_s.tracks.iter_mut().for_each(|t| t.id = TrackId(t.id.0 + 1000));
    render(&mut eng, &other_s, 10..20);
    eng.stop_scout();
    let out = eng.scout_done().unwrap().finish(&other_s);
    assert_eq!(out.len(), 3);
    assert!(out.iter().all(|v| v.note.as_deref() == Some("track removed during capture")));
}

#[test]
fn sixty_four_strips_capture_without_allocating() {
    const SMALL: usize = 64;
    let mut s = Session::default();
    let mut strips = Vec::new();
    for i in 0..4 {
        let bus = s.add_bus(&format!("B{i}"), ChannelFormat::Stereo);
        let a = s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some(&format!("Bus {i}")));
        let t = s.track_mut(a).unwrap();
        t.mixer.input = Route::Bus(bus);
        t.mixer.output = Route::Main;
        strips.push(a);
        for k in 0..16 {
            let fmt = if k % 2 == 0 { ChannelFormat::Mono } else { ChannelFormat::Stereo };
            let id = s.add_track(TrackKind::Audio, fmt, Some(&format!("T{i}-{k}")));
            s.track_mut(id).unwrap().mixer.output = Route::Bus(bus);
            strips.push(id);
        }
    }
    let master = s.add_track(TrackKind::Master, ChannelFormat::Stereo, Some("Master"));
    strips.push(master);
    assert_eq!(strips.len(), 64 + 4 + 1);
    let mut eng = MixEngine::new(48_000.0, SMALL);
    // Transport stopped: monitoring only, the position does not move.
    eng.monitor_only = true;
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&s, &strips, None, None))).is_none());
    let mut out = vec![vec![0.0f32; SMALL]; 2];
    eng.render(&s, 0, SMALL, &mut out);
    let mut before = eng.scout_buffers();
    before.sort_unstable();
    assert!(before.len() > 4 * strips.len(), "every tap reports its buffers ({})", before.len());
    for _ in 0..500 {
        eng.render(&s, 0, SMALL, &mut out);
    }
    let mut after = eng.scout_buffers();
    after.sort_unstable();
    assert_eq!(before, after);
    eng.stop_scout();
    let rig = eng.scout_done().unwrap();
    assert!(rig.played().is_empty(), "nothing on the timeline was played");
    let res = rig.finish(&s);
    assert_eq!(res.len(), strips.len());
    for v in &res {
        assert!(!v.base.valid && !v.post.valid, "{}", v.name);
        assert_eq!((v.base.activity_ratio, v.post.activity_ratio), (0.0, 0.0), "{}", v.name);
        assert_eq!(v.base.duration_sec, 501.0 * SMALL as f32 / 48_000.0, "{}", v.name);
    }
}

#[test]
fn channel_type_follows_routing() {
    let mut f = fixture();
    let fx_bus = f.s.add_bus("Verb", ChannelFormat::Stereo);
    let verb = f.s.add_track(TrackKind::Aux, ChannelFormat::Stereo, Some("Verb"));
    f.s.track_mut(verb).unwrap().mixer.input = Route::Bus(fx_bus);
    f.s.track_mut(f.track).unwrap().mixer.sends.push(Some(SendSlot::new(Route::Bus(fx_bus))));
    let folder = f.s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Drums"));
    let s = &f.s;
    let ct = |t: TrackId| channel_type(s, s.track(t).unwrap());
    assert_eq!(ct(f.track), ("channel", "Channel"));
    assert_eq!(ct(f.aux), ("mix_bus", "Bus"));
    assert_eq!(ct(verb), ("fx_bus", "Bus"));
    assert_eq!(ct(folder), ("mix_bus", "Bus"));
    assert_eq!(ct(f.master), ("master", "Master"));
}

#[test]
fn automated_params_lists_only_moving_lanes() {
    let mut f = fixture();
    let tr = f.s.track_mut(f.track).unwrap();
    tr.mixer.inserts[0].params.insert("pan".into(), 0.0);
    let mut moving = AutomationLane::new(AutoParam::Plugin { module: f.gain, param: "gain".into() });
    moving.set_point(0, 0.0);
    moving.set_point(48_000, 6.0);
    let mut flat = AutomationLane::new(AutoParam::Plugin { module: f.gain, param: "pan".into() });
    flat.set_point(0, 0.25);
    flat.set_point(48_000, 0.25);
    // A lane on a param the module does not store is not something `apply_params` drives.
    let mut stray = AutomationLane::new(AutoParam::Plugin { module: f.gain, param: "nope".into() });
    stray.set_point(0, 0.0);
    stray.set_point(48_000, 1.0);
    tr.automation.extend([moving, flat, stray]);
    let got = automated_params(&f.s, f.track, Range { start: 0, end: 24_000 });
    assert_eq!(got.len(), 1, "{got:?}");
    assert_eq!((got[0].module, got[0].param.as_str(), got[0].start, got[0].end), (f.gain, "gain", 0.0, 3.0));
    // Automation off: nothing is read, nothing moves.
    f.s.track_mut(f.track).unwrap().mixer.automation_mode = soundcraft_model::AutomationMode::Off;
    assert!(automated_params(&f.s, f.track, Range { start: 0, end: 24_000 }).is_empty());
    // A capture lists it too.
    let mut f = fixture();
    let mut moving = AutomationLane::new(AutoParam::Plugin { module: f.gain, param: "gain".into() });
    moving.set_point(0, 0.0);
    moving.set_point(48_000, 6.0);
    f.s.track_mut(f.track).unwrap().automation.push(moving);
    let out = capture(&f.s, &[f.track], 10);
    assert_eq!(strip(&out, f.track).automated.len(), 1);
    assert_eq!(strip(&out, f.track).mixer, f.s.track(f.track).unwrap().mixer);
}

#[test]
fn master_taps_sit_around_its_inserts_and_before_its_fader() {
    let mut f = fixture();
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), 6.0);
    f.s.track_mut(f.master).unwrap().mixer.add_module(g, None).unwrap();
    let out = capture(&f.s, &[f.aux, f.master], 200);
    let (a, m) = (strip(&out, f.aux), strip(&out, f.master));
    // PRE is the main mix: the aux's output, unity fader and pan.
    assert!((m.base.peak_dbfs - a.post.peak_dbfs).abs() < 0.2, "master pre {} aux post {}", m.base.peak_dbfs, a.post.peak_dbfs);
    assert!((m.post.peak_dbfs - m.base.peak_dbfs - 6.0).abs() < 0.05, "master post {}", m.post.peak_dbfs);
    assert_eq!((m.channel_type.as_str(), m.role.as_str()), ("master", "Master"));
    f.s.track_mut(f.master).unwrap().mixer.volume_db = -10.0;
    let quiet = capture(&f.s, &[f.master], 200);
    let q = strip(&quiet, f.master);
    assert_eq!((q.base.peak_dbfs, q.post.peak_dbfs), (m.base.peak_dbfs, m.post.peak_dbfs));
}

#[test]
fn a_folder_is_tapped_like_an_aux() {
    let mut f = fixture();
    let bus = match f.s.track(f.track).unwrap().mixer.output {
        Route::Bus(b) => b,
        _ => panic!("fixture routes to a bus"),
    };
    let folder = f.s.add_track(TrackKind::Folder, ChannelFormat::Stereo, Some("Drums"));
    let mut g = Insert::new("gain");
    g.params.insert("gain".into(), 3.0);
    let fm = &mut f.s.track_mut(folder).unwrap().mixer;
    fm.input = Route::Bus(bus);
    fm.output = Route::Main;
    fm.add_module(g, None).unwrap();
    let out = capture(&f.s, &[f.track, folder], 200);
    let (v, d) = (strip(&out, f.track), strip(&out, folder));
    assert!((d.base.peak_dbfs - v.post.peak_dbfs).abs() < 0.2, "folder pre {} track post {}", d.base.peak_dbfs, v.post.peak_dbfs);
    assert!((d.post.peak_dbfs - d.base.peak_dbfs - 3.0).abs() < 0.05, "folder post {}", d.post.peak_dbfs);
}

#[test]
fn a_track_deleted_before_the_first_render_is_reported_removed() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track, f.aux], None, None))).is_none());
    let mut edited = f.s.clone();
    edited.tracks.retain(|t| t.id != f.track);
    render(&mut eng, &edited, 0..10);
    eng.stop_scout();
    let out = eng.scout_done().unwrap().finish(&edited);
    let gone = strip(&out, f.track);
    assert_eq!(gone.note.as_deref(), Some(soundcraft_mix::scout::REMOVED_NOTE));
    assert!(!gone.base.valid && !gone.post.valid);
    assert_eq!(strip(&out, f.aux).note, None);
}

#[test]
fn a_track_deleted_after_the_capture_ended_keeps_its_data() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track, f.master], None, Some(72_000)))).is_none());
    // The capture ends on its own; the host has not collected it yet when the track goes.
    render(&mut eng, &f.s, 0..150);
    let mut edited = f.s.clone();
    edited.tracks.retain(|t| t.id != f.track && t.id != f.master);
    render(&mut eng, &edited, 150..151);
    let out = eng.scout_done().unwrap().finish(&edited);
    for t in [f.track, f.master] {
        let v = strip(&out, t);
        assert_eq!(v.note, None, "{}", v.name);
        assert!(v.base.valid && v.post.valid, "{}", v.name);
        assert_eq!(v.base.duration_sec, 1.5);
    }
}

#[test]
fn many_locates_widen_the_last_played_range_and_say_so() {
    let f = fixture();
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track], None, None))).is_none());
    let mut out = vec![vec![0.0f32; BLOCK]; 2];
    // 100 blocks, each somewhere new (forwards and backwards).
    for i in 0..100usize {
        let pos = ((i * 37) % 101) * 2 * BLOCK;
        eng.render(&f.s, pos as i64, BLOCK, &mut out);
    }
    eng.stop_scout();
    let rig = eng.scout_done().unwrap();
    assert_eq!(rig.frames(), 100 * BLOCK as u64);
    assert!(rig.played_truncated());
    assert_eq!(rig.played().len(), 64);
    assert!(rig.played().iter().all(|r| r.start <= r.end), "{:?}", rig.played());
    // The first 63 are exact, one block each.
    assert!(rig.played().iter().take(63).all(|r| r.end - r.start == BLOCK as i64));
}

#[test]
fn inactive_and_vca_strips_are_noted() {
    let mut f = fixture();
    let vca = f.s.add_track(TrackKind::Vca, ChannelFormat::Mono, Some("VCA"));
    let mut eng = MixEngine::new(48_000.0, BLOCK);
    assert!(eng.arm_scout(Box::new(ScoutRig::new(&f.s, &[f.track, f.aux, vca], None, None))).is_none());
    render(&mut eng, &f.s, 0..50);
    let mut edited = f.s.clone();
    edited.track_mut(f.track).unwrap().inactive = true;
    render(&mut eng, &edited, 50..100);
    eng.stop_scout();
    let out = eng.scout_done().unwrap().finish(&edited);
    let note = Some(soundcraft_mix::scout::INACTIVE_NOTE);
    assert_eq!(strip(&out, f.track).note.as_deref(), note);
    assert_eq!(strip(&out, vca).note.as_deref(), note);
    assert_eq!(strip(&out, f.aux).note, None);
}

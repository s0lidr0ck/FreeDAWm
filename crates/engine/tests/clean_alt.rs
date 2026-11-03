//! Clean Vocal's non-destructive result: a clip keeps its original source and the cleaned one,
//! mixed sample for sample by `clean_mix` before its gain and fades, in playback, bounce and every
//! render; edits keep both aligned; save/reload keeps it; re-running cleans the original.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{ChannelFormat, CleanAlt, Clip, ClipId, LyricLine, LyricWord, Session, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};

const N: usize = 48_000;
const OFFSET: Samples = 2_000;
const START: Samples = 1_000;
const LEN: Samples = 20_000;

fn orig(i: usize) -> f32 {
    0.5 * (i as f32 * 0.013).sin()
}

fn wet(k: usize) -> f32 {
    0.3 * (k as f32 * 0.07).cos()
}

fn scratch(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("soundcraft-cleanalt-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A mono track "Vox" with a clip at `START` playing `OFFSET..` of the original, -6 dB with a
/// fade-in, and a cleaned source covering original samples `OFFSET..OFFSET + LEN` at `mix`.
fn session(mix: f32) -> (Engine, TrackId, ClipId, SourceId) {
    let mut s = Session::default();
    let rate = s.sample_rate.hz();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let o = soundcraft_engine::io::add_source(
        &mut s,
        "Vox",
        AudioBuffer { sample_rate: rate, channels: vec![(0..N).map(orig).collect()] },
        None,
        FileFormat::Wav,
    );
    let c = soundcraft_engine::io::add_source(
        &mut s,
        "Vox-clean",
        AudioBuffer { sample_rate: rate, channels: vec![(0..LEN as usize).map(wet).collect()] },
        None,
        FileFormat::Wav,
    );
    let id = s.new_clip_id();
    let mut clip = Clip::audio(id, "Vox", o, OFFSET, START, LEN);
    clip.gain_db = -6.0;
    clip.fade_in.len = 500;
    clip.lyrics = vec![LyricLine { start: 3_000, end: 3_400, words: vec![LyricWord { start: 3_000, end: 3_400, text: "face".into() }] }];
    clip.clean = Some(CleanAlt { source: c, original: o, origin: OFFSET, mix, bypass: false });
    soundcraft_engine::edit::place_clip(&mut s, t, clip);
    (Engine::new(s), t, id, o)
}

/// What the clip should play at timeline position `p` (hand-computed).
fn expected(c: &Clip, p: Samples, mix: f32) -> f32 {
    let rel = p - c.start;
    let at = c.source_offset() + rel;
    let o = orig(at as usize);
    let k = at - OFFSET;
    // The 5 ms crossfade inside the cleaned part where it ends inside the clip's window.
    let ramp = 48_000.0 * 0.005;
    let mut e = 1.0f64;
    if c.source_offset() < OFFSET {
        e = e.min((k + 1) as f64 / ramp);
    }
    if c.source_offset() + c.length > OFFSET + LEN {
        e = e.min((LEN - k) as f64 / ramp);
    }
    let mix = mix * e.clamp(0.0, 1.0) as f32;
    let v = if (0..LEN).contains(&k) { (1.0 - mix) * o + mix * wet(k as usize) } else { o };
    v * c.gain_at(rel)
}

fn render(e: &Engine, t: TrackId, r: Range) -> Vec<f32> {
    soundcraft_mix::render_clips(e.session(), t, r).into_iter().next().unwrap()
}

fn assert_matches(e: &Engine, t: TrackId, mix: f32, what: &str) {
    let r = Range::new(0, 40_000);
    let got = render(e, t, r);
    let clips: Vec<Clip> = e.session().track(t).unwrap().clips().to_vec();
    let mut err = 0.0f32;
    for (i, g) in got.iter().enumerate() {
        let p = i as Samples;
        let want: f32 = clips.iter().filter(|c| c.range().contains(p)).map(|c| expected(c, p, mix)).sum();
        err = err.max((g - want).abs());
    }
    // Gain is evaluated every 32 samples and interpolated: allow that ramp's rounding.
    assert!(err < 2e-3, "{what}: max error {err}");
}

#[test]
fn the_clip_plays_the_mix_before_its_gain_and_fades() {
    for mix in [0.0, 0.3, 1.0] {
        let (e, t, ..) = session(mix);
        assert_matches(&e, t, mix, &format!("mix {mix}"));
    }
    // Bypass plays the original and keeps the amount.
    let (mut e, t, id, _) = session(0.3);
    e.execute("clip.clean_toggle", &json!({"clip": id.0})).unwrap();
    assert_matches(&e, t, 0.0, "bypassed");
    assert!((e.session().find_clip(id).unwrap().1.clean.unwrap().mix - 0.3).abs() < 1e-6);
    e.execute("clip.clean_toggle", &json!({"clip": id.0})).unwrap();
    assert_matches(&e, t, 0.3, "back on");
    // Drags merge into one undo step.
    for m in [40, 50, 60] {
        e.execute_merged("clip.clean_mix", &json!({"clip": id.0, "mix": m}), "clean_mix").unwrap();
    }
    e.end_merge();
    assert_matches(&e, t, 0.6, "amount 60");
    assert!(e.undo());
    assert_matches(&e, t, 0.3, "one undo undoes the drag");
    assert!(e.redo());
    assert_matches(&e, t, 0.6, "redo");
}

#[test]
fn bounce_matches_a_hand_computed_mix() {
    let mix = 0.35;
    let (e, t, id, _) = session(mix);
    // The reference: the same session with the clip playing a hand-mixed source, no alternate.
    let mut s = e.session().clone();
    let rate = s.sample_rate.hz();
    let hand: Vec<f32> = (0..N)
        .map(|i| {
            let k = i as Samples - OFFSET;
            if (0..LEN).contains(&k) { (1.0 - mix) * orig(i) + mix * wet(k as usize) } else { orig(i) }
        })
        .collect();
    let hs = soundcraft_engine::io::add_source(&mut s, "hand", AudioBuffer { sample_rate: rate, channels: vec![hand] }, None, FileFormat::Wav);
    let c = s.find_clip_mut(id).unwrap();
    c.content = soundcraft_model::ClipContent::Audio { source: hs, offset: OFFSET };
    c.clean = None;
    let reference = Engine::new(s);
    let out = scratch("bounce");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    let range = Range::new(0, 30_000);
    let bounce = |e: &Engine, name: &str| {
        let path = out.join(name);
        soundcraft_engine::io::Bounce::new(e.session_arc(), &path, range, &opts, None, false, false).unwrap().run().unwrap();
        soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap().1
    };
    let (a, b) = (bounce(&e, "alt.wav"), bounce(&reference, "hand.wav"));
    let err = a.channels.iter().zip(&b.channels).flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs())).fold(0.0f32, f32::max);
    let peak = a.channels.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    assert!(peak > 0.05 && err < 1e-5, "bounce vs hand-computed mix: {err} (peak {peak})");
    // Freeze renders the same audio.
    let fz = soundcraft_mix::render_track_frozen(e.session(), t, 30_000, None);
    let fr = soundcraft_mix::render_track_frozen(reference.session(), t, 30_000, None);
    let err = fz.iter().zip(&fr).flat_map(|(x, y)| x.iter().zip(y).map(|(p, q)| (p - q).abs())).fold(0.0f32, f32::max);
    assert!(err < 1e-5, "freeze: {err}");
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn edits_keep_both_sources_aligned() {
    let mix = 0.5;
    // Split.
    let (mut e, t, ..) = session(mix);
    e.execute("edit.separate", &json!({"tracks": [t.0], "at": 9_000})).unwrap();
    assert_eq!(e.session().track(t).unwrap().clips().len(), 2);
    assert!(e.session().track(t).unwrap().clips().iter().all(|c| c.clean_alt().is_some()));
    assert_matches(&e, t, mix, "split");
    // Trim the start later and the end past the cleaned window (the original plays there).
    let (mut e, t, id, _) = session(mix);
    e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 4_000})).unwrap();
    assert_matches(&e, t, mix, "trim start");
    let (mut e, t, id, _) = session(mix);
    e.execute("edit.trim_clip", &json!({"clip": id.0, "end": 30_000})).unwrap();
    assert_eq!(e.session().find_clip(id).unwrap().1.end(), 30_000);
    assert_matches(&e, t, mix, "trim end past the cleaned audio");
    // Duplicate.
    let (mut e, t, ..) = session(mix);
    e.execute("edit.duplicate", &json!({"tracks": [t.0], "start": START, "end": START + LEN})).unwrap();
    assert_eq!(e.session().track(t).unwrap().clips().len(), 2);
    assert_matches(&e, t, mix, "duplicate");
    // Lyrics stay in the original's time.
    let c = e.session().track(t).unwrap().clips()[0].clone();
    assert_eq!(c.lyrics[0].words[0].start, 3_000);
}

#[test]
fn save_and_reload_keep_the_alternate_and_old_sessions_load() {
    let mix = 0.4;
    let (mut e, t, id, o) = session(mix);
    e.execute("clip.clean_toggle", &json!({"clip": id.0})).unwrap();
    let out = scratch("save");
    let file = out.join("clean.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let mut again = Engine::new(Session::default());
    soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    let c = again.session().find_clip(id).unwrap().1.clone();
    let k = c.clean_alt().copied().expect("the alternate reloads");
    assert_eq!((k.original, k.origin, k.bypass), (o, OFFSET, true));
    assert!((k.mix - mix).abs() < 1e-6);
    assert_eq!(again.session().pool.get(k.source).unwrap().frames(), LEN as usize);
    again.execute("clip.clean_toggle", &json!({"clip": id.0})).unwrap();
    assert_matches(&again, t, mix, "reloaded");
    let _ = std::fs::remove_dir_all(&out);
    // A clip saved before Clean was non-destructive has no `clean`; an alternate without `mix` /
    // `bypass` is all cleaned.
    let mut v = serde_json::to_value(&c).unwrap();
    v.as_object_mut().unwrap().remove("clean");
    let old: Clip = serde_json::from_value(v.clone()).unwrap();
    assert!(old.clean.is_none());
    v["clean"] = json!({"source": k.source, "original": k.original, "origin": OFFSET});
    let partial: Clip = serde_json::from_value(v).unwrap();
    assert_eq!(partial.clean.map(|k| (k.mix, k.bypass)), Some((1.0, false)));
}

#[test]
fn new_audio_on_the_clip_leaves_the_alternate_inert() {
    // Process and friends give the clip a new source rendered from what it played: the
    // alternate no longer applies (it was made from the old source).
    let (mut e, t, id, _) = session(0.5);
    let before = render(&e, t, Range::new(0, 30_000));
    e.execute("process.run", &json!({"process": "duplicate", "clips": [id.0]})).unwrap();
    let c = e.session().track(t).unwrap().clips()[0].clone();
    assert!(c.clean_alt().is_none());
    let after = render(&e, t, Range::new(0, 30_000));
    let err = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 5e-3, "the processed clip keeps the mixed sound: {err}");
    let _ = id;
}

#[test]
fn a_missing_cleaned_file_plays_the_original_and_is_reported() {
    let (mut e, t, id, _) = session(0.6);
    let out = scratch("missing");
    let file = out.join("clean.scraft");
    soundcraft_engine::io::save_session(&mut e, &file.to_string_lossy()).unwrap();
    let k = e.session().find_clip(id).unwrap().1.clean.unwrap();
    let path = e.session().sources.iter().find(|s| s.id == k.source).unwrap().path.clone();
    let abs = if std::path::Path::new(&path).is_absolute() { std::path::PathBuf::from(&path) } else { out.join(&path) };
    std::fs::remove_file(&abs).unwrap_or_else(|err| panic!("{}: {err}", abs.display()));
    let mut again = Engine::new(Session::default());
    let notes = soundcraft_engine::io::open_session(&mut again, &file.to_string_lossy()).unwrap();
    assert!(notes.iter().any(|n| n.contains("lost the cleaned audio")), "{notes:?}");
    let c = again.session().find_clip(id).unwrap().1.clone();
    assert!(again.session().clean_missing(&c));
    assert_matches(&again, t, 0.0, "missing cleaned audio: the original plays");
    again.execute("clip.clean_remove", &json!({"clip": id.0})).unwrap();
    assert!(again.session().find_clip(id).unwrap().1.clean.is_none());
    assert!(again.undo());
    assert!(again.session().find_clip(id).unwrap().1.clean.is_some());
    let _ = std::fs::remove_dir_all(&out);
}

#[test]
fn commit_renders_the_clip_window_with_handles_and_keeps_it_sounding_the_same() {
    // A 6 s original, the clip 2.5 s in, the cleaned audio over its window.
    let mut s = Session::default();
    let rate = s.sample_rate.hz();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let total = 6 * rate as usize;
    let off: Samples = 120_000;
    let len: Samples = 30_000;
    let o = soundcraft_engine::io::add_source(
        &mut s,
        "Vox",
        AudioBuffer { sample_rate: rate, channels: vec![(0..total).map(orig).collect()] },
        None,
        FileFormat::Wav,
    );
    let c = soundcraft_engine::io::add_source(
        &mut s,
        "Vox-clean",
        AudioBuffer { sample_rate: rate, channels: vec![(0..len as usize).map(wet).collect()] },
        None,
        FileFormat::Wav,
    );
    let id = s.new_clip_id();
    let mut clip = Clip::audio(id, "Vox", o, off, 10_000, len);
    clip.fade_out.len = 2_000;
    clip.lyrics =
        vec![LyricLine { start: off + 500, end: off + 900, words: vec![LyricWord { start: off + 500, end: off + 900, text: "show".into() }] }];
    clip.clean = Some(CleanAlt { source: c, original: o, origin: off, mix: 0.7, bypass: false });
    soundcraft_engine::edit::place_clip(&mut s, t, clip);
    let mut e = Engine::new(s);
    let r = Range::new(0, 50_000);
    let before = render(&e, t, r);
    e.execute("clip.clean_commit", &json!({"clip": id.0})).unwrap();
    let cm = e.session().find_clip(id).unwrap().1.clone();
    let handle = i64::from(rate);
    assert!(cm.clean.is_none());
    assert_eq!(cm.source_offset(), handle, "the window starts one handle before the clip");
    let frames = e.session().pool.get(cm.source().unwrap()).unwrap().frames() as i64;
    assert_eq!(frames, len + 1 + 2 * handle, "only the window and its handles, not the 6 s file");
    assert_eq!(cm.lyrics[0].words[0].start, handle + 500, "lyrics move with the new file's start");
    let after = render(&e, t, r);
    let err = before.iter().zip(&after).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
    assert!(err < 1e-6, "commit changes the sound: {err}");
    // Trimming open into the handle plays the original there.
    e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 9_000})).unwrap();
    let cm = e.session().find_clip(id).unwrap().1.clone();
    assert_eq!(cm.source_offset(), handle - 1_000);
    let got = render(&e, t, Range::new(9_000, 9_010));
    let want = orig((off - 1_000) as usize) * cm.gain_at(0);
    assert!((got[0] - want).abs() < 1e-4, "{} vs {want}", got[0]);
}

#[test]
fn the_cleaned_part_crossfades_where_it_ends_inside_the_clip() {
    // All cleaned, the clip trimmed open on both sides past the cleaned audio: around each edge the
    // level glides over 5 ms instead of stepping (the cleaned audio here is a different signal).
    let (mut e, t, id, _) = session(1.0);
    e.execute("edit.trim_clip", &json!({"clip": id.0, "start": 0})).unwrap();
    e.execute("edit.trim_clip", &json!({"clip": id.0, "end": 30_000})).unwrap();
    let c = e.session().find_clip(id).unwrap().1.clone();
    assert!(c.source_offset() < OFFSET && c.source_offset() + c.length > OFFSET + LEN);
    assert_matches(&e, t, 1.0, "both edges crossfade");
    let got = render(&e, t, Range::new(0, 30_000));
    // Without the fade the step at the cleaned end is |orig - wet|; with it each sample moves little.
    let at_end = (c.start + (OFFSET + LEN - c.source_offset())) as usize;
    let worst = (at_end - 300..at_end + 300).map(|i| (got[i + 1] - got[i]).abs()).fold(0.0f32, f32::max);
    let g = c.gain_at(at_end as Samples - c.start);
    let step = (orig((OFFSET + LEN) as usize) - wet(LEN as usize - 1)).abs() * g;
    println!("worst sample step near the end {worst}, unfaded step {step}");
    // The signals' own slopes are at most 0.021 * gain per sample; the switch would add `step`.
    assert!(step > 0.05 && worst < 0.03 * g + 1e-3, "worst {worst}, unfaded {step}");
}

//! Tests of Edit › Split into Files (`split_files`).

use super::*;
use soundcraft_model::ChannelFormat;

const SR: Samples = 48_000;
const AT: Samples = 1_000;
const LEN: Samples = 6 * SR;
const M1: Samples = AT + 2 * SR;
const M2: Samples = AT + 4 * SR;

/// Repeatable noise: every sample tells where it came from.
fn noise(n: usize, seed: u32) -> Vec<f32> {
    let mut x = seed.wrapping_mul(2_654_435_761).max(1);
    (0..n)
        .map(|_| {
            x ^= x << 13;
            x ^= x >> 17;
            x ^= x << 5;
            (x as f32 / u32::MAX as f32) * 2.0 - 1.0
        })
        .collect()
}

fn frames(n: Samples) -> usize {
    usize::try_from(n).unwrap()
}

/// A "service": a mono Vox track and a stereo Band track holding one long recording each from
/// `AT`, a MIDI track with one long clip, and two song markers.
fn service() -> (Engine, TrackId, TrackId, TrackId, SourceId) {
    let mut s = Session::default();
    let vox = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
    let band = s.add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Band"));
    let keys = s.add_track(TrackKind::Midi, ChannelFormat::Mono, Some("Keys"));
    let n = frames(LEN);
    let src = crate::io::add_source(&mut s, "Vox_01", AudioBuffer { sample_rate: 48_000, channels: vec![noise(n, 1)] }, None, FileFormat::Wav);
    let bsrc = crate::io::add_source(
        &mut s,
        "Band_01",
        AudioBuffer { sample_rate: 48_000, channels: vec![noise(n, 2), noise(n, 3)] },
        None,
        FileFormat::Wav,
    );
    let (c1, c2, c3) = (s.new_clip_id(), s.new_clip_id(), s.new_clip_id());
    let mut vc = Clip::audio(c1, "Vox_01", src, 0, AT, LEN);
    vc.gain_db = -3.0;
    s.track_mut(vox).unwrap().playlist_mut().unwrap().clips.push(vc);
    // The band clip is trimmed: it starts half a second into its file.
    s.track_mut(band).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(c2, "Band_01", bsrc, SR / 2, AT, LEN - SR / 2));
    let mut seq = soundcraft_midi::Sequence::default();
    seq.notes.push(soundcraft_midi::Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: 0, length: 960 });
    s.track_mut(keys).unwrap().playlist_mut().unwrap().clips.push(Clip::midi(c3, "Keys", AT, LEN, seq));
    s.add_marker("Amazing Grace", MarkerKind::Marker, M1, M1);
    s.add_marker("Song: 2/B", MarkerKind::Marker, M2, M2);
    (Engine::new(s), vox, band, keys, src)
}

fn render(e: &Engine) -> Vec<Vec<f32>> {
    soundcraft_mix::render_range(e.session(), Range::new(0, AT + LEN + 1_000), 1024)
}

fn source_of(e: &Engine, c: &Clip) -> Arc<SourceAudio> {
    Arc::clone(e.session().pool.get(c.source().unwrap()).unwrap())
}

#[test]
fn splitting_at_markers_makes_one_exact_file_per_song_per_track_and_plays_the_same() {
    let (mut e, vox, band, keys, long) = service();
    let before = render(&e);
    let long_audio = Arc::clone(e.session().pool.get(long).unwrap());
    let v = e.execute(ID, &json!({})).unwrap();
    assert_eq!(v["files"], 6, "{v}");
    assert!(v["kept"].as_array().unwrap().is_empty());
    let clips = e.session().track(vox).unwrap().clips().to_vec();
    assert_eq!(clips.iter().map(|c| (c.start, c.length)).collect::<Vec<_>>(), vec![(AT, M1 - AT), (M1, M2 - M1), (M2, AT + LEN - M2)]);
    let names: Vec<&str> = clips.iter().map(|c| c.name.as_str()).collect();
    assert_eq!(names, ["Vox - Part 1", "Vox - Amazing Grace", "Vox - Song- 2-B"]);
    for c in &clips {
        assert_eq!(c.source_offset(), 0);
        assert_eq!(c.gain_db, -3.0, "the clip keeps its gain");
        let from = frames(c.start - AT);
        let want = &long_audio.buffer().channels[0][from..from + frames(c.length)];
        assert_eq!(source_of(&e, c).buffer().channels[0], want, "{}: sample-exact", c.name);
        let src = e.session().sources.iter().find(|x| Some(x.id) == c.source()).unwrap();
        assert!(src.unsaved && src.path.starts_with("Audio Files/Vox - "), "{}", src.path);
    }
    // The trimmed band clip: its pieces come from half a second into its file, both channels.
    let bclips = e.session().track(band).unwrap().clips().to_vec();
    assert_eq!(bclips.len(), 3);
    let b0 = source_of(&e, &bclips[0]);
    assert_eq!(b0.buffer().channels.len(), 2);
    assert_eq!(b0.buffer().channels[1][0], noise(frames(LEN), 3)[frames(SR / 2)]);
    // MIDI is cut at the same points.
    assert_eq!(e.session().track(keys).unwrap().clips().iter().map(|c| c.start).collect::<Vec<_>>(), vec![AT, M1, M2]);
    // The long files left the session; the audio sounds the same.
    let s = e.session();
    assert!(!s.sources.iter().any(|x| x.id == long) && !s.pool.contains(long));
    assert_eq!(render(&e), before);
}

#[test]
fn undo_puts_the_long_files_back_in_one_step() {
    let (mut e, vox, _, keys, long) = service();
    let before = e.session().clone();
    let before_audio = render(&e);
    e.execute(ID, &json!({})).unwrap();
    assert!(e.undo());
    let s = e.session();
    assert_eq!(s.tracks, before.tracks);
    assert_eq!(s.sources, before.sources);
    assert!(s.pool.contains(long));
    assert_eq!(s.track(vox).unwrap().clips().len(), 1);
    assert_eq!(s.track(keys).unwrap().clips().len(), 1);
    assert_eq!(render(&e), before_audio);
}

#[test]
fn a_selection_splits_only_inside_it_on_the_named_tracks() {
    let (mut e, vox, band, _, long) = service();
    let before = render(&e);
    let (a, b) = (M1 - SR / 2, M2 + SR / 2);
    let v = e.execute(ID, &json!({"tracks": [vox.0], "start": a, "end": b})).unwrap();
    assert_eq!(v["files"], 3, "{v}");
    assert_eq!(v["cuts"], json!([a, M1, M2, b]));
    let s = e.session();
    let clips = s.track(vox).unwrap().clips().to_vec();
    assert_eq!(clips.iter().map(|c| c.start).collect::<Vec<_>>(), vec![AT, a, M1, M2, b]);
    // Outside the selection the clips keep the long file, which stays.
    assert_eq!(clips[0].source(), Some(long));
    assert_eq!(clips[4].source(), Some(long));
    assert!(s.pool.contains(long));
    // The piece before the first marker inside is named for its place, after it for the song.
    assert_eq!(clips[1].name, "Vox - Part 1");
    assert_eq!(clips[2].name, "Vox - Amazing Grace");
    assert_ne!(clips[3].source(), Some(long));
    assert_eq!(s.track(band).unwrap().clips().len(), 1, "the band track is not split");
    assert_eq!(render(&e), before);
}

#[test]
fn the_job_copies_ahead_and_the_command_uses_its_copies() {
    let (mut e, vox, _, _, _) = service();
    let (mut direct, ..) = service();
    let want = direct.execute(ID, &json!({})).unwrap();
    let mut job = SplitJob::new(&e, &json!({})).unwrap().unwrap();
    assert_eq!(job.counts(), (0, 6));
    assert!(!job.step(|| false), "one piece per call when told to stop");
    assert_eq!(job.counts(), (1, 6));
    while !job.step(|| true) {}
    assert_eq!(job.progress(), 1.0);
    let first = Arc::clone(&job.done[0].audio);
    let got = job.run(&mut e).unwrap();
    assert_eq!(got["pieces"], want["pieces"]);
    let c = e.session().track(vox).unwrap().clips()[0].clone();
    assert!(Arc::ptr_eq(e.session().pool.get(c.source().unwrap()).unwrap(), &first), "the copy made ahead is used");
    assert!(e.take_split_pieces().is_empty(), "nothing left over");
}

#[test]
fn clips_the_copy_cannot_reproduce_are_cut_but_keep_their_file() {
    let (mut e, vox, _, _, long) = service();
    let id = e.session().track(vox).unwrap().clips()[0].id;
    e.session_mut().find_clip_mut(id).unwrap().stretch = 2.0;
    let v = e.execute(ID, &json!({"tracks": ["Vox"]})).unwrap();
    assert_eq!(v["files"], 0);
    assert_eq!(v["kept"].as_array().unwrap().len(), 3, "{v}");
    let s = e.session();
    assert_eq!(s.track(vox).unwrap().clips().len(), 3);
    assert!(s.track(vox).unwrap().clips().iter().all(|c| c.source() == Some(long)));
    assert!(SplitJob::new(&e, &json!({"tracks": ["Vox"]})).unwrap().is_none());
}

#[test]
fn a_second_split_without_markers_changes_no_file() {
    let (mut e, vox, _, _, _) = service();
    e.execute(ID, &json!({})).unwrap();
    e.session_mut().markers.clear();
    let sources = e.session().sources.clone();
    let v = e.execute(ID, &json!({})).unwrap();
    assert_eq!(v["files"], 0, "{v}");
    assert_eq!(e.session().sources, sources);
    assert_eq!(e.session().track(vox).unwrap().clips().len(), 3);
}

#[test]
fn bad_ranges_and_empty_sessions_are_refused() {
    let (mut e, ..) = service();
    assert!(e.execute(ID, &json!({"start": 10, "end": 5})).is_err());
    assert!(e.execute(ID, &json!({"start": 10})).is_err());
    assert!(e.execute(ID, &json!({"tracks": ["Nope"]})).is_err());
    let mut empty = Engine::new(Session::default());
    assert!(empty.execute(ID, &json!({})).is_err());
}

#[test]
fn the_session_saves_each_piece_as_its_own_wav() {
    let (mut e, vox, ..) = service();
    e.execute(ID, &json!({})).unwrap();
    let dir = std::env::temp_dir().join(format!("freedawm-split-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Service.fdawm");
    crate::io::save_session(&mut e, path.to_str().unwrap()).unwrap();
    for c in e.session().track(vox).unwrap().clips() {
        let f = dir.join("Audio Files").join(format!("{}.wav", c.name));
        assert!(f.is_file(), "{} written", f.display());
    }
    let _ = std::fs::remove_dir_all(&dir);
}

/// The vocal tools on a split piece hear that piece, not the long file under it.
#[test]
fn the_vocal_tools_hear_only_the_split_piece() {
    let (mut e, vox, _, _, _) = service();
    e.execute(ID, &json!({})).unwrap();
    let piece = e.session().track(vox).unwrap().clips()[1].clone();
    let n = frames(piece.length);
    // Vocal tuning analyses the clip's whole source: now the piece.
    let heard = crate::vocal::heard_audio(e.session(), &piece).unwrap();
    assert_eq!(heard[0].len(), n);
    assert!(crate::vocal::analysable(e.session(), &piece));
    let job = crate::vocal::next_job(&e, Some(piece.id), 0).unwrap();
    assert!(matches!(job, crate::vocal::VocalJob::Analyse { clip, .. } if clip == piece.id));
    // Clean Vocal (and the other ML tools) render the clip's range.
    #[cfg(not(target_arch = "wasm32"))]
    {
        let ml = crate::ml::prepare(&e, "ml.clean", &json!({"clip": piece.id.0})).unwrap();
        assert_eq!(ml.parts.len(), 1);
        assert_eq!(ml.parts[0].audio[0].len(), n);
        let stems = crate::ml::prepare(&e, "ml.separate", &json!({"clip": piece.id.0})).unwrap();
        assert_eq!(stems.parts[0].audio[0].len(), n);
    }
    // Tempo detection over the clip hears its range.
    let src = crate::tempo_detect::TempoSource { tracks: vec![vox], range: piece.range(), clips: vec![piece.id], mix: false };
    assert_eq!(crate::tempo_detect::source_audio(e.session(), &src).len(), n);
}

#[test]
fn two_clips_in_one_song_get_numbered_names() {
    let (mut e, vox, ..) = service();
    e.execute("edit.separate", &json!({"tracks": [vox.0], "at": M1 + SR / 2})).unwrap();
    let v = e.execute(ID, &json!({"tracks": [vox.0]})).unwrap();
    assert_eq!(v["files"], 4, "{v}");
    let names: Vec<String> = e.session().track(vox).unwrap().clips().iter().map(|c| c.name.clone()).collect();
    assert_eq!(names, ["Vox - Part 1", "Vox - Amazing Grace", "Vox - Amazing Grace 2", "Vox - Song- 2-B"]);
}

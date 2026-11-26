//! A file that cannot play from disk as it is (here an AIFF, larger than the in-memory threshold)
//! is decoded on the first open and kept decoded in the project's `Decoded` folder; from then on
//! (that first open too) it plays from that decode, so the decoded audio is not kept in memory.

use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileFormat};
use soundcraft_engine::Engine;
use soundcraft_model::{Clip, Source, SourceId};

#[test]
fn an_aiff_is_decoded_once_and_then_streamed() {
    let dir = std::env::temp_dir().join(format!("freedawm-decode-cache-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let aif = dir.join("tone.aif");
    let ramp: Vec<f32> = (0..96_000).map(|i| (i % 480) as f32 / 1000.0).collect();
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![ramp.clone()] };
    let opts = EncodeOptions { format: FileFormat::Aiff, bit_depth: soundcraft_audio_io::BitDepth::Int24, ..EncodeOptions::default() };
    std::fs::write(&aif, soundcraft_audio_io::encode(&buf, &opts).unwrap()).unwrap();

    let mut e = Engine::default();
    e.execute("track.new", &json!({"count": 1})).unwrap();
    let t = e.session().tracks[0].id;
    let s = e.session_mut();
    s.sources.push(Source {
        id: SourceId(77),
        name: "tone".into(),
        path: aif.to_string_lossy().into_owned(),
        channels: 1,
        frames: 96_000,
        sample_rate: 48_000,
        format: FileFormat::Aiff,
        time_reference: 0,
        unsaved: false,
    });
    let id = s.new_clip_id();
    s.track_mut(t).and_then(|t| t.playlist_mut()).unwrap().clips = vec![Clip::audio(id, "tone", SourceId(77), 0, 0, 96_000)];
    let session = dir.join("song.fdawm");
    e.execute("session.save_as", &json!({"path": session.to_string_lossy()})).unwrap();
    let session = e.path.clone().map(std::path::PathBuf::from).unwrap_or(session);

    let open = || {
        let mut r = Engine::default();
        r.execute("session.open", &json!({"path": session.to_string_lossy()})).unwrap();
        r
    };
    let first = open();
    let a = first.session().pool.get(SourceId(77)).unwrap().clone();
    assert!(a.disk_file().is_some() && a.in_memory().is_none(), "the first open decodes it, then plays the decode from disk");
    let folder = session.parent().unwrap().join(soundcraft_engine::decode_cache::FOLDER);
    let cached: Vec<_> = std::fs::read_dir(&folder).unwrap().flatten().collect();
    assert_eq!(cached.len(), 1, "one decode kept");

    let second = open();
    let b = second.session().pool.get(SourceId(77)).unwrap().clone();
    assert!(b.disk_file().is_some(), "the next open plays the decode from disk");
    let (mut x, mut y) = (vec![0.0f32; 96_000], vec![0.0f32; 96_000]);
    a.read(0, 0, &mut x);
    b.read(0, 0, &mut y);
    assert!(x.iter().zip(&y).all(|(p, q)| (p - q).abs() < 1e-6), "the same audio");
    assert!((x[100] - ramp[100]).abs() < 1e-3);
    let _ = std::fs::remove_dir_all(&dir);
}

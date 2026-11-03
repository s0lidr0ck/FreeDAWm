//! Bounces play the session's band into tracks whose input is a musician, in both passes of a
//! normalized bounce, exactly as the offline render does.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use soundcraft_audio_io::{BitDepth, EncodeOptions, FileFormat};
use soundcraft_engine::io::Bounce;
use soundcraft_model::{ChannelFormat, Insert, Route, TrackKind};
use soundcraft_time::Range;

#[test]
fn bounces_play_the_band_in_both_passes() {
    let mut e = soundcraft_engine::Engine::default();
    let mut s = e.session().clone();
    let songs = soundcraft_band::song_file::factory_songs();
    let blues = songs.iter().find(|f| f.name == "12-bar Blues").unwrap();
    let (defs, blocks, chords) = soundcraft_mix::lanes_from_song(&s, &blues.song, 0);
    s.band.defs = defs;
    s.band.sections = blocks;
    s.band.chords = chords;
    for (k, m) in s.band.slots.iter_mut().enumerate() {
        m.enabled = k == 0;
    }
    let t = s.add_track(TrackKind::Instrument, ChannelFormat::Stereo, None);
    let tr = s.track_mut(t).unwrap();
    tr.instrument = Some(Insert::new("subtractive_synth"));
    tr.mixer.input = Route::Musician(0);
    e.replace_session(s);
    let s = e.session_arc();
    let r = Range::new(0, 4 * 48_000);

    // The offline render with the band, in memory.
    let mut rr = soundcraft_mix::RangeRender::new(&s, None, r, 1024);
    assert!(rr.adopt_band_library(&s, &e.band_library()));
    let mut mix: Vec<Vec<f32>> = vec![Vec::new(); 2];
    while let Some((chunk, n)) = rr.next_block(&s) {
        for (o, c) in mix.iter_mut().zip(chunk) {
            o.extend_from_slice(&c[..n]);
        }
    }

    let dir = std::env::temp_dir().join(format!("soundcraft-band-bounce-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    let path = dir.join("Band.wav");
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None };
    for normalize in [false, true] {
        Bounce::new(s.clone(), &path, r, &opts, None, normalize, false).unwrap().with_band(e.band_library()).run().unwrap();
        let (_, file) = soundcraft_audio_io::decode(&std::fs::read(&path).unwrap(), None).unwrap();
        let mut want = mix.clone();
        if normalize {
            soundcraft_dsp::offline::normalize(&mut want, -0.1, false);
        }
        assert_eq!(file.channels, want, "normalize: {normalize}");
    }
    let _ = std::fs::remove_dir_all(&dir);
    assert!(mix.iter().flatten().any(|x| *x != 0.0), "the bounce plays the band");
}

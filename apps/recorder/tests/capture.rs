//! Quick capture: a take from a synthetic input and MIDI keyboard to a WAV and a MIDI file.

mod common;

use common::{RATE, offline, ramp, temp_dir};
use soundcraft_recorder::rig::InputChoice;
use soundcraft_recorder::settings::{stamp_of, unique_stem};

const MONO: InputChoice = InputChoice { on: true, channel: 0, stereo: false };

fn wav(path: &std::path::Path) -> (soundcraft_audio_io::AudioInfo, soundcraft_audio_io::AudioBuffer) {
    soundcraft_audio_io::decode(&std::fs::read(path).unwrap(), Some("wav")).unwrap()
}

#[test]
fn a_take_writes_a_wav_and_a_midi_file_with_the_same_name() {
    let dir = temp_dir("take");
    let mut o = offline(2, &["Keys"], InputChoice { on: true, channel: 0, stereo: true });
    o.run(0.1, &ramp);
    o.rig.start(&dir, "Idea 2026-11-22 14-03-10", false, 0).unwrap();
    o.run(0.3, &ramp);
    assert!(o.keys.send("Keys", &[0x90, 60, 100]));
    o.run(0.3, &ramp);
    o.keys.send("Keys", &[0x80, 60, 0]);
    o.keys.send("Keys", &[0x90, 64, 90]);
    o.run(0.3, &ramp);
    o.keys.send("Keys", &[0x80, 64, 0]);
    o.run(0.2, &ramp);
    let saved = o.rig.stop();
    assert_eq!(saved.problem, None);

    // The WAV: 24-bit at the device rate, about the take's length, holding the input as it came.
    let path = saved.wav.clone().unwrap();
    assert_eq!(path.file_name().unwrap(), "Idea 2026-11-22 14-03-10.wav");
    let (info, buf) = wav(&path);
    assert_eq!(info.sample_rate, RATE);
    assert_eq!(info.channels, 2);
    assert_eq!(info.sample_format, soundcraft_audio_io::SampleFormat::Int24);
    let secs = info.frames as f64 / f64::from(RATE);
    assert!((1.0..1.3).contains(&secs), "the take is as long as it recorded: {secs} s");
    // Each frame is the ramp at some offset (the feed's latency): find it from the first frame,
    // then every frame follows, on both channels.
    let (l, r) = (&buf.channels[0], &buf.channels[1]);
    let first = l[100];
    let at = (0..8000).find(|i| (ramp(*i, 0) - first).abs() < 1e-5).unwrap();
    for k in 100..l.len() {
        let i = at + k - 100;
        assert!((l[k] - ramp(i, 0)).abs() < 1e-5, "left frame {k}: {} vs {}", l[k], ramp(i, 0));
        assert!((r[k] - ramp(i, 1)).abs() < 1e-5, "right frame {k}");
    }

    // The MIDI file: the two notes, in order, about 0.3 s each at 120 bpm (960 ticks a beat).
    let mid = saved.mid.clone().unwrap();
    assert_eq!(mid.file_name().unwrap(), "Idea 2026-11-22 14-03-10.mid");
    assert_eq!(saved.notes, 2);
    let smf = soundcraft_midi::read_smf(&std::fs::read(&mid).unwrap()).unwrap();
    assert_eq!(smf.tempos.first().map(|t| t.1.round()), Some(120.0));
    let notes: Vec<_> = smf.tracks.iter().flat_map(|t| t.sequence.notes.iter()).collect();
    assert_eq!(notes.iter().map(|n| n.pitch).collect::<Vec<_>>(), vec![60, 64]);
    // 0.3 s at 120 bpm is 0.6 beats: 576 ticks.
    for n in &notes {
        assert!((400..800).contains(&n.length), "a note as long as it was held: {}", n.length);
    }
    // Played 0.3 s and 0.6 s into the take (with a block or two of slack).
    let at = |n: &&soundcraft_midi::Note| n.start as f64 / 1920.0;
    assert!((0.2..0.45).contains(&at(&notes[0])), "first note at {} s", at(&notes[0]));
    assert!((0.5..0.75).contains(&at(&notes[1])), "second note at {} s", at(&notes[1]));
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn audio_alone_writes_no_midi_file_and_midi_alone_no_wav() {
    let dir = temp_dir("alone");
    let mut o = offline(1, &[], MONO);
    o.rig.start(&dir, "a", false, 0).unwrap();
    o.run(0.2, &ramp);
    let saved = o.rig.stop();
    assert!(saved.wav.is_some() && saved.mid.is_none());
    assert!(!dir.join("a.mid").exists());

    let mut o = offline(0, &["Keys"], InputChoice { on: false, ..MONO });
    assert!(o.rig.can_record());
    o.rig.start(&dir, "b", false, 0).unwrap();
    o.run(0.1, &ramp);
    o.keys.send("Keys", &[0x90, 67, 100]);
    o.run(0.1, &ramp);
    o.keys.send("Keys", &[0x80, 67, 0]);
    o.run(0.1, &ramp);
    let saved = o.rig.stop();
    assert!(saved.wav.is_none(), "MIDI only: no WAV");
    assert_eq!(saved.notes, 1);
    assert!(dir.join("b.mid").exists());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn takes_in_the_same_second_never_overwrite_each_other() {
    let dir = temp_dir("names");
    let at = stamp_of(1_795_356_190);
    let first = unique_stem(&dir, "Idea", at);
    assert_eq!(first, "Idea 2026-11-22 14-03-10");
    std::fs::write(dir.join(format!("{first}.wav")), b"x").unwrap();
    let second = unique_stem(&dir, "Idea", at);
    assert_eq!(second, "Idea 2026-11-22 14-03-10 (2)");
    // A MIDI file alone holds the name too.
    std::fs::write(dir.join(format!("{second}.mid")), b"x").unwrap();
    assert_eq!(unique_stem(&dir, "Idea", at), "Idea 2026-11-22 14-03-10 (3)");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_crash_mid_take_leaves_a_readable_wav() {
    let dir = temp_dir("crash");
    let mut o = offline(1, &[], MONO);
    o.rig.start(&dir, "crash", false, 0).unwrap();
    // The writer rewrites the header every second: past that, the file on disk is a WAV that
    // says how long it is, whatever happens to the app next.
    o.run(1.5, &ramp);
    let (info, buf) = wav(&dir.join("crash.wav"));
    let secs = info.frames as f64 / f64::from(RATE);
    assert!(secs >= 0.9, "the file holds the take so far: {secs} s");
    assert!(buf.channels[0].iter().any(|x| x.abs() > 0.1), "and the input in it");
    // (Then the app goes on and the take ends normally.)
    let saved = o.rig.stop();
    assert!(saved.wav.is_some());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn an_unplugged_input_ends_the_take_and_keeps_it() {
    let dir = temp_dir("unplug");
    let mut o = offline(1, &[], MONO);
    o.rig.start(&dir, "unplugged", false, 0).unwrap();
    o.run(0.3, &ramp);
    o.input.as_ref().unwrap().unplug();
    let saved = o.rig.step().expect("the take stops when the input goes");
    assert!(saved.problem.as_deref().unwrap_or("").contains("audio input stopped"), "{:?}", saved.problem);
    assert!(saved.wav.as_ref().is_some_and(|p| p.exists()), "what was recorded is kept");
    assert!(!o.rig.recording());
    assert!(o.rig.input_error.is_some(), "the window says the input is gone");
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn a_folder_that_cannot_be_made_is_said_not_crashed_on() {
    let dir = temp_dir("badfolder");
    let file = dir.join("not a folder");
    std::fs::write(&file, b"x").unwrap();
    let mut o = offline(1, &[], MONO);
    let e = o.rig.start(&file.join("ideas"), "x", false, 0).unwrap_err();
    assert!(e.contains("Cannot use the folder"), "{e}");
    assert!(!o.rig.recording());
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn no_input_at_all_cannot_record_and_says_why() {
    let mut o = offline(0, &[], MONO);
    assert!(!o.rig.can_record());
    let e = o.rig.start(&std::env::temp_dir(), "x", false, 0).unwrap_err();
    assert!(e.contains("Nothing to record from"), "{e}");
}

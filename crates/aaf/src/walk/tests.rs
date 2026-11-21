//! The reader on synthetic AAF files made by the test writer.

use super::*;
use crate::testwrite::{Item, MediaSpec, Spec, ramp, session};

fn spec() -> Spec {
    Spec {
        name: "Synthetic".into(),
        rate: (25, 1),
        tc_start: Some(90_000),
        tracks: vec![
            (
                "Dialog".into(),
                vec![
                    Item::Filler(25),
                    Item::Clip { media: 0, start: 5, len: 50, gain: Some(0.5) },
                    Item::Transition(10),
                    Item::Clip { media: 1, start: 0, len: 40, gain: None },
                    Item::Transition(5),
                    Item::Filler(20),
                ],
            ),
            ("Music".into(), vec![Item::Stereo { left: 0, right: 1, start: 0, len: 25 }]),
        ],
        media: vec![
            MediaSpec { name: "tone".into(), rate: 48_000, samples: ramp(48_000 * 3), url: None },
            MediaSpec { name: "voice".into(), rate: 48_000, samples: ramp(48_000 * 2), url: None },
        ],
        markers: vec![(50, "Scene 2".into())],
    }
}

fn opts() -> Options {
    Options { sample_rate: 48_000, ..Options::default() }
}

#[test]
fn a_synthetic_composition_reads_back_whole() {
    for sector in [512, 4096] {
        let bytes = session(&spec(), sector);
        let imp = read(&bytes, &opts()).unwrap();
        assert_eq!(imp.name, "Synthetic");
        assert_eq!(imp.writer, "FreeDAWm tests testwrite 1");
        assert_eq!(imp.timecode.map(|t| (t.frames, t.fps)), Some((90_000, 25)));
        assert_eq!(imp.start, 90_000 * 1920);
        assert_eq!(imp.tracks.len(), 2);
        let d = &imp.tracks[0];
        assert_eq!(d.name, "Dialog");
        assert_eq!(d.clips.len(), 2);
        let (a, b) = (&d.clips[0], &d.clips[1]);
        // 1920 samples a frame at 25 fps and 48 kHz.
        assert_eq!((a.start, a.len, a.offset), (25 * 1920, 50 * 1920, 5 * 1920));
        assert_eq!(a.gain_db.map(|g| (g * 10.0).round()), Some(-60.0));
        assert_eq!(a.name, "tone");
        // The transition overlaps the two clips by its length: a crossfade.
        assert_eq!(b.start, (75 - 10) * 1920);
        assert_eq!(a.fade_out, Some(Fade { len: 10 * 1920, curve: Curve::Linear, cross: true }));
        assert_eq!(b.fade_in, Some(Fade { len: 10 * 1920, curve: Curve::Linear, cross: true }));
        // Before a filler: a fade out.
        assert_eq!(b.fade_out, Some(Fade { len: 5 * 1920, curve: Curve::Linear, cross: false }));
        assert_eq!(a.fade_in, None);
        let m = &imp.tracks[1];
        assert_eq!(m.channels, 2);
        assert_eq!(m.clips[0].channels.len(), 2);
        assert_eq!(imp.markers.len(), 1);
        assert_eq!((imp.markers[0].name.as_str(), imp.markers[0].start), ("Scene 2", 50 * 1920));
        assert_eq!(imp.media.len(), 2);
        assert_eq!(imp.media_rate, Some(48_000));
        let MediaData::Embedded { bytes: wav, ext } = &imp.media[0].data else { panic!("embedded") };
        assert_eq!(*ext, "wav");
        assert!(wav.starts_with(b"RIFF"));
        assert_eq!(wav.len(), 44 + 48_000 * 3 * 2);
    }
}

#[test]
fn positions_follow_the_requested_rate() {
    let imp = read(&session(&spec(), 512), &Options { sample_rate: 44_100, ..Options::default() }).unwrap();
    assert_eq!(imp.tracks[0].clips[0].start, 25 * 1764);
    assert_eq!(imp.tracks[0].clips[0].offset, 5 * 1764);
}

#[test]
fn external_media_is_named_not_read() {
    let mut s = spec();
    s.media[1].url = Some("file:///c%3a/Media/voice%201.wav".into());
    let imp = read(&session(&s, 512), &opts()).unwrap();
    let MediaData::External { locations } = &imp.media[1].data else { panic!("external") };
    assert_eq!(locations, &["file:///c%3a/Media/voice%201.wav".to_string()]);
    assert_eq!(imp.media[1].frames, 96_000);
}

#[test]
fn truncated_and_corrupted_files_never_panic() {
    let good = session(&spec(), 512);
    assert!(read(&good, &opts()).is_ok());
    for cut in (0..good.len()).step_by(61) {
        let _ = read(&good[..cut], &opts());
    }
    let mut x: u32 = 0x9e37_79b9;
    for round in 0..1500 {
        let mut bytes = good.clone();
        for _ in 0..(1 + round % 6) {
            x = x.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            let at = (x as usize >> 4) % bytes.len();
            bytes[at] = (x >> 24) as u8;
        }
        if let Ok(imp) = read(&bytes, &opts()) {
            assert!(imp.tracks.len() <= 16);
        }
    }
}

#[test]
fn a_composition_that_uses_itself_ends() {
    use crate::testwrite::{Obj, Val, class, file};
    // A composition whose only clip plays the composition itself.
    let id: Vec<u8> = (0..32).map(|i| i as u8 + 1).collect();
    let clip = Obj::new(class(0x11))
        .with(pid::COMPONENT_DATA_DEF, Val::Weak(crate::testwrite::SOUND))
        .i64(pid::COMPONENT_LENGTH, 10)
        .data(pid::SOURCEREF_ID, id.clone())
        .u32(pid::SOURCEREF_SLOT, 1)
        .i64(pid::SOURCECLIP_START, 0);
    let slot = Obj::new(class(0x3B)).u32(pid::SLOT_ID, 1).rational(pid::TIMELINE_SLOT_EDIT_RATE, 25, 1).with(pid::SLOT_SEGMENT, Val::Strong(clip));
    let mob = Obj::new(class(0x35)).data(pid::MOB_ID, id).with(pid::MOB_SLOTS, Val::Vector(vec![slot]));
    let dict = Obj::new(class(0x22)).with(
        pid::DICT_DATA_DEFS,
        Val::Set(vec![Obj::new(class(0x1B)).data(pid::DEF_IDENTIFICATION, crate::testwrite::SOUND.to_vec()).string(pid::DEF_NAME, "Sound")]),
    );
    let header = Obj::new(class(0x2F))
        .with(pid::HEADER_CONTENT, Val::Strong(Obj::new(class(0x18)).with(pid::CONTENT_MOBS, Val::Set(vec![mob]))))
        .with(pid::HEADER_DICTIONARY, Val::Strong(dict));
    let imp = read(&file(header, 512), &opts()).unwrap();
    assert_eq!(imp.tracks.len(), 1);
    assert!(imp.tracks[0].clips.is_empty());
    assert!(imp.skipped.iter().any(|s| s.contains("too deeply")), "{:?}", imp.skipped);
}

#[test]
fn not_aaf_is_an_error() {
    assert!(read(b"", &opts()).is_err());
    assert!(read(&[0u8; 2048], &opts()).is_err());
    // A compound file without an AAF header.
    let other = crate::cfb::writer::write([0; 16], &[crate::cfb::writer::Node::Stream { name: "x".into(), data: vec![1] }], 512, true);
    assert!(read(&other, &opts()).is_err());
}

#[test]
fn summaries_and_pcm_headers() {
    let wav = pcm_wav(&[1, 0, 2, 0, 3], 1, 44_100, 16);
    assert_eq!(wave_format(&wav), Some((1, 44_100, 16)));
    assert_eq!(wav.len(), 44 + 4, "a partial sample is dropped");
    let mut aiff = b"FORM\0\0\0\x26AIFFCOMM\0\0\0\x12".to_vec();
    aiff.extend([0, 2, 0, 0, 0, 10, 0, 24]);
    aiff.extend([0x40, 0x0E, 0xAC, 0x44, 0, 0, 0, 0, 0, 0]);
    assert_eq!(aifc_format(&aiff), Some((2, 44_100, 24)));
    assert_eq!(aifc_format(&aiff[..20]), None);
    assert_eq!(wave_format(b"RIFF"), None);
    assert_eq!(to_db(1.0), 0.0);
    assert_eq!(to_db(0.0), -144.0);
    assert_eq!(pan_of(0.25), -0.5);
}

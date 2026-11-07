//! A session longer than the block cache plays without a gap: the disk thread keeps the seconds
//! ahead of the playhead loaded and evicts what was played longest ago, and the cache never holds
//! more than it is allowed. (Its own test binary: it lowers the process-wide cache limit.)

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat, decode, encode};
use soundcraft_model::disk::{self, BLOCK, DiskFile};
use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, SourceAudio, SourceId, TrackKind};
use std::sync::Arc;
use std::time::Duration;

#[test]
fn a_session_longer_than_the_cache_streams_without_underruns() {
    let d = std::env::temp_dir().join(format!("freedawm-disk-evict-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    std::fs::create_dir_all(&d).unwrap();
    // 60 blocks (~41 s) of mono audio against a cache of 40 pages.
    let frames = 60 * BLOCK;
    let buf = AudioBuffer { sample_rate: 48_000, channels: vec![(0..frames).map(|i| ((i % 4801) as f32 / 4801.0) - 0.5).collect()] };
    let bytes =
        encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Float32, dither: false, bwf: None, bitrate_kbps: 0 }).unwrap();
    let path = d.join("long.wav");
    std::fs::write(&path, &bytes).unwrap();
    let want = decode(&bytes, Some("wav")).unwrap().1;
    disk::set_limit_bytes(40 * BLOCK * 4);
    let audio = Arc::new(SourceAudio::from_disk_scanned(DiskFile::open(&path).unwrap()));
    let mut s = Session::default();
    let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Long"));
    s.pool.insert(SourceId(1), Arc::clone(&audio));
    s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(ClipId(2), "Long", SourceId(1), 0, 0, frames as i64));
    let s = Arc::new(s);
    disk::focus(Some(Arc::clone(&s)));
    assert!(disk::prepare(&s, 0, 1.0, Duration::from_secs(10)));
    let before = disk::underruns();
    // Play it through at 4× real time, reading every sample as the audio thread does.
    let step = 4800usize;
    let mut pos = 0usize;
    let mut wrong = 0usize;
    while pos < frames {
        disk::set_playhead(pos as i64, true);
        let mut c = audio.cursor(0, true);
        let mut out = vec![0.0f32; step.min(frames - pos)];
        if let soundcraft_model::SampleCursor::Disk(_) = &c {
            c.fill(pos as i64, &mut out);
        }
        wrong += out.iter().zip(&want.channels[0][pos..]).filter(|(a, b)| a.to_bits() != b.to_bits()).count();
        let (held, limit) = disk::usage();
        assert!(held <= limit, "the cache stays within its limit: {held} > {limit}");
        pos += step;
        std::thread::sleep(Duration::from_millis(25));
    }
    assert_eq!(disk::underruns() - before, 0, "no block was late");
    assert_eq!(wrong, 0, "every sample read is the file's");
    disk::focus(None);
    let _ = std::fs::remove_dir_all(&d);
}

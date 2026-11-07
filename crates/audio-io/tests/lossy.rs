//! MP3 and Ogg Opus export, and Opus import: round trips through `decode`, timing, streaming.

use soundcraft_audio_io::{AudioBuffer, EncodeOptions, FileEncoder, FileFormat, SampleFormat, decode, detect_format, encode, probe};
use std::io::Cursor;

/// A test mix: a chord, a decaying click every 0.25 s (for timing) and a little noise.
fn music(sample_rate: u32, channels: usize, frames: usize) -> AudioBuffer {
    let sr = sample_rate as f32;
    let mut seed = 0x2545_f491u32;
    let channels = (0..channels)
        .map(|c| {
            (0..frames)
                .map(|i| {
                    let t = i as f32 / sr;
                    seed ^= seed << 13;
                    seed ^= seed >> 17;
                    seed ^= seed << 5;
                    let noise = (seed as f32 / u32::MAX as f32 - 0.5) * 0.02;
                    let chord: f32 = [220.0f32, 277.2, 329.6].iter().map(|f| (std::f32::consts::TAU * f * (1.0 + c as f32 * 0.003) * t).sin()).sum();
                    let beat = (i as f32 / (sr * 0.25)).fract() * 0.25;
                    let click = (std::f32::consts::TAU * 1500.0 * beat).sin() * (-beat * 60.0).exp();
                    0.15 * chord + 0.4 * click + noise
                })
                .collect()
        })
        .collect();
    AudioBuffer { sample_rate, channels }
}

fn opts(format: FileFormat, kbps: u32) -> EncodeOptions {
    EncodeOptions { format, bitrate_kbps: kbps, ..Default::default() }
}

/// The offset (in samples, positive = the decoded audio is late) that best lines `got` up with
/// `want` over a stretch past the start, and the signal-to-error ratio there in dB.
fn align(want: &[f32], got: &[f32]) -> (i64, f64) {
    let (from, len, reach) = (want.len() / 4, want.len() / 2, 2048i64);
    let w = &want[from..from + len];
    let mut best = (0i64, f64::MIN);
    for off in -reach..=reach {
        let start = from as i64 + off;
        if start < 0 || start as usize + len > got.len() {
            continue;
        }
        let g = &got[start as usize..start as usize + len];
        let dot: f64 = w.iter().zip(g).map(|(a, b)| f64::from(*a) * f64::from(*b)).sum();
        if dot > best.1 {
            best = (off, dot);
        }
    }
    let start = (from as i64 + best.0) as usize;
    let g = &got[start..start + len];
    let sig: f64 = w.iter().map(|a| f64::from(*a).powi(2)).sum();
    let err: f64 = w.iter().zip(g).map(|(a, b)| (f64::from(*a) - f64::from(*b)).powi(2)).sum();
    (best.0, 10.0 * (sig / err.max(1e-20)).log10())
}

/// `buf` converted to `rate` (the decoded file's rate), for comparison.
fn at_rate(buf: &AudioBuffer, rate: u32) -> AudioBuffer {
    if buf.sample_rate == rate {
        return buf.clone();
    }
    music(rate, buf.channels.len(), (buf.frames() as u64 * u64::from(rate) / u64::from(buf.sample_rate)) as usize)
}

#[test]
fn mp3_round_trips_in_time_at_every_session_rate() {
    for (rate, file_rate) in [(44_100, 44_100), (48_000, 48_000), (96_000, 48_000), (88_200, 44_100)] {
        for channels in [1, 2] {
            let src = music(rate, channels, rate as usize * 2);
            let bytes = encode(&src, &opts(FileFormat::Mp3, 192)).unwrap();
            assert_eq!(detect_format(&bytes, None), FileFormat::Mp3);
            let (info, got) = decode(&bytes, Some("mp3")).unwrap();
            assert_eq!((info.sample_rate, info.channels as usize), (file_rate, channels), "{rate} Hz");
            let want = at_rate(&src, file_rate);
            // The Info frame makes the file gapless: the decoder trims delay and padding.
            assert!(got.frames().abs_diff(want.frames()) <= 1, "{rate} Hz {channels} ch: {} vs {} frames", got.frames(), want.frames());
            for (c, (w, g)) in want.channels.iter().zip(&got.channels).enumerate() {
                let (off, snr) = align(w, g);
                assert!(off.abs() <= 1, "{rate} Hz ch {c}: offset {off}");
                assert!(snr > 12.0, "{rate} Hz ch {c}: {snr:.1} dB");
            }
        }
    }
}

#[test]
fn opus_round_trips_in_time_at_every_bitrate_and_rate() {
    for (rate, kbps, channels) in
        [(48_000, 64, 1), (48_000, 64, 2), (48_000, 160, 2), (48_000, 256, 2), (48_000, 256, 1), (44_100, 128, 2), (96_000, 96, 2)]
    {
        let src = music(rate, channels, rate as usize * 2);
        let bytes = encode(&src, &opts(FileFormat::Opus, kbps)).unwrap();
        assert_eq!(detect_format(&bytes, None), FileFormat::Opus);
        let (info, got) = decode(&bytes, None).unwrap();
        assert_eq!((info.format, info.sample_format), (FileFormat::Opus, SampleFormat::Compressed));
        assert_eq!((info.sample_rate, info.channels as usize), (48_000, channels));
        let want = at_rate(&src, 48_000);
        assert!(got.frames().abs_diff(want.frames()) <= 1, "{rate} Hz {kbps}k: {} vs {} frames", got.frames(), want.frames());
        for (c, (w, g)) in want.channels.iter().zip(&got.channels).enumerate() {
            let (off, snr) = align(w, g);
            assert!(off.abs() <= 1, "{rate} Hz {kbps}k {channels} ch: offset {off} (pre-skip does not match the encoder's delay)");
            assert!(snr > 8.0, "{rate} Hz {kbps}k ch {c}: {snr:.1} dB");
        }
        let probed = probe(&bytes, Some("opus")).unwrap();
        assert_eq!(probed.frames, info.frames);
    }
}

#[test]
fn more_than_two_channels_are_refused_with_a_way_out() {
    for format in [FileFormat::Mp3, FileFormat::Opus] {
        let err = encode(&music(48_000, 6, 4800), &opts(format, 0)).unwrap_err().to_string();
        assert!(err.contains("fold the bounce down to stereo"), "{format:?}: {err}");
    }
}

#[test]
fn bitrates_out_of_range_are_refused() {
    assert!(encode(&music(48_000, 2, 4800), &opts(FileFormat::Opus, 600)).is_err());
    assert!(soundcraft_audio_io::check_kbps(FileFormat::Mp3, 400).is_err());
    assert!(soundcraft_audio_io::check_kbps(FileFormat::Mp3, 320).is_ok());
    assert!(soundcraft_audio_io::check_kbps(FileFormat::Opus, 6).is_ok());
}

#[test]
fn an_empty_bounce_is_still_a_valid_file() {
    for (format, ext) in [(FileFormat::Mp3, "mp3"), (FileFormat::Opus, "opus")] {
        for rate in [44_100, 96_000] {
            let bytes = encode(&AudioBuffer::new(rate, 2, 0), &opts(format, 0)).unwrap();
            assert!(!bytes.is_empty(), "{format:?}");
            let decoded = decode(&bytes, Some(ext));
            assert!(decoded.as_ref().is_ok_and(|(_, b)| b.frames() <= 1152 * 2), "{format:?} {rate}: {:?}", decoded.map(|(i, _)| i));
        }
    }
}

fn streamed(buf: &AudioBuffer, o: &EncodeOptions, sizes: &[usize]) -> Vec<u8> {
    let mut enc = FileEncoder::new(Cursor::new(Vec::new()), o, buf.channels.len(), buf.sample_rate, None).unwrap();
    let mut start = 0;
    for len in sizes.iter().copied().cycle() {
        if start >= buf.frames() {
            break;
        }
        let len = len.min(buf.frames() - start);
        let block: Vec<Vec<f32>> = buf.channels.iter().map(|c| c[start..start + len].to_vec()).collect();
        enc.write(&block, len).unwrap();
        start += len;
    }
    enc.finish().unwrap().into_inner()
}

#[test]
fn block_size_does_not_change_the_file() {
    for (format, kbps, rate) in
        [(FileFormat::Mp3, 320, 44_100), (FileFormat::Mp3, 128, 96_000), (FileFormat::Opus, 128, 48_000), (FileFormat::Opus, 96, 44_100)]
    {
        let src = music(rate, 2, rate as usize);
        let o = opts(format, kbps);
        let whole = encode(&src, &o).unwrap();
        assert_eq!(streamed(&src, &o, &[1, 7, 4095, 4097, 333]), whole, "{format:?} {kbps}k {rate} Hz");
        assert_eq!(streamed(&src, &o, &[src.frames()]), whole, "{format:?} {kbps}k {rate} Hz");
    }
}

#[test]
fn the_requested_bitrate_sets_the_size() {
    let src = music(48_000, 2, 48_000 * 4);
    let size = |format, kbps| encode(&src, &opts(format, kbps)).unwrap().len() as f64 * 8.0 / 4.0 / 1000.0;
    let (mp3_128, mp3_320) = (size(FileFormat::Mp3, 128), size(FileFormat::Mp3, 320));
    assert!((mp3_128 - 128.0).abs() < 16.0 && (mp3_320 - 320.0).abs() < 32.0, "MP3: {mp3_128:.0} / {mp3_320:.0} kbps");
    // Opus is variable-rate: the average lands near the target.
    let (opus_64, opus_192) = (size(FileFormat::Opus, 64), size(FileFormat::Opus, 192));
    assert!(opus_64 < opus_192 && opus_64 < 100.0 && opus_192 > 120.0, "Opus: {opus_64:.0} / {opus_192:.0} kbps");
}

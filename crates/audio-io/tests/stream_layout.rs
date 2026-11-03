//! `stream_layout` + `StreamLayout::decode_into` read the same samples `decode` does, block by
//! block, from the file's first bytes alone.

use soundcraft_audio_io::{AudioBuffer, BitDepth, BwfInfo, EncodeOptions, FileFormat, decode, encode, stream_layout, wav_float_header};

fn signal(channels: usize, frames: usize) -> AudioBuffer {
    AudioBuffer {
        sample_rate: 48_000,
        channels: (0..channels).map(|c| (0..frames).map(|i| (i as f32 * 0.013 + c as f32).sin() * 0.9).collect()).collect(),
    }
}

fn check(bytes: &[u8], prefix: usize) {
    let (_, whole) = decode(bytes, Some("wav")).unwrap();
    let l = stream_layout(&bytes[..prefix.min(bytes.len())], bytes.len() as u64).expect("streamable");
    assert_eq!(l.info.frames as usize, whole.frames());
    assert_eq!(usize::from(l.info.channels), whole.num_channels());
    let fb = l.frame_bytes();
    // Read in odd-sized pieces, as the disk reader does with its blocks.
    let mut got: Vec<Vec<f32>> = vec![vec![0.0; whole.frames()]; whole.num_channels()];
    let mut at = 0usize;
    while at < whole.frames() {
        let n = 777.min(whole.frames() - at);
        let start = l.data_start as usize + at * fb;
        let mut outs: Vec<&mut [f32]> = got.iter_mut().map(|c| &mut c[at..at + n]).collect();
        assert_eq!(l.decode_into(&bytes[start..start + n * fb], &mut outs), n);
        at += n;
    }
    for (a, b) in got.iter().zip(&whole.channels) {
        assert!(a.iter().zip(b).all(|(x, y)| x.to_bits() == y.to_bits()), "bit-identical");
    }
}

#[test]
fn every_native_depth_streams_bit_identically() {
    let buf = signal(2, 10_000);
    for depth in [BitDepth::Int16, BitDepth::Int24, BitDepth::Int32, BitDepth::Float32] {
        let bwf = Some(BwfInfo { description: "x".into(), ..BwfInfo::default() });
        let bytes = encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: depth, dither: false, bwf }).unwrap();
        check(&bytes, 64 * 1024);
    }
}

#[test]
fn a_recording_header_streams() {
    let buf = signal(1, 5_000);
    let mut bytes = wav_float_header(1, 48_000, 5_000).unwrap();
    for x in &buf.channels[0] {
        bytes.extend_from_slice(&x.to_le_bytes());
    }
    check(&bytes, 4096);
}

#[test]
fn what_cannot_stream_says_so() {
    assert!(stream_layout(b"not a wav at all", 16).is_none());
    let buf = signal(1, 100);
    let bytes = encode(&buf, &EncodeOptions { format: FileFormat::Wav, bit_depth: BitDepth::Int16, dither: false, bwf: None }).unwrap();
    // The header cut before the data chunk starts.
    assert!(stream_layout(&bytes[..20], bytes.len() as u64).is_none());
}

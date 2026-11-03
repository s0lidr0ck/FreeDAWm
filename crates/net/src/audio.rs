//! Remote audio takes (spec `docs/superpowers/specs/2026-11-02-remote-audio-recording-design.md`).
//!
//! A remote recorder records its armed audio tracks locally at full quality, as any take. At
//! Stop each track's take file becomes a transfer file ([`encode_transfer`]): FLAC at the booth
//! project's bit depth (lossless, about half the size of WAV), or 32-bit float WAV when the
//! project records float (FLAC has no float). It travels in chunks, resumable
//! (`take.offer` / `take.chunk`, see `docs/control-protocol.md`), and the booth turns it into a
//! normal take file of its own project ([`align`] then [`encode_landed`]): at the session rate
//! (converted when the remote recorded at another one) and bit depth, each sample moved onto the
//! booth's clock through the take's sync points (clock drift corrected by a windowed-sinc
//! interpolation; when the clocks agree to within half a sample over the whole take, the samples
//! are copied as they are, so a take at the session rate lands sample-exact).
//!
//! Clips (samples at full scale) are found on both sides ([`clip_frames`]): the performer's
//! screen and the booth's say so the moment it happens (the live feed) and when the take lands.

use soundcraft_audio_io::{AudioBuffer, BitDepth, EncodeOptions, FileFormat};

use crate::sync::ClockMap;

/// A sample at least this loud counts as a clip (about -0.01 dBFS).
pub const CLIP_LEVEL: f32 = 0.999;
/// Clips closer than this (seconds) are one clip.
pub const CLIP_GAP: f64 = 0.1;
/// Bytes per `take.chunk` (before base64).
pub const CHUNK: usize = 192 * 1024;
/// Chunks sent ahead of the booth's answers.
pub const WINDOW: u64 = 4;

/// The wire's bit depth of a project setting: 16, 24, or 32 (float).
pub fn bits_of(depth: soundcraft_model::BitDepthSetting) -> u16 {
    match depth {
        soundcraft_model::BitDepthSetting::Int16 => 16,
        soundcraft_model::BitDepthSetting::Int24 => 24,
        soundcraft_model::BitDepthSetting::Float32 => 32,
    }
}

fn depth_of(bits: u16) -> BitDepth {
    match bits {
        16 => BitDepth::Int16,
        32 => BitDepth::Float32,
        _ => BitDepth::Int24,
    }
}

/// How a take with project bit depth `bits` crosses the network: its format and file extension.
pub fn transfer_format(bits: u16) -> (FileFormat, &'static str) {
    if bits == 32 { (FileFormat::Wav, "wav") } else { (FileFormat::Flac, "flac") }
}

/// The transfer file of a take recorded into `buf`, at project bit depth `bits`.
pub fn encode_transfer(buf: &AudioBuffer, bits: u16) -> Result<Vec<u8>, String> {
    let (format, _) = transfer_format(bits);
    let opts = EncodeOptions { format, bit_depth: depth_of(bits), dither: false, bwf: None };
    soundcraft_audio_io::encode(buf, &opts).map_err(|e| e.to_string())
}

/// A landed take as the booth project keeps it: WAV at its bit depth.
pub fn encode_landed(buf: &AudioBuffer, bits: u16) -> Result<Vec<u8>, String> {
    let opts = EncodeOptions { format: FileFormat::Wav, bit_depth: depth_of(bits), dither: false, bwf: None };
    soundcraft_audio_io::encode(buf, &opts).map_err(|e| e.to_string())
}

/// The frames where clips start: a sample at [`CLIP_LEVEL`] or louder on any channel, clips
/// closer than [`CLIP_GAP`] counted once.
pub fn clip_frames(buf: &AudioBuffer) -> Vec<u64> {
    let gap = (CLIP_GAP * f64::from(buf.sample_rate.max(1))) as usize;
    let frames = buf.channels.iter().map(Vec::len).max().unwrap_or(0);
    let mut out: Vec<u64> = Vec::new();
    let mut last: Option<usize> = None;
    for i in 0..frames {
        let hot = buf.channels.iter().any(|c| c.get(i).is_some_and(|x| x.abs() >= CLIP_LEVEL));
        if !hot {
            continue;
        }
        if last.is_none_or(|l| i.saturating_sub(l) > gap) {
            out.push(i as u64);
        }
        last = Some(i);
    }
    out
}

/// A FNV-1a hash of the transfer file (hex), checked when it has arrived.
pub fn hash(bytes: &[u8]) -> String {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    for b in bytes {
        h ^= u64::from(*b);
        h = h.wrapping_mul(0x0100_0000_01b3);
    }
    format!("{h:016x}")
}

/// A take moved onto the booth's take clock.
#[derive(Debug, Clone, PartialEq)]
pub struct Aligned {
    pub buf: AudioBuffer,
    /// The booth take-clock sample (session rate, rolled since its transport started) of the
    /// buffer's first frame.
    pub first: i64,
    /// Converted from another sample rate.
    pub converted: bool,
    /// The drift corrected over the take, in samples (0 when copied as it is).
    pub drift: f64,
}

/// Taps either side of the interpolation kernel.
const TAPS: usize = 16;
/// Kernel phases tabulated per sample.
const PHASES: usize = 1024;

/// A Lanczos kernel (`a` = [`TAPS`]), [`PHASES`] fractions of a sample.
fn kernel() -> Vec<[f32; 2 * TAPS]> {
    let a = TAPS as f64;
    let sinc = |x: f64| if x.abs() < 1e-12 { 1.0 } else { (std::f64::consts::PI * x).sin() / (std::f64::consts::PI * x) };
    (0..=PHASES)
        .map(|p| {
            let frac = p as f64 / PHASES as f64;
            let mut k = [0f32; 2 * TAPS];
            let mut sum = 0.0;
            for (i, v) in k.iter_mut().enumerate() {
                // Tap i sits at input offset i - TAPS + 1 from the integer part.
                let x = (i as f64 - TAPS as f64 + 1.0) - frac;
                let w = if x.abs() < a { sinc(x) * sinc(x / a) } else { 0.0 };
                *v = w as f32;
                sum += w;
            }
            if sum.abs() > 1e-9 {
                for v in &mut k {
                    *v = (f64::from(*v) / sum) as f32;
                }
            }
            k
        })
        .collect()
}

/// The most frames a landed take may have (24 h at 192 kHz).
const MAX_FRAMES: i64 = 24 * 3600 * 192_000;

/// Moves a take recorded on a remote PC onto the booth's take clock at `host_rate`: `t0` is the
/// remote take-clock time of its first frame, `map` the remote clock to the booth's (none: the
/// same clock). See the module docs.
pub fn align(buf: AudioBuffer, t0: f64, map: Option<&ClockMap>, host_rate: u32) -> Aligned {
    let host_rate = host_rate.max(1);
    let converted = buf.sample_rate != host_rate && buf.sample_rate > 0;
    let buf = if converted {
        AudioBuffer { sample_rate: host_rate, channels: soundcraft_dsp::offline::resample(&buf.channels, buf.sample_rate, host_rate) }
    } else {
        AudioBuffer { sample_rate: host_rate, channels: buf.channels }
    };
    let r = f64::from(host_rate);
    let frames = buf.channels.iter().map(Vec::len).max().unwrap_or(0);
    let t0 = if t0.is_finite() { t0 } else { 0.0 };
    let booth = |local: f64| map.map_or(local, |m| m.map(local));
    let b0 = booth(t0) * r;
    if frames < 2 {
        return Aligned { buf, first: b0.round().clamp(-9.0e15, 9.0e15) as i64, converted, drift: 0.0 };
    }
    let last = (frames - 1) as f64;
    let bl = booth(t0 + last / r) * r;
    let drift = (bl - b0) - last;
    // Half a sample over the whole take: copied as it is.
    let unmapped = |x: f64| map.map_or(x, |m| m.unmap(x));
    let mid = booth(t0 + last / (2.0 * r)) * r;
    let bend = (mid - b0) - last / 2.0;
    if drift.abs() < 0.5 && bend.abs() < 0.5 {
        return Aligned { buf, first: b0.round().clamp(-9.0e15, 9.0e15) as i64, converted, drift: 0.0 };
    }
    let first = b0.ceil().clamp(-9.0e15, 9.0e15) as i64;
    let end = bl.floor().clamp(-9.0e15, 9.0e15) as i64;
    let n = end.saturating_sub(first).saturating_add(1).clamp(0, MAX_FRAMES) as usize;
    let table = kernel();
    // Where each output frame reads the input (input frames, fractional).
    let pos: Vec<f64> = (0..n).map(|j| (unmapped((first as f64 + j as f64) / r) - t0) * r).collect();
    let channels = buf
        .channels
        .iter()
        .map(|c| {
            pos.iter()
                .map(|&p| {
                    if !p.is_finite() {
                        return 0.0;
                    }
                    let base = p.floor();
                    let frac = p - base;
                    let phase = ((frac * PHASES as f64).round() as usize).min(PHASES);
                    let Some(k) = table.get(phase) else { return 0.0 };
                    let i0 = base as i64 - TAPS as i64 + 1;
                    let mut acc = 0f32;
                    for (t, w) in k.iter().enumerate() {
                        let i = i0 + t as i64;
                        if let Ok(i) = usize::try_from(i)
                            && let Some(x) = c.get(i)
                        {
                            acc += x * w;
                        }
                    }
                    acc
                })
                .collect()
        })
        .collect();
    Aligned { buf: AudioBuffer { sample_rate: host_rate, channels }, first, converted, drift }
}

const B64: &[u8; 64] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789+/";

/// Standard base64 (with padding), for chunks in JSON lines.
pub fn b64_encode(data: &[u8]) -> String {
    let mut out = String::with_capacity(data.len().div_ceil(3) * 4);
    for chunk in data.chunks(3) {
        let b = [chunk.first().copied().unwrap_or(0), chunk.get(1).copied().unwrap_or(0), chunk.get(2).copied().unwrap_or(0)];
        let n = (u32::from(b[0]) << 16) | (u32::from(b[1]) << 8) | u32::from(b[2]);
        for i in 0..4 {
            if i <= chunk.len() {
                let idx = ((n >> (18 - 6 * i)) & 63) as usize;
                out.push(char::from(B64.get(idx).copied().unwrap_or(b'A')));
            } else {
                out.push('=');
            }
        }
    }
    out
}

/// Base64 back to bytes; `None` when it isn't base64.
pub fn b64_decode(s: &str) -> Option<Vec<u8>> {
    let val = |c: u8| -> Option<u32> {
        match c {
            b'A'..=b'Z' => Some(u32::from(c - b'A')),
            b'a'..=b'z' => Some(u32::from(c - b'a') + 26),
            b'0'..=b'9' => Some(u32::from(c - b'0') + 52),
            b'+' => Some(62),
            b'/' => Some(63),
            _ => None,
        }
    };
    let bytes = s.as_bytes();
    if !bytes.len().is_multiple_of(4) {
        return None;
    }
    let mut out = Vec::with_capacity(bytes.len() / 4 * 3);
    for q in bytes.chunks(4) {
        let pad = q.iter().rev().take_while(|c| **c == b'=').count();
        if pad > 2 {
            return None;
        }
        let mut n = 0u32;
        for (i, c) in q.iter().enumerate() {
            let v = if i >= 4 - pad { 0 } else { val(*c)? };
            n = (n << 6) | v;
        }
        let got = [(n >> 16) as u8, (n >> 8) as u8, n as u8];
        out.extend_from_slice(got.get(..3 - pad)?);
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::sync::SyncPoint;

    fn tone(rate: u32, frames: usize, hz: f64) -> AudioBuffer {
        let c: Vec<f32> = (0..frames).map(|i| (0.5 * (2.0 * std::f64::consts::PI * hz * i as f64 / f64::from(rate)).sin()) as f32).collect();
        AudioBuffer { sample_rate: rate, channels: vec![c] }
    }

    #[test]
    fn base64_round_trips() {
        for n in 0..20 {
            let data: Vec<u8> = (0..n).map(|i| (i * 37 + 11) as u8).collect();
            assert_eq!(b64_decode(&b64_encode(&data)).unwrap(), data);
        }
        assert_eq!(b64_encode(b"Man"), "TWFu");
        assert_eq!(b64_encode(b"Ma"), "TWE=");
        assert!(b64_decode("TW!u").is_none());
        assert!(b64_decode("abc").is_none());
    }

    #[test]
    fn transfer_is_lossless_at_the_project_depth() {
        let buf = tone(48_000, 4_800, 440.0);
        for bits in [16u16, 24, 32] {
            let bytes = encode_transfer(&buf, bits).unwrap();
            let (info, back) = soundcraft_audio_io::decode(&bytes, Some(transfer_format(bits).1)).unwrap();
            assert_eq!(info.frames, 4_800);
            let q = if bits == 32 { 1e-7 } else { 1.0 / f32::powi(2.0, i32::from(bits) - 1) };
            let worst = back.channels[0].iter().zip(&buf.channels[0]).map(|(a, b)| (a - b).abs()).fold(0f32, f32::max);
            assert!(worst <= q, "{bits} bits: {worst}");
            // Landed at the same depth: exactly what came over.
            let landed = encode_landed(&back, bits).unwrap();
            let (_, again) = soundcraft_audio_io::decode(&landed, Some("wav")).unwrap();
            assert_eq!(again.channels[0], back.channels[0], "{bits} bits");
        }
    }

    #[test]
    fn clips_are_found_once_per_burst() {
        let mut buf = tone(48_000, 48_000, 100.0);
        for i in [1_000usize, 1_010, 1_500, 30_000] {
            buf.channels[0][i] = 1.0;
        }
        assert_eq!(clip_frames(&buf), vec![1_000, 30_000]);
        assert!(clip_frames(&tone(48_000, 1_000, 100.0)).is_empty());
    }

    #[test]
    fn a_take_at_the_session_rate_without_drift_lands_sample_exact() {
        let buf = tone(48_000, 9_600, 1_000.0);
        // The remote's clock is 2.25 s behind the booth's; its first frame at remote 0.5 s.
        let map = ClockMap::new(&[SyncPoint { local: 0.0, booth: 2.25, rtt: 0.001 }, SyncPoint { local: 100.0, booth: 102.25, rtt: 0.001 }]).unwrap();
        let a = align(buf.clone(), 0.5, Some(&map), 48_000);
        assert_eq!(a.first, (2.75 * 48_000.0) as i64);
        assert_eq!(a.buf.channels, buf.channels, "copied as it is");
        assert!(!a.converted);
    }

    #[test]
    fn drift_is_corrected_onto_the_booth_clock() {
        // The remote's device runs 200 ppm fast: 2 s of its frames are 1.9996 s on the booth.
        let rate = 48_000u32;
        let hz = 997.0;
        let buf = tone(rate, 2 * rate as usize, hz);
        let slope = 1.0 / 1.0002;
        let pts: Vec<SyncPoint> = (0..5).map(f64::from).map(|l| SyncPoint { local: l, booth: 1.0 + l * slope, rtt: 0.001 }).collect();
        let map = ClockMap::new(&pts).unwrap();
        let a = align(buf, 0.0, Some(&map), rate);
        assert!(a.drift.abs() > 10.0, "{}", a.drift);
        assert_eq!(a.first, 48_000);
        // Booth frame `first + j` holds the remote's tone at remote time unmap(booth time).
        let r = f64::from(rate);
        let mut worst = 0f64;
        for j in (200..a.buf.channels[0].len() - 200).step_by(97) {
            let booth = (a.first as f64 + j as f64) / r;
            let local = map.unmap(booth);
            let want = 0.5 * (2.0 * std::f64::consts::PI * hz * local).sin();
            worst = worst.max((f64::from(a.buf.channels[0][j]) - want).abs());
        }
        assert!(worst < 1e-3, "interpolation error {worst}");
    }

    #[test]
    fn another_rate_is_converted() {
        let buf = tone(44_100, 44_100, 440.0);
        let a = align(buf, 0.0, None, 48_000);
        assert!(a.converted);
        assert_eq!(a.buf.sample_rate, 48_000);
        assert!((a.buf.channels[0].len() as i64 - 48_000).abs() <= 1);
        assert_eq!(a.first, 0);
    }
}

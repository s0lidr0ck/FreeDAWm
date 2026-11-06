//! Opus in Ogg (RFC 7845): export through `rusty-opus`, a pure-Rust encoder, and import through
//! its decoder (symphonia has no Opus decoder).
//!
//! Mono and stereo only (channel mapping family 0). The codec runs at 48 kHz; other session
//! rates are converted on the way in, and the file records the original rate.

use std::io::{Seek, Write};

use crate::ogg::PageWriter;
use crate::pcm::MAX_DECODE_SAMPLES;
use crate::resample::Resampler;
use crate::{AudioBuffer, AudioError, AudioInfo, FileFormat, Result, SampleFormat};

/// The codec's rate: Opus timestamps always count 48 kHz samples.
const RATE: u32 = 48_000;
/// 20 ms frames.
const FRAME: usize = 960;
/// The longest Opus packet decodes to 120 ms.
const MAX_FRAME: usize = 5760;
/// Samples the encoder delays its output by, which the decoder skips (`pre_skip`).
pub(crate) const ENCODER_DELAY: usize = 120;
/// The bitrate when none is asked for (both channels together).
pub const DEFAULT_KBPS: u32 = 160;
/// The bitrates Opus can code, in kbps.
pub const KBPS_RANGE: std::ops::RangeInclusive<u32> = 6..=510;
const SERIAL: u32 = 0x4644_4157; // "FDAW"
const VENDOR: &str = "FreeDAWm (rusty-opus)";

fn check(channels: usize) -> Result<()> {
    match channels {
        1 | 2 => Ok(()),
        0 => Err(AudioError::Encode("an Opus file needs at least one channel".into())),
        n => Err(AudioError::Unsupported(format!("Opus export holds mono or stereo, not {n} channels: fold the bounce down to stereo"))),
    }
}

/// Runs the third-party codec, turning a panic inside it into `fail`.
fn guarded<T>(fail: impl FnOnce() -> AudioError, f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(fail()))
}

fn opus_head(channels: usize, input_rate: u32) -> Vec<u8> {
    let mut h = b"OpusHead".to_vec();
    h.push(1); // version
    h.push(channels as u8);
    h.extend_from_slice(&(ENCODER_DELAY as u16).to_le_bytes());
    h.extend_from_slice(&input_rate.to_le_bytes());
    h.extend_from_slice(&0i16.to_le_bytes()); // output gain
    h.push(0); // mapping family 0: mono or stereo
    h
}

fn opus_tags() -> Vec<u8> {
    let mut t = b"OpusTags".to_vec();
    t.extend_from_slice(&(VENDOR.len() as u32).to_le_bytes());
    t.extend_from_slice(VENDOR.as_bytes());
    t.extend_from_slice(&0u32.to_le_bytes());
    t
}

pub(crate) struct Writer<W> {
    out: W,
    channels: usize,
    enc: rusty_opus::OpusEncoder,
    pages: Option<PageWriter>,
    resampler: Option<Resampler>,
    resampled: Vec<Vec<f32>>,
    /// Interleaved 48 kHz input not yet a whole frame.
    pending: Vec<f32>,
    packet: Vec<u8>,
    /// 48 kHz frames taken in, and frames coded (the encoder's output runs `ENCODER_DELAY` behind).
    input: u64,
    coded: u64,
    failed: bool,
}

impl<W: Write + Seek> Writer<W> {
    pub(crate) fn new(mut out: W, channels: usize, sample_rate: u32, kbps: u32) -> Result<Self> {
        check(channels)?;
        if sample_rate == 0 {
            return Err(AudioError::Encode("an Opus file needs a sample rate".into()));
        }
        let kbps = if kbps == 0 { DEFAULT_KBPS } else { kbps };
        if !KBPS_RANGE.contains(&kbps) {
            return Err(AudioError::Unsupported(format!("Opus codes 6 to 510 kbps, not {kbps}")));
        }
        let mut enc = guarded(
            || AudioError::Encode("the Opus encoder failed to start".into()),
            || {
                rusty_opus::OpusEncoder::new(RATE as i32, channels, rusty_opus::Application::Audio)
                    .map_err(|e| AudioError::Encode(format!("Opus: {e:?}")))
            },
        )?;
        enc.bitrate_bps = (kbps * 1000) as i32;
        enc.complexity = 10;
        enc.signal_type = Some(rusty_opus::SignalType::Music);
        let mut pages = PageWriter::new(SERIAL);
        pages.header_packet(&mut out, &opus_head(channels, sample_rate))?;
        pages.header_packet(&mut out, &opus_tags())?;
        Ok(Writer {
            out,
            channels,
            enc,
            pages: Some(pages),
            resampler: (sample_rate != RATE).then(|| Resampler::new(sample_rate, RATE, channels)),
            resampled: vec![Vec::new(); channels],
            pending: Vec::with_capacity(FRAME * channels),
            packet: vec![0; 1500],
            input: 0,
            coded: 0,
            failed: false,
        })
    }

    /// Appends `frames` frames of planar audio (missing samples are silence).
    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        let r = match self.resampler.as_mut() {
            Some(rs) => {
                for c in &mut self.resampled {
                    c.clear();
                }
                rs.process(block, frames, &mut self.resampled);
                let planar = std::mem::take(&mut self.resampled);
                let n = planar.first().map_or(0, Vec::len);
                self.input += n as u64;
                let r = self.queue(&planar, n, false);
                self.resampled = planar;
                r
            }
            None => {
                self.input += frames as u64;
                self.queue(block, frames, false)
            }
        };
        self.failed = r.is_err();
        r
    }

    /// Takes in `frames` 48 kHz frames and codes every whole 20 ms frame; with `last`, pads the
    /// remainder with silence and codes it too.
    fn queue(&mut self, block: &[Vec<f32>], frames: usize, last: bool) -> Result<()> {
        let ch = self.channels;
        for i in 0..frames {
            for c in 0..ch {
                self.pending.push(block.get(c).and_then(|s| s.get(i)).copied().unwrap_or(0.0));
            }
            if self.pending.len() == FRAME * ch {
                self.code()?;
            }
        }
        if last && (!self.pending.is_empty() || self.coded == 0) {
            self.pending.resize(FRAME * ch, 0.0);
            self.code()?;
        }
        Ok(())
    }

    fn code(&mut self) -> Result<()> {
        let (enc, pcm, packet) = (&mut self.enc, &self.pending, &mut self.packet);
        let n = guarded(
            || AudioError::Encode("the Opus encoder failed".into()),
            || enc.encode(pcm, FRAME, packet).map_err(|e| AudioError::Encode(format!("Opus: {e:?}"))),
        )?;
        self.pending.clear();
        self.coded += FRAME as u64;
        let bytes = self.packet.get(..n).ok_or_else(|| AudioError::Encode("Opus packet overran its buffer".into()))?;
        if let Some(p) = self.pages.as_mut() {
            p.packet(&mut self.out, bytes, i64::try_from(self.coded).unwrap_or(i64::MAX))?;
        }
        Ok(())
    }

    /// Codes the rest, ends the stream and hands back `out`, positioned at the end of the file.
    pub(crate) fn finish(mut self) -> Result<W> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        if let Some(mut rs) = self.resampler.take() {
            for c in &mut self.resampled {
                c.clear();
            }
            rs.finish(&mut self.resampled);
            let planar = std::mem::take(&mut self.resampled);
            let n = planar.first().map_or(0, Vec::len);
            self.input += n as u64;
            self.queue(&planar, n, false)?;
        }
        // Silence as long as the encoder's delay pushes the last samples out of it.
        self.queue(&[], ENCODER_DELAY, true)?;
        // The last page's granule ends the audio exactly: the decoder drops the padding past it.
        let end = self.input + ENCODER_DELAY as u64;
        if let Some(p) = self.pages.take() {
            p.finish(&mut self.out, i64::try_from(end).unwrap_or(i64::MAX))?;
        }
        Ok(self.out)
    }
}

/// What the `OpusHead` packet says.
struct Head {
    channels: usize,
    pre_skip: u64,
    gain_db: f32,
}

fn malformed(why: &str) -> AudioError {
    AudioError::Malformed(format!("Opus: {why}"))
}

fn parse_head(p: &[u8]) -> Result<Head> {
    if p.get(..8) != Some(b"OpusHead") {
        return Err(malformed("missing OpusHead"));
    }
    if p.get(8).is_none_or(|&v| v & 0xF0 != 0) {
        return Err(AudioError::Unsupported("Opus: unknown header version".into()));
    }
    let channels = usize::from(*p.get(9).ok_or_else(|| malformed("short OpusHead"))?);
    let pre_skip = u16::from_le_bytes(p.get(10..12).and_then(|b| b.try_into().ok()).ok_or_else(|| malformed("short OpusHead"))?);
    // Bytes 12..16 hold the rate before encoding: informational only, the audio is 48 kHz.
    let gain = i16::from_le_bytes(p.get(16..18).and_then(|b| b.try_into().ok()).ok_or_else(|| malformed("short OpusHead"))?);
    let family = *p.get(18).ok_or_else(|| malformed("short OpusHead"))?;
    if family != 0 || !(1..=2).contains(&channels) {
        return Err(AudioError::Unsupported(format!("Opus: {channels}-channel streams (mapping family {family}) are not supported yet")));
    }
    Ok(Head { channels, pre_skip: u64::from(pre_skip), gain_db: f32::from(gain) / 256.0 })
}

/// Whether `bytes` start an Ogg stream whose first packet is an `OpusHead`.
pub(crate) fn is_opus(bytes: &[u8]) -> bool {
    bytes.get(..4) == Some(b"OggS") && bytes.get(26).is_some_and(|&n| bytes.get(27 + usize::from(n)..35 + usize::from(n)) == Some(b"OpusHead"))
}

fn info(head: &Head, last_granule: i64) -> AudioInfo {
    let frames = u64::try_from(last_granule).unwrap_or(0).saturating_sub(head.pre_skip);
    AudioInfo {
        format: FileFormat::Opus,
        sample_format: SampleFormat::Compressed,
        sample_rate: RATE,
        channels: head.channels as u16,
        frames,
        bwf: None,
    }
}

/// Format, rate, channels and length of an Ogg Opus file.
pub(crate) fn probe(bytes: &[u8]) -> Result<AudioInfo> {
    let read = crate::ogg::read_packets(bytes, usize::MAX, bytes.len())?;
    let head = parse_head(read.packets.first().map_or(&[][..], Vec::as_slice))?;
    Ok(info(&head, read.last_granule))
}

/// Decodes an Ogg Opus file to planar 48 kHz audio, trimmed to its stated length.
pub(crate) fn decode(bytes: &[u8]) -> Result<(AudioInfo, AudioBuffer)> {
    let read = crate::ogg::read_packets(bytes, usize::MAX, bytes.len())?;
    let mut packets = read.packets.into_iter();
    let head = parse_head(packets.next().as_deref().unwrap_or_default())?;
    let info = info(&head, read.last_granule);
    let _tags = packets.next();
    let ch = head.channels;
    // The stated length is only an upper bound: a file can claim more than its packets hold, so
    // the allocation follows the packets (each decodes to at most 120 ms).
    let want = usize::try_from(info.frames.saturating_add(head.pre_skip)).unwrap_or(usize::MAX).min(packets.len().saturating_mul(MAX_FRAME));
    let limit = usize::try_from(MAX_DECODE_SAMPLES).unwrap_or(usize::MAX);
    if want.saturating_mul(ch) > limit {
        return Err(AudioError::TooLarge(format!("{want} frames exceeds the decode limit")));
    }
    let mut dec = guarded(
        || AudioError::Malformed("the Opus decoder failed to start".into()),
        || rusty_opus::OpusDecoder::new(RATE as i32, ch).map_err(|e| AudioError::Malformed(format!("Opus: {e:?}"))),
    )?;
    let mut interleaved: Vec<f32> = Vec::new();
    interleaved.try_reserve(want.saturating_mul(ch)).map_err(|_| AudioError::TooLarge("cannot allocate the decoded Opus audio".into()))?;
    let mut frame = vec![0f32; MAX_FRAME * ch];
    for p in packets {
        if interleaved.len() / ch >= want {
            break;
        }
        let n = guarded(
            || AudioError::Malformed("the Opus decoder failed on malformed input".into()),
            || dec.decode(&p, MAX_FRAME, &mut frame).map_err(|e| AudioError::Malformed(format!("Opus: {e:?}"))),
        )?;
        interleaved.extend_from_slice(frame.get(..n.saturating_mul(ch)).ok_or_else(|| malformed("decoder overran its frame"))?);
    }
    let skip = usize::try_from(head.pre_skip).unwrap_or(usize::MAX).saturating_mul(ch).min(interleaved.len());
    let gain = 10f32.powf(head.gain_db / 20.0);
    let body = interleaved.get(skip..).unwrap_or_default();
    let mut buf = AudioBuffer::from_interleaved(RATE, ch, body);
    for c in &mut buf.channels {
        c.truncate(usize::try_from(info.frames).unwrap_or(usize::MAX));
        if gain != 1.0 {
            c.iter_mut().for_each(|s| *s *= gain);
        }
    }
    let mut info = info;
    info.frames = buf.frames() as u64;
    Ok((info, buf))
}

//! MP3 (MPEG-1/2/2.5 Layer III, CBR) export through `rusty_mp3`, a pure-Rust encoder.
//!
//! The stream opens with an Info frame (the LAME/Xing tag): it carries the frame count and the
//! encoder delay and padding, so players seek and trim the file gaplessly. Its totals are known
//! only at the end, so [`Writer::finish`] writes it over its placeholder at the start of the file.
//!
//! At 256 kbps and below the encoder holds every encoded frame until the end (its bit-reservoir
//! pass spreads bits across them), so memory grows with the compressed size; at 320 kbps it streams.

use std::io::{Seek, SeekFrom, Write};

use crate::resample::Resampler;
use crate::{AudioError, Result};

/// The rates Layer III can store.
const RATES: [u32; 9] = [8_000, 11_025, 12_000, 16_000, 22_050, 24_000, 32_000, 44_100, 48_000];
/// The bitrate when none is asked for.
pub const DEFAULT_KBPS: u32 = 320;
/// Frames handed to the encoder at a time.
const CHUNK: usize = 4096;

/// The rate an MP3 of a `rate` Hz session is written at: the rate itself when MP3 can store it,
/// else 44.1 kHz for the 44.1 family (88.2, 176.4 kHz) and 48 kHz for the rest.
pub fn mp3_rate(rate: u32) -> u32 {
    if RATES.contains(&rate) {
        rate
    } else if rate.is_multiple_of(11_025) {
        44_100
    } else {
        48_000
    }
}

fn check(channels: usize) -> Result<()> {
    match channels {
        1 | 2 => Ok(()),
        0 => Err(AudioError::Encode("an MP3 needs at least one channel".into())),
        n => Err(AudioError::Unsupported(format!("MP3 holds mono or stereo, not {n} channels: fold the bounce down to stereo"))),
    }
}

fn encode_err(e: rusty_mp3::Error) -> AudioError {
    AudioError::Encode(format!("MP3: {e}"))
}

/// Runs the third-party encoder, turning a panic inside it into an error.
fn guarded<T>(f: impl FnOnce() -> Result<T>) -> Result<T> {
    std::panic::catch_unwind(std::panic::AssertUnwindSafe(f)).unwrap_or_else(|_| Err(AudioError::Encode("the MP3 encoder failed".into())))
}

pub(crate) struct Writer<W> {
    out: W,
    start: u64,
    channels: usize,
    rate: u32,
    enc: rusty_mp3::Mp3Encoder,
    resampler: Option<Resampler>,
    /// Planar output of the resampler, waiting to be encoded.
    resampled: Vec<Vec<f32>>,
    interleaved: Vec<f32>,
    failed: bool,
}

impl<W: Write + Seek> Writer<W> {
    pub(crate) fn new(mut out: W, channels: usize, sample_rate: u32, kbps: u32) -> Result<Self> {
        check(channels)?;
        if sample_rate == 0 {
            return Err(AudioError::Encode("an MP3 needs a sample rate".into()));
        }
        let rate = mp3_rate(sample_rate);
        let kbps = if kbps == 0 { DEFAULT_KBPS } else { kbps };
        let enc = rusty_mp3::Mp3Encoder::new(rusty_mp3::Mp3EncoderConfig { bitrate_kbps: kbps, vbr_quality: None });
        let start = out.stream_position()?;
        Ok(Writer {
            out,
            start,
            channels,
            rate,
            enc,
            resampler: (rate != sample_rate).then(|| Resampler::new(sample_rate, rate, channels)),
            resampled: vec![Vec::new(); channels],
            interleaved: Vec::new(),
            failed: false,
        })
    }

    /// Appends `frames` frames of planar audio (missing samples are silence).
    pub(crate) fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        if self.failed {
            return Err(crate::failed_write_error());
        }
        let r = self.write_inner(block, frames);
        self.failed = r.is_err();
        r
    }

    fn write_inner(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        match &mut self.resampler {
            Some(rs) => {
                for c in &mut self.resampled {
                    c.clear();
                }
                rs.process(block, frames, &mut self.resampled);
                let n = self.resampled.first().map_or(0, Vec::len);
                let planar = std::mem::take(&mut self.resampled);
                let r = self.encode(&planar, n);
                self.resampled = planar;
                r
            }
            None => self.encode(block, frames),
        }
    }

    /// Interleaves and encodes `frames` frames, writing out every finished MP3 frame.
    fn encode(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        let mut done = 0;
        while done < frames {
            let n = CHUNK.min(frames - done);
            self.interleaved.clear();
            for i in done..done + n {
                for c in 0..self.channels {
                    self.interleaved.push(block.get(c).and_then(|ch| ch.get(i)).copied().unwrap_or(0.0));
                }
            }
            let (enc, pcm, ch, rate) = (&mut self.enc, &self.interleaved, self.channels as u16, self.rate);
            guarded(|| enc.push_pcm_f32(pcm, ch, rate).map_err(encode_err))?;
            self.drain()?;
            done += n;
        }
        Ok(())
    }

    fn drain(&mut self) -> Result<()> {
        while let Ok(packet) = self.enc.next_packet() {
            self.out.write_all(&packet)?;
        }
        Ok(())
    }

    /// Ends the stream, writes the completed Info frame over its placeholder and hands back
    /// `out`, positioned at the end of the file.
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
            self.encode(&planar, n)?;
        }
        // The encoder learns the format from its first push: give an empty bounce one, so it
        // still writes a valid (silent, minimal) stream.
        let (enc, ch, rate) = (&mut self.enc, self.channels as u16, self.rate);
        guarded(|| enc.push_pcm_f32(&[], ch, rate).map_err(encode_err))?;
        guarded(|| {
            enc.finish();
            Ok(())
        })?;
        self.drain()?;
        if let Some(tag) = self.enc.lame_tag_frame() {
            let end = self.out.stream_position()?;
            self.out.seek(SeekFrom::Start(self.start))?;
            self.out.write_all(tag)?;
            self.out.seek(SeekFrom::Start(end))?;
        }
        Ok(self.out)
    }
}

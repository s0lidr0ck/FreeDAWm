//! SoundCraft audio file I/O.
//!
//! Bytes in, bytes out (no filesystem access):
//!
//! * [`detect_format`] / [`probe`] / [`decode`] read WAV (PCM 8/16/24/32, float 32/64,
//!   `WAVE_FORMAT_EXTENSIBLE`, RF64/BW64, BWF `bext`) and AIFF/AIFF-C (big-endian, `sowt`,
//!   `fl32`/`fl64`) with native readers, and everything else (FLAC, MP3, Ogg Vorbis, AAC/ALAC in
//!   MP4, CAF, Matroska, ADPCM/A-law/µ-law WAV, ...) through `symphonia`.
//! * [`encode`] writes WAV/BWF (RF64 above 4 GiB), AIFF / AIFF-C `fl32`, and FLAC (built-in
//!   encoder), with optional TPDF dither, and MP3 and Ogg Opus through pure-Rust encoders
//!   (`rusty_mp3`, `rusty-opus`), converting the rate where the codec needs to.
//! * Ogg Opus is also read back through `rusty-opus` (symphonia has no Opus decoder).
//! * [`wav_float_header`] heads a float WAV written while it grows (recording).
//! * [`peaks`] builds multi-resolution min/max overviews for waveform drawing.
//! * [`FileEncoder`] writes the same formats block by block into any `Write + Seek`, so a file
//!   of any length never has to fit in memory.
#![deny(clippy::unwrap_used, clippy::expect_used, clippy::panic, clippy::unimplemented, clippy::todo, clippy::unreachable)]

mod aiff;
mod buffer;
mod caf;
pub mod export;
mod flac;
pub mod ixml;
pub mod loop_info;
mod mp3;
mod mp4;
mod ogg;
mod opus;
mod pcm;
pub mod peaks;
mod resample;
mod symph;
mod wav;

pub use mp3::mp3_rate;
pub use resample::Resampler;

pub use buffer::AudioBuffer;
use std::io::{Seek, Write};
pub use wav::{StreamLayout, bext_time_reference_offset, stream_layout};

/// Errors from probing, decoding and encoding.
#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    /// The format, codec or option is valid but not supported.
    #[error("unsupported audio: {0}")]
    Unsupported(String),
    /// The input is corrupt or not what it claims to be.
    #[error("malformed audio: {0}")]
    Malformed(String),
    /// The input (or requested output) exceeds a size limit.
    #[error("audio too large: {0}")]
    TooLarge(String),
    /// Encoding failed (e.g. an empty buffer).
    #[error("audio encode failed: {0}")]
    Encode(String),
    /// Writing the output failed.
    #[error("audio write failed: {0}")]
    Io(#[from] std::io::Error),
}

/// Result alias for this crate.
pub type Result<T> = std::result::Result<T, AudioError>;

/// Container / codec family of an audio file.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum FileFormat {
    Wav,
    Aiff,
    Flac,
    Mp3,
    /// Ogg holding something other than Opus (Vorbis, FLAC).
    Ogg,
    /// Opus in Ogg.
    Opus,
    Aac,
    Alac,
    Caf,
    Other,
}

/// Stored sample representation.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum SampleFormat {
    Int8,
    Int16,
    Int24,
    Int32,
    Float32,
    Float64,
    /// A lossy codec (MP3, AAC, Vorbis, ...): no fixed bit depth.
    Compressed,
}

/// Broadcast Wave (`bext`) metadata.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct BwfInfo {
    pub description: String,
    pub originator: String,
    pub originator_reference: String,
    /// `yyyy-mm-dd`.
    pub origination_date: String,
    /// `hh:mm:ss`.
    pub origination_time: String,
    /// Sample count since midnight of the first sample (the timestamp Pro Tools spots to).
    pub time_reference: u64,
}

/// What [`probe`] learns about a file without decoding all of it.
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct AudioInfo {
    pub format: FileFormat,
    pub sample_format: SampleFormat,
    pub sample_rate: u32,
    pub channels: u16,
    pub frames: u64,
    pub bwf: Option<BwfInfo>,
}

impl AudioInfo {
    /// Duration in seconds.
    pub fn duration_secs(&self) -> f64 {
        if self.sample_rate == 0 { 0.0 } else { self.frames as f64 / f64::from(self.sample_rate) }
    }
}

/// Bit depth for [`encode`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum BitDepth {
    Int16,
    Int24,
    Int32,
    Float32,
}

/// Options for [`encode`].
#[derive(Debug, Clone, PartialEq, serde::Serialize, serde::Deserialize)]
pub struct EncodeOptions {
    /// `Wav`, `Aiff` or `Flac`; anything else is [`AudioError::Unsupported`].
    pub format: FileFormat,
    pub bit_depth: BitDepth,
    /// Add TPDF dither when quantizing to an integer depth.
    pub dither: bool,
    /// Write a BWF `bext` chunk (WAV only; ignored for other formats).
    pub bwf: Option<BwfInfo>,
    /// Bitrate of an MP3 or Opus file in kbps, 0 for the format's default ([`default_kbps`]);
    /// ignored by the lossless formats, which ignore `bit_depth` and `dither` in turn.
    #[serde(default)]
    pub bitrate_kbps: u32,
}

impl Default for EncodeOptions {
    fn default() -> Self {
        Self { format: FileFormat::Wav, bit_depth: BitDepth::Int24, dither: false, bwf: None, bitrate_kbps: 0 }
    }
}

/// Whether `format` is written by a lossy encoder (so `bitrate_kbps` applies, not `bit_depth`).
pub fn is_lossy(format: FileFormat) -> bool {
    matches!(format, FileFormat::Mp3 | FileFormat::Opus)
}

/// The bitrate, in kbps, of an MP3 or Opus file written with `bitrate_kbps: 0`.
pub fn default_kbps(format: FileFormat) -> u32 {
    match format {
        FileFormat::Mp3 => mp3::DEFAULT_KBPS,
        FileFormat::Opus => opus::DEFAULT_KBPS,
        _ => 0,
    }
}

/// The bitrates offered for `format`, in kbps (empty for the lossless formats). MP3 also takes
/// the other Layer III rates (snapped to the nearest); Opus any rate from 6 to 510.
pub fn offered_kbps(format: FileFormat) -> &'static [u32] {
    match format {
        FileFormat::Mp3 => &[128, 160, 192, 256, 320],
        FileFormat::Opus => &[64, 96, 128, 160, 192, 256],
        _ => &[],
    }
}

/// Checks a bitrate asked for `format`: 32 to 320 kbps for MP3, 6 to 510 for Opus.
pub fn check_kbps(format: FileFormat, kbps: u32) -> Result<()> {
    let ok = match format {
        FileFormat::Mp3 => (32..=320).contains(&kbps),
        FileFormat::Opus => opus::KBPS_RANGE.contains(&kbps),
        _ => true,
    };
    if ok { Ok(()) } else { Err(AudioError::Unsupported(format!("{format:?} cannot be written at {kbps} kbps"))) }
}

/// The extension a file [`encode`] writes in `format` is given (`wav`, `aif`, `flac`, `mp3`, `opus`).
pub fn encode_extension(format: FileFormat) -> &'static str {
    match format {
        FileFormat::Aiff => "aif",
        FileFormat::Flac => "flac",
        FileFormat::Mp3 => "mp3",
        FileFormat::Opus => "opus",
        _ => "wav",
    }
}

/// Size of an ID3v2 tag at the start of `b`, if there is one.
fn id3v2_len(b: &[u8]) -> Option<usize> {
    if b.get(..3)? != b"ID3" {
        return None;
    }
    let flags = *b.get(5)?;
    let size = b.get(6..10)?.iter().fold(0usize, |acc, &x| (acc << 7) | usize::from(x & 0x7F));
    Some(10 + size + if flags & 0x10 != 0 { 10 } else { 0 })
}

fn detect_by_magic(b: &[u8], depth: u32) -> Option<FileFormat> {
    let head = |n: usize| b.get(..n);
    if wav::is_wav(b) {
        return Some(FileFormat::Wav);
    }
    if aiff::is_aiff(b) {
        return Some(FileFormat::Aiff);
    }
    match head(4)? {
        b"fLaC" => return Some(FileFormat::Flac),
        b"OggS" if opus::is_opus(b) => return Some(FileFormat::Opus),
        b"OggS" => return Some(FileFormat::Ogg),
        b"caff" => return Some(FileFormat::Caf),
        [0x1A, 0x45, 0xDF, 0xA3] => return Some(FileFormat::Other),
        _ => {}
    }
    if b.get(4..8) == Some(b"ftyp") {
        // MP4 / M4A: the sample entry inside `stsd` names the codec.
        let found = b.windows(4).position(|w| w == b"stsd");
        let codec = found.and_then(|p| b.get(p.checked_add(16)?..p.checked_add(20)?));
        return Some(if codec == Some(b"alac") { FileFormat::Alac } else { FileFormat::Aac });
    }
    if let Some(len) = id3v2_len(b) {
        if depth == 0
            && let Some(f) = b.get(len..).and_then(|rest| detect_by_magic(rest, 1))
        {
            return Some(f);
        }
        return Some(FileFormat::Mp3);
    }
    let (b0, b1) = (*b.first()?, *b.get(1)?);
    if b0 == 0xFF && b1 & 0xE0 == 0xE0 {
        let layer = (b1 >> 1) & 0x3;
        if b1 & 0xF0 == 0xF0 && layer == 0 {
            return Some(FileFormat::Aac); // ADTS
        }
        if layer != 0 {
            return Some(FileFormat::Mp3);
        }
    }
    None
}

fn detect_by_ext(ext: &str) -> FileFormat {
    match ext.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "wav" | "wave" | "bwf" | "rf64" => FileFormat::Wav,
        "aif" | "aiff" | "aifc" => FileFormat::Aiff,
        "flac" => FileFormat::Flac,
        "mp3" | "mp2" | "mp1" => FileFormat::Mp3,
        "ogg" | "oga" => FileFormat::Ogg,
        "opus" => FileFormat::Opus,
        "aac" | "m4a" | "m4b" | "mp4" | "adts" => FileFormat::Aac,
        "caf" => FileFormat::Caf,
        _ => FileFormat::Other,
    }
}

/// Identify a file by its magic bytes, falling back to the extension hint (`"wav"`, `".flac"`).
pub fn detect_format(bytes: &[u8], ext_hint: Option<&str>) -> FileFormat {
    detect_by_magic(bytes, 0).unwrap_or_else(|| ext_hint.map(detect_by_ext).unwrap_or(FileFormat::Other))
}

/// Read a file's format, sample rate, channel count, length and BWF metadata.
pub fn probe(bytes: &[u8], ext_hint: Option<&str>) -> Result<AudioInfo> {
    let format = detect_format(bytes, ext_hint);
    match format {
        FileFormat::Wav if wav::is_wav(bytes) => match wav::parse(bytes)? {
            wav::WavParse::Native(l) => Ok(l.info),
            wav::WavParse::Foreign { bwf } => {
                let mut info = symph::probe(bytes, format)?;
                info.bwf = bwf;
                Ok(info)
            }
        },
        FileFormat::Aiff if aiff::is_aiff(bytes) => match aiff::parse(bytes)? {
            aiff::AiffParse::Native(l) => Ok(l.info),
            aiff::AiffParse::Foreign => symph::probe(bytes, format),
        },
        FileFormat::Opus if opus::is_opus(bytes) => opus::probe(bytes),
        _ => symph::probe(bytes, format),
    }
}

/// Decode a whole file to planar `f32`.
pub fn decode(bytes: &[u8], ext_hint: Option<&str>) -> Result<(AudioInfo, AudioBuffer)> {
    let format = detect_format(bytes, ext_hint);
    match format {
        FileFormat::Wav if wav::is_wav(bytes) => match wav::parse(bytes)? {
            wav::WavParse::Native(l) => {
                let buf = wav::decode(bytes, &l)?;
                let mut info = l.info;
                info.frames = buf.frames() as u64;
                Ok((info, buf))
            }
            wav::WavParse::Foreign { bwf } => {
                let (mut info, buf) = symph::decode(bytes, format)?;
                info.bwf = bwf;
                Ok((info, buf))
            }
        },
        FileFormat::Aiff if aiff::is_aiff(bytes) => match aiff::parse(bytes)? {
            aiff::AiffParse::Native(l) => {
                let buf = aiff::decode(bytes, &l)?;
                let mut info = l.info;
                info.frames = buf.frames() as u64;
                Ok((info, buf))
            }
            aiff::AiffParse::Foreign => symph::decode(bytes, format),
        },
        FileFormat::Opus if opus::is_opus(bytes) => opus::decode(bytes),
        _ => symph::decode(bytes, format),
    }
}

/// Encode `buf` as WAV/BWF, AIFF/AIFF-C, FLAC, MP3 or Ogg Opus. Samples are clamped to [-1, 1]
/// for the PCM formats.
pub fn encode(buf: &AudioBuffer, opts: &EncodeOptions) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Wav => wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), false, None),
        FileFormat::Aiff => aiff::encode(buf, opts.bit_depth, opts.dither),
        FileFormat::Flac => flac::encode(buf, opts.bit_depth, opts.dither),
        FileFormat::Mp3 | FileFormat::Opus => {
            let mut enc = FileEncoder::new(std::io::Cursor::new(Vec::new()), opts, buf.channels.len(), buf.sample_rate, None)?;
            enc.write(&buf.channels, buf.frames())?;
            Ok(enc.finish()?.into_inner())
        }
        other => Err(AudioError::Unsupported(format!("encoding {other:?} is not supported"))),
    }
}

/// Formats [`encode`] can write, as the extensions that name them.
pub const ENCODE_EXTENSIONS: &str = "wav, aif, aiff, flac, mp3, opus";

/// The format [`encode`] writes for a file extension or format name (`wav`, `aiff`, `flac`, `mp3`, `opus`, case-insensitive), or `None` when it cannot write it.
pub fn encode_format_for(name: &str) -> Option<FileFormat> {
    match name.trim_start_matches('.').to_ascii_lowercase().as_str() {
        "wav" | "wave" | "bwf" => Some(FileFormat::Wav),
        "aif" | "aiff" => Some(FileFormat::Aiff),
        "flac" => Some(FileFormat::Flac),
        "mp3" => Some(FileFormat::Mp3),
        "opus" => Some(FileFormat::Opus),
        _ => None,
    }
}

/// Encode a WAV file in the RF64 layout regardless of size (normally used only above 4 GiB).
pub fn encode_wav_rf64(buf: &AudioBuffer, opts: &EncodeOptions) -> Result<Vec<u8>> {
    wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), true, None)
}

/// Like [`encode`], with an explicit WAV speaker mask (`dwChannelMask`, e.g. 0x60F) for layouts
/// a channel count alone cannot identify (7.0 vs 6.1, 5.1.4 vs 7.1.2, Ambisonics = 0). AIFF and
/// FLAC have fixed channel orders and ignore it.
pub fn encode_with_channel_mask(buf: &AudioBuffer, opts: &EncodeOptions, mask: u32) -> Result<Vec<u8>> {
    match opts.format {
        FileFormat::Wav => wav::encode(buf, opts.bit_depth, opts.dither, opts.bwf.as_ref(), false, Some(mask)),
        _ => encode(buf, opts),
    }
}

/// The error a stream encoder returns for every call after one of its writes has failed: the
/// sink may already hold a prefix of the failed block, so the encoder's counters can no longer
/// describe the bytes on the sink, and later writes or `finish` must not claim success.
pub(crate) fn failed_write_error() -> AudioError {
    AudioError::Encode("the stream encoder already failed on an earlier write; its output is incomplete".into())
}

/// Writes WAV/BWF (RF64 above 4 GiB), AIFF / AIFF-C `fl32`, FLAC, MP3 or Ogg Opus block by block,
/// producing the same bytes [`encode_with_channel_mask`] would for the whole buffer. MP3 and Opus
/// hold mono or stereo, at a rate the codec takes (see [`mp3_rate`]; Opus runs at 48 kHz): other
/// rates are converted as the blocks arrive.
pub struct FileEncoder<W: Write + Seek>(Writer<W>);

enum Writer<W> {
    Wav(wav::Writer<W>),
    Aiff(aiff::Writer<W>),
    Flac(flac::Writer<W>),
    Mp3(Box<mp3::Writer<W>>),
    Opus(Box<opus::Writer<W>>),
}

impl<W: Write + Seek> FileEncoder<W> {
    /// Starts a file at the current position of `out`. `mask` overrides the WAV speaker mask
    /// derived from the channel count.
    pub fn new(out: W, opts: &EncodeOptions, channels: usize, sample_rate: u32, mask: Option<u32>) -> Result<Self> {
        Self::with_tags(out, opts, channels, sample_rate, mask, None)
    }

    /// [`FileEncoder::new`] with text tags written into the formats that keep them in the stream:
    /// a FLAC's `VORBIS_COMMENT` block, an Opus file's `OpusTags`, an AIFF's text chunks (after
    /// the sound data). WAV and MP3 take theirs from the caller ([`export::Tags::wav_info`],
    /// [`export::Tags::id3v2`]); empty tags write nothing.
    pub fn with_tags(
        out: W,
        opts: &EncodeOptions,
        channels: usize,
        sample_rate: u32,
        mask: Option<u32>,
        tags: Option<&export::Tags>,
    ) -> Result<Self> {
        let (depth, dither) = (opts.bit_depth, opts.dither);
        let comments = tags.map(export::Tags::vorbis_comments).unwrap_or_default();
        Ok(FileEncoder(match opts.format {
            FileFormat::Wav => Writer::Wav(wav::Writer::new(out, channels, sample_rate, depth, dither, opts.bwf.as_ref(), mask, false)?),
            FileFormat::Aiff => {
                let chunks = tags.map(export::Tags::aiff_chunks).unwrap_or_default();
                Writer::Aiff(aiff::Writer::new(out, channels, sample_rate, depth, dither, chunks)?)
            }
            FileFormat::Flac => Writer::Flac(flac::Writer::new(out, channels, sample_rate, depth, dither, &comments)?),
            FileFormat::Mp3 => Writer::Mp3(Box::new(mp3::Writer::new(out, channels, sample_rate, opts.bitrate_kbps)?)),
            FileFormat::Opus => Writer::Opus(Box::new(opus::Writer::new(out, channels, sample_rate, opts.bitrate_kbps, &comments)?)),
            other => return Err(AudioError::Unsupported(format!("encoding {other:?} is not supported"))),
        }))
    }

    /// Fails, without writing anything, when `frames` more frames would not fit the format (an
    /// AIFF holds 4 GiB, a FLAC 2^36 frames; MP3 and Opus have no limit a bounce reaches).
    pub fn check_room(&self, frames: u64) -> Result<()> {
        match &self.0 {
            Writer::Wav(w) => w.check_room(frames).map(drop),
            Writer::Aiff(w) => w.check_room(frames).map(drop),
            Writer::Flac(w) => w.check_room(frames).map(drop),
            Writer::Mp3(_) | Writer::Opus(_) => Ok(()),
        }
    }

    /// Appends the first `frames` frames of `block`, one `Vec` per channel (missing samples are
    /// silence), unless they don't fit (see [`FileEncoder::check_room`]). Once a write has
    /// failed, the encoder is unusable: the sink may hold a prefix of the failed block, so
    /// every later `write` and `finish` fails instead of completing an inconsistent file.
    pub fn write(&mut self, block: &[Vec<f32>], frames: usize) -> Result<()> {
        match &mut self.0 {
            Writer::Wav(w) => w.write(block, frames),
            Writer::Aiff(w) => w.write(block, frames),
            Writer::Flac(w) => w.write(block, frames),
            Writer::Mp3(w) => w.write(block, frames),
            Writer::Opus(w) => w.write(block, frames),
        }
    }

    /// Completes the headers and hands back `out`, positioned at the end of the file.
    pub fn finish(self) -> Result<W> {
        match self.0 {
            Writer::Wav(w) => w.finish(),
            Writer::Aiff(w) => w.finish(),
            Writer::Flac(w) => w.finish(),
            Writer::Mp3(w) => w.finish(),
            Writer::Opus(w) => w.finish(),
        }
    }
}

/// The WAVE_FORMAT_EXTENSIBLE speaker mask of a WAV file, if it has one.
pub fn wav_channel_mask(bytes: &[u8]) -> Option<u32> {
    wav::read_channel_mask(bytes)
}

/// The header of a take file holding `frames` frames: a WAV (with a `bext` chunk when `bwf` is
/// given; RF64 above 4 GiB) or an AIFF (AIFF-C `fl32` for float), at `depth`. Its length doesn't
/// depend on `frames`, so a recorder can rewrite it in place as the file grows. Samples follow
/// it as [`take_sample`] writes them.
pub fn take_header(format: FileFormat, channels: usize, sample_rate: u32, depth: BitDepth, bwf: Option<&BwfInfo>, frames: u64) -> Result<Vec<u8>> {
    if channels == 0 || sample_rate == 0 {
        return Err(AudioError::Encode("a take needs channels and a sample rate".into()));
    }
    match format {
        FileFormat::Wav => wav::header(channels, sample_rate, pcm::sample_format(depth), bwf, None, frames, false),
        FileFormat::Aiff => aiff::take_header(channels, sample_rate, depth, frames),
        other => Err(AudioError::Unsupported(format!("takes are not recorded as {other:?}"))),
    }
}

/// Appends sample `x` to `out` as a take file of `format` at `depth` holds it (WAV little-endian,
/// AIFF big-endian; integers rounded and clipped, no dither: an input is never finer than 24 bits).
pub fn take_sample(format: FileFormat, depth: BitDepth, x: f32, out: &mut Vec<u8>) {
    let x = if x.is_finite() { x } else { 0.0 };
    let big = format == FileFormat::Aiff;
    let int = |bits: u32| {
        let full = (1i64 << (bits - 1)) as f64;
        ((f64::from(x) * full).round() as i64).clamp(-(full as i64), full as i64 - 1)
    };
    match depth {
        BitDepth::Float32 => out.extend_from_slice(&if big { x.to_be_bytes() } else { x.to_le_bytes() }),
        BitDepth::Int16 => {
            let v = int(16) as i16;
            out.extend_from_slice(&if big { v.to_be_bytes() } else { v.to_le_bytes() });
        }
        BitDepth::Int24 => {
            let b = (int(24) as i32).to_le_bytes();
            if big {
                out.extend_from_slice(&[b[2], b[1], b[0]]);
            } else {
                out.extend_from_slice(&[b[0], b[1], b[2]]);
            }
        }
        BitDepth::Int32 => {
            let v = int(32) as i32;
            out.extend_from_slice(&if big { v.to_be_bytes() } else { v.to_le_bytes() });
        }
    }
}

/// Bytes [`take_sample`] writes per sample.
pub fn take_sample_bytes(depth: BitDepth) -> usize {
    match depth {
        BitDepth::Int16 => 2,
        BitDepth::Int24 => 3,
        BitDepth::Int32 | BitDepth::Float32 => 4,
    }
}

/// The header of a 32-bit float WAV holding `frames` frames (little-endian samples follow it). Its
/// length doesn't depend on `frames`, so a recorder can rewrite it in place as the file grows; it
/// switches to RF64 above 4 GiB.
pub fn wav_float_header(channels: usize, sample_rate: u32, frames: u64) -> Result<Vec<u8>> {
    if channels == 0 || sample_rate == 0 {
        return Err(AudioError::Encode("a WAV needs channels and a sample rate".into()));
    }
    wav::header(channels, sample_rate, (32, true), None, None, frames, false)
}

#[cfg(test)]
mod stream_tag_tests {
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
    use super::*;
    use std::io::Cursor;

    fn tagged(format: FileFormat, tags: Option<&export::Tags>) -> Vec<u8> {
        let buf: Vec<Vec<f32>> = (0..2).map(|c| (0..9600).map(|i| ((i + c * 7) as f32 * 0.01).sin() * 0.3).collect()).collect();
        let opts = EncodeOptions { format, bit_depth: BitDepth::Int16, ..EncodeOptions::default() };
        let mut enc = FileEncoder::with_tags(Cursor::new(Vec::new()), &opts, 2, 48_000, None, tags).unwrap();
        enc.write(&buf, 9600).unwrap();
        enc.finish().unwrap().into_inner()
    }

    fn has(bytes: &[u8], s: &[u8]) -> bool {
        bytes.windows(s.len()).any(|w| w == s)
    }

    /// FLAC, AIFF and Opus bounces carry their tags (Vorbis comments, text chunks), still read
    /// back as the same audio, and untagged files are byte for byte as before.
    #[test]
    fn flac_aiff_and_opus_carry_their_tags() {
        let tags = export::Tags { title: "Song".into(), artist: "Me".into(), album: "LP".into(), date: "2026-11-18".into(), comment: "mix 3".into() };
        for format in [FileFormat::Flac, FileFormat::Aiff, FileFormat::Opus] {
            let plain = tagged(format, None);
            assert_eq!(plain, tagged(format, Some(&export::Tags::default())), "{format:?}: empty tags write nothing");
            let with = tagged(format, Some(&tags));
            let (_, a) = decode(&plain, None).unwrap();
            let (info, b) = decode(&with, None).unwrap();
            assert_eq!(info.format, format);
            assert_eq!(a.channels, b.channels, "{format:?}: the same audio");
            match format {
                FileFormat::Aiff => {
                    for s in [&b"NAME\0\0\0\x04Song"[..], b"AUTH", b"ANNO\0\0\0\x09Album: LP", b"Date: 2026-11-18", b"mix 3"] {
                        assert!(has(&with, s), "AIFF lacks {:?}", String::from_utf8_lossy(s));
                    }
                    let form = u32::from_be_bytes(with[4..8].try_into().unwrap()) as usize;
                    assert_eq!(form + 8, with.len(), "the FORM size counts the tags");
                }
                _ => {
                    for s in ["TITLE=Song", "ARTIST=Me", "ALBUM=LP", "DATE=2026-11-18", "COMMENT=mix 3"] {
                        assert!(has(&with, s.as_bytes()), "{format:?} lacks {s}");
                    }
                }
            }
        }
        // FLAC: STREAMINFO then VORBIS_COMMENT, the last block.
        let f = tagged(FileFormat::Flac, Some(&tags));
        assert_eq!(f[4], 0x00, "STREAMINFO is no longer the last block");
        assert_eq!(f[4 + 4 + 34], 0x84, "VORBIS_COMMENT, last");
    }
}

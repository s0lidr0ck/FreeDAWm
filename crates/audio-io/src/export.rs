//! Export helpers a bounce uses around [`crate::FileEncoder`]: noise-shaped dither ahead of an
//! integer file, and text tags (ID3v2 for MP3, a `LIST`/`INFO` chunk for WAV, Vorbis comments for
//! FLAC and Opus, text chunks for AIFF).

/// TPDF dither with second-order noise shaping, quantizing to an integer bit depth ahead of the
/// encoder: each output is exactly on the grid of that depth, so an encoder writing it without
/// dither of its own stores it unchanged. The quantization error is fed back through
/// `2e[n-1] - e[n-2]`, which shapes the noise by `(1 - z^-1)^2`: lower in the midrange where the
/// ear is most sensitive, higher towards Nyquist. One shaper per channel.
#[derive(Debug, Clone)]
pub struct NoiseShaper {
    scale: f64,
    e1: f64,
    e2: f64,
    rng: u64,
}

impl NoiseShaper {
    /// A shaper to `bits` (2..=24) for channel `channel` (each channel its own dither sequence).
    pub fn new(bits: u32, channel: usize) -> NoiseShaper {
        let bits = bits.clamp(2, 24);
        let rng = 0xA076_1D64_78BD_642Fu64 ^ (channel as u64).wrapping_add(1).wrapping_mul(0xE703_7ED1_A0B4_28DB);
        NoiseShaper { scale: (1u64 << (bits - 1)) as f64, e1: 0.0, e2: 0.0, rng: if rng == 0 { 1 } else { rng } }
    }

    fn uniform(&mut self) -> f64 {
        let mut x = self.rng;
        x ^= x >> 12;
        x ^= x << 25;
        x ^= x >> 27;
        self.rng = x;
        (x.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }

    /// Quantizes `xs` in place.
    pub fn process(&mut self, xs: &mut [f32]) {
        let (lo, hi) = (-self.scale, self.scale - 1.0);
        for x in xs {
            let v = if x.is_finite() { f64::from(*x).clamp(-1.0, 1.0) * self.scale } else { 0.0 };
            // The shaped error of the samples before, taken off this one.
            let want = v - (2.0 * self.e1 - self.e2);
            let q = (want + self.uniform() - self.uniform()).round().clamp(lo, hi);
            // The error as heard (clipped so a full-scale run cannot wind the loop up).
            let e = (q - want).clamp(-4.0, 4.0);
            self.e2 = self.e1;
            self.e1 = e;
            *x = (q / self.scale) as f32;
        }
    }
}

/// Text a bounce writes into its files: an MP3's ID3v2 tag, a WAV's `LIST`/`INFO` chunk, a FLAC's
/// or an Opus file's Vorbis comments and an AIFF's `NAME`/`AUTH`/`ANNO` chunks. Empty fields are
/// left out.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize, serde::Deserialize)]
#[serde(default)]
pub struct Tags {
    pub title: String,
    pub artist: String,
    pub album: String,
    /// `yyyy` or `yyyy-mm-dd`.
    pub date: String,
    pub comment: String,
}

/// The longest text one tag field keeps, in bytes.
const MAX_FIELD: usize = 1024;

fn clip(s: &str) -> &str {
    let mut end = s.len().min(MAX_FIELD);
    while !s.is_char_boundary(end) {
        end -= 1;
    }
    &s[..end]
}

impl Tags {
    pub fn is_empty(&self) -> bool {
        [&self.title, &self.artist, &self.album, &self.date, &self.comment].iter().all(|s| s.trim().is_empty())
    }

    /// An ID3v2.3 tag (UTF-16 text with a BOM), written ahead of an MP3's frames. Empty when
    /// there is nothing to tag.
    pub fn id3v2(&self) -> Vec<u8> {
        let mut frames = Vec::new();
        let utf16 = |s: &str| -> Vec<u8> {
            let mut v = vec![1u8, 0xFF, 0xFE];
            for u in s.encode_utf16() {
                v.extend_from_slice(&u.to_le_bytes());
            }
            v
        };
        let year = self.date.get(..4).unwrap_or("");
        for (id, text) in [("TIT2", self.title.as_str()), ("TPE1", &self.artist), ("TALB", &self.album), ("TYER", year)] {
            let t = clip(text.trim());
            if t.is_empty() {
                continue;
            }
            let body = utf16(t);
            frames.extend_from_slice(id.as_bytes());
            frames.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(&body);
        }
        let c = clip(self.comment.trim());
        if !c.is_empty() {
            // COMM: encoding, language, an empty description, then the text.
            let mut body = vec![1u8];
            body.extend_from_slice(b"eng");
            body.extend_from_slice(&[0xFF, 0xFE, 0, 0]);
            for u in c.encode_utf16() {
                body.extend_from_slice(&u.to_le_bytes());
            }
            frames.extend_from_slice(b"COMM");
            frames.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_be_bytes());
            frames.extend_from_slice(&[0, 0]);
            frames.extend_from_slice(&body);
        }
        if frames.is_empty() {
            return Vec::new();
        }
        // The tag size is "synchsafe": seven bits per byte.
        let n = u32::try_from(frames.len()).unwrap_or(0);
        let size = [((n >> 21) & 0x7F) as u8, ((n >> 14) & 0x7F) as u8, ((n >> 7) & 0x7F) as u8, (n & 0x7F) as u8];
        let mut out = b"ID3\x03\x00\x00".to_vec();
        out.extend_from_slice(&size);
        out.extend_from_slice(&frames);
        out
    }

    /// The fields as Vorbis comments (`TITLE=…`, `ARTIST=…`, `ALBUM=…`, `DATE=…`, `COMMENT=…`), as
    /// a FLAC's `VORBIS_COMMENT` block and an Opus file's `OpusTags` hold them.
    pub fn vorbis_comments(&self) -> Vec<String> {
        [("TITLE", &self.title), ("ARTIST", &self.artist), ("ALBUM", &self.album), ("DATE", &self.date), ("COMMENT", &self.comment)]
            .into_iter()
            .filter_map(|(k, v)| {
                let t = clip(v.trim());
                (!t.is_empty()).then(|| format!("{k}={t}"))
            })
            .collect()
    }

    /// The AIFF text chunks: `NAME` (title), `AUTH` (artist) and an `ANNO` each for the album
    /// (`Album: …`), the date (`Date: …`) and the comment, to append after the sound data. AIFF
    /// has no chunk of its own for an album or a date. Empty when there is nothing to tag.
    pub fn aiff_chunks(&self) -> Vec<u8> {
        let album = format!("Album: {}", clip(self.album.trim()));
        let date = format!("Date: {}", clip(self.date.trim()));
        let mut out = Vec::new();
        for (id, text, on) in [
            (b"NAME", self.title.trim(), true),
            (b"AUTH", self.artist.trim(), true),
            (b"ANNO", album.as_str(), !self.album.trim().is_empty()),
            (b"ANNO", date.as_str(), !self.date.trim().is_empty()),
            (b"ANNO", self.comment.trim(), true),
        ] {
            let t = clip(text);
            if t.is_empty() || !on {
                continue;
            }
            out.extend_from_slice(id);
            out.extend_from_slice(&u32::try_from(t.len()).unwrap_or(0).to_be_bytes());
            out.extend_from_slice(t.as_bytes());
            if t.len() % 2 == 1 {
                out.push(0);
            }
        }
        out
    }

    /// A WAV `LIST` chunk of type `INFO` (INAM, IART, IPRD, ICRD, ICMT; text as UTF-8), to append
    /// after the data chunk. Empty when there is nothing to tag.
    pub fn wav_info(&self) -> Vec<u8> {
        let mut body = b"INFO".to_vec();
        for (id, text) in
            [("INAM", self.title.as_str()), ("IART", &self.artist), ("IPRD", &self.album), ("ICRD", &self.date), ("ICMT", &self.comment)]
        {
            let t = clip(text.trim());
            if t.is_empty() {
                continue;
            }
            let len = t.len() + 1;
            body.extend_from_slice(id.as_bytes());
            body.extend_from_slice(&u32::try_from(len).unwrap_or(0).to_le_bytes());
            body.extend_from_slice(t.as_bytes());
            body.push(0);
            if len % 2 == 1 {
                body.push(0);
            }
        }
        if body.len() == 4 {
            return Vec::new();
        }
        let mut out = b"LIST".to_vec();
        out.extend_from_slice(&u32::try_from(body.len()).unwrap_or(0).to_le_bytes());
        out.extend_from_slice(&body);
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Shaped output sits on the 16-bit grid, tracks the input, and puts less noise in the
    /// midrange than plain TPDF of the same depth.
    #[test]
    fn shaped_dither_lands_on_the_grid_with_its_noise_moved_up() {
        let n = 1 << 14;
        let x: Vec<f32> = (0..n).map(|i| (i as f32 * 0.01).sin() * 0.3).collect();
        let mut y = x.clone();
        NoiseShaper::new(16, 0).process(&mut y);
        for v in &y {
            let k = f64::from(*v) * 32768.0;
            assert!((k - k.round()).abs() < 1e-6, "{v} is not a 16-bit step");
        }
        let err: Vec<f64> = x.iter().zip(&y).map(|(a, b)| f64::from(*b - *a)).collect();
        // The error's power below a sixteenth of the sample rate (every 8th DFT bin there).
        let low = |e: &[f64]| {
            let n = e.len();
            (1..n / 32)
                .step_by(8)
                .map(|k| {
                    let w = 2.0 * std::f64::consts::PI * k as f64 / n as f64;
                    let (re, im) =
                        e.iter().enumerate().fold((0.0, 0.0), |(re, im), (i, x)| (re + x * (w * i as f64).cos(), im - x * (w * i as f64).sin()));
                    re * re + im * im
                })
                .sum::<f64>()
        };
        let tpdf: Vec<f64> = {
            let mut s = 1u64;
            x.iter()
                .map(|a| {
                    let mut u = || {
                        s ^= s << 13;
                        s ^= s >> 7;
                        s ^= s << 17;
                        (s >> 11) as f64 / (1u64 << 53) as f64
                    };
                    let v = f64::from(*a) * 32768.0;
                    ((v + u() - u()).round() - v) / 32768.0
                })
                .collect()
        };
        assert!(low(&err) < low(&tpdf) / 4.0, "shaped {} vs flat {}", low(&err), low(&tpdf));
    }

    #[test]
    fn tags_write_id3_and_wav_info() {
        let t = Tags { title: "Song".into(), artist: "Me".into(), date: "2026-11-17".into(), ..Tags::default() };
        let id3 = t.id3v2();
        assert!(id3.starts_with(b"ID3\x03"));
        assert!(id3.windows(4).any(|w| w == b"TIT2") && id3.windows(4).any(|w| w == b"TYER"));
        assert!(!id3.windows(4).any(|w| w == b"TALB"), "empty fields are left out");
        let info = t.wav_info();
        assert!(info.starts_with(b"LIST") && info.get(8..12) == Some(&b"INFO"[..]));
        assert_eq!(u32::from_le_bytes(info[4..8].try_into().unwrap()) as usize, info.len() - 8);
        assert!(Tags::default().id3v2().is_empty() && Tags::default().wav_info().is_empty());
    }
}

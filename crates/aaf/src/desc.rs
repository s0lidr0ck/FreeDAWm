//! The neutral import description: what an interchange file holds for an audio timeline, in
//! samples at the rate the caller asked for, independent of the file format it came from (AAF
//! now, OMF later). The engine turns it into tracks, clips and sources.

/// A whole import.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Import {
    /// The composition's name.
    pub name: String,
    /// The application that wrote the file (company, product, version).
    pub writer: String,
    /// The rate every position and length below is in.
    pub sample_rate: u32,
    /// The composition's start timecode, when it has one.
    pub timecode: Option<Timecode>,
    /// Where the composition starts, in samples since timecode 00:00:00:00 (0 without timecode).
    /// Clip and marker positions are from the composition start.
    pub start: i64,
    /// The media's most common sample rate and bit depth.
    pub media_rate: Option<u32>,
    pub media_bits: Option<u16>,
    pub tracks: Vec<Track>,
    pub markers: Vec<Marker>,
    /// Every audio file the clips use (embedded or external), referred to by index.
    pub media: Vec<Media>,
    /// What the file holds that is not carried, each with why.
    pub skipped: Vec<String>,
}

/// A composition's start timecode.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timecode {
    /// Frames since 00:00:00:00 at `fps`.
    pub frames: i64,
    /// Nominal frames per second (24, 25, 30, ...).
    pub fps: u16,
    pub drop: bool,
    /// The edit rate the timecode counts in (30000/1001 for 29.97).
    pub rate: (i32, i32),
}

/// One audio track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Track {
    pub name: String,
    /// Channels of the track's clips (1 mono, 2 stereo, 6 for 5.1, 8 for 7.1).
    pub channels: u16,
    pub clips: Vec<Clip>,
    /// Track volume (dB), when the file sets one.
    pub gain_db: Option<f32>,
    /// Track volume automation: (position from the composition start, gain as a factor).
    pub gain_auto: Vec<(i64, f64)>,
    /// Track pan -1 (left) .. 1 (right), when the file sets one.
    pub pan: Option<f32>,
    /// Pan automation: (position, pan -1..1).
    pub pan_auto: Vec<(i64, f32)>,
    pub mute: bool,
    pub solo: bool,
}

/// One clip on a track.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Clip {
    /// The clip's own name (a subclip's), else the media's.
    pub name: String,
    /// Position from the composition start.
    pub start: i64,
    pub len: i64,
    /// Where the clip starts in its media.
    pub offset: i64,
    /// What each channel plays: `None` for a channel whose media is not in the file.
    pub channels: Vec<Option<ChannelRef>>,
    pub fade_in: Option<Fade>,
    pub fade_out: Option<Fade>,
    /// Static clip gain (dB).
    pub gain_db: Option<f32>,
    /// Clip gain envelope: (offset from the clip start, gain as a factor).
    pub gain_env: Vec<(i64, f64)>,
    pub muted: bool,
}

/// One channel of a clip: channel `channel` (from 0) of `media`, starting `offset` samples in
/// (its own; usually the clip's).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChannelRef {
    pub media: usize,
    pub channel: u16,
    pub offset: i64,
    /// The file `media` was made from, when the AAF names it: (media, channel, offset). Media
    /// Composer's own media is MXF; the file it imported is often still at hand.
    pub original: Option<(usize, u16, i64)>,
}

impl ChannelRef {
    /// Moves where the channel starts reading by `by` samples (in the original too).
    pub fn shift(&mut self, by: i64) {
        self.offset += by;
        if let Some(o) = &mut self.original {
            o.2 += by;
        }
    }
}

/// A fade's shape.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Curve {
    #[default]
    Linear,
    /// Equal power.
    Power,
    Log,
    Exp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Fade {
    pub len: i64,
    pub curve: Curve,
    /// Part of a crossfade with the neighbouring clip (the two overlap by `len`).
    pub cross: bool,
}

/// A marker (a memory location).
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Marker {
    pub name: String,
    pub comment: String,
    /// Position from the composition start.
    pub start: i64,
    /// 0 for a point marker.
    pub len: i64,
    pub color: Option<[u8; 3]>,
}

/// An audio file a clip plays.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Media {
    pub name: String,
    pub channels: u16,
    pub sample_rate: u32,
    pub bits: u16,
    /// Length in frames at `sample_rate` (0 when the file does not say).
    pub frames: u64,
    pub data: MediaData,
}

/// Where a media file's audio is.
#[derive(Debug, Clone, Default, PartialEq)]
pub enum MediaData {
    /// Inside the interchange file: a complete WAV or AIFF file (raw PCM gets a WAV header).
    Embedded { bytes: Vec<u8>, ext: &'static str },
    /// Beside it: the locations the file names (URLs or paths), in order.
    External { locations: Vec<String> },
    /// Nowhere the file says.
    #[default]
    Missing,
}

impl Import {
    /// How many clips the tracks hold.
    pub fn clip_count(&self) -> usize {
        self.tracks.iter().map(|t| t.clips.len()).sum()
    }
}

/// The largest position or length (2^50 samples, about 750 years at 48 kHz).
pub const MAX_SAMPLES: i64 = 1 << 50;

/// Converts `n` units of `rate` (`num/den` per second) to samples at `sr`, rounding to nearest
/// and clamped to ±[`MAX_SAMPLES`].
pub fn to_samples(n: i64, rate: (i32, i32), sr: u32) -> i64 {
    let (num, den) = (i128::from(rate.0), i128::from(rate.1));
    if num <= 0 || den <= 0 {
        return 0;
    }
    let x = i128::from(n) * i128::from(sr) * den;
    let q = if x >= 0 { (x + num / 2) / num } else { (x - num / 2) / num };
    // Clamped far inside i64, so sums of positions and lengths cannot overflow.
    q.clamp(-i128::from(MAX_SAMPLES), i128::from(MAX_SAMPLES)) as i64
}

/// The file paths an external media location may be at: the location itself when it is a path or
/// a `file:` URL; then each longer tail of its path (most specific first, down to the bare
/// name) joined to `dir`, the interchange file's folder, and then to the folder above it (media
/// in a folder beside the one holding the file).
pub fn candidate_paths(location: &str, dir: Option<&std::path::Path>) -> Vec<std::path::PathBuf> {
    let path = url_to_path(location);
    let mut out = Vec::new();
    if !path.is_empty() {
        out.push(std::path::PathBuf::from(&path));
    }
    let parts: Vec<&str> = path.split(['/', '\\']).filter(|p| !p.is_empty() && *p != "." && *p != "..").collect();
    let n = parts.len();
    for base in [dir, dir.and_then(std::path::Path::parent)].into_iter().flatten() {
        for k in (1..=n.min(4)).rev() {
            let mut p = base.to_path_buf();
            for part in &parts[n - k..] {
                if part.ends_with(':') {
                    continue;
                }
                p.push(part);
            }
            if !out.contains(&p) {
                out.push(p);
            }
        }
    }
    out
}

/// A `file:` URL (or plain path) as a path: the scheme and host dropped, `%XX` decoded as UTF-8,
/// a Windows drive (`/c:/...`) kept.
pub fn url_to_path(location: &str) -> String {
    let s = location.trim();
    let rest = if let Some(r) = s.get(..5).filter(|p| p.eq_ignore_ascii_case("file:")).and_then(|_| s.get(5..)) {
        match r.strip_prefix("//") {
            // file://host/path or file:///path
            Some(r) => match r.find('/') {
                Some(i) => {
                    let host = &r[..i];
                    if host.is_empty() || host.eq_ignore_ascii_case("localhost") { &r[i..] } else { r }
                }
                None => r,
            },
            None => r,
        }
    } else {
        s
    };
    let decoded = percent_decode(rest);
    // `/c:/Users/...` → `c:/Users/...`
    let b = decoded.as_bytes();
    if b.len() >= 3 && b[0] == b'/' && b[1].is_ascii_alphabetic() && b[2] == b':' { decoded[1..].to_string() } else { decoded }
}

fn percent_decode(s: &str) -> String {
    let b = s.as_bytes();
    let mut out = Vec::with_capacity(b.len());
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'%'
            && let Some(h) = b.get(i + 1..i + 3)
            && let Ok(t) = std::str::from_utf8(h)
            && let Ok(v) = u8::from_str_radix(t, 16)
        {
            out.push(v);
            i += 3;
            continue;
        }
        out.push(b[i]);
        i += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edit_units_become_samples() {
        assert_eq!(to_samples(108_000, (30000, 1001), 48_000), 172_972_800);
        assert_eq!(to_samples(1, (30, 1), 44_100), 1470);
        assert_eq!(to_samples(4, (30000, 1001), 48_000), 6406);
        assert_eq!(to_samples(-4, (30000, 1001), 48_000), -6406);
        assert_eq!(to_samples(10, (0, 1), 48_000), 0);
        assert_eq!(to_samples(i64::MAX, (1, 1), 48_000), MAX_SAMPLES);
    }

    #[test]
    fn locations_become_paths() {
        assert_eq!(url_to_path("file:///c%3a/Users/user/a%20b.wav"), "c:/Users/user/a b.wav");
        assert_eq!(url_to_path("file://localhost/C:/x/5.1.wav"), "C:/x/5.1.wav");
        assert_eq!(url_to_path("file:///home/u/%E2%82%AC.wav"), "/home/u/€.wav");
        assert_eq!(url_to_path("file://server/share/x.wav"), "server/share/x.wav");
        assert_eq!(url_to_path("/plain/path.wav"), "/plain/path.wav");
        assert_eq!(url_to_path("file:///bad%zz%"), "/bad%zz%");
        let dir = std::path::Path::new("/aaf");
        let c = candidate_paths("file:///c%3a/media/sub/x.wav", Some(dir));
        assert_eq!(c[0], std::path::PathBuf::from("c:/media/sub/x.wav"));
        assert!(c.contains(&std::path::PathBuf::from("/aaf/sub/x.wav")));
        assert!(c.contains(&std::path::PathBuf::from("/aaf/x.wav")));
        assert_eq!(c.last(), Some(&std::path::PathBuf::from("/x.wav")), "then the folder above");
    }
}

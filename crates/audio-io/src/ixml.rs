//! iXML: the location-sound metadata field recorders write into a WAV file beside `bext`
//! (an `iXML` chunk of UTF-8 XML): project, scene, take, tape, a note and the name of each
//! channel. Read without an XML library: only the few elements FreeDAWm uses, each found by its
//! tag, with every length bounded (the input is hostile).

use std::io::{Read, Seek, SeekFrom};
use std::path::Path;

/// The most bytes of an `iXML` chunk read.
const MAX_IXML: u64 = 1 << 20;

/// What a field recorder wrote about a take.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct IxmlInfo {
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub project: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub scene: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub take: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub tape: String,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
    /// Each channel's name (`TRACK_LIST`), in channel order; empty where none is given.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub channels: Vec<String>,
}

impl IxmlInfo {
    /// "Scene 12 Take 3" (what is known of it), for showing beside a clip; empty with neither.
    pub fn label(&self) -> String {
        let mut parts = Vec::new();
        if !self.scene.is_empty() {
            parts.push(format!("Scene {}", self.scene));
        }
        if !self.take.is_empty() {
            parts.push(format!("Take {}", self.take));
        }
        parts.join(" ")
    }

    /// Whether `text` (lower case) is in any of its fields.
    pub fn matches(&self, text: &str) -> bool {
        [&self.project, &self.scene, &self.take, &self.tape, &self.note]
            .into_iter()
            .chain(self.channels.iter())
            .any(|f| f.to_lowercase().contains(text))
    }
}

/// The text of the first `<tag>…</tag>` in `xml` (unescaped, trimmed, at most 256 characters).
fn element<'a>(xml: &'a str, tag: &str) -> Option<&'a str> {
    let open = format!("<{tag}>");
    let close = format!("</{tag}>");
    let start = xml.find(&open)? + open.len();
    let end = start + xml.get(start..)?.find(&close)?;
    xml.get(start..end)
}

fn text(s: &str) -> String {
    let t = s.trim().replace("&lt;", "<").replace("&gt;", ">").replace("&quot;", "\"").replace("&apos;", "'").replace("&amp;", "&");
    t.chars().filter(|c| !c.is_control()).take(256).collect()
}

/// Parses the text of an `iXML` chunk. `None` when it is not iXML (no `BWFXML` root).
pub fn parse_ixml(xml: &str) -> Option<IxmlInfo> {
    let root = element(xml, "BWFXML")?;
    let get = |t: &str| element(root, t).map(text).unwrap_or_default();
    let mut info =
        IxmlInfo { project: get("PROJECT"), scene: get("SCENE"), take: get("TAKE"), tape: get("TAPE"), note: get("NOTE"), channels: Vec::new() };
    if let Some(list) = element(root, "TRACK_LIST") {
        let mut rest = list;
        let mut named: Vec<(usize, String)> = Vec::new();
        while let Some(track) = element(rest, "TRACK") {
            let index = element(track, "CHANNEL_INDEX").and_then(|i| i.trim().parse::<usize>().ok()).unwrap_or(named.len() + 1);
            if (1..=64).contains(&index) {
                named.push((index, element(track, "NAME").map(text).unwrap_or_default()));
            }
            let Some(after) = rest.find("</TRACK>").map(|i| i + "</TRACK>".len()) else { break };
            rest = rest.get(after..).unwrap_or("");
            if named.len() >= 64 {
                break;
            }
        }
        let n = named.iter().map(|x| x.0).max().unwrap_or(0);
        info.channels = vec![String::new(); n];
        for (i, name) in named {
            if let Some(c) = info.channels.get_mut(i - 1) {
                *c = name;
            }
        }
    }
    Some(info)
}

/// The iXML of a WAV (or RF64/BW64) file held in `b`. `None` without one, or for another kind
/// of file.
pub fn ixml_of_bytes(b: &[u8]) -> Option<IxmlInfo> {
    if !matches!(b.get(0..4), Some(b"RIFF" | b"RF64" | b"BW64")) || b.get(8..12) != Some(&b"WAVE"[..]) {
        return None;
    }
    let mut pos = 12usize;
    for _ in 0..10_000 {
        let id = b.get(pos..pos.checked_add(4)?)?;
        let size = b.get(pos + 4..pos.checked_add(8)?).and_then(|x| <[u8; 4]>::try_from(x).ok()).map(u32::from_le_bytes)?;
        let body = pos.checked_add(8)?;
        if id == b"iXML" {
            let end = body.saturating_add(usize::try_from(size).ok()?).min(b.len()).min(body.saturating_add(MAX_IXML as usize));
            return parse_ixml(&String::from_utf8_lossy(b.get(body..end)?));
        }
        if size == u32::MAX {
            return None;
        }
        let size = usize::try_from(size).ok()?;
        pos = body.checked_add(size)?.checked_add(size & 1)?;
    }
    None
}

/// The iXML of the WAV (or RF64/BW64) file at `path`, read by walking its chunk headers (the
/// audio is skipped, not read). `None` without one, or for another kind of file.
pub fn read_ixml(path: &Path) -> Option<IxmlInfo> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if !matches!(head.get(0..4), Some(b"RIFF" | b"RF64" | b"BW64")) || head.get(8..12) != Some(b"WAVE") {
        return None;
    }
    let len = f.metadata().ok()?.len();
    let mut pos = 12u64;
    for _ in 0..10_000 {
        if pos.saturating_add(8) > len {
            return None;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch).ok()?;
        let [_, _, _, _, a, b, c, d] = ch;
        let size = u64::from(u32::from_le_bytes([a, b, c, d]));
        if ch.get(0..4) == Some(&b"iXML"[..]) {
            let mut buf = Vec::new();
            f.take(size.min(MAX_IXML)).read_to_end(&mut buf).ok()?;
            return parse_ixml(&String::from_utf8_lossy(&buf));
        }
        // An RF64 data chunk's size is in ds64: past it, there is no knowing where the next is.
        if size == u64::from(u32::MAX) {
            return None;
        }
        pos = pos.saturating_add(8).saturating_add(size).saturating_add(size & 1);
    }
    None
}

/// What a location recorder's WAV says about itself, read from its headers only (the audio is
/// not read): enough to match it to a clip by timecode, scene and take.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct FieldHeader {
    pub sample_rate: u32,
    pub channels: u16,
    /// Frames of audio (the `data` chunk's size over the frame size).
    pub frames: u64,
    /// The BWF `bext` time reference: samples since midnight at the file's rate (0 without one).
    pub time_reference: u64,
    pub ixml: Option<IxmlInfo>,
}

/// Reads `n` little-endian bytes at `at` of `f` (at most 8).
fn le_at(f: &mut std::fs::File, at: u64, n: usize) -> Option<u64> {
    let mut b = [0u8; 8];
    f.seek(SeekFrom::Start(at)).ok()?;
    f.read_exact(b.get_mut(..n.min(8))?).ok()?;
    Some(u64::from_le_bytes(b))
}

/// The header of the WAV (or RF64/BW64) file at `path`: format, length, BWF time reference and
/// iXML, found by walking its chunk headers (the audio is skipped). `None` for another kind of
/// file or one without a format chunk.
pub fn read_field_header(path: &Path) -> Option<FieldHeader> {
    let mut f = std::fs::File::open(path).ok()?;
    let mut head = [0u8; 12];
    f.read_exact(&mut head).ok()?;
    if !matches!(head.get(0..4), Some(b"RIFF" | b"RF64" | b"BW64")) || head.get(8..12) != Some(b"WAVE") {
        return None;
    }
    let len = f.metadata().ok()?.len();
    let (mut out, mut block, mut data_size, mut ds64_data, mut fmt) = (FieldHeader::default(), 0u64, None, None, false);
    let mut pos = 12u64;
    for _ in 0..10_000 {
        if pos.saturating_add(8) > len {
            break;
        }
        f.seek(SeekFrom::Start(pos)).ok()?;
        let mut ch = [0u8; 8];
        f.read_exact(&mut ch).ok()?;
        let [a, b, c, d, e, g, h, i] = ch;
        let id = [a, b, c, d];
        let mut size = u64::from(u32::from_le_bytes([e, g, h, i]));
        let body = pos.saturating_add(8);
        match &id {
            b"ds64" if size >= 16 => ds64_data = le_at(&mut f, body.saturating_add(8), 8),
            b"fmt " if size >= 16 => {
                out.channels = u16::try_from(le_at(&mut f, body.saturating_add(2), 2)?).unwrap_or(0);
                out.sample_rate = u32::try_from(le_at(&mut f, body.saturating_add(4), 4)?).unwrap_or(0);
                block = le_at(&mut f, body.saturating_add(12), 2)?;
                fmt = true;
            }
            b"bext" if size >= 346 => out.time_reference = le_at(&mut f, body.saturating_add(338), 8)?,
            b"iXML" => {
                let mut buf = Vec::new();
                f.seek(SeekFrom::Start(body)).ok()?;
                (&mut f).take(size.min(MAX_IXML)).read_to_end(&mut buf).ok()?;
                out.ixml = parse_ixml(&String::from_utf8_lossy(&buf));
            }
            b"data" => {
                // An RF64 data chunk's size is in ds64.
                if size == u64::from(u32::MAX) {
                    size = ds64_data?;
                }
                data_size = Some(size.min(len.saturating_sub(body)));
            }
            _ => {}
        }
        pos = body.saturating_add(size).saturating_add(size & 1);
    }
    if !fmt || out.channels == 0 || out.sample_rate == 0 {
        return None;
    }
    out.frames = data_size.unwrap_or(0).checked_div(block).unwrap_or(0);
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_field_files_header_says_its_length_timecode_and_ixml() {
        let dir = std::env::temp_dir().join(format!("ixml-head-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("take.wav");
        // Two channels of 16-bit at 48 kHz: 10 frames.
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend_from_slice(b"fmt \x10\0\0\0\x01\0\x02\0\x80\xbb\0\0\0\xee\x02\0\x04\0\x10\0");
        let mut bext = vec![0u8; 602];
        bext[338..346].copy_from_slice(&(48_000u64 * 3600).to_le_bytes());
        b.extend_from_slice(b"bext");
        b.extend_from_slice(&(bext.len() as u32).to_le_bytes());
        b.extend_from_slice(&bext);
        b.extend_from_slice(b"data\x28\0\0\0");
        b.extend_from_slice(&[0u8; 40]);
        b.extend_from_slice(b"iXML");
        b.extend_from_slice(&(SAMPLE.len() as u32).to_le_bytes());
        b.extend_from_slice(SAMPLE.as_bytes());
        std::fs::write(&path, &b).unwrap();
        let h = read_field_header(&path).unwrap();
        assert_eq!((h.sample_rate, h.channels, h.frames, h.time_reference), (48_000, 2, 10, 48_000 * 3600));
        assert_eq!(h.ixml.unwrap().take, "3");
        // Cut short inside the data: the frames that are there.
        std::fs::write(&path, &b[..b.len() - SAMPLE.len() - 8 - 20]).unwrap();
        let h = read_field_header(&path).unwrap();
        assert_eq!((h.frames, h.ixml), (5, None));
        std::fs::write(&path, b"RIFF\0\0\0\0WAVEdata\xff\xff\xff\x7f").unwrap();
        assert!(read_field_header(&path).is_none(), "no format");
        let _ = std::fs::remove_dir_all(&dir);
    }

    const SAMPLE: &str = "<?xml version=\"1.0\"?><BWFXML><IXML_VERSION>1.61</IXML_VERSION><PROJECT>Night Shoot</PROJECT><SCENE>12A</SCENE><TAKE>3</TAKE><TAPE>D04</TAPE><NOTE>plane &amp; wind</NOTE><TRACK_LIST><TRACK_COUNT>2</TRACK_COUNT><TRACK><CHANNEL_INDEX>2</CHANNEL_INDEX><NAME>Lav Anna</NAME></TRACK><TRACK><CHANNEL_INDEX>1</CHANNEL_INDEX><NAME>Boom</NAME></TRACK></TRACK_LIST></BWFXML>";

    #[test]
    fn ixml_fields_and_channel_names_are_read() {
        let i = parse_ixml(SAMPLE).unwrap();
        assert_eq!(
            (i.project.as_str(), i.scene.as_str(), i.take.as_str(), i.tape.as_str(), i.note.as_str()),
            ("Night Shoot", "12A", "3", "D04", "plane & wind")
        );
        assert_eq!(i.channels, vec!["Boom".to_string(), "Lav Anna".to_string()], "in channel order");
        assert_eq!(i.label(), "Scene 12A Take 3");
        assert!(i.matches("lav") && i.matches("d04") && !i.matches("drums"));
        assert!(parse_ixml("<xml>not ixml</xml>").is_none());
        assert_eq!(parse_ixml("<BWFXML><SCENE>1</BWFXML>").unwrap().scene, "", "an unclosed element is left out");
    }

    #[test]
    fn a_wav_files_ixml_chunk_is_found_after_its_audio() {
        let dir = std::env::temp_dir().join(format!("ixml-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("take.wav");
        let mut b = b"RIFF\0\0\0\0WAVE".to_vec();
        b.extend_from_slice(b"fmt \x10\0\0\0\x01\0\x01\0\x80\xbb\0\0\0\x77\x01\0\x02\0\x10\0");
        b.extend_from_slice(b"data\x03\0\0\0\x01\x02\x03\0");
        b.extend_from_slice(b"iXML");
        b.extend_from_slice(&(SAMPLE.len() as u32).to_le_bytes());
        b.extend_from_slice(SAMPLE.as_bytes());
        std::fs::write(&path, &b).unwrap();
        assert_eq!(read_ixml(&path).unwrap().scene, "12A");
        assert_eq!(ixml_of_bytes(&b).unwrap().channels.len(), 2);
        assert!(ixml_of_bytes(b"RIFF\0\0\0\0WAVEdata\xff\xff\xff\x7f").is_none());
        std::fs::write(&path, b"RIFF\0\0\0\0WAVEdata\xff\xff\xff\x7f").unwrap();
        assert!(read_ixml(&path).is_none(), "a size past the end is not followed");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

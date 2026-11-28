// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/timeline.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: no indexing or slicing; tempo and meter maps capped at
// `MAX_MAP_EVENTS` and their capacity at what the block can hold; at most `MAX_MARKERS` markers.
//! Timeline, tempo, meter, key signatures, markers, session format, and session settings decoding.

use serde::{Deserialize, Serialize};

use crate::primitives::{byte, le_u64, plain_ticks, round_n, span, u16_at, u32_at, u64_at};

/// At most this many tempo or meter events are read from one map.
pub const MAX_MAP_EVENTS: usize = 100_000;
/// At most this many markers are read.
pub const MAX_MARKERS: usize = 100_000;
use crate::tree::Block;

/// Session audio format (`0x1028`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionFormat {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sample_rate: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub bit_depth: Option<u8>,
    #[serde(rename = "float", skip_serializing_if = "Option::is_none")]
    pub is_float: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file_format: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub interleaved: Option<bool>,
}

/// Timecode and pull settings (`0x204D`, `0x204B`, `0x2023`).
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct TimelineInfo {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timecode_rate_code: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timecode_rate: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub drop_frame: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_start: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pull: Option<String>,
}

/// Transport, edit mode, sub-counter, and timeline selection settings.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct SessionSettings {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_mode_code: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_mode: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub loop_playback: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_mode_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smart_tool: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub edit_tool_code: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_counter_code: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sub_counter: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timeline_selection_ticks: Option<[u64; 2]>,
}

/// Memory location / marker (`0x2030 > 0x2077`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Marker {
    pub number: u16,
    pub name: String,
    pub ticks: u64,
    pub flags: String,
    pub kind: serde_json::Value,
    pub is_marker: bool,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<u64>,
    pub end: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
}

/// Constant-tempo event (`0x2028`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct TempoEvent {
    pub ticks: u64,
    pub bpm: f64,
    pub bpm_exact: f64,
}

/// Meter event (`0x2029`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MeterEvent {
    pub ticks: u64,
    pub bar: u32,
    pub numerator: u32,
    pub denominator: u32,
}

/// Key signature event (`0x271B > 0x2433 > 0x2432`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct KeySignature {
    pub ticks: u64,
    pub key: String,
    pub major: bool,
    pub sharps: i8,
}

/// Gather non-child payload slices of container block `n` (excluding 9-byte header).
#[must_use]
pub fn own_bytes(d: &[u8], n: &Block) -> Vec<u8> {
    let mut out = Vec::new();
    let mut cur = n.data_start();
    for k in &n.kids {
        if k.pos > cur
            && let Some(slice) = d.get(cur..k.pos.min(d.len()))
        {
            out.extend_from_slice(slice);
        }
        cur = k.end();
    }
    let end = n.end().min(d.len());
    if end > cur
        && let Some(slice) = d.get(cur..end)
    {
        out.extend_from_slice(slice);
    }
    out
}

/// Parse session version string from `0x0003` (e.g. `"2026.11.0"`).
#[must_use]
pub fn parse_session_version(d: &[u8], tree: &[Block]) -> Option<String> {
    let b3 = Block::find(tree, 0x0003)?;
    let slice = d.get(b3.data_start()..b3.end().min(d.len()))?;
    let mut i = 0;
    while i < slice.len() {
        if (0x20..=0x7e).contains(&byte(slice, i)) {
            let start = i;
            while i < slice.len() && (0x20..=0x7e).contains(&byte(slice, i)) {
                i += 1;
            }
            if i - start >= 3 {
                let s = span(slice, start, i);
                if is_dotted_version(s) {
                    return Some(String::from_utf8_lossy(s).into_owned());
                }
            }
        } else {
            i += 1;
        }
    }
    None
}

fn is_dotted_version(bytes: &[u8]) -> bool {
    let mut has_dot = false;
    let mut digit_run = false;
    for &b in bytes {
        if b.is_ascii_digit() {
            digit_run = true;
        } else if b == b'.' {
            if !digit_run {
                return false;
            }
            has_dot = true;
            digit_run = false;
        } else {
            return false;
        }
    }
    has_dot && digit_run
}

/// Parse numeric version components for version-gated fields.
#[must_use]
pub fn version_tuple(v: Option<&str>) -> Vec<u32> {
    let Some(s) = v else { return Vec::new() };
    let mut out = Vec::new();
    for part in s.split('.') {
        let Ok(num) = part.parse::<u32>() else { return Vec::new() };
        out.push(num);
    }
    out
}

/// Parse session audio format (`0x1028`).
#[must_use]
pub fn parse_session_format(d: &[u8], tree: &[Block]) -> SessionFormat {
    let Some(n) = Block::find(tree, 0x1028) else {
        return SessionFormat::default();
    };
    let a = n.data_start();
    let e = n.end().min(d.len());
    let Some(len) = e.checked_sub(a) else {
        return SessionFormat::default();
    };
    let Some(&fmt_byte) = d.get(a) else {
        return SessionFormat::default();
    };
    if len < 12 || (fmt_byte != 3 && fmt_byte != 4) {
        return SessionFormat::default();
    }
    let Some(sr) = u32_at(d, a + 2) else {
        return SessionFormat::default();
    };
    let Some(&bd) = e.checked_sub(6).and_then(|idx| d.get(idx)) else {
        return SessionFormat::default();
    };
    let is_float = e.checked_sub(8).and_then(|idx| d.get(idx)).copied() == Some(1);
    let interleaved = if len >= 25 { d.get(a + 24).map(|&b| b != 0) } else { None };
    SessionFormat {
        sample_rate: Some(sr),
        bit_depth: Some(bd),
        is_float: Some(is_float),
        file_format: Some(if fmt_byte == 3 { "WAV" } else { "AIFF" }.to_owned()),
        interleaved,
    }
}

fn tc_rate_name(code: u8) -> String {
    match code {
        1 => "24",
        2 => "25",
        3 => "29.97",
        5 => "29.97 Drop",
        7 => "30",
        8 => "30 Drop",
        9 => "23.976",
        10 => "50",
        11 => "59.94",
        12 => "59.94 Drop",
        13 => "60",
        14 => "60 Drop",
        15 => "47.952",
        16 => "48",
        17 => "100",
        18 => "119.88",
        19 => "119.88 Drop",
        20 => "120",
        21 => "120 Drop",
        _ => return code.to_string(),
    }
    .to_owned()
}

const fn is_tc_known(code: u8) -> bool {
    matches!(code, 1..=3 | 5 | 7..=21)
}

const fn is_drop_frame_code(code: u8) -> bool {
    matches!(code, 5 | 8 | 12 | 14 | 19 | 21)
}

fn pull_name(code: u16) -> String {
    match code {
        12000 => "None",
        12011 => "+0.1%",
        11988 => "-0.1%",
        12500 => "+4.1667%",
        12512 => "+4.1667% +0.1%",
        12487 => "+4.1667% -0.1%",
        11520 => "-4%",
        11531 => "-4% +0.1%",
        11508 => "-4% -0.1%",
        _ => return code.to_string(),
    }
    .to_owned()
}

/// Parse timeline timecode/start/pull (`0x204D`, `0x204B`, `0x2023`).
#[must_use]
pub fn parse_timeline(d: &[u8], tree: &[Block]) -> TimelineInfo {
    let mut out = TimelineInfo::default();
    if let Some(b4d) = Block::find(tree, 0x204D)
        && b4d.size >= 2 + 44
    {
        let p = b4d.data_start();
        let e = b4d.end().min(d.len());
        let ln1 = u32_at(d, p + 25).and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
        let q = if ln1 > 0 && ln1 <= 32 && p + 29 + ln1 + 16 <= e { p + 29 + ln1 } else { p + 40 };
        let rc = d.get(q).copied().filter(|&c| q < e && is_tc_known(c)).unwrap_or_else(|| d.get(p).copied().unwrap_or(0));
        out.timecode_rate_code = Some(rc);
        out.timecode_rate = Some(tc_rate_name(rc));
        out.drop_frame = Some(is_drop_frame_code(rc));
        if q + 16 <= e
            && let Some(ln2) = u32_at(d, q + 12).and_then(|v| usize::try_from(v).ok())
            && ln2 > 0
            && ln2 <= 32
            && q + 16 + ln2 <= e
            && let Some(raw_tc) = d.get(q + 16..q + 16 + ln2)
        {
            let tc = String::from_utf8_lossy(raw_tc).into_owned();
            if tc.contains(';') {
                out.drop_frame = Some(true);
            }
            out.session_start = Some(tc);
        }
    }
    if out.timecode_rate.is_none()
        && let Some(b4b) = Block::find(tree, 0x204B)
        && b4b.size >= 2 + 163
        && let Some(&rc) = d.get(b4b.data_start() + 162)
    {
        out.timecode_rate_code = Some(rc);
        out.timecode_rate = Some(tc_rate_name(rc));
        out.drop_frame = Some(is_drop_frame_code(rc));
    }
    if let Some(b23) = Block::find(tree, 0x2023)
        && b23.size >= 2 + 64
        && let Some(pf) = u16_at(d, b23.data_start() + 62)
    {
        out.pull_code = Some(pf);
        out.pull = Some(pull_name(pf));
    }
    out
}

fn record_mode_value(code: u8) -> serde_json::Value {
    let s = match code {
        0 => "Normal",
        1 => "Loop",
        2 => "Destructive",
        3 => "QuickPunch",
        4 => "TrackPunch",
        5 => "DestructivePunch",
        other => return serde_json::Value::from(other),
    };
    serde_json::Value::from(s)
}

/// Parse session settings (`0x2058`, `0x2024`, `0x2056`, `0x201F`, `0x2016`).
#[must_use]
pub fn parse_session_settings(d: &[u8], tree: &[Block]) -> SessionSettings {
    let mut out = SessionSettings::default();
    if let Some(b58) = Block::find(tree, 0x2058)
        && b58.size >= 2 + 49
    {
        let p = b58.data_start();
        if let (Some(&b48), Some(&b32), Some(&b26)) = (d.get(p + 48), d.get(p + 32), d.get(p + 26)) {
            let mut rc = if b48 <= 5 && b48 != 0 { b48 } else { b32 };
            if rc == 0 && Block::find(tree, 0xD470).is_some() {
                rc = 2;
            }
            out.record_mode_code = Some(rc);
            out.record_mode = Some(record_mode_value(rc));
            out.loop_playback = Some(b26 != 0);
        }
    }
    if let Some(b24) = Block::find(tree, 0x2024)
        && b24.size >= 4
        && let Some(em) = u16_at(d, b24.data_start())
    {
        out.edit_mode_code = Some(em);
        out.edit_mode = Some(match em {
            0 => "Shuffle".to_owned(),
            3 => "Grid".to_owned(),
            _ => em.to_string(),
        });
    }
    if let Some(b56) = Block::find(tree, 0x2056) {
        let o56 = own_bytes(d, b56);
        if o56.len() >= 26 {
            out.smart_tool = Some(byte(&o56, 0) != 0);
            out.edit_tool_code = Some(byte(&o56, 13));
        }
    }
    if let Some(b1f) = Block::find(tree, 0x201F) {
        let o1f = own_bytes(d, b1f);
        if o1f.len() >= 6 {
            let sc = byte(&o1f, 4);
            out.sub_counter_code = Some(sc);
            out.sub_counter = Some(match sc {
                1 => "BarsBeats".to_owned(),
                4 => "Samples".to_owned(),
                _ => sc.to_string(),
            });
        }
    }
    if let Some(b16) = Block::find(tree, 0x2016) {
        let o16 = own_bytes(d, b16);
        if o16.len() >= 68
            && byte(&o16, 59) == 0x40
            && byte(&o16, 67) == 0x40
            && let (Some(s0), Some(s1)) = (le_u64(&o16, 52, 7), le_u64(&o16, 60, 7))
        {
            out.timeline_selection_ticks = Some([plain_ticks(s0), plain_ticks(s1)]);
        }
    }
    out
}

/// Parse markers / memory locations (`0x2030 > 0x2077`).
pub fn parse_markers(d: &[u8], tree: &[Block], warn: &mut Vec<String>) -> Vec<Marker> {
    let lst = Block::walk_all(tree).into_iter().find(|n| n.content_type == 0x2030 && n.kids.iter().any(|k| k.content_type == 0x2077));
    let Some(lst) = lst else { return Vec::new() };
    let mut out = Vec::new();
    for m in &lst.kids {
        if m.content_type != 0x2077 {
            continue;
        }
        if out.len() >= MAX_MARKERS {
            warn.push(format!("more than {MAX_MARKERS} markers: the rest are left out"));
            break;
        }
        let s = m.data_start();
        let m_end = m.end().min(d.len());
        let Some(ln) = u32_at(d, s + 6).and_then(|v| usize::try_from(v).ok()) else {
            continue;
        };
        if ln > 1024 || s + 15 + ln > m_end {
            warn.push(format!("marker at 0x{:x} does not match", m.pos));
            continue;
        }
        let Some(raw) = le_u64(d, s + 10 + ln, 5) else { continue };
        let Some(number) = u16_at(d, s) else { continue };
        let Some(name_bytes) = d.get(s + 10..s + 10 + ln) else { continue };
        let name = String::from_utf8_lossy(name_bytes).into_owned();
        let flags = d.get(s + 2..s + 4).map(|sl| format!("{:02x}{:02x}", byte(sl, 0), byte(sl, 1))).unwrap_or_default();
        let kind_code = d.get(s + 3).copied().unwrap_or(0) & 3;
        let kind = match kind_code {
            0 => serde_json::Value::from("none"),
            1 => serde_json::Value::from("marker"),
            2 => serde_json::Value::from("selection"),
            other => serde_json::Value::from(other),
        };
        let is_marker = kind_code == 1;
        let is_ticks = d.get(s + 10 + ln + 7).copied() == Some(0x40) || raw >= crate::primitives::OFFSET_FORM;
        let unit = if is_ticks { "ticks" } else { "samples" }.to_owned();
        let samples = if is_ticks { None } else { Some(raw) };
        let raw2 = le_u64(d, s + 10 + ln + 8, 5).unwrap_or(0);
        let end = if is_ticks { plain_ticks(raw2) } else { raw2 };
        let q = s + 10 + ln;
        let comment = u32_at(d, q + 66).and_then(|v| usize::try_from(v).ok()).filter(|&nt| nt <= 64 && q + 70 + nt + 9 <= m_end).and_then(|nt| {
            let a = q + 70 + nt;
            let tag = u32_at(d, a)?;
            let (cl_off, txt_off) = match tag {
                1 => (a + 5, a + 9),
                2 if a + 10 <= m_end => (a + 6, a + 10),
                _ => return Some(String::new()),
            };
            let cl = u32_at(d, cl_off).and_then(|v| usize::try_from(v).ok())?;
            if cl < 4096 && txt_off.checked_add(cl).is_some_and(|e| e <= m_end) {
                d.get(txt_off..txt_off + cl).map(|sl| String::from_utf8_lossy(sl).into_owned())
            } else {
                Some(String::new())
            }
        });
        out.push(Marker { number, name, ticks: plain_ticks(raw), flags, kind, is_marker, unit, samples, end, comment });
    }
    out
}

fn map_header(d: &[u8], b: &Block, magic: &[u8], count_len: usize) -> Option<(usize, usize)> {
    let s = b.data_start();
    if d.get(s..s.checked_add(magic.len())?)? != magic {
        return None;
    }
    let cnt = usize::try_from(u32_at(d, s + magic.len() + 6)?).ok()?;
    Some((s + magic.len() + 6 + count_len, cnt))
}

/// All `0x2028` tempo blocks in tree walk order.
#[must_use]
pub fn tempo_blocks(tree: &[Block]) -> Vec<&Block> {
    Block::walk_all(tree).into_iter().filter(|n| n.content_type == 0x2028).collect()
}

/// Parse constant-tempo events from the `which`-th `0x2028` block.
pub fn parse_tempo(d: &[u8], tree: &[Block], warn: &mut Vec<String>, which: usize) -> Vec<TempoEvent> {
    let blocks = tempo_blocks(tree);
    let Some(&b) = blocks.get(which) else {
        return Vec::new();
    };
    let Some((mut p, cnt)) = map_header(d, b, b"Tempo", 8) else {
        warn.push(format!("tempo block {which} header"));
        return Vec::new();
    };
    if cnt > MAX_MAP_EVENTS {
        warn.push(format!("tempo block {which} header"));
        return Vec::new();
    }
    let end = b.end().min(d.len());
    let mut out = Vec::with_capacity(cnt.min(end.saturating_sub(p) / 61));
    for _ in 0..cnt {
        if p + 61 > end || d.get(p..p + 5) != Some(b"Const") {
            warn.push(format!("tempo record at 0x{p:x} is not 'Const'"));
            break;
        }
        let Some(raw_ticks) = le_u64(d, p + 30, 5) else { break };
        let Some(bpm) = crate::primitives::f64_at(d, p + 40) else { break };
        out.push(TempoEvent { ticks: plain_ticks(raw_ticks), bpm: round_n(bpm, 6), bpm_exact: bpm });
        p += 61;
    }
    out
}

/// Parse meter events from the `which`-th `0x2029` block.
pub fn parse_meter(d: &[u8], tree: &[Block], warn: &mut Vec<String>, which: usize) -> Vec<MeterEvent> {
    let blocks: Vec<&Block> = Block::walk_all(tree).into_iter().filter(|n| n.content_type == 0x2029).collect();
    let Some(&b) = blocks.get(which) else {
        return Vec::new();
    };
    let Some((mut p, cnt)) = map_header(d, b, b"Meter", 4) else {
        warn.push("meter block header".to_owned());
        return Vec::new();
    };
    if cnt > MAX_MAP_EVENTS {
        warn.push("meter block header".to_owned());
        return Vec::new();
    }
    let end = b.end().min(d.len());
    let mut out = Vec::with_capacity(cnt.min(end.saturating_sub(p) / 36));
    for _ in 0..cnt {
        if p + 36 > end {
            warn.push("meter record beyond the block".to_owned());
            break;
        }
        let (Some(raw_t), Some(bar), Some(num), Some(den)) = (u64_at(d, p), u32_at(d, p + 8), u32_at(d, p + 12), u32_at(d, p + 16)) else {
            break;
        };
        out.push(MeterEvent { ticks: plain_ticks(raw_t), bar, numerator: num, denominator: den });
        p += 36;
    }
    out
}

fn key_name(major: bool, sf: i8) -> String {
    if major {
        match sf {
            -7 => "Cb",
            -6 => "Gb",
            -5 => "Db",
            -4 => "Ab",
            -3 => "Eb",
            -2 => "Bb",
            -1 => "F",
            0 => "C",
            1 => "G",
            2 => "D",
            3 => "A",
            4 => "E",
            5 => "B",
            6 => "F#",
            7 => "C#",
            _ => return sf.to_string(),
        }
    } else {
        match sf {
            -7 => "Abm",
            -6 => "Ebm",
            -5 => "Bbm",
            -4 => "Fm",
            -3 => "Cm",
            -2 => "Gm",
            -1 => "Dm",
            0 => "Am",
            1 => "Em",
            2 => "Bm",
            3 => "F#m",
            4 => "C#m",
            5 => "G#m",
            6 => "D#m",
            7 => "A#m",
            _ => return sf.to_string(),
        }
    }
    .to_owned()
}

/// Parse key signatures (`0x271B > 0x2433 > 0x2432`).
#[must_use]
pub fn parse_key_signatures(d: &[u8], tree: &[Block]) -> Vec<KeySignature> {
    let Some(b33) = Block::find(tree, 0x2433) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in &b33.kids {
        if k.content_type != 0x2432 || k.size < 13 {
            continue;
        }
        let p = k.data_start();
        let Some(t_raw) = le_u64(d, p, 7) else { continue };
        let (Some(&maj_b), Some(&sh_b), Some(&cnt_b)) = (d.get(p + 8), d.get(p + 9), d.get(p + 10)) else {
            continue;
        };
        let major = maj_b != 0;
        let cnt = i8::try_from(cnt_b).unwrap_or(0);
        let sf = if sh_b != 0 { cnt } else { -cnt };
        out.push(KeySignature { ticks: plain_ticks(t_raw), key: key_name(major, sf), major, sharps: sf });
    }
    out
}

/// Parse automation write-enable switches (`0x204A`).
#[must_use]
pub fn parse_write_enables(d: &[u8], tree: &[Block]) -> Option<Vec<u8>> {
    let b4a = Block::find(tree, 0x204A)?;
    let p = b4a.data_start();
    let end = b4a.end().min(d.len());
    let cnt = u32_at(d, p + 20).and_then(|v| usize::try_from(v).ok())?;
    if cnt > 64 || p + 24 + cnt > end {
        return None;
    }
    d.get(p + 24..p + 24 + cnt).map(<[u8]>::to_vec)
}

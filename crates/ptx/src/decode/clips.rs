// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/clips.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: no indexing or slicing (`byte`/`span`/`get`); a loop's raw hex kept
// to its first `MAX_RAW_LOOP_BYTES`; playlist names de-duplicated through a set (the list scan
// was quadratic in the number of playlists); at most `MAX_PLACEMENTS` playlist entries looked at per
// list read (nested playlists in a crafted file made the upstream read the same entries again).
//! Regions (`0x2628`), UMIDs (`0x1004`/`0x1003`), playlists (`0x1054`/`0x1052`),
//! clip gains (`0x2637`), fades (`0x262F`), clip groups (`0x262C`/`0x2428`), loops (`0x242B`),
//! and MIDI placements (`0x1058 > 0x1057 > 0x1056 > 0x104F`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::primitives::{
    byte, f32_at, f64_at, find_bytes, i16_at, le_i64, le_u64, lp_str, plain_ticks, round_n, scan_lp_strings, span, u16_at, u32_at,
};

/// At most this many bytes of a clip loop's record are kept as hex.
pub const MAX_RAW_LOOP_BYTES: usize = 64;
/// At most this many playlist entries are looked at by one of the playlist readers (all playlists
/// together).
pub const MAX_PLACEMENTS: usize = 1_000_000;
use crate::tree::Block;

/// Region definition (`0x2628`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Region {
    pub name: String,
    pub flag: u8,
    pub extra: u8,
    pub w: [u64; 4],
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_offset: Option<u64>,
    pub start: u64,
    pub length: u64,
    pub time_stamp: i64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_gain_index: Option<Option<usize>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_gain: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub list: Option<u32>,
    #[serde(skip)]
    pub file_index: Option<usize>,
}

struct RegionVarints {
    flag: u8,
    ex: u8,
    v: [u64; 4],
    sync_offset: Option<u64>,
    widths: [usize; 4],
}

/// Parse the `0x2628` nibble-packed region fields.
///
/// Low nibble `b2 & 0x0F` encodes an inline sample `sync_offset` placed immediately between `W1` and `W2`.
fn parse_2628_varints(d: &[u8], q: usize) -> Option<RegionVarints> {
    let [flag, b1, b2, b3, ex] = crate::primitives::bytes_at::<5>(d, q)?;
    let widths = [usize::from(b1 >> 4), usize::from(b2 >> 4), usize::from(b3 >> 4), usize::from(b3 & 15)];
    let sync_w = usize::from(b2 & 15);
    if sync_w > 8 || widths.iter().any(|&w| w > 8) {
        return None;
    }
    let mut o = q + 5;
    let mut v = [0u64; 4];
    let [w0, ..] = widths;
    if w0 > 0 {
        v[0] = le_u64(d, o, w0)?;
        o += w0;
    }
    let sync_offset = if sync_w > 0 {
        let sv = le_u64(d, o, sync_w)?;
        o += sync_w;
        Some(sv)
    } else {
        None
    };
    for (slot, &w) in v.iter_mut().zip(widths.iter()).skip(1) {
        if w > 0 {
            *slot = le_u64(d, o, w)?;
            o += w;
        }
    }
    Some(RegionVarints { flag, ex, v, sync_offset, widths })
}

/// Playlist metadata from `0x1052` leaf records.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PlaylistMeta {
    pub name: String,
    pub time_base: serde_json::Value,
}

/// Clip gain lane (`0x2637`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipGainLane {
    pub points: Vec<(u32, f64)>,
    pub base: u32,
}

/// Decoded `0x242B` clip loop child.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClipLoop {
    pub mode: u8,
    pub bars: u32,
    pub beats: u32,
    pub ticks: u32,
    pub count: u32,
    pub unit: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim_ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iter_ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub trim_samples: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub iter_samples: Option<u64>,
    pub raw: String,
}

/// Audio playlist placement entry (`0x104F`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioPlacement {
    pub kind: serde_json::Value,
    pub sub: u8,
    pub index: u16,
    pub muted: bool,
    pub link: i16,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "loop")]
    pub loop_info: Option<ClipLoop>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_name: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_length: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sync_offset: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub clip_start_samples: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_playlist_index: Option<u32>,
}

/// Fade object (`0x262F`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct FadeObject {
    pub length: u64,
    #[serde(rename = "type")]
    pub fade_type: serde_json::Value,
    pub slope: serde_json::Value,
    pub curve: f64,
    pub s_curve: [u8; 2],
    pub size: usize,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length2: Option<u64>,
}

/// Clip-group region definition (`0x262C > 0x262B > 0x2628`).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ClipGroupRegion {
    pub name: String,
    pub unit: String,
    pub length: u64,
    pub origin: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub origin_ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub length_samples: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub member_playlist_index: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tempo: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group_id: Option<u32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<u16>,
}

/// MIDI placement (`0x1058 > 0x1057 > ... > 0x104F`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiPlacement {
    pub region: u16,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ticks: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub samples: Option<i64>,
    pub muted: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    #[serde(rename = "loop")]
    pub loop_info: Option<ClipLoop>,
}

/// Region blocks (`0x2628`) in the exact order `parse_regions` decodes them, returning `(regions, block_positions)`.
pub fn parse_regions(d: &[u8], tree: &[Block], warn: &mut Vec<String>) -> (Vec<Region>, Vec<usize>) {
    let mut parent_2629_ends: BTreeMap<usize, usize> = BTreeMap::new();
    for w in Block::walk_all(tree) {
        if w.content_type == 0x2629 {
            let wend = w.end().min(d.len());
            for k in &w.kids {
                if k.content_type == 0x2628 {
                    parent_2629_ends.insert(k.pos, wend);
                }
            }
        }
    }
    let lib = Block::find(tree, 0x2634);
    let mut src: Vec<&Block> = match lib {
        Some(l) => Block::walk_all(&l.kids),
        None => Block::walk_all(tree),
    };
    if lib.is_some() {
        let inlib: BTreeSet<usize> = src.iter().map(|b| b.pos).collect();
        for w in Block::walk_all(tree) {
            if w.content_type == 0x2629 {
                for k in &w.kids {
                    if k.content_type == 0x2628 && !inlib.contains(&k.pos) {
                        src.push(k);
                    }
                }
            }
        }
    }

    let mut out = Vec::new();
    let mut positions = Vec::new();
    for b in src {
        if b.content_type != 0x2628 {
            continue;
        }
        let p = b.data_start();
        let Some(ln) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
            continue;
        };
        if ln > 1024 {
            warn.push(format!("region at 0x{:x}: name length {ln}", b.pos));
            continue;
        }
        let q = p + 4 + ln;
        let Some(rv) = parse_2628_varints(d, q) else {
            if let Some([_, b1, b2, b3, _]) = crate::primitives::bytes_at::<5>(d, q)
                && ((b2 & 15) > 8 || (b1 >> 4) > 8 || (b2 >> 4) > 8 || (b3 >> 4) > 8 || (b3 & 15) > 8)
            {
                warn.push(format!("region at 0x{:x}: width > 8", b.pos));
            }
            continue;
        };
        let RegionVarints { flag, ex, v, sync_offset, widths } = rv;
        let Some(nm_sl) = d.get(p + 4..p + 4 + ln) else { continue };
        let name = String::from_utf8_lossy(nm_sl).into_owned();
        let ts = if widths[2] == 8 && v[2] >= (1u64 << 63) { v[2] as i64 } else { i64::try_from(v[2]).unwrap_or(i64::MAX) };
        positions.push(b.pos);
        let file_index = parent_2629_ends
            .get(&b.pos)
            .filter(|&&wend| b.end() + 4 <= wend)
            .and_then(|_| u32_at(d, b.end()))
            .and_then(|idx| usize::try_from(idx).ok());
        out.push(Region {
            name,
            flag,
            extra: ex,
            w: v,
            sync_offset,
            start: plain_ticks(v[0]),
            length: v[1],
            time_stamp: ts,
            clip_gain_index: None,
            clip_gain: None,
            list: None,
            file_index,
        });
    }
    (out, positions)
}

/// MIDI region chunk index per `0x2634 > 0x2633` wrapper.
#[must_use]
pub fn parse_region_lists(d: &[u8], tree: &[Block]) -> Vec<u32> {
    let Some(lib) = Block::find(tree, 0x2634) else {
        return Vec::new();
    };
    lib.kids.iter().filter(|w| w.content_type == 0x2633).filter_map(|w| u32_at(d, w.end().saturating_sub(4))).collect()
}

const AUDIO_EXTS: &[&str] = &[".wav", ".aif", ".aiff", ".bwf", ".mp3", ".m4a", ".flac", ".mxf", ".sd2"];
const UMID_LABEL: &[u8] = &[0x06, 0x0a, 0x2b, 0x34, 0x01, 0x01, 0x01, 0x05, 0x01, 0x01];

fn strip_audio_ext(name: &str) -> String {
    let lower = name.to_ascii_lowercase();
    for ext in AUDIO_EXTS {
        if lower.ends_with(ext)
            && let Some(stem) = name.get(..name.len() - ext.len())
        {
            return stem.to_owned();
        }
    }
    name.to_owned()
}

fn split_audio_stem_ext(name: &str) -> Option<(String, String)> {
    let lower = name.to_ascii_lowercase();
    for ext in AUDIO_EXTS {
        if lower.ends_with(ext)
            && let (Some(stem), Some(raw_ext)) = (name.get(..name.len() - ext.len()), name.get(name.len() - ext.len() + 1..))
        {
            return Some((stem.to_owned(), raw_ext.to_owned()));
        }
    }
    None
}

/// Map `{file_stem: raw_extension}` (without leading dot) from `0x1004 > 0x103A`.
#[must_use]
pub fn parse_audio_file_exts(d: &[u8], tree: &[Block]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(lst) = Block::find(tree, 0x1004) else {
        return out;
    };
    let Some(nb) = lst.kids.iter().find(|k| k.content_type == 0x103A) else {
        return out;
    };
    for (_, t) in scan_lp_strings(d, nb.data_start(), nb.end().min(d.len()), 1, 400) {
        if let Some((stem, ext)) = split_audio_stem_ext(&t) {
            out.entry(stem).or_insert(ext);
        }
    }
    out
}

/// Ordered audio file stems from `0x1004 > 0x103A` (indexed by `Region::file_index`).
#[must_use]
pub fn parse_audio_file_stems(d: &[u8], tree: &[Block]) -> Vec<String> {
    let Some(lst) = Block::find(tree, 0x1004) else {
        return Vec::new();
    };
    let Some(nb) = lst.kids.iter().find(|k| k.content_type == 0x103A) else {
        return Vec::new();
    };
    scan_lp_strings(d, nb.data_start(), nb.end().min(d.len()), 1, 400)
        .into_iter()
        .filter(|(_, t)| {
            let lower = t.to_ascii_lowercase();
            AUDIO_EXTS.iter().any(|ext| lower.ends_with(ext))
        })
        .map(|(_, t)| strip_audio_ext(&t))
        .collect()
}

/// Audio file UMID hex map `{file_stem: umid_hex}` from `0x1004 > (0x103A, 0x1003)`.
#[must_use]
pub fn parse_umids(d: &[u8], tree: &[Block]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(lst) = Block::find(tree, 0x1004) else {
        return out;
    };
    let names_blk = lst.kids.iter().find(|k| k.content_type == 0x103A);
    let mut names = Vec::new();
    if let Some(nb) = names_blk {
        for (_, t) in scan_lp_strings(d, nb.data_start(), nb.end().min(d.len()), 1, 400) {
            let lower = t.to_ascii_lowercase();
            if AUDIO_EXTS.iter().any(|ext| lower.ends_with(ext)) {
                names.push(t);
            }
        }
    }
    let recs: Vec<&Block> = lst.kids.iter().filter(|k| k.content_type == 0x1003).collect();
    for (nm, rec) in names.into_iter().zip(recs) {
        let stem = strip_audio_ext(&nm);
        let slice = d.get(rec.pos..rec.end().min(d.len())).unwrap_or(&[]);
        let umid = if let Some(u) = find_bytes(slice, UMID_LABEL, 0)
            && u + 32 <= slice.len()
        {
            span(slice, u, u + 32).iter().map(|b| format!("{b:02x}")).collect::<String>()
        } else {
            String::new()
        };
        out.insert(stem, umid);
    }
    out
}

/// Playlist leaf metadata (`0x1052` without children).
#[must_use]
pub fn parse_playlists(d: &[u8], tree: &[Block]) -> Vec<PlaylistMeta> {
    let mut out = Vec::new();
    for n in Block::walk_all(tree) {
        if n.content_type != 0x1052 || !n.kids.is_empty() {
            continue;
        }
        let Some(x) = d.get(n.data_start()..n.end().min(d.len())) else {
            continue;
        };
        if x.len() < 4 {
            continue;
        }
        let Some(ln) = u32_at(x, 0).and_then(|v| usize::try_from(v).ok()) else {
            continue;
        };
        if ln > 256 || 4 + ln + 5 > x.len() {
            continue;
        }
        let tb = byte(x, 4 + ln + 4);
        let time_base = match tb {
            1 => serde_json::Value::from("samples"),
            2 => serde_json::Value::from("ticks"),
            other => serde_json::Value::from(other),
        };
        out.push(PlaylistMeta { name: String::from_utf8_lossy(span(x, 4, 4 + ln)).into_owned(), time_base });
    }
    out
}

/// Clip gain first-point values (`0x2637`).
#[must_use]
pub fn parse_clip_gains(d: &[u8], tree: &[Block]) -> Vec<f64> {
    parse_clip_gain_lanes(d, tree).into_iter().map(|l| l.points.first().map_or(0.0, |p| p.1)).collect()
}

/// Full clip gain lanes from `0x2637`.
#[must_use]
pub fn parse_clip_gain_lanes(d: &[u8], tree: &[Block]) -> Vec<ClipGainLane> {
    let Some(b) = Block::find(tree, 0x2637) else {
        return Vec::new();
    };
    let end = b.end().min(d.len());
    let mut i = b.data_start() + 4;
    let mut out = Vec::new();
    while i + 8 <= end && d.get(i..i + 4) == Some(&[0x01, 0x46, 0x01, 0x00]) {
        let Some(body_len) = u32_at(d, i + 4).and_then(|v| usize::try_from(v).ok()) else {
            break;
        };
        let body = i + 8;
        if body + body_len > end || body + 14 > end {
            break;
        }
        let n = u32_at(d, body + 2).and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
        let base = u32_at(d, body + 10).unwrap_or(0);
        let mut pts = Vec::new();
        for k in 0..n {
            let p = body + 14 + k * 8;
            if p + 8 > body + body_len {
                break;
            }
            if let (Some(pos), Some(db)) = (u32_at(d, p), f32_at(d, p + 4)) {
                pts.push((pos, round_n(f64::from(db), 3)));
            }
        }
        out.push(ClipGainLane { points: pts, base });
        i = body + body_len;
    }
    out
}

const GAIN_TAIL: [u8; 8] = [0x00, 0x00, 0xFF, 0xFF, 0x04, 0x00, 0x04, 0x00];

/// Map `{0x2628 block_pos -> Option<clip_gain_index>}` per `0x2629 > 0x2628`.
#[must_use]
pub fn parse_region_gain_links(d: &[u8], tree: &[Block]) -> BTreeMap<usize, Option<usize>> {
    let mut out = BTreeMap::new();
    for w in Block::walk_all(tree) {
        if w.content_type != 0x2629 {
            continue;
        }
        let Some(r) = w.kids.iter().find(|k| k.content_type == 0x2628) else {
            continue;
        };
        let start = r.data_start();
        let end = r.end().min(d.len());
        if let Some(hay) = d.get(start..end)
            && let Some(at) = hay.windows(GAIN_TAIL.len()).rposition(|sl| sl == GAIN_TAIL)
        {
            let p = start + at;
            if p + 33 <= end
                && d.get(p + 8..p + 28).is_some_and(|z| z.iter().all(|&x| x == 0))
                && let Some(v) = u32_at(d, p + 29)
            {
                let idx = (v != u32::MAX).then(|| usize::try_from(v).ok()).flatten();
                out.insert(r.pos, idx);
            }
        }
    }
    out
}

/// Decode `0x242B` clip loop child.
#[must_use]
pub fn parse_clip_loop(o: &[u8]) -> ClipLoop {
    let mode = byte(o, 0);
    let bars = u32_at(o, 11).unwrap_or(0);
    let beats = u32_at(o, 15).unwrap_or(0);
    let ticks = u32_at(o, 19).unwrap_or(0);
    let count = u32_at(o, 27).unwrap_or(0);
    let is_ticks = byte(o, 38) == 0x40 || byte(o, 46) == 0x40;
    let unit = if is_ticks { "ticks" } else { "samples" }.to_owned();
    let trim_v = le_u64(o, 31, 7).unwrap_or(0);
    let iter_v = le_u64(o, 39, 7).unwrap_or(0);
    let raw = o.iter().take(MAX_RAW_LOOP_BYTES).map(|b| format!("{b:02x}")).collect();
    if is_ticks {
        ClipLoop {
            mode,
            bars,
            beats,
            ticks,
            count,
            unit,
            trim_ticks: Some(trim_v),
            iter_ticks: Some(iter_v),
            trim_samples: None,
            iter_samples: None,
            raw,
        }
    } else {
        ClipLoop {
            mode,
            bars,
            beats,
            ticks,
            count,
            unit,
            trim_ticks: None,
            iter_ticks: None,
            trim_samples: Some(trim_v),
            iter_samples: Some(iter_v),
            raw,
        }
    }
}

fn group_inner_positions(tree: &[Block]) -> BTreeSet<usize> {
    let mut set = BTreeSet::new();
    for g in Block::walk_all(tree) {
        if g.content_type == 0x2428 {
            for n in Block::walk_all(&g.kids) {
                set.insert(n.pos);
            }
        }
    }
    set
}

fn trailers_1050(d: &[u8], kids: &[Block]) -> BTreeMap<usize, u8> {
    let mut out = BTreeMap::new();
    for n in Block::walk_all(kids) {
        if n.content_type == 0x1050
            && let Some(k) = n.kids.first()
            && k.content_type == 0x104F
        {
            let q = k.end();
            if q < n.end().min(d.len())
                && let Some(&b) = d.get(q)
            {
                out.insert(k.pos, b);
            }
        }
    }
    out
}

fn entry_kind(o: &[u8], tr: Option<u8>) -> serde_json::Value {
    if o.get(18).copied() == Some(1) {
        return serde_json::Value::from("group");
    }
    if let Some(t) = tr {
        return serde_json::Value::from(if t == 1 { "fade" } else { "clip" });
    }
    match byte(o, 15) {
        3 => serde_json::Value::from("clip"),
        1 => serde_json::Value::from("fade"),
        other => serde_json::Value::from(other),
    }
}

fn parse_104f_audio_entry(d: &[u8], e: &Block, tr: Option<u8>, in_group: bool) -> Option<AudioPlacement> {
    let o = d.get(e.data_start()..e.end().min(d.len()))?;
    if o.len() < 18 {
        return None;
    }
    let kd = entry_kind(o, tr);
    let lp = e
        .kids
        .iter()
        .find(|k| k.content_type == 0x242B && k.size >= 49)
        .and_then(|lp_blk| d.get(lp_blk.data_start()..lp_blk.end().min(d.len())))
        .map(parse_clip_loop);
    let (ticks, samples) = if byte(o, 14) == 0x40 {
        (Some(plain_ticks(le_u64(o, 7, 5)?)), None)
    } else if in_group {
        let uv = le_u64(o, 7, 5)?;
        let sv = if uv >= crate::primitives::OFFSET_FORM { i64::try_from(uv - crate::primitives::OFFSET_FORM).ok()? } else { le_i64(o, 7, 5)? };
        (None, Some(sv))
    } else {
        let sv = le_i64(o, 7, 5)?;
        (None, Some(sv))
    };
    Some(AudioPlacement {
        kind: kd,
        sub: byte(o, 15),
        index: u16_at(o, 2)?,
        muted: byte(o, 0) == 1,
        link: i16_at(o, 16)?,
        loop_info: lp,
        ticks,
        samples,
        clip_name: None,
        clip_length: None,
        sync_offset: None,
        clip_start_samples: None,
        group_id: None,
        member_playlist_index: None,
    })
}

/// Audio placements grouped per playlist name (`channels=false`) or list of channel playlists (`channels=true`).
#[must_use]
pub fn parse_audio_placements(d: &[u8], tree: &[Block], channels: bool, groups: bool) -> BTreeMap<String, Vec<serde_json::Value>> {
    let mut out: BTreeMap<String, Vec<serde_json::Value>> = BTreeMap::new();
    let inner = group_inner_positions(tree);
    let mut walked = 0usize;
    for top in Block::walk_all(tree) {
        if top.content_type != 0x1054 || (inner.contains(&top.pos) != groups) {
            continue;
        }
        for pl in &top.kids {
            if pl.content_type != 0x1052 {
                continue;
            }
            let p = pl.data_start();
            let Some(n) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
                continue;
            };
            if n == 0 || n >= 256 || p + 4 + n > d.len() {
                continue;
            }
            let name = String::from_utf8_lossy(span(d, p + 4, p + 4 + n)).into_owned();
            let trs = trailers_1050(d, &pl.kids);
            let mut lst = Vec::new();
            for e in Block::walk_all(&pl.kids) {
                walked += 1;
                if walked > MAX_PLACEMENTS {
                    break;
                }
                if e.content_type != 0x104F {
                    continue;
                }
                if let Some(ent) = parse_104f_audio_entry(d, e, trs.get(&e.pos).copied(), groups)
                    && let Ok(val) = serde_json::to_value(ent)
                {
                    lst.push(val);
                }
            }
            if channels {
                out.entry(name).or_default().push(serde_json::Value::Array(lst));
            } else if !lst.is_empty() || !out.contains_key(&name) {
                out.insert(name, lst);
            }
        }
    }
    out
}

/// Every audio playlist inside `0x2428 > 0x1054 > 0x1052` in file order (indexed by `member_playlist_index`).
#[must_use]
pub fn parse_clip_group_playlists(d: &[u8], tree: &[Block]) -> Vec<Vec<AudioPlacement>> {
    let mut out: Vec<Vec<AudioPlacement>> = Vec::new();
    let inner = group_inner_positions(tree);
    let mut walked = 0usize;
    for top in Block::walk_all(tree) {
        if top.content_type != 0x1054 || !inner.contains(&top.pos) {
            continue;
        }
        for pl in &top.kids {
            if pl.content_type != 0x1052 {
                continue;
            }
            let trs = trailers_1050(d, &pl.kids);
            let mut lst = Vec::new();
            for e in Block::walk_all(&pl.kids) {
                walked += 1;
                if walked > MAX_PLACEMENTS {
                    break;
                }
                if e.content_type != 0x104F {
                    continue;
                }
                if let Some(ent) = parse_104f_audio_entry(d, e, trs.get(&e.pos).copied(), true) {
                    lst.push(ent);
                }
            }
            out.push(lst);
        }
    }
    out
}

/// Every audio playlist in file order `[(name, entries)]`.
#[must_use]
pub fn parse_audio_playlists(d: &[u8], tree: &[Block]) -> Vec<(String, Vec<AudioPlacement>)> {
    let mut out: Vec<(String, Vec<AudioPlacement>)> = Vec::new();
    let mut names: BTreeSet<String> = BTreeSet::new();
    let inner = group_inner_positions(tree);
    let mut walked = 0usize;
    for top in Block::walk_all(tree) {
        if top.content_type != 0x1054 || inner.contains(&top.pos) {
            continue;
        }
        for pl in &top.kids {
            if pl.content_type != 0x1052 {
                continue;
            }
            let p = pl.data_start();
            let Some(n) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
                continue;
            };
            if n == 0 || n >= 256 || p + 4 + n > d.len() {
                continue;
            }
            let name = String::from_utf8_lossy(span(d, p + 4, p + 4 + n)).into_owned();
            let trs = trailers_1050(d, &pl.kids);
            let mut lst = Vec::new();
            for e in Block::walk_all(&pl.kids) {
                walked += 1;
                if walked > MAX_PLACEMENTS {
                    break;
                }
                if e.content_type != 0x104F {
                    continue;
                }
                if let Some(mut ent) = parse_104f_audio_entry(d, e, trs.get(&e.pos).copied(), false) {
                    ent.loop_info = None; // Audio playlist loop metadata stays on `audio_placements`.
                    lst.push(ent);
                }
            }
            if !lst.is_empty() || !names.contains(&name) {
                names.insert(name.clone());
                out.push((name, lst));
            }
        }
    }
    out
}

/// Fade objects (`0x262F`) in file order.
#[must_use]
pub fn parse_fades(d: &[u8], tree: &[Block]) -> Vec<FadeObject> {
    let mut out = Vec::new();
    for e in Block::walk_all(tree) {
        if e.content_type != 0x262F {
            continue;
        }
        let Some(o) = d.get(e.data_start()..e.end().min(d.len())) else {
            continue;
        };
        if o.len() < 12 {
            continue;
        }
        let (w1, w2) = (usize::from(byte(o, 5) >> 4), usize::from(byte(o, 5) & 15));
        let p = 8 + w1 + w2;
        if p + 10 > o.len() {
            continue;
        }
        let length = if w1 > 0 { le_u64(o, 8, w1).unwrap_or(0) } else { 0 };
        let length2 = if w2 > 0 { le_u64(o, 8 + w1, w2) } else { None };
        let fade_type = match byte(o, p) {
            1 => serde_json::Value::from("crossfade"),
            2 => serde_json::Value::from("fade_out"),
            3 => serde_json::Value::from("fade_in"),
            other => serde_json::Value::from(other),
        };
        let slope = match byte(o, p + 1) {
            1 => serde_json::Value::from("equal_power"),
            2 => serde_json::Value::from("equal_gain"),
            other => serde_json::Value::from(other),
        };
        let curve = round_n(f64::from(f32_at(o, p + 2).unwrap_or(0.0)), 6);
        out.push(FadeObject { length, fade_type, slope, curve, s_curve: [byte(o, p + 6), byte(o, p + 9)], size: o.len(), length2 });
    }
    out
}

/// Clip-group region definitions (`0x262C > 0x262B > 0x2628`).
#[must_use]
pub fn parse_clip_group_regions(d: &[u8], tree: &[Block]) -> Vec<ClipGroupRegion> {
    let Some(b2c) = Block::find(tree, 0x262C) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in &b2c.kids {
        if k.content_type != 0x262B {
            continue;
        }
        let Some(r) = k.kids.first().filter(|c| c.content_type == 0x2628) else {
            continue;
        };
        let p0 = r.data_start();
        let re = r.end().min(d.len());
        if p0 + 9 > re {
            continue;
        }
        let Some((nm, p)) = lp_str(d, p0, 1024) else {
            continue;
        };
        if p + 5 > re {
            continue;
        }
        let vals = parse_2628_varints(d, p).map_or([0u64; 4], |rv| rv.v);
        let b23 = r.kids.iter().find(|x| x.content_type == 0x2523);
        let post = b23.and_then(|bx| bx.kids.last().map(|last| (last.end(), bx.end().min(d.len())))).and_then(|(s, e)| d.get(s..e)).unwrap_or(&[]);
        let is_ticks = d.get(r.pos + 1).copied() == Some(4) || (post.len() >= 28 && byte(post, 24) == 1);
        let unit = if is_ticks { "ticks" } else { "samples" }.to_owned();
        let member_playlist_index = if post.len() >= 20 { u32_at(post, 16) } else { None };
        let tempo = if post.len() >= 76 { f64_at(post, 36).map(|t| round_n(t, 4)) } else { None };
        let ktail = d.get(re..k.end().min(d.len())).unwrap_or(&[]);
        let (group_id, channel) =
            if ktail.len() >= 6 { (u32_at(ktail, 0), crate::primitives::bytes_at::<2>(ktail, 4).map(u16::from_be_bytes)) } else { (None, None) };
        out.push(ClipGroupRegion {
            name: nm,
            unit,
            length: vals[1],
            origin: vals[2],
            length_ticks: is_ticks.then_some(vals[1]),
            origin_ticks: is_ticks.then_some(vals[2]),
            length_samples: (!is_ticks).then_some(vals[1]),
            member_playlist_index,
            tempo,
            group_id,
            channel,
        });
    }
    out
}

/// MIDI placements per `0x1058 > 0x1057` route (`0x104F` children).
#[must_use]
pub fn parse_midi_placements(d: &[u8], tree: &[Block]) -> Vec<Vec<MidiPlacement>> {
    let Some(par) = Block::find(tree, 0x1058) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for rt in &par.kids {
        if rt.content_type != 0x1057 {
            continue;
        }
        let mut ents = Vec::new();
        for e in Block::walk_all(&rt.kids) {
            if e.content_type != 0x104F {
                continue;
            }
            let Some(o) = d.get(e.data_start()..e.end().min(d.len())) else {
                continue;
            };
            if o.len() < 12 {
                continue;
            }
            let muted = byte(o, 0) == 1;
            let region = u16_at(o, 2).unwrap_or(0);
            let is_ticks = o.get(14).copied() == Some(0x40);
            let raw_pos = le_u64(o, 7, 5).unwrap_or(0);
            let (ticks, samples) = if is_ticks || raw_pos >= crate::primitives::OFFSET_FORM {
                (Some(plain_ticks(raw_pos)), None)
            } else {
                (None, Some(le_i64(o, 7, 5).unwrap_or(0)))
            };
            let lp = e
                .kids
                .iter()
                .find(|k| k.content_type == 0x242B && k.size >= 49)
                .and_then(|lp_blk| d.get(lp_blk.data_start()..lp_blk.end().min(d.len())))
                .map(parse_clip_loop);
            ents.push(MidiPlacement { region, ticks, samples, muted, loop_info: lp });
        }
        out.push(ents);
    }
    out
}

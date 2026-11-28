// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/tracks.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: no indexing or slicing; at most `MAX_TRACKS` tracks, `MAX_GROUPS`
// groups and `MAX_ALTERNATES` alternate playlists per track; group names and alternate
// playlists found through sets and maps (the list scans were quadratic).
//! Track list (`0x2519`), MIDI/instrument routes (`0x1058`), folders (`0x210C`), groups (`0x202B`),
//! track colours (`0x251A`), Elastic Audio (`0x200B`), MPE (`0x2010`), comments (`0x2015`), inputs (`0x261B`), and playlists (`0x1015`).

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::decode::mixer::subtree;
use crate::decode::plugins::{Insert, parse_elck_params};
use crate::primitives::{byte, find_bytes, i16_at, lp_str, scan_lp_strings, span, u16_at, u32_at};

/// At most this many tracks are read (Pro Tools itself allows far fewer).
pub const MAX_TRACKS: usize = 8_192;
/// At most this many groups are read.
pub const MAX_GROUPS: usize = 4_096;
/// At most this many alternate playlists are listed for one track.
pub const MAX_ALTERNATES: usize = 256;
use crate::tree::Block;

/// Track summary and per-channel attributes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Track {
    pub name: String,
    pub list_name: String,
    pub kind: u16,
    #[serde(rename = "type")]
    pub track_type: String,
    pub stereo: bool,
    pub id: String,
    pub number: u8,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub format: Option<String>,
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stem_index: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_enable: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_mode: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub user_delay: Option<i32>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub comment: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub height_code: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub solo: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hidden: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elastic_mode: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elastic_params: Option<BTreeMap<String, serde_json::Value>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timebase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mpe: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inserts: Option<Vec<Insert>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_path: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub input_type: Option<u8>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub colour: Option<i16>,
}

/// Detailed track group (`0x202B > 0x202A`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GroupDetail {
    pub name: String,
    #[serde(rename = "type")]
    pub group_type: u8,
    pub id: u32,
    pub enabled: bool,
    pub member_indices: Vec<u16>,
    pub members: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attributes: Option<u32>,
}

/// Alternate playlist info for an audio track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AudioPlaylistAlt {
    pub name: String,
    pub channel_playlists: Vec<u16>,
}

/// Active & alternate playlist info per track.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum TrackPlaylists {
    Audio { active: String, channel_playlists: Vec<u16>, alternates: Vec<AudioPlaylistAlt> },
    Midi { active: String, active_index: usize, alternates: Vec<String> },
}

fn track_kind_name(kind: u16) -> String {
    match kind {
        0 => "Audio".to_owned(),
        1 => "Midi".to_owned(),
        2 => "Aux".to_owned(),
        5 => "Master".to_owned(),
        7 => "Instrument".to_owned(),
        9 => "VCA".to_owned(),
        11 => "BasicFolder".to_owned(),
        other => format!("Other_{other}"),
    }
}

fn fmt_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}

/// Parse route block names (`0x1058 > 0x1057`).
#[must_use]
pub fn parse_route_names(d: &[u8], tree: &[Block]) -> Vec<String> {
    let Some(par) = Block::find(tree, 0x1058) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in &par.kids {
        if k.content_type == 0x1057 {
            let s = k.data_start();
            if let Some(ln) = u32_at(d, s).and_then(|v| usize::try_from(v).ok())
                && ln <= 1024
                && s + 4 + ln <= k.end().min(d.len())
                && let Some(slice) = d.get(s + 4..s + 4 + ln)
            {
                out.push(String::from_utf8_lossy(slice).into_owned());
            }
        }
    }
    out
}

/// Parse tracks from `0x2519` head table, overriding MIDI/Instrument track names with `0x1058 > 0x1057`.
pub fn parse_tracks(d: &[u8], tree: &[Block], warn: &mut Vec<String>) -> Vec<Track> {
    let Some(b) = Block::find(tree, 0x2519) else {
        return Vec::new();
    };
    let end = b.kids.first().map_or_else(|| b.end().min(d.len()), |k| k.pos.min(d.len()));
    let Some(pre) = u32_at(d, b.data_start() + 10).and_then(|v| usize::try_from(v).ok()) else {
        return Vec::new();
    };
    if pre > 4096 {
        warn.push(format!("u16 list length {pre} at +10 > 4096"));
        return Vec::new();
    }
    let mut p = b.data_start() + 14 + pre * 2;
    let Some(n) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
        return Vec::new();
    };
    p += 4;
    if n > MAX_TRACKS {
        warn.push(format!("{n} tracks: the first {MAX_TRACKS} are read"));
    }
    let mut out = Vec::with_capacity(n.min(MAX_TRACKS));
    for i in 0..n.min(MAX_TRACKS) {
        if p + 6 > end {
            warn.push(format!("track entry {i} beyond the block"));
            return out;
        }
        let (Some(kind), Some(ln)) = (u16_at(d, p), u32_at(d, p + 2).and_then(|v| usize::try_from(v).ok())) else {
            return out;
        };
        if ln > 1024 || p + 6 + ln + 6 + 15 > end + 3 {
            warn.push(format!("track entry {i} beyond the block"));
            return out;
        }
        let name_at = p + 6;
        let tag = name_at + ln + 6;
        if d.get(tag..tag + 4) != Some(&[0x2a, 0x00, 0x00, 0x00]) {
            warn.push(format!("track entry {i}: id tag 2A000000 not after the name"));
            return out;
        }
        let Some(name_slice) = d.get(name_at..name_at + ln) else {
            return out;
        };
        let name = String::from_utf8_lossy(name_slice).into_owned();
        let stereo = d.get(name_at + ln).copied() == Some(1);
        let (Some(id_a), Some(id_b), Some(&num)) = (d.get(tag + 4..tag + 8), d.get(tag + 8..tag + 12), d.get(tag + 12)) else {
            return out;
        };
        let id = format!("{{00000000-2a000000-{}-{}}}", fmt_hex(id_a), fmt_hex(id_b));
        let format = if matches!(kind, 0 | 2 | 5 | 7) { Some(if stereo { "Stereo" } else { "Mono" }.to_owned()) } else { None };
        out.push(Track {
            name: name.clone(),
            list_name: name,
            kind,
            track_type: track_kind_name(kind),
            stereo,
            id,
            number: num,
            format,
            folder: None,
            output: None,
            stem_index: None,
            active: None,
            record_enable: None,
            automation_mode: None,
            user_delay: None,
            comment: None,
            height_code: None,
            solo: None,
            hidden: None,
            elastic_mode: None,
            elastic_params: None,
            timebase: None,
            mpe: None,
            inserts: None,
            input_path: None,
            input: None,
            input_type: None,
            colour: None,
        });
        p = tag + 4 + 8 + 3;
    }

    let routes = parse_route_names(d, tree);
    let target_indices: Vec<usize> = out.iter().enumerate().filter_map(|(idx, t)| matches!(t.kind, 1 | 7).then_some(idx)).collect();
    if routes.len() == target_indices.len() {
        for (idx, rname) in target_indices.into_iter().zip(routes) {
            if let Some(t) = out.get_mut(idx) {
                t.name = rname;
            }
        }
    } else if !target_indices.is_empty() {
        warn.push(format!("{} route names for {} MIDI / instrument tracks: list names kept", routes.len(), target_indices.len()));
    }
    out
}

/// Solo switch from channel `0x102D`.
#[must_use]
pub fn parse_solo(d: &[u8], ch: &Block) -> Option<bool> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let inf = all.into_iter().find(|x| x.content_type == 0x102D)?;
    let nlen = usize::try_from(u32_at(d, inf.pos + 18)?).ok()?;
    let idx = inf.pos + 161 + nlen;
    if idx < inf.end().min(d.len()) { d.get(idx).map(|&b| b == 1) } else { None }
}

/// Automation mode from channel `0x102D`.
#[must_use]
pub fn parse_automation_mode(d: &[u8], ch: &Block) -> Option<serde_json::Value> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let inf = all.into_iter().find(|x| x.content_type == 0x102D)?;
    let nlen = usize::try_from(u32_at(d, inf.pos + 18)?).ok()?;
    let idx = inf.pos + 162 + nlen;
    let &code = d.get(idx).filter(|_| idx < inf.end().min(d.len()))?;
    Some(match code {
        0 => serde_json::Value::from("off"),
        1 => serde_json::Value::from("read"),
        2 => serde_json::Value::from("touch"),
        3 => serde_json::Value::from("latch"),
        4 => serde_json::Value::from("write"),
        other => serde_json::Value::from(other),
    })
}

/// User delay offset in samples from channel container (`0x261B` or channel itself).
#[must_use]
pub fn parse_user_delay(d: &[u8], ch: &Block) -> Option<i32> {
    let box_blk = ch.kids.iter().find(|k| k.content_type == 0x261B).unwrap_or(ch);
    let last = box_blk.kids.iter().rfind(|k| k.content_type == 0x260D)?;
    let lo = box_blk.kids.iter().filter(|k| k.content_type == 0x485A && k.pos > last.pos).map(Block::end).chain(std::iter::once(last.end())).max()?;
    let hi = box_blk.kids.iter().find(|k| k.pos > last.pos && k.content_type != 0x485A).map_or(box_blk.end(), |k| k.pos);
    let gap = d.get(lo..hi.min(d.len()))?;
    let i = if gap.len() >= 4 && byte(gap, 0) == 1 && byte(gap, 1) <= 1 { 0 } else { find_bytes(gap, &[1, 1], 0)? };
    let n = usize::from(u16_at(gap, i + 2)?);
    if n > 64 {
        return None;
    }
    let j = i.checked_add(4)?.checked_add(4 * n)?;
    crate::primitives::i32_at(gap, j)
}

/// Track comment and height code from `0x2015`.
#[must_use]
pub fn parse_track_comment(d: &[u8], ch: &Block) -> (Option<String>, Option<u16>) {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let Some(b) = all.into_iter().find(|n| n.content_type == 0x2015) else {
        return (None, None);
    };
    let s = b.data_start();
    let e = b.end().min(d.len());
    if d.get(s).copied() != Some(0x5A) {
        return (None, None);
    }
    let Some(first_sz) = u32_at(d, s + 3).and_then(|v| usize::try_from(v).ok()) else {
        return (None, None);
    };
    let c = s + 7 + first_sz;
    if c + 12 > e {
        return (None, None);
    }
    let Some(ln) = u32_at(d, c + 8).and_then(|v| usize::try_from(v).ok()) else {
        return (None, None);
    };
    if ln > 4096 || c + 12 + ln + 9 > e {
        return (None, None);
    }
    let cm = d.get(c + 12..c + 12 + ln).map(|sl| String::from_utf8_lossy(sl).into_owned());
    let hc = u16_at(d, c + 6);
    (cm, hc)
}

const ELASTIC_ALGOS: &[&str] = &["Polyphonic", "Rhythmic", "Monophonic", "Varispeed", "X-Form"];

/// Elastic Audio algorithm name from `0x200B`.
#[must_use]
pub fn parse_elastic_mode(d: &[u8], ch: &Block) -> Option<String> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let b0b = all.into_iter().find(|n| n.content_type == 0x200B)?;
    let strs = scan_lp_strings(d, b0b.data_start(), b0b.end().min(d.len()), 5, 20);
    strs.into_iter().rev().map(|(_, s)| s).find(|s| ELASTIC_ALGOS.contains(&s.as_str()))
}

/// Elastic Audio processor parameters (`0x200B > 0x252C` `elckComplete Controls State`).
#[must_use]
pub fn parse_elastic_params(d: &[u8], ch: &Block) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let Some(b0b) = all.into_iter().find(|n| n.content_type == 0x200B) else {
        return out;
    };
    let mut sub = Vec::new();
    subtree(b0b, &mut sub);
    for b in sub {
        if b.content_type == 0x252C {
            let s = b.data_start();
            let e = b.end().min(d.len());
            if let Some(slice) = d.get(s..e)
                && let Some(idx) = find_bytes(slice, b"elckComplete Controls State", 0)
            {
                out.extend(parse_elck_params(slice.get(idx + 4..).unwrap_or(&[])));
            }
        }
    }
    out
}

/// Timebase (`"Ticks"` or `"Samples"`) for MIDI / Instrument tracks from `0x2010`.
#[must_use]
pub fn parse_track_timebase(d: &[u8], ch: &Block) -> Option<String> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let b10 = all.into_iter().find(|n| n.content_type == 0x2010)?;
    let s = b10.data_start();
    let e = b10.end().min(d.len());
    let slice = d.get(s..e)?;
    let idx = find_bytes(slice, &[0xfe, 0xff, 0x01], 0)?;
    if idx > 0 {
        match byte(slice, idx - 1) {
            2 => Some("Ticks".to_owned()),
            1 => Some("Samples".to_owned()),
            _ => None,
        }
    } else {
        None
    }
}

/// MPE mode (`"off"`, `"poly"`, `"mono"`) from MIDI/Instrument `0x2010` (2026.11+ layout).
#[must_use]
pub fn parse_mpe_mode(d: &[u8], ch: &Block) -> Option<String> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let b10 = all.into_iter().find(|n| n.content_type == 0x2010)?;
    let b_poly = d.get(b10.pos + 47).copied().unwrap_or(0);
    let b_mono = d.get(b10.pos + 80).copied().unwrap_or(0);
    if b_poly == 1 {
        Some("poly".to_owned())
    } else if b_mono == 1 {
        Some("mono".to_owned())
    } else {
        Some("off".to_owned())
    }
}

/// Input path number of an aux / routing-folder channel (`0x261E`).
#[must_use]
pub fn parse_aux_input_path(d: &[u8], ch: &Block) -> Option<u16> {
    let box_blk = ch.kids.iter().find(|x| x.content_type == 0x261B)?;
    let first = box_blk.kids.first().filter(|k| k.content_type == 0x102D)?;
    u16_at(d, first.end() + 1).filter(|&v| v != 0xFFFF)
}

/// Input name and input type (`2` = bus, `0` = hardware/none) from `0x261B`.
#[must_use]
pub fn parse_track_input(d: &[u8], ch: &Block) -> (Option<String>, Option<u8>) {
    let Some(box_blk) = ch.kids.iter().find(|x| x.content_type == 0x261B) else {
        return (None, None);
    };
    let Some(first) = box_blk.kids.first().filter(|k| k.content_type == 0x102D) else {
        return (None, None);
    };
    let Some(tail) = d.get(first.end()..box_blk.end().min(d.len())) else {
        return (None, None);
    };
    let Some(i) = find_bytes(tail, &[0x01, 0x01, 0x0a, 0x00], 0) else {
        return (None, None);
    };
    if i + 57 > tail.len() {
        return (None, None);
    }
    let Some(ln) = u32_at(tail, i + 53).and_then(|v| usize::try_from(v).ok()) else {
        return (None, None);
    };
    if ln > 256 || i + 57 + ln + 18 > tail.len() {
        return (None, None);
    }
    let nm = if ln > 0 { Some(String::from_utf8_lossy(span(tail, i + 57, i + 57 + ln)).into_owned()) } else { None };
    let ty = tail.get(i + 57 + ln + 17).copied();
    (nm, ty)
}

/// Map `{member_track_name: parent_folder_name}` from `0x210C`.
#[must_use]
pub fn parse_folder_map(d: &[u8], tree: &[Block], trks: &[Track]) -> BTreeMap<String, String> {
    let mut out = BTreeMap::new();
    let Some(b) = Block::find(tree, 0x210C) else {
        return out;
    };
    // Build id8 hex -> track name lookup from Track.id ("{00000000-2a000000-xxxxxxxx-yyyyyyyy}")
    let mut by_id: BTreeMap<String, String> = BTreeMap::new();
    for t in trks {
        if let Some(rest) = t.id.strip_prefix("{00000000-2a000000-") {
            let hex16: String = rest.chars().filter(|c| c.is_ascii_hexdigit()).collect();
            if hex16.len() == 16 {
                by_id.insert(hex16, t.name.clone());
            }
        }
    }
    let mut p = b.data_start();
    let end = b.end().min(d.len());
    let Some(n) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
        return out;
    };
    p += 4;
    for _ in 0..n {
        if p + 21 > end {
            break;
        }
        if d.get(p + 5..p + 9) != Some(&[0x2a, 0x00, 0x00, 0x00]) {
            break;
        }
        let fid_hex = fmt_hex(span(d, p + 9, p + 17));
        p += 17;
        let Some(m) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
            break;
        };
        p += 4;
        let fname = by_id.get(&fid_hex).cloned();
        for _ in 0..m {
            if p + 17 > end {
                break;
            }
            let kid_hex = fmt_hex(span(d, p + 9, p + 17));
            if let (Some(fnm), Some(knm)) = (&fname, by_id.get(&kid_hex)) {
                out.insert(knm.clone(), fnm.clone());
            }
            p += 17;
        }
    }
    out
}

/// Map `{track_name: Option<colour_index>}` from `0x251A` (byte 9 == 0).
#[must_use]
pub fn parse_track_colours(d: &[u8], tree: &[Block]) -> BTreeMap<String, Option<i16>> {
    let mut out = BTreeMap::new();
    for b in Block::walk_all(tree) {
        if b.content_type != 0x251A {
            continue;
        }
        let s = b.data_start();
        if d.get(s).copied() != Some(0) {
            continue;
        }
        let Some(ln) = u32_at(d, s + 2).and_then(|v| usize::try_from(v).ok()) else {
            continue;
        };
        let end_nm = s + 6 + ln;
        if ln <= 255
            && end_nm + 64 <= b.end().min(d.len())
            && let Some(col) = i16_at(d, end_nm + 62)
            && let Some(nm_sl) = d.get(s + 6..end_nm)
        {
            let nm = String::from_utf8_lossy(nm_sl).into_owned();
            out.insert(nm, if col == -2 { None } else { Some(col) });
        }
    }
    out
}

fn parse_202a_record(d: &[u8], a: &Block, trks: &[Track]) -> Option<GroupDetail> {
    let s = a.data_start();
    let end = a.end().min(d.len());
    let (nm, mut p) = lp_str(d, s, 256)?;
    if p + 11 > end {
        return None;
    }
    let ty = *d.get(p)?;
    let gid = u32_at(d, p + 1)?;
    let en = d.get(p + 5).copied().unwrap_or(0) != 0 && d.get(p + 6).copied().unwrap_or(0) != 0;
    p += 7;
    let mcnt = usize::try_from(u32_at(d, p)?).ok()?;
    p += 4;
    if mcnt > 4096 || p + mcnt * 2 > end {
        return None;
    }
    let mut member_indices = Vec::with_capacity(mcnt);
    for _ in 0..mcnt {
        member_indices.push(u16_at(d, p)?);
        p += 2;
    }
    let members = member_indices.iter().filter_map(|&idx| trks.get(usize::from(idx)).map(|t| t.name.clone())).collect();
    // Optional `0xFE 0xFF` + u32 attributes before 0x258E child
    let attributes = if d.get(p..p + 2) == Some(&[0xFE, 0xFF]) { u32_at(d, p + 2) } else { None };
    Some(GroupDetail { name: nm, group_type: ty, id: gid, enabled: en, member_indices, members, attributes })
}

/// Unique group names in file order (`0x202B > 0x202A`).
#[must_use]
pub fn parse_groups(d: &[u8], tree: &[Block]) -> Vec<String> {
    let mut out = Vec::new();
    let mut seen = BTreeSet::new();
    let Some(b2b) = Block::find(tree, 0x202B) else {
        return out;
    };
    for k in &b2b.kids {
        if out.len() >= MAX_GROUPS {
            break;
        }
        if k.content_type == 0x202A
            && let Some((nm, _)) = lp_str(d, k.data_start(), 256)
            && seen.insert(nm.clone())
        {
            out.push(nm);
        }
    }
    out
}

/// Full group definitions from the first half of `0x202B`.
#[must_use]
pub fn parse_group_details(d: &[u8], tree: &[Block], trks: &[Track]) -> Vec<GroupDetail> {
    let Some(b2b) = Block::find(tree, 0x202B) else {
        return Vec::new();
    };
    let s = b2b.data_start();
    let cnt = u32_at(d, s).and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
    let kids_202a: Vec<&Block> = b2b.kids.iter().filter(|k| k.content_type == 0x202A).take(if cnt > 0 { cnt } else { usize::MAX }).collect();
    let mut seen_names = BTreeSet::new();
    let mut out = Vec::new();
    for k in kids_202a.into_iter().take(MAX_GROUPS) {
        if let Some(gd) = parse_202a_record(d, k, trks)
            && (cnt > 0 || !seen_names.contains(&gd.name))
        {
            seen_names.insert(gd.name.clone());
            out.push(gd);
        }
    }
    out
}

fn strip_playlist_suffix(name: &str) -> &str {
    if let Some((prefix, suffix)) = name.rsplit_once('.')
        && !suffix.is_empty()
        && suffix.bytes().all(|b| b.is_ascii_digit())
    {
        return prefix;
    }
    name
}

/// Per-track active & alternate playlists (`0x1015 > 0x1014` and `0x1058 > 0x1057`).
#[must_use]
pub fn parse_track_playlists(d: &[u8], tree: &[Block], trks: &[Track]) -> BTreeMap<String, TrackPlaylists> {
    let mut out = BTreeMap::new();
    let atks: Vec<&Track> = trks.iter().filter(|t| t.kind == 0).collect();
    if let Some(b15) = Block::find(tree, 0x1015) {
        let mut entries: Vec<(String, Vec<u16>)> = Vec::new();
        for k in &b15.kids {
            if k.content_type != 0x1014 {
                continue;
            }
            let p = k.data_start();
            let e = k.end().min(d.len());
            if p + 9 > e {
                continue;
            }
            let Some(ln) = u32_at(d, p).and_then(|v| usize::try_from(v).ok()) else {
                continue;
            };
            if ln > 256 || p + 9 + ln > e {
                continue;
            }
            let nm = String::from_utf8_lossy(span(d, p + 4, p + 4 + ln)).into_owned();
            let ch_cnt = u32_at(d, p + 5 + ln).and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
            let mut pls = Vec::new();
            for c in 0..ch_cnt.min(16) {
                if p + 11 + ln + 2 * c <= e
                    && let Some(idx) = u16_at(d, p + 9 + ln + 2 * c)
                {
                    pls.push(idx);
                }
            }
            entries.push((nm, pls));
        }

        let top = Block::find(tree, 0x2624);
        let ach: Vec<&Block> = top.map(|t| t.kids.iter().filter(|ch| ch.content_type == 0x261C).collect()).unwrap_or_default();

        // Alternates of the playlists listed after the tracks' own, by base name (a set lookup
        // instead of a scan per track).
        let mut by_base: BTreeMap<String, Vec<usize>> = BTreeMap::new();
        for (j, (anm, _)) in entries.iter().enumerate().skip(atks.len()) {
            let list = by_base.entry(strip_playlist_suffix(anm).to_owned()).or_default();
            if list.len() <= MAX_ALTERNATES {
                list.push(j);
            }
        }
        for (idx, t) in atks.iter().enumerate() {
            let Some((first_nm, first_pls)) = entries.get(idx) else {
                break;
            };
            let (mut nm, mut pls) = (first_nm.clone(), first_pls.clone());
            let mut alts = None;
            if let Some(ch) = ach.get(idx) {
                let b1b = ch.kids.iter().find(|x| x.content_type == 0x261B);
                let b0b = ch.kids.iter().find(|x| x.content_type == 0x200B);
                if let (Some(b1b), Some(b0b)) = (b1b, b0b)
                    && b1b.end() + 8 <= b0b.pos
                    && let Some(mid) = d.get(b1b.end()..b0b.pos)
                    && let (Some(aidx_u32), Some(npl_u16)) = (u32_at(mid, 0), u16_at(mid, 4))
                {
                    let aidx = usize::try_from(aidx_u32).unwrap_or(usize::MAX);
                    let npl = usize::from(npl_u16);
                    if 6 + 4 * npl <= mid.len()
                        && let Some((anm, apls)) = entries.get(aidx)
                    {
                        (nm, pls) = (anm.clone(), apls.clone());
                        let mut list = Vec::new();
                        for c in 0..npl.min(MAX_ALTERNATES) {
                            if let Some(j_u32) = u32_at(mid, 6 + 4 * c) {
                                let j = usize::try_from(j_u32).unwrap_or(usize::MAX);
                                if j != aidx
                                    && let Some((jnm, jpls)) = entries.get(j)
                                {
                                    list.push(AudioPlaylistAlt { name: jnm.clone(), channel_playlists: jpls.clone() });
                                }
                            }
                        }
                        alts = Some(list);
                    }
                }
            }
            let alternates = alts.unwrap_or_else(|| {
                by_base
                    .get(strip_playlist_suffix(&nm))
                    .map(|js| {
                        js.iter()
                            .filter_map(|&j| entries.get(j))
                            .filter(|(anm, _)| anm != &nm)
                            .take(MAX_ALTERNATES)
                            .map(|(anm, apls)| AudioPlaylistAlt { name: anm.clone(), channel_playlists: apls.clone() })
                            .collect()
                    })
                    .unwrap_or_default()
            });
            out.insert(t.name.clone(), TrackPlaylists::Audio { active: nm, channel_playlists: pls, alternates });
        }
    }

    let mtks: Vec<&Track> = trks.iter().filter(|t| matches!(t.kind, 1 | 7)).collect();
    let rnames = parse_route_names(d, tree);
    let mut midi_by_base: BTreeMap<&str, Vec<&String>> = BTreeMap::new();
    for rn in rnames.get(mtks.len()..).unwrap_or(&[]) {
        let list = midi_by_base.entry(strip_playlist_suffix(rn)).or_default();
        if list.len() <= MAX_ALTERNATES {
            list.push(rn);
        }
    }
    for (i, t) in mtks.iter().enumerate() {
        let act_nm = rnames.get(i).cloned().unwrap_or_else(|| t.name.clone());
        let alternates = midi_by_base
            .get(strip_playlist_suffix(&act_nm))
            .map(|v| v.iter().filter(|rn| ***rn != act_nm).take(MAX_ALTERNATES).map(|rn| (*rn).clone()).collect())
            .unwrap_or_default();
        out.insert(t.name.clone(), TrackPlaylists::Midi { active: act_nm, active_index: i, alternates });
    }
    out
}

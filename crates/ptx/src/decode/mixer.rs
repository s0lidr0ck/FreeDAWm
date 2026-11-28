// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/mixer.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: no indexing; at most `MAX_SENDS` send strips per channel and
// `MAX_STRIP_POINTS` automation points over all strips.
//! Mixer strips, automation lanes, panners, and sends (`0x2624`, `0x260D`, `0x260A`, `0x260C`, `0x260E`, `0x1029`).

use serde::{Deserialize, Serialize};

use crate::primitives::{i16_at, scan_lp_strings, u16_at, u32_at, u64_at};
use crate::tree::Block;

const MAX_POINTS: usize = 1_000_000;
/// At most this many send strips are read per channel.
pub const MAX_SENDS: usize = 64;
/// At most this many mixer automation points are kept over all strips (the rest are dropped).
pub const MAX_STRIP_POINTS: usize = 8_000_000;

fn strip_points(s: &Strip) -> usize {
    s.volume.len() + s.pan.len() + s.pan2.len() + s.lfe.len() + s.mute.len() + s.sends.as_ref().map_or(0, |v| v.iter().map(strip_points).sum())
}

/// Drops the lanes of strips past [`MAX_STRIP_POINTS`].
fn cap_points(strips: &mut [Strip]) {
    let mut total = 0usize;
    for s in strips {
        total = total.saturating_add(strip_points(s));
        if total > MAX_STRIP_POINTS {
            for l in [&mut s.volume, &mut s.pan, &mut s.pan2, &mut s.lfe, &mut s.mute] {
                l.clear();
                l.shrink_to_fit();
            }
            s.sends = Some(Vec::new());
        }
    }
}

/// Mixer strip (track main output strip or send strip).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Strip {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub output: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub stem_index: Option<u8>,
    pub volume: Vec<(u64, i16)>,
    pub pan: Vec<(u64, i16)>,
    pub pan2: Vec<(u64, i16)>,
    #[serde(serialize_with = "serialize_opt_neg_inf")]
    pub volume_db: Option<f64>,
    pub lfe: Vec<(u64, i16)>,
    #[serde(serialize_with = "serialize_opt_neg_inf")]
    pub lfe_db: Option<f64>,
    pub mute: Vec<(u64, i16)>,
    pub active: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub record_enable: Option<bool>,
    pub dest_path: Option<u16>,
    pub post_fader: Option<bool>,
    pub lane_time_base: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub channel: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub track: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub sends: Option<Vec<Strip>>,
}

fn serialize_opt_neg_inf<S: serde::Serializer>(val: &Option<f64>, ser: S) -> Result<S::Ok, S::Error> {
    match val {
        None => ser.serialize_none(),
        Some(v) if *v == f64::NEG_INFINITY => ser.serialize_f64(-144.0), // Keep finite JSON-safe sentinel if needed; callers inspect Option
        Some(v) => ser.serialize_f64(*v),
    }
}

/// Convert raw tenth-dB integer (`-1440` = `-inf`) to dB.
#[must_use]
pub fn tenths_to_db(tenths: i16) -> f64 {
    if tenths <= -1440 { f64::NEG_INFINITY } else { f64::from(tenths) / 10.0 }
}

/// Parse lane timebase string from `0x260A` header (`"ticks"` or `"samples"`).
#[must_use]
pub fn lane_time_base(d: &[u8], b: &Block) -> Option<String> {
    let s = b.data_start();
    let end = b.end().min(d.len());
    if d.get(s..s.checked_add(4)?)? != [0x01, 0x46, 0x01, 0x00] || s + 10 > end {
        return None;
    }
    let tb = u16_at(d, s + 8)?;
    Some(if tb == 1 { "ticks" } else { "samples" }.to_owned())
}

/// Parse a `0x260A` mixer lane (`[(pos, i16_val)]`).
#[must_use]
pub fn parse_lane(d: &[u8], b: &Block) -> Option<Vec<(u64, i16)>> {
    let s = b.data_start();
    let end = b.end().min(d.len());
    if d.get(s..s.checked_add(4)?)? != [0x01, 0x46, 0x01, 0x00] || s + 28 > end {
        return None;
    }
    let tb = u16_at(d, s + 8)?;
    let n = usize::try_from(u32_at(d, s + 10)?).ok()?;
    if n == 0 || n > MAX_POINTS {
        return None;
    }
    let need = s.checked_add(28)?.checked_add((n - 1).checked_mul(6)?)?;
    if need > end {
        return None;
    }
    let segs = u32_at(d, s + 16)?;
    let first = if tb == 0 && segs >= 65_536 { u64::from(u32_at(d, s + 22)?) } else { u64_at(d, s + 18)? };
    let mut pts = Vec::with_capacity(n);
    pts.push((first, i16_at(d, s + 26)?));
    let mut p = s + 28;
    for _ in 1..n {
        let pos = u64::from(u32_at(d, p)?);
        let val = i16_at(d, p + 4)?;
        pts.push((pos, val));
        p += 6;
    }
    Some(pts)
}

fn out_dest(d: &[u8], st: &Block) -> (Option<String>, Option<u8>) {
    let Some(o) = st.kids.iter().find(|k| k.content_type == 0x260E) else {
        return (None, None);
    };
    let end = o.end().min(d.len());
    let strs: Vec<(usize, String)> =
        scan_lp_strings(d, o.data_start(), end, 1, 200).into_iter().filter(|(_, s)| s.bytes().all(|b| (32..127).contains(&b))).collect();
    if let Some((at, name)) = strs.last() {
        let stem = if at + 4 + name.len() < end { d.get(at + 4 + name.len()).copied() } else { None };
        (Some(name.clone()), stem)
    } else {
        (Some(String::new()), None)
    }
}

/// Parse one `0x260D` strip (main strip or send).
#[must_use]
pub fn parse_strip(d: &[u8], st: &Block) -> Strip {
    let c0 = st.kids.iter().find(|k| k.content_type == 0x260C).map_or_else(|| st.end(), |k| k.pos);
    let lanes: Vec<&Block> = st.kids.iter().filter(|k| k.content_type == 0x260A && k.pos < c0).collect();
    let o = st.kids.iter().find(|k| k.content_type == 0x260E);
    let t = st.kids.iter().find(|k| k.content_type == 0x1029);

    let vol = lanes.first().and_then(|b| parse_lane(d, b));
    let mute = lanes.get(1).and_then(|b| parse_lane(d, b)).unwrap_or_default();
    let lfe = lanes.get(2).and_then(|b| parse_lane(d, b));
    let mut pcs = st.kids.iter().filter(|k| k.content_type == 0x260C);
    let pc = pcs.next();
    let pan_blk = pc.and_then(|p| p.kids.iter().find(|k| k.content_type == 0x260A));
    let pan = pan_blk.and_then(|b| parse_lane(d, b)).unwrap_or_default();
    let pc2 = pcs.next();
    let pan2_blk = pc2.and_then(|p| p.kids.iter().find(|k| k.content_type == 0x260A));
    let pan2 = pan2_blk.and_then(|b| parse_lane(d, b)).unwrap_or_default();

    let volume_db = vol.as_ref().and_then(|v| v.first()).map(|p| tenths_to_db(p.1));
    let lfe_db = lfe.as_ref().and_then(|v| v.first()).map(|p| tenths_to_db(p.1));
    let lane_tb = lanes.first().and_then(|b| lane_time_base(d, b));

    let active = o.and_then(|ob| d.get(ob.pos + 12)).map(|&b| b != 0);
    // 0x1029+5 (`tb.pos + 14`, `tm[1]`) is `mute_static`, not `record_enable`.
    let record_enable = t.filter(|tb| tb.size >= 10).map(|_| false);
    let dest_path = o.and_then(|ob| u32_at(d, ob.pos + 9)).map(|v| (v & 0xFFFF) as u16);
    let post_fader = t.and_then(|tb| d.get(tb.pos + 9)).map(|&b| b != 0);
    let (output, stem_index) = out_dest(d, st);

    Strip {
        output,
        stem_index,
        volume: vol.unwrap_or_default(),
        pan,
        pan2,
        volume_db,
        lfe: lfe.unwrap_or_default(),
        lfe_db,
        mute,
        active,
        record_enable,
        dest_path,
        post_fader,
        lane_time_base: lane_tb,
        channel: None,
        track: None,
        sends: None,
    }
}

/// Map channel block content type to channel name.
#[must_use]
pub fn channel_block_name(ct: u16) -> String {
    match ct {
        0x261C => "audio".to_owned(),
        0x261E => "aux".to_owned(),
        0x261D => "master".to_owned(),
        0x2621 => "instrument".to_owned(),
        0x2620 => "midi".to_owned(),
        0x263B => "folder".to_owned(),
        other => format!("{other:04X}"),
    }
}

/// Collect all blocks strictly below `n` in depth-first file order.
pub fn subtree<'a>(n: &'a Block, out: &mut Vec<&'a Block>) {
    for k in &n.kids {
        out.push(k);
        subtree(k, out);
    }
}

/// Parse all mixer strips from `0x2624` (or fallback flat `0x260D` scan for older sessions).
#[must_use]
pub fn parse_strips(d: &[u8], tree: &[Block]) -> Vec<Strip> {
    let mut out = parse_strips_uncapped(d, tree);
    cap_points(&mut out);
    out
}

fn parse_strips_uncapped(d: &[u8], tree: &[Block]) -> Vec<Strip> {
    let Some(top) = Block::find(tree, 0x2624) else {
        return Block::walk_all(tree)
            .into_iter()
            .filter(|st| st.content_type == 0x260D && st.kids.iter().any(|k| k.content_type == 0x260A))
            .map(|st| {
                let mut s = parse_strip(d, st);
                s.sends = Some(Vec::new());
                s
            })
            .collect();
    };
    let mut out = Vec::new();
    for (i, ch) in top.kids.iter().enumerate() {
        let mut all = Vec::new();
        subtree(ch, &mut all);
        let lanes: Vec<&Block> = all.into_iter().filter(|x| x.content_type == 0x260D && x.kids.iter().any(|k| k.content_type == 0x260A)).collect();
        if lanes.is_empty() {
            continue;
        }
        let main = lanes.iter().copied().find(|x| x.kids.iter().any(|k| k.content_type == 0x260E)).or_else(|| lanes.first().copied());
        let Some(main) = main else { continue };
        let mut s = parse_strip(d, main);
        s.channel = Some(channel_block_name(ch.content_type));
        s.track = Some(i);
        s.sends = Some(lanes.into_iter().filter(|x| x.pos != main.pos).take(MAX_SENDS).map(|x| parse_strip(d, x)).collect());
        out.push(s);
    }
    out
}

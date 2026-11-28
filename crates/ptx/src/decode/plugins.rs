// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/plugins.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: no indexing or slicing; plug-in automation positions saturate
// (`p0 + off` overflowed on a crafted lane); at most `MAX_INSERTS` inserts per channel.
//! Plug-in table (`0x1018`), settings files (`0x2027 > 0x2064`), channel inserts (`0x2627`), and AAX state decoders.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::decode::mixer::subtree;
use crate::primitives::{
    TRIM_KNOTS, be_f64_at, be_i32_at, be_u32_at, byte, find_bytes, i32_at, plain_ticks, round_n, scan_lp_strings, span, u16_at, u32_at, u64_at,
};

/// At most this many inserts are read per channel.
pub const MAX_INSERTS: usize = 64;
use crate::tree::Block;

/// Entry of the session plug-in table (`0x1018 > 0x1017`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginTableEntry {
    pub index: usize,
    pub name: Option<String>,
    pub vendor_id: String,
    pub kind: Option<serde_json::Value>,
    pub codes: Vec<String>,
}

/// Entry of the settings-file table (`0x2027 > 0x2064`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SettingsFile {
    pub file: String,
    pub factory_default: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub volume: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub folder: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub session_folder: Option<String>,
}

/// One decoded insert slot (`a`..`j`, or integer index for extended slots such as HEAT).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Insert {
    pub slot: serde_json::Value,
    pub multi_mono: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ids: Option<[String; 3]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub state_format: Option<String>,
    pub name: String,
    pub active: Option<bool>,
    #[serde(default)]
    pub params: BTreeMap<String, serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<Vec<f64>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values_i32: Option<Vec<i32>>,
    #[serde(default)]
    pub automation: Vec<Vec<(u64, f64)>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub automation_params: Option<Vec<[Option<String>; 2]>>,
    pub bypass: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub table_index: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_index: Option<u16>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub plugin: Option<String>,
    #[serde(default)]
    pub vendor_id: String,
    #[serde(default)]
    pub stock: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub settings_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
}

const SLOTS: [char; 10] = ['a', 'b', 'c', 'd', 'e', 'f', 'g', 'h', 'i', 'j'];

/// Big-endian signed `i32` normalized to `[0.0 .. 1.0]` (`0` -> `0.5`).
#[must_use]
pub fn be_norm(val: i32) -> f64 {
    round_n((f64::from(val) / 4_294_967_296.0 + 0.5).clamp(0.0, 1.0), 6)
}

/// Empirical Pro Tools Trim gain curve (`norm [0..1]` -> `dB`, `<= 0.06 dB` error).
#[must_use]
pub fn trim_norm_to_db(norm: f64, range_db: f64) -> f64 {
    if norm <= 0.0 {
        return -144.0;
    }
    if norm >= 0.755 {
        return round_n(((norm - 0.755) / (1.0 - 0.755)) * range_db, 2);
    }
    for w in TRIM_KNOTS.windows(2) {
        let [(x0, y0), (x1, y1)] = [w.first().copied().unwrap_or((0.0, 0.0)), w.get(1).copied().unwrap_or((1.0, 0.0))];
        if norm <= x1 {
            let t = (norm - x0) / (x1 - x0);
            return round_n(y0 + t * (y1 - y0), 2);
        }
    }
    0.0
}

fn is_pname_char(b: u8) -> bool {
    b.is_ascii_alphanumeric() || matches!(b, b' ' | b'/' | b'(' | b')' | b'.' | b'+' | b'#' | b'-')
}

/// Parse `"Complete Controls State"` named parameter dictionary (`d_` -> BE `f64`, `l_` -> BE `i32`).
#[must_use]
pub fn parse_elck_params(b: &[u8]) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    let Some(s) = find_bytes(b, b"Complete Controls State\0", 0) else {
        return out;
    };
    let mut pos = s + 24;
    // Scan forward for the first `[dl]_[A-Za-z0-9 /().+#-]{1,40}\0`
    while pos + 3 < b.len() {
        if (byte(b, pos) == b'd' || byte(b, pos) == b'l')
            && byte(b, pos + 1) == b'_'
            && let Some((nm, end_with_nul)) = match_pname(b, pos)
        {
            let mut cur = (nm, end_with_nul);
            loop {
                let (name, nul_end) = cur;
                let pad = (4 - ((name.len() + 1) % 4)) % 4;
                let j = nul_end + pad;
                let is_f64 = name.starts_with(b"d");
                let w = if is_f64 { 8 } else { 4 };
                if j + w > b.len() {
                    break;
                }
                let key = String::from_utf8_lossy(name).into_owned();
                let val = if is_f64 {
                    be_f64_at(b, j).and_then(serde_json::Number::from_f64).map(serde_json::Value::Number)
                } else {
                    be_i32_at(b, j).map(serde_json::Value::from)
                };
                if let Some(v) = val {
                    out.insert(key, v);
                }
                let next_pos = j + w;
                if let Some(next_match) = match_pname(b, next_pos) {
                    cur = next_match;
                } else {
                    break;
                }
            }
            return out;
        }
        pos += 1;
    }
    out
}

fn match_pname(b: &[u8], at: usize) -> Option<(&[u8], usize)> {
    if at + 3 >= b.len() {
        return None;
    }
    if (byte(b, at) != b'd' && byte(b, at) != b'l') || byte(b, at + 1) != b'_' {
        return None;
    }
    let max_end = (at + 42).min(b.len());
    for i in (at + 2)..max_end {
        if byte(b, i) == 0 {
            let body_len = i - (at + 2);
            if (1..=40).contains(&body_len) {
                return Some((span(b, at, i), i + 1));
            }
            return None;
        }
        if !is_pname_char(byte(b, i)) {
            return None;
        }
    }
    None
}

fn insert_f64(map: &mut BTreeMap<String, serde_json::Value>, k: &str, v: f64) {
    if let Some(num) = serde_json::Number::from_f64(v) {
        map.insert(k.to_owned(), serde_json::Value::Number(num));
    }
}

fn decode_md3(b: &[u8]) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    // `b` begins at the 56-byte big-endian AAX chunk header; payload starts at +56.
    let body = b.get(56..).unwrap_or(&[]);
    if body.len() < 120 {
        return out;
    }
    // +0..56: 14 big-endian normalized i32 parameters, followed by 2 pad bytes, tempo, meter, note, invert, and groove.
    let pairs = [
        ("output_left", 0),
        ("output_right", 4),
        ("rate_left", 8),
        ("rate_right", 12),
        ("lpf_left", 16),
        ("lpf_right", 20),
        ("delay_left", 24),
        ("delay_right", 28),
        ("mix_left", 32),
        ("mix_right", 36),
        ("depth_left", 40),
        ("depth_right", 44),
        ("feedback_left", 48),
        ("feedback_right", 52),
    ];
    for (name, off) in pairs {
        if let Some(iv) = be_i32_at(body, off) {
            insert_f64(&mut out, name, be_norm(iv));
        }
    }
    if let Some(tv) = be_i32_at(body, 58) {
        let tempo_bpm = round_n(5.0 + be_norm(tv) * 495.0, 3);
        insert_f64(&mut out, "tempo_bpm", tempo_bpm);
    }
    if let Some(nv) = be_i32_at(body, 62) {
        let m_num = (be_norm(nv) * 31.0 + 1.0).round() as i64;
        out.insert("meter_num".to_owned(), serde_json::Value::from(m_num));
    }
    if let Some(dv) = be_i32_at(body, 66) {
        let m_den = (be_norm(dv) * 31.0 + 1.0).round() as i64;
        out.insert("meter_den".to_owned(), serde_json::Value::from(m_den));
    }
    if let Some(nl) = be_i32_at(body, 70) {
        insert_f64(&mut out, "note_left_norm", be_norm(nl));
    }
    if let Some(nr) = be_i32_at(body, 74) {
        insert_f64(&mut out, "note_right_norm", be_norm(nr));
    }
    if let (Some(&il), Some(&ir)) = (body.get(78), body.get(79)) {
        out.insert("invert_left".to_owned(), serde_json::Value::from(il != 0));
        out.insert("invert_right".to_owned(), serde_json::Value::from(ir != 0));
    }
    if let Some(gl) = be_i32_at(body, 84) {
        insert_f64(&mut out, "groove_left", be_norm(gl));
    }
    if let Some(gr) = be_i32_at(body, 88) {
        insert_f64(&mut out, "groove_right", be_norm(gr));
    }
    out
}

fn decode_piii(b: &[u8]) -> BTreeMap<String, serde_json::Value> {
    let mut out = BTreeMap::new();
    // Payload starts after the 56-byte BE header: version (+0), L mix/lpf (+64/+80), R mix/lpf (+108/+124).
    let body = b.get(56..).unwrap_or(&[]);
    if body.len() < 132 {
        return out;
    }
    if let Some(ver) = be_u32_at(body, 0) {
        out.insert("version".to_owned(), serde_json::Value::from(ver));
    }
    if let Some(ml) = be_f64_at(body, 64) {
        insert_f64(&mut out, "mix_left", round_n(ml, 3));
    }
    if let Some(ll) = be_f64_at(body, 80) {
        insert_f64(&mut out, "lpf_left", round_n(ll, 1));
    }
    if let Some(mr) = be_f64_at(body, 108) {
        insert_f64(&mut out, "mix_right", round_n(mr, 3));
    }
    if let Some(lr) = be_f64_at(body, 124) {
        insert_f64(&mut out, "lpf_right", round_n(lr, 1));
    }
    out
}

fn decode_all_params(pid2: &str, body: &[u8]) -> (BTreeMap<String, serde_json::Value>, Vec<f64>, Vec<i32>) {
    let mut out = BTreeMap::new();
    let cnt = body.len() / 4;
    let mut vi = Vec::with_capacity(cnt);
    let mut vs = Vec::with_capacity(cnt);
    for idx in 0..cnt {
        if let Some(v) = be_i32_at(body, idx * 4) {
            vi.push(v);
            vs.push(be_norm(v));
        }
    }
    let vs_at = |i: usize| vs.get(i).copied().unwrap_or(0.5);
    let vi_at = |i: usize| vi.get(i).copied().unwrap_or(0);
    match pid2 {
        "Trim" if vi.len() >= 5 => {
            let gn = vs_at(0);
            let rng = if vi_at(3) > 0 { 12.0 } else { 6.0 };
            insert_f64(&mut out, "gain_norm", gn);
            insert_f64(&mut out, "gain_db", trim_norm_to_db(gn, rng));
            out.insert("phase_invert".to_owned(), serde_json::Value::from(vi_at(1) > 0));
            out.insert("gain_range_db".to_owned(), serde_json::Value::from(if vi_at(3) > 0 { 12 } else { 6 }));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(4) > 0));
        }
        "PIGD" if vi.len() >= 9 => {
            out.insert("delay_samples".to_owned(), serde_json::Value::from(vi_at(6)));
            out.insert("delay_samples_r".to_owned(), serde_json::Value::from(vi_at(7)));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(8) > 0));
        }
        "SciF" if vi.len() >= 8 => {
            insert_f64(&mut out, "input_level", vs_at(0));
            insert_f64(&mut out, "effect_type", vs_at(1));
            insert_f64(&mut out, "effect_frequency", vs_at(2));
            insert_f64(&mut out, "effect_depth", vs_at(3));
            insert_f64(&mut out, "slot_4", vs_at(4));
            insert_f64(&mut out, "mod_depth", vs_at(5));
            insert_f64(&mut out, "mod_rate_slew_thresh", vs_at(6));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(7) > 0));
        }
        "LoFi" if vi.len() >= 9 => {
            insert_f64(&mut out, "sample_rate_norm", vs_at(0));
            insert_f64(&mut out, "sample_size_norm", vs_at(1));
            insert_f64(&mut out, "anti_alias_norm", vs_at(2));
            insert_f64(&mut out, "noise_norm", vs_at(3));
            insert_f64(&mut out, "distortion", vs_at(6));
            insert_f64(&mut out, "saturation", vs_at(7));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(8) > 0));
        }
        "1176" if vi.len() >= 7 => {
            insert_f64(&mut out, "input", vs_at(0));
            insert_f64(&mut out, "output", vs_at(1));
            insert_f64(&mut out, "attack", vs_at(2));
            insert_f64(&mut out, "release", vs_at(3));
            insert_f64(&mut out, "ratio", vs_at(4));
            insert_f64(&mut out, "meter_mode", vs_at(5));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(6) > 0));
        }
        "Rvrb" if vi.len() >= 11 => {
            insert_f64(&mut out, "algorithm_norm", vs_at(0));
            insert_f64(&mut out, "size_norm", vs_at(1));
            insert_f64(&mut out, "decay_norm", vs_at(2));
            insert_f64(&mut out, "mix", vs_at(3));
            insert_f64(&mut out, "diffusion", vs_at(4));
            insert_f64(&mut out, "pre_delay_norm", vs_at(8));
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(10) > 0));
        }
        "PSA1" if vi.len() >= 10 => {
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(9) > 0));
        }
        "Dthr" if vi.len() >= 4 => {
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(3) > 0));
        }
        "POWr" if vi.len() >= 3 => {
            out.insert("master_bypass".to_owned(), serde_json::Value::from(vi_at(2) > 0));
        }
        _ => {}
    }
    (out, vs, vi)
}

/// Parse plug-in automation lane `0x260B` (`[(pos, norm)]`) using continuous fixed-point scaling.
#[must_use]
pub fn parse_plugin_lane(d: &[u8], n: &Block) -> Vec<(u64, f64)> {
    parse_plugin_lane_with_steps(d, n, None)
}

/// Parse plug-in automation lane `0x260B` (`[(pos, norm)]`), handling discrete integer lanes when `discrete_steps` is `Some(n >= 2)`.
#[must_use]
pub fn parse_plugin_lane_with_steps(d: &[u8], n: &Block, discrete_steps: Option<u8>) -> Vec<(u64, f64)> {
    let s = n.data_start();
    let e = n.end().min(d.len());
    if d.get(s..s.saturating_add(4)) != Some(&[0x01, 0x46, 0x01, 0x00]) || s + 30 > e {
        return Vec::new();
    }
    let Some(tb) = u16_at(d, s + 8) else { return Vec::new() };
    let Some(cnt) = u32_at(d, s + 10).and_then(|v| usize::try_from(v).ok()) else {
        return Vec::new();
    };
    if !(1..=1_000_000).contains(&cnt) || s + 30 + 8 * (cnt - 1) > e {
        return Vec::new();
    }
    let Some(mut p0) = u64_at(d, s + 18) else { return Vec::new() };
    if tb == 1 {
        p0 = plain_ticks(p0);
    }
    let decode_val = |vi: i32| -> f64 {
        match discrete_steps {
            Some(steps) if steps <= 2 => {
                if vi != 0 {
                    1.0
                } else {
                    0.0
                }
            }
            Some(steps) => {
                let denom = f64::from(steps.saturating_sub(1).max(1));
                round_n((f64::from(vi) / denom).clamp(0.0, 1.0), 6)
            }
            None => round_n(f64::from(vi) / 2_147_483_648.0 + 0.5, 6),
        }
    };
    let Some(v0) = i32_at(d, s + 26) else { return Vec::new() };
    let mut pts = Vec::with_capacity(cnt);
    pts.push((p0, decode_val(v0)));
    let mut j = s + 30;
    for _ in 1..cnt {
        let (Some(off), Some(vi)) = (u32_at(d, j), i32_at(d, j + 4)) else {
            break;
        };
        pts.push((p0.saturating_add(u64::from(off)), decode_val(vi)));
        j += 8;
    }
    pts
}

/// Extract discrete step count from a `0x260F` entry when its metadata marks a stepped/boolean parameter (`is_continuous == 0`).
#[must_use]
pub fn plugin_lane_discrete_steps(d: &[u8], n: &Block) -> Option<u8> {
    let tail_start = n.kids.last().map_or(n.pos + 10, Block::end);
    let end = n.end().min(d.len());
    if tail_start + 4 > end {
        return None;
    }
    let name_len = usize::try_from(u32_at(d, tail_start)?).ok()?;
    let meta_pos = tail_start.checked_add(4)?.checked_add(name_len)?.checked_add(4)?;
    let meta = d.get(meta_pos..meta_pos.checked_add(4)?)?;
    if byte(meta, 1) == 0 && byte(meta, 3) >= 2 { Some(byte(meta, 3)) } else { None }
}

/// Parse parameter name & id from `0x260F` plug-in automation entry.
#[must_use]
pub fn plugin_lane_names(d: &[u8], n: &Block) -> [Option<String>; 2] {
    let tail_start = n.kids.last().map_or(n.pos + 10, Block::end);
    let end = n.end().min(d.len());
    if tail_start >= end {
        return [None, None];
    }
    let s = scan_lp_strings(d, tail_start, end, 1, 200);
    if s.is_empty() { [None, None] } else { [s.first().map(|x| x.1.clone()), s.last().map(|x| x.1.clone())] }
}

/// Parse inserts (`0x2627 > 0x2616 | 0x2617`) for one channel block.
#[must_use]
pub fn parse_inserts(d: &[u8], ch: &Block) -> Vec<Insert> {
    let mut all = Vec::new();
    subtree(ch, &mut all);
    let Some(list) = all.into_iter().find(|x| x.content_type == 0x2627) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (k, sl) in list.kids.iter().enumerate().take(MAX_INSERTS) {
        if sl.content_type != 0x2616 && sl.content_type != 0x2617 {
            continue;
        }
        let multi = sl.content_type == 0x2617;
        let sl_inner = if multi { sl.kids.first().unwrap_or(sl) } else { sl };
        let mut sub = Vec::new();
        subtree(sl_inner, &mut sub);
        let st = sub.iter().copied().find(|x| x.content_type == 0x1038);
        let rec = sub.iter().copied().find(|x| x.content_type == 0x2613);
        let inst = sl_inner.kids.iter().find(|x| x.content_type == 0x2615);
        let auto = sl_inner.kids.iter().find(|x| x.content_type == 0x2614);

        let table_index = inst.and_then(|ib| ib.kids.iter().find(|x| x.content_type == 0x2626)).and_then(|b26| {
            let b26_end = b26.end();
            if b26_end + 2 <= inst?.end().min(d.len()) { u16_at(d, b26_end) } else { None }
        });
        let settings_index = rec.and_then(|rb| if rb.size >= 4 { u16_at(d, rb.data_start()) } else { None });

        let active = rec
            .and_then(|rb| rb.kids.last().map(Block::end))
            .filter(|&e| rec.is_some_and(|rb| e < rb.end().min(d.len())))
            .and_then(|e| d.get(e))
            .map(|&b| b != 0);

        let bypass = inst.map(Block::end).filter(|&e| e < sl_inner.end().min(d.len())).and_then(|e| d.get(e)).map(|&b| b != 0);

        let mut ids = None;
        let mut state_format = None;
        let mut state_name = String::new();
        let mut params = BTreeMap::new();
        let mut values = None;
        let mut values_i32 = None;

        if let Some(st_blk) = st {
            let st_end = st_blk.end().min(d.len());
            if let Some(raw_st) = d.get(st_blk.data_start()..st_end)
                && raw_st.len() >= 12
            {
                // Skip LE wrapper (u32 1, u32 len) when present
                let b = if raw_st.len() > 8 && raw_st.get(..4) == Some(&[1, 0, 0, 0]) { raw_st.get(8..).unwrap_or(&[]) } else { raw_st };
                if b.len() >= 24 {
                    let p0 = String::from_utf8_lossy(span(b, 8, 12)).into_owned();
                    let p1 = String::from_utf8_lossy(span(b, 12, 16)).into_owned();
                    let p2 = String::from_utf8_lossy(span(b, 16, 20)).into_owned();
                    let fmt_raw = String::from_utf8_lossy(span(b, 20, 24)).into_owned();
                    if p0.bytes().all(|c| (0x20..=0x7e).contains(&c))
                        && p1.bytes().all(|c| (0x20..=0x7e).contains(&c))
                        && p2.bytes().all(|c| (0x20..=0x7e).contains(&c))
                    {
                        ids = Some([p0, p1.clone(), p2]);
                    }
                    let fmt_trim = fmt_raw.trim_end_matches(' ');
                    if !fmt_trim.is_empty() && fmt_raw.bytes().all(|c| (0x20..=0x7e).contains(&c)) {
                        state_format = Some(fmt_raw.clone());
                    }
                    if b.len() >= 56 {
                        let cname_bytes = span(b, 24, 56);
                        state_name = String::from_utf8_lossy(cname_bytes.split(|&x| x == 0).next().unwrap_or(&[])).into_owned();
                    }
                    if fmt_raw == "elck" {
                        params = parse_elck_params(b);
                    } else if fmt_raw == "All " && b.len() >= 56 {
                        let pid2 = ids.as_ref().map_or("", |[_, a1, _]| a1.as_str());
                        let (pm, vs, vi) = decode_all_params(pid2, b.get(56..).unwrap_or(&[]));
                        params = pm;
                        values = Some(vs);
                        values_i32 = Some(vi);
                    } else if fmt_raw == "MD_3" && b.len() >= 176 {
                        params = decode_md3(b);
                    } else if fmt_raw == "PiII" && b.len() >= 188 {
                        params = decode_piii(b);
                    }
                }
            }
        }

        let mut automation = Vec::new();
        let mut automation_params = None;
        if let Some(au_blk) = auto {
            let mut ap = Vec::new();
            for entry in au_blk.kids.iter().filter(|x| x.content_type == 0x260F) {
                let steps = plugin_lane_discrete_steps(d, entry);
                let mut entry_sub = Vec::new();
                subtree(entry, &mut entry_sub);
                for x in entry_sub {
                    if x.content_type == 0x260B {
                        automation.push(parse_plugin_lane_with_steps(d, x, steps));
                    }
                }
                ap.push(plugin_lane_names(d, entry));
            }
            automation_params = Some(ap);
        }

        out.push(Insert {
            slot: SLOTS.get(k).map_or_else(|| serde_json::Value::from(k), |c| serde_json::Value::from(c.to_string())),
            multi_mono: multi,
            ids,
            state_format,
            name: state_name,
            active,
            params,
            values,
            values_i32,
            automation,
            automation_params,
            bypass,
            table_index,
            settings_index,
            plugin: None,
            vendor_id: String::new(),
            stock: false,
            settings_file: None,
            preset: None,
        });
    }
    out
}

/// Parse structured `0x2064` preset locator.
fn parse_2064(b: &[u8]) -> Option<SettingsFile> {
    if b.len() < 13 || b.get(..2) != Some(b"\x5a\x02") {
        return None;
    }
    let mut p = 9;
    let vl = usize::try_from(u32_at(b, p)?).ok()?;
    p += 4;
    if p + vl + 4 > b.len() {
        return None;
    }
    let vol = String::from_utf8_lossy(span(b, p, p + vl)).into_owned();
    p += vl;
    let fl = usize::try_from(u32_at(b, p)?).ok()?;
    p += 4;
    if p + fl + 14 > b.len() {
        return None;
    }
    let fn_str = String::from_utf8_lossy(span(b, p, p + fl)).into_owned();
    p += fl + 10;
    let depth = usize::try_from(u32_at(b, p)?).ok()?;
    p += 4;
    let mut parts = Vec::new();
    for _ in 0..depth.min(64) {
        if p + 4 > b.len() {
            return None;
        }
        let pl = usize::try_from(u32_at(b, p)?).ok()?;
        p += 4;
        if p + pl > b.len() {
            return None;
        }
        parts.push(String::from_utf8_lossy(span(b, p, p + pl)).into_owned());
        p += pl;
    }
    let folder = parts.split_last().map(|(_, dirs)| dirs.join("/")).unwrap_or_default();
    let mut session_folder = None;
    if p + 20 < b.len() && b.get(p + 5..p + 7) == Some(b"\x5a\x01") {
        let mut q = p + 5 + 12;
        if q + 4 <= b.len()
            && let Some(d2) = u32_at(b, q).and_then(|v| usize::try_from(v).ok())
        {
            q += 4;
            let mut sparts = Vec::new();
            for _ in 0..d2.min(64) {
                if q + 4 > b.len() {
                    break;
                }
                let Some(pl) = u32_at(b, q).and_then(|v| usize::try_from(v).ok()) else {
                    break;
                };
                q += 4;
                if q + pl > b.len() {
                    break;
                }
                sparts.push(String::from_utf8_lossy(span(b, q, q + pl)).into_owned());
                q += pl;
            }
            if !sparts.is_empty() {
                session_folder = Some(sparts.join("/"));
            }
        }
    }
    let factory_default = fn_str.ends_with("Fact.tfx");
    Some(SettingsFile { file: fn_str, factory_default, volume: Some(vol), folder: Some(folder), session_folder })
}

/// Parse settings files (`0x2027 > 0x2064`).
#[must_use]
pub fn parse_settings_files(d: &[u8], tree: &[Block]) -> Vec<SettingsFile> {
    let Some(t) = Block::find(tree, 0x2027) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for k in &t.kids {
        if k.content_type != 0x2064 {
            continue;
        }
        let Some(b) = d.get(k.data_start()..k.end().min(d.len())) else {
            continue;
        };
        if let Some(parsed) = parse_2064(b) {
            out.push(parsed);
        } else {
            let mut names = Vec::new();
            let mut i = 0;
            while i < b.len() {
                if (0x20..=0x7e).contains(&byte(b, i)) {
                    let s = i;
                    while i < b.len() && (0x20..=0x7e).contains(&byte(b, i)) {
                        i += 1;
                    }
                    if i - s >= 3 {
                        names.push(String::from_utf8_lossy(span(b, s, i)).into_owned());
                    }
                } else {
                    i += 1;
                }
            }
            let fn_str = names.iter().find(|n| n.to_ascii_lowercase().ends_with(".tfx")).or_else(|| names.last()).cloned().unwrap_or_default();
            let factory_default = fn_str.ends_with("Fact.tfx");
            out.push(SettingsFile { file: fn_str, factory_default, volume: None, folder: None, session_folder: None });
        }
    }
    out
}

/// Parse session plug-in table (`0x1018 > 0x1017`).
#[must_use]
pub fn parse_plugin_table(d: &[u8], tree: &[Block]) -> Vec<PluginTableEntry> {
    let Some(t) = Block::find(tree, 0x1018) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (i, k) in t.kids.iter().filter(|x| x.content_type == 0x1017).enumerate() {
        let Some(b) = d.get(k.data_start()..k.end().min(d.len())) else {
            out.push(PluginTableEntry { index: i, name: None, vendor_id: String::new(), kind: None, codes: Vec::new() });
            continue;
        };
        let parsed = (|| -> Option<PluginTableEntry> {
            let ln = usize::try_from(u32_at(b, 1)?).ok()?;
            let name = String::from_utf8_lossy(b.get(5..5 + ln)?).into_owned();
            let codes_raw = b.get(5 + ln..17 + ln)?;
            let mut vid = String::new();
            if let Some(q) = find_bytes(b, b"com.", 17 + ln)
                && q >= 4
                && let Some(vl) = u32_at(b, q - 4).and_then(|v| usize::try_from(v).ok())
                && vl > 0
                && vl < 200
                && let Some(vslice) = b.get(q..q + vl)
            {
                vid = String::from_utf8_lossy(vslice).into_owned();
            }
            let kind = match *b.first()? {
                3 => serde_json::Value::from("effect"),
                4 => serde_json::Value::from("instrument"),
                other => serde_json::Value::from(other),
            };
            let codes = [0usize, 4, 8]
                .iter()
                .map(|&j| {
                    let rev: Vec<u8> = span(codes_raw, j, j + 4).iter().copied().rev().collect();
                    String::from_utf8_lossy(&rev).into_owned()
                })
                .collect();
            Some(PluginTableEntry { index: i, name: Some(name), vendor_id: vid, kind: Some(kind), codes })
        })();
        out.push(parsed.unwrap_or(PluginTableEntry { index: i, name: None, vendor_id: String::new(), kind: None, codes: Vec::new() }));
    }
    out
}

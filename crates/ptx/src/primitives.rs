// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/primitives.rs`. MIT licence, Copyright (c) 2026 John Forward
// (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: the write side (`lp_bytes`, `unix_ns_to_filetime`) and the unused
// `seconds_to_ticks` and timecode-string helpers removed; `ticks_to_seconds` no longer indexes;
// `find_bytes` refuses an empty needle (`windows(0)` panics); `scan_lp_strings` returns at most
// `MAX_SCANNED_STRINGS`.
//! Low-level bounds-checked binary primitives and Pro Tools timebase math.

/// Ticks per quarter note in Pro Tools sessions.
pub const TPQ: f64 = 960_000.0;

/// Integer ticks per quarter note.
pub const TPQ_U64: u64 = 960_000;

/// Stored offset added to tick-based positions/timestamps (`10^12`).
pub const OFFSET_FORM: u64 = 1_000_000_000_000;

/// Remove `10^12` offset from tick-based values when present.
#[must_use]
pub const fn plain_ticks(v: u64) -> u64 {
    if v >= OFFSET_FORM { v - OFFSET_FORM } else { v }
}

/// Read fixed-size byte array safely.
#[must_use]
pub fn bytes_at<const N: usize>(d: &[u8], at: usize) -> Option<[u8; N]> {
    let end = at.checked_add(N)?;
    d.get(at..end)?.try_into().ok()
}

/// The byte at `at`, or 0 past the end (FreeDAWm: replaces `d[at]` where the bounds are already
/// checked, so indexing can never panic).
#[must_use]
pub fn byte(d: &[u8], at: usize) -> u8 {
    d.get(at).copied().unwrap_or(0)
}

/// The bytes `d[from..to]`, or none when that range is not inside `d` (FreeDAWm: replaces
/// slicing).
#[must_use]
pub fn span(d: &[u8], from: usize, to: usize) -> &[u8] {
    d.get(from..to).unwrap_or(&[])
}

/// Read little-endian `u16`.
#[must_use]
pub fn u16_at(d: &[u8], at: usize) -> Option<u16> {
    bytes_at::<2>(d, at).map(u16::from_le_bytes)
}

/// Read little-endian `i16`.
#[must_use]
pub fn i16_at(d: &[u8], at: usize) -> Option<i16> {
    bytes_at::<2>(d, at).map(i16::from_le_bytes)
}

/// Read little-endian `u32`.
#[must_use]
pub fn u32_at(d: &[u8], at: usize) -> Option<u32> {
    bytes_at::<4>(d, at).map(u32::from_le_bytes)
}

/// Read little-endian `i32`.
#[must_use]
pub fn i32_at(d: &[u8], at: usize) -> Option<i32> {
    bytes_at::<4>(d, at).map(i32::from_le_bytes)
}

/// Read big-endian `i32`.
#[must_use]
pub fn be_i32_at(d: &[u8], at: usize) -> Option<i32> {
    bytes_at::<4>(d, at).map(i32::from_be_bytes)
}

/// Read big-endian `u32`.
#[must_use]
pub fn be_u32_at(d: &[u8], at: usize) -> Option<u32> {
    bytes_at::<4>(d, at).map(u32::from_be_bytes)
}

/// Read little-endian `u64`.
#[must_use]
pub fn u64_at(d: &[u8], at: usize) -> Option<u64> {
    bytes_at::<8>(d, at).map(u64::from_le_bytes)
}

/// Read little-endian `f32`.
#[must_use]
pub fn f32_at(d: &[u8], at: usize) -> Option<f32> {
    bytes_at::<4>(d, at).map(f32::from_le_bytes)
}

/// Read little-endian `f64`.
#[must_use]
pub fn f64_at(d: &[u8], at: usize) -> Option<f64> {
    bytes_at::<8>(d, at).map(f64::from_le_bytes)
}

/// Read big-endian `f64`.
#[must_use]
pub fn be_f64_at(d: &[u8], at: usize) -> Option<f64> {
    bytes_at::<8>(d, at).map(f64::from_be_bytes)
}

/// Read little-endian unsigned integer of `1..=8` bytes.
#[must_use]
pub fn le_u64(d: &[u8], at: usize, len: usize) -> Option<u64> {
    if len == 0 || len > 8 {
        return None;
    }
    let slice = d.get(at..at.checked_add(len)?)?;
    Some(slice.iter().rev().fold(0u64, |acc, &b| (acc << 8) | u64::from(b)))
}

/// Read little-endian signed integer of `1..=8` bytes (sign-extended).
#[must_use]
pub fn le_i64(d: &[u8], at: usize, len: usize) -> Option<i64> {
    let raw = le_u64(d, at, len)?;
    let bits = len * 8;
    if bits == 64 {
        Some(raw as i64)
    } else {
        let sign = 1u64 << (bits - 1);
        if raw >= sign { Some((raw as i64) - ((1u64 << bits) as i64)) } else { Some(raw as i64) }
    }
}

/// Read a `u32`-length-prefixed UTF-8 string (`<= max_len` bytes), returning `(string, next_offset)`.
#[must_use]
pub fn lp_str(d: &[u8], at: usize, max_len: usize) -> Option<(String, usize)> {
    let len = usize::try_from(u32_at(d, at)?).ok()?;
    if len > max_len {
        return None;
    }
    let start = at.checked_add(4)?;
    let end = start.checked_add(len)?;
    let s = String::from_utf8_lossy(d.get(start..end)?).into_owned();
    Some((s, end))
}

/// At most this many strings are returned by one [`scan_lp_strings`].
pub const MAX_SCANNED_STRINGS: usize = 65_536;

/// Scan a slice `[start..end)` for valid length-prefixed printable ASCII/UTF-8 strings (mirrors `ptx_probe.lp_strings`).
#[must_use]
pub fn scan_lp_strings(d: &[u8], start: usize, end: usize, min_len: usize, max_len: usize) -> Vec<(usize, String)> {
    let end = end.min(d.len());
    let mut out = Vec::new();
    let mut p = start;
    while p + 4 <= end && out.len() < MAX_SCANNED_STRINGS {
        if let Some(n) = u32_at(d, p).and_then(|v| usize::try_from(v).ok())
            && n >= min_len
            && n <= max_len
            && p + 4 + n <= end
            && let Some(bytes) = d.get(p + 4..p + 4 + n)
            && let Ok(s) = core::str::from_utf8(bytes)
            && s.chars().all(|c| !c.is_control())
        {
            out.push((p, s.to_owned()));
            p += 4 + n;
            continue;
        }
        p += 1;
    }
    out
}

/// Convert tick position to seconds using a sorted constant-tempo map (`[(ticks, bpm)]`).
#[must_use]
pub fn ticks_to_seconds(ticks: f64, tempo_map: &[(u64, f64)]) -> f64 {
    let Some(&(_, first_bpm)) = tempo_map.first() else {
        return ticks / TPQ * (60.0 / 120.0);
    };
    let mut sec = 0.0_f64;
    let mut prev_tick = 0.0_f64;
    let mut prev_bpm = first_bpm.max(1e-6);
    for &(ev_tick, ev_bpm) in tempo_map {
        let t = ev_tick as f64;
        if ticks <= t {
            return sec + (ticks - prev_tick) / TPQ * (60.0 / prev_bpm);
        }
        sec += (t - prev_tick) / TPQ * (60.0 / prev_bpm);
        prev_tick = t;
        prev_bpm = ev_bpm.max(1e-6);
    }
    sec + (ticks - prev_tick) / TPQ * (60.0 / prev_bpm)
}

/// Round floating-point value to `decimals` decimal places.
#[must_use]
pub fn round_n(val: f64, decimals: i32) -> f64 {
    let scale = 10_f64.powi(decimals);
    (val * scale).round() / scale
}

/// Search `haystack[from..]` for the first occurrence of `needle`.
#[must_use]
pub fn find_bytes(haystack: &[u8], needle: &[u8], from: usize) -> Option<usize> {
    if needle.is_empty() {
        return None;
    }
    haystack.get(from..)?.windows(needle.len()).position(|w| w == needle).map(|off| from + off)
}

/// Verified piecewise-linear Pro Tools Trim gain taper knots `(normalized_u, db)` up to `0 dB`.
pub const TRIM_KNOTS: [(f64, f64); 6] = [(0.0, -144.0), (0.10, -53.10), (0.25, -23.40), (0.40, -14.60), (0.50, -10.40), (0.755, 0.0)];

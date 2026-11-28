// Vendored from ptx-session-kit (https://github.com/knobmob/ptx-session-kit), commit 294c59a,
// `scraft-bridge/crates/ptx-format/src/decode/midi.rs`. MIT licence, Copyright (c) 2026 John
// Forward (jbl>>FF) / knobmob prod. (crates/ptx/LICENSE-MIT-ptx-session-kit).
// Changed for FreeDAWm: bounded (`MAX_MIDI_*`: notes, points, SysEx bytes and records looked at
// for the whole session); the `Mdss`/`MdCD` records inside each controller record found from
// lists made in one pass, where the upstream searched the chunk again for each (quadratic on a
// crafted file); the MPE loop always moves forward; no indexing or slicing.
//! MIDI data block (`0x2000`: `MdChun`, `MdNLB`, `MdCDS`, `MdES` / `MdTEL`) decoding.

use serde::{Deserialize, Serialize};

use crate::primitives::{byte, find_bytes, i32_at, le_u64, plain_ticks, span, u32_at};
use crate::tree::Block;

/// MIDI program-change decoded fields.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProgramBank {
    pub ticks: u64,
    pub program: u32,
    pub bank_msb: u32,
    pub bank_lsb: u32,
}

/// MIDI controller / pitch-bend / pressure / program lane (`MdCDS`).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MidiController {
    pub kind: serde_json::Value,
    pub number: u8,
    pub points: Vec<[i64; 2]>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub program_banks: Option<Vec<ProgramBank>>,
}

/// Per-note MPE curve inside an MPE `MdCDS` container.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct MpeCurve {
    pub kind: serde_json::Value,
    pub number: u8,
    pub mpe_channel: u8,
    pub key: u8,
    pub velocity: u8,
    pub note_start: u64,
    pub points: Vec<[i64; 2]>,
}

/// One `MdChun` chunk inside `0x2000`.
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct MidiChunk {
    /// Notes as `[start_ticks, key, length_ticks, velocity]`.
    pub notes: Vec<[u64; 4]>,
    pub controllers: Vec<MidiController>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub mpe_curves: Vec<MpeCurve>,
    pub sysex: Vec<(u64, String)>,
}

fn ctrl_kind_value(code: u8) -> serde_json::Value {
    let s = match code {
        1 => "pitch_bend",
        2 => "controller",
        3 => "program",
        4 => "channel_pressure",
        5 => "poly_pressure",
        other => return serde_json::Value::from(other),
    };
    serde_json::Value::from(s)
}

/// At most this many `MdChun` chunks are read.
pub const MAX_MIDI_CHUNKS: usize = 65_536;
/// At most this many notes are read from the whole session.
pub const MAX_MIDI_NOTES: usize = 2_000_000;
/// At most this many controller points (CC, pitch bend, pressure, program) are read from the
/// whole session.
pub const MAX_MIDI_POINTS: usize = 4_000_000;
/// At most this many SysEx bytes are read from the whole session.
pub const MAX_SYSEX_BYTES: usize = 1 << 20;
/// At most this many controller records (and MPE sub-records) are looked at in the whole
/// session: overlapping records in a crafted file cannot make the reader repeat itself without end.
pub const MAX_MIDI_STEPS: usize = 4_000_000;

/// How much more the reader may read (FreeDAWm: bounds the work and the result on a hostile file).
struct Budget {
    notes: usize,
    points: usize,
    sysex: usize,
    steps: usize,
    spent: bool,
}

impl Budget {
    fn step(&mut self) -> bool {
        if self.steps == 0 {
            self.spent = true;
            return false;
        }
        self.steps -= 1;
        true
    }
}

fn decode_mdcd_points(body: &[u8], c_at: usize, budget: &mut Budget) -> Option<(Vec<[i64; 2]>, usize)> {
    let n = usize::try_from(u32_at(body, c_at.checked_add(10)?)?).ok()?;
    let end = c_at.checked_add(14)?.checked_add(n.checked_mul(12)?)?;
    if end > body.len() {
        return None;
    }
    let take = n.min(budget.points);
    if take < n {
        budget.spent = true;
    }
    budget.points -= take;
    let mut pts = Vec::with_capacity(take);
    for r in 0..take {
        let off = c_at + 14 + 12 * r;
        let (raw_t, val) = (le_u64(body, off, 5)?, i32_at(body, off + 8)?);
        let t_plain = i64::try_from(plain_ticks(raw_t)).unwrap_or(i64::MAX);
        pts.push([t_plain, i64::from(val)]);
    }
    Some((pts, end))
}

/// Every position of `needle` in `hay`, in order (one pass).
fn positions(hay: &[u8], needle: &[u8]) -> Vec<usize> {
    let mut out = Vec::new();
    let mut scan = 0;
    while let Some(pos) = find_bytes(hay, needle, scan) {
        out.push(pos);
        scan = pos + 1;
    }
    out
}

/// The first position in `list` (sorted) at or after `from` whose `len`-byte match ends by `end`.
fn first_in(list: &[usize], from: usize, len: usize, end: usize) -> Option<usize> {
    let i = list.partition_point(|&p| p < from);
    list.get(i).copied().filter(|&p| p + len <= end)
}

/// Parse all `MdChun` chunks in `0x2000`.
pub fn parse_midi_lists(d: &[u8], tree: &[Block], warn: &mut Vec<String>) -> Vec<MidiChunk> {
    let Some(b) = tree.iter().find(|t| t.content_type == 0x2000) else {
        return Vec::new();
    };
    let Some(x) = d.get(b.data_start()..b.end().min(d.len())) else {
        return Vec::new();
    };
    let mut at = positions(x, b"MdChun");
    if at.len() > MAX_MIDI_CHUNKS {
        warn.push(format!("{} MIDI chunks: the first {MAX_MIDI_CHUNKS} are read", at.len()));
        at.truncate(MAX_MIDI_CHUNKS);
    }
    at.push(x.len());
    let mut budget = Budget { notes: MAX_MIDI_NOTES, points: MAX_MIDI_POINTS, sysex: MAX_SYSEX_BYTES, steps: MAX_MIDI_STEPS, spent: false };

    let mut out = Vec::with_capacity(at.len().saturating_sub(1));
    for (k, w) in at.windows(2).enumerate() {
        let (Some(&from), Some(&to)) = (w.first(), w.get(1)) else { continue };
        let seg = span(x, from, to);
        let mut ent = MidiChunk::default();

        if let Some(j) = find_bytes(seg, b"MdNLB", 0)
            && let Some(n) = u32_at(seg, j + 11).and_then(|v| usize::try_from(v).ok())
        {
            if n.checked_mul(35).and_then(|l| l.checked_add(j + 15)).is_some_and(|e| e <= seg.len()) {
                let take = n.min(budget.notes);
                if take < n {
                    budget.spent = true;
                }
                budget.notes -= take;
                for r in 0..take {
                    let o = j + 15 + 35 * r;
                    let (Some(raw_start), Some(&key), Some(len_t), Some(&vel)) =
                        (le_u64(seg, o, 5), seg.get(o + 8), le_u64(seg, o + 9, 5), seg.get(o + 17))
                    else {
                        break;
                    };
                    ent.notes.push([plain_ticks(raw_start), u64::from(key), len_t, u64::from(vel)]);
                }
            } else {
                warn.push(format!("MIDI chunk {k}: note count {n} does not fit"));
            }
        }

        // Controller records: every `MdCDS` in the chunk, with the `Mdss`/`MdCD` inside each found
        // from lists made in one pass (the upstream searched the chunk again for each record).
        let cds = positions(seg, b"MdCDS");
        let ss = positions(seg, b"Mdss");
        let cd = positions(seg, b"MdCD");
        for &j in &cds {
            if !budget.step() {
                break;
            }
            let body_len = u32_at(seg, j + 7).and_then(|v| usize::try_from(v).ok()).unwrap_or(0);
            let body_start = j + 11;
            let body_end = body_start.saturating_add(body_len).min(seg.len());
            let Some(body) = seg.get(body_start..body_end) else { continue };
            let (Some(s_), Some(c_)) = (first_in(&ss, body_start, 4, body_end), first_in(&cd, body_start + 4, 4, body_end)) else {
                continue;
            };
            let (s_, c_) = (s_ - body_start, c_ - body_start);
            if s_ + 24 > body.len() {
                continue;
            }
            let Some((pts, mut sub_pos)) = decode_mdcd_points(body, c_, &mut budget) else { continue };
            let m = body.get(s_ + 10..).unwrap_or(&[]);
            let kind_byte = byte(m, 0);
            let num_byte = byte(m, 1);
            let is_mpe_primary = m.get(7..9) != Some(&[0xFF, 0xFF]);
            let program_banks = if kind_byte == 3 && !pts.is_empty() {
                Some(
                    pts.iter()
                        .map(|pt| {
                            let v = u32::from_le_bytes((pt[1] as i32).to_le_bytes());
                            let bank = (v >> 8) & 0x3FFF;
                            ProgramBank {
                                ticks: u64::try_from(pt[0].max(0)).unwrap_or(0),
                                program: v & 0xFF,
                                bank_msb: (bank >> 7) & 0x7F,
                                bank_lsb: bank & 0x7F,
                            }
                        })
                        .collect(),
                )
            } else {
                None
            };
            ent.controllers.push(MidiController { kind: ctrl_kind_value(kind_byte), number: num_byte, points: pts, program_banks });

            if is_mpe_primary {
                while budget.step()
                    && let Some(s_sub) = first_in(&ss, body_start + sub_pos, 4, body_end).map(|p| p - body_start)
                {
                    let Some(c_sub) = first_in(&cd, body_start + s_sub + 4, 4, body_end).map(|p| p - body_start) else {
                        break;
                    };
                    let Some((sub_pts, next_pos)) = decode_mdcd_points(body, c_sub, &mut budget) else {
                        break;
                    };
                    if s_sub + 24 <= body.len() {
                        let sm = body.get(s_sub + 10..).unwrap_or(&[]);
                        if byte(sm, 5) == 4 && sm.get(7..9) != Some(&[0xFF, 0xFF]) {
                            let ns = le_u64(sm, 9, 5).map(plain_ticks).unwrap_or(0);
                            ent.mpe_curves.push(MpeCurve {
                                kind: ctrl_kind_value(byte(sm, 0)),
                                number: byte(sm, 1),
                                mpe_channel: byte(sm, 6),
                                key: byte(sm, 7),
                                velocity: byte(sm, 8),
                                note_start: ns,
                                points: sub_pts,
                            });
                        }
                    }
                    // Always forward: `next_pos` is past `c_sub`, which is past `s_sub`.
                    sub_pos = next_pos.max(s_sub + 1);
                }
            }
        }

        if let Some(tel_at) = find_bytes(seg, b"MdTEL", 0)
            && tel_at + 15 <= seg.len()
            && let Some(tlen) = u32_at(seg, tel_at + 7).and_then(|v| usize::try_from(v).ok())
            && let Some(m) = u32_at(seg, tel_at + 11).and_then(|v| usize::try_from(v).ok())
            && m > 0
        {
            let tend = (tel_at + 11).saturating_add(tlen).min(seg.len());
            let mut p = tel_at + 15;
            for _ in 0..m {
                if p + 19 > tend {
                    break;
                }
                let Some(raw_t) = le_u64(seg, p, 5) else { break };
                let Some(pay_len) = u32_at(seg, p + 15).and_then(|v| usize::try_from(v).ok()) else {
                    break;
                };
                if p + 19 + pay_len > tend {
                    break;
                }
                if pay_len > budget.sysex {
                    budget.spent = true;
                    break;
                }
                budget.sysex -= pay_len;
                let hex = span(seg, p + 19, p + 19 + pay_len).iter().map(|b| format!("{b:02x}")).collect::<String>();
                ent.sysex.push((plain_ticks(raw_t), hex));
                p += 19 + pay_len;
            }
        }

        out.push(ent);
    }
    if budget.spent {
        warn.push("MIDI: more notes, controller points or SysEx than are read; the rest is left out".to_owned());
    }
    out
}

//! 14-bit controllers and RPN/NRPN parameters as one value (the MIDI Editor's Lanes › 14-bit and
//! RPN). The clip keeps ordinary 7-bit controller events; these commands read and write them as
//! one value 0..16383:
//!
//! - `cc14:N` (N 0..31): CC N is the high 7 bits, CC N+32 at the same tick (or the last one before
//!   on the channel) the low 7 bits. Writing puts both at each point.
//! - `rpn:P` and `nrpn:P` (P 0..16383, MSB × 128 + LSB; also hex, `nrpn:0x82`): the parameter
//!   selected by CC 101/100
//!   (RPN) or CC 99/98 (NRPN), its value set by Data Entry (CC 6 the high bits, CC 38 the low).
//!   Writing puts the select, the data entry and, after them, the null RPN (101/100 = 127), so a
//!   later CC 6 is not taken for the parameter.
//!
//! Playback needs nothing new: the events are ordinary controllers, and the chase on a locate
//! already sends each parameter's select before its value (`mix::rpn`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{CtrlEvent, CtrlKind, Sequence};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "midi.ctrl14s",
            "List 14-bit and RPN Values",
            [],
            None,
            "{clip, lane?: cc14:1|rpn:0|nrpn:130, channel?: 0..15 (every channel when absent)} -> {points: [{tick, channel, value 0..16383}], lanes: [lane with data]}",
            always,
            list
        ),
        cmd!(
            "midi.ctrl14_draw",
            "Draw 14-bit Controller",
            [],
            None,
            "{clip, lane: cc14:N|rpn:P|nrpn:P, points: [[tick, value 0..16383]…], channel?: 0} -> {points} — replaces the lane's values on the channel inside the points' span with these (both controllers of a 14-bit pair; select, data entry and the null RPN for a parameter)",
            always,
            draw
        ),
        cmd!(
            "midi.ctrl14_clear",
            "Clear 14-bit Controller",
            [],
            None,
            "{clip, lane, from: tick, to: tick, channel?: 0} -> {removed: events} — removes the lane's values on the channel in from..=to",
            always,
            clear
        ),
    ]
}

/// A lane of values 0..16383.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WideLane {
    /// CC n (0..31) with CC n+32 as its low bits.
    Cc14(u8),
    /// A registered parameter (0..16383).
    Rpn(u16),
    /// A non-registered parameter.
    Nrpn(u16),
}

/// The highest value of a wide lane.
pub const WIDE_MAX: i32 = 16383;

impl WideLane {
    /// Reads `cc14:1`, `rpn:0`, `nrpn:130` (also hex, `nrpn:0x82`).
    pub fn parse(name: &str) -> Option<WideLane> {
        let n = name.trim().to_ascii_lowercase();
        let (kind, num) = n.split_once(':')?;
        let num = num.trim();
        let v = match num.strip_prefix("0x") {
            Some(h) => u32::from_str_radix(h, 16).ok()?,
            None => num.parse::<u32>().ok()?,
        };
        match kind.trim() {
            "cc14" => (v <= 31).then_some(WideLane::Cc14(v as u8)),
            "rpn" => (v <= 16383).then_some(WideLane::Rpn(v as u16)),
            "nrpn" => (v <= 16383).then_some(WideLane::Nrpn(v as u16)),
            _ => None,
        }
    }

    /// The lane's id, as [`WideLane::parse`] reads it.
    pub fn id(self) -> String {
        match self {
            WideLane::Cc14(n) => format!("cc14:{n}"),
            WideLane::Rpn(p) => format!("rpn:{p}"),
            WideLane::Nrpn(p) => format!("nrpn:{p}"),
        }
    }

    /// A name for menus and lane labels.
    pub fn label(self) -> String {
        match self {
            WideLane::Cc14(n) => format!("CC{n}/{} (14-bit)", n + 32),
            WideLane::Rpn(0) => "Pitch Bend Range (RPN 0)".into(),
            WideLane::Rpn(1) => "Fine Tuning (RPN 1)".into(),
            WideLane::Rpn(2) => "Coarse Tuning (RPN 2)".into(),
            WideLane::Rpn(5) => "Modulation Depth (RPN 5)".into(),
            WideLane::Rpn(p) => format!("RPN {}/{}", p >> 7, p & 127),
            WideLane::Nrpn(p) => format!("NRPN {}/{}", p >> 7, p & 127),
        }
    }

    /// The (MSB, LSB) select controllers of a parameter lane.
    fn select_ccs(self) -> Option<(u8, u8)> {
        match self {
            WideLane::Rpn(_) => Some((101, 100)),
            WideLane::Nrpn(_) => Some((99, 98)),
            WideLane::Cc14(_) => None,
        }
    }

    fn param(self) -> Option<u16> {
        match self {
            WideLane::Rpn(p) | WideLane::Nrpn(p) => Some(p),
            WideLane::Cc14(_) => None,
        }
    }
}

fn cc_of(c: &CtrlEvent) -> Option<(u8, i32)> {
    match c.kind {
        CtrlKind::Cc(n) => Some((n, c.value.clamp(0, 127))),
        _ => None,
    }
}

/// Which parameter is selected: RPN or NRPN, MSB, LSB (127/127 is none).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Selection {
    nrpn: bool,
    msb: u8,
    lsb: u8,
}

impl Selection {
    fn lane(self) -> Option<WideLane> {
        if self.msb == 127 && self.lsb == 127 {
            return None;
        }
        let p = u16::from(self.msb) << 7 | u16::from(self.lsb);
        Some(if self.nrpn { WideLane::Nrpn(p) } else { WideLane::Rpn(p) })
    }
}

/// What each controller event of `seq` is to the wide lanes: for each event, the parameter lane
/// it belongs to (a select of it, the null after it, or a data entry while it is selected), and
/// the points each lane has (tick, channel, value).
struct Reading {
    /// Per event of `seq.ctrls`: the parameter lane it writes, if any.
    owner: Vec<Option<WideLane>>,
    points: Vec<(WideLane, i64, u8, i32)>,
}

fn read(seq: &Sequence) -> Reading {
    let mut owner = vec![None; seq.ctrls.len()];
    let mut points: Vec<(WideLane, i64, u8, i32)> = Vec::new();
    let mut sel = [Selection { nrpn: false, msb: 127, lsb: 127 }; 16];
    let mut data = [(0i32, 0i32); 16];
    // The select events since the last data entry or null, per channel: they go with the
    // parameter the next data entry sets (or, before a null, the one set last).
    let mut pending: [Vec<usize>; 16] = Default::default();
    let mut last_param: [Option<WideLane>; 16] = [None; 16];
    // 14-bit pairs: the latest MSB and LSB value per channel and controller.
    let mut msb = [[None::<i32>; 32]; 16];
    let mut lsb = [[0i32; 32]; 16];
    for (i, c) in seq.ctrls.iter().enumerate() {
        let Some((n, v)) = cc_of(c) else { continue };
        let ch = usize::from(c.channel.min(15));
        let (Some(msb), Some(lsb)) = (msb.get_mut(ch), lsb.get_mut(ch)) else { continue };
        let (Some(s), Some(d)) = (sel.get_mut(ch), data.get_mut(ch)) else { continue };
        match n {
            98..=101 => {
                let nrpn = n == 99 || n == 98;
                if s.nrpn != nrpn {
                    *s = Selection { nrpn, msb: 127, lsb: 127 };
                }
                if n == 101 || n == 99 {
                    s.msb = v as u8;
                } else {
                    s.lsb = v as u8;
                }
                if let Some(p) = pending.get_mut(ch) {
                    p.push(i);
                    if s.lane().is_none() {
                        for k in p.drain(..) {
                            if let Some(o) = owner.get_mut(k) {
                                *o = last_param.get(ch).copied().flatten();
                            }
                        }
                    }
                }
            }
            6 | 38 => {
                if let Some(l) = s.lane() {
                    if n == 6 {
                        *d = (v, 0);
                    } else {
                        d.1 = v;
                    }
                    if let Some(lp) = last_param.get_mut(ch) {
                        *lp = Some(l);
                    }
                    let taken: Vec<usize> = pending.get_mut(ch).map(std::mem::take).unwrap_or_default();
                    for k in taken.into_iter().chain([i]) {
                        if let Some(o) = owner.get_mut(k) {
                            *o = Some(l);
                        }
                    }
                    let value = d.0 * 128 + d.1;
                    match points.last_mut() {
                        Some(p) if p.0 == l && p.1 == c.tick && p.2 == c.channel => p.3 = value,
                        _ => points.push((l, c.tick, c.channel, value)),
                    }
                }
            }
            0..=31 => {
                if let (Some(m), Some(lo)) = (msb.get_mut(usize::from(n)), lsb.get(usize::from(n))) {
                    *m = Some(v);
                    points.push((WideLane::Cc14(n), c.tick, c.channel, v * 128 + lo));
                }
            }
            32..=63 => {
                let k = usize::from(n - 32);
                if let Some(lo) = lsb.get_mut(k) {
                    *lo = v;
                }
                if let Some(Some(m)) = msb.get(k).copied() {
                    let l = WideLane::Cc14(n - 32);
                    let value = m * 128 + v;
                    match points.iter_mut().rev().find(|p| p.0 == l && p.2 == c.channel) {
                        Some(p) if p.1 == c.tick => p.3 = value,
                        _ => points.push((l, c.tick, c.channel, value)),
                    }
                }
            }
            _ => {}
        }
    }
    // A 14-bit lane needs its low controller somewhere: a lone CC n stays a 7-bit lane.
    let paired = |n: u8, ch: u8| seq.ctrls.iter().any(|c| c.kind == CtrlKind::Cc(n + 32) && c.channel == ch);
    points.retain(|p| match p.0 {
        WideLane::Cc14(n) => paired(n, p.2),
        _ => true,
    });
    Reading { owner, points }
}

/// The points of `lane` (tick, channel, value), in time order; every channel when `channel` is
/// `None`.
pub fn points_of(seq: &Sequence, lane: WideLane, channel: Option<u8>) -> Vec<(i64, u8, i32)> {
    read(seq).points.into_iter().filter(|p| p.0 == lane && channel.is_none_or(|ch| p.2 == ch)).map(|p| (p.1, p.2, p.3)).collect()
}

/// The wide lanes `seq` has values on, 14-bit pairs first.
pub fn lanes_of(seq: &Sequence) -> Vec<WideLane> {
    let mut v: Vec<WideLane> = Vec::new();
    for p in read(seq).points {
        if !v.contains(&p.0) {
            v.push(p.0);
        }
    }
    v.sort_by_key(|l| match l {
        WideLane::Cc14(n) => u32::from(*n),
        WideLane::Rpn(p) => 100 + u32::from(*p),
        WideLane::Nrpn(p) => 20_000 + u32::from(*p),
    });
    v
}

/// Removes `lane`'s events on `channel` in `lo..=hi`; returns how many went.
pub fn clear_span(seq: &mut Sequence, lane: WideLane, channel: u8, lo: i64, hi: i64) -> usize {
    let before = seq.ctrls.len();
    match lane {
        WideLane::Cc14(n) => {
            seq.ctrls
                .retain(|c| !(c.channel == channel && c.tick >= lo && c.tick <= hi && (c.kind == CtrlKind::Cc(n) || c.kind == CtrlKind::Cc(n + 32))));
        }
        _ => {
            let owner = read(seq).owner;
            let mut i = 0;
            seq.ctrls.retain(|c| {
                let mine = owner.get(i).copied().flatten() == Some(lane);
                i += 1;
                !(mine && c.channel == channel && c.tick >= lo && c.tick <= hi)
            });
        }
    }
    before - seq.ctrls.len()
}

/// Writes `pts` (tick, value 0..16383) onto `lane` on `channel`, replacing what it had in their
/// span.
pub fn write(seq: &mut Sequence, lane: WideLane, channel: u8, pts: &[(i64, i32)]) {
    let (Some(lo), Some(hi)) = (pts.iter().map(|p| p.0).min(), pts.iter().map(|p| p.0).max()) else { return };
    clear_span(seq, lane, channel, lo, hi);
    let ev = |tick: i64, n: u8, value: i32| CtrlEvent { tick, channel, kind: CtrlKind::Cc(n), value, shape: Default::default() };
    let mut add: Vec<CtrlEvent> = Vec::new();
    for &(tick, value) in pts {
        let value = value.clamp(0, WIDE_MAX);
        let (hi7, lo7) = (value >> 7, value & 127);
        match lane {
            WideLane::Cc14(n) => {
                add.push(ev(tick, n, hi7));
                add.push(ev(tick, n + 32, lo7));
            }
            _ => {
                let (Some((sm, sl)), Some(p)) = (lane.select_ccs(), lane.param()) else { continue };
                add.push(ev(tick, sm, i32::from(p >> 7)));
                add.push(ev(tick, sl, i32::from(p & 127)));
                add.push(ev(tick, 6, hi7));
                add.push(ev(tick, 38, lo7));
                add.push(ev(tick, 101, 127));
                add.push(ev(tick, 100, 127));
            }
        }
    }
    // Inserted after every event at or before its tick, in the order written.
    for e in add {
        let at = seq.ctrls.partition_point(|c| c.tick <= e.tick);
        seq.ctrls.insert(at, e);
    }
}

fn lane_param(p: &Value, cmd: &str) -> Result<WideLane> {
    let name = str_param(p, "lane").ok_or_else(|| bad(cmd, "`lane` required (cc14:1, rpn:0, nrpn:130)"))?;
    WideLane::parse(name).ok_or_else(|| bad(cmd, format!("unknown lane '{name}': use cc14:0…cc14:31, rpn:0…16383 or nrpn:0…16383 (0x hex too)")))
}

fn seq_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Sequence> {
    match e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl14s";
    let id = clip_id(p, C)?;
    let lane = match str_param(p, "lane") {
        Some(_) => Some(lane_param(p, C)?),
        None => None,
    };
    let channel = p.get("channel").and_then(Value::as_i64).map(|c| c.clamp(0, 15) as u8);
    let Some((_, clip)) = e.session().find_clip(id) else { return Err(bad(C, "no such clip")) };
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let points: Vec<Value> = match lane {
        Some(l) => {
            points_of(sequence, l, channel).into_iter().map(|(tick, ch, value)| json!({"tick": tick, "channel": ch, "value": value})).collect()
        }
        None => Vec::new(),
    };
    let lanes: Vec<String> = lanes_of(sequence).into_iter().map(WideLane::id).collect();
    Ok(json!({"points": points, "lanes": lanes}))
}

/// The most points one call takes.
const MAX_POINTS: usize = 20_000;

fn draw(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl14_draw";
    let id = clip_id(p, C)?;
    let lane = lane_param(p, C)?;
    let channel = i64_or(p, "channel", 0).clamp(0, 15) as u8;
    let arr = p.get("points").and_then(Value::as_array).ok_or_else(|| bad(C, "`points` (an array of [tick, value]) required"))?;
    if arr.is_empty() || arr.len() > MAX_POINTS {
        return Err(bad(C, format!("`points` must hold 1 to {MAX_POINTS} points")));
    }
    let mut pts: Vec<(i64, i32)> = Vec::with_capacity(arr.len());
    for v in arr {
        match v.as_array().map(|a| (a.first().and_then(Value::as_i64), a.get(1).and_then(Value::as_i64))) {
            Some((Some(t), Some(val))) => pts.push((t.max(0), val.clamp(0, i64::from(WIDE_MAX)) as i32)),
            _ => return Err(bad(C, format!("bad point {v}: want [tick, value]"))),
        }
    }
    // One point a tick (the last wins), in time order, without repeats.
    pts.sort_by_key(|x| x.0);
    let mut thin: Vec<(i64, i32)> = Vec::with_capacity(pts.len());
    for (t, v) in pts {
        match thin.last_mut() {
            Some(last) if last.0 == t => last.1 = v,
            _ => thin.push((t, v)),
        }
    }
    thin.dedup_by(|later, kept| later.1 == kept.1);
    let n = thin.len();
    write(seq_mut(e, id, C)?, lane, channel, &thin);
    Ok(json!({"points": n}))
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.ctrl14_clear";
    let id = clip_id(p, C)?;
    let lane = lane_param(p, C)?;
    let channel = i64_or(p, "channel", 0).clamp(0, 15) as u8;
    let from = p.get("from").and_then(Value::as_i64).ok_or_else(|| bad(C, "`from` required"))?;
    let to = p.get("to").and_then(Value::as_i64).ok_or_else(|| bad(C, "`to` required"))?;
    let removed = clear_span(seq_mut(e, id, C)?, lane, channel, from.min(to), from.max(to));
    Ok(json!({"removed": removed}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Engine;

    fn cc(tick: i64, n: u8, value: i32) -> CtrlEvent {
        CtrlEvent { tick, channel: 0, kind: CtrlKind::Cc(n), value, shape: Default::default() }
    }

    #[test]
    fn lane_names_read_and_write() {
        assert_eq!(WideLane::parse("cc14:1"), Some(WideLane::Cc14(1)));
        assert_eq!(WideLane::parse("cc14:32"), None);
        assert_eq!(WideLane::parse("NRPN:0x0102"), Some(WideLane::Nrpn(0x0102)));
        assert_eq!(WideLane::parse("rpn:0"), Some(WideLane::Rpn(0)));
        assert_eq!(WideLane::parse("rpn:16384"), None);
        for l in [WideLane::Cc14(7), WideLane::Rpn(2), WideLane::Nrpn(300)] {
            assert_eq!(WideLane::parse(&l.id()), Some(l));
        }
    }

    #[test]
    fn a_cc1_and_cc33_pair_reads_as_one_value() {
        let seq = Sequence { notes: vec![], ctrls: vec![cc(0, 1, 64), cc(0, 33, 5), cc(480, 1, 10), cc(960, 33, 100), cc(960, 7, 90)] };
        // A new MSB keeps the low bits last sent until a new LSB comes.
        assert_eq!(points_of(&seq, WideLane::Cc14(1), None), vec![(0, 0, 64 * 128 + 5), (480, 0, 10 * 128 + 5), (960, 0, 10 * 128 + 100)]);
        assert_eq!(lanes_of(&seq), vec![WideLane::Cc14(1)], "CC7 has no low partner: it stays a 7-bit lane");
    }

    #[test]
    fn an_nrpn_lane_reads_its_parameter_from_four_controllers() {
        let seq = Sequence {
            notes: vec![],
            ctrls: vec![cc(0, 99, 1), cc(0, 98, 2), cc(0, 6, 3), cc(0, 38, 4), cc(240, 6, 9), cc(240, 101, 0), cc(240, 100, 0), cc(480, 6, 2)],
        };
        assert_eq!(points_of(&seq, WideLane::Nrpn(130), None), vec![(0, 0, 3 * 128 + 4), (240, 0, 9 * 128)], "MSB 1, LSB 2: parameter 130");
        assert_eq!(points_of(&seq, WideLane::Rpn(0), None), vec![(480, 0, 256)], "a later select moves data entry to RPN 0");
    }

    #[test]
    fn drawing_round_trips_and_replaces_only_its_span() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "K", "kind": "midi"})).unwrap();
        let clip = e.execute("midi.new_clip", &json!({"track": "K", "start": 0, "end": 96_000})).unwrap()["clip"].clone();
        e.execute("midi.ctrl_add", &json!({"clip": clip, "lane": "cc6", "tick": 2000, "value": 50})).unwrap();
        e.execute("midi.ctrl14_draw", &json!({"clip": clip, "lane": "cc14:1", "points": [[0, 0], [480, 8192], [960, 16383]]})).unwrap();
        e.execute("midi.ctrl14_draw", &json!({"clip": clip, "lane": "rpn:0", "points": [[0, 12 * 128]]})).unwrap();
        let r = e.execute("midi.ctrl14s", &json!({"clip": clip, "lane": "cc14:1"})).unwrap();
        let vals: Vec<(i64, i64)> =
            r["points"].as_array().unwrap().iter().map(|p| (p["tick"].as_i64().unwrap(), p["value"].as_i64().unwrap())).collect();
        assert_eq!(vals, vec![(0, 0), (480, 8192), (960, 16383)]);
        assert_eq!(r["lanes"], json!(["cc14:1", "rpn:0"]));
        let r = e.execute("midi.ctrl14s", &json!({"clip": clip, "lane": "rpn:0"})).unwrap();
        assert_eq!(r["points"], json!([{"tick": 0, "channel": 0, "value": 1536}]));
        // The null RPN after the parameter keeps the later CC 6 a plain controller.
        let r = e.execute("midi.ctrl14s", &json!({"clip": clip})).unwrap();
        assert!(!r["lanes"].as_array().unwrap().iter().any(|l| l != "cc14:1" && l != "rpn:0"), "{r}");
        // A second pass over the middle replaces only the middle.
        e.execute("midi.ctrl14_draw", &json!({"clip": clip, "lane": "cc14:1", "points": [[400, 100], [600, 200]]})).unwrap();
        let r = e.execute("midi.ctrl14s", &json!({"clip": clip, "lane": "cc14:1"})).unwrap();
        let vals: Vec<i64> = r["points"].as_array().unwrap().iter().map(|p| p["value"].as_i64().unwrap()).collect();
        assert_eq!(vals, vec![0, 100, 200, 16383]);
        let r = e.execute("midi.ctrl14_clear", &json!({"clip": clip, "lane": "rpn:0", "from": 0, "to": 0})).unwrap();
        assert_eq!(r["removed"], 6);
        let all = e.execute("midi.ctrls", &json!({"clip": clip, "lane": "cc6"})).unwrap();
        assert_eq!(all["events"].as_array().unwrap().len(), 1, "the plain CC6 at 2000 stays");
        e.execute("edit.undo", &json!({})).unwrap();
        let r = e.execute("midi.ctrl14s", &json!({"clip": clip, "lane": "rpn:0"})).unwrap();
        assert_eq!(r["points"].as_array().unwrap().len(), 1, "one undo step");
    }
}

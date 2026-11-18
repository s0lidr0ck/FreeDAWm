//! A MIDI clip's System Exclusive messages (`Sequence::sysex`), as the MIDI Event List shows
//! them: list, add, edit (move, or new bytes) and delete. Bytes are written as hex (`F0 43 10
//! 4C 00 00 7E 00 F7`); `F0` and `F7` are added when left out. Messages are addressed by index
//! in the clip's SysEx list (sorted by tick); a command that can re-sort it returns where the
//! message ended up.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_midi::{MAX_SYSEX, Sequence, SysEx};
use soundcraft_model::{ClipContent, ClipId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "midi.sysex", "List SysEx", [], None, "{clip} -> {events: [{index, tick, bytes: hex, length}]}", always, list),
        cmd!(
            "midi.sysex_add",
            "Add SysEx",
            [],
            None,
            "{clip, tick, bytes: hex (\"F0 7E 7F 09 01 F7\"; F0/F7 added when left out; at most 1024 bytes)} -> {index}",
            always,
            add
        ),
        cmd!("midi.sysex_edit", "Edit SysEx", [], None, "{clip, index, tick?, bytes?: hex} -> {index}", always, edit),
        cmd!("midi.sysex_delete", "Delete SysEx", [], None, "{clip, indices: [n]} -> {deleted}", always, delete),
    ]
}

/// The bytes as the Event List writes them: two hex digits each, spaced.
pub fn hex(data: &[u8]) -> String {
    data.iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(" ")
}

/// Bytes from hex text: pairs of digits, spaces, commas and `0x` prefixes ignored. `None` for
/// anything else or an odd number of digits.
pub fn parse_hex(text: &str) -> Option<Vec<u8>> {
    let digits: String = text.replace("0x", " ").replace("0X", " ").chars().filter(|c| !c.is_whitespace() && *c != ',').collect();
    if digits.is_empty() || !digits.len().is_multiple_of(2) || digits.len() > MAX_SYSEX * 2 {
        return None;
    }
    (0..digits.len()).step_by(2).map(|i| digits.get(i..i + 2).and_then(|d| u8::from_str_radix(d, 16).ok())).collect()
}

fn message(p: &Value, cmd: &str, tick: i64) -> Result<SysEx> {
    let text = str_param(p, "bytes").ok_or_else(|| bad(cmd, "`bytes` required (hex, like \"F0 7E 7F 09 01 F7\")"))?;
    let bytes = parse_hex(text).ok_or_else(|| bad(cmd, "`bytes` must be hex digits in pairs"))?;
    SysEx::new(tick, &bytes)
        .ok_or_else(|| bad(cmd, format!("not a SysEx message: data bytes are below 80 and the whole is at most {MAX_SYSEX} bytes")))
}

fn seq_mut<'a>(e: &'a mut Engine, id: ClipId, cmd: &str) -> Result<&'a mut Sequence> {
    match e.session_mut().find_clip_mut(id).map(|c| &mut c.content) {
        Some(ClipContent::Midi { sequence }) => Ok(sequence),
        Some(_) => Err(bad(cmd, "not a MIDI clip")),
        None => Err(bad(cmd, "no such clip")),
    }
}

/// Sorts the messages by tick (stable); where the one at old index `i` ended up.
fn sort_tracked(seq: &mut Sequence, i: usize) -> Option<usize> {
    let mut work: Vec<(SysEx, bool)> = std::mem::take(&mut seq.sysex).into_iter().enumerate().map(|(k, x)| (x, k == i)).collect();
    work.sort_by_key(|(x, _)| x.tick);
    let at = work.iter().position(|(_, m)| *m);
    seq.sysex = work.into_iter().map(|(x, _)| x).collect();
    at
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.sysex";
    let id = clip_id(p, C)?;
    let Some((_, clip)) = e.session().find_clip(id) else { return Err(bad(C, "no such clip")) };
    let ClipContent::Midi { sequence } = &clip.content else { return Err(bad(C, "not a MIDI clip")) };
    let events: Vec<Value> =
        sequence.sysex.iter().enumerate().map(|(i, x)| json!({"index": i, "tick": x.tick, "bytes": hex(&x.data), "length": x.data.len()})).collect();
    Ok(json!({"events": events}))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.sysex_add";
    let id = clip_id(p, C)?;
    let tick = p.get("tick").and_then(Value::as_i64).ok_or_else(|| bad(C, "`tick` required"))?.max(0);
    let x = message(p, C, tick)?;
    let seq = seq_mut(e, id, C)?;
    seq.sysex.push(x);
    let last = seq.sysex.len() - 1;
    Ok(json!({"index": sort_tracked(seq, last)}))
}

fn edit(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.sysex_edit";
    let id = clip_id(p, C)?;
    let idx = p.get("index").and_then(Value::as_u64).and_then(|i| usize::try_from(i).ok()).ok_or_else(|| bad(C, "`index` required"))?;
    let new = p.get("bytes").map(|_| message(p, C, 0)).transpose()?;
    let tick = p.get("tick").and_then(Value::as_i64);
    let seq = seq_mut(e, id, C)?;
    let x = seq.sysex.get_mut(idx).ok_or_else(|| bad(C, "index out of range"))?;
    if let Some(n) = new {
        x.data = n.data;
    }
    if let Some(t) = tick {
        x.tick = t.max(0);
    }
    Ok(json!({"index": sort_tracked(seq, idx)}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "midi.sysex_delete";
    let id = clip_id(p, C)?;
    let idx: Vec<usize> = p
        .get("indices")
        .and_then(Value::as_array)
        .map(|a| a.iter().filter_map(Value::as_u64).filter_map(|x| usize::try_from(x).ok()).collect())
        .unwrap_or_default();
    let seq = seq_mut(e, id, C)?;
    let before = seq.sysex.len();
    let mut i = 0;
    seq.sysex.retain(|_| {
        let keep = !idx.contains(&i);
        i += 1;
        keep
    });
    Ok(json!({"deleted": before - seq.sysex.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_clips_sysex_is_listed_added_moved_changed_and_deleted() {
        let mut e = Engine::default();
        let t = e.execute("track.new", &json!({"kind": "instrument"})).unwrap()["tracks"][0].as_u64().unwrap();
        let c = e.execute("midi.new_clip", &json!({"track": t, "start": 0, "end": 96_000})).unwrap()["clip"].as_u64().unwrap();
        let at = |e: &mut Engine, tick: i64, bytes: &str| {
            e.execute("midi.sysex_add", &json!({"clip": c, "tick": tick, "bytes": bytes})).unwrap()["index"].clone()
        };
        assert_eq!(at(&mut e, 960, "F0 7E 7F 09 01 F7"), json!(0));
        assert_eq!(at(&mut e, 0, "43 10 4C 00 00 7E 00"), json!(0), "F0 and F7 added; sorted by tick");
        let l = e.execute("midi.sysex", &json!({"clip": c})).unwrap();
        assert_eq!(l["events"][0]["bytes"], "F0 43 10 4C 00 00 7E 00 F7");
        assert_eq!((l["events"][1]["tick"].as_i64(), l["events"][1]["length"].as_u64()), (Some(960), Some(6)));
        // Moving the first past the second re-sorts; new bytes replace them.
        let i = e.execute("midi.sysex_edit", &json!({"clip": c, "index": 0, "tick": 1920, "bytes": "0xF0,0x41,0xF7"})).unwrap();
        assert_eq!(i["index"], json!(1));
        let l = e.execute("midi.sysex", &json!({"clip": c})).unwrap();
        assert_eq!((l["events"][1]["bytes"].as_str(), l["events"][1]["tick"].as_i64()), (Some("F0 41 F7"), Some(1920)));
        for wrong in ["F0 80 F7", "F0 4", "zz", ""] {
            assert!(e.execute("midi.sysex_add", &json!({"clip": c, "tick": 0, "bytes": wrong})).is_err(), "{wrong}");
        }
        assert!(e.execute("midi.sysex_edit", &json!({"clip": c, "index": 9, "tick": 0})).is_err());
        assert_eq!(e.execute("midi.sysex_delete", &json!({"clip": c, "indices": [0]})).unwrap()["deleted"], 1);
        let l = e.execute("midi.sysex", &json!({"clip": c})).unwrap();
        assert_eq!(l["events"].as_array().unwrap().len(), 1);
        assert!(e.undo());
        assert_eq!(e.execute("midi.sysex", &json!({"clip": c})).unwrap()["events"].as_array().unwrap().len(), 2, "one undo step");
    }
}

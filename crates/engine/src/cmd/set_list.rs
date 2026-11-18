//! The show's set list (parity section 8, the show page, step 1): songs of this session, each a
//! memory location holding a range, played in order. `setlist.play` plays one song from its
//! start to its end; the app then stops on the next one, or goes straight on to it when the
//! song says so (`go_on`), so a set is played with the space bar or without a hand. The list
//! is saved with the session ([`Session::set_list`]); a song whose memory location was deleted
//! is skipped and listed as missing.

use super::*;
use crate::{TransportRequest, cmd};
use serde_json::json;
use soundcraft_model::{MarkerId, MarkerKind, MemoryLocation, Session, SetListEntry};

const ADD: &str = "setlist.add";
const REMOVE: &str = "setlist.remove";
const MOVE: &str = "setlist.move";
const GO_ON: &str = "setlist.go_on";
const PLAY: &str = "setlist.play";

/// The most songs a set list holds.
pub const MAX_SONGS: usize = 500;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "setlist.list",
            "List Set List",
            [],
            None,
            "{} -> {songs: [{index, location, name, start, end, go_on, missing?}]}",
            always,
            list
        ),
        cmd!(
            "setlist.add",
            "Add to Set List",
            [],
            None,
            "{location: memory location id (a selection: a range), at?: index (default: the end)} — the song is played from the location's start to its end -> {index}",
            always,
            add
        ),
        cmd!("setlist.remove", "Remove from Set List", [], None, "{index} -> {songs}", always, remove),
        cmd!("setlist.move", "Move in Set List", [], None, "{index, to} -> {index}", always, move_song),
        cmd!(
            "setlist.go_on",
            "Go Straight On",
            [],
            None,
            "{index, value?: bool (toggles when left out)} — at the song's end the next one plays at once, instead of the transport stopping on it -> {go_on}",
            always,
            go_on
        ),
        cmd!(
            noundo "setlist.play",
            "Play Song",
            [],
            None,
            "{index?: (default 0)} — plays the song from its start to its end -> {index, name, start, end, next?: index}",
            always,
            play
        ),
    ]
}

/// The memory location of song `i`, when it still holds a range.
pub fn song(s: &Session, i: usize) -> Option<&MemoryLocation> {
    let e = s.set_list.get(i)?;
    s.marker(e.location).filter(|m| m.end > m.start)
}

/// The song after `i` that can be played (missing ones skipped).
pub fn next_song(s: &Session, i: usize) -> Option<usize> {
    (i.checked_add(1)?..s.set_list.len()).find(|k| song(s, *k).is_some())
}

fn index(p: &Value, key: &str, cmd: &str, len: usize) -> Result<usize> {
    let i = p.get(key).and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).ok_or_else(|| bad(cmd, format!("`{key}` required")))?;
    if i < len { Ok(i) } else { Err(bad(cmd, format!("no song {i} in the set list"))) }
}

fn list(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session();
    let songs: Vec<Value> = s
        .set_list
        .iter()
        .enumerate()
        .map(|(i, x)| match song(s, i) {
            Some(m) => json!({"index": i, "location": x.location.0, "name": m.name, "start": m.start, "end": m.end, "go_on": x.go_on}),
            None => json!({"index": i, "location": x.location.0, "name": "", "go_on": x.go_on, "missing": true}),
        })
        .collect();
    Ok(json!({"songs": songs}))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = p.get("location").and_then(Value::as_u64).map(MarkerId).ok_or_else(|| bad(ADD, "`location` (a memory location id) required"))?;
    let s = e.session();
    let m = s.marker(id).ok_or_else(|| bad(ADD, format!("no memory location with id {}", id.0)))?;
    if m.kind != MarkerKind::Selection || m.end <= m.start {
        return Err(bad(ADD, format!("`{}` is not a range: a song is a memory location that keeps a selection", m.name)));
    }
    if s.set_list.len() >= MAX_SONGS {
        return Err(bad(ADD, format!("a set list holds at most {MAX_SONGS} songs")));
    }
    let len = s.set_list.len();
    let at = p.get("at").and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).map_or(len, |x| x.min(len));
    e.session_mut().set_list.insert(at, SetListEntry { location: id, go_on: false });
    Ok(json!({"index": at}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = index(p, "index", REMOVE, e.session().set_list.len())?;
    let s = e.session_mut();
    s.set_list.remove(i);
    Ok(json!({"songs": s.set_list.len()}))
}

fn move_song(e: &mut Engine, p: &Value) -> Result<Value> {
    let len = e.session().set_list.len();
    let i = index(p, "index", MOVE, len)?;
    let to = p.get("to").and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).ok_or_else(|| bad(MOVE, "`to` required"))?.min(len - 1);
    let s = e.session_mut();
    let x = s.set_list.remove(i);
    s.set_list.insert(to, x);
    Ok(json!({"index": to}))
}

fn go_on(e: &mut Engine, p: &Value) -> Result<Value> {
    let i = index(p, "index", GO_ON, e.session().set_list.len())?;
    let s = e.session_mut();
    let x = s.set_list.get_mut(i).ok_or_else(|| bad(GO_ON, "no such song"))?;
    x.go_on = p.get("value").and_then(Value::as_bool).unwrap_or(!x.go_on);
    Ok(json!({"go_on": x.go_on}))
}

fn play(e: &mut Engine, p: &Value) -> Result<Value> {
    if e.transport.recording {
        return Err(bad(PLAY, "not while recording"));
    }
    let s = e.session();
    if s.set_list.is_empty() {
        return Err(bad(PLAY, "the set list is empty: add songs in Window › Set List"));
    }
    let i = match p.get("index") {
        Some(_) => index(p, "index", PLAY, s.set_list.len())?,
        None => 0,
    };
    let m = song(s, i).ok_or_else(|| bad(PLAY, format!("song {} of the set list has no memory location any more", i + 1)))?;
    let (name, start, end) = (m.name.clone(), m.start.max(0), m.end);
    let next = next_song(s, i);
    e.transport_requests.push(TransportRequest::PlayRange { from: start, end: Some(end), looped: None });
    Ok(json!({"index": i, "name": name, "start": start, "end": end, "next": next}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn location(e: &mut Engine, name: &str, start: i64, end: i64) -> u64 {
        e.execute("markers.add", &json!({"name": name, "kind": "selection", "start": start, "end": end})).unwrap()["id"].as_u64().unwrap()
    }

    #[test]
    fn a_set_list_orders_songs_and_plays_each_from_start_to_end() {
        let mut e = Engine::default();
        let a = location(&mut e, "Opener", 0, 48_000);
        let b = location(&mut e, "Ballad", 96_000, 192_000);
        let c = location(&mut e, "Encore", 200_000, 300_000);
        let point = e.execute("markers.add", &json!({"name": "Hit", "kind": "marker", "start": 10})).unwrap()["id"].clone();
        assert!(e.execute(ADD, &json!({"location": point})).is_err(), "a point is not a song");
        for id in [a, c] {
            e.execute(ADD, &json!({"location": id})).unwrap();
        }
        assert_eq!(e.execute(ADD, &json!({"location": b, "at": 1})).unwrap()["index"], 1);
        e.execute(GO_ON, &json!({"index": 0})).unwrap();
        let names = |e: &mut Engine| -> Vec<String> {
            e.execute("setlist.list", &json!({})).unwrap()["songs"]
                .as_array()
                .unwrap()
                .iter()
                .map(|x| x["name"].as_str().unwrap().to_string())
                .collect()
        };
        assert_eq!(names(&mut e), ["Opener", "Ballad", "Encore"]);
        e.execute(MOVE, &json!({"index": 2, "to": 0})).unwrap();
        assert_eq!(names(&mut e), ["Encore", "Opener", "Ballad"]);
        assert!(e.session().set_list[1].go_on, "the setting moves with its song");
        let out = e.execute(PLAY, &json!({"index": 1})).unwrap();
        assert_eq!((out["name"].as_str(), out["next"].as_u64()), (Some("Opener"), Some(2)));
        assert_eq!(e.transport_requests.last(), Some(&TransportRequest::PlayRange { from: 0, end: Some(48_000), looped: None }));
        // Saved with the session; an older session has none.
        let v = serde_json::to_value(e.session()).unwrap();
        let back: Session = serde_json::from_value(v.clone()).unwrap();
        assert_eq!(back.set_list, e.session().set_list);
        let mut old = v;
        old.as_object_mut().unwrap().remove("set_list");
        assert!(serde_json::from_value::<Session>(old).unwrap().set_list.is_empty());
        // A deleted location leaves its song missing and skipped.
        e.execute("markers.delete", &json!({"id": a})).unwrap();
        let songs = e.execute("setlist.list", &json!({})).unwrap()["songs"].clone();
        assert_eq!(songs[1]["missing"], true);
        assert_eq!(next_song(e.session(), 0), Some(2));
        assert!(e.execute(PLAY, &json!({"index": 1})).is_err());
        e.execute(REMOVE, &json!({"index": 1})).unwrap();
        assert_eq!(names(&mut e), ["Encore", "Ballad"]);
        assert!(e.undo(), "one undo step per change");
        assert_eq!(e.session().set_list.len(), 3);
    }
}

//! Alternative arrangements (scratch pads, Logic's alternatives): a session keeps several versions
//! of its arrangement, each the playlist every track plays in it and its own markers, and
//! switches between them in one step. Built on playlists: a new arrangement gives every track a
//! copy of the playlist it plays now, so editing one arrangement leaves the others alone.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Arrangement, Playlist, Session, TrackId};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(query "arrangement.list", "List Arrangements", [], None, "{} -> {arrangements: [name], active}", always, list),
        cmd!(
            "arrangement.new",
            "New Arrangement",
            [],
            None,
            "{name?} — keeps the arrangement as it is (the first time, as \"Main\") and starts a new one: every track gets a copy of the playlist it plays and plays it, the markers carry over",
            always,
            new
        ),
        cmd!(
            "arrangement.switch",
            "Switch Arrangement",
            [],
            None,
            "{name} — keeps the arrangement playing now, then every track plays the playlist it had in `name` and the markers are that arrangement's -> {missing}",
            has_arrangements,
            switch
        ),
        cmd!(
            "arrangement.delete",
            "Delete Arrangement",
            [],
            None,
            "{name} — forgets an arrangement that is not playing (its playlists stay on the tracks)",
            has_arrangements,
            delete
        ),
    ]
}

fn has_arrangements(e: &Engine) -> std::result::Result<(), String> {
    if e.session().arrangements.is_empty() { Err("no arrangements yet: make one with New Arrangement".into()) } else { Ok(()) }
}

/// Most arrangements a session keeps.
const MAX_ARRANGEMENTS: usize = 64;

/// The playlist each track with a timeline plays now, by name.
fn playing(s: &Session) -> Vec<(TrackId, String)> {
    s.tracks.iter().filter(|t| t.kind.has_playlist()).filter_map(|t| t.playlists.get(t.active_playlist).map(|p| (t.id, p.name.clone()))).collect()
}

/// The arrangement playing now takes the session as it is: its playlists and markers.
fn keep_current(s: &mut Session) {
    let (now, markers) = (playing(s), s.markers.clone());
    let active = s.arrangement.clone();
    if let Some(a) = s.arrangements.iter_mut().find(|a| Some(&a.name) == active.as_ref()) {
        (a.playlists, a.markers) = (now, markers);
    }
}

fn list(e: &mut Engine, _: &Value) -> Result<Value> {
    let s = e.session();
    Ok(json!({"arrangements": s.arrangements.iter().map(|a| a.name.clone()).collect::<Vec<_>>(), "active": s.arrangement}))
}

fn named(p: &Value, id: &str) -> Result<String> {
    str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()).map(str::to_string).ok_or_else(|| bad(id, "`name` required"))
}

fn new(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrangement.new";
    let s = e.session_mut();
    if s.arrangements.len() >= MAX_ARRANGEMENTS {
        return Err(bad(C, format!("a session keeps at most {MAX_ARRANGEMENTS} arrangements")));
    }
    if s.arrangements.is_empty() {
        s.arrangements.push(Arrangement { name: "Main".into(), playlists: Vec::new(), markers: Vec::new() });
        s.arrangement = Some("Main".into());
    }
    keep_current(s);
    let taken = |n: &str, s: &Session| s.arrangements.iter().any(|a| a.name.eq_ignore_ascii_case(n));
    let name = match str_param(p, "name").map(str::trim).filter(|n| !n.is_empty()) {
        Some(n) if taken(n, s) => return Err(bad(C, format!("there is already an arrangement called '{n}'"))),
        Some(n) => n.to_string(),
        None => (2..).map(|i| format!("Arrangement {i}")).find(|n| !taken(n, s)).unwrap_or_default(),
    };
    let ids: Vec<TrackId> = s.tracks.iter().filter(|t| t.kind.has_playlist()).map(|t| t.id).collect();
    let mut playlists = Vec::new();
    for t in ids {
        let Some(tr) = s.track(t) else { continue };
        let Some(pl) = tr.playlists.get(tr.active_playlist) else { continue };
        let (mut clips, base) = (pl.clips.clone(), pl.name.clone());
        let names: Vec<String> = tr.playlists.iter().map(|p| p.name.clone()).collect();
        let pname = (1..)
            .map(|i| if i == 1 { format!("{base} · {name}") } else { format!("{base} · {name} {i}") })
            .find(|n| !names.contains(n))
            .unwrap_or_default();
        for c in &mut clips {
            c.id = s.new_clip_id();
        }
        if let Some(tr) = s.track_mut(t) {
            tr.playlists.push(Playlist { name: pname.clone(), clips, take_color: None });
            tr.active_playlist = tr.playlists.len() - 1;
        }
        playlists.push((t, pname));
    }
    let markers = s.markers.clone();
    s.arrangements.push(Arrangement { name: name.clone(), playlists, markers });
    s.arrangement = Some(name.clone());
    Ok(json!({"name": name}))
}

fn switch(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrangement.switch";
    let name = named(p, C)?;
    let s = e.session_mut();
    let Some(target) = s.arrangements.iter().find(|a| a.name.eq_ignore_ascii_case(&name)).cloned() else {
        return Err(bad(C, format!("no arrangement called '{name}'")));
    };
    keep_current(s);
    let mut missing = 0;
    for (t, pname) in &target.playlists {
        let Some(tr) = s.track_mut(*t) else { continue };
        match tr.playlists.iter().position(|p| &p.name == pname) {
            Some(i) => tr.active_playlist = i,
            None => missing += 1,
        }
    }
    s.markers = target.markers;
    s.arrangement = Some(target.name.clone());
    Ok(json!({"name": target.name, "missing": missing}))
}

fn delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "arrangement.delete";
    let name = named(p, C)?;
    let s = e.session_mut();
    if s.arrangement.as_deref().is_some_and(|a| a.eq_ignore_ascii_case(&name)) {
        return Err(bad(C, "that arrangement is playing: switch to another first"));
    }
    let before = s.arrangements.len();
    s.arrangements.retain(|a| !a.name.eq_ignore_ascii_case(&name));
    if s.arrangements.len() == before {
        return Err(bad(C, format!("no arrangement called '{name}'")));
    }
    // One left: there is nothing to switch between any more.
    if s.arrangements.len() == 1 {
        s.arrangements.clear();
        s.arrangement = None;
    }
    Ok(json!({"deleted": name}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_arrangement_plays_its_own_playlists_and_markers() {
        let mut e = crate::demo::demo_engine();
        let kick = e.session().track_by_name("Kick").unwrap().id;
        let clips = |e: &Engine| e.session().track(kick).unwrap().clips().len();
        let before = clips(&e);
        let marks = e.session().markers.len() + 1;
        e.execute("markers.add", &json!({"at": 0, "name": "Intro"})).unwrap();
        assert_eq!(e.execute("arrangement.new", &json!({"name": "Short"})).unwrap()["name"], "Short");
        // Edit the new arrangement: its own playlist and markers.
        e.execute("edit.clear", &json!({"tracks": [kick.0], "start": 0, "end": e.session().content_end()})).unwrap();
        e.session_mut().markers.clear();
        assert_eq!(clips(&e), 0);
        e.execute("arrangement.switch", &json!({"name": "main"})).unwrap();
        assert_eq!(clips(&e), before, "Main keeps its playlist");
        assert_eq!(e.session().markers.len(), marks, "and its markers");
        let r = e.execute("arrangement.list", &json!({})).unwrap();
        assert_eq!(r, json!({"arrangements": ["Main", "Short"], "active": "Main"}));
        e.execute("arrangement.switch", &json!({"name": "Short"})).unwrap();
        assert_eq!((clips(&e), e.session().markers.len()), (0, 0));
        assert!(e.execute("arrangement.delete", &json!({"name": "Short"})).is_err(), "it is playing");
        assert!(e.undo());
        assert_eq!(e.session().arrangement.as_deref(), Some("Main"), "a switch is one undo step");
        e.execute("arrangement.delete", &json!({"name": "Short"})).unwrap();
        assert!(e.session().arrangements.is_empty(), "one left is none");
    }
}

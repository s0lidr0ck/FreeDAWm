//! The Spectrum editor's view state, kept with the session as `edit` flags (as the Mix window's
//! `mix.pitch_expanded` is): which tracks' curves it shows, whether it follows the Edit window's
//! track selection instead, and whether it shows its stereo scope. None of these is an undo step (a view
//! setting); sessions saved before the editor existed have none of the flags and load with the
//! defaults below. See `docs/spectrum.md`.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{Session, Track, TrackKind};

/// The flag: the editor shows the selected tracks only (whatever their own switches say).
pub const ONLY_SELECTED: &str = "spectrum.only_selected";

/// The flag: the editor shows its stereo scope beside the spectrum.
pub const SCOPE: &str = "spectrum.scope";

/// Curves drawn at once at most. Measured (`docs/spectrum.md`): 24 shown curves cost a UI frame
/// about 1.7 ms and the audio thread about 1% of a block; past that the graph no longer reads.
pub const MAX_VISIBLE: usize = 24;

/// Why a curve cannot be switched on at [`MAX_VISIBLE`].
pub fn cap_reason() -> String {
    format!("The Spectrum shows at most {MAX_VISIBLE} curves at once: hide one first")
}

/// What one curve of the editor stands for.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Curve {
    Track(TrackId),
    /// The main mix after the master faders.
    Master,
}

/// The list's sections: the tracks that make sounds, the busses (Aux inputs and routing folders),
/// and the master.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Section {
    Tracks,
    Buses,
    Master,
}

impl Section {
    pub const ALL: [Section; 3] = [Section::Tracks, Section::Buses, Section::Master];

    /// The id commands take (`spectrum.show {section}`).
    pub fn id(self) -> &'static str {
        match self {
            Section::Tracks => "tracks",
            Section::Buses => "buses",
            Section::Master => "master",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Section::Tracks => "Tracks",
            Section::Buses => "Buses",
            Section::Master => "Master",
        }
    }

    fn parse(id: &str) -> Option<Section> {
        Section::ALL.into_iter().find(|s| s.id() == id)
    }
}

/// Which section of the list `c` is in.
pub fn section_of(s: &Session, c: Curve) -> Section {
    match c {
        Curve::Master => Section::Master,
        Curve::Track(t) => {
            if s.track(t).is_some_and(|t| matches!(t.kind, TrackKind::Aux | TrackKind::Folder)) {
                Section::Buses
            } else {
                Section::Tracks
            }
        }
    }
}

/// The curves of section `sec`, in list order.
pub fn section_curves(s: &Session, sec: Section) -> Vec<Curve> {
    let mut v: Vec<Curve> = listed(s).map(|t| Curve::Track(t.id)).filter(|c| section_of(s, *c) == sec).collect();
    if sec == Section::Master {
        v.push(Curve::Master);
    }
    v
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "spectrum.show",
            "Show in Spectrum",
            [],
            None,
            "{track? | tracks? | master?: true | section?: \"tracks\"|\"buses\"|\"master\", shown?: bool} → {shown, curves: n, capped?}. Shows (true) or hides (false) these curves in the Spectrum editor; without `shown` it flips the first one's state and gives it to every one named. A section shows as many of its curves as the cap leaves room for (`capped` then true). Without `track`/`tracks`/`master`/`section`, the selected tracks. Saved in the session; not an undo step (a view setting).",
            always,
            show_cmd
        ),
        cmd!(
            noundo "spectrum.show_all",
            "Show All in Spectrum",
            [],
            None,
            "{} → {curves: n}. Shows every track's curve (and the master's) in the Spectrum editor, and stops it following the selection. Not an undo step.",
            always,
            |e, _| set_all(e, true)
        ),
        cmd!(
            noundo "spectrum.hide_all",
            "Hide All in Spectrum",
            [],
            None,
            "{} → {curves: n}. Hides every curve in the Spectrum editor, and stops it following the selection. Not an undo step.",
            always,
            |e, _| set_all(e, false)
        ),
        cmd!(
            noundo "spectrum.only_selected",
            "Spectrum: Only Selected Tracks",
            [],
            None,
            "{on?: bool} → {on}. The Spectrum editor shows only the tracks selected in the Edit window (and follows the selection), whatever each track's own switch says; off, the switches decide again. Without `on` it flips. Not an undo step.",
            always,
            |e, p| flag_cmd(e, p, "spectrum.only_selected", ONLY_SELECTED)
        ),
        cmd!(
            noundo "spectrum.scope",
            "Spectrum: Stereo Scope",
            [],
            None,
            "{on?: bool} → {on}. Shows (or hides) the Spectrum editor's stereo scope beside the graph: each shown track's left and right as a vectorscope, in its curve's colour and line. Without `on` it flips. Not an undo step.",
            always,
            |e, p| flag_cmd(e, p, "spectrum.scope", SCOPE)
        ),
    ]
}

/// The tracks the editor lists, in session order: those that put audio into the mix (not VCAs,
/// video tracks or Master Faders: the master is its own row).
pub fn listed(s: &Session) -> impl Iterator<Item = &Track> {
    s.tracks.iter().filter(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Aux | TrackKind::Midi | TrackKind::Instrument | TrackKind::Folder))
}

fn key(c: Curve, on: bool) -> String {
    let state = if on { "shown" } else { "hidden" };
    match c {
        Curve::Track(t) => format!("spectrum.{state}.{}", t.0),
        Curve::Master => format!("spectrum.{state}.master"),
    }
}

/// Shown unless switched off: the tracks that make sounds. Busses (Aux inputs, routing folders) and
/// the master start hidden, so the first look is at the sources.
fn shown_by_default(s: &Session, c: Curve) -> bool {
    match c {
        Curve::Master => false,
        Curve::Track(t) => s.track(t).is_some_and(|t| matches!(t.kind, TrackKind::Audio | TrackKind::Midi | TrackKind::Instrument)),
    }
}

/// The curve's own switch (what the list's toggle shows), ignoring Only Selected.
pub fn switched_on(s: &Session, c: Curve) -> bool {
    if s.edit.flag(&key(c, true)) {
        true
    } else if s.edit.flag(&key(c, false)) {
        false
    } else {
        shown_by_default(s, c)
    }
}

/// Whether the editor draws the curve now: with Only Selected on, a selected track (never the
/// master); otherwise its own switch.
pub fn visible(s: &Session, c: Curve) -> bool {
    if s.edit.flag(ONLY_SELECTED) {
        return matches!(c, Curve::Track(t) if s.edit.selected_tracks.contains(&t));
    }
    switched_on(s, c)
}

/// Every curve whose own switch is on (or, with Only Selected, every selected track), in list
/// order: tracks as listed, then the master. May be more than [`MAX_VISIBLE`].
pub fn wanted(s: &Session) -> Vec<Curve> {
    let mut v: Vec<Curve> = listed(s).map(|t| Curve::Track(t.id)).filter(|c| visible(s, *c)).collect();
    if visible(s, Curve::Master) {
        v.push(Curve::Master);
    }
    v
}

/// The curves the editor draws: [`wanted`]'s first [`MAX_VISIBLE`].
pub fn drawn(s: &Session) -> Vec<Curve> {
    let mut v = wanted(s);
    v.truncate(MAX_VISIBLE);
    v
}

/// Curves whose own switch is on, Only Selected aside.
fn switched_count(s: &Session) -> usize {
    listed(s).filter(|t| switched_on(s, Curve::Track(t.id))).count() + usize::from(switched_on(s, Curve::Master))
}

fn set(s: &mut Session, c: Curve, on: bool) {
    s.edit.set_flag(&key(c, !on), false);
    if on == shown_by_default(s, c) {
        // Back at its default: no flag (a session keeps only what differs).
        s.edit.set_flag(&key(c, on), false);
    } else {
        s.edit.set_flag(&key(c, on), true);
    }
}

fn show_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "spectrum.show";
    if let Some(v) = p.get("section").filter(|v| !v.is_null()) {
        let sec = v.as_str().and_then(Section::parse).ok_or_else(|| bad(C, "`section` must be \"tracks\", \"buses\" or \"master\""))?;
        return show_section(e, sec, p);
    }
    let mut curves: Vec<Curve> = Vec::new();
    match p.get("master") {
        None | Some(Value::Null) => {}
        Some(v) => {
            if !v.as_bool().ok_or_else(|| bad(C, "`master` must be a boolean"))? {
                return Err(bad(C, "`master` can only be true"));
            }
            curves.push(Curve::Master);
        }
    }
    if curves.is_empty() || p.get("track").is_some() || p.get("tracks").is_some() {
        curves.extend(tracks_required(e, C, p)?.into_iter().map(Curve::Track));
    }
    let on = match p.get("shown") {
        None | Some(Value::Null) => !curves.first().is_some_and(|c| switched_on(e.session(), *c)),
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`shown` must be a boolean"))?,
    };
    if on {
        let s = e.session();
        let more = curves.iter().filter(|c| !switched_on(s, **c)).count();
        if more > 0 && switched_count(s) + more > MAX_VISIBLE {
            return Err(bad(C, cap_reason()));
        }
    }
    let s = e.session_mut();
    for c in &curves {
        set(s, *c, on);
    }
    Ok(json!({"shown": on, "curves": curves.len()}))
}

/// A section's show or hide: hiding hides all of it; showing switches its curves on, in order, as
/// far as the cap allows. Without `shown`, shows unless every one is on already.
fn show_section(e: &mut Engine, sec: Section, p: &Value) -> Result<Value> {
    let curves = section_curves(e.session(), sec);
    let on = match p.get("shown") {
        None | Some(Value::Null) => !curves.iter().all(|c| switched_on(e.session(), *c)),
        Some(v) => v.as_bool().ok_or_else(|| bad("spectrum.show", "`shown` must be a boolean"))?,
    };
    let mut room = MAX_VISIBLE.saturating_sub(switched_count(e.session()));
    let mut capped = false;
    let s = e.session_mut();
    let mut n = 0;
    for c in &curves {
        if !on {
            set(s, *c, false);
            continue;
        }
        if switched_on(s, *c) {
            n += 1;
        } else if room > 0 {
            set(s, *c, true);
            room -= 1;
            n += 1;
        } else {
            capped = true;
        }
    }
    s.edit.set_flag(ONLY_SELECTED, false);
    Ok(json!({"shown": on, "curves": if on { n } else { 0 }, "capped": capped}))
}

/// Show All switches on the first [`MAX_VISIBLE`] curves in list order (the rest off); Hide All
/// switches every one off.
fn set_all(e: &mut Engine, on: bool) -> Result<Value> {
    let s = e.session_mut();
    let mut curves: Vec<Curve> = listed(s).map(|t| Curve::Track(t.id)).collect();
    curves.push(Curve::Master);
    let total = curves.len();
    for (i, c) in curves.iter().enumerate() {
        set(s, *c, on && i < MAX_VISIBLE);
    }
    s.edit.set_flag(ONLY_SELECTED, false);
    Ok(json!({"curves": if on { total.min(MAX_VISIBLE) } else { 0 }, "capped": on && total > MAX_VISIBLE}))
}

fn flag_cmd(e: &mut Engine, p: &Value, cmd: &str, flag: &str) -> Result<Value> {
    let on = match p.get("on") {
        None | Some(Value::Null) => !e.session().edit.flag(flag),
        Some(v) => v.as_bool().ok_or_else(|| bad(cmd, "`on` must be a boolean"))?,
    };
    e.session_mut().edit.set_flag(flag, on);
    Ok(json!({"on": on}))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engine() -> (Engine, TrackId, TrackId, TrackId) {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2, "format": "mono"})).unwrap();
        e.execute("track.new", &json!({"kind": "aux", "format": "stereo"})).unwrap();
        let ids: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        (e, ids[0], ids[1], ids[2])
    }

    #[test]
    fn tracks_show_by_default_busses_and_master_do_not() {
        let (e, a, _, aux) = engine();
        let s = e.session();
        assert_eq!(s.track(aux).map(|t| t.kind), Some(TrackKind::Aux));
        assert!(visible(s, Curve::Track(a)));
        assert!(!visible(s, Curve::Track(aux)));
        assert!(!visible(s, Curve::Master));
        assert_eq!(listed(s).count(), 3);
    }

    #[test]
    fn show_hide_flip_and_save() {
        let (mut e, a, b, aux) = engine();
        let r = e.execute("spectrum.show", &json!({"track": a.0})).unwrap();
        assert_eq!(r["shown"], false);
        assert!(!visible(e.session(), Curve::Track(a)));
        e.execute("spectrum.show", &json!({"tracks": [aux.0], "shown": true})).unwrap();
        e.execute("spectrum.show", &json!({"master": true, "shown": true})).unwrap();
        assert!(visible(e.session(), Curve::Track(aux)) && visible(e.session(), Curve::Master));
        assert!(visible(e.session(), Curve::Track(b)));
        assert!(e.execute("spectrum.show", &json!({"track": a.0, "shown": "yes"})).is_err());
        assert!(e.execute("spectrum.show", &json!({"master": false})).is_err());
        // Saved with the session.
        let text = serde_json::to_string(e.session()).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert!(!visible(&back, Curve::Track(a)) && visible(&back, Curve::Track(aux)) && visible(&back, Curve::Master));
        // Back to its default leaves no flag behind.
        e.execute("spectrum.show", &json!({"track": a.0, "shown": true})).unwrap();
        assert!(!e.session().edit.flags.iter().any(|f| f.ends_with(&format!(".{}", a.0))));
        // Not an undo step.
        assert!(e.undo_label().is_none_or(|l| !l.contains("Spectrum")), "{:?}", e.undo_label());
    }

    #[test]
    fn all_and_only_selected() {
        let (mut e, a, b, aux) = engine();
        e.execute("spectrum.hide_all", &json!({})).unwrap();
        assert!(!visible(e.session(), Curve::Track(a)) && !visible(e.session(), Curve::Master));
        e.execute("spectrum.show_all", &json!({})).unwrap();
        assert!(visible(e.session(), Curve::Track(aux)) && visible(e.session(), Curve::Master));
        e.execute("edit.select", &json!({"tracks": [b.0]})).unwrap();
        assert_eq!(e.execute("spectrum.only_selected", &json!({})).unwrap()["on"], true);
        let s = e.session();
        assert!(visible(s, Curve::Track(b)) && !visible(s, Curve::Track(a)) && !visible(s, Curve::Master));
        // The switches are kept underneath.
        assert!(switched_on(s, Curve::Track(a)));
        // Show All ends following the selection.
        e.execute("spectrum.show_all", &json!({})).unwrap();
        assert!(!e.session().edit.flag(ONLY_SELECTED));
    }

    #[test]
    fn sections_show_and_hide_together() {
        let (mut e, a, b, aux) = engine();
        let s = e.session();
        assert_eq!(section_curves(s, Section::Tracks), vec![Curve::Track(a), Curve::Track(b)]);
        assert_eq!(section_curves(s, Section::Buses), vec![Curve::Track(aux)]);
        assert_eq!(section_curves(s, Section::Master), vec![Curve::Master]);
        // Buses start hidden: one click shows them all.
        let r = e.execute("spectrum.show", &json!({"section": "buses"})).unwrap();
        assert_eq!((r["shown"].as_bool(), r["curves"].as_u64()), (Some(true), Some(1)));
        assert!(visible(e.session(), Curve::Track(aux)));
        e.execute("spectrum.show", &json!({"section": "tracks", "shown": false})).unwrap();
        assert!(!visible(e.session(), Curve::Track(a)) && !visible(e.session(), Curve::Track(b)));
        // Flipping a section that is all off shows it.
        assert_eq!(e.execute("spectrum.show", &json!({"section": "tracks"})).unwrap()["shown"], true);
        assert!(e.execute("spectrum.show", &json!({"section": "drums"})).is_err());
    }

    #[test]
    fn at_most_max_visible_curves_are_switched_on() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": MAX_VISIBLE + 6, "format": "mono"})).unwrap();
        let ids: Vec<TrackId> = e.session().tracks.iter().map(|t| t.id).collect();
        // New tracks default to shown: more than the cap want a curve, the first ones are drawn.
        assert_eq!(wanted(e.session()).len(), MAX_VISIBLE + 6);
        assert_eq!(drawn(e.session()), ids.iter().take(MAX_VISIBLE).map(|t| Curve::Track(*t)).collect::<Vec<_>>());
        let r = e.execute("spectrum.show_all", &json!({})).unwrap();
        assert_eq!((r["curves"].as_u64(), r["capped"].as_bool()), (Some(MAX_VISIBLE as u64), Some(true)));
        assert_eq!(wanted(e.session()).len(), MAX_VISIBLE);
        // One more is refused, with the reason; hiding one makes room.
        let last = ids[MAX_VISIBLE + 2];
        let err = e.execute("spectrum.show", &json!({"track": last.0, "shown": true})).unwrap_err();
        assert!(err.to_string().contains("at most 24"), "{err}");
        assert!(e.execute("spectrum.show", &json!({"master": true, "shown": true})).is_err());
        e.execute("spectrum.show", &json!({"track": ids[0].0, "shown": false})).unwrap();
        e.execute("spectrum.show", &json!({"track": last.0, "shown": true})).unwrap();
        assert!(drawn(e.session()).contains(&Curve::Track(last)));
    }
}

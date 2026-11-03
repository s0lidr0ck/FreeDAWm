//! Mix window view state kept with the session: whether a vocal strip's PITCH box is expanded to
//! the full Pitch panel or shows the compact box (`docs/superpowers/specs/2026-11-02-mix-view-tweaks.md`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::Session;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
        noundo "mix.pitch_expanded",
        "Expand Pitch Panel",
        [],
        None,
        "{track? | tracks?, expanded?: bool} → {expanded, tracks: n}. A vocal strip's PITCH box in the Mix window: the full Pitch panel (true) or the compact box (false). Without `expanded` it flips the first track's state and gives it to every track named. Without `track`/`tracks`, the selected tracks. Saved in the session; not an undo step (a view setting).",
        has_selection,
        pitch_expanded_cmd
    )]
}

/// The session flag holding track `t`'s expanded state.
fn key(t: TrackId) -> String {
    format!("mix.pitch_expanded.{}", t.0)
}

/// Whether track `t`'s PITCH box shows the full Pitch panel (else the compact box).
pub fn pitch_expanded(s: &Session, t: TrackId) -> bool {
    s.edit.flag(&key(t))
}

fn pitch_expanded_cmd(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.pitch_expanded";
    let tracks = tracks_required(e, C, p)?;
    let on = match p.get("expanded") {
        None | Some(Value::Null) => !tracks.first().is_some_and(|t| pitch_expanded(e.session(), *t)),
        Some(v) => v.as_bool().ok_or_else(|| bad(C, "`expanded` must be a boolean"))?,
    };
    let s = e.session_mut();
    for t in &tracks {
        s.edit.set_flag(&key(*t), on);
    }
    Ok(json!({"expanded": on, "tracks": tracks.len()}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn expanded_is_kept_per_track_and_flips_without_a_value() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"count": 2, "format": "mono"})).unwrap();
        let (a, b) = (e.session().tracks[0].id, e.session().tracks[1].id);
        assert!(!pitch_expanded(e.session(), a));
        let r = e.execute("mix.pitch_expanded", &json!({"track": a.0})).unwrap();
        assert_eq!(r["expanded"], true);
        assert!(pitch_expanded(e.session(), a));
        assert!(!pitch_expanded(e.session(), b));
        e.execute("mix.pitch_expanded", &json!({"tracks": [a.0, b.0], "expanded": false})).unwrap();
        assert!(!pitch_expanded(e.session(), a));
        assert!(e.execute("mix.pitch_expanded", &json!({"track": a.0, "expanded": "yes"})).is_err());
        // Saved with the session.
        e.execute("mix.pitch_expanded", &json!({"track": b.0, "expanded": true})).unwrap();
        let text = serde_json::to_string(e.session()).unwrap();
        let back: Session = serde_json::from_str(&text).unwrap();
        assert!(pitch_expanded(&back, b));
    }
}

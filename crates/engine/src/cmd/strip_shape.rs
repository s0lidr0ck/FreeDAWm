//! The strip's stereo shape and time: width and balance for stereo tracks, and a per-track delay
//! (Pro Tools' and Cubase's channel delay). The mix applies them in `process_strip` and
//! `route_strip` (`soundcraft_mix`): the delay before the inserts, the width before the pans,
//! the balance after them.

use super::*;
use crate::cmd;
use serde_json::{Value, json};
use soundcraft_model::MAX_TRACK_DELAY;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "mix.width",
            "Stereo Width",
            [],
            None,
            "{tracks?, width: -1..1 — 1 as it comes, 0 mono, -1 the sides swapped; stereo tracks} → {width}",
            has_selection,
            width
        ),
        cmd!(
            "mix.balance",
            "Balance",
            [],
            None,
            "{tracks?, balance: -1..1 — after the pans, -1 only the left side, +1 only the right; stereo tracks} → {balance}",
            has_selection,
            balance
        ),
        cmd!(
            "mix.track_delay",
            "Track Delay",
            [],
            None,
            "{tracks?, samples?: 0..192000 | ms?: number} — delays the track's signal before its inserts; delay compensation leaves it in → {samples, ms}",
            has_selection,
            track_delay
        ),
    ]
}

fn number(p: &Value, key: &str) -> Option<f64> {
    p.get(key).and_then(Value::as_f64).filter(|v| v.is_finite())
}

/// The stereo tracks of `tracks` (their group's too, for `attr`), refusing when there is none.
fn stereo_targets(e: &Engine, c: &str, p: &Value, attr: &str) -> Result<Vec<TrackId>> {
    let mut out = Vec::new();
    for t in tracks_required(e, c, p)? {
        for id in super::mix::grouped_ids(e.session(), t, attr) {
            if !out.contains(&id) && e.session().track(id).is_some_and(|x| x.mixer.pan.len() == 2) {
                out.push(id);
            }
        }
    }
    if out.is_empty() {
        return Err(bad(c, "only stereo tracks have a width and a balance"));
    }
    Ok(out)
}

fn width(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.width";
    let v = number(p, "width").ok_or_else(|| bad(C, "`width` (number, -1..1) required"))?.clamp(-1.0, 1.0) as f32;
    for t in stereo_targets(e, C, p, "pan")? {
        if let Some(tr) = e.session_mut().track_mut(t) {
            tr.mixer.width = v;
        }
    }
    Ok(json!({"width": v}))
}

fn balance(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.balance";
    let v = number(p, "balance").ok_or_else(|| bad(C, "`balance` (number, -1..1) required"))?.clamp(-1.0, 1.0) as f32;
    for t in stereo_targets(e, C, p, "pan")? {
        if let Some(tr) = e.session_mut().track_mut(t) {
            tr.mixer.balance = v;
        }
    }
    Ok(json!({"balance": v}))
}

fn track_delay(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.track_delay";
    let sr = e.session().sample_rate.as_f64();
    let samples = match (number(p, "samples"), number(p, "ms")) {
        (Some(n), _) => n,
        (None, Some(ms)) => ms * sr / 1000.0,
        (None, None) => return Err(bad(C, "`samples` or `ms` required")),
    };
    let n = samples.round().clamp(0.0, f64::from(MAX_TRACK_DELAY)) as u32;
    let tracks = tracks_required(e, C, p)?;
    for t in &tracks {
        if let Some(tr) = e.session_mut().track_mut(*t) {
            tr.mixer.delay = n;
        }
    }
    Ok(json!({"samples": n, "ms": f64::from(n) * 1000.0 / sr}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    #[test]
    fn width_balance_and_delay_are_set_clamped_saved_and_undone() {
        let mut e = Engine::default();
        let st = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Pad"));
        let mono = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        assert_eq!(e.execute("mix.width", &json!({"track": st.0, "width": 0.5})).unwrap()["width"], 0.5);
        assert_eq!(e.execute("mix.balance", &json!({"track": st.0, "balance": -3})).unwrap()["balance"], -1.0);
        assert!(e.execute("mix.width", &json!({"track": mono.0, "width": 0.5})).is_err(), "a mono track has no width");
        let r = e.execute("mix.track_delay", &json!({"track": mono.0, "ms": 10})).unwrap();
        assert_eq!(r["samples"], 480);
        assert!(e.execute("mix.track_delay", &json!({"track": mono.0, "samples": 1e9})).unwrap()["samples"] == MAX_TRACK_DELAY);
        let saved = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&saved).unwrap();
        let m = &back.track(st).unwrap().mixer;
        assert_eq!((m.width, m.balance), (0.5, -1.0));
        assert_eq!(back.track(mono).unwrap().mixer.delay, MAX_TRACK_DELAY);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().track(mono).unwrap().mixer.delay, 480);
    }
}

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
            "mix.pan_link",
            "Link Pans",
            [],
            None,
            "{tracks?, link: off|link|mirror — a stereo track's two pans: moving one moves the other by as much, the same way (link) or the other (mirror)} → {link}",
            has_selection,
            pan_link
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
        cmd!(
            "mix.summing",
            "Summing",
            [],
            None,
            "{tracks?, model: off|console|tape, drive?: 0..1 (0.5)} — Mix Engine FX on a strip that sums a bus (an aux or a routing folder): every source passes through the model as it goes into that bus; off sums cleanly → {model, drive}",
            has_selection,
            summing
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
        e.automation_changed(t, soundcraft_model::AutoParam::Width);
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
        e.automation_changed(t, soundcraft_model::AutoParam::Balance);
    }
    Ok(json!({"balance": v}))
}

fn pan_link(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.pan_link";
    let link =
        p.get("link").and_then(Value::as_str).and_then(soundcraft_model::PanLink::parse).ok_or_else(|| bad(C, "`link` is off, link or mirror"))?;
    for t in stereo_targets(e, C, p, "pan")? {
        if let Some(tr) = e.session_mut().track_mut(t) {
            tr.mixer.pan_link = link;
        }
    }
    Ok(json!({"link": link.id()}))
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

fn summing(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "mix.summing";
    let model = p.get("model").and_then(Value::as_str).ok_or_else(|| bad(C, "`model` required: off, console or tape"))?;
    let model = match model.trim() {
        m if m.eq_ignore_ascii_case("off") => None,
        m => Some(soundcraft_model::SummingModel::parse(m).ok_or_else(|| bad(C, format!("unknown model `{m}` (off, console, tape)")))?),
    };
    let drive = number(p, "drive").map(|d| d.clamp(0.0, 1.0) as f32);
    let tracks = tracks_required(e, C, p)?;
    let s = e.session();
    // Only a strip that sums a bus: its input is one.
    let sums = |t: &TrackId| s.track(*t).is_some_and(|tr| matches!(tr.mixer.input, soundcraft_model::Route::Bus(_)));
    let targets: Vec<TrackId> = tracks.iter().copied().filter(sums).collect();
    if targets.is_empty() {
        return Err(bad(C, "only an aux or a routing folder sums a bus"));
    }
    let mut out = json!({"model": "off"});
    for t in targets {
        if let Some(tr) = e.session_mut().track_mut(t) {
            tr.mixer.summing = model.map(|m| {
                let was = tr.mixer.summing.map_or(0.5, |x| x.drive);
                soundcraft_model::Summing { model: m, drive: drive.unwrap_or(was) }
            });
            if let Some(sm) = tr.mixer.summing {
                out = json!({"model": sm.model.id(), "drive": sm.drive});
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn summing_is_set_on_a_strip_that_sums_a_bus_only() {
        let mut e = Engine::default();
        e.execute("track.new", &json!({"name": "Gtr"})).unwrap();
        let gtr = e.session().tracks[0].id;
        assert!(e.execute("mix.summing", &json!({"track": gtr.0, "model": "console"})).is_err(), "a track that sums no bus");
        let bus = e.session_mut().add_bus("Drums", soundcraft_model::ChannelFormat::Stereo);
        let aux = e.session_mut().add_track(soundcraft_model::TrackKind::Aux, soundcraft_model::ChannelFormat::Stereo, Some("Drums"));
        e.session_mut().track_mut(aux).unwrap().mixer.input = soundcraft_model::Route::Bus(bus);
        let r = e.execute("mix.summing", &json!({"track": aux.0, "model": "tape", "drive": 0.8})).unwrap();
        assert_eq!(r["model"], "tape");
        let sm = e.session().track(aux).unwrap().mixer.summing.unwrap();
        assert_eq!((sm.model, sm.drive), (soundcraft_model::SummingModel::Tape, 0.8));
        e.execute("mix.summing", &json!({"track": aux.0, "model": "console"})).unwrap();
        assert_eq!(e.session().track(aux).unwrap().mixer.summing.unwrap().drive, 0.8, "the drive stays");
        e.execute("mix.summing", &json!({"track": aux.0, "model": "off"})).unwrap();
        assert!(e.session().track(aux).unwrap().mixer.summing.is_none());
        assert!(e.execute("mix.summing", &json!({"track": aux.0, "model": "valve"})).is_err());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().track(aux).unwrap().mixer.summing.is_some());
    }
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

    /// Linked pans move together; mirrored ones move apart about the centre; the link is saved.
    #[test]
    fn linked_and_mirrored_pans_follow_each_other() {
        let mut e = Engine::default();
        let st = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Pad"));
        let mono = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let pans = |e: &Engine| e.session().track(st).unwrap().mixer.pan.clone();
        e.execute("mix.pan", &json!({"track": st.0, "pan": -0.8, "index": 0})).unwrap();
        assert_eq!(pans(&e), vec![-0.8, 1.0], "independent by default");
        assert_eq!(e.execute("mix.pan_link", &json!({"track": st.0, "link": "link"})).unwrap()["link"], "link");
        e.execute("mix.pan", &json!({"track": st.0, "pan": -0.5, "index": 0})).unwrap();
        let p = pans(&e);
        assert!((p[0] + 0.5).abs() < 1e-6 && (p[1] - 1.0).abs() < 1e-6, "the right one is at the edge: {p:?}");
        e.execute("mix.pan", &json!({"track": st.0, "pan": 0.6, "index": 1})).unwrap();
        let p = pans(&e);
        assert!((p[0] + 0.9).abs() < 1e-5 && (p[1] - 0.6).abs() < 1e-6, "linked: {p:?}");
        e.execute("mix.pan_link", &json!({"track": st.0, "link": "mirror"})).unwrap();
        e.execute("mix.pan", &json!({"track": st.0, "pan": -0.5, "index": 0})).unwrap();
        let p = pans(&e);
        assert!((p[0] + 0.5).abs() < 1e-6 && (p[1] - 0.2).abs() < 1e-5, "mirrored: {p:?}");
        assert!(e.execute("mix.pan_link", &json!({"track": mono.0, "link": "link"})).is_err(), "a mono track has one pan");
        assert!(e.execute("mix.pan_link", &json!({"track": st.0, "link": "sideways"})).is_err());
        let saved = serde_json::to_string(e.session()).unwrap();
        let back: soundcraft_model::Session = serde_json::from_str(&saved).unwrap();
        assert_eq!(back.track(st).unwrap().mixer.pan_link, soundcraft_model::PanLink::Mirror);
    }
}

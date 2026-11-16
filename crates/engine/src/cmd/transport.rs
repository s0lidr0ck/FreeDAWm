//! Transport commands. They update the session's stopped playhead and queue requests for the
//! host's audio engine (which may be absent when headless).

use super::*;
use crate::{TransportRequest, cmd};
use serde_json::json;

fn shuttle(e: &mut Engine, step: i8, speed: Option<f32>) -> Result<Value> {
    if e.transport.recording {
        return Err(bad("transport.shuttle", "not while recording"));
    }
    e.transport_requests.push(TransportRequest::Shuttle { step, speed });
    Ok(json!({}))
}

/// The edit flag that makes Varispeed keep the pitch.
pub const KEEP_PITCH: &str = "transport.varispeed_keep_pitch";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "transport.play", "Play", [], Some("Space"), "{from?: position}", always, |e, p| {
            // An explicit `from` plays from there, stopped or playing; without it Play starts
            // from the selection as the transport button does.
            match position_param(e, "transport.play", p, "from")? {
                Some(at) => e.transport_requests.push(TransportRequest::PlayRange { from: at.max(0), end: None, looped: None }),
                None => e.transport_requests.push(TransportRequest::Play),
            }
            Ok(json!({}))
        }),
        cmd!(noundo "transport.stop", "Stop", [], Some("Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::Stop); Ok(json!({})) }),
        cmd!(noundo "transport.toggle", "Play/Stop", [], Some("Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::TogglePlay); Ok(json!({})) }),
        // `countoff: false` starts the take at once even with Options › Count Off on (a remote
        // recorder following a booth whose own take has started).
        cmd!(noundo "transport.record", "Record", [], Some("Cmd+Space"), "{countoff?: bool}", always, |e, p| {
            let countoff = p.get("countoff").and_then(Value::as_bool).unwrap_or(true);
            e.transport_requests.push(TransportRequest::Record { countoff });
            Ok(json!({}))
        }),
        cmd!(noundo "transport.pause", "Pause", [], None, "{}", always, |e, _| { e.transport_requests.push(TransportRequest::Pause); Ok(json!({})) }),
        cmd!(noundo "transport.varispeed", "Varispeed...", [], None, "{speed?: 0.1..4 (1 or omitted: off)} — plays at this speed until turned off, the pitch moving with it", always, |e, p| {
            let speed = match p.get("speed") {
                None | Some(Value::Null) => None,
                Some(v) => Some(v.as_f64().filter(|x| x.is_finite() && (0.1..=4.0).contains(x)).ok_or_else(|| bad("transport.varispeed", "speed must be a number from 0.1 to 4"))?),
            };
            let speed = speed.filter(|x| (x - 1.0).abs() > 1e-6).map(|x| x as f32);
            e.transport_requests.push(TransportRequest::Varispeed(speed));
            Ok(json!({"speed": speed.unwrap_or(1.0)}))
        }),
        cmd!(noundo "transport.varispeed_keep_pitch", "Varispeed Keeps Pitch", [], None, "{value?: bool} (toggles when omitted) — Varispeed and Shuttle play at their speed with the pitch kept, rather than moving with it as tape does", always, |e, p| super::more_util::toggle_flag(e, p, KEEP_PITCH)),
        cmd!(noundo "transport.shuttle_forward", "Shuttle Forward", [], None, "{} — plays forwards at 1×; again doubles the speed (to 4×). Bind it to L for J/K/L shuttle", always, |e, _| shuttle(e, 1, None)),
        cmd!(noundo "transport.shuttle_back", "Shuttle Back", [], None, "{} — plays backwards at 1× (heard); again doubles the speed (to 8×). Bind it to J for J/K/L shuttle", always, |e, _| shuttle(e, -1, None)),
        cmd!(noundo "transport.shuttle_stop", "Shuttle Stop", [], None, "{} — stops a shuttle. Bind it to K for J/K/L shuttle", always, |e, _| shuttle(e, 0, None)),
        cmd!(noundo "transport.shuttle", "Shuttle", [], None, "{speed: -8..4} — plays at this speed from the playhead, backwards when negative (heard); 0 stops", always, |e, p| {
            let v = p.get("speed").and_then(Value::as_f64).filter(|x| x.is_finite() && (-8.0..=4.0).contains(x)).ok_or_else(|| bad("transport.shuttle", "speed must be a number from -8 to 4"))?;
            shuttle(e, 0, Some(v as f32))
        }),
        cmd!(noundo "transport.half_speed", "Half-Speed Playback", [], Some("Shift+Space"), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::HalfSpeed); Ok(json!({})) }),
        cmd!(noundo "transport.play_selection", "Play Edit", ["Edit", "Selection"], Some("Alt+["), "{}", always, |e, _| { e.transport_requests.push(TransportRequest::PlaySelection); Ok(json!({})) }),
        cmd!(noundo "transport.rtz", "Return to Zero", [], Some("Home"), "{}", always, |e, _| locate(e, 0)),
        cmd!(noundo "transport.go_to_end", "Go to End", [], Some("End"), "{}", always, |e, _| { let end = e.session().content_end(); locate(e, end) }),
        cmd!(noundo "transport.rewind", "Rewind", [], None, "{seconds?: 1}", always, |e, p| {
            let s = e.session();
            let d = s.sample_rate.samples(f64_or(p, "seconds", 1.0).clamp(0.0, 86_400.0));
            let at = (cur(e) - d).max(0);
            locate(e, at)
        }),
        cmd!(noundo "transport.fast_forward", "Fast Forward", [], None, "{seconds?: 1}", always, |e, p| {
            let s = e.session();
            let d = s.sample_rate.samples(f64_or(p, "seconds", 1.0).clamp(0.0, 86_400.0));
            let at = cur(e) + d;
            locate(e, at)
        }),
        cmd!(noundo "transport.locate", "Locate", [], None, "{at: position}", always, |e, p| {
            let at = position_param(e, "transport.locate", p, "at")?.ok_or_else(|| bad("transport.locate", "`at` required"))?;
            locate(e, at.max(0))
        }),
        cmd!(noundo "transport.status", "Transport Status", [], None, "{}", always, |e, _| Ok(json!(e.transport))),
    ]
}

fn cur(e: &Engine) -> i64 {
    if e.transport.playing { e.transport.position } else { e.session().edit.selection.start }
}

fn locate(e: &mut Engine, at: i64) -> Result<Value> {
    let s = e.session_mut();
    s.edit.playhead = at;
    if s.edit.selection.is_empty() || !s.edit.selection.contains(at) {
        s.edit.selection = soundcraft_time::Range::point(at);
    }
    e.transport_requests.push(TransportRequest::Locate(at));
    Ok(json!({"at": at}))
}

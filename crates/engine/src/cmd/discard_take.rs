//! `transport.discard_take`: stop recording and throw the take away (Pro Tools' Command-Period
//! while recording): no clip is placed and its files are deleted. Nothing to undo.
//! `transport.restart_take` does the same and records again at once.

use super::*;
use crate::{TransportRequest, cmd};
use serde_json::json;

/// The flag of Options › Recording › Lock Record: while a take records, a Stop is refused once
/// (the app says so) and a second Stop soon after stops it, so a stray key does not end a take.
pub const RECORD_LOCK: &str = "options.record_lock";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "options.record_lock", "Lock Record", ["Options"], None, "{value?: bool} (toggles when omitted) — while recording, Stop must be pressed twice", always, |e, p| super::more_util::toggle_flag(e, p, RECORD_LOCK)),
        cmd!(
            noundo "transport.discard_take",
            "Discard Take",
            [],
            Some("Cmd+Alt+."),
            "{} (while recording) → {}: stops and throws the take away, its files with it",
            recording,
            |e, _| {
                e.transport_requests.push(TransportRequest::DiscardTake);
                Ok(json!({}))
            }
        ),
        // Cubase's Restart and Logic's discard-and-record again: one step for "that was no good,
        // again". The new take starts as Record would start it (count-off and pre-roll included).
        cmd!(
            noundo "transport.restart_take",
            "Restart Take",
            [],
            Some("Cmd+Alt+Space"),
            "{} (while recording) → {}: throws the take away, its files with it, and records again from the start",
            recording,
            |e, _| {
                e.transport_requests.push(TransportRequest::RestartTake);
                Ok(json!({}))
            }
        ),
    ]
}

fn recording(e: &Engine) -> std::result::Result<(), String> {
    if e.transport.recording { Ok(()) } else { Err("nothing is recording".into()) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discard_take_asks_the_host_only_while_recording() {
        let mut e = Engine::default();
        assert!(e.execute("transport.discard_take", &json!({})).is_err());
        assert!(e.transport_requests.is_empty());
        e.transport.recording = true;
        e.execute("transport.discard_take", &json!({})).unwrap();
        assert_eq!(e.transport_requests, [TransportRequest::DiscardTake]);
        assert!(!e.can_undo(), "nothing to undo");
    }

    #[test]
    fn restart_take_asks_the_host_only_while_recording() {
        let mut e = Engine::default();
        assert!(e.execute("transport.restart_take", &json!({})).is_err());
        e.transport.recording = true;
        e.execute("transport.restart_take", &json!({})).unwrap();
        assert_eq!(e.transport_requests, [TransportRequest::RestartTake]);
    }
}

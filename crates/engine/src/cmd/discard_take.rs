//! `transport.discard_take`: stop recording and throw the take away (Pro Tools' Command-Period
//! while recording): no clip is placed and its files are deleted. Nothing to undo.

use super::*;
use crate::{TransportRequest, cmd};
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![cmd!(
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
    )]
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
}

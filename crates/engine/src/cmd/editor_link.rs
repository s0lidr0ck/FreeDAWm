//! The editors' Link (spec 2026-11-04-editor-timeline-link): a linked Vocal or MIDI Editor
//! follows the playhead itself. On by default; turning it off is kept in the session's view
//! state as a flag (`vocal_editor.unlinked`, `midi_editor.unlinked`), so sessions without the
//! flag open linked.

use super::*;
use crate::cmd;
use serde_json::json;

/// The flag set while the Vocal Editor is unlinked.
pub const VOCAL_UNLINKED: &str = "vocal_editor.unlinked";
/// The flag set while the MIDI Editor is unlinked.
pub const MIDI_UNLINKED: &str = "midi_editor.unlinked";

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(noundo "view.vocal_editor_link", "Link Vocal Editor to Playhead", [], None, "{value?: bool} (toggles when omitted)", always, |e, p| set(e, p, VOCAL_UNLINKED)),
        cmd!(noundo "view.midi_editor_link", "Link MIDI Editor to Playhead", [], None, "{value?: bool} (toggles when omitted)", always, |e, p| set(e, p, MIDI_UNLINKED)),
    ]
}

/// Whether the editor whose unlinked flag is `flag` is linked.
pub fn linked(s: &soundcraft_model::Session, flag: &str) -> bool {
    !s.edit.flag(flag)
}

fn set(e: &mut Engine, p: &Value, flag: &str) -> Result<Value> {
    let s = e.session_mut();
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!linked(s, flag));
    s.edit.set_flag(flag, !on);
    Ok(json!({"linked": on}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editors_start_linked_and_toggle_apart() {
        let mut e = Engine::default();
        assert!(linked(e.session(), VOCAL_UNLINKED) && linked(e.session(), MIDI_UNLINKED));
        assert_eq!(e.execute("view.vocal_editor_link", &json!({})).unwrap()["linked"], false);
        assert!(!linked(e.session(), VOCAL_UNLINKED));
        assert!(linked(e.session(), MIDI_UNLINKED), "each editor has its own");
        e.execute("view.midi_editor_link", &json!({"value": false})).unwrap();
        e.execute("view.vocal_editor_link", &json!({"value": true})).unwrap();
        assert!(linked(e.session(), VOCAL_UNLINKED) && !linked(e.session(), MIDI_UNLINKED));
    }
}

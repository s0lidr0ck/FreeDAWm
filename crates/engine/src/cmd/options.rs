//! Options menu toggles and Setup › Session settings.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{BitDepthSetting, EditState};
use soundcraft_time::FrameRate;

macro_rules! toggle {
    ($id:literal, $label:literal, [$($m:literal),*], $sc:expr, $field:ident) => {
        cmd!(noundo $id, $label, [$($m),*], $sc, "{value?: bool}", always, |e, p| {
            let s = e.session_mut();
            s.edit.$field = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.$field);
            Ok(json!({"value": s.edit.$field}))
        })
    };
}

pub fn specs() -> Vec<CommandSpec> {
    vec![
        toggle!("options.loop_record", "Loop Record", ["Options"], Some("Alt+L"), loop_record),
        toggle!("options.quickpunch", "QuickPunch", ["Options"], Some("Cmd+Shift+P"), quickpunch),
        toggle!("options.pre_post_roll", "Pre/Post-Roll", ["Options"], Some("Cmd+K"), pre_post_roll),
        toggle!("options.loop_playback", "Loop Playback", ["Options"], Some("Cmd+Shift+L"), loop_playback),
        toggle!("options.link_timeline_edit", "Link Timeline and Edit Selection", ["Options"], Some("Shift+/"), link_timeline_edit),
        toggle!("options.link_track_edit", "Link Track and Edit Selection", ["Options"], None, link_track_edit),
        toggle!("options.insertion_follows_playback", "Insertion Follows Playback", ["Options"], None, insertion_follows_playback),
        toggle!("options.tab_to_transient", "Tab to Transient", ["Options"], Some("Cmd+Alt+Tab"), tab_to_transient),
        toggle!("options.mirrored_midi", "Mirrored MIDI Editing", ["Options"], None, mirrored_midi),
        toggle!("options.automation_follows_edit", "Automation Follows Edit", ["Options"], None, automation_follows_edit),
        toggle!("options.markers_follow_edit", "Markers Follow Edit", ["Options"], None, markers_follow_edit),
        toggle!("options.layered_editing", "Layered Editing", ["Options"], None, layered_editing),
        toggle!("options.click", "Click", ["Options"], None, click),
        toggle!("options.countoff", "Count Off", [], None, countoff),
        toggle!("options.midi_merge", "MIDI Merge", [], None, midi_merge),
        toggle!("options.pre_fader_metering", "Pre-Fader Metering", ["Options"], None, pre_fader_metering),
        toggle!("options.delay_compensation", "Delay Compensation", ["Options"], None, delay_compensation),
        // Short ramps on clip edges without a fade of their own (and across the loop's edges
        // while looping), so a cut never clicks. On by default at `AUTO_FADE_MS`.
        cmd!(noundo "options.auto_fades", "Automatic Edge Fades", ["Options"], None, "{value?: bool, ms?: 0.1..100}", always, |e, p| {
            let s = e.session_mut();
            let now = s.edit.value("playback.auto_fade_ms", soundcraft_mix::AUTO_FADE_MS);
            let ms = match (p.get("ms").and_then(Value::as_f64), p.get("value").and_then(Value::as_bool)) {
                (Some(ms), _) if ms.is_finite() => ms.clamp(0.1, 100.0),
                (Some(_), _) => return Err(bad("options.auto_fades", "`ms` must be a number")),
                (None, Some(false)) => 0.0,
                (None, Some(true)) => s.edit.value("playback.auto_fade_ms_on", soundcraft_mix::AUTO_FADE_MS),
                (None, None) if now > 0.0 => 0.0,
                (None, None) => s.edit.value("playback.auto_fade_ms_on", soundcraft_mix::AUTO_FADE_MS),
            };
            if ms > 0.0 {
                // The length to come back to when it is turned on again.
                s.edit.values.insert("playback.auto_fade_ms_on".into(), ms);
            }
            s.edit.values.insert("playback.auto_fade_ms".into(), ms);
            Ok(json!({"value": ms > 0.0, "ms": ms}))
        }),
        cmd!(
            noundo "options.record_mode",
            "Record Mode",
            ["Options", "Record Mode"],
            None,
            "{mode: replace|lanes|playlists}: what a take does to the audio it records over: replaces it, goes on a take lane (comped), or goes on a new playlist each take → {mode}",
            always,
            |e, p| {
                const C: &str = "options.record_mode";
                let mode = match p.get("mode").and_then(Value::as_str).map(str::to_ascii_lowercase).as_deref() {
                    Some("replace" | "normal") => "replace",
                    Some("lanes" | "takes" | "keep_takes") => "lanes",
                    Some("playlists" | "playlist") => "playlists",
                    Some(other) => return Err(bad(C, format!("unknown mode `{other}` (replace, lanes or playlists)"))),
                    None => return Err(bad(C, "`mode` required (replace, lanes or playlists)")),
                };
                e.session_mut().edit.record_mode = mode.into();
                Ok(json!({"mode": mode}))
            }
        ),
        cmd!(noundo "options.destructive_record", "Destructive Record", ["Options"], None, "{value?: bool}", always, |e, p| record_mode(e, p, "destructive")),
        cmd!(noundo "options.trackpunch", "TrackPunch", ["Options"], Some("Cmd+Shift+T"), "{value?: bool}", always, |e, p| record_mode(e, p, "trackpunch")),
        cmd!(noundo "options.scrolling", "Edit Window Scrolling", ["Options", "Edit Window Scrolling"], None, "{mode: none|after_playback|page|continuous|center}", always, |e, p| {
            let m = str_param(p, "mode").filter(|m| ["none", "after_playback", "page", "continuous", "center"].contains(m)).ok_or_else(|| bad("options.scrolling", "unknown mode"))?.to_string();
            e.session_mut().edit.scrolling = m.clone();
            Ok(json!({"mode": m}))
        }),
        cmd!(noundo "options.solo_mode", "Solo Mode", ["Options", "Solo Mode"], None, "{mode: sip|afl|pfl, latch?: bool, xor?: bool}", always, |e, p| {
            let m = str_param(p, "mode").unwrap_or("sip").to_string();
            let mut v = m.clone();
            if bool_or(p, "xor", false) { v.push_str("+xor") }
            e.session_mut().edit.solo_mode = v.clone();
            Ok(json!({"mode": v}))
        }),
        cmd!(noundo "options.keyboard_focus", "Keyboard Focus", [], None, "{focus?: commands|clips|groups|none} — toggles Commands Keyboard Focus when omitted", always, |e, p| {
            let s = e.session_mut();
            let f = match str_param(p, "focus") {
                Some(f @ ("commands" | "clips" | "groups" | "none")) => f.to_string(),
                Some(other) => return Err(bad("options.keyboard_focus", format!("unknown focus `{other}`"))),
                None => if s.edit.keyboard_focus == "commands" { "none".into() } else { "commands".into() },
            };
            s.edit.keyboard_focus = f.clone();
            Ok(json!({"focus": f}))
        }),
        cmd!(noundo "options.pre_roll", "Pre-Roll Amount", [], None, "{length: samples|{seconds}}", always, |e, p| {
            let v = position_param(e, "options.pre_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.pre_roll = v;
            Ok(json!({"pre_roll": v}))
        }),
        cmd!(noundo "options.post_roll", "Post-Roll Amount", [], None, "{length}", always, |e, p| {
            let v = position_param(e, "options.post_roll", p, "length")?.unwrap_or(0).max(0);
            e.session_mut().edit.post_roll = v;
            Ok(json!({"post_roll": v}))
        }),
        cmd!(
            "setup.session",
            "Session",
            ["Setup"],
            Some("Cmd+2"),
            "{frame_rate?: '29.97 Drop'…, bit_depth?: 16|24|32f, timecode_start?: position, name?}",
            always,
            session_setup
        ),
        cmd!(noundo "setup.preferences", "Preferences...", ["Setup"], None, "{}", always, |e, _| Ok(json!({"edit": e.session().edit.clone()}))),
        cmd!(
            noundo "setup.copy_imported_audio",
            "Copy Imported Audio into the Project",
            [],
            None,
            "{value?: bool}: whether saving copies imported audio into the project's Audio Files (default on; the original is never moved or deleted). Off, a file imported at the session rate stays where it is and the project refers to it. Not part of the session → {value}",
            always,
            |e, p| {
                e.copy_imported_audio = p.get("value").and_then(Value::as_bool).unwrap_or(!e.copy_imported_audio);
                Ok(json!({"value": e.copy_imported_audio}))
            }
        ),
        cmd!(noundo "setup.reset_edit_state", "Reset Edit Settings", [], None, "{}", always, |e, _| {
            let s = e.session_mut();
            let keep = (s.edit.selection, s.edit.selected_tracks.clone());
            s.edit = EditState::default();
            s.edit.selection = keep.0;
            s.edit.selected_tracks = keep.1;
            Ok(json!({}))
        }),
    ]
}

/// Destructive Record and TrackPunch: a stored flag nothing reads yet (FreeDAWm records
/// non-destructively, every track at once). Kept apart from `record_mode`, which they used to
/// overwrite.
fn record_mode(e: &mut Engine, p: &Value, mode: &str) -> Result<Value> {
    let s = e.session_mut();
    let key = format!("options.{mode}");
    let on = p.get("value").and_then(Value::as_bool).unwrap_or(!s.edit.flag(&key));
    s.edit.set_flag(&key, on);
    Ok(json!({"value": on}))
}

fn session_setup(e: &mut Engine, p: &Value) -> Result<Value> {
    let fr = match str_param(p, "frame_rate") {
        Some(f) => {
            Some(FrameRate::ALL.into_iter().find(|r| r.label().eq_ignore_ascii_case(f)).ok_or_else(|| bad("setup.session", "unknown frame rate"))?)
        }
        None => None,
    };
    let bd = match p.get("bit_depth").map(|v| v.to_string().trim_matches('"').to_ascii_lowercase()) {
        Some(b) if b == "16" => Some(BitDepthSetting::Int16),
        Some(b) if b == "24" => Some(BitDepthSetting::Int24),
        Some(b) if b.starts_with("32") => Some(BitDepthSetting::Float32),
        Some(_) => return Err(bad("setup.session", "bit_depth must be 16, 24 or 32f")),
        None => None,
    };
    let tc = position_param(e, "setup.session", p, "timecode_start")?;
    let s = e.session_mut();
    if let Some(f) = fr {
        s.frame_rate = f;
    }
    if let Some(b) = bd {
        s.bit_depth = b;
    }
    if let Some(t) = tc {
        s.timecode_start = t;
    }
    if let Some(n) = p.get("name").and_then(Value::as_str) {
        s.name = n.to_string();
    }
    Ok(json!({"frame_rate": s.frame_rate.label(), "bit_depth": s.bit_depth, "sample_rate": s.sample_rate.hz()}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    #[test]
    fn automatic_edge_fades_toggle_and_keep_their_length() {
        let mut e = Engine::default();
        let fades = |e: &Engine| soundcraft_mix::auto_fade_frames(e.session());
        assert_eq!(fades(&e), 48, "on by default, 1 ms at 48 kHz");
        assert_eq!(e.execute("options.auto_fades", &json!({})).unwrap()["value"], false);
        assert_eq!(fades(&e), 0);
        e.execute("options.auto_fades", &json!({"ms": 2.0})).unwrap();
        assert_eq!(fades(&e), 96);
        e.execute("options.auto_fades", &json!({"value": false})).unwrap();
        assert_eq!(e.execute("options.auto_fades", &json!({})).unwrap()["ms"], 2.0, "back on at the length it had");
        assert!(e.execute("options.auto_fades", &json!({"ms": "long"})).is_ok(), "a non-number `ms` is ignored, not stored");
    }
}

//! ARA plug-ins on clips (Clip › ARA Plug-in…): `clip.ara_add` gives the selected audio clips to
//! an ARA plug-in the VST3 scan found (it reads their audio and follows their edits, see
//! [`crate::ara`]); `clip.ara_remove` takes them back. One undo step each. Stage one: the clips
//! still play their own audio.

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::ClipAra;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.ara_add",
            "Add ARA Plug-in",
            [],
            None,
            "{clips?, plugin: \"vst3:<class id>\" (an ARA 2 plug-in the scan found)} — the audio clips are given to the ARA plug-in, which reads their audio from the session and follows their edits; they still play their own audio (playback through ARA is not built yet). Stretched, warped, reversed, looped or transposed clips are left out",
            ara_available,
            add
        ),
        cmd!("clip.ara_remove", "Remove ARA Plug-in", [], None, "{clips?} — the clips are taken back from their ARA plug-in", has_ara_clip, remove),
        cmd!(
            query "engine.ara_plugins",
            "List ARA Plug-ins",
            [],
            None,
            "{} → [{id, name, vendor, ara: {name, version, roles, content, …}}]: the VST3 plug-ins the last scan found to offer usable ARA 2 (never scans)",
            always,
            |_, _| Ok(json!(soundcraft_vst3_host::ara_plugins()))
        ),
    ]
}

/// Why no ARA plug-in can be added now. (Whether the plug-in is one the scan found is checked when
/// it runs; the menu item, Clip › ARA Plug-ins…, is greyed out until the scan finds one.)
pub fn ara_available(e: &Engine) -> std::result::Result<(), String> {
    if !selected_audio(e) {
        return Err("Select an audio clip".into());
    }
    Ok(())
}

fn selected_audio(e: &Engine) -> bool {
    clip_ids_param(e, &json!({})).iter().any(|id| e.session().find_clip(*id).is_some_and(|(_, c)| c.is_audio()))
}

fn has_ara_clip(e: &Engine) -> std::result::Result<(), String> {
    let any = clip_ids_param(e, &json!({})).iter().any(|id| e.session().find_clip(*id).is_some_and(|(_, c)| c.ara.is_some()));
    if any { Ok(()) } else { Err("Select a clip given to an ARA plug-in".into()) }
}

/// The plug-in's display name, if `id` is an ARA plug-in this host can use.
fn ara_plugin(id: &str) -> Option<String> {
    #[cfg(test)]
    if let Some(name) = id.strip_prefix("test:") {
        return Some(name.to_string());
    }
    soundcraft_vst3_host::ara_plugins().into_iter().find(|d| d.id == id).map(|d| d.ara.map(|a| a.name).filter(|n| !n.is_empty()).unwrap_or(d.name))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "clip.ara_add";
    let plugin = str_param(p, "plugin").ok_or_else(|| bad(C, "`plugin` required"))?.to_string();
    let name = ara_plugin(&plugin).ok_or_else(|| bad(C, format!("`{plugin}` is not an ARA 2 plug-in the scan found")))?;
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let (mut given, mut left_out) = (0, Vec::new());
    for id in ids {
        let Some(c) = s.find_clip_mut(id) else { continue };
        if let Some(why) = crate::ara::unsupported(c) {
            left_out.push(json!({"clip": id.0, "why": why}));
            continue;
        }
        c.ara = Some(ClipAra { plugin: plugin.clone(), name: name.clone() });
        given += 1;
    }
    if given == 0 {
        return Err(bad(
            C,
            "no audio clip that ARA can take is selected (stretched, warped, reversed, looped or transposed clips are not given to ARA plug-ins yet)",
        ));
    }
    e.message(format!("{name} reads {given} clip(s) through ARA; they still play their own audio (playback through ARA is not built yet)"));
    Ok(json!({"plugin": plugin, "name": name, "clips": given, "left_out": left_out}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    let ids = clip_ids_param(e, p);
    let s = e.session_mut();
    let mut n = 0;
    for id in ids {
        if let Some(c) = s.find_clip_mut(id)
            && c.ara.take().is_some()
        {
            n += 1;
        }
    }
    Ok(json!({"clips": n}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, SourceAudio, SourceId, TrackKind};
    use std::sync::Arc;

    fn engine_with_clip() -> (Engine, ClipId) {
        let mut s = Session::default();
        s.pool.insert(SourceId(910), Arc::new(SourceAudio::new(AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.25; 48_000]] })));
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let id = s.new_clip_id();
        s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(Clip::audio(id, "take", SourceId(910), 0, 0, 48_000));
        s.edit.selected_clips = vec![id];
        (Engine::new(s), id)
    }

    #[test]
    fn a_clip_is_given_to_an_ara_plug_in_and_taken_back_in_one_undo_step_each() {
        let (mut e, id) = engine_with_clip();
        assert!(e.execute("clip.ara_add", &json!({"plugin": "vst3:00000000000000000000000000000000"})).is_err(), "not an ARA plug-in");
        assert!(e.execute("clip.ara_add", &json!({})).is_err());
        let r = e.execute("clip.ara_add", &json!({"plugin": "test:Pitch Tool"})).unwrap();
        assert_eq!(r["clips"], 1);
        let ara = e.session().find_clip(id).unwrap().1.ara.clone().unwrap();
        assert_eq!((ara.plugin.as_str(), ara.name.as_str()), ("test:Pitch Tool", "Pitch Tool"));
        e.execute("clip.ara_remove", &json!({})).unwrap();
        assert!(e.session().find_clip(id).unwrap().1.ara.is_none());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(id).unwrap().1.ara.is_some());
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(e.session().find_clip(id).unwrap().1.ara.is_none());
    }

    #[test]
    fn a_reversed_clip_is_left_out() {
        let (mut e, id) = engine_with_clip();
        e.session_mut().find_clip_mut(id).unwrap().reversed = true;
        let err = e.execute("clip.ara_add", &json!({"plugin": "test:Pitch Tool"})).unwrap_err();
        assert!(err.to_string().contains("reversed"), "{err}");
    }

    #[test]
    fn old_sessions_have_no_ara_and_write_none() {
        let (e, id) = engine_with_clip();
        let text = serde_json::to_string(e.session().find_clip(id).unwrap().1).unwrap();
        assert!(!text.contains("ara"), "{text}");
    }
}

//! Clean Vocal's non-destructive result on a clip ([`soundcraft_model::CleanAlt`]): its Amount
//! (the cleaned share), the Original/Cleaned toggle, and Commit (render the mix into one source).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::ClipContent;

/// The audio Commit keeps on each side of the clip's window (for trims and fades), seconds.
pub const COMMIT_HANDLE_SECS: f64 = 1.0;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "clip.clean_mix",
            "Clean Vocal Amount",
            [],
            None,
            "{clip? | clips?, mix?: 0..100 (% cleaned; the original plays the rest), bypass?: bool (true plays the original, keeping the amount), toggle?: bool (flips bypass)} — clips cleaned by Clean Vocal (`ml.clean`) only; undoable (the UI merges a drag into one step)",
            has_selection,
            clean_mix
        ),
        cmd!(
            "clip.clean_toggle",
            "Original/Cleaned",
            ["Clip"],
            None,
            "{clip? | clips?} — flips Clean Vocal's Original/Cleaned toggle (the amount is kept)",
            has_selection,
            |e, p| {
                let mut q = p.clone();
                if let Some(o) = q.as_object_mut() {
                    o.insert("toggle".into(), json!(true));
                } else {
                    q = json!({"toggle": true});
                }
                clean_mix(e, &q)
            }
        ),
        cmd!(
            "clip.clean_remove",
            "Remove Cleaned Version",
            ["Clip"],
            None,
            "{clip? | clips?} — drops Clean Vocal's cleaned audio from the clip (it plays the original); for a cleaned file gone missing, or to start over",
            has_selection,
            |e, p| {
                let ids = cleaned(e, p);
                if ids.is_empty() {
                    return Err(bad("clip.clean_remove", "choose a clip cleaned by Clean Vocal"));
                }
                let s = e.session_mut();
                for id in &ids {
                    if let Some(c) = s.find_clip_mut(*id) {
                        c.clean = None;
                    }
                }
                Ok(json!({"clips": ids.iter().map(|i| i.0).collect::<Vec<_>>()}))
            }
        ),
        cmd!(
            "clip.clean_commit",
            "Commit Clean Vocal",
            ["Clip"],
            None,
            "{clip? | clips?} — renders a cleaned clip's current mix of original and cleaned audio over the clip's window plus 1 s handles each side into one new source (`<clip>-clean`; the clip's offset and lyrics move with it) and drops the alternate; at 0 % or bypassed it keeps the original",
            has_selection,
            commit
        ),
    ]
}

/// The cleaned clips among `p`'s clips (or the selection).
fn cleaned(e: &Engine, p: &Value) -> Vec<ClipId> {
    clip_ids_param(e, p).into_iter().filter(|id| e.session().find_clip(*id).is_some_and(|(_, c)| c.clean_alt().is_some())).collect()
}

fn clean_mix(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "clip.clean_mix";
    let ids = cleaned(e, p);
    if ids.is_empty() {
        return Err(bad(ID, "choose a clip cleaned by Clean Vocal"));
    }
    let mix = match p.get("mix") {
        None | Some(Value::Null) => None,
        Some(v) => match v.as_f64().filter(|m| m.is_finite() && (0.0..=100.0).contains(m)) {
            Some(m) => Some((m / 100.0) as f32),
            None => return Err(bad(ID, "`mix` must be a number from 0 to 100 (%)")),
        },
    };
    let bypass = p.get("bypass").and_then(Value::as_bool);
    let toggle = bool_or(p, "toggle", false);
    let s = e.session_mut();
    let mut out = Vec::new();
    for id in ids {
        if let Some(k) = s.find_clip_mut(id).and_then(|c| c.clean.as_mut()) {
            if let Some(m) = mix {
                k.mix = m;
            }
            if let Some(b) = bypass {
                k.bypass = b;
            }
            if toggle {
                k.bypass = !k.bypass;
            }
            out.push(json!({"clip": id.0, "mix": f64::from(k.amount()) * 100.0, "bypass": k.bypass}));
        }
    }
    Ok(json!({"clips": out}))
}

fn commit(e: &mut Engine, p: &Value) -> Result<Value> {
    const ID: &str = "clip.clean_commit";
    let ids = cleaned(e, p);
    if ids.is_empty() {
        return Err(bad(ID, "choose a clip cleaned by Clean Vocal"));
    }
    let rate = e.session().sample_rate.hz();
    let mut out = Vec::new();
    for id in ids {
        let Some(c) = e.session().find_clip(id).map(|(_, c)| c.clone()) else { continue };
        let Some(k) = c.clean_alt().copied() else { continue };
        let s = e.session_mut();
        if k.wet() <= 0.0 {
            if let Some(cm) = s.find_clip_mut(id) {
                cm.clean = None;
            }
            out.push(json!({"clip": id.0, "source": k.original.0}));
            continue;
        }
        // The clip's window of the original (in source samples), with a handle each side so the
        // clip can still be trimmed open and faded a little; clamped to the original.
        let total = Samples::try_from(s.pool.get(k.original).map_or(0, |a| a.frames())).unwrap_or(Samples::MAX);
        let handle = s.sample_rate.samples(COMMIT_HANDLE_SECS);
        let stretch = if c.stretch.is_finite() && c.stretch > 0.0 { c.stretch } else { 1.0 };
        let span = ((c.length as f64 / stretch).ceil() as Samples).saturating_add(1);
        let offset = c.source_offset();
        let a = offset.saturating_sub(handle).clamp(0, total);
        let b = offset.saturating_add(span).saturating_add(handle).clamp(a, total);
        let len = usize::try_from(b - a).unwrap_or(0);
        crate::io::refuse_longer(s, ID, &c.name, "render", b - a, crate::io::render_limit(2))?;
        let Some(audio) = soundcraft_mix::clip_source_window(s, &c, a, len) else { continue };
        let name = format!("{}-clean", c.name);
        let src = crate::io::add_source(s, &name, AudioBuffer { sample_rate: rate, channels: audio }, None, FileFormat::Wav);
        if let Some(cm) = s.find_clip_mut(id) {
            if let ClipContent::Audio { source, offset } = &mut cm.content {
                *source = src;
                *offset -= a;
            }
            // Lyrics are in the source's time: they move with the new file's start.
            soundcraft_model::rebase_lyrics(cm, a);
            cm.clean = None;
        }
        out.push(json!({"clip": id.0, "source": src.0}));
    }
    Ok(json!({"clips": out}))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, CleanAlt, Clip, Session, TrackKind};

    /// A mono track with one clip at 100 playing samples 10.. of a ramp, and a cleaned source of
    /// constant 1.0 covering original samples 10..60.
    fn cleaned_session() -> (Engine, ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        let rate = s.sample_rate.hz();
        let ramp: Vec<f32> = (0..100).map(|i| i as f32 / 100.0).collect();
        let orig = crate::io::add_source(&mut s, "Vox", AudioBuffer { sample_rate: rate, channels: vec![ramp] }, None, FileFormat::Wav);
        let clean =
            crate::io::add_source(&mut s, "Vox-clean", AudioBuffer { sample_rate: rate, channels: vec![vec![1.0; 50]] }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "Vox", orig, 10, 100, 50);
        c.clean = Some(CleanAlt { source: clean, original: orig, origin: 10, mix: 0.25, bypass: false });
        crate::edit::place_clip(&mut s, t, c);
        (Engine::new(s), id)
    }

    #[test]
    fn amount_toggle_and_commit() {
        let (mut e, id) = cleaned_session();
        let r = e.execute("clip.clean_mix", &json!({"clip": id.0, "mix": 60})).unwrap();
        assert_eq!(r["clips"][0]["mix"].as_f64().map(|m| m.round()), Some(60.0));
        assert!(e.execute("clip.clean_mix", &json!({"clip": id.0, "mix": 101})).is_err());
        e.execute("clip.clean_toggle", &json!({"clip": id.0})).unwrap();
        let k = e.session().find_clip(id).unwrap().1.clean.unwrap();
        assert!(k.bypass && (k.mix - 0.6).abs() < 1e-6 && k.wet() == 0.0, "bypass keeps the amount");
        assert!(e.undo());
        assert!(!e.session().find_clip(id).unwrap().1.clean.unwrap().bypass);
        // Commit at 60 %: one source, the original's length and time.
        e.execute("clip.clean_commit", &json!({"clip": id.0})).unwrap();
        let c = e.session().find_clip(id).unwrap().1.clone();
        assert!(c.clean.is_none());
        assert_eq!(c.source_offset(), 10);
        let buf = &e.session().pool.get(c.source().unwrap()).unwrap().buffer();
        assert_eq!(buf.frames(), 100);
        let want = |i: usize| 0.4 * (i as f32 / 100.0) + 0.6;
        assert!((buf.channels[0][20] - want(20)).abs() < 1e-6);
        assert!((buf.channels[0][5] - 0.05).abs() < 1e-6, "outside the cleaned window: the original");
        assert!(e.undo());
        assert!(e.session().find_clip(id).unwrap().1.clean.is_some());
        // Not a cleaned clip: refused.
        assert!(e.execute("clip.clean_mix", &json!({"clip": 9999, "mix": 10})).is_err());
    }
}

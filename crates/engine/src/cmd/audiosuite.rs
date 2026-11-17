//! Process: offline processing of selected clips (`process.run`; the old id `audiosuite.process`
//! still works through `COMMAND_ALIASES`).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, ClipId, ProcessStep, TrackId, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "process.history",
            "Process History",
            [],
            None,
            "{clip?} -> {clip, steps: [{step, process, params, bypassed}]}: the processes the clip's audio came through, oldest first",
            has_selection,
            history
        ),
        cmd!(
            "process.remove_step",
            "Remove Process",
            [],
            None,
            "{clip?, step} — takes the step out of the clip's history and makes the clip again from the audio before it, the later steps applied again in order",
            has_selection,
            |e, p| edit_history(e, p, "process.remove_step")
        ),
        cmd!(
            "process.bypass_step",
            "Bypass Process",
            [],
            None,
            "{clip?, step, value?: bool} (toggles when omitted) — the clip is made again without (or again with) the step, which stays in its history",
            has_selection,
            |e, p| edit_history(e, p, "process.bypass_step")
        ),
        cmd!(
            "process.move_step",
            "Move Process",
            [],
            None,
            "{clip?, step, to} — moves the step to place `to` in the clip's history and makes the clip again in the new order",
            has_selection,
            |e, p| edit_history(e, p, "process.move_step")
        ),
        cmd!(
            "process.run",
            "Process",
            [],
            None,
            "{process: <plugin id> | normalize | reverse | invert | gain | duplicate | time_stretch | pitch_shift | varispeed | signal_generator, params?: {..}, clips?, tracks?, start?, end?, handle_ms?: 1000} — signal_generator fills start..end (default: the selection) with a new clip on each audio track in `tracks` (default: the selected tracks); processes that keep the length render up to `handle_ms` of the file beyond each edge too, so the new clip can still be trimmed open",
            has_selection,
            process
        ),
    ]
}

/// The audio clip `clip` names, else the first selected one with a process history, else the
/// first selected audio clip.
fn history_clip(e: &Engine, p: &Value, id: &str) -> Result<(TrackId, Clip)> {
    let s = e.session();
    let ids: Vec<ClipId> = clip_ids_param(e, p);
    let audio = |c: &&ClipId| s.find_clip(**c).is_some_and(|(_, c)| c.is_audio());
    let pick = ids.iter().filter(audio).find(|c| s.find_clip(**c).is_some_and(|(_, c)| !c.processes.is_empty())).or_else(|| ids.iter().find(audio));
    let (t, c) = pick.and_then(|c| s.find_clip(*c)).ok_or_else(|| bad(id, "select an audio clip"))?;
    Ok((t, c.clone()))
}

fn history(e: &mut Engine, p: &Value) -> Result<Value> {
    let (_, c) = history_clip(e, p, "process.history")?;
    let steps: Vec<Value> = c
        .processes
        .iter()
        .enumerate()
        .map(|(i, x)| json!({"step": i, "process": x.process, "params": x.params.iter().map(|(k, v)| (k.clone(), json!(v))).collect::<serde_json::Map<_, _>>(), "bypassed": x.bypassed}))
        .collect();
    Ok(json!({"clip": c.id.0, "steps": steps}))
}

/// Remove, bypass or move one step of a clip's process history, and make the clip again from the
/// first step that changed: from the audio before that step (as the clip is trimmed and placed
/// now), every later step not bypassed applied again in the new order. One undo step.
fn edit_history(e: &mut Engine, p: &Value, id: &str) -> Result<Value> {
    let (track, clip) = history_clip(e, p, id)?;
    let old = clip.processes.clone();
    let step = p.get("step").and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).filter(|x| *x < old.len());
    let Some(k) = step else { return Err(bad(id, format!("`step` must be one of the clip's {} steps", old.len()))) };
    let mut new = old.clone();
    let from = match id {
        "process.remove_step" => {
            new.remove(k);
            k
        }
        "process.bypass_step" => {
            let Some(x) = new.get_mut(k) else { return Err(bad(id, "no such step")) };
            x.bypassed = p.get("value").and_then(Value::as_bool).unwrap_or(!x.bypassed);
            k
        }
        _ => {
            let to =
                p.get("to").and_then(Value::as_u64).and_then(|x| usize::try_from(x).ok()).ok_or_else(|| bad(id, "`to` required"))?.min(old.len() - 1);
            let x = new.remove(k);
            new.insert(to, x);
            k.min(to)
        }
    };
    if new == old {
        return Ok(json!({"clip": clip.id.0, "steps": old.len()}));
    }
    // The clip as it was before the first changed step, at today's edges. The steps that made
    // today's file from there (`made`, the ones not bypassed) each kept the timeline in place, so
    // the edges map straight back: each step's file is entered `lead` frames in, and a trim made
    // between two steps moved where the next one entered its input.
    let made: Vec<&ProcessStep> = old.iter().skip(from).filter(|x| !x.bypassed).collect();
    let base = match made.as_slice() {
        [] => {
            // Nothing from there on changed the file: today's clip is where to start.
            let mut c = clip.clone();
            c.processes.clear();
            c
        }
        [only] if !keeps_length(&only.process) => {
            // A length-changing step can't be mapped through; alone, the clip goes back to just
            // before it (its edges then).
            (*only.before).clone()
        }
        steps => {
            let plain = |c: &Clip| c.stretch == 1.0 && c.warp.is_empty() && c.loop_len == 0 && !c.reversed;
            if steps.iter().any(|x| !keeps_length(&x.process) || !plain(&x.before)) {
                return Err(bad(
                    id,
                    "a step after it changed the length (time stretch, pitch shift or varispeed) or ran on stretched audio: remove the later steps first",
                ));
            }
            let mut offset = steps.first().map_or(0, |x| x.before.source_offset());
            for w in steps.windows(2) {
                if let (Some(a), Some(b)) = (w.first(), w.get(1)) {
                    offset = offset.saturating_add(b.before.source_offset().saturating_sub(a.lead));
                }
            }
            offset = offset.saturating_add(clip.source_offset().saturating_sub(steps.last().map_or(0, |x| x.lead)));
            let mut c = steps.first().map_or_else(|| clip.clone(), |x| (*x.before).clone());
            if let ClipContent::Audio { offset: o, .. } = &mut c.content {
                *o = offset.max(0);
            }
            (c.start, c.length, c.fade_in, c.fade_out, c.muted) = (clip.start, clip.length, clip.fade_in, clip.fade_out, clip.muted);
            c
        }
    };
    // Every step before `from` is unchanged; from there each is applied again (or kept, bypassed).
    let mut steps: Vec<ProcessStep> = new.get(..from).unwrap_or_default().to_vec();
    let mut cur = base;
    for mut x in new.into_iter().skip(from) {
        let mut before = cur.clone();
        before.processes.clear();
        if !x.bypassed {
            let Some((nc, lead)) = run_step(e, track, &cur, &x.process, &x.params, x.handle_ms)? else {
                return Err(bad(id, format!("`{}` made no audio", x.process)));
            };
            x.lead = lead;
            cur = nc;
        }
        x.before = Box::new(before);
        steps.push(x);
    }
    // A clip made again keeps the clip gain it has now (set after the steps, or kept by them).
    if steps.iter().skip(from).any(|x| !x.bypassed) {
        (cur.gain_db, cur.gain_env) = (clip.gain_db, clip.gain_env.clone());
    }
    cur.processes = steps;
    let n = cur.processes.len();
    let s = e.session_mut();
    if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
        pl.clips.retain(|c| c.id != clip.id);
    }
    crate::edit::place_clip(s, track, cur);
    Ok(json!({"clip": clip.id.0, "steps": n}))
}

/// Process each target clip's audio and replace the clip with one backed by a new source.
fn process(e: &mut Engine, p: &Value) -> Result<Value> {
    let name = str_param(p, "process").or_else(|| str_param(p, "plugin")).ok_or_else(|| bad("process.run", "`process` required"))?.to_string();
    let params: Vec<(String, f32)> = p
        .get("params")
        .and_then(Value::as_object)
        .map(|o| o.iter().filter_map(|(k, v)| v.as_f64().filter(|x| x.is_finite()).map(|x| (k.clone(), x as f32))).collect())
        .unwrap_or_default();
    let known = soundcraft_dsp::plugin_info(&name).is_some()
        || matches!(
            name.as_str(),
            "normalize" | "reverse" | "invert" | "gain" | "duplicate" | "time_stretch" | "pitch_shift" | "varispeed" | "dc_offset_removal"
        );
    if !known {
        return Err(bad("process.run", format!("unknown process `{name}`")));
    }
    if soundcraft_dsp::is_generator(&name) && p.get("clips").is_none() && p.get("clip").is_none() {
        let r = range_param(e, "process.run", p)?;
        if !r.is_empty() {
            return generate(e, p, &name, &params, r);
        }
    }
    let ids = clip_ids_param(e, p);
    if ids.is_empty() {
        return Err(bad("process.run", "select audio clips first"));
    }
    // Processes that keep the length also process the material beyond the clip's edges (handles),
    // up to this much each side, so the new clip can still be trimmed open.
    let handle_ms = p.get("handle_ms").and_then(Value::as_f64).filter(|x| x.is_finite()).unwrap_or(1000.0).clamp(0.0, 60_000.0);
    let mut done = 0;
    for id in ids {
        let Some((track, clip)) = e.session().find_clip(id).map(|(t, c)| (t, c.clone())) else { continue };
        if !clip.is_audio() {
            continue;
        }
        let Some((mut nc, lead)) = run_step(e, track, &clip, &name, &params, handle_ms)? else { continue };
        // The step joins the clip's history, with the clip as it was before it.
        let mut before = clip.clone();
        before.processes.clear();
        nc.processes = clip.processes.clone();
        nc.processes.push(ProcessStep { process: name.clone(), params: params.clone(), handle_ms, bypassed: false, lead, before: Box::new(before) });
        let s = e.session_mut();
        if let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) {
            pl.clips.retain(|c| c.id != id);
        }
        crate::edit::place_clip(s, track, nc);
        done += 1;
    }
    Ok(json!({"processed": done, "process": name}))
}

/// Whether process `name` keeps the clip's length (and renders handles).
fn keeps_length(name: &str) -> bool {
    !matches!(name, "time_stretch" | "pitch_shift" | "varispeed")
}

/// Render `clip` (on `track`; it need not be the clip there now, only have its id) through
/// process `name` into a new source: the clip made from it, not placed, and where it starts in
/// the new file. `None` when the render is empty.
fn run_step(e: &mut Engine, track: TrackId, clip: &Clip, name: &str, params: &[(String, f32)], handle_ms: f64) -> Result<Option<(Clip, Samples)>> {
    let get = |k: &str, d: f32| params.iter().find(|(n, _)| n == k).map_or(d, |(_, v)| *v);
    let id = clip.id;
    let sr = e.session().sample_rate;
    let ratio = f64::from(get("ratio", 1.0).clamp(0.25, 4.0));
    let semitones = get("semitones", 0.0).clamp(-24.0, 24.0);
    let to = (sr.as_f64() / f64::from(get("speed", 1.0).clamp(0.25, 4.0))).round().max(1.0) as u32;
    // These processes cut their output at MAX_OFFLINE_SAMPLES frames.
    let longest = |grows: f64| (soundcraft_dsp::offline::MAX_OFFLINE_SAMPLES as f64 / grows) as usize;
    let most = match name {
        "time_stretch" => longest(ratio),
        "pitch_shift" => longest(2f64.powf(f64::from(semitones) / 12.0)),
        "varispeed" => longest(f64::from(to) / sr.as_f64()),
        _ => usize::MAX,
    };
    let keeps_length = keeps_length(name);
    {
        let channels = e.session().track(track).map_or(1, |t| t.channels());
        // The handles: material the file has before and after the clip (at its stretch), within
        // `handle_ms`, and never before the session start.
        let (before, after) = if keeps_length {
            let most = sr.samples(handle_ms / 1000.0);
            let file_before = clip.timeline_span(clip.source_offset()).max(0);
            let file_after = crate::edit::material_len(e.session(), clip)
                .map_or(0, |n| clip.timeline_span(n.saturating_sub(clip.source_offset()).saturating_sub(clip.source_span(clip.length))))
                .max(0);
            (most.min(file_before).min(clip.start.max(0)), most.min(file_after))
        } else {
            (0, 0)
        };
        let span = Range::new(clip.start - before, clip.end().saturating_add(after));
        crate::io::refuse_longer(e.session(), "process.run", &clip.name, "process", span.len(), most.min(crate::io::render_limit(channels)))?;
        let mut audio = Vec::new();
        // Render without the clip's own fades: re-render a copy without fades so they stay editable.
        let mut tmp = e.session().clone();
        // The clip to render in the place of the one with its id.
        if let Some(c) = tmp.find_clip_mut(id) {
            *c = clip.clone();
        }
        // The clip as it plays, without its fades and gain, before the handles are added.
        let mut unextended = None;
        if let Some(c) = tmp.find_clip_mut(id) {
            c.fade_in = Default::default();
            c.fade_out = Default::default();
            c.gain_db = 0.0;
            c.gain_env.clear();
            // A muted clip is processed as it sounds unmuted, not rendered to silence.
            c.muted = false;
            if before > 0 || after > 0 {
                unextended = Some(c.clone());
            }
            if before > 0 {
                c.trim_start_to(clip.start - before);
            }
            c.length = c.length.saturating_add(after);
        }
        if let Some(pl) = tmp.track_mut(track).and_then(|t| t.playlist_mut()) {
            pl.clips.retain(|c| c.id == id);
            audio = soundcraft_mix::render_clips(&tmp, track, span);
        }
        // Lengthening a clip can change how it sounds inside its old edges (Clean's alternate
        // crossfades over 5 ms where its cleaned audio ends inside the clip, so a clip that ended
        // with it would fade back to the original there). The handles are only what lies beyond:
        // the clip's own window is rendered as it played and written over the middle.
        if let Some(c0) = unextended {
            if let Some(c) = tmp.find_clip_mut(id) {
                *c = c0;
            }
            let window = soundcraft_mix::render_clips(&tmp, track, clip.range());
            let at = usize::try_from(before).unwrap_or(0);
            for (dst, src) in audio.iter_mut().zip(&window) {
                if let Some(dst) = dst.get_mut(at..) {
                    dst.iter_mut().zip(src).for_each(|(d, s)| *d = *s);
                }
            }
        }
        let srf = sr.as_f64() as f32;
        let out: Vec<Vec<f32>> = match name {
            "normalize" => {
                soundcraft_dsp::offline::normalize(&mut audio, get("target_db", -0.1), false);
                audio
            }
            "reverse" => {
                soundcraft_dsp::offline::reverse(&mut audio);
                audio
            }
            "invert" => {
                soundcraft_dsp::offline::invert(&mut audio);
                audio
            }
            "gain" if soundcraft_dsp::plugin_info("gain").is_none() || params.iter().any(|(k, _)| k == "db") => {
                soundcraft_dsp::offline::gain(&mut audio, get("db", 0.0));
                audio
            }
            "duplicate" => audio,
            "time_stretch" => soundcraft_dsp::offline::time_stretch(&audio, ratio, srf),
            "pitch_shift" => soundcraft_dsp::offline::pitch_shift(&audio, semitones, srf),
            "varispeed" => soundcraft_dsp::offline::resample(&audio, sr.hz(), to),
            plugin => {
                apply(plugin, params, &mut audio, srf)?;
                audio
            }
        };
        let frames = out.first().map_or(0, Vec::len);
        if frames == 0 {
            return Ok(None);
        }
        let buf = AudioBuffer { sample_rate: sr.hz(), channels: out };
        let new_name = format!("{}-{}", clip.name, short(name));
        let s = e.session_mut();
        let src = crate::io::add_source(s, &new_name, buf, None, FileFormat::Wav);
        let mut nc: Clip = clip.clone();
        nc.processes.clear();
        nc.name = new_name;
        // Reversed, what was after the clip comes first in the new file.
        let lead = if name == "reverse" { after } else { before };
        nc.content = ClipContent::Audio { source: src, offset: lead };
        soundcraft_model::rebase_lyrics(&mut nc, clip.source_offset());
        for w in nc.lyrics.iter_mut().flat_map(|l| l.words.iter_mut()) {
            (w.start, w.end) = (w.start.saturating_add(lead), w.end.saturating_add(lead));
        }
        nc.length = if keeps_length {
            clip.length.min(i64::try_from(frames).unwrap_or(0).saturating_sub(lead)).max(0)
        } else {
            i64::try_from(frames).unwrap_or(nc.length)
        };
        // The render already has the clip's stretch applied; keeping it would stretch the new file again.
        nc.stretch = 1.0;
        // Warp markers described the old render; the new file is plain audio.
        nc.warp.clear();
        if name == "reverse" {
            // Played backwards, the fade-in is now at the end, and the sync point mirrors.
            std::mem::swap(&mut nc.fade_in, &mut nc.fade_out);
            if nc.sync_point != 0 {
                nc.sync_point = nc.length.saturating_sub(nc.sync_point);
            }
        }
        // A new length (stretch, pitch, varispeed) keeps the fades inside the clip.
        nc.clamp_fades();
        if !(0..nc.length).contains(&nc.sync_point) {
            nc.sync_point = 0;
        }
        // Invert and Duplicate are phase/copy operations. The clip's gain stays a clip gain,
        // the same way its fades stay editable. Other processes bake a new file at unity gain.
        if !matches!(name, "invert" | "duplicate") {
            nc.gain_db = 0.0;
            nc.gain_env.clear();
        }
        // The render went through the clip's effects unless they were bypassed: baked into the
        // new file, they come off the clip (else they would be heard twice). Bypassed, they
        // stay as they were.
        if !nc.effects_bypass {
            nc.effects.clear();
        }
        Ok(Some((nc, lead)))
    }
}

/// Fill `r` on each selected audio track with a new clip from the generator `name`, over
/// whatever was there, like Pro Tools' Signal Generator.
fn generate(e: &mut Engine, p: &Value, name: &str, params: &[(String, f32)], r: Range) -> Result<Value> {
    let sr = e.session().sample_rate;
    let tracks = tracks_param(e, "process.run", p)?;
    // Refuse before generating anything, rather than cut the clip short.
    for t in tracks.iter().filter_map(|t| e.session().track(*t)).filter(|t| t.kind == TrackKind::Audio) {
        crate::io::refuse_longer(e.session(), "process.run", &t.name, "generate on", r.len(), crate::io::render_limit(t.channels()))?;
    }
    let mut clips = Vec::new();
    for track in tracks {
        let Some(t) = e.session().track(track).filter(|t| t.kind == TrackKind::Audio) else { continue };
        let channels = t.channels().max(1);
        let frames = usize::try_from(r.len()).unwrap_or(0).min(crate::io::render_limit(channels));
        let clip_name = format!("{}-{}", t.name, short(name));
        let mut audio = vec![vec![0.0; frames]; channels];
        apply(name, params, &mut audio, sr.as_f64() as f32)?;
        let s = e.session_mut();
        let src = crate::io::add_source(s, &clip_name, AudioBuffer { sample_rate: sr.hz(), channels: audio }, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(s, track, Clip::audio(id, &clip_name, src, 0, r.start, i64::try_from(frames).unwrap_or(0)));
        clips.push(id.0);
    }
    if clips.is_empty() {
        return Err(bad("process.run", "select an audio track to generate on"));
    }
    Ok(json!({"processed": clips.len(), "process": name, "clips": clips}))
}

/// Run plugin `id` with `params` over `audio`, in place.
fn apply(id: &str, params: &[(String, f32)], audio: &mut [Vec<f32>], srf: f32) -> Result<()> {
    let Some(mut pl) = soundcraft_dsp::create(id) else { return Err(bad("process.run", format!("cannot create `{id}`"))) };
    pl.prepare(srf, 1024, audio.len().max(1));
    for (k, v) in params {
        pl.set_param(k, *v);
    }
    soundcraft_dsp::offline::apply_plugin(audio, pl.as_mut(), srf);
    Ok(())
}

fn short(n: &str) -> String {
    let s: String = n.split('_').map(|w| w.chars().take(4).collect::<String>()).collect::<Vec<_>>().join("");
    s.chars().take(8).collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_model::{ChannelFormat, Session, TrackKind};

    fn gained_clip() -> (crate::Engine, soundcraft_model::ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![vec![0.5; 64]] };
        let src = crate::io::add_source(&mut s, "tone", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        let mut c = Clip::audio(id, "tone", src, 0, 0, 64);
        c.gain_db = -6.0;
        c.gain_env = vec![(0, 0.0), (32, -12.0)];
        crate::edit::place_clip(&mut s, t, c);
        s.edit.selected_clips = vec![id];
        (crate::Engine::new(s), id)
    }

    #[test]
    fn invert_and_duplicate_keep_clip_gain() {
        let (mut e, id) = gained_clip();
        e.execute("process.run", &json!({"process": "invert", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!((c.gain_db + 6.0).abs() < 1e-6, "{}", c.gain_db);
        assert_eq!(c.gain_env, vec![(0, 0.0), (32, -12.0)]);
        let sample = e.session().pool.get(c.source().unwrap()).unwrap().buffer().channels[0][0];
        assert!((sample + 0.5).abs() < 1e-3, "invert flips the file and leaves the gain on the clip, got {sample}");

        let (mut e, id) = gained_clip();
        e.execute("process.run", &json!({"process": "duplicate", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!((c.gain_db + 6.0).abs() < 1e-6, "{}", c.gain_db);
        assert_eq!(c.gain_env, vec![(0, 0.0), (32, -12.0)]);
        let sample = e.session().pool.get(c.source().unwrap()).unwrap().buffer().channels[0][0];
        assert!((sample - 0.5).abs() < 1e-3, "duplicate keeps the file level and the clip gain, got {sample}");
    }

    #[test]
    fn process_takes_off_the_clip_effects_it_baked() {
        // Bug: the effects were rendered into the new file and still active, so heard twice.
        let (mut e, id) = gained_clip();
        e.execute("clip.effects_set", &json!({"clips": [id.0], "params": {"gain": -12.0}})).unwrap();
        let before = soundcraft_mix::render_clips(e.session(), e.session().tracks[0].id, Range::new(0, 64));
        e.execute("process.run", &json!({"process": "duplicate", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!(c.effects.is_empty(), "no effects left on the clip");
        let after = soundcraft_mix::render_clips(e.session(), e.session().tracks[0].id, Range::new(0, 64));
        assert!((after[0][40] - before[0][40]).abs() < 1e-3, "heard once, as before: {} vs {}", after[0][40], before[0][40]);

        // Bypassed effects are not in the render, so they stay on the clip, still bypassed.
        let (mut e, id) = gained_clip();
        e.execute("clip.effects_set", &json!({"clips": [id.0], "params": {"gain": -12.0}})).unwrap();
        e.execute("clip.effects_bypass", &json!({"clips": [id.0], "value": true})).unwrap();
        e.execute("process.run", &json!({"process": "duplicate", "clips": [id.0]})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert_eq!(c.effect("gain"), Some(-12.0));
        assert!(c.effects_bypass);
    }

    #[test]
    fn processed_clip_does_not_keep_the_stretch_it_baked() {
        for stretch in [0.5, 2.0] {
            for process in ["invert", "duplicate"] {
                let (mut e, id) = gained_clip();
                e.session_mut().find_clip_mut(id).unwrap().stretch = stretch;
                e.execute("process.run", &json!({"process": process, "clips": [id.0]})).unwrap();
                let c = e.session().tracks[0].clips()[0].clone();
                let frames = e.session().pool.get(c.source().unwrap()).unwrap().buffer().channels[0].len();
                assert_eq!(c.stretch, 1.0, "{process} at {stretch}");
                // The new file holds the clip and its handles, at unity stretch.
                assert!(c.source_offset() + c.length <= i64::try_from(frames).unwrap(), "{process} at {stretch}");
                assert_eq!(c.length, 64, "{process} at {stretch}: the clip keeps its length on the timeline");
            }
        }
    }

    #[test]
    fn signal_generator_fills_the_selection_with_a_clip() {
        let (mut e, _) = gained_clip();
        let tone = e.session().tracks[0].id;
        let t = e.session_mut().add_track(TrackKind::Audio, ChannelFormat::Stereo, Some("Empty"));
        e.execute("edit.select", &json!({"tracks": [t.0], "start": 48_000, "end": 96_000})).unwrap();
        assert!(matches!(e.execute("process.run", &json!({"process": "normalize"})), Err(crate::EngineError::BadParams(..))));

        let generator = json!({"process": "signal_generator", "params": {"freq": 440.0, "level": -6.0}});
        assert_eq!(e.execute("process.run", &generator).unwrap()["processed"], 1);
        let c = e.session().tracks[1].clips()[0].clone();
        assert_eq!((c.start, c.length), (48_000, 48_000));
        let buf = &e.session().pool.get(c.source().unwrap()).unwrap().buffer();
        assert_eq!(buf.channels.len(), 2);
        let peak = buf.channels[1].iter().fold(0.0f32, |m, x| m.max(x.abs()));
        assert!((peak - 0.5).abs() < 0.01, "-6 dB sine peaks near 0.5, got {peak}");

        // Over existing audio it replaces just the selection.
        e.execute("edit.select", &json!({"tracks": [tone.0], "start": 32, "end": 96})).unwrap();
        e.execute("process.run", &generator).unwrap();
        let ranges: Vec<_> = e.session().tracks[0].clips().iter().map(|c| (c.start, c.length)).collect();
        assert_eq!(ranges, vec![(0, 32), (32, 64)]);
    }

    /// A clip at 1 000 over frames 100.. of a ramp file (frame `i` holds `i / 10 000`).
    fn ramp_clip() -> (crate::Engine, soundcraft_model::ClipId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Ramp"));
        let buf = AudioBuffer { sample_rate: 48_000, channels: vec![(0..4_000).map(|i| i as f32 / 10_000.0).collect()] };
        let src = crate::io::add_source(&mut s, "ramp", buf, None, FileFormat::Wav);
        let id = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(id, "ramp", src, 100, 1_000, 500));
        s.edit.selected_clips = vec![id];
        (crate::Engine::new(s), id)
    }

    fn first_sample(e: &crate::Engine) -> f32 {
        let c = e.session().tracks[0].clips()[0].clone();
        let at = usize::try_from(c.source_offset()).unwrap();
        e.session().pool.get(c.source().unwrap()).unwrap().buffer().channels[0][at]
    }

    #[test]
    fn a_process_step_can_be_removed_bypassed_and_moved_later() {
        let (mut e, id) = ramp_clip();
        let original = e.session().tracks[0].clips()[0].source();
        e.execute("process.run", &json!({"process": "gain", "params": {"db": 6.0206}, "clips": [id.0]})).unwrap();
        e.execute("process.run", &json!({"process": "invert", "clips": [id.0]})).unwrap();
        assert!((first_sample(&e) + 0.02).abs() < 1e-4, "doubled and inverted: {}", first_sample(&e));
        let h = e.execute("process.history", &json!({"clip": id.0})).unwrap();
        let names: Vec<&str> = h["steps"].as_array().unwrap().iter().map(|x| x["process"].as_str().unwrap()).collect();
        assert_eq!(names, ["gain", "invert"]);

        // Trimmed since: the clip made again keeps today's edges.
        let c = e.session_mut().find_clip_mut(id).unwrap();
        c.trim_start_to(1_016);
        e.execute("process.remove_step", &json!({"clip": id.0, "step": 0})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert_eq!((c.start, c.length), (1_016, 484));
        assert!((first_sample(&e) + 0.0116).abs() < 1e-4, "only inverted, from frame 116: {}", first_sample(&e));
        assert_eq!(c.processes.len(), 1);

        e.execute("process.bypass_step", &json!({"clip": id.0, "step": 0})).unwrap();
        let c = e.session().tracks[0].clips()[0].clone();
        assert!(c.processes[0].bypassed);
        assert_eq!((c.source(), c.source_offset()), (original, 116), "nothing applied: the original file again");
        e.execute("process.bypass_step", &json!({"clip": id.0, "step": 0})).unwrap();
        assert!((first_sample(&e) + 0.0116).abs() < 1e-4, "inverted again: {}", first_sample(&e));
        assert!(e.undo());
        assert_eq!(e.session().tracks[0].clips()[0].source(), original, "one undo step each");

        // The order matters: normalize then gain is louder than gain then normalize.
        let (mut e, id) = ramp_clip();
        e.execute("process.run", &json!({"process": "normalize", "params": {"target_db": -6.0206}, "clips": [id.0]})).unwrap();
        e.execute("process.run", &json!({"process": "gain", "params": {"db": 6.0206}, "clips": [id.0]})).unwrap();
        let peak = |e: &crate::Engine| {
            let c = e.session().tracks[0].clips()[0].clone();
            let buf = e.session().pool.get(c.source().unwrap()).unwrap().buffer();
            buf.channels[0].iter().fold(0f32, |m, x| m.max(x.abs()))
        };
        assert!((peak(&e) - 1.0).abs() < 0.01, "{}", peak(&e));
        e.execute("process.move_step", &json!({"clip": id.0, "step": 1, "to": 0})).unwrap();
        assert!((peak(&e) - 0.5).abs() < 0.01, "{}", peak(&e));
        let names: Vec<String> = e.session().tracks[0].clips()[0].processes.iter().map(|x| x.process.clone()).collect();
        assert_eq!(names, ["gain", "normalize"]);
    }

    #[test]
    fn the_old_audiosuite_id_still_runs_process() {
        // Scripts and control clients written before the rename keep working.
        assert_eq!(crate::find_command("audiosuite.process").map(|c| c.id), Some("process.run"));
        assert!(crate::command_specs().iter().all(|c| c.id != "audiosuite.process"), "the old id is an alias, not a listed command");
        let (mut e, id) = gained_clip();
        assert_eq!(e.execute("audiosuite.process", &json!({"process": "invert", "clips": [id.0]})).unwrap()["processed"], 1);
        assert_eq!(e.undo_label(), Some("Process"));
    }
}

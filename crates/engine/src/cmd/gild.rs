//! Gild: effects on chosen words of a vocal (docs/gild.md, `soundcraft_model::gild`). A gilding
//! is named by `clip` and `id` (as `gild.list` gives them); words by their refs `{clip, line,
//! word}` (as `lyrics.words` lists them). Every edit is one undo step.

use super::*;
use crate::cmd;
use crate::gild::GildCopy;
use crate::word_time::{self as wt, TrackWord, WordRef};
use serde_json::json;
use soundcraft_model::gild::{GILD_KINDS, ensure_word_ids, next_gild_id, render_key};
use soundcraft_model::{Clip, ClipId, GildFx, Gilding, Session, TrackKind};

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "gild.list",
            "Gildings",
            [],
            None,
            "{track?} — the track's gildings in time order: {clip, id, start, end (timeline), seconds, text, words (word ids), held, fade_in_ms, fade_out_ms, kinds, fx (each effect's settings), render (none | ready | making | failed: why)}, the track's `dry` and `level_db`, and `words` (how many timed words it has)",
            always,
            list
        ),
        cmd!(
            "gild.add",
            "Gild Words",
            [],
            None,
            "{words: [{clip, line, word}] | track? + start + end (timeline), fx?: {shift|grit|double|harmony|delay|reverb|swell: true | {settings}}, preset?} — gilds the words: each run of adjacent words on one clip becomes one gilding (its span the words', fades 8/20 ms), separate words separate gildings sharing the settings. An effect given `true` (or no `fx` at all: the last effects used, else a delay throw) starts from the settings last used for it. Refused on words already gilded (gild.set changes those) and on untimed tracks → {gildings: [{clip, id}]}",
            always,
            add
        ),
        cmd!(
            "gild.set",
            "Change Gild",
            [],
            None,
            "{gildings: [{clip, id}] | clip + id, fx?: {kind: true | false | null | {settings}}, start?, end? (timeline: moves that handle; the span then keeps its own times), held?: false (follow the words again), fade_in_ms?, fade_out_ms?} — changes every gilding named the same way (settings merge into each effect's)",
            always,
            set
        ),
        cmd!(
            "gild.remove",
            "Remove Gild",
            [],
            None,
            "{gildings: [{clip, id}] | clip + id | words: [{clip, line, word}]} — removes the gildings named, or those on the words",
            always,
            remove
        ),
        cmd!(
            "gild.clear_track",
            "Remove All Gild on Track",
            [],
            None,
            "{track?} — removes every gilding on the track's clips (every playlist); the panel asks first",
            has_gild_track,
            clear_track
        ),
        cmd!(
            noundo "gild.copy",
            "Copy Gild",
            [],
            None,
            "{clip, id} — keeps the gilding's effects and fades for gild.paste → {fx}",
            always,
            copy
        ),
        cmd!(
            "gild.paste",
            "Paste Gild",
            [],
            None,
            "{gildings: [{clip, id}] | words: [{clip, line, word}]} — the copied effects and fades onto the gildings, or as new gildings on the words (words already gilded take them too)",
            has_copy,
            paste
        ),
        cmd!(
            "gild.track",
            "Gild Track Settings",
            [],
            None,
            "{track?, dry?: bool (off: the track gives only what Gild adds), level_db? (-24..+12: what Gild adds)} → {dry, level_db}",
            always,
            track_settings
        ),
        cmd!(
            noundo "gild.audition",
            "Audition Gilding",
            [],
            None,
            "{clip, id, tail? (seconds after the span, default the throws' ring, at most 8)} — plays the gilding's span and its tail",
            always,
            audition
        ),
        cmd!(
            noundo "gild.render",
            "Make Gild Renders",
            [],
            None,
            "{} — makes every Gild render still missing now (the made-ahead effects: Shift, Grit, Double, Harmony, Swell-in), instead of in the background → {made}",
            always,
            render_now
        ),
        cmd!(
            query "gild.presets",
            "Gild Presets",
            [],
            None,
            "{} — the built-in Gild presets: [{name, kind (an effect, or `gilding` for a whole set), fx}]",
            always,
            presets
        ),
    ]
}

fn has_gild_track(e: &Engine) -> std::result::Result<(), String> {
    if e.session().tracks.iter().any(|t| t.playlists.iter().any(|p| p.clips.iter().any(|c| !c.gild.is_empty()))) {
        Ok(())
    } else {
        Err("no words are gilded".into())
    }
}

fn has_copy(e: &Engine) -> std::result::Result<(), String> {
    if e.gild_copy.is_some() { Ok(()) } else { Err("copy a gilding first (Copy Gild)".into()) }
}

/// The track Gild acts on: `track`, else the track of the first word or gilding named, else as
/// the word lanes' commands choose.
fn gild_track(e: &Engine, cmd: &str, p: &Value) -> Result<TrackId> {
    if let Some(t) = track_param(e, cmd, p, "track")? {
        return Ok(t);
    }
    let first_clip = p
        .get("words")
        .and_then(Value::as_array)
        .and_then(|a| a.first())
        .or_else(|| p.get("gildings").and_then(Value::as_array).and_then(|a| a.first()))
        .or(Some(p))
        .and_then(|v| v.get("clip"))
        .and_then(Value::as_u64);
    if let Some(c) = first_clip
        && let Some(t) = e.session().tracks.iter().find(|t| t.playlists.iter().any(|pl| pl.clips.iter().any(|x| x.id.0 == c)))
    {
        return Ok(t.id);
    }
    super::lyric_timing::timing_track(e, cmd, p)
}

fn word_refs(cmd: &str, p: &Value) -> Result<Vec<WordRef>> {
    let a =
        p.get("words").and_then(Value::as_array).ok_or_else(|| bad(cmd, "`words` required: [{clip, line, word}] (as lyrics.words lists them)"))?;
    a.iter()
        .map(|v| {
            let get = |k: &str| v.get(k).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("each word needs `{k}`")));
            Ok(WordRef {
                clip: ClipId(get("clip")?),
                line: usize::try_from(get("line")?).unwrap_or(usize::MAX),
                word: usize::try_from(get("word")?).unwrap_or(usize::MAX),
            })
        })
        .collect()
}

/// The gildings `p` names: `gildings: [{clip, id}]`, or `clip` + `id`.
fn gild_refs(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<(ClipId, u32)>> {
    let one = |v: &Value| -> Result<(ClipId, u32)> {
        let c = v.get("clip").and_then(Value::as_u64).ok_or_else(|| bad(cmd, "each gilding needs `clip` and `id` (as gild.list gives them)"))?;
        let id = v.get("id").and_then(Value::as_u64).and_then(|i| u32::try_from(i).ok()).ok_or_else(|| bad(cmd, "each gilding needs `id`"))?;
        Ok((ClipId(c), id))
    };
    let refs: Vec<(ClipId, u32)> = match p.get("gildings").and_then(Value::as_array) {
        Some(a) => a.iter().map(one).collect::<Result<_>>()?,
        None => vec![one(p)?],
    };
    for (c, id) in &refs {
        let found = e.session().find_clip(*c).is_some_and(|(_, clip)| clip.gild.iter().any(|g| g.id == *id));
        if !found {
            return Err(bad(cmd, format!("no gilding {id} on clip {}", c.0)));
        }
    }
    Ok(refs)
}

fn track_of_clip(s: &Session, c: ClipId) -> Option<TrackId> {
    s.tracks.iter().find(|t| t.playlists.iter().any(|p| p.clips.iter().any(|x| x.id == c))).map(|t| t.id)
}

fn render_state(s: &Session, clip: &Clip, g: &Gilding) -> Value {
    match render_key(clip, g, s.sample_rate.hz()) {
        None => json!("none"),
        Some(k) if s.gild.renders.get(&(clip.id, g.id)).is_some_and(|r| r.key == k) => json!("ready"),
        Some(k) => match s.gild.failed.get(&k) {
            Some(why) => json!(format!("failed: {why}")),
            None => json!("making"),
        },
    }
}

/// One gilding as the protocol gives it.
pub fn gild_json(s: &Session, clip: &Clip, g: &Gilding) -> Value {
    let rate = s.sample_rate.as_f64().max(1.0);
    let (a, b) = g.on_timeline(clip).unwrap_or((Gilding::timeline_of(clip, g.start), Gilding::timeline_of(clip, g.end)));
    json!({"clip": clip.id, "id": g.id, "start": a, "end": b, "seconds": a as f64 / rate, "text": g.text, "words": g.words, "held": g.held,
           "fade_in_ms": g.fade_in_ms, "fade_out_ms": g.fade_out_ms, "kinds": g.fx.kinds(), "fx": serde_json::to_value(&g.fx).unwrap_or(Value::Null),
           "render": render_state(s, clip, g), "heard": g.on_timeline(clip).is_some()})
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.list";
    let t = gild_track(e, C, p)?;
    let s = e.session();
    let tr = s.track(t).ok_or_else(|| bad(C, "no such track"))?;
    let mut out: Vec<(Samples, Value)> = Vec::new();
    for c in tr.clips() {
        for g in &c.gild {
            out.push((Gilding::timeline_of(c, g.start), gild_json(s, c, g)));
        }
    }
    out.sort_by_key(|(a, _)| *a);
    let words = wt::track_words(s, t).len();
    Ok(json!({"track": t, "dry": tr.gild.dry, "level_db": tr.gild.level_db, "words": words,
              "gildings": out.into_iter().map(|(_, v)| v).collect::<Vec<_>>()}))
}

/// Why `fx` can't go on `clip`: the changes and layers come from the clip's own audio, which an
/// ARA plug-in replaces; the changes need a clip played as recorded.
fn refusal(clip: &Clip, fx: &GildFx) -> Option<String> {
    let made_from_audio = fx.shift.is_some() || fx.grit.is_some() || fx.double.is_some() || fx.harmony.is_some();
    if made_from_audio && let Some(name) = crate::vocal::plays_through_ara(clip) {
        return Some(format!(
            "this clip plays through {name} (ARA): Shift, Grit, Double and Harmony are made from the recorded audio, which the plug-in replaces. Bypass {name} for the clip, or use Delay, Reverb and Swell-in"
        ));
    }
    let plain = (clip.stretch - 1.0).abs() < 1e-9 && !clip.reversed && clip.loop_len == 0 && clip.transpose_cents == 0 && clip.warp.is_empty();
    if fx.changes_word() && !plain {
        return Some("Shift (in place) and Grit work on a clip played as recorded (not stretched, reversed, looped or transposed)".into());
    }
    None
}

/// `base` with `patch` laid over it: an effect `true` turns it on (from `last`'s settings, else the
/// default), `false`/`null` off, an object merges into its settings.
fn patch_fx(cmd: &str, base: &GildFx, patch: &Value, last: &GildFx) -> Result<GildFx> {
    let obj = patch.as_object().ok_or_else(|| bad(cmd, "`fx` is an object: {delay: true | {...}, …}"))?;
    let mut v = serde_json::to_value(base).map_err(|e| bad(cmd, e.to_string()))?;
    let lastv = serde_json::to_value(last).unwrap_or(Value::Null);
    let map = v.as_object_mut().ok_or_else(|| bad(cmd, "bad settings"))?;
    for (k, val) in obj {
        if !GILD_KINDS.contains(&k.as_str()) {
            return Err(bad(cmd, format!("unknown effect `{k}` ({})", GILD_KINDS.join(", "))));
        }
        match val {
            Value::Bool(false) | Value::Null => {
                map.remove(k);
            }
            Value::Bool(true) => {
                if !map.contains_key(k) {
                    map.insert(k.clone(), lastv.get(k).cloned().unwrap_or_else(|| json!({})));
                }
            }
            Value::Object(o) => {
                let cur = map.entry(k.clone()).or_insert_with(|| lastv.get(k).cloned().unwrap_or_else(|| json!({})));
                if !cur.is_object() {
                    *cur = json!({});
                }
                if let Some(c) = cur.as_object_mut() {
                    for (f, x) in o {
                        c.insert(f.clone(), x.clone());
                    }
                }
            }
            _ => return Err(bad(cmd, format!("`fx.{k}` is true, false or an object of settings"))),
        }
    }
    let mut fx: GildFx = serde_json::from_value(v).map_err(|e| bad(cmd, format!("bad settings: {e}")))?;
    fx.sanitize();
    Ok(fx)
}

/// Remembers the settings of every effect `fx` has as the last used.
fn remember(e: &mut Engine, fx: &GildFx) {
    let l = &mut e.gild_last;
    l.shift = fx.shift.or(l.shift);
    l.grit = fx.grit.or(l.grit);
    l.double = fx.double.or(l.double);
    l.delay = fx.delay.or(l.delay);
    l.reverb = fx.reverb.or(l.reverb);
    l.swell = fx.swell.or(l.swell);
    if fx.harmony.is_some() {
        l.harmony = fx.harmony.clone();
    }
}

/// Runs of adjacent words: the words of each clip in time order, split where a word of the clip
/// in between was not chosen.
fn runs(s: &Session, track: TrackId, refs: &[WordRef]) -> Vec<(ClipId, Vec<TrackWord>)> {
    let all = wt::track_words(s, track);
    let mut out: Vec<(ClipId, Vec<TrackWord>)> = Vec::new();
    let mut last: Option<usize> = None;
    for (i, w) in all.iter().enumerate() {
        if !refs.contains(&w.r) {
            continue;
        }
        let adjacent = last.is_some_and(|l| l + 1 == i) && out.last().is_some_and(|(c, _)| *c == w.r.clip);
        if adjacent && let Some((_, v)) = out.last_mut() {
            v.push(w.clone());
        } else {
            out.push((w.r.clip, vec![w.clone()]));
        }
        last = Some(i);
    }
    out
}

/// Makes gildings on `refs` (one per run of adjacent words) with `fx` and fades. Returns their
/// refs; words already gilded are skipped (their gildings returned in `taken`).
fn make_gildings(
    e: &mut Engine,
    cmd: &str,
    t: TrackId,
    refs: &[WordRef],
    fx: &GildFx,
    fades: Option<(f32, f32)>,
) -> Result<(Vec<(ClipId, u32)>, Vec<(ClipId, u32)>)> {
    let groups = runs(e.session(), t, refs);
    if groups.is_empty() {
        return Err(bad(cmd, "none of those words is on the track's playing clips (lyrics.words lists them)"));
    }
    for (c, _) in &groups {
        if let Some((_, clip)) = e.session().find_clip(*c)
            && let Some(why) = refusal(clip, fx)
        {
            return Err(bad(cmd, why));
        }
    }
    let mut made = Vec::new();
    let mut taken = Vec::new();
    for (cid, words) in groups {
        let Some(clip) = e.session_mut().find_clip_mut(cid) else { continue };
        ensure_word_ids(clip);
        // Already gilded: a word inside an existing gilding.
        let src = |w: &TrackWord| clip.lyrics.get(w.r.line).and_then(|l| l.words.get(w.r.word)).map(|x| (x.start, x.end, x.id));
        let ws: Vec<(Samples, Samples, u32)> = words.iter().filter_map(src).collect();
        if let Some(g) = clip.gild.iter().find(|g| ws.iter().any(|(a, b, id)| g.words.contains(id) || (*a < g.end && *b > g.start))) {
            taken.push((cid, g.id));
            continue;
        }
        let (Some(a), Some(b)) = (ws.iter().map(|w| w.0).min(), ws.iter().map(|w| w.1).max()) else { continue };
        if clip.gild.len() >= soundcraft_model::gild::MAX_GILDINGS {
            return Err(bad(cmd, "this clip has as many gildings as it can hold"));
        }
        let mut g = Gilding::new(next_gild_id(clip), a, b);
        g.words = ws.iter().map(|w| w.2).collect();
        g.text = words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ");
        if let Some((fi, fo)) = fades {
            g.fade_in_ms = fi;
            g.fade_out_ms = fo;
        }
        g.fx = fx.clone();
        let id = g.id;
        clip.gild.push(g);
        clip.gild.sort_by_key(|g| (g.start, g.id));
        made.push((cid, id));
    }
    Ok((made, taken))
}

fn add(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.add";
    let t = gild_track(e, C, p)?;
    if e.session().track(t).is_none_or(|tr| tr.kind != TrackKind::Audio) {
        return Err(bad(C, "Gild works on an audio track (the vocal)"));
    }
    let last = e.gild_last.clone();
    let fx = match (p.get("preset").and_then(Value::as_str), p.get("fx")) {
        (Some(name), _) => {
            crate::gild_presets::find(name).map(|pr| pr.fx).ok_or_else(|| bad(C, format!("no Gild preset `{name}` (gild.presets)")))?
        }
        (None, Some(v)) => patch_fx(C, &GildFx::default(), v, &last)?,
        (None, None) if !last.is_empty() => last.clone(),
        (None, None) => GildFx { delay: Some(Default::default()), ..GildFx::default() },
    };
    if fx.is_empty() {
        return Err(bad(C, "turn at least one effect on (`fx`)"));
    }
    let made = if p.get("words").is_some() {
        let refs = word_refs(C, p)?;
        if wt::track_words(e.session(), t).is_empty() {
            return Err(bad(C, "time the words first (Lyrics › Timing › Time Words to Vocal, or Tap Words)"));
        }
        let (made, taken) = make_gildings(e, C, t, &refs, &fx, None)?;
        if made.is_empty() && !taken.is_empty() {
            return Err(bad(C, "those words are gilded already: change them with gild.set (or Remove first)"));
        }
        made
    } else {
        let (a, b) = match (position_param(e, C, p, "start")?, position_param(e, C, p, "end")?) {
            (Some(a), Some(b)) if b > a => (a, b),
            _ => return Err(bad(C, "give `words`, or `start` and `end` (timeline)")),
        };
        let s = e.session();
        let tr = s.track(t).ok_or_else(|| bad(C, "no such track"))?;
        let clip = tr.clips().iter().find(|c| c.is_audio() && !c.muted && c.range().contains(a)).ok_or_else(|| bad(C, "no clip plays at `start`"))?;
        if let Some(why) = refusal(clip, &fx) {
            return Err(bad(C, why));
        }
        let cid = clip.id;
        let (sa, sb) = (soundcraft_model::to_clip_time(clip, a), soundcraft_model::to_clip_time(clip, b.min(clip.end())));
        let c = e.session_mut().find_clip_mut(cid).ok_or_else(|| bad(C, "no such clip"))?;
        let mut g = Gilding::new(next_gild_id(c), sa, sb);
        g.held = true;
        g.fx = fx.clone();
        let id = g.id;
        c.gild.push(g);
        c.gild.sort_by_key(|g| (g.start, g.id));
        vec![(cid, id)]
    };
    remember(e, &fx);
    Ok(json!({"gildings": made.iter().map(|(c, id)| json!({"clip": c, "id": id})).collect::<Vec<_>>()}))
}

fn set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.set";
    let refs = gild_refs(e, C, p)?;
    let last = e.gild_last.clone();
    let (start, end) = (position_param(e, C, p, "start")?, position_param(e, C, p, "end")?);
    let fade = |k: &str| p.get(k).and_then(Value::as_f64).filter(|v| v.is_finite()).map(|v| v.clamp(0.0, 200.0) as f32);
    let (fi, fo) = (fade("fade_in_ms"), fade("fade_out_ms"));
    let unhold = p.get("held").and_then(Value::as_bool) == Some(false);
    let rate = e.session().sample_rate.as_f64();
    let shortest = (0.02 * rate) as Samples;
    let mut used = None;
    for (cid, id) in &refs {
        let Some(clip) = e.session().find_clip(*cid).map(|(_, c)| c.clone()) else { continue };
        let Some(g) = clip.gild.iter().find(|g| g.id == *id).cloned() else { continue };
        let mut ng = g.clone();
        if let Some(f) = p.get("fx") {
            ng.fx = patch_fx(C, &g.fx, f, &last)?;
            if ng.fx.is_empty() {
                return Err(bad(C, "that would leave no effect on: use gild.remove"));
            }
            if let Some(why) = refusal(&clip, &ng.fx) {
                return Err(bad(C, why));
            }
        }
        // Handles: timeline positions into the clip's source time, a span at least 20 ms.
        if start.is_some() || end.is_some() {
            let to = |at: Samples| soundcraft_model::to_clip_time(&clip, at);
            let lo = clip.source_offset();
            let a = start.map_or(ng.start, |a| to(a).max(lo));
            let b = end.map_or(ng.end, |b| to(b).min(lo.saturating_add(clip.length)));
            if b - a < shortest {
                return Err(bad(C, "a gilding is at least 20 ms long"));
            }
            ng.start = a;
            ng.end = b;
            ng.held = true;
        }
        if unhold {
            ng.held = false;
        }
        if let Some(v) = fi {
            ng.fade_in_ms = v;
        }
        if let Some(v) = fo {
            ng.fade_out_ms = v;
        }
        used = Some(ng.fx.clone());
        if let Some(c) = e.session_mut().find_clip_mut(*cid)
            && let Some(slot) = c.gild.iter_mut().find(|g| g.id == *id)
        {
            *slot = ng;
            c.gild.sort_by_key(|g| (g.start, g.id));
        }
    }
    if unhold {
        crate::gild::follow(e.session_mut());
    }
    if let Some(fx) = used {
        remember(e, &fx);
    }
    let s = e.session();
    let out: Vec<Value> = refs
        .iter()
        .filter_map(|(c, id)| s.find_clip(*c).and_then(|(_, clip)| clip.gild.iter().find(|g| g.id == *id).map(|g| gild_json(s, clip, g))))
        .collect();
    Ok(json!({"gildings": out}))
}

fn remove(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.remove";
    let refs: Vec<(ClipId, u32)> = if p.get("words").is_some() {
        let words = word_refs(C, p)?;
        let s = e.session();
        let mut v = Vec::new();
        for w in &words {
            let Some((_, clip)) = s.find_clip(w.clip) else { continue };
            let Some(word) = clip.lyrics.get(w.line).and_then(|l| l.words.get(w.word)) else { continue };
            for g in clip.gild.iter().filter(|g| (word.id != 0 && g.words.contains(&word.id)) || (word.start < g.end && word.end > g.start)) {
                if !v.contains(&(clip.id, g.id)) {
                    v.push((clip.id, g.id));
                }
            }
        }
        v
    } else {
        gild_refs(e, C, p)?
    };
    let mut n = 0;
    for (cid, id) in &refs {
        if let Some(c) = e.session_mut().find_clip_mut(*cid) {
            let before = c.gild.len();
            c.gild.retain(|g| g.id != *id);
            n += before - c.gild.len();
        }
    }
    if n == 0 {
        return Err(bad(C, "nothing gilded there"));
    }
    Ok(json!({"removed": n}))
}

fn clear_track(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.clear_track";
    let t = gild_track(e, C, p)?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no such track"))?;
    let mut n = 0;
    for pl in &mut tr.playlists {
        for c in &mut pl.clips {
            n += c.gild.len();
            c.gild.clear();
        }
    }
    if n == 0 {
        return Err(bad(C, "nothing on this track is gilded"));
    }
    Ok(json!({"removed": n}))
}

fn copy(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.copy";
    let refs = gild_refs(e, C, p)?;
    let (cid, id) = *refs.first().ok_or_else(|| bad(C, "name a gilding"))?;
    let g = e.session().find_clip(cid).and_then(|(_, c)| c.gild.iter().find(|g| g.id == id).cloned()).ok_or_else(|| bad(C, "no such gilding"))?;
    e.gild_copy = Some(GildCopy { fx: g.fx.clone(), fade_in_ms: g.fade_in_ms, fade_out_ms: g.fade_out_ms });
    Ok(json!({"fx": g.fx, "kinds": g.fx.kinds()}))
}

fn paste(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.paste";
    let cp = e.gild_copy.clone().ok_or_else(|| bad(C, "copy a gilding first (Copy Gild)"))?;
    let mut targets: Vec<(ClipId, u32)> = Vec::new();
    if p.get("words").is_some() {
        let words = word_refs(C, p)?;
        let t = gild_track(e, C, p)?;
        let (made, taken) = make_gildings(e, C, t, &words, &cp.fx, Some((cp.fade_in_ms, cp.fade_out_ms)))?;
        targets.extend(made);
        targets.extend(taken);
    } else {
        targets = gild_refs(e, C, p)?;
    }
    for (cid, id) in &targets {
        let Some(clip) = e.session().find_clip(*cid).map(|(_, c)| c.clone()) else { continue };
        if let Some(why) = refusal(&clip, &cp.fx) {
            return Err(bad(C, why));
        }
        if let Some(c) = e.session_mut().find_clip_mut(*cid)
            && let Some(g) = c.gild.iter_mut().find(|g| g.id == *id)
        {
            g.fx = cp.fx.clone();
            g.fade_in_ms = cp.fade_in_ms;
            g.fade_out_ms = cp.fade_out_ms;
        }
    }
    if targets.is_empty() {
        return Err(bad(C, "nothing to paste onto"));
    }
    Ok(json!({"gildings": targets.iter().map(|(c, id)| json!({"clip": c, "id": id})).collect::<Vec<_>>()}))
}

fn track_settings(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.track";
    let t = gild_track(e, C, p)?;
    let tr = e.session_mut().track_mut(t).ok_or_else(|| bad(C, "no such track"))?;
    if tr.kind != TrackKind::Audio {
        return Err(bad(C, "Gild works on an audio track"));
    }
    if let Some(d) = p.get("dry").and_then(Value::as_bool) {
        tr.gild.dry = d;
    }
    if let Some(l) = p.get("level_db").and_then(Value::as_f64) {
        tr.gild.level_db = l as f32;
    }
    tr.gild.sanitize();
    Ok(json!({"track": t, "dry": tr.gild.dry, "level_db": tr.gild.level_db}))
}

/// How long a gilding's throws ring after it, seconds (Audition plays that much more).
pub fn ring_secs(e: &Engine, g: &Gilding) -> f64 {
    let s = e.session();
    let beat = 60.0 / soundcraft_mix::transport_at(s, s.edit.playhead, true, false).tempo.max(1.0);
    let delay = g.fx.delay.map_or(0.0, |d| {
        let t = if d.sync { d.note.beats() * beat } else { f64::from(d.ms) / 1000.0 };
        let r = f64::from(d.repeats.clamp(0.0, 0.9)).max(0.05);
        t * (-60.0 / (20.0 * r.log10())).clamp(1.0, 12.0)
    });
    let verb = g.fx.reverb.map_or(0.0, |v| f64::from(v.decay_s));
    delay.max(verb).min(8.0)
}

fn audition(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "gild.audition";
    let refs = gild_refs(e, C, p)?;
    let (cid, id) = *refs.first().ok_or_else(|| bad(C, "name a gilding"))?;
    let (clip, g) = e
        .session()
        .find_clip(cid)
        .and_then(|(_, c)| c.gild.iter().find(|g| g.id == id).map(|g| (c.clone(), g.clone())))
        .ok_or_else(|| bad(C, "no such gilding"))?;
    let (a, b) = g.on_timeline(&clip).ok_or_else(|| bad(C, "that gilding is outside its clip's window"))?;
    let tail = p.get("tail").and_then(Value::as_f64).filter(|v| v.is_finite()).unwrap_or_else(|| ring_secs(e, &g)).clamp(0.0, 8.0);
    let pre = g.fx.swell.map_or(0.0, |s| f64::from(s.length_s));
    let rate = e.session().sample_rate;
    let from = (a - rate.samples(pre)).max(0);
    let to = b + rate.samples(tail);
    e.transport_requests.push(crate::TransportRequest::PlayRange { from, end: Some(to), looped: None });
    Ok(json!({"start": from, "end": to, "track": track_of_clip(e.session(), cid)}))
}

fn render_now(e: &mut Engine, _p: &Value) -> Result<Value> {
    Ok(json!({"made": crate::gild::settle(e)}))
}

fn presets(_e: &mut Engine, _p: &Value) -> Result<Value> {
    Ok(json!({"presets": crate::gild_presets::all().iter().map(|p| json!({"name": p.name, "kind": p.kind, "fx": p.fx})).collect::<Vec<_>>()}))
}

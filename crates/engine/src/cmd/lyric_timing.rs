//! Timing words on a vocal (see [`crate::word_time`]): list them, edit one by hand, spread the
//! typed sheet's placed lines over the sung notes (Time Words to Vocal), apply taps, and select
//! words by the shape of the lyric. A word is named by `clip`, `line` and `word` (indices into the
//! clip's lyrics, as `lyrics.words` lists them); every edit returns the word's new ref.

use super::*;
use crate::cmd;
use crate::word_time::{self as wt, Tap, TrackWord, WordRef};
use serde_json::json;
use soundcraft_model::{ClipId, Session, TrackKind, WordTiming};
use soundcraft_time::Range;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            query "lyrics.words",
            "Timed Words",
            [],
            None,
            "{track?} — the track's words in time order: {clip, line, word, start, end, seconds, text, timing (heard | guess | confirmed), sheet_line}, and how many are still guessed / confirmed",
            always,
            words
        ),
        cmd!(
            "lyrics.word_set",
            "Set Word Time",
            [],
            None,
            "{clip, line, word, start?, end?, text?, alone?: bool, snap?: bool | ms} — moves a word's start and/or end (timeline) or retypes it; a neighbour whose edge touches follows unless `alone`, otherwise edges stop at the neighbours; `snap` (true: 60 ms) moves each edge to the nearest note start/end or breath edge first. The word becomes confirmed",
            always,
            word_set
        ),
        cmd!(
            "lyrics.word_move",
            "Move Word",
            [],
            None,
            "{clip, line, word, by (samples) | by_ms, alone?: bool} — moves a whole word (nudge); touching neighbours' edges follow unless `alone`",
            always,
            word_move
        ),
        cmd!(
            "lyrics.word_split",
            "Split Word",
            [],
            None,
            "{clip, line, word, at? (default its middle), texts?: [first, second] (default split by syllable: \"A-\" \"men\")} — one word into two",
            always,
            word_split
        ),
        cmd!(
            "lyrics.word_merge",
            "Merge with Next Word",
            [],
            None,
            "{clip, line, word} — joins a word with the next word of its line",
            always,
            word_merge
        ),
        cmd!(
            "lyrics.word_delete",
            "Delete Words",
            [],
            None,
            "{clip, line, word} or {words: [{clip, line, word}]} — removes words",
            always,
            word_delete
        ),
        cmd!(
            "lyrics.word_insert",
            "Insert Word",
            [],
            None,
            "{text, track?, at? (default the playhead), end?} — a confirmed word on the clip playing there, 300 ms long by default (up to the next word)",
            always,
            word_insert
        ),
        cmd!(
            "lyrics.words_confirm",
            "Confirm Word Times",
            [],
            None,
            "{words: [{clip, line, word}]} or {track?, all: true} — marks guessed words as checked without moving them",
            always,
            words_confirm
        ),
        cmd!(
            "lyrics.time_words",
            "Time Words to Vocal",
            [],
            None,
            "{track? (default the selected track, else the first vocal track), lines?: [ids] | number?, force?: bool} — spreads each placed lyric line's typed words over the notes sung in its range on the track (syllables from the dictionary, word starts on note onsets and consonants, never in a breath, a melisma kept on its word, ends where the voice stops); the words are guesses until checked. Unplaced lines are first placed from transcribed words when the track has them. A line with hand-timed words is left alone unless `force`. Analyses the track first if needed",
            has_sheet_lines,
            time_words
        ),
        cmd!(
            "lyrics.tap_words",
            "Tap Words",
            [],
            None,
            "{line (sheet id) | number, taps: [[down, up?], …] (timeline samples), track?, latency_ms? (default 0: taken off every tap), snap_ms? (default 60; 0: raw)} — the line's words at the tapped times, confirmed: each start snapped to the nearest note onset, each end at the key-up or where the voice stops before the next word. Fewer taps than words: the first words only",
            has_sheet_lines,
            tap_words
        ),
        cmd!(
            "lyrics.tap_lines",
            "Tap Line Starts",
            [],
            None,
            "{taps: [{line (id) | number, at}], track?, latency_ms?, snap_ms?} — places each line on the Lyrics ruler from its tap to the next tap (the last: to where its singing ends), then times its words as `lyrics.time_words` does",
            has_sheet_lines,
            tap_lines
        ),
        cmd!(
            noundo "lyrics.select_words",
            "Select Words",
            [],
            None,
            "{track?, which?: all | first_of_line | last_of_line | guessed | same_word | find, text? (same_word: the word; find: words or a phrase), words?: [{clip, line, word}], play?: bool} — the matching words (or `words`): their span becomes the edit selection and the playhead goes to the first; with `play`, plays just that span → {words: [{clip, line, word, start, end, text}], start, end}",
            always,
            select_words
        ),
    ]
}

fn has_sheet_lines(e: &Engine) -> std::result::Result<(), String> {
    if e.session().lyric_sheet.kinds().iter().any(|k| matches!(k, soundcraft_model::LineKind::Lyric(_))) {
        Ok(())
    } else {
        Err("type the lyrics first (Window › Transport & Session › Lyrics)".into())
    }
}

/// The word `clip`/`line`/`word` (or `p[key]` holding them) name.
fn word_ref(cmd: &str, p: &Value) -> Result<WordRef> {
    let get = |k: &str| p.get(k).and_then(Value::as_u64).ok_or_else(|| bad(cmd, format!("`{k}` required (as lyrics.words lists it)")));
    let idx = |v: u64| usize::try_from(v).unwrap_or(usize::MAX);
    Ok(WordRef { clip: ClipId(get("clip")?), line: idx(get("line")?), word: idx(get("word")?) })
}

fn refs_param(cmd: &str, p: &Value) -> Result<Vec<WordRef>> {
    match p.get("words").and_then(Value::as_array) {
        Some(a) => a.iter().map(|v| word_ref(cmd, v)).collect(),
        None => Ok(vec![word_ref(cmd, p)?]),
    }
}

fn exists(e: &Engine, cmd: &str, r: WordRef) -> Result<()> {
    wt::word(e.session(), r).map(|_| ()).ok_or_else(|| bad(cmd, "no such word (lyrics.words lists them)"))
}

/// The track timing acts on: `track`, else the first selected audio track, else the first vocal
/// track with clips, else the first track with timed words.
pub fn timing_track(e: &Engine, cmd: &str, p: &Value) -> Result<TrackId> {
    if let Some(t) = track_param(e, cmd, p, "track")? {
        return Ok(t);
    }
    let s = e.session();
    let audio = |t: &soundcraft_model::Track| t.kind == TrackKind::Audio && !t.clips().is_empty();
    s.tracks
        .iter()
        .find(|t| s.edit.selected_tracks.contains(&t.id) && audio(t))
        .or_else(|| s.tracks.iter().find(|t| t.is_vocal() && audio(t)))
        .or_else(|| s.tracks.iter().find(|t| soundcraft_model::track_has_lyrics(t)))
        .map(|t| t.id)
        .ok_or_else(|| bad(cmd, "select the vocal track"))
}

fn word_json(s: &Session, w: &TrackWord) -> Value {
    let rate = s.sample_rate.as_f64().max(1.0);
    json!({"clip": w.r.clip, "line": w.r.line, "word": w.r.word, "start": w.start, "end": w.end, "seconds": w.start as f64 / rate,
           "text": w.text, "timing": w.timing.name(), "sheet_line": w.sheet_line})
}

fn edited_json(r: &wt::Edited) -> Value {
    json!({"clip": r.r.clip, "line": r.r.line, "word": r.r.word, "start": r.start, "end": r.end})
}

fn words(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = timing_track(e, "lyrics.words", p)?;
    let s = e.session();
    let list = wt::track_words(s, t);
    let (guessed, confirmed) = wt::counts(s, t);
    Ok(json!({"track": t, "words": list.iter().map(|w| word_json(s, w)).collect::<Vec<_>>(), "guessed": guessed, "confirmed": confirmed,
              "heard": list.len() - guessed - confirmed}))
}

/// `snap`: true (60 ms), a number of ms, or false/absent (none).
fn snap_param(s: &Session, p: &Value, key: &str) -> Option<Samples> {
    match p.get(key) {
        Some(Value::Bool(true)) => Some(s.sample_rate.samples(wt::SNAP_SECS)),
        Some(v) => v.as_f64().filter(|ms| *ms > 0.0 && ms.is_finite()).map(|ms| s.sample_rate.samples(ms.min(1000.0) / 1000.0)),
        None => None,
    }
}

fn word_set(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_set";
    let mut r = word_ref(C, p)?;
    exists(e, C, r)?;
    let (start, end) = (position_param(e, C, p, "start")?, position_param(e, C, p, "end")?);
    let text = str_param(p, "text");
    if start.is_none() && end.is_none() && text.is_none() {
        return Err(bad(C, "give `start`, `end` or `text`"));
    }
    let snap = snap_param(e.session(), p, "snap");
    let alone = bool_or(p, "alone", false);
    let mut out = None;
    if let Some(text) = text {
        let ed = wt::set_text(e.session_mut(), r, text).map_err(|m| bad(C, m))?;
        r = ed.r;
        out = Some(ed);
    }
    if start.is_some() || end.is_some() {
        out = Some(wt::set_edges(e.session_mut(), r, start, end, alone, snap).map_err(|m| bad(C, m))?);
    }
    out.map(|ed| edited_json(&ed)).ok_or_else(|| bad(C, "nothing changed"))
}

fn word_move(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_move";
    let r = word_ref(C, p)?;
    exists(e, C, r)?;
    let by = match (p.get("by").and_then(Value::as_i64), p.get("by_ms").and_then(Value::as_f64).filter(|v| v.is_finite())) {
        (Some(b), _) => b,
        (None, Some(ms)) => soundcraft_time::to_samples(ms / 1000.0 * e.session().sample_rate.as_f64()),
        _ => return Err(bad(C, "`by` (samples) or `by_ms` required")),
    };
    let ed = wt::move_word(e.session_mut(), r, by, bool_or(p, "alone", false)).map_err(|m| bad(C, m))?;
    Ok(edited_json(&ed))
}

fn word_split(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_split";
    let r = word_ref(C, p)?;
    exists(e, C, r)?;
    let at = position_param(e, C, p, "at")?;
    let texts = p.get("texts").and_then(Value::as_array).and_then(|a| Some((a.first()?.as_str()?.to_string(), a.get(1)?.as_str()?.to_string())));
    let (a, b) = wt::split(e.session_mut(), r, at, texts).map_err(|m| bad(C, m))?;
    Ok(json!({"first": edited_json(&a), "second": edited_json(&b)}))
}

fn word_merge(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_merge";
    let r = word_ref(C, p)?;
    exists(e, C, r)?;
    let ed = wt::merge(e.session_mut(), r).map_err(|m| bad(C, m))?;
    Ok(edited_json(&ed))
}

fn word_delete(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_delete";
    let refs = refs_param(C, p)?;
    for r in &refs {
        exists(e, C, *r)?;
    }
    let n = wt::delete(e.session_mut(), &refs);
    Ok(json!({"removed": n}))
}

fn word_insert(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.word_insert";
    let text = str_param(p, "text").ok_or_else(|| bad(C, "`text` required"))?;
    let t = timing_track(e, C, p)?;
    let at = position_param(e, C, p, "at")?.unwrap_or(e.session().edit.playhead);
    let end = position_param(e, C, p, "end")?;
    let ed = wt::insert(e.session_mut(), t, at, end, text).map_err(|m| bad(C, m))?;
    Ok(edited_json(&ed))
}

fn words_confirm(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.words_confirm";
    let refs = if bool_or(p, "all", false) {
        let t = timing_track(e, C, p)?;
        wt::track_words(e.session(), t).into_iter().map(|w| w.r).collect()
    } else {
        refs_param(C, p)?
    };
    let n = wt::confirm(e.session_mut(), &refs);
    Ok(json!({"confirmed": n}))
}

/// Sheet line ids from `lines` (ids) or `number`.
fn lines_param(e: &Engine, cmd: &str, p: &Value) -> Result<Vec<u64>> {
    if let Some(a) = p.get("lines").and_then(Value::as_array) {
        let ids: Vec<u64> = a.iter().filter_map(Value::as_u64).collect();
        if let Some(bad_id) = ids.iter().find(|id| e.session().lyric_sheet.number_of(**id).is_none()) {
            return Err(bad(cmd, format!("no lyric line with id {bad_id}")));
        }
        return Ok(ids);
    }
    Ok(super::lyric_place::line_param(e.session(), cmd, p)?.into_iter().collect())
}

fn time_words(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.time_words";
    let t = timing_track(e, C, p)?;
    if e.session().track(t).is_some_and(|tr| tr.kind != TrackKind::Audio) {
        return Err(bad(C, "choose an audio track (the vocal)"));
    }
    let lines = lines_param(e, C, p)?;
    // Lines not on the ruler yet: from transcribed words when the track has some.
    let mut placed_from_transcript = 0;
    let unplaced = super::lyric_place::unplaced(e.session());
    let heard = wt::track_words(e.session(), t).iter().any(|w| w.timing == WordTiming::Heard);
    if !unplaced.is_empty() && heard {
        let r = super::lyric_place::place_transcribed(e, &json!({"track": t}), &unplaced, Range::new(0, 0));
        placed_from_transcript = r.ok().and_then(|v| v["placed"].as_array().map(Vec::len)).unwrap_or(0);
    }
    if e.session().lyric_sheet.placed.is_empty() {
        return Err(bad(C, "no lyric line is on the Lyrics ruler yet: place them (Lyrics › Place lines on ruler, or Tap Line Starts) first"));
    }
    wt::analyse_track(e, t);
    let force = bool_or(p, "force", false);
    let done = wt::time_lines(e.session_mut(), t, &lines, force);
    if done.iter().all(|l| l.words == 0) && done.iter().all(|l| l.skipped.as_deref().is_some_and(|w| w.starts_with("no singing"))) {
        return Err(bad(C, "no singing found under the placed lines on that track (is it the vocal, and analysed?)"));
    }
    let still_unplaced = super::lyric_place::unplaced(e.session()).len();
    let (guessed, confirmed) = wt::counts(e.session(), t);
    let timed: usize = done.iter().map(|l| l.words).sum();
    Ok(json!({"track": t, "lines": done, "words": timed, "guessed": guessed, "confirmed": confirmed, "unplaced": still_unplaced,
              "placed_from_transcript": placed_from_transcript}))
}

/// `latency_ms` in samples (0..500 ms).
fn latency_param(s: &Session, p: &Value) -> Samples {
    let ms = f64_or(p, "latency_ms", 0.0).clamp(0.0, 500.0);
    soundcraft_time::to_samples(ms / 1000.0 * s.sample_rate.as_f64())
}

/// `snap_ms` (default 60; 0 = raw taps).
fn tap_snap(s: &Session, p: &Value) -> Option<Samples> {
    let ms = f64_or(p, "snap_ms", wt::SNAP_SECS * 1000.0).clamp(0.0, 500.0);
    (ms > 0.0).then(|| soundcraft_time::to_samples(ms / 1000.0 * s.sample_rate.as_f64()))
}

fn tap_words(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.tap_words";
    let t = timing_track(e, C, p)?;
    let line = super::lyric_place::line_param(e.session(), C, p)?.ok_or_else(|| bad(C, "`line` (id) or `number` required"))?;
    let lat = latency_param(e.session(), p);
    let taps: Vec<Tap> = p
        .get("taps")
        .and_then(Value::as_array)
        .ok_or_else(|| bad(C, "`taps` required: [[down, up?], …]"))?
        .iter()
        .filter_map(|v| {
            let a = v.as_array();
            let down = a.and_then(|a| a.first()).and_then(Value::as_i64).or_else(|| v.as_i64())?;
            let up = a.and_then(|a| a.get(1)).and_then(Value::as_i64);
            Some(Tap { down: (down - lat).max(0), up: up.map(|u| (u - lat).max(0)) })
        })
        .collect();
    let words = wt::sheet_words(e.session(), line);
    if taps.is_empty() || words.is_empty() {
        return Err(bad(C, "no taps, or the line has no words"));
    }
    wt::analyse_track(e, t);
    let s = e.session();
    let (segs, _) = wt::track_segs(s, t);
    let times = wt::tap_times(&taps, &segs, tap_snap(s, p), s.sample_rate.as_f64());
    let ws: Vec<(Samples, Samples, String)> = times.iter().zip(&words).map(|((a, b), w)| (*a, *b, w.clone())).collect();
    let span = Range::new(ws.first().map_or(0, |w| w.0), ws.last().map_or(0, |w| w.1));
    clear_line(e.session_mut(), t, line, span);
    let n = wt::put_words(e.session_mut(), t, Some(line), &ws, WordTiming::Confirmed);
    let (guessed, confirmed) = wt::counts(e.session(), t);
    Ok(json!({"track": t, "line": line, "words": n, "of": words.len(), "times": times, "guessed": guessed, "confirmed": confirmed}))
}

/// Before tapping a line in again: its earlier words, and anything else in the tapped span.
fn clear_line(s: &mut Session, t: TrackId, line: u64, span: Range) {
    let refs: Vec<WordRef> = wt::track_words(s, t)
        .into_iter()
        .filter(|w| w.sheet_line == Some(line) || (w.start >= span.start && w.start < span.end))
        .map(|w| w.r)
        .collect();
    wt::delete(s, &refs);
}

fn tap_lines(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.tap_lines";
    let t = timing_track(e, C, p)?;
    let lat = latency_param(e.session(), p);
    let arr = p.get("taps").and_then(Value::as_array).ok_or_else(|| bad(C, "`taps` required: [{line, at}, …]"))?;
    let mut taps: Vec<(u64, Samples)> = Vec::new();
    for v in arr {
        let line = super::lyric_place::line_param(e.session(), C, v)?.ok_or_else(|| bad(C, "each tap needs `line` or `number`"))?;
        let at = v.get("at").and_then(Value::as_i64).ok_or_else(|| bad(C, "each tap needs `at` (samples)"))?;
        taps.push((line, (at - lat).max(0)));
    }
    if taps.is_empty() {
        return Err(bad(C, "no taps"));
    }
    taps.sort_by_key(|(_, at)| *at);
    wt::analyse_track(e, t);
    let s = e.session();
    let (segs, _) = wt::track_segs(s, t);
    let snap = tap_snap(s, p);
    let starts: Vec<Samples> = segs.iter().map(|g| g.lead).collect();
    let mut placed = Vec::new();
    for (i, (line, at)) in taps.iter().enumerate() {
        let start = snap.map_or(*at, |w| wt::snap_to(*at, &starts, w));
        let next = taps.get(i + 1).map(|(_, n)| *n);
        // The last line runs to where its singing ends (the next long gap), at most 30 s.
        let end = next.unwrap_or_else(|| {
            let limit = start + e.session().sample_rate.samples(30.0);
            let mut end = start;
            for g in segs.iter().filter(|g| g.start >= start && g.start < limit) {
                if end > start && g.gap_before > e.session().sample_rate.samples(1.0) {
                    break;
                }
                end = g.end;
            }
            end.max(start + 1)
        });
        placed.push((*line, start, end.max(start + 1)));
    }
    let s = e.session_mut();
    for (line, a, b) in &placed {
        super::lyric_place::set_place(s, *line, *a, *b);
    }
    let ids: Vec<u64> = placed.iter().map(|(l, _, _)| *l).collect();
    let done = wt::time_lines(e.session_mut(), t, &ids, false);
    let (guessed, confirmed) = wt::counts(e.session(), t);
    Ok(json!({"track": t, "placed": placed.iter().map(|(l, a, b)| json!({"line": l, "start": a, "end": b})).collect::<Vec<_>>(),
              "lines": done, "guessed": guessed, "confirmed": confirmed}))
}

/// A word as compared: lower case, letters and digits only.
fn fold(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// The words of `track` that `which` picks (see `lyrics.select_words`).
pub fn pick_words(s: &Session, track: TrackId, which: &str, text: &str) -> std::result::Result<Vec<TrackWord>, String> {
    let all = wt::track_words(s, track);
    // Lines as the words are grouped on their clips.
    let line_of = |w: &TrackWord| (w.r.clip, w.r.line);
    Ok(match which {
        "all" => all,
        "guessed" => all.into_iter().filter(|w| w.timing == WordTiming::Guess).collect(),
        "first_of_line" => all
            .iter()
            .enumerate()
            .filter(|(i, w)| *i == 0 || all.get(i - 1).is_none_or(|p| line_of(p) != line_of(w)))
            .map(|(_, w)| w.clone())
            .collect(),
        "last_of_line" => {
            all.iter().enumerate().filter(|(i, w)| all.get(i + 1).is_none_or(|n| line_of(n) != line_of(w))).map(|(_, w)| w.clone()).collect()
        }
        "same_word" => {
            let want = fold(text);
            if want.is_empty() {
                return Err("give the word (`text`)".into());
            }
            all.into_iter().filter(|w| fold(&w.text) == want).collect()
        }
        "find" => {
            let want: Vec<String> = text.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
            if want.is_empty() {
                return Err("give the words to find (`text`)".into());
            }
            let folded: Vec<String> = all.iter().map(|w| fold(&w.text)).collect();
            let mut out = Vec::new();
            for k in 0..all.len() {
                if want.iter().enumerate().all(|(j, w)| folded.get(k + j) == Some(w)) {
                    out.extend(all.iter().skip(k).take(want.len()).cloned());
                }
            }
            out.dedup_by_key(|w| w.r);
            out
        }
        other => return Err(format!("unknown `which` `{other}` (all, first_of_line, last_of_line, guessed, same_word, find)")),
    })
}

fn select_words(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.select_words";
    let t = timing_track(e, C, p)?;
    let picked: Vec<TrackWord> = if p.get("words").is_some() {
        let refs = refs_param(C, p)?;
        let all = wt::track_words(e.session(), t);
        let mut v: Vec<TrackWord> = refs.iter().filter_map(|r| all.iter().find(|w| w.r == *r).cloned()).collect();
        v.sort_by_key(|w| w.start);
        v
    } else {
        pick_words(e.session(), t, str_param(p, "which").unwrap_or("all"), str_param(p, "text").unwrap_or("")).map_err(|m| bad(C, m))?
    };
    let list: Vec<Value> = picked
        .iter()
        .map(|w| json!({"clip": w.r.clip, "line": w.r.line, "word": w.r.word, "start": w.start, "end": w.end, "text": w.text}))
        .collect();
    let (Some(a), Some(b)) = (picked.iter().map(|w| w.start).min(), picked.iter().map(|w| w.end).max()) else {
        return Ok(json!({"words": list}));
    };
    let st = e.session_mut();
    st.edit.selection = Range::new(a, b);
    st.edit.selected_tracks = vec![t];
    st.edit.playhead = a;
    if bool_or(p, "play", false) {
        e.transport_requests.push(crate::TransportRequest::PlayRange { from: a, end: Some(b), looped: None });
    } else {
        e.transport_requests.push(crate::TransportRequest::Locate(a));
    }
    Ok(json!({"words": list, "start": a, "end": b}))
}

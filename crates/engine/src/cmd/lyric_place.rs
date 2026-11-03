//! The Lyrics ruler: lyric-sheet lines placed in time as range markers titled by the line's
//! current text (`LyricSheet::placed`). Place, move, retime, remove, spread over a range, and make
//! a placed line's range the edit selection (to loop, punch or comp it).

use super::*;
use crate::cmd;
use serde_json::json;
use soundcraft_model::{LineKind, LyricPlace, Session};
use soundcraft_time::Range;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "lyrics.place",
            "Place Lyric Line",
            [],
            None,
            "{line? (id) or number?, start?, end?} — places a lyric line on the Lyrics ruler (moves it when placed). Without a line: the next unplaced one. Without start/end: the edit selection; from a point, up to the next placed line, at most 2 bars",
            has_lines,
            place
        ),
        cmd!("lyrics.unplace", "Remove Lyric Marker", [], None, "{line? (id) or number?} — takes a line off the Lyrics ruler", has_places, unplace),
        cmd!(
            "lyrics.place_lines",
            "Place Lines on Ruler",
            [],
            None,
            "{count? (default: every unplaced line), start?, end? (default the edit selection), from? (even|transcribed), track?} — places the next unplaced lyric lines over the range: spread evenly, or at the times Transcribe Lyrics found for their words",
            has_lines,
            place_lines
        ),
        cmd!(
            "lyrics.select_line",
            "Select Lyric Line",
            [],
            None,
            "{line? (id) or number?} — makes a placed line's time range the edit selection (to loop, punch or comp it); without a line, the one at the insertion point",
            has_places,
            select_line
        ),
    ]
}

fn has_lines(e: &Engine) -> std::result::Result<(), String> {
    if e.session().lyric_sheet.kinds().iter().any(|k| matches!(k, LineKind::Lyric(_))) {
        Ok(())
    } else {
        Err("the lyric sheet has no lines (Window › Lyrics)".into())
    }
}

fn has_places(e: &Engine) -> std::result::Result<(), String> {
    if e.session().lyric_sheet.placed.is_empty() { Err("no lyric line is on the Lyrics ruler".into()) } else { Ok(()) }
}

/// The lyric line `line` (id) or `number` names.
fn line_param(s: &Session, cmd: &str, p: &Value) -> Result<Option<u64>> {
    let sheet = &s.lyric_sheet;
    if let Some(id) = p.get("line").and_then(Value::as_u64) {
        return sheet.number_of(id).map(|_| Some(id)).ok_or_else(|| bad(cmd, format!("no lyric line with id {id}")));
    }
    if let Some(n) = p.get("number").and_then(Value::as_u64) {
        let n = u32::try_from(n).unwrap_or(u32::MAX);
        return sheet.line_numbered(n).map(|l| Some(l.id)).ok_or_else(|| bad(cmd, format!("no lyric line {n}")));
    }
    Ok(None)
}

/// The lyric lines (ids) in sheet order.
pub fn lyric_ids(s: &Session) -> Vec<u64> {
    let sheet = &s.lyric_sheet;
    sheet.lines.iter().zip(sheet.kinds()).filter(|(_, k)| matches!(k, LineKind::Lyric(_))).map(|(l, _)| l.id).collect()
}

/// The lyric lines not on the ruler, in sheet order from the one after the last placed line
/// (then round from the top).
pub fn unplaced(s: &Session) -> Vec<u64> {
    let sheet = &s.lyric_sheet;
    let lyric = lyric_ids(s);
    let last = sheet.placed.iter().max_by_key(|p| p.start).and_then(|p| lyric.iter().position(|id| *id == p.line));
    let from = last.map_or(0, |i| i + 1);
    lyric.iter().skip(from).chain(lyric.iter().take(from)).copied().filter(|id| sheet.place_of(*id).is_none()).collect()
}

/// `start` plus two bars of the meter there.
fn two_bars(s: &Session, start: Samples) -> Samples {
    let tick = s.tempo.samples_to_ticks(start, s.sample_rate);
    let bar = s.tempo.meter_at_tick(tick).ticks_per_bar();
    s.tempo.tick_to_samples(tick.saturating_add(bar.saturating_mul(2)), s.sample_rate).max(start.saturating_add(1))
}

pub(crate) fn set_place(s: &mut Session, line: u64, start: Samples, end: Samples) {
    let sheet = &mut s.lyric_sheet;
    sheet.placed.retain(|p| p.line != line);
    let start = start.max(0);
    sheet.placed.push(LyricPlace { line, start, end: end.max(start.saturating_add(1)) });
    sheet.tidy_places();
}

fn place(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.place";
    let s = e.session();
    let line = match line_param(s, C, p)? {
        Some(l) => l,
        None => *unplaced(s).first().ok_or_else(|| bad(C, "every lyric line is placed"))?,
    };
    let sel = s.edit.selection;
    let start = position_param(e, C, p, "start")?.unwrap_or(sel.start).max(0);
    let end = match position_param(e, C, p, "end")? {
        Some(end) => end,
        None if p.get("start").is_none() && sel.end > sel.start => sel.end,
        None => {
            let next = s.lyric_sheet.placed.iter().filter(|q| q.line != line && q.start > start).map(|q| q.start).min();
            let bars = two_bars(s, start);
            next.map_or(bars, |n| n.min(bars))
        }
    };
    if end <= start {
        return Err(bad(C, "the end must be after the start"));
    }
    set_place(e.session_mut(), line, start, end);
    let number = e.session().lyric_sheet.number_of(line);
    Ok(json!({"line": line, "number": number, "start": start, "end": end}))
}

/// The line `line`/`number` names, else the placed one at the insertion point.
fn placed_line(s: &Session, cmd: &str, p: &Value) -> Result<u64> {
    let line = match line_param(s, cmd, p)? {
        Some(l) => l,
        None => s.lyric_sheet.placed_at(s.edit.selection.start).map(|q| q.line).ok_or_else(|| bad(cmd, "give `line` or `number`"))?,
    };
    if s.lyric_sheet.place_of(line).is_none() {
        return Err(bad(cmd, "that line is not on the Lyrics ruler"));
    }
    Ok(line)
}

fn unplace(e: &mut Engine, p: &Value) -> Result<Value> {
    let line = placed_line(e.session(), "lyrics.unplace", p)?;
    e.session_mut().lyric_sheet.placed.retain(|q| q.line != line);
    Ok(json!({"line": line}))
}

fn place_lines(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.place_lines";
    let r = super::range_param(e, C, p)?;
    let mut lines = unplaced(e.session());
    if let Some(n) = p.get("count").and_then(Value::as_u64) {
        lines.truncate(usize::try_from(n).unwrap_or(usize::MAX));
    }
    if lines.is_empty() {
        return Err(bad(C, "every lyric line is placed"));
    }
    if str_param(p, "from") == Some("transcribed") {
        return place_transcribed(e, p, &lines, r);
    }
    if r.end <= r.start {
        return Err(bad(C, "select a time range to spread the lines over"));
    }
    let n = lines.len() as i64;
    let len = r.end - r.start;
    let s = e.session_mut();
    let mut placed = Vec::new();
    for (i, line) in lines.iter().enumerate() {
        let i = i as i64;
        let (a, b) = (r.start + len.saturating_mul(i) / n, r.start + len.saturating_mul(i + 1) / n);
        if b > a {
            set_place(s, *line, a, b);
            placed.push(json!({"line": line, "start": a, "end": b}));
        }
    }
    Ok(json!({"placed": placed}))
}

/// A word as matched: lowercase letters and digits only.
fn norm(w: &str) -> String {
    w.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// Where `line` (normalised words) was sung in `heard` (normalised words), from index `from`:
/// the first run of heard words starting with one of the line's first two words that holds at
/// least 60% of the line's words in order. Returns the first and last heard index of the run.
pub fn match_line(line: &[String], heard: &[String], from: usize) -> Option<(usize, usize)> {
    let n = line.len();
    if n == 0 {
        return None;
    }
    let need = (n * 3).div_ceil(5).max(1);
    let window = n + n / 2 + 2;
    for i in from..heard.len() {
        let first = heard.get(i)?;
        if !line.iter().take(2).any(|w| w == first) {
            continue;
        }
        // In-order greedy match of the line's words over the window.
        let (mut k, mut matched, mut last) = (0usize, 0usize, i);
        for (j, h) in heard.iter().enumerate().skip(i).take(window) {
            if let Some(pos) = line.iter().skip(k).position(|w| w == h) {
                k += pos + 1;
                matched += 1;
                last = j;
                if k >= n {
                    break;
                }
            }
        }
        if matched >= need {
            return Some((i, last));
        }
    }
    None
}

/// `place_lines` from Transcribe Lyrics' timed words: each line takes the time of the run of
/// words it matches on the track (within the range when one is given), in order.
fn place_transcribed(e: &mut Engine, p: &Value, lines: &[u64], r: Range) -> Result<Value> {
    const C: &str = "lyrics.place_lines";
    let s = e.session();
    let asked = track_param(e, C, p, "track")?;
    let track = crate::lyrics::lyrics_track(s, asked).ok_or_else(|| bad(C, "no track has transcribed lyrics (AI › Transcribe Lyrics…)"))?;
    let in_range = |at: Samples| r.end <= r.start || (at >= r.start && at < r.end);
    let words: Vec<soundcraft_model::LyricWord> = s
        .track(track)
        .map(|t| soundcraft_model::track_lines(t).into_iter().flat_map(|l| l.words).filter(|w| in_range(w.start)).collect())
        .unwrap_or_default();
    if words.is_empty() {
        return Err(bad(C, "that track has no transcribed words there"));
    }
    let heard: Vec<String> = words.iter().map(|w| norm(&w.text)).collect();
    // The lines in sheet order (matching runs forward through the song).
    let order = lyric_ids(s);
    let mut todo: Vec<u64> = lines.to_vec();
    todo.sort_by_key(|id| order.iter().position(|o| o == id));
    let texts: Vec<(u64, Vec<String>)> = todo
        .iter()
        .map(|id| {
            let text = s.lyric_sheet.lines.iter().find(|l| l.id == *id).map_or("", |l| l.text.as_str());
            (*id, text.split_whitespace().map(norm).filter(|w| !w.is_empty()).collect())
        })
        .collect();
    let mut found = Vec::new();
    let mut from = 0;
    for (id, ws) in texts {
        if let Some((a, b)) = match_line(&ws, &heard, from)
            && let (Some(wa), Some(wb)) = (words.get(a), words.get(b))
        {
            found.push((id, wa.start, wb.end.max(wa.start.saturating_add(1))));
            from = b + 1;
        }
    }
    let s = e.session_mut();
    let mut placed = Vec::new();
    for (id, a, b) in &found {
        set_place(s, *id, *a, *b);
        placed.push(json!({"line": id, "start": a, "end": b}));
    }
    let missed = lines.len() - found.len();
    Ok(json!({"placed": placed, "track": track, "unmatched": missed}))
}

/// The vocal track whose comp the Lyrics ruler shows: the first selected comping track, else the
/// track with transcribed lyrics when it comps, else the first comping track.
pub fn comp_track(s: &Session) -> Option<soundcraft_model::TrackId> {
    let comps = |t: &soundcraft_model::Track| super::comp::comping(t) && t.playlists.len() > 1;
    s.tracks
        .iter()
        .find(|t| s.edit.selected_tracks.contains(&t.id) && comps(t))
        .or_else(|| crate::lyrics::lyrics_track(s, None).and_then(|id| s.track(id)).filter(|t| comps(t)))
        .or_else(|| s.tracks.iter().find(|t| comps(t)))
        .map(|t| t.id)
}

/// The take layers the comp of `track` plays over `start..end`, in time order (names, distinct).
pub fn comp_takes(s: &Session, track: soundcraft_model::TrackId, start: Samples, end: Samples) -> Vec<String> {
    let Some(t) = s.track(track) else { return Vec::new() };
    let mut out: Vec<String> = Vec::new();
    for c in t.clips().iter().filter(|c| c.start < end && c.end() > start) {
        let Some(name) = c.from_layer.and_then(|l| t.playlists.get(l)).map(|p| p.name.clone()) else { continue };
        if !out.contains(&name) {
            out.push(name);
        }
    }
    out
}

fn select_line(e: &mut Engine, p: &Value) -> Result<Value> {
    let line = placed_line(e.session(), "lyrics.select_line", p)?;
    let Some(q) = e.session().lyric_sheet.place_of(line) else { return Err(bad("lyrics.select_line", "not placed")) };
    e.session_mut().edit.selection = Range::new(q.start, q.end);
    let s = e.session();
    let takes = comp_track(s).map(|t| comp_takes(s, t, q.start, q.end)).unwrap_or_default();
    Ok(json!({"line": line, "start": q.start, "end": q.end, "takes": takes}))
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;

    const SONG: &str =
        "[Verse 1]\nWhen the night was long\nAnd my heart could not be still\nYou were there to sing the song\nStanding strong upon the hill";

    #[test]
    fn placing_retitles_deletes_and_undoes_with_the_line() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        // L with a range selected: the next unplaced line (line 1) over it.
        e.session_mut().edit.selection = soundcraft_time::Range::new(48_000, 96_000);
        let v = e.execute("lyrics.place", &json!({})).unwrap();
        assert_eq!((v["number"].as_u64(), v["start"].as_i64(), v["end"].as_i64()), (Some(1), Some(48_000), Some(96_000)));
        // From a point: two bars when nothing follows, else up to the next placed line.
        let v = e.execute("lyrics.place", &json!({"number": 3, "start": 192_000})).unwrap();
        assert!(v["end"].as_i64().unwrap() > 192_000);
        let v = e.execute("lyrics.place", &json!({"number": 2, "start": 100_000})).unwrap();
        assert_eq!(v["end"], 192_000);
        // The next unplaced line after the last placed one (line 3) is line 4.
        assert_eq!(super::unplaced(e.session()), vec![e.session().lyric_sheet.line_numbered(4).unwrap().id]);
        // Edit line 2's text: its marker stays, titled by the new text (sheet_get shows it placed).
        let id2 = e.session().lyric_sheet.line_numbered(2).unwrap().id;
        e.execute("lyrics.sheet_set", &json!({"text": SONG.replace("could not be still", "would not be still")})).unwrap();
        assert_eq!(e.session().lyric_sheet.place_of(id2).map(|q| q.start), Some(100_000));
        let got = e.execute("lyrics.sheet_get", &json!({})).unwrap();
        assert_eq!(got["lines"][2]["placed"]["start"], 100_000);
        assert_eq!(got["lines"][2]["text"], "And my heart would not be still");
        // Delete line 2: its marker goes; undo brings both back.
        e.execute("lyrics.sheet_set", &json!({"text": SONG.replace("And my heart could not be still\n", "")})).unwrap();
        assert!(e.session().lyric_sheet.place_of(id2).is_none());
        assert_eq!(e.session().lyric_sheet.placed.len(), 2);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().lyric_sheet.place_of(id2).map(|q| q.start), Some(100_000));
        // Drag / retime: placing again moves it.
        e.execute("lyrics.place", &json!({"line": id2, "start": 110_000, "end": 150_000})).unwrap();
        assert_eq!(e.session().lyric_sheet.place_of(id2).map(|q| (q.start, q.end)), Some((110_000, 150_000)));
        // Line to time.
        e.execute("lyrics.select_line", &json!({"number": 2})).unwrap();
        assert_eq!(e.session().edit.selection, soundcraft_time::Range::new(110_000, 150_000));
        e.execute("lyrics.unplace", &json!({"line": id2})).unwrap();
        assert!(e.session().lyric_sheet.place_of(id2).is_none());
        assert!(e.execute("lyrics.select_line", &json!({"line": id2})).is_err());
        assert!(e.execute("lyrics.place", &json!({"number": 99})).is_err());
        assert!(e.execute("lyrics.place", &json!({"number": 1, "start": 50, "end": 10})).is_err());
    }

    #[test]
    fn lines_match_runs_of_transcribed_words() {
        let w = |s: &str| s.split(' ').map(super::norm).collect::<Vec<String>>();
        let heard = w("oh when the night was so long and my heart could not be still yeah");
        assert_eq!(super::match_line(&w("When the night was long"), &heard, 0), Some((1, 6)));
        assert_eq!(super::match_line(&w("And my heart could not be still"), &heard, 7), Some((7, 13)));
        assert_eq!(super::match_line(&w("Standing strong upon the hill"), &heard, 0), None);
        assert_eq!(super::match_line(&[], &heard, 0), None);
    }

    #[test]
    fn place_lines_from_transcribed_words() {
        use crate::lyrics::TimedWord;
        let mut e = Engine::default();
        let rate = e.session().sample_rate;
        let t = {
            let s = e.session_mut();
            let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("Vox"));
            let id = s.new_clip_id();
            if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
                pl.clips.push(soundcraft_model::Clip::midi(id, "Vox", 0, rate.samples(20.0), Default::default()));
            }
            t
        };
        let tw = |a: f64, s: &str| TimedWord { start: a, end: a + 0.4, text: s.into() };
        let line1 = vec![tw(1.0, "When"), tw(1.5, "the"), tw(2.0, "night"), tw(2.5, "was"), tw(3.0, "long")];
        let line2 = vec![tw(5.0, "And"), tw(5.5, "my"), tw(6.0, "heart"), tw(6.5, "could"), tw(7.0, "not"), tw(7.5, "be"), tw(8.0, "still")];
        let len = rate.samples(20.0);
        crate::lyrics::place_lyrics(e.session_mut(), t, 0, len, &[line1, line2]);
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        let v = e.execute("lyrics.place_lines", &json!({"from": "transcribed"})).unwrap();
        assert_eq!(v["placed"].as_array().unwrap().len(), 2, "{v}");
        assert_eq!(v["unmatched"], 2);
        let id1 = e.session().lyric_sheet.line_numbered(1).unwrap().id;
        let q = e.session().lyric_sheet.place_of(id1).unwrap();
        assert_eq!((q.start, q.end), (rate.samples(1.0), rate.samples(3.4)));
    }

    #[test]
    fn line_range_shows_the_comp_takes() {
        let mut e = Engine::default();
        let t = {
            let s = e.session_mut();
            let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some("Vox"));
            let (a, b) = (s.new_clip_id(), s.new_clip_id());
            if let Some(tr) = s.track_mut(t) {
                tr.layers_open = true;
                let mut take = tr.playlists[0].clone();
                take.name = "Take 2".into();
                tr.playlists.push(take);
                let mut c1 = soundcraft_model::Clip::midi(a, "Vox", 0, 100_000, Default::default());
                c1.from_layer = Some(1);
                let mut c2 = soundcraft_model::Clip::midi(b, "Vox", 100_000, 100_000, Default::default());
                c2.from_layer = Some(0);
                if let Some(pl) = tr.playlist_mut() {
                    pl.clips = vec![c1, c2];
                }
            }
            t
        };
        assert_eq!(super::comp_track(e.session()), Some(t));
        let first = e.session().track(t).unwrap().playlists[0].name.clone();
        assert_eq!(super::comp_takes(e.session(), t, 50_000, 150_000), vec!["Take 2".to_string(), first]);
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        e.execute("lyrics.place", &json!({"number": 1, "start": 10_000, "end": 90_000})).unwrap();
        let v = e.execute("lyrics.select_line", &json!({"number": 1})).unwrap();
        assert_eq!(v["takes"], json!(["Take 2"]));
    }

    #[test]
    fn place_lines_spreads_evenly() {
        let mut e = Engine::default();
        e.execute("lyrics.sheet_set", &json!({"text": SONG})).unwrap();
        let v = e.execute("lyrics.place_lines", &json!({"start": 0, "end": 400_000, "count": 4})).unwrap();
        let starts: Vec<i64> = v["placed"].as_array().unwrap().iter().map(|p| p["start"].as_i64().unwrap()).collect();
        assert_eq!(starts, vec![0, 100_000, 200_000, 300_000]);
        assert!(e.execute("lyrics.place_lines", &json!({"start": 0, "end": 10})).is_err());
    }
}

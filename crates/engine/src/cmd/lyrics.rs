//! A track's timed lyrics: export as `.lrc` or text, list, clear. Transcribe Lyrics (`ml.lyrics`)
//! fills them. See `crate::lyrics`.

use super::*;
use crate::cmd;
use serde_json::json;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            noundo "file.export_lyrics",
            "Lyrics (LRC)...",
            ["File", "Export"],
            None,
            "{path, track? (default: the selected track with lyrics, else the first track with lyrics)} — the track's lyrics as an LRC file: `[mm:ss.xx]` per line, `<mm:ss.xx>` per word",
            has_lyrics,
            |e, p| export(e, p, "file.export_lyrics", false)
        ),
        cmd!(
            noundo "file.export_lyrics_text",
            "Lyrics as Text...",
            ["File", "Export"],
            None,
            "{path, track?} — the track's lyrics as plain text, one line per line",
            has_lyrics,
            |e, p| export(e, p, "file.export_lyrics_text", true)
        ),
        cmd!(query "lyrics.list", "Lyrics", [], None, "{track?} — the track's lyric lines and words (times in samples and seconds)", always, list),
        cmd!(
            query "lyrics.search",
            "Search Transcript",
            [],
            None,
            "{text, tracks?} — every place the words of `text` are sung or spoken, in order, in the timed lyrics of the tracks (all by default): case, punctuation and line breaks ignored → {matches: [{track, name, start, end, seconds, line}]} in time order (at most 1000)",
            always,
            search
        ),
        cmd!(
            noundo "lyrics.find",
            "Find in Transcript...",
            [],
            None,
            "{text, backward?: bool} — selects the next place (previous with `backward`) after the insertion point where `text` is sung or spoken, on its track, and moves the playhead there; wraps around → {track, start, end, line, index, of}",
            has_lyrics,
            find
        ),
        cmd!(
            "lyrics.clear",
            "Clear Lyrics",
            [],
            None,
            "{track?, start?, end?} — removes the lyric lines of the track's clips (those starting in start..end on the timeline when given; without a range, all of them, hidden ones too)",
            always,
            clear
        ),
    ]
}

fn has_lyrics(e: &Engine) -> std::result::Result<(), String> {
    if e.session().tracks.iter().any(soundcraft_model::track_has_lyrics) {
        Ok(())
    } else {
        Err("no track has lyrics (AI › Transcribe Lyrics…)".into())
    }
}

fn which(e: &Engine, cmd: &str, p: &Value) -> Result<TrackId> {
    let asked = track_param(e, cmd, p, "track")?;
    crate::lyrics::lyrics_track(e.session(), asked).ok_or_else(|| bad(cmd, "no track has lyrics"))
}

fn export(e: &mut Engine, p: &Value, cmd: &str, text: bool) -> Result<Value> {
    let path = str_param(p, "path").filter(|s| !s.trim().is_empty()).ok_or_else(|| bad(cmd, "`path` required"))?.to_string();
    let t = which(e, cmd, p)?;
    let s = e.session();
    if s.track(t).is_some_and(|t| soundcraft_model::track_lines(t).is_empty()) {
        return Err(bad(cmd, "that track has no lyrics"));
    }
    let body = if text { crate::lyrics::plain_text(s, t) } else { crate::lyrics::lrc(s, t) };
    crate::io::write_file(&path, body.as_bytes())?;
    let lines = s.track(t).map_or(0, |t| soundcraft_model::track_lines(t).len());
    Ok(json!({"path": path, "track": t, "lines": lines, "bytes": body.len()}))
}

fn list(e: &mut Engine, p: &Value) -> Result<Value> {
    let t = which(e, "lyrics.list", p)?;
    let s = e.session();
    let rate = s.sample_rate.as_f64().max(1.0);
    let lines: Vec<Value> = s
        .track(t)
        .map(|t| {
            soundcraft_model::track_lines(t)
                .iter()
                .map(|l| {
                    let words: Vec<Value> =
                        l.words.iter().map(|w| json!({"start": w.start, "end": w.end, "seconds": w.start as f64 / rate, "text": w.text})).collect();
                    json!({"start": l.start, "end": l.end, "seconds": l.start as f64 / rate, "text": l.text(), "words": words})
                })
                .collect()
        })
        .unwrap_or_default();
    Ok(json!({"track": t, "lines": lines}))
}

/// Most matches one search reports.
const MAX_MATCHES: usize = 1000;

/// A word as searched: lower case, letters and digits only (apostrophes inside a word kept out:
/// "don't" is "dont").
fn fold(word: &str) -> String {
    word.chars().filter(|c| c.is_alphanumeric()).flat_map(char::to_lowercase).collect()
}

/// One place `text` was found: the track, the timeline range from its first word's start to its
/// last word's end, and the line it starts in.
#[derive(Debug, Clone, PartialEq)]
pub struct TranscriptMatch {
    pub track: TrackId,
    pub start: Samples,
    pub end: Samples,
    pub line: String,
}

/// Every match of `text`'s words, consecutive, in the timed lyrics of `tracks` (all tracks when
/// empty), across line breaks, in time order.
pub fn search_transcript(s: &soundcraft_model::Session, text: &str, tracks: &[TrackId]) -> Vec<TranscriptMatch> {
    let want: Vec<String> = text.split_whitespace().map(fold).filter(|w| !w.is_empty()).collect();
    if want.is_empty() {
        return Vec::new();
    }
    let mut out = Vec::new();
    for t in s.tracks.iter().filter(|t| tracks.is_empty() || tracks.contains(&t.id)) {
        let lines = soundcraft_model::track_lines(t);
        // The track's words in time order, each with the line it is in.
        let words: Vec<(String, Samples, Samples, usize)> = lines
            .iter()
            .enumerate()
            .flat_map(|(i, l)| l.words.iter().map(move |w| (fold(&w.text), w.start, w.end, i)))
            .filter(|w| !w.0.is_empty())
            .collect();
        for k in 0..words.len() {
            let hit = want.iter().enumerate().all(|(j, w)| words.get(k + j).is_some_and(|x| x.0 == *w));
            if !hit {
                continue;
            }
            let (Some(first), Some(last)) = (words.get(k), words.get(k + want.len() - 1)) else { continue };
            let line = lines.get(first.3).map(soundcraft_model::LyricLine::text).unwrap_or_default();
            out.push(TranscriptMatch { track: t.id, start: first.1, end: last.2.max(first.1), line });
        }
    }
    out.sort_by_key(|m| (m.start, m.track.0));
    out.truncate(MAX_MATCHES);
    out
}

fn search(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.search";
    let text = str_param(p, "text").ok_or_else(|| bad(C, "`text` required"))?;
    let tracks = if p.get("tracks").is_some() || p.get("track").is_some() { tracks_param(e, C, p)? } else { Vec::new() };
    let s = e.session();
    let rate = s.sample_rate.as_f64().max(1.0);
    let matches: Vec<Value> = search_transcript(s, text, &tracks)
        .iter()
        .map(|m| {
            let name = s.track(m.track).map(|t| t.name.clone()).unwrap_or_default();
            json!({"track": m.track, "name": name, "start": m.start, "end": m.end, "seconds": m.start as f64 / rate, "line": m.line})
        })
        .collect();
    Ok(json!({"matches": matches}))
}

fn find(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.find";
    let text = str_param(p, "text").filter(|t| !fold(t).is_empty()).ok_or_else(|| bad(C, "`text` required: the words to find"))?;
    let all = search_transcript(e.session(), text, &[]);
    if all.is_empty() {
        return Err(bad(C, format!("“{}” is not in any track's transcript", text.trim())));
    }
    let at = e.session().edit.selection.start;
    let index = if p.get("backward").and_then(Value::as_bool).unwrap_or(false) {
        all.iter().rposition(|m| m.start < at).unwrap_or(all.len() - 1)
    } else {
        all.iter().position(|m| m.start > at).unwrap_or(0)
    };
    let Some(m) = all.get(index).cloned() else { return Err(bad(C, "no match")) };
    let s = e.session_mut();
    s.edit.selection = soundcraft_time::Range::new(m.start, m.end);
    s.edit.selected_tracks = vec![m.track];
    s.edit.playhead = m.start;
    e.transport_requests.push(crate::TransportRequest::Locate(m.start));
    Ok(json!({"track": m.track, "start": m.start, "end": m.end, "line": m.line, "index": index + 1, "of": all.len()}))
}

fn clear(e: &mut Engine, p: &Value) -> Result<Value> {
    const C: &str = "lyrics.clear";
    let t = which(e, C, p)?;
    let (a, b) = (super::position_param(e, C, p, "start")?, super::position_param(e, C, p, "end")?);
    // Without a range, every line goes (hidden ones of a trimmed clip too).
    let all = a.is_none() && b.is_none();
    let (a, b) = (a.unwrap_or(Samples::MIN), b.unwrap_or(Samples::MAX));
    let Some(pl) = e.session_mut().track_mut(t).and_then(|t| t.playlist_mut()) else { return Err(bad(C, "no such track")) };
    let mut removed = 0;
    for c in &mut pl.clips {
        // Lines whose timeline start falls in a..b (hidden lines of a trimmed clip stay).
        let (lo, hi) = (c.start, c.end());
        let before = c.lyrics.len();
        let clip = c.clone();
        c.lyrics.retain(|l| {
            let at = clip.start.saturating_add(l.start.saturating_sub(soundcraft_model::to_clip_time(&clip, clip.start)));
            !(all || (at >= lo && at < hi && at >= a && at < b))
        });
        removed += before - c.lyrics.len();
    }
    Ok(json!({"track": t, "removed": removed}))
}

#[cfg(test)]
mod search_tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, Clip, LyricLine, LyricWord, TrackKind};

    fn line(words: &[(&str, Samples)]) -> LyricLine {
        let words: Vec<LyricWord> = words.iter().map(|(t, at)| LyricWord { start: *at, end: at + 1000, text: t.to_string() }).collect();
        LyricLine { start: words.first().map_or(0, |w| w.start), end: words.last().map_or(0, |w| w.end), words }
    }

    /// Two tracks with a transcript each: one MIDI clip each, starting at 0 (clip time is
    /// timeline time), the second track's clip moved later.
    fn engine() -> Engine {
        let mut e = Engine::default();
        let s = e.session_mut();
        for (name, start, lines) in [
            (
                "Host",
                0,
                vec![line(&[("Welcome", 0), ("back,", 10_000), ("everyone.", 20_000)]), line(&[("Back", 50_000), ("to", 60_000), ("work!", 70_000)])],
            ),
            ("Guest", 100_000, vec![line(&[("Glad", 0), ("to", 10_000), ("be", 20_000), ("back", 30_000)]), line(&[("everyone", 40_000)])]),
        ] {
            let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some(name));
            let id = s.new_clip_id();
            let mut c = Clip::midi(id, name, start, 200_000, soundcraft_midi::Sequence::default());
            c.lyrics = lines;
            s.track_mut(t).unwrap().playlist_mut().unwrap().clips.push(c);
        }
        e
    }

    #[test]
    fn search_finds_phrases_across_tracks_lines_case_and_punctuation() {
        let mut e = engine();
        let r = e.execute("lyrics.search", &json!({"text": "BACK"})).unwrap();
        let starts: Vec<i64> = r["matches"].as_array().unwrap().iter().map(|m| m["start"].as_i64().unwrap()).collect();
        assert_eq!(starts, vec![10_000, 50_000, 130_000], "in time order, both tracks");
        // A phrase across a line break, on the clip moved later: timeline times.
        let r = e.execute("lyrics.search", &json!({"text": "back everyone"})).unwrap();
        let m = &r["matches"].as_array().unwrap();
        assert_eq!(m.len(), 2, "{r}");
        assert_eq!((m[0]["start"].as_i64(), m[0]["end"].as_i64()), (Some(10_000), Some(21_000)));
        assert_eq!((m[1]["name"].as_str(), m[1]["start"].as_i64(), m[1]["end"].as_i64()), (Some("Guest"), Some(130_000), Some(141_000)));
        assert_eq!(e.execute("lyrics.search", &json!({"text": "nowhere"})).unwrap()["matches"], json!([]));
        assert_eq!(e.execute("lyrics.search", &json!({"text": " ,. "})).unwrap()["matches"], json!([]));
    }

    #[test]
    fn find_selects_the_next_match_and_wraps() {
        let mut e = engine();
        let r = e.execute("lyrics.find", &json!({"text": "back"})).unwrap();
        assert_eq!((r["start"].as_i64(), r["index"].as_i64(), r["of"].as_i64()), (Some(10_000), Some(1), Some(3)));
        assert_eq!(e.session().edit.selection, soundcraft_time::Range::new(10_000, 11_000));
        e.execute("lyrics.find", &json!({"text": "back"})).unwrap();
        let r = e.execute("lyrics.find", &json!({"text": "back"})).unwrap();
        assert_eq!(r["start"].as_i64(), Some(130_000));
        let guest = e.session().tracks[1].id;
        assert_eq!(e.session().edit.selected_tracks, vec![guest]);
        assert_eq!(e.execute("lyrics.find", &json!({"text": "back"})).unwrap()["start"].as_i64(), Some(10_000), "wraps");
        assert_eq!(e.execute("lyrics.find", &json!({"text": "back", "backward": true})).unwrap()["start"].as_i64(), Some(130_000));
        assert!(e.execute("lyrics.find", &json!({"text": "nowhere"})).is_err());
    }
}

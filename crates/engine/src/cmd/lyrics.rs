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

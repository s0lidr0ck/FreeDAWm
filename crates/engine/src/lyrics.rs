//! Timed lyrics in the session: placing transcribed words on the clips they were sung on, and the
//! `.lrc`, plain-text and MIDI lyric exports.
//!
//! Lyrics live on clips (`Clip::lyrics`), in the clip's source time, so they follow every clip
//! edit; a track's lyrics are its playing clips' visible words (`soundcraft_model::track_lines`).
//! Transcribe Lyrics replaces the lines a clip held over the range it heard and keeps the rest.

use soundcraft_model::{LyricLine, LyricWord, Session, TrackId, sanitize_lyrics, to_clip_time, track_has_lyrics, track_lines};
use soundcraft_time::Samples;

/// A word from a transcriber: seconds from the start of its source.
#[derive(Debug, Clone, PartialEq)]
pub struct TimedWord {
    pub start: f64,
    pub end: f64,
    pub text: String,
}

/// Puts `lines` (each a list of words, seconds from `start`) on the audio or MIDI clips of `track`
/// under them, over `start..start+len`: each word goes to the clip playing at its time (words
/// over no clip are dropped), in that clip's source time; a clip's existing lines that start in
/// the range go. Returns the number of lines and words placed.
pub fn place_lyrics(s: &mut Session, track: TrackId, start: Samples, len: Samples, lines: &[Vec<TimedWord>]) -> (usize, usize) {
    let rate = s.sample_rate.as_f64();
    let end = start.saturating_add(len.max(0));
    let at = |secs: f64| -> Samples {
        if secs.is_finite() { start.saturating_add((secs * rate).round().clamp(0.0, len.max(0) as f64) as Samples) } else { start }
    };
    let Some(pl) = s.track_mut(track).and_then(|t| t.playlist_mut()) else { return (0, 0) };
    // Clear the range on every clip it touches (in each clip's own time).
    for c in pl.clips.iter_mut().filter(|c| !c.is_video() && c.start < end && c.end() > start) {
        let (a, b) = (to_clip_time(c, start.max(c.start)), to_clip_time(c, end.min(c.end())));
        c.lyrics.retain(|l| l.start < a || l.start >= b);
    }
    let (mut n_lines, mut n_words) = (0, 0);
    for l in lines {
        // A line may cross clips: one piece per clip.
        let mut pieces: Vec<(usize, Vec<LyricWord>)> = Vec::new();
        for w in l.iter().filter(|w| w.start.is_finite() && w.start >= 0.0 && at(w.start) < end) {
            let (ws, we) = (at(w.start), at(w.end).max(at(w.start)));
            let Some(ci) = pl.clips.iter().position(|c| !c.is_video() && c.range().contains(ws)) else { continue };
            let c = &pl.clips[ci];
            let word = LyricWord { start: to_clip_time(c, ws), end: to_clip_time(c, we.min(c.end())), text: w.text.clone() };
            match pieces.last_mut() {
                Some((i, v)) if *i == ci => v.push(word),
                _ => pieces.push((ci, vec![word])),
            }
        }
        for (ci, words) in pieces {
            let Some(c) = pl.clips.get_mut(ci) else { continue };
            let (a, b) = (words.first().map_or(0, |w| w.start), words.iter().map(|w| w.end).max().unwrap_or(0));
            n_words += words.len();
            n_lines += 1;
            c.lyrics.push(LyricLine { start: a, end: b, words });
            sanitize_lyrics(&mut c.lyrics);
        }
    }
    (n_lines, n_words)
}

/// `[mm:ss.xx]`.
fn lrc_time(secs: f64) -> String {
    let cs = (secs.max(0.0) * 100.0).round() as u64;
    format!("[{:02}:{:02}.{:02}]", cs / 6000, (cs / 100) % 60, cs % 100)
}

/// `<mm:ss.xx>` (enhanced LRC word time).
fn word_time(secs: f64) -> String {
    let t = lrc_time(secs);
    format!("<{}>", t.trim_start_matches('[').trim_end_matches(']'))
}

/// The track's lyrics as LRC: a title tag, then one `[mm:ss.xx]` line per line, each word with
/// its own `<mm:ss.xx>` time (enhanced LRC; players without word times show the line).
pub fn lrc(s: &Session, track: TrackId) -> String {
    let rate = s.sample_rate.as_f64().max(1.0);
    let secs = |at: Samples| at as f64 / rate;
    let Some(t) = s.track(track) else { return String::new() };
    let mut out = format!("[ti:{}]\n[re:FreeDAWm]\n", t.name.replace(['[', ']', '\n', '\r'], " "));
    for l in &track_lines(t) {
        out.push_str(&lrc_time(secs(l.start)));
        let words: Vec<String> = l.words.iter().map(|w| format!("{}{}", word_time(secs(w.start)), w.text)).collect();
        out.push_str(&words.join(" "));
        out.push('\n');
    }
    out
}

/// The track's lyrics as plain text, one line per line.
pub fn plain_text(s: &Session, track: TrackId) -> String {
    s.track(track).map(|t| track_lines(t).iter().map(|l| l.text() + "\n").collect()).unwrap_or_default()
}

/// The track a lyrics command acts on: `track`, else the first selected track with lyrics, else
/// the first track with lyrics.
pub fn lyrics_track(s: &Session, asked: Option<TrackId>) -> Option<TrackId> {
    asked
        .or_else(|| s.tracks.iter().find(|t| s.edit.selected_tracks.contains(&t.id) && track_has_lyrics(t)).map(|t| t.id))
        .or_else(|| s.tracks.iter().find(|t| track_has_lyrics(t)).map(|t| t.id))
}

/// Lyric meta events of `track` in ticks on the session's tempo map: one per word.
pub fn midi_lyrics(s: &Session, track: TrackId) -> Vec<(i64, String)> {
    s.track(track)
        .map(|t| {
            track_lines(t).iter().flat_map(|l| l.words.iter()).map(|w| (s.tempo.samples_to_ticks(w.start, s.sample_rate), w.text.clone())).collect()
        })
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_model::{ChannelFormat, TrackKind};

    fn w(start: f64, end: f64, text: &str) -> TimedWord {
        TimedWord { start, end, text: text.into() }
    }

    #[test]
    fn places_replaces_and_exports() {
        let mut s = Session { sample_rate: soundcraft_time::SampleRate::HZ_48000, ..Session::default() };
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Vox"));
        // A clip from 1 s to 31 s holds them, in its own time.
        let id = s.new_clip_id();
        if let Some(pl) = s.track_mut(t).and_then(|t| t.playlist_mut()) {
            pl.clips.push(soundcraft_model::Clip::midi(id, "Vox", 48_000, 48_000 * 30, Default::default()));
        }
        let at = 48_000 * 2;
        let (l, n) = place_lyrics(&mut s, t, at, 48_000 * 10, &[vec![w(0.5, 1.0, "Show"), w(1.0, 1.4, "me")], vec![w(61.0, 62.0, "late")]]);
        assert_eq!((l, n), (1, 2));
        let clip = &s.track(t).unwrap().clips()[0];
        assert_eq!(clip.lyrics[0].words[0].start, at + 24_000 - 48_000, "clip time");
        assert_eq!(lrc(&s, t), "[ti:Vox]\n[re:FreeDAWm]\n[00:02.50]<00:02.50>Show <00:03.00>me\n");
        assert_eq!(plain_text(&s, t), "Show me\n");
        // A second run over the same range replaces the line; one elsewhere stays.
        place_lyrics(&mut s, t, 48_000 * 20, 48_000, &[vec![w(0.0, 0.5, "Lord")]]);
        place_lyrics(&mut s, t, at, 48_000 * 10, &[vec![w(0.5, 1.0, "Seek")]]);
        assert_eq!(plain_text(&s, t), "Seek\nLord\n");
        assert_eq!(lrc_time(61.234), "[01:01.23]");
        assert_eq!(lyrics_track(&s, None), Some(t));
    }
}

//! Timed lyrics: lines of words held by the clip whose material they were sung on (Track ›
//! Transcribe Lyrics, MIDI import), with times in the clip's source, so editing the clip edits its
//! lyrics: move, nudge, trim (words outside the window hide, not lost), split, delete, duplicate.
//! File › Export › Lyrics writes `.lrc` or text; MIDI export writes lyric meta events.

use crate::{Clip, ClipContent, Track};
use serde::{Deserialize, Deserializer, Serialize};
use soundcraft_time::Samples;

/// Most lines one track keeps.
pub const MAX_LYRIC_LINES: usize = 4096;
/// Most words in one line.
pub const MAX_LINE_WORDS: usize = 256;
/// Longest word, characters.
pub const MAX_WORD_CHARS: usize = 64;

/// A sung word.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LyricWord {
    pub start: Samples,
    pub end: Samples,
    pub text: String,
}

/// A line of lyrics: its words, in order.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct LyricLine {
    pub start: Samples,
    pub end: Samples,
    pub words: Vec<LyricWord>,
}

impl LyricLine {
    /// The words joined by spaces.
    pub fn text(&self) -> String {
        self.words.iter().map(|w| w.text.as_str()).collect::<Vec<_>>().join(" ")
    }
}

fn cut_chars(s: &mut String, max: usize) {
    if let Some((i, _)) = s.char_indices().nth(max) {
        s.truncate(i);
    }
}

/// Lyrics a session file can hold: control characters become spaces, words trimmed and capped,
/// empty words and lines dropped, times non-negative and ordered, lines sorted and capped.
pub fn sanitize_lyrics(lines: &mut Vec<LyricLine>) {
    lines.retain_mut(|l| {
        l.words.retain_mut(|w| {
            w.text = w.text.chars().map(|c| if c.is_control() { ' ' } else { c }).collect::<String>().trim().to_string();
            cut_chars(&mut w.text, MAX_WORD_CHARS);
            w.start = w.start.max(0);
            w.end = w.end.max(w.start);
            !w.text.is_empty()
        });
        l.words.truncate(MAX_LINE_WORDS);
        l.words.sort_by_key(|w| w.start);
        if let (Some(a), Some(b)) = (l.words.first().map(|w| w.start), l.words.iter().map(|w| w.end).max()) {
            l.start = a;
            l.end = b.max(a);
        }
        !l.words.is_empty()
    });
    lines.sort_by_key(|l| l.start);
    lines.truncate(MAX_LYRIC_LINES);
}

/// The clip's source position of the timeline position `at` (audio: into the file; MIDI: from the
/// clip start).
pub fn to_clip_time(clip: &Clip, at: Samples) -> Samples {
    at.saturating_sub(clip.start).saturating_add(source_base(clip))
}

fn source_base(clip: &Clip) -> Samples {
    match clip.content {
        ClipContent::Audio { offset, .. } | ClipContent::Video { offset, .. } => offset,
        ClipContent::Midi { .. } => 0,
    }
}

/// The clip's visible lyrics on the timeline: the words inside its window, at their timeline
/// positions (ends cut at the clip's end), lines without a visible word left out.
pub fn clip_lines(clip: &Clip) -> Vec<LyricLine> {
    let base = source_base(clip);
    let (lo, hi) = (base, base.saturating_add(clip.length));
    let at = |src: Samples| clip.start.saturating_add(src.saturating_sub(base));
    clip.lyrics
        .iter()
        .filter_map(|l| {
            let words: Vec<LyricWord> = l
                .words
                .iter()
                .filter(|w| w.start >= lo && w.start < hi)
                .map(|w| LyricWord { start: at(w.start), end: at(w.end.min(hi)).max(at(w.start)), text: w.text.clone() })
                .collect();
            let (a, b) = (words.first()?.start, words.iter().map(|w| w.end).max()?);
            Some(LyricLine { start: a, end: b, words })
        })
        .collect()
}

/// A track's visible lyrics on the timeline (its playing clips), in time order.
pub fn track_lines(track: &Track) -> Vec<LyricLine> {
    let mut v: Vec<LyricLine> = track.clips().iter().filter(|c| !c.muted).flat_map(clip_lines).collect();
    v.sort_by_key(|l| l.start);
    v
}

/// Whether any clip of the track's playing playlist holds lyrics.
pub fn track_has_lyrics(track: &Track) -> bool {
    track.clips().iter().any(|c| !c.lyrics.is_empty())
}

/// After a clip's audio is replaced by a render of its window (a new source from offset 0), its
/// lyrics move into the new audio's time: `old_offset` earlier. Words before the window go.
pub fn rebase_lyrics(clip: &mut Clip, old_offset: Samples) {
    if old_offset == 0 || clip.lyrics.is_empty() {
        return;
    }
    for l in &mut clip.lyrics {
        l.words.retain(|w| w.start >= old_offset);
        for w in &mut l.words {
            (w.start, w.end) = (w.start - old_offset, w.end.saturating_sub(old_offset).max(w.start - old_offset));
        }
    }
    sanitize_lyrics(&mut clip.lyrics);
}

/// Moves lyrics stored the first way (absolute samples on the track, `Track::lyrics`) onto the
/// clips under each line's start, in their source time; lines under no clip are dropped.
pub fn migrate_track_lyrics(track: &mut Track) {
    if track.lyrics.is_empty() {
        return;
    }
    let lines = std::mem::take(&mut track.lyrics);
    let Some(pl) = track.playlist_mut() else { return };
    for line in lines {
        let Some(c) = pl.clips.iter_mut().find(|c| c.range().contains(line.start)) else { continue };
        let to = |at: Samples| to_clip_time(c, at);
        let words: Vec<LyricWord> = line.words.iter().map(|w| LyricWord { start: to(w.start), end: to(w.end), text: w.text.clone() }).collect();
        let l = LyricLine { start: to(line.start), end: to(line.end), words };
        c.lyrics.push(l);
        sanitize_lyrics(&mut c.lyrics);
    }
}

/// Reads lyrics leniently: anything unreadable loads as no lyrics, never a failed session.
pub(crate) fn de_lyrics<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<LyricLine>, D::Error> {
    let v = serde_json::Value::deserialize(d)?;
    let Ok(mut lines) = serde_json::from_value::<Vec<LyricLine>>(v) else { return Ok(Vec::new()) };
    sanitize_lyrics(&mut lines);
    Ok(lines)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hostile_lyrics_are_tamed() {
        let w = |s: i64, e: i64, t: &str| LyricWord { start: s, end: e, text: t.into() };
        let mut l = vec![
            LyricLine { start: 0, end: 0, words: vec![w(50, 10, " show\u{0}me "), w(-5, 3, "x".repeat(100).as_str()), w(7, 9, "  ")] },
            LyricLine { start: 0, end: 0, words: vec![w(1, 2, "\n")] },
        ];
        sanitize_lyrics(&mut l);
        assert_eq!(l.len(), 1);
        assert_eq!(l[0].words.len(), 2);
        assert_eq!((l[0].start, l[0].end), (0, 50));
        assert_eq!(l[0].words[0].text.chars().count(), MAX_WORD_CHARS);
        assert_eq!(l[0].words[1].text, "show me");
        let bad: Vec<LyricLine> = de_lyrics(serde_json::json!([{"start": "soon"}])).unwrap();
        assert!(bad.is_empty());
    }
}

//! Edit › Split into Files (`edit.split_into_files`): a long recording (a whole church service,
//! an hour of 16+ tracks) cut at the markers into one audio file per piece, staying in the one
//! session.
//!
//! At every ruler marker inside the range (the edit selection, else the whole session) and at the
//! selection's edges, every clip of the chosen tracks (the selected tracks, else every audio, MIDI
//! and instrument track) is cut, as Edit › Separate cuts. Each audio piece then gets **its own
//! source**: the samples of its window of the long file, copied as they are, named
//! `<track> - <marker name or Part N>` and written to `Audio Files/` when the session is saved
//! (as Consolidate's and Commit's are). The clip stays where it was with everything else it had
//! (gain, fades, colour…), now reading its own file from its start: playback is identical. MIDI
//! clips are cut at the same points (no files). One undo step; a long file no clip uses any
//! more leaves the session (not the disk), and Undo brings it back.
//!
//! The vocal tools then work on one song: vocal tuning analyses a clip's whole source, which is
//! now the piece (an hour-long file is over its 30-minute limit); Clean Vocal, stems, tempo
//! detection and Scout already read only the clip's range.
//!
//! Copying an hour of 16 tracks takes a while, so a host runs a [`SplitJob`] first, a piece at a
//! time behind a progress window, and hands the copies to the engine
//! ([`Engine::offer_split_pieces`]); the command takes them instead of copying (and copies
//! whatever it doesn't find, as a headless call does).
//!
//! Clips the copy can't reproduce exactly keep their long file (they are still cut): a stretched
//! or warped clip, one with a Clean Vocal blend or a vocal tuning. The result lists them.

use crate::cmd::{bad, position_param, tracks_param};
use crate::{Engine, Result};
use serde_json::{Value, json};
use soundcraft_audio_io::{AudioBuffer, FileFormat};
use soundcraft_model::{Clip, ClipContent, ClipId, MarkerKind, Session, SourceAudio, SourceId, TrackId, TrackKind};
use soundcraft_time::{Range, Samples};
use std::collections::VecDeque;
use std::sync::Arc;

pub const ID: &str = "edit.split_into_files";

/// Where the command cuts and what it names the pieces.
#[derive(Debug, Clone, PartialEq)]
pub struct Plan {
    pub tracks: Vec<TrackId>,
    /// The range split: the selection, else the whole session.
    pub range: Range,
    /// Cut points, ascending: the markers strictly inside `range`, plus its edges when it is a
    /// selection.
    pub cuts: Vec<Samples>,
    /// The segments between `range.start`, the cuts and `range.end`, each with its name.
    pub segments: Vec<(Range, String)>,
    /// The range is a selection (or `start`/`end`), not the whole session.
    pub selection: bool,
}

/// One clip's window of a long file, copied into a source of its own.
#[derive(Debug, Clone)]
pub struct Piece {
    pub source: SourceId,
    /// First sample of the window in the long file.
    pub offset: Samples,
    pub frames: usize,
    pub audio: Arc<SourceAudio>,
}

/// What a clip's piece is copied from (the key a [`Piece`] is found by).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Window {
    source: SourceId,
    offset: Samples,
    frames: usize,
}

/// One audio clip, after the cuts, to get its own file.
#[derive(Debug, Clone)]
struct Todo {
    clip: ClipId,
    window: Window,
    name: String,
}

/// The tracks split when none are named or selected: every audio, MIDI and instrument track.
fn default_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks.iter().filter(|t| t.kind.has_playlist() && t.kind != TrackKind::Video).map(|t| t.id).collect()
}

/// A piece name usable in a file name (marker names may hold `/`, `:` …): see `io::sanitize_name`.
fn part_name(marker: &str) -> String {
    marker
        .chars()
        .map(|c| if matches!(c, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|') { '-' } else { c })
        .collect::<String>()
        .trim()
        .to_string()
}

/// The command's plan for params `p`: `tracks`/`track` (else the selected tracks, else every
/// audio, MIDI and instrument track) and `start`/`end` (else the edit selection, else the whole
/// session).
pub fn plan(e: &Engine, p: &Value) -> Result<Plan> {
    let s = e.session();
    let mut tracks = tracks_param(e, ID, p)?;
    if tracks.is_empty() {
        tracks = default_tracks(s);
    }
    tracks.retain(|t| s.track(*t).is_some_and(|tr| tr.kind.has_playlist() && tr.kind != TrackKind::Video));
    if tracks.is_empty() {
        return Err(bad(ID, "no audio or MIDI tracks to split"));
    }
    let start = position_param(e, ID, p, "start")?;
    let end = position_param(e, ID, p, "end")?;
    let (range, edges) = match (start, end) {
        (Some(a), Some(b)) if b > a => (Range::new(a, b), true),
        (Some(_), Some(_)) => return Err(bad(ID, "`end` must be after `start`")),
        (Some(_), None) | (None, Some(_)) => return Err(bad(ID, "give both `start` and `end`, or neither")),
        (None, None) if !s.edit.selection.is_empty() => (s.edit.selection, true),
        (None, None) => (Range::new(0, s.content_end().max(1)), false),
    };
    let mut cuts: Vec<Samples> = s
        .markers
        .iter()
        .filter(|m| m.kind == MarkerKind::Marker && m.track.is_none() && m.start > range.start && m.start < range.end)
        .map(|m| m.start)
        .collect();
    if edges {
        cuts.push(range.start);
        cuts.push(range.end);
    }
    cuts.sort_unstable();
    cuts.dedup();
    let mut bounds = vec![range.start];
    bounds.extend(cuts.iter().copied().filter(|c| *c > range.start && *c < range.end));
    bounds.push(range.end);
    let segments = bounds
        .windows(2)
        .enumerate()
        .filter_map(|(i, w)| {
            let (a, b) = (*w.first()?, *w.get(1)?);
            let marker = s
                .markers
                .iter()
                .filter(|m| m.kind == MarkerKind::Marker && m.track.is_none() && m.start == a)
                .map(|m| part_name(&m.name))
                .find(|n| !n.is_empty());
            Some((Range::new(a, b), marker.unwrap_or_else(|| format!("Part {}", i + 1))))
        })
        .collect();
    Ok(Plan { tracks, range, cuts, segments, selection: edges })
}

/// Why `c` keeps its long file (it is still cut): its copy would not play the same.
fn kept_because(c: &Clip) -> Option<&'static str> {
    if crate::vocal::stretched(c) || !c.warp.is_empty() {
        Some("stretched or warped")
    } else if c.clean.is_some() {
        Some("Clean Vocal is applied")
    } else if c.tune.is_some() {
        Some("it has a vocal tuning")
    } else {
        None
    }
}

/// Cuts `s` as the plan says and lists the audio clips inside the range that get a file of
/// their own, and those that keep their long file (clip, why).
fn cut(s: &mut Session, plan: &Plan) -> (Vec<Todo>, Vec<(ClipId, &'static str)>) {
    for t in &plan.tracks {
        for at in &plan.cuts {
            crate::edit::separate_at(s, *t, *at);
        }
    }
    let (mut todo, mut kept) = (Vec::new(), Vec::new());
    for t in &plan.tracks {
        let Some(tr) = s.track(*t) else { continue };
        if tr.kind.is_midi() {
            continue;
        }
        for c in tr.clips() {
            if c.start < plan.range.start || c.end() > plan.range.end {
                continue;
            }
            let ClipContent::Audio { source, offset } = c.content else { continue };
            if let Some(why) = kept_because(c) {
                kept.push((c.id, why));
                continue;
            }
            let total = s.pool.get(source).map_or(0, |a| a.frames());
            let Ok(first) = usize::try_from(offset) else { continue };
            // A clip running past the end of its file plays silence there: the copy stops at the
            // end of the file, and the clip keeps its length.
            let frames = usize::try_from(c.length).unwrap_or(0).min(total.saturating_sub(first));
            if frames == 0 || (first == 0 && frames == total) {
                // Nothing to copy, or the clip already plays a whole file of its own.
                continue;
            }
            let mid = c.start.saturating_add(c.length / 2);
            let seg = plan.segments.iter().find(|(r, _)| r.start <= mid && mid < r.end).or(plan.segments.first());
            let name = format!("{} - {}", tr.name, seg.map_or("Part 1", |(_, n)| n.as_str()));
            todo.push(Todo { clip: c.id, window: Window { source, offset, frames }, name });
        }
    }
    (todo, kept)
}

/// The window `w` of its source, copied (with its waveform overview). `None` when the source's
/// audio is missing.
fn copy(s: &Session, w: Window) -> Option<Arc<SourceAudio>> {
    let src = s.pool.get(w.source)?;
    let first = usize::try_from(w.offset).ok()?;
    let channels = (0..src.num_channels())
        .map(|ch| {
            let mut v = vec![0.0; w.frames];
            src.read(ch, first, &mut v);
            v
        })
        .collect();
    Some(Arc::new(SourceAudio::new(AudioBuffer { sample_rate: s.sample_rate.hz(), channels })))
}

/// The command: cut, then give each audio piece its own source. One undo step (`execute`).
pub fn run(e: &mut Engine, p: &Value) -> Result<Value> {
    let plan = plan(e, p)?;
    let mut pieces = e.take_split_pieces();
    let s = e.session_mut();
    let (todo, kept) = cut(s, &plan);
    let mut made = Vec::new();
    let mut longs: Vec<SourceId> = Vec::new();
    for t in todo {
        let w = t.window;
        let audio = match pieces.iter().position(|x| x.source == w.source && x.offset == w.offset && x.frames == w.frames) {
            Some(i) => pieces.swap_remove(i).audio,
            None => match copy(s, w) {
                Some(a) => a,
                None => continue,
            },
        };
        // Two clips of one track in one song: "Bass - Verse", "Bass - Verse 2".
        let name = (1..)
            .map(|n: u32| if n == 1 { t.name.clone() } else { format!("{} {n}", t.name) })
            .find(|n| !s.sources.iter().any(|x| x.name == *n))
            .unwrap_or_else(|| t.name.clone());
        let src = crate::io::add_source_audio(s, &format!("{name}.wav"), audio, FileFormat::Wav);
        let Some(c) = s.find_clip_mut(t.clip) else { continue };
        c.content = ClipContent::Audio { source: src, offset: 0 };
        // Lyrics are in the source's time: they move with the new file's start.
        soundcraft_model::rebase_lyrics(c, w.offset);
        c.name = name;
        if !longs.contains(&w.source) {
            longs.push(w.source);
        }
        made.push(json!({"clip": t.clip.0, "source": src.0, "name": c.name}));
    }
    for src in longs {
        crate::cmd::freeze::drop_unused_source(s, src);
    }
    let kept: Vec<Value> = kept.into_iter().map(|(c, why)| json!({"clip": c.0, "why": why})).collect();
    Ok(json!({
        "files": made.len(),
        "pieces": made,
        "kept": kept,
        "cuts": plan.cuts,
        "segments": plan.segments.iter().map(|(r, n)| json!({"start": r.start, "end": r.end, "name": n})).collect::<Vec<_>>(),
    }))
}

/// Edit › Split into Files copied ahead of the command, a piece at a time, so a host can keep
/// drawing a progress window.
pub struct SplitJob {
    params: Value,
    session: Arc<Session>,
    todo: VecDeque<Window>,
    done: Vec<Piece>,
    total: usize,
}

impl SplitJob {
    /// The job for `edit.split_into_files` with `params`, planned as the command would. `None`
    /// when there is nothing to copy (only cuts, MIDI): run the command as it is.
    pub fn new(e: &Engine, params: &Value) -> Result<Option<SplitJob>> {
        let plan = plan(e, params)?;
        let mut trial = e.session().clone();
        let (todo, _) = cut(&mut trial, &plan);
        if todo.is_empty() {
            return Ok(None);
        }
        let total = todo.len();
        // The command runs later: pinned to the tracks and range planned now, whatever is
        // selected by then.
        let mut params = json!({"tracks": plan.tracks.iter().map(|t| t.0).collect::<Vec<_>>()});
        if plan.selection {
            params["start"] = json!(plan.range.start);
            params["end"] = json!(plan.range.end);
        }
        Ok(Some(SplitJob { params, session: e.session_arc(), todo: todo.into_iter().map(|t| t.window).collect(), done: Vec::new(), total }))
    }

    /// The command and its params: run it after [`SplitJob::finish`].
    pub fn command(&self) -> (&'static str, &Value) {
        (ID, &self.params)
    }

    /// How many pieces are copied, of how many.
    pub fn counts(&self) -> (usize, usize) {
        (self.done.len(), self.total)
    }

    /// How far along, from 0 to 1.
    pub fn progress(&self) -> f32 {
        (self.done.len() as f32 / self.total.max(1) as f32).clamp(0.0, 1.0)
    }

    /// Copies pieces until `more` returns false (at least one per call). True once all are copied.
    pub fn step(&mut self, mut more: impl FnMut() -> bool) -> bool {
        while let Some(w) = self.todo.pop_front() {
            if let Some(audio) = copy(&self.session, w) {
                self.done.push(Piece { source: w.source, offset: w.offset, frames: w.frames, audio });
            }
            if self.todo.is_empty() {
                break;
            }
            if !more() {
                return false;
            }
        }
        true
    }

    /// Hands the copies to `e` for the command ([`Engine::offer_split_pieces`]). Run it right after.
    pub fn finish(self, e: &mut Engine) {
        e.offer_split_pieces(self.done);
    }

    /// Copies everything and runs the command on `e`, as a headless host would.
    pub fn run(mut self, e: &mut Engine) -> Result<Value> {
        while !self.step(|| true) {}
        let params = self.params.clone();
        self.finish(e);
        e.execute(ID, &params)
    }
}

#[cfg(test)]
#[path = "tests_split_files.rs"]
mod tests;

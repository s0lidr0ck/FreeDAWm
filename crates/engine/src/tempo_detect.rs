//! Detect Tempo: what audio is analysed, the beat tracker's result and how it is written into the
//! tempo map (Event › Detect Tempo…, `event.tempo_detect`).
//!
//! The analysis ([`source_audio`] + [`detect`]) only reads the session, so the UI runs it off its
//! thread and hands the beats back to the command to apply.

use soundcraft_dsp::beat::{BeatOptions, downmix, track_beats};
use soundcraft_model::{ClipId, Session, TrackId};
use soundcraft_time::{MAX_POSITION, Range, Samples, TempoMap};

/// Longest stretch of audio analysed at once (seconds).
pub const MAX_DETECT_SECONDS: f64 = 20.0 * 60.0;
/// Most tempo points one detection writes.
pub const MAX_POINTS: usize = 10_000;

/// How the detected beats become the tempo map.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum TempoMode {
    /// One tempo: the average.
    Constant,
    /// A tempo point per bar, so every detected downbeat lands on its bar line.
    #[default]
    Follow,
    /// Per bar, smoothed over five bars.
    Smooth,
}

impl TempoMode {
    pub fn parse(s: &str) -> Option<TempoMode> {
        match s.trim().to_ascii_lowercase().as_str() {
            "constant" => Some(TempoMode::Constant),
            "follow" => Some(TempoMode::Follow),
            "smooth" => Some(TempoMode::Smooth),
            _ => None,
        }
    }
    pub fn id(self) -> &'static str {
        match self {
            TempoMode::Constant => "constant",
            TempoMode::Follow => "follow",
            TempoMode::Smooth => "smooth",
        }
    }
}

/// The tempo-range hints (fix half/double tempo): id, min, max BPM.
pub const RANGES: [(&str, f64, f64); 4] = [("auto", 60.0, 200.0), ("60-90", 60.0, 90.0), ("80-160", 80.0, 160.0), ("140-200", 140.0, 200.0)];

/// A range hint by id ("80-160") or "min-max".
pub fn parse_range(s: &str) -> Option<(f64, f64)> {
    if let Some((_, a, b)) = RANGES.iter().find(|r| r.0 == s.trim()) {
        return Some((*a, *b));
    }
    let (a, b) = s.split_once('-')?;
    let (a, b): (f64, f64) = (a.trim().parse().ok()?, b.trim().parse().ok()?);
    (a.is_finite() && b.is_finite() && a >= 20.0 && b <= 400.0 && b > a).then_some((a, b))
}

/// What is analysed: these tracks' clips over `range`, or only the listed clips.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct TempoSource {
    pub tracks: Vec<TrackId>,
    pub range: Range,
    /// When non-empty, only these clips (their own ranges) are heard.
    pub clips: Vec<ClipId>,
    /// The main mix as heard (through the mixer: inserts, instruments, sends, faders) instead of
    /// the tracks' clips: the whole-session detection.
    pub mix: bool,
}

/// Audio tracks with clips (the tracks a whole-session detection listens to).
pub fn audio_tracks(s: &Session) -> Vec<TrackId> {
    s.tracks.iter().filter(|t| t.kind.has_playlist() && !t.kind.is_midi() && !t.clips().is_empty()).map(|t| t.id).collect()
}

/// The analysed audio, mono, starting at `src.range.start`: the main mix as heard for
/// [`TempoSource::mix`], else the tracks' (or clips') clip renders, before the mixer.
pub fn source_audio(s: &Session, src: &TempoSource) -> Vec<f32> {
    let max = s.sample_rate.samples(MAX_DETECT_SECONDS);
    let r = Range::new(src.range.start.max(0), src.range.end.min(src.range.start.max(0).saturating_add(max)));
    let len = usize::try_from(r.len().max(0)).unwrap_or(0);
    if src.mix {
        let mut mono = downmix(&soundcraft_mix::render_range(s, r, 1024));
        mono.resize(len, 0.0);
        return mono;
    }
    let mut out = vec![0.0f32; len];
    let mut add = |t: TrackId, part: Range| {
        let part = Range::new(part.start.max(r.start), part.end.min(r.end));
        if part.is_empty() || !s.track(t).is_some_and(|x| x.kind.has_playlist() && !x.kind.is_midi()) {
            return;
        }
        let mono = downmix(&soundcraft_mix::render_clips(s, t, part));
        let at = usize::try_from(part.start - r.start).unwrap_or(0);
        for (o, v) in out.iter_mut().skip(at).zip(mono) {
            *o += v;
        }
    };
    if src.clips.is_empty() {
        for t in &src.tracks {
            add(*t, r);
        }
    } else {
        for id in &src.clips {
            if let Some((t, c)) = s.find_clip(*id) {
                add(t, c.range());
            }
        }
    }
    out
}

/// The detection: beats and downbeats as session positions.
#[derive(Debug, Clone, Default, PartialEq, serde::Serialize)]
pub struct Detection {
    /// Average tempo (quarter notes per minute).
    pub bpm: f64,
    pub beats: Vec<Samples>,
    pub downbeats: Vec<Samples>,
}

/// Quarter notes per beat for the meter at the session start of `r`.
fn meter_of(s: &Session, at: Samples) -> (usize, f64) {
    let m = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(at, s.sample_rate));
    (m.numerator.clamp(1, 64) as usize, 4.0 / f64::from(m.denominator.max(1)))
}

/// Track the beats in `mono` (which starts at session position `start`).
pub fn detect(s: &Session, mono: &[f32], start: Samples, range: (f64, f64)) -> Detection {
    let (bpb, q) = meter_of(s, start);
    let opts = BeatOptions { min_bpm: range.0, max_bpm: range.1, beats_per_bar: bpb };
    let t = track_beats(mono, s.sample_rate.as_f64() as f32, &opts);
    let pos = |v: &[usize]| v.iter().map(|b| start.saturating_add(i64::try_from(*b).unwrap_or(0))).collect::<Vec<_>>();
    Detection { bpm: t.bpm * q, beats: pos(&t.beats), downbeats: pos(&t.downbeats) }
}

/// The tempo points (position, quarter-note BPM) `mode` writes for downbeats `d` with `bpb` beats
/// of `q` quarters per bar. The first point is at the first downbeat.
pub fn tempo_points(s: &Session, d: &[Samples], mode: TempoMode) -> Vec<(Samples, f64)> {
    let Some(first) = d.first().copied() else { return Vec::new() };
    let (bpb, q) = meter_of(s, first);
    let sr = s.sample_rate.as_f64();
    let quarters_per_bar = bpb as f64 * q;
    let bar_bpm = |a: Samples, b: Samples| if b > a { quarters_per_bar * 60.0 * sr / (b - a) as f64 } else { 0.0 };
    match mode {
        TempoMode::Constant => {
            let (Some(a), Some(b)) = (d.first(), d.last()) else { return Vec::new() };
            if d.len() < 2 {
                return Vec::new();
            }
            vec![(*a, bar_bpm(*a, *b) * (d.len() - 1) as f64)]
        }
        TempoMode::Follow | TempoMode::Smooth => {
            let pos: Vec<Samples> = if mode == TempoMode::Smooth {
                (0..d.len())
                    .map(|i| {
                        let (lo, hi) = (i.saturating_sub(2), (i + 3).min(d.len()));
                        // Keep the ends where they are; inside, the average of a symmetric window.
                        let w = (i - lo).min(hi - 1 - i);
                        let (lo, hi) = (i - w, i + w + 1);
                        let v = d.get(lo..hi).unwrap_or(&[]);
                        (v.iter().map(|x| *x as f64).sum::<f64>() / v.len().max(1) as f64).round() as Samples
                    })
                    .collect()
            } else {
                d.to_vec()
            };
            pos.windows(2)
                .filter_map(|w| Some((*w.first()?, bar_bpm(*w.first()?, *w.get(1)?))))
                .filter(|(_, b)| (5.0..=1000.0).contains(b))
                .take(MAX_POINTS)
                .collect()
        }
    }
}

/// Write `points` into the session's tempo map. With `set_bar1`, bar 1 moves to the first point
/// (clips keep their absolute time) and the map is rebuilt from there; otherwise the points replace
/// the tempo events from the first point on and the map before it is kept.
pub fn write_points(s: &mut Session, points: &[(Samples, f64)], set_bar1: bool) -> Result<(), String> {
    let Some((first, bpm0)) = points.first().copied() else { return Err("no tempo found".into()) };
    let sr = s.sample_rate;
    let old = s.tempo.clone();
    let m0 = old.meter_at_tick(old.samples_to_ticks(first, sr));
    let mut map = if set_bar1 {
        let mut m = TempoMap::new(bpm0, m0.numerator, m0.denominator).map_err(|e| e.to_string())?;
        m.set_origin(first.clamp(0, MAX_POSITION));
        m
    } else {
        let mut m = old.clone();
        let t0 = old.samples_to_ticks(first, sr);
        for t in old.tempos().iter().map(|t| t.tick).filter(|t| *t >= t0) {
            m.remove_tempo(t);
        }
        m.set_tempo(t0, bpm0).map_err(|e| e.to_string())?;
        m
    };
    // Points are one bar apart: bar i starts i bars after the first.
    let t0 = map.samples_to_ticks(first, sr);
    let tpb = m0.ticks_per_bar();
    for (i, (_, bpm)) in points.iter().enumerate().skip(1) {
        let tick = t0.saturating_add(tpb.saturating_mul(i64::try_from(i).unwrap_or(i64::MAX)));
        map.set_tempo(tick, *bpm).map_err(|e| e.to_string())?;
    }
    s.tempo = map;
    Ok(())
}

/// Columns of the preview waveform [`analyse`] returns.
pub const WAVE_COLUMNS: usize = 600;

/// A finished analysis for the Detect Tempo dialog.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Analysis {
    pub source: TempoSource,
    pub detection: Detection,
    /// The analysed audio as `WAVE_COLUMNS` (min, max) columns over `source.range`.
    pub waveform: Vec<(f32, f32)>,
}

/// Read the source, track its beats and summarise its waveform (runs on a worker).
pub fn analyse(s: &Session, source: TempoSource, range: (f64, f64)) -> Analysis {
    let mono = source_audio(s, &source);
    let detection = detect(s, &mono, source.range.start, range);
    let per = mono.len().div_ceil(WAVE_COLUMNS).max(1);
    let waveform = mono.chunks(per).map(|c| c.iter().fold((0.0f32, 0.0f32), |(lo, hi), v| (lo.min(*v), hi.max(*v)))).collect();
    Analysis { source, detection, waveform }
}

/// Name of the click track the audition adds.
pub const AUDITION_TRACK: &str = "Detected Beats";

/// A copy of the session for Play in the dialog: the source tracks soloed with a click on the
/// detected beats (downbeats accented). The document is untouched.
pub fn audition_session(s: &Session, a: &Analysis) -> Option<(Session, Range)> {
    let r = a.source.range;
    let len = usize::try_from(r.len()).ok().filter(|l| *l > 0)?;
    let sr = s.sample_rate.as_f64() as f32;
    let rel = |v: &[Samples]| v.iter().filter_map(|b| usize::try_from(b - r.start).ok()).collect::<Vec<_>>();
    let mut click = soundcraft_dsp::beat::click_track(len, &rel(&a.detection.beats), sr, 0);
    let accent = soundcraft_dsp::beat::click_track(len, &rel(&a.detection.downbeats), sr, 1);
    for (c, x) in click.iter_mut().zip(&accent) {
        *c = (*c + 0.6 * *x).clamp(-1.0, 1.0);
    }
    let mut s = s.clone();
    let t = s.add_track(soundcraft_model::TrackKind::Audio, soundcraft_model::ChannelFormat::Mono, Some(AUDITION_TRACK));
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: vec![click] };
    let src = crate::io::add_source(&mut s, AUDITION_TRACK, buf, None, soundcraft_audio_io::FileFormat::Wav);
    let cid = s.new_clip_id();
    crate::edit::place_clip(&mut s, t, soundcraft_model::Clip::audio(cid, AUDITION_TRACK, src, 0, r.start, r.len()));
    for tr in &mut s.tracks {
        tr.mixer.solo = tr.id == t || a.source.tracks.contains(&tr.id);
    }
    Some((s, r))
}

/// A bar stretched or squeezed by more than this share warns (Straighten).
pub const LARGE_DRIFT: f64 = 0.08;

/// Straighten's warp: each beat `b` and where it goes, `(b, target)`. Beat `anchor` stays; the
/// others go `keep_feel` (0 = all the way, 1 = not at all) of the way toward the steady grid of
/// `period` samples through it. Pairs that would fold time back are left out.
pub fn conform_pairs(beats: &[Samples], anchor: usize, period: f64, keep_feel: f64) -> Vec<(Samples, Samples)> {
    let Some(a) = beats.get(anchor).copied() else { return Vec::new() };
    let k = if keep_feel.is_finite() { keep_feel.clamp(0.0, 1.0) } else { 0.0 };
    let mut out: Vec<(Samples, Samples)> = Vec::with_capacity(beats.len());
    for (i, b) in beats.iter().enumerate() {
        let g = a as f64 + (i as f64 - anchor as f64) * period;
        let t = (g + k * (*b as f64 - g)).round() as Samples;
        if out.last().is_none_or(|(pb, pt)| *b > *pb && t > *pt) {
            out.push((*b, t));
        }
    }
    out
}

/// Where timeline position `x` goes under `pairs`: linear between pairs, shifted by the first
/// (last) pair's offset before (after) them.
pub fn warp_at(pairs: &[(Samples, Samples)], x: f64) -> f64 {
    let (Some(first), Some(last)) = (pairs.first(), pairs.last()) else { return x };
    if x <= first.0 as f64 {
        return x + (first.1 - first.0) as f64;
    }
    if x >= last.0 as f64 {
        return x + (last.1 - last.0) as f64;
    }
    let i = pairs.partition_point(|p| (p.0 as f64) <= x).saturating_sub(1);
    match (pairs.get(i), pairs.get(i + 1)) {
        (Some(a), Some(b)) if b.0 > a.0 => a.1 as f64 + (x - a.0 as f64) * (b.1 - a.1) as f64 / (b.0 - a.0) as f64,
        _ => x,
    }
}

/// The largest stretch any bar of `bpb` beats needs under `pairs`, as a share (0.1 = 10 %).
pub fn max_bar_drift(pairs: &[(Samples, Samples)], bpb: usize) -> f64 {
    let bpb = bpb.max(1);
    let mut worst = 0.0f64;
    let mut i = 0;
    while let (Some(a), Some(b)) = (pairs.get(i), pairs.get((i + bpb).min(pairs.len().saturating_sub(1)))) {
        if b.0 > a.0 {
            worst = worst.max(((b.1 - a.1) as f64 / (b.0 - a.0) as f64 - 1.0).abs());
        }
        if i + bpb >= pairs.len() {
            break;
        }
        i += bpb;
    }
    worst
}

/// Beats per bar and quarters per beat at `at` (for callers outside this module).
pub fn meter_at(s: &Session, at: Samples) -> (usize, f64) {
    meter_of(s, at)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn range_hints_parse() {
        assert_eq!(parse_range("80-160"), Some((80.0, 160.0)));
        assert_eq!(parse_range("auto"), Some((60.0, 200.0)));
        assert_eq!(parse_range(" 70 - 140 "), Some((70.0, 140.0)));
        assert_eq!(parse_range("140-70"), None);
        assert_eq!(parse_range("nan-5"), None);
        assert_eq!(TempoMode::parse("Smooth"), Some(TempoMode::Smooth));
    }

    #[test]
    fn conform_pairs_move_toward_the_grid_by_keep_feel() {
        let beats = [1000, 2100, 2950, 4000];
        assert_eq!(conform_pairs(&beats, 0, 1000.0, 0.0), [(1000, 1000), (2100, 2000), (2950, 3000), (4000, 4000)]);
        assert_eq!(conform_pairs(&beats, 0, 1000.0, 0.5), [(1000, 1000), (2100, 2050), (2950, 2975), (4000, 4000)]);
        assert_eq!(conform_pairs(&beats, 0, 1000.0, 1.0).iter().map(|p| p.1).collect::<Vec<_>>(), beats);
        let p = conform_pairs(&beats, 0, 1000.0, 0.0);
        assert_eq!(warp_at(&p, 500.0), 500.0);
        assert_eq!(warp_at(&p, 1550.0), 1500.0);
        assert_eq!(warp_at(&p, 5000.0), 5000.0);
        assert!((max_bar_drift(&p, 1) - 0.176).abs() < 0.01);
        assert!(max_bar_drift(&p, 3) < 1e-9, "the whole span keeps its length");
        assert!(conform_pairs(&[], 0, 1000.0, 0.0).is_empty());
    }

    #[test]
    fn audition_adds_a_soloed_click_and_leaves_the_document() {
        let pos: Vec<usize> = (0..16).map(|i| 12_000 + i * 24_000).collect();
        let (e, tracks, _) = crate::cmd::tempo::tests::click_session(&[&pos]);
        let s = e.session();
        let src = TempoSource { tracks: tracks.clone(), range: Range::new(0, s.content_end()), clips: Vec::new(), mix: false };
        let a = analyse(s, src, (60.0, 200.0));
        assert!((500..=WAVE_COLUMNS).contains(&a.waveform.len()));
        assert!((a.detection.bpm - 120.0).abs() < 0.5);
        let (copy, r) = audition_session(s, &a).unwrap();
        assert_eq!(r, a.source.range);
        assert_eq!(copy.tracks.len(), s.tracks.len() + 1);
        assert!(copy.tracks.iter().all(|t| t.mixer.solo));
        assert_eq!(e.session().tracks.len(), 1);
    }
}

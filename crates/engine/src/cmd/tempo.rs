//! Detect Tempo and the tempo markers on the Tempo ruler: `event.tempo_detect`,
//! `event.tempo_move`, `event.tempo_ramp`.

use super::*;
use crate::cmd;
use crate::tempo_detect::{self as td, Detection, TempoMode, TempoSource};
use serde_json::json;
use soundcraft_model::Clip;

pub fn specs() -> Vec<CommandSpec> {
    vec![
        cmd!(
            "event.tempo_detect",
            "Detect Tempo...",
            ["Event"],
            None,
            "{source?: \"clips\"|\"selection\"|\"session\", clips?, tracks?, start?, end?, mode?: \"constant\"|\"follow\"|\"smooth\" (follow), range?: \"auto\"|\"60-90\"|\"80-160\"|\"140-200\"|\"min-max\", set_bar1?: true, apply?: false, beats?: [samples], downbeats?: [samples]} → {bpm, beats, downbeats, points: [{at, bpm}], bars: [bpm], tracks, start, end, applied}. `beats`/`downbeats` skip the analysis (the dialog passes back what it showed)",
            always,
            tempo_detect
        ),
        cmd!(
            "event.tempo_conform",
            "Straighten",
            [],
            None,
            "{beats: [samples], downbeats?: [samples], bpm?, keep_feel?: 0..1 (0), tracks?, start?, end?, set_tempo?: true} -> {bpm, clips, tracks, pairs, max_drift, warnings}. Stretches the tracks' audio so each beat lands on the steady grid at bpm (default: the beats' average), the same warp on every track; then a constant tempo with bar 1 on the first downbeat",
            always,
            tempo_conform
        ),
        cmd!(
            "event.tempo_move",
            "Move Tempo Change",
            [],
            None,
            "{tick | at: position of the tempo change, to_tick? | to?: new position, bpm?} — moves a tempo change (the one at bar 1 stays) and/or sets its tempo; its ramp goes with it",
            always,
            tempo_move
        ),
        cmd!(
            "event.tempo_ramp",
            "Ramp to Next Tempo",
            [],
            None,
            "{tick | at, ramp?: bool (default: toggle)} — the tempo changes linearly from this change to the next",
            always,
            tempo_ramp
        ),
    ]
}

/// Tracks Straighten works on: `tracks`, else the selected tracks, else every audio track.
pub fn conform_tracks(e: &Engine, p: &Value) -> Result<Vec<TrackId>> {
    let id = "event.tempo_conform";
    let mut tracks = tracks_param(e, id, p)?;
    if tracks.is_empty() {
        tracks = td::audio_tracks(e.session());
    }
    let s = e.session();
    tracks.retain(|t| s.track(*t).is_some_and(|x| x.kind.has_playlist() && !x.kind.is_midi()));
    Ok(tracks)
}

/// Longest stretch of audio Straighten renders (seconds).
const MAX_CONFORM_SECONDS: f64 = 30.0 * 60.0;

fn tempo_conform(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_conform";
    let beats = samples_list(p, "beats").filter(|b| b.len() >= 2).ok_or_else(|| bad(id, "`beats` (at least two positions) required"))?;
    let downbeats = samples_list(p, "downbeats").unwrap_or_default();
    let tracks = conform_tracks(e, p)?;
    if tracks.is_empty() {
        return Err(bad(id, "no audio tracks to straighten"));
    }
    let range = match (position_param(e, id, p, "start")?, position_param(e, id, p, "end")?) {
        (Some(a), Some(b)) if b > a => Some(Range::new(a, b)),
        _ => None,
    };
    let s = e.session();
    let sr = s.sample_rate.as_f64();
    let first = beats.first().copied().unwrap_or(0);
    let (bpb, q) = td::meter_at(s, first);
    let avg = match (beats.first(), beats.last()) {
        (Some(a), Some(b)) if b > a => 60.0 * sr * (beats.len() - 1) as f64 / (b - a) as f64 * q,
        _ => 0.0,
    };
    let bpm = p.get("bpm").and_then(Value::as_f64).unwrap_or(avg);
    if !(bpm.is_finite() && (5.0..=1000.0).contains(&bpm)) {
        return Err(bad(id, "`bpm` must be 5..1000"));
    }
    let keep = f64_or(p, "keep_feel", 0.0);
    if !(0.0..=1.0).contains(&keep) {
        return Err(bad(id, "`keep_feel` must be 0..1"));
    }
    // The first downbeat stays put (else the first beat); the grid runs through it.
    let near = (sr / 50.0) as Samples;
    let anchor = downbeats.first().and_then(|d| beats.iter().position(|b| (b - d).abs() <= near)).unwrap_or(0);
    let period = q * 60.0 * sr / bpm;
    let pairs = td::conform_pairs(&beats, anchor, period, keep);
    let drift = td::max_bar_drift(&pairs, bpb);
    // Every audio clip the warp touches (inside the range when given).
    let mut jobs: Vec<(TrackId, Clip)> = Vec::new();
    for t in &tracks {
        for c in s.track(*t).map(|x| x.clips()).unwrap_or(&[]) {
            if c.is_audio() && range.is_none_or(|r| c.range().overlaps(&r)) {
                jobs.push((*t, c.clone()));
            }
        }
    }
    let (Some(w0), Some(w1)) = (jobs.iter().map(|j| j.1.start).min(), jobs.iter().map(|j| j.1.end()).max()) else {
        return Err(bad(id, "no audio clips to straighten"));
    };
    let f = |x: Samples| td::warp_at(&pairs, x as f64).round() as Samples;
    let total = f(w1) - f(w0);
    if total <= 0 || total > s.sample_rate.samples(MAX_CONFORM_SECONDS) {
        return Err(bad(id, format!("straighten at most {:.0} minutes at a time", MAX_CONFORM_SECONDS / 60.0)));
    }
    // One stretch for all tracks: each segment between beats is planned once, on the sum of the
    // tracks, and applied to every track, so they stay sample-aligned (a drum kit's mics).
    let mut cuts: Vec<Samples> = vec![w0];
    cuts.extend(pairs.iter().map(|p| p.0).filter(|b| *b > w0 && *b < w1));
    cuts.push(w1);
    let used: Vec<TrackId> = tracks.iter().copied().filter(|t| jobs.iter().any(|j| j.0 == *t)).collect();
    let chans: Vec<usize> = used.iter().map(|t| s.track(*t).map_or(1, |x| x.channels().max(1))).collect();
    let mut outs: Vec<Vec<Vec<f32>>> = chans.iter().map(|n| vec![Vec::new(); *n]).collect();
    // The beat joins: each segment runs on past its end beat and that tail crossfades (`JOIN_XF`)
    // into the next segment's head, which starts on the beat exactly; the tail reads near the
    // beat, give or take the stretch's splice search, so the join doesn't step. Where a transient
    // starts on the beat (judged on the sum of the tracks) the segment stops at the beat as
    // before: run on, its splice search could pull the transient in early (a flam), and the
    // transient masks the join.
    let xf = (sr * JOIN_XF) as usize;
    let frame = (sr * 0.04) as usize;
    let mut tails: Vec<Vec<Vec<f32>>> = chans.iter().map(|n| vec![Vec::new(); *n]).collect();
    let fsr = sr as f32;
    for w in cuts.windows(2) {
        let (Some(a), Some(b)) = (w.first().copied(), w.get(1).copied()) else { continue };
        let n_in = usize::try_from(b - a).unwrap_or(0);
        let n_out = usize::try_from(f(b) - f(a)).unwrap_or(0);
        if n_in == 0 || n_out == 0 {
            continue;
        }
        let ratio = n_out as f64 / n_in as f64;
        // The tail, plus a frame: the stretch's last frame is only partly overlapped.
        let mut ext = if b < w1 { (((xf + frame) as f64 / ratio).ceil() as usize).min(usize::try_from(w1 - b).unwrap_or(0)) } else { 0 };
        let mut raws: Vec<Vec<Vec<f32>>> =
            used.iter().zip(&chans).map(|(t, n)| raw_window(s, jobs.iter().filter(|j| j.0 == *t).map(|j| &j.1), a, n_in + ext, *n)).collect();
        let mut guide = vec![0.0f32; n_in + ext];
        for ch in raws.iter().flatten() {
            for (g, v) in guide.iter_mut().zip(ch) {
                *g += *v;
            }
        }
        if onset(&guide, n_in, xf) {
            ext = 0;
            guide.truncate(n_in);
            raws.iter_mut().flatten().for_each(|c| c.truncate(n_in));
        }
        let tail_len = ((ext as f64 * ratio).floor() as usize).saturating_sub(frame).min(xf);
        let plan = soundcraft_dsp::offline::wsola_plan(&guide, ratio, fsr);
        for ((raw, out), tail) in raws.iter().zip(outs.iter_mut()).zip(tails.iter_mut()) {
            for ((o, mut x), t) in out.iter_mut().zip(soundcraft_dsp::offline::wsola_apply(raw, &plan)).zip(tail.iter_mut()) {
                x.resize(n_out + tail_len, 0.0);
                crossfade_join(t, &mut x);
                *t = x.split_off(n_out);
                o.extend_from_slice(&x);
            }
        }
    }
    let s = e.session_mut();
    let start = f(w0);
    let mut made = 0;
    for (t, out) in used.iter().zip(outs) {
        let name = s.track(*t).map(|x| x.name.clone()).unwrap_or_default();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: out };
        let src = crate::io::add_source(s, &format!("{name}-Straight"), buf, None, soundcraft_audio_io::FileFormat::Wav);
        let mine: Vec<&Clip> = jobs.iter().filter(|j| j.0 == *t).map(|j| &j.1).collect();
        // Take this track's old clips out first, then place the new ones.
        if let Some(pl) = s.track_mut(*t).and_then(|x| x.playlist_mut()) {
            pl.clips.retain(|c| !mine.iter().any(|m| m.id == c.id));
        }
        for c in mine {
            let soundcraft_model::ClipContent::Audio { source, offset } = c.content else { continue };
            let (ns, ne) = (f(c.start), f(c.end()));
            let key = |k: &str| format!("elastic.{}.{k}", c.id.0);
            if !s.edit.values.contains_key(&key("src")) {
                s.edit.values.insert(key("src"), source.0 as f64);
                s.edit.values.insert(key("off"), offset as f64);
                s.edit.values.insert(key("len"), c.length as f64);
                s.edit.values.insert(key("start"), c.start as f64);
            }
            let mut nc = c.clone();
            nc.content = soundcraft_model::ClipContent::Audio { source: src, offset: ns - start };
            nc.start = ns;
            nc.length = (ne - ns).max(1);
            nc.stretch = 1.0;
            nc.fade_in.len = nc.fade_in.len.min(nc.length);
            nc.fade_out.len = nc.fade_out.len.min(nc.length);
            let mut marks = vec![c.start];
            marks.extend(pairs.iter().map(|p| p.0).filter(|b| *b > c.start && *b < c.end()));
            marks.push(c.end());
            nc.warp = marks.iter().map(|x| (x - c.start, f(*x) - ns)).collect();
            crate::edit::place_clip(s, *t, nc);
            made += 1;
        }
    }
    let old_map = s.tempo.clone();
    if bool_or(p, "set_tempo", true) {
        let bar1 = pairs.get(anchor).map_or(first, |p| p.1);
        let m = s.tempo.meter_at_tick(s.tempo.samples_to_ticks(bar1, s.sample_rate));
        let mut map = soundcraft_time::TempoMap::new(bpm, m.numerator, m.denominator).map_err(|err| bad(id, err.to_string()))?;
        map.set_origin(bar1);
        s.tempo = map;
    }
    // MIDI clips move and stretch with the timeline: each event goes where the warp puts the time
    // it played at, in ticks of the new tempo map.
    let (new_map, rate) = (s.tempo.clone(), s.sample_rate);
    let mut midi_clips = 0;
    let mut originals: Vec<(u64, Samples, Samples)> = Vec::new();
    for t in s.tracks.iter_mut().filter(|t| t.kind.is_midi()) {
        for c in t.playlists.iter_mut().flat_map(|pl| pl.clips.iter_mut()) {
            if !range.is_none_or(|r| c.range().overlaps(&r)) {
                continue;
            }
            let (start, ns, ne) = (c.start, f(c.start), f(c.end()));
            let soundcraft_model::ClipContent::Midi { sequence } = &mut c.content else { continue };
            warp_sequence(sequence, &old_map, &new_map, rate, start, ns, &f);
            originals.push((c.id.0, start, ns));
            c.start = ns;
            c.length = (ne - ns).max(1);
            midi_clips += 1;
        }
    }
    for (id, old_start, ns) in originals {
        if let Some(seq) = s.midi_originals.get_mut(&id) {
            warp_sequence(seq, &old_map, &new_map, rate, old_start, ns, &f);
        }
    }
    let mut warnings = Vec::new();
    if drift > td::LARGE_DRIFT {
        warnings.push(format!("large drift: a bar is stretched {:.0} %, cymbals may sound stretched", drift * 100.0));
    }
    Ok(json!({
        "bpm": bpm,
        "clips": made,
        "midi_clips": midi_clips,
        "tracks": used.iter().map(|t| t.0).collect::<Vec<_>>(),
        "pairs": pairs.len(),
        "max_drift": drift,
        "warnings": warnings,
    }))
}

/// A MIDI clip's events through Straighten's warp `f`: the clip started at `old_start` under
/// `old` and starts at `new_start` under `new`; each event's tick is where `f` puts the time it
/// played at (note ends too, so a note stretches with its beat).
fn warp_sequence(
    seq: &mut soundcraft_midi::Sequence,
    old: &soundcraft_time::TempoMap,
    new: &soundcraft_time::TempoMap,
    rate: soundcraft_time::SampleRate,
    old_start: Samples,
    new_start: Samples,
    f: &dyn Fn(Samples) -> Samples,
) {
    let (base_old, base_new) = (old.samples_to_ticks(old_start, rate), new.samples_to_ticks(new_start, rate));
    let tick = |t: i64| new.samples_to_ticks(f(old.tick_to_samples(base_old.saturating_add(t), rate)), rate) - base_new;
    for n in &mut seq.notes {
        let (a, b) = (tick(n.start), tick(n.start.saturating_add(n.length)));
        n.start = a;
        n.length = (b - a).max(1);
    }
    for c in &mut seq.ctrls {
        c.tick = tick(c.tick);
    }
    seq.sort();
}

/// Straighten's crossfade at each beat join (seconds).
const JOIN_XF: f64 = 0.005;

/// Whether a transient starts at `at` in `guide`: the `n` samples from it are much louder than
/// the `n` before it.
fn onset(guide: &[f32], at: usize, n: usize) -> bool {
    let energy = |x: &[f32]| x.iter().map(|v| f64::from(*v) * f64::from(*v)).sum::<f64>() / x.len().max(1) as f64;
    let before = guide.get(at.saturating_sub(n)..at).unwrap_or_default();
    let after = guide.get(at..(at + n).min(guide.len())).unwrap_or_default();
    energy(after) > 4.0 * energy(before) + 1e-12
}

/// Fades the previous segment's `tail` out over the head of `head` (equal gain: the two are
/// near the same material).
fn crossfade_join(tail: &[f32], head: &mut [f32]) {
    let n = tail.len().min(head.len());
    for (i, (h, t)) in head.iter_mut().zip(tail).take(n).enumerate() {
        let w = (i + 1) as f32 / (n + 1) as f32;
        *h = *t * (1.0 - w) + *h * w;
    }
}

/// A track's clip material (before clip gain and fades) over `n` samples from timeline position
/// `a`, `channels` wide; silence where no clip plays.
fn raw_window<'a>(s: &soundcraft_model::Session, clips: impl Iterator<Item = &'a Clip>, a: Samples, n: usize, channels: usize) -> Vec<Vec<f32>> {
    let mut out = vec![vec![0.0f32; n]; channels.max(1)];
    let b = a.saturating_add(i64::try_from(n).unwrap_or(0));
    for c in clips {
        let soundcraft_model::ClipContent::Audio { offset, .. } = c.content else { continue };
        let (lo, hi) = (a.max(c.start), b.min(c.end()));
        let Some(len) = usize::try_from(hi - lo).ok().filter(|l| *l > 0) else { continue };
        let Some(data) = soundcraft_mix::clip_source_window(s, c, offset.saturating_add(lo - c.start), len) else { continue };
        if data.is_empty() {
            continue;
        }
        let at = usize::try_from(lo - a).unwrap_or(0);
        for (k, o) in out.iter_mut().enumerate() {
            let Some(src) = data.get(k % data.len()) else { continue };
            for (d, v) in o.iter_mut().skip(at).zip(src.iter().take(len)) {
                *d = *v;
            }
        }
    }
    out
}

/// The tick of an existing tempo change from `tick` or the position `at`.
fn event_tick(e: &Engine, id: &str, p: &Value) -> Result<i64> {
    let s = e.session();
    let tick = match p.get("tick").and_then(Value::as_i64) {
        Some(t) => t,
        None => {
            let at = position_param(e, id, p, "at")?.ok_or_else(|| bad(id, "`tick` or `at` required"))?;
            s.tempo.samples_to_ticks(at, s.sample_rate)
        }
    };
    s.tempo.tempos().iter().any(|t| t.tick == tick).then_some(tick).ok_or_else(|| bad(id, "no tempo change there"))
}

fn tempo_move(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_move";
    let from = event_tick(e, id, p)?;
    let s = e.session();
    let to = match (p.get("to_tick").and_then(Value::as_i64), position_param(e, id, p, "to")?) {
        (Some(t), _) => Some(t),
        (None, Some(at)) => Some(s.tempo.samples_to_ticks(at, s.sample_rate)),
        _ => None,
    };
    let to = match to {
        Some(_) if from == 0 => return Err(bad(id, "the tempo at bar 1 cannot move")),
        Some(t) => t.clamp(1, soundcraft_time::MAX_POSITION),
        None => from,
    };
    let ev = s.tempo.tempos().iter().find(|t| t.tick == from).copied().ok_or_else(|| bad(id, "no tempo change there"))?;
    let bpm = p.get("bpm").and_then(Value::as_f64).unwrap_or(ev.bpm);
    let mut map = s.tempo.clone();
    map.remove_tempo(from);
    map.set_tempo(to, bpm).map_err(|err| bad(id, err.to_string()))?;
    map.set_ramp(to, ev.ramp);
    e.session_mut().tempo = map;
    Ok(json!({"tick": to, "bpm": bpm}))
}

fn tempo_ramp(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_ramp";
    let tick = event_tick(e, id, p)?;
    let s = e.session();
    let now = s.tempo.tempos().iter().any(|t| t.tick == tick && t.ramp);
    let ramp = p.get("ramp").and_then(Value::as_bool).unwrap_or(!now);
    if ramp == now {
        return Ok(json!({"tick": tick, "ramp": ramp}));
    }
    if ramp && !s.tempo.tempos().iter().any(|t| t.tick > tick) {
        return Err(bad(id, "add a tempo change after this one to ramp to"));
    }
    e.session_mut().tempo.set_ramp(tick, ramp);
    Ok(json!({"tick": tick, "ramp": ramp}))
}

/// Which audio a detection hears, from the parameters (else the selection, else the whole session).
pub fn detect_source(e: &Engine, p: &Value) -> Result<TempoSource> {
    let id = "event.tempo_detect";
    let s = e.session();
    // The whole session is analysed as heard: the main mix, through the mixer.
    let whole = || TempoSource { tracks: td::audio_tracks(s), range: Range::new(0, s.content_end()), clips: Vec::new(), mix: true };
    let clips_src = |clips: Vec<ClipId>| {
        let found: Vec<(TrackId, Range)> = clips.iter().filter_map(|c| s.find_clip(*c).map(|(t, c)| (t, c.range()))).collect();
        let start = found.iter().map(|f| f.1.start).min().unwrap_or(0);
        let end = found.iter().map(|f| f.1.end).max().unwrap_or(0);
        let mut tracks: Vec<TrackId> = found.iter().map(|f| f.0).collect();
        tracks.dedup();
        TempoSource { tracks, range: Range::new(start, end), clips, mix: false }
    };
    let selection = |p: &Value| -> Result<TempoSource> {
        let r = range_param(e, id, p)?;
        let mut tracks = tracks_param(e, id, p)?;
        if tracks.is_empty() {
            tracks = td::audio_tracks(s);
        }
        Ok(TempoSource { tracks, range: r, clips: Vec::new(), mix: false })
    };
    let src = match str_param(p, "source") {
        Some("session") => whole(),
        Some("clips") => clips_src(clip_ids_param(e, p)),
        Some("selection") => selection(p)?,
        Some(other) => return Err(bad(id, format!("unknown source `{other}`"))),
        None if p.get("clips").is_some() => clips_src(clip_ids_param(e, p)),
        None if p.get("start").is_some() || p.get("tracks").is_some() || p.get("track").is_some() => selection(p)?,
        None if !s.edit.selected_clips.is_empty() => clips_src(s.edit.selected_clips.clone()),
        None if !s.edit.selection.is_empty() => selection(p)?,
        None => whole(),
    };
    if src.range.is_empty() || (src.tracks.is_empty() && src.clips.is_empty() && !src.mix) {
        return Err(bad(id, "nothing to analyse: select clips or a range with audio"));
    }
    Ok(src)
}

fn samples_list(p: &Value, key: &str) -> Option<Vec<Samples>> {
    let v: Vec<Samples> = p.get(key)?.as_array()?.iter().filter_map(Value::as_i64).filter(|x| (0..=MAX_POSITION).contains(x)).take(200_000).collect();
    let mut v = v;
    v.sort_unstable();
    v.dedup();
    Some(v)
}

fn tempo_detect(e: &mut Engine, p: &Value) -> Result<Value> {
    let id = "event.tempo_detect";
    let mode = match str_param(p, "mode") {
        Some(m) => TempoMode::parse(m).ok_or_else(|| bad(id, format!("unknown mode `{m}`")))?,
        None => TempoMode::Follow,
    };
    let range = match str_param(p, "range") {
        Some(r) => td::parse_range(r).ok_or_else(|| bad(id, format!("unknown range `{r}`")))?,
        None => (60.0, 200.0),
    };
    let src = detect_source(e, p)?;
    let det = match (samples_list(p, "beats"), samples_list(p, "downbeats")) {
        (Some(beats), Some(downbeats)) => {
            let s = e.session();
            let bpm = match (beats.first(), beats.last()) {
                (Some(a), Some(b)) if b > a => 60.0 * s.sample_rate.as_f64() * (beats.len() - 1) as f64 / (b - a) as f64,
                _ => 0.0,
            };
            Detection { bpm, beats, downbeats }
        }
        _ => {
            let s = e.session();
            let mono = td::source_audio(s, &src);
            td::detect(s, &mono, src.range.start, range)
        }
    };
    let points = td::tempo_points(e.session(), &det.downbeats, mode);
    let bars: Vec<f64> = td::tempo_points(e.session(), &det.downbeats, TempoMode::Follow).iter().map(|p| p.1).collect();
    let apply = bool_or(p, "apply", false);
    if apply {
        if points.is_empty() {
            return Err(bad(id, "no tempo found: the audio needs at least two bars of clear beats"));
        }
        td::write_points(e.session_mut(), &points, bool_or(p, "set_bar1", true)).map_err(|m| bad(id, m))?;
    }
    let pts: Vec<Value> = points.iter().map(|(at, bpm)| json!({"at": at, "bpm": bpm})).collect();
    Ok(json!({
        "bpm": det.bpm,
        "beats": det.beats,
        "downbeats": det.downbeats,
        "points": pts,
        "bars": bars,
        "mode": mode.id(),
        "tracks": src.tracks.iter().map(|t| t.0).collect::<Vec<_>>(),
        "start": src.range.start,
        "end": src.range.end,
        "applied": apply,
    }))
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::Engine;
    use soundcraft_audio_io::AudioBuffer;
    use soundcraft_dsp::beat::click_track;
    use soundcraft_model::Clip;

    pub(crate) const SR: f64 = 48_000.0;

    /// Click positions (samples) for a tempo going linearly from `a` to `b` BPM over `beats` beats.
    pub(crate) fn ramp(a: f64, b: f64, beats: usize, start: f64) -> Vec<usize> {
        let mut t = start;
        let mut out = Vec::new();
        for i in 0..beats {
            out.push((t * SR).round() as usize);
            let bpm = a + (b - a) * i as f64 / (beats.max(2) - 1) as f64;
            t += 60.0 / bpm;
        }
        out
    }

    /// A session with one audio track per click list (accent every 4th click), clips at 0.
    pub(crate) fn click_session(tracks: &[&[usize]]) -> (Engine, Vec<TrackId>, Vec<ClipId>) {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        let mut s = Session::default();
        let mut ts = Vec::new();
        let mut cs = Vec::new();
        for (i, pos) in tracks.iter().enumerate() {
            let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some(&format!("Click {i}")));
            let len = pos.last().copied().unwrap_or(0) + 48_000;
            let audio = click_track(len, pos, SR as f32, 4);
            let src = crate::io::add_source(
                &mut s,
                "click",
                AudioBuffer { sample_rate: 48_000, channels: vec![audio] },
                None,
                soundcraft_audio_io::FileFormat::Wav,
            );
            let cid = s.new_clip_id();
            crate::edit::place_clip(&mut s, t, Clip::audio(cid, "click", src, 0, 0, len as i64));
            ts.push(t);
            cs.push(cid);
        }
        (Engine::new(s), ts, cs)
    }

    #[test]
    fn steady_click_gives_its_tempo() {
        let pos = ramp(120.0, 120.0, 48, 0.5);
        let (mut e, _, _) = click_session(&[&pos]);
        let r = e.execute("event.tempo_detect", &json!({"source": "session"})).unwrap();
        let bpm = r["bpm"].as_f64().unwrap();
        assert!((bpm - 120.0).abs() < 0.5, "bpm {bpm}");
        assert_eq!(r["applied"], false);
        assert_eq!(e.session().tempo, soundcraft_time::TempoMap::default(), "analysis alone changes nothing");
    }

    #[test]
    fn the_whole_session_is_heard_through_the_mixer() {
        // A 120 BPM click and a 90 BPM one, the second muted: the mix as heard is 120.
        let (a, b) = (ramp(120.0, 120.0, 48, 0.5), ramp(90.0, 90.0, 36, 0.3));
        let (mut e, tracks, _) = click_session(&[&a, &b]);
        e.session_mut().track_mut(tracks[1]).unwrap().mixer.mute = true;
        let r = e.execute("event.tempo_detect", &json!({"source": "session"})).unwrap();
        let bpm = r["bpm"].as_f64().unwrap();
        assert!((bpm - 120.0).abs() < 0.5, "bpm {bpm}");
        // Muted, the 120 BPM track leaves the 90 BPM one.
        let s = e.session_mut();
        s.track_mut(tracks[0]).unwrap().mixer.mute = true;
        s.track_mut(tracks[1]).unwrap().mixer.mute = false;
        let r = e.execute("event.tempo_detect", &json!({"source": "session"})).unwrap();
        let bpm = r["bpm"].as_f64().unwrap();
        assert!((bpm - 90.0).abs() < 0.5, "bpm {bpm}");
    }

    #[test]
    fn follow_writes_a_point_per_bar_within_one_bpm() {
        let pos = ramp(100.0, 110.0, 81, 0.4);
        let (mut e, _, _) = click_session(&[&pos]);
        let r = e.execute("event.tempo_detect", &json!({"source": "session", "mode": "follow", "apply": true})).unwrap();
        let pts = r["points"].as_array().unwrap();
        assert!(pts.len() >= 18, "{} points", pts.len());
        // The true tempo over each bar: four beats of the ramp starting at that downbeat.
        let dbs: Vec<i64> = r["downbeats"].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect();
        for p in pts {
            let at = p["at"].as_i64().unwrap();
            let near = pos.iter().map(|x| (*x as i64 - at).abs()).min().unwrap();
            let i = pos.iter().position(|x| (*x as i64 - at).abs() < 200).unwrap_or_else(|| panic!("point {at} is {near} from a click"));
            let Some(end) = pos.get(i + 4) else { continue };
            let truth = 4.0 * 60.0 * SR / (*end as f64 - pos[i] as f64);
            let got = p["bpm"].as_f64().unwrap();
            assert!((got - truth).abs() < 1.0, "bar at {at}: {got} vs {truth}");
        }
        // Bar 1 moved to the first downbeat; each later downbeat sits on its bar line.
        let s = e.session();
        assert_eq!(s.tempo.origin(), dbs[0]);
        for (k, d) in dbs.iter().enumerate().take(pts.len()) {
            let bb = s.tempo.bar_beat_at(*d, s.sample_rate);
            let line = s.tempo.samples_at_bar_beat(soundcraft_time::BarBeat { bar: k as i64 + 1, beat: 1, tick: 0 }, s.sample_rate);
            assert!((line - d).abs() <= 2, "downbeat {k} at {d}, bar line {line} ({bb:?})");
        }
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tempo, soundcraft_time::TempoMap::default(), "one undo step");
    }

    #[test]
    fn constant_and_range_hint() {
        let pos = ramp(150.0, 150.0, 64, 0.25);
        let (mut e, _, cids) = click_session(&[&pos]);
        let r = e
            .execute("event.tempo_detect", &json!({"clips": [cids[0].0], "range": "60-90", "mode": "constant", "apply": true, "set_bar1": false}))
            .unwrap();
        assert!((r["bpm"].as_f64().unwrap() - 75.0).abs() < 1.0, "{}", r["bpm"]);
        let s = e.session();
        assert_eq!(s.tempo.origin(), 0, "bar 1 stays");
        let d0 = r["downbeats"][0].as_i64().unwrap();
        let t0 = s.tempo.samples_to_ticks(d0, s.sample_rate);
        assert_eq!(s.tempo.tempos().len(), 2, "the map before the first downbeat is kept");
        assert!((s.tempo.tempo_at_tick(t0) - 75.0).abs() < 1.0);
        assert!(e.execute("event.tempo_detect", &json!({"range": "fast"})).is_err());
    }

    #[test]
    fn tempo_markers_move_retempo_ramp_and_undo() {
        let mut e = Engine::new(soundcraft_model::Session::default());
        let bar = 3840;
        e.execute("event.tempo", &json!({"bpm": 100.0, "at": 0})).unwrap();
        let at = e.session().tempo.tick_to_samples(4 * bar, e.session().sample_rate);
        e.execute("event.tempo", &json!({"bpm": 140.0, "at": at})).unwrap();
        // Drag left/right: the change moves, its tempo kept.
        e.execute("event.tempo_move", &json!({"tick": 4 * bar, "to_tick": 6 * bar})).unwrap();
        let ticks: Vec<(i64, f64)> = e.session().tempo.tempos().iter().map(|t| (t.tick, t.bpm)).collect();
        assert_eq!(ticks, [(0, 100.0), (6 * bar, 140.0)]);
        // Drag up/down: the tempo changes, in place.
        e.execute("event.tempo_move", &json!({"tick": 6 * bar, "bpm": 150.5})).unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(6 * bar), 150.5);
        e.execute("event.tempo_move", &json!({"tick": 0, "bpm": 90.0})).unwrap();
        assert!(e.execute("event.tempo_move", &json!({"tick": 0, "to_tick": 960})).is_err(), "bar 1 stays");
        assert!(e.execute("event.tempo_move", &json!({"tick": 5})).is_err(), "no change there");
        // Ramp to next: linear in between; the last change can't ramp.
        e.execute("event.tempo_ramp", &json!({"tick": 0})).unwrap();
        assert!((e.session().tempo.tempo_at_tick(3 * bar) - 120.25).abs() < 1e-9);
        assert!(e.execute("event.tempo_ramp", &json!({"tick": 6 * bar})).is_err());
        // A moved ramp keeps its ramp; each gesture is one undo step.
        e.execute("edit.undo", &json!({})).unwrap();
        assert!(!e.session().tempo.tempos()[0].ramp);
        e.execute("edit.undo", &json!({})).unwrap();
        assert_eq!(e.session().tempo.tempo_at_tick(0), 100.0);
        e.execute("event.tempo_remove", &json!({"tick": 6 * bar})).unwrap();
        assert_eq!(e.session().tempo.tempos().len(), 1);
    }

    /// Detect the beats of the whole session (no map written).
    fn detected(e: &mut Engine) -> (Vec<i64>, Vec<i64>) {
        let r = e.execute("event.tempo_detect", &json!({"source": "session"})).unwrap();
        let list = |k: &str| r[k].as_array().unwrap().iter().map(|v| v.as_i64().unwrap()).collect::<Vec<_>>();
        (list("beats"), list("downbeats"))
    }

    /// The clicks of track `t` as rendered now, found by the beat tracker.
    fn rendered_clicks(e: &Engine, t: TrackId) -> Vec<i64> {
        let s = e.session();
        let audio = soundcraft_mix::render_clips(s, t, Range::new(0, s.content_end()));
        let mono = soundcraft_dsp::beat::downmix(&audio);
        soundcraft_dsp::beat::track_beats(&mono, SR as f32, &soundcraft_dsp::beat::BeatOptions::default()).beats.iter().map(|b| *b as i64).collect()
    }

    /// Distance (samples) from `at` to the nearest beat of the session's tempo map.
    fn off_grid(s: &soundcraft_model::Session, at: i64) -> i64 {
        let tick = s.tempo.samples_to_ticks_f(at, s.sample_rate);
        let beat = (tick / 960.0).round() as i64 * 960;
        (s.tempo.tick_to_samples(beat, s.sample_rate) - at).abs()
    }

    #[test]
    fn straighten_puts_a_drifting_click_on_the_grid() {
        let pos = ramp(100.0, 110.0, 81, 0.4);
        let (mut e, tracks, cids) = click_session(&[&pos]);
        let (beats, downbeats) = detected(&mut e);
        let r = e.execute("event.tempo_conform", &json!({"beats": beats, "downbeats": downbeats, "bpm": 105.0})).unwrap();
        assert_eq!(r["clips"], 1);
        assert!(r["warnings"].as_array().unwrap().is_empty(), "about ±5 %: no large-drift warning");
        let s = e.session();
        assert_eq!(s.tempo.tempos().len(), 1);
        assert_eq!(s.tempo.tempos()[0].bpm, 105.0);
        let clicks = rendered_clicks(&e, tracks[0]);
        assert!(clicks.len() + 2 >= pos.len(), "{} clicks", clicks.len());
        let worst = clicks.iter().map(|c| off_grid(s, *c)).max().unwrap();
        assert!(worst <= 96, "every click within 2 ms of the grid: worst {worst} samples");
        // The clip carries its warp markers; Remove Warp restores the original.
        let c = s.find_clip(cids[0]).unwrap().1.clone();
        assert!(c.warp.len() > 70, "{} warp markers", c.warp.len());
        assert!(c.warp.windows(2).all(|w| w[0].0 < w[1].0 && w[0].1 < w[1].1));
        e.execute("clip.remove_warp", &json!({"clips": [cids[0].0]})).unwrap();
        let back = e.session().find_clip(cids[0]).unwrap().1;
        assert!(back.warp.is_empty());
        assert_eq!((back.start, back.length), (0, *pos.last().unwrap() as i64 + 48_000));
        let again = rendered_clicks(&e, tracks[0]);
        assert!(again.iter().zip(&pos).all(|(a, p)| (a - *p as i64).abs() < 48), "the original clicks are back");
    }

    #[test]
    fn straighten_keeps_tracks_sample_aligned_and_keep_feel_halves_the_deviation() {
        let pos = ramp(100.0, 110.0, 41, 0.4);
        // Two mics of one kit: the same clicks, the second quieter with a low hum on top.
        let kit = || {
            let (mut e, tracks, cids) = click_session(&[&pos, &pos]);
            let s = e.session_mut();
            let c = s.find_clip(cids[1]).unwrap().1.clone();
            let w = soundcraft_mix::clip_source_window(s, &c, 0, c.length as usize).unwrap();
            let mic2: Vec<f32> = w[0].iter().enumerate().map(|(i, v)| v * 0.5 + 0.05 * (i as f32 * 0.01).sin()).collect();
            let src = crate::io::add_source(
                s,
                "mic2",
                AudioBuffer { sample_rate: 48_000, channels: vec![mic2] },
                None,
                soundcraft_audio_io::FileFormat::Wav,
            );
            s.find_clip_mut(cids[1]).unwrap().content = soundcraft_model::ClipContent::Audio { source: src, offset: 0 };
            (e, tracks, cids)
        };
        let (mut e, tracks, cids) = kit();
        let (beats, downbeats) = detected(&mut e);
        let (mut half, _, _) = kit();
        e.execute("event.tempo_conform", &json!({"beats": beats, "downbeats": downbeats, "bpm": 105.0})).unwrap();
        let s = e.session();
        let (a, b) = (s.find_clip(cids[0]).unwrap().1, s.find_clip(cids[1]).unwrap().1);
        assert_eq!(a.warp, b.warp, "identical warp maps");
        assert_eq!((a.start, a.length), (b.start, b.length));
        let (ca, cb) = (rendered_clicks(&e, tracks[0]), rendered_clicks(&e, tracks[1]));
        assert_eq!(ca.len(), cb.len());
        let worst = ca.iter().zip(&cb).map(|(x, y)| (x - y).abs()).max().unwrap();
        assert!(worst <= 1, "the pair stays sample-aligned: {worst}");
        // Keep feel 50 %: each click sits half way between where it was played and the grid.
        let r = half
            .execute("event.tempo_conform", &json!({"beats": beats, "downbeats": downbeats, "bpm": 105.0, "keep_feel": 0.5, "tracks": [tracks[0].0]}))
            .unwrap();
        assert_eq!(r["tracks"].as_array().unwrap().len(), 1);
        let hs = half.session();
        let full: Vec<i64> = ca.iter().map(|c| off_grid(s, *c)).collect();
        let anchor = hs.tempo.origin();
        let period = 60.0 * SR / 105.0;
        let k0 = pos.iter().position(|p| (*p as i64 - anchor).abs() < 200).unwrap();
        for (k, (orig, now)) in pos.iter().zip(rendered_clicks(&half, tracks[0])).enumerate() {
            let grid = anchor as f64 + (k as f64 - k0 as f64) * period;
            let (dev0, dev1) = (*orig as f64 - grid, now as f64 - grid);
            assert!((dev1 - dev0 / 2.0).abs() <= 96.0, "click {k}: {dev0:.0} → {dev1:.0}");
        }
        assert!(full.iter().all(|d| *d <= 96));
    }

    #[test]
    fn straighten_moves_midi_clips_with_the_timeline() {
        use soundcraft_midi::{Note, Sequence};
        use soundcraft_model::{ChannelFormat, TrackKind};
        let pos = ramp(100.0, 110.0, 41, 0.4);
        let (mut e, _, _) = click_session(&[&pos]);
        // A MIDI part played with the clicks: a note on each, an eighth long, in a clip from 0.
        let s = e.session_mut();
        let t = s.add_track(TrackKind::Midi, ChannelFormat::Stereo, Some("Keys"));
        let (rate, len) = (s.sample_rate, *pos.last().unwrap() as i64 + 48_000);
        let notes: Vec<Note> = pos
            .iter()
            .map(|p| {
                let a = s.tempo.samples_to_ticks(*p as i64, rate);
                let length = s.tempo.samples_to_ticks(*p as i64 + 12_000, rate) - a;
                Note { pitch: 60, velocity: 100, release_velocity: 64, channel: 0, start: a, length }
            })
            .collect();
        let cid = s.new_clip_id();
        crate::edit::place_clip(s, t, Clip::midi(cid, "keys", 0, len, Sequence { notes, ctrls: Vec::new() }));
        let (beats, downbeats) = detected(&mut e);
        let r = e.execute("event.tempo_conform", &json!({"beats": beats, "downbeats": downbeats, "bpm": 105.0})).unwrap();
        assert_eq!(r["midi_clips"], 1);
        let s = e.session();
        let c = s.find_clip(cid).unwrap().1;
        let base = s.tempo.samples_to_ticks(c.start, s.sample_rate);
        let Some(seq) = c.midi_playing() else { panic!("a MIDI clip") };
        assert_eq!(seq.notes.len(), pos.len());
        // Each note is on the grid now, as its click is.
        let worst = seq.notes.iter().map(|n| off_grid(s, s.tempo.tick_to_samples(base + n.start, s.sample_rate))).max().unwrap();
        assert!(worst <= 96, "every note within 2 ms of the grid: worst {worst} samples");
    }

    #[test]
    fn straighten_crossfades_the_beat_joins() {
        use soundcraft_model::{ChannelFormat, Session, TrackKind};
        // A steady tone: any jump at a beat join stands out against its smooth slope.
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
        let len = 10 * 48_000;
        let tone: Vec<f32> = (0..len).map(|i| 0.5 * (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / SR as f32).sin()).collect();
        let src = crate::io::add_source(
            &mut s,
            "tone",
            AudioBuffer { sample_rate: 48_000, channels: vec![tone] },
            None,
            soundcraft_audio_io::FileFormat::Wav,
        );
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "tone", src, 0, 0, len as i64));
        let mut e = Engine::new(s);
        let beats: Vec<i64> = ramp(100.0, 115.0, 16, 0.5).into_iter().map(|b| b as i64).collect();
        e.execute("event.tempo_conform", &json!({"beats": beats, "bpm": 105.0})).unwrap();
        let s = e.session();
        let c = s.find_clip(cid).unwrap().1.clone();
        let w = soundcraft_mix::clip_source_window(s, &c, 0, c.length as usize).unwrap();
        // The tone's steepest step is 2π·220/48k·0.5 ≈ 0.0144; WSOLA's own overlap-adds stay near it.
        let jump = w[0].windows(2).skip(4_800).take(c.length as usize - 9_600).map(|p| (p[1] - p[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.05, "a click at a join: a step of {jump}");
    }

    #[test]
    fn given_beats_skip_the_analysis() {
        let (mut e, _, _) = click_session(&[&ramp(120.0, 120.0, 16, 0.0)]);
        let d: Vec<i64> = (0..5).map(|i| i * 96_000).collect();
        let r = e.execute("event.tempo_detect", &json!({"source": "session", "beats": d, "downbeats": d, "apply": true})).unwrap();
        assert_eq!(r["points"].as_array().unwrap().len(), 4);
        assert!((e.session().tempo.tempos()[0].bpm - 120.0).abs() < 1e-6);
    }
}

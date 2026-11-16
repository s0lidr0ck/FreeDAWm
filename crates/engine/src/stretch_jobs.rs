//! Stretches heard at once: the pitch-preserving stretch renders (Elastic Properties, Conform to
//! Tempo, TCE to the timeline, audio following a tempo edit on an Elastic track, warp markers)
//! are made off the UI thread while the clip is heard through the real-time stretcher.
//!
//! When the engine defers renders ([`crate::Engine::defer_stretch`], the app with a worker), a
//! command that would render a stretch instead reserves the render's source (its id, name, length
//! and channels, no audio yet) and records how to hear it meanwhile in
//! [`soundcraft_model::Session::live_stretch`]: the original audio, the map from render positions
//! to file positions, and the job that makes the render. The clip points at the reserved source
//! from the start, exactly as it will once the render is in, so trims, moves and splits made
//! before the render arrives need nothing redone. The mixer reads such a clip through
//! [`soundcraft_dsp::live_stretch`] (no latency, no allocation, exact across locates and loop
//! wraps). A worker ([`next_job`], [`Job::run`]) makes the render with the very same offline
//! algorithm the command used to run, and [`install`] puts it in the session's pool and drops the
//! live entry: from then on the clip reads the render, sample for sample what rendering in the
//! command would have given. The swap is not an edit (no undo step); renders already made are
//! kept so that an undo or a redo that brings a live entry back gets its render at once
//! ([`Engine::restore_stretch_renders`]). A save renders what is still live first.
//!
//! Without deferral (the CLI, scripts, tests by default) the commands render as they always did.

use crate::Engine;
use soundcraft_model::{Clip, LiveStretch, Session, SourceAudio, SourceId, StretchKind};
use std::cell::Cell;
use std::sync::Arc;

thread_local! {
    static DEFER: Cell<bool> = const { Cell::new(false) };
}

/// Whether the command running on this thread defers its stretch renders.
pub fn deferring() -> bool {
    DEFER.with(Cell::get)
}

/// Run `f` with deferral set to `on` on this thread (restored after).
pub(crate) fn with_defer<T>(on: bool, f: impl FnOnce() -> T) -> T {
    let was = DEFER.with(|d| d.replace(on));
    let out = f();
    DEFER.with(|d| d.set(was));
    out
}

/// The render a live stretch's job makes from its material `ch` (the base clip's window), at
/// `sample_rate`: what the commands rendered in place before deferral.
pub fn render(kind: &StretchKind, ch: &[Vec<f32>], sample_rate: f32) -> Vec<Vec<f32>> {
    use soundcraft_dsp::offline::{StretchMode, time_stretch_mode, warp_render};
    match kind {
        StretchKind::Ratio { ratio, mode, frames } => {
            let mode = mode.as_deref().and_then(StretchMode::from_id).unwrap_or_default();
            let mut out = time_stretch_mode(ch, *ratio, sample_rate, mode);
            let want = usize::try_from(*frames).unwrap_or(0);
            for o in &mut out {
                o.resize(want, 0.0);
            }
            out
        }
        StretchKind::Warp { marks, mode } => {
            let mode = Some(mode.as_deref().and_then(StretchMode::from_id).unwrap_or_default());
            let um: Vec<(usize, usize)> = marks.iter().map(|m| (usize::try_from(m.0).unwrap_or(0), usize::try_from(m.1).unwrap_or(0))).collect();
            warp_render(ch, &um, sample_rate, mode)
        }
    }
}

/// The material a live stretch renders from: the base clip's window as it plays (Clean Vocal
/// mixed in), `None` when its audio is gone.
fn material(s: &Session, live: &LiveStretch) -> Option<Vec<Vec<f32>>> {
    soundcraft_mix::clip_source_window(s, &live.base, live.start.max(0), live.len)
}

/// A stretch render for `kind` made from `base`'s window `[start, start + len)`: rendered now,
/// or (deferring) reserved and heard live until a worker renders it. `channels` is the
/// render's channel count. `None` when the material is missing.
pub(crate) fn stretch_source(s: &mut Session, name: &str, base: &Clip, start: i64, len: usize, kind: StretchKind) -> Option<SourceId> {
    let frames = kind.frames();
    if deferring() && frames > 0 {
        let source = base.source()?;
        let channels = s.pool.get(source).map(|a| a.num_channels())?;
        let live = LiveStretch { source, map: kind.map(start, len), base: base.clone(), start, len, kind };
        let id = crate::io::reserve_source(s, name, channels, frames);
        s.live_stretch.insert(id, live);
        return Some(id);
    }
    let live = LiveStretch { source: base.source()?, map: Vec::new(), base: base.clone(), start, len, kind };
    let ch = material(s, &live)?;
    let out = render(&live.kind, &ch, s.sample_rate.as_f64() as f32);
    let buf = soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: out };
    Some(crate::io::add_source(s, name, buf, None, soundcraft_audio_io::FileFormat::Wav))
}

/// A render to make: which reserved source, and the live entry it was asked for (installed only
/// while the session still holds that very entry).
#[derive(Debug, Clone)]
pub struct Job {
    pub id: SourceId,
    pub live: LiveStretch,
    snap: Arc<Session>,
}

impl Job {
    /// Make the render (slow: run it off the UI thread). `None` when the material is gone.
    pub fn run(&self) -> Option<Vec<Vec<f32>>> {
        let ch = material(&self.snap, &self.live)?;
        Some(render(&self.live.kind, &ch, self.snap.sample_rate.as_f64() as f32))
    }
}

/// The next live stretch to render, nearest the start of the session first.
pub fn next_job(e: &Engine) -> Option<Job> {
    let s = e.session_arc();
    let (id, live) = s.live_stretch.iter().find(|(id, l)| !e.stretch_done.contains_key(id) && s.pool.get(l.source).is_some())?;
    Some(Job { id: *id, live: live.clone(), snap: Arc::clone(&s) })
}

/// Put a finished render in place: the reserved source gets its audio and stops being live. A
/// render whose entry changed or went meanwhile is kept for an undo or redo but not installed.
/// Not an edit: no undo step. Returns whether the session changed.
pub fn install(e: &mut Engine, job: &Job, out: Vec<Vec<f32>>) -> bool {
    let sr = e.session().sample_rate.hz();
    let audio = Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: sr, channels: out }));
    e.stretch_done.insert(job.id, (job.live.clone(), Arc::clone(&audio)));
    e.restore_stretch_renders()
}

/// Render every live stretch now (a save, a bounce that must not wait, the tests).
pub fn finish_all(e: &mut Engine) -> usize {
    let mut n = 0;
    for _ in 0..10_000 {
        let Some(job) = next_job(e) else { break };
        match job.run() {
            Some(out) => {
                install(e, &job, out);
                n += 1;
            }
            None => {
                // Material gone: nothing can be rendered for it; forget it so the loop ends.
                e.stretch_done.insert(job.id, (job.live.clone(), Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer::default()))));
            }
        }
    }
    n
}

/// Render `source` now if it is a live stretch (an edit about to read its audio: stretching it
/// again). Returns whether it was.
pub(crate) fn settle(s: &mut Session, source: SourceId) -> bool {
    let Some(live) = s.live_stretch.get(&source).cloned() else { return false };
    let Some(ch) = material(s, &live) else { return false };
    let out = render(&live.kind, &ch, s.sample_rate.as_f64() as f32);
    s.pool.insert(source, Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: s.sample_rate.hz(), channels: out })));
    s.live_stretch.remove(&source);
    true
}

/// `s` with every live stretch rendered into its pool (what a save writes), or `None` when
/// nothing is live.
pub fn rendered(s: &Session) -> Option<Session> {
    if s.live_stretch.is_empty() {
        return None;
    }
    let mut out = s.clone();
    let sr = s.sample_rate;
    for (id, live) in &s.live_stretch {
        let Some(ch) = material(s, live) else { continue };
        let buf = render(&live.kind, &ch, sr.as_f64() as f32);
        out.pool.insert(*id, Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: sr.hz(), channels: buf })));
    }
    out.live_stretch.clear();
    Some(out)
}

impl Engine {
    /// Install every render made for a live entry the session holds (after a render finishes,
    /// and after an undo or redo brings a live entry back). Returns whether the session changed.
    pub fn restore_stretch_renders(&mut self) -> bool {
        let ready: Vec<SourceId> = self
            .session()
            .live_stretch
            .iter()
            .filter(|(id, live)| self.stretch_done.get(id).is_some_and(|(made, a)| made == *live && a.num_channels() > 0))
            .map(|(id, _)| *id)
            .collect();
        if ready.is_empty() {
            return false;
        }
        let audio: Vec<(SourceId, Arc<SourceAudio>)> =
            ready.iter().filter_map(|id| self.stretch_done.get(id).map(|(_, a)| (*id, Arc::clone(a)))).collect();
        let s = self.session_mut();
        for (id, a) in audio {
            s.pool.insert(id, a);
            s.live_stretch.remove(&id);
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use crate::Engine;
    use serde_json::json;
    use soundcraft_model::{ChannelFormat, Clip, ClipId, Session, TrackId, TrackKind};
    use soundcraft_time::Range;

    const SR: f64 = 48_000.0;

    /// A mono 440 Hz tone of 2 s on an Elastic (polyphonic) track, and a plain clip of the same
    /// file on another track; deferring or not.
    fn tone(defer: bool) -> (Engine, TrackId, ClipId, TrackId) {
        let mut s = Session::default();
        let t = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Tone"));
        let plain = s.add_track(TrackKind::Audio, ChannelFormat::Mono, Some("Plain"));
        let v: Vec<f32> = (0..96_000).map(|i| (std::f64::consts::TAU * 440.0 * i as f64 / SR).sin() as f32 * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] };
        let src = crate::io::add_source(&mut s, "Tone", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let cid = s.new_clip_id();
        crate::edit::place_clip(&mut s, t, Clip::audio(cid, "Tone", src, 0, 0, 96_000));
        let pid = s.new_clip_id();
        crate::edit::place_clip(&mut s, plain, Clip::audio(pid, "Plain", src, 1_000, 5_000, 50_000));
        s.track_mut(t).unwrap().elastic = Some("polyphonic".into());
        let mut e = Engine::new(s);
        e.defer_stretch = defer;
        (e, t, cid, plain)
    }

    fn out(e: &Engine, t: TrackId, len: i64) -> Vec<f32> {
        soundcraft_mix::render_clips(e.session(), t, Range::new(0, len)).remove(0)
    }

    /// The frequency the grains play at, over output samples `[from, to)` of `v` (positions
    /// from the clip start): measured in the middle fifth of each grain, where one grain is all
    /// but alone (the other's fade is under a tenth), from the first to the last rising zero
    /// crossing, averaged over the grains. This is the pitch the stretcher holds; between grains
    /// two copies at different phases overlap, which a whole-span count or spectrum would mix
    /// into the measure (the phasiness the offline render removes).
    fn hz_of(v: &[f32], from: usize, to: usize) -> f64 {
        let grain = SR * soundcraft_dsp::live_stretch::GRAIN_SECS;
        let mut sum = 0.0;
        let mut n = 0.0;
        let mut j = (from as f64 / grain).ceil();
        while (j + 1.0) * grain < to as f64 {
            for k in [0.0, 0.5] {
                let centre = (j + 0.5 - k) * grain;
                let (a, b) = ((centre - 0.1 * grain) as usize, (centre + 0.1 * grain) as usize);
                let Some(w) = v.get(a..b) else { continue };
                let ups: Vec<f64> = w
                    .windows(2)
                    .enumerate()
                    .filter(|(_, x)| x[0] < 0.0 && x[1] >= 0.0)
                    .map(|(i, x)| i as f64 + f64::from(-x[0] / (x[1] - x[0])))
                    .collect();
                if let (Some(f), Some(l)) = (ups.first(), ups.last())
                    && ups.len() >= 3
                {
                    sum += (ups.len() - 1) as f64 * SR / (l - f);
                    n += 1.0;
                }
            }
            j += 1.0;
        }
        if n > 0.0 { sum / n } else { 0.0 }
    }

    fn rms(v: &[f32]) -> f64 {
        (v.iter().map(|x| f64::from(*x).powi(2)).sum::<f64>() / v.len().max(1) as f64).sqrt()
    }

    /// Elastic Properties is heard at once, at its new length and the old pitch, through the
    /// real-time stretcher; the worker's render then replaces it, sample for sample what the
    /// command renders without deferral. A clip with no stretch plays exactly as before.
    #[test]
    fn a_stretch_is_heard_at_once_and_its_render_replaces_it() {
        let (mut now, t, cid, plain) = tone(false);
        now.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 1.5})).unwrap();
        let rendered = out(&now, t, 144_000);
        let plain_before = out(&now, plain, 60_000);

        let (mut e, t2, cid2, plain2) = tone(true);
        e.execute("clip.elastic_properties", &json!({"clips": [cid2.0], "ratio": 1.5})).unwrap();
        let c = e.session().find_clip(cid2).unwrap().1.clone();
        assert_eq!(c.length, 144_000, "the new length at once");
        let src = c.source().unwrap();
        assert!(e.session().live_stretch.contains_key(&src), "heard live");
        assert!(e.session().pool.get(src).is_none(), "no render yet");
        let live = out(&e, t2, 144_000);
        let hz = hz_of(&live, 10_000, 134_000);
        assert!((hz / 440.0 - 1.0).abs() < 0.01, "the pitch holds: {hz} Hz");
        assert!(rms(&live[120_000..134_000]) > 0.25, "material to the end");
        // A plain clip beside it: bit-identical, live stretch or not.
        assert_eq!(out(&e, plain2, 60_000), plain_before);

        // The worker renders; the clip now plays the render.
        let job = super::next_job(&e).unwrap();
        let rev = e.revision;
        let done = job.run().unwrap();
        assert!(super::install(&mut e, &job, done));
        assert!(e.revision != rev && e.session().live_stretch.is_empty());
        assert!(super::next_job(&e).is_none());
        assert_eq!(out(&e, t2, 144_000), rendered, "the very render the command made in place");
        // Not an undo step: one undo takes the stretch back.
        assert!(e.undo());
        assert_eq!(e.session().find_clip(cid2).unwrap().1.length, 96_000);
    }

    /// An edit made before the render arrives keeps it; an undo back to the stretched state
    /// that still holds the live entry gets the render that was made (no second render).
    #[test]
    fn undo_and_redo_find_the_render_made_meanwhile() {
        let (mut e, _, cid, plain) = tone(true);
        e.execute("clip.elastic_properties", &json!({"clips": [cid.0], "ratio": 0.75})).unwrap();
        let pc = e.session().track(plain).unwrap().clips()[0].id;
        e.execute("clip.mute", &json!({"clips": [pc.0]})).ok();
        e.execute("edit.move_clips", &json!({"clips": [pc.0], "by": 100})).unwrap();
        assert_eq!(super::finish_all(&mut e), 1);
        assert!(e.session().live_stretch.is_empty());
        assert!(e.undo());
        // The stretched state before the move held the live entry: the render is put back.
        assert!(e.session().live_stretch.is_empty(), "restored from the render made");
        let src = e.session().find_clip(cid).unwrap().1.source().unwrap();
        assert!(e.session().pool.get(src).is_some());
        assert!(super::next_job(&e).is_none());
    }

    /// A tempo edit on a tick track with an Elastic clip, and a warp-marker drag, are heard
    /// live; a save writes the render.
    #[test]
    fn tempo_follow_and_warp_markers_defer_and_a_save_renders() {
        let (mut e, t, cid, _) = tone(true);
        e.session_mut().track_mut(t).unwrap().ticks_timebase = true;
        e.execute("event.tempo", &json!({"bpm": 100.0, "at": 0})).unwrap();
        let c = e.session().find_clip(cid).unwrap().1.clone();
        assert!((c.length - 115_200).abs() <= 2, "120 to 100 bpm: 1.2 times as long: {}", c.length);
        assert_eq!(e.session().live_stretch.len(), 1);
        let s = super::rendered(e.session()).unwrap();
        assert!(s.live_stretch.is_empty());
        assert!(s.pool.get(c.source().unwrap()).is_some(), "a save writes the render");
        super::finish_all(&mut e);

        e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "add", "at": 40_000})).unwrap();
        e.execute("clip.warp_marker", &json!({"clip": cid.0, "op": "move", "index": 1, "to": 50_000})).unwrap();
        assert_eq!(e.session().live_stretch.len(), 1, "the drag is heard live");
        let live = out(&e, t, 110_000);
        assert!((hz_of(&live, 5_000, 45_000) / 440.0 - 1.0).abs() < 0.01);
        assert!((hz_of(&live, 60_000, 105_000) / 440.0 - 1.0).abs() < 0.01);
        super::finish_all(&mut e);
        assert!(e.session().live_stretch.is_empty());
    }
}

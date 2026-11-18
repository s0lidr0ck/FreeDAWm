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

/// Which transpose render a job makes: clip, file, cents, first file position and length, and
/// the Clean Vocal mix it renders (cleaned source, amount bits and bypass), if any.
pub type PitchKey = (soundcraft_model::ClipId, SourceId, i32, i64, usize, Option<(SourceId, u32, bool)>);

/// How much of the file beyond a transposed clip's window its render takes (so a small trim
/// open still finds it), seconds.
pub const PITCH_HANDLE_SECS: f64 = 1.0;

/// A transposed clip's high-quality render to make (Clip › Real-Time › Transpose): its file
/// window, pitch-shifted offline by Pitch Shift's algorithm, at the same length.
#[derive(Debug, Clone)]
pub struct PitchJob {
    pub key: PitchKey,
    base: Clip,
    snap: Arc<Session>,
}

impl PitchJob {
    /// Make the render (slow: off the UI thread). `None` when the audio is gone.
    pub fn run(&self) -> Option<soundcraft_model::PitchRender> {
        let (_, source, cents, start, len, _) = self.key;
        let ch = soundcraft_mix::clip_heard_window(&self.snap, &self.base, start, len)?;
        let sr = self.snap.sample_rate;
        let out = soundcraft_dsp::offline::pitch_shift(&ch, cents as f32 / 100.0, sr.as_f64() as f32);
        let audio = Arc::new(SourceAudio::new(soundcraft_audio_io::AudioBuffer { sample_rate: sr.hz(), channels: out }));
        // The render is of what the clip is heard playing: Clean Vocal's mix blended in.
        let clean = self.base.clean_alt().copied();
        Some(soundcraft_model::PitchRender { source, channel: self.base.source_channel, cents, start, audio, clean })
    }
}

/// The next transposed clip without a render that serves it (only where renders are made in
/// the background: the CLI and scripts keep the live shifter alone, as before).
pub fn next_pitch_job(e: &Engine) -> Option<PitchJob> {
    if !e.defer_stretch {
        return None;
    }
    let s = e.session_arc();
    let handle = s.sample_rate.samples(PITCH_HANDLE_SECS).max(0);
    for c in s.all_clips().filter(|c| c.transpose_cents != 0 && c.is_audio()) {
        let Some(source) = c.source() else { continue };
        let Some(audio) = s.pool.get(source) else { continue };
        if s.pitch_renders.0.get(&c.id).is_some_and(|r| r.serves(c)) {
            continue;
        }
        let frames = i64::try_from(audio.frames()).unwrap_or(0);
        let span = c.source_span(if c.loop_len > 0 { c.loop_len } else { c.length });
        let lo = c.source_offset().saturating_sub(handle).max(0);
        let hi = c.source_offset().saturating_add(span).saturating_add(handle).min(frames);
        let Some(len) = usize::try_from(hi.saturating_sub(lo)).ok().filter(|n| *n > 0) else { continue };
        let clean = c.clean_alt().map(|k| (k.source, k.mix.to_bits(), k.bypass));
        let key = (c.id, source, c.transpose_cents, lo, len, clean);
        if e.pitch_tried.contains(&key) {
            continue;
        }
        return Some(PitchJob { key, base: c.clone(), snap: Arc::clone(&s) });
    }
    None
}

/// Put a finished transpose render in place (derived data: no undo step). Returns whether it
/// serves its clip as the clip is now.
pub fn install_pitch(e: &mut Engine, job: &PitchJob, render: Option<soundcraft_model::PitchRender>) -> bool {
    e.pitch_tried.insert(job.key);
    let Some(render) = render else { return false };
    let clip = job.key.0;
    if !e.session().find_clip(clip).is_some_and(|(_, c)| render.serves(c)) {
        return false;
    }
    let mut map = (*e.session().pitch_renders.0).clone();
    map.insert(clip, Arc::new(render));
    // Derived data: the session is no more modified than it was.
    let dirty = e.dirty;
    e.session_mut().pitch_renders = soundcraft_model::PitchRenders(Arc::new(map));
    e.dirty = dirty;
    true
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

    /// The strongest frequency of `v` between `lo` and `hi` Hz, in 0.5 Hz steps (a steady tone:
    /// the offline render has no grains to beat).
    fn peak_hz(v: &[f32], lo: f64, hi: f64) -> f64 {
        let v = &v[..v.len().min(24_000)];
        let power = |hz: f64| {
            let w = std::f64::consts::TAU * hz / SR;
            let (mut re, mut im) = (0.0, 0.0);
            for (i, x) in v.iter().enumerate() {
                re += f64::from(*x) * (w * i as f64).cos();
                im += f64::from(*x) * (w * i as f64).sin();
            }
            re * re + im * im
        };
        let steps = ((hi - lo) * 2.0) as i64;
        (0..steps).map(|k| lo + k as f64 * 0.5).max_by(|a, b| power(*a).total_cmp(&power(*b))).unwrap_or(0.0)
    }

    /// A transposed clip playing a Clean Vocal mix gets a render too, of the mix it plays.
    #[test]
    fn a_transposed_clip_with_clean_vocal_gets_a_render_of_what_it_plays() {
        let (mut e, t, cid, _) = tone(true);
        e.session_mut().track_mut(t).unwrap().elastic = None;
        // The cleaned audio: a 660 Hz tone, all of it heard (mix 1).
        let v: Vec<f32> = (0..96_000).map(|i| (std::f64::consts::TAU * 660.0 * i as f64 / SR).sin() as f32 * 0.5).collect();
        let buf = soundcraft_audio_io::AudioBuffer { sample_rate: 48_000, channels: vec![v] };
        let s = e.session_mut();
        let cleaned = crate::io::add_source(s, "Tone clean", buf, None, soundcraft_audio_io::FileFormat::Wav);
        let c = s.find_clip_mut(cid).unwrap();
        let original = c.source().unwrap();
        c.clean = Some(soundcraft_model::CleanAlt { source: cleaned, original, origin: 0, mix: 1.0, bypass: false });
        e.execute("clip.transpose", &json!({"clips": [cid.0], "semitones": 7})).unwrap();
        let job = super::next_pitch_job(&e).expect("a Clean Vocal clip gets a render too");
        assert!(super::install_pitch(&mut e, &job, job.run()));
        let hq = out(&e, t, 96_000);
        let want = 660.0 * 2f64.powf(7.0 / 12.0);
        let got = peak_hz(&hq[20_000..80_000], want - 40.0, want + 40.0);
        assert!((got / want - 1.0).abs() < 0.005, "the cleaned audio, transposed: {got} Hz for {want}");
        // A new Clean Vocal mix is heard live again until its own render.
        e.session_mut().find_clip_mut(cid).unwrap().clean.as_mut().unwrap().mix = 0.5;
        assert!(soundcraft_mix::pitch_render(e.session(), e.session().find_clip(cid).unwrap().1).is_none());
        assert!(super::next_pitch_job(&e).is_some());
    }

    /// Clip › Real-Time › Transpose: the live shifter is heard at once; the background render,
    /// by Pitch Shift's algorithm, replaces it at the asked pitch and the clip's length, without
    /// an undo step, and stays across undo and redo. Untransposed clips are untouched.
    #[test]
    fn a_transpose_gets_a_high_quality_render_in_the_background() {
        let (mut e, t, cid, plain) = tone(true);
        e.session_mut().track_mut(t).unwrap().elastic = None;
        let plain_before = out(&e, plain, 60_000);
        e.execute("clip.transpose", &json!({"clips": [cid.0], "semitones": 7})).unwrap();
        let live = out(&e, t, 96_000);
        assert!(super::next_pitch_job(&e).is_some());
        let job = super::next_pitch_job(&e).unwrap();
        let undo_steps = e.undo_history().len();
        assert!(super::install_pitch(&mut e, &job, job.run()));
        assert_eq!(e.undo_history().len(), undo_steps, "no undo step");
        assert!(super::next_pitch_job(&e).is_none(), "nothing left to render");
        let hq = out(&e, t, 96_000);
        assert_ne!(hq, live, "the render replaced the live shifter");
        assert_eq!(e.session().find_clip(cid).unwrap().1.length, 96_000, "the length is unchanged");
        let want = 440.0 * 2f64.powf(7.0 / 12.0);
        let got = peak_hz(&hq[20_000..80_000], want - 40.0, want + 40.0);
        assert!((got / want - 1.0).abs() < 0.005, "{got} Hz for {want}");
        // An untransposed clip: bit-identical.
        assert_eq!(out(&e, plain, 60_000), plain_before);
        // A new transpose is heard live again until its own render; undo brings the first
        // transpose back with its render still there.
        e.execute("clip.transpose", &json!({"clips": [cid.0], "semitones": -5})).unwrap();
        assert!(soundcraft_mix::pitch_render(e.session(), e.session().find_clip(cid).unwrap().1).is_none());
        assert!(e.undo());
        assert_eq!(out(&e, t, 96_000), hq, "the render made is kept across undo");
        assert!(e.undo());
        assert_eq!(e.session().find_clip(cid).unwrap().1.transpose_cents, 0, "one undo step for the transpose");
        assert_eq!(out(&e, t, 96_000), out(&tone(true).0, t, 96_000), "back to the plain clip");
    }
}

//! Gild in the mix (see `soundcraft_model::gild` and docs/gild.md).
//!
//! **Where it sits.** An audio track's strip runs, after its pre-fader inserts and before its fader
//! section (so before the pre-fader copy, the fader, mute, sends and pan), the **Gild return**:
//! - the Delay and Reverb throws run built-in module instances ([`GildRun`]), fed by the strip's
//!   signal *after* the inserts, gated to each gilding's span (with its fades), so a throw hears
//!   the vocal as the track's chain made it. Gildings whose throws have the very same settings
//!   share one run (the modules are linear: one run fed with both words is the same sound as two),
//!   so "the same throw on the last word of every line" costs one delay and one reverb;
//! - what a gilding's made-ahead render adds (Double, Shift layered, Harmony: the `layer`) joins
//!   that feed, so the echoes and the reverb hear it too; the swell is added on its own;
//! - every added signal is summed (each effect at its level), trimmed by the track's Gild level,
//!   and added to the strip's signal; with Dry off the strip's own signal is dropped first.
//!
//! The return adds no latency: the modules are zero-latency and the made-ahead parts are read at
//! the timeline position of the audio in the buffer (the inserts' latency and the track delay
//! taken into account). A run starts a moment before its first word (its settings settle on
//! silence; nothing it makes before the word is heard) and rings on past the span and past the
//! clip's end until it has been silent a while ([`GILD_TAIL_MAX_S`] at most), then rests at no
//! cost. A frozen track's render holds its Gild, so a frozen track does not run it again.
//!
//! **Threads.** Module instances are made when the session changes (`sync`), or by the host's
//! control thread for a realtime engine ([`crate::BuiltinPrep`]): rendering creates nothing,
//! and the strip's Gild buffers and run list are sized ahead (`sync_strip` for a strip made in
//! `sync`, `sized_strip` for one the host makes).

use crate::{Strip, db_to_gain, is_third_party};
use soundcraft_dsp::{Plugin, Transport};
use soundcraft_model::{Clip, GildEcho, GildSpace, GildVerb, Gilding, Session, Track, TrackId, TrackKind};
use soundcraft_time::Samples;
use std::collections::HashMap;

/// Most throw runs one track has (each a distinct set of throw settings); more are not heard.
pub const MAX_LIVE_GILDINGS: usize = 256;

/// The longest a throw rings after its last word, seconds.
pub const GILD_TAIL_MAX_S: f32 = 30.0;

/// The block size the modules are prepared for (as Clip Effects').
const PREP_BLOCK: usize = 16_384;

/// The modules start this long before a word (their settings settle on silence), seconds.
const PREROLL_S: f32 = 0.1;
/// Below this peak a throw's output is silent.
const SILENT: f32 = 1e-5;
/// A throw silent this long after its last word has rung out, seconds.
const RUNG_OUT_S: f32 = 0.1;

/// One set of throw settings' live modules on a strip, shared by every gilding with them.
pub(crate) struct GildRun {
    /// [`throw_key`] of the settings.
    pub(crate) key: u64,
    pub(crate) delay: Option<Box<dyn Plugin>>,
    pub(crate) reverb: Option<Box<dyn Plugin>>,
    /// Running: off, the modules are reset when it next starts, so nothing of an earlier pass
    /// rings into it.
    pub(crate) on: bool,
    pub(crate) ch: usize,
    /// Frames in a row its output has been silent with no word feeding it, and frames since a
    /// word last fed it.
    pub(crate) quiet: usize,
    pub(crate) since_fed: usize,
    /// Where (timeline) the word that started it starts: what it made before is not heard.
    pub(crate) heard_from: Samples,
}

/// Live modules made off the audio thread for one throw run ([`crate::BuiltinPrep`]).
pub struct PreparedGild {
    pub track: TrackId,
    /// [`throw_key`] of the run's settings.
    pub key: u64,
    pub ch: usize,
    pub delay: Option<Box<dyn Plugin>>,
    pub reverb: Option<Box<dyn Plugin>>,
}

impl PreparedGild {
    pub(crate) fn into_plugins(self) -> impl Iterator<Item = Box<dyn Plugin>> {
        self.delay.into_iter().chain(self.reverb)
    }
}

pub(crate) const DELAY_ID: &str = "fs_delay";
pub(crate) const REVERB_ID: &str = "fs_reverb";

/// A built-in module for a throw, prepared (allocates: never on the audio thread).
pub(crate) fn make(id: &str, sr: f32, ch: usize) -> Option<Box<dyn Plugin>> {
    if is_third_party(id) {
        return None;
    }
    soundcraft_dsp::create(id).map(|mut p| {
        p.prepare(sr, PREP_BLOCK, ch.max(1));
        p
    })
}

struct Fnv(u64);

impl Fnv {
    fn put(&mut self, v: u64) {
        for b in v.to_le_bytes() {
            self.0 ^= u64::from(b);
            self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
        }
    }
}

/// The settings that make a throw run what it is (its Delay and Reverb, levels included): equal
/// keys share one run. Never 0 for a gilding with a throw. No allocation.
pub(crate) fn throw_key(g: &Gilding) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    let f = |h: &mut Fnv, v: f32| h.put(u64::from(v.to_bits()));
    if let Some(e) = g.fx.delay {
        h.put(1);
        h.put(u64::from(e.sync));
        h.put(e.note as u64);
        for v in [e.ms, e.repeats, e.low_cut, e.high_cut, e.level_db] {
            f(&mut h, v);
        }
        h.put(u64::from(e.bounce));
    }
    if let Some(v) = g.fx.reverb {
        h.put(2);
        h.put(v.space as u64);
        for x in [v.size, v.decay_s, v.pre_ms, v.width, v.level_db] {
            f(&mut h, x);
        }
    }
    h.0.max(1)
}

/// The track's throw runs it needs, in play order, each once: (key, delay, reverb). At most
/// [`MAX_LIVE_GILDINGS`].
pub(crate) fn live_wants(t: &Track) -> impl Iterator<Item = (u64, bool, bool)> + '_ {
    let all = t
        .heard_clips()
        .iter()
        .filter(|c| c.is_audio() && !c.muted && t.kind == TrackKind::Audio)
        .flat_map(|c| c.gild.iter().filter(|g| g.fx.live()).map(move |g| (c, g)));
    all.clone()
        .enumerate()
        .filter(move |(i, (_, g))| {
            let k = throw_key(g);
            !all.clone().take(*i).any(|(_, x)| throw_key(x) == k)
        })
        .map(|(_, (_, g))| (throw_key(g), g.fx.delay.is_some(), g.fx.reverb.is_some()))
        .take(MAX_LIVE_GILDINGS)
}

/// Whether the track has anything for the Gild return to do (or its Dry is off).
pub(crate) fn track_uses(t: &Track) -> bool {
    t.kind == TrackKind::Audio && (!t.gild.dry || t.heard_clips().iter().any(|c| !c.gild.is_empty()))
}

/// Brings the strip's throw runs in line with the track (called by `sync`): runs no longer wanted
/// (or of another width) are retired, missing modules come from `host` (a realtime engine) or are
/// made here (counted in `made` when a host should have made them), and the buffers are sized.
pub(crate) fn sync_strip(
    strip: &mut Strip,
    t: &Track,
    sr: f32,
    mb: usize,
    retired: &mut Vec<Box<dyn Plugin>>,
    mut host: Option<&mut HashMap<(TrackId, u64), PreparedGild>>,
    made: &mut u64,
) {
    let ch = strip.buf.len();
    strip.gild.retain_mut(|r| {
        let want = live_wants(t).find(|w| w.0 == r.key);
        let keep = r.ch == ch && want.is_some();
        match want.filter(|_| keep) {
            Some((_, d, v)) => {
                if !d {
                    retired.extend(r.delay.take());
                }
                if !v {
                    retired.extend(r.reverb.take());
                }
            }
            None => retired.extend(r.delay.take().into_iter().chain(r.reverb.take())),
        }
        keep
    });
    let any = live_wants(t).next().is_some();
    if any && strip.gild.capacity() < MAX_LIVE_GILDINGS {
        // A strip made here rather than by the host (offline): its room, once.
        strip.gild.reserve_exact(MAX_LIVE_GILDINGS - strip.gild.len());
    }
    for (key, d, v) in live_wants(t) {
        let at = match strip.gild.iter().position(|r| r.key == key) {
            Some(i) => i,
            None => {
                if strip.gild.len() >= MAX_LIVE_GILDINGS {
                    continue;
                }
                // Room was reserved with the strip: no allocation on the render path.
                strip.gild.push(GildRun { key, delay: None, reverb: None, on: false, ch, quiet: 0, since_fed: 0, heard_from: 0 });
                strip.gild.len() - 1
            }
        };
        let Some(r) = strip.gild.get_mut(at) else { continue };
        if let Some(p) = host.as_deref_mut().and_then(|h| h.remove(&(t.id, key))) {
            if p.ch == ch {
                for (have, new) in [(&mut r.delay, p.delay), (&mut r.reverb, p.reverb)] {
                    match (have.is_none(), new) {
                        (true, n) => *have = n,
                        (false, Some(n)) => retired.push(n),
                        (false, None) => {}
                    }
                }
            } else {
                retired.extend(p.into_plugins());
            }
        }
        for (want, slot, pid) in [(d, &mut r.delay, DELAY_ID), (v, &mut r.reverb, REVERB_ID)] {
            if want && slot.is_none() {
                *slot = make(pid, sr, ch);
                if host.is_some() && slot.is_some() {
                    *made = made.saturating_add(1);
                }
                r.on = false;
            }
        }
    }
    let needs = any || track_uses(t);
    if needs {
        for b in [&mut strip.gild_feed, &mut strip.gild_fx, &mut strip.gild_wet] {
            if b.len() != ch || b.iter().any(|c| c.len() < mb) {
                *b = vec![vec![0.0; mb]; ch];
            }
        }
    }
}

/// `v` in the delay's 0..1 knob for a time of `ms` (20 ms .. 2 s, logarithmic).
fn delay_time_knob(ms: f32) -> f32 {
    ((ms.max(20.0) / 20.0).ln() / 100f32.ln()).clamp(0.0, 1.0)
}

/// A cut frequency in a 0..1 knob spanning `lo` .. `lo · span` logarithmically.
fn log_knob(hz: f32, lo: f32, span: f32) -> f32 {
    ((hz.max(lo) / lo).ln() / span.ln()).clamp(0.0, 1.0)
}

/// The delay's note value index for a [`soundcraft_model::GildNote`].
fn division_index(n: soundcraft_model::GildNote) -> f32 {
    use soundcraft_model::GildNote as N;
    match n {
        N::Sixteenth => 1.0,
        N::EighthTriplet => 2.0,
        N::Eighth => 4.0,
        N::DottedEighth => 6.0,
        N::QuarterTriplet => 5.0,
        N::Quarter => 7.0,
        N::DottedQuarter => 9.0,
        N::Half => 10.0,
        N::Whole => 13.0,
    }
}

/// The reverb's machine index for a space.
pub(crate) fn machine_index(s: GildSpace) -> f32 {
    match s {
        GildSpace::Room => 0.0,
        GildSpace::Hall => 1.0,
        GildSpace::Plate => 2.0,
        GildSpace::Cloud => 4.0,
    }
}

/// The delay module's settings for a throw: wet only, clean (Digital), at unity, synced to the
/// session's tempo or at a free time.
pub(crate) fn echo_params(e: &GildEcho, ch: usize) -> [(&'static str, f32); 24] {
    let routing = match (ch >= 2, e.bounce) {
        (false, _) => 0.0,
        (true, true) => 2.0,
        (true, false) => 1.0,
    };
    [
        ("mix", 1.0),
        ("style", 0.0),
        ("routing", routing),
        ("trim", 1.0 / 3.0),
        ("level", std::f32::consts::FRAC_1_SQRT_2),
        ("sync", if e.sync { 1.0 } else { 0.0 }),
        ("division", division_index(e.note)),
        ("time", delay_time_knob(e.ms)),
        ("feedback", (e.repeats / 1.15).clamp(0.0, 1.0)),
        ("lowCut", log_knob(e.low_cut, 20.0, 50.0)),
        ("highCut", log_knob(e.high_cut, 1000.0, 20.0)),
        ("character", 0.0),
        ("modDepth", 0.0),
        ("heads", 0.0),
        ("diffusion", 0.0),
        ("duck", 0.0),
        ("pitch", 0.0),
        ("freeze", 0.0),
        ("width", 0.5),
        ("trails", 1.0),
        ("tempoSrc", 0.0),
        ("threshold", 0.0),
        ("reverse", 0.0),
        ("pattern", 0.0),
    ]
}

/// The reverb module's settings for a throw (or a swell): wet only, at unity.
pub fn verb_params(v: &GildVerb) -> [(&'static str, f32); 12] {
    [
        ("mix", 1.0),
        ("machine", machine_index(v.space)),
        ("size", v.size.clamp(0.0, 1.0)),
        ("decay", ((v.decay_s.max(0.2) / 0.2).ln() / 150f32.ln()).clamp(0.0, 1.0)),
        ("preDelay", (v.pre_ms.clamp(0.0, 250.0) / 250.0).sqrt()),
        ("width", (v.width / 2.0).clamp(0.0, 1.0)),
        ("level", std::f32::consts::FRAC_1_SQRT_2),
        ("sync", 0.0),
        ("freeze", 0.0),
        ("duck", 0.0),
        ("swell", 0.0),
        ("trails", 1.0),
    ]
}

/// Sets each parameter that differs. No allocation.
pub(crate) fn apply(p: &mut dyn Plugin, params: &[(&'static str, f32)]) {
    for (k, v) in params {
        if p.param(k) != Some(*v) {
            p.set_param(k, *v);
        }
    }
}

/// The gain of a span `[a, b)` with fades of `fi` and `fo` samples at timeline position `t`.
fn gate(t: Samples, a: Samples, b: Samples, fi: f32, fo: f32) -> f32 {
    if t < a || t >= b {
        return 0.0;
    }
    let x = if fi >= 1.0 { ((t - a + 1) as f32 / fi).min(1.0) } else { 1.0 };
    let y = if fo >= 1.0 { ((b - t) as f32 / fo).min(1.0) } else { 1.0 };
    x.min(y)
}

fn add_scaled(dst: &mut [Vec<f32>], src: &[Vec<f32>], g: f32, frames: usize) {
    for (d, s) in dst.iter_mut().zip(src.iter()) {
        for (x, y) in d.iter_mut().zip(s.iter()).take(frames) {
            *x += *y * g;
        }
    }
}

fn clear(b: &mut [Vec<f32>], frames: usize) {
    for c in b.iter_mut() {
        c.iter_mut().take(frames).for_each(|x| *x = 0.0);
    }
}

/// Adds `audio` (source time of `clip`) for the block whose first sample is timeline position
/// `t0`, at `gain`, into `dst`. True when any of it falls in the block.
fn add_render(dst: &mut [Vec<f32>], audio: &soundcraft_model::GildAudio, clip: &Clip, t0: Samples, frames: usize, gain: f32) -> bool {
    let src0 = t0.saturating_sub(Gilding::timeline_of(clip, 0));
    if audio.end() <= src0 || audio.start >= src0.saturating_add(frames as i64) {
        return false;
    }
    for (c, d) in dst.iter_mut().enumerate() {
        for (k, x) in d.iter_mut().enumerate().take(frames) {
            if let Some(v) = audio.sample_at(c, src0.saturating_add(k as i64)) {
                *x += v * gain;
            }
        }
    }
    true
}

/// The Gild return of one block (see the module docs): `strip.buf` holds the track's signal after
/// its pre-fader inserts, whose first sample is timeline position `t0`. No allocation.
pub(crate) fn render_return(s: &Session, t: &Track, strip: &mut Strip, transport: &Transport, t0: Samples, frames: usize) {
    let ch = strip.buf.len();
    let sized = |b: &[Vec<f32>]| b.len() == ch && b.iter().all(|c| c.len() >= frames);
    if !(sized(&strip.gild_feed) && sized(&strip.gild_fx) && sized(&strip.gild_wet)) {
        if !t.gild.dry {
            clear(&mut strip.buf, frames);
        }
        return;
    }
    let rate = s.sample_rate.hz();
    let sr = s.sample_rate.as_f64() as f32;
    let ms = sr / 1000.0;
    let pre = (PREROLL_S * sr) as i64;
    let block_end = t0.saturating_add(frames as i64);
    clear(&mut strip.gild_wet, frames);
    let mut added = false;
    let Strip { buf, gild, gild_feed, gild_fx, gild_wet, .. } = strip;
    let clips = || t.heard_clips().iter().filter(|c| c.is_audio() && !c.muted);
    // What is added without the throws: swells, and the layers of gildings with no throw.
    for clip in clips() {
        for g in &clip.gild {
            let Some(render) = s.gild.current(clip, g, rate) else { continue };
            let gain = clip.gain_at(g.start.saturating_sub(clip.source_offset()).clamp(0, clip.length));
            for a in render.swell.iter().chain(render.layer.iter().filter(|_| !g.fx.live())) {
                added |= add_render(gild_wet, a, clip, t0, frames, gain);
            }
        }
    }
    // The throw runs: each fed by every gilding with its settings.
    for run in gild.iter_mut() {
        // Which of its gildings feed it this block (their spans, a moment before included).
        let mut fed_from = Samples::MAX;
        for clip in clips() {
            for g in clip.gild.iter().filter(|g| g.fx.live() && throw_key(g) == run.key) {
                if let Some((a, b)) = g.on_timeline(clip)
                    && block_end > a.saturating_sub(pre)
                    && t0 < b
                {
                    fed_from = fed_from.min(a);
                }
            }
        }
        let fed = fed_from != Samples::MAX;
        if !fed && !run.on {
            continue;
        }
        let Some(g0) = clips().flat_map(|c| c.gild.iter()).find(|g| g.fx.live() && throw_key(g) == run.key) else {
            run.on = false;
            continue;
        };
        let (delay, reverb) = (g0.fx.delay, g0.fx.reverb);
        if !run.on {
            // Settings first, then a clean start on them (a delay's time set before its reset
            // is already in place: no glide into the first echo).
            if let (Some(e), Some(p)) = (delay, run.delay.as_mut()) {
                apply(&mut **p, &echo_params(&e, ch));
                p.set_transport(transport);
            }
            if let (Some(v), Some(p)) = (reverb, run.reverb.as_mut()) {
                apply(&mut **p, &verb_params(&v));
                p.set_transport(transport);
            }
            for p in run.delay.iter_mut().chain(run.reverb.iter_mut()) {
                p.reset();
            }
            run.on = true;
            run.quiet = 0;
            run.heard_from = fed_from;
        }
        // The feed: the track's signal gated to each span, and each layer.
        clear(gild_feed, frames);
        if fed {
            run.since_fed = 0;
            for clip in clips() {
                for g in clip.gild.iter().filter(|g| g.fx.live() && throw_key(g) == run.key) {
                    let Some((a, b)) = g.on_timeline(clip) else { continue };
                    if t0 < b && block_end > a {
                        let (fi, fo) = (g.fade_in_ms * ms, g.fade_out_ms * ms);
                        for (f, src) in gild_feed.iter_mut().zip(buf.iter()) {
                            for (k, (x, y)) in f.iter_mut().zip(src.iter()).take(frames).enumerate() {
                                *x += *y * gate(t0.saturating_add(k as i64), a, b, fi, fo);
                            }
                        }
                    }
                    if let Some(l) = s.gild.current(clip, g, rate).and_then(|r| r.layer.as_ref()) {
                        let gain = clip.gain_at(g.start.saturating_sub(clip.source_offset()).clamp(0, clip.length));
                        add_render(gild_feed, l, clip, t0, frames, gain);
                        add_render(gild_wet, l, clip, t0, frames, gain);
                    }
                }
            }
        } else {
            run.since_fed = run.since_fed.saturating_add(frames);
        }
        // Nothing it makes before the word that started it is heard.
        let mute_to = usize::try_from(run.heard_from.saturating_sub(t0)).unwrap_or(0).min(frames);
        let mut loud = 0.0f32;
        if let (Some(e), Some(p)) = (delay, run.delay.as_mut()) {
            apply(&mut **p, &echo_params(&e, ch));
            p.set_transport(transport);
            copy(gild_fx, gild_feed, frames);
            p.process(gild_fx, frames);
            silence_to(gild_fx, mute_to);
            loud = loud.max(peak(gild_fx, frames));
            let lv = db_to_gain(e.level_db);
            add_scaled(gild_wet, gild_fx, lv, frames);
            if run.reverb.is_some() {
                // The reverb hears the echoes too.
                add_scaled(gild_feed, gild_fx, lv, frames);
            }
        }
        if let (Some(v), Some(p)) = (reverb, run.reverb.as_mut()) {
            apply(&mut **p, &verb_params(&v));
            p.set_transport(transport);
            copy(gild_fx, gild_feed, frames);
            p.process(gild_fx, frames);
            silence_to(gild_fx, mute_to);
            loud = loud.max(peak(gild_fx, frames));
            add_scaled(gild_wet, gild_fx, db_to_gain(v.level_db), frames);
        }
        added = true;
        // Rung out: silent a while with no word feeding it (or past the longest tail), it rests
        // at no cost until a word feeds it again.
        if !fed && loud < SILENT {
            run.quiet = run.quiet.saturating_add(frames);
        } else {
            run.quiet = 0;
        }
        if !fed && (run.quiet as f32 >= RUNG_OUT_S * sr || run.since_fed as f32 >= GILD_TAIL_MAX_S * sr) {
            run.on = false;
        }
    }
    if !t.gild.dry {
        clear(buf, frames);
    }
    if added {
        add_scaled(buf, gild_wet, db_to_gain(t.gild.level_db), frames);
        // A module gone wild never reaches the mix.
        for c in buf.iter_mut() {
            c.iter_mut().take(frames).filter(|x| !x.is_finite()).for_each(|x| *x = 0.0);
        }
    }
}

fn copy(dst: &mut [Vec<f32>], src: &[Vec<f32>], frames: usize) {
    for (d, f) in dst.iter_mut().zip(src.iter()) {
        if let (Some(d), Some(f)) = (d.get_mut(..frames), f.get(..frames)) {
            d.copy_from_slice(f);
        }
    }
}

fn silence_to(b: &mut [Vec<f32>], n: usize) {
    for c in b.iter_mut() {
        c.iter_mut().take(n).for_each(|x| *x = 0.0);
    }
}

fn peak(b: &[Vec<f32>], frames: usize) -> f32 {
    b.iter().map(|c| c.iter().take(frames).fold(0.0f32, |m, x| m.max(x.abs()))).fold(0.0, f32::max)
}

/// Stops every run (a stop or a locate): each starts clean the next time a word feeds it.
pub(crate) fn reset(strip: &mut Strip) {
    for r in strip.gild.iter_mut() {
        if r.on {
            for p in r.delay.iter_mut().chain(r.reverb.iter_mut()) {
                p.reset();
            }
        }
        r.on = false;
        r.quiet = 0;
    }
}

/// The changed words (Shift in place, Grit) over the source range `[lo, hi)` of `clip`: up to
/// four renders to read in place of the clip's audio. No allocation.
pub(crate) fn changes<'a>(s: &'a Session, clip: &Clip, lo: Samples, hi: Samples) -> [Option<&'a soundcraft_model::GildAudio>; 4] {
    let mut out = [None; 4];
    if clip.gild.is_empty() || s.gild.renders.is_empty() {
        return out;
    }
    let rate = s.sample_rate.hz();
    let mut n = 0;
    for g in clip.gild.iter().filter(|g| g.fx.changes_word() && g.end > lo && g.start < hi) {
        if let Some(c) = s.gild.current(clip, g, rate).and_then(|r| r.change.as_ref())
            && c.start < hi
            && c.end() > lo
            && let Some(slot) = out.get_mut(n)
        {
            *slot = Some(c);
            n += 1;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn knobs_map_back_to_the_units() {
        use soundcraft_dsp::plugins::fs::delay::delay_engine_dsp as d;
        assert!((d::knob_to_ms(delay_time_knob(360.0)) - 360.0).abs() < 0.5);
        assert!((d::knob_to_low_cut_hz(log_knob(220.0, 20.0, 50.0)) as f32 - 220.0).abs() < 0.5);
        assert!((d::knob_to_high_cut_hz(log_knob(5200.0, 1000.0, 20.0)) as f32 - 5200.0).abs() < 1.0);
        let v = verb_params(&GildVerb { decay_s: 2.8, pre_ms: 35.0, ..GildVerb::default() });
        use soundcraft_dsp::plugins::fs::reverb::reverb_engine_dsp as r;
        let get = |k: &str| v.iter().find(|x| x.0 == k).map(|x| x.1).unwrap();
        assert!((r::knob_to_decay_sec(get("decay")) - 2.8).abs() < 0.01);
        assert!((r::knob_to_pre_ms(get("preDelay")) - 35.0).abs() < 0.01);
    }

    /// Settings applied once read back the same: a block re-sends nothing (a re-sent setting
    /// can make a module work out its tables again).
    #[test]
    fn settings_read_back_as_set() {
        for (id, params) in [
            (DELAY_ID, echo_params(&GildEcho { bounce: true, ..GildEcho::default() }, 2).to_vec()),
            (DELAY_ID, echo_params(&GildEcho { sync: false, ..GildEcho::default() }, 1).to_vec()),
            (REVERB_ID, verb_params(&GildVerb::default()).to_vec()),
        ] {
            let mut p = make(id, 48_000.0, 2).unwrap();
            apply(&mut *p, &params);
            for (k, v) in &params {
                assert_eq!(p.param(k), Some(*v), "{id}.{k}");
            }
        }
    }

    #[test]
    fn the_gate_fades_at_both_ends() {
        assert_eq!(gate(99, 100, 200, 10.0, 10.0), 0.0);
        assert!(gate(100, 100, 200, 10.0, 10.0) > 0.0);
        assert_eq!(gate(150, 100, 200, 10.0, 10.0), 1.0);
        assert!(gate(199, 100, 200, 10.0, 10.0) < 0.2);
        assert_eq!(gate(200, 100, 200, 10.0, 10.0), 0.0);
    }
}

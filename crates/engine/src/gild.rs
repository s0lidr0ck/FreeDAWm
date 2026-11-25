//! Gild's made-ahead effects (see `soundcraft_model::gild` and docs/gild.md): each gilding with
//! Shift, Grit, Double, Harmony or Swell-in gets a [`GildRender`] made off the audio thread from
//! the clip's audio as it plays before the track's inserts (the source with Clean Vocal and the
//! vocal tuning in it), kept in [`soundcraft_model::GildCache`] (never saved, rebuilt after a load)
//! and made again whenever its [`render_key`] changes (the audio, the span or the settings).
//!
//! - The host runs one job at a time in the background ([`next_job`], [`GildJob::run`],
//!   [`install`]); until a render is in, the gilding plays without its made-ahead part.
//! - A bounce, a stems export, a freeze and a commit make every missing render first
//!   ([`ready`]), so what they print always holds the whole of Gild.
//! - The words a gilding was made from carry it along when they are retimed ([`follow`]).
//!
//! How each part is made:
//! - **Change** (Shift in place, Grit): the word shifted by the Vocal Editor's formant-keeping
//!   shifter (its pitch tracked here), then saturated by the Sat module (its latency taken off)
//!   and blended, crossfaded into the clip's own audio over the gilding's fades.
//! - **Layer**: Double (the word shifted a few cents and delayed a few ms), Shift layered, and
//!   Harmony voices (each sung note moved along the song's key), gated to the span with its
//!   fades, at their levels.
//! - **Swell**: the (changed) word reversed, through the Reverb module wet only, reversed back,
//!   and placed so it ends where the word starts (moved by its offset), at most its length long,
//!   faded in.

use crate::Engine;
use soundcraft_model::gild::{GildAudio, render_key};
use soundcraft_model::{Clip, ClipId, GildRender, GildVerb, Gilding, GritFlavour, Session};
use soundcraft_time::Samples;
use std::collections::BTreeMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

/// Audio read around a span for the shifter's context, seconds.
const CONTEXT_SECS: f64 = 0.12;
/// Longest swell tail rendered, seconds.
const SWELL_MAX_SECS: f64 = 4.0;

/// Copied Gild settings (`gild.copy`): the effects and the fades.
#[derive(Debug, Clone, PartialEq)]
pub struct GildCopy {
    pub fx: soundcraft_model::GildFx,
    pub fade_in_ms: f32,
    pub fade_out_ms: f32,
}

/// A render to make: the gilding, its key, and the session it is made from.
pub struct GildJob {
    pub clip: ClipId,
    pub id: u32,
    pub key: u64,
    session: Arc<Session>,
}

/// A finished render.
pub struct GildDone {
    pub clip: ClipId,
    pub id: u32,
    pub render: GildRender,
}

/// Every gilding whose render is missing or stale (and has not failed): (clip, gilding, key).
pub fn missing(s: &Session) -> Vec<(ClipId, u32, u64)> {
    let rate = s.sample_rate.hz();
    let mut out = Vec::new();
    for t in &s.tracks {
        for c in t.playlists.iter().flat_map(|p| p.clips.iter()).filter(|c| c.is_audio()) {
            for g in &c.gild {
                let Some(key) = render_key(c, g, rate) else { continue };
                if s.gild.renders.get(&(c.id, g.id)).is_some_and(|r| r.key == key) || s.gild.failed.contains_key(&key) {
                    continue;
                }
                out.push((c.id, g.id, key));
            }
        }
    }
    out
}

/// The next render the host should make: the gilding nearest the playhead first.
pub fn next_job(e: &Engine) -> Option<GildJob> {
    let s = e.session();
    let at = s.edit.playhead;
    let mut m = missing(s);
    m.sort_by_key(|(c, id, _)| {
        s.find_clip(*c)
            .and_then(|(_, clip)| clip.gild.iter().find(|g| g.id == *id).map(|g| (Gilding::timeline_of(clip, g.start) - at).abs()))
            .unwrap_or(i64::MAX)
    });
    let (clip, id, key) = *m.first()?;
    Some(GildJob { clip, id, key, session: e.session_arc() })
}

impl GildJob {
    /// What the status bar says while it runs.
    pub fn label(&self) -> String {
        let s = &self.session;
        let track =
            s.tracks.iter().find(|t| t.playlists.iter().any(|p| p.clips.iter().any(|c| c.id == self.clip))).map_or("vocal", |t| t.name.as_str());
        format!("Gilding {track}…")
    }

    /// Makes the render (worker threads). `Err` says why it cannot be made.
    pub fn run(self, cancel: &AtomicBool) -> Result<GildDone, String> {
        let s = &self.session;
        let (_, clip) = s.find_clip(self.clip).ok_or("the clip is gone")?;
        let g = clip.gild.iter().find(|g| g.id == self.id).ok_or("the gilding is gone")?;
        let mut render = make(s, clip, g, cancel)?;
        render.key = self.key;
        Ok(GildDone { clip: self.clip, id: self.id, render })
    }
}

/// Installs a finished render (no undo step: derived data) while it still matches its gilding.
pub fn install(e: &mut Engine, d: GildDone) -> bool {
    let s = e.session();
    let rate = s.sample_rate.hz();
    let current = s.find_clip(d.clip).and_then(|(_, c)| c.gild.iter().find(|g| g.id == d.id).and_then(|g| render_key(c, g, rate)));
    if current != Some(d.render.key) {
        return false;
    }
    let mut renders = (*s.gild.renders).clone();
    renders.insert((d.clip, d.id), Arc::new(d.render));
    prune(s, &mut renders);
    e.session_mut().gild.renders = Arc::new(renders);
    true
}

/// Marks a render that cannot be made, so it is not offered again.
pub fn mark_failed(e: &mut Engine, key: u64, why: &str) {
    let mut f = (*e.session().gild.failed).clone();
    f.insert(key, why.to_string());
    e.session_mut().gild.failed = Arc::new(f);
}

/// Drops the renders no gilding has any more (other clips' and removed gildings').
fn prune(s: &Session, renders: &mut BTreeMap<(ClipId, u32), Arc<GildRender>>) {
    renders.retain(|(c, id), _| s.find_clip(*c).is_some_and(|(_, clip)| clip.gild.iter().any(|g| g.id == *id)));
}

/// Makes every missing render now, in place (tests, scripts, a bounce's session). Renders that
/// cannot be made are marked failed. Returns how many were made.
pub fn render_all(s: &mut Session) -> usize {
    let cancel = AtomicBool::new(false);
    let todo = missing(s);
    if todo.is_empty() {
        return 0;
    }
    let mut renders = (*s.gild.renders).clone();
    let mut failed = (*s.gild.failed).clone();
    let mut n = 0;
    for (cid, id, key) in todo {
        let Some((_, clip)) = s.find_clip(cid) else { continue };
        let Some(g) = clip.gild.iter().find(|g| g.id == id) else { continue };
        match make(s, clip, g, &cancel) {
            Ok(mut r) => {
                r.key = key;
                renders.insert((cid, id), Arc::new(r));
                n += 1;
            }
            Err(why) => {
                failed.insert(key, why);
            }
        }
    }
    prune(s, &mut renders);
    s.gild.renders = Arc::new(renders);
    s.gild.failed = Arc::new(failed);
    n
}

/// `s` with every Gild render it needs made (the same `Arc` when none is missing): what a bounce,
/// a stems export, a freeze or a commit renders from.
pub fn ready(s: &Arc<Session>) -> Arc<Session> {
    if missing(s).is_empty() {
        return Arc::clone(s);
    }
    let mut out = (**s).clone();
    render_all(&mut out);
    Arc::new(out)
}

/// The engine's session with every missing render made and installed (no undo step).
pub fn settle(e: &mut Engine) -> usize {
    if missing(e.session()).is_empty() {
        return 0;
    }
    let mut s = e.session().clone();
    let n = render_all(&mut s);
    let (renders, failed) = (s.gild.renders, s.gild.failed);
    let g = &mut e.session_mut().gild;
    g.renders = renders;
    g.failed = failed;
    n
}

/// Gildings follow their words (in the same undo step as the edit that moved them). True when
/// anything changed.
pub fn follow(s: &mut Session) -> bool {
    let mut changed = false;
    for t in &mut s.tracks {
        for p in &mut t.playlists {
            for c in p.clips.iter_mut().filter(|c| !c.gild.is_empty()) {
                changed |= soundcraft_model::gild::follow_words(c);
            }
        }
    }
    changed
}

/// Whether [`follow`] would change anything (read only: the engine calls it after every edit).
pub fn needs_follow(s: &Session) -> bool {
    s.tracks.iter().flat_map(|t| t.playlists.iter()).flat_map(|p| p.clips.iter()).filter(|c| !c.gild.is_empty()).any(|c| {
        let mut probe = c.clone();
        soundcraft_model::gild::follow_words(&mut probe)
    })
}

// ─── Making a render ──────────────────────────────────────────────────────────────────────────

fn secs(rate: f64, v: f64) -> Samples {
    (v * rate).round() as Samples
}

fn db(v: f32) -> f32 {
    10f32.powf(v / 20.0)
}

/// The gate of the span `[a, b)` (source positions) with fades `fi`, `fo` (samples) at `p`.
fn gate(p: Samples, a: Samples, b: Samples, fi: f64, fo: f64) -> f32 {
    if p < a || p >= b {
        return 0.0;
    }
    let x = if fi >= 1.0 { ((p - a + 1) as f64 / fi).min(1.0) } else { 1.0 };
    let y = if fo >= 1.0 { ((b - p) as f64 / fo).min(1.0) } else { 1.0 };
    x.min(y) as f32
}

/// The channels a render adds at the return carries: the track's width (a stereo source on a
/// mono track folded).
fn track_width(s: &Session, clip: &Clip) -> usize {
    s.tracks.iter().find(|t| t.playlists.iter().any(|p| p.clips.iter().any(|c| c.id == clip.id))).map_or(1, |t| t.channels().clamp(1, 2))
}

fn fold(chans: &[Vec<f32>], width: usize) -> Vec<Vec<f32>> {
    if chans.len() <= width || chans.is_empty() {
        return chans.to_vec();
    }
    let n = chans.iter().map(Vec::len).min().unwrap_or(0);
    let k = chans.len() as f32;
    let mono: Vec<f32> = (0..n).map(|i| chans.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / k).collect();
    vec![mono; width]
}

/// The word pitch-shifted by `cents` (a constant, or per frame from `per_frame`) and its formant
/// by `formant` semitones, by the Vocal Editor's shifter, every channel alike. Unvoiced sound
/// (consonants, breaths) stays as it is.
fn shifted(chans: &[Vec<f32>], rate: u32, cents: &dyn Fn(Option<f32>) -> f32, formant: f32, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    let n = chans.iter().map(Vec::len).min().unwrap_or(0);
    if n == 0 {
        return Ok(chans.to_vec());
    }
    let mono: Vec<f32> = (0..n).map(|i| chans.iter().map(|c| c.get(i).copied().unwrap_or(0.0)).sum::<f32>() / chans.len() as f32).collect();
    let track = soundcraft_vocal::track_pitch(&mono, rate, cancel).map_err(|e| e.to_string())?;
    let nf = track.frames.len();
    let curves = soundcraft_vocal::EditCurves { shift_cents: track.frames.iter().map(|f| cents(f.cents)).collect(), gain_db: vec![0.0; nf] };
    let formant: Vec<f32> = track.frames.iter().map(|f| if f.cents.is_some() { formant } else { 0.0 }).collect();
    #[allow(clippy::single_range_in_vec_init)]
    let frames = [0..u32::try_from(nf).unwrap_or(u32::MAX)];
    let mut out = Vec::with_capacity(chans.len());
    for c in chans {
        let spans = soundcraft_vocal::resynth::render_spans_formant(c, &track, &curves, &formant, &frames, cancel).map_err(|e| e.to_string())?;
        let mut y = c.clone();
        soundcraft_vocal::resynth::splice(&mut y, &spans);
        out.push(y);
    }
    Ok(out)
}

/// `chans` through a built-in module (wet as it sets it), its latency taken off, `extra` samples
/// of tail rendered after.
fn through(id: &str, params: &[(&str, f32)], chans: &[Vec<f32>], rate: f32, extra: usize, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    let ch = chans.len().max(1);
    let mut p = soundcraft_dsp::create(id).ok_or_else(|| format!("{id} is not available"))?;
    p.prepare(rate, 1024, ch);
    for (k, v) in params {
        p.set_param(k, *v);
    }
    p.reset();
    let n = chans.iter().map(Vec::len).min().unwrap_or(0);
    // A first block pushes the settings (some modules take them at their first block).
    let mut warm = vec![vec![0.0f32; 64]; ch];
    p.process(&mut warm, 64);
    p.reset();
    let lat = p.latency();
    let total = n + extra + lat;
    let mut out = vec![Vec::with_capacity(total); ch];
    let mut block = vec![vec![0.0f32; 1024]; ch];
    let mut at = 0;
    while at < total {
        if cancel.load(Ordering::Relaxed) {
            return Err("cancelled".into());
        }
        let len = (total - at).min(1024);
        for (k, b) in block.iter_mut().enumerate() {
            for (i, x) in b.iter_mut().enumerate().take(len) {
                *x = chans.get(k).and_then(|c| c.get(at + i)).copied().unwrap_or(0.0);
            }
        }
        p.process(&mut block, len);
        for (o, b) in out.iter_mut().zip(&block) {
            o.extend(b.iter().take(len).map(|x| if x.is_finite() { *x } else { 0.0 }));
        }
        at += len;
    }
    for o in &mut out {
        o.drain(..lat.min(o.len()));
    }
    Ok(out)
}

fn grit(chans: &[Vec<f32>], g: &soundcraft_model::GildGrit, rate: f32, cancel: &AtomicBool) -> Result<Vec<Vec<f32>>, String> {
    let circuit = match g.flavour {
        GritFlavour::Warm => 1.0,
        GritFlavour::Bright => 2.0,
        GritFlavour::Hard => 4.0,
    };
    let params = [("circuit", circuit), ("drive", g.drive.clamp(0.0, 1.0)), ("mix", 1.0), ("autoGain", 1.0), ("output", 0.5), ("quality", 1.0)];
    let wet = through("fs_sat", &params, chans, rate, 0, cancel)?;
    let (b, lv) = (g.blend.clamp(0.0, 1.0), db(g.level_db));
    Ok(chans.iter().zip(&wet).map(|(x, w)| x.iter().zip(w.iter()).map(|(a, y)| (a * (1.0 - b) + y * b) * lv).collect()).collect())
}

/// The scale step `steps` from MIDI note `m` along `scale` (pitch classes in order).
fn scale_step(m: i32, steps: i32, scale: &[i32]) -> i32 {
    let n = scale.len() as i32;
    if n == 0 {
        return m + steps;
    }
    let (oct, pc) = (m.div_euclid(12), m.rem_euclid(12));
    let (idx, base) = match scale.iter().rposition(|p| *p <= pc) {
        Some(i) => (i as i32, oct),
        None => (n - 1, oct - 1),
    };
    let t = idx + steps;
    let pc = scale.get(t.rem_euclid(n) as usize).copied().unwrap_or(0);
    (base + t.div_euclid(n)) * 12 + pc
}

/// Makes `g`'s render from `clip`'s audio.
pub fn make(s: &Session, clip: &Clip, g: &Gilding, cancel: &AtomicBool) -> Result<GildRender, String> {
    let rate = s.sample_rate.as_f64();
    let hz = s.sample_rate.hz();
    let fx = &g.fx;
    let pad = secs(rate, CONTEXT_SECS);
    let lo = (g.start - pad).max(0);
    let hi = g.end + pad + fx.double.map_or(0, |d| secs(rate, f64::from(d.ms) / 1000.0));
    let n = usize::try_from(hi - lo).map_err(|_| "empty span")?;
    let dry = soundcraft_mix::clip_source_window(s, clip, lo, n).ok_or("the clip's audio is missing")?;
    if dry.is_empty() {
        return Err("the clip's audio is missing".into());
    }
    let (fi, fo) = (f64::from(g.fade_in_ms) * rate / 1000.0, f64::from(g.fade_out_ms) * rate / 1000.0);
    let gate_at = |i: usize| gate(lo + i as Samples, g.start, g.end, fi, fo);
    // The changed word: Shift in place, then Grit.
    let mut word = dry.clone();
    if let Some(sh) = fx.shift.filter(|x| !x.layer) {
        let c = sh.semitones * 100.0;
        word = shifted(&word, hz, &|_| c, sh.formant, cancel)?;
    }
    if let Some(gr) = &fx.grit {
        word = grit(&word, gr, rate as f32, cancel)?;
    }
    let change = fx.changes_word().then(|| {
        let ch = word.iter().zip(&dry).map(|(w, d)| {
            Arc::new(
                w.iter()
                    .zip(d.iter())
                    .enumerate()
                    .map(|(i, (w, d))| {
                        let k = gate_at(i);
                        d * (1.0 - k) + w * k
                    })
                    .collect::<Vec<f32>>(),
            )
        });
        GildAudio { start: lo, channels: ch.collect() }
    });
    let width = track_width(s, clip);
    // What is added beside the word: Double, Shift layered, Harmony.
    let mut layer: Option<Vec<Vec<f32>>> = None;
    let mut add = |v: Vec<Vec<f32>>, delay: usize, gain: f32, pan: Option<f32>| {
        let v = fold(&v, width);
        let l = layer.get_or_insert_with(|| vec![vec![0.0; n]; width]);
        for (c, dst) in l.iter_mut().enumerate() {
            let src = v.get(c % v.len().max(1));
            let pg = match (pan, width) {
                (Some(p), 2) => {
                    let a = (p.clamp(-1.0, 1.0) + 1.0) * std::f32::consts::FRAC_PI_4;
                    (if c == 0 { a.cos() } else { a.sin() }) * std::f32::consts::SQRT_2
                }
                _ => 1.0,
            };
            for (i, x) in dst.iter_mut().enumerate() {
                let Some(j) = i.checked_sub(delay) else { continue };
                if let Some(y) = src.and_then(|s| s.get(j)) {
                    *x += y * gate_at(j) * gain * pg;
                }
            }
        }
    };
    if let Some(d) = fx.double {
        let c = d.cents;
        let v = shifted(&word, hz, &|_| c, 0.0, cancel)?;
        add(v, secs(rate, f64::from(d.ms) / 1000.0) as usize, db(d.level_db), Some(0.35));
    }
    if let Some(sh) = fx.shift.filter(|x| x.layer) {
        let c = sh.semitones * 100.0;
        let v = shifted(&word, hz, &|_| c, sh.formant, cancel)?;
        add(v, 0, db(sh.level_db), None);
    }
    if let Some(h) = &fx.harmony {
        let at = Gilding::timeline_of(clip, g.start);
        let (key, sc) = crate::vocal::key_pc_at(s, at);
        let set = crate::vocal::key_set(key, sc);
        let scale: Vec<i32> = (0..12).filter(|pc| set.contains(*pc as u8)).collect();
        for v in &h.voices {
            let steps = i32::from(v.steps);
            let per = |c: Option<f32>| match c {
                Some(cents) => {
                    let sung = (cents / 100.0).round() as i32;
                    ((scale_step(sung, steps, &scale) - sung) * 100) as f32
                }
                None => 0.0,
            };
            let out = shifted(&word, hz, &per, 0.0, cancel)?;
            add(out, 0, db(v.level_db), Some(v.pan));
        }
    }
    let layer = layer.map(|l| GildAudio { start: lo, channels: l.into_iter().map(Arc::new).collect() });
    // The swell: the word reversed, through the reverb, reversed back, ending where it starts.
    let swell = match fx.swell {
        Some(sw) => {
            let (a, b) = (usize::try_from(g.start - lo).unwrap_or(0), usize::try_from(g.end - lo).unwrap_or(0).min(n));
            let w: Vec<Vec<f32>> = fold(&word, width.max(2).min(word.len().max(1)))
                .iter()
                .map(|c| (a..b).rev().map(|i| c.get(i).copied().unwrap_or(0.0) * gate_at(i)).collect())
                .collect();
            let len = f64::from(sw.length_s).min(SWELL_MAX_SECS);
            let verb = GildVerb { space: sw.space, size: 0.8, decay_s: (len * 0.8) as f32, pre_ms: 0.0, width: 1.0, level_db: 0.0 };
            let tail = secs(rate, len) as usize;
            let out = through("fs_reverb", &soundcraft_mix::verb_params(&verb), &w, rate as f32, tail, cancel)?;
            // Reversed back: it rises and ends at the word's end; moved to end at its start.
            let keep = secs(rate, len) as usize;
            let fade_in = (keep / 4).max(1);
            let fade_out = secs(rate, 0.01).max(1) as usize;
            let lv = db(sw.level_db);
            let chans: Vec<Arc<Vec<f32>>> = fold(&out, width)
                .into_iter()
                .map(|c| {
                    let mut r: Vec<f32> = c.into_iter().rev().collect();
                    let cut = r.len().saturating_sub(keep);
                    r.drain(..cut);
                    let m = r.len();
                    for (i, x) in r.iter_mut().enumerate() {
                        let fin = ((i + 1) as f32 / fade_in as f32).min(1.0);
                        let fout = ((m - i) as f32 / fade_out as f32).min(1.0);
                        *x *= lv * fin * fin * fout;
                    }
                    Arc::new(r)
                })
                .collect();
            let m = chans.first().map_or(0, |c| c.len()) as Samples;
            let end = g.start + secs(rate, f64::from(sw.offset_ms) / 1000.0);
            Some(GildAudio { start: end - m, channels: chans })
        }
        None => None,
    };
    if cancel.load(Ordering::Relaxed) {
        return Err("cancelled".into());
    }
    Ok(GildRender { key: 0, change, layer, swell })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scale_steps_stay_in_key() {
        let c_major = [0, 2, 4, 5, 7, 9, 11];
        assert_eq!(scale_step(60, 2, &c_major), 64, "C up a third: E");
        assert_eq!(scale_step(62, 2, &c_major), 65, "D up a third: F (minor)");
        assert_eq!(scale_step(64, -2, &c_major), 60);
        assert_eq!(scale_step(71, 2, &c_major), 74, "B up a third: D above");
    }
}

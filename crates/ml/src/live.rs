//! The worker behind Mend (`soundcraft_dsp::plugins::mend`): one thread that runs the Smule
//! Renaissance model for every Mend instance in the process, a block at a time.
//!
//! Each instance sends its input through a `Link`. For every block of the stream
//! (`Plan::block`, about a second) the worker takes the block with the sound before it
//! (`Plan::past`) and after it (`Plan::look`), brings that span to 48 kHz, runs the same steps
//! as Clean Vocal on it ([`crate::srs`]: the 60 Hz high-pass, peak normalisation, the network in
//! one pass, the gain back) and answers with the block. The network was made to hear 129 frames
//! (5.5 s) each way; here it hears 40 back and 17 ahead, which measures within about 18 dB of
//! the whole-file result on sung vocals. Less look-ahead costs more than that quickly, and none
//! at all does not work.
//!
//! - A link in **Live** mode gets DeepFilterNet 3 instead ([`crate::denoise::Stream`]): every
//!   hop of 10 ms is answered as soon as the three hops after it are in, and each pass costs
//!   only the new hops (the graphs carry their state), a millisecond or two.
//! - One model for the whole process, loaded when the first link arrives and let go when the
//!   last has been gone a while. Instances take turns, so two cost twice the time, not twice the
//!   threads.
//! - A span that is silent is answered with silence without running the network.
//! - A stereo link whose sides are the same costs one pass.
//! - A model that is not installed is looked for again every few seconds, so installing it
//!   starts Mend without a restart.

use crate::denoise;
use crate::job::Ctx;
use crate::runtime::{Device, OnnxModel};
use crate::stft::Stft;
use crate::{MlError, Result, srs};
use soundcraft_dsp::plugins::mend::link::{HOP, LIVE_HOP, Link, MODEL_RATE, Mode, Plan, State};
use soundcraft_dsp::plugins::mend::take_new_links;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// A span quieter than this (peak, after the high-pass) is answered with silence.
const SILENCE: f32 = 1e-4;
/// How long the join between two blocks' answers is faded over, in seconds.
const JOIN_SECS: f32 = 0.01;
/// How many blocks the answers may fall behind the input before the stream is started again.
const BEHIND_BLOCKS: usize = 3;
/// How often a model that is not there is looked for again.
const RETRY: Duration = Duration::from_secs(3);
/// How long the model stays loaded with no link to serve.
const KEEP: Duration = Duration::from_secs(30);

static STARTED: AtomicBool = AtomicBool::new(false);

/// Starts the worker (once per process; later calls do nothing). It idles until a Mend instance
/// exists.
pub fn start() {
    if STARTED.swap(true, Ordering::SeqCst) {
        return;
    }
    soundcraft_dsp::plugins::mend::set_worker_present();
    let spawned = std::thread::Builder::new().name("mend".into()).spawn(|| serve(&mut Loader::default()));
    if let Err(e) = spawned {
        log::warn!("Mend's worker could not start: {e}");
        STARTED.store(false, Ordering::SeqCst);
    }
}

/// What cleans a span: mono, 48 kHz, the same length out, and the gain (linear) that brings the
/// result to the span's loudness. The gain is kept apart so that it can glide from one block's
/// to the next's: the network settles its own output level whatever the input's (halving the
/// input lowers its output by under 1 dB), so each pass has to be matched, and two passes never
/// match by quite the same amount.
pub trait Clean {
    fn clean(&mut self, span: &[f32]) -> Result<(Vec<f32>, f32)>;
}

/// The furthest the matching gain goes either way, in dB.
const MATCH_LIMIT_DB: f64 = 30.0;

/// The Renaissance network with Clean Vocal's steps around it.
struct Renaissance {
    onnx: OnnxModel,
    stft: Stft,
}

impl Clean for Renaissance {
    fn clean(&mut self, span: &[f32]) -> Result<(Vec<f32>, f32)> {
        // The reference clamps the high-passed sound to ±1: keep a hot input under that.
        let over = span.iter().fold(1.0f32, |m, v| m.max(v.abs()));
        let scaled: Vec<f32> = span.iter().map(|v| v / over).collect();
        let x = srs::highpass60(&scaled);
        let g = x.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        if g < SILENCE {
            return Ok((vec![0.0; span.len()], 1.0));
        }
        let x: Vec<f32> = x.iter().map(|v| v / g).collect();
        // One pass: the span is all the context there is.
        let frames = self.stft.frames(x.len()).max(1);
        let y = srs::restore_one(&mut self.onnx, &self.stft, &x, frames, 0, &Ctx::default())?;
        let y: Vec<f32> = y.iter().map(|v| if v.is_finite() { v * g * over } else { 0.0 }).collect();
        // Clean Vocal's loudness measure (the voiced parts, rumble left out), over this span.
        let gain = match crate::clean::gated_powers(std::slice::from_ref(&span.to_vec()), std::slice::from_ref(&y), MODEL_RATE) {
            Some((pi, po)) if po > -300.0 => 10f64.powf((pi - po).clamp(-MATCH_LIMIT_DB, MATCH_LIMIT_DB) / 20.0) as f32,
            _ => 1.0,
        };
        // Clean Vocal's ceiling too: the network also restores clipped peaks, so on a hot track
        // matched loudness can pass full scale. Only ever lowers the gain.
        let peak = y.iter().fold(0.0f32, |m, v| m.max(v.abs()));
        let ceiling = 10f32.powf(crate::clean::PEAK_CEILING_DBFS as f32 / 20.0);
        let gain = if peak * gain > ceiling { ceiling / peak } else { gain };
        Ok((y, if gain.is_finite() { gain } else { 1.0 }))
    }
}

/// Loads the model, and remembers when it last failed to.
#[derive(Default)]
struct Loader {
    model: Option<Renaissance>,
    tried: Option<Instant>,
    failed: Option<State>,
    /// DeepFilterNet's graphs (built in), for the links in Live mode.
    live: Option<denoise::Models>,
    live_tried: Option<Instant>,
}

impl Loader {
    /// The model, loading it if it is time to; `Err` is the state the links should show.
    /// The live model, loading it the first time.
    fn live(&mut self, link: &Link) -> std::result::Result<&mut denoise::Models, State> {
        if self.live.is_none() {
            if self.live_tried.is_some_and(|at| at.elapsed() < RETRY) {
                return Err(State::Failed);
            }
            link.set_state(State::Loading);
            self.live_tried = Some(Instant::now());
            match denoise::Models::open() {
                Ok(m) => self.live = Some(m),
                Err(e) => {
                    log::warn!("Mend (Live): {e}");
                    return Err(State::Failed);
                }
            }
        }
        self.live.as_mut().ok_or(State::Failed)
    }

    fn get(&mut self, link: &Link) -> std::result::Result<&mut Renaissance, State> {
        if self.model.is_none() {
            if let (Some(at), Some(state)) = (self.tried, self.failed)
                && at.elapsed() < RETRY
            {
                return Err(state);
            }
            link.set_state(State::Loading);
            self.tried = Some(Instant::now());
            match load() {
                Ok(m) => (self.model, self.failed) = (Some(m), None),
                Err(e) => {
                    let state = if matches!(e, MlError::NotInstalled(_)) { State::NoModel } else { State::Failed };
                    if self.failed != Some(state) {
                        log::warn!("Mend: {e}");
                    }
                    self.failed = Some(state);
                    return Err(state);
                }
            }
        }
        self.model.as_mut().ok_or(State::Failed)
    }
}

fn load() -> Result<Renaissance> {
    let model = crate::models::load(&crate::models::models_dir(), srs::ID)?;
    let card = &model.card;
    let n_fft = crate::drumsep::card_usize(card, "n_fft", 4096);
    let hop = crate::drumsep::card_usize(card, "hop", HOP);
    if hop != HOP || n_fft != 2 * HOP {
        return Err(MlError::Model(srs::ID.into(), "Mend needs the 4096 / 2048 model".into()));
    }
    let onnx = OnnxModel::open_model_steady(&model, Device::Auto, &crate::job::Cancel::new())?;
    Ok(Renaissance { onnx, stft: Stft::new(n_fft, hop) })
}

fn serve(loader: &mut Loader) {
    let mut links: Vec<Served> = Vec::new();
    let mut idle_since = Instant::now();
    loop {
        links.extend(take_new_links().into_iter().map(Served::new));
        links.retain(|s| !s.link.closed());
        if links.is_empty() {
            if loader.model.is_some() && idle_since.elapsed() > KEEP {
                loader.model = None;
            }
            std::thread::sleep(Duration::from_millis(20));
            continue;
        }
        idle_since = Instant::now();
        for s in &mut links {
            s.listen();
        }
        let mut worked = false;
        let mut any_live = false;
        for s in &mut links {
            let link = Arc::clone(&s.link);
            let live = s.mode == Mode::Live;
            any_live |= live;
            let did = if live {
                loader.live(&link).map(|m| {
                    link.set_state(State::Ready);
                    s.step_live(m)
                })
            } else {
                loader.get(&link).map(|m| {
                    link.set_state(State::Ready);
                    s.step(m)
                })
            };
            match did {
                Ok(Ok(did)) => worked |= did,
                Ok(Err(e)) => {
                    log::warn!("Mend: {e}");
                    // Start again with a fresh model after a pause.
                    if live {
                        (loader.live, loader.live_tried) = (None, Some(Instant::now()));
                    } else {
                        (loader.model, loader.tried, loader.failed) = (None, Some(Instant::now()), Some(State::Failed));
                    }
                    link.set_state(State::Failed);
                    s.forget();
                }
                Err(state) => {
                    link.set_state(state);
                    s.forget();
                }
            }
        }
        if !links.iter().any(|s| s.mode == Mode::Live) {
            loader.live = None;
        }
        if !worked {
            // A live link's answer is due within a hop.
            std::thread::sleep(Duration::from_millis(if any_live { 1 } else { 5 }));
        }
    }
}

/// One link being served.
pub struct Served {
    link: Arc<Link>,
    /// The stream being answered, the sent frame it began at, and sent frames received so far.
    stream: u32,
    from: u64,
    received: u64,
    /// This stream's frames, `[channel][frame]`, from stream frame `base` on.
    heard: Vec<Vec<f32>>,
    base: usize,
    /// The next block of the stream to answer.
    block: usize,
    /// Answer frames queued so far, on every stream.
    made: u64,
    /// An answer waiting for room in the queue (interleaved).
    waiting: Option<Vec<f32>>,
    /// What the last block's pass made of the start of this one, to fade from: `[channel]`.
    tail: Vec<Vec<f32>>,
    /// The last block's matching gain (`None`: this stream has no block yet).
    gain: Option<f32>,
    fresh: Vec<f32>,
    /// Frames of this stream were dropped: it cannot be answered, a new one has been asked for.
    lost: bool,
    /// The stream's mode, and in Live mode its de-noiser, whether its first hop (the lead-in) has
    /// been dropped, and whether both sides have carried the same sound all along.
    mode: Mode,
    live: Option<denoise::Stream>,
    led_in: bool,
    same: bool,
}

impl Served {
    pub fn new(link: Arc<Link>) -> Served {
        let ch = link.channels;
        Served {
            link,
            stream: 0,
            from: 0,
            received: 0,
            heard: vec![Vec::new(); ch],
            base: 0,
            block: 0,
            made: 0,
            waiting: None,
            tail: vec![Vec::new(); ch],
            gain: None,
            fresh: Vec::new(),
            lost: false,
            mode: Mode::Studio,
            live: None,
            led_in: false,
            same: true,
        }
    }

    fn begin(&mut self, stream: u32, from: u64) {
        (self.stream, self.from, self.block, self.base, self.lost) = (stream, from, 0, 0, false);
        for h in &mut self.heard {
            h.clear();
        }
        for t in &mut self.tail {
            t.clear();
        }
        self.gain = None;
        self.waiting = None;
        self.mode = self.link.mode();
        self.live = (self.mode == Mode::Live).then(|| denoise::Stream::new(self.link.channels));
        (self.led_in, self.same) = (false, true);
        self.link.answer_starts(stream, self.made);
    }

    /// Takes what the audio side has sent, keeping the frames of its current stream.
    pub fn listen(&mut self) {
        let ch = self.link.channels;
        self.fresh.clear();
        let n = self.link.receive(&mut self.fresh) as u64;
        let (stream, from) = self.link.stream();
        if stream != self.stream {
            self.begin(stream, from);
        }
        let first = self.received;
        self.received += n;
        for (i, frame) in self.fresh.chunks_exact(ch).enumerate() {
            if first + i as u64 >= self.from {
                if self.lost {
                    continue;
                }
                for (h, v) in self.heard.iter_mut().zip(frame) {
                    h.push(*v);
                }
            }
        }
    }

    /// Drops what has been heard without answering it (no model, or too far behind): the audio
    /// side plays its input. The rest of the stream cannot be answered either, so once the
    /// model is there [`Served::step`] asks for a new one.
    fn forget(&mut self) {
        for h in &mut self.heard {
            h.clear();
        }
        for t in &mut self.tail {
            t.clear();
        }
        self.gain = None;
        self.waiting = None;
        self.live = None;
        self.lost = true;
    }

    /// Queues an answer waiting for room; `None` when there was none, else whether it went.
    fn flush(&mut self) -> Option<bool> {
        let out = self.waiting.take()?;
        if !self.link.room_for_answer(out.len() / self.link.channels) {
            self.waiting = Some(out);
            return Some(false);
        }
        self.link.answer(&out);
        self.made += (out.len() / self.link.channels) as u64;
        Some(true)
    }

    /// Live mode: de-noises every hop that is in; whether it did any work.
    pub fn step_live(&mut self, models: &mut denoise::Models) -> Result<bool> {
        if let Some(went) = self.flush() {
            return Ok(went);
        }
        if self.lost {
            self.link.ask_restart();
            return Ok(false);
        }
        let ch = self.link.channels;
        let have = self.heard.first().map_or(0, Vec::len);
        let hops = have / LIVE_HOP;
        if hops == 0 {
            return Ok(false);
        }
        // Half a second behind in real time: the answers would all be late.
        if !self.link.offline() && have > self.link.plan.rate as usize / 2 {
            self.forget();
            self.link.ask_restart();
            return Ok(false);
        }
        let began = Instant::now();
        let n = hops * LIVE_HOP;
        if self.same && ch == 2 && !self.sides_match(self.base, self.base + n) {
            if self.led_in {
                // The second side's model has not heard the stream so far: start a new one.
                self.forget();
                self.link.ask_restart();
                return Ok(false);
            }
            self.same = false;
        }
        let Some(stream) = self.live.as_mut() else { return Ok(false) };
        for h in 0..hops {
            let hop: Vec<&[f32]> = self.heard.iter().map(|c| c.get(h * LIVE_HOP..(h + 1) * LIVE_HOP).unwrap_or_default()).collect();
            stream.push(&hop);
        }
        for h in &mut self.heard {
            h.drain(..n.min(h.len()));
        }
        self.base += n;
        let mut cleaned = stream.pull(models, self.same && ch == 2)?;
        let mut frames = cleaned.first().map_or(0, Vec::len);
        if !self.led_in && frames >= LIVE_HOP {
            // The first hop out is the synthesis's lead-in, not sound.
            for c in &mut cleaned {
                c.drain(..LIVE_HOP.min(c.len()));
            }
            frames -= LIVE_HOP;
            self.led_in = true;
        }
        if frames == 0 {
            return Ok(true);
        }
        let mut out = vec![0.0f32; frames * ch];
        for (i, frame) in out.chunks_exact_mut(ch).enumerate() {
            for (c, o) in frame.iter_mut().enumerate() {
                *o = cleaned.get(c).or(cleaned.first()).and_then(|x| x.get(i)).copied().unwrap_or(0.0);
            }
        }
        // The audio side may have moved on while the model ran.
        if self.link.stream().0 != self.stream {
            return Ok(true);
        }
        self.link.set_load(began.elapsed().as_secs_f32() / (n as f32 / self.link.plan.rate as f32));
        self.waiting = Some(out);
        self.flush();
        Ok(true)
    }

    /// Answers the next block if it can be; whether it did any work.
    pub fn step(&mut self, cleaner: &mut dyn Clean) -> Result<bool> {
        if let Some(went) = self.flush() {
            return Ok(went);
        }
        if self.lost {
            // Asked every time: the request is cheap and a new stream clears `lost`.
            self.link.ask_restart();
            return Ok(false);
        }
        let p = self.link.plan;
        let end = (self.block + 1) * p.block;
        let have = self.base + self.heard.first().map_or(0, Vec::len);
        if have < end + p.look {
            return Ok(false);
        }
        // More than a few blocks behind in real time: the answers would all be late. Start
        // again from now. (An offline render waits for its answers, so it is never behind.)
        if !self.link.offline() && have > end + p.look + BEHIND_BLOCKS * p.block {
            self.forget();
            self.link.ask_restart();
            return Ok(false);
        }
        let began = Instant::now();
        let start = end - p.block;
        let ch = self.link.channels;
        let same = ch == 2 && self.sides_match(start.saturating_sub(p.past), end + p.look);
        // Every side that needs its own pass, then one gain for all (so the image holds).
        let mut cleaned: Vec<(Vec<f32>, Vec<f32>)> = Vec::with_capacity(ch);
        let mut gain_db = 0.0f32;
        for c in 0..if same { 1 } else { ch } {
            let span = self.span(c, start as i64 - p.past as i64, p.past + p.block + p.look);
            let (block, next, gain) = clean_block(cleaner, &span, &p)?;
            gain_db += 20.0 * gain.max(1e-6).log10();
            cleaned.push((block, next));
        }
        let gain = 10f32.powf(gain_db / cleaned.len().max(1) as f32 / 20.0);
        // The gain glides from the last block's to this one's over the block.
        let from = self.gain.unwrap_or(gain);
        self.gain = Some(gain);
        let mut out = vec![0.0f32; p.block * ch];
        for c in 0..ch {
            let Some((block, next)) = cleaned.get(c).or(cleaned.first()) else { continue };
            let tail = self.tail.get(c).map_or(&[][..], Vec::as_slice);
            let join = tail.len().min(p.block);
            for (i, new) in block.iter().enumerate().take(p.block) {
                let new = new * (from + (gain - from) * (i as f32 / p.block as f32));
                let v = match tail.get(i).filter(|_| i < join) {
                    Some(old) => old + (new - old) * ((i as f32 + 0.5) / join as f32),
                    None => new,
                };
                if let Some(o) = out.get_mut(i * ch + c) {
                    *o = v;
                }
            }
            let next: Vec<f32> = next.iter().map(|v| v * gain).collect();
            if let Some(t) = self.tail.get_mut(c) {
                *t = next;
            }
        }
        // The audio side may have moved on while the model ran.
        if self.link.stream().0 != self.stream {
            return Ok(true);
        }
        self.block += 1;
        // Keep only what the next block's span reaches back to.
        let keep_from = (end.saturating_sub(p.past)).max(self.base);
        for h in &mut self.heard {
            h.drain(..(keep_from - self.base).min(h.len()));
        }
        self.base = keep_from;
        self.link.set_load(began.elapsed().as_secs_f32() / (p.block as f32 / p.rate as f32));
        self.waiting = Some(out);
        self.flush();
        Ok(true)
    }

    /// `len` frames of channel `c` from stream frame `from` (silence before the stream began).
    fn span(&self, c: usize, from: i64, len: usize) -> Vec<f32> {
        let h = self.heard.get(c).map_or(&[][..], Vec::as_slice);
        (0..len as i64)
            .map(|i| {
                let at = from + i - self.base as i64;
                usize::try_from(at).ok().and_then(|k| h.get(k)).copied().unwrap_or(0.0)
            })
            .collect()
    }

    /// Whether both sides carry the same sound from stream frame `from` to `to`.
    fn sides_match(&self, from: usize, to: usize) -> bool {
        let (Some(l), Some(r)) = (self.heard.first(), self.heard.get(1)) else { return false };
        let a = from.saturating_sub(self.base);
        let b = to.saturating_sub(self.base).min(l.len()).min(r.len());
        l.get(a..b).zip(r.get(a..b)).is_some_and(|(l, r)| l.iter().zip(r).all(|(x, y)| (x - y).abs() <= 1e-7))
    }
}

/// Cleans one block: `span` is `past + block + look` frames at the session rate. Returns the
/// block, what the same pass made of the start of the next (for the join), and the pass's
/// matching gain (not applied).
fn clean_block(cleaner: &mut dyn Clean, span: &[f32], p: &Plan) -> Result<(Vec<f32>, Vec<f32>, f32)> {
    let join = ((p.rate as f32 * JOIN_SECS) as usize).min(p.look);
    if p.rate == MODEL_RATE {
        let (y, gain) = cleaner.clean(span)?;
        let at = p.past;
        let block = y.get(at..at + p.block).map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; p.block]);
        let next = y.get(at + p.block..at + p.block + join).map(<[f32]>::to_vec).unwrap_or_default();
        return Ok((block, next, gain));
    }
    let total = (p.past_hops + p.block_hops + p.look_hops) * HOP;
    let up = soundcraft_dsp::offline::resample(&[span.to_vec()], p.rate, MODEL_RATE).into_iter().next().unwrap_or_default();
    let mut x = up;
    x.resize(total, 0.0);
    let (y, gain) = cleaner.clean(&x)?;
    // Back to the session rate with the look-ahead as a margin after the block and as much
    // before it, so the converter's ends are well away from what is kept.
    let margin_hops = p.look_hops.min(p.past_hops);
    let margin = p.look.min(p.past);
    let a = (p.past_hops - margin_hops) * HOP;
    let b = (p.past_hops + p.block_hops + margin_hops) * HOP;
    let piece = y.get(a..b).map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; b - a]);
    let down = soundcraft_dsp::offline::resample(&[piece], MODEL_RATE, p.rate).into_iter().next().unwrap_or_default();
    let block = down.get(margin..margin + p.block).map(<[f32]>::to_vec).unwrap_or_else(|| vec![0.0; p.block]);
    let next = down.get(margin + p.block..margin + p.block + join).map(<[f32]>::to_vec).unwrap_or_default();
    Ok((block, next, gain))
}

#[cfg(test)]
mod tests {
    use super::*;
    use soundcraft_dsp::Plugin;
    use soundcraft_dsp::plugins::mend::Mend;

    /// Stands in for the network: half level, and counts its passes.
    #[derive(Default)]
    struct Half {
        passes: usize,
        lens: Vec<usize>,
    }

    impl Clean for Half {
        fn clean(&mut self, span: &[f32]) -> Result<(Vec<f32>, f32)> {
            self.passes += 1;
            self.lens.push(span.len());
            Ok((span.iter().map(|v| v * 0.25).collect(), 2.0))
        }
    }

    fn tone(n: usize, rate: f32) -> Vec<f32> {
        (0..n).map(|i| (i as f32 * 2.0 * std::f32::consts::PI * 220.0 / rate).sin() * 0.5).collect()
    }

    /// Plays a tone through a Mend at `rate` served by a `Half`; (input, left out, right out,
    /// latency, passes).
    fn run(rate: f32, stereo_differs: bool, total_after: usize) -> (Vec<f32>, Vec<f32>, Vec<f32>, usize, Half) {
        let mut m = Mend::new();
        m.prepare(rate, 512, 2);
        let lat = m.latency();
        let mut served = Served::new(Arc::clone(m.link().unwrap()));
        let mut half = Half::default();
        let total = lat + total_after;
        let x = tone(total, rate);
        let (mut l, mut r) = (Vec::new(), Vec::new());
        for b in x.chunks(512) {
            let right: Vec<f32> = if stereo_differs { b.iter().map(|v| -0.5 * v).collect() } else { b.to_vec() };
            let mut io = vec![b.to_vec(), right];
            m.process(&mut io, b.len());
            l.extend_from_slice(&io[0]);
            r.extend_from_slice(&io[1]);
            served.link.set_state(State::Ready);
            served.listen();
            served.step(&mut half).unwrap();
        }
        (x, l, r, lat, half)
    }

    #[test]
    fn blocks_come_back_in_time_at_the_models_rate() {
        let (x, l, r, lat, half) = run(48_000.0, false, 200_000);
        for i in 4_000..200_000 {
            assert!((l[lat + i] - 0.5 * x[i]).abs() < 1e-4, "frame {i}: {} against {}", l[lat + i], 0.5 * x[i]);
            assert_eq!(l[lat + i], r[lat + i]);
        }
        // The same sound on both sides: one pass a block, each over past + block + look.
        assert!(half.passes >= 4 && half.passes <= 7, "{}", half.passes);
        assert!(half.lens.iter().all(|n| *n == (40 + 25 + 17) * HOP));
    }

    #[test]
    fn two_different_sides_each_get_their_own_pass() {
        let (x, l, r, lat, half) = run(48_000.0, true, 120_000);
        for i in 4_000..120_000 {
            assert!((l[lat + i] - 0.5 * x[i]).abs() < 1e-4, "left {i}");
            assert!((r[lat + i] + 0.25 * x[i]).abs() < 1e-4, "right {i}");
        }
        assert!(half.passes % 2 == 0 && half.passes >= 6, "{}", half.passes);
    }

    #[test]
    fn another_rate_goes_through_the_models_and_back_in_time() {
        let (x, l, _, lat, half) = run(44_100.0, false, 150_000);
        assert!(half.lens.iter().all(|n| *n == (40 + 25 + 20) * HOP), "{:?}", half.lens);
        let mut worst = 0.0f32;
        for i in 4_000..150_000 {
            worst = worst.max((l[lat + i] - 0.5 * x[i]).abs());
        }
        assert!(worst < 2e-3, "worst error {worst}");
    }

    #[test]
    fn silence_costs_no_pass_of_the_network() {
        // The real wrapper's own rule, without a network: a silent span never reaches it.
        let x = vec![0.0f32; 4096];
        let hp = srs::highpass60(&x);
        assert!(hp.iter().fold(0.0f32, |m, v| m.max(v.abs())) < SILENCE);
    }

    #[test]
    fn a_link_with_no_model_is_told_and_plays_its_input() {
        let mut m = Mend::new();
        m.prepare(48_000.0, 512, 2);
        let lat = m.latency();
        let mut served = Served::new(Arc::clone(m.link().unwrap()));
        let x = tone(lat + 30_000, 48_000.0);
        let mut l = Vec::new();
        for b in x.chunks(512) {
            let mut io = vec![b.to_vec(), b.to_vec()];
            m.process(&mut io, b.len());
            l.extend_from_slice(&io[0]);
            served.listen();
            served.link.set_state(State::NoModel);
            served.forget();
        }
        for i in 0..30_000 {
            assert!((l[lat + i] - x[i]).abs() < 1e-6, "frame {i}");
        }
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert_eq!((r[0], r[2]), (3.0, 0.0));
    }

    /// A voice-like tone with noise, and the tone alone.
    fn noisy_voice(n: usize, rate: f32) -> (Vec<f32>, Vec<f32>) {
        let mut seed = 7u32;
        let clean: Vec<f32> = (0..n)
            .map(|i| {
                let t = i as f32 / rate;
                let f = 220.0 * (1.0 + 0.01 * (2.0 * std::f32::consts::PI * 5.0 * t).sin());
                (1..6).map(|h| 0.15 / h as f32 * (2.0 * std::f32::consts::PI * f * h as f32 * t).sin()).sum::<f32>()
            })
            .collect();
        let noisy = clean
            .iter()
            .map(|c| {
                seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
                c + ((seed >> 8) as f32 / (1u32 << 24) as f32 - 0.5) * 0.08
            })
            .collect();
        (noisy, clean)
    }

    #[test]
    fn live_mode_answers_within_its_short_latency_with_the_de_noised_sound() {
        let mut m = Mend::new();
        assert!(m.set_param("mode", 1.0));
        m.prepare(48_000.0, 512, 2);
        let lat = m.latency();
        assert_eq!(lat, 2912, "61 ms at a block of 512");
        let mut served = Served::new(Arc::clone(m.link().unwrap()));
        let mut models = denoise::Models::open().unwrap();
        let (noisy, _) = noisy_voice(96_000 + lat, 48_000.0);
        let mut out = Vec::new();
        for b in noisy.chunks(512) {
            let mut io = vec![b.to_vec(), b.to_vec()];
            m.process(&mut io, b.len());
            assert_eq!(io[0], io[1]);
            out.extend_from_slice(&io[0]);
            served.link.set_state(State::Ready);
            served.listen();
            assert_eq!(served.mode, Mode::Live);
            served.step_live(&mut models).unwrap();
        }
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert_eq!((r[0], r[2], r[4]), (2.0, 1.0, 0.0), "answering, never late: {r:?}");
        // What comes out is the whole-clip de-noiser's result, one latency later.
        let whole = denoise::denoise_48k(std::slice::from_ref(&noisy), 100.0, &Ctx::default()).unwrap().remove(0);
        let (mut sig, mut diff) = (0.0f64, 0.0f64);
        for i in 24_000..90_000 {
            sig += f64::from(whole[i]).powi(2);
            diff += f64::from(whole[i] - out[lat + i]).powi(2);
        }
        let db = 10.0 * (sig / diff.max(1e-30)).log10();
        assert!(db > 50.0, "Live is {db:.1} dB from the whole-clip de-noiser");
    }

    #[test]
    fn changing_mode_changes_the_latency_and_starts_a_new_stream() {
        let mut m = Mend::new();
        m.prepare(48_000.0, 512, 2);
        let studio = m.latency();
        let mut served = Served::new(Arc::clone(m.link().unwrap()));
        let mut models = denoise::Models::open().unwrap();
        let mut half = Half::default();
        let (noisy, _) = noisy_voice(200_000, 48_000.0);
        let mut out = Vec::new();
        for (k, b) in noisy.chunks(512).enumerate() {
            if k == 40 {
                assert!(m.set_param("mode", 1.0));
                assert!(m.latency() < studio / 10);
            }
            let mut io = vec![b.to_vec(), b.to_vec()];
            m.process(&mut io, b.len());
            out.extend_from_slice(&io[0]);
            served.link.set_state(State::Ready);
            served.listen();
            if served.mode == Mode::Live {
                served.step_live(&mut models).unwrap()
            } else {
                served.step(&mut half).unwrap()
            };
        }
        assert_eq!(served.mode, Mode::Live);
        let lat = m.latency();
        // Well after the change the de-noised sound plays, in time with the input: the two
        // line up at the new latency, not a little either side of it.
        let corr =
            |lag: i64| -> f64 { (100_000..180_000).map(|i| f64::from(noisy[i]) * f64::from(out[(lat as i64 + i as i64 + lag) as usize])).sum() };
        assert!(corr(0) > 1.5 * corr(-20) && corr(0) > 1.5 * corr(20), "{} {} {}", corr(-20), corr(0), corr(20));
        let mut r = [f32::NAN; 8];
        m.readouts(&mut r);
        assert_eq!(r[2], 1.0);
    }

    #[test]
    fn live_mode_at_a_rate_it_cannot_run_passes_the_sound_through() {
        let mut m = Mend::new();
        assert!(m.set_param("mode", 1.0));
        m.prepare(96_000.0, 512, 2);
        assert_eq!(m.latency(), 0);
        let mut io = vec![vec![0.25f32; 512]; 2];
        m.process(&mut io, 512);
        assert!(io[0].iter().all(|v| *v == 0.25));
        assert!(m.set_param("mode", 0.0));
        assert!(m.latency() > 200_000, "Studio still runs there");
    }
}

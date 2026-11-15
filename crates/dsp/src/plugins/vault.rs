//! Vault: a convolution reverb. It plays a sound through the impulse response of a real place or
//! device (a hall, a plate, a spring, something captured with Imprint), loaded from a file.
//!
//! - The response arrives from the host as a [`ir::SpaceIr`] through [`Plugin::set_asset`] under
//!   [`IR_KEY`], made ready off the audio thread. Until one is loaded Vault plays a space of its
//!   own ([`ir::SpaceIr::built_in`], generated, made once per sample rate and shared).
//! - A mono response gives the same reverb on both sides; a stereo pair is fed the mono sum of
//!   the input and gives each side its own; a four-channel file is true stereo (left in to left
//!   and right out, right in to left and right out).
//! - Every control works while it plays, none makes the response again: Pre-delay (before the
//!   convolution), Decay (turns the response down along its length, [`conv`]), Low Cut and High
//!   Cut (on the reverb only), Width (the reverb's side against its mid), Mix and Output.
//! - A new response fades in: the reverb is faded out over a few milliseconds, the new response
//!   starts from silence, the reverb fades back. The response let go of comes back out of
//!   [`Plugin::take_released_asset`], so the audio thread never frees one.
//! - No latency. Nothing is allocated after `prepare`.
//!
//! `readouts`: `[0]` the response's length in seconds, `[1]` its channels, `[2]` 1 when it is a
//! loaded one (0: the built-in space), `[3]` the reverb's peak in dBFS over the last block.

// The loops run over channels by index: each indexes several per-channel buffers side by side.
#![allow(clippy::needless_range_loop)]

pub mod conv;
pub mod ir;

use crate::biquad::{Biquad, FilterType};
use crate::params::{Params, lin, log};
use crate::util::{DelayLine, Smoother, frames_in, ms_to_samples, sane_channels, sane_sr};
use crate::{Category, ParamInfo, Plugin, PluginInfo, SampleSource, Unit, db_to_gain, gain_to_db};
use conv::Convolver;
use ir::{SpaceIr, as_space};
use std::sync::{Arc, Mutex};

/// The asset key of the impulse response ([`Plugin::set_asset`]; the session model's too).
pub const IR_KEY: &str = "vaultIr";
/// The longest Pre-delay, in ms.
pub const MAX_PREDELAY_MS: f32 = 250.0;
/// How long the reverb takes to fade out and back in around a new response, in ms.
const SWAP_FADE_MS: f32 = 15.0;
/// Low Cut and High Cut at these ends are out of the path.
const LOW_CUT_OFF_HZ: f32 = 20.0;
const HIGH_CUT_OFF_HZ: f32 = 20_000.0;

const P_MIX: usize = 0;
const P_PREDELAY: usize = 1;
const P_DECAY: usize = 2;
const P_LOW_CUT: usize = 3;
const P_HIGH_CUT: usize = 4;
const P_WIDTH: usize = 5;
const P_OUT: usize = 6;

static VAULT_PARAMS: [ParamInfo; 7] = [
    lin("mix", "Mix", 0.0, 100.0, 30.0, Unit::Percent),
    lin("predelay", "Pre-delay", 0.0, MAX_PREDELAY_MS, 0.0, Unit::Ms),
    lin("decay", "Decay", 5.0, 100.0, 100.0, Unit::Percent),
    log("low_cut", "Low Cut", LOW_CUT_OFF_HZ, 1000.0, LOW_CUT_OFF_HZ, Unit::Hz),
    log("high_cut", "High Cut", 1000.0, HIGH_CUT_OFF_HZ, HIGH_CUT_OFF_HZ, Unit::Hz),
    lin("width", "Width", 0.0, 200.0, 100.0, Unit::Percent),
    lin("out", "Output", -24.0, 12.0, 0.0, Unit::Db),
];

pub static VAULT_INFO: PluginInfo = PluginInfo {
    id: "vault",
    name: "Vault",
    short_name: "Vault",
    category: Category::Reverb,
    params: &VAULT_PARAMS,
    is_instrument: false,
    audiosuite: false,
    sidechain: false,
    link: None,
    references: 0,
    midi_out: false,
};

/// The built-in space at each rate asked for so far, shared by every instance.
static BUILT_IN: Mutex<Vec<(u32, Arc<dyn SampleSource>)>> = Mutex::new(Vec::new());

/// The built-in space at `rate` (made the first time; `None` only if it could not be).
fn built_in(rate: f32) -> Option<Arc<dyn SampleSource>> {
    let key = rate.round() as u32;
    let mut made = BUILT_IN.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
    if let Some((_, ir)) = made.iter().find(|(r, _)| *r == key) {
        return Some(Arc::clone(ir));
    }
    let ir: Arc<dyn SampleSource> = Arc::new(SpaceIr::built_in(f64::from(rate)).ok()?);
    made.push((key, Arc::clone(&ir)));
    Some(ir)
}

/// Where the reverb's fade around a new response is.
#[derive(Debug, Clone, Copy, PartialEq)]
enum Swap {
    Steady,
    Out,
    In,
}

pub struct Vault {
    p: Params,
    sr: f32,
    ch: usize,
    conv: Option<Box<Convolver>>,
    /// The loaded response (`None`: the built-in space).
    loaded: Option<Arc<dyn SampleSource>>,
    fallback: Option<Arc<dyn SampleSource>>,
    /// A response waiting for the fade-out (`Some(None)`: back to the built-in space).
    pending: Option<Option<Arc<dyn SampleSource>>>,
    released: [Option<Arc<dyn SampleSource>>; 4],
    swap: Swap,
    fade: f32,
    decay_set: f32,
    delay: [DelayLine; 2],
    low: [Biquad; 2],
    high: [Biquad; 2],
    cuts: (f32, f32),
    mix: Smoother,
    width: Smoother,
    out: Smoother,
    input: [Vec<f32>; 2],
    wet: [Vec<f32>; 2],
    block: usize,
    peak: f32,
}

impl Default for Vault {
    fn default() -> Self {
        Self::new()
    }
}

impl Vault {
    pub fn new() -> Self {
        let flat = || Biquad::design(FilterType::HighPass, LOW_CUT_OFF_HZ, 0.707, 0.0, 48_000.0);
        Vault {
            p: Params::new(&VAULT_PARAMS),
            sr: 48_000.0,
            ch: 2,
            conv: None,
            loaded: None,
            fallback: None,
            pending: None,
            released: Default::default(),
            swap: Swap::Steady,
            fade: 1.0,
            decay_set: f32::NAN,
            delay: Default::default(),
            low: [flat(), flat()],
            high: [flat(), flat()],
            cuts: (f32::NAN, f32::NAN),
            mix: Smoother::new(0.3),
            width: Smoother::new(1.0),
            out: Smoother::new(1.0),
            input: Default::default(),
            wet: Default::default(),
            block: 0,
            peak: 0.0,
        }
    }

    fn update(&mut self) {
        self.mix.set(self.p.v(P_MIX) / 100.0);
        self.width.set(self.p.v(P_WIDTH) / 100.0);
        self.out.set(db_to_gain(self.p.v(P_OUT)));
    }

    /// The response in use: the loaded one if it is usable, else the built-in space.
    fn active(&self) -> Option<&Arc<dyn SampleSource>> {
        self.loaded.as_ref().filter(|a| as_space(a.as_ref()).is_some()).or(self.fallback.as_ref())
    }

    fn release(&mut self, ir: Arc<dyn SampleSource>) {
        if let Some(slot) = self.released.iter_mut().find(|s| s.is_none()) {
            *slot = Some(ir);
        }
        // (all four full means the host never collects them: the reference drops here)
    }

    /// Puts the waiting response in place and starts the convolution from silence.
    fn install(&mut self) {
        if let Some(next) = self.pending.take()
            && let Some(old) = std::mem::replace(&mut self.loaded, next)
        {
            self.release(old);
        }
        let active = self.active().cloned();
        if let (Some(conv), Some(ir)) = (self.conv.as_mut(), active.as_ref().and_then(|a| as_space(a.as_ref()))) {
            conv.configure(ir, self.ch);
        }
        self.decay_set = f32::NAN;
    }

    fn set_cuts(&mut self) {
        let cuts = (self.p.v(P_LOW_CUT), self.p.v(P_HIGH_CUT));
        if cuts != self.cuts {
            self.cuts = cuts;
            for f in &mut self.low {
                f.set(FilterType::HighPass, cuts.0, 0.707, 0.0, self.sr);
            }
            for f in &mut self.high {
                f.set(FilterType::LowPass, cuts.1.min(0.45 * self.sr), 0.707, 0.0, self.sr);
            }
        }
    }

    /// One stretch of at most `self.block` frames, starting at `at`.
    fn run(&mut self, io: &mut [Vec<f32>], at: usize, n: usize) {
        let ch = self.ch.min(io.len()).min(2);
        // A response waiting: fade the reverb out, swap at silence, fade back in.
        if self.pending.is_some() && self.swap == Swap::Steady {
            self.swap = Swap::Out;
        }
        if self.swap == Swap::Out && self.fade <= 0.0 {
            self.install();
            self.swap = Swap::In;
        }
        let Some(active) = self.active().cloned() else { return };
        let Some(ir) = as_space(active.as_ref()) else { return };
        let Some(conv) = self.conv.as_mut() else { return };
        let decay = self.p.v(P_DECAY) / 100.0;
        if decay != self.decay_set {
            conv.set_decay(ir.len(), decay);
            self.decay_set = decay;
        }
        // The convolution's input: the mono sum or the two sides, after the Pre-delay.
        let (nin, nout) = (conv.inputs(), conv.outputs());
        let pre = (ms_to_samples(self.p.v(P_PREDELAY), self.sr).round() as usize).min(self.delay[0].max_delay() as usize);
        let clean = |x: f32| if x.is_finite() { x } else { 0.0 };
        for i in 0..n {
            let l = clean(io[0][at + i]);
            let r = if ch > 1 { clean(io[1][at + i]) } else { l };
            let feed = if nin == 1 { [0.5 * (l + r), 0.0] } else { [l, r] };
            for c in 0..nin {
                self.delay[c].push(feed[c]);
                self.input[c][i] = self.delay[c].tap(pre + 1);
            }
        }
        conv.process(ir, &self.input, &mut self.wet, n);
        if nout == 1 {
            let (a, b) = self.wet.split_at_mut(1);
            b[0][..n].copy_from_slice(&a[0][..n]);
        }
        // The reverb's own filters.
        let (low_on, high_on) = (self.cuts.0 > LOW_CUT_OFF_HZ * 1.01, self.cuts.1 < HIGH_CUT_OFF_HZ * 0.99);
        for c in 0..2 {
            if low_on {
                self.low[c].process_block(&mut self.wet[c][..n]);
            }
            if high_on {
                self.high[c].process_block(&mut self.wet[c][..n]);
            }
        }
        let step = 1.0 / (SWAP_FADE_MS * 0.001 * self.sr).max(1.0);
        let mut peak = 0.0f32;
        for i in 0..n {
            self.fade = match self.swap {
                Swap::Out => (self.fade - step).max(0.0),
                Swap::In => (self.fade + step).min(1.0),
                Swap::Steady => 1.0,
            };
            let (mix, width, out) = (self.mix.next(), self.width.next(), self.out.next());
            let (wl, wr) = (self.wet[0][i], self.wet[1][i]);
            let (mid, side) = (0.5 * (wl + wr), 0.5 * (wl - wr) * width);
            let wet = [(mid + side) * self.fade, (mid - side) * self.fade];
            for (c, w) in wet.iter().enumerate().take(ch) {
                // A mono track hears the mid.
                let w = if ch == 1 { mid * self.fade } else { *w };
                peak = peak.max(w.abs());
                let dry = clean(io[c][at + i]);
                let y = (dry * (1.0 - mix) + w * mix) * out;
                io[c][at + i] = if y.is_finite() { y } else { 0.0 };
            }
        }
        if self.swap == Swap::In && self.fade >= 1.0 {
            self.swap = Swap::Steady;
        }
        self.peak = self.peak.max(peak);
    }
}

impl Plugin for Vault {
    crate::params::param_plumbing!(VAULT_INFO);

    fn prepare(&mut self, sample_rate: f32, max_block: usize, channels: usize) {
        let sr = sane_sr(sample_rate).min(768_000.0);
        let block = max_block.clamp(1, 1 << 16);
        if self.conv.is_none() || sr != self.sr || block != self.block {
            self.conv = Some(Box::new(Convolver::new(f64::from(sr))));
            for d in &mut self.delay {
                d.allocate(ms_to_samples(MAX_PREDELAY_MS, sr) as usize + 2);
            }
            self.input = [vec![0.0; block], vec![0.0; block]];
            self.wet = [vec![0.0; block], vec![0.0; block]];
            self.fallback = built_in(sr);
        }
        (self.sr, self.block, self.ch) = (sr, block, sane_channels(channels));
        for s in [&mut self.mix, &mut self.width, &mut self.out] {
            s.set_time(20.0, sr);
        }
        self.cuts = (f32::NAN, f32::NAN);
        self.update();
        self.reset();
    }

    fn reset(&mut self) {
        self.install();
        (self.swap, self.fade, self.peak) = (Swap::Steady, 1.0, 0.0);
        self.delay.iter_mut().for_each(DelayLine::clear);
        self.low.iter_mut().chain(&mut self.high).for_each(Biquad::reset);
        for s in [&mut self.mix, &mut self.width, &mut self.out] {
            s.snap();
        }
    }

    fn process(&mut self, io: &mut [Vec<f32>], frames: usize) {
        let ch = self.ch.min(io.len()).min(2);
        let frames = frames_in(io, frames, ch);
        if ch == 0 || self.block == 0 {
            return;
        }
        self.set_cuts();
        self.peak = 0.0;
        let mut at = 0;
        while at < frames {
            let n = (frames - at).min(self.block);
            self.run(io, at, n);
            at += n;
        }
    }

    fn tail_samples(&self) -> usize {
        let ir = self.active().map_or(0, |a| a.frames());
        ir + ms_to_samples(self.p.v(P_PREDELAY), self.sr) as usize
    }

    fn readouts(&self, out: &mut [f32; 8]) {
        let active = self.active();
        out[0] = active.map_or(0.0, |a| a.frames() as f32 / a.sample_rate().max(1.0));
        out[1] = active.map_or(0.0, |a| a.num_channels() as f32);
        out[2] = if self.loaded.as_ref().is_some_and(|a| as_space(a.as_ref()).is_some()) { 1.0 } else { 0.0 };
        out[3] = gain_to_db(self.peak);
    }

    fn set_asset(&mut self, key: &str, asset: Option<Arc<dyn SampleSource>>) -> bool {
        if key != IR_KEY {
            return false;
        }
        // The one waiting, if any, never played: let go of it.
        if let Some(Some(old)) = self.pending.replace(asset) {
            self.release(old);
        }
        true
    }

    fn take_released_asset(&mut self) -> Option<Arc<dyn SampleSource>> {
        self.released.iter_mut().find_map(Option::take)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SR: f32 = 48_000.0;
    const BLOCK: usize = 512;

    fn vault(set: &[(&str, f32)]) -> Vault {
        let mut v = Vault::new();
        for (id, x) in set {
            assert!(v.set_param(id, *x), "{id}");
        }
        v.prepare(SR, BLOCK, 2);
        v
    }

    fn asset(channels: &[Vec<f32>]) -> Arc<dyn SampleSource> {
        Arc::new(SpaceIr::new(channels, f64::from(SR), f64::from(SR)).unwrap())
    }

    fn run(v: &mut Vault, l: &[f32], r: &[f32]) -> (Vec<f32>, Vec<f32>) {
        let (mut ol, mut or) = (Vec::new(), Vec::new());
        for (a, b) in l.chunks(BLOCK).zip(r.chunks(BLOCK)) {
            let mut io = vec![a.to_vec(), b.to_vec()];
            v.process(&mut io, a.len());
            ol.extend_from_slice(&io[0]);
            or.extend_from_slice(&io[1]);
        }
        (ol, or)
    }

    fn click(len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        x[0] = 1.0;
        x
    }

    fn energy(x: &[f32]) -> f32 {
        x.iter().map(|v| v * v).sum()
    }

    fn readouts(v: &Vault) -> [f32; 8] {
        let mut out = [f32::NAN; 8];
        v.readouts(&mut out);
        out
    }

    #[test]
    fn it_plays_its_own_space_until_a_response_is_loaded() {
        let mut v = vault(&[("mix", 100.0)]);
        let x = click(96_000);
        let (l, r) = run(&mut v, &x, &x);
        assert!(energy(&l) > 0.01 && energy(&r) > 0.01, "a reverb out of the box");
        assert!(l != r, "a stereo one");
        assert!(energy(&l[90_000..]) < 1.0e-6 * energy(&l), "that dies away");
        let out = readouts(&v);
        assert!((1.0..=1.81).contains(&out[0]) && out[1] == 2.0 && out[2] == 0.0, "{out:?}");
        assert_eq!(v.latency(), 0);
        assert!(v.tail_samples() > 48_000);
    }

    #[test]
    fn a_loaded_response_is_what_a_click_gives_back() {
        let h: Vec<f32> = (0..3000).map(|i| ((i as f32) * 0.2).sin() * (-(i as f32) / 500.0).exp()).collect();
        let a = asset(&[h.clone(), h.iter().map(|x| -x).collect()]);
        let want = as_space(a.as_ref()).unwrap().channel(0).unwrap().to_vec();
        let mut v = vault(&[("mix", 100.0)]);
        assert!(v.set_asset(IR_KEY, Some(Arc::clone(&a))));
        assert!(!v.set_asset("irPathTwin", None), "only its own key");
        v.reset();
        let x = click(8192);
        let (l, r) = run(&mut v, &x, &x);
        let err = l.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
        assert!(err < 1.0e-5, "{err}");
        assert!(l.iter().zip(&r).all(|(a, b)| (a + b).abs() < 1.0e-5), "the other side its own response");
        let out = readouts(&v);
        assert!(out[2] == 1.0 && out[1] == 2.0, "{out:?}");
        // Mix: at 0 the input, untouched.
        let mut v = vault(&[("mix", 0.0)]);
        let x: Vec<f32> = (0..2048).map(|i| (i as f32 * 0.1).sin() * 0.5).collect();
        let (l, _) = run(&mut v, &x, &x);
        assert_eq!(l, x);
    }

    #[test]
    fn predelay_width_and_the_cuts_act_on_the_reverb() {
        let h: Vec<f32> = (0..2000).map(|i| if i == 0 { 1.0 } else { 0.0 }).collect();
        let a = asset(&[h.clone(), h.iter().map(|x| -x).collect()]);
        let gain = as_space(a.as_ref()).unwrap().channel(0).unwrap()[0];
        let x = click(4096);
        // Pre-delay: the click comes back 10 ms later.
        let mut v = vault(&[("mix", 100.0), ("predelay", 10.0)]);
        v.set_asset(IR_KEY, Some(Arc::clone(&a)));
        v.reset();
        let (l, _) = run(&mut v, &x, &x);
        let at = l.iter().position(|s| s.abs() > 0.5 * gain).unwrap();
        assert_eq!(at, 480);
        // Width 0: both sides the mid (here the two responses cancel).
        let mut v = vault(&[("mix", 100.0), ("width", 0.0)]);
        v.set_asset(IR_KEY, Some(Arc::clone(&a)));
        v.reset();
        let (l, r) = run(&mut v, &x, &x);
        assert!(energy(&l) < 1.0e-8 && energy(&r) < 1.0e-8);
        // High Cut takes the top off a click; Low Cut takes the bottom.
        let flat = {
            let mut v = vault(&[("mix", 100.0)]);
            v.set_asset(IR_KEY, Some(Arc::clone(&a)));
            v.reset();
            energy(&run(&mut v, &x, &x).0)
        };
        for (id, hz) in [("high_cut", 2000.0), ("low_cut", 800.0)] {
            let mut v = vault(&[("mix", 100.0), (id, hz)]);
            v.set_asset(IR_KEY, Some(Arc::clone(&a)));
            v.reset();
            let e = energy(&run(&mut v, &x, &x).0);
            assert!(e < 0.98 * flat && e > 0.01 * flat, "{id}: {e} of {flat}");
        }
    }

    #[test]
    fn decay_shortens_the_tail() {
        let x = click(96_000);
        let late = |decay: f32| {
            let mut v = vault(&[("mix", 100.0), ("decay", decay)]);
            let (l, _) = run(&mut v, &x, &x);
            (energy(&l[..12_000]), energy(&l[36_000..]))
        };
        let (full_early, full_late) = late(100.0);
        let (half_early, half_late) = late(50.0);
        assert!(half_late < 0.01 * full_late, "{half_late} against {full_late}");
        assert!(half_early > 0.5 * full_early, "the start is nearly as it was");
    }

    #[test]
    fn a_new_response_fades_in_and_the_old_one_is_handed_back() {
        let a = asset(&[vec![1.0, 0.0, 0.0, 0.5]]);
        let b = asset(&[vec![0.0, 0.0, 1.0]]);
        let mut v = vault(&[("mix", 100.0)]);
        v.set_asset(IR_KEY, Some(Arc::clone(&a)));
        v.reset();
        assert!(v.take_released_asset().is_none());
        let x = vec![0.25f32; 4096];
        run(&mut v, &x, &x);
        v.set_asset(IR_KEY, Some(Arc::clone(&b)));
        let (l, _) = run(&mut v, &x, &x);
        let jump = l.windows(2).map(|w| (w[1] - w[0]).abs()).fold(0.0f32, f32::max);
        assert!(jump < 0.02, "no click at the change: {jump}");
        assert!(l[4000] > 0.05, "the new response is playing");
        let back = v.take_released_asset().expect("the old response");
        assert!(std::ptr::addr_eq(Arc::as_ptr(&back), Arc::as_ptr(&a)));
        assert!(v.take_released_asset().is_none());
        // Cleared: the built-in space again, and the response comes back out.
        v.set_asset(IR_KEY, None);
        run(&mut v, &x, &x);
        assert!(std::ptr::addr_eq(Arc::as_ptr(&v.take_released_asset().unwrap()), Arc::as_ptr(&b)));
        assert_eq!(readouts(&v)[2], 0.0);
        // One replaced before it ever played comes back too.
        v.set_asset(IR_KEY, Some(Arc::clone(&a)));
        v.set_asset(IR_KEY, Some(Arc::clone(&b)));
        assert!(std::ptr::addr_eq(Arc::as_ptr(&v.take_released_asset().unwrap()), Arc::as_ptr(&a)));
    }

    #[test]
    fn mono_tracks_odd_blocks_and_hostile_input_stay_finite() {
        let mut v = Vault::new();
        v.prepare(SR, 64, 1);
        for n in [1usize, 63, 64, 500, 0] {
            let mut io = vec![vec![0.3f32; n]];
            v.process(&mut io, n);
            assert!(io[0].iter().all(|x| x.is_finite()));
        }
        let mut v = vault(&[("mix", 100.0), ("decay", f32::NAN), ("predelay", f32::INFINITY)]);
        let mut io = vec![vec![f32::NAN; BLOCK], vec![f32::INFINITY; BLOCK]];
        v.process(&mut io, BLOCK);
        assert!(io.iter().flatten().all(|x| x.is_finite()));
        // An asset that is not a prepared response is ignored: the built-in space plays on.
        let plain: Arc<dyn SampleSource> = Arc::new(crate::SampleBuffer { sample_rate: SR, channels: vec![vec![1.0; 8]] });
        v.set_asset(IR_KEY, Some(plain));
        v.set_param("predelay", 0.0);
        v.reset();
        let x = click(4096);
        assert!(energy(&run(&mut v, &x, &x).0) > 1.0e-4);
        assert_eq!(readouts(&v)[2], 0.0);
    }
}

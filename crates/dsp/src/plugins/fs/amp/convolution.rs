//! The cabinet convolution the C++ processor runs (`juce::dsp::Convolution`, zero latency,
//! uniform partitions): `ConvolutionEngine::processSamples` (overlap-add, zero latency) and the
//! `CrossoverMixer` that fades from the previous engine to a new one over 50 ms when the impulse
//! response changes. Translated from JUCE 8's `juce_Convolution.cpp`.
//!
//! Rust differences, none of them in the sound:
//! - The response's partition spectra come ready in the [`ImpulseAsset`] (made off the audio
//!   thread); only the running state (the input spectra, the overlap) lives here, sized in
//!   `prepare` for the longest response accepted.
//! - JUCE keeps one `Convolution` per amp, each with its own history; this keeps the two the
//!   crossfade needs and starts an amp's convolution clean when the amp changes (where the C++
//!   would resume that amp's convolution from its stale history).
//! - The FFT is `rustfft`'s complex transform on the real block (JUCE's real-only transform):
//!   the same convolution to float rounding.

use super::ir::{BINS, CONV_BLOCK, FFT_SIZE, ImpulseAsset, max_segments};
use rustfft::num_complex::Complex;
use rustfft::{Fft, FftPlanner};
use std::sync::Arc;

/// One engine's running state: the input segments' spectra (newest at `current_segment`), the
/// block being filled, the overlap.
#[derive(Clone)]
struct ConvState {
    input_segments: Vec<Complex<f32>>,
    input_data: Vec<f32>,
    output_temp: Vec<Complex<f32>>,
    output_data: Vec<f32>,
    overlap: Vec<f32>,
    current_segment: usize,
    input_data_pos: usize,
    /// Segments of the response this state runs (`numSegments` = `numInputSegments`).
    num_segments: usize,
}

impl ConvState {
    fn new(max_segments: usize) -> Self {
        ConvState {
            input_segments: vec![Complex::new(0.0, 0.0); max_segments * BINS],
            input_data: vec![0.0; FFT_SIZE],
            output_temp: vec![Complex::new(0.0, 0.0); BINS],
            output_data: vec![0.0; FFT_SIZE],
            overlap: vec![0.0; FFT_SIZE],
            current_segment: 0,
            input_data_pos: 0,
            num_segments: 0,
        }
    }

    fn capacity(&self) -> usize {
        self.input_segments.len() / BINS
    }

    /// `reset()` for a response of `num_segments` segments (only the part in use is cleared).
    fn reset(&mut self, num_segments: usize) {
        let used = self.num_segments.max(num_segments).min(self.capacity());
        self.input_segments[..used * BINS].iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
        self.input_data.iter_mut().for_each(|x| *x = 0.0);
        self.output_temp.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
        self.output_data.iter_mut().for_each(|x| *x = 0.0);
        self.overlap.iter_mut().for_each(|x| *x = 0.0);
        self.current_segment = 0;
        self.input_data_pos = 0;
        self.num_segments = num_segments.min(self.capacity());
    }
}

/// The FFTs and their scratch, shared by both states.
struct Transforms {
    forward: Arc<dyn Fft<f32>>,
    inverse: Arc<dyn Fft<f32>>,
    buf: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
}

impl Transforms {
    fn new() -> Self {
        let mut planner = FftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(FFT_SIZE);
        let inverse = planner.plan_fft_inverse(FFT_SIZE);
        let len = forward.get_inplace_scratch_len().max(inverse.get_inplace_scratch_len());
        Transforms { forward, inverse, buf: vec![Complex::new(0.0, 0.0); FFT_SIZE], scratch: vec![Complex::new(0.0, 0.0); len] }
    }
}

/// `ConvolutionEngine::processSamples` (zero latency, uniform partitions), `n` samples of `input`
/// into `output`.
fn process_samples(st: &mut ConvState, ir: &ImpulseAsset, t: &mut Transforms, input: &[f32], output: &mut [f32]) {
    let num_segments = st.num_segments.max(1).min(ir.num_segments().max(1));
    let num_input_segments = num_segments;
    let index_step = 1; // numInputSegments / numSegments
    let n = input.len().min(output.len());
    let mut processed = 0usize;
    while processed < n {
        let input_data_was_empty = st.input_data_pos == 0;
        let k = (n - processed).min(CONV_BLOCK - st.input_data_pos);
        st.input_data[st.input_data_pos..st.input_data_pos + k].copy_from_slice(&input[processed..processed + k]);

        // this block's spectrum, into the current input segment
        for (c, &x) in t.buf.iter_mut().zip(&st.input_data) {
            *c = Complex::new(x, 0.0);
        }
        t.forward.process_with_scratch(&mut t.buf, &mut t.scratch);
        let cur = st.current_segment.min(num_input_segments - 1);
        st.input_segments[cur * BINS..(cur + 1) * BINS].copy_from_slice(&t.buf[..BINS]);

        // the older blocks against the response's later segments, once per block
        if input_data_was_empty {
            st.output_temp.iter_mut().for_each(|c| *c = Complex::new(0.0, 0.0));
            let mut index = cur;
            for i in 1..num_segments {
                index += index_step;
                if index >= num_input_segments {
                    index -= num_input_segments;
                }
                let x = &st.input_segments[index * BINS..(index + 1) * BINS];
                let h = ir.segment(i);
                for ((o, a), b) in st.output_temp.iter_mut().zip(x).zip(h) {
                    *o += a * b;
                }
            }
        }

        // this block against the first segment, plus the rest; back to time
        let h0 = ir.segment(0);
        for (j, slot) in t.buf.iter_mut().take(BINS).enumerate() {
            let x = st.input_segments[cur * BINS + j];
            let h = h0.get(j).copied().unwrap_or_default();
            *slot = st.output_temp[j] + x * h;
        }
        // the symmetric half, so the inverse transform is real
        for j in 1..FFT_SIZE / 2 {
            t.buf[FFT_SIZE - j] = t.buf[j].conj();
        }
        t.buf[0].im = 0.0;
        t.buf[FFT_SIZE / 2].im = 0.0;
        t.inverse.process_with_scratch(&mut t.buf, &mut t.scratch);
        let scale = 1.0 / FFT_SIZE as f32;
        for (o, c) in st.output_data.iter_mut().zip(&t.buf) {
            *o = c.re * scale;
        }

        // add the overlap
        let pos = st.input_data_pos;
        for i in 0..k {
            output[processed + i] = st.output_data[pos + i] + st.overlap[pos + i];
        }

        // input buffer full => next block
        st.input_data_pos += k;
        if st.input_data_pos == CONV_BLOCK {
            st.input_data.iter_mut().for_each(|x| *x = 0.0);
            st.input_data_pos = 0;
            // (fftSize - 2 blockSize is 0: nothing extra to add)
            let (head, tail) = st.output_data.split_at(CONV_BLOCK);
            st.overlap[..FFT_SIZE - CONV_BLOCK].copy_from_slice(&tail[..FFT_SIZE - CONV_BLOCK]);
            let _ = head;
            st.current_segment = if cur > 0 { cur - 1 } else { num_input_segments - 1 };
        }
        processed += k;
    }
}

/// `LinearSmoothedValue<float>` as the `CrossoverMixer` uses it.
#[derive(Debug, Clone, Copy)]
struct Smoother {
    current: f32,
    target: f32,
    step: f32,
    countdown: i32,
    steps_to_target: i32,
}

impl Smoother {
    fn reset(&mut self, sample_rate: f64, ramp_seconds: f64) {
        self.steps_to_target = (ramp_seconds * sample_rate).floor() as i32;
        self.set_current_and_target(self.target);
    }
    fn set_current_and_target(&mut self, v: f32) {
        self.target = v;
        self.current = v;
        self.countdown = 0;
    }
    fn set_target(&mut self, v: f32) {
        if v == self.target {
            return;
        }
        if self.steps_to_target <= 0 {
            self.set_current_and_target(v);
            return;
        }
        self.target = v;
        self.countdown = self.steps_to_target;
        self.step = (self.target - self.current) / self.countdown as f32;
    }
    fn is_smoothing(&self) -> bool {
        self.countdown > 0
    }
    fn next(&mut self) -> f32 {
        if !self.is_smoothing() {
            return self.target;
        }
        self.countdown -= 1;
        if self.is_smoothing() {
            self.current += self.step;
        } else {
            self.current = self.target;
        }
        self.current
    }
}

/// An engine in use: its response and which state runs it.
#[derive(Clone)]
struct Engine {
    ir: Arc<dyn crate::SampleSource>,
    state: usize,
}

/// The cabinet convolution: the engine in use, the previous one while the crossfade runs, and an
/// engine waiting for the crossfade to end (`Convolution::Impl` with its `CrossoverMixer`).
pub struct Convolver {
    states: [ConvState; 2],
    transforms: Transforms,
    current: Option<Engine>,
    previous: Option<Engine>,
    pending: Option<Arc<dyn crate::SampleSource>>,
    smoother: Smoother,
    smoother_buf: Vec<f32>,
    mix_buf: Vec<f32>,
    in_buf: Vec<f32>,
    /// Responses this no longer needs, for the host to drop off the audio thread.
    released: [Option<Arc<dyn crate::SampleSource>>; 8],
}

/// The usable asset behind a shared response (`None`: not one made by [`ImpulseAsset::new`]).
pub fn as_impulse(s: &Arc<dyn crate::SampleSource>) -> Option<&ImpulseAsset> {
    s.as_any()?.downcast_ref::<ImpulseAsset>()
}

impl Convolver {
    /// Sized for responses up to [`super::ir::MAX_IR_SECONDS`] at `rate` and host blocks of up to
    /// `max_block`. Allocates.
    pub fn new(rate: f64, max_block: usize) -> Self {
        let segs = max_segments(rate);
        let mut smoother = Smoother { current: 1.0, target: 1.0, step: 0.0, countdown: 0, steps_to_target: 0 };
        smoother.reset(rate, 0.05);
        smoother.set_current_and_target(1.0);
        let mb = max_block.max(1);
        Convolver {
            states: [ConvState::new(segs), ConvState::new(segs)],
            transforms: Transforms::new(),
            current: None,
            previous: None,
            pending: None,
            smoother,
            smoother_buf: vec![0.0; mb],
            mix_buf: vec![0.0; mb],
            in_buf: vec![0.0; mb],
            released: Default::default(),
        }
    }

    /// The most samples one `process` call takes.
    pub fn max_block(&self) -> usize {
        self.in_buf.len()
    }

    fn release(&mut self, ir: Arc<dyn crate::SampleSource>) {
        if let Some(slot) = self.released.iter_mut().find(|s| s.is_none()) {
            *slot = Some(ir);
        }
        // (all eight full means the host never collects them: the reference drops here)
    }

    /// One response the host may now drop (off the audio thread).
    pub fn take_released(&mut self) -> Option<Arc<dyn crate::SampleSource>> {
        self.released.iter_mut().find_map(Option::take)
    }

    /// The response in use (`None`: no engine).
    pub fn current(&self) -> Option<&Arc<dyn crate::SampleSource>> {
        self.current.as_ref().map(|e| &e.ir)
    }

    fn uses(&self, ir: &Arc<dyn crate::SampleSource>) -> bool {
        let same = |a: &Arc<dyn crate::SampleSource>| std::ptr::addr_eq(Arc::as_ptr(a), Arc::as_ptr(ir));
        self.pending.as_ref().is_some_and(same) || (self.pending.is_none() && self.current.as_ref().is_some_and(|e| same(&e.ir)))
    }

    /// Installs `ir` at once, with clean history and no crossfade (what `prepare` does with the
    /// latest response; also an amp change, and a first response).
    pub fn install_now(&mut self, ir: Option<Arc<dyn crate::SampleSource>>) {
        for e in [self.previous.take(), self.current.take()].into_iter().flatten() {
            self.release(e.ir);
        }
        if let Some(p) = self.pending.take() {
            self.release(p);
        }
        self.smoother.set_current_and_target(1.0);
        if let Some(ir) = ir {
            let segs = as_impulse(&ir).map_or(0, ImpulseAsset::num_segments);
            self.states[0].reset(segs);
            self.current = Some(Engine { ir, state: 0 });
        }
    }

    /// A new response for the engine in use: it takes over through the crossfade (`loadImpulse`
    /// then the next `process`). The same response again does nothing.
    pub fn load(&mut self, ir: Arc<dyn crate::SampleSource>) {
        if self.uses(&ir) {
            return;
        }
        if self.current.is_none() {
            self.install_now(Some(ir));
            return;
        }
        if let Some(old) = self.pending.replace(ir) {
            self.release(old);
        }
    }

    /// `prepare` on a live instance: the latest response, its history cleared, no crossfade.
    pub fn reset(&mut self) {
        let latest = self.pending.take().or_else(|| self.current.as_ref().map(|e| Arc::clone(&e.ir)));
        self.install_now(latest);
    }

    /// `installPendingEngine` when no crossfade is running.
    fn install_pending(&mut self) {
        let Some(ir) = self.pending.take() else { return };
        let state = self.current.as_ref().map_or(0, |e| 1 - e.state);
        let segs = as_impulse(&ir).map_or(0, ImpulseAsset::num_segments);
        self.states[state].reset(segs);
        self.previous = self.current.replace(Engine { ir, state });
        // CrossoverMixer::beginTransition
        self.smoother.set_current_and_target(1.0);
        self.smoother.set_target(0.0);
    }

    /// `Convolution::processSamples` in place on one channel, at most [`Convolver::max_block`]
    /// samples (the wrapper chunks).
    pub fn process(&mut self, io: &mut [f32]) {
        let n = io.len().min(self.in_buf.len());
        if n == 0 {
            return;
        }
        if self.previous.is_none() {
            self.install_pending();
        }
        let Some(cur) = self.current.clone() else { return };
        let Some(cur_ir) = as_impulse(&cur.ir) else { return };
        self.in_buf[..n].copy_from_slice(&io[..n]);
        if self.smoother.is_smoothing() {
            for s in self.smoother_buf[..n].iter_mut() {
                *s = self.smoother.next();
            }
            // previous (or the input itself) into the mix block, times the fade
            match self.previous.clone() {
                Some(prev) => match as_impulse(&prev.ir) {
                    Some(prev_ir) => {
                        let (input, mix) = (&self.in_buf[..n], &mut self.mix_buf[..n]);
                        process_samples(&mut self.states[prev.state], prev_ir, &mut self.transforms, input, mix);
                    }
                    None => self.mix_buf[..n].iter_mut().for_each(|x| *x = 0.0),
                },
                None => self.mix_buf[..n].copy_from_slice(&self.in_buf[..n]),
            }
            for (m, s) in self.mix_buf[..n].iter_mut().zip(&self.smoother_buf[..n]) {
                *m *= s;
            }
            for s in self.smoother_buf[..n].iter_mut() {
                *s = *s * -1.0 + 1.0;
            }
            process_samples(&mut self.states[cur.state], cur_ir, &mut self.transforms, &self.in_buf[..n], &mut io[..n]);
            for ((o, s), m) in io[..n].iter_mut().zip(&self.smoother_buf[..n]).zip(&self.mix_buf[..n]) {
                *o *= s;
                *o += m;
            }
            if !self.smoother.is_smoothing() {
                // destroyPreviousEngine (dropped off the audio thread by the host)
                if let Some(prev) = self.previous.take() {
                    self.release(prev.ir);
                }
            }
        } else {
            process_samples(&mut self.states[cur.state], cur_ir, &mut self.transforms, &self.in_buf[..n], &mut io[..n]);
        }
    }

    /// Whether a crossfade is running (tests).
    pub fn crossfading(&self) -> bool {
        self.smoother.is_smoothing()
    }

    /// The addresses of the state buffers (allocation tests).
    pub fn buffer_ptrs(&self) -> Vec<usize> {
        let mut v = Vec::new();
        for s in &self.states {
            v.extend([s.input_segments.as_ptr() as usize, s.input_data.as_ptr() as usize, s.overlap.as_ptr() as usize]);
        }
        v.extend([self.mix_buf.as_ptr() as usize, self.in_buf.as_ptr() as usize, self.transforms.buf.as_ptr() as usize]);
        v
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::SampleSource;

    fn asset(ir: &[f32]) -> Arc<dyn SampleSource> {
        Arc::new(ImpulseAsset::new(ir, 48_000.0, 48_000.0).unwrap())
    }

    /// The direct convolution with the asset's own (normalised) response.
    fn direct(x: &[f32], h: &[f32]) -> Vec<f32> {
        (0..x.len()).map(|n| (0..h.len().min(n + 1)).map(|k| f64::from(x[n - k]) * f64::from(h[k])).sum::<f64>() as f32).collect()
    }

    #[test]
    fn matches_direct_convolution_in_any_block_size() {
        let h: Vec<f32> = (0..1700).map(|i| ((i as f32) * 0.37).sin() * (-(i as f32) / 400.0).exp()).collect();
        let x: Vec<f32> = (0..5000).map(|i| ((i as f32) * 0.05).sin() * 0.5 + if i % 777 == 0 { 0.8 } else { 0.0 }).collect();
        let a = asset(&h);
        let want = direct(&x, as_impulse(&a).unwrap().channel(0).unwrap());
        for block in [1usize, 37, 512, 1024] {
            let mut c = Convolver::new(48_000.0, 1024);
            c.install_now(Some(Arc::clone(&a)));
            let mut got = x.clone();
            for chunk in got.chunks_mut(block) {
                c.process(chunk);
            }
            let err = got.iter().zip(&want).map(|(a, b)| (a - b).abs()).fold(0.0f32, f32::max);
            assert!(err < 1e-5, "block {block}: {err}");
        }
    }

    #[test]
    fn a_new_response_crossfades_over_fifty_ms_and_releases_the_old() {
        let mut c = Convolver::new(48_000.0, 512);
        let a = asset(&[1.0]);
        let b = asset(&[0.0, 1.0]);
        c.install_now(Some(Arc::clone(&a)));
        c.load(Arc::clone(&b));
        let mut io = vec![0.5f32; 512];
        c.process(&mut io);
        assert!(c.crossfading());
        for _ in 0..5 {
            c.process(&mut io);
        }
        assert!(!c.crossfading(), "2400 samples");
        let mut got = Vec::new();
        while let Some(r) = c.take_released() {
            got.push(r);
        }
        assert_eq!(got.len(), 1);
        assert!(std::ptr::addr_eq(Arc::as_ptr(&got[0]), Arc::as_ptr(&a)));
        assert!(std::ptr::addr_eq(Arc::as_ptr(c.current().unwrap()), Arc::as_ptr(&b)));
    }
}

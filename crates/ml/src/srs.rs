//! Smule Renaissance Small (vocal restoration: de-reverb, de-noise, de-clip), mono 48 kHz, STFT on
//! the host. Port of `tools/ml-export/srs/reference_infer.py`: a 60 Hz high-pass (RBJ biquad,
//! float32 coefficients, Direct Form I in float64, clamped to ±1), peak normalisation, STFT
//! 4096 / 2048 (periodic Hann, centred), the network over 256-frame chunks with 129 frames of
//! context on each side (its exact receptive field), iSTFT, and the peak gain back.

use crate::drumsep::card_usize;
use crate::job::Ctx;
use crate::models::InstalledModel;
use crate::runtime::{Device, OnnxModel, Tensor};
use crate::stft::Stft;
use crate::{MlError, Result};
use rustfft::num_complex::Complex32;

pub const ID: &str = "srs_small";

pub const SAMPLE_RATE: u32 = 48_000;

/// The 60 Hz high-pass of the reference (`torchaudio.functional.highpass_biquad`, `clamp=True`).
pub fn highpass60(x: &[f32]) -> Vec<f32> {
    let w0 = (2.0 * std::f64::consts::PI * 60.0 / f64::from(SAMPLE_RATE)) as f32;
    let q = 0.707f32;
    let alpha = w0.sin() / 2.0 / q;
    let c = w0.cos();
    let a0 = 1.0 + alpha;
    let b = [(1.0 + c) / 2.0 / a0, (-1.0 - c) / a0, (1.0 + c) / 2.0 / a0].map(f64::from);
    let a1 = f64::from(-2.0 * c / a0);
    let a2 = f64::from((1.0 - alpha) / a0);
    let (mut x1, mut x2, mut y1, mut y2) = (0.0f64, 0.0f64, 0.0f64, 0.0f64);
    x.iter()
        .map(|&v| {
            let xn = f64::from(v);
            let yn = b[0] * xn + b[1] * x1 + b[2] * x2 - a1 * y1 - a2 * y2;
            (x2, x1, y2, y1) = (x1, xn, y1, yn);
            yn.clamp(-1.0, 1.0) as f32
        })
        .collect()
}

/// Restore `audio` (`[channel][frames]` at 48 kHz). The model is mono: each channel of a stereo
/// source runs through it on its own (one peak gain for both, so the balance holds), so the image
/// is kept. Returns `("restored", [channel][frames])` and the device.
pub fn run(model: &InstalledModel, audio: &[Vec<f32>], device: Device, ctx: &Ctx) -> Result<(Vec<(String, Vec<Vec<f32>>)>, String)> {
    let card = &model.card;
    let n_fft = card_usize(card, "n_fft", 4096);
    let hop = card_usize(card, "hop", 2048);
    let core = card_usize(card, "core", 256).max(1);
    let context = card_usize(card, "context", 129);
    if n_fft < 2 || hop == 0 {
        return Err(MlError::Model(ID.into(), "bad STFT settings on the card".into()));
    }
    let len = crate::frames(audio);
    let channels: Vec<Vec<f32>> = crate::to_channels(audio, audio.len().clamp(1, 2));
    ctx.check()?;
    let filtered: Vec<Vec<f32>> = channels.iter().map(|c| highpass60(c)).collect();
    drop(channels);
    let g = filtered.iter().flatten().fold(0.0f32, |m, v| m.max(v.abs()));
    ctx.say("Loading Renaissance (de-reverb)");
    let mut onnx = OnnxModel::open_model(model, device, ctx.cancel_flag())?;
    ctx.say(&format!("De-reverbing on the {}", onnx.device_label()));
    let stft = Stft::new(n_fft, hop);
    let n_ch = filtered.len().max(1);
    let mut restored = Vec::with_capacity(n_ch);
    for (c, x) in filtered.into_iter().enumerate() {
        let cctx = ctx.stage(c as f32 / n_ch as f32, (c + 1) as f32 / n_ch as f32);
        let x: Vec<f32> = if g > 0.0 { x.iter().map(|v| v / g).collect() } else { x };
        let y = restore_one(&mut onnx, &stft, &x, core, context, &cctx)?;
        restored.push(if g > 0.0 { y.iter().map(|v| v * g).collect() } else { y });
    }
    let note = onnx.fallback_note.clone().unwrap_or_else(|| onnx.device_label().to_string());
    let restored = crate::fit(restored, len);
    Ok((vec![("restored".into(), restored)], note))
}

/// One normalised channel through the network, in frame chunks with context.
fn restore_one(onnx: &mut OnnxModel, stft: &Stft, x: &[f32], core: usize, context: usize, ctx: &Ctx) -> Result<Vec<f32>> {
    let len = x.len();
    let spec = stft.forward(x);
    let (bins, frames) = (stft.bins(), stft.frames(len));
    let mut out = vec![vec![Complex32::default(); frames]; bins];
    let starts: Vec<usize> = (0..frames).step_by(core).collect();
    for (i, &c0) in starts.iter().enumerate() {
        ctx.check()?;
        let (a, b) = (c0.saturating_sub(context), (c0 + core + context).min(frames));
        let n = b - a;
        // [1, F, n, (re, im)]
        let mut input = vec![0.0f32; bins * n * 2];
        for (f, row) in spec.iter().enumerate() {
            for t in 0..n {
                let v = row.get(a + t).copied().unwrap_or_default();
                if let Some(s) = input.get_mut((f * n + t) * 2..(f * n + t) * 2 + 2) {
                    s.copy_from_slice(&[v.re, v.im]);
                }
            }
        }
        let res = onnx.run(vec![Tensor::new(vec![1, bins as i64, n as i64, 2], input)])?;
        let Some(y) = res.into_iter().next() else { return Err(MlError::Model(ID.into(), "no output".into())) };
        if y.data.len() != bins * n * 2 {
            return Err(MlError::Model(ID.into(), format!("output shape {:?}", y.shape)));
        }
        let e = (c0 + core).min(frames);
        for (f, row) in out.iter_mut().enumerate() {
            for t in c0..e {
                let k = (f * n + (t - a)) * 2;
                if let Some(cell) = row.get_mut(t) {
                    *cell = Complex32::new(y.data.get(k).copied().unwrap_or(0.0), y.data.get(k + 1).copied().unwrap_or(0.0));
                }
            }
        }
        ctx.report((i + 1) as f32 / starts.len().max(1) as f32);
    }
    Ok(stft.inverse(&out, len))
}
